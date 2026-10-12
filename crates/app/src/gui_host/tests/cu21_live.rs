//! CU-21: one real isolated-Firefox turn.
//! Missing PAWORK_CU21_ISOLATED=1 fails the test instead of reporting success.

use super::*;
use async_trait::async_trait;
use pawork_domain::{
    ApprovalDecision, CancellationToken, CanonicalModelRequest, ContentPart, MessageRole,
    ModelCapabilities, ModelId, ModelProvider, ModelResponseSummary, ProviderError,
    ProviderErrorKind, ProviderEventSink, ProviderId, ProviderStreamEvent, RunId, StopReason,
    TokenUsage, ToolCallId,
};
use pawork_gui_server::GuiHost;
use pawork_protocol::{
    AppCommand, AppQuery, AppResponse, ComputerAppFamily, ComputerAppIdentity, ComputerGrantKind,
    ComputerTargetApproveParams, ComputerTargetIdentity, MAX_OBSERVATION_READ_BYTES,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FIREFOX: &str = "guest.browser.firefox";
const EVIDENCE_JPEG: &str = "/tmp/pawork-cu21-isolated-linux/evidence/cu21-for-run-observation.jpg";

struct ApproveOnce;

#[async_trait]
impl crate::approval::ApprovalPromptHost for ApproveOnce {
    async fn decide(
        &self,
        _ask: &crate::ApprovalAsk,
        _cancel: CancellationToken,
    ) -> ApprovalDecision {
        ApprovalDecision::ApprovedOnce
    }
}

struct Gate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

struct Cu21Provider {
    provider_id: ProviderId,
    gate: Arc<Gate>,
    round: AtomicUsize,
    offered: Mutex<Vec<Vec<String>>>,
    tried: Mutex<Vec<String>>,
    rereads: AtomicUsize,
}

#[async_trait]
impl ModelProvider for Cu21Provider {
    fn id(&self) -> ProviderId {
        self.provider_id.clone()
    }

    async fn list_models(
        &self,
        _credential: Option<&pawork_domain::ResolvedCredential>,
    ) -> Result<Vec<pawork_domain::ModelDefinition>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream(
        &self,
        request: &CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        self.offered
            .lock()
            .expect("offered")
            .push(request.tools.iter().map(|tool| tool.name.clone()).collect());
        let round = self.round.fetch_add(1, Ordering::SeqCst);
        let arguments = match round {
            0 => json!({"action": "request", "bundle_id": FIREFOX}),
            1 => {
                let waiting = self.gate.release.notified();
                tokio::pin!(waiting);
                waiting.as_mut().enable();
                self.gate.entered.notify_one();
                tokio::select! {
                    _ = &mut waiting => {}
                    _ = cancel.cancelled() => {
                        return Err(ProviderError::cancelled("cu21 approval wait cancelled"));
                    }
                }
                json!({"action": "bind", "bundle_id": FIREFOX})
            }
            _ => {
                let action = {
                    let mut tried = self.tried.lock().expect("tried");
                    next_action(request, &mut tried, &self.rereads).map_err(|error| {
                        ProviderError::new(ProviderErrorKind::InvalidRequest, error)
                    })?
                };
                match action {
                    Some(arguments) => arguments,
                    None => {
                        sink.emit(ProviderStreamEvent::TextDelta("pressed cu21-button".into()))
                            .await?;
                        sink.emit(ProviderStreamEvent::ResponseCompleted(
                            StopReason::Completed,
                        ))
                        .await?;
                        return Ok(done(StopReason::Completed));
                    }
                }
            }
        };
        emit_tool(sink, &format!("cu21-{round}"), arguments).await
    }
}

fn done(stop_reason: StopReason) -> ModelResponseSummary {
    ModelResponseSummary {
        stop_reason,
        usage: TokenUsage::default(),
        response_id: None,
        provider_metadata: Value::Null,
    }
}

async fn emit_tool(
    sink: &dyn ProviderEventSink,
    id: &str,
    arguments: Value,
) -> Result<ModelResponseSummary, ProviderError> {
    let id = ToolCallId::from(id);
    sink.emit(ProviderStreamEvent::ToolCallStarted {
        id: id.clone(),
        name: "computer_native".into(),
    })
    .await?;
    sink.emit(ProviderStreamEvent::ToolCallArgumentsDelta {
        id: id.clone(),
        json: serde_json::to_string(&arguments).expect("tool arguments serialize"),
    })
    .await?;
    sink.emit(ProviderStreamEvent::ToolCallCompleted { id })
        .await?;
    sink.emit(ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse))
        .await?;
    Ok(done(StopReason::ToolUse))
}

