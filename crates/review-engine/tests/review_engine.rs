//! P16-8 Review Engine 集成测试：
//! 生命周期 / re-anchor / 只读断言 / 补丁 dry-run / ForgeAdapter / 聚合 /
//! 重放一致性 / 事件持久化 / core 无平台名称分支。

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use agent_domain::{ReviewAnchor, ReviewResolution, ReviewSessionId, ReviewSeverity, WorkspaceId};
use diff_service::model::{DiffFile, DiffHunk, DiffLine, FileStatus, HunkId, LineKind};
use review_engine::*;

// ---------------------------------------------------------------------------
// 测试工具
// ---------------------------------------------------------------------------

fn fixture_file(root: &Path, rel: &str, line_count: u32, prefix: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let content = (1..=line_count)
        .map(|i| format!("{prefix} {i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&path, content).unwrap();
    path
}

fn read_file(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// 递归收集目录内容（相对路径 → 内容），用于只读断言。
fn snapshot_dir(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .to_string();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                out.insert(rel, read_file(&path));
            }
        }
    }
    walk(root, root, &mut out);
    out
}

fn open_input(
    session_id: ReviewSessionId,
    anchor: ReviewAnchor,
    severity: ReviewSeverity,
    body: &str,
) -> OpenFindingInput {
    OpenFindingInput {
        session_id,
        anchor,
        severity,
        body: body.to_string(),
        evidence: vec!["evidence-1".to_string()],
        assignee: Some("reviewer".to_string()),
        suggested_patch: None,
    }
}

// ---------------------------------------------------------------------------
// 生命周期与 resolution 状态机
// ---------------------------------------------------------------------------

#[test]
fn finding_lifecycle_with_rich_fields_and_fix_ref() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture_file(root, "src/app.rs", 12, "line");
    let engine = ReviewEngine::new(Some(root.to_path_buf()));

    let created = engine
        .create_session(Some(WorkspaceId::new("ws-1")))
        .unwrap();
    let ReviewEvent::SessionCreated { session_id, .. } = created else {
        panic!("期望 SessionCreated");
    };

    let opened = engine
        .open_finding(open_input(
            session_id.clone(),
            ReviewAnchor {
                file: "src/app.rs".to_string(),
                line: 6,
                end_line: Some(7),
            },
            ReviewSeverity::Major,
            "越界访问",
        ))
        .unwrap();
    let ReviewEvent::FindingOpened {
        finding_id, anchor, ..
    } = &opened
    else {
        panic!("期望 FindingOpened");
    };
    assert_eq!(anchor.file, "src/app.rs");
    assert_eq!(anchor.line, 6);

    // 富快照字段在内存态可见。
    let snap = engine.snapshot(&session_id).unwrap();
    assert_eq!(snap.findings.len(), 1);
    assert_eq!(snap.findings[0].evidence, vec!["evidence-1".to_string()]);
    assert_eq!(snap.findings[0].assignee.as_deref(), Some("reviewer"));
    assert_eq!(snap.findings[0].severity, ReviewSeverity::Major);
    assert_eq!(snap.findings[0].resolution, ReviewResolution::Open);
    assert!(!snap.findings[0].stale, "文件未编辑不应漂移");

    // open → addressed → resolved（带 fix_ref）。
    let addressed = engine
        .resolve_finding(
            &session_id,
            finding_id,
            ReviewResolution::Addressed,
            Some("patch:fix.patch".to_string()),
        )
        .unwrap();
    let ReviewEvent::FindingResolved {
        resolution,
        fix_ref,
        ..
    } = addressed
    else {
        panic!("期望 FindingResolved");
    };
    assert_eq!(resolution, ReviewResolution::Addressed);
    assert_eq!(fix_ref.as_deref(), Some("patch:fix.patch"));

    let resolved = engine
        .resolve_finding(
            &session_id,
            finding_id,
            ReviewResolution::Resolved,
            Some("commit:abc123".to_string()),
        )
        .unwrap();
    let ReviewEvent::FindingResolved {
        resolution,
        fix_ref,
        ..
    } = resolved
    else {
        panic!("期望 FindingResolved");
    };
    assert_eq!(resolution, ReviewResolution::Resolved);
    assert_eq!(fix_ref.as_deref(), Some("commit:abc123"));

    let snap = engine.snapshot(&session_id).unwrap();
    assert_eq!(snap.findings[0].resolution, ReviewResolution::Resolved);
    assert_eq!(snap.findings[0].fix_ref.as_deref(), Some("commit:abc123"));
}

