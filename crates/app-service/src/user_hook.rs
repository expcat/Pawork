//! User Hooks（P17-1）生产接线：六类 handler 的真实 adapter + 宿主装配。
//!
//! 本模块把纯领域 crate `user-hooks` 的注入执行器接口接到真实运行时：
//! - [`SandboxCommandExecutor`]：`Command` 经 **Sandbox Runtime → Process
//!   Runtime** 统一路径执行（本模块不直接 spawn 子进程；进程树清理 /
//!   timeout / cancel 语义由 Sandbox/Process 承担）；
//! - [`HttpHookExecutor`]：`Http` 复用共享 HTTP 运行时
//!   （`provider_runtime::http::HttpClient`，同一超时/代理/UA 配置栈）；
//! - [`HookPolicyGate`]：`policy-engine` 综合裁决；PromptTransform 的
//!   system 改写默认拒绝（不可绕过 security policy）；
//! - [`CanonicalJudge`]：`PromptEval` 单轮 canonical 模型判定、
//!   `AgentEval` 经 `agent-engine::ProviderLoop` 跑受限 Agent（受限
//!   profile / 工具 allowlist / 预算），**不按 Provider 名分支**；
//! - [`McpToolInvokerHost`]：`McpTool` 复用 `mcp-client` 的 P9 能力桥
//!   （`register_server_tools` → `McpToolAdapter`，含 P9-5 每 server
//!   审批与输出限制），handler 不获额外特权；
//! - [`BackendSecretResolver`]：secret 明文只经 `auth_service::SecretBackend`
//!   解析，配置 / 事件 / 日志全程只存引用；
//! - [`SqliteHookAuditSink`]：canonical `UserHookEvent` 的 durable、
//!   可重放审计存储。
//!
//! [`UserHookHost`] 装配以上全部 adapter 为 `user_hooks::Executors` +
//! `HookDispatcher`，并把 canonical `agent_events::AgentEvent` → 触发点的
//! 映射与事件桥（[`UserHookHost::spawn_event_bridge`]）落地：桥只订阅既有
//! run loop 的 `EventBroadcaster` 事件流，**不启动第二 run loop**。

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_domain::{
    CancellationToken, ContentPart, EventId, Message, MessageId, MessageMetadata, MessageRole,
    ModelId, ProviderId, RequestId, RunId, SessionId, TextContent, ToolCallId, WorkspaceId,
};
use agent_engine::{
    ApprovalOutcome, CancelHandle, EventBroadcaster, LoopContext, LoopError, MessageQueue,
    NoopProcessTreeCleaner, PendingToolInvocation, ProviderLoop, ProviderLoopConfig,
    ToolCallResult,
};
use agent_events::{AgentEvent, AgentEventEnvelope};
use async_trait::async_trait;
use auth_service::SecretBackend;
use mcp_client::capabilities::{
    namespaced_name, register_server_tools, McpApproval, McpApprovalDecision, McpApprovalRequest,
};
use mcp_client::config::McpPermissions;
use mcp_client::McpError;
use policy_engine::{ApprovalMode, PolicyDecision, PolicyEngine, PolicyInput};
use process_runtime::{CommandSpec, ProcessEvent};
use provider_api::{
    CanonicalModelRequest, ModelProvider, ProviderError, ProviderEventSink, ProviderStreamEvent,
    RequestBudget, ResponseFormat,
};
use provider_runtime::http::{HttpClient, HttpClientConfig};
use sandbox_runtime::{
    default_env_allowlist, default_secret_paths, FilesystemPolicy, NetworkMode, ResourceLimits,
    SandboxBackend, SandboxPolicy, SandboxProcessSpec, SandboxSelector,
};
use serde_json::json;
use tool_api::{ToolCapability, ToolExecutionContext, ToolRequest, ToolResult};
use tool_runtime::{NoopToolEventSink, ToolRegistry};
use user_hooks::audit::{DispatchOutcome, UserHookEvent, UserHookEventPayload};
use user_hooks::config::{HandlerConfig, HandlerLifecycle, HookConfig, HookScope};
use user_hooks::error::HookError;
use user_hooks::exec::{
    AsyncRunner, AuditSink, CommandExecutor, CommandRequest, CommandResult, Executors,
    HttpExecutor, JudgeDecision, JudgeMode, JudgeRequest, McpToolInvoker, McpToolRequest,
    McpToolResult, PolicyAction, PolicyGate, PolicyOutcome, ProviderJudge, SecretResolver,
    SystemHookClock, WebhookRequest, WebhookResult,
};
use user_hooks::secret::{SecretRef, SecretValue};
use user_hooks::trigger::{TriggerPayload, TriggerPoint};
use user_hooks::{HookCapability, HookDispatcher};

/// 执行器错误中使用的占位 hook id（执行器 trait 不携带 hook 上下文；
/// user-hooks dispatcher 会把错误包装进对应 hook 的审计记录）。
const GENERIC_HOOK_ID: &str = "user-hook";

// =========================================================================
// 支持 trait（canonical 解析接口）
// =========================================================================

/// 一个受限 Agent / 单轮判定的模型落点（provider + model，可选预算）。
#[derive(Clone, Debug)]
pub struct EvalProfile {
    pub provider_id: ProviderId,
    pub model: ModelId,
    /// P17-5 profile 自身的 system/instructions；AgentEval 不继承主 Agent prompt。
    pub system_prompt: Option<String>,
    /// P17-5 canonical reasoning effort；None 仅用于不引用 profile 的 PromptEval。
    pub reasoning_effort: Option<agent_domain::ReasoningEffort>,
    /// 额外预算覆盖（AgentEval）；None = 使用请求内 budget。
    pub budget: Option<agent_engine::BudgetLimits>,
    /// P17-5 profile 的工具约束；与 handler allowlist 取交集，deny 优先。
    pub tool_rules: agent_domain::ProfileToolRules,
    /// AgentEval 必须使用 P17-5 明确声明的受限隔离等级。
    pub isolation: agent_domain::ProfileIsolation,
}

impl EvalProfile {
    /// 从 P17-5 Agent Profile v2 构造受限 eval 落点。`max_turns` 直接成为
    /// ProviderLoop 的迭代硬上限；handler 自身 token/time 预算仍必须显式提供。
    pub fn restricted(
        provider_id: ProviderId,
        model: ModelId,
        system_prompt: String,
        reasoning_effort: agent_domain::ReasoningEffort,
        tool_rules: agent_domain::ProfileToolRules,
        max_turns: Option<u64>,
        isolation: agent_domain::ProfileIsolation,
    ) -> Self {
        Self {
            provider_id,
            model,
            system_prompt: Some(system_prompt),
            reasoning_effort: Some(reasoning_effort),
            budget: max_turns.map(|max_iterations| agent_engine::BudgetLimits {
                max_iterations: Some(max_iterations),
                ..Default::default()
            }),
            tool_rules,
            isolation,
        }
    }
}

/// 把 profile 引用解析为模型落点。宿主可接 agent profile 配置（P17-5）；
/// 本模块只定义契约，不做 Provider 名分支。
pub trait EvalProfileResolver: Send + Sync {
    fn resolve(&self, workspace_id: Option<&WorkspaceId>, profile: &str) -> Option<EvalProfile>;
}

/// 按 ProviderId 解析 canonical `ModelProvider`。
pub trait ProviderResolver: Send + Sync {
    fn resolve(&self, id: &ProviderId) -> Option<Arc<dyn ModelProvider>>;
}

/// 生产解析器：直接查 `CommandRouter` 的共享 Provider 注册表（正式宿主
/// 经 `AppService::register_provider` 注入同一批 provider），不按名称分支。
impl ProviderResolver for crate::router::CommandRouter {
    fn resolve(&self, id: &ProviderId) -> Option<Arc<dyn ModelProvider>> {
        self.provider(id)
    }
}

/// McpTool / AgentEval 执行时的工作区与 run 上下文（执行器 trait 不携带）。
pub trait HookRunContext: Send + Sync {
    fn workspace_id(&self) -> WorkspaceId;
    fn run_id(&self) -> RunId;
}

/// 固定上下文的 [`HookRunContext`]（测试 / 单 run 场景）。
#[derive(Clone, Debug)]
pub struct StaticHookRunContext {
    workspace_id: WorkspaceId,
    run_id: RunId,
}

impl StaticHookRunContext {
    pub fn new(workspace_id: impl Into<String>, run_id: impl Into<String>) -> Self {
        Self {
            workspace_id: WorkspaceId::new(workspace_id),
            run_id: RunId::new(run_id),
        }
    }
}

impl HookRunContext for StaticHookRunContext {
    fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id.clone()
    }
    fn run_id(&self) -> RunId {
        self.run_id.clone()
    }
}

// =========================================================================
// 1. Command：Sandbox → Process
// =========================================================================

/// `Command` handler 的生产执行器：经注入的 [`SandboxBackend`]（内部为
/// Sandbox Runtime → Process Runtime）执行，本模块不直接 spawn 进程。
///
/// 沙箱策略：workspace 根只读 + secret 目录拒绝 + env 清洗（默认 allowlist
/// 叠加请求 allowlist 的注入变量）+ 输出上限。命令 cwd 按首个 workspace 根
/// 解析相对路径；绝对路径由 sandbox 的 `ensure_within` 复核。
pub struct SandboxCommandExecutor {
    backend: Arc<dyn SandboxBackend>,
    workspace_roots: Vec<PathBuf>,
    max_output_bytes: u64,
}

impl SandboxCommandExecutor {
    pub fn new(backend: Arc<dyn SandboxBackend>, workspace_roots: Vec<PathBuf>) -> Self {
        Self {
            backend,
            workspace_roots,
            max_output_bytes: 1024 * 1024,
        }
    }

    pub fn with_max_output_bytes(mut self, bytes: u64) -> Self {
        self.max_output_bytes = bytes.max(1);
        self
    }

    fn policy(&self, request: &CommandRequest) -> SandboxPolicy {
        let mut env_allowlist = default_env_allowlist();
        for (name, _) in &request.env {
            if !env_allowlist.contains(name) {
                env_allowlist.push(name.clone());
            }
        }
        SandboxPolicy {
            filesystem: FilesystemPolicy {
                read_roots: self.workspace_roots.clone(),
                write_roots: Vec::new(),
                deny: default_secret_paths(),
            },
            network_mode: NetworkMode::Enforce,
            network_allow_hosts: Vec::new(),
            allow_spawn: true,
            max_procs: None,
            env_clear: true,
            env_allowlist,
            env_denylist: Vec::new(),
            resources: ResourceLimits {
                max_output_bytes: Some(self.max_output_bytes),
                ..Default::default()
            },
        }
    }

    fn resolve_cwd(&self, working_directory: &Option<String>) -> Option<PathBuf> {
        let dir = working_directory.as_ref()?;
        let path = Path::new(dir);
        if path.is_absolute() {
            return Some(path.to_path_buf());
        }
        let root = self.workspace_roots.first()?;
        Some(root.join(path))
    }
}

#[async_trait]
impl CommandExecutor for SandboxCommandExecutor {
    async fn run(
        &self,
        request: CommandRequest,
        timeout: Option<Duration>,
    ) -> Result<CommandResult, HookError> {
        // env：请求注入的 allowlisted 明文；PATH 缺失时补父进程 PATH，保证
        // program 可解析（env 清洗后子进程不继承任何环境）。
        let mut env: Vec<(String, String)> = request
            .env
            .iter()
            .map(|(name, value)| (name.clone(), value.as_str().to_string()))
            .collect();
        if !env.iter().any(|(name, _)| name == "PATH") {
            if let Ok(path) = std::env::var("PATH") {
                env.push(("PATH".into(), path));
            }
        }
        let mut spec = CommandSpec::new(request.program.clone());
        spec.args = request.args.clone();
        spec.cwd = self.resolve_cwd(&request.working_directory);
        spec.env_clear = true;
        spec.env = env;
        spec.max_output_bytes = self.max_output_bytes;

        let sandbox_spec = SandboxProcessSpec {
            command: spec,
            workspace_roots: self.workspace_roots.clone(),
        };
        let cancel = CancellationToken::new();
        let mut process = match self
            .backend
            .spawn(sandbox_spec, self.policy(&request), cancel.clone())
            .await
        {
            Ok(p) => p,
            Err(e) => {
                return Err(HookError::executor(
                    GENERIC_HOOK_ID,
                    format!("sandbox spawn failed: {e}"),
                ))
            }
        };

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code = None;
        let mut timed_out = false;
        let deadline = timeout.map(|duration| tokio::time::Instant::now() + duration);
        loop {
            let event = match deadline {
                Some(deadline) => {
                    match tokio::time::timeout_at(deadline, process.events.recv()).await {
                        Ok(event) => event,
                        Err(_) => {
                            // 超时：取消沙箱进程树，等待清理完成与 Exit 事件。
                            timed_out = true;
                            cancel.cancel();
                            let grace = tokio::time::Instant::now() + Duration::from_secs(5);
                            loop {
                                match tokio::time::timeout_at(grace, process.events.recv()).await {
                                    Ok(Some(ProcessEvent::Exit { code, .. })) => {
                                        exit_code = code;
                                        break;
                                    }
                                    Ok(Some(_)) => continue,
                                    _ => break,
                                }
                            }
                            break;
                        }
                    }
                }
                None => process.events.recv().await,
            };
            match event {
                Some(ProcessEvent::Stdout(bytes)) => stdout.extend(bytes),
                Some(ProcessEvent::Stderr(bytes)) => stderr.extend(bytes),
                Some(ProcessEvent::Exit { code, .. }) => {
                    exit_code = code;
                    break;
                }
                None => break,
            }
        }
        let limit = self.max_output_bytes as usize;
        stdout.truncate(limit);
        stderr.truncate(limit);
        // 输出 redaction：子进程 stdout/stderr 可能回显注入的 secret 明文
        // （如 `echo $TOKEN`），按请求 env 明文逐值替换为占位符后再返回。
        let secret_values: Vec<&str> = request
            .env
            .iter()
            .map(|(_, value)| value.as_str())
            .collect();
        let stdout = redact_all(&String::from_utf8_lossy(&stdout), &secret_values);
        let stderr = redact_all(&String::from_utf8_lossy(&stderr), &secret_values);
        Ok(CommandResult {
            exit_code: exit_code.unwrap_or(if timed_out { 124 } else { 1 }),
            stdout,
            stderr,
            timed_out,
        })
    }
}

