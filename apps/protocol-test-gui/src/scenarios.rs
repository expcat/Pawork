//! --self-test 场景集：逐项在进程内装配 server（memory transport + tempdir
//! token）跑契约场景，输出 PASS / FAIL。

use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_domain::{ArtifactId, CommandId, EventId, ProviderId, RunId, SessionId, Timestamp};
use artifact_store::ArtifactStore;
use core_api::{
    ApiVersion, AppCommand, AppEvent, AppEventEnvelope, AppResponse, CommandSource, EventSource,
    EventStream, GlobalSequence, RunState, API_VERSION,
};
use gui_client::{ClientConfig, ClientError, GuiClient, ResumeDisposition};
use gui_protocol::ProtocolErrorCode;
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
    client
        .subscribe_all()
        .await
        .map_err(|e| format!("subscribe: {e}"))?;
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
    gui_a
        .subscribe_all()
        .await
        .map_err(|e| format!("subscribe a: {e}"))?;
    gui_b
        .subscribe_all()
        .await
        .map_err(|e| format!("subscribe b: {e}"))?;
    gui_c
        .subscribe_all()
        .await
        .map_err(|e| format!("subscribe c: {e}"))?;

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
    client
        .subscribe_all()
        .await
        .map_err(|e| format!("subscribe: {e}"))?;
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
// 辅助
// ---------------------------------------------------------------------------

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
