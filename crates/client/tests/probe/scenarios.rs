//! `--self-test` 场景集：进程内 MemoryTransport + GuiHostAdapter。

use std::sync::Arc;
use std::time::{Duration, Instant};

use pawork_app::{gui_server::GuiHost, ApprovalMode};
use pawork_client::{ClientConfig, ClientError, GuiClient, ResumeDisposition};
use pawork_domain::{
    ArtifactId, CommandId, EventId, ProviderId, RunId, SessionId, TenantId, Timestamp,
};
use pawork_protocol::{
    decode_server_frame, encode_server_frame, mask_credential_hint, ApiVersion, AppCommand,
    AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery, AppResponse, ArtifactChunk,
    EventSource, EventStream, GlobalSequence, ProtocolErrorCode, QuotaAdapterKind, QuotaAlert,
    QuotaAlertKind, QuotaAlertSeverity, QuotaFailureView, QuotaUnit, QuotaWindow, RunState,
    ServerFrame, SnapshotSectionKind, API_VERSION,
};
use pawork_testkit::MockScript;
use serde_json::{json, Value};

use crate::harness::{self, Harness};

const TIMEOUT: Duration = Duration::from_secs(10);

/// 运行全部场景，返回退出码（0 = 全部 PASS，1 = 存在 FAIL）。
pub async fn run_all() -> i32 {
    let scenarios = [
        "session-events",
        "snapshot-reconnect",
        "resume-snapshot-fallback",
        "three-gui-sync",
        "command-idempotency",
        "terminal-gate",
        "artifact-chunks",
        "version-reject",
        "disconnect-keeps-run",
        "quota-alert-roundtrip",
        "diff-list-files",
        "diff-get",
        "mcp-list",
    ];
    let mut failed = 0;
    for name in scenarios {
        match run_scenario(name).await {
            Ok(()) => println!("PASS {name}"),
            Err(error) => {
                println!("FAIL {name}: {error}");
                failed += 1;
            }
        }
    }
    if failed == 0 {
        0
    } else {
        1
    }
}

async fn run_scenario(name: &str) -> Result<(), String> {
    match name {
        "session-events" => session_events().await,
        "snapshot-reconnect" => snapshot_reconnect().await,
        "resume-snapshot-fallback" => resume_snapshot_fallback().await,
        "three-gui-sync" => three_gui_sync().await,
        "command-idempotency" => command_idempotency().await,
        "terminal-gate" => terminal_gate().await,
        "artifact-chunks" => artifact_chunks().await,
        "version-reject" => version_reject().await,
        "disconnect-keeps-run" => disconnect_keeps_run().await,
        "quota-alert-roundtrip" => quota_alert_roundtrip(),
        "diff-list-files" => diff_list_files().await,
        "diff-get" => diff_get().await,
        "mcp-list" => mcp_list().await,
        other => Err(format!("unknown scenario {other}")),
    }
}

async fn session_events() -> Result<(), String> {
    let mut harness = Harness::new("session-events", streaming_script()).await;
    let client = harness
        .connect_gui("session-events", "session-events")
        .await?;
    if client.initial_snapshot().is_none() {
        return Err("握手后应消费首帧 Snapshot".into());
    }
    let session_id = prepare_session_via_client(&client).await?;
    client
        .subscribe_all()
        .await
        .map_err(|error| format!("subscribe: {error}"))?;

    let run = client
        .command(
            AppCommand::RunStart {
                session_id,
                user_message: "hello from self-test".into(),
                model: None,
                provider: None,
                profile: None,
            },
            harness::gui_source(&client),
            harness::local_user(),
        )
        .await
        .map_err(|error| format!("RunStart: {error}"))?;
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = &run.response
    else {
        return Err(format!("RunStart 响应缺少 run id，got {:?}", run.response));
    };
    let run_id = run_id.clone();

    let (done, events) = recv_until(&client, |event| {
        run_state(event, &run_id) == Some(RunState::Completed)
    })
    .await?;
    if !done {
        return Err("未收到 Run 的 Completed 事件".into());
    }
    if !events
        .iter()
        .any(|event| matches!(&event.payload, AppEvent::AssistantDelta { .. }))
    {
        return Err("未收到流式增量事件".into());
    }
    Ok(())
}

