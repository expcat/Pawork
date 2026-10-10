//! Pawork UI fixture 工具（dev-only example；CLI 冻结，W2 脚本按此调用）。
//!
//! 子命令（全部要求显式 --root）：
//! - seed：读 fixtures/ui/seed.json，把数据集写入隔离 root；
//! - serve：以真实 GuiServer + fixture 脚本 provider 起 host，写 barrier；
//! - self-check：内置 client 验证 snapshot / RunStart / Resume Replay；
//! - snapshot-dump：进程内 snapshot() 归一化后写 JSON（Phase C golden 基线）。
//!
//! serve 额外消费 barrier（只在本 example，不进生产二进制）：
//! register_simulated_target.request 为 JSON {"bundle_id":"..."} 或空文件，
//! 把模拟应用登记进当前唯一活动 GUI run 的真实 TargetRegistry，不解析、
//! 不启动、不探测本机进程，成功写 register_simulated_target.done。
//! dump_targets.request 写 dump_targets.done，内容是活动 run 与登记快照。
//! fixture:observe-hang <http(s)-url>：navigate 后两次真实 browser screenshot，
//! 然后停在模型轮询里等取消；barriers/observe_more.request 再补一次截图。
//! 图像只来自 Desktop WebView 的正式工具回执，不写假观测。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use pawork_app::devfixture::{self, SeedSpec};
use pawork_app::{AppCore, GuiHostAdapter};
use pawork_domain::{
    CancellationToken, CanonicalModelRequest, CommandId, ContentPart, ImageSource, MessageRole,
    ModelCapabilities, ModelDefinition, ModelId, ModelProvider, ModelResponseSummary,
    ProviderError, ProviderErrorKind, ProviderEventSink, ResolvedCredential, SessionId, StopReason,
    TextContent, Timestamp, TokenUsage, WorkspaceId,
};
use pawork_gui_server::{GuiHost, GuiServer, GuiServerConfig};
use pawork_protocol::app::registry::gui_supported_capabilities;
use pawork_protocol::client_auth::{TokenAuthenticator, TokenStore, TOKEN_SCHEME};
use pawork_protocol::{
    decode_server_frame, encode_client_frame, ActorIdentity, ApiVersion, AppCommand,
    AppCommandEnvelope, AppEvent, AppResponse, ClientAuthentication, ClientFrame, CommandSource,
    GlobalSequence, GuiCapability, HandshakeRequest, HandshakeResponse, HandshakeService,
    ResumeDisposition, ResumeRequest, RunState, ServerFrame, Snapshot, SnapshotSectionKind,
    SubscribeRequest, SUPPORTED_API_VERSIONS,
};
use pawork_storage::session::SessionStore;
use pawork_testkit::{MockProvider, MockScript};
use pawork_transport::{
    ConnectOptions, GuiConnection, GuiTransportClient, GuiTransportServer, LocalTransport,
    TransportEndpoint, TransportFrame,
};
use serde_json::{json, Value};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().cloned() else {
        usage_and_exit();
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let result = runtime.block_on(async {
        match command.as_str() {
            "seed" => cmd_seed(&args[1..]).await,
            "serve" => cmd_serve(&args[1..]).await,
            "self-check" => cmd_self_check(&args[1..]).await,
            "snapshot-dump" => cmd_snapshot_dump(&args[1..]).await,
            _ => Err(format!("未知子命令 {command:?}")),
        }
    });
    if let Err(error) = result {
        eprintln!("ui_fixture: {error}");
        std::process::exit(1);
    }
}

fn usage_and_exit() -> ! {
    eprintln!(
        "用法：ui_fixture <seed|serve|self-check|snapshot-dump> --root <dir> [--now-ms <i64>] [--out <file>] [--profile <default|r6-terminal|r6-resources|r6-read-only>]"
    );
    std::process::exit(2);
}

struct ParsedArgs {
    root: PathBuf,
    now_ms: Option<i64>,
    out: Option<PathBuf>,
    profile: Option<String>,
}

fn parse_args(args: &[String], need_out: bool) -> Result<ParsedArgs, String> {
    let mut root = None;
    let mut now_ms = None;
    let mut out = None;
    let mut profile = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let Some(value) = args.get(index + 1) else {
            return Err(format!("参数 {flag:?} 缺少值"));
        };
        match flag {
            "--root" => root = Some(PathBuf::from(value)),
            "--now-ms" => {
                now_ms = Some(value.parse::<i64>().map_err(|_| "--now-ms 需要 i64")?);
            }
            "--out" => out = Some(PathBuf::from(value)),
            "--profile" => profile = Some(value.clone()),
            other => return Err(format!("未知参数 {other:?}")),
        }
        index += 2;
    }
    let root = root.ok_or("缺少 --root <dir>")?;
    if need_out && out.is_none() {
        return Err("缺少 --out <file>".into());
    }
    Ok(ParsedArgs {
        root,
        now_ms,
        out,
        profile,
    })
}

fn seed_json_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/ui/seed.json")
}

fn pty_script_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/ui/pty-fixture.sh")
}

/// serve 进程内的证据目录。只给 dev example 的 browser 脚本落 JPEG，不进生产二进制。
fn fixture_log_dir() -> &'static OnceLock<PathBuf> {
    static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();
    &LOG_DIR
}

fn fixture_barrier_dir() -> &'static OnceLock<PathBuf> {
    static BARRIER_DIR: OnceLock<PathBuf> = OnceLock::new();
    &BARRIER_DIR
}

