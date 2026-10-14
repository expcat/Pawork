//! CU-17（GUI API 1.27）：观测证据查询与 computer-use 目标接入。
//!
//! 观测读取只消费持久化事件账本（ToolExecutionCompleted 的 Base64
//! Image part），永不触发新截图或输入；目标状态只读登记事实。两个
//! 写命令（approve / revoke）是 Host 侧目标登记的唯一 GUI 入口，均以
//! 本会话的活动 run 归属 fail-closed。GUI 注册表里的 run 保持原路径；
//! CLI 与子 run 只在该会话投影状态恰为 `running`、且同一 `NativeHost`
//! 上对应 scope 仍开着时可见。绑定会话只用它自己的 workspace，未绑定
//! 会话只用 `ws-unbound`。

use pawork_computer_use::approval::{GrantKind, TargetIdentity};
use pawork_computer_use::target::{
    background_capability, AppFamily, AppIdentity, NativeActionKind, Scope,
};
use pawork_domain::{AgentEvent, ContentPart, ImageSource};
use pawork_gui_server::GuiHostError;
use pawork_protocol::{
    AppQuery, AppResponse, ComputerActionKind, ComputerAppFamily, ComputerAppIdentity,
    ComputerCapabilitySupport, ComputerGrantKind, ComputerTargetApproveParams,
    ComputerTargetCapability, ComputerTargetIdentity, ComputerTargetRevokeParams,
    ComputerTargetScope, ComputerTargetState, ObservationEvidenceEntry, ObservationEvidenceStatus,
    ObservationImageData, ObservationListData, DEFAULT_OBSERVATION_LIST_LIMIT,
    MAX_COMPUTER_TARGET_LIST_RESPONSE_BYTES, MAX_COMPUTER_TARGET_LIST_TARGETS,
    MAX_OBSERVATION_LIST_LIMIT, MAX_OBSERVATION_LIST_RESPONSE_BYTES, MAX_OBSERVATION_READ_BYTES,
};
use serde_json::Value;

use super::super::GuiHostAdapter;
use crate::services::extension::{base64_decode_range, base64_decoded_len, base64_encode};

fn unknown_run() -> GuiHostError {
    GuiHostAdapter::host_error(
        "unknown_run",
        "computer target commands require an active run of this session",
    )
}

/// 绑定会话返回它的 workspace。已存在但未绑定的会话只能是 `ws-unbound`。
/// 不存在的会话返回 `None`，列表因此保持空且 `workspace_id` 为 null。
async fn session_computer_workspace(
    adapter: &GuiHostAdapter,
    session_id: &pawork_domain::SessionId,
) -> Result<Option<String>, GuiHostError> {
    let core = adapter.core.read().await;
    if let Some(bound) = core.session_workspace_for_record(session_id.as_str()) {
        return Ok(Some(bound.as_str().to_string()));
    }
    let store = core
        .store()
        .map_err(|error| GuiHostAdapter::host_error("internal", error.to_string()))?;
    let store = store.clone();
    drop(core);
    match store.get_session(session_id).await {
        Ok(record) => Ok(Some(
            record
                .workspace_id
                .unwrap_or_else(|| "ws-unbound".to_string()),
        )),
        Err(pawork_storage::session::SessionStoreError::SessionNotFound(_)) => Ok(None),
        Err(error) => Err(GuiHostAdapter::host_error("internal", error.to_string())),
    }
}

/// 投影里状态恰为 `running`、且 scope 仍开着的 (workspace, run)。
/// 已完成的投影即使 scope 泄漏开着也不计入。
async fn projection_open_runs(
    adapter: &GuiHostAdapter,
    session_id: &pawork_domain::SessionId,
    workspace_id: &str,
) -> Result<Vec<(String, String)>, GuiHostError> {
    let store = {
        let core = adapter.core.read().await;
        core.store()
            .map_err(|error| GuiHostAdapter::host_error("internal", error.to_string()))?
            .clone()
    };
    let snapshot = store
        .projection_snapshot(session_id)
        .await
        .map_err(|error| GuiHostAdapter::host_error("internal", error.to_string()))?;
    let open = adapter.core.read().await.native.active_scopes();
    Ok(snapshot
        .runs
        .into_iter()
        .filter(|run| run.state == "running")
        .filter(|run| {
            open.iter().any(|scope| {
                scope.workspace_id == workspace_id && scope.run_id == run.run_id.as_str()
            })
        })
        .map(|run| (workspace_id.to_string(), run.run_id.as_str().to_string()))
        .collect())
}

