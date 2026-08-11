//! Tool 调度器实现（P3-4）。
//!
//! 调度策略（control-flow.md §5 与 ADR-008 capability）：
//! - ReadOnly 可并发（受全局 max_concurrent 上限约束）。
//! - WorkspaceWrite / Process / Network / UserInteraction / ExternalPlugin 默认串行。
//! - GitWrite 全局串行（Git index 串行）。
//! - 命中同一文件路径的调用串行（从 input 提取路径 key）。
//! - 审批期间暂停相关调用，await 决策后再执行或拒绝。
//! - 所有调用可取消（CancellationToken 传播到 AgentTool::execute）。

use std::collections::HashMap;
use std::sync::Arc;

use agent_domain::{CancellationToken, ToolCallId};
pub use policy_engine::ApprovalMode;
use policy_engine::{ExecutionConstraints, PolicyDecision, PolicyEngine, PolicyInput};
use tokio::sync::{Mutex, OwnedMutexGuard, Semaphore};
use tool_api::{
    AgentTool, ContinuationMode, ToolCapability, ToolDescriptor, ToolError, ToolErrorKind,
    ToolKind, ToolRequest, ToolResult,
};

#[derive(Clone)]
struct RegistryEntry {
    descriptor: ToolDescriptor,
    local_executor: Option<Arc<dyn AgentTool>>,
}

/// 全局工具注册表：三类 descriptor 共表，本地 executor 仅属于 ClientFunction。
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Arc<HashMap<String, RegistryEntry>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个 ClientFunction executor（合法同名覆盖；非法 descriptor 返回错误）。
    pub fn register(&mut self, tool: Arc<dyn AgentTool>) -> Result<(), ToolRegistryError> {
        let descriptor = tool.descriptor();
        validate_descriptor(&descriptor)?;
        if descriptor.kind != ToolKind::ClientFunction {
            return Err(ToolRegistryError::ExecutorForNonClientFunction {
                name: descriptor.name,
                kind: descriptor.kind,
            });
        }
        let mut map = (*self.tools).clone();
        map.insert(
            descriptor.name.clone(),
            RegistryEntry {
                descriptor,
                local_executor: Some(tool),
            },
        );
        self.tools = Arc::new(map);
        Ok(())
    }

    /// 注册 descriptor-only 的 ProviderHosted / ProviderExtension（覆盖同名）。
    pub fn register_descriptor(
        &mut self,
        descriptor: ToolDescriptor,
    ) -> Result<(), ToolRegistryError> {
        validate_descriptor(&descriptor)?;
        if descriptor.kind == ToolKind::ClientFunction {
            return Err(ToolRegistryError::MissingClientFunctionExecutor {
                name: descriptor.name,
            });
        }
        let mut map = (*self.tools).clone();
        map.insert(
            descriptor.name.clone(),
            RegistryEntry {
                descriptor,
                local_executor: None,
            },
        );
        self.tools = Arc::new(map);
        Ok(())
    }

    /// 批量注册；任一非法 descriptor 立即返回错误（已注册项保持生效）。
    pub fn extend<I>(&mut self, tools: I) -> Result<(), ToolRegistryError>
    where
        I: IntoIterator<Item = Arc<dyn AgentTool>>,
    {
        for tool in tools {
            self.register(tool)?;
        }
        Ok(())
    }

    /// 取得本地 executor；Hosted/Extension 永远返回 `None`。
    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools
            .get(name)
            .and_then(|entry| entry.local_executor.clone())
    }

    /// 查询 canonical descriptor（包括 descriptor-only 外部工具）。
    pub fn descriptor(&self, name: &str) -> Option<ToolDescriptor> {
        self.tools.get(name).map(|entry| entry.descriptor.clone())
    }

    /// 按执行位点查询已注册工具（P15-1 路由依据）；未注册返回 `None`。
    pub fn kind_of(&self, name: &str) -> Option<ToolKind> {
        self.tools.get(name).map(|entry| entry.descriptor.kind)
    }

    /// 返回所有已注册工具的 descriptor（供 Provider 请求携带 tool 定义）。
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        let mut desc: Vec<_> = self
            .tools
            .values()
            .map(|entry| entry.descriptor.clone())
            .collect();
        desc.sort_by(|a, b| a.name.cmp(&b.name));
        desc
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// 从注册表移除工具（P15-6 预算淘汰 / 停用）。返回是否确实存在并移除。
    pub fn remove(&mut self, name: &str) -> bool {
        let mut map = (*self.tools).clone();
        let removed = map.remove(name).is_some();
        if removed {
            self.tools = Arc::new(map);
        }
        removed
    }
}

fn validate_descriptor(descriptor: &ToolDescriptor) -> Result<(), ToolRegistryError> {
    if descriptor.has_consistent_hosting() {
        Ok(())
    } else {
        Err(ToolRegistryError::KindHostingMismatch {
            name: descriptor.name.clone(),
            kind: descriptor.kind,
            hosting_kind: descriptor.hosting.tool_kind(),
        })
    }
}

/// Registry 边界错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ToolRegistryError {
    #[error("tool `{name}` has kind {kind:?}, but its hosting describes {hosting_kind:?}")]
    KindHostingMismatch {
        name: String,
        kind: ToolKind,
        hosting_kind: ToolKind,
    },
    #[error("tool `{name}` has a local executor but kind is {kind:?}")]
    ExecutorForNonClientFunction { name: String, kind: ToolKind },
    #[error("ClientFunction `{name}` requires a local executor")]
    MissingClientFunctionExecutor { name: String },
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("count", &self.tools.len())
            .finish_non_exhaustive()
    }
}

/// 调度配置。
#[derive(Clone, Debug)]
pub struct ToolSchedulerConfig {
    /// 全局最大并发执行数（跨所有 capability）。
    pub max_concurrent: usize,
    /// 每次工具调用使用的审批模式。
    pub approval_mode: ApprovalMode,
    /// 当前 workspace 是否已被用户信任。
    pub workspace_trusted: bool,
}

impl Default for ToolSchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 8,
            approval_mode: ApprovalMode::ReadOnly,
            workspace_trusted: false,
        }
    }
}

/// 审批决策。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Approved,
    Denied,
}

/// 审批解析器：宿主注入，决定一组 tool call 是否放行。
///
/// 返回值顺序须与输入一致；`Denied` 的工具直接构造拒绝结果回填，不执行。
#[async_trait::async_trait]
pub trait ApprovalResolver: Send + Sync {
    /// 是否代表一次真实、可审计的用户审批通道。
    ///
    /// 自动放行器必须返回 `false`，防止其满足 PolicyEngine 产生的 AskUser。
    fn can_resolve_policy_prompt(&self) -> bool {
        true
    }