fn note_browser_trace(line: &str) {
    eprintln!("[ui_fixture] {line}");
    let Some(dir) = fixture_log_dir().get() else {
        return;
    };
    let path = dir.join("browser-script.log");
    let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
    existing.push_str(line);
    existing.push('\n');
    let _ = std::fs::write(path, existing);
}

fn note_browser_image(data: &str) {
    let Some(dir) = fixture_log_dir().get() else {
        return;
    };
    let path = dir.join("browser-screenshot.b64");
    if path.is_file() {
        return;
    }
    if std::fs::write(&path, data).is_ok() {
        note_browser_trace(&format!("saved screenshot base64 bytes={}", data.len()));
    }
}

fn truncate_trace(text: &str) -> String {
    let mut end = text.len().min(180);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].replace('\n', " ")
}

fn load_spec() -> Result<SeedSpec, String> {
    let text = std::fs::read_to_string(seed_json_path())
        .map_err(|error| format!("读取 seed.json 失败：{error}"))?;
    let spec =
        serde_json::from_str(&text).map_err(|error| format!("解析 seed.json 失败：{error}"))?;
    devfixture::validate_spec(&spec)?;
    Ok(spec)
}

fn require_seeded_root(root: &Path) -> Result<(), String> {
    devfixture::validate_root(root)?;
    if !devfixture::fixture_marker_ready(root) {
        return Err(format!(
            "{} 缺少 ready fixture marker（可能是未完成的 seed），请重新运行 seed",
            root.display()
        ));
    }
    if !root.join("data/session.db").is_file() {
        return Err(format!("{} 下没有 data/session.db", root.display()));
    }
    Ok(())
}