// =========================================================================
// 2. Http：复用共享 HTTP 运行时
// =========================================================================

/// `Http` handler 的生产执行器：复用 `provider_runtime::http::HttpClient`
/// （同一超时 / 代理 / UA 配置栈），允许任意方法 / header / body。
pub struct HttpHookExecutor {
    client: HttpClient,
}

impl HttpHookExecutor {
    pub fn new(config: HttpClientConfig) -> Result<Self, HookError> {
        Ok(Self {
            client: HttpClient::new(config).map_err(|e| {
                HookError::executor(GENERIC_HOOK_ID, format!("http client init failed: {e}"))
            })?,
        })
    }
}

#[async_trait]
impl HttpExecutor for HttpHookExecutor {
    async fn send(
        &self,
        request: WebhookRequest,
        timeout: Option<Duration>,
    ) -> Result<WebhookResult, HookError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|e| {
            HookError::executor(GENERIC_HOOK_ID, format!("invalid http method: {e}"))
        })?;
        let mut builder = self.client.inner().request(method, &request.url);
        for (name, value) in &request.headers {
            // allowlisted header 明文只短暂存活于请求构造，随后 Drop 清零。
            builder = builder.header(name, value.as_str());
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        let send_fut = builder.send();
        tokio::pin!(send_fut);
        let mut timed_out = false;
        let response = if let Some(duration) = timeout {
            match tokio::time::timeout(duration, &mut send_fut).await {
                Ok(Ok(response)) => response,
                Ok(Err(e)) => {
                    return Err(HookError::executor(
                        GENERIC_HOOK_ID,
                        format!("http request failed: {e}"),
                    ))
                }
                Err(_) => {
                    timed_out = true;
                    // 请求已交给 reqwest 内部取消；无法直接中止，记录超时返回。
                    return Ok(WebhookResult {
                        status: 0,
                        body: String::new(),
                        timed_out,
                    });
                }
            }
        } else {
            match send_fut.await {
                Ok(response) => response,
                Err(e) => {
                    return Err(HookError::executor(
                        GENERIC_HOOK_ID,
                        format!("http request failed: {e}"),
                    ))
                }
            }
        };
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        // 输出 redaction：响应体可能回显注入的 header secret 明文，按请求
        // header 明文逐值替换为占位符后再返回。
        let secret_values: Vec<&str> = request
            .headers
            .iter()
            .map(|(_, value)| value.as_str())
            .collect();
        let body = redact_all(&body, &secret_values);
        Ok(WebhookResult {
            status,
            body,
            timed_out,
        })
    }
}

/// 按一组明文逐值替换为 redaction 占位符（不泄露 secret）。
fn redact_all(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for plain in secrets {
        if !plain.is_empty() && out.contains(plain) {
            out = out.replace(plain, user_hooks::REDACTED);
        }
    }
    out
}

// =========================================================================
// 3. 策略门：policy-engine + PromptTransform system 保护
// =========================================================================

/// `PolicyGate` 生产实现：经 `policy-engine` 综合裁决；hook 能力 → canonical
/// 工具能力映射；`AskUser` 对 user hook 一律 fail-closed（hook 是非交互
/// 自动化；交互审批不在本门面，McpTool 的 P9-5 审批在其 adapter 内）。
///
/// PromptTransform 的 security policy：改写 `System` 且未显式
/// `allow_system_override` 一律拒绝；显式允许后仍受 [`PolicyEngine`] 复核。
/// Policy 每次执行时读取的 workspace trust。生产实现必须读取真实服务状态，
/// 解析失败返回 None，由 gate 按 untrusted 处理。
pub trait HookWorkspaceTrustResolver: Send + Sync {
    fn is_trusted(&self, workspace_id: &WorkspaceId) -> Option<bool>;
}

struct StaticWorkspaceTrust(BTreeMap<WorkspaceId, bool>);

impl HookWorkspaceTrustResolver for StaticWorkspaceTrust {
    fn is_trusted(&self, workspace_id: &WorkspaceId) -> Option<bool> {
        self.0.get(workspace_id).copied()
    }
}

pub struct HookPolicyGate {
    engine: PolicyEngine,
    default_trusted: bool,
    workspace_trust: Option<Arc<dyn HookWorkspaceTrustResolver>>,
}

impl HookPolicyGate {
    pub fn new(mode: ApprovalMode, trusted: bool) -> Self {
        Self {
            engine: PolicyEngine::new(mode),
            default_trusted: trusted,
            workspace_trust: None,
        }
    }

    pub fn with_workspace_trust(
        mode: ApprovalMode,
        workspace_trust: BTreeMap<WorkspaceId, bool>,
    ) -> Self {
        Self {
            engine: PolicyEngine::new(mode),
            default_trusted: false,
            workspace_trust: Some(Arc::new(StaticWorkspaceTrust(workspace_trust))),
        }
    }

    pub fn with_trust_resolver(
        mode: ApprovalMode,
        workspace_trust: Arc<dyn HookWorkspaceTrustResolver>,
    ) -> Self {
        Self {
            engine: PolicyEngine::new(mode),
            default_trusted: false,
            workspace_trust: Some(workspace_trust),
        }
    }

    fn is_trusted(&self, workspace: Option<&WorkspaceId>) -> bool {
        match (&self.workspace_trust, workspace) {
            (Some(resolver), Some(id)) => resolver.is_trusted(id).unwrap_or(false),
            (Some(_), None) => false,
            (None, _) => self.default_trusted,
        }
    }

    fn capability_for(capability: HookCapability) -> ToolCapability {
        match capability {
            HookCapability::Process => ToolCapability::Process,
            HookCapability::Network => ToolCapability::Network,
            HookCapability::McpTool => ToolCapability::ExternalPlugin,
            // 模型判定本质是 Provider 调用（token 消耗），按 Network 语义门控。
            HookCapability::PromptEval | HookCapability::AgentEval => ToolCapability::Network,
            // PromptTransform 在 evaluate 中单独裁决，不进入引擎。
            HookCapability::PromptTransform => ToolCapability::ReadOnly,
        }
    }
}

#[async_trait]
impl PolicyGate for HookPolicyGate {
    async fn evaluate(&self, action: PolicyAction<'_>) -> PolicyOutcome {
        // PromptTransform security policy：system 改写默认拒绝。
        if action.capability == HookCapability::PromptTransform {
            if let Some(user_hooks::config::PromptTarget::System) = action.prompt_target {
                if !action.allow_system_override {
                    return PolicyOutcome::Deny {
                        reason: "system prompt override is not permitted by user-hook policy"
                            .into(),
                    };
                }
                // 显式 override 仍必须过 policy-engine 复核，绝不旁路引擎。
            }
        }
        self.engine_decision(&action)
    }

    fn allows_eval_fail_open(&self, workspace: Option<&WorkspaceId>) -> bool {
        self.is_trusted(workspace)
    }
}

impl HookPolicyGate {
    fn engine_decision(&self, action: &PolicyAction<'_>) -> PolicyOutcome {
        let decision = self.engine.decide(&PolicyInput {
            capability: Self::capability_for(action.capability),
            input: action.description.clone(),
            trusted: self.is_trusted(action.workspace_id),
            allowed_in_untrusted_workspace: false,
            approval_mode: self.engine.mode(),
        });
        match decision {
            PolicyDecision::Allow => PolicyOutcome::Allow,
            PolicyDecision::Deny { reason } => PolicyOutcome::Deny { reason },
            PolicyDecision::AllowWithConstraints { constraints } => {
                PolicyOutcome::AllowWithConstraints {
                    timeout_ms: constraints.timeout_ms,
                }
            }
            PolicyDecision::AskUser { .. } => PolicyOutcome::Deny {
                reason: "user hooks are non-interactive; interactive approval is not supported"
                    .into(),
            },
        }
    }
}

// =========================================================================
// 4. PromptEval / AgentEval：canonical provider / engine，受限 Agent
// =========================================================================

/// 收集 provider 流式文本增量的事件 sink（判定文本）。
struct TextSink {
    text: std::sync::Mutex<String>,
}

impl TextSink {
    fn new() -> Self {
        Self {
            text: std::sync::Mutex::new(String::new()),
        }
    }
}

#[async_trait]
impl ProviderEventSink for TextSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        if let ProviderStreamEvent::TextDelta(delta) = event {
            self.text.lock().expect("text sink").push_str(&delta);
        }
        Ok(())
    }
}

/// 受限 Agent 判定的 [`LoopContext`]：工具仅 allowlist 内可执行（no-op
/// 成功语义，与 P13-1 `AppLoopContext` 一致），其余拒绝；无审批交互
/// （工具已预授权）；无 hosted / extension 工具。
struct EvalLoopContext {
    allowlist: Vec<String>,
    next_message: AtomicU64,
    next_request: AtomicU64,
}

