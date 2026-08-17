//! 最小工具调度器：注册、并发上限、超时、PolicyEngine 闸门、审批挂点。
//!
//! 已接 PolicyEngine（capability + trusted + descriptor → decide）。
//! 仍不接写锁 / git 锁 / 文件锁。
//! 输出截断留在各工具的 `MAX_OUTPUT_BYTES`，本模块不二次截断。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use pawork_api::{
    AgentTool, ToolError, ToolErrorKind, ToolEventSink, ToolExecutionContext, ToolRequest,
    ToolResult, ToolStreamEvent,
};
use pawork_domain::{CancellationToken, ErrorCategory, ErrorContext, ToolDescriptor, ToolKind};
use pawork_policy::{
    ApprovalMode, ApprovalPrompt, ExecutionConstraints, PolicyDecision, PolicyEngine, PolicyInput,
    RiskLevel,
};
use tokio::sync::Semaphore;

#[derive(Clone)]
struct RegistryEntry {
    descriptor: ToolDescriptor,
    local_executor: Arc<dyn AgentTool>,
}

/// 全局工具注册表：只接受 ClientFunction + 本地 executor。
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
                local_executor: tool,
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

    /// 取得本地 executor。
    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools
            .get(name)
            .map(|entry| entry.local_executor.clone())
    }

    /// 查询 canonical descriptor。
    pub fn descriptor(&self, name: &str) -> Option<ToolDescriptor> {
        self.tools.get(name).map(|entry| entry.descriptor.clone())
    }

    /// 返回所有已注册工具的 descriptor（按名称排序）。
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
    /// 全局最大并发执行数。
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

/// 一次调度的句柄：持有信号量许可；drop 时释放。
struct ToolHandle {
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Tool 调度器。
pub struct ToolScheduler {
    registry: ToolRegistry,
    global: Arc<Semaphore>,
    policy: PolicyEngine,
    config: ToolSchedulerConfig,
}

impl ToolScheduler {
    pub fn new(registry: ToolRegistry, config: ToolSchedulerConfig) -> Self {
        let max = config.max_concurrent.max(1);
        let policy = PolicyEngine::new(config.approval_mode);
        Self {
            registry,
            global: Arc::new(Semaphore::new(max)),
            policy,
            config,
        }
    }

    /// 已注册工具数。
    pub fn tool_count(&self) -> usize {
        self.registry.len()
    }