/// serve / self-check / snapshot-dump 共用的 fixture host 装配。
async fn fixture_core(root: &Path, spec: &SeedSpec, profile: &str) -> Result<AppCore, String> {
    let (store, _) = SessionStore::open(root.join("data/session.db"))
        .await
        .map_err(|error| format!("打开 session.db 失败：{error}"))?;
    let mut core = AppCore::from_parts(
        Arc::new(FixtureDispatchProvider),
        None,
        ModelId::from("fixture-model"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    devfixture::configure_fixture_host_profile(&mut core, profile)?;
    let workspaces = devfixture::resolve_workspaces(spec, root)?;
    devfixture::attach_fixture_workspaces(&mut core, &workspaces)
        .map_err(|error| error.to_string())?;
    core.open_checkpoints(root.join("data/checkpoints"))
        .await
        .map_err(|error| error.to_string())?;
    devfixture::bind_fixture_sessions(&core, spec);
    // RunStart 的 browser 视觉闸读已合并目录，不读当场的 list_models。
    // 沿正式 switch_model → refresh_provider_registry 把本次声明写进目录。
    let logs = root.join("logs");
    std::fs::create_dir_all(&logs).map_err(|error| format!("创建 fixture logs 失败：{error}"))?;
    let _ = fixture_log_dir().set(logs);
    let _ = fixture_barrier_dir().set(barrier_dir(root));
    core.switch_model(None, "fixture-model")
        .await
        .map_err(|error| format!("fixture model discovery failed: {error}"))?;
    Ok(core)
}

/// serve 模式脚本 provider：按 user 文本首行前缀分派（§5 冻结合同）。
/// 只存在于 dev example，不进任何生产二进制。
struct FixtureDispatchProvider;

#[async_trait]
impl ModelProvider for FixtureDispatchProvider {
    fn id(&self) -> pawork_domain::ProviderId {
        pawork_domain::ProviderId::from("mock")
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        Ok(vec![ModelDefinition {
            id: ModelId::from("fixture-model"),
            display_name: "Fixture model".into(),
            context_window_tokens: 128_000,
            max_output_tokens: 8_192,
            capabilities: ModelCapabilities {
                text: true,
                image_input: true,
                tool_calls: true,
                ..ModelCapabilities::default()
            },
        }])
    }

    async fn stream(
        &self,
        request: &CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        eprintln!("[ui_fixture] provider stream called");
        let script = match observe_hang_turn(request, &cancel).await {
            Some(script) => script,
            None => script_for_request(request),
        };
        let outcome = MockProvider::new(script)
            .stream(request, sink, cancel)
            .await;
        eprintln!("[ui_fixture] provider stream done: ok={}", outcome.is_ok());
        outcome
    }
}

fn latest_user_line(request: &CanonicalModelRequest) -> String {
    request
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text(TextContent { text }) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 正式 browser 工具链上的有界观测：一次导航、两张截图，然后停住。
/// 停住时返回 None，由 observe_hang_turn 等取消或补拍。
fn observe_hang_action(request: &CanonicalModelRequest, url: &str) -> Option<MockScript> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Some(
            MockScript::new()
                .text("fixture observe stopped: url must be http(s)")
                .complete(),
        );
    }
    let seen = seen_tools(request);
    if let Some(last) = seen.last() {
        if last.is_error {
            let detail = truncate_trace(&last.text);
            note_browser_trace(&format!("observe-hang stop error {detail}"));
            return Some(
                MockScript::new()
                    .text(format!("fixture observe stopped: {detail}"))
                    .complete(),
            );
        }
    }
    let shots = seen.iter().filter(|item| is_screenshot(item)).count();
    let reads = seen
        .iter()
        .filter(|item| !item.is_error && item.text.contains("dom_revision"))
        .count();
    if shots >= 2 {
        return None;
    }
    if reads >= 1 {
        note_browser_trace(&format!("observe-hang screenshot shots={shots}"));
        return Some(tool_use("browser", json!({"action": "screenshot"})));
    }
    note_browser_trace(&format!("observe-hang navigate {url}"));
    Some(tool_use("browser", json!({"action":"navigate","url":url})))
}

fn is_screenshot(item: &SeenTool) -> bool {
    // navigate / read 的文本元数据也带 viewport 与 generation，不能据此计数。
    // 真实截图是 Image part；上下文裁剪后只剩 image trimmed 说明。
    item.has_image || item.text.contains("image trimmed:")
}

fn take_observe_more() -> bool {
    let Some(dir) = fixture_barrier_dir().get() else {
        return false;
    };
    let path = dir.join("observe_more.request");
    if !path.is_file() {
        return false;
    }
    let _ = std::fs::remove_file(&path);
    note_browser_trace("observe-hang extra screenshot");
    let _ = write_barrier(dir, "observe_more.done", json!({"action": "screenshot"}));
    true
}

/// fixture:observe-hang 不走 script_for_request：两张截图之后必须停在
/// 这次模型调用里，活动 run 才能继续等停止或补拍。
async fn observe_hang_turn(
    request: &CanonicalModelRequest,
    cancel: &CancellationToken,
) -> Option<MockScript> {
    let prefix = latest_user_line(request);
    let Some(url) = prefix
        .strip_prefix("fixture:observe-hang ")
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return None;
    };
    if let Some(script) = observe_hang_action(request, url) {
        return Some(script);
    }
    note_browser_trace("observe-hang waiting for observe_more or cancel");
    loop {
        if cancel.is_cancelled() || take_observe_more() {
            break;
        }
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    if cancel.is_cancelled() {
        note_browser_trace("observe-hang cancelled");
        return Some(MockScript::new().wait_for_cancellation());
    }
    Some(tool_use("browser", json!({"action": "screenshot"})))
}

fn script_for_request(request: &CanonicalModelRequest) -> MockScript {
    let first_user = request
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text(TextContent { text }) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .unwrap_or_default();
    let prefix = first_user.lines().next().unwrap_or("").trim();
    if let Some(handle) = prefix.strip_prefix("fixture:browser-ttl ").map(str::trim) {
        return browser_ttl_script(request, handle);
    }
    if let Some(url) = prefix.strip_prefix("fixture:browser ").map(str::trim) {
        return browser_script(request, url);
    }
    let has_tool_results = request
        .messages
        .iter()
        .any(|message| message.role == MessageRole::Tool);
    match prefix {
        "fixture:hang" => MockScript::new().wait_for_cancellation(),
        "fixture:fail" => MockScript::new().fail(ProviderError::new(
            ProviderErrorKind::StreamInterrupted,
            "fixture scripted provider failure",
        )),
        "fixture:tool" if !has_tool_results => MockScript::new()
            .tool_call("read_file", json!({"path": "README.md"}))
            .complete_with(StopReason::ToolUse),
        "fixture:tool" => MockScript::new().text("fixture tool finished").complete(),
        _ => MockScript::new()
            .text("fixture chunk 1 ")
            .text("fixture chunk 2 ")
            .text("fixture chunk 3")
            .usage(TokenUsage {
                input_tokens: 32,
                output_tokens: 48,
                ..TokenUsage::default()
            })
            .complete(),
    }
}

struct SeenTool {
    is_error: bool,
    text: String,
    has_image: bool,
}

fn seen_tools(request: &CanonicalModelRequest) -> Vec<SeenTool> {
    let start = request
        .messages
        .iter()
        .rposition(|message| message.role == MessageRole::User)
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut seen = Vec::new();
    for message in request.messages.iter().skip(start) {
        if message.role != MessageRole::Tool {
            continue;
        }
        for part in &message.content {
            let ContentPart::ToolResult(result) = part else {
                continue;
            };
            let mut text = String::new();
            let mut has_image = false;
            for inner in &result.content {
                match inner {
                    ContentPart::Text(text_content) => text.push_str(&text_content.text),
                    ContentPart::Image(image) => {
                        has_image = true;
                        if let ImageSource::Base64(data) = &image.source {
                            note_browser_image(data);
                        }
                    }
                    _ => {}
                }
            }
            seen.push(SeenTool {
                is_error: result.is_error,
                text,
                has_image,
            });
        }
    }
    seen
}

fn tool_use(name: &str, arguments: Value) -> MockScript {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = pawork_domain::ToolCallId::from(format!(
        "browser-tool-{}-{}",
        browser_tool_namespace(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let json = serde_json::to_string(&arguments).expect("browser tool arguments");
    MockScript::new()
        .tool_call_chunks(id, name, [json])
        .complete_with(StopReason::ToolUse)
}

/// tool_calls.tool_call_id 是全库主键。计数每个进程从 1 起，Host 重启会撞上旧行。
/// 前缀在进程内只生成一次：pid 区分进程，墙钟纳秒区分重启；同进程内再靠计数器。
fn browser_tool_namespace() -> &'static str {
    static NAMESPACE: OnceLock<String> = OnceLock::new();
    NAMESPACE
        .get_or_init(|| {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            format!("{}-{nanos}", std::process::id())
        })
        .as_str()
}

/// 有界 browser 链：navigate、read、screenshot、密码拒绝、文件拒绝、再读。
/// 只消费正式工具回执，不执行页面脚本。
fn browser_script(request: &CanonicalModelRequest, url: &str) -> MockScript {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return MockScript::new()
            .text("fixture browser stopped: url must be http(s)")
            .complete();
    }
    let seen = seen_tools(request);
    let rejects = seen
        .iter()
        .filter(|item| item.text.contains("Password and file"))
        .count();
    let reads = seen
        .iter()
        .filter(|item| !item.is_error && item.text.contains("dom_revision"))
        .count();
    let saw_image = seen.iter().any(|item| item.has_image);
    if let Some(last) = seen.last() {
        if last.is_error && !last.text.contains("Password and file") {
            let detail = truncate_trace(&last.text);
            note_browser_trace(&format!("stop unexpected error {detail}"));
            return MockScript::new()
                .text(format!("fixture browser stopped: {detail}"))
                .complete();
        }
    }
    if rejects >= 2 && reads >= 3 {
        note_browser_trace("complete after secret rejects and final read");
        return MockScript::new()
            .text("fixture browser finished")
            .complete();
    }
    if rejects >= 2 {
        note_browser_trace("read after secret rejects");
        return tool_use("browser", json!({"action": "read"}));
    }
    if rejects == 1 {
        note_browser_trace("type file input");
        return tool_use(
            "browser",
            json!({"action":"type","selector":"#file","text":"should-not-land"}),
        );
    }
    if saw_image {
        note_browser_trace("type password input");
        return tool_use(
            "browser",
            json!({"action":"type","selector":"#pw","text":"should-not-land"}),
        );
    }
    if reads >= 2 {
        note_browser_trace("screenshot");
        return tool_use("browser", json!({"action": "screenshot"}));
    }
    if reads >= 1 {
        note_browser_trace("read");
        return tool_use("browser", json!({"action": "read"}));
    }
    note_browser_trace(&format!("navigate {url}"));
    tool_use("browser", json!({"action":"navigate","url":url}))
}

/// 用上一次 read 回执里的句柄再 type。过期由真实 WebView 句柄表拒绝。
fn browser_ttl_script(request: &CanonicalModelRequest, handle: &str) -> MockScript {
    if !handle.starts_with("e-") || handle.len() > 128 {
        return MockScript::new()
            .text("fixture ttl stopped: handle missing")
            .complete();
    }
    let seen = seen_tools(request);
    if let Some(last) = seen.last() {
        if last.text.contains("expired") {
            note_browser_trace("ttl rejected");
            return MockScript::new().text("fixture ttl rejected").complete();
        }
        let detail = truncate_trace(&last.text);
        note_browser_trace(&format!("ttl unexpected {detail}"));
        return MockScript::new()
            .text(format!("fixture ttl stopped: {detail}"))
            .complete();
    }
    note_browser_trace(&format!("ttl type handle {handle}"));
    tool_use(
        "browser",
        json!({"action":"type","handle":handle,"text":"ttl-should-not-land"}),
    )
}

async fn cmd_seed(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, false)?;
    if parsed.profile.is_some() {
        return Err("--profile 只允许用于 serve".into());
    }
    let spec = load_spec()?;
    let outcome = devfixture::seed(&parsed.root, parsed.now_ms, &spec, &pty_script_path()).await?;
    println!("ui_fixture seed ok");
    println!("  root: {}", outcome.root.display());
    println!("  now_ms: {}", outcome.now_ms);
    println!(
        "  workspaces={} sessions={} events={} diff-files={}",
        outcome.workspaces, outcome.sessions, outcome.events, outcome.checkpoints
    );
    println!("  manifest: {}", outcome.manifest.display());
    Ok(())
}

async fn cmd_serve(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, false)?;
    require_seeded_root(&parsed.root)?;
    let spec = load_spec()?;
    let profile = parsed.profile.as_deref().unwrap_or("default");
    let mut core = fixture_core(&parsed.root, &spec, profile).await?;
    let approvals = Arc::new(pawork_app::GuiApprovalHost::new());
    // r6 profile 默认 DenyAll，browser 的 requires_approval 会被立刻拒绝。
    // 仅当验收显式打开时，把同一 GuiApprovalHost 装进 Core，卡片走正式 GUI。
    if std::env::var("PAWORK_UI_GUI_APPROVAL").ok().as_deref() == Some("1") {
        core.set_approval_host(approvals.clone());
    }
    let core = Arc::new(tokio::sync::RwLock::new(core));
    let adapter = GuiHostAdapter::from_locked(Arc::clone(&core), approvals);
    let target_adapter = adapter.clone();
    let pty = adapter.pty();

    let data_dir = parsed.root.join("data");
    let socket_path = data_dir.join("pawork-gui.sock");
    let token_path = data_dir.join("gui.token");
    let store = TokenStore::new(&token_path);
    if token_path.exists() {
        store
            .load()
            .map_err(|error| format!("加载 gui token 失败：{error}"))?;
    } else {
        store
            .generate()
            .map_err(|error| format!("生成 gui token 失败：{error}"))?;
    }

    let log_path = parsed.root.join("logs/serve.log");
    log(&log_path, format!("serve starting profile={profile}"))?;

    let transport = Arc::new(LocalTransport::default());
    let handshake = HandshakeService::new(
        adapter.instance_id(),
        SUPPORTED_API_VERSIONS.to_vec(),
        gui_supported_capabilities(),
    )
    .with_authenticator(Box::new(TokenAuthenticator::new(store)));
    let server = GuiServer::new(GuiServerConfig {
        host: Arc::new(adapter),
        handshake,
        transport: Arc::clone(&transport) as Arc<dyn GuiTransportServer>,
        connections: None,
    });
    let listener = Arc::new(
        server
            .bind(TransportEndpoint::Local {
                address: socket_path.to_string_lossy().to_string(),
            })
            .await
            .map_err(|error| format!("bind 失败：{error}"))?,
    );

    let barriers = barrier_dir(&parsed.root);
    // 消费残留的停机 barrier：重启 host 属于新生命周期，不得继承旧停机请求。
    for name in [
        "serve_stop.request",
        "drop_socket.request",
        "drop_socket.done",
        "register_simulated_target.request",
        "register_simulated_target.done",
        "register_simulated_target.failed",
        "dump_targets.request",
        "dump_targets.done",
        "dump_targets.failed",
        "observe_more.request",
        "observe_more.done",
    ] {
        let _ = std::fs::remove_file(barriers.join(name));
    }
    let target_watch = {
        let barriers = barriers.clone();
        let log_path = log_path.clone();
        tokio::spawn(
            async move { watch_simulated_targets(target_adapter, barriers, log_path).await },
        )
    };
    write_barrier(
        &barriers,
        "host_ready",
        json!({"socket": socket_path.display().to_string()}),
    )?;
    log(
        &log_path,
        format!("host_ready socket={}", socket_path.display()),
    )?;
    println!(
        "ui_fixture serving on {} (barriers {})",
        socket_path.display(),
        barriers.display()
    );

    // 保留连接句柄：SessionHandle 被丢弃会关闭该连接任务（同 cli gui.rs）。
    let mut connections: Vec<Box<dyn GuiConnection>> = Vec::new();
    // select! 每轮重建 accept future：UnixListener 内部互斥锁的排队请求随 future
    // 丢弃被取消，多线程 worker 下反复入队出队导致 accept 抢不到锁、连接任务饿死。
    // 先把 accept 固化成 JoinHandle，select 只轮询句柄本身。
    let mut accept_task = {
        let listener = Arc::clone(&listener);
        tokio::spawn(async move { listener.accept().await })
    };
    loop {
        tokio::select! {
            accepted = &mut accept_task => {
                match accepted {
                    Ok(Ok(handle)) => {
                        log(&log_path, format!("accepted connection {}", handle.info().connection_id.as_str()))?;
                        connections.retain(|connection| connection.info().connection_id != handle.info().connection_id);
                        connections.push(handle);
                        accept_task = {
                            let listener = Arc::clone(&listener);
                            tokio::spawn(async move { listener.accept().await })
                        };
                    }
                    Ok(Err(error)) => {
                        log(&log_path, format!("accept failed: {error}"))?;
                        break;
                    }
                    Err(join_error) => {
                        log(&log_path, format!("accept task failed: {join_error}"))?;
                        break;
                    }
                }
            }
            _ = watch_stop_request(&barriers) => {
                log(&log_path, "serve_stop request")?;
                // UnixSocketListener::accept 会在持有内部互斥锁期间等待新连接，
                // close() 需要同一把锁；必须先终止 accept 任务（drop 释放锁）再 close，
                // 否则 shutdown 死锁（gui.rs 靠 select! 丢弃 accept future 达到同效）。
                accept_task.abort();
                let _ = accept_task.await;
                if let Err(error) = listener.close().await {
                    log(&log_path, format!("listener close failed: {error}"))?;
                }
                break;
            }
            _ = watch_drop_request(&barriers) => {
                // 先消费 request，再写 done；下一次 request 可在同一 serve 生命周期
                // 重新触发，且 driver 不会把上轮 done 误当成本轮完成。
                let _ = std::fs::remove_file(barriers.join("drop_socket.request"));
                connections.clear();
                write_barrier(&barriers, "drop_socket.done", json!({"connections": "dropped"}))?;
                log(&log_path, "drop_socket done")?;
            }
        }
    }
    drop(connections);
    target_watch.abort();
    let _ = target_watch.await;
    if let Err(error) = pty.shutdown().await {
        log(&log_path, format!("pty shutdown failed: {error}"))?;
    }
    if let Ok(core) = Arc::try_unwrap(core) {
        core.into_inner()
            .shutdown()
            .await
            .map_err(|error| format!("core shutdown 失败：{error}"))?;
    }
    log(&log_path, "shutdown complete")?;
    println!("ui_fixture serve stopped");
    Ok(())
}

