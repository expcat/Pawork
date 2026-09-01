//! Hunk / Line 级暂存（P7-7）。
//!
//! 基于结构化 [`super::model::DiffHunk`] / [`super::model::DiffLine`] 构造
//! 最小 unified patch，经 [`crate::StageService::apply_patch_to_index`]
//! 用 `git apply --cached [--reverse]` 应用到 index，实现按块 / 按行暂存与
//! 取消暂存，不触碰工作区。
//!
//! 语义约定：
//! - stage_*：`file` 应来自 **worktree vs index** 的 diff
//!   （[`super::service::DiffOptions::default`]）。
//! - unstage_*：`file` 应来自 **staged** diff（`DiffOptions { staged: true, .. }`）。
//! - 行选择 `selection` 与 `hunk.lines` 逐行对齐：`Context` 行恒保留；
//!   未选中的 `Addition` 留在工作区不进 index；未选中的 `Deletion` 转为
//!   context（即不暂存该删除）。
//! - rename / copy / typechange / unmerged / binary 不支持块行暂存，返回
//!   [`GitError::Other`]，上层应回退整文件 stage。

use std::path::{Path, PathBuf};

use crate::{GitError, GitRunner, StageService};
use pawork_domain::CancellationToken;

use super::model::{DiffFile, DiffHunk, DiffLine, FileStatus, HunkId, LineKind};

/// Hunk / Line 暂存服务。
pub struct HunkStageService {
    git: GitRunner,
    work_dir: PathBuf,
}

impl HunkStageService {
    pub fn new(git: GitRunner, work_dir: &Path) -> Self {
        Self {
            git,
            work_dir: work_dir.to_path_buf(),
        }
    }

    /// 暂存选中的 hunk（worktree → index）。
    ///
    /// `hunk_ids` 为空或无匹配时为无操作。
    pub async fn stage_hunks(
        &self,
        file: &DiffFile,
        hunk_ids: &[HunkId],
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        self.apply_hunks(file, hunk_ids, false, cancel).await
    }

    /// 取消暂存选中的 hunk（index → worktree 方向，`--reverse` 应用）。
    pub async fn unstage_hunks(
        &self,
        file: &DiffFile,
        hunk_ids: &[HunkId],
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        self.apply_hunks(file, hunk_ids, true, cancel).await
    }

    /// 按行暂存：只暂存 `selection` 标记为 true 的增删行。
    ///
    /// 选择后无可暂存内容时为无操作。
    pub async fn stage_lines(
        &self,
        file: &DiffFile,
        hunk_id: HunkId,
        selection: &[bool],
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        self.apply_lines(file, hunk_id, selection, false, cancel)
            .await
    }

    /// 按行取消暂存（针对 staged diff 的 hunk）。
    pub async fn unstage_lines(
        &self,
        file: &DiffFile,
        hunk_id: HunkId,
        selection: &[bool],
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        self.apply_lines(file, hunk_id, selection, true, cancel)
            .await
    }

    async fn apply_hunks(
        &self,
        file: &DiffFile,
        hunk_ids: &[HunkId],
        reverse: bool,
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        check_supported(file)?;
        let hunks: Vec<&DiffHunk> = file
            .hunks
            .iter()
            .filter(|h| hunk_ids.contains(&h.id))
            .collect();
        if hunks.is_empty() {
            return Ok(());
        }
        let patch = build_hunk_patch(file, &hunks);
        StageService::new(&self.git, &self.work_dir)
            .apply_patch_to_index(&patch, reverse, cancel)
            .await
    }

    async fn apply_lines(
        &self,
        file: &DiffFile,
        hunk_id: HunkId,
        selection: &[bool],
        reverse: bool,
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        check_supported(file)?;
        let hunk = file
            .hunks
            .iter()
            .find(|h| h.id == hunk_id)
            .ok_or_else(|| GitError::Other(format!("hunk {hunk_id:?} not found in diff")))?;
        let patch = match build_line_patch(file, hunk, selection) {
            Some(p) => p,
            None => return Ok(()), // 选择后无可暂存内容。
        };
        StageService::new(&self.git, &self.work_dir)
            .apply_patch_to_index(&patch, reverse, cancel)
            .await
    }
}