    /// 按显式工具名调度并执行。
    ///
    /// `approval` 为 `None` 与 [`AutoApproveResolver`] 同等：策略 Allow 时放行，
    /// 但不能满足 AskUser。Denied 不执行，返回 failure [`ToolResult`]。
    pub async fn execute_named(
        &self,
        name: &str,
        request: ToolRequest,
        context: ToolExecutionContext,
        cancel: CancellationToken,
        approval: Option<&dyn ApprovalResolver>,
        sink: &dyn ToolEventSink,
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

    async fn execute_with_tool(
        &self,
        descriptor: ToolDescriptor,
        mut request: ToolRequest,
        context: ToolExecutionContext,
        cancel: CancellationToken,
        approval: Option<&dyn ApprovalResolver>,
        sink: &dyn ToolEventSink,
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

        let asked_user = match self.check_gate(&descriptor, &mut request, approval).await {
            GateOutcome::Denied { reason } => {
                return Ok(denied_result(reason));
            }
            GateOutcome::Approved { asked_user } => asked_user,
        };

        // S2 审批钩子仅在 check_gate 为 Allow / AllowWithConstraints 之后；
        // AskUser 已问过则跳过，避免双问。
        if !asked_user {
            if let Some(resolver) = approval {
                let outcomes = resolver.resolve(std::slice::from_ref(&request)).await;
                if !matches!(outcomes.first(), Some(ApprovalOutcome::Approved)) {
                    return Ok(denied_result(
                        "tool call denied or approval was not provided",
                    ));
                }
            }
        }

        if cancel.is_cancelled() {
            return Err(ToolError::cancelled("tool cancelled before execution"));
        }

        let handle = self.acquire(&cancel).await?;

        let exec = tool.execute(request, context, sink, cancel);
        let result = if let Some(ms) = descriptor.default_timeout_ms {
            match tokio::time::timeout(Duration::from_millis(ms), exec).await {
                Ok(result) => result,
                Err(_) => Err(ToolError {
                    kind: ToolErrorKind::Timeout,
                    message: format!("tool `{}` timed out after {ms}ms", descriptor.name),
                    retryable: false,
                    retry_after_ms: None,
                }),
            }
        } else {
            exec.await
        };

        drop(handle);
        result
    }

    async fn acquire(&self, cancel: &CancellationToken) -> Result<ToolHandle, ToolError> {
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

        if cancel.is_cancelled() {
            return Err(ToolError::cancelled(
                "tool cancelled while waiting for scheduling lock",
            ));
        }

        Ok(ToolHandle { _permit: permit })
    }

    /// 策略 + 审批闸门。
    ///
    /// - 策略 `Deny`（未信任工作区 / ReadOnly 写能力）优先，直接拒绝；
    /// - 描述符 `requires_approval` 把策略放行升级为显式用户审批；
    /// - `AskUser` 必须由真实审批通道放行，否则 fail closed。
    async fn check_gate(
        &self,
        descriptor: &ToolDescriptor,
        request: &mut ToolRequest,
        approval: Option<&dyn ApprovalResolver>,
    ) -> GateOutcome {
        let mut decision = self.policy.decide(&PolicyInput {
            capability: descriptor.capability.clone(),
            input: request.input.clone(),
            trusted: self.config.workspace_trusted,
            allowed_in_untrusted_workspace: descriptor.allowed_in_untrusted_workspace,
            approval_mode: self.config.approval_mode,
        });
        if descriptor.requires_approval && !matches!(decision, PolicyDecision::Deny { .. }) {
            decision = PolicyDecision::AskUser {
                prompt: ApprovalPrompt {
                    message: format!("tool `{}` requires explicit approval", descriptor.name),
                    risk: RiskLevel::Moderate,
                },
            };
        }

        match decision {
            PolicyDecision::Deny { reason } => GateOutcome::Denied { reason },
            PolicyDecision::AskUser { .. } => {
                let fallback = AutoApproveResolver;
                let resolver = approval.unwrap_or(&fallback);
                if !resolver.can_resolve_policy_prompt() {
                    return GateOutcome::Denied {
                        reason: "policy requires explicit user approval; automatic approval is forbidden"
                            .into(),
                    };
                }
                let outcomes = resolver.resolve(std::slice::from_ref(request)).await;
                if !matches!(outcomes.first(), Some(ApprovalOutcome::Approved)) {
                    return GateOutcome::Denied {
                        reason: "tool call denied or approval was not provided".into(),
                    };
                }
                GateOutcome::Approved { asked_user: true }
            }
            PolicyDecision::AllowWithConstraints { constraints } => {
                apply_execution_constraints(request, &constraints);
                GateOutcome::Approved { asked_user: false }
            }
            PolicyDecision::Allow => GateOutcome::Approved { asked_user: false },
        }
    }
}

enum GateOutcome {
    Denied { reason: String },
    Approved { asked_user: bool },
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

fn denied_result(reason: impl Into<String>) -> ToolResult {
    ToolResult::failure(ErrorContext {
        category: ErrorCategory::Authorization,
        message: reason.into(),
        retryable: false,
        retry_after_ms: None,
        diagnostics: Default::default(),
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopToolEventSink;

#[async_trait::async_trait]
impl ToolEventSink for NoopToolEventSink {
    async fn emit(&self, _event: ToolStreamEvent) -> Result<(), ToolError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    use pawork_domain::{RunId, ToolCallId, ToolCapability, ToolHosting, WorkspaceId};
    use serde_json::json;

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

    fn execution_context() -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_id: WorkspaceId::from("workspace-real"),
            run_id: RunId::from("run-real"),
            working_directory: Some("project".into()),
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
                Some(&AutoApproveResolver),
                &NoopToolEventSink,
            )
            .await
    }

    fn probe_shared() -> ProbeShared {
        ProbeShared {
            counter: Arc::new(AtomicU64::new(0)),
            max_seen: Arc::new(AtomicU64::new(0)),
            current: Arc::new(AtomicU64::new(0)),
        }
    }

    fn probe(name: &'static str, shared: ProbeShared) -> Arc<dyn AgentTool> {
        Arc::new(ProbeTool { name, shared })
    }

    #[derive(Clone)]
    struct ProbeShared {
        counter: Arc<AtomicU64>,
        max_seen: Arc<AtomicU64>,
        current: Arc<AtomicU64>,
    }

    struct ProbeTool {
        name: &'static str,
        shared: ProbeShared,
    }

    #[async_trait::async_trait]
    impl AgentTool for ProbeTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: self.name.into(),
                description: "probe".into(),
                input_schema: json!({"type": "object"}),
                capability: ToolCapability::ReadOnly,
                kind: ToolKind::ClientFunction,
                hosting: ToolHosting::Local,
                capabilities: Vec::new(),
                requires_approval: false,
                read_only: true,
                supports_concurrency: true,
                default_timeout_ms: Some(5_000),
                max_output_bytes: 1024,
                allowed_in_untrusted_workspace: true,
            }
        }