    async fn resolve(&self, requests: &[ToolRequest]) -> Vec<ApprovalOutcome>;
}

/// 仅用于策略已直接 Allow 的无审批路径；不能满足 AskUser。
#[derive(Debug, Default, Clone)]
pub struct AutoApproveResolver;

#[async_trait::async_trait]
impl ApprovalResolver for AutoApproveResolver {
    fn can_resolve_policy_prompt(&self) -> bool {
        false
    }

    async fn resolve(&self, requests: &[ToolRequest]) -> Vec<ApprovalOutcome> {
        requests.iter().map(|_| ApprovalOutcome::Approved).collect()
    }
}

/// 调度错误。
#[derive(Debug, thiserror::Error)]
pub enum ToolSchedulerError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
}

/// 本地执行闸门结果（P4-9 策略 + 审批）。
enum GateOutcome {
    /// 放行。
    Approved,
    /// 拒绝；调用方决定返回本地失败结果或 provider-dispatch 错误。
    Denied { reason: String },
}

/// 已授权、等待 Provider transcript 通道接管的调用。
///
/// 此类型不携带 `ToolResult`；续接方式由 descriptor kind 唯一推导。
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderCallDispatch {
    descriptor: ToolDescriptor,
    request: ToolRequest,
}

impl ProviderCallDispatch {
    pub fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    pub fn request(&self) -> &ToolRequest {
        &self.request
    }

    pub fn into_request(self) -> ToolRequest {
        self.request
    }

    pub const fn continuation_mode(&self) -> ContinuationMode {
        self.descriptor.continuation_mode()
    }
}

/// 一次调度的句柄：持有信号量许可与（可选的）串行锁 owned guard。
/// drop 时按字段声明序释放（permit → capability → file）；各锁相互独立、
/// 无层级嵌套，故顺序不影响正确性，也不会死锁。
struct ToolHandle {
    _permit: tokio::sync::OwnedSemaphorePermit,
    _capability_guard: Option<OwnedMutexGuard<()>>,
    _file_guard: Option<OwnedMutexGuard<()>>,
}

/// 调度 key：决定串行的依据（审计/扩展用）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SchedulingKey {
    Concurrent,
    Capability(ToolCapability),
    FilePath(String),
}

/// 调度器的审批状态观测（审计/扩展用）。
#[derive(Clone, Debug, Default)]
pub struct ApprovalState {
    pub pending: usize,
    pub denied: usize,
    pub approved: usize,
}

/// Tool 调度器。
pub struct ToolScheduler {
    registry: ToolRegistry,
    config: ToolSchedulerConfig,
    policy: PolicyEngine,
    global: Arc<Semaphore>,
    /// 每个 capability 一个锁：串行该类别的写/Shell。所有锁为 Arc 以支持 owned guard。
    capability_locks: Mutex<HashMap<ToolCapability, Arc<tokio::sync::Mutex<()>>>>,
    /// 每个文件路径一个锁：同文件串行。
    file_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    // 注：file_locks / capability_locks 惰性建表且当前不回收。每个 ToolScheduler
    // 实例通常对应单个 Run 生命周期，条目数有界；若未来用于长生命周期守护进程，
    // 需引入 LRU 或无等待者时回收（见 ROADMAP 后续强化）。
    /// Git index 全局锁（GitWrite 串行）。
    git_index_lock: Arc<tokio::sync::Mutex<()>>,
}