async fn snapshot_reconnect() -> Result<(), String> {
    let mut harness = Harness::new("reconnect", waiting_script()).await;
    let session_id = harness.prepare_session("reconnect").await?;
    let client = harness.connect_gui("reconnect", "first").await?;
    client
        .subscribe_all()
        .await
        .map_err(|error| format!("subscribe: {error}"))?;
    let run_id = harness.start_run_cli(&session_id, "long run").await?;
    wait_until(|| harness.adapter.runs().contains(&run_id)).await?;
    let (done, events) = recv_until(&client, |event| run_state(event, &run_id).is_some()).await?;
    let last_sequence = if done {
        events
            .iter()
            .map(|event| event.global_sequence.0)
            .max()
            .unwrap_or(0)
    } else {
        0
    };

    let snapshot = client
        .snapshot()
        .await
        .map_err(|error| format!("snapshot: {error}"))?;
    let active_runs = snapshot
        .sections
        .iter()
        .find(|section| section.kind == SnapshotSectionKind::ActiveRuns)
        .ok_or("缺少 ActiveRuns section")?;
    let runs = active_runs
        .data
        .as_ref()
        .and_then(Value::as_array)
        .ok_or("ActiveRuns data 非数组")?;
    if !runs
        .iter()
        .any(|run| run["run_id"] == json!(run_id.as_str()))
    {
        return Err("快照未重建 Run 的活跃状态".into());
    }

    client
        .ack(GlobalSequence(last_sequence))
        .await
        .map_err(|error| format!("ack: {error}"))?;
    client
        .close()
        .await
        .map_err(|error| format!("close: {error}"))?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    if !harness.adapter.runs().contains(&run_id) {
        return Err("GUI 断线不得取消 Run".into());
    }

    harness.cancel_run_cli(&run_id).await?;
    wait_until(|| !harness.adapter.runs().contains(&run_id)).await?;

    let reconnected = harness.connect_gui("reconnect", "second").await?;
    let outcome = reconnected
        .resume(GlobalSequence(last_sequence))
        .await
        .map_err(|error| format!("resume: {error}"))?;
    match &outcome.disposition {
        ResumeDisposition::Replay { .. }
        | ResumeDisposition::SnapshotRequired { .. }
        | ResumeDisposition::UpToDate { .. } => {}
    }
    let _ = reconnected
        .heartbeat()
        .await
        .map_err(|error| format!("heartbeat: {error}"))?;
    Ok(())
}

async fn resume_snapshot_fallback() -> Result<(), String> {
    let mut harness = Harness::new("fallback", streaming_script()).await;
    let client = harness.connect_gui("fallback", "fallback").await?;
    let session_id = prepare_session_via_client(&client).await?;
    client
        .subscribe_all()
        .await
        .map_err(|error| format!("subscribe: {error}"))?;
    let run = client
        .command(
            AppCommand::RunStart {
                session_id,
                user_message: "fill resume log".into(),
                model: None,
                provider: None,
                profile: None,
            },
            harness::gui_source(&client),
            harness::local_user(),
        )
        .await
        .map_err(|error| format!("RunStart: {error}"))?;
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = &run.response
    else {
        return Err(format!("RunStart 响应缺少 run id，got {:?}", run.response));
    };
    let (done, events) = recv_until(&client, |event| {
        run_state(event, run_id) == Some(RunState::Completed)
    })
    .await?;
    if !done {
        return Err("应先产生可重放事件".into());
    }
    let last = events
        .iter()
        .map(|event| event.global_sequence.0)
        .max()
        .ok_or("events")?;
    let outcome = client
        .resume(GlobalSequence(last + 100))
        .await
        .map_err(|error| format!("resume ahead: {error}"))?;
    if !matches!(
        outcome.disposition,
        ResumeDisposition::SnapshotRequired { .. }
    ) {
        return Err(format!(
            "领先于服务端当前序列应 SnapshotRequired，got {:?}",
            outcome.disposition
        ));
    }
    Ok(())
}