#[test]
fn resolution_state_machine_rejects_illegal_transitions() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_file(tmp.path(), "a.rs", 10, "line");
    let engine = ReviewEngine::new(Some(tmp.path().to_path_buf()));
    let created = engine.create_session(None).unwrap();
    let ReviewEvent::SessionCreated { session_id, .. } = created else {
        unreachable!()
    };
    let opened = engine
        .open_finding(open_input(
            session_id.clone(),
            ReviewAnchor {
                file: "a.rs".to_string(),
                line: 3,
                end_line: None,
            },
            ReviewSeverity::Info,
            "问题",
        ))
        .unwrap();
    let ReviewEvent::FindingOpened { finding_id, .. } = &opened else {
        unreachable!()
    };

    // open → resolved 直接跳转：非法。
    let err = engine
        .resolve_finding(&session_id, finding_id, ReviewResolution::Resolved, None)
        .unwrap_err();
    assert!(matches!(err, ReviewError::InvalidTransition { .. }));
    // resolved → open：非法。
    engine
        .resolve_finding(&session_id, finding_id, ReviewResolution::Addressed, None)
        .unwrap();
    engine
        .resolve_finding(&session_id, finding_id, ReviewResolution::Resolved, None)
        .unwrap();
    let err = engine
        .resolve_finding(&session_id, finding_id, ReviewResolution::Open, None)
        .unwrap_err();
    assert!(matches!(err, ReviewError::InvalidTransition { .. }));
    // 终结态不可再转移。
    let err = engine
        .resolve_finding(&session_id, finding_id, ReviewResolution::Wontfix, None)
        .unwrap_err();
    assert!(matches!(err, ReviewError::InvalidTransition { .. }));

    // 未知 session / finding。
    assert!(matches!(
        engine.snapshot(&ReviewSessionId::new("nope")).unwrap_err(),
        ReviewError::UnknownSession(_)
    ));
    assert!(matches!(
        engine
            .resolve_finding(
                &session_id,
                &agent_domain::ReviewFindingId::new("nope"),
                ReviewResolution::Addressed,
                None
            )
            .unwrap_err(),
        ReviewError::UnknownFinding(_)
    ));
}

#[test]
fn wontfix_is_legal_from_addressed() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_file(tmp.path(), "a.rs", 5, "line");
    let engine = ReviewEngine::new(Some(tmp.path().to_path_buf()));
    let ReviewEvent::SessionCreated { session_id, .. } = engine.create_session(None).unwrap()
    else {
        unreachable!()
    };
    let ReviewEvent::FindingOpened { finding_id, .. } = engine
        .open_finding(open_input(
            session_id.clone(),
            ReviewAnchor {
                file: "a.rs".to_string(),
                line: 2,
                end_line: None,
            },
            ReviewSeverity::Minor,
            "风格",
        ))
        .unwrap()
    else {
        unreachable!()
    };
    engine
        .resolve_finding(&session_id, &finding_id, ReviewResolution::Addressed, None)
        .unwrap();
    engine
        .resolve_finding(
            &session_id,
            &finding_id,
            ReviewResolution::Wontfix,
            Some("run:r-42".to_string()),
        )
        .unwrap();
    let snap = engine.snapshot(&session_id).unwrap();
    assert_eq!(snap.findings[0].resolution, ReviewResolution::Wontfix);
    assert_eq!(snap.findings[0].fix_ref.as_deref(), Some("run:r-42"));
}

// ---------------------------------------------------------------------------
// re-anchor
// ---------------------------------------------------------------------------

