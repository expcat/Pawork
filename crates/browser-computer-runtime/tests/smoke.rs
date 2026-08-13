//! P17-10 Browser / Computer Runtime 定向 smoke 测试。
//!
//! 覆盖执行位点路由（no_provider_branch）、ProviderHosted 不入本地 execute、
//! 探测回退可观测、跨 trust 闸门、大 payload 折叠 artifact、Core-owned 子进程
//! 经注入 sandbox、policy 审批。全部为 Mock，不依赖真实浏览器/网络。

use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agent_domain::{CancellationToken, RunId, ServerToolEvent, ToolCallId, WorkspaceId};
use async_trait::async_trait;
use browser_computer_runtime::AuditSinkError;
use browser_computer_runtime::{
    action_capability, enforce_decision, normalize_snapshot, policy_input_for,
    reject_non_client_function_for_local, select_for_local, AuditRecord, AuditSink, BackendKind,
    BrowserComputerAction, BrowserComputerBackend, BrowserComputerCapability, BrowserComputerError,
    BrowserComputerSnapshot, CanonicalHostedEmitter, ExecutionSite, FileAuditSink, LocalBackend,
    LocalDriver, McpBackend, McpDriver, McpOwnership, PlaywrightBackend, ProcessMode,
    ProviderHostedBackend, SandboxAuthorization, SelectionPolicy, TrustBoundary,
    AUDIT_FORMAT_VERSION,
};
use policy_engine::{ApprovalMode, PolicyEngine};
use sandbox_runtime::{SandboxBackend, SandboxError, SandboxPolicy, SandboxProcessSpec};
use tool_api::{
    AgentTool, ToolCapability, ToolErrorKind, ToolEventSink, ToolExecutionContext, ToolHosting,
    ToolKind, ToolRequest, ToolStreamEvent,
};

fn ws() -> WorkspaceId {
    WorkspaceId::new("ws-test")
}

fn cancel() -> CancellationToken {
    CancellationToken::new()
}

fn exec_ctx() -> ToolExecutionContext {
    ToolExecutionContext {
        workspace_id: ws(),
        run_id: RunId::new("run-test"),
        working_directory: None,
    }
}

// ---------- Mock 驱动 ----------

struct RecordingLocalDriver {
    labels: Mutex<Vec<&'static str>>,
    authorizations: Mutex<Vec<(ProcessMode, bool)>>,
}

impl RecordingLocalDriver {
    fn new() -> Self {
        Self {
            labels: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        }
    }

    fn recorded(&self) -> Vec<&'static str> {
        self.labels.lock().unwrap().clone()
    }

    /// driver 实际收到的授权（mode, is_sandboxed）序列。
    fn authorizations(&self) -> Vec<(ProcessMode, bool)> {
        self.authorizations.lock().unwrap().clone()
    }
}

#[async_trait]
impl LocalDriver for RecordingLocalDriver {
    async fn act(
        &self,
        action: BrowserComputerAction,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        self.labels.lock().unwrap().push(action.label());
        self.authorizations
            .lock()
            .unwrap()
            .push((authorization.mode(), authorization.is_sandboxed()));
        Ok(BrowserComputerSnapshot {
            title: Some("Example".into()),
            url: Some("https://example.test".into()),
            summary: format!("did {}", action.label()),
            ..Default::default()
        })
    }

    async fn snapshot(
        &self,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        self.authorizations
            .lock()
            .unwrap()
            .push((authorization.mode(), authorization.is_sandboxed()));
        Ok(BrowserComputerSnapshot::from_summary("snapshot"))
    }
}

/// MCP mock：仅用于证明 MCP 位点由 ownership 决定，不真正调用 MCP server。
struct RecordingMcpDriver {
    authorizations: Mutex<Vec<(ProcessMode, bool)>>,
}

impl RecordingMcpDriver {
    fn new() -> Self {
        Self {
            authorizations: Mutex::new(Vec::new()),
        }
    }

    fn authorizations(&self) -> Vec<(ProcessMode, bool)> {
        self.authorizations.lock().unwrap().clone()
    }
}

#[async_trait]
impl McpDriver for RecordingMcpDriver {
    async fn act(
        &self,
        action: BrowserComputerAction,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        self.authorizations
            .lock()
            .unwrap()
            .push((authorization.mode(), authorization.is_sandboxed()));
        Ok(BrowserComputerSnapshot::from_summary(format!(
            "mcp {}",
            action.label()
        )))
    }

    async fn snapshot(
        &self,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        self.authorizations
            .lock()
            .unwrap()
            .push((authorization.mode(), authorization.is_sandboxed()));
        Ok(BrowserComputerSnapshot::from_summary("mcp snapshot"))
    }
}

/// Playwright mock：ready=true，记录调用与收到的授权。
struct RecordingPlaywrightDriver {
    labels: Mutex<Vec<&'static str>>,
    authorizations: Mutex<Vec<(ProcessMode, bool)>>,
}

impl RecordingPlaywrightDriver {
    fn new() -> Self {
        Self {
            labels: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
        }
    }

    fn recorded(&self) -> Vec<&'static str> {
        self.labels.lock().unwrap().clone()
    }

    fn authorizations(&self) -> Vec<(ProcessMode, bool)> {
        self.authorizations.lock().unwrap().clone()
    }
}

#[async_trait]
impl browser_computer_runtime::PlaywrightDriver for RecordingPlaywrightDriver {
    fn ready(&self) -> bool {
        true
    }

    async fn act(
        &self,
        action: BrowserComputerAction,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        self.labels.lock().unwrap().push(action.label());
        self.authorizations
            .lock()
            .unwrap()
            .push((authorization.mode(), authorization.is_sandboxed()));
        Ok(BrowserComputerSnapshot::from_summary(format!(
            "pw {}",
            action.label()
        )))
    }

    async fn snapshot(
        &self,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        self.authorizations
            .lock()
            .unwrap()
            .push((authorization.mode(), authorization.is_sandboxed()));
        Ok(BrowserComputerSnapshot::from_summary("pw snapshot"))
    }
}