async fn three_gui_sync() -> Result<(), String> {
    let mut harness = Harness::new("three-gui", streaming_script()).await;
    let session_id = harness.prepare_session("three-gui").await?;
    let gui_a = harness.connect_gui("three-gui", "a").await?;
    let gui_b = harness.connect_gui("three-gui", "b").await?;
    let gui_c = harness.connect_gui("three-gui", "c").await?;
    gui_a
        .subscribe_all()
        .await
        .map_err(|error| format!("subscribe a: {error}"))?;
    gui_b
        .subscribe_all()
        .await
        .map_err(|error| format!("subscribe b: {error}"))?;
    gui_c
        .subscribe_all()
        .await
        .map_err(|error| format!("subscribe c: {error}"))?;

    let run_id = harness.start_run_cli(&session_id, "cli run").await?;
    for (name, gui) in [("A", &gui_a), ("B", &gui_b), ("C", &gui_c)] {
        let (done, _) = recv_until(gui, |event| {
            run_state(event, &run_id) == Some(RunState::Completed)
        })
        .await?;
        if !done {
            return Err(format!("GUI {name} 未收到 CLI Run 的 Completed"));
        }
    }

    let mut cli_observer = harness.adapter.subscribe_events();
    let run = gui_a
        .command(
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: "run from gui a".into(),
                model: None,
                provider: None,
                profile: None,
            },
            harness::gui_source(&gui_a),
            harness::local_user(),
        )
        .await
        .map_err(|error| format!("gui a RunStart: {error}"))?;
    let AppResponse::Accepted {
        run_id: Some(gui_run_id),
        ..
    } = &run.response
    else {
        return Err(format!("RunStart 响应缺少 run id，got {:?}", run.response));
    };
    let gui_run_id = gui_run_id.clone();
    for (name, gui) in [("B", &gui_b), ("C", &gui_c)] {
        let (done, _) = recv_until(gui, |event| {
            run_state(event, &gui_run_id) == Some(RunState::Completed)
        })
        .await?;
        if !done {
            return Err(format!("GUI {name} 未收到 GUI A Run 的 Completed"));
        }
    }
    let deadline = Instant::now() + TIMEOUT;
    let mut cli_saw_completed = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            cli_observer.recv(),
        )
        .await
        {
            Ok(Ok(event)) => {
                if run_state(&event, &gui_run_id) == Some(RunState::Completed) {
                    cli_saw_completed = true;
                    break;
                }
            }
            _ => break,
        }
    }
    if !cli_saw_completed {
        return Err("CLI 观察者未收到 GUI A Run 的 Completed".into());
    }
    Ok(())
}

async fn command_idempotency() -> Result<(), String> {
    let mut harness = Harness::new("idempotency", streaming_script()).await;
    let client = harness.connect_gui("idempotency", "idempotency").await?;
    let envelope = AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("idem-session-1"),
        source: harness::gui_source(&client),
        identity: harness::local_user(),
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(7),
        command: AppCommand::SessionCreate {
            workspace_id: pawork_domain::WorkspaceId::from("ws-unbound"),
            title: Some("idempotent".into()),
        },
    };
    let first = client
        .command_envelope(envelope.clone())
        .await
        .map_err(|error| format!("首次执行: {error}"))?;
    let replayed = client
        .command_envelope(envelope)
        .await
        .map_err(|error| format!("重放: {error}"))?;
    if replayed.response != first.response {
        return Err(format!(
            "同 command_id 重放未返回相同响应: first={:?} replayed={:?}",
            first.response, replayed.response
        ));
    }
    Ok(())
}