impl ToolScheduler {
    pub fn new(registry: ToolRegistry, config: ToolSchedulerConfig) -> Self {
        let max = config.max_concurrent.max(1);
        let policy = PolicyEngine::new(config.approval_mode);
        Self {
            registry,
            config,
            policy,
            global: Arc::new(Semaphore::new(max)),
            capability_locks: Mutex::new(HashMap::new()),
            file_locks: Mutex::new(HashMap::new()),
            git_index_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// 已注册工具数。
    pub fn tool_count(&self) -> usize {
        self.registry.len()
    }

    /// 按显式工具名调度并执行。
    ///
    /// 工具名不从 `request.input` 推断，避免与工具自身名为 `name` 的输入字段冲突。
    /// 调用方必须同时提供当前 workspace/run 上下文与流式事件 sink。
    ///
    /// P15-1 位点路由：只允许 `ClientFunction` 进入本地执行。Hosted/Extension
    /// 均返回明确错误，且绝不构造 `ToolResult`。
    pub async fn execute_named(
        &self,
        name: &str,
        request: ToolRequest,
        context: tool_api::ToolExecutionContext,
        cancel: CancellationToken,
        approval: &(dyn ApprovalResolver + Send + Sync),
        sink: &dyn tool_api::ToolEventSink,
    ) -> Result<ToolResult, ToolError> {
        let descriptor = self.registry.descriptor(name).ok_or_else(|| ToolError {
            kind: ToolErrorKind::NotFound,
            message: format!("unknown tool: {name}"),
            retryable: false,
            retry_after_ms: None,
        })?;
        match descriptor.kind {
            ToolKind::ClientFunction => {
                self.execute_with_tool(descriptor, request, context, cancel, approval, sink)
                    .await
            }
            ToolKind::ProviderHosted => {
                Err(ToolError::not_locally_executable(name, "provider-hosted"))
            }
            ToolKind::ProviderExtension => Err(ToolError::not_locally_executable(
                name,
                "provider-extension",
            )),
        }
    }

    /// 授权一次 Provider-owned 调用并交给 transcript/dispatch hook。
    ///
    /// 返回值不含 `ToolResult`。Hosted 与本地工具一致过 [`Self::check_gate`]：
    /// 尊重 descriptor 的 `requires_approval` 与 `allowed_in_untrusted_workspace`；
    /// Extension 强制显式审批，且未信任 workspace 无条件拒绝（不允许 descriptor
    /// 自降级）。ClientFunction 必须走 [`Self::execute_named`]。
    pub async fn authorize_provider_call(
        &self,
        name: &str,
        mut request: ToolRequest,
        cancel: CancellationToken,
        approval: &(dyn ApprovalResolver + Send + Sync),
    ) -> Result<ProviderCallDispatch, ToolError> {
        let descriptor = self.registry.descriptor(name).ok_or_else(|| ToolError {
            kind: ToolErrorKind::NotFound,
            message: format!("unknown tool: {name}"),
            retryable: false,
            retry_after_ms: None,
        })?;
        if cancel.is_cancelled() {
            return Err(ToolError::cancelled(
                "provider-owned tool cancelled before authorization",
            ));
        }

        match descriptor.kind {
            ToolKind::ClientFunction => Err(ToolError {
                kind: ToolErrorKind::InvalidInput,
                message: format!(
                    "ClientFunction `{name}` must be executed by core, not provider dispatch"
                ),
                retryable: false,
                retry_after_ms: None,
            }),
            ToolKind::ProviderHosted | ToolKind::ProviderExtension => {
                let (requires_explicit_approval, allowed_in_untrusted_workspace) =
                    match descriptor.kind {
                        ToolKind::ProviderHosted => (
                            descriptor.requires_approval,
                            descriptor.allowed_in_untrusted_workspace,
                        ),
                        // Extension 永远强制显式审批 + 未信任拒绝。
                        _ => (true, false),
                    };
                if let GateOutcome::Denied { reason } = self
                    .check_gate(
                        &descriptor,
                        &mut request,
                        approval,
                        requires_explicit_approval,
                        allowed_in_untrusted_workspace,
                    )
                    .await
                {
                    return Err(ToolError {
                        kind: ToolErrorKind::PermissionDenied,
                        message: reason,
                        retryable: false,
                        retry_after_ms: None,
                    });
                }
                if cancel.is_cancelled() {
                    return Err(ToolError::cancelled(
                        "provider-owned tool cancelled after authorization",
                    ));
                }
                Ok(ProviderCallDispatch {
                    descriptor,
                    request,
                })
            }
        }
    }

    /// 按执行位点查询已注册工具（P15-1 路由依据）；未注册返回 `None`。
    pub fn kind_of(&self, name: &str) -> Option<ToolKind> {
        self.registry.kind_of(name)
    }

    async fn execute_with_tool(
        &self,
        descriptor: ToolDescriptor,
        mut request: ToolRequest,
        context: tool_api::ToolExecutionContext,
        cancel: CancellationToken,
        approval: &(dyn ApprovalResolver + Send + Sync),
        sink: &dyn tool_api::ToolEventSink,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .registry
            .get(&descriptor.name)
            .ok_or_else(|| ToolError {
                kind: ToolErrorKind::Internal,
                message: format!("ClientFunction `{}` has no local executor", descriptor.name),
                retryable: false,
                retry_after_ms: None,
            })?;
        let capability = descriptor.capability.clone();
        if let GateOutcome::Denied { reason } = self
            .check_gate(
                &descriptor,
                &mut request,
                approval,
                descriptor.requires_approval,
                descriptor.allowed_in_untrusted_workspace,
            )
            .await
        {
            return Ok(denied_result(&request.tool_call_id, reason));
        }

        if cancel.is_cancelled() {
            return Err(ToolError::cancelled("tool cancelled before execution"));
        }

        // 获取调度锁。
        let handle = self.acquire(&capability, &request, &cancel).await?;

        let result = tool
            .execute(request.clone(), context, sink, cancel.clone())
            .await;

        drop(handle);
        result
    }

    /// 策略 + 审批闸门（P4-9 PolicyEngine）。
    ///
    /// - 策略 `Deny`（未信任工作区 / 危险命令地板）优先，直接拒绝；
    /// - 描述符 `requires_approval` 把策略放行升级为显式用户审批（与 PolicyEngine
    ///   对齐，自动放行器不能满足）；
    /// - `AskUser` 必须由真实审批通道放行，否则 fail closed。
    async fn check_gate(
        &self,
        descriptor: &ToolDescriptor,
        request: &mut ToolRequest,
        approval: &(dyn ApprovalResolver + Send + Sync),
        requires_explicit_approval: bool,
        allowed_in_untrusted_workspace: bool,
    ) -> GateOutcome {
        let mut decision = self.policy.decide(&PolicyInput {
            capability: descriptor.capability.clone(),
            input: request.input.clone(),
            trusted: self.config.workspace_trusted,
            allowed_in_untrusted_workspace,
            approval_mode: self.config.approval_mode,
        });
        if requires_explicit_approval && !matches!(decision, PolicyDecision::Deny { .. }) {
            decision = PolicyDecision::AskUser {
                prompt: policy_engine::ApprovalPrompt {
                    message: format!("tool `{}` requires explicit approval", descriptor.name),
                    risk: policy_engine::RiskLevel::Moderate,
                },
            };
        }

        match decision {
            PolicyDecision::Deny { reason } => GateOutcome::Denied { reason },
            PolicyDecision::AskUser { .. } => {
                if !approval.can_resolve_policy_prompt() {
                    return GateOutcome::Denied {
                        reason: "policy requires explicit user approval; automatic approval is forbidden"
                            .into(),
                    };
                }
                let outcomes = approval.resolve(std::slice::from_ref(request)).await;
                if !matches!(outcomes.first(), Some(ApprovalOutcome::Approved)) {
                    return GateOutcome::Denied {
                        reason: "tool call denied or approval was not provided".into(),
                    };
                }
                GateOutcome::Approved
            }
            PolicyDecision::AllowWithConstraints { constraints } => {
                apply_execution_constraints(request, &constraints);
                GateOutcome::Approved
            }
            PolicyDecision::Allow => GateOutcome::Approved,
        }
    }

    /// 获取调度锁。按 capability 决定并发/串行，命中文件 key 时加文件锁。
    async fn acquire(
        &self,
        capability: &ToolCapability,
        request: &ToolRequest,
        cancel: &CancellationToken,
    ) -> Result<ToolHandle, ToolError> {
        // 1) 全局并发许可。
        let permit = self
            .global
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ToolError {
                kind: ToolErrorKind::Internal,
                message: "scheduler semaphore closed".into(),
                retryable: false,
                retry_after_ms: None,
            })?;

        // 2) capability 串行锁（非只读）。
        let capability_guard = if capability.permits_concurrent_execution() {
            None
        } else if matches!(capability, ToolCapability::GitWrite) {
            Some(self.git_index_lock.clone().lock_owned().await)
        } else {
            let lock = self.capability_lock(capability).await;
            Some(lock.lock_owned().await)
        };

        // 3) 文件锁（命中文件 key）。
        let file_guard = if let Some(path) = extract_file_key(&request.input) {
            let lock = self.file_lock(&path).await;
            Some(lock.lock_owned().await)
        } else {
            None
        };

        if cancel.is_cancelled() {
            return Err(ToolError::cancelled(
                "tool cancelled while waiting for scheduling lock",
            ));
        }

        Ok(ToolHandle {
            _permit: permit,
            _capability_guard: capability_guard,
            _file_guard: file_guard,
        })
    }

    async fn capability_lock(&self, capability: &ToolCapability) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.capability_locks.lock().await;
        locks
            .entry(capability.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    async fn file_lock(&self, path: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.file_locks.lock().await;
        locks
            .entry(path.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

/// 从工具 input 中提取文件路径 key（用于同文件串行）。
///
/// 支持常见键名：path / file / filename / glob / pattern（取首个非空字符串值）。
pub fn extract_file_key(input: &serde_json::Value) -> Option<String> {
    if let Some(obj) = input.as_object() {
        for key in &["path", "file", "filename", "glob", "pattern"] {
            if let Some(val) = obj.get(*key).and_then(|v| v.as_str()) {
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

fn apply_execution_constraints(request: &mut ToolRequest, constraints: &ExecutionConstraints) {
    let Some(input) = request.input.as_object_mut() else {
        return;
    };
    for (key, limit) in [
        ("timeout_ms", constraints.timeout_ms),
        ("max_output_bytes", constraints.max_output_bytes),
    ] {
        let Some(limit) = limit else { continue };
        let current = input.get(key).and_then(serde_json::Value::as_u64);
        if match current {
            Some(value) => value > limit,
            None => true,
        } {
            input.insert(key.into(), serde_json::Value::from(limit));
        }
    }
}

fn denied_result(_tool_call_id: &ToolCallId, reason: impl Into<String>) -> ToolResult {
    ToolResult::failure(agent_domain::ErrorContext {
        category: agent_domain::ErrorCategory::Authorization,
        message: reason.into(),
        retryable: false,
        retry_after_ms: None,
        diagnostics: Default::default(),
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopToolEventSink;

#[async_trait::async_trait]
impl tool_api::ToolEventSink for NoopToolEventSink {
    async fn emit(&self, _event: tool_api::ToolStreamEvent) -> Result<(), ToolError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    use serde_json::json;

    type SeenPolicyCalls = Arc<StdMutex<Vec<(tool_api::ToolExecutionContext, serde_json::Value)>>>;

    fn make_scheduler(
        tools: Vec<Arc<dyn AgentTool>>,
        config: ToolSchedulerConfig,
    ) -> ToolScheduler {
        let mut registry = ToolRegistry::new();
        registry.extend(tools).expect("test tools must register");
        ToolScheduler::new(registry, config)
    }

    fn req(name: &str, input: serde_json::Value) -> ToolRequest {
        ToolRequest {
            tool_call_id: ToolCallId::from(name),
            input,
        }
    }

    fn execution_context() -> tool_api::ToolExecutionContext {
        tool_api::ToolExecutionContext {
            workspace_id: agent_domain::WorkspaceId::from("workspace-real"),
            run_id: agent_domain::RunId::from("run-real"),
            working_directory: Some("project".into()),
        }
    }

    fn policy_config(approval_mode: ApprovalMode, workspace_trusted: bool) -> ToolSchedulerConfig {
        ToolSchedulerConfig {
            max_concurrent: 4,
            approval_mode,
            workspace_trusted,
        }
    }

    struct AutomaticApprovalSpy(Arc<AtomicU64>);

    #[async_trait::async_trait]
    impl ApprovalResolver for AutomaticApprovalSpy {
        fn can_resolve_policy_prompt(&self) -> bool {
            false
        }

        async fn resolve(&self, requests: &[ToolRequest]) -> Vec<ApprovalOutcome> {
            self.0.fetch_add(requests.len() as u64, Ordering::SeqCst);
            requests.iter().map(|_| ApprovalOutcome::Approved).collect()
        }
    }

    struct ExplicitApprove;

    #[async_trait::async_trait]
    impl ApprovalResolver for ExplicitApprove {
        async fn resolve(&self, requests: &[ToolRequest]) -> Vec<ApprovalOutcome> {
            requests.iter().map(|_| ApprovalOutcome::Approved).collect()
        }
    }

    async fn execute_named(
        scheduler: &ToolScheduler,
        name: &str,
        input: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        scheduler
            .execute_named(
                name,
                req(name, input),
                execution_context(),
                CancellationToken::new(),
                &AutoApproveResolver,
                &NoopToolEventSink,
            )
            .await
    }

    /// 创建一个共享探测状态（多个工具共享同一组计数器，便于测量跨工具并发）。
    fn probe_shared() -> ProbeShared {
        ProbeShared {
            counter: Arc::new(AtomicU64::new(0)),
            max_seen: Arc::new(AtomicU64::new(0)),
            current: Arc::new(AtomicU64::new(0)),
        }
    }

    fn probe(name: &'static str, cap: ToolCapability, shared: ProbeShared) -> Arc<dyn AgentTool> {
        let tool = ProbeTool {
            name,
            capability: cap,
            shared: shared.clone(),
        };
        Arc::new(tool)
    }

    #[derive(Clone)]
    struct ProbeShared {
        counter: Arc<AtomicU64>,
        max_seen: Arc<AtomicU64>,
        current: Arc<AtomicU64>,
    }

    struct ProbeTool {
        name: &'static str,
        capability: ToolCapability,
        shared: ProbeShared,
    }

    struct PolicyProbeTool {
        name: &'static str,
        capability: ToolCapability,
        allowed_in_untrusted_workspace: bool,
        calls: Arc<AtomicU64>,
        seen: SeenPolicyCalls,
    }

    #[async_trait::async_trait]
    impl AgentTool for PolicyProbeTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: self.name.into(),
                description: "policy probe".into(),
                input_schema: json!({"type": "object"}),
                capability: self.capability.clone(),
                kind: tool_api::ToolKind::ClientFunction,
                hosting: tool_api::ToolHosting::Local,
                capabilities: Vec::new(),
                requires_approval: false,
                read_only: matches!(self.capability, ToolCapability::ReadOnly),
                supports_concurrency: matches!(self.capability, ToolCapability::ReadOnly),
                default_timeout_ms: None,
                max_output_bytes: 2 * 1024 * 1024,
                allowed_in_untrusted_workspace: self.allowed_in_untrusted_workspace,
            }
        }

        async fn execute(
            &self,
            request: ToolRequest,
            context: tool_api::ToolExecutionContext,
            _sink: &dyn tool_api::ToolEventSink,
            _cancel: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen
                .lock()
                .expect("policy probe")
                .push((context, request.input));
            Ok(ToolResult::success(Vec::new()))
        }
    }

    fn policy_probe(
        name: &'static str,
        capability: ToolCapability,
        allowed_in_untrusted_workspace: bool,
    ) -> (Arc<dyn AgentTool>, Arc<AtomicU64>, SeenPolicyCalls) {
        let calls = Arc::new(AtomicU64::new(0));
        let seen = Arc::new(StdMutex::new(Vec::new()));
        (
            Arc::new(PolicyProbeTool {
                name,
                capability,
                allowed_in_untrusted_workspace,
                calls: calls.clone(),
                seen: seen.clone(),
            }),
            calls,
            seen,
        )
    }

    #[async_trait::async_trait]
    impl AgentTool for ProbeTool {
        fn descriptor(&self) -> ToolDescriptor {
            let read_only = matches!(self.capability, ToolCapability::ReadOnly);
            ToolDescriptor {
                name: self.name.into(),
                description: "probe".into(),
                input_schema: json!({"type": "object"}),
                capability: self.capability.clone(),
                kind: tool_api::ToolKind::ClientFunction,
                hosting: tool_api::ToolHosting::Local,
                capabilities: Vec::new(),
                requires_approval: false,
                read_only,
                supports_concurrency: read_only,
                default_timeout_ms: Some(5_000),
                max_output_bytes: 1024,
                allowed_in_untrusted_workspace: true,
            }
        }
        async fn execute(
            &self,
            _request: ToolRequest,
            _context: tool_api::ToolExecutionContext,
            _sink: &dyn tool_api::ToolEventSink,
            cancel: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            self.shared.counter.fetch_add(1, Ordering::SeqCst);
            let cur = self.shared.current.fetch_add(1, Ordering::SeqCst) + 1;
            loop {
                let prev = self.shared.max_seen.load(Ordering::SeqCst);
                if cur <= prev {
                    break;
                }
                if self
                    .shared
                    .max_seen
                    .compare_exchange(prev, cur, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            self.shared.current.fetch_sub(1, Ordering::SeqCst);
            if cancel.is_cancelled() {
                return Err(ToolError::cancelled("probe cancelled"));
            }
            Ok(ToolResult::success(vec![]))
        }
    }

    #[tokio::test]
    async fn read_only_tools_run_concurrently() {
        let shared = probe_shared();
        let a = probe("read_a", ToolCapability::ReadOnly, shared.clone());
        let b = probe("read_b", ToolCapability::ReadOnly, shared.clone());
        let scheduler = make_scheduler(
            vec![a, b],
            ToolSchedulerConfig {
                max_concurrent: 2,
                approval_mode: ApprovalMode::ReadOnly,
                workspace_trusted: false,
            },
        );
        let (r1, r2) = tokio::join!(
            execute_named(&scheduler, "read_a", json!({})),
            execute_named(&scheduler, "read_b", json!({})),
        );
        r1.unwrap();
        r2.unwrap();
        assert_eq!(shared.counter.load(Ordering::SeqCst), 2);
        let peak = shared.max_seen.load(Ordering::SeqCst);
        assert_eq!(peak, 2, "只读工具应并发执行，峰值并发应为 2");
    }

    #[tokio::test]
    async fn write_tools_run_serially() {
        let shared = probe_shared();
        let a = probe("write_a", ToolCapability::WorkspaceWrite, shared.clone());
        let b = probe("write_b", ToolCapability::WorkspaceWrite, shared.clone());
        let scheduler = make_scheduler(
            vec![a, b],
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::NeverAsk,
                workspace_trusted: true,
            },
        );
        let (r1, r2) = tokio::join!(
            execute_named(&scheduler, "write_a", json!({})),
            execute_named(&scheduler, "write_b", json!({})),
        );
        r1.unwrap();
        r2.unwrap();
        assert_eq!(shared.counter.load(Ordering::SeqCst), 2);
        let peak = shared.max_seen.load(Ordering::SeqCst);
        assert_eq!(peak, 1, "写工具应串行执行，峰值并发应为 1");
    }

    #[tokio::test]
    async fn same_file_operations_are_serial() {
        let shared = probe_shared();
        let a = probe("file_a", ToolCapability::ReadOnly, shared.clone());
        let b = probe("file_b", ToolCapability::ReadOnly, shared.clone());
        let scheduler = make_scheduler(
            vec![a, b],
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::ReadOnly,
                workspace_trusted: false,
            },
        );
        let (r1, r2) = tokio::join!(
            execute_named(&scheduler, "file_a", json!({"path": "same.txt"})),
            execute_named(&scheduler, "file_b", json!({"path": "same.txt"})),
        );
        r1.unwrap();
        r2.unwrap();
        let peak = shared.max_seen.load(Ordering::SeqCst);
        assert_eq!(peak, 1, "同一文件上的操作应串行");
    }

    #[tokio::test]
    async fn git_index_operations_are_serial() {
        let shared = probe_shared();
        let a = probe("git_a", ToolCapability::GitWrite, shared.clone());
        let b = probe("git_b", ToolCapability::GitWrite, shared.clone());
        let scheduler = make_scheduler(
            vec![a, b],
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::NeverAsk,
                workspace_trusted: true,
            },
        );
        let (r1, r2) = tokio::join!(
            execute_named(&scheduler, "git_a", json!({})),
            execute_named(&scheduler, "git_b", json!({})),
        );
        r1.unwrap();
        r2.unwrap();
        let peak = shared.max_seen.load(Ordering::SeqCst);
        assert_eq!(peak, 1, "Git index 操作应串行");
    }

    #[tokio::test]
    async fn approval_can_pause_and_deny() {
        struct DenyAll;
        #[async_trait::async_trait]
        impl ApprovalResolver for DenyAll {
            async fn resolve(&self, _requests: &[ToolRequest]) -> Vec<ApprovalOutcome> {
                vec![ApprovalOutcome::Denied]
            }
        }
        let tool = probe("write_d", ToolCapability::WorkspaceWrite, probe_shared());
        let scheduler = make_scheduler(
            vec![tool],
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::AskForWrites,
                workspace_trusted: true,
            },
        );
        let result = scheduler
            .execute_named(
                "write_d",
                req("write_d", json!({})),
                execution_context(),
                CancellationToken::new(),
                &DenyAll,
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(result.is_error(), "拒绝的工具应返回错误结果");
    }

    #[tokio::test]
    async fn cancellation_propagates_to_tool() {
        let tool = probe("read_x", ToolCapability::ReadOnly, probe_shared());
        let scheduler = make_scheduler(
            vec![tool],
            ToolSchedulerConfig {
                max_concurrent: 2,
                approval_mode: ApprovalMode::ReadOnly,
                workspace_trusted: false,
            },
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = scheduler
            .execute_named(
                "read_x",
                req("read_x", json!({})),
                execution_context(),
                cancel,
                &AutoApproveResolver,
                &NoopToolEventSink,
            )
            .await;
        assert!(
            matches!(result, Err(ref e) if e.kind == ToolErrorKind::Cancelled),
            "取消应传播为 Cancelled 错误，实际 {result:?}"
        );
    }

    #[tokio::test]
    async fn unknown_tool_returns_not_found() {
        let scheduler = make_scheduler(vec![], ToolSchedulerConfig::default());
        let err = scheduler
            .execute_named(
                "ghost",
                req("ghost", json!({})),
                execution_context(),
                CancellationToken::new(),
                &AutoApproveResolver,
                &NoopToolEventSink,
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::NotFound);
    }

    fn site_descriptor(name: &str, kind: ToolKind) -> ToolDescriptor {
        let hosting = match kind {
            ToolKind::ClientFunction => tool_api::ToolHosting::Local,
            ToolKind::ProviderHosted => tool_api::ToolHosting::ProviderHosted {
                hosted_name: name.into(),
                kind: tool_api::ToolCapabilityTag::WebSearch,
            },
            ToolKind::ProviderExtension => tool_api::ToolHosting::ProviderExtension {
                reference: "connector:probe".into(),
            },
        };
        ToolDescriptor {
            name: name.into(),
            description: "site probe".into(),
            input_schema: json!({"type": "object"}),
            capability: ToolCapability::Network,
            kind,
            hosting,
            capabilities: Vec::new(),
            requires_approval: kind == ToolKind::ProviderExtension,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: None,
            max_output_bytes: 1024,
            allowed_in_untrusted_workspace: kind == ToolKind::ProviderHosted,
        }
    }

    #[test]
    fn registry_reports_execution_kind_for_all_three_sites() {
        let mut registry = ToolRegistry::new();
        registry
            .register(probe("client", ToolCapability::ReadOnly, probe_shared()))
            .expect("client tool registers");
        registry
            .register_descriptor(site_descriptor("hosted", ToolKind::ProviderHosted))
            .unwrap();
        registry
            .register_descriptor(site_descriptor("extension", ToolKind::ProviderExtension))
            .unwrap();
        assert_eq!(registry.kind_of("client"), Some(ToolKind::ClientFunction));
        assert_eq!(registry.kind_of("hosted"), Some(ToolKind::ProviderHosted));
        assert_eq!(
            registry.kind_of("extension"),
            Some(ToolKind::ProviderExtension)
        );
        assert!(registry.get("client").is_some());
        assert!(registry.get("hosted").is_none());
        assert!(registry.get("extension").is_none());
        assert_eq!(registry.descriptors().len(), 3);
        assert_eq!(registry.kind_of("ghost"), None);
    }

    #[test]
    fn registry_validates_kind_hosting_and_executor_ownership() {
        let mut registry = ToolRegistry::new();
        let mut mismatched = site_descriptor("bad", ToolKind::ProviderHosted);
        mismatched.hosting = tool_api::ToolHosting::Local;
        assert!(matches!(
            registry.register_descriptor(mismatched),
            Err(ToolRegistryError::KindHostingMismatch { .. })
        ));
        assert!(matches!(
            registry.register_descriptor(site_descriptor(
                "client_without_executor",
                ToolKind::ClientFunction
            )),
            Err(ToolRegistryError::MissingClientFunctionExecutor { .. })
        ));
    }

    #[test]
    fn registry_remove_drops_existing_entry_and_reports_missing() {
        let mut registry = ToolRegistry::new();
        registry
            .register(probe("client", ToolCapability::ReadOnly, probe_shared()))
            .unwrap();
        registry
            .register_descriptor(site_descriptor("extension", ToolKind::ProviderExtension))
            .unwrap();
        assert_eq!(registry.len(), 2);

        assert!(registry.remove("client"));
        assert_eq!(registry.len(), 1);
        assert!(registry.get("client").is_none());
        assert!(registry.descriptor("client").is_none());
        assert_eq!(registry.kind_of("client"), None);
        assert_eq!(
            registry.kind_of("extension"),
            Some(ToolKind::ProviderExtension)
        );

        assert!(!registry.remove("client"), "重复移除返回 false");
        assert!(!registry.remove("ghost"));
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn provider_hosted_tool_call_is_never_executed_locally() {
        let mut registry = ToolRegistry::new();
        registry
            .register_descriptor(site_descriptor("web_search", ToolKind::ProviderHosted))
            .unwrap();
        let scheduler = ToolScheduler::new(
            registry,
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::NeverAsk,
                workspace_trusted: true,
            },
        );
        let error = scheduler
            .execute_named(
                "web_search",
                req("web_search", json!({"query": "pawork"})),
                execution_context(),
                CancellationToken::new(),
                &AutoApproveResolver,
                &NoopToolEventSink,
            )
            .await
            .expect_err("hosted tool must not be executed locally");
        assert_eq!(error.kind, ToolErrorKind::NotLocallyExecutable);

        let dispatch = scheduler
            .authorize_provider_call(
                "web_search",
                req("web_search", json!({"query": "pawork"})),
                CancellationToken::new(),
                &AutoApproveResolver,
            )
            .await
            .expect("hosted call is dispatched without a local result");
        assert_eq!(dispatch.descriptor().kind, ToolKind::ProviderHosted);
        assert_eq!(
            dispatch.continuation_mode(),
            ContinuationMode::ProviderTranscript
        );
    }

    #[tokio::test]
    async fn provider_hosted_respects_descriptor_gate_flags() {
        // hosted 默认（requires_approval=false, allowed_in_untrusted=true）。
        let mut gated = site_descriptor("hosted_gated", ToolKind::ProviderHosted);
        gated.requires_approval = true;
        gated.allowed_in_untrusted_workspace = false;
        let mut registry = ToolRegistry::new();
        registry.register_descriptor(gated).unwrap();

        // 未信任 workspace + allowed=false：策略直接拒绝。
        let untrusted = ToolScheduler::new(
            registry.clone(),
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::NeverAsk,
                workspace_trusted: false,
            },
        );
        let denied = untrusted
            .authorize_provider_call(
                "hosted_gated",
                req("hosted_gated", json!({})),
                CancellationToken::new(),
                &AutoApproveResolver,
            )
            .await
            .expect_err("untrusted workspace must deny hosted tool not allowed there");
        assert_eq!(denied.kind, ToolErrorKind::PermissionDenied);

        // 信任 workspace + requires_approval=true + 自动放行器：升级 AskUser 后被拒。
        let trusted = ToolScheduler::new(
            registry.clone(),
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::NeverAsk,
                workspace_trusted: true,
            },
        );
        let auto_denied = trusted
            .authorize_provider_call(
                "hosted_gated",
                req("hosted_gated", json!({})),
                CancellationToken::new(),
                &AutoApproveResolver,
            )
            .await
            .expect_err("auto approval must not satisfy requires_approval");
        assert_eq!(auto_denied.kind, ToolErrorKind::PermissionDenied);

        // 显式审批通过后只产生 dispatch，不产生 ToolResult。
        let dispatch = trusted
            .authorize_provider_call(
                "hosted_gated",
                req("hosted_gated", json!({})),
                CancellationToken::new(),
                &ExplicitApprove,
            )
            .await
            .expect("explicit approval dispatches hosted call");
        assert_eq!(dispatch.descriptor().kind, ToolKind::ProviderHosted);
        assert_eq!(
            dispatch.continuation_mode(),
            ContinuationMode::ProviderTranscript
        );

        // 未信任 workspace + allowed=true 仍可 dispatch（descriptor 显式授权）。
        let permissive = site_descriptor("hosted_untrusted_ok", ToolKind::ProviderHosted);
        let mut registry2 = ToolRegistry::new();
        registry2.register_descriptor(permissive).unwrap();
        let untrusted_ok = ToolScheduler::new(
            registry2,
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::NeverAsk,
                workspace_trusted: false,
            },
        );
        let dispatch = untrusted_ok
            .authorize_provider_call(
                "hosted_untrusted_ok",
                req("hosted_untrusted_ok", json!({})),
                CancellationToken::new(),
                &AutoApproveResolver,
            )
            .await
            .expect("allowed_in_untrusted_workspace hosted tool dispatches");
        assert_eq!(dispatch.descriptor().kind, ToolKind::ProviderHosted);
    }

    #[tokio::test]
    async fn provider_extension_keeps_approval_gate_and_is_never_executed_locally() {
        let mut registry = ToolRegistry::new();
        registry
            .register_descriptor(site_descriptor("remote_mcp", ToolKind::ProviderExtension))
            .unwrap();
        let untrusted = ToolScheduler::new(
            registry.clone(),
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::NeverAsk,
                workspace_trusted: false,
            },
        );
        let denied = untrusted
            .authorize_provider_call(
                "remote_mcp",
                req("remote_mcp", json!({})),
                CancellationToken::new(),
                &AutoApproveResolver,
            )
            .await
            .expect_err("untrusted workspace must deny extension");
        assert_eq!(denied.kind, ToolErrorKind::PermissionDenied);

        // 信任工作区 + 自动放行器：requires_approval 升级为 AskUser，自动放行被拒。
        let trusted = ToolScheduler::new(
            registry.clone(),
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::NeverAsk,
                workspace_trusted: true,
            },
        );
        let auto_denied = trusted
            .authorize_provider_call(
                "remote_mcp",
                req("remote_mcp", json!({})),
                CancellationToken::new(),
                &AutoApproveResolver,
            )
            .await
            .expect_err("auto approval must not satisfy explicit extension approval");
        assert_eq!(auto_denied.kind, ToolErrorKind::PermissionDenied);

        // 显式审批通过后只产生 dispatch，不产生 ToolResult。
        let approved_scheduler = ToolScheduler::new(
            registry,
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::NeverAsk,
                workspace_trusted: true,
            },
        );
        let dispatch = approved_scheduler
            .authorize_provider_call(
                "remote_mcp",
                req("remote_mcp", json!({})),
                CancellationToken::new(),
                &ExplicitApprove,
            )
            .await
            .expect("explicitly approved extension is handed to provider dispatch");
        assert_eq!(dispatch.descriptor().kind, ToolKind::ProviderExtension);
        assert_eq!(
            dispatch.continuation_mode(),
            ContinuationMode::ProviderTranscript
        );

        let error = approved_scheduler
            .execute_named(
                "remote_mcp",
                req("remote_mcp", json!({})),
                execution_context(),
                CancellationToken::new(),
                &ExplicitApprove,
                &NoopToolEventSink,
            )
            .await
            .expect_err("extension must never enter the local execution API");
        assert_eq!(error.kind, ToolErrorKind::NotLocallyExecutable);
    }

    #[test]
    fn extract_file_key_supports_common_keys() {
        assert_eq!(
            extract_file_key(&json!({"path": "a.txt"})),
            Some("a.txt".into())
        );
        assert_eq!(
            extract_file_key(&json!({"file": "b.rs"})),
            Some("b.rs".into())
        );
        assert_eq!(extract_file_key(&json!({"foo": 1})), None);
    }

    #[tokio::test]
    async fn global_concurrency_limit_enforced() {
        let shared = probe_shared();
        let a = probe("r1", ToolCapability::ReadOnly, shared.clone());
        let b = probe("r2", ToolCapability::ReadOnly, shared.clone());
        let scheduler = make_scheduler(
            vec![a, b],
            ToolSchedulerConfig {
                max_concurrent: 1,
                approval_mode: ApprovalMode::ReadOnly,
                workspace_trusted: false,
            },
        );
        let (r1, r2) = tokio::join!(
            execute_named(&scheduler, "r1", json!({})),
            execute_named(&scheduler, "r2", json!({})),
        );
        r1.unwrap();
        r2.unwrap();
        let peak = shared.max_seen.load(Ordering::SeqCst);
        assert_eq!(peak, 1, "全局并发上限=1 应强制串行");
    }

    #[tokio::test]
    async fn execute_named_passes_real_workspace_and_run_context() {
        struct ContextProbe {
            seen: Arc<std::sync::Mutex<Option<tool_api::ToolExecutionContext>>>,
        }

        #[async_trait::async_trait]
        impl AgentTool for ContextProbe {
            fn descriptor(&self) -> ToolDescriptor {
                ToolDescriptor {
                    name: "context_probe".into(),
                    description: "records context".into(),
                    input_schema: json!({"type": "object"}),
                    capability: ToolCapability::ReadOnly,
                    kind: tool_api::ToolKind::ClientFunction,
                    hosting: tool_api::ToolHosting::Local,
                    capabilities: Vec::new(),
                    requires_approval: false,
                    read_only: true,
                    supports_concurrency: true,
                    default_timeout_ms: None,
                    max_output_bytes: 1024,
                    allowed_in_untrusted_workspace: true,
                }
            }

            async fn execute(
                &self,
                _request: ToolRequest,
                context: tool_api::ToolExecutionContext,
                _sink: &dyn tool_api::ToolEventSink,
                _cancel: CancellationToken,
            ) -> Result<ToolResult, ToolError> {
                *self.seen.lock().expect("context") = Some(context);
                Ok(ToolResult::success(Vec::new()))
            }
        }

        let seen = Arc::new(std::sync::Mutex::new(None));
        let scheduler = make_scheduler(
            vec![Arc::new(ContextProbe { seen: seen.clone() })],
            ToolSchedulerConfig::default(),
        );
        scheduler
            .execute_named(
                "context_probe",
                req("context_probe", json!({"name": "input-owned-name"})),
                execution_context(),
                CancellationToken::new(),
                &AutoApproveResolver,
                &NoopToolEventSink,
            )
            .await
            .unwrap();

        let context = seen.lock().expect("context").clone().expect("context seen");
        assert_eq!(context.workspace_id.as_str(), "workspace-real");
        assert_eq!(context.run_id.as_str(), "run-real");
        assert_eq!(context.working_directory.as_deref(), Some("project"));
    }

    #[tokio::test]
    async fn untrusted_workspace_enforces_descriptor_gate_then_policy() {
        let (blocked, blocked_calls, _) =
            policy_probe("blocked_write", ToolCapability::WorkspaceWrite, false);
        let blocked_scheduler =
            make_scheduler(vec![blocked], policy_config(ApprovalMode::NeverAsk, false));
        let blocked_result = blocked_scheduler
            .execute_named(
                "blocked_write",
                req("blocked_write", json!({"path": "a.txt"})),
                execution_context(),
                CancellationToken::new(),
                &AutoApproveResolver,
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(blocked_result.is_error());
        assert_eq!(blocked_calls.load(Ordering::SeqCst), 0);

        let (allowed, allowed_calls, _) =
            policy_probe("allowed_write", ToolCapability::WorkspaceWrite, true);
        let allowed_scheduler =
            make_scheduler(vec![allowed], policy_config(ApprovalMode::NeverAsk, false));
        let allowed_result = allowed_scheduler
            .execute_named(
                "allowed_write",
                req("allowed_write", json!({"path": "a.txt"})),
                execution_context(),
                CancellationToken::new(),
                &AutoApproveResolver,
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(!allowed_result.is_error());
        assert_eq!(allowed_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn scheduler_loop_context_auto_approve_cannot_bypass_ask_for_writes() {
        struct NoDecision;
        #[async_trait::async_trait]
        impl ApprovalResolver for NoDecision {
            async fn resolve(&self, _requests: &[ToolRequest]) -> Vec<ApprovalOutcome> {
                Vec::new()
            }
        }

        let (tool, calls, _) = policy_probe("ask_write", ToolCapability::WorkspaceWrite, true);
        let scheduler = make_scheduler(vec![tool], policy_config(ApprovalMode::AskForWrites, true));
        assert!(!AutoApproveResolver.can_resolve_policy_prompt());
        let auto_denied = scheduler
            .execute_named(
                "ask_write",
                req("ask_write", json!({"path": "a.txt"})),
                execution_context(),
                CancellationToken::new(),
                &AutoApproveResolver,
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(auto_denied.is_error());
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let automatic_calls = Arc::new(AtomicU64::new(0));
        let automatic = AutomaticApprovalSpy(automatic_calls.clone());
        let spy_denied = scheduler
            .execute_named(
                "ask_write",
                req("ask_write", json!({"path": "a.txt"})),
                execution_context(),
                CancellationToken::new(),
                &automatic,
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(spy_denied.is_error());
        assert_eq!(automatic_calls.load(Ordering::SeqCst), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let denied = scheduler
            .execute_named(
                "ask_write",
                req("ask_write", json!({"path": "a.txt"})),
                execution_context(),
                CancellationToken::new(),
                &NoDecision,
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(denied.is_error(), "缺失审批必须 fail closed");
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let approved = scheduler
            .execute_named(
                "ask_write",
                req("ask_write", json!({"path": "a.txt"})),
                execution_context(),
                CancellationToken::new(),
                &ExplicitApprove,
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(!approved.is_error());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn allow_with_constraints_injects_caps_without_loosening_request() {
        let (tool, calls, seen) = policy_probe("process", ToolCapability::Process, true);
        let scheduler = make_scheduler(vec![tool], policy_config(ApprovalMode::NeverAsk, true));
        let result = scheduler
            .execute_named(
                "process",
                req(
                    "process",
                    json!({
                        "command": "echo",
                        "args": ["ok"],
                        "timeout_ms": 120_000,
                        "max_output_bytes": 512
                    }),
                ),
                execution_context(),
                CancellationToken::new(),
                &AutoApproveResolver,
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(!result.is_error());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let seen = seen.lock().expect("seen");
        let input = &seen[0].1;
        assert_eq!(input["timeout_ms"], json!(60_000));
        assert_eq!(input["max_output_bytes"], json!(512));
    }

    #[tokio::test]
    async fn scheduler_loop_context_auto_approve_cannot_bypass_never_ask_danger_floor() {
        let (tool, calls, _) = policy_probe("process", ToolCapability::Process, true);
        let scheduler = make_scheduler(vec![tool], policy_config(ApprovalMode::NeverAsk, true));
        for input in [
            json!({"command": "rm", "args": ["-rf", "/"]}),
            json!({"command": "mkfs", "args": ["/dev/sda1"]}),
            json!({"command": "dd", "args": ["if=image", "of=/dev/sda"]}),
        ] {
            let result = scheduler
                .execute_named(
                    "process",
                    req("process", input),
                    execution_context(),
                    CancellationToken::new(),
                    &AutoApproveResolver,
                    &NoopToolEventSink,
                )
                .await
                .unwrap();
            assert!(result.is_error());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn scheduler_preserves_workspace_and_isolates_cross_run_contexts() {
        let (tool, calls, seen) = policy_probe("context", ToolCapability::ReadOnly, true);
        let scheduler = make_scheduler(vec![tool], policy_config(ApprovalMode::ReadOnly, false));
        for (workspace, run) in [("workspace-a", "run-a"), ("workspace-b", "run-b")] {
            scheduler
                .execute_named(
                    "context",
                    req("context", json!({})),
                    tool_api::ToolExecutionContext {
                        workspace_id: agent_domain::WorkspaceId::from(workspace),
                        run_id: agent_domain::RunId::from(run),
                        working_directory: Some(format!("{workspace}/repo")),
                    },
                    CancellationToken::new(),
                    &AutoApproveResolver,
                    &NoopToolEventSink,
                )
                .await
                .unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let seen = seen.lock().expect("seen");
        assert_eq!(seen[0].0.workspace_id.as_str(), "workspace-a");
        assert_eq!(seen[0].0.run_id.as_str(), "run-a");
        assert_eq!(seen[1].0.workspace_id.as_str(), "workspace-b");
        assert_eq!(seen[1].0.run_id.as_str(), "run-b");
    }
}