/// 注入式失败 audit sink：append 恒失败（模拟磁盘故障）。
struct FailingAuditSink;

impl AuditSink for FailingAuditSink {
    fn append(
        &self,
        _audit: &browser_computer_runtime::BrowserComputerAudit,
    ) -> Result<AuditRecord, AuditSinkError> {
        Err(AuditSinkError::Io("injected append failure".into()))
    }

    fn replay(&self) -> Result<Vec<AuditRecord>, AuditSinkError> {
        Ok(Vec::new())
    }
}

/// 返回大 DOM 的 in-process driver，用于验证 facade 归一化失败路径不泄漏正文。
struct LargeDomDriver {
    dom: String,
}

#[async_trait]
impl LocalDriver for LargeDomDriver {
    async fn act(
        &self,
        _action: BrowserComputerAction,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        _authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        Ok(BrowserComputerSnapshot {
            dom: Some(self.dom.clone()),
            ..Default::default()
        })
    }

    async fn snapshot(
        &self,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        _authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        Ok(BrowserComputerSnapshot {
            dom: Some(self.dom.clone()),
            ..Default::default()
        })
    }
}

/// probe 调用计数后端：验证一次 selector 选择中只探测一次。
struct CountingProbeBackend {
    probes: Arc<AtomicUsize>,
}

#[async_trait]
impl BrowserComputerBackend for CountingProbeBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Local
    }

    fn execution_site(&self) -> ExecutionSite {
        ExecutionSite::ClientFunction
    }

    fn trust_boundary(&self) -> TrustBoundary {
        TrustBoundary::CoreOwned
    }

    fn descriptor_name(&self) -> &'static str {
        "browser_computer.counting_probe"
    }

    fn probe(&self) -> browser_computer_runtime::BackendProbe {
        self.probes.fetch_add(1, Ordering::SeqCst);
        browser_computer_runtime::BackendProbe::available()
    }

    async fn act(
        &self,
        _action: BrowserComputerAction,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        Ok(BrowserComputerSnapshot::default())
    }

    async fn snapshot(
        &self,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        Ok(BrowserComputerSnapshot::default())
    }
}

struct RecordingSandbox {
    spawns: Mutex<u32>,
}

impl RecordingSandbox {
    fn new() -> Self {
        Self {
            spawns: Mutex::new(0),
        }
    }

    fn spawn_calls(&self) -> u32 {
        *self.spawns.lock().unwrap()
    }
}

#[async_trait]
impl SandboxBackend for RecordingSandbox {
    fn id(&self) -> &'static str {
        "recording"
    }
    fn available(&self) -> bool {
        true
    }
    async fn spawn(
        &self,
        _spec: SandboxProcessSpec,
        _policy: SandboxPolicy,
        _cancel: CancellationToken,
    ) -> Result<sandbox_runtime::SandboxProcess, SandboxError> {
        *self.spawns.lock().unwrap() += 1;
        // 不真正起进程；只记录路由命中，返回结构化错误供调用方观察。
        Err(SandboxError::Denied(
            "recording sandbox: spawn routed (test)".into(),
        ))
    }
}

struct NoopSink;

#[async_trait]
impl ToolEventSink for NoopSink {
    async fn emit(&self, _event: ToolStreamEvent) -> Result<(), tool_api::ToolError> {
        Ok(())
    }
}

// ---------- 1. 执行位点驱动路由（no_provider_branch）----------

#[test]
fn execution_site_drives_routing_not_provider_name() {
    // 两个不同 provider 标签的 hosted 后端：位点/信任边界一致，selector 平等对待。
    let a = ProviderHostedBackend::new("anthropic");
    let b = ProviderHostedBackend::new("openai");
    assert_eq!(a.execution_site(), b.execution_site());
    assert_eq!(a.trust_boundary(), b.trust_boundary());
    assert_eq!(a.execution_site(), ExecutionSite::ProviderHosted);
    assert_eq!(a.trust_boundary(), TrustBoundary::ExternallyOwned);
    // provider_label 仅用于审计，不参与位点决策。
    assert_ne!(a.provider_label(), b.provider_label());
}

#[test]
fn mcp_ownership_drives_site() {
    let local = McpBackend::new(
        McpOwnership::LocalProcess,
        Arc::new(RecordingMcpDriver::new()),
    );
    let mediated = McpBackend::new(
        McpOwnership::ProviderMediated,
        Arc::new(RecordingMcpDriver::new()),
    );
    assert_eq!(local.execution_site(), ExecutionSite::ClientFunction);
    assert_eq!(local.trust_boundary(), TrustBoundary::CoreOwned);
    assert_eq!(mediated.execution_site(), ExecutionSite::ProviderExtension);
    assert_eq!(mediated.trust_boundary(), TrustBoundary::ExternallyOwned);
}

// ---------- 2. ProviderHosted 不进入本地 execute ----------