/// 校验文件是否支持 hunk / line 暂存。
fn check_supported(file: &DiffFile) -> Result<(), GitError> {
    if file.binary {
        return Err(GitError::Other(format!(
            "binary file {:?} cannot be hunk-staged; stage the whole file",
            file.path
        )));
    }
    match file.status {
        FileStatus::Renamed
        | FileStatus::Copied
        | FileStatus::TypeChanged
        | FileStatus::Unmerged
        | FileStatus::Untracked => Err(GitError::Other(format!(
            "file {:?} with status {:?} cannot be hunk-staged; stage the whole file",
            file.path, file.status
        ))),
        _ => Ok(()),
    }
}

/// 构造 hunk 级 patch：原样复用 hunk 头与行内容（含无末尾换行标记）。
pub fn build_hunk_patch(file: &DiffFile, hunks: &[&DiffHunk]) -> String {
    let (minus, plus) = file_header(file);
    let mut out = format!(
        "diff --git a/{p} b/{p}\n--- {minus}\n+++ {plus}\n",
        p = file.path
    );
    for hunk in hunks {
        out.push_str(&hunk.header);
        out.push('\n');
        for line in &hunk.lines {
            emit_line(&mut out, line);
        }
    }
    out
}

/// 构造行级 patch：按 `selection` 重组单个 hunk。
///
/// - `Context`：恒保留；
/// - 选中 `Addition`：保留为 `+`，未选中则丢弃（留在工作区）；
/// - 选中 `Deletion`：保留为 `-`，未选中转为 context（不暂存删除）。
///
/// 重新计算 hunk 头计数；选择后没有任何增删行时返回 `None`。
pub fn build_line_patch(file: &DiffFile, hunk: &DiffHunk, selection: &[bool]) -> Option<String> {
    let mut body = String::new();
    let (mut old_lines, mut new_lines) = (0u32, 0u32);
    let mut changed = false;
    for (i, line) in hunk.lines.iter().enumerate() {
        let selected = selection.get(i).copied().unwrap_or(false);
        match (line.kind, selected) {
            (LineKind::Context, _) => {
                push_line(&mut body, ' ', line);
                old_lines += 1;
                new_lines += 1;
            }
            (LineKind::Addition, true) => {
                push_line(&mut body, '+', line);
                new_lines += 1;
                changed = true;
            }
            (LineKind::Addition, false) => {} // 不进 index，留在工作区。
            (LineKind::Deletion, true) => {
                push_line(&mut body, '-', line);
                old_lines += 1;
                changed = true;
            }
            (LineKind::Deletion, false) => {
                // 转为 context：index 保留该行。
                push_line(&mut body, ' ', line);
                old_lines += 1;
                new_lines += 1;
            }
        }
    }
    if !changed {
        return None;
    }
    let (minus, plus) = file_header(file);
    let header = format!(
        "@@ -{},{} +{},{} @@",
        hunk.old_start, old_lines, hunk.new_start, new_lines
    );
    Some(format!(
        "diff --git a/{p} b/{p}\n--- {minus}\n+++ {plus}\n{header}\n{body}",
        p = file.path
    ))
}

/// `--- / +++ ` 路径头：新增文件 `--- /dev/null`，删除文件 `+++ /dev/null`。
fn file_header(file: &DiffFile) -> (String, String) {
    let minus = if file.status == FileStatus::Added {
        "/dev/null".to_string()
    } else {
        format!("a/{}", file.path)
    };
    let plus = if file.status == FileStatus::Deleted {
        "/dev/null".to_string()
    } else {
        format!("b/{}", file.path)
    };
    (minus, plus)
}

/// 按原行类型输出一行（hunk 级原样复用）。
fn emit_line(out: &mut String, line: &DiffLine) {
    let prefix = match line.kind {
        LineKind::Context => ' ',
        LineKind::Addition => '+',
        LineKind::Deletion => '-',
    };
    push_line(out, prefix, line);
}

