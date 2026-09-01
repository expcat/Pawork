//! DiffService：调用系统 git 获取 diff 并解析为结构化 [`DiffFile`]，支持分页。
//!
//! - [`DiffService::diff_summary`]：仅文件清单（`--raw -z` + `--numstat -z`），轻量入口。
//! - [`DiffService::diff`]：在 summary 基础上，对每个非 binary 文件再跑
//!   `git diff -U<n> -- <path>` 拿 unified patch，经 [`super::parser`] 解析为 hunks。
//!
//! 大 diff 通过 [`paginate`] 分页浏览；HunkId 全局自增。

use std::path::{Path, PathBuf};

use crate::{validate_position_arg, GitError, GitRunner};
use pawork_domain::CancellationToken;

use super::model::{DiffFile, FileStatus};
use super::parser::parse_unified_with_start;

/// diff 选项。
#[derive(Clone, Debug)]
pub struct DiffOptions {
    /// `true`→对比 index（`--cached`）；`false`→对比 worktree。
    pub staged: bool,
    /// `-U<n>` 上下文行数，默认 3。
    pub context: u32,
    /// 是否加 `-M`（rename detection），默认 true。
    pub detect_renames: bool,
    /// 指定 commit range（如 `HEAD~1..HEAD`）；设了则按该 range diff。
    pub commit_range: Option<String>,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            staged: false,
            context: 3,
            detect_renames: true,
            commit_range: None,
        }
    }
}

/// 分页结果。
#[derive(Clone, Debug)]
pub struct DiffPage {
    pub files: Vec<DiffFile>,
    pub total_files: usize,
    pub page: usize,
    pub page_size: usize,
}

/// 对 diff 文件清单分页。`page` 从 1 起；`page_size == 0` 返回全部。
pub fn paginate(files: Vec<DiffFile>, page: usize, page_size: usize) -> DiffPage {
    let total = files.len();
    if page_size == 0 {
        return DiffPage {
            files,
            total_files: total,
            page: 1,
            page_size: 0,
        };
    }
    let page = page.max(1);
    let start = (page - 1) * page_size;
    let slice: Vec<DiffFile> = if start >= total {
        Vec::new()
    } else {
        let end = (start + page_size).min(total);
        files[start..end].to_vec()
    };
    DiffPage {
        files: slice,
        total_files: total,
        page,
        page_size,
    }
}

/// 结构化 Diff 服务。
pub struct DiffService {
    git: GitRunner,
    work_dir: PathBuf,
}

impl DiffService {
    pub fn new(git: GitRunner, work_dir: &Path) -> Self {
        Self {
            git,
            work_dir: work_dir.to_path_buf(),
        }
    }

    /// 仅文件清单（不含 hunks，但 binary 标记与 add/del 行数已填）——大 diff 的轻量入口。
    pub async fn diff_summary(
        &self,
        opts: &DiffOptions,
        cancel: CancellationToken,
    ) -> Result<Vec<DiffFile>, GitError> {
        let raw = self.run_raw(opts, cancel.clone()).await?;
        let numstat = self.run_numstat(opts, cancel.clone()).await?;

        let mut files = parse_raw(&raw, opts.staged);
        merge_numstat(&mut files, &numstat);
        // `git diff --raw` 不含未跟踪文件；工作区视角补齐，status=Untracked。
        if !opts.staged {
            self.append_untracked(&mut files, cancel).await?;
        }
        Ok(files)
    }

    /// 完整 diff：文件清单 + 全部 hunks。
    pub async fn diff(
        &self,
        opts: &DiffOptions,
        cancel: CancellationToken,
    ) -> Result<Vec<DiffFile>, GitError> {
        let mut files = self.diff_summary(opts, cancel.clone()).await?;
        let mut next_id: u64 = 0;
        for file in &mut files {
            // binary / gitlink / untracked：不跑普通文本 hunk，避免把 submodule
            // 工作树当文件内容解析。
            if file.binary || file.status == FileStatus::Untracked {
                continue;
            }
            let patch = self
                .run_file_patch(opts, &file.path, cancel.clone())
                .await?;
            let (hunks, nid) = parse_unified_with_start(&patch, next_id);
            file.hunks = hunks;
            next_id = nid;
        }
        Ok(files)
    }