impl EvalLoopContext {
    fn new(allowlist: Vec<String>) -> Self {
        Self {
            allowlist,
            next_message: AtomicU64::new(0),
            next_request: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl LoopContext for EvalLoopContext {
    async fn execute_tools(
        &self,
        calls: Vec<PendingToolInvocation>,
        _events: agent_engine::LoopEventEmitter,
        _cancel: CancellationToken,
    ) -> Vec<ToolCallResult> {
        calls
            .into_iter()
            .map(|call| {
                let allowed = self.allowlist.contains(&call.name);
                ToolCallResult {
                    tool_call_id: call.tool_call_id,
                    tool_name: call.name.clone(),
                    arguments: call.arguments,
                    result: if allowed {
                        ToolResult::success(vec![ContentPart::Text(TextContent {
                            text: format!("eval tool `{}` executed", call.name),
                        })])
                    } else {
                        ToolResult::failure(agent_domain::ErrorContext {
                            category: agent_domain::ErrorCategory::Authorization,
                            message: format!("tool `{}` is not on the eval allowlist", call.name),
                            retryable: false,
                            retry_after_ms: None,
                            diagnostics: Default::default(),
                        })
                    },
                }
            })
            .collect()
    }

    async fn request_approval(
        &self,
        calls: &[PendingToolInvocation],
        _cancel: CancellationToken,
    ) -> Vec<ApprovalOutcome> {
        // 工具已由 allowlist 预授权，判定 Agent 不请求用户审批。
        vec![ApprovalOutcome::Approved; calls.len()]
    }

    fn next_message_id(&self) -> MessageId {
        MessageId::new(format!(
            "hook-eval-{}",
            self.next_message.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn next_request_id(&self) -> RequestId {
        RequestId::new(format!(
            "hook-eval-{}",
            self.next_request.fetch_add(1, Ordering::Relaxed)
        ))
    }
}

/// PromptEval / AgentEval 的生产判定执行器。
///
/// - `SingleTurn`：一条 canonical 模型请求（无工具），解析判定文本；
/// - `ConstrainedAgent`：`agent-engine::ProviderLoop` 单次受限运行
///   （独立 profile / 工具 allowlist / 受限预算），解析最终 assistant 文本。
///
/// 两者都不按 Provider 名分支；profile → (provider, model) 经
/// [`EvalProfileResolver`] 解析。
pub struct CanonicalJudge {
    providers: Arc<dyn ProviderResolver>,
    default_profile: EvalProfile,
    profiles: Arc<dyn EvalProfileResolver>,
    broadcaster: EventBroadcaster,
    next_eval_run: AtomicU64,
}

impl CanonicalJudge {
    pub fn new(
        providers: Arc<dyn ProviderResolver>,
        default_profile: EvalProfile,
        profiles: Arc<dyn EvalProfileResolver>,
    ) -> Self {
        Self {
            providers,
            default_profile,
            profiles,
            broadcaster: EventBroadcaster::new(),
            next_eval_run: AtomicU64::new(0),
        }
    }

    fn profile_for(&self, request: &JudgeRequest) -> Result<EvalProfile, HookError> {
        match request.restricted_profile.as_deref() {
            Some(name) => self
                .profiles
                .resolve(request.workspace_id.as_ref(), name)
                .ok_or_else(|| HookError::PolicyDenied {
                    hook_id: GENERIC_HOOK_ID.into(),
                    reason: format!("restricted eval profile `{name}` is unavailable"),
                }),
            None => Ok(self.default_profile.clone()),
        }
    }

    fn provider_for(&self, profile: &EvalProfile) -> Result<Arc<dyn ModelProvider>, HookError> {
        self.providers.resolve(&profile.provider_id).ok_or_else(|| {
            HookError::executor(
                GENERIC_HOOK_ID,
                format!("no provider registered for `{}`", profile.provider_id),
            )
        })
    }

    async fn single_turn(
        &self,
        request: JudgeRequest,
        timeout: Option<Duration>,
    ) -> Result<JudgeDecision, HookError> {
        let profile = self.profile_for(&request)?;
        let provider = self.provider_for(&profile)?;
        let user_message = Message {
            id: MessageId::new("hook-eval-prompt"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: request.prompt.clone(),
            })],
            metadata: MessageMetadata::default(),
        };
        let mut messages = Vec::new();
        if let Some(system_prompt) = profile
            .system_prompt
            .as_ref()
            .filter(|prompt| !prompt.trim().is_empty())
        {
            messages.push(Message {
                id: MessageId::new("hook-eval-system"),
                role: MessageRole::System,
                content: vec![ContentPart::Text(TextContent {
                    text: system_prompt.clone(),
                })],
                metadata: MessageMetadata::default(),
            });
        }
        messages.push(user_message);
        let response_format = match &request.response_schema {
            Some(schema) => ResponseFormat::JsonSchema {
                name: "hook_decision".into(),
                schema: schema.clone(),
            },
            None => ResponseFormat::Text,
        };
        let mut canonical = CanonicalModelRequest {
            request_id: RequestId::new("hook-eval-request"),
            model: profile.model.clone(),
            messages,
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: provider_api::ToolChoice::default(),
            thinking: None,
            reasoning: profile
                .reasoning_effort
                .map(provider_api::ReasoningConfig::new),
            temperature: Some(0.0),
            max_output_tokens: None,
            stop_sequences: Vec::new(),
            response_format,
            prompt_cache: provider_api::PromptCachePreference::default(),
            budget: RequestBudget {
                timeout_ms: timeout.map(|d| d.as_millis() as u64),
                ..Default::default()
            },
            provider_options: BTreeMap::new(),
            trace_id: None,
        };
        if let Some(budget) = profile.budget {
            canonical.budget.max_input_tokens = budget.max_input_tokens;
            canonical.budget.max_cost_micros = budget.max_cost_micros;
        }
        let sink = TextSink::new();
        let cancel = CancellationToken::new();
        self.with_timeout(timeout, provider.stream(canonical, &sink, cancel.clone()))
            .await?;
        let text = sink.text.lock().expect("text sink").clone();
        Ok(parse_hook_decision(&text))
    }

    async fn constrained_agent(
        &self,
        request: JudgeRequest,
        timeout: Option<Duration>,
    ) -> Result<JudgeDecision, HookError> {
        let profile = self.profile_for(&request)?;
        let (effective_tools, budget) = constrained_eval_limits(&profile, &request)?;
        let provider = self.provider_for(&profile)?;
        let eval_run = self.next_eval_run.fetch_add(1, Ordering::Relaxed);
        let run_id = RunId::new(format!("hook-agent-eval-{eval_run}"));
        let session_id = SessionId::new(format!("hook-agent-eval-session-{eval_run}"));

        let tools: Vec<provider_api::ToolDefinition> = effective_tools
            .iter()
            .map(|name| provider_api::ToolDefinition {
                name: name.clone(),
                description: "restricted eval tool".into(),
                input_schema: json!({ "type": "object" }),
            })
            .collect();
        let user_message = Message {
            id: MessageId::new(format!("hook-agent-eval-{eval_run}")),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: request.prompt.clone(),
            })],
            metadata: MessageMetadata::default(),
        };
        let mut initial_messages = Vec::new();
        if let Some(system_prompt) = profile
            .system_prompt
            .as_ref()
            .filter(|prompt| !prompt.trim().is_empty())
        {
            initial_messages.push(Message {
                id: MessageId::new(format!("hook-agent-eval-system-{eval_run}")),
                role: MessageRole::System,
                content: vec![ContentPart::Text(TextContent {
                    text: system_prompt.clone(),
                })],
                metadata: MessageMetadata::default(),
            });
        }
        initial_messages.push(user_message);
        let config = ProviderLoopConfig {
            session_id,
            run_id: run_id.clone(),
            provider_id: profile.provider_id.clone(),
            model: profile.model.clone(),
            tools,
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            initial_messages,
            max_iterations: budget.max_iterations.unwrap_or(3).min(3),
            budget,
            retry: agent_engine::RetryPolicy::default(),
            thinking: None,
            reasoning: profile
                .reasoning_effort
                .map(provider_api::ReasoningConfig::new),
        };
        let context = Arc::new(EvalLoopContext::new(effective_tools));
        let mut loop_engine =
            ProviderLoop::new(provider, context, config, 1, self.broadcaster.clone());
        let queue = Arc::new(MessageQueue::new());
        let cancel = CancelHandle::new(run_id.clone(), Arc::new(NoopProcessTreeCleaner));
        let run = async {
            loop_engine
                .run(queue, cancel.clone())
                .await
                .map_err(|e| match e {
                    LoopError::Cancelled => HookError::Cancelled {
                        hook_id: GENERIC_HOOK_ID.into(),
                    },
                    other => HookError::executor(
                        GENERIC_HOOK_ID,
                        format!("constrained eval agent failed: {other}"),
                    ),
                })
        };
        let outcome = self.with_timeout(timeout, run).await;
        match outcome {
            Ok((_state, _summary)) => {
                let text = last_assistant_text(loop_engine.messages());
                Ok(parse_hook_decision(&text))
            }
            Err(HookError::Cancelled { .. }) => Err(HookError::Cancelled {
                hook_id: GENERIC_HOOK_ID.into(),
            }),
            Err(e) => Err(e),
        }
    }

    /// 统一超时裁剪；None → 不限。超时返回 [`HookError::Timeout`]。
    async fn with_timeout<T, E>(
        &self,
        timeout: Option<Duration>,
        future: impl std::future::Future<Output = Result<T, E>>,
    ) -> Result<T, HookError>
    where
        E: std::fmt::Display,
    {
        match timeout {
            Some(duration) => match tokio::time::timeout(duration, future).await {
                Ok(result) => result.map_err(|e| {
                    HookError::executor(GENERIC_HOOK_ID, format!("eval execution failed: {e}"))
                }),
                Err(_) => Err(HookError::Timeout {
                    hook_id: GENERIC_HOOK_ID.into(),
                    timeout_ms: duration.as_millis() as u64,
                }),
            },
            None => future.await.map_err(|e| {
                HookError::executor(GENERIC_HOOK_ID, format!("eval execution failed: {e}"))
            }),
        }
    }
}

#[async_trait]
impl ProviderJudge for CanonicalJudge {
    async fn judge(
        &self,
        request: JudgeRequest,
        timeout: Option<Duration>,
    ) -> Result<JudgeDecision, HookError> {
        match request.mode {
            JudgeMode::SingleTurn => self.single_turn(request, timeout).await,
            JudgeMode::ConstrainedAgent => self.constrained_agent(request, timeout).await,
        }
    }
}

/// 判定文本 → 决策（与 user-hooks 的 McpTool 解析共用同一 canonical 约定：
/// `allow`（默认）→ Allow；`deny[: reason]` → Deny；`transform: <text>` →
/// Transform）。
fn parse_hook_decision(text: &str) -> JudgeDecision {
    let lower = text.trim().to_ascii_lowercase();
    if lower.starts_with("deny") {
        let reason = text.trim().trim_start_matches("deny").trim();
        let reason = reason.trim_start_matches(':').trim();
        JudgeDecision::Deny {
            reason: if reason.is_empty() {
                "denied by eval hook".into()
            } else {
                reason.to_string()
            },
        }
    } else if lower.starts_with("transform:") {
        let new_prompt = text
            .trim()
            .trim_start_matches("transform:")
            .trim()
            .to_string();
        JudgeDecision::Transform { new_prompt }
    } else {
        JudgeDecision::Allow
    }
}

/// 取消息历史中最后一条 assistant 文本（判定 Agent 的最终答复）。
fn last_assistant_text(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::Assistant)
        .map(|m| {
            m.content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn min_limit(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

/// AgentEval 的唯一约束归一入口：handler 与 P17-5 profile 均只能收紧权限。
/// 未显式隔离、未显式预算、未被 profile allowlist 点名的工具全部 fail-closed。
fn constrained_eval_limits(
    profile: &EvalProfile,
    request: &JudgeRequest,
) -> Result<(Vec<String>, agent_engine::BudgetLimits), HookError> {
    if profile.isolation == agent_domain::ProfileIsolation::None {
        return Err(HookError::PolicyDenied {
            hook_id: GENERIC_HOOK_ID.into(),
            reason: "constrained eval profile must declare restricted or container isolation"
                .into(),
        });
    }
    let requested_budget = request.budget.ok_or_else(|| HookError::PolicyDenied {
        hook_id: GENERIC_HOOK_ID.into(),
        reason: "constrained eval requires an explicit token/time budget".into(),
    })?;
    let max_tokens = requested_budget
        .max_tokens
        .filter(|value| *value > 0)
        .ok_or_else(|| HookError::PolicyDenied {
            hook_id: GENERIC_HOOK_ID.into(),
            reason: "constrained eval requires a positive max_tokens budget".into(),
        })?;
    let timeout_ms = requested_budget
        .timeout_ms
        .filter(|value| *value > 0)
        .ok_or_else(|| HookError::PolicyDenied {
            hook_id: GENERIC_HOOK_ID.into(),
            reason: "constrained eval requires a positive timeout_ms budget".into(),
        })?;
    for name in &request.tool_allowlist {
        if !profile.tool_rules.is_allowed(name) {
            return Err(HookError::PolicyDenied {
                hook_id: GENERIC_HOOK_ID.into(),
                reason: format!("tool `{name}` is outside the restricted eval profile allowlist"),
            });
        }
    }

    let mut budget = agent_engine::BudgetLimits {
        max_iterations: Some(3),
        max_input_tokens: Some(max_tokens),
        max_output_tokens: Some(max_tokens),
        max_duration_ms: Some(timeout_ms),
        ..Default::default()
    };
    if let Some(extra) = &profile.budget {
        budget.max_iterations = min_limit(budget.max_iterations, extra.max_iterations);
        budget.max_tool_calls = min_limit(budget.max_tool_calls, extra.max_tool_calls);
        budget.max_input_tokens = min_limit(budget.max_input_tokens, extra.max_input_tokens);
        budget.max_output_tokens = min_limit(budget.max_output_tokens, extra.max_output_tokens);
        budget.max_cost_micros = min_limit(budget.max_cost_micros, extra.max_cost_micros);
        budget.max_duration_ms = min_limit(budget.max_duration_ms, extra.max_duration_ms);
        budget.max_output_bytes = min_limit(budget.max_output_bytes, extra.max_output_bytes);
        budget.max_artifact_bytes = min_limit(budget.max_artifact_bytes, extra.max_artifact_bytes);
        budget.max_concurrency = min_limit(budget.max_concurrency, extra.max_concurrency);
    }
    Ok((request.tool_allowlist.clone(), budget))
}

// =========================================================================
// 5. McpTool：复用 mcp-client（P9 能力桥 + P9-5 每 server 审批）
// =========================================================================

/// user hook 的 P9-5 审批通道：hook 是非交互自动化，`AskUser` 一律
/// fail-closed 拒绝（host 可替换为真实审批通道）。
pub struct HookMcpApproval;

#[async_trait]
impl McpApproval for HookMcpApproval {
    async fn decide(&self, _request: &McpApprovalRequest) -> McpApprovalDecision {
        McpApprovalDecision::Denied {
            reason: "user hooks are non-interactive; per-server approval denied".into(),
        }
    }
}

/// `McpTool` handler 的生产执行器：经 `mcp-client` 注册的 canonical
/// `AgentTool`（`McpToolAdapter`，含 P9-5 审批与输出限制）执行，handler
/// 不获额外特权。
pub struct McpToolInvokerHost {
    registry: Arc<tokio::sync::Mutex<ToolRegistry>>,
    context: Arc<dyn HookRunContext>,
    next_call: AtomicU64,
}

impl Clone for McpToolInvokerHost {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
            context: Arc::clone(&self.context),
            next_call: AtomicU64::new(0),
        }
    }
}

impl McpToolInvokerHost {
    pub fn new(
        registry: Arc<tokio::sync::Mutex<ToolRegistry>>,
        context: Arc<dyn HookRunContext>,
    ) -> Self {
        Self {
            registry,
            context,
            next_call: AtomicU64::new(0),
        }
    }

    /// 注册一个 MCP server 的全部工具到共享 registry（复用 mcp-client 的
    /// discovery + `McpToolAdapter`，P9-5 语义由 adapter 承担）。
    pub async fn register_mcp_server(
        &self,
        server: &str,
        peer: Arc<dyn mcp_client::McpPeer>,
        permissions: McpPermissions,
        trusted: bool,
        approval: Arc<dyn McpApproval>,
    ) -> Result<Vec<tool_api::ToolDescriptor>, McpError> {
        let mut registry = self.registry.lock().await;
        register_server_tools(&mut registry, server, peer, permissions, trusted, approval).await
    }
}

#[async_trait]
impl McpToolInvoker for McpToolInvokerHost {
    async fn invoke(
        &self,
        request: McpToolRequest,
        timeout: Option<Duration>,
    ) -> Result<McpToolResult, HookError> {
        let namespaced = namespaced_name(&request.server_id, &request.tool_name);
        let tool = {
            let registry = self.registry.lock().await;
            registry.get(&namespaced)
        };
        let tool = tool.ok_or_else(|| {
            HookError::executor(
                GENERIC_HOOK_ID,
                format!("mcp tool `{namespaced}` is not registered"),
            )
        })?;
        let tool_request = ToolRequest {
            tool_call_id: ToolCallId::new(format!(
                "hook-mcp-{}",
                self.next_call.fetch_add(1, Ordering::Relaxed)
            )),
            input: request.arguments,
        };
        let context = ToolExecutionContext {
            workspace_id: request
                .workspace_id
                .clone()
                .unwrap_or_else(|| self.context.workspace_id()),
            run_id: request
                .run_id
                .clone()
                .unwrap_or_else(|| self.context.run_id()),
            working_directory: None,
        };
        let cancel = CancellationToken::new();
        let execute = tool.execute(tool_request, context, &NoopToolEventSink, cancel.clone());
        tokio::pin!(execute);
        let result = if let Some(duration) = timeout {
            match tokio::time::timeout(duration, &mut execute).await {
                Ok(result) => result,
                Err(_) => {
                    cancel.cancel();
                    return Err(HookError::Timeout {
                        hook_id: GENERIC_HOOK_ID.into(),
                        timeout_ms: duration.as_millis() as u64,
                    });
                }
            }
        } else {
            execute.await
        };
        match result {
            Ok(result) => Ok(McpToolResult {
                success: result.success,
                text: text_of_tool_result(&result),
            }),
            Err(e) if e.kind == tool_api::ToolErrorKind::Cancelled => Err(HookError::Cancelled {
                hook_id: GENERIC_HOOK_ID.into(),
            }),
            Err(e) if e.kind == tool_api::ToolErrorKind::Timeout => Err(HookError::Timeout {
                hook_id: GENERIC_HOOK_ID.into(),
                timeout_ms: timeout.map(|d| d.as_millis() as u64).unwrap_or(0),
            }),
            Err(e) => Err(HookError::executor(
                GENERIC_HOOK_ID,
                format!("mcp tool execution failed: {e}"),
            )),
        }
    }
}

fn text_of_tool_result(result: &ToolResult) -> String {
    let mut parts = Vec::new();
    for part in &result.content {
        if let ContentPart::Text(t) = part {
            parts.push(t.text.clone());
        }
    }
    if let Some(error) = &result.error {
        parts.push(format!("error: {}", error.message));
    }
    parts.join("\n")
}

// =========================================================================
// 6. Secret 解析 + 审计持久化 + async 投递
// =========================================================================

/// `SecretResolver` 生产实现：明文只存于 `auth_service::SecretBackend`
/// （Keychain / 内存），配置与事件只存引用；解析失败统一归一为
/// `SecretUnavailable`，不泄露引用细节。
pub struct BackendSecretResolver {
    backend: Arc<dyn SecretBackend>,
}

/// SecretBackend 中的 user-hook secret 命名空间。
pub const HOOK_SECRET_SERVICE: &str = "pawork.user-hooks";

impl BackendSecretResolver {
    pub fn new(backend: Arc<dyn SecretBackend>) -> Self {
        Self { backend }
    }
}

impl SecretResolver for BackendSecretResolver {
    fn resolve(&self, reference: &SecretRef) -> Result<SecretValue, HookError> {
        match self.backend.get(HOOK_SECRET_SERVICE, reference.as_str()) {
            Ok(value) => Ok(SecretValue::new(value)),
            Err(_) => Err(HookError::SecretUnavailable {
                hook_id: reference.as_str().into(),
            }),
        }
    }
}

/// `AuditSink` 生产实现：canonical `UserHookEvent` 追加到 SQLite（幂等
/// INSERT OR IGNORE，按 event_id 去重），可全量重放。
pub struct SqliteHookAuditSink {
    conn: Mutex<rusqlite::Connection>,
    /// 写失败计数（序列化失败 / SQLite 错误）；**写失败不静默**：每次失败
    /// 都会 `tracing::error!` 记录并递增计数，调用方可轮询诊断。
    failures: AtomicU64,
}

impl SqliteHookAuditSink {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HookError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                HookError::executor(
                    GENERIC_HOOK_ID,
                    format!("create dir {}: {e}", parent.display()),
                )
            })?;
        }
        let conn = rusqlite::Connection::open(path).map_err(|e| {
            HookError::executor(GENERIC_HOOK_ID, format!("open {}: {e}", path.display()))
        })?;
        Self::with_connection(conn)
    }

    pub fn in_memory() -> Result<Self, HookError> {
        let conn = rusqlite::Connection::open_in_memory()
            .map_err(|e| HookError::executor(GENERIC_HOOK_ID, format!("open in-memory: {e}")))?;
        Self::with_connection(conn)
    }

    fn with_connection(conn: rusqlite::Connection) -> Result<Self, HookError> {
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| HookError::executor(GENERIC_HOOK_ID, format!("busy timeout: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS user_hook_events (
                event_id TEXT PRIMARY KEY,
                schema_version INTEGER NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                hook_id TEXT NOT NULL,
                trigger TEXT NOT NULL,
                scope TEXT NOT NULL,
                capability TEXT NOT NULL,
                lifecycle TEXT NOT NULL,
                payload TEXT NOT NULL
            )",
        )
        .map_err(|e| HookError::executor(GENERIC_HOOK_ID, format!("create table: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
            failures: AtomicU64::new(0),
        })
    }

    /// 自打开以来审计写入失败次数（写失败不静默的观测出口）。
    pub fn failure_count(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    /// 全量重放（按写入顺序）；用于重启恢复 / 审计查询。
    pub fn replay(&self) -> Result<Vec<UserHookEvent>, HookError> {
        let conn = self.conn.lock().expect("audit sink");
        let mut stmt = conn
            .prepare(
                "SELECT schema_version, event_id, timestamp_ms, hook_id, trigger, scope,
                        capability, lifecycle, payload
                 FROM user_hook_events ORDER BY rowid",
            )
            .map_err(|e| HookError::executor(GENERIC_HOOK_ID, format!("prepare replay: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                let scope_json: String = row.get(5)?;
                let payload_json: String = row.get(8)?;
                let scope: HookScope = serde_json::from_str(&scope_json).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                let payload: UserHookEventPayload =
                    serde_json::from_str(&payload_json).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            8,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;
                let trigger_json: String = row.get(4)?;
                Ok(UserHookEvent {
                    schema_version: row.get(0)?,
                    event_id: EventId::new(row.get::<_, String>(1)?),
                    timestamp: agent_domain::Timestamp::from_unix_millis(row.get(2)?),
                    hook_id: row.get(3)?,
                    trigger: serde_json::from_str::<TriggerPoint>(&trigger_json).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    scope,
                    capability: row.get(6)?,
                    lifecycle: row.get(7)?,
                    payload,
                })
            })
            .map_err(|e| HookError::executor(GENERIC_HOOK_ID, format!("query replay: {e}")))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(
                row.map_err(|e| HookError::executor(GENERIC_HOOK_ID, format!("replay row: {e}")))?,
            );
        }
        Ok(events)
    }
}

#[async_trait]
impl AuditSink for SqliteHookAuditSink {
    async fn record(&self, event: UserHookEvent) {
        let conn = self.conn.lock().expect("audit sink");
        let scope_json = match serde_json::to_string(&event.scope) {
            Ok(v) => v,
            Err(error) => {
                self.failures.fetch_add(1, Ordering::Relaxed);
                tracing::error!(
                    hook_id = %event.hook_id,
                    event_id = %event.event_id,
                    "user hook audit serialization failed: {error}"
                );
                return;
            }
        };
        let payload_json = match serde_json::to_string(&event.payload) {
            Ok(v) => v,
            Err(error) => {
                self.failures.fetch_add(1, Ordering::Relaxed);
                tracing::error!(
                    hook_id = %event.hook_id,
                    event_id = %event.event_id,
                    "user hook audit payload serialization failed: {error}"
                );
                return;
            }
        };
        if let Err(error) = conn.execute(
            "INSERT OR IGNORE INTO user_hook_events
                (event_id, schema_version, timestamp_ms, hook_id, trigger, scope,
                 capability, lifecycle, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                event.event_id.as_str(),
                event.schema_version,
                event.timestamp.as_unix_millis(),
                event.hook_id,
                serde_json::to_string(&event.trigger).unwrap_or_default(),
                scope_json,
                event.capability,
                event.lifecycle,
                payload_json,
            ],
        ) {
            // 写失败不静默：记录并计数（不 panic——审计失败不应拖垮 run loop）。
            self.failures.fetch_add(1, Ordering::Relaxed);
            tracing::error!(
                hook_id = %event.hook_id,
                event_id = %event.event_id,
                "user hook audit write failed: {error}"
            );
        }
    }
}

/// `AsyncRunner` 生产实现：tokio::spawn（async fire-and-forget 不阻塞 run loop）。
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioAsyncRunner;

impl AsyncRunner for TokioAsyncRunner {
    fn spawn(&self, future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>) {
        tokio::spawn(future);
    }
}

// =========================================================================
// 宿主装配
// =========================================================================

/// [`UserHookHost`] 装配选项。`new` 提供默认值；字段可覆盖后再 `build`。
pub struct UserHookHostOptions {
    pub workspace_roots: Vec<PathBuf>,
    pub approval_mode: ApprovalMode,
    pub http: HttpClientConfig,
    pub providers: Arc<dyn ProviderResolver>,
    pub default_eval: EvalProfile,
    pub profiles: Arc<dyn EvalProfileResolver>,
    pub secret_backend: Arc<dyn SecretBackend>,
    pub audit_sink: Arc<dyn AuditSink>,
    pub mcp_registry: Arc<tokio::sync::Mutex<ToolRegistry>>,
    pub run_context: Arc<dyn HookRunContext>,
    /// 测试 / 显式嵌入场景的无 workspace trust。生产默认 false，未知
    /// workspace 始终 fail-closed。
    pub trusted: bool,
    /// 生产 workspace trust 快照；按真实 WorkspaceService 状态注入。
    pub workspace_trust: Option<Arc<dyn HookWorkspaceTrustResolver>>,
    pub command_max_output_bytes: u64,
}

impl UserHookHostOptions {
    pub fn new(
        workspace_roots: Vec<PathBuf>,
        providers: Arc<dyn ProviderResolver>,
        default_eval: EvalProfile,
        profiles: Arc<dyn EvalProfileResolver>,
        secret_backend: Arc<dyn SecretBackend>,
    ) -> Self {
        Self {
            workspace_roots,
            approval_mode: ApprovalMode::ReadOnly,
            http: HttpClientConfig::default(),
            providers,
            default_eval,
            profiles,
            secret_backend,
            audit_sink: Arc::new(
                SqliteHookAuditSink::in_memory().expect("in-memory audit sink must construct"),
            ),
            mcp_registry: Arc::new(tokio::sync::Mutex::new(ToolRegistry::new())),
            run_context: Arc::new(StaticHookRunContext::new("hook-default", "hook-default")),
            trusted: false,
            workspace_trust: None,
            command_max_output_bytes: 1024 * 1024,
        }
    }

    pub fn build(self) -> Result<UserHookHost, HookError> {
        let selector = SandboxSelector::new();
        let (backend, _selection) = selector.pick();
        let command = SandboxCommandExecutor::new(Arc::from(backend), self.workspace_roots.clone())
            .with_max_output_bytes(self.command_max_output_bytes);
        let http = HttpHookExecutor::new(self.http)?;
        let policy = if let Some(workspace_trust) = self.workspace_trust {
            HookPolicyGate::with_trust_resolver(self.approval_mode, workspace_trust)
        } else {
            HookPolicyGate::new(self.approval_mode, self.trusted)
        };
        let judge = CanonicalJudge::new(
            Arc::clone(&self.providers),
            self.default_eval.clone(),
            Arc::clone(&self.profiles),
        );
        let mcp = McpToolInvokerHost::new(
            Arc::clone(&self.mcp_registry),
            Arc::clone(&self.run_context),
        );
        let secret = BackendSecretResolver::new(Arc::clone(&self.secret_backend));
        let exec = Executors::builder()
            .policy(Arc::new(policy))
            .command(Arc::new(command))
            .http(Arc::new(http))
            .judge(Arc::new(judge))
            .mcp(Arc::new(mcp.clone()))
            .audit(Arc::clone(&self.audit_sink))
            .secret(Arc::new(secret))
            .async_runner(Arc::new(TokioAsyncRunner))
            .clock(Arc::new(SystemHookClock::default()))
            .build();
        Ok(UserHookHost {
            dispatcher: HookDispatcher::new(),
            exec,
            mcp,
            task_kinds: Mutex::new(BTreeMap::new()),
        })
    }
}

/// User Hooks 生产宿主：装配 [`Executors`] 与 [`HookDispatcher`]，提供
/// AgentEvent → 触发点的 canonical 映射与事件桥。
///
/// 事件桥只订阅既有 run loop 的 `EventBroadcaster`，**不启动第二 run loop**；
/// 同步 handler 在事件抵达时 await，async handler 经 [`TokioAsyncRunner`]
/// fire-and-forget。
pub struct UserHookHost {
    dispatcher: HookDispatcher,
    exec: Executors,
    /// 具体 McpTool 执行器（与 `exec.mcp` 共享同一 registry），提供生产
    /// MCP 注册入口。
    mcp: McpToolInvokerHost,
    /// 已观测到的后台任务种类（TaskStarted 时记录，TaskFinished 时用于区分
    /// SubagentStop 与 TaskCompleted）。事件桥与 run loop 共用同一宿主实例，
    /// 天然串行：先 Started 后 Finished，顺序由事件流保证。
    task_kinds: Mutex<BTreeMap<String, agent_domain::TaskKind>>,
}

impl std::fmt::Debug for UserHookHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserHookHost").finish_non_exhaustive()
    }
}