#[test]
fn reanchor_relocates_after_edit_and_marks_stale_on_drift() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture_file(root, "src/lib.rs", 12, "line");
    let engine = ReviewEngine::new(Some(root.to_path_buf()));
    let ReviewEvent::SessionCreated { session_id, .. } = engine.create_session(None).unwrap()
    else {
        unreachable!()
    };
    let ReviewEvent::FindingOpened { finding_id, .. } = engine
        .open_finding(open_input(
            session_id.clone(),
            ReviewAnchor {
                file: "src/lib.rs".to_string(),
                line: 6,
                end_line: Some(7),
            },
            ReviewSeverity::Critical,
            "数据竞争",
        ))
        .unwrap()
    else {
        unreachable!()
    };

    // 未编辑：原位，不漂移。
    let outcome = engine.reanchor(&session_id, &finding_id).unwrap();
    assert!(!outcome.stale);
    assert_eq!(outcome.anchor.line, 6);

    // 在锚点上方远处插入 2 行：整块上下文下移 → 重新定位到 line 8，end_line 9。
    let path = root.join("src/lib.rs");
    let text = read_file(&path);
    let mut lines: Vec<&str> = text.lines().collect();
    lines.splice(1..1, ["inserted A", "inserted B"]);
    fs::write(&path, lines.join("\n")).unwrap();

    let outcome = engine.reanchor(&session_id, &finding_id).unwrap();
    assert!(!outcome.stale, "邻近行指纹应重新定位");
    assert_eq!(outcome.anchor.line, 8);
    assert_eq!(outcome.anchor.end_line, Some(9));

    // 全量重写：指纹失配 → stale，锚点保留原值（不静默失效）。
    let content = (1..=20)
        .map(|i| format!("changed {i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&path, content).unwrap();
    let outcome = engine.reanchor(&session_id, &finding_id).unwrap();
    assert!(outcome.stale);
    assert_eq!(outcome.reason, StaleReason::ContextMoved);
    assert_eq!(outcome.anchor.line, 6, "漂移时保留原锚点");

    // 文件删除：stale=FileUnavailable。
    fs::remove_file(&path).unwrap();
    let outcome = engine.reanchor(&session_id, &finding_id).unwrap();
    assert!(outcome.stale);
    assert_eq!(outcome.reason, StaleReason::FileUnavailable);

    // snapshot 同样反映 stale 派生结果。
    let snap = engine.snapshot(&session_id).unwrap();
    assert!(snap.findings[0].stale);
    assert_eq!(
        snap.findings[0].stale_reason.as_deref(),
        Some("file_unavailable")
    );
}

#[test]
fn anchor_rejects_traversal_and_out_of_range_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture_file(root, "a.rs", 5, "line");
    let engine = ReviewEngine::new(Some(root.to_path_buf()));
    let ReviewEvent::SessionCreated { session_id, .. } = engine.create_session(None).unwrap()
    else {
        unreachable!()
    };

    // 绝对路径 / `..` 逃逸：拒绝。
    for bad in ["/etc/passwd", "../secret.txt", "a/../../b"] {
        let err = engine
            .open_finding(open_input(
                session_id.clone(),
                ReviewAnchor {
                    file: bad.to_string(),
                    line: 1,
                    end_line: None,
                },
                ReviewSeverity::Info,
                "bad",
            ))
            .unwrap_err();
        assert!(
            matches!(err, ReviewError::TraversalDenied(_)),
            "应拒绝 {bad}"
        );
    }
    // 行号越界。
    let err = engine
        .open_finding(open_input(
            session_id.clone(),
            ReviewAnchor {
                file: "a.rs".to_string(),
                line: 99,
                end_line: None,
            },
            ReviewSeverity::Info,
            "bad",
        ))
        .unwrap_err();
    assert!(matches!(err, ReviewError::InvalidAnchor { .. }));
    // 未配置 workspace root：无法解析锚点。
    let no_root = ReviewEngine::new(None);
    let ReviewEvent::SessionCreated {
        session_id: rootless,
        ..
    } = no_root.create_session(None).unwrap()
    else {
        unreachable!()
    };
    let err = no_root
        .open_finding(open_input(
            rootless,
            ReviewAnchor {
                file: "a.rs".to_string(),
                line: 1,
                end_line: None,
            },
            ReviewSeverity::Info,
            "bad",
        ))
        .unwrap_err();
    assert!(matches!(err, ReviewError::InvalidAnchor { .. }));
}

// ---------------------------------------------------------------------------
// 只读断言
// ---------------------------------------------------------------------------

#[test]
fn engine_never_writes_to_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture_file(root, "src/app.rs", 12, "line");
    fixture_file(root, "src/other.rs", 4, "line");
    let before = snapshot_dir(root);

    let engine = ReviewEngine::new(Some(root.to_path_buf()));
    let ReviewEvent::SessionCreated { session_id, .. } = engine
        .create_session(Some(WorkspaceId::new("ws-1")))
        .unwrap()
    else {
        unreachable!()
    };
    let ReviewEvent::FindingOpened { finding_id, .. } = engine
        .open_finding(open_input(
            session_id.clone(),
            ReviewAnchor {
                file: "src/app.rs".to_string(),
                line: 5,
                end_line: None,
            },
            ReviewSeverity::Major,
            "越界",
        ))
        .unwrap()
    else {
        unreachable!()
    };
    engine
        .resolve_finding(
            &session_id,
            &finding_id,
            ReviewResolution::Addressed,
            Some("commit:x".to_string()),
        )
        .unwrap();
    let _ = engine.snapshot(&session_id).unwrap();
    let _ = engine.reanchor(&session_id, &finding_id).unwrap();

    let adapter = GenericForgeAdapter::default();
    let pr = PrReference {
        repo: "org/repo".to_string(),
        pr_number: 42,
        head_sha: None,
    };
    let _ = engine.export_comments(&session_id, &adapter, &pr).unwrap();
    let _ = engine
        .publish_comment(&session_id, &finding_id, &adapter, &pr)
        .unwrap();
    let _ = engine.aggregate(&session_id, AggregateBy::File).unwrap();

    assert_eq!(before, snapshot_dir(root), "评审引擎必须保持工作区只读");
}

