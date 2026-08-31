use crate::gui_server::{GuiHost, GuiHostError};
use pawork_protocol::{AppQuery, AppResponse};
use serde_json::{json, Value};

use super::super::{session_tree_entry, GuiHostAdapter};

pub(crate) async fn workspace_list(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::WorkspaceList = query else {
        unreachable!("workspace_list handler receives WorkspaceList")
    };
    let core = adapter.core.read().await;
    let entries: Vec<Value> = core
        .registered_workspaces()
        .into_iter()
        .map(|record| {
            json!({
                "id": record.workspace_id.as_str(),
                "name": record.name,
                "trusted": core.workspace_trusted(),
                "roots": [{ "path": record.root_path.display().to_string() }],
            })
        })
        .collect();
    Ok(AppResponse::Data(Value::Array(entries)))
}

pub(crate) async fn session_get(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::SessionGet {
        session_id,
        timeline_after_sequence,
        timeline_limit,
    } = query
    else {
        unreachable!("session_get handler receives SessionGet")
    };
    let record = adapter
        .core
        .read()
        .await
        .get_session(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let workspace_id = adapter
        .core
        .read()
        .await
        .session_workspace_for_record(record.session_id.as_str());
    let mut data = session_tree_entry(&record, workspace_id);
    if timeline_after_sequence.is_some() || timeline_limit.is_some() {
        let page = adapter
            .timeline(session_id, *timeline_after_sequence, *timeline_limit)
            .await?;
        data["timeline_page"] = serde_json::to_value(page)
            .map_err(|error| GuiHostAdapter::host_error("internal", error.to_string()))?;
    }
    Ok(AppResponse::Data(data))
}

pub(crate) async fn model_list(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::ModelList { provider_id } = query else {
        unreachable!("model_list handler receives ModelList")
    };
    // 与 `pawork models` 同一聚合目录，供 Desktop 切换已配置
    // provider/model；单通道 `model_catalog` 只含当前宿主。
    let catalog = adapter.core.read().await.models_overview().await;
    let entries: Vec<_> = catalog
        .iter()
        .filter(|entry| {
            provider_id
                .as_ref()
                .map(|id| id.as_str() == entry.provider.as_str())
                .unwrap_or(true)
        })
        .map(|entry| {
            json!({
                "provider_id": entry.provider.as_str(),
                "id": entry.id.as_str(),
                "display_name": entry.display_name,
                "context_window_tokens": entry.context_window_tokens,
            })
        })
        .collect();
    Ok(AppResponse::Data(Value::Array(entries)))
}

pub(crate) async fn run_status(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::RunStatus { run_id } = query else {
        unreachable!("run_status handler receives RunStatus")
    };
    let state = if adapter.runs.contains(run_id) {
        "running"
    } else {
        "unknown"
    };
    Ok(AppResponse::Data(json!({
        "run_id": run_id.as_str(),
        "state": state,
    })))
}

pub(crate) async fn diff_list_files(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::DiffListFiles { workspace_id } = query else {
        unreachable!("diff_list_files handler receives DiffListFiles")
    };
    let core = adapter.core.read().await;
    match core.latest_session_for_workspace(workspace_id).await {
        Ok(Some(session)) => {
            let diff = core
                .session_diff(&session)
                .await
                .map_err(GuiHostAdapter::app_error)?;
            Ok(AppResponse::Data(json!({
                "session_id": session.as_str(),
                "files": diff.files.iter().map(|file| json!({
                    "path": file.path,
                    "status": file.status,
                    "additions": file.additions,
                    "deletions": file.deletions,
                    "binary": file.binary,
                })).collect::<Vec<_>>(),
                "git": diff.git.as_ref().map(|git| json!({
                    "branch": git.branch,
                    "work_dir": git.work_dir,
                    "dirty_files": git.dirty_files,
                })),
            })))
        }
        Ok(None) => Ok(AppResponse::Data(json!({ "files": [] }))),
        Err(error) => Err(GuiHostAdapter::app_error(error)),
    }
}

pub(crate) async fn quota_overview(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::QuotaOverview { query } = query else {
        unreachable!("quota_overview handler receives QuotaOverview")
    };
    let provider = query
        .provider_id
        .as_ref()
        .map(|id| id.as_str().to_string())
        .filter(|id| !id.is_empty());
    let session = None;
    let overview = adapter
        .core
        .read()
        .await
        .usage_overview(provider.as_deref(), session)
        .await
        .map_err(|error| GuiHostAdapter::host_error("quota", error.to_string()))?;
    Ok(AppResponse::Data(serde_json::to_value(overview).map_err(
        |error| GuiHostAdapter::host_error("internal", error.to_string()),
    )?))
}

pub(crate) async fn mcp_list(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::McpList = query else {
        unreachable!("mcp_list handler receives McpList")
    };
    let servers = adapter.core.read().await.mcp_list();
    let servers = serde_json::to_value(servers)
        .map_err(|error| GuiHostAdapter::host_error("internal", error.to_string()))?;
    Ok(AppResponse::Data(json!({ "servers": servers })))
}

pub(crate) async fn diff_get(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::DiffGet {
        workspace_id,
        path,
        cursor,
    } = query
    else {
        unreachable!("diff_get handler receives DiffGet")
    };
    let core = adapter.core.read().await;
    let session = match core.latest_session_for_workspace(workspace_id).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            return Ok(AppResponse::Data(json!({
                "path": path.as_str(),
                "files": [],
                "complete": true,
            })));
        }
        Err(error) => return Err(GuiHostAdapter::app_error(error)),
    };
    let diff = core
        .session_diff(&session)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let Some(file) = diff
        .files
        .iter()
        .find(|file| file.path == path.as_str())
        .cloned()
    else {
        return Ok(AppResponse::Data(json!({
            "session_id": session.as_str(),
            "path": path.as_str(),
            "files": [],
            "complete": true,
        })));
    };
    let page = cursor
        .as_deref()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    let paged = crate::paginate_diff(vec![file], page, 1);
    Ok(AppResponse::Data(json!({
        "session_id": session.as_str(),
        "path": path.as_str(),
        "page": paged.page,
        "total_files": paged.total_files,
        "files": paged.files,
        "complete": page >= paged.page && paged.files.is_empty() || paged.page * 1 >= paged.total_files,
    })))
}