fn tool_results(request: &CanonicalModelRequest) -> Vec<(bool, String, Value)> {
    let mut results = Vec::new();
    for message in &request.messages {
        if message.role != MessageRole::Tool {
            continue;
        }
        for part in &message.content {
            let ContentPart::ToolResult(result) = part else {
                continue;
            };
            let text = result
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            results.push((result.is_error, text, result.metadata.clone()));
        }
    }
    results
}

fn next_action(
    request: &CanonicalModelRequest,
    tried: &mut Vec<String>,
    rereads: &AtomicUsize,
) -> Result<Option<Value>, String> {
    let results = tool_results(request);
    let Some((is_error, text, meta)) = results.last() else {
        return Err("computer_native continued without a tool result".into());
    };
    let failed = *is_error;
    if !failed {
        if let Some(outcome) = meta.get("outcome").and_then(Value::as_str) {
            let dispatched = meta.get("input_dispatched").and_then(Value::as_bool) == Some(true);
            if outcome != "dispatched" || !dispatched {
                return Err(format!(
                    "press receipt outcome={outcome} input_dispatched={dispatched} text={text}"
                ));
            }
            if meta
                .get("observation_failure")
                .and_then(Value::as_str)
                .is_some()
            {
                return Err(format!("post-action observation failed: {text}"));
            }
            return Ok(None);
        }
        if let Some(element) = meta.get("element_id").and_then(Value::as_str) {
            if !element.is_empty() {
                return Ok(Some(json!({
                    "action": "semantic",
                    "element": element,
                    "semantic": "press"
                })));
            }
        }
        if let Some((_, _, tree)) = results.iter().rev().find(|(_, text, meta)| {
            text.contains("name=cu21-button")
                && meta.get("observation").and_then(Value::as_str).is_some()
        }) {
            let observation = tree
                .get("observation")
                .and_then(Value::as_str)
                .expect("observation checked");
            return Ok(Some(json!({
                "action": "find_element",
                "observation": observation,
                "role": "push button",
                "name": "cu21-button"
            })));
        }
    }
    if failed && text.contains("re-read it") && rereads.load(Ordering::SeqCst) < 3 {
        if let Some(window) = tried.last().cloned() {
            rereads.fetch_add(1, Ordering::SeqCst);
            return Ok(Some(json!({"action": "read_tree", "window": window})));
        }
    }
    let mut windows = Vec::new();
    for (_, text, meta) in &results {
        if let Some(id) = meta.get("window_id").and_then(Value::as_str) {
            collect_handles(id, &mut windows);
        }
        collect_handles(text, &mut windows);
    }
    if let Some(window) = windows
        .iter()
        .find(|window| !tried.iter().any(|seen| seen == *window))
    {
        tried.push(window.clone());
        return Ok(Some(json!({"action": "read_tree", "window": window})));
    }
    if failed {
        return Err(format!("computer_native failed: {text}"));
    }
    Err(format!(
        "no unread firefox window contains cu21-button; windows={windows:?}; last={text}"
    ))
}

fn collect_handles(text: &str, into: &mut Vec<String>) {
    let mut rest = text;
    while let Some(start) = rest.find("w-") {
        let candidate = &rest[start..];
        if let Some(handle) = take_handle(candidate) {
            if !into.iter().any(|have| have == handle) {
                into.push(handle.to_string());
            }
            rest = &candidate[handle.len()..];
        } else {
            rest = &candidate[2..];
        }
    }
}

fn take_handle(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    if bytes.len() < 6 || &bytes[..2] != b"w-" {
        return None;
    }
    let mut hyphens = 0;
    let mut digits = 0;
    let mut index = 2;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            digits += 1;
            index += 1;
            continue;
        }
        if bytes[index] == b'-' && digits > 0 && hyphens < 2 {
            hyphens += 1;
            digits = 0;
            index += 1;
            continue;
        }
        break;
    }
    (hyphens == 2 && digits > 0).then_some(&text[..index])
}

fn guest_tree() -> Result<String, String> {
    let output = std::process::Command::new("docker")
        .args([
            "exec",
            "-e",
            "DISPLAY=:0",
            "pawork-cu21-isolated",
            "xwininfo",
            "-root",
            "-tree",
        ])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "xwininfo exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn wait_for_guest(label: &str, present: &str, absent: Option<&str>) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match guest_tree() {
            Ok(tree)
                if tree.contains(present) && absent.is_none_or(|needle| !tree.contains(needle)) =>
            {
                return;
            }
            Ok(tree) if Instant::now() >= deadline => {
                panic!("{label} not observed in guest window tree:\n{tree}");
            }
            Err(error) if Instant::now() >= deadline => panic!("{label} failed: {error}"),
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}