async fn watch_drop_request(barriers: &Path) {
    let request = barriers.join("drop_socket.request");
    loop {
        if request.is_file() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn watch_stop_request(barriers: &Path) {
    let request = barriers.join("serve_stop.request");
    loop {
        if request.is_file() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

const SIMULATED_BUNDLE_ID: &str = "dev.pawork.fixture.simulated";

fn simulated_bundle_from_request(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(SIMULATED_BUNDLE_ID.to_string());
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let bundle = value
            .get("bundle_id")
            .and_then(Value::as_str)
            .unwrap_or(SIMULATED_BUNDLE_ID)
            .trim();
        if bundle.is_empty() {
            return Err("bundle_id 为空".into());
        }
        return Ok(bundle.to_string());
    }
    if trimmed
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err("bundle_id 含空白".into());
    }
    Ok(trimmed.to_string())
}

async fn watch_simulated_targets(adapter: GuiHostAdapter, barriers: PathBuf, log_path: PathBuf) {
    let register = barriers.join("register_simulated_target.request");
    let dump = barriers.join("dump_targets.request");
    loop {
        if register.is_file() {
            let body = std::fs::read_to_string(&register).unwrap_or_default();
            let _ = std::fs::remove_file(&register);
            match simulated_bundle_from_request(&body) {
                Ok(bundle) => match adapter.fixture_note_simulated_target(&bundle).await {
                    Ok(snapshot) => {
                        let _ = log(&log_path, format!("registered simulated target {bundle}"));
                        let _ =
                            write_barrier(&barriers, "register_simulated_target.done", snapshot);
                    }
                    Err(error) => {
                        let _ = log(
                            &log_path,
                            format!("register simulated target failed: {error}"),
                        );
                        let _ = write_barrier(
                            &barriers,
                            "register_simulated_target.failed",
                            json!({"error": error, "bundle_id": bundle}),
                        );
                    }
                },
                Err(error) => {
                    let _ = log(
                        &log_path,
                        format!("register simulated target rejected: {error}"),
                    );
                    let _ = write_barrier(
                        &barriers,
                        "register_simulated_target.failed",
                        json!({"error": error}),
                    );
                }
            }
        }
        if dump.is_file() {
            let _ = std::fs::remove_file(&dump);
            let snapshot = adapter.fixture_target_snapshot().await;
            let _ = log(&log_path, "dumped target registry snapshot");
            let _ = write_barrier(&barriers, "dump_targets.done", snapshot);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct Client {
    conn: Box<dyn GuiConnection>,
}

impl Client {
    async fn connect(address: &str) -> Result<Self, String> {
        let transport = LocalTransport::default();
        let conn = transport
            .connect(
                TransportEndpoint::Local {
                    address: address.into(),
                },
                ConnectOptions {
                    timeout_ms: 3000,
                    client_label: Some("ui-fixture-self-check".into()),
                    max_frame_bytes: 1024 * 1024,
                },
            )
            .await
            .map_err(|error| format!("connect 失败：{error}"))?;
        Ok(Self { conn })
    }

    async fn send(&self, frame: &ClientFrame) -> Result<(), String> {
        let encoded = encode_client_frame(frame).map_err(|error| error.to_string())?;
        self.conn
            .send(TransportFrame::new(encoded))
            .await
            .map_err(|error| format!("send 失败：{error}"))
    }

    async fn recv(&self) -> Result<ServerFrame, String> {
        let frame = tokio::time::timeout(Duration::from_secs(15), self.conn.receive())
            .await
            .map_err(|_| "recv 超时（15s）".to_string())?
            .map_err(|error| format!("recv 失败：{error}"))?;
        decode_server_frame(frame.as_bytes()).map_err(|error| format!("decode 失败：{error}"))
    }

    async fn handshake(&mut self, token: &str) -> Result<HandshakeResponse, String> {
        self.send(&ClientFrame::Handshake(HandshakeRequest {
            request_id: "self-check-handshake".into(),
            client_name: "ui-fixture-self-check".into(),
            client_version: "1".into(),
            supported_api_versions: SUPPORTED_API_VERSIONS.to_vec(),
            capabilities: vec![
                GuiCapability::Events,
                GuiCapability::Snapshots,
                GuiCapability::Approvals,
                GuiCapability::ArtifactStreaming,
                GuiCapability::TerminalStreaming,
            ],
            authentication: Some(ClientAuthentication {
                scheme: TOKEN_SCHEME.into(),
                proof: token.into(),
            }),
        }))
        .await?;
        match self.recv().await? {
            ServerFrame::Handshake(response) => match &response {
                HandshakeResponse::Accepted { .. } => Ok(response),
                other => Err(format!("握手被拒绝：{other:?}")),
            },
            other => Err(format!("期望握手响应，得到 {other:?}")),
        }
    }

    async fn expect_snapshot(&mut self) -> Result<Snapshot, String> {
        match self.recv().await? {
            ServerFrame::Snapshot(snapshot) => Ok(snapshot),
            other => Err(format!("期望 snapshot，得到 {other:?}")),
        }
    }

    async fn subscribe_global(&mut self) -> Result<(), String> {
        self.send(&ClientFrame::Subscribe(SubscribeRequest {
            request_id: "self-check-subscribe".into(),
            subscription_id: "self-check-events".into(),
            // 空 streams = 订阅全部流；run 事件在 Session 流上发布。
            streams: Vec::new(),
        }))
        .await
    }

    fn command_envelope(command_id: &str, command: AppCommand) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: ApiVersion { major: 1, minor: 2 },
            command_id: CommandId::from(command_id),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: "ui-fixture-self-check".into(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(wall_now_ms()),
            command,
        }
    }

    async fn create_session(&mut self, workspace_id: &str) -> Result<String, String> {
        self.send(&ClientFrame::Command(Self::command_envelope(
            &format!("self-check-session-create-{}", wall_now_ms()),
            AppCommand::SessionCreate {
                workspace_id: Some(WorkspaceId::from(workspace_id)),
                title: Some("ui fixture self-check".into()),
            },
        )))
        .await?;
        loop {
            match self.recv().await? {
                ServerFrame::Response(envelope) => match envelope.response {
                    AppResponse::Data(value) => {
                        if let Some(id) = value.get("session_id").and_then(Value::as_str) {
                            return Ok(id.to_string());
                        }
                        continue;
                    }
                    other => return Err(format!("SessionCreate 失败：{other:?}")),
                },
                ServerFrame::Error(error) => {
                    return Err(format!("SessionCreate 协议错误：{error:?}"));
                }
                _ => continue,
            }
        }
    }

    async fn run_start(&mut self, session_id: &str) -> Result<(), String> {
        self.send(&ClientFrame::Command(Self::command_envelope(
            &format!("self-check-run-start-{}", wall_now_ms()),
            AppCommand::RunStart {
                session_id: SessionId::from(session_id),
                user_message: "fixture self-check：默认脚本一轮".into(),
                model: None,
                provider: None,
                profile: None,
                effort: None,
                attachment_ids: Vec::new(),
                web_search: None,
                video_urls: Vec::new(),
            },
        )))
        .await
    }
}

async fn cmd_self_check(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, false)?;
    if parsed.profile.is_some() {
        return Err("--profile 只允许用于 serve".into());
    }
    require_seeded_root(&parsed.root)?;
    let spec = load_spec()?;
    let barriers = barrier_dir(&parsed.root);
    // 每次自检都必须产出新 barrier；清掉旧文件，避免存在性检查误收陈旧结果。
    let _ = std::fs::remove_file(barriers.join("replay_complete"));
    wait_for_barrier(&barriers, "host_ready", Duration::from_secs(30)).await?;
    let log_path = parsed.root.join("logs/self-check.log");
    log(&log_path, "self-check starting")?;

    let token = std::fs::read_to_string(parsed.root.join("data/gui.token"))
        .map_err(|error| format!("读取 gui token 失败：{error}"))?
        .trim()
        .to_string();
    let socket = parsed
        .root
        .join("data/pawork-gui.sock")
        .display()
        .to_string();

    // 第一次连接：握手 + snapshot 校验 + 订阅 + 脚本 run。
    let mut client = Client::connect(&socket).await?;
    client.handshake(&token).await?;
    let snapshot = client.expect_snapshot().await?;
    assert_snapshot(&spec, &snapshot)?;
    log(
        &log_path,
        format!("snapshot ok ({} sections)", snapshot.sections.len()),
    )?;
    client.subscribe_global().await?;
    let session_id = client.create_session("fx-alpha-app").await?;
    log(&log_path, format!("session created {session_id}"))?;
    client.run_start(&session_id).await?;

    let mut collected: Vec<(u64, Value)> = Vec::new();
    let mut delta_text = String::new();
    loop {
        let frame = client.recv().await.map_err(|error| {
            let _ = log(
                &log_path,
                format!(
                    "recv failed after {} collected events: {error}",
                    collected.len()
                ),
            );
            error
        })?;
        match frame {
            ServerFrame::Event(envelope) => {
                let sequence = envelope.global_sequence.0;
                let _ = log(
                    &log_path,
                    format!(
                        "event seq={sequence} payload={}",
                        serde_json::to_string(&envelope.payload)
                            .unwrap_or_else(|_| "<unserializable>".into())
                    ),
                );
                match &envelope.payload {
                    AppEvent::AssistantDelta { delta, .. } => {
                        delta_text.push_str(delta);
                        collected.push((
                            sequence,
                            serde_json::to_value(&envelope.payload)
                                .map_err(|error| error.to_string())?,
                        ));
                    }
                    AppEvent::RunChanged { state, .. } => {
                        collected.push((
                            sequence,
                            serde_json::to_value(&envelope.payload)
                                .map_err(|error| error.to_string())?,
                        ));
                        if *state == RunState::Completed {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            other => {
                let _ = log(&log_path, format!("non-event frame: {other:?}"));
                continue;
            }
        }
    }
    let expected_text = "fixture chunk 1 fixture chunk 2 fixture chunk 3";
    if delta_text != expected_text {
        return Err(format!(
            "assistant delta 文本不符：{delta_text:?}（期望 {expected_text:?}）"
        ));
    }
    let run_events = collected.len();
    if run_events != 5 {
        return Err(format!(
            "run 事件数不符：{run_events}（期望 5：created + 3 delta + completed）"
        ));
    }
    log(
        &log_path,
        format!("first connection collected {run_events} events"),
    )?;
    drop(client);

    // 第二次连接：陈旧 last_acked → Resume Replay，重放内容与首连一致。
    let mut client = Client::connect(&socket).await?;
    client.handshake(&token).await?;
    // 握手后服务端会自动推送一帧 Snapshot（连接被授予 Snapshots 能力），
    // 必须先消费掉，否则紧随其后的 recv 会把它误当成 Resume 响应。
    client.expect_snapshot().await?;
    let first_sequence = collected[0].0;
    client
        .send(&ClientFrame::Resume(ResumeRequest {
            request_id: "self-check-resume".into(),
            last_global_sequence: GlobalSequence(first_sequence - 1),
        }))
        .await?;
    let (from, through) = match client.recv().await? {
        ServerFrame::Resume(response) => match response.disposition {
            ResumeDisposition::Replay {
                from_sequence,
                through_sequence,
            } => Ok((from_sequence.0, through_sequence.0)),
            other => Err(format!("期望 Resume Replay，得到 {other:?}")),
        },
        other => Err(format!("期望 Resume 响应，得到 {other:?}")),
    }?;
    let mut replayed: Vec<(u64, Value)> = Vec::new();
    loop {
        match client.recv().await? {
            ServerFrame::Event(envelope) => {
                let sequence = envelope.global_sequence.0;
                replayed.push((
                    sequence,
                    serde_json::to_value(&envelope.payload).map_err(|error| error.to_string())?,
                ));
                if sequence >= through {
                    break;
                }
            }
            _ => continue,
        }
    }
    if from != first_sequence {
        return Err(format!(
            "replay 起点 {from} 与首连首个事件 {first_sequence} 不符"
        ));
    }
    if replayed != collected {
        return Err(format!(
            "重放事件与首连不一致：replayed={} collected={}",
            replayed.len(),
            collected.len()
        ));
    }
    log(
        &log_path,
        format!(
            "resume replay ok ({}/{} through {})",
            replayed.len(),
            from,
            through
        ),
    )?;
    drop(client);

    write_barrier(
        &barriers,
        "replay_complete",
        json!({"events": replayed.len(), "from": from, "through": through}),
    )?;
    println!("ui_fixture self-check ok");
    println!("  snapshot sections: {}", snapshot.sections.len());
    println!("  run events: {run_events} replayed: {}", replayed.len());
    println!("  barrier: {}", barriers.join("replay_complete").display());
    Ok(())
}

fn assert_snapshot(spec: &SeedSpec, snapshot: &Snapshot) -> Result<(), String> {
    let seeded_ids: BTreeSet<&str> = spec.sessions.iter().map(|s| s.id.as_str()).collect();
    let tree = snapshot
        .sections
        .iter()
        .find(|section| section.kind == SnapshotSectionKind::SessionTree)
        .ok_or("snapshot 缺少 session_tree")?;
    let entries = tree
        .data
        .as_ref()
        .and_then(Value::as_array)
        .ok_or("session_tree 数据不是数组")?;
    let ids: BTreeSet<&str> = entries
        .iter()
        .filter_map(|entry| entry.get("session_id").and_then(Value::as_str))
        .collect();
    for id in &seeded_ids {
        if !ids.contains(id) {
            return Err(format!("snapshot session_tree 缺少种子 session {id}"));
        }
    }
    let pending = snapshot
        .sections
        .iter()
        .find(|section| section.kind == SnapshotSectionKind::PendingToolApprovals)
        .and_then(|section| section.data.as_ref())
        .and_then(Value::as_array);
    let has_pending = pending.is_some_and(|items| {
        items.iter().any(|item| {
            item.get("tool_call_id").and_then(Value::as_str) == Some("call-fx-ses-beta-pending-0-0")
        })
    });
    if !has_pending {
        return Err("snapshot 未重建种子 pending approval（call-fx-ses-beta-pending-0-0）".into());
    }
    Ok(())
}

async fn cmd_snapshot_dump(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, true)?;
    if parsed.profile.is_some() {
        return Err("--profile 只允许用于 serve".into());
    }
    require_seeded_root(&parsed.root)?;
    let spec = load_spec()?;
    let core = fixture_core(&parsed.root, &spec, "default").await?;
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let snapshot = adapter
        .snapshot()
        .await
        .map_err(|error| format!("snapshot 失败：{error}"))?;
    let mut value =
        serde_json::to_value(&snapshot).map_err(|error| format!("序列化失败：{error}"))?;
    normalize_snapshot(&mut value, &spec);
    let mut text =
        serde_json::to_string_pretty(&value).map_err(|error| format!("序列化失败：{error}"))?;
    text.push('\n');
    let out = parsed.out.expect("checked by parse_args");
    std::fs::write(&out, text.as_bytes()).map_err(|error| format!("写输出失败：{error}"))?;
    let sessions_kept = value
        .pointer("/sections")
        .and_then(Value::as_array)
        .and_then(|sections| {
            sections
                .iter()
                .find(|section| section.get("kind").and_then(Value::as_str) == Some("session_tree"))
                .and_then(|section| section.get("data"))
                .and_then(Value::as_array)
                .map(|entries| entries.len())
        })
        .unwrap_or(0);
    println!("ui_fixture snapshot-dump ok");
    println!("  out: {}", out.display());
    println!(
        "  sections: {} sessions: {sessions_kept}",
        snapshot.sections.len()
    );
    println!("  bytes: {}", text.len());
    Ok(())
}

/// volatile 字段归一化：instance_id / generated_at 换固定占位；
/// session_tree 只保留 seed 声明的会话（self-check 运行产物不进 golden）。
fn normalize_snapshot(value: &mut Value, spec: &SeedSpec) {
    value["instance_id"] = json!("<normalized>");
    value["generated_at"] = json!(0);
    let seeded_ids: BTreeSet<&str> = spec.sessions.iter().map(|s| s.id.as_str()).collect();
    let Some(sections) = value.get_mut("sections").and_then(Value::as_array_mut) else {
        return;
    };
    for section in sections {
        let is_tree = section
            .get("kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "session_tree" || kind == "SessionTree");
        if !is_tree {
            continue;
        }
        if let Some(entries) = section.get_mut("data").and_then(Value::as_array_mut) {
            entries.retain(|entry| {
                entry
                    .get("session_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| seeded_ids.contains(id))
            });
        }
    }
}

fn barrier_dir(root: &Path) -> PathBuf {
    std::env::var_os("PAWORK_UI_BARRIER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("barriers"))
}

fn wall_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn write_barrier(dir: &Path, name: &str, detail: Value) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("创建 barrier 目录失败：{error}"))?;
    let path = dir.join(name);
    let body = json!({"at_ms": wall_now_ms(), "detail": detail});
    let mut text = serde_json::to_string_pretty(&body)
        .map_err(|error| format!("序列化 barrier 失败：{error}"))?;
    text.push('\n');
    std::fs::write(&path, text.as_bytes()).map_err(|error| format!("写 barrier 失败：{error}"))?;
    Ok(path)
}

async fn wait_for_barrier(dir: &Path, name: &str, timeout: Duration) -> Result<PathBuf, String> {
    let path = dir.join(name);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if path.is_file() {
            return Ok(path);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("等待 barrier {name} 超时（{}）", dir.display()));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn log(path: &Path, line: impl AsRef<str>) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建日志目录失败：{error}"))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("打开日志失败：{error}"))?;
    let line = line.as_ref();
    writeln!(file, "[{}] {line}", wall_now_ms()).map_err(|error| format!("写日志失败：{error}"))
}
