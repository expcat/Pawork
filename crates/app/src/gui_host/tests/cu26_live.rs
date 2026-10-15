//! CU-26: one Wayland guest-seat turn on the `pawork gui serve` assembly.
//! The provider is scripted; every approval travels over the token-authenticated
//! local socket the Desktop uses. Missing PAWORK_CU26_WAYLAND=1 fails the test.

use super::*;
use async_trait::async_trait;
use pawork_client::{ClientConfig, ClientError, GuiClient};
use pawork_domain::{
    ApprovalDecision, CancellationToken, CanonicalModelRequest, ContentPart, ImageSource,
    MessageRole, ModelCapabilities, ModelId, ModelProvider, ModelResponseSummary, ProviderError,
    ProviderErrorKind, ProviderEventSink, ProviderId, ProviderStreamEvent, SessionId, StopReason,
    TokenUsage, ToolCallId,
};
use pawork_gui_server::GuiHost;
use pawork_protocol::client_auth::{TokenAuthenticator, TokenStore, TOKEN_SCHEME};
use pawork_protocol::{
    ActorIdentity, AppCommand, AppEvent, AppQuery, AppResponse, ClientAuthentication,
    CommandSource, ComputerAppFamily, ComputerAppIdentity, ComputerGrantKind,
    ComputerTargetApproveParams, ComputerTargetIdentity, RunState, MAX_OBSERVATION_READ_BYTES,
};
use pawork_transport::{ConnectOptions, GuiListener, LocalTransport, TransportEndpoint};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BUNDLE: &str = "guest.wayland.cu26target";
const TEXT: &str = "Pawork CU-26 formal {ok}";

struct Gate {
    waiting: AtomicBool,
    release: tokio::sync::Notify,
}

struct LastResult {
    is_error: bool,
    text: String,
    metadata: Value,
    jpeg: Option<Vec<u8>>,
}

struct Cu26Provider {
    provider_id: ProviderId,
    gate: Arc<Gate>,
    round: AtomicUsize,
    offered: Mutex<Vec<Vec<String>>>,
    trace: Mutex<Vec<Value>>,
    leases: Mutex<Vec<String>>,
    failure: Mutex<Option<String>>,
}

#[async_trait]
impl ModelProvider for Cu26Provider {
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
        let last = last_result(request);
        if let Some(last) = &last {
            self.trace.lock().expect("trace").push(json!({
                "after_round": round.checked_sub(1),
                "is_error": last.is_error,
                "text": last.text,
                "metadata": last.metadata,
                "jpeg_bytes": last.jpeg.as_ref().map(Vec::len),
            }));
        }
        let arguments = match self.next(round, last.as_ref(), &cancel).await {
            Ok(Some(arguments)) => arguments,
            Ok(None) => {
                sink.emit(ProviderStreamEvent::TextDelta(format!("typed {TEXT}")))
                    .await?;
                sink.emit(ProviderStreamEvent::ResponseCompleted(
                    StopReason::Completed,
                ))
                .await?;
                return Ok(done(StopReason::Completed));
            }
            Err(message) => {
                *self.failure.lock().expect("failure") = Some(message.clone());
                return Err(ProviderError::new(
                    ProviderErrorKind::InvalidRequest,
                    message,
                ));
            }
        };
        self.trace
            .lock()
            .expect("trace")
            .push(json!({"round": round, "arguments": arguments}));
        emit_tool(sink, &format!("cu26-{round}"), arguments).await
    }
}