impl UserHookHost {
    pub fn new(options: UserHookHostOptions) -> Result<Self, HookError> {
        options.build()
    }

    pub fn dispatcher(&self) -> &HookDispatcher {
        &self.dispatcher
    }

    pub fn executors(&self) -> &Executors {
        &self.exec
    }

    /// 生产 MCP 注册入口：把 MCP server 的发现工具注册进共享 registry
    /// （复用 mcp-client 的 discovery + `McpToolAdapter`，P9-5 每 server
    /// 审批与输出限制由 adapter 承担）。P17-1 的 `McpTool` handler 从该
    /// registry 按 `server_id/tool_name` 命名空间调用，handler 不获额外特权。
    pub async fn register_mcp_server(
        &self,
        server: &str,
        peer: Arc<dyn mcp_client::McpPeer>,
        permissions: McpPermissions,
        trusted: bool,
        approval: Arc<dyn McpApproval>,
    ) -> Result<Vec<tool_api::ToolDescriptor>, McpError> {
        self.mcp
            .register_mcp_server(server, peer, permissions, trusted, approval)
            .await
    }

    pub fn register(&mut self, config: HookConfig) -> Result<(), HookError> {
        self.dispatcher.register_config(config)
    }

    pub fn register_configs<I>(&mut self, configs: I) -> Result<(), HookError>
    where
        I: IntoIterator<Item = HookConfig>,
    {
        for config in configs {
            self.dispatcher.register_config(config)?;
        }
        Ok(())
    }