    /// `git diff [<opts>] --raw -z`：结构化文件清单。
    async fn run_raw(
        &self,
        opts: &DiffOptions,
        cancel: CancellationToken,
    ) -> Result<String, GitError> {
        let args = self.base_args(opts)?;
        let mut full: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        full.extend_from_slice(&["--raw", "-z"]);
        self.git.run(&self.work_dir, &full, cancel).await
    }

    /// `git diff [<opts>] --numstat -z`：行数统计。
    async fn run_numstat(
        &self,
        opts: &DiffOptions,
        cancel: CancellationToken,
    ) -> Result<String, GitError> {
        let args = self.base_args(opts)?;
        let mut full: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        full.extend_from_slice(&["--numstat", "-z"]);
        self.git.run(&self.work_dir, &full, cancel).await
    }

    /// `git diff [<opts>] -U<n> -- <path>`：单文件的 unified patch。
    async fn run_file_patch(
        &self,
        opts: &DiffOptions,
        path: &str,
        cancel: CancellationToken,
    ) -> Result<String, GitError> {
        let args = self.base_args(opts)?;
        let mut full: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let context = format!("-U{}", opts.context);
        full.extend_from_slice(&["--no-color", &context, "--", path]);
        self.git.run(&self.work_dir, &full, cancel).await
    }

    /// 构造公共 diff 参数前缀（`diff`、可选 `--cached`、可选 range、可选 `-M`）。
    fn base_args(&self, opts: &DiffOptions) -> Result<Vec<String>, GitError> {
        let mut args: Vec<String> = vec!["diff".into()];
        if let Some(range) = &opts.commit_range {
            validate_position_arg("commit_range", range)?;
            args.push(range.clone());
        }
        if opts.staged {
            args.push("--cached".into());
        }
        if opts.detect_renames {
            args.push("-M".into());
        }
        Ok(args)
    }

    /// `git ls-files --others --exclude-standard -z`：未跟踪且未被 ignore 的路径。
    async fn append_untracked(
        &self,
        files: &mut Vec<DiffFile>,
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        let stdout = self
            .git
            .run(
                &self.work_dir,
                &["ls-files", "--others", "--exclude-standard", "-z"],
                cancel,
            )
            .await?;
        for path in stdout.split('\0') {
            if path.is_empty() || files.iter().any(|f| f.path == path) {
                continue;
            }
            files.push(DiffFile {
                path: path.to_string(),
                previous_path: None,
                status: FileStatus::Untracked,
                staged: false,
                binary: false,
                additions: 0,
                deletions: 0,
                hunks: Vec::new(),
            });
        }
        Ok(())
    }
}

/// 解析 `git diff --raw -z` 输出为 [`DiffFile`] 列表（不含行数/hunks）。
///
/// 每条形如 `:<oldmode> <newmode> <oldsha> <newsha> <STATUS>\0<path>\0[<origpath>\0]`。
/// STATUS 单字母：M/A/D/T/U；R/C 带相似度（如 R100），后续多一个 origpath 段。
fn parse_raw(raw: &str, staged: bool) -> Vec<DiffFile> {
    let tokens: Vec<&str> = raw.split('\0').collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let header = tokens[i];
        if header.is_empty() {
            i += 1;
            continue;
        }
        // header 形如 ":100644 100644 <sha> <sha> M"；gitlink 为 160000。
        let status = raw_status(header);
        let gitlink = is_gitlink(header);
        let i_path = i + 1;
        if i_path >= tokens.len() {
            break;
        }
        let path = tokens[i_path];
        let mut previous_path = None;
        // rename/copy 时 `--raw` 输出为 "<status>\0<origpath>\0<newpath>"：
        // 第一段是原始路径（previous），第二段是新路径（path）。
        if matches!(status, FileStatus::Renamed | FileStatus::Copied) {
            let i_orig = i_path + 1;
            if i_orig < tokens.len() && !tokens[i_orig].is_empty() {
                previous_path = Some(path.to_string());
                // 新路径为第二段。
                let new_path = if i_orig < tokens.len() {
                    tokens[i_orig].to_string()
                } else {
                    path.to_string()
                };
                i = i_orig + 1;
                files.push(DiffFile {
                    path: new_path,
                    previous_path,
                    status,
                    staged,
                    binary: gitlink,
                    additions: 0,
                    deletions: 0,
                    hunks: Vec::new(),
                });
                continue;
            } else {
                i = i_orig;
            }
        } else {
            i = i_path + 1;
        }
        if path.is_empty() {
            continue;
        }
        files.push(DiffFile {
            path: path.to_string(),
            previous_path,
            status,
            staged,
            binary: gitlink,
            additions: 0,
            deletions: 0,
            hunks: Vec::new(),
        });
    }
    files
}