// ---------------------------------------------------------------------------
// SuggestedPatch dry-run
// ---------------------------------------------------------------------------

#[test]
fn patch_dry_run_validates_and_applies_in_memory_without_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let file = fixture_file(root, "a.rs", 4, "line");
    let current = read_file(&file);

    let valid = SuggestedPatch {
        file: "a.rs".to_string(),
        payload: "@@ -2,2 +2,3 @@\n line 02\n+inserted\n line 03\n".to_string(),
    };
    let report = PatchValidator::dry_run(&valid).unwrap();
    assert_eq!(report.hunks, 1);
    assert_eq!(report.additions, 1);
    assert_eq!(report.deletions, 0);

    let applied = PatchValidator::apply_in_memory(&valid, &current).unwrap();
    assert_eq!(applied, "line 01\nline 02\ninserted\nline 03\nline 04");
    // dry-run 不写盘。
    assert_eq!(read_file(&file), current);

    // hunk 头行数与内容行数不一致 → InvalidPatch。
    let bad_counts = SuggestedPatch {
        file: "a.rs".to_string(),
        payload: "@@ -1,2 +1,3 @@\n line 01\n+only\n".to_string(),
    };
    assert!(matches!(
        PatchValidator::dry_run(&bad_counts).unwrap_err(),
        ReviewError::InvalidPatch(_)
    ));

    // context 不匹配 → PatchContextMismatch。
    let mismatched = SuggestedPatch {
        file: "a.rs".to_string(),
        payload: "@@ -2,2 +2,2 @@\n line 99\n line 03\n".to_string(),
    };
    assert!(matches!(
        PatchValidator::apply_in_memory(&mismatched, &current).unwrap_err(),
        ReviewError::PatchContextMismatch { .. }
    ));

    // 空补丁。
    let empty = SuggestedPatch {
        file: "a.rs".to_string(),
        payload: "".to_string(),
    };
    assert!(matches!(
        PatchValidator::dry_run(&empty).unwrap_err(),
        ReviewError::InvalidPatch(_)
    ));
}

#[test]
fn open_finding_rejects_invalid_patch_without_state_change() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture_file(root, "a.rs", 4, "line");
    let engine = ReviewEngine::new(Some(root.to_path_buf()));
    let ReviewEvent::SessionCreated { session_id, .. } = engine.create_session(None).unwrap()
    else {
        unreachable!()
    };
    let mut input = open_input(
        session_id.clone(),
        ReviewAnchor {
            file: "a.rs".to_string(),
            line: 2,
            end_line: None,
        },
        ReviewSeverity::Major,
        "问题",
    );
    input.suggested_patch = Some(SuggestedPatch {
        file: "a.rs".to_string(),
        payload: "not a patch".to_string(),
    });
    assert!(matches!(
        engine.open_finding(input).unwrap_err(),
        ReviewError::InvalidPatch(_)
    ));
    assert!(engine.snapshot(&session_id).unwrap().findings.is_empty());
}

// ---------------------------------------------------------------------------
// ForgeAdapter：生成 ≠ 发布；core 无平台分支
// ---------------------------------------------------------------------------

/// 记录型 adapter：验证引擎只经显式发布路径触发外部副作用。
#[derive(Clone)]
struct RecordingAdapter {
    kind: ForgeKind,
    calls: Arc<Mutex<Vec<String>>>,
    fail_publish: bool,
}

impl ForgeAdapter for RecordingAdapter {
    fn kind(&self) -> ForgeKind {
        self.kind
    }

    fn fetch_pr_context(&self, pr: &PrReference) -> Result<PRContext, ReviewError> {
        Ok(PRContext {
            repo: pr.repo.clone(),
            pr_number: pr.pr_number,
            title: format!("PR #{}", pr.pr_number),
            files: vec!["src/app.rs".to_string()],
            head_sha: pr.head_sha.clone(),
            base_ref: Some("main".to_string()),
            raw: None,
        })
    }