#[tokio::test]
async fn provider_hosted_act_returns_not_locally_executable() {
    let backend = ProviderHostedBackend::new("anthropic");
    let err = backend
        .act(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap_err();
    assert!(
        matches!(err, BrowserComputerError::NotLocallyExecutable { .. }),
        "{err:?}"
    );
}

#[test]
fn select_for_local_never_picks_hosted() {
    let hosted: Arc<dyn browser_computer_runtime::BrowserComputerBackend> =
        Arc::new(ProviderHostedBackend::new("anthropic"));
    let result = select_for_local(&[hosted]);
    assert!(
        result.is_err(),
        "ProviderHosted must never be selected for local"
    );
}

#[tokio::test]
async fn tool_descriptor_is_client_function_local() {
    let cap = Arc::new(
        BrowserComputerCapability::builder()
            .backend(Arc::new(ProviderHostedBackend::new("anthropic")))
            .trusted(true)
            .build(),
    );
    let tool = browser_computer_runtime::BrowserComputerTool::new(cap);
    let desc = tool.descriptor();
    // descriptor 必须是 ClientFunction（hosted 不经此工具）。
    assert_eq!(desc.kind, ToolKind::ClientFunction);
    assert_eq!(desc.hosting, ToolHosting::Local);
    assert!(desc
        .capabilities
        .contains(&tool_api::ToolCapabilityTag::ComputerUse));
}

#[tokio::test]
async fn tool_execute_rejects_when_only_hosted_available() {
    let cap = Arc::new(
        BrowserComputerCapability::builder()
            .backend(Arc::new(ProviderHostedBackend::new("anthropic")))
            .trusted(true)
            .build(),
    );
    let tool = browser_computer_runtime::BrowserComputerTool::new(cap);
    let request = ToolRequest {
        tool_call_id: ToolCallId::new("call-1"),
        input: serde_json::json!({"action": "title"}),
    };
    let err = tool
        .execute(request, exec_ctx(), &NoopSink, cancel())
        .await
        .unwrap_err();
    // 只有 hosted：本地无后端 → resolve_no_local 找到 hosted 但跨 trust 默认拒绝
    // （CrossTrustFallbackDenied）→ 归一为 PermissionDenied。hosted 绝不在本地执行。
    assert_eq!(err.kind, ToolErrorKind::PermissionDenied);
}

#[tokio::test]
async fn tool_execute_returns_not_found_with_no_backends() {
    let cap = Arc::new(BrowserComputerCapability::builder().trusted(true).build());
    let tool = browser_computer_runtime::BrowserComputerTool::new(cap);
    let request = ToolRequest {
        tool_call_id: ToolCallId::new("call-2"),
        input: serde_json::json!({"action": "title"}),
    };
    let err = tool
        .execute(request, exec_ctx(), &NoopSink, cancel())
        .await
        .unwrap_err();
    // 无任何后端（亦无 hosted 降级目标）→ NoLocalBackend → NotFound。
    assert_eq!(err.kind, ToolErrorKind::NotFound);
}

#[tokio::test]
async fn hosted_request_emits_server_tool_events_never_executes() {
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(ProviderHostedBackend::new("anthropic")))
        .hosted_emitter(Arc::new(CanonicalHostedEmitter))
        .trusted(true)
        .build();
    let events = cap
        .hosted_request(
            &BrowserComputerAction::Screenshot,
            &ToolCallId::new("hosted-call"),
        )
        .unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        ServerToolEvent::ComputerActionRequested { .. }
    ));
    let audit = cap.last_audit().expect("audit recorded");
    assert_eq!(
        audit.site.as_deref(),
        Some(ExecutionSite::ProviderHosted.as_str())
    );
    assert_eq!(audit.trust.as_deref(), Some("externally_owned"));
}

// ---------- 3. 探测 + 可观测回退 ----------

#[tokio::test]
async fn local_backend_selected_and_executes() {
    let driver = Arc::new(RecordingLocalDriver::new());
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(LocalBackend::with_driver(driver.clone())))
        .trusted(true)
        .build();
    let snapshot = cap
        .act_local(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap();
    assert_eq!(snapshot.title.as_deref(), Some("Example"));
    assert_eq!(driver.recorded(), vec!["title"]);
    let audit = cap.last_audit().expect("audit recorded");
    assert_eq!(audit.backend.as_deref(), Some(BackendKind::Local.as_str()));
    assert_eq!(
        audit.site.as_deref(),
        Some(ExecutionSite::ClientFunction.as_str())
    );
    assert_eq!(
        audit.trust.as_deref(),
        Some(TrustBoundary::CoreOwned.as_str())
    );
    assert!(!audit.cross_trust_fallback);
}

#[tokio::test]
async fn probe_fallback_is_observable_when_local_unavailable() {
    // Local 标记为不可用；MCP 本地进程可用 → 回退到 MCP（同 trust，不跨 boundary）。
    let driver = Arc::new(RecordingLocalDriver::new());
    let local = LocalBackend::with_driver(driver).with_probe(false, "no browser");
    let mcp = McpBackend::new(
        McpOwnership::LocalProcess,
        Arc::new(RecordingMcpDriver::new()),
    );
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(local))
        .backend(Arc::new(mcp))
        .trusted(true)
        .build();
    let selection = cap.select_for_local().unwrap();
    assert_eq!(selection.route.kind, BackendKind::Mcp);
    assert!(!selection.cross_trust_fallback);
    // attempted 记录包含被跳过的 Local。
    assert!(selection
        .attempted
        .iter()
        .any(|p| p.kind == "local" && !p.available));
}

// ---------- 4. 跨 trust boundary 闸门 ----------

#[tokio::test]
async fn cross_trust_fallback_denied_without_policy() {
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(ProviderHostedBackend::new("anthropic")))
        .trusted(true)
        // 默认 SelectionPolicy::default() → allow_cross_trust_fallback = false
        .build();
    let err = cap
        .act_local(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap_err();
    assert!(
        matches!(err, BrowserComputerError::CrossTrustFallbackDenied { .. }),
        "{err:?}"
    );
    let audit = cap.last_audit().expect("audit recorded");
    assert!(audit.cross_trust_fallback);
    assert_eq!(audit.policy, "deny");
}

#[tokio::test]
async fn cross_trust_fallback_allowed_with_policy() {
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(ProviderHostedBackend::new("anthropic")))
        .hosted_emitter(Arc::new(CanonicalHostedEmitter))
        .trusted(true)
        .selection(SelectionPolicy {
            allow_cross_trust_fallback: true,
        })
        .build();
    let err = cap
        .act_local(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap_err();
    assert!(
        matches!(err, BrowserComputerError::HostedFallbackRequired { .. }),
        "{err:?}"
    );
    let audit = cap.last_audit().expect("audit recorded");
    assert!(audit.cross_trust_fallback);
    assert_eq!(audit.policy, "cross_trust_fallback");
}

// ---------- 5. 大 payload → artifact 引用（ADR-018）----------