/// 从 raw header 提取状态码。
fn raw_status(header: &str) -> FileStatus {
    // 取最后一个非空白字段的首字母。
    let last_field = header.split_whitespace().last().unwrap_or("");
    match last_field.chars().next().unwrap_or(' ') {
        'A' => FileStatus::Added,
        'D' => FileStatus::Deleted,
        'R' => FileStatus::Renamed,
        'C' => FileStatus::Copied,
        'T' => FileStatus::TypeChanged,
        'U' => FileStatus::Unmerged,
        _ => FileStatus::Modified,
    }
}

/// raw header 的 old/new mode 任一为 `160000` 即 gitlink（submodule）。
fn is_gitlink(header: &str) -> bool {
    header
        .split_whitespace()
        .take(2)
        .any(|mode| mode.trim_start_matches(':') == "160000")
}

/// 用 numstat 输出填充每个文件的 additions/deletions 与 binary 标记。
fn merge_numstat(files: &mut [DiffFile], numstat: &str) {
    use std::collections::HashMap;
    // numstat -z 的真实布局是逐行、行尾 NUL；按 \n 分割后逐行处理。
    let mut stats: HashMap<String, (u32, u32, bool)> = HashMap::new();
    for raw in numstat.split('\n') {
        let raw = raw.trim_end_matches('\0');
        if raw.is_empty() {
            continue;
        }
        let mut parts = raw.splitn(3, '\t');
        let added = parts.next().unwrap_or("0");
        let del = parts.next().unwrap_or("0");
        let path = parts.next().unwrap_or("");
        let binary = added == "-" && del == "-";
        let a: u32 = added.parse().unwrap_or(0);
        let d: u32 = del.parse().unwrap_or(0);
        stats.insert(path.to_string(), (a, d, binary));
    }
    for f in files.iter_mut() {
        if let Some((a, d, bin)) = stats.get(&f.path) {
            f.additions = *a;
            f.deletions = *d;
            f.binary = f.binary || *bin;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GitRunner;
    use std::process::Command;

    fn git_env() -> Vec<(&'static str, &'static str)> {
        vec![
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
            ("GIT_TERMINAL_PROMPT", "0"),
        ]
    }

    fn run_git(cwd: &Path, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        cmd.current_dir(cwd).args(args);
        for (k, v) in git_env() {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("git exec");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn make_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tmpdir");
        let repo = dir.path().to_path_buf();
        run_git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("f.txt"), "a\nb\nc\n").expect("write");
        run_git(&repo, &["add", "f.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        (dir, repo)
    }

    /// 以 HEAD 为基准的 diff 选项：同时覆盖 index 与 worktree 的变更。
    fn opts_vs_head() -> DiffOptions {
        DiffOptions {
            commit_range: Some("HEAD".into()),
            ..DiffOptions::default()
        }
    }

    #[tokio::test]
    async fn diff_modified_file_has_hunks_and_counts() {
        let (_dir, repo) = make_repo();
        std::fs::write(repo.join("f.txt"), "a\nB\nc\nd\n").expect("write");
        let svc = DiffService::new(GitRunner::new(), &repo);
        let files = svc
            .diff(&opts_vs_head(), CancellationToken::new())
            .await
            .expect("diff");
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.status, FileStatus::Modified);
        assert_eq!(f.additions, 2);
        assert_eq!(f.deletions, 1);
        assert!(!f.hunks.is_empty());
        // 验证 hunk 内出现新增 +B / +d 与删除 -b。
        let all_text: Vec<&str> = f
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.text.as_str()))
            .collect();
        assert!(all_text.contains(&"B"));
        assert!(all_text.contains(&"d"));
        assert!(all_text.contains(&"b"));
    }

    #[tokio::test]
    async fn diff_summary_add_delete() {
        let (_dir, repo) = make_repo();
        std::fs::write(repo.join("new.txt"), "n\n").expect("write");
        run_git(&repo, &["add", "new.txt"]);
        // f.txt 删除（git rm 让删除进入 index，从而对 HEAD 可见）。
        run_git(&repo, &["rm", "-q", "--cached", "f.txt"]);
        std::fs::remove_file(repo.join("f.txt")).ok();
        let svc = DiffService::new(GitRunner::new(), &repo);
        let files = svc
            .diff_summary(&opts_vs_head(), CancellationToken::new())
            .await
            .expect("summary");
        let statuses: Vec<FileStatus> = files.iter().map(|f| f.status).collect();
        assert!(statuses.contains(&FileStatus::Added), "{statuses:?}");
        assert!(statuses.contains(&FileStatus::Deleted), "{statuses:?}");
    }

    #[tokio::test]
    async fn diff_rename_sets_previous_path() {
        let (_dir, repo) = make_repo();
        run_git(&repo, &["mv", "f.txt", "g.txt"]);
        let svc = DiffService::new(GitRunner::new(), &repo);
        let files = svc
            .diff(&opts_vs_head(), CancellationToken::new())
            .await
            .expect("diff");
        let ren = files
            .iter()
            .find(|f| f.status == FileStatus::Renamed)
            .expect("a rename entry");
        assert_eq!(ren.path, "g.txt");
        assert_eq!(ren.previous_path.as_deref(), Some("f.txt"));
    }

    #[tokio::test]
    async fn diff_binary_file_marked_no_hunks() {
        let (_dir, repo) = make_repo();
        std::fs::write(repo.join("bin.dat"), vec![0u8, 1, 2, 0, 0, 255]).expect("write");
        run_git(&repo, &["add", "bin.dat"]);
        let svc = DiffService::new(GitRunner::new(), &repo);
        let files = svc
            .diff(&opts_vs_head(), CancellationToken::new())
            .await
            .expect("diff");
        let bin = files
            .iter()
            .find(|f| f.path == "bin.dat")
            .expect("bin file");
        assert!(bin.binary, "should be binary");
        assert!(bin.hunks.is_empty(), "binary should have no hunks");
    }

    #[tokio::test]
    async fn diff_no_newline_at_end() {
        let (_dir, repo) = make_repo();
        // 写一个无末尾换行的修改。
        std::fs::write(repo.join("f.txt"), "a\nb\nxyz").expect("write");
        let svc = DiffService::new(GitRunner::new(), &repo);
        let files = svc
            .diff(&opts_vs_head(), CancellationToken::new())
            .await
            .expect("diff");
        let f = &files[0];
        let any_nonl = f
            .hunks
            .iter()
            .any(|h| h.lines.iter().any(|l| l.new_no_newline));
        assert!(any_nonl, "expected a no-newline line, got {f:?}");
    }

    #[test]
    fn paginate_basic() {
        let mk = |p: &str| DiffFile {
            path: p.into(),
            previous_path: None,
            status: FileStatus::Modified,
            staged: false,
            binary: false,
            additions: 1,
            deletions: 1,
            hunks: Vec::new(),
        };
        let files: Vec<DiffFile> = vec![mk("a"), mk("b"), mk("c")];
        let page = paginate(files, 1, 2);
        assert_eq!(page.files.len(), 2);
        assert_eq!(page.total_files, 3);
        assert_eq!(page.page, 1);
        let page0 = paginate(vec![mk("a"), mk("b"), mk("c")], 1, 0);
        assert_eq!(page0.files.len(), 3);
    }

    #[tokio::test]
    async fn option_like_commit_range_is_rejected() {
        let svc = DiffService::new(GitRunner::new(), Path::new("."));
        let opts = DiffOptions {
            commit_range: Some("--output=stolen.patch".into()),
            ..Default::default()
        };
        let error = svc
            .diff_summary(&opts, CancellationToken::new())
            .await
            .expect_err("option-like commit range must be rejected");
        assert!(matches!(
            error,
            GitError::InvalidPositionArgument {
                name: "commit_range",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn untracked_file_appears_with_untracked_status() {
        let (_dir, repo) = make_repo();
        run_git(&repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("fresh.txt"), "brand new\n").expect("write untracked");
        let svc = DiffService::new(GitRunner::new(), &repo);
        let files = svc
            .diff(&DiffOptions::default(), CancellationToken::new())
            .await
            .expect("diff");
        let fresh = files
            .iter()
            .find(|f| f.path == "fresh.txt")
            .expect("untracked file must appear");
        assert_eq!(fresh.status, FileStatus::Untracked);
        assert!(
            fresh.hunks.is_empty(),
            "untracked is listed, not text-hunked"
        );
    }

    #[tokio::test]
    async fn submodule_gitlink_is_parsed_without_text_hunks() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let child = dir.path().join("child");
        let parent = dir.path().join("parent");
        std::fs::create_dir_all(&child).expect("mkdir child");
        std::fs::create_dir_all(&parent).expect("mkdir parent");

        run_git(&child, &["init", "-q"]);
        run_git(&child, &["config", "core.autocrlf", "false"]);
        std::fs::write(child.join("in-sub.txt"), "inside submodule\n").expect("write child");
        run_git(&child, &["add", "in-sub.txt"]);
        run_git(&child, &["commit", "-q", "-m", "sub init"]);
        let sha = run_git(&child, &["rev-parse", "HEAD"]).trim().to_string();

        run_git(&parent, &["init", "-q"]);
        run_git(&parent, &["config", "core.autocrlf", "false"]);
        std::fs::write(parent.join("f.txt"), "a\n").expect("write parent");
        run_git(&parent, &["add", "f.txt"]);
        run_git(&parent, &["commit", "-q", "-m", "init"]);
        let cacheinfo = format!("160000,{sha},sub");
        run_git(
            &parent,
            &["update-index", "--add", "--cacheinfo", &cacheinfo],
        );

        let svc = DiffService::new(GitRunner::new(), &parent);
        let opts = DiffOptions {
            staged: true,
            ..DiffOptions::default()
        };
        let files = svc
            .diff(&opts, CancellationToken::new())
            .await
            .expect("submodule diff must parse");
        let sub = files
            .iter()
            .find(|f| f.path == "sub")
            .expect("submodule entry");
        assert!(
            sub.hunks.is_empty(),
            "gitlink must not be exploded as a text hunk: {sub:?}"
        );
        assert!(
            sub.binary || sub.status == FileStatus::Added,
            "gitlink should be recorded as an entry: {sub:?}"
        );
    }

    #[tokio::test]
    async fn crlf_file_diff_is_correct_with_autocrlf_false() {
        let (_dir, repo) = make_repo();
        run_git(&repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("crlf.txt"), b"hello\r\nworld\r\n").expect("write crlf");
        run_git(&repo, &["add", "crlf.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "crlf base"]);
        std::fs::write(repo.join("crlf.txt"), b"hello\r\nWORLD\r\n").expect("write crlf edit");

        let svc = DiffService::new(GitRunner::new(), &repo);
        let files = svc
            .diff(&opts_vs_head(), CancellationToken::new())
            .await
            .expect("crlf diff");
        let f = files
            .iter()
            .find(|f| f.path == "crlf.txt")
            .expect("crlf.txt");
        let texts: Vec<String> = f
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.text.replace('\r', "")))
            .collect();
        assert!(
            texts.iter().any(|t| t == "WORLD"),
            "expected WORLD in hunks: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "world"),
            "expected world in hunks: {texts:?}"
        );
    }

    #[tokio::test]
    async fn unicode_chinese_filename_is_preserved() {
        let (_dir, repo) = make_repo();
        run_git(&repo, &["config", "core.autocrlf", "false"]);
        let name = "中文文件.txt";
        std::fs::write(repo.join(name), "你好\n").expect("write unicode name");
        run_git(&repo, &["add", "--", name]);
        run_git(&repo, &["commit", "-q", "-m", "unicode"]);
        std::fs::write(repo.join(name), "你好世界\n").expect("edit unicode name");

        let svc = DiffService::new(GitRunner::new(), &repo);
        let files = svc
            .diff(&opts_vs_head(), CancellationToken::new())
            .await
            .expect("unicode diff");
        assert!(
            files.iter().any(|f| f.path == name),
            "expected {name} in {:?}",
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>()
        );
    }
}