impl Cu26Provider {
    async fn next(
        &self,
        round: usize,
        last: Option<&LastResult>,
        cancel: &CancellationToken,
    ) -> Result<Option<Value>, String> {
        let ok = |what: &str| -> Result<&LastResult, String> {
            match last {
                Some(result) if !result.is_error => Ok(result),
                Some(result) => Err(format!("{what} failed: {}", result.text)),
                None => Err(format!("{what} has no tool result")),
            }
        };
        let rejected = |what: &str, needle: &str| -> Result<(), String> {
            match last {
                Some(result) if result.is_error && result.text.contains(needle) => Ok(()),
                Some(result) => Err(format!(
                    "{what} must be rejected with {needle:?}: {}",
                    result.text
                )),
                None => Err(format!("{what} has no tool result")),
            }
        };
        let dispatched = |what: &str| -> Result<String, String> {
            let result = ok(what)?;
            let meta = &result.metadata;
            if meta["outcome"] != "dispatched" || meta["input_dispatched"] != true {
                return Err(format!("{what} receipt: {}", result.text));
            }
            if !meta["observation_failure"].is_null() || result.jpeg.is_none() {
                return Err(format!(
                    "{what} returned no fresh observation: {}",
                    result.text
                ));
            }
            meta["observation"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{what} returned no observation id: {}", result.text))
        };
        Ok(Some(match round {
            0 => json!({"action": "discover"}),
            1 => {
                ok("discover")?;
                json!({"action": "request", "bundle_id": BUNDLE})
            }
            2 => {
                ok("request")?;
                json!({"action": "bind", "bundle_id": BUNDLE})
            }
            3 => {
                rejected("bind before GUI target approval", "not authorized")?;
                let release = self.gate.release.notified();
                tokio::pin!(release);
                release.as_mut().enable();
                self.gate.waiting.store(true, Ordering::SeqCst);
                tokio::select! {
                    _ = &mut release => {}
                    _ = cancel.cancelled() => return Err("cu26 approval wait cancelled".into()),
                }
                json!({"action": "bind", "bundle_id": BUNDLE})
            }
            4 => {
                let bound = ok("bind")?;
                let window = bound.metadata["window_id"]
                    .as_str()
                    .ok_or_else(|| format!("bind returned no window: {}", bound.text))?;
                json!({"action": "screenshot", "window": window})
            }
            5 => {
                let shot = ok("screenshot")?;
                let observation = shot.metadata["observation"]
                    .as_str()
                    .ok_or_else(|| format!("screenshot returned no lease: {}", shot.text))?;
                let (width, height) =
                    shot.jpeg.as_deref().and_then(jpeg_size).ok_or_else(|| {
                        format!("screenshot has no decodable jpeg: {}", shot.text)
                    })?;
                self.leases.lock().expect("leases").push(observation.into());
                json!({
                    "action": "observed_click",
                    "observation": observation,
                    "x": f64::from(width) / 2.0,
                    "y": f64::from(height) / 2.0
                })
            }
            6 => {
                let fresh = dispatched("observed_click")?;
                let mut leases = self.leases.lock().expect("leases");
                let spent = leases.first().cloned().expect("screenshot lease");
                leases.push(fresh);
                json!({"action": "observed_keys", "observation": spent, "text": "stale"})
            }
            7 => {
                rejected("spent screenshot lease", "expired or already consumed")?;
                let fresh = self.leases.lock().expect("leases").last().cloned();
                json!({"action": "observed_keys", "observation": fresh, "text": TEXT})
            }
            8 => {
                let fresh = dispatched("observed_keys text")?;
                json!({"action": "observed_keys", "observation": fresh, "key": "return"})
            }
            _ => {
                dispatched("observed_keys return")?;
                return Ok(None);
            }
        }))
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

fn last_result(request: &CanonicalModelRequest) -> Option<LastResult> {
    let message = request
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::Tool)?;
    message.content.iter().rev().find_map(|part| {
        let ContentPart::ToolResult(result) = part else {
            return None;
        };
        let mut text = Vec::new();
        let mut jpeg = None;
        for part in &result.content {
            match part {
                ContentPart::Text(content) => text.push(content.text.as_str()),
                ContentPart::Image(image) => {
                    if let ImageSource::Base64(data) = &image.source {
                        jpeg = crate::services::extension::base64_decode(data).ok();
                    }
                }
                _ => {}
            }
        }
        Some(LastResult {
            is_error: result.is_error,
            text: text.join("\n"),
            metadata: result.metadata.clone(),
            jpeg,
        })
    })
}

/// Width and height from the first baseline/progressive SOF segment.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut index = 2;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xFF {
            return None;
        }
        let marker = bytes[index + 1];
        let length = usize::from(u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]));
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]);
            let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]);
            return Some((u32::from(width), u32::from(height)));
        }
        index += 2 + length;
    }
    None
}

fn evidence_dir() -> std::path::PathBuf {
    let dir = std::env::var("PAWORK_CU26_EVIDENCE").expect("PAWORK_CU26_EVIDENCE is required");
    let dir = std::path::PathBuf::from(dir);
    assert!(dir.is_dir(), "evidence dir {dir:?} must exist");
    dir
}

fn write_json(dir: &std::path::Path, name: &str, value: &Value) {
    let text = serde_json::to_string_pretty(value).expect("evidence json");
    std::fs::write(dir.join(name), text).unwrap_or_else(|error| panic!("write {name}: {error}"));
}