async fn terminal_gate() -> Result<(), String> {
    let mut allow_harness = Harness::new("terminal-gate-allow", streaming_script()).await;
    let allow_client = allow_harness
        .connect_gui("terminal-gate-allow", "terminal-gate-allow")
        .await?;
    let allow_envelope = AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("terminal-gate-allow-1"),
        source: harness::gui_source(&allow_client),
        identity: harness::local_user(),
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(7),
        command: AppCommand::TerminalCreate {
            workspace_id: pawork_domain::WorkspaceId::from("ws-unbound"),
            working_directory: None,
        },
    };
    let allow = allow_client
        .command_envelope(allow_envelope)
        .await
        .map_err(|error| format!("放行路径: {error}"))?;
    let AppResponse::Data(value) = allow.response else {
        return Err(format!("放行路径应返回 Data，got {:?}", allow.response));
    };
    let terminal_session_id = value
        .get("terminal_session_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if terminal_session_id.is_empty() {
        return Err(format!(
            "放行路径 terminal_session_id 应非空，got {value:?}"
        ));
    }
    if value.get("sandboxed") != Some(&Value::Bool(false)) {
        return Err(format!("放行路径 sandboxed 应为 false，got {value:?}"));
    }
    if value.get("approval_mode").and_then(Value::as_str) != Some("ask_for_dangerous") {
        return Err(format!(
            "放行路径 approval_mode 应为 ask_for_dangerous，got {value:?}"
        ));
    }
    if value.get("policy").and_then(Value::as_str) != Some("allow_with_constraints") {
        return Err(format!(
            "放行路径 policy 应为 allow_with_constraints，got {value:?}"
        ));
    }
    let note = value
        .get("note")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !note.contains("policy 闸") {
        return Err(format!("放行路径 note 应含 policy 闸，got {value:?}"));
    }

    let mut deny_harness = Harness::new_with_approval(
        "terminal-gate-deny",
        streaming_script(),
        ApprovalMode::ReadOnly,
        true,
    )
    .await;
    let deny_client = deny_harness
        .connect_gui("terminal-gate-deny", "terminal-gate-deny")
        .await?;
    let deny_envelope = AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("terminal-gate-deny-1"),
        source: harness::gui_source(&deny_client),
        identity: harness::local_user(),
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(7),
        command: AppCommand::TerminalCreate {
            workspace_id: pawork_domain::WorkspaceId::from("ws-unbound"),
            working_directory: None,
        },
    };
    match deny_client.command_envelope(deny_envelope).await {
        Err(error) => {
            let text = error.to_string();
            if !text.contains("禁止创建终端") || !text.contains("fail-closed") {
                return Err(format!("拒绝路径文案不符，got {error}"));
            }
        }
        Ok(envelope) => {
            return Err(format!(
                "拒绝路径应为 ClientError，got {:?}",
                envelope.response
            ));
        }
    }
    Ok(())
}

async fn artifact_chunks() -> Result<(), String> {
    let mut harness = Harness::new("artifact", streaming_script()).await;
    let client = harness.connect_gui("artifact", "artifact").await?;
    let response = client
        .query(
            AppQuery::ArtifactRead {
                artifact_id: ArtifactId::from("art-missing"),
                offset: 0,
                limit: 1024,
            },
            harness::gui_source(&client),
            harness::local_user(),
        )
        .await;
    match response {
        Ok(envelope) => match envelope.response {
            AppResponse::Error(_) => {}
            other => return Err(format!("缺失 artifact 应为 Error，got {other:?}")),
        },
        Err(error) => {
            let text = error.to_string();
            if !error.is_request_not_found()
                && !text.contains("unsupported")
                && !text.contains("not part of")
                && !text.contains("not available")
            {
                return Err(format!("缺失 artifact 应 fail-closed，got {error}"));
            }
        }
    }

    let chunk = ArtifactChunk {
        request_id: "art-1".into(),
        artifact_id: ArtifactId::from("art-1"),
        offset: 0,
        data: vec![1, 2, 3, 4],
        eof: true,
    };
    let encoded = encode_server_frame(&ServerFrame::ArtifactChunk(chunk.clone()))
        .map_err(|error| format!("encode ArtifactChunk: {error}"))?;
    let decoded = decode_server_frame(&encoded).map_err(|error| format!("decode: {error}"))?;
    let ServerFrame::ArtifactChunk(decoded) = decoded else {
        return Err("应解码回 ArtifactChunk".into());
    };
    if decoded != chunk {
        return Err("ArtifactChunk 往返不一致".into());
    }
    Ok(())
}