#[tokio::test]
async fn large_dom_folded_to_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let store = artifact_store::ArtifactStore::open(dir.path())
        .await
        .unwrap();
    let big = "x".repeat(64 * 1024);
    let snapshot = BrowserComputerSnapshot {
        summary: "page".into(),
        dom: Some(big),
        ..Default::default()
    };
    let normalized = normalize_snapshot(snapshot, Some(&store), 16 * 1024).await;
    assert!(normalized.dom.is_none(), "dom must be folded out");
    assert_eq!(normalized.artifacts.len(), 1);
    assert_eq!(normalized.artifacts[0].byte_length, 64 * 1024);
    // 引用可读回。
    let blob_id = artifact_store::BlobId::from_str(normalized.artifacts[0].id.as_str()).unwrap();
    let blob = store.get(&blob_id).await.unwrap();
    assert_eq!(
        blob.len(),
        64 * 1024,
        "artifact must be retrievable at full size"
    );
}

#[tokio::test]
async fn large_dom_marked_truncated_without_store() {
    let big = "y".repeat(64 * 1024);
    let snapshot = BrowserComputerSnapshot {
        dom: Some(big),
        ..Default::default()
    };
    let normalized = normalize_snapshot(snapshot, None, 16 * 1024).await;
    assert_eq!(
        normalized.metadata["truncated"].as_str(),
        Some("true"),
        "must be flagged truncated when no store"
    );
}

// ---------- 6. Core-owned 子进程只经注入 sandbox ----------

#[tokio::test]
async fn playwright_spawn_routes_through_injected_sandbox() {
    let sandbox = Arc::new(RecordingSandbox::new());
    let backend = PlaywrightBackend::new().with_sandbox(sandbox.clone());
    assert!(backend.sandbox().is_some());
    let spec = SandboxProcessSpec {
        command: process_runtime::CommandSpec::new("npx"),
        workspace_roots: Vec::new(),
    };
    let result = backend
        .spawn_driver(spec, SandboxPolicy::default(), cancel())
        .await;
    // 注入的 recording sandbox 被命中（spawn 被路由）；返回其结构化错误。
    assert!(result.is_err());
    assert_eq!(sandbox.spawn_calls(), 1);
}

#[tokio::test]
async fn playwright_spawn_denied_without_sandbox() {
    let backend = PlaywrightBackend::new();
    assert!(backend.sandbox().is_none());
    let spec = SandboxProcessSpec {
        command: process_runtime::CommandSpec::new("npx"),
        workspace_roots: Vec::new(),
    };
    let err = backend
        .spawn_driver(spec, SandboxPolicy::default(), cancel())
        .await
        .err()
        .unwrap();
    // 未注入 sandbox → Core-owned 后端不得直接 spawn。
    assert!(matches!(err, SandboxError::Denied(_)));
}

// ---------- 7. Policy 审批 ----------

#[test]
fn read_only_action_maps_to_read_only_capability() {
    assert_eq!(
        action_capability(&BrowserComputerAction::Screenshot),
        ToolCapability::ReadOnly
    );
    assert_eq!(
        action_capability(&BrowserComputerAction::Title),
        ToolCapability::ReadOnly
    );
    assert_eq!(
        action_capability(&BrowserComputerAction::SnapshotDom { selector: None }),
        ToolCapability::ReadOnly
    );
    assert_eq!(
        action_capability(&BrowserComputerAction::Navigate { url: "x".into() }),
        ToolCapability::Network
    );
}

#[tokio::test]
async fn untrusted_workspace_denies_network_action() {
    // 未信任工作区：descriptor 硬门（allowed_in_untrusted_workspace=false）直接拒绝。
    let driver = Arc::new(RecordingLocalDriver::new());
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(LocalBackend::with_driver(driver)))
        .trusted(false)
        .build();
    let err = cap
        .act_local(
            BrowserComputerAction::Navigate {
                url: "https://example.test".into(),
            },
            &ws(),
            cancel(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, BrowserComputerError::PolicyDenied(_)),
        "{err:?}"
    );
    let audit = cap.last_audit().expect("deny recorded as audit");
    assert_eq!(audit.policy, "deny");
    assert!(audit.backend.is_none());
}

#[test]
fn read_only_capability_allowed_for_trusted_in_read_only_mode() {
    let input = policy_input_for(
        &BrowserComputerAction::Screenshot,
        &serde_json::json!({"action": "screenshot"}),
        true,
        ApprovalMode::ReadOnly,
    );
    let engine = PolicyEngine::new(ApprovalMode::ReadOnly);
    let decision = engine.decide(&input);
    let constraints = enforce_decision(decision).unwrap();
    assert!(
        constraints.is_none(),
        "ReadOnly capability + trusted → Allow (no constraints)"
    );
}

// ---------- 8. P17-10 review：snapshot_local / hosted_request 必须 policy + audit ----------

#[tokio::test]
async fn snapshot_local_denied_by_policy_in_untrusted_workspace() {
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(LocalBackend::with_driver(Arc::new(
            RecordingLocalDriver::new(),
        ))))
        .trusted(false)
        .build();
    let err = cap.snapshot_local(&ws(), cancel()).await.unwrap_err();
    assert!(
        matches!(err, BrowserComputerError::PolicyDenied(_)),
        "{err:?}"
    );
    let audit = cap.last_audit().expect("deny must be audited");
    assert_eq!(audit.action, "snapshot_local");
    assert_eq!(audit.policy, "deny");
    assert!(audit.backend.is_none());
}

#[tokio::test]
async fn snapshot_local_allowed_and_audited() {
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(LocalBackend::with_driver(Arc::new(
            RecordingLocalDriver::new(),
        ))))
        .trusted(true)
        .build();
    let snapshot = cap.snapshot_local(&ws(), cancel()).await.unwrap();
    assert_eq!(snapshot.summary, "snapshot");
    let audit = cap.last_audit().expect("allow must be audited");
    assert_eq!(audit.action, "snapshot_local");
    assert_eq!(audit.policy, "allow");
    assert_eq!(audit.backend.as_deref(), Some(BackendKind::Local.as_str()));
    assert_eq!(
        audit.site.as_deref(),
        Some(ExecutionSite::ClientFunction.as_str())
    );
}