/// 输出 `<prefix><text>\n`，并按**输出前缀**选择正确的无末尾换行标志：
/// `-`（删除）看旧侧、`+`（新增）看新侧、` `（context）看两侧
/// （任一侧无换行即标记）。修复此前对所有前缀都看 `new_no_newline` 导致
/// 删除行/被转为 context 的旧行漏标旧侧 `\ No newline` 的语义缺陷。
fn push_line(out: &mut String, prefix: char, line: &DiffLine) {
    out.push(prefix);
    out.push_str(&line.text);
    out.push('\n');
    let no_newline = match prefix {
        '-' => line.old_no_newline,
        '+' => line.new_no_newline,
        _ => line.old_no_newline || line.new_no_newline,
    };
    if no_newline {
        out.push_str("\\ No newline at end of file\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiffOptions, DiffService};
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
        run_git(&repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("f.txt"), "line1\n").expect("write");
        run_git(&repo, &["add", "f.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        (dir, repo)
    }

    /// index 中文件的当前内容（`git show :<path>`）。
    fn index_content(cwd: &Path, path: &str) -> String {
        run_git(cwd, &["show", &format!(":{path}")])
    }

    /// 制造两个相距较远的改动区域（两个 hunk）。
    fn write_two_hunk_file(repo: &Path) {
        let mut lines: Vec<String> = (1..=20).map(|i| format!("L{i}")).collect();
        lines[1] = "M2".into();
        lines[17] = "M18".into();
        let content = lines.join("\n") + "\n";
        std::fs::write(repo.join("f.txt"), content).expect("write");
    }

    #[tokio::test]
    async fn stage_hunks_stages_only_selected_hunk() {
        let (_dir, repo) = make_repo();
        // 提交 20 行版本作为基线。
        let baseline: String = (1..=20).map(|i| format!("L{i}\n")).collect();
        std::fs::write(repo.join("f.txt"), &baseline).expect("write");
        run_git(&repo, &["add", "f.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "baseline"]);
        write_two_hunk_file(&repo);

        let diff = DiffService::new(GitRunner::new(), &repo);
        let files = diff
            .diff(&DiffOptions::default(), CancellationToken::new())
            .await
            .expect("diff");
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.hunks.len(), 2, "应有两个 hunk：{:?}", file.hunks.len());

        let svc = HunkStageService::new(GitRunner::new(), &repo);
        let first = file.hunks[0].id;
        svc.stage_hunks(file, &[first], CancellationToken::new())
            .await
            .expect("stage_hunks");

        // 只有第一个 hunk（M2）进 index。
        let staged = run_git(&repo, &["diff", "--cached"]);
        assert!(staged.contains("M2"), "staged diff 应含 M2：{staged}");
        assert!(!staged.contains("M18"), "staged diff 不应含 M18：{staged}");
        // 工作区保持全部改动。
        let worktree = std::fs::read_to_string(repo.join("f.txt")).unwrap();
        assert!(worktree.contains("M2") && worktree.contains("M18"));
        // index 内容同样只含 M2。
        let idx = index_content(&repo, "f.txt");
        assert!(idx.contains("M2") && !idx.contains("M18"));
    }

    #[tokio::test]
    async fn stage_lines_stages_only_selected_lines() {
        let (_dir, repo) = make_repo();
        std::fs::write(repo.join("f.txt"), "l1\nl2\nl3\n").expect("write");
        run_git(&repo, &["add", "f.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "base"]);
        // worktree：改 l2→L2，追加 l4。
        std::fs::write(repo.join("f.txt"), "l1\nL2\nl3\nl4\n").expect("write");

        let diff = DiffService::new(GitRunner::new(), &repo);
        let files = diff
            .diff(&DiffOptions::default(), CancellationToken::new())
            .await
            .expect("diff");
        let file = &files[0];
        assert_eq!(file.hunks.len(), 1);
        let hunk = &file.hunks[0];
        // 行序：ctx l1, del l2, add L2, ctx l3, add l4。
        let kinds: Vec<LineKind> = hunk.lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![
                LineKind::Context,
                LineKind::Deletion,
                LineKind::Addition,
                LineKind::Context,
                LineKind::Addition
            ]
        );

        let svc = HunkStageService::new(GitRunner::new(), &repo);
        // 只暂存 del l2 与 add L2，不暂存 add l4。
        let selection = [false, true, true, false, false];
        svc.stage_lines(file, hunk.id, &selection, CancellationToken::new())
            .await
            .expect("stage_lines");

        assert_eq!(index_content(&repo, "f.txt"), "l1\nL2\nl3\n");
        // 工作区不变。
        assert_eq!(
            std::fs::read_to_string(repo.join("f.txt")).unwrap(),
            "l1\nL2\nl3\nl4\n"
        );
    }

    #[tokio::test]
    async fn unstage_hunks_reverses_selected_hunk() {
        let (_dir, repo) = make_repo();
        let baseline: String = (1..=20).map(|i| format!("L{i}\n")).collect();
        std::fs::write(repo.join("f.txt"), &baseline).expect("write");
        run_git(&repo, &["add", "f.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "baseline"]);
        write_two_hunk_file(&repo);
        // 整文件暂存。
        run_git(&repo, &["add", "f.txt"]);

        let diff = DiffService::new(GitRunner::new(), &repo);
        let opts = DiffOptions {
            staged: true,
            ..Default::default()
        };
        let files = diff
            .diff(&opts, CancellationToken::new())
            .await
            .expect("diff");
        let file = &files[0];
        assert_eq!(file.hunks.len(), 2);

        let svc = HunkStageService::new(GitRunner::new(), &repo);
        let first = file.hunks[0].id;
        svc.unstage_hunks(file, &[first], CancellationToken::new())
            .await
            .expect("unstage_hunks");

        // index 只剩第二个 hunk（M18）。
        let staged = run_git(&repo, &["diff", "--cached"]);
        assert!(staged.contains("M18"), "{staged}");
        assert!(!staged.contains("M2"), "{staged}");
        // 工作区不变。
        let worktree = std::fs::read_to_string(repo.join("f.txt")).unwrap();
        assert!(worktree.contains("M2") && worktree.contains("M18"));
    }

    #[tokio::test]
    async fn renamed_file_is_unsupported() {
        let (_dir, repo) = make_repo();
        run_git(&repo, &["mv", "f.txt", "g.txt"]);
        let diff = DiffService::new(GitRunner::new(), &repo);
        let opts = DiffOptions {
            commit_range: Some("HEAD".into()),
            ..Default::default()
        };
        let files = diff
            .diff(&opts, CancellationToken::new())
            .await
            .expect("diff");
        let file = files
            .iter()
            .find(|f| f.status == FileStatus::Renamed)
            .expect("rename entry");
        let svc = HunkStageService::new(GitRunner::new(), &repo);
        let err = svc
            .stage_hunks(file, &[], CancellationToken::new())
            .await
            .expect_err("rename 应不支持");
        assert!(matches!(err, GitError::Other(_)), "err = {err:?}");
    }

    #[tokio::test]
    async fn stale_diff_maps_to_patch_does_not_apply() {
        let (_dir, repo) = make_repo();
        std::fs::write(repo.join("f.txt"), "l1\nl2\n").expect("write");
        let diff = DiffService::new(GitRunner::new(), &repo);
        let files = diff
            .diff(&DiffOptions::default(), CancellationToken::new())
            .await
            .expect("diff");
        let file = files[0].clone();
        // 期间 index 变化：整文件暂存后 patch preimage 不再匹配。
        run_git(&repo, &["add", "f.txt"]);
        let svc = HunkStageService::new(GitRunner::new(), &repo);
        let ids: Vec<HunkId> = file.hunks.iter().map(|h| h.id).collect();
        let err = svc
            .stage_hunks(&file, &ids, CancellationToken::new())
            .await
            .expect_err("过期 diff 应失败");
        assert!(matches!(err, GitError::PatchDoesNotApply), "err = {err:?}");
    }

    fn mk_file() -> DiffFile {
        DiffFile {
            path: "f.txt".into(),
            status: FileStatus::Modified,
            ..Default::default()
        }
    }

    fn mk_hunk() -> DiffHunk {
        DiffHunk {
            id: HunkId(0),
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 3,
            header: "@@ -1,3 +1,3 @@".into(),
            lines: vec![
                DiffLine {
                    kind: LineKind::Context,
                    text: "a".into(),
                    old_no_newline: false,
                    new_no_newline: false,
                },
                DiffLine {
                    kind: LineKind::Deletion,
                    text: "b".into(),
                    old_no_newline: false,
                    new_no_newline: false,
                },
                DiffLine {
                    kind: LineKind::Addition,
                    text: "B".into(),
                    old_no_newline: false,
                    new_no_newline: false,
                },
                DiffLine {
                    kind: LineKind::Addition,
                    text: "X".into(),
                    old_no_newline: false,
                    new_no_newline: false,
                },
                DiffLine {
                    kind: LineKind::Context,
                    text: "c".into(),
                    old_no_newline: false,
                    new_no_newline: false,
                },
            ],
        }
    }

    #[test]
    fn build_hunk_patch_reuses_header_and_lines() {
        let file = mk_file();
        let hunk = mk_hunk();
        let patch = build_hunk_patch(&file, &[&hunk]);
        assert!(patch.starts_with("diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n"));
        assert!(patch.contains("@@ -1,3 +1,3 @@\n a\n-b\n+B\n+X\n c\n"));
    }

    #[test]
    fn build_line_patch_recomputes_counts() {
        let file = mk_file();
        let hunk = mk_hunk();
        // 选中 del b 与 add B；add X 不暂存。
        let selection = [false, true, true, false, false];
        let patch = build_line_patch(&file, &hunk, &selection).expect("patch");
        // old: ctx a + ctx c + del b = 3；new: ctx a + ctx c + add B = 3。
        assert!(
            patch.contains("@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"),
            "{patch}"
        );
        assert!(!patch.contains("+X"), "未选中的新增不应出现：{patch}");
    }

    #[test]
    fn build_line_patch_unselected_deletion_becomes_context() {
        let file = mk_file();
        let mut hunk = mk_hunk();
        hunk.lines = vec![
            DiffLine {
                kind: LineKind::Deletion,
                text: "x".into(),
                old_no_newline: false,
                new_no_newline: false,
            },
            DiffLine {
                kind: LineKind::Deletion,
                text: "y".into(),
                old_no_newline: false,
                new_no_newline: false,
            },
        ];
        let selection = [true, false];
        let patch = build_line_patch(&file, &hunk, &selection).expect("patch");
        assert!(patch.contains("-x\n y\n"), "{patch}");
        assert!(patch.contains("@@ -1,2 +1,1 @@"), "{patch}");
    }

    #[test]
    fn build_line_patch_nothing_selected_is_none() {
        let file = mk_file();
        let hunk = mk_hunk();
        let selection = [false; 5];
        assert!(build_line_patch(&file, &hunk, &selection).is_none());
        // 只「选中」 context 行也视为无内容。
        let selection_ctx = [true, false, false, false, true];
        assert!(build_line_patch(&file, &hunk, &selection_ctx).is_none());
    }

    #[test]
    fn build_hunk_patch_preserves_no_newline_marker() {
        let file = mk_file();
        let mut hunk = mk_hunk();
        hunk.lines = vec![
            DiffLine {
                kind: LineKind::Deletion,
                text: "old".into(),
                old_no_newline: true,
                new_no_newline: false,
            },
            DiffLine {
                kind: LineKind::Addition,
                text: "new".into(),
                old_no_newline: false,
                new_no_newline: true,
            },
        ];
        let patch = build_hunk_patch(&file, &[&hunk]);
        assert!(patch
            .contains("-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n"));
    }

    #[test]
    fn file_header_handles_add_delete() {
        let mut file = mk_file();
        file.status = FileStatus::Added;
        assert_eq!(file_header(&file).0, "/dev/null");
        file.status = FileStatus::Deleted;
        assert_eq!(file_header(&file).1, "/dev/null");
    }

    #[test]
    fn build_line_patch_unselected_deletion_old_no_newline_emits_marker() {
        // 回归（P7-9 V9）：未选中的 deletion 转为 context 时，须按「输出前缀为
        // context」选择旧侧 old_no_newline 标志，正确补出 `\ No newline`，
        // 修复此前只看 new_no_newline 而漏标旧侧、导致生成的 patch 不合法的问题。
        let file = mk_file();
        let hunk = DiffHunk {
            id: HunkId(0),
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 0,
            header: "@@ -1,2 +0,0 @@".into(),
            lines: vec![
                DiffLine {
                    kind: LineKind::Deletion,
                    text: "keep".into(),
                    old_no_newline: true,
                    new_no_newline: false,
                },
                DiffLine {
                    kind: LineKind::Deletion,
                    text: "drop".into(),
                    old_no_newline: false,
                    new_no_newline: false,
                },
            ],
        };
        // 只暂存 drop（选中），keep 不选 → 转为 context 并保留旧侧无末尾换行标记。
        let patch = build_line_patch(&file, &hunk, &[false, true]).expect("patch");
        assert!(
            patch.contains(" keep\n\\ No newline at end of file\n"),
            "converted context line must carry old-side no-newline marker: {patch}"
        );
        assert!(patch.contains("-drop\n"), "{patch}");
        // old: keep(ctx) + drop(del) = 2；new: keep(ctx) = 1。
        assert!(patch.contains("@@ -1,2 +1,1 @@"), "{patch}");
    }

    #[tokio::test]
    async fn stage_lines_keeps_unselected_deletion_with_old_no_newline() {
        // 回归（P7-9 V9）：旧文件末行无末尾换行、被删除；用户只暂存同 hunk 的
        // 另一处改动，不暂存该删除。生成的 patch 须把未选 deletion 转为 context
        // 并保留旧侧 `\ No newline`，否则 `git apply --cached` 因 preimage 不匹配
        // （末行换行状态不一致）而失败。
        let (_dir, repo) = make_repo();
        // HEAD 基线：x\n a\n last（last 无末尾换行）。
        std::fs::write(repo.join("f.txt"), "x\na\nlast").expect("write baseline");
        run_git(&repo, &["add", "f.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "baseline"]);
        // 工作区：x→X，且删除末行 last。
        std::fs::write(repo.join("f.txt"), "X\na\n").expect("write worktree");

        let diff = DiffService::new(GitRunner::new(), &repo);
        let files = diff
            .diff(&DiffOptions::default(), CancellationToken::new())
            .await
            .expect("diff");
        let file = &files[0];
        assert_eq!(file.hunks.len(), 1);
        let hunk = &file.hunks[0];
        // 期望行序：del x, add X, ctx a, del last(old_no_newline=true)。
        let last = hunk.lines.last().expect("has lines");
        assert_eq!(last.kind, LineKind::Deletion);
        assert!(
            last.old_no_newline,
            "末行删除应标记旧侧无末尾换行: {hunk:?}"
        );

        let svc = HunkStageService::new(GitRunner::new(), &repo);
        // 只暂存 del x / add X（索引 0、1）；ctx a(2) 与 del last(3) 不选。
        let selection = [true, true, false, false];
        svc.stage_lines(file, hunk.id, &selection, CancellationToken::new())
            .await
            .expect("stage_lines");

        // index：保留 last（未暂存删除）且仍无末尾换行，仅应用 x→X。
        assert_eq!(index_content(&repo, "f.txt"), "X\na\nlast");
        // 工作区不变。
        assert_eq!(
            std::fs::read_to_string(repo.join("f.txt")).unwrap(),
            "X\na\n"
        );
    }
}