async fn version_reject() -> Result<(), String> {
    let harness = Harness::new("version", streaming_script()).await;
    let listener = Arc::clone(&harness.listener);
    let accept = tokio::spawn(async move { listener.accept().await });
    let transport: Arc<dyn pawork_transport::GuiTransportClient> = harness.transport.clone();
    let config = ClientConfig {
        supported_api_versions: vec![ApiVersion { major: 2, minor: 0 }],
        ..ClientConfig::default()
    };
    let error = match GuiClient::connect_with_config(
        transport,
        harness.endpoint("version"),
        Harness::connect_options("version-reject"),
        None,
        config,
    )
    .await
    {
        Err(error) => error,
        Ok(_) => return Err("版本不兼容必须拒绝".into()),
    };
    if !error.is_incompatible_version() {
        return Err(format!("应报 IncompatibleVersion，got {error:?}"));
    }
    match error {
        ClientError::HandshakeRejected(protocol_error) => {
            if protocol_error.code != ProtocolErrorCode::IncompatibleVersion {
                return Err("错误码不是 IncompatibleVersion".into());
            }
        }
        other => return Err(format!("应 HandshakeRejected，got {other:?}")),
    }
    let _session = accept
        .await
        .map_err(|error| format!("accept task: {error}"))?
        .map_err(|error| format!("accept: {error}"))?;
    Ok(())
}

async fn disconnect_keeps_run() -> Result<(), String> {
    let mut harness = Harness::new("disconnect", waiting_script()).await;
    let session_id = harness.prepare_session("disconnect").await?;
    let client = harness.connect_gui("disconnect", "disconnect").await?;
    client
        .subscribe_all()
        .await
        .map_err(|error| format!("subscribe: {error}"))?;
    let run_id = harness
        .start_run_cli(&session_id, "survives disconnect")
        .await?;
    wait_until(|| harness.adapter.runs().contains(&run_id)).await?;
    client
        .close()
        .await
        .map_err(|error| format!("close: {error}"))?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    if !harness.adapter.runs().contains(&run_id) {
        return Err("GUI 断线不得取消 Run".into());
    }
    harness.cancel_run_cli(&run_id).await?;
    wait_until(|| !harness.adapter.runs().contains(&run_id)).await?;
    Ok(())
}

