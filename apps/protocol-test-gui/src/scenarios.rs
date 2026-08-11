//! --self-test 场景集：逐项在进程内装配 server（memory transport + tempdir
//! token）跑契约场景，输出 PASS / FAIL。

use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_domain::{
    ArtifactId, CommandId, EventId, ProviderId, RunId, SessionId, TenantId, Timestamp,
};
use artifact_store::ArtifactStore;
use core_api::{
    mask_credential_hint, ApiVersion, AppCommand, AppEvent, AppEventEnvelope, AppResponse,
    CommandSource, EventSource, EventStream, GlobalSequence, QuotaAdapterKind, QuotaAlert,
    QuotaAlertKind, QuotaAlertSeverity, QuotaFailureView, QuotaUnit, QuotaWindow, RunState,
    API_VERSION,
};
use gui_client::{ClientConfig, ClientError, GuiClient, ResumeDisposition};
use gui_protocol::{decode_server_frame, encode_server_frame, ProtocolErrorCode, ServerFrame};
use provider_api::ModelProvider;
use serde_json::{json, Value};
use subscription_hub::EventHub;

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
        "artifact-chunks",
        "version-reject",
        "disconnect-keeps-run",
        "quota-alert-roundtrip",
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
        "artifact-chunks" => artifact_chunks().await,
        "version-reject" => version_reject().await,
        "disconnect-keeps-run" => disconnect_keeps_run().await,
        "quota-alert-roundtrip" => quota_alert_roundtrip().await,
        other => Err(format!("unknown scenario {other}")),
    }
}

// ---------------------------------------------------------------------------
// a) 创建 session / 发消息 / 收流式 Run 事件
// ---------------------------------------------------------------------------