#[tokio::test]
async fn hosted_request_denied_by_policy_in_untrusted_workspace() {
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(ProviderHostedBackend::new("anthropic")))
        .hosted_emitter(Arc::new(CanonicalHostedEmitter))
        .trusted(false)
        .build();
    let err = cap
        .hosted_request(
            &BrowserComputerAction::Navigate {
                url: "https://example.test".into(),
            },
            &ToolCallId::new("hosted-deny"),
        )
        .unwrap_err();
    assert!(
        matches!(err, BrowserComputerError::PolicyDenied(_)),
        "{err:?}"
    );
    let audit = cap.last_audit().expect("deny must be audited");
    assert_eq!(audit.policy, "deny");
    assert_eq!(audit.action, "navigate");
    assert_eq!(
        audit.site.as_deref(),
        Some(ExecutionSite::ProviderHosted.as_str())
    );
    assert_eq!(audit.trust.as_deref(), Some("externally_owned"));
}

// ---------- 9. P17-10 review：ProviderExtension / Hosted 不进入本地 execute ----------

#[test]
fn reject_non_client_function_blocks_provider_extension_and_hosted() {
    let mediated = McpBackend::new(
        McpOwnership::ProviderMediated,
        Arc::new(RecordingMcpDriver::new()),
    );
    assert_eq!(mediated.execution_site(), ExecutionSite::ProviderExtension);
    let err = reject_non_client_function_for_local(&mediated).unwrap_err();
    assert!(
        matches!(err, BrowserComputerError::NotLocallyExecutable { .. }),
        "{err:?}"
    );

    let hosted = ProviderHostedBackend::new("anthropic");
    let err = reject_non_client_function_for_local(&hosted).unwrap_err();
    assert!(matches!(
        err,
        BrowserComputerError::NotLocallyExecutable { .. }
    ));
}

#[tokio::test]
async fn tool_execute_never_runs_provider_extension_backend() {
    // ProviderExtension 位点的 MCP 后端 + 本地后端：本地选中并执行，extension 绝不入选。
    let local_driver = Arc::new(RecordingLocalDriver::new());
    let cap = Arc::new(
        BrowserComputerCapability::builder()
            .backend(Arc::new(McpBackend::new(
                McpOwnership::ProviderMediated,
                Arc::new(RecordingMcpDriver::new()),
            )))
            .backend(Arc::new(LocalBackend::with_driver(local_driver.clone())))
            .trusted(true)
            .build(),
    );
    let tool = browser_computer_runtime::BrowserComputerTool::new(cap);
    let request = ToolRequest {
        tool_call_id: ToolCallId::new("call-ext"),
        input: serde_json::json!({"action": "title"}),
    };
    let result = tool
        .execute(request, exec_ctx(), &NoopSink, cancel())
        .await
        .unwrap();
    assert!(
        result.artifacts.is_empty(),
        "title snapshot carries no artifacts"
    );
    assert!(result.success);
    // 本地驱动被调用（执行确实发生在 ClientFunction 位点）。
    assert_eq!(local_driver.recorded(), vec!["title"]);
}

// ---------- 10. P17-10 review：CoreOwned 进程统一经 SandboxGate（fail closed / 不 spawn）----------

fn spec() -> SandboxProcessSpec {
    SandboxProcessSpec {
        command: process_runtime::CommandSpec::new("npx"),
        workspace_roots: Vec::new(),
    }
}

#[tokio::test]
async fn local_spawn_denied_when_in_process() {
    let backend = LocalBackend::with_driver(Arc::new(RecordingLocalDriver::new()));
    assert_eq!(backend.process_gate(), ProcessMode::InProcess);
    assert!(backend.sandbox().is_none());
    let err = backend
        .spawn_driver(spec(), SandboxPolicy::default(), cancel())
        .await
        .err()
        .unwrap();
    // in-process 明确不 spawn。
    assert!(matches!(err, SandboxError::Denied(_)));
}

#[tokio::test]
async fn local_spawn_routes_through_injected_sandbox() {
    let sandbox = Arc::new(RecordingSandbox::new());
    let backend = LocalBackend::with_driver(Arc::new(RecordingLocalDriver::new()))
        .with_sandbox(sandbox.clone());
    assert_eq!(backend.process_gate(), ProcessMode::SpawnViaSandbox);
    assert!(backend.sandbox().is_some());
    let result = backend
        .spawn_driver(spec(), SandboxPolicy::default(), cancel())
        .await;
    assert!(result.is_err());
    assert_eq!(sandbox.spawn_calls(), 1);
}

#[tokio::test]
async fn mcp_local_spawn_fail_closed_without_sandbox() {
    let backend = McpBackend::new(
        McpOwnership::LocalProcess,
        Arc::new(RecordingMcpDriver::new()),
    );
    assert_eq!(backend.process_gate(), ProcessMode::SpawnViaSandbox);
    assert!(backend.sandbox().is_none());
    let err = backend
        .spawn_server(spec(), SandboxPolicy::default(), cancel())
        .await
        .err()
        .unwrap();
    // 进程型后端未注入 sandbox → fail closed。
    assert!(matches!(err, SandboxError::Denied(_)));
}

#[tokio::test]
async fn mcp_preconnected_never_spawns() {
    let backend = McpBackend::new(
        McpOwnership::LocalProcess,
        Arc::new(RecordingMcpDriver::new()),
    )
    .preconnected();
    assert_eq!(backend.process_gate(), ProcessMode::Preconnected);
    let err = backend
        .spawn_server(spec(), SandboxPolicy::default(), cancel())
        .await
        .err()
        .unwrap();
    // preconnected 明确不 spawn。
    assert!(matches!(err, SandboxError::Denied(_)));
}

#[tokio::test]
async fn mcp_provider_mediated_never_spawns_even_with_sandbox_attempt() {
    let backend = McpBackend::new(
        McpOwnership::ProviderMediated,
        Arc::new(RecordingMcpDriver::new()),
    )
    .with_sandbox(Arc::new(RecordingSandbox::new()));
    // 外部所有：不接受 sandbox 注入，闸门保持 in-process。
    assert_eq!(backend.process_gate(), ProcessMode::InProcess);
    assert!(backend.sandbox().is_none());
    let err = backend
        .spawn_server(spec(), SandboxPolicy::default(), cancel())
        .await
        .err()
        .unwrap();
    assert!(matches!(err, SandboxError::Denied(_)));
}