fn quota_alert_roundtrip() -> Result<(), String> {
    let alert = QuotaAlert {
        tenant_id: TenantId::new("local"),
        account_id: "local/default".into(),
        provider_id: ProviderId::from("openai"),
        model_id: None,
        window: QuotaWindow::Monthly,
        unit: QuotaUnit::Token,
        kind: Some(QuotaAlertKind::ReauthorizationRequired),
        severity: QuotaAlertSeverity::Warning,
        source: Some("ApiKeyApi:api.openai.com/v1/organization/usage".into()),
        message: "low balance".into(),
        snapshot: None,
        credential_hint: mask_credential_hint("sk-leak"),
    };
    let envelope = AppEventEnvelope {
        api_version: API_VERSION,
        instance_id: pawork_domain::CoreInstanceId::from("self-test-quota-alert"),
        event_id: EventId::from("quota-alert-1"),
        global_sequence: GlobalSequence(1),
        stream: EventStream::Global,
        stream_sequence: 1,
        timestamp: Timestamp::from_unix_millis(1),
        source: EventSource::Core,
        payload: AppEvent::QuotaAlert {
            alert: Box::new(alert),
        },
    };
    let encoded = encode_server_frame(&ServerFrame::Event(envelope))
        .map_err(|error| format!("encode ServerFrame: {error}"))?;
    let wire = String::from_utf8(encoded.clone()).map_err(|error| error.to_string())?;
    if !wire.contains("\"kind\":\"reauthorization_required\"")
        || !wire.contains("\"source\":\"ApiKeyApi:api.openai.com/v1/organization/usage\"")
    {
        return Err(format!("新事件线上 JSON 必须携带 kind/source: {wire}"));
    }
    if wire.contains("sk-leak") {
        return Err("线上 JSON 不得泄露 secret 原文".into());
    }

    let decoded = decode_server_frame(&encoded).map_err(|error| format!("decode: {error}"))?;
    let ServerFrame::Event(decoded) = decoded else {
        return Err("应解码回 Event 帧".into());
    };
    let AppEvent::QuotaAlert { alert } = &decoded.payload else {
        return Err("payload 应为 QuotaAlert".into());
    };
    if alert.kind != Some(QuotaAlertKind::ReauthorizationRequired)
        || alert.source.as_deref() != Some("ApiKeyApi:api.openai.com/v1/organization/usage")
    {
        return Err(format!("roundtrip 后 kind/source 丢失: {alert:?}"));
    }

    let mut legacy: Value = serde_json::from_slice(&encoded).map_err(|error| error.to_string())?;
    let alert_json = legacy
        .pointer_mut("/data/payload/data/alert")
        .and_then(Value::as_object_mut)
        .ok_or("legacy JSON 定位 alert 失败")?;
    for key in ["kind", "source"] {
        if alert_json.remove(key).is_none() {
            return Err(format!("precondition: 新事件应序列化 {key}"));
        }
    }
    let legacy_frame = serde_json::from_value::<ServerFrame>(legacy)
        .map_err(|error| format!("旧 JSON 解码失败: {error}"))?;
    let ServerFrame::Event(legacy) = legacy_frame else {
        return Err("旧 JSON 应解码为 Event 帧".into());
    };
    let AppEvent::QuotaAlert { alert } = &legacy.payload else {
        return Err("旧 JSON payload 应为 QuotaAlert".into());
    };
    if alert.kind.is_some() || alert.source.is_some() {
        return Err(format!("缺字段旧 JSON 应解码为 None: {alert:?}"));
    }

    let failure_with_kind = QuotaFailureView {
        adapter_kind: Some(QuotaAdapterKind::ApiKeyApi),
        error_code: "forbidden".into(),
        detail: "credential rejected".into(),
        retry_after_ms: Some(30_000),
    };
    let failure_json =
        serde_json::to_string(&failure_with_kind).map_err(|error| error.to_string())?;
    if !failure_json.contains("\"adapter_kind\":\"api_key_api\"") {
        return Err(format!(
            "adapter_kind Some 应按冻结形态序列化: {failure_json}"
        ));
    }
    let decoded_failure: QuotaFailureView =
        serde_json::from_str(&failure_json).map_err(|error| error.to_string())?;
    if decoded_failure != failure_with_kind {
        return Err(format!("adapter_kind Some 往返失败: {decoded_failure:?}"));
    }
    let failure = QuotaFailureView {
        adapter_kind: None,
        error_code: "timeout".into(),
        detail: "adapter timed out".into(),
        retry_after_ms: None,
    };
    let none_json = serde_json::to_string(&failure).map_err(|error| error.to_string())?;
    if none_json.contains("adapter_kind") {
        return Err(format!("adapter_kind None 不应序列化: {none_json}"));
    }
    Ok(())
}

/// 无会话分支的形状验证：DiffListFiles 往返应返回空 files 数组。
async fn diff_list_files() -> Result<(), String> {
    let mut harness = Harness::new("diff-list-files", streaming_script()).await;
    let client = harness
        .connect_gui("diff-list-files", "diff-list-files")
        .await?;
    let response = client
        .query(
            AppQuery::DiffListFiles {
                workspace_id: pawork_domain::WorkspaceId::from("ws-unbound"),
            },
            harness::gui_source(&client),
            harness::local_user(),
        )
        .await
        .map_err(|error| format!("DiffListFiles: {error}"))?;
    let AppResponse::Data(data) = &response.response else {
        return Err(format!(
            "DiffListFiles 应返回 Data，got {:?}",
            response.response
        ));
    };
    let files = data
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("DiffListFiles 响应应携带 files 数组: {data:?}"))?;
    if !files.is_empty() {
        return Err(format!("无会话时 files 应为空数组，got {files:?}"));
    }
    Ok(())
}