    /// 派发一次触发（显式入口；事件桥与宿主调用方共用）。
    pub async fn dispatch(
        &self,
        trigger: TriggerPoint,
        payload: &TriggerPayload,
        workspace: Option<&WorkspaceId>,
    ) -> user_hooks::DispatchOutcome {
        self.dispatcher
            .dispatch(trigger, payload, workspace, &self.exec)
            .await
    }

    /// canonical `AgentEvent` → 触发点映射（仅订阅，不改写事件源）。
    ///
    /// 覆盖 Run / Prompt / Tool / Permission / Compact / Task / Subagent 族；
    /// `SessionStart` / `SessionEnd` / `Notification` 在 agent-events 中暂无
    /// 对应事件，由调用方在拥有该上下文时显式 dispatch。
    pub fn trigger_point_for(event: &AgentEvent) -> Option<(TriggerPoint, TriggerPayload)> {
        let mut payload = TriggerPayload::builder();
        match event {
            AgentEvent::RunStarted { trigger_message_id } => {
                payload =
                    payload.details(json!({ "trigger_message_id": trigger_message_id.as_str() }));
                Some((TriggerPoint::RunStarted, payload.build()))
            }
            AgentEvent::ContextPrepared {
                message_count,
                estimated_input_tokens,
            } => {
                payload = payload.details(json!({
                    "message_count": message_count,
                    "estimated_input_tokens": estimated_input_tokens,
                }));
                Some((TriggerPoint::PromptAssembled, payload.build()))
            }
            AgentEvent::ToolCallStarted { tool_call_id, name } => {
                payload = payload
                    .tool_call_id(tool_call_id.clone())
                    .details(json!({ "tool_name": name }));
                Some((TriggerPoint::PreToolUse, payload.build()))
            }
            AgentEvent::ToolApprovalRequested {
                tool_call_id,
                reason,
            } => {
                payload = payload
                    .tool_call_id(tool_call_id.clone())
                    .details(json!({ "reason": reason }));
                Some((TriggerPoint::PermissionRequest, payload.build()))
            }
            AgentEvent::ToolExecutionCompleted {
                tool_call_id,
                result,
            } => {
                payload = payload.tool_call_id(tool_call_id.clone());
                if result.is_error {
                    Some((TriggerPoint::ToolFailed, payload.build()))
                } else {
                    Some((TriggerPoint::PostToolUse, payload.build()))
                }
            }
            AgentEvent::CompactionStarted { source_event_count } => {
                payload = payload.details(json!({ "source_event_count": source_event_count }));
                Some((TriggerPoint::PreCompact, payload.build()))
            }
            AgentEvent::CompactionCompleted {
                summary_message_id,
                compacted_through,
            } => {
                payload = payload.details(json!({
                    "summary_message_id": summary_message_id.as_str(),
                    "compacted_through": compacted_through.0,
                }));
                Some((TriggerPoint::PostCompact, payload.build()))
            }
            AgentEvent::RunCompleted { stop_reason, .. } => {
                payload = payload.details(json!({ "stop_reason": format!("{stop_reason:?}") }));
                Some((TriggerPoint::RunCompleted, payload.build()))
            }
            AgentEvent::RunFailed { error } => {
                payload = payload.details(json!({
                    "category": format!("{:?}", error.category),
                    "message": error.message,
                }));
                Some((TriggerPoint::RunFailed, payload.build()))
            }
            AgentEvent::Task(event) => match event {
                agent_domain::TaskEvent::Started {
                    task_id, task_kind, ..
                } => {
                    payload = payload.details(json!({
                        "task_id": task_id.as_str(),
                        "task_kind": format!("{task_kind:?}"),
                    }));
                    match task_kind {
                        agent_domain::TaskKind::Agent => {
                            Some((TriggerPoint::SubagentStart, payload.build()))
                        }
                        _ => Some((TriggerPoint::TaskStarted, payload.build())),
                    }
                }
                agent_domain::TaskEvent::Finished {
                    task_id,
                    status,
                    detail,
                } => {
                    payload = payload.details(json!({
                        "task_id": task_id.as_str(),
                        "status": format!("{status:?}"),
                        "detail": detail,
                    }));
                    Some((TriggerPoint::TaskCompleted, payload.build()))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// 实例级 canonical `AgentEvent` → 触发点映射：在静态映射之上维护
    /// 后台任务种类，使 `TaskKind::Agent` 的 `Finished` 收敛为
    /// [`TriggerPoint::SubagentStop`]（`Finished` 事件自身不携带 task_kind，
    /// 种类由先前同 task 的 `Started` 事件记录）。
    fn trigger_point_for_event(
        &self,
        event: &AgentEvent,
    ) -> Option<(TriggerPoint, TriggerPayload)> {
        if let AgentEvent::Task(agent_domain::TaskEvent::Started {
            task_id, task_kind, ..
        }) = event
        {
            self.task_kinds
                .lock()
                .expect("task kinds")
                .insert(task_id.as_str().to_string(), *task_kind);
        }
        if let AgentEvent::Task(agent_domain::TaskEvent::Finished { task_id, .. }) = event {
            let is_agent = self
                .task_kinds
                .lock()
                .expect("task kinds")
                .get(task_id.as_str())
                .copied()
                == Some(agent_domain::TaskKind::Agent);
            let (trigger, payload) = Self::trigger_point_for(event)?;
            let trigger = if is_agent {
                TriggerPoint::SubagentStop
            } else {
                trigger
            };
            return Some((trigger, payload));
        }
        Self::trigger_point_for(event)
    }

    /// 权威 pre-prompt 位点（P17-1）：run loop 在每轮请求发送给 Provider 之前
    /// 调用（`LoopContext::pre_prompt`）。派发 `PromptAssembled`，payload 携带
    /// **真实 prompt**（请求 messages 的完整文本）；Eval/McpTool 判定拒绝 →
    /// 返回 `Err`（run 走既有 Failed 收敛路径）；PromptTransform 改写回灌进
    /// `request.messages`。这是唯一权威回灌位点，事件桥已跳过该触发点。
    pub async fn pre_prompt(
        &self,
        request: &mut CanonicalModelRequest,
        workspace_id: Option<&WorkspaceId>,
        workspace_roots: &[PathBuf],
        session_id: &SessionId,
        run_id: &RunId,
    ) -> Result<(), HookError> {
        let prompt_text = content_text(&request.messages);
        let system_text = request
            .messages
            .iter()
            .find(|message| message.role == MessageRole::System)
            .map(|message| content_text(std::slice::from_ref(message)))
            .unwrap_or_default();
        let user_text = request
            .messages
            .iter()
            .rfind(|message| message.role == MessageRole::User)
            .map(|message| content_text(std::slice::from_ref(message)))
            .unwrap_or_default();
        let mut outcome = user_hooks::DispatchOutcome::default();
        for workspace in hook_workspaces(workspace_id, workspace_roots) {
            let mut payload = TriggerPayload::builder()
                .session_id(session_id.clone())
                .run_id(run_id.clone())
                .prompt(prompt_text.clone())
                .system_prompt(system_text.clone())
                .user_prompt(user_text.clone())
                .injected_prompt("");
            if let Some(id) = workspace {
                payload = payload.workspace_id(id.clone());
            }
            let payload = payload.build();
            outcome.merge(
                self.dispatch(TriggerPoint::PromptAssembled, &payload, workspace)
                    .await,
            );
            if outcome.is_denied() {
                return Err(HookError::executor(
                    GENERIC_HOOK_ID,
                    "PromptEval/AgentEval/McpTool denied the run",
                ));
            }
        }
        if !outcome
            .effects
            .iter()
            .any(|(_, effect)| matches!(effect, user_hooks::HookEffect::Transform { .. }))
        {
            return Ok(());
        }
        let mut candidate = request.clone();
        apply_prompt_transforms(&mut candidate, &outcome)?;
        validate_prompt_post_transform(request, &candidate, &outcome)?;
        *request = candidate;
        Ok(())
    }

    /// 权威 pre-tool 位点（P17-1）：审批通过后、本地工具执行之前调用
    /// （`LoopContext::pre_tool`）。对每个调用派发 `PreToolUse`；被 Eval/McpTool
    /// 拒绝的调用从执行列表移除（run loop 按审批拒绝语义回填 denied 结果）。
    /// 事件桥已跳过该触发点，不重复派发。
    pub async fn pre_tool(
        &self,
        invocations: &mut Vec<PendingToolInvocation>,
        workspace_id: Option<&WorkspaceId>,
        workspace_roots: &[PathBuf],
        session_id: &SessionId,
        run_id: &RunId,
    ) -> Result<(), HookError> {
        let mut denied = BTreeSet::new();
        for invocation in invocations.iter() {
            for workspace in hook_workspaces(workspace_id, workspace_roots) {
                let mut payload = TriggerPayload::builder()
                    .session_id(session_id.clone())
                    .run_id(run_id.clone())
                    .tool_call_id(invocation.tool_call_id.clone())
                    .details(json!({ "tool_name": invocation.name }));
                if let Some(id) = workspace {
                    payload = payload.workspace_id(id.clone());
                }
                let payload = payload.build();
                let outcome = self
                    .dispatch(TriggerPoint::PreToolUse, &payload, workspace)
                    .await;
                if outcome.is_denied() {
                    denied.insert(invocation.tool_call_id.as_str().to_string());
                }
            }
        }
        invocations.retain(|invocation| !denied.contains(invocation.tool_call_id.as_str()));
        Ok(())
    }

    /// 处理一条 canonical AgentEvent（带可选的 workspace 上下文）。
    /// 调用方（run loop / 事件桥）在事件抵达时同步调用。
    pub async fn on_agent_event(
        &self,
        envelope: &AgentEventEnvelope,
        workspace: Option<&WorkspaceId>,
    ) -> user_hooks::DispatchOutcome {
        let Some((trigger, mut payload)) = self.trigger_point_for_event(&envelope.payload) else {
            return user_hooks::DispatchOutcome::default();
        };
        payload.session_id = Some(envelope.session_id.clone());
        payload.run_id = Some(envelope.run_id.clone());
        self.dispatch(trigger, &payload, workspace).await
    }

    /// 订阅既有 run loop 的 `EventBroadcaster`，事件驱动派发（无轮询、
    /// 无第二 run loop）。返回的任务在 hub 关闭时自然结束。
    pub fn spawn_event_bridge(
        self: &Arc<Self>,
        broadcaster: EventBroadcaster,
    ) -> tokio::task::JoinHandle<()> {
        let mut subscriber = broadcaster.subscribe();
        let host = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                match subscriber.recv().await {
                    Ok(envelope) => {
                        // PromptAssembled / PreToolUse 由 run loop 的权威位点
                        // （LoopContext::pre_prompt / pre_tool）回灌，事件桥
                        // 跳过这两点，避免同一触发被派发两次。
                        if let Some((trigger, _)) = Self::trigger_point_for(&envelope.payload) {
                            if matches!(
                                trigger,
                                TriggerPoint::PromptAssembled | TriggerPoint::PreToolUse
                            ) {
                                continue;
                            }
                        }
                        host.on_agent_event(&envelope, None).await;
                    }
                    Err(agent_engine::BroadcastError::Closed)
                    | Err(agent_engine::BroadcastError::NoSubscribers) => break,
                    Err(agent_engine::BroadcastError::Lagged { .. }) => continue,
                }
            }
        })
    }
}

/// 请求 messages 的完整文本（“真实 prompt”）：按顺序拼接全部文本内容，
/// 作为 `PromptAssembled` 触发负载的 `prompt` 字段。
fn content_text(messages: &[Message]) -> String {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|part| match part {
            ContentPart::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 权威 pre-prompt / pre-tool 位点的派发目标 workspace 列表。
///
/// run 有归属 workspace（session 聚合）时只派发该 workspace：global 与该
/// workspace 作用域的 hook 命中，其他 workspace 的 hook 不触发（不跨
/// workspace 泄露 prompt / 工具调用）。run 无归属 workspace（旧构造点）时
/// 单次 global 派发（仅 global 作用域 hook 命中，不猜测 root 归属；
/// 与 [`RunRequest`] 的约定一致）。
fn hook_workspaces<'a>(
    workspace_id: Option<&'a WorkspaceId>,
    _workspace_roots: &[PathBuf],
) -> Vec<Option<&'a WorkspaceId>> {
    match workspace_id {
        Some(id) => vec![Some(id)],
        None => vec![None],
    }
}

/// 把 `PromptAssembled` 派发结果的 Transform 效果回灌进 `request.messages`：
/// - `System` → 替换首条 System 消息文本（无则前置插入）；
/// - `User` → 替换最后一条 User 消息文本；
/// - `Injected` → 追加为新 User 消息。
fn apply_prompt_transforms(
    request: &mut CanonicalModelRequest,
    outcome: &DispatchOutcome,
) -> Result<(), HookError> {
    let system_text = request
        .messages
        .iter()
        .find(|message| message.role == MessageRole::System)
        .map(|message| content_text(std::slice::from_ref(message)))
        .unwrap_or_default();
    let new_system = outcome.transformed_prompt("System", &system_text);
    if new_system != system_text {
        if let Some(system) = request
            .messages
            .iter_mut()
            .find(|message| message.role == MessageRole::System)
        {
            set_message_text(system, &new_system)?;
        } else {
            request.messages.insert(
                0,
                Message {
                    id: MessageId::from(format!("hook-system-{}", request.request_id)),
                    role: MessageRole::System,
                    content: vec![ContentPart::Text(TextContent { text: new_system })],
                    metadata: MessageMetadata::default(),
                },
            );
        }
    }
    if let Some(user_index) = request
        .messages
        .iter()
        .rposition(|message| message.role == MessageRole::User)
    {
        let user_text = content_text(std::slice::from_ref(&request.messages[user_index]));
        let new_user = outcome.transformed_prompt("User", &user_text);
        if new_user != user_text {
            set_message_text(&mut request.messages[user_index], &new_user)?;
        }
    }
    let injected = outcome.transformed_prompt("Injected", "");
    if !injected.is_empty() {
        request.messages.push(Message {
            id: MessageId::from(format!("hook-injected-{}", request.request_id)),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: injected })],
            metadata: MessageMetadata::default(),
        });
    }
    Ok(())
}