#[tokio::test]
async fn playwright_preconnected_never_spawns() {
    let backend = PlaywrightBackend::new().with_preconnected();
    assert_eq!(backend.process_gate(), ProcessMode::Preconnected);
    let err = backend
        .spawn_driver(spec(), SandboxPolicy::default(), cancel())
        .await
        .err()
        .unwrap();
    assert!(matches!(err, SandboxError::Denied(_)));
}

#[tokio::test]
async fn builder_sandbox_uniformly_injects_into_process_backends() {
    let sandbox = Arc::new(RecordingSandbox::new());
    let local = LocalBackend::with_driver(Arc::new(RecordingLocalDriver::new()));
    let mcp = McpBackend::new(
        McpOwnership::LocalProcess,
        Arc::new(RecordingMcpDriver::new()),
    );
    let pw = PlaywrightBackend::new();
    let local_view = local.clone();
    let mcp_view = mcp.clone();
    let pw_view = pw.clone();
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(local))
        .backend(Arc::new(mcp))
        .backend(Arc::new(pw))
        .sandbox(sandbox.clone())
        .trusted(true)
        .build();
    assert!(cap.sandbox().is_some());
    // 统一注入后三个进程型后端都持有同一 sandbox。
    assert!(local_view.sandbox().is_some());
    assert!(mcp_view.sandbox().is_some());
    assert!(pw_view.sandbox().is_some());
    // 且 spawn 全部路由到注入的 sandbox。
    let spec = spec();
    let _ = local_view
        .spawn_driver(spec.clone(), SandboxPolicy::default(), cancel())
        .await;
    let _ = mcp_view
        .spawn_server(spec.clone(), SandboxPolicy::default(), cancel())
        .await;
    let _ = pw_view
        .spawn_driver(spec, SandboxPolicy::default(), cancel())
        .await;
    assert_eq!(sandbox.spawn_calls(), 3);
}

// ---------- 11. P17-10 review：versioned durable audit sink + replay/restart ----------

#[tokio::test]
async fn audit_sink_persists_and_replays_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");
    let sink = Arc::new(FileAuditSink::open(&path).unwrap());

    // 第一段「运行」：act_local + snapshot_local + hosted_request 全部落盘。
    {
        let cap = BrowserComputerCapability::builder()
            .backend(Arc::new(LocalBackend::with_driver(Arc::new(
                RecordingLocalDriver::new(),
            ))))
            .backend(Arc::new(ProviderHostedBackend::new("anthropic")))
            .hosted_emitter(Arc::new(CanonicalHostedEmitter))
            .trusted(true)
            .audit_sink(sink.clone())
            .build();
        cap.act_local(BrowserComputerAction::Title, &ws(), cancel())
            .await
            .unwrap();
        cap.snapshot_local(&ws(), cancel()).await.unwrap();
        cap.hosted_request(
            &BrowserComputerAction::Screenshot,
            &ToolCallId::new("hosted-call"),
        )
        .unwrap();
    }

    // 模拟重启：同一路径重新打开 sink，续写并全量 replay。
    let restarted = Arc::new(FileAuditSink::open(&path).unwrap());
    let cap2 = BrowserComputerCapability::builder()
        .backend(Arc::new(LocalBackend::with_driver(Arc::new(
            RecordingLocalDriver::new(),
        ))))
        .trusted(true)
        .audit_sink(restarted.clone())
        .build();
    cap2.act_local(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap();

    let records: Vec<AuditRecord> = restarted.replay().unwrap();
    assert!(records.iter().all(|r| r.version == AUDIT_FORMAT_VERSION));
    // 序号跨重启单调连续（3 + 1）。
    let seqs: Vec<u64> = records.iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4]);
    // 三类路径的审计都在（可重放）。
    let actions: Vec<&str> = records.iter().map(|r| r.audit.action.as_str()).collect();
    assert_eq!(
        actions,
        vec!["title", "snapshot_local", "screenshot", "title"]
    );
    assert!(records[0].audit.backend.is_some());
    assert_eq!(records[1].audit.policy, "allow");
    assert_eq!(
        records[2].audit.site.as_deref(),
        Some(ExecutionSite::ProviderHosted.as_str())
    );
}

// ---------- 12. P17-10 review（二轮）：act/snapshot 因果通过 SandboxGate ----------