    fn map_comment(&self, _context: &PRContext, finding: &ReviewFinding) -> PRComment {
        PRComment {
            id: None,
            anchor: Some(finding.anchor.clone()),
            body: finding.body.clone(),
            published: false,
        }
    }

    fn publish_comment(
        &self,
        _context: &PRContext,
        comment: &PRComment,
    ) -> Result<PRComment, ReviewError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("publish:{}", comment.body));
        if self.fail_publish {
            return Err(ReviewError::Forge("publish failed".to_string()));
        }
        Ok(PRComment {
            id: Some("remote-1".to_string()),
            published: true,
            ..comment.clone()
        })
    }
}

fn pr_ref() -> PrReference {
    PrReference {
        repo: "org/repo".to_string(),
        pr_number: 7,
        head_sha: Some("sha1".to_string()),
    }
}

#[test]
fn generating_comments_never_publishes_and_publish_is_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture_file(root, "src/app.rs", 12, "line");
    let engine = ReviewEngine::new(Some(root.to_path_buf()));
    let ReviewEvent::SessionCreated { session_id, .. } = engine.create_session(None).unwrap()
    else {
        unreachable!()
    };
    let ReviewEvent::FindingOpened { finding_id, .. } = engine
        .open_finding(open_input(
            session_id.clone(),
            ReviewAnchor {
                file: "src/app.rs".to_string(),
                line: 4,
                end_line: None,
            },
            ReviewSeverity::Major,
            "越界",
        ))
        .unwrap()
    else {
        unreachable!()
    };

    let adapter = RecordingAdapter {
        kind: ForgeKind::GitHub,
        calls: Arc::new(Mutex::new(Vec::new())),
        fail_publish: false,
    };

    // 生成评论：adapter 未被调用 publish，引擎无 CommentPublished。
    let (context, comments) = engine
        .export_comments(&session_id, &adapter, &pr_ref())
        .unwrap();
    assert_eq!(context.pr_number, 7);
    assert_eq!(comments.len(), 1);
    assert!(!comments[0].published);
    assert!(adapter.calls.lock().unwrap().is_empty());
    assert!(engine
        .snapshot(&session_id)
        .unwrap()
        .published_comments
        .is_empty());

    // 显式发布：唯一触发外部副作用的路径。
    let event = engine
        .publish_comment(&session_id, &finding_id, &adapter, &pr_ref())
        .unwrap();
    let ReviewEvent::CommentPublished { forge, .. } = event else {
        panic!("期望 CommentPublished");
    };
    assert_eq!(forge, "github", "forge 标签来自 adapter，不分支");
    assert_eq!(adapter.calls.lock().unwrap().len(), 1);
    let snap = engine.snapshot(&session_id).unwrap();
    assert_eq!(snap.published_comments.len(), 1);
    assert_eq!(snap.published_comments[0].forge, "github");
}

#[test]
fn publish_failure_leaves_no_event() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture_file(root, "a.rs", 5, "line");
    let engine = ReviewEngine::new(Some(root.to_path_buf()));
    let ReviewEvent::SessionCreated { session_id, .. } = engine.create_session(None).unwrap()
    else {
        unreachable!()
    };
    let ReviewEvent::FindingOpened { finding_id, .. } = engine
        .open_finding(open_input(
            session_id.clone(),
            ReviewAnchor {
                file: "a.rs".to_string(),
                line: 2,
                end_line: None,
            },
            ReviewSeverity::Minor,
            "风格",
        ))
        .unwrap()
    else {
        unreachable!()
    };
    let adapter = RecordingAdapter {
        kind: ForgeKind::GitLab,
        calls: Arc::new(Mutex::new(Vec::new())),
        fail_publish: true,
    };
    assert!(matches!(
        engine
            .publish_comment(&session_id, &finding_id, &adapter, &pr_ref())
            .unwrap_err(),
        ReviewError::Forge(_)
    ));
    assert!(engine
        .snapshot(&session_id)
        .unwrap()
        .published_comments
        .is_empty());
}

#[test]
fn generic_adapter_works_identically_without_platform_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture_file(root, "a.rs", 5, "line");
    let engine = ReviewEngine::new(Some(root.to_path_buf()));
    let ReviewEvent::SessionCreated { session_id, .. } = engine.create_session(None).unwrap()
    else {
        unreachable!()
    };
    let ReviewEvent::FindingOpened { finding_id, .. } = engine
        .open_finding(open_input(
            session_id.clone(),
            ReviewAnchor {
                file: "a.rs".to_string(),
                line: 2,
                end_line: None,
            },
            ReviewSeverity::Info,
            "说明",
        ))
        .unwrap()
    else {
        unreachable!()
    };
    let generic = GenericForgeAdapter {
        base_ref: Some("main".to_string()),
    };
    let event = engine
        .publish_comment(&session_id, &finding_id, &generic, &pr_ref())
        .unwrap();
    let ReviewEvent::CommentPublished { forge, .. } = event else {
        panic!("期望 CommentPublished");
    };
    assert_eq!(forge, "generic");
    let snap = engine.snapshot(&session_id).unwrap();
    assert_eq!(snap.published_comments[0].forge, "generic");
    assert_eq!(snap.pending_comments.len(), 1);
}