fn set_message_text(message: &mut Message, text: &str) -> Result<(), HookError> {
    if message
        .content
        .iter()
        .any(|part| !matches!(part, ContentPart::Text(_)))
    {
        return Err(HookError::executor(
            GENERIC_HOOK_ID,
            "prompt transform refused a target containing non-text canonical content",
        ));
    }
    message.content = vec![ContentPart::Text(TextContent {
        text: text.to_string(),
    })];
    Ok(())
}

/// Transform 后的最后一道 fail-closed 检查。dispatcher policy 只授权目标文本
/// 改写；这里验证所有既有 message 的 role / id / metadata 与非目标 content
/// 仍完全不变，禁止结果借回灌破坏 immutable system/security 上下文。失败时
/// caller 尚未替换原请求，因此天然回滚。
fn validate_prompt_post_transform(
    original: &CanonicalModelRequest,
    candidate: &CanonicalModelRequest,
    outcome: &DispatchOutcome,
) -> Result<(), HookError> {
    let has_target = |target: &str| {
        outcome.effects.iter().any(|(_, effect)| {
            matches!(effect, user_hooks::HookEffect::Transform { target: actual, .. } if actual == target)
        })
    };
    let mutable_system = has_target("System")
        .then(|| {
            original
                .messages
                .iter()
                .find(|message| message.role == MessageRole::System)
                .map(|message| message.id.clone())
        })
        .flatten();
    let mutable_user = has_target("User")
        .then(|| {
            original
                .messages
                .iter()
                .rfind(|message| message.role == MessageRole::User)
                .map(|message| message.id.clone())
        })
        .flatten();

    for before in &original.messages {
        let matches: Vec<&Message> = candidate
            .messages
            .iter()
            .filter(|after| after.id == before.id)
            .collect();
        let [after] = matches.as_slice() else {
            return Err(HookError::executor(
                GENERIC_HOOK_ID,
                "prompt transform changed canonical message identity",
            ));
        };
        if after.role != before.role || after.metadata != before.metadata {
            return Err(HookError::executor(
                GENERIC_HOOK_ID,
                "prompt transform changed immutable canonical message fields",
            ));
        }
        let content_may_change = mutable_system.as_ref() == Some(&before.id)
            || mutable_user.as_ref() == Some(&before.id);
        if !content_may_change && after.content != before.content {
            return Err(HookError::executor(
                GENERIC_HOOK_ID,
                "prompt transform changed immutable system/security context",
            ));
        }
    }

    for extra in candidate
        .messages
        .iter()
        .filter(|message| !original.messages.iter().any(|old| old.id == message.id))
    {
        let valid_system = has_target("System")
            && original
                .messages
                .iter()
                .all(|message| message.role != MessageRole::System)
            && extra.id == MessageId::from(format!("hook-system-{}", original.request_id))
            && extra.role == MessageRole::System;
        let valid_injected = has_target("Injected")
            && extra.id == MessageId::from(format!("hook-injected-{}", original.request_id))
            && extra.role == MessageRole::User;
        if !valid_system && !valid_injected {
            return Err(HookError::executor(
                GENERIC_HOOK_ID,
                "prompt transform injected an unauthorized canonical message",
            ));
        }
    }
    Ok(())
}