        async fn execute(
            &self,
            _request: ToolRequest,
            _context: ToolExecutionContext,
            _sink: &dyn ToolEventSink,
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

    struct SleepTool {
        name: &'static str,
        timeout_ms: Option<u64>,
        sleep_ms: u64,
    }

    #[async_trait::async_trait]
    impl AgentTool for SleepTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: self.name.into(),
                description: "sleep".into(),
                input_schema: json!({"type": "object"}),
                capability: ToolCapability::ReadOnly,
                kind: ToolKind::ClientFunction,
                hosting: ToolHosting::Local,
                capabilities: Vec::new(),
                requires_approval: false,
                read_only: true,
                supports_concurrency: true,
                default_timeout_ms: self.timeout_ms,
                max_output_bytes: 1024,
                allowed_in_untrusted_workspace: true,
            }
        }

        async fn execute(
            &self,
            _request: ToolRequest,
            _context: ToolExecutionContext,
            _sink: &dyn ToolEventSink,
            _cancel: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
            Ok(ToolResult::success(vec![]))
        }
    }

    struct DescriptorTool {
        descriptor: ToolDescriptor,
    }

    #[async_trait::async_trait]
    impl AgentTool for DescriptorTool {
        fn descriptor(&self) -> ToolDescriptor {
            self.descriptor.clone()
        }