/// 无会话分支的形状验证：DiffGet 往返应返回空 files 且 complete=true。
async fn diff_get() -> Result<(), String> {
    let mut harness = Harness::new("diff-get", streaming_script()).await;
    let client = harness.connect_gui("diff-get", "diff-get").await?;
    let response = client
        .query(
            AppQuery::DiffGet {
                workspace_id: pawork_domain::WorkspaceId::from("ws-unbound"),
                path: "README.md"
                    .parse()
                    .map_err(|error| format!("path: {error:?}"))?,
                cursor: None,
            },
            harness::gui_source(&client),
            harness::local_user(),
        )
        .await
        .map_err(|error| format!("DiffGet: {error}"))?;
    let AppResponse::Data(data) = &response.response else {
        return Err(format!("DiffGet 应返回 Data，got {:?}", response.response));
    };
    let files = data
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("DiffGet 响应应携带 files 数组: {data:?}"))?;
    if !files.is_empty() {
        return Err(format!("无会话时 files 应为空数组，got {files:?}"));
    }
    if data.get("complete").and_then(Value::as_bool) != Some(true) {
        return Err(format!("无会话时 complete 应为 true，got {data:?}"));
    }
    Ok(())
}

/// mcp_list 命令往返：响应应携带 servers 数组（装配未 prime，恒为空）。
async fn mcp_list() -> Result<(), String> {
    let mut harness = Harness::new("mcp-list", streaming_script()).await;
    let client = harness.connect_gui("mcp-list", "mcp-list").await?;
    let response = client
        .query(
            AppQuery::McpList,
            harness::gui_source(&client),
            harness::local_user(),
        )
        .await
        .map_err(|error| format!("McpList: {error}"))?;
    let AppResponse::Data(data) = &response.response else {
        return Err(format!("McpList 应返回 Data，got {:?}", response.response));
    };
    let servers = data
        .get("servers")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("McpList 响应应携带 servers 数组: {data:?}"))?;
    if !servers.is_empty() {
        return Err(format!("未装配 MCP 时 servers 应为空数组，got {servers:?}"));
    }
    Ok(())
}

async fn prepare_session_via_client(client: &GuiClient) -> Result<SessionId, String> {
    let session = client
        .command(
            AppCommand::SessionCreate {
                workspace_id: pawork_domain::WorkspaceId::from("ws-unbound"),
                title: Some("self-test".into()),
            },
            harness::gui_source(client),
            harness::local_user(),
        )
        .await
        .map_err(|error| format!("SessionCreate: {error}"))?;
    match &session.response {
        AppResponse::Data(value) => harness::session_id_from_data(value),
        other => Err(format!("SessionCreate 应返回 Data，got {other:?}")),
    }
}

fn streaming_script() -> MockScript {
    MockScript::new()
        .response_started("self-test-r1")
        .text("hello ")
        .text("self-test")
        .complete()
}

fn waiting_script() -> MockScript {
    MockScript::new().wait_for_cancellation()
}

fn run_state(event: &AppEventEnvelope, run_id: &RunId) -> Option<RunState> {
    match &event.payload {
        AppEvent::RunChanged { run_id: id, state } if id == run_id => Some(state.clone()),
        _ => None,
    }
}

async fn recv_until<F: Fn(&AppEventEnvelope) -> bool>(
    client: &GuiClient,
    predicate: F,
) -> Result<(bool, Vec<AppEventEnvelope>), String> {
    let deadline = Instant::now() + TIMEOUT;
    let mut received = Vec::new();
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match client.next_event_timeout(remaining).await {
            Ok(event) => {
                if predicate(&event) {
                    received.push(event);
                    return Ok((true, received));
                }
                received.push(event);
            }
            Err(error) => return Err(format!("等待事件失败: {error}")),
        }
    }
    Ok((false, received))
}

async fn wait_until(mut condition: impl FnMut() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + TIMEOUT;
    while !condition() {
        if Instant::now() >= deadline {
            return Err("等待条件超时".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}