fn gui_source(client: &GuiClient) -> CommandSource {
    CommandSource::RemoteGui {
        client_id: client.client_id().clone(),
        connection_id: client.connection_id().clone(),
    }
}

fn gui_identity() -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: pawork_domain::ActorId::from("cu26-gui-driver"),
        display_name: None,
    }
}

fn connect_options() -> ConnectOptions {
    ConnectOptions {
        timeout_ms: 5_000,
        client_label: Some("cu26-gui-driver".into()),
        max_frame_bytes: pawork_transport::DEFAULT_MAX_FRAME_BYTES,
    }
}

async fn connect(endpoint: &TransportEndpoint, proof: &str) -> Result<GuiClient, ClientError> {
    GuiClient::connect_with_config(
        Arc::new(LocalTransport::default()),
        endpoint.clone(),
        connect_options(),
        Some(ClientAuthentication {
            scheme: TOKEN_SCHEME.into(),
            proof: proof.into(),
        }),
        ClientConfig {
            client_name: "cu26-gui-driver".into(),
            ..ClientConfig::default()
        },
    )
    .await
}

async fn command(gui: &GuiClient, command: AppCommand, what: &str) -> AppResponse {
    gui.command(command, gui_source(gui), gui_identity())
        .await
        .unwrap_or_else(|error| panic!("wire {what}: {error}"))
        .response
}

async fn data(gui: &GuiClient, query: AppQuery, what: &str) -> Value {
    let response = gui
        .query(query, gui_source(gui), gui_identity())
        .await
        .unwrap_or_else(|error| panic!("wire {what}: {error}"))
        .response;
    let AppResponse::Data(data) = response else {
        panic!("wire {what} returns data: {response:?}");
    };
    data
}

async fn read_observation(gui: &GuiClient, session: &SessionId, entry: &Value) -> Vec<u8> {
    let total = entry["byte_length"].as_u64().expect("byte_length");
    let sequence = entry["sequence"].as_u64().expect("sequence");
    let part_index = entry["part_index"].as_u64().expect("part_index") as u32;
    let mut bytes = Vec::new();
    while (bytes.len() as u64) < total {
        let offset = bytes.len() as u64;
        let image = data(
            gui,
            AppQuery::ObservationImageRead {
                session_id: session.clone(),
                sequence,
                part_index,
                offset,
                limit: (total - offset).min(MAX_OBSERVATION_READ_BYTES),
            },
            "ObservationImageRead",
        )
        .await;
        let chunk = crate::services::extension::base64_decode(
            image["data"].as_str().expect("base64 chunk"),
        )
        .expect("decode observation chunk");
        assert!(!chunk.is_empty(), "empty observation chunk at {offset}");
        bytes.extend(chunk);
        if image["eof"] == true {
            break;
        }
    }
    assert_eq!(bytes.len() as u64, total);
    bytes
}

fn target() -> ComputerTargetIdentity {
    ComputerTargetIdentity::Application {
        app: ComputerAppIdentity {
            bundle_id: BUNDLE.into(),
            family: ComputerAppFamily::Browser,
        },
    }
}

async fn wait_for_target_facts(dir: &std::path::Path) -> (Vec<u8>, String) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let click = std::fs::read(dir.join("click.bin")).unwrap_or_default();
        let typed = std::fs::read_to_string(dir.join("typed.txt")).unwrap_or_default();
        if click.len() == 12 && !typed.is_empty() {
            return (click, typed);
        }
        assert!(
            Instant::now() < deadline,
            "target facts missing: click={click:?} typed={typed:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires PAWORK_CU26_WAYLAND=1, a running computer_guest --wayland and its Sway target"]
