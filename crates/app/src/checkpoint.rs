//! 写前快照、checkpoint 列表与回滚（Blob，绝不 `git reset --hard`）。

use std::collections::BTreeSet;
use std::path::PathBuf;

use pawork_domain::{AgentEvent, ArtifactId, CancellationToken, CheckpointId, RunId, SessionId};
use pawork_engine::{LoopEventEmitter, PendingToolInvocation, WriteCheckpoint};
use pawork_storage::blob::{CheckpointService, FileSnapshot, RunCheckpoint};

/// 快照失败诊断的用户可见文案；与 protocol 投影的空 message 兜底一致。
const SNAPSHOT_FAILED_MESSAGE: &str =
    "checkpoint snapshot failed — write proceeded without rollback point";

use crate::{AppCore, AppError};

pub(crate) const WRITE_TOOLS: &[&str] = &["write_file", "edit_file", "apply_patch"];

pub(crate) fn is_write_tool(name: &str) -> bool {
    WRITE_TOOLS.contains(&name)
}

pub(crate) fn write_paths(name: &str, args: &serde_json::Value) -> Vec<String> {
    match name {
        "write_file" | "edit_file" => crate::approval::relative_path_from_input(args)
            .into_iter()
            .collect(),
        "apply_patch" => args
            .get("ops")
            .and_then(|value| value.as_array())
            .map(|ops| {
                ops.iter()
                    .flat_map(|op| {
                        let mut paths = Vec::new();
                        if let Some(path) = op.get("path").and_then(|value| value.as_str()) {
                            if !path.is_empty() {
                                paths.push(path.to_string());
                            }
                        }
                        if let Some(to) = op.get("to").and_then(|value| value.as_str()) {
                            if !to.is_empty() {
                                paths.push(to.to_string());
                            }
                        }
                        paths
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub(crate) async fn snapshot_write_tools(
    checkpoints: &CheckpointService,
    run_id: &RunId,
    roots: &[PathBuf],
    calls: &[PendingToolInvocation],
    events: LoopEventEmitter<'_>,
    _cancel: CancellationToken,
) -> Vec<WriteCheckpoint> {
    let mut out = Vec::new();
    for call in calls {
        if !is_write_tool(&call.name) {
            continue;
        }
        let paths = write_paths(&call.name, &call.arguments);
        if paths.is_empty() {
            continue;
        }
        let mut artifacts = Vec::new();
        let mut any = false;
        for path in &paths {
            match checkpoints
                .snapshot_before_write(run_id.as_str(), call.tool_call_id.as_str(), roots, path)
                .await
            {
                Ok(snap) => {
                    any = true;
                    if let Some(blob) = snap.pre_blob {
                        artifacts.push(ArtifactId::from(blob.as_str()));
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        run_id = run_id.as_str(),
                        tool_call_id = call.tool_call_id.as_str(),
                        path,
                        error = %error,
                        "write-tool snapshot skipped"
                    );
                    // 快照失败不能静默：用户会误以为存在回滚点。发诊断
                    // （persist-first 落库并广播），写入仍继续。
                    let details = serde_json::json!({
                        "message": SNAPSHOT_FAILED_MESSAGE,
                        "path": path,
                        "error": error.to_string(),
                    });
                    if let Err(emit_error) = events
                        .emit(AgentEvent::Diagnostic {
                            code: "checkpoint.snapshot_failed".into(),
                            details,
                        })
                        .await
                    {
                        tracing::warn!(
                            run_id = run_id.as_str(),
                            error = %emit_error,
                            "checkpoint snapshot_failed diagnostic emit failed"
                        );
                    }
                }
            }
        }
        if any {
            out.push(WriteCheckpoint {
                checkpoint_id: CheckpointId::from(format!(
                    "{}/{}",
                    run_id.as_str(),
                    call.tool_call_id.as_str()
                )),
                artifacts,
            });
        }
    }
    out
}

/// 会话内一个可回滚点（run 级或 tool-call 级）。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CheckpointSummary {
    pub checkpoint_id: String,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub created_at_ms: u64,
    pub files: Vec<String>,
}

/// 一次回滚的结果。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RollbackOutcome {
    pub checkpoint_id: String,
    pub restored: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedCheckpoint {
    pub checkpoint_id: CheckpointId,
    pub run_id: String,
    pub tool_call_id: Option<String>,
}

pub(crate) async fn session_run_ids(
    core: &AppCore,
    session_id: &SessionId,
) -> Result<Vec<String>, AppError> {
    let events = core
        .store()?
        .replay_events(session_id, 1, usize::MAX)
        .await?;
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    for envelope in events {
        let run = envelope.run_id.as_str().to_string();
        if seen.insert(run.clone()) {
            order.push(run);
        }
        if let AgentEvent::CheckpointCreated { checkpoint_id, .. } = envelope.payload {
            if let Some((run_id, _)) = checkpoint_id.as_str().split_once('/') {
                if seen.insert(run_id.to_string()) {
                    order.push(run_id.to_string());
                }
            }
        }
    }
    Ok(order)
}

pub(crate) fn run_checkpoints(
    service: &CheckpointService,
    run_ids: &[String],
) -> Vec<RunCheckpoint> {
    run_ids
        .iter()
        .filter_map(|run_id| service.list_changes(run_id))
        .collect()
}

pub(crate) fn summaries_from_runs(runs: &[RunCheckpoint]) -> Vec<CheckpointSummary> {
    let mut out = Vec::new();
    for run in runs {
        for change in &run.changes {
            let files: Vec<String> = change
                .files
                .iter()
                .map(|file| file.relative_path.clone())
                .collect();
            out.push(CheckpointSummary {
                checkpoint_id: format!("{}/{}", run.run_id, change.tool_call_id),
                run_id: run.run_id.clone(),
                tool_call_id: Some(change.tool_call_id.clone()),
                created_at_ms: run.created_at_ms,
                files,
            });
        }
        let files: Vec<String> = run
            .changes
            .iter()
            .flat_map(|change| change.files.iter().map(|file| file.relative_path.clone()))
            .collect();
        if !files.is_empty() || !run.changes.is_empty() {
            out.push(CheckpointSummary {
                checkpoint_id: run.run_id.clone(),
                run_id: run.run_id.clone(),
                tool_call_id: None,
                created_at_ms: run.created_at_ms,
                files,
            });
        }
    }
    out
}

pub(crate) fn session_changed_paths(runs: &[RunCheckpoint]) -> BTreeSet<String> {
    runs.iter()
        .flat_map(|run| {
            run.changes.iter().flat_map(|change| {
                change
                    .files
                    .iter()
                    .map(|file| file.relative_path.replace('\\', "/"))
            })
        })
        .collect()
}

pub(crate) fn first_snapshots(runs: &[RunCheckpoint]) -> Vec<FileSnapshot> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for run in runs {
        for change in &run.changes {
            for file in &change.files {
                let key = file.relative_path.replace('\\', "/");
                if seen.insert(key) {
                    out.push(file.clone());
                }
            }
        }
    }
    out
}

pub(crate) fn resolve_spec(
    listed: &[CheckpointSummary],
    spec: &str,
) -> Result<ResolvedCheckpoint, AppError> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(AppError::CheckpointNotFound(spec.into()));
    }
    if let Some(hit) = listed.iter().find(|item| item.checkpoint_id == spec) {
        return Ok(resolved_from(hit));
    }
    let by_call: Vec<_> = listed
        .iter()
        .filter(|item| item.tool_call_id.as_deref() == Some(spec))
        .collect();
    match by_call.as_slice() {
        [only] => return Ok(resolved_from(only)),
        [] => {}
        many => {
            return Err(AppError::AmbiguousCheckpoint {
                prefix: spec.into(),
                matches: many
                    .iter()
                    .map(|item| item.checkpoint_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
    }
    let prefixes: Vec<_> = listed
        .iter()
        .filter(|item| item.checkpoint_id.starts_with(spec))
        .collect();
    match prefixes.as_slice() {
        [only] => Ok(resolved_from(only)),
        [] => Err(AppError::CheckpointNotFound(spec.into())),
        many => Err(AppError::AmbiguousCheckpoint {
            prefix: spec.into(),
            matches: many
                .iter()
                .map(|item| item.checkpoint_id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

fn resolved_from(summary: &CheckpointSummary) -> ResolvedCheckpoint {
    ResolvedCheckpoint {
        checkpoint_id: CheckpointId::from(summary.checkpoint_id.as_str()),
        run_id: summary.run_id.clone(),
        tool_call_id: summary.tool_call_id.clone(),
    }
}

pub(crate) async fn perform_rollback(
    service: &CheckpointService,
    resolved: &ResolvedCheckpoint,
) -> Result<Vec<FileSnapshot>, AppError> {
    let restored = if let Some(tool_call_id) = &resolved.tool_call_id {
        service
            .rollback_tool_call(&resolved.run_id, tool_call_id)
            .await?
    } else {
        service.rollback_run(&resolved.run_id).await?
    };
    Ok(restored)
}

pub(crate) async fn persist_rolled_back(
    core: &AppCore,
    session_id: &SessionId,
    resolved: &ResolvedCheckpoint,
) -> Result<(), AppError> {
    let run_id = RunId::from(resolved.run_id.as_str());
    let mut sequence = core.next_sequence(session_id).await?;
    core.append_payload(
        session_id,
        &run_id,
        &mut sequence,
        AgentEvent::CheckpointRolledBack {
            checkpoint_id: resolved.checkpoint_id.clone(),
        },
    )
    .await
    .map(|_| ())
}