/// 从 resource-loader 的中性 DTO 转换为 user-hooks `HookConfig`。
///
/// 依赖方向：`resource-loader` 不依赖 `user-hooks`（见 workspace-layout），
/// 转换发生在消费侧（正式宿主 / app-service 装配）。字段与 DTO 的
/// serde 序列化形式一一对应（trigger / scope / lifecycle 为 tagged JSON 或
/// 字符串，handler 为 handler 配置的完整 JSON）。
pub fn hook_config_from_resource(
    id: String,
    trigger: serde_json::Value,
    scope: serde_json::Value,
    enabled: bool,
    lifecycle: Option<String>,
    handler: serde_json::Value,
) -> Result<HookConfig, HookError> {
    let trigger: TriggerPoint = serde_json::from_value(trigger)
        .map_err(|error| HookError::executor(&id, format!("invalid trigger: {error}")))?;
    let scope: HookScope = serde_json::from_value(scope)
        .map_err(|error| HookError::executor(&id, format!("invalid scope: {error}")))?;
    let lifecycle: Option<HandlerLifecycle> = lifecycle
        .map(|lifecycle| {
            serde_json::from_value(serde_json::Value::String(lifecycle))
                .map_err(|error| HookError::executor(&id, format!("invalid lifecycle: {error}")))
        })
        .transpose()?;
    let handler: HandlerConfig = serde_json::from_value(handler)
        .map_err(|error| HookError::executor(&id, format!("invalid handler: {error}")))?;
    Ok(HookConfig {
        id,
        trigger,
        scope,
        lifecycle,
        enabled,
        handler,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppService;
    use user_hooks::config::PromptTarget;

    #[test]
    fn parse_hook_decision_conventions() {
        assert_eq!(parse_hook_decision("allow"), JudgeDecision::Allow);
        assert_eq!(parse_hook_decision("unrelated text"), JudgeDecision::Allow);
        assert_eq!(
            parse_hook_decision("deny: too risky"),
            JudgeDecision::Deny {
                reason: "too risky".into()
            }
        );
        assert_eq!(
            parse_hook_decision("transform: safer prompt"),
            JudgeDecision::Transform {
                new_prompt: "safer prompt".into()
            }
        );
    }

    #[tokio::test]
    async fn policy_gate_blocks_system_transform_without_override() {
        let gate = HookPolicyGate::new(ApprovalMode::NeverAsk, true);
        let action = PolicyAction {
            capability: HookCapability::PromptTransform,
            hook_id: "h",
            workspace_id: None,
            description: json!({}),
            prompt_target: Some(PromptTarget::System),
            allow_system_override: false,
        };
        let outcome = gate.evaluate(action).await;
        assert!(!outcome.is_allowed());
    }

    #[tokio::test]
    async fn policy_gate_allows_user_transform_and_denies_ask_user_for_process() {
        let gate = HookPolicyGate::new(ApprovalMode::AlwaysAsk, true);
        let action = PolicyAction {
            capability: HookCapability::PromptTransform,
            hook_id: "h",
            workspace_id: None,
            description: json!({}),
            prompt_target: Some(PromptTarget::User),
            allow_system_override: false,
        };
        assert!(gate.evaluate(action).await.is_allowed());

        let process = PolicyAction {
            capability: HookCapability::Process,
            hook_id: "h",
            workspace_id: None,
            description: json!({ "program": "echo" }),
            prompt_target: None,
            allow_system_override: false,
        };
        // AlwaysAsk 下 hook 无交互审批 → fail-closed 拒绝。
        assert!(!gate.evaluate(process).await.is_allowed());
    }

    #[tokio::test]
    async fn policy_gate_unknown_workspace_is_untrusted_and_fail_closed() {
        let trusted_id = WorkspaceId::from("trusted");
        let unknown_id = WorkspaceId::from("unknown");
        let gate = HookPolicyGate::with_workspace_trust(
            ApprovalMode::NeverAsk,
            BTreeMap::from([(trusted_id.clone(), true)]),
        );
        let action = |workspace_id| PolicyAction {
            capability: HookCapability::PromptTransform,
            hook_id: "h",
            workspace_id,
            description: json!({}),
            prompt_target: Some(PromptTarget::User),
            allow_system_override: false,
        };
        assert!(gate.evaluate(action(Some(&trusted_id))).await.is_allowed());
        assert!(!gate.evaluate(action(Some(&unknown_id))).await.is_allowed());
        assert!(!gate.evaluate(action(None)).await.is_allowed());
        assert!(gate.allows_eval_fail_open(Some(&trusted_id)));
        assert!(!gate.allows_eval_fail_open(Some(&unknown_id)));
        assert!(!gate.allows_eval_fail_open(None));
    }

    #[test]
    fn user_hook_host_defaults_are_untrusted_and_read_only() {
        let default_eval = EvalProfile {
            provider_id: ProviderId::from("default"),
            model: ModelId::from("default"),
            system_prompt: None,
            reasoning_effort: None,
            budget: None,
            tool_rules: agent_domain::ProfileToolRules::default(),
            isolation: agent_domain::ProfileIsolation::None,
        };
        let options = UserHookHostOptions::new(
            Vec::new(),
            Arc::new(TestProviders(Arc::new(AppService::new("safe-defaults")))),
            default_eval.clone(),
            Arc::new(TestProfiles(default_eval)),
            Arc::new(auth_service::MemoryBackend::default()),
        );
        assert_eq!(options.approval_mode, ApprovalMode::ReadOnly);
        assert!(!options.trusted);
        assert!(options.workspace_trust.is_none());
    }

    #[test]
    fn restricted_profile_lookup_never_falls_back_to_default() {
        let default_eval = EvalProfile {
            provider_id: ProviderId::from("default-provider"),
            model: ModelId::from("default-model"),
            system_prompt: None,
            reasoning_effort: None,
            budget: None,
            tool_rules: agent_domain::ProfileToolRules::default(),
            isolation: agent_domain::ProfileIsolation::None,
        };
        let judge = CanonicalJudge::new(
            Arc::new(TestProviders(Arc::new(AppService::new(
                "profile-fail-closed",
            )))),
            default_eval,
            Arc::new(TestProfiles(EvalProfile {
                provider_id: ProviderId::from("known-provider"),
                model: ModelId::from("known-model"),
                system_prompt: Some("restricted".into()),
                reasoning_effort: Some(agent_domain::ReasoningEffort::Low),
                budget: None,
                tool_rules: agent_domain::ProfileToolRules::default(),
                isolation: agent_domain::ProfileIsolation::Restricted,
            })),
        );
        let request = JudgeRequest {
            mode: JudgeMode::ConstrainedAgent,
            workspace_id: Some(WorkspaceId::from("workspace")),
            prompt: "judge".into(),
            response_schema: None,
            restricted_profile: Some("unknown-profile".into()),
            tool_allowlist: Vec::new(),
            budget: Some(user_hooks::config::BudgetLimit {
                max_tokens: Some(32),
                timeout_ms: Some(100),
            }),
        };
        assert!(matches!(
            judge.profile_for(&request),
            Err(HookError::PolicyDenied { .. })
        ));
    }

    #[test]
    fn constrained_eval_enforces_profile_isolation_tool_allowlist_and_tightest_budget() {
        let profile = EvalProfile {
            provider_id: ProviderId::from("restricted-provider"),
            model: ModelId::from("restricted-model"),
            system_prompt: Some("independent restricted prompt".into()),
            reasoning_effort: Some(agent_domain::ReasoningEffort::Low),
            budget: Some(agent_engine::BudgetLimits {
                max_iterations: Some(2),
                max_tool_calls: Some(1),
                max_duration_ms: Some(80),
                max_input_tokens: Some(40),
                max_output_tokens: Some(30),
                ..Default::default()
            }),
            tool_rules: agent_domain::ProfileToolRules {
                allowed: vec!["read_file".into(), "shell".into()],
                denied: vec!["shell".into()],
            },
            isolation: agent_domain::ProfileIsolation::Restricted,
        };
        let request = JudgeRequest {
            mode: JudgeMode::ConstrainedAgent,
            workspace_id: Some(WorkspaceId::from("workspace")),
            prompt: "judge".into(),
            response_schema: None,
            restricted_profile: Some("restricted".into()),
            tool_allowlist: vec!["read_file".into()],
            budget: Some(user_hooks::config::BudgetLimit {
                max_tokens: Some(100),
                timeout_ms: Some(200),
            }),
        };
        let (tools, budget) = constrained_eval_limits(&profile, &request).expect("valid limits");
        assert_eq!(tools, vec!["read_file"]);
        assert_eq!(budget.max_iterations, Some(2));
        assert_eq!(budget.max_tool_calls, Some(1));
        assert_eq!(budget.max_input_tokens, Some(40));
        assert_eq!(budget.max_output_tokens, Some(30));
        assert_eq!(budget.max_duration_ms, Some(80));

        for forbidden in ["shell", "write_file"] {
            let mut denied = request.clone();
            denied.tool_allowlist = vec![forbidden.into()];
            assert!(matches!(
                constrained_eval_limits(&profile, &denied),
                Err(HookError::PolicyDenied { .. })
            ));
        }
        let mut unisolated = profile;
        unisolated.isolation = agent_domain::ProfileIsolation::None;
        assert!(matches!(
            constrained_eval_limits(&unisolated, &request),
            Err(HookError::PolicyDenied { .. })
        ));
    }

    // —— 宿主装配 helper（真实 adapter 链：Sandbox/Http/CanonicalJudge/MCP）——

    struct TestProviders(Arc<AppService>);
    impl ProviderResolver for TestProviders {
        fn resolve(&self, id: &ProviderId) -> Option<Arc<dyn ModelProvider>> {
            if id.as_str() == "default" {
                self.0.first_provider()
            } else {
                self.0.provider(id)
            }
        }
    }

    #[derive(Clone)]
    struct TestProfiles(EvalProfile);
    impl EvalProfileResolver for TestProfiles {
        fn resolve(
            &self,
            _workspace_id: Option<&WorkspaceId>,
            profile: &str,
        ) -> Option<EvalProfile> {
            if profile.is_empty() || profile == "default" {
                Some(self.0.clone())
            } else {
                None
            }
        }
    }

    fn test_host(service: &Arc<AppService>) -> UserHookHost {
        let default_eval = EvalProfile {
            provider_id: ProviderId::from("mock"),
            model: ModelId::from("mock-model"),
            system_prompt: None,
            reasoning_effort: None,
            budget: None,
            tool_rules: agent_domain::ProfileToolRules::default(),
            isolation: agent_domain::ProfileIsolation::None,
        };
        let mut options = UserHookHostOptions::new(
            Vec::new(),
            Arc::new(TestProviders(Arc::clone(service))),
            default_eval.clone(),
            Arc::new(TestProfiles(default_eval)),
            Arc::new(auth_service::MemoryBackend::default()),
        );
        // 测试显式选择 trusted + NeverAsk；生产构造默认仍是 untrusted + ReadOnly。
        options.trusted = true;
        options.approval_mode = ApprovalMode::NeverAsk;
        UserHookHost::new(options).expect("host must construct")
    }

    fn request_with_texts(system: &str, user: &str) -> CanonicalModelRequest {
        CanonicalModelRequest {
            request_id: RequestId::from("hook-test-request"),
            model: ModelId::from("mock-model"),
            messages: vec![
                Message {
                    id: MessageId::from("hook-sys"),
                    role: MessageRole::System,
                    content: vec![ContentPart::Text(TextContent {
                        text: system.to_string(),
                    })],
                    metadata: MessageMetadata::default(),
                },
                Message {
                    id: MessageId::from("hook-user"),
                    role: MessageRole::User,
                    content: vec![ContentPart::Text(TextContent {
                        text: user.to_string(),
                    })],
                    metadata: MessageMetadata::default(),
                },
            ],
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: provider_api::ToolChoice::default(),
            thinking: None,
            reasoning: None,
            temperature: None,
            max_output_tokens: None,
            stop_sequences: Vec::new(),
            response_format: ResponseFormat::Text,
            prompt_cache: provider_api::PromptCachePreference::default(),
            budget: RequestBudget::default(),
            provider_options: BTreeMap::new(),
            trace_id: None,
        }
    }

    // —— 回归：pre_prompt 真实 payload + PromptTransform 回灌 ——

    #[tokio::test]
    async fn pre_prompt_carries_real_prompt_and_feeds_transform_back_into_request() {
        let service = Arc::new(AppService::new("pre-prompt-transform-test"));
        let mut host = test_host(&service);
        host.register(HookConfig {
            id: "pt-inject".into(),
            trigger: TriggerPoint::PromptAssembled,
            scope: HookScope::Global,
            lifecycle: None,
            enabled: true,
            handler: HandlerConfig::PromptTransform(user_hooks::config::PromptTransformHandler {
                target: user_hooks::config::PromptTarget::Injected,
                rewrite_kind: "prefix".into(),
                template: "HOOK-REVIEW:".into(),
                allow_system_override: false,
            }),
        })
        .expect("register transform hook");

        let mut request = request_with_texts("system base prompt", "please help me write code");
        host.pre_prompt(
            &mut request,
            None,
            &[],
            &SessionId::from("s1"),
            &RunId::from("r1"),
        )
        .await
        .expect("pre_prompt must succeed");

        // 回灌：Injected 只基于自身（空）原文，不把完整 prompt 双写进目标。
        let injected = request.messages.last().expect("injected message appended");
        assert_eq!(injected.role, MessageRole::User);
        let text = content_text(std::slice::from_ref(injected));
        assert_eq!(text, "HOOK-REVIEW:");
        assert!(!text.contains("please help me write code"));
        assert!(!text.contains("system base prompt"));
        // 原 system / user 消息不被替换（Injected 是追加语义）。
        assert_eq!(request.messages.len(), 3);
    }

    #[tokio::test]
    async fn pre_prompt_system_user_and_injected_transforms_use_target_originals() {
        let service = Arc::new(AppService::new("pre-prompt-three-targets"));
        let mut host = test_host(&service);
        for (id, target, kind, template, allow_system_override) in [
            (
                "01-system",
                PromptTarget::System,
                "suffix",
                "SYS-HOOK",
                true,
            ),
            ("02-user", PromptTarget::User, "prefix", "USER-HOOK", false),
            (
                "03-injected",
                PromptTarget::Injected,
                "replace",
                "INJECTED-HOOK",
                false,
            ),
        ] {
            host.register(HookConfig {
                id: id.into(),
                trigger: TriggerPoint::PromptAssembled,
                scope: HookScope::Global,
                lifecycle: None,
                enabled: true,
                handler: HandlerConfig::PromptTransform(
                    user_hooks::config::PromptTransformHandler {
                        target,
                        rewrite_kind: kind.into(),
                        template: template.into(),
                        allow_system_override,
                    },
                ),
            })
            .expect("register transform");
        }

        let mut request = request_with_texts("system-original", "user-original");
        host.pre_prompt(
            &mut request,
            None,
            &[],
            &SessionId::from("s-targets"),
            &RunId::from("r-targets"),
        )
        .await
        .expect("three target transforms apply");
        assert_eq!(
            content_text(std::slice::from_ref(&request.messages[0])),
            "system-original\nSYS-HOOK"
        );
        assert_eq!(
            content_text(std::slice::from_ref(&request.messages[1])),
            "USER-HOOK\nuser-original"
        );
        assert_eq!(
            content_text(std::slice::from_ref(&request.messages[2])),
            "INJECTED-HOOK"
        );
    }

    #[tokio::test]
    async fn pre_prompt_rejects_transform_of_mixed_security_content_without_mutation() {
        let service = Arc::new(AppService::new("pre-prompt-security-rollback"));
        let mut host = test_host(&service);
        host.register(HookConfig {
            id: "system-transform".into(),
            trigger: TriggerPoint::PromptAssembled,
            scope: HookScope::Global,
            lifecycle: None,
            enabled: true,
            handler: HandlerConfig::PromptTransform(user_hooks::config::PromptTransformHandler {
                target: PromptTarget::System,
                rewrite_kind: "replace".into(),
                template: "changed".into(),
                allow_system_override: true,
            }),
        })
        .expect("register transform");
        let mut request = request_with_texts("immutable", "user");
        request.messages[0]
            .content
            .push(ContentPart::Thinking(agent_domain::ThinkingContent {
                text: "security context".into(),
                reasoning_item_id: None,
                redacted: true,
            }));
        let original = request.clone();
        assert!(host
            .pre_prompt(
                &mut request,
                None,
                &[],
                &SessionId::from("s-security"),
                &RunId::from("r-security"),
            )
            .await
            .is_err());
        assert_eq!(request, original, "failed post-validation must roll back");
    }

    #[tokio::test]
    async fn pre_prompt_eval_transform_is_reinjected_into_user_message() {
        let service = Arc::new(AppService::new("pre-prompt-eval-transform"));
        service.register_provider(Arc::new(test_support::MockProvider::new(
            test_support::MockScript::new()
                .text("transform: policy-approved user prompt")
                .complete(),
        )));
        let mut host = test_host(&service);
        host.register(HookConfig {
            id: "eval-transform".into(),
            trigger: TriggerPoint::PromptAssembled,
            scope: HookScope::Global,
            lifecycle: None,
            enabled: true,
            handler: HandlerConfig::PromptEval(user_hooks::config::PromptEvalHandler {
                prompt_template: "judge {trigger}".into(),
                response_schema: None,
                on_failure: user_hooks::config::EvalFallback::Deny,
            }),
        })
        .expect("register eval");
        let mut request = request_with_texts("immutable system", "unsafe user");
        host.pre_prompt(
            &mut request,
            None,
            &[],
            &SessionId::from("s-eval-transform"),
            &RunId::from("r-eval-transform"),
        )
        .await
        .expect("eval transform applies");
        assert_eq!(content_text(&request.messages[..1]), "immutable system");
        assert_eq!(
            content_text(&request.messages[1..]),
            "policy-approved user prompt"
        );
    }

    // —— 回归：pre_prompt 的 PromptEval 拒绝 → run 收敛为 Err ——

    #[tokio::test]
    async fn pre_prompt_eval_deny_returns_error() {
        let service = Arc::new(AppService::new("pre-prompt-eval-deny-test"));
        service.register_provider(Arc::new(test_support::MockProvider::new(
            test_support::MockScript::new()
                .text("deny: too risky")
                .complete(),
        )));
        let mut host = test_host(&service);
        host.register(HookConfig {
            id: "pe-deny".into(),
            trigger: TriggerPoint::PromptAssembled,
            scope: HookScope::Global,
            lifecycle: None,
            enabled: true,
            handler: HandlerConfig::PromptEval(user_hooks::config::PromptEvalHandler {
                prompt_template: "is this run safe? {trigger}".into(),
                response_schema: None,
                on_failure: user_hooks::config::EvalFallback::Allow,
            }),
        })
        .expect("register eval hook");

        let mut request = request_with_texts("sys", "user text");
        let err = host
            .pre_prompt(
                &mut request,
                None,
                &[],
                &SessionId::from("s1"),
                &RunId::from("r1"),
            )
            .await
            .expect_err("eval deny must fail the run");
        assert!(
            err.to_string().contains("denied the run"),
            "denial must surface as run error: {err}"
        );
    }

    // —— 回归：pre_tool 经注册的 MCP tool 拒绝 → 调用被移除 ——

    /// 最小 McpPeer：只广告一个 read-only 工具并返回固定文本。
    struct GatePeer {
        tool: rmcp::model::Tool,
        response: rmcp::model::CallToolResult,
    }

    impl GatePeer {
        fn denying() -> Self {
            let mut tool = rmcp::model::Tool::new(
                "gate-tool",
                "hook gate tool",
                serde_json::json!({"type": "object"})
                    .as_object()
                    .expect("schema object")
                    .clone(),
            );
            tool = tool.with_annotations(rmcp::model::ToolAnnotations::new().read_only(true));
            Self {
                tool,
                response: rmcp::model::CallToolResult::success(vec![
                    rmcp::model::ContentBlock::text("deny: gate blocked this call"),
                ]),
            }
        }
    }

    #[async_trait]
    impl mcp_client::McpPeer for GatePeer {
        async fn list_tools(&self) -> Result<Vec<rmcp::model::Tool>, mcp_client::McpError> {
            Ok(vec![self.tool.clone()])
        }
        async fn list_resources(&self) -> Result<Vec<rmcp::model::Resource>, mcp_client::McpError> {
            Ok(Vec::new())
        }
        async fn list_resource_templates(
            &self,
        ) -> Result<Vec<rmcp::model::ResourceTemplate>, mcp_client::McpError> {
            Ok(Vec::new())
        }
        async fn list_prompts(&self) -> Result<Vec<rmcp::model::Prompt>, mcp_client::McpError> {
            Ok(Vec::new())
        }
        async fn read_resource(
            &self,
            _params: rmcp::model::ReadResourceRequestParams,
        ) -> Result<rmcp::model::ReadResourceResult, mcp_client::McpError> {
            Err(mcp_client::McpError::Protocol(
                "not implemented in gate peer".into(),
            ))
        }
        async fn get_prompt(
            &self,
            _params: rmcp::model::GetPromptRequestParams,
        ) -> Result<rmcp::model::GetPromptResult, mcp_client::McpError> {
            Err(mcp_client::McpError::Protocol(
                "not implemented in gate peer".into(),
            ))
        }
        async fn call_tool(
            &self,
            _params: rmcp::model::CallToolRequestParams,
            _cancel: agent_domain::CancellationToken,
        ) -> Result<rmcp::model::CallToolResult, mcp_client::McpError> {
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn pre_tool_mcp_handler_registered_server_deny_removes_invocation() {
        let service = Arc::new(AppService::new("pre-tool-mcp-test"));
        let mut host = test_host(&service);
        // 真实注册路径：discovery → McpToolAdapter → 共享 ToolRegistry。
        let descriptors = host
            .register_mcp_server(
                "gate",
                Arc::new(GatePeer::denying()),
                mcp_client::config::McpPermissions::default(),
                true,
                Arc::new(HookMcpApproval),
            )
            .await
            .expect("mcp server registers");
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].name, "gate.gate-tool");

        host.register(HookConfig {
            id: "mcp-gate".into(),
            trigger: TriggerPoint::PreToolUse,
            scope: HookScope::Global,
            lifecycle: None,
            enabled: true,
            handler: HandlerConfig::McpTool(user_hooks::config::McpToolHandler {
                server_id: "gate".into(),
                tool_name: "gate-tool".into(),
                arg_template: Some(serde_json::json!({})),
                on_failure: user_hooks::McpFallback::default(),
            }),
        })
        .expect("register mcp hook");

        let mut invocations = vec![agent_engine::PendingToolInvocation {
            tool_call_id: ToolCallId::from("inv-1"),
            name: "some-tool".into(),
            arguments: serde_json::json!({}),
        }];
        host.pre_tool(
            &mut invocations,
            None,
            &[],
            &SessionId::from("s1"),
            &RunId::from("r1"),
        )
        .await
        .expect("pre_tool must succeed");
        assert!(
            invocations.is_empty(),
            "mcp deny must remove the invocation from the execution list"
        );
    }

    // —— 回归：SubagentStart / SubagentStop 映射 ——

    #[tokio::test]
    async fn subagent_start_and_stop_map_from_task_events() {
        let service = Arc::new(AppService::new("subagent-map-test"));
        let host = test_host(&service);

        let started = AgentEvent::Task(agent_domain::TaskEvent::Started {
            task_id: agent_domain::BackgroundTaskId::from("t1"),
            task_kind: agent_domain::TaskKind::Agent,
            parent_task_id: None,
        });
        let (trigger, payload) =
            UserHookHost::trigger_point_for(&started).expect("agent task start maps");
        assert_eq!(trigger, TriggerPoint::SubagentStart);
        let details = payload.details.expect("details present");
        assert_eq!(details["task_kind"], "Agent");

        // 实例级映射先观测 Started（记录任务种类），事件桥与 run loop 天然串行。
        let (trigger, _) = host
            .trigger_point_for_event(&started)
            .expect("instance maps start");
        assert_eq!(trigger, TriggerPoint::SubagentStart);

        // Finished 自身不携带 task_kind：实例级映射用先前 Started 记录的种类
        // 收敛为 SubagentStop。
        let finished = AgentEvent::Task(agent_domain::TaskEvent::Finished {
            task_id: agent_domain::BackgroundTaskId::from("t1"),
            status: agent_domain::TaskStatus::Completed,
            detail: None,
        });
        let (trigger, _) = host
            .trigger_point_for_event(&finished)
            .expect("agent task finish maps");
        assert_eq!(trigger, TriggerPoint::SubagentStop);

        // 非 Agent 任务 → TaskStarted / TaskCompleted（不误映射为 Subagent）。
        let proc_started = AgentEvent::Task(agent_domain::TaskEvent::Started {
            task_id: agent_domain::BackgroundTaskId::from("t2"),
            task_kind: agent_domain::TaskKind::Process,
            parent_task_id: None,
        });
        assert_eq!(
            UserHookHost::trigger_point_for(&proc_started).map(|(t, _)| t),
            Some(TriggerPoint::TaskStarted)
        );
        let _ = host
            .trigger_point_for_event(&proc_started)
            .expect("instance maps process start");
        let proc_finished = AgentEvent::Task(agent_domain::TaskEvent::Finished {
            task_id: agent_domain::BackgroundTaskId::from("t2"),
            status: agent_domain::TaskStatus::Completed,
            detail: None,
        });
        assert_eq!(
            host.trigger_point_for_event(&proc_finished).map(|(t, _)| t),
            Some(TriggerPoint::TaskCompleted)
        );
    }

    // —— 回归：审计失败可见 + replay 去重 ——

    #[tokio::test]
    async fn audit_sink_failures_are_visible_and_replay_dedups_by_event_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("audit.sqlite");
        let sink = SqliteHookAuditSink::open(&db).expect("sink opens");

        let event = UserHookEvent::new(
            EventId::from("evt-1"),
            agent_domain::Timestamp::from_unix_millis(1),
            "h1".into(),
            TriggerPoint::RunStarted,
            HookScope::Global,
            "Process".into(),
            "Sync".into(),
            UserHookEventPayload::Dispatch {
                status: user_hooks::HookStatus::Success,
                duration_ms: 1,
                summary: Some("ok".into()),
            },
        );
        // 同 event_id 重复写入 → INSERT OR IGNORE 去重。
        sink.record(event.clone()).await;
        sink.record(event).await;
        assert_eq!(sink.failure_count(), 0, "dedup insert must not fail");
        let replayed = sink.replay().expect("replay works");
        assert_eq!(replayed.len(), 1, "duplicate event_id must dedup");
        assert_eq!(replayed[0].event_id.as_str(), "evt-1");

        // 写失败不静默：破坏表后再次写入 → 失败计数可见。
        let other = rusqlite::Connection::open(&db).expect("second connection");
        other
            .execute_batch("DROP TABLE user_hook_events")
            .expect("drop audit table");
        let failing = UserHookEvent::new(
            EventId::from("evt-2"),
            agent_domain::Timestamp::from_unix_millis(2),
            "h1".into(),
            TriggerPoint::RunStarted,
            HookScope::Global,
            "Process".into(),
            "Sync".into(),
            UserHookEventPayload::Dispatch {
                status: user_hooks::HookStatus::Success,
                duration_ms: 1,
                summary: None,
            },
        );
        sink.record(failing).await;
        assert!(
            sink.failure_count() > 0,
            "audit write failure must be observable via failure_count"
        );
    }

    // —— 回归：真实 Command 超时 + 输出 secret redaction（Sandbox→Process）——

    #[tokio::test]
    async fn command_executor_times_out_and_redacts_output() {
        // 用永远可用的 NativeRestricted 软沙箱后端（同一 Sandbox → Process
        // 执行链，redaction / timeout / cancel 语义后端无关）。
        let executor = SandboxCommandExecutor::new(
            Arc::new(sandbox_runtime::NativeRestricted::new()),
            Vec::new(),
        )
        .with_max_output_bytes(1024 * 1024);

        // 输出 redaction：子进程 stdout 回显 env 注入的明文 → 替换为占位符。
        let secret = "sk-output-secret-777";
        let result = executor
            .run(
                CommandRequest {
                    program: "sh".into(),
                    args: vec!["-c".into(), "echo MARKER=$PAWORK_TEST_SECRET".into()],
                    env: vec![(
                        "PAWORK_TEST_SECRET".into(),
                        user_hooks::SecretString::new(secret),
                    )],
                    working_directory: None,
                },
                Some(std::time::Duration::from_secs(10)),
            )
            .await
            .expect("sandboxed command runs");
        assert_eq!(result.exit_code, 0);
        assert!(
            !result.stdout.contains(secret),
            "stdout must not leak secret: {}",
            result.stdout
        );
        assert!(
            result.stdout.contains(user_hooks::REDACTED),
            "stdout must be redacted: {}",
            result.stdout
        );

        // 真实超时：sleep 被取消并收敛为 timed_out。
        let result = executor
            .run(
                CommandRequest {
                    program: "/bin/sleep".into(),
                    args: vec!["30".into()],
                    env: Vec::new(),
                    working_directory: None,
                },
                Some(std::time::Duration::from_millis(150)),
            )
            .await
            .expect("timeout path returns result");
        assert!(result.timed_out, "sleep must be cancelled by timeout");
    }

    // —— 回归：真实 HTTP 超时（静默 listener）——

    #[tokio::test]
    async fn http_executor_times_out_against_silent_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            // 接受连接但永不响应：逼出读超时路径。
            if let Ok((stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        });
        let executor = HttpHookExecutor::new(
            provider_runtime::http::HttpClientConfig::builder()
                .disable_system_proxy()
                .build(),
        )
        .expect("http executor");
        let result = executor
            .send(
                WebhookRequest {
                    url: format!("http://127.0.0.1:{port}/hook"),
                    method: "POST".into(),
                    headers: Vec::new(),
                    body: Some("hello".into()),
                },
                Some(std::time::Duration::from_millis(200)),
            )
            .await
            .expect("timeout must return a result");
        assert!(result.timed_out, "silent listener must produce timeout");
    }
}