// ---------------------------------------------------------------------------
// 聚合与 diff 导入
// ---------------------------------------------------------------------------

fn build_diff() -> DiffFile {
    DiffFile {
        path: "src/app.rs".to_string(),
        previous_path: None,
        status: FileStatus::Modified,
        staged: false,
        binary: false,
        additions: 2,
        deletions: 0,
        hunks: vec![DiffHunk {
            id: HunkId(0),
            old_start: 2,
            old_lines: 2,
            new_start: 2,
            new_lines: 4,
            header: "@@ -2,2 +2,4 @@".to_string(),
            lines: vec![
                DiffLine {
                    kind: LineKind::Context,
                    text: "line 02".to_string(),
                    old_no_newline: false,
                    new_no_newline: false,
                },
                DiffLine {
                    kind: LineKind::Addition,
                    text: "added A".to_string(),
                    old_no_newline: false,
                    new_no_newline: false,
                },
                DiffLine {
                    kind: LineKind::Addition,
                    text: "added B".to_string(),
                    old_no_newline: false,
                    new_no_newline: false,
                },
                DiffLine {
                    kind: LineKind::Context,
                    text: "line 03".to_string(),
                    old_no_newline: false,
                    new_no_newline: false,
                },
            ],
        }],
    }
}

#[test]
fn import_diff_generates_pending_comments_without_publishing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture_file(root, "src/app.rs", 4, "line");
    let engine = ReviewEngine::new(Some(root.to_path_buf()));
    let ReviewEvent::SessionCreated { session_id, .. } = engine.create_session(None).unwrap()
    else {
        unreachable!()
    };
    let diff = build_diff();
    let events = engine.import_diff(&session_id, &diff).unwrap();
    assert_eq!(events.len(), 2, "两条 Addition 行 → 两个 finding");
    for event in &events {
        assert!(matches!(event, ReviewEvent::FindingOpened { .. }));
    }

    let snap = engine.snapshot(&session_id).unwrap();
    assert_eq!(snap.findings.len(), 2);
    // 新侧行号锚点：3 与 4。
    let mut lines: Vec<u32> = snap.findings.iter().map(|f| f.anchor.line).collect();
    lines.sort_unstable();
    assert_eq!(lines, vec![3, 4]);
    assert_eq!(snap.findings[0].severity, ReviewSeverity::Info);
    assert!(snap.findings[0].evidence[0].starts_with("diff:src/app.rs:"));
    // 待发布评论已生成，但未发布。
    assert_eq!(snap.pending_comments.len(), 2);
    assert!(snap.published_comments.is_empty());
}

#[test]
fn aggregate_by_file_severity_status() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture_file(root, "a.rs", 6, "line");
    fixture_file(root, "b.rs", 6, "line");
    let engine = ReviewEngine::new(Some(root.to_path_buf()));
    let ReviewEvent::SessionCreated { session_id, .. } = engine.create_session(None).unwrap()
    else {
        unreachable!()
    };
    for (file, line, severity) in [
        ("a.rs", 2, ReviewSeverity::Critical),
        ("a.rs", 4, ReviewSeverity::Major),
        ("b.rs", 3, ReviewSeverity::Critical),
        ("b.rs", 5, ReviewSeverity::Info),
    ] {
        engine
            .open_finding(open_input(
                session_id.clone(),
                ReviewAnchor {
                    file: file.to_string(),
                    line,
                    end_line: None,
                },
                severity,
                "问题",
            ))
            .unwrap();
    }
    let snap = engine.snapshot(&session_id).unwrap();

    let by_file = &snap.aggregate.by_file;
    assert_eq!(by_file.len(), 2);
    let a = by_file.iter().find(|g| g.key == "a.rs").unwrap();
    assert_eq!(a.total, 2);
    let b = by_file.iter().find(|g| g.key == "b.rs").unwrap();
    assert_eq!(b.total, 2);

    let by_severity = &snap.aggregate.by_severity;
    let critical = by_severity.iter().find(|g| g.key == "critical").unwrap();
    assert_eq!(critical.total, 2);
    let info = by_severity.iter().find(|g| g.key == "info").unwrap();
    assert_eq!(info.total, 1);

    let by_status = &snap.aggregate.by_status;
    let open = by_status.iter().find(|g| g.key == "open").unwrap();
    assert_eq!(open.total, 4);

    // 直接聚合 API 与快照一致。
    let by_file_direct = engine.aggregate(&session_id, AggregateBy::File).unwrap();
    assert_eq!(by_file_direct, *by_file);
}