fn require_api_1_27(envelope: &pawork_protocol::AppQueryEnvelope) -> Result<(), GuiHostError> {
    if envelope.api_version.minor < 27 {
        return Err(GuiHostAdapter::host_error(
            "unsupported",
            "observation and computer target APIs require GUI API 1.27",
        ));
    }
    Ok(())
}

fn require_command_api_1_27(
    envelope: &pawork_protocol::AppCommandEnvelope,
) -> Result<(), GuiHostError> {
    if envelope.api_version.minor < 27 {
        return Err(GuiHostAdapter::host_error(
            "unsupported",
            "computer target commands require GUI API 1.27",
        ));
    }
    Ok(())
}

/// 从事件 result.metadata.observation 读取像素尺寸（无则 null）。
fn observation_dimensions(metadata: &Value) -> (Option<u32>, Option<u32>) {
    let width = metadata
        .get("observation")
        .and_then(|observation| observation.get("image_width"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let height = metadata
        .get("observation")
        .and_then(|observation| observation.get("image_height"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    (width, height)
}

/// 单个事件内的观测图像条目（跳过非 Base64 的 Image part）。
fn evidence_entries(envelope: &pawork_domain::AgentEventEnvelope) -> Vec<ObservationEvidenceEntry> {
    let AgentEvent::ToolExecutionCompleted { result, .. } = &envelope.payload else {
        return Vec::new();
    };
    result
        .content
        .iter()
        .enumerate()
        .filter_map(|(part_index, part)| {
            let ContentPart::Image(image) = part else {
                return None;
            };
            let ImageSource::Base64(data) = &image.source else {
                return None;
            };
            let byte_length = u64::try_from(base64_decoded_len(data).ok()?).ok()?;
            let (image_width, image_height) = observation_dimensions(&result.metadata);
            Some(ObservationEvidenceEntry {
                sequence: envelope.sequence.0,
                event_id: envelope.event_id.as_str().to_string(),
                run_id: Some(envelope.run_id.as_str().to_string()),
                tool_call_id: result.tool_call_id.as_str().to_string(),
                tool_name: result.tool_name.clone(),
                part_index: part_index as u32,
                media_type: image.media_type.clone(),
                byte_length,
                image_width,
                image_height,
                captured_at_ms: envelope.timestamp.as_unix_millis(),
                status: ObservationEvidenceStatus::Historical,
            })
        })
        .collect()
}

/// 单页条目的序列化字节数（observation_list 预算口径）。
fn entry_serialized_bytes(entries: &[ObservationEvidenceEntry]) -> Result<usize, GuiHostError> {
    let mut total = 0usize;
    for entry in entries {
        let bytes = serde_json::to_vec(entry)
            .map_err(|error| GuiHostAdapter::host_error("internal", error.to_string()))?;
        total = total.saturating_add(bytes.len());
    }
    Ok(total)
}

pub(crate) async fn observation_list(
    adapter: &GuiHostAdapter,
    envelope: &pawork_protocol::AppQueryEnvelope,
) -> Result<AppResponse, GuiHostError> {
    require_api_1_27(envelope)?;
    let AppQuery::ObservationList {
        session_id,
        after_sequence,
        limit,
    } = &envelope.query
    else {
        unreachable!("observation_list handler receives ObservationList")
    };
    let limit = limit
        .unwrap_or(DEFAULT_OBSERVATION_LIST_LIMIT)
        .clamp(1, MAX_OBSERVATION_LIST_LIMIT) as usize;
    let core = adapter.core.read().await;
    let head = core
        .next_sequence(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?
        .saturating_sub(1);
    let from = after_sequence
        .map(|value| value.saturating_add(1))
        .unwrap_or(1);
    if from > head {
        let data = ObservationListData {
            session_id: session_id.clone(),
            entries: Vec::new(),
            next_sequence: None,
            head_sequence: head,
            complete: true,
        };
        return Ok(AppResponse::Data(serde_json::to_value(data).map_err(
            |error| GuiHostAdapter::host_error("internal", error.to_string()),
        )?));
    }
    let store = core.store().map_err(GuiHostAdapter::app_error)?;
    let active_branch = store
        .get_session(session_id)
        .await
        .map_err(|error| GuiHostAdapter::host_error("app_error", error.to_string()))?
        .active_branch;
    let envelopes = store
        .events_on_lineage(session_id, &active_branch, from, limit)
        .await
        .map_err(|error| GuiHostAdapter::host_error("app_error", error.to_string()))?;
    let mut entries = Vec::new();
    // 审查修复：单页条目序列化字节预算（帧上限防线）。按事件粒度累计
    // 与停止——游标按事件推进，停止页可从下一事件完整续读；页内首
    // 个条目事件单独超预算则显式拒绝（fail-closed，不能静默跳过）。
    let mut used_bytes = 0usize;
    let mut last_included: Option<u64> = None;
    let mut budget_stopped = false;
    for event in &envelopes {
        let event_entries = evidence_entries(event);
        let event_bytes = entry_serialized_bytes(&event_entries)?;
        if used_bytes.saturating_add(event_bytes) > MAX_OBSERVATION_LIST_RESPONSE_BYTES {
            if entries.is_empty() {
                return Err(GuiHostAdapter::host_error(
                    "response_too_large",
                    format!(
                        "observation event {} entries exceed the {} byte list budget",
                        event.sequence.0, MAX_OBSERVATION_LIST_RESPONSE_BYTES
                    ),
                ));
            }
            budget_stopped = true;
            break;
        }
        used_bytes += event_bytes;
        entries.extend(event_entries);
        last_included = Some(event.sequence.0);
    }
    // 分页游标与 timeline 同口径：按事件序推进，不按过滤后条目计数。
    let page_cursor = if budget_stopped {
        last_included
    } else {
        envelopes.last().map(|event| event.sequence.0)
    };
    let complete = !budget_stopped
        && ((envelopes.len() < limit) || page_cursor.is_some_and(|sequence| sequence >= head));
    let data = ObservationListData {
        session_id: session_id.clone(),
        entries,
        next_sequence: if complete { None } else { page_cursor },
        head_sequence: head,
        complete,
    };
    Ok(AppResponse::Data(serde_json::to_value(data).map_err(
        |error| GuiHostAdapter::host_error("internal", error.to_string()),
    )?))
}

pub(crate) async fn observation_image_read(
    adapter: &GuiHostAdapter,
    envelope: &pawork_protocol::AppQueryEnvelope,
) -> Result<AppResponse, GuiHostError> {
    require_api_1_27(envelope)?;
    let AppQuery::ObservationImageRead {
        session_id,
        sequence,
        part_index,
        offset,
        limit,
    } = &envelope.query
    else {
        unreachable!("observation_image_read handler receives ObservationImageRead")
    };
    if *limit == 0 || *limit > MAX_OBSERVATION_READ_BYTES {
        return Err(GuiHostAdapter::host_error(
            "invalid_query",
            format!(
                "observation read limit must be in 1..={MAX_OBSERVATION_READ_BYTES} decoded bytes"
            ),
        ));
    }
    let core = adapter.core.read().await;
    let head = core
        .next_sequence(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?
        .saturating_sub(1);
    if *sequence == 0 || *sequence > head {
        return Err(not_found(*sequence));
    }
    let store = core.store().map_err(GuiHostAdapter::app_error)?;
    let active_branch = store
        .get_session(session_id)
        .await
        .map_err(|error| GuiHostAdapter::host_error("app_error", error.to_string()))?
        .active_branch;
    // lineage 精确定位：返回事件的 sequence 必须等于请求值（fork 支线上
    // 同号事件不可读）。
    let event = store
        .events_on_lineage(session_id, &active_branch, *sequence, 1)
        .await
        .map_err(|error| GuiHostAdapter::host_error("app_error", error.to_string()))?
        .into_iter()
        .next()
        .filter(|event| event.sequence.0 == *sequence)
        .ok_or_else(|| not_found(*sequence))?;
    let AgentEvent::ToolExecutionCompleted { result, .. } = &event.payload else {
        return Err(not_found(*sequence));
    };
    let image = result
        .content
        .get(usize::try_from(*part_index).unwrap_or(usize::MAX))
        .and_then(|part| match part {
            ContentPart::Image(image) => Some(image),
            _ => None,
        })
        .ok_or_else(|| not_found(*sequence))?;
    let ImageSource::Base64(encoded) = &image.source else {
        return Err(GuiHostAdapter::host_error(
            "not_found",
            format!("observation part {part_index} of event {sequence} is not inlined"),
        ));
    };
    // 审查修复：长度经 base64_decoded_len 整串校验（零分配）、切片只
    // 解码覆盖 quad，不整图解码。
    let byte_length = u64::try_from(base64_decoded_len(encoded).map_err(|error| {
        GuiHostAdapter::host_error(
            "app_error",
            format!("observation image of event {sequence} is not valid base64: {error}"),
        )
    })?)
    .map_err(|_| {
        GuiHostAdapter::host_error("app_error", "observation image exceeds addressable size")
    })?;
    if *offset >= byte_length {
        return Err(GuiHostAdapter::host_error(
            "invalid_query",
            format!("observation read offset {offset} is beyond the {byte_length} decoded bytes"),
        ));
    }
    let end = (*offset + *limit).min(byte_length);
    let slice = base64_decode_range(encoded, *offset as usize, (end - *offset) as usize).map_err(
        |error| {
            GuiHostAdapter::host_error(
                "app_error",
                format!("observation image of event {sequence} failed range decode: {error}"),
            )
        },
    )?;
    let (image_width, image_height) = observation_dimensions(&result.metadata);
    let data = ObservationImageData {
        session_id: session_id.clone(),
        sequence: *sequence,
        event_id: event.event_id.as_str().to_string(),
        part_index: *part_index,
        media_type: image.media_type.clone(),
        byte_length,
        offset: *offset,
        data: base64_encode(&slice),
        eof: end >= byte_length,
        captured_at_ms: event.timestamp.as_unix_millis(),
        image_width,
        image_height,
        status: ObservationEvidenceStatus::Historical,
    };
    Ok(AppResponse::Data(serde_json::to_value(data).map_err(
        |error| GuiHostAdapter::host_error("internal", error.to_string()),
    )?))
}

fn not_found(sequence: u64) -> GuiHostError {
    GuiHostAdapter::host_error(
        "not_found",
        format!("observation event {sequence} is not on the session's active lineage"),
    )
}

fn wire_app_identity(app: &AppIdentity) -> ComputerAppIdentity {
    ComputerAppIdentity {
        bundle_id: app.bundle_id.clone(),
        family: match app.family {
            AppFamily::Appkit => ComputerAppFamily::Appkit,
            AppFamily::Chromium => ComputerAppFamily::Chromium,
            AppFamily::Browser => ComputerAppFamily::Browser,
        },
    }
}

fn native_app_identity(app: &ComputerAppIdentity) -> AppIdentity {
    AppIdentity {
        bundle_id: app.bundle_id.clone(),
        family: match app.family {
            ComputerAppFamily::Appkit => AppFamily::Appkit,
            ComputerAppFamily::Chromium => AppFamily::Chromium,
            ComputerAppFamily::Browser => AppFamily::Browser,
        },
    }
}

fn wire_target(target: &TargetIdentity) -> ComputerTargetIdentity {
    match target {
        TargetIdentity::Application { app } => ComputerTargetIdentity::Application {
            app: wire_app_identity(app),
        },
        TargetIdentity::Website { browser, origin } => ComputerTargetIdentity::Website {
            browser: wire_app_identity(browser),
            origin: origin.clone(),
        },
    }
}

/// wire → 契约：Website 走构造器重新归一化 origin，再整体 validate。
fn native_target(target: &ComputerTargetIdentity) -> Result<TargetIdentity, GuiHostError> {
    let native = match target {
        ComputerTargetIdentity::Application { app } => {
            TargetIdentity::application(native_app_identity(app))
        }
        ComputerTargetIdentity::Website { browser, origin } => {
            TargetIdentity::website(native_app_identity(browser), origin)
                .map_err(|error| GuiHostAdapter::host_error("invalid_query", error.to_string()))?
        }
    };
    native
        .validate()
        .map_err(|error| GuiHostAdapter::host_error("invalid_query", error.to_string()))?;
    Ok(native)
}

fn target_capabilities(family: AppFamily, bundle_id: &str) -> Vec<ComputerTargetCapability> {
    const ACTIONS: [NativeActionKind; 10] = [
        NativeActionKind::CaptureWindow,
        NativeActionKind::AxRead,
        NativeActionKind::AxSemanticWrite,
        NativeActionKind::AxPress,
        NativeActionKind::TargetedKeys,
        NativeActionKind::MenuShortcut,
        NativeActionKind::MenubarPress,
        NativeActionKind::Pointer,
        NativeActionKind::WindowMove,
        NativeActionKind::WindowResize,
    ];
    // Guest surfaces reuse a family but gate actions by their own proof; the
    // rows must mirror each surface's `admit_action`, never the macOS matrix.
    let guest_proved: Option<&[NativeActionKind]> =
        if bundle_id == pawork_computer_use::guest::FIREFOX_BUNDLE_ID {
            Some(&[
                NativeActionKind::CaptureWindow,
                NativeActionKind::AxRead,
                NativeActionKind::AxPress,
            ])
        } else if bundle_id.starts_with(pawork_computer_use::guest::WAYLAND_BUNDLE_PREFIX) {
            Some(&[
                NativeActionKind::CaptureWindow,
                NativeActionKind::Pointer,
                NativeActionKind::TargetedKeys,
            ])
        } else {
            None
        };
    ACTIONS
        .into_iter()
        .map(|action| {
            let support = |minimized: bool| {
                if let Some(proved) = guest_proved {
                    if !minimized && proved.contains(&action) {
                        ComputerCapabilitySupport::Supported
                    } else {
                        ComputerCapabilitySupport::Unsupported
                    }
                } else {
                    match background_capability(family, action, minimized) {
                        pawork_computer_use::target::Capability::Supported => {
                            ComputerCapabilitySupport::Supported
                        }
                        pawork_computer_use::target::Capability::Unsupported => {
                            ComputerCapabilitySupport::Unsupported
                        }
                        pawork_computer_use::target::Capability::Unverified => {
                            ComputerCapabilitySupport::Unverified
                        }
                    }
                }
            };
            ComputerTargetCapability {
                action: match action {
                    NativeActionKind::CaptureWindow => ComputerActionKind::CaptureWindow,
                    NativeActionKind::AxRead => ComputerActionKind::AxRead,
                    NativeActionKind::AxSemanticWrite => ComputerActionKind::AxSemanticWrite,
                    NativeActionKind::AxPress => ComputerActionKind::AxPress,
                    NativeActionKind::TargetedKeys => ComputerActionKind::TargetedKeys,
                    NativeActionKind::MenuShortcut => ComputerActionKind::MenuShortcut,
                    NativeActionKind::MenubarPress => ComputerActionKind::MenubarPress,
                    NativeActionKind::Pointer => ComputerActionKind::Pointer,
                    NativeActionKind::WindowMove => ComputerActionKind::WindowMove,
                    NativeActionKind::WindowResize => ComputerActionKind::WindowResize,
                },
                background: support(false),
                minimized: support(true),
            }
        })
        .collect()
}

fn wire_scope(scope: &Scope) -> ComputerTargetScope {
    ComputerTargetScope {
        workspace_id: scope.workspace_id.clone(),
        run_id: scope.run_id.clone(),
    }
}

pub(crate) async fn computer_target_list(
    adapter: &GuiHostAdapter,
    envelope: &pawork_protocol::AppQueryEnvelope,
) -> Result<AppResponse, GuiHostError> {
    require_api_1_27(envelope)?;
    let AppQuery::ComputerTargetList { session_id } = &envelope.query else {
        unreachable!("computer_target_list handler receives ComputerTargetList")
    };
    let workspace_id = session_computer_workspace(adapter, session_id).await?;
    // GUI 注册表优先。CLI / 子 run 没有这份登记，改看本会话投影。
    let mut active: Vec<(String, String)> = adapter
        .runs
        .active()
        .into_iter()
        .filter(|run| run.session_id == *session_id)
        .map(|run| {
            (
                run.workspace_id.as_str().to_string(),
                run.run_id.as_str().to_string(),
            )
        })
        .collect();
    if let Some(workspace) = &workspace_id {
        for pair in projection_open_runs(adapter, session_id, workspace).await? {
            if !active.iter().any(|(_, run_id)| run_id == &pair.1) {
                active.push(pair);
            }
        }
    }
    let targets = if active.is_empty() {
        Vec::new()
    } else {
        adapter
            .core
            .read()
            .await
            .target_states()
            .into_iter()
            .filter(|state| {
                active.iter().any(|(workspace, run)| {
                    state.scope.workspace_id == *workspace && state.scope.run_id == *run
                })
            })
            .map(|state| ComputerTargetState {
                target: wire_target(&state.target),
                scope: wire_scope(&state.scope),
                grant: state.grant.map(|kind| match kind {
                    GrantKind::Once => ComputerGrantKind::Once,
                    GrantKind::ForRun => ComputerGrantKind::ForRun,
                }),
                bound_window: state.window.map(|handle| handle.as_str().to_string()),
                occupied_by: state.occupied_by.as_ref().map(wire_scope),
                dispatch_paused: state.dispatch_paused,
                capabilities: target_capabilities(
                    state.target.app().family,
                    state.target.app().bundle_id.as_str(),
                ),
            })
            .collect()
    };
    // 审查修复：目标数上限——正常会话个位数，超限说明登记泄漏或攻击，
    // 拒绝整页而不是产出超限帧。
    if targets.len() > MAX_COMPUTER_TARGET_LIST_TARGETS {
        return Err(GuiHostAdapter::host_error(
            "response_too_large",
            format!(
                "computer target list exceeds {} entries",
                MAX_COMPUTER_TARGET_LIST_TARGETS
            ),
        ));
    }
    let data = pawork_protocol::ComputerTargetListData {
        session_id: session_id.clone(),
        workspace_id: workspace_id.map(|id| id.as_str().to_string()),
        targets,
    };
    // 审查修复：条目数上限不约束单条大小（bundle_id 无长度上限），
    // 完整响应再做序列化字节预算，超限整页拒绝（帧上限防线）。
    let payload = serde_json::to_vec(&data)
        .map_err(|error| GuiHostAdapter::host_error("internal", error.to_string()))?;
    if payload.len() > MAX_COMPUTER_TARGET_LIST_RESPONSE_BYTES {
        return Err(GuiHostAdapter::host_error(
            "response_too_large",
            format!(
                "computer target list response exceeds {} bytes",
                MAX_COMPUTER_TARGET_LIST_RESPONSE_BYTES
            ),
        ));
    }
    Ok(AppResponse::Data(serde_json::to_value(data).map_err(
        |error| GuiHostAdapter::host_error("internal", error.to_string()),
    )?))
}

/// run 归属校验：approve / revoke 共用；未知或跨 session 一律 unknown_run
/// （fail-closed，不区分「不存在」与「不归属」）。
async fn require_session_run(
    adapter: &GuiHostAdapter,
    session_id: &pawork_domain::SessionId,
    run_id: &pawork_domain::RunId,
) -> Result<(String, String), GuiHostError> {
    if let Some(run) = adapter
        .runs
        .active()
        .into_iter()
        .find(|run| run.run_id == *run_id && run.session_id == *session_id)
    {
        return Ok((
            run.workspace_id.as_str().to_string(),
            run.run_id.as_str().to_string(),
        ));
    }
    let Some(workspace) = session_computer_workspace(adapter, session_id).await? else {
        return Err(unknown_run());
    };
    let visible = projection_open_runs(adapter, session_id, &workspace)
        .await?
        .into_iter()
        .any(|(_, open_run)| open_run == run_id.as_str());
    if !visible {
        return Err(unknown_run());
    }
    Ok((workspace, run_id.as_str().to_string()))
}

pub(crate) async fn computer_target_approve(
    adapter: &GuiHostAdapter,
    envelope: &pawork_protocol::AppCommandEnvelope,
    command: &pawork_protocol::AppCommand,
) -> Result<AppResponse, GuiHostError> {
    require_command_api_1_27(envelope)?;
    let pawork_protocol::AppCommand::ComputerTargetApprove(ComputerTargetApproveParams {
        session_id,
        run_id,
        target,
        kind,
    }) = command
    else {
        unreachable!("computer_target_approve handler receives ComputerTargetApprove")
    };
    let (workspace_id, run_id) = require_session_run(adapter, session_id, run_id).await?;
    let native = native_target(target)?;
    let native_kind = match kind {
        ComputerGrantKind::Once => GrantKind::Once,
        ComputerGrantKind::ForRun => GrantKind::ForRun,
    };
    let core = adapter.core.read().await;
    core.grant_target(&Scope::new(workspace_id, run_id), &native, native_kind)
        .map_err(|error| match error {
            pawork_computer_use::target::TargetError::ScopeClosed => {
                GuiHostAdapter::host_error("unknown_run", "run already reached a terminal state")
            }
            pawork_computer_use::target::TargetError::ForbiddenTarget => {
                GuiHostAdapter::host_error(
                    "forbidden_target",
                    "protected applications can never be authorized",
                )
            }
            other => GuiHostAdapter::host_error("invalid_query", other.to_string()),
        })?;
    // 回执回显生效 target（Website 已归一化 origin）与授权寿命。
    let receipt = serde_json::json!({
        "target": wire_target(&native),
        "kind": kind,
    });
    Ok(AppResponse::Data(receipt))
}

pub(crate) async fn computer_target_revoke(
    adapter: &GuiHostAdapter,
    envelope: &pawork_protocol::AppCommandEnvelope,
    command: &pawork_protocol::AppCommand,
) -> Result<AppResponse, GuiHostError> {
    require_command_api_1_27(envelope)?;
    let pawork_protocol::AppCommand::ComputerTargetRevoke(ComputerTargetRevokeParams {
        session_id,
        run_id,
        target,
    }) = command
    else {
        unreachable!("computer_target_revoke handler receives ComputerTargetRevoke")
    };
    let (workspace_id, run_id) = require_session_run(adapter, session_id, run_id).await?;
    let native = native_target(target)?;
    let core = adapter.core.read().await;
    core.revoke_target(&Scope::new(workspace_id, run_id), &native);
    let receipt = serde_json::json!({ "target": wire_target(&native) });
    Ok(AppResponse::Data(receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ComputerActionKind::*;
    use ComputerCapabilitySupport::{Supported, Unsupported, Unverified};

    fn matrix(
        family: AppFamily,
        bundle_id: &str,
    ) -> Vec<(
        ComputerActionKind,
        ComputerCapabilitySupport,
        ComputerCapabilitySupport,
    )> {
        target_capabilities(family, bundle_id)
            .into_iter()
            .map(|row| (row.action, row.background, row.minimized))
            .collect()
    }

    fn guest_rows(proved: &[ComputerActionKind]) -> Vec<(ComputerActionKind, bool)> {
        [
            CaptureWindow,
            AxRead,
            AxSemanticWrite,
            AxPress,
            TargetedKeys,
            MenuShortcut,
            MenubarPress,
            Pointer,
            WindowMove,
            WindowResize,
        ]
        .into_iter()
        .map(|action| (action, proved.contains(&action)))
        .collect()
    }

    fn assert_guest(bundle_id: &str, proved: &[ComputerActionKind]) {
        let rows = matrix(AppFamily::Browser, bundle_id);
        let expected = guest_rows(proved);
        assert_eq!(rows.len(), 10, "{bundle_id}: ten-action matrix");
        for ((action, background, minimized), (want_action, want)) in rows.into_iter().zip(expected)
        {
            assert_eq!(action, want_action, "{bundle_id}: row order");
            let want = if want { Supported } else { Unsupported };
            assert_eq!(background, want, "{bundle_id}: {action:?} background");
            assert_eq!(minimized, Unsupported, "{bundle_id}: {action:?} minimized");
        }
    }

    #[test]
    fn wayland_guest_matrix_mirrors_seat_surface_gate() {
        for bundle_id in ["guest.wayland.foot", "guest.wayland.org.gnome.texteditor"] {
            assert!(bundle_id.starts_with(pawork_computer_use::guest::WAYLAND_BUNDLE_PREFIX));
            assert_guest(bundle_id, &[CaptureWindow, Pointer, TargetedKeys]);
        }
    }

    #[test]
    fn firefox_guest_and_browser_family_matrices_do_not_regress() {
        assert_guest(
            pawork_computer_use::guest::FIREFOX_BUNDLE_ID,
            &[CaptureWindow, AxRead, AxPress],
        );
        // Only the exact prefix selects the seat surface.
        for bundle_id in [
            "org.mozilla.firefox",
            "guest.waylandfoot",
            "guest.browser.wayland",
        ] {
            assert_eq!(
                matrix(AppFamily::Browser, bundle_id),
                vec![
                    (CaptureWindow, Supported, Supported),
                    (AxRead, Supported, Supported),
                    (AxSemanticWrite, Unverified, Unverified),
                    (AxPress, Supported, Supported),
                    (TargetedKeys, Unsupported, Unsupported),
                    (MenuShortcut, Unverified, Unverified),
                    (MenubarPress, Unverified, Unverified),
                    (Pointer, Unsupported, Unsupported),
                    (WindowMove, Unsupported, Unsupported),
                    (WindowResize, Supported, Unverified),
                ],
                "{bundle_id}: browser family path",
            );
        }
    }
}