fn clear_mousepad() {
    clear_guest_process("mousepad");
}

fn clear_xmessage() {
    clear_guest_process("xmessage");
}

fn clear_guest_process(name: &str) {
    let output = std::process::Command::new("docker")
        .args(["exec", "pawork-cu21-isolated", "pkill", "-x", name])
        .output()
        .unwrap_or_else(|error| panic!("pkill {name}: {error}"));
    let code = output.status.code();
    assert!(
        code == Some(0) || code == Some(1),
        "pkill {name} exited {code:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn query(id: &str, app_query: AppQuery) -> pawork_protocol::AppQueryEnvelope {
    let mut envelope = query_envelope(app_query);
    envelope.request_id = pawork_domain::QueryId::from(id);
    envelope
}

async fn persisted_jpeg(adapter: &GuiHostAdapter, session: &pawork_domain::SessionId) -> Vec<u8> {
    let listed = adapter
        .query(&query(
            "query-cu21-list",
            AppQuery::ObservationList {
                session_id: session.clone(),
                after_sequence: None,
                limit: None,
            },
        ))
        .await
        .expect("observation list");
    let AppResponse::Data(data) = listed else {
        panic!("observation list returns data: {listed:?}");
    };
    let entries = data["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1, "one for-run screenshot: {data}");
    let entry = &entries[0];
    assert_eq!(entry["tool_name"], "computer_native");
    assert_eq!(entry["part_index"], 1);
    assert_eq!(entry["media_type"], "image/jpeg");
    let total = entry["byte_length"].as_u64().expect("byte_length");
    assert!(total > 4, "jpeg must be longer than the 4-byte fixture");
    let sequence = entry["sequence"].as_u64().expect("sequence");
    let mut bytes = Vec::new();
    let mut offset = 0;
    while offset < total {
        let limit = (total - offset).min(MAX_OBSERVATION_READ_BYTES);
        let read = adapter
            .query(&query(
                "query-cu21-read",
                AppQuery::ObservationImageRead {
                    session_id: session.clone(),
                    sequence,
                    part_index: 1,
                    offset,
                    limit,
                },
            ))
            .await
            .expect("observation image read");
        let AppResponse::Data(image) = read else {
            panic!("observation read returns data: {read:?}");
        };
        let encoded = image["data"].as_str().expect("base64");
        let chunk = crate::services::extension::base64_decode(encoded).expect("decode jpeg");
        assert!(!chunk.is_empty(), "empty observation chunk at {offset}");
        offset += chunk.len() as u64;
        bytes.extend(chunk);
        if image["eof"] == true {
            break;
        }
    }
    assert_eq!(bytes.len() as u64, total);
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "jpeg must start with SOI");
    bytes
}

#[tokio::test]
#[ignore = "requires PAWORK_CU21_ISOLATED=1 and the running cu21 guest"]
async fn cu21_isolated_firefox() {
    match std::env::var("PAWORK_CU21_ISOLATED") {
        Ok(value) if value == "1" => {}
        other => panic!("PAWORK_CU21_ISOLATED=1 is required, got {other:?}"),
    }
    crate::testsupport::set_env("PAWORK_ISOLATED_GUEST_TOKEN", "cu21-isolated-token");
    wait_for_guest("cu21-ready", "cu21-ready", None).await;
    clear_mousepad();
    clear_xmessage();
    wait_for_guest(
        "cu21-ready after clearing mousepad",
        "cu21-ready",
        Some("mousepad"),
    )
    .await;
    wait_for_guest(
        "cu21-ready after clearing xmessage",
        "cu21-ready",
        Some("xmessage"),
    )
    .await;

    let workspace = tempfile::tempdir().expect("workspace");
    let data = tempfile::tempdir().expect("data dir");
    let backend: Arc<dyn pawork_auth::SecretBackend> = Arc::new(pawork_auth::MemoryBackend::new());
    let mut core = crate::AppCore::load_for_catalog(crate::AppLoadOptions {
        workspace_root: Some(workspace.path().to_path_buf()),
        data_dir: Some(data.path().to_path_buf()),
        approval_mode: Some(crate::ApprovalMode::AskForWrites),
        trust_workspaces: Some(true),
        approval_host: Some(Arc::new(ApproveOnce)),
        auth_backend: Some(backend),
        instance: "cu21-live".into(),
        computer_native: crate::ComputerNativeMode::Isolated,
        ..crate::AppLoadOptions::default()
    })
    .await
    .expect("load isolated catalog core");
    assert!(
        core.native.isolated_desktop(),
        "load_for_catalog Isolated installs the shared guest host"
    );
    assert_eq!(core.workspace_id().as_str(), "ws-default");
    let provider_id = core.provider_id.clone();
    let auth_revision = core.provider_auth_revision;
    let gate = Arc::new(Gate {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let script = Arc::new(Cu21Provider {
        provider_id: provider_id.clone(),
        gate: Arc::clone(&gate),
        round: AtomicUsize::new(0),
        offered: Mutex::new(Vec::new()),
        tried: Mutex::new(Vec::new()),
        rereads: AtomicUsize::new(0),
    });
    core.provider = script.clone();
    core.provider_pending = false;
    core.provider_stale = false;
    assert_eq!(core.provider_auth_revision, auth_revision);
    assert!(
        !core.provider_needs_rebuild(),
        "replacing the adapter must not make the snapshot rebuild a real provider"
    );
    std::sync::Arc::make_mut(&mut core.registry)
        .register(pawork_models::CatalogEntry {
            id: ModelId::from("cu21-vision"),
            provider: provider_id,
            display_name: "cu21-vision".into(),
            context_window_tokens: 0,
            max_output_tokens: 0,
            capabilities: ModelCapabilities {
                text: true,
                image_input: true,
                tool_calls: true,
                ..ModelCapabilities::default()
            },
            pricing: None,
            aliases: Vec::new(),
        })
        .expect("register cu21-vision");
    core.model = ModelId::from("cu21-vision");

    let locked = Arc::new(tokio::sync::RwLock::new(core));
    let adapter =
        GuiHostAdapter::from_locked(Arc::clone(&locked), Arc::new(crate::GuiApprovalHost::new()));
    let session = locked
        .read()
        .await
        .create_session("cu21 live")
        .await
        .expect("session");
    let sink = Arc::new(crate::testsupport::RecordingEvents::default());
    let entered = gate.entered.notified();
    tokio::pin!(entered);
    entered.as_mut().enable();
    let core_for_turn = Arc::clone(&locked);
    let sink_for_turn = Arc::clone(&sink);
    let session_for_turn = session.clone();
    let turn = tokio::spawn(async move {
        let guard = core_for_turn.read().await;
        guard
            .chat_turn_with_run_id(
                RunId::from("run-cu21"),
                &session_for_turn,
                vec![crate::testsupport::user_hello()],
                &*sink_for_turn as &dyn pawork_engine::AgentEventSink,
                CancellationToken::new(),
                None,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(60), entered)
        .await
        .expect("provider did not pause before bind");
    let approved = adapter
        .command(&command_envelope(AppCommand::ComputerTargetApprove(
            ComputerTargetApproveParams {
                session_id: session.clone(),
                run_id: RunId::from("run-cu21"),
                target: ComputerTargetIdentity::Application {
                    app: ComputerAppIdentity {
                        bundle_id: FIREFOX.into(),
                        family: ComputerAppFamily::Browser,
                    },
                },
                kind: ComputerGrantKind::ForRun,
            },
        )))
        .await;
    gate.release.notify_one();
    let approved = approved.expect("ComputerTargetApprove");
    let AppResponse::Data(receipt) = approved else {
        panic!("approve returns data: {approved:?}");
    };
    assert_eq!(receipt["kind"], "for_run");
    assert_eq!(receipt["target"]["app"]["bundle_id"], FIREFOX);
    assert_eq!(receipt["target"]["app"]["family"], "browser");

    let summary = tokio::time::timeout(Duration::from_secs(180), turn)
        .await
        .expect("cu21 turn timed out")
        .expect("cu21 turn task")
        .expect("cu21 turn");
    assert_eq!(summary.stop_reason, StopReason::Completed);
    let offered = script.offered.lock().expect("offered").clone();
    assert!(
        offered
            .iter()
            .all(|names| names.iter().any(|name| name == "computer_native")),
        "vision model must be offered computer_native: {offered:?}"
    );
    wait_for_guest("cu21-marked", "cu21-marked", None).await;

    let jpeg = persisted_jpeg(&adapter, &session).await;
    std::fs::write(EVIDENCE_JPEG, &jpeg).expect("write observation jpeg");

    let scope = pawork_computer_use::target::Scope::new("ws-default", "run-cu21");
    {
        let core = locked.read().await;
        assert!(
            !core
                .native
                .active_scopes()
                .iter()
                .any(|open| open == &scope),
            "turn end closes the native scope"
        );
    }
    let other = serde_json::to_string(&("ws-default", "run-cu21-later")).expect("scope json");
    pawork_computer_use::Computer::claim_shared(&other).expect("desktop occupancy was released");
    pawork_computer_use::Computer::isolated().release(&other);

    drop(locked);
    adapter.shutdown().await.expect("shutdown");
}