        async fn execute(
            &self,
            _request: ToolRequest,
            _context: ToolExecutionContext,
            _sink: &dyn ToolEventSink,
            _cancel: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::success(vec![]))
        }
    }

    fn client_descriptor(name: &str) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            description: "client".into(),
            input_schema: json!({"type": "object"}),
            capability: ToolCapability::ReadOnly,
            kind: ToolKind::ClientFunction,
            hosting: ToolHosting::Local,
            capabilities: Vec::new(),
            requires_approval: false,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: None,
            max_output_bytes: 1024,
            allowed_in_untrusted_workspace: true,
        }
    }

    #[tokio::test]
    async fn read_only_tools_run_concurrently() {
        let shared = probe_shared();
        let a = probe("read_a", shared.clone());
        let b = probe("read_b", shared.clone());
        let scheduler = make_scheduler(
            vec![a, b],
            ToolSchedulerConfig {
                max_concurrent: 2,
                ..Default::default()
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
    async fn global_concurrency_limit_enforced() {
        let shared = probe_shared();
        let a = probe("r1", shared.clone());
        let b = probe("r2", shared.clone());
        let scheduler = make_scheduler(
            vec![a, b],
            ToolSchedulerConfig {
                max_concurrent: 1,
                ..Default::default()
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
    async fn unknown_tool_returns_not_found() {
        let scheduler = make_scheduler(vec![], ToolSchedulerConfig::default());
        let err = scheduler
            .execute_named(
                "ghost",
                req("ghost", json!({})),
                execution_context(),
                CancellationToken::new(),
                Some(&AutoApproveResolver),
                &NoopToolEventSink,
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::NotFound);
    }

    #[tokio::test]
    async fn execute_named_passes_real_workspace_and_run_context() {
        struct ContextProbe {
            seen: Arc<StdMutex<Option<ToolExecutionContext>>>,
        }

        #[async_trait::async_trait]
        impl AgentTool for ContextProbe {
            fn descriptor(&self) -> ToolDescriptor {
                client_descriptor("context_probe")
            }

            async fn execute(
                &self,
                _request: ToolRequest,
                context: ToolExecutionContext,
                _sink: &dyn ToolEventSink,
                _cancel: CancellationToken,
            ) -> Result<ToolResult, ToolError> {
                *self.seen.lock().expect("context") = Some(context);
                Ok(ToolResult::success(Vec::new()))
            }
        }

        let seen = Arc::new(StdMutex::new(None));
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
                None,
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
    async fn cancellation_propagates_to_tool() {
        let tool = probe("read_x", probe_shared());
        let scheduler = make_scheduler(
            vec![tool],
            ToolSchedulerConfig {
                max_concurrent: 2,
                ..Default::default()
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
                Some(&AutoApproveResolver),
                &NoopToolEventSink,
            )
            .await;
        assert!(
            matches!(result, Err(ref e) if e.kind == ToolErrorKind::Cancelled),
            "取消应传播为 Cancelled 错误，实际 {result:?}"
        );
    }

    #[tokio::test]
    async fn cancellation_reaches_tool_during_execute() {
        struct WaitTool;

        #[async_trait::async_trait]
        impl AgentTool for WaitTool {
            fn descriptor(&self) -> ToolDescriptor {
                let mut descriptor = client_descriptor("wait");
                descriptor.default_timeout_ms = Some(5_000);
                descriptor
            }

            async fn execute(
                &self,
                _request: ToolRequest,
                _context: ToolExecutionContext,
                _sink: &dyn ToolEventSink,
                cancel: CancellationToken,
            ) -> Result<ToolResult, ToolError> {
                cancel.cancelled().await;
                Err(ToolError::cancelled("wait saw cancel"))
            }
        }

        let scheduler = make_scheduler(
            vec![Arc::new(WaitTool)],
            ToolSchedulerConfig::default(),
        );
        let cancel = CancellationToken::new();
        let exec = scheduler.execute_named(
            "wait",
            req("wait", json!({})),
            execution_context(),
            cancel.clone(),
            Some(&AutoApproveResolver),
            &NoopToolEventSink,
        );
        tokio::pin!(exec);
        tokio::task::yield_now().await;
        cancel.cancel();
        let result = exec.await;
        assert!(
            matches!(result, Err(ref e) if e.kind == ToolErrorKind::Cancelled),
            "执行中取消应传到工具，实际 {result:?}"
        );
    }

    #[tokio::test]
    async fn default_timeout_ms_maps_to_timeout_error() {
        let scheduler = make_scheduler(
            vec![Arc::new(SleepTool {
                name: "sleepy",
                timeout_ms: Some(40),
                sleep_ms: 2_000,
            })],
            ToolSchedulerConfig::default(),
        );
        let err = execute_named(&scheduler, "sleepy", json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::Timeout);
    }

    #[tokio::test]
    async fn denied_approval_returns_failure_result_without_executing() {
        struct DenyAll;
        #[async_trait::async_trait]
        impl ApprovalResolver for DenyAll {
            async fn resolve(&self, _requests: &[ToolRequest]) -> Vec<ApprovalOutcome> {
                vec![ApprovalOutcome::Denied]
            }
        }

        let shared = probe_shared();
        let scheduler = make_scheduler(
            vec![probe("denied", shared.clone())],
            ToolSchedulerConfig::default(),
        );
        let result = scheduler
            .execute_named(
                "denied",
                req("denied", json!({})),
                execution_context(),
                CancellationToken::new(),
                Some(&DenyAll),
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(result.is_error(), "拒绝的工具应返回 failure ToolResult");
        assert_eq!(shared.counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn auto_approve_cannot_resolve_policy_prompt() {
        assert!(!AutoApproveResolver.can_resolve_policy_prompt());
    }

    #[test]
    fn registry_validates_kind_hosting_and_rejects_non_client_function() {
        let mut registry = ToolRegistry::new();
        let mut mismatched = client_descriptor("bad");
        mismatched.kind = ToolKind::ProviderHosted;
        assert!(matches!(
            registry.register(Arc::new(DescriptorTool {
                descriptor: mismatched,
            })),
            Err(ToolRegistryError::KindHostingMismatch { .. })
        ));

        let hosted = ToolDescriptor {
            name: "hosted".into(),
            description: "hosted".into(),
            input_schema: json!({"type": "object"}),
            capability: ToolCapability::Network,
            kind: ToolKind::ProviderHosted,
            hosting: ToolHosting::ProviderHosted {
                hosted_name: "hosted".into(),
                kind: pawork_domain::ToolCapabilityTag::WebSearch,
            },
            capabilities: Vec::new(),
            requires_approval: false,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: None,
            max_output_bytes: 1024,
            allowed_in_untrusted_workspace: true,
        };
        assert!(matches!(
            registry.register(Arc::new(DescriptorTool { descriptor: hosted })),
            Err(ToolRegistryError::ExecutorForNonClientFunction { .. })
        ));

        registry
            .register(Arc::new(DescriptorTool {
                descriptor: client_descriptor("ok"),
            }))
            .expect("client function registers");
        assert_eq!(registry.len(), 1);
        assert!(registry.get("ok").is_some());
        assert_eq!(registry.descriptors()[0].name, "ok");
    }

    struct WriteProbe {
        name: &'static str,
        allowed_in_untrusted_workspace: bool,
        calls: Arc<AtomicU64>,
    }

    #[async_trait::async_trait]
    impl AgentTool for WriteProbe {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: self.name.into(),
                description: "write probe".into(),
                input_schema: json!({"type": "object"}),
                capability: ToolCapability::WorkspaceWrite,
                kind: ToolKind::ClientFunction,
                hosting: ToolHosting::Local,
                capabilities: Vec::new(),
                requires_approval: false,
                read_only: false,
                supports_concurrency: false,
                default_timeout_ms: None,
                max_output_bytes: 1024,
                allowed_in_untrusted_workspace: self.allowed_in_untrusted_workspace,
            }
        }

        async fn execute(
            &self,
            _request: ToolRequest,
            _context: ToolExecutionContext,
            _sink: &dyn ToolEventSink,
            _cancel: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult::success(Vec::new()))
        }
    }

    fn write_probe(
        name: &'static str,
        allowed_in_untrusted_workspace: bool,
    ) -> (Arc<dyn AgentTool>, Arc<AtomicU64>) {
        let calls = Arc::new(AtomicU64::new(0));
        (
            Arc::new(WriteProbe {
                name,
                allowed_in_untrusted_workspace,
                calls: calls.clone(),
            }),
            calls,
        )
    }

    fn policy_config(approval_mode: ApprovalMode, workspace_trusted: bool) -> ToolSchedulerConfig {
        ToolSchedulerConfig {
            max_concurrent: 4,
            approval_mode,
            workspace_trusted,
        }
    }

    #[tokio::test]
    async fn untrusted_write_tool_denied_even_in_never_ask() {
        let (tool, calls) = write_probe("blocked_write", false);
        let scheduler = make_scheduler(vec![tool], policy_config(ApprovalMode::NeverAsk, false));
        let result = scheduler
            .execute_named(
                "blocked_write",
                req("blocked_write", json!({"path": "a.txt"})),
                execution_context(),
                CancellationToken::new(),
                Some(&AutoApproveResolver),
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(result.is_error());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn ask_for_writes_cannot_bypass_with_auto_approve() {
        let (tool, calls) = write_probe("ask_write", false);
        let scheduler = make_scheduler(vec![tool], policy_config(ApprovalMode::AskForWrites, true));
        let result = scheduler
            .execute_named(
                "ask_write",
                req("ask_write", json!({"path": "a.txt"})),
                execution_context(),
                CancellationToken::new(),
                Some(&AutoApproveResolver),
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(result.is_error());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn read_only_mode_denies_write_even_when_trusted() {
        let (tool, calls) = write_probe("ro_write", false);
        let scheduler = make_scheduler(vec![tool], policy_config(ApprovalMode::ReadOnly, true));
        let result = scheduler
            .execute_named(
                "ro_write",
                req("ro_write", json!({"path": "a.txt"})),
                execution_context(),
                CancellationToken::new(),
                Some(&AutoApproveResolver),
                &NoopToolEventSink,
            )
            .await
            .unwrap();
        assert!(result.is_error());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn process_never_ask_trusted_injects_execution_constraints() {
        struct InputProbe {
            seen: Arc<StdMutex<Option<serde_json::Value>>>,
        }

        #[async_trait::async_trait]
        impl AgentTool for InputProbe {
            fn descriptor(&self) -> ToolDescriptor {
                ToolDescriptor {
                    name: "process_probe".into(),
                    description: "process probe".into(),
                    input_schema: json!({"type": "object"}),
                    capability: ToolCapability::Process,
                    kind: ToolKind::ClientFunction,
                    hosting: ToolHosting::Local,
                    capabilities: Vec::new(),
                    requires_approval: false,
                    read_only: false,
                    supports_concurrency: false,
                    default_timeout_ms: None,
                    max_output_bytes: 8 * 1024 * 1024,
                    allowed_in_untrusted_workspace: false,
                }
            }

            async fn execute(
                &self,
                request: ToolRequest,
                _context: ToolExecutionContext,
                _sink: &dyn ToolEventSink,
                _cancel: CancellationToken,
            ) -> Result<ToolResult, ToolError> {
                *self.seen.lock().expect("input") = Some(request.input);
                Ok(ToolResult::success(Vec::new()))
            }
        }

        let seen = Arc::new(StdMutex::new(None));
        let scheduler = make_scheduler(
            vec![Arc::new(InputProbe { seen: seen.clone() })],
            policy_config(ApprovalMode::NeverAsk, true),
        );
        scheduler
            .execute_named(
                "process_probe",
                req("process_probe", json!({"command": "echo hi"})),
                execution_context(),
                CancellationToken::new(),
                Some(&AutoApproveResolver),
                &NoopToolEventSink,
            )
            .await
            .unwrap();

        let input = seen.lock().expect("input").clone().expect("input seen");
        assert_eq!(input["timeout_ms"], 60_000);
        assert_eq!(input["max_output_bytes"], 1_048_576);
        assert_eq!(input["command"], "echo hi");
    }
}