#[tokio::test]
async fn local_process_style_act_fails_closed_before_driver() {
    let driver = Arc::new(RecordingLocalDriver::new());
    let backend = LocalBackend::with_driver(driver.clone()).process_style();
    assert_eq!(backend.process_gate(), ProcessMode::SpawnViaSandbox);
    let err = backend
        .act(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap_err();
    assert!(
        matches!(err, BrowserComputerError::SandboxDenied { .. }),
        "{err:?}"
    );
    // 闸门失败必须阻断 driver：调用顺序（gate 先于 driver）与失败阻断同时验证。
    assert!(driver.recorded().is_empty(), "driver must not be reached");
    assert!(driver.authorizations().is_empty());
}

#[tokio::test]
async fn local_process_style_snapshot_fails_closed_before_driver() {
    let driver = Arc::new(RecordingLocalDriver::new());
    let backend = LocalBackend::with_driver(driver.clone()).process_style();
    let err = backend.snapshot(&ws(), cancel()).await.unwrap_err();
    assert!(matches!(err, BrowserComputerError::SandboxDenied { .. }));
    assert!(driver.authorizations().is_empty());
}

#[tokio::test]
async fn local_act_receives_process_authorization_with_sandbox() {
    let sandbox = Arc::new(RecordingSandbox::new());
    let driver = Arc::new(RecordingLocalDriver::new());
    let backend = LocalBackend::with_driver(driver.clone()).with_sandbox(sandbox.clone());
    let snapshot = backend
        .act(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap();
    assert_eq!(snapshot.title.as_deref(), Some("Example"));
    // driver 收到携带 sandbox 的进程型授权。
    assert_eq!(
        driver.authorizations(),
        vec![(ProcessMode::SpawnViaSandbox, true)]
    );
}

#[tokio::test]
async fn local_in_process_act_explicit_no_spawn_authorization() {
    let driver = Arc::new(RecordingLocalDriver::new());
    let backend = LocalBackend::with_driver(driver.clone());
    assert_eq!(backend.process_gate(), ProcessMode::InProcess);
    backend
        .act(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap();
    // in-process 授权显式声明不 spawn（sandbox 恒 None）。
    assert_eq!(
        driver.authorizations(),
        vec![(ProcessMode::InProcess, false)]
    );
}

#[tokio::test]
async fn mcp_local_act_fails_closed_without_sandbox() {
    let driver = Arc::new(RecordingMcpDriver::new());
    let backend = McpBackend::new(McpOwnership::LocalProcess, driver.clone());
    let err = backend
        .act(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap_err();
    assert!(matches!(err, BrowserComputerError::SandboxDenied { .. }));
    assert!(driver.authorizations().is_empty());
}

#[tokio::test]
async fn mcp_act_receives_process_authorization_with_sandbox() {
    let sandbox = Arc::new(RecordingSandbox::new());
    let driver = Arc::new(RecordingMcpDriver::new());
    let backend =
        McpBackend::new(McpOwnership::LocalProcess, driver.clone()).with_sandbox(sandbox.clone());
    let snapshot = backend
        .act(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap();
    assert_eq!(snapshot.summary, "mcp title");
    assert_eq!(
        driver.authorizations(),
        vec![(ProcessMode::SpawnViaSandbox, true)]
    );
}

#[tokio::test]
async fn mcp_provider_mediated_act_and_snapshot_not_locally_executable() {
    let backend = McpBackend::new(
        McpOwnership::ProviderMediated,
        Arc::new(RecordingMcpDriver::new()),
    );
    let err = backend
        .act(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap_err();
    assert!(
        matches!(err, BrowserComputerError::NotLocallyExecutable { .. }),
        "{err:?}"
    );
    let err = backend.snapshot(&ws(), cancel()).await.unwrap_err();
    assert!(matches!(
        err,
        BrowserComputerError::NotLocallyExecutable { .. }
    ));
}

#[tokio::test]
async fn restart_without_sandbox_does_not_downgrade_to_in_process() {
    // 模拟重启后 sandbox 配置丢失：进程型后端必须 fail closed，不得静默退回 in-process。
    let driver = Arc::new(RecordingPlaywrightDriver::new());
    let backend = PlaywrightBackend::with_driver(driver.clone());
    assert_eq!(backend.process_gate(), ProcessMode::SpawnViaSandbox);
    let err = backend
        .act(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap_err();
    assert!(matches!(err, BrowserComputerError::SandboxDenied { .. }));
    assert!(driver.recorded().is_empty());
    assert!(driver.authorizations().is_empty());
}

#[tokio::test]
async fn playwright_act_receives_process_authorization_with_sandbox() {
    let sandbox = Arc::new(RecordingSandbox::new());
    let driver = Arc::new(RecordingPlaywrightDriver::new());
    let backend = PlaywrightBackend::with_driver(driver.clone()).with_sandbox(sandbox.clone());
    let snapshot = backend
        .act(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap();
    assert_eq!(snapshot.summary, "pw title");
    assert_eq!(
        driver.authorizations(),
        vec![(ProcessMode::SpawnViaSandbox, true)]
    );
}

#[tokio::test]
async fn authorization_spawn_explicitly_denied_for_in_process_and_preconnected() {
    use browser_computer_runtime::SandboxGate;
    for gate in [SandboxGate::in_process(), SandboxGate::preconnected()] {
        let auth = gate.acquire().expect("no-spawn authorization is granted");
        assert!(!auth.is_sandboxed());
        let err = auth
            .spawn(spec(), SandboxPolicy::default(), cancel())
            .await
            .err()
            .expect("spawn must be denied");
        assert!(
            matches!(err, SandboxError::Denied(_)),
            "no-spawn authorization must refuse spawn: {err:?}"
        );
    }
}

// ---------- 13. P17-10 review（二轮）：durable sink 失败在副作用前 fail-closed ----------

#[tokio::test]
async fn act_local_fails_closed_when_audit_sink_fails() {
    let driver = Arc::new(RecordingLocalDriver::new());
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(LocalBackend::with_driver(driver.clone())))
        .trusted(true)
        .audit_sink(Arc::new(FailingAuditSink))
        .build();
    let err = cap
        .act_local(BrowserComputerAction::Title, &ws(), cancel())
        .await
        .unwrap_err();
    assert!(matches!(err, BrowserComputerError::AuditSink(_)), "{err:?}");
    // 副作用前 fail-closed：driver 未被触达。
    assert!(driver.recorded().is_empty());
    assert!(driver.authorizations().is_empty());
}

#[tokio::test]
async fn snapshot_local_fails_closed_when_audit_sink_fails() {
    let driver = Arc::new(RecordingLocalDriver::new());
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(LocalBackend::with_driver(driver.clone())))
        .trusted(true)
        .audit_sink(Arc::new(FailingAuditSink))
        .build();
    let err = cap.snapshot_local(&ws(), cancel()).await.unwrap_err();
    assert!(matches!(err, BrowserComputerError::AuditSink(_)));
    assert!(driver.authorizations().is_empty());
}

#[tokio::test]
async fn hosted_request_fails_closed_when_audit_sink_fails() {
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(ProviderHostedBackend::new("anthropic")))
        .hosted_emitter(Arc::new(CanonicalHostedEmitter))
        .trusted(true)
        .audit_sink(Arc::new(FailingAuditSink))
        .build();
    let err = cap
        .hosted_request(
            &BrowserComputerAction::Screenshot,
            &ToolCallId::new("hosted-call"),
        )
        .unwrap_err();
    // 事件（副作用）不返回：调用方拿不到 ServerToolEvent。
    assert!(matches!(err, BrowserComputerError::AuditSink(_)));
}

#[tokio::test]
async fn policy_deny_still_denies_when_audit_sink_fails() {
    let driver = Arc::new(RecordingLocalDriver::new());
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(LocalBackend::with_driver(driver.clone())))
        .trusted(false)
        .audit_sink(Arc::new(FailingAuditSink))
        .build();
    let err = cap
        .act_local(
            BrowserComputerAction::Navigate {
                url: "https://example.test".into(),
            },
            &ws(),
            cancel(),
        )
        .await
        .unwrap_err();
    // deny 路径本身已阻断副作用；策略拒绝仍是操作结果。
    assert!(matches!(err, BrowserComputerError::PolicyDenied(_)));
    assert!(driver.recorded().is_empty());
}

// ---------- 14. P17-10 后审：ToolDescriptor schema 与 canonical parser 一致 ----------

#[test]
fn tool_schema_covers_every_action_and_required_fields_match_parser() {
    let cap = Arc::new(BrowserComputerCapability::builder().trusted(true).build());
    let schema = browser_computer_runtime::BrowserComputerTool::new(cap)
        .descriptor()
        .input_schema;
    let branches = schema["oneOf"].as_array().expect("schema oneOf");

    let expected_required = [
        ("navigate", vec!["action", "url"]),
        ("click", vec!["action"]),
        ("type", vec!["action", "text"]),
        ("key", vec!["action", "keys"]),
        ("scroll", vec!["action"]),
        ("screenshot", vec!["action"]),
        ("snapshot_dom", vec!["action"]),
        ("title", vec!["action"]),
    ];
    assert_eq!(branches.len(), expected_required.len());

    for (action, required) in expected_required {
        let branch = branches
            .iter()
            .find(|branch| branch["properties"]["action"]["const"] == action)
            .unwrap_or_else(|| panic!("missing schema branch for {action}"));
        let actual: Vec<&str> = branch["required"]
            .as_array()
            .expect("required array")
            .iter()
            .map(|value| value.as_str().expect("required string"))
            .collect();
        assert_eq!(actual, required, "required fields differ for {action}");
    }

    // click 的 parser 语义要求 selector / coordinate 至少一个，schema 用 anyOf 同步表达。
    let click = branches
        .iter()
        .find(|branch| branch["properties"]["action"]["const"] == "click")
        .unwrap();
    let click_alternatives: Vec<&str> = click["anyOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|alternative| alternative["required"][0].as_str().unwrap())
        .collect();
    assert_eq!(click_alternatives, vec!["selector", "coordinate"]);

    let valid = [
        serde_json::json!({"action":"navigate", "url":"https://example.test"}),
        serde_json::json!({"action":"click", "selector":"#submit"}),
        serde_json::json!({"action":"click", "coordinate":[12, 34]}),
        serde_json::json!({"action":"type", "text":"hello", "selector":"#input"}),
        serde_json::json!({"action":"key", "keys":"CTRL+L"}),
        serde_json::json!({"action":"scroll", "dx":1, "dy":2}),
        serde_json::json!({"action":"screenshot"}),
        serde_json::json!({"action":"snapshot_dom", "selector":"main"}),
        serde_json::json!({"action":"title"}),
    ];
    for input in valid {
        BrowserComputerAction::from_input(&input)
            .unwrap_or_else(|err| panic!("schema-valid input failed parser: {input}: {err}"));
    }

    let missing_required = [
        serde_json::json!({"action":"navigate"}),
        serde_json::json!({"action":"click"}),
        serde_json::json!({"action":"type"}),
        serde_json::json!({"action":"key"}),
    ];
    for input in missing_required {
        assert!(
            matches!(
                BrowserComputerAction::from_input(&input),
                Err(BrowserComputerError::InvalidInput(_))
            ),
            "missing required fields must fail parser: {input}"
        );
    }
}

// ---------- 15. P17-10 后审：artifact / selector / hosted emitter 回归 ----------

#[tokio::test]
async fn configured_artifact_store_failure_never_leaks_large_dom_from_facade() {
    let dir = tempfile::tempdir().unwrap();
    let store =
        artifact_store::ArtifactStore::open_with_options(artifact_store::ArtifactStoreOptions {
            root: dir.path().to_path_buf(),
            disk_budget: Some(1),
        })
        .await
        .unwrap();
    let secret_dom = "SENSITIVE-DOM".repeat(8 * 1024);
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(LocalBackend::with_driver(Arc::new(
            LargeDomDriver {
                dom: secret_dom.clone(),
            },
        ))))
        .trusted(true)
        .artifact_store(store)
        .large_payload_bytes(1024)
        .build();

    let snapshot = cap
        .act_local(
            BrowserComputerAction::SnapshotDom { selector: None },
            &ws(),
            cancel(),
        )
        .await
        .unwrap();
    assert!(snapshot.dom.is_none(), "full DOM must never escape facade");
    assert!(
        snapshot.artifacts.is_empty(),
        "failed put creates no reference"
    );
    assert_eq!(snapshot.metadata["truncated"].as_str(), Some("true"));
    assert!(snapshot.metadata["artifact_error"].as_str().is_some());
    assert!(!snapshot.summary.contains(&secret_dom));
}

#[test]
fn select_for_local_probes_each_backend_once() {
    let probes = Arc::new(AtomicUsize::new(0));
    let backend: Arc<dyn BrowserComputerBackend> = Arc::new(CountingProbeBackend {
        probes: probes.clone(),
    });
    let selection = select_for_local(&[backend]).expect("available backend selected");
    assert_eq!(selection.route.kind, BackendKind::Local);
    assert_eq!(probes.load(Ordering::SeqCst), 1);
    assert_eq!(selection.attempted.len(), 1);
    assert!(selection.attempted[0].available);
}

#[test]
fn hosted_request_fails_when_emitter_is_missing() {
    let cap = BrowserComputerCapability::builder()
        .backend(Arc::new(ProviderHostedBackend::new("anthropic")))
        .trusted(true)
        .build();
    let err = cap
        .hosted_request(
            &BrowserComputerAction::Screenshot,
            &ToolCallId::new("missing-emitter"),
        )
        .unwrap_err();
    assert!(matches!(
        err,
        BrowserComputerError::Backend {
            backend: "provider_hosted",
            ..
        }
    ));
    assert!(cap.last_audit().is_none(), "no hosted event was emitted");
}