// ---------------------------------------------------------------------------
// 重放一致性
// ---------------------------------------------------------------------------

#[test]
fn replay_rebuilds_identical_lifecycle_state() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture_file(root, "src/app.rs", 12, "line");
    let engine = ReviewEngine::new(Some(root.to_path_buf()));
    let mut events = Vec::new();

    let e1 = engine
        .create_session(Some(WorkspaceId::new("ws-1")))
        .unwrap();
    events.push(e1.clone());
    let ReviewEvent::SessionCreated { session_id, .. } = &e1 else {
        unreachable!()
    };
    let e2 = engine
        .open_finding(open_input(
            session_id.clone(),
            ReviewAnchor {
                file: "src/app.rs".to_string(),
                line: 6,
                end_line: None,
            },
            ReviewSeverity::Major,
            "越界",
        ))
        .unwrap();
    events.push(e2.clone());
    let ReviewEvent::FindingOpened { finding_id, .. } = &e2 else {
        unreachable!()
    };
    events.push(
        engine
            .resolve_finding(
                session_id,
                finding_id,
                ReviewResolution::Addressed,
                Some("patch:f.patch".to_string()),
            )
            .unwrap(),
    );
    events.push(
        engine
            .resolve_finding(
                session_id,
                finding_id,
                ReviewResolution::Resolved,
                Some("commit:deadbeef".to_string()),
            )
            .unwrap(),
    );
    let adapter = GenericForgeAdapter::default();
    events.push(
        engine
            .publish_comment(session_id, finding_id, &adapter, &pr_ref())
            .unwrap(),
    );

    // 新引擎重放同一事件序列。
    let replay_engine = ReviewEngine::new(Some(root.to_path_buf()));
    replay_engine.replay(events.clone()).unwrap();

    // 逐步 apply 与整体 replay 结果一致。
    let step_engine = ReviewEngine::new(Some(root.to_path_buf()));
    for event in &events {
        step_engine.apply(event).unwrap();
    }

    // live→fresh replay 必须完整 snapshot 相等：含 evidence / assignee /
    // suggested_patch / fingerprint / resolution / fix_ref（ADR-016）。
    let original = engine.snapshot(session_id).unwrap();
    let replayed = replay_engine.snapshot(session_id).unwrap();
    let stepped = step_engine.snapshot(session_id).unwrap();
    assert_eq!(
        original, replayed,
        "live→fresh replay 必须完整 snapshot 相等"
    );
    assert_eq!(original, stepped, "逐步 apply 与整体 replay 必须相等");

    // 重放后继续发命令：确定性 id 不冲突。
    let e = replay_engine.create_session(None).unwrap();
    let ReviewEvent::SessionCreated {
        session_id: new_id, ..
    } = e
    else {
        unreachable!()
    };
    assert_ne!(new_id.to_string(), "session_1");
    // 三个会话（含原始 ws-1）。
    assert_eq!(replay_engine.snapshot(&new_id).unwrap().session_id, new_id);
}

#[test]
fn replay_rejects_invalid_event_sequences() {
    let engine = ReviewEngine::new(None);
    let created = engine.create_session(None).unwrap();
    let ReviewEvent::SessionCreated { .. } = &created else {
        unreachable!()
    };
    // 未创建会话就开 finding：重放报错。
    let orphan = ReviewEvent::FindingOpened {
        session_id: ReviewSessionId::new("orphan"),
        finding_id: agent_domain::ReviewFindingId::new("f1"),
        anchor: ReviewAnchor {
            file: "a.rs".to_string(),
            line: 1,
            end_line: None,
        },
        severity: ReviewSeverity::Info,
        body: "x".to_string(),
        evidence: Vec::new(),
        assignee: None,
        suggested_patch: None,
        fingerprint: None,
    };
    assert!(matches!(
        engine.replay(vec![orphan]).unwrap_err(),
        ReviewError::UnknownSession(_)
    ));
    // 重复 SessionCreated。
    assert!(matches!(
        engine.replay(vec![created.clone(), created]).unwrap_err(),
        ReviewError::Duplicate { .. }
    ));
    // 非法转移（open → resolved 直跳）。
    let opened = ReviewEvent::FindingOpened {
        session_id: ReviewSessionId::new("s-fixed"),
        finding_id: agent_domain::ReviewFindingId::new("f1"),
        anchor: ReviewAnchor {
            file: "a.rs".to_string(),
            line: 1,
            end_line: None,
        },
        severity: ReviewSeverity::Info,
        body: "x".to_string(),
        evidence: Vec::new(),
        assignee: None,
        suggested_patch: None,
        fingerprint: None,
    };
    let bad = ReviewEvent::FindingResolved {
        finding_id: agent_domain::ReviewFindingId::new("f1"),
        resolution: ReviewResolution::Resolved,
        fix_ref: None,
    };
    assert!(matches!(
        engine
            .replay(vec![session_id_fixed(), opened, bad])
            .unwrap_err(),
        ReviewError::InvalidTransition { .. }
    ));
}