async fn cu26_wayland_gui_serve_assembly() {
    match std::env::var("PAWORK_CU26_WAYLAND") {
        Ok(value) if value == "1" => {}
        other => panic!("PAWORK_CU26_WAYLAND=1 is required, got {other:?}"),
    }
    assert!(
        std::env::var("PAWORK_ISOLATED_GUEST_TOKEN").is_ok_and(|token| !token.is_empty()),
        "PAWORK_ISOLATED_GUEST_TOKEN must match the running guest"
    );
    let evidence = evidence_dir();

    let workspace = tempfile::tempdir().expect("workspace");
    let data_dir = tempfile::tempdir().expect("data dir");
    let backend: Arc<dyn pawork_auth::SecretBackend> = Arc::new(pawork_auth::MemoryBackend::new());
    let mut core = crate::AppCore::load_for_catalog(crate::AppLoadOptions {
        workspace_root: Some(workspace.path().to_path_buf()),
        data_dir: Some(data_dir.path().to_path_buf()),
        approval_mode: Some(crate::ApprovalMode::AskForWrites),
        trust_workspaces: Some(true),
        approval_host: Some(Arc::new(GuiApprovalHost::new())),
        auth_backend: Some(backend),
        instance: "cu26-live".into(),
        instance_role: crate::InstanceRole::GuiHost,
        computer_native: crate::ComputerNativeMode::Wayland,
        ..crate::AppLoadOptions::default()
    })
    .await
    .expect("load gui host core");
    assert_eq!(core.computer_native, crate::ComputerNativeMode::Wayland);
    assert!(std::ptr::eq(
        Arc::as_ptr(&core.native) as *const (),
        Arc::as_ptr(&pawork_computer_use::wayland_surface::shared_wayland_host()) as *const (),
    ));
    assert_eq!(core.workspace_id().as_str(), "ws-default");

    let provider_id = core.provider_id.clone();
    let auth_revision = core.provider_auth_revision;
    let gate = Arc::new(Gate {
        waiting: AtomicBool::new(false),
        release: tokio::sync::Notify::new(),
    });
    let script = Arc::new(Cu26Provider {
        provider_id: provider_id.clone(),
        gate: Arc::clone(&gate),
        round: AtomicUsize::new(0),
        offered: Mutex::new(Vec::new()),
        trace: Mutex::new(Vec::new()),
        leases: Mutex::new(Vec::new()),
        failure: Mutex::new(None),
    });
    core.provider = script.clone();
    core.provider_pending = false;
    core.provider_stale = false;
    assert_eq!(core.provider_auth_revision, auth_revision);
    assert!(!core.provider_needs_rebuild());
    Arc::make_mut(&mut core.registry)
        .register(pawork_models::CatalogEntry {
            id: ModelId::from("cu26-vision"),
            provider: provider_id.clone(),
            display_name: "cu26-vision".into(),
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
        .expect("register cu26-vision");
    core.model = ModelId::from("cu26-vision");

    // Same order as `pawork gui serve`: approval host, adapter, token, socket.
    let approvals = Arc::new(GuiApprovalHost::new());
    core.set_approval_host(approvals.clone());
    let locked = Arc::new(tokio::sync::RwLock::new(core));
    let connections = Arc::new(pawork_gui_server::ConnectionManager::new());
    let adapter = GuiHostAdapter::from_locked(Arc::clone(&locked), approvals)
        .with_connections(Arc::clone(&connections));
    let token_path = data_dir.path().join("gui.token");
    let store = TokenStore::new(&token_path);
    store.generate().expect("generate gui token");
    let proof = std::fs::read_to_string(&token_path)
        .expect("read gui token")
        .trim()
        .to_string();
    let handshake = pawork_protocol::HandshakeService::new(
        adapter.instance_id(),
        pawork_protocol::SUPPORTED_API_VERSIONS.to_vec(),
        pawork_protocol::app::registry::gui_supported_capabilities(),
    )
    .with_host_data_dir(data_dir.path().to_string_lossy().into_owned())
    .with_authenticator(Box::new(TokenAuthenticator::new(store)));
    let server = pawork_gui_server::GuiServer::new(pawork_gui_server::GuiServerConfig {
        host: Arc::new(adapter.clone()),
        handshake,
        transport: Arc::new(LocalTransport::default()),
        connections: Some(Arc::clone(&connections)),
    });
    let socket = data_dir.path().join("gui.sock");
    let endpoint = TransportEndpoint::Local {
        address: socket.to_string_lossy().into_owned(),
    };
    let listener: Arc<dyn GuiListener> = Arc::from(
        server
            .bind(endpoint.clone())
            .await
            .expect("bind gui socket"),
    );
    drop(server);
    let accepting = {
        let listener = Arc::clone(&listener);
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok(connection) = listener.accept().await {
                held.push(connection);
            }
            held
        })
    };

    let refused = connect(&endpoint, "not-the-gui-token")
        .await
        .err()
        .expect("a wrong token must not complete the handshake");
    assert!(refused.is_auth_failure(), "{refused:?}");

    let gui = connect(&endpoint, &proof).await.expect("gui handshake");
    gui.subscribe_all().await.expect("subscribe");
    let created = command(
        &gui,
        AppCommand::SessionCreate {
            workspace_id: Some(pawork_domain::WorkspaceId::from("ws-default")),
            title: Some("cu26 wayland".into()),
        },
        "SessionCreate",
    )
    .await;
    let AppResponse::Data(created) = created else {
        panic!("SessionCreate returns data: {created:?}");
    };
    let session = SessionId::from(created["session_id"].as_str().expect("session_id"));
    let started = command(
        &gui,
        AppCommand::RunStart {
            session_id: session.clone(),
            user_message: format!("Click the cu26target terminal, then type {TEXT} and Return."),
            model: None,
            provider: None,
            profile: None,
            effort: None,
            attachment_ids: Vec::new(),
            web_search: None,
            video_urls: Vec::new(),
        },
        "RunStart",
    )
    .await;
    let AppResponse::Accepted {
        run_id: Some(run), ..
    } = started
    else {
        panic!("RunStart must be accepted: {started:?}");
    };

    let mut names: HashMap<ToolCallId, String> = HashMap::new();
    let mut approved = Vec::new();
    let mut completed = Vec::new();
    let mut before_grant = Value::Null;
    let mut receipt = Value::Null;
    let deadline = Instant::now() + Duration::from_secs(240);
    let terminal = loop {
        if gate.waiting.load(Ordering::SeqCst) && receipt.is_null() {
            before_grant = data(
                &gui,
                AppQuery::ComputerTargetList {
                    session_id: session.clone(),
                },
                "ComputerTargetList before approval",
            )
            .await;
            let response = command(
                &gui,
                AppCommand::ComputerTargetApprove(ComputerTargetApproveParams {
                    session_id: session.clone(),
                    run_id: run.clone(),
                    target: target(),
                    kind: ComputerGrantKind::ForRun,
                }),
                "ComputerTargetApprove",
            )
            .await;
            let AppResponse::Data(granted) = response else {
                panic!("ComputerTargetApprove returns data: {response:?}");
            };
            receipt = granted;
            gate.release.notify_one();
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_else(|| panic!("run did not finish; trace={:?}", script.trace));
        let envelope = match gui
            .next_event_timeout(remaining.min(Duration::from_millis(200)))
            .await
        {
            Ok(envelope) => envelope,
            Err(ClientError::Timeout { .. }) => continue,
            Err(error) => panic!("event stream failed: {error}"),
        };
        gui.ack(envelope.global_sequence).await.expect("ack");
        match envelope.payload {
            AppEvent::ToolStarted {
                run_id,
                tool_call_id,
                name,
            } if run_id == run => {
                names.insert(tool_call_id, name);
            }
            AppEvent::ToolApprovalRequired {
                run_id,
                tool_call_id,
                reason,
            } if run_id == run => {
                let name = names.get(&tool_call_id).cloned();
                assert_eq!(name.as_deref(), Some("computer_native"), "{reason}");
                let response = command(
                    &gui,
                    AppCommand::ToolApprove {
                        run_id: run.clone(),
                        tool_call_id: tool_call_id.clone(),
                        decision: pawork_protocol::ApprovalDecision::ApproveOnce,
                    },
                    "ToolApprove",
                )
                .await;
                assert!(
                    matches!(response, AppResponse::Accepted { .. }),
                    "{response:?}"
                );
                approved.push(json!({"tool_call_id": tool_call_id, "reason": reason}));
            }
            AppEvent::ToolCompleted {
                run_id,
                tool_call_id,
                success,
            } if run_id == run => {
                completed.push(json!({"tool_call_id": tool_call_id, "success": success}));
            }
            AppEvent::RunChanged { run_id, state }
                if run_id == run
                    && matches!(
                        state,
                        RunState::Completed | RunState::Cancelled | RunState::Failed
                    ) =>
            {
                break state;
            }
            _ => {}
        }
    };
    let trace = script.trace.lock().expect("trace").clone();
    write_json(&evidence, "tool-trace.json", &Value::Array(trace.clone()));
    assert_eq!(
        terminal,
        RunState::Completed,
        "failure={:?} trace={trace:#?}",
        script.failure.lock().expect("failure")
    );
    let offered = script.offered.lock().expect("offered").clone();
    assert!(
        offered
            .iter()
            .all(|names| names.iter().any(|name| name == "computer_native")),
        "vision model must be offered computer_native: {offered:?}"
    );

    assert_eq!(approved.len(), 9, "one GUI approval per tool call");
    let failed: Vec<_> = completed
        .iter()
        .filter(|entry| entry["success"] == false)
        .collect();
    assert_eq!(
        failed.len(),
        2,
        "unapproved bind and spent lease: {completed:?}"
    );
    let listed: &Vec<Value> = before_grant["targets"].as_array().expect("targets");
    let pending = listed
        .iter()
        .find(|state| state["target"]["app"]["bundle_id"] == BUNDLE)
        .unwrap_or_else(|| panic!("requested target listed before approval: {before_grant}"));
    assert!(pending.get("grant").is_none(), "{pending}");
    assert_eq!(receipt["kind"], "for_run", "{receipt}");
    assert_eq!(receipt["target"]["app"]["bundle_id"], BUNDLE, "{receipt}");

    let observations = data(
        &gui,
        AppQuery::ObservationList {
            session_id: session.clone(),
            after_sequence: None,
            limit: None,
        },
        "ObservationList",
    )
    .await;
    let entries = observations["entries"].as_array().expect("entries").clone();
    assert_eq!(
        entries.len(),
        4,
        "screenshot plus one fresh observation per dispatched action: {observations}"
    );
    let mut images = Vec::new();
    for entry in &entries {
        assert_eq!(entry["tool_name"], "computer_native", "{entry}");
        assert_eq!(entry["media_type"], "image/jpeg", "{entry}");
        assert_eq!(entry["status"], "historical", "{entry}");
        let jpeg = read_observation(&gui, &session, entry).await;
        let (width, height) = jpeg_size(&jpeg).expect("persisted jpeg decodes");
        let name = format!("observation-{}.jpg", entry["sequence"]);
        std::fs::write(evidence.join(&name), &jpeg).expect("write observation");
        images.push(json!({"file": name, "width": width, "height": height}));
    }

    let after_run = data(
        &gui,
        AppQuery::ComputerTargetList {
            session_id: session.clone(),
        },
        "ComputerTargetList after run",
    )
    .await;
    assert!(
        after_run["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .all(|state| state.get("grant").is_none()),
        "run end closes the grant: {after_run}"
    );

    let (click, typed) = wait_for_target_facts(&evidence).await;
    assert_eq!(&click[..4], &[27, 91, 77, 32], "left press: {click:?}");
    assert_eq!(&click[6..10], &[27, 91, 77, 35], "release: {click:?}");
    assert_eq!(typed, TEXT);

    let replay = adapter
        .session_store()
        .await
        .expect("store")
        .replay_events(&session, 1, usize::MAX)
        .await
        .expect("replay");
    let mut persisted: HashMap<&'static str, usize> = HashMap::new();
    for envelope in &replay {
        let kind = match &envelope.payload {
            pawork_domain::AgentEvent::ToolApprovalRequested { .. } => "tool_approval_requested",
            pawork_domain::AgentEvent::ToolApprovalResponded { decision, .. } => {
                assert_eq!(*decision, ApprovalDecision::ApprovedOnce);
                "tool_approval_responded"
            }
            pawork_domain::AgentEvent::ToolExecutionCompleted { .. } => "tool_execution_completed",
            pawork_domain::AgentEvent::RunCompleted { .. } => "run_completed",
            _ => continue,
        };
        *persisted.entry(kind).or_default() += 1;
    }
    assert_eq!(persisted.get("tool_approval_requested"), Some(&9));
    assert_eq!(persisted.get("tool_approval_responded"), Some(&9));
    assert_eq!(persisted.get("tool_execution_completed"), Some(&9));

    write_json(
        &evidence,
        "formal.json",
        &json!({
            "assembly": {
                "instance_role": "gui_host",
                "computer_native": "wayland",
                "approval_host": "GuiApprovalHost",
                "transport": "LocalTransport unix socket",
                "authenticator": "TokenAuthenticator",
                "client": "pawork-client GuiClient",
                "provider": {"id": provider_id.as_str(), "model": "cu26-vision", "kind": "scripted"},
            },
            "wrong_token": format!("{refused}"),
            "session_id": session.as_str(),
            "run_id": run.as_str(),
            "terminal": format!("{terminal:?}"),
            "tool_approvals": approved,
            "tool_completed": completed,
            "target_list_before_approval": before_grant,
            "target_approve_receipt": receipt,
            "target_list_after_run": after_run,
            "observation_list": observations,
            "observation_images": images,
            "persisted_events": persisted,
            "target_facts": {"click_bin": click, "typed": typed},
        }),
    );

    let _ = gui.close().await;
    let _ = listener.close().await;
    let held = accepting.await.expect("accept task");
    for connection in held {
        let _ = connection.close().await;
    }
    drop(listener);
    for _ in 0..200 {
        if Arc::strong_count(&locked) == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(locked);
    adapter.shutdown().await.expect("shutdown");
}