async fn session_events() -> Result<(), String> {
    let mut harness = Harness::new("self-test-session-events").await;
    harness.register_provider(streaming_provider());
    let client = harness.connect_gui("session-events").await?;
    if client.initial_snapshot().is_none() {
        return Err("握手后应消费首帧 Snapshot".into());
    }
    let session_id = prepare_session_via_client(&client).await?;
    client
        .subscribe_all()
        .await
        .map_err(|e| format!("subscribe: {e}"))?;

    let run = client
        .command(
            AppCommand::RunStart {
                session_id,
                user_message: "hello from self-test".into(),
                model: None,
            },
            CommandSource::LocalGui {
                client_id: client.client_id().clone(),
            },
            harness::local_user(),
        )
        .await
        .map_err(|e| format!("RunStart: {e}"))?;
    if !matches!(run.response, AppResponse::Accepted { .. }) {
        return Err(format!("RunStart 应 Accepted，got {:?}", run.response));
    }
    let run_id = harness
        .app_service
        .router()
        .last_started_run()
        .ok_or("last_started_run 缺失")?;

    let (done, events) = recv_until(&client, |e| {
        run_state(e, &run_id) == Some(RunState::Completed)
    })
    .await?;
    if !done {
        return Err("未收到 Run 的 Completed 事件".into());
    }
    if !events
        .iter()
        .any(|e| matches!(&e.payload, AppEvent::AssistantDelta { .. }))
    {
        return Err("未收到流式增量事件".into());
    }
    if !events
        .iter()
        .all(|e| e.stream == EventStream::Run(run_id.clone()))
    {
        return Err("Run 事件应属于该 Run 流".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// b) 快照与断线重连（Replay 补发）
// ---------------------------------------------------------------------------

async fn snapshot_reconnect() -> Result<(), String> {
    let mut harness = Harness::new("self-test-reconnect").await;
    harness.register_provider(Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().wait_for_cancellation())
            .with_id(ProviderId::from("mock")),
    ));
    let session_id = harness.prepare_session()?;
    let client = harness.connect_gui("first").await?;
    subscribe_all_landed(&harness, &client).await?;
    let run_id = harness.start_run_cli(&session_id, "long run")?;
    let (done, events) = recv_until(&client, |e| {
        run_state(e, &run_id) == Some(RunState::StreamingResponse)
    })
    .await?;
    if !done {
        return Err("第一段连接应进入 StreamingResponse".into());
    }
    let last_sequence = events
        .iter()
        .map(|e| e.global_sequence.0)
        .max()
        .ok_or("至少一个事件")?;

    // SnapshotRequest：快照重建 Run 活跃状态。
    let snapshot = client
        .snapshot()
        .await
        .map_err(|e| format!("snapshot: {e}"))?;
    let active_runs = snapshot
        .sections
        .iter()
        .find(|section| section.kind == gui_protocol::SnapshotSectionKind::ActiveRuns)
        .ok_or("缺少 ActiveRuns section")?;
    let runs = active_runs.data.as_ref().ok_or("ActiveRuns 无内联 data")?["runs"]
        .as_array()
        .ok_or("runs 非数组")?;
    if !runs.iter().any(|run| {
        run["run_id"] == json!(run_id.as_str()) && run["state"] == json!("streaming_response")
    }) {
        return Err("快照未重建 Run 的活跃状态".into());
    }

    client
        .ack(GlobalSequence(last_sequence))
        .await
        .map_err(|e| format!("ack: {e}"))?;
    client.close().await.map_err(|e| format!("close: {e}"))?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 断线期间 CLI 取消 Run → 产生缺失事件。
    harness.cancel_run_cli(&run_id);
    wait_until(|| harness.hub.current().0 > last_sequence).await?;

    let (reconnected, outcome) = harness
        .reconnect_gui("second", Some(GlobalSequence(last_sequence)))
        .await?;
    let outcome = outcome.ok_or("缺少 resume outcome")?;
    let (from, through) = match outcome.disposition {
        ResumeDisposition::Replay {
            from_sequence,
            through_sequence,
        } => (from_sequence.0, through_sequence.0),
        other => return Err(format!("应 Replay，got {other:?}")),
    };
    if from != last_sequence + 1 || through != harness.hub.current().0 {
        return Err(format!("Replay 范围不符: from={from} through={through}"));
    }
    if !outcome.replayed.iter().any(|e| {
        matches!(
            e.payload,
            AppEvent::RunChanged {
                ref state,
                ..
            } if *state == RunState::Cancelled
        )
    }) {
        return Err("重放应包含断线期间的取消事件".into());
    }
    let _ = reconnected
        .heartbeat()
        .await
        .map_err(|e| format!("重连后 heartbeat: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// b2) Resume 降级 SnapshotRequired 重建
// ---------------------------------------------------------------------------

async fn resume_snapshot_fallback() -> Result<(), String> {
    let hub = Arc::new(EventHub::with_capacity(2));
    let mut harness = Harness::new_with("self-test-fallback", Some(hub), None).await;
    let run_id = RunId::from("synth-run");
    for i in 1..=20u64 {
        harness
            .hub
            .publish(synthetic_event(&run_id, i, RunState::StreamingResponse));
    }
    if harness.hub.earliest_available().ok_or("earliest")?.0 <= 2 {
        return Err("ring 应已淘汰早期事件".into());
    }
    let client = harness.connect_gui("fallback").await?;
    let outcome = client
        .resume(GlobalSequence(1))
        .await
        .map_err(|e| format!("resume: {e}"))?;
    if !matches!(
        outcome.disposition,
        ResumeDisposition::SnapshotRequired { .. }
    ) {
        return Err(format!(
            "应降级 SnapshotRequired，got {:?}",
            outcome.disposition
        ));
    }
    if outcome.snapshot.is_none() {
        return Err("降级后应补发 Snapshot".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// c) 3 GUI 并发同步
// ---------------------------------------------------------------------------

async fn three_gui_sync() -> Result<(), String> {
    let mut harness = Harness::new("self-test-three-gui").await;
    harness.register_provider(streaming_provider());
    let session_id = harness.prepare_session()?;

    let gui_a = harness.connect_gui("a").await?;
    let gui_b = harness.connect_gui("b").await?;
    let gui_c = harness.connect_gui("c").await?;
    if harness.connections.count() != 3 {
        return Err(format!(
            "应登记 3 个 GUI，got {}",
            harness.connections.count()
        ));
    }
    subscribe_all_landed(&harness, &gui_a).await?;
    subscribe_all_landed(&harness, &gui_b).await?;
    subscribe_all_landed(&harness, &gui_c).await?;

    let run_id = harness.start_run_cli(&session_id, "cli run")?;
    for (name, gui) in [("A", &gui_a), ("B", &gui_b), ("C", &gui_c)] {
        let (done, _) =
            recv_until(gui, |e| run_state(e, &run_id) == Some(RunState::Completed)).await?;
        if !done {
            return Err(format!("GUI {name} 未收到 CLI Run 的 Completed"));
        }
    }

    // GUI A 发起的 Run 同步到 GUI B / C 与 CLI 观察者。
    let mut cli_observer = harness.hub.subscribe();
    gui_a
        .command(
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: "run from gui a".into(),
                model: None,
            },
            CommandSource::LocalGui {
                client_id: gui_a.client_id().clone(),
            },
            harness::local_user(),
        )
        .await
        .map_err(|e| format!("gui a RunStart: {e}"))?;
    let gui_run_id = harness
        .app_service
        .router()
        .last_started_run()
        .ok_or("gui run id 缺失")?;
    for (name, gui) in [("B", &gui_b), ("C", &gui_c)] {
        let (done, _) = recv_until(gui, |e| {
            run_state(e, &gui_run_id) == Some(RunState::Completed)
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

// ---------------------------------------------------------------------------
// d) 命令幂等
// ---------------------------------------------------------------------------

async fn command_idempotency() -> Result<(), String> {
    let mut harness = Harness::new("self-test-idempotency").await;
    let client = harness.connect_gui("idempotency").await?;
    let envelope = core_api::AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("idem-session-1"),
        source: CommandSource::LocalGui {
            client_id: client.client_id().clone(),
        },
        identity: harness::local_user(),
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(7),
        command: AppCommand::SessionCreate {
            workspace_id: session_workspace_id(&harness).await?,
            title: Some("idempotent".into()),
        },
    };
    let first = client
        .command_envelope(envelope.clone())
        .await
        .map_err(|e| format!("首次执行: {e}"))?;
    let replayed = client
        .command_envelope(envelope)
        .await
        .map_err(|e| format!("重放: {e}"))?;
    if replayed != first {
        return Err("同 command_id 重放未返回相同响应".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// e) 大 artifact 分片读取
// ---------------------------------------------------------------------------

async fn artifact_chunks() -> Result<(), String> {
    let diff = diff_payload(100_000);
    if diff.len() <= 5 * 1024 * 1024 {
        return Err(format!("diff 应约 5MiB，实际 {}", diff.len()));
    }
    let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let store = Arc::new(
        ArtifactStore::open(temp.path().join("store"))
            .await
            .map_err(|e| format!("open store: {e}"))?,
    );
    let outcome = store
        .put(&diff)
        .await
        .map_err(|e| format!("put blob: {e}"))?;
    let artifact_id = ArtifactId::from(outcome.id.as_str());
    let mut harness = Harness::new_with("self-test-artifact", None, Some(Arc::clone(&store))).await;
    harness
        .app_service
        .router()
        .aggregate()
        .put_artifact(artifact_id.clone(), diff.len() as u64, "text/x-diff".into())
        .map_err(|e| format!("register artifact: {e}"))?;
    let client = harness.connect_gui("artifact").await?;

    let assembled = client
        .read_artifact(&artifact_id, 0, 0)
        .await
        .map_err(|e| format!("read artifact: {e}"))?;
    if assembled != diff {
        return Err("分片重组与原始 payload 不一致".into());
    }
    let partial = client
        .read_artifact(&artifact_id, 64 * 1024, 70 * 1024)
        .await
        .map_err(|e| format!("read partial: {e}"))?;
    if partial != diff[64 * 1024..64 * 1024 + 70 * 1024] {
        return Err("limit 截断不一致".into());
    }
    let error = client
        .read_artifact(&ArtifactId::from("art-missing"), 0, 0)
        .await
        .expect_err("缺失 artifact 应报错");
    if !error.is_request_not_found() {
        return Err(format!("应报 RequestNotFound，got {error:?}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// f) 版本不兼容握手被拒
// ---------------------------------------------------------------------------

async fn version_reject() -> Result<(), String> {
    let harness = Harness::new("self-test-version").await;
    let listener = Arc::clone(&harness.listener);
    let accept = tokio::spawn(async move { listener.accept().await });
    let transport: Arc<dyn transport_api::GuiTransportClient> = harness.transport.clone();
    let config = ClientConfig {
        supported_api_versions: vec![ApiVersion { major: 2, minor: 0 }],
        ..ClientConfig::default()
    };
    let error = match GuiClient::connect_with_config(
        transport,
        transport_api::TransportEndpoint::Memory {
            channel: "protocol-self-test".into(),
        },
        Harness::connect_options("version-reject"),
        &harness.token,
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
        .map_err(|e| format!("accept task: {e}"))?
        .map_err(|e| format!("accept: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// g) GUI 断线不取消 Run
// ---------------------------------------------------------------------------

async fn disconnect_keeps_run() -> Result<(), String> {
    let mut harness = Harness::new("self-test-disconnect").await;
    harness.register_provider(Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().wait_for_cancellation())
            .with_id(ProviderId::from("mock")),
    ));
    let session_id = harness.prepare_session()?;
    let client = harness.connect_gui("disconnect").await?;
    subscribe_all_landed(&harness, &client).await?;
    let run_id = harness.start_run_cli(&session_id, "survives disconnect")?;
    let (done, _) = recv_until(&client, |e| {
        run_state(e, &run_id) == Some(RunState::StreamingResponse)
    })
    .await?;
    if !done {
        return Err("Run 应进入 StreamingResponse".into());
    }
    client.close().await.map_err(|e| format!("close: {e}"))?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    if !harness.app_service.router().supervisor().is_active(&run_id) {
        return Err("GUI 断线不得取消 Run".into());
    }
    harness.cancel_run_cli(&run_id);
    wait_until(|| !harness.app_service.router().supervisor().is_active(&run_id)).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// h) QuotaAlert 协议 roundtrip：kind/source 保留 + 旧 JSON 缺字段兼容
// ---------------------------------------------------------------------------

async fn quota_alert_roundtrip() -> Result<(), String> {
    // 新事件：kind/source 总是 Some，经真实 ServerFrame 线上编解码后原样保留。
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
        instance_id: agent_domain::CoreInstanceId::from("self-test-quota-alert"),
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
        .map_err(|e| format!("encode ServerFrame: {e}"))?;
    let wire = String::from_utf8(encoded.clone()).map_err(|e| e.to_string())?;
    if !wire.contains("\"kind\":\"reauthorization_required\"")
        || !wire.contains("\"source\":\"ApiKeyApi:api.openai.com/v1/organization/usage\"")
    {
        return Err(format!("新事件线上 JSON 必须携带 kind/source: {wire}"));
    }
    if wire.contains("sk-leak") {
        return Err("线上 JSON 不得泄露 secret 原文".into());
    }

    let decoded = decode_server_frame(&encoded).map_err(|e| format!("decode ServerFrame: {e}"))?;
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
    if alert.severity != QuotaAlertSeverity::Warning || alert.message != "low balance" {
        return Err(format!("roundtrip 后其余字段漂移: {alert:?}"));
    }

    // 旧事件 JSON：缺 kind/source 时解码为 None（重放兼容），其余字段保留。
    let mut legacy: Value = serde_json::from_slice(&encoded).map_err(|e| e.to_string())?;
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
        .map_err(|e| format!("旧 JSON 解码失败: {e}"))?;
    let ServerFrame::Event(legacy) = legacy_frame else {
        return Err("旧 JSON 应解码为 Event 帧".into());
    };
    let AppEvent::QuotaAlert { alert } = &legacy.payload else {
        return Err("旧 JSON payload 应为 QuotaAlert".into());
    };
    if alert.kind.is_some() || alert.source.is_some() {
        return Err(format!("缺字段旧 JSON 应解码为 None: {alert:?}"));
    }
    if alert.severity != QuotaAlertSeverity::Warning || alert.message != "low balance" {
        return Err(format!("旧 JSON 其余字段应保留: {alert:?}"));
    }

    // adapter_kind 可选：Some 往返保留，None 不序列化；旧失败 JSON 缺字段解码为 None。
    let failure_with_kind = QuotaFailureView {
        adapter_kind: Some(QuotaAdapterKind::ApiKeyApi),
        error_code: "forbidden".into(),
        detail: "credential rejected".into(),
        retry_after_ms: Some(30_000),
    };
    let failure_with_kind_json =
        serde_json::to_string(&failure_with_kind).map_err(|e| e.to_string())?;
    if !failure_with_kind_json.contains("\"adapter_kind\":\"api_key_api\"") {
        return Err(format!(
            "adapter_kind Some 应按冻结形态序列化: {failure_with_kind_json}"
        ));
    }
    let decoded_failure_with_kind: QuotaFailureView =
        serde_json::from_str(&failure_with_kind_json).map_err(|e| e.to_string())?;
    if decoded_failure_with_kind != failure_with_kind {
        return Err(format!(
            "adapter_kind Some 往返失败: {decoded_failure_with_kind:?}"
        ));
    }
    let failure = QuotaFailureView {
        adapter_kind: None,
        error_code: "timeout".into(),
        detail: "adapter timed out".into(),
        retry_after_ms: None,
    };
    let failure_json = serde_json::to_string(&failure).map_err(|e| e.to_string())?;
    if failure_json.contains("adapter_kind") {
        return Err(format!("adapter_kind None 不应序列化: {failure_json}"));
    }
    let decoded_failure: QuotaFailureView =
        serde_json::from_str(&failure_json).map_err(|e| e.to_string())?;
    if decoded_failure != failure {
        return Err(format!("adapter_kind None 往返失败: {decoded_failure:?}"));
    }
    let legacy_failure: QuotaFailureView =
        serde_json::from_str(r#"{"error_code":"forbidden","detail":"credential rejected"}"#)
            .map_err(|e| e.to_string())?;
    if legacy_failure.adapter_kind.is_some() {
        return Err("旧失败 JSON 缺 adapter_kind 应解码为 None".into());
    }
    if legacy_failure.error_code != "forbidden" {
        return Err("旧失败 JSON 其余字段应保留".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

/// subscribe_all 后等待服务端订阅落地（以 ConnectionManager 会话视图为事实源）。
///
/// `subscribe_all` 只保证 Subscribe 帧已发出；服务端 `connections.subscribe`
/// 在连接读循环中异步落地。若另一 CLI 连接在落地前启动 Run，事件会先于订阅
/// 注册发布而被 `should_forward` 过滤漏投递。此处有界轮询
/// `connections.session(client_id)` 直到出现全量订阅（"all" + 空 streams），
/// 限时未落地即报错，不无限阻塞、不依赖增大超时。
async fn subscribe_all_landed(harness: &Harness, client: &GuiClient) -> Result<(), String> {
    client
        .subscribe_all()
        .await
        .map_err(|e| format!("subscribe: {e}"))?;
    let client_id = client.client_id().clone();
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        let landed = harness
            .connections
            .session(&client_id)
            .map(|session| {
                session
                    .subscriptions
                    .iter()
                    .any(|sub| sub.subscription_id == "all" && sub.streams.is_empty())
            })
            .unwrap_or(false);
        if landed {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Err(format!(
        "客户端 {} 的 subscribe_all 未在限时内落地",
        client.client_id()
    ))
}

async fn prepare_session_via_client(client: &GuiClient) -> Result<SessionId, String> {
    let workspace_id = session_workspace_id_via(client).await?;
    let session = client
        .command(
            AppCommand::SessionCreate {
                workspace_id,
                title: Some("self-test".into()),
            },
            CommandSource::LocalGui {
                client_id: client.client_id().clone(),
            },
            harness::local_user(),
        )
        .await
        .map_err(|e| format!("SessionCreate: {e}"))?;
    let session_id = match &session.response {
        AppResponse::Data(value) => SessionId::from(
            value
                .get("session_id")
                .and_then(Value::as_str)
                .ok_or("SessionCreate 响应缺少 session_id")?,
        ),
        other => return Err(format!("SessionCreate 应返回 Data，got {other:?}")),
    };
    Ok(session_id)
}

async fn session_workspace_id(harness: &Harness) -> Result<agent_domain::WorkspaceId, String> {
    let dir = std::env::temp_dir().join(format!("pawork-idem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let response = harness.app_service.dispatch_envelope(harness::command(
        harness::cli_source(),
        harness::cli_identity(),
        AppCommand::WorkspaceAdd {
            root_path: dir.to_string_lossy().into_owned(),
        },
    ));
    match &response.response {
        AppResponse::Data(value) => Ok(agent_domain::WorkspaceId::from(
            value
                .get("id")
                .and_then(Value::as_str)
                .ok_or("WorkspaceAdd 响应缺少 id")?,
        )),
        other => Err(format!("WorkspaceAdd 应返回 Data，got {other:?}")),
    }
}

async fn session_workspace_id_via(client: &GuiClient) -> Result<agent_domain::WorkspaceId, String> {
    let dir = std::env::temp_dir().join(format!("pawork-self-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let workspace = client
        .command(
            AppCommand::WorkspaceAdd {
                root_path: dir.to_string_lossy().into_owned(),
            },
            CommandSource::LocalGui {
                client_id: client.client_id().clone(),
            },
            harness::local_user(),
        )
        .await
        .map_err(|e| format!("WorkspaceAdd: {e}"))?;
    match &workspace.response {
        AppResponse::Data(value) => Ok(agent_domain::WorkspaceId::from(
            value
                .get("id")
                .and_then(Value::as_str)
                .ok_or("WorkspaceAdd 响应缺少 id")?,
        )),
        other => Err(format!("WorkspaceAdd 应返回 Data，got {other:?}")),
    }
}

fn streaming_provider() -> Arc<dyn ModelProvider> {
    Arc::new(
        test_support::MockProvider::new(
            test_support::MockScript::new()
                .response_started("self-test-r1")
                .text("hello ")
                .text("self-test")
                .complete(),
        )
        .with_id(ProviderId::from("mock")),
    )
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

fn synthetic_event(run_id: &RunId, sequence_hint: u64, state: RunState) -> AppEventEnvelope {
    AppEventEnvelope {
        api_version: API_VERSION,
        instance_id: agent_domain::CoreInstanceId::from("self-test-synthetic"),
        event_id: EventId::from(format!("synth-{sequence_hint}")),
        global_sequence: GlobalSequence(sequence_hint),
        stream: EventStream::Run(run_id.clone()),
        stream_sequence: sequence_hint,
        timestamp: Timestamp::from_unix_millis(sequence_hint),
        source: EventSource::Core,
        payload: AppEvent::RunChanged {
            run_id: run_id.clone(),
            state,
        },
    }
}

fn diff_payload(lines: usize) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("diff --git a/src/main.rs b/src/main.rs\n");
    out.push_str("--- a/src/main.rs\n");
    out.push_str("+++ b/src/main.rs\n");
    out.push_str(&format!("@@ -1,{lines} +1,{lines} @@\n"));
    for i in 0..lines {
        out.push_str(&format!(
            "+pub fn line_{i}(value: usize) -> usize {{ value + {i} }}\n"
        ));
    }
    out.into_bytes()
}