fn session_id_fixed() -> ReviewEvent {
    ReviewEvent::SessionCreated {
        session_id: ReviewSessionId::new("s-fixed"),
        workspace_id: None,
    }
}

// ---------------------------------------------------------------------------
// 事件持久化（agent-events wrapping）
// ---------------------------------------------------------------------------

#[test]
fn review_event_round_trips_through_agent_event_wrapper() {
    let event = ReviewEvent::FindingOpened {
        session_id: ReviewSessionId::new("s1"),
        finding_id: agent_domain::ReviewFindingId::new("f1"),
        anchor: ReviewAnchor {
            file: "src/app.rs".to_string(),
            line: 12,
            end_line: Some(14),
        },
        severity: ReviewSeverity::Critical,
        body: "越界".to_string(),
        evidence: vec!["e1".to_string()],
        assignee: Some("alice".to_string()),
        suggested_patch: None,
        fingerprint: Some("fp-1".to_string()),
    };
    let wrapped = agent_events::AgentEvent::Review(event.clone());
    let json = serde_json::to_string(&wrapped).unwrap();
    assert!(
        json.contains("\"finding_opened\""),
        "snake_case tag：{json}"
    );
    let back: agent_events::AgentEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(back, wrapped);

    let resolved = ReviewEvent::FindingResolved {
        finding_id: agent_domain::ReviewFindingId::new("f1"),
        resolution: ReviewResolution::Resolved,
        fix_ref: Some("commit:abc".to_string()),
    };
    let wrapped = agent_events::AgentEvent::Review(resolved);
    let json = serde_json::to_string(&wrapped).unwrap();
    assert!(json.contains("\"finding_resolved\""));
    assert!(json.contains("commit:abc"));
    let back: agent_events::AgentEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(back, wrapped);
}

// ---------------------------------------------------------------------------
// core 无 GitHub / GitLab 名称 match 分支
// ---------------------------------------------------------------------------

#[test]
fn review_core_has_no_platform_name_branch() {
    // 平台差异必须收敛在 forge.rs（adapter 层）；core 行为文件不得出现平台名称。
    for (file, source) in [
        ("engine.rs", include_str!("../src/engine.rs")),
        ("aggregate.rs", include_str!("../src/aggregate.rs")),
    ] {
        assert!(
            !source.contains("GitHub") && !source.contains("GitLab"),
            "{file} 不得包含平台名称分支"
        );
    }
    // adapter 层（forge.rs）允许且必须承载平台枚举——扫描有效性自检。
    let forge_source = include_str!("../src/forge.rs");
    assert!(forge_source.contains("GitHub") && forge_source.contains("GitLab"));
}

// ---------------------------------------------------------------------------
// 旧流 serde 兼容（FindingOpened 新增字段）
// ---------------------------------------------------------------------------

#[test]
fn legacy_finding_opened_json_without_rich_fields_defaults() {
    // 旧流 FindingOpened 事件缺 evidence / assignee / suggested_patch / fingerprint
    // → serde default（空 / None），不破坏历史持久化事件反序列化。
    let legacy = r#"{"kind":"finding_opened","session_id":"s1","finding_id":"f1","anchor":{"file":"a.rs","line":1},"severity":"info","body":"x"}"#;
    let event: ReviewEvent = serde_json::from_str(legacy).unwrap();
    let ReviewEvent::FindingOpened {
        evidence,
        assignee,
        suggested_patch,
        fingerprint,
        ..
    } = event
    else {
        panic!("expected FindingOpened");
    };
    assert!(evidence.is_empty());
    assert!(assignee.is_none());
    assert!(suggested_patch.is_none());
    assert!(fingerprint.is_none());
}
