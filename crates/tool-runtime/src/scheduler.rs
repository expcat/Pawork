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
use tokio::sync::{Mutex, OwnedMutexGuard, Semaphore};
use tool_api::{
    AgentTool, ToolCapability, ToolDescriptor, ToolError, ToolErrorKind, ToolRequest, ToolResult,
};

/// 全局工具注册表：按工具名索引已注册的 [`AgentTool`]。
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Arc<HashMap<String, Arc<dyn AgentTool>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个工具（覆盖同名）。
    pub fn register(&mut self, tool: Arc<dyn AgentTool>) {
        let mut map = (*self.tools).clone();
        map.insert(tool.descriptor().name, tool);
        self.tools = Arc::new(map);
    }

    /// 批量注册。
    pub fn extend<I>(&mut self, tools: I)
    where
        I: IntoIterator<Item = Arc<dyn AgentTool>>,
    {
        let mut map = (*self.tools).clone();
        for tool in tools {
            map.insert(tool.descriptor().name, tool);
        }
        self.tools = Arc::new(map);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools.get(name).cloned()
    }

    /// 返回所有已注册工具的 descriptor（供 Provider 请求携带 tool 定义）。
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        let mut desc: Vec<_> = self.tools.values().map(|t| t.descriptor()).collect();
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
    /// 是否需要审批才执行写/Shell/网络类工具。
    pub require_approval_for_writes: bool,
}

impl Default for ToolSchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 8,
            require_approval_for_writes: true,
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
    async fn resolve(&self, requests: &[ToolRequest]) -> Vec<ApprovalOutcome>;
}

/// 默认全部自动放行（测试与无审批策略场景）。
#[derive(Debug, Default, Clone)]
pub struct AutoApproveResolver;

#[async_trait::async_trait]
impl ApprovalResolver for AutoApproveResolver {
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
        Self {
            registry,
            config,
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

    /// 调度并执行（按 request.input.name 查找工具）。
    pub async fn execute(
        &self,
        request: ToolRequest,
        cancel: CancellationToken,
        approval: &(dyn ApprovalResolver + Send + Sync),
    ) -> Result<ToolResult, ToolError> {
        let name = request
            .input
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from);
        let tool = match name.and_then(|n| self.registry.get(&n)) {
            Some(t) => t,
            None => {
                return Err(ToolError {
                    kind: ToolErrorKind::InvalidInput,
                    message: "tool name missing or unknown in request.input.name".into(),
                    retryable: false,
                    retry_after_ms: None,
                });
            }
        };
        self.execute_with_tool(tool, request, cancel, approval)
            .await
    }

    /// 按工具名调度并执行（推荐入口）。
    pub async fn execute_named(
        &self,
        name: &str,
        request: ToolRequest,
        cancel: CancellationToken,
        approval: &(dyn ApprovalResolver + Send + Sync),
    ) -> Result<ToolResult, ToolError> {
        let tool = self.registry.get(name).ok_or_else(|| ToolError {
            kind: ToolErrorKind::NotFound,
            message: format!("unknown tool: {name}"),
            retryable: false,
            retry_after_ms: None,
        })?;
        self.execute_with_tool(tool, request, cancel, approval)
            .await
    }

    async fn execute_with_tool(
        &self,
        tool: Arc<dyn AgentTool>,
        request: ToolRequest,
        cancel: CancellationToken,
        approval: &(dyn ApprovalResolver + Send + Sync),
    ) -> Result<ToolResult, ToolError> {
        let capability = tool.descriptor().capability.clone();

        // 审批：写/Shell/网络/Git 类工具需要审批（受配置控制）。
        if self.requires_approval(&capability) {
            let outcomes = approval.resolve(std::slice::from_ref(&request)).await;
            match outcomes.first() {
                Some(ApprovalOutcome::Denied) => {
                    return Ok(denied_result(&request.tool_call_id));
                }
                Some(ApprovalOutcome::Approved) | None => {}
            }
        }

        if cancel.is_cancelled() {
            return Err(ToolError::cancelled("tool cancelled before execution"));
        }

        // 获取调度锁。
        let handle = self.acquire(&capability, &request, &cancel).await?;

        let ctx = tool_api::ToolExecutionContext {
            workspace_id: agent_domain::WorkspaceId::from("default"),
            run_id: agent_domain::RunId::from("default"),
            working_directory: None,
        };
        let sink = NoopToolSink;
        let result = tool
            .execute(request.clone(), ctx, &sink, cancel.clone())
            .await;

        drop(handle);
        result
    }

    fn requires_approval(&self, capability: &ToolCapability) -> bool {
        if !self.config.require_approval_for_writes {
            return false;
        }
        matches!(
            capability,
            ToolCapability::WorkspaceWrite
                | ToolCapability::GitWrite
                | ToolCapability::Process
                | ToolCapability::Network
                | ToolCapability::ExternalPlugin
        )
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

fn denied_result(_tool_call_id: &ToolCallId) -> ToolResult {
    ToolResult::failure(agent_domain::ErrorContext {
        category: agent_domain::ErrorCategory::Authorization,
        message: "tool call denied by user".into(),
        retryable: false,
        retry_after_ms: None,
        diagnostics: Default::default(),
    })
}

struct NoopToolSink;

#[async_trait::async_trait]
impl tool_api::ToolEventSink for NoopToolSink {
    async fn emit(&self, _event: tool_api::ToolStreamEvent) -> Result<(), ToolError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use serde_json::json;

    fn make_scheduler(
        tools: Vec<Arc<dyn AgentTool>>,
        config: ToolSchedulerConfig,
    ) -> ToolScheduler {
        let mut registry = ToolRegistry::new();
        registry.extend(tools);
        ToolScheduler::new(registry, config)
    }

    fn req(name: &str, input: serde_json::Value) -> ToolRequest {
        ToolRequest {
            tool_call_id: ToolCallId::from(name),
            input,
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
                CancellationToken::new(),
                &AutoApproveResolver,
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

    #[async_trait::async_trait]
    impl AgentTool for ProbeTool {
        fn descriptor(&self) -> ToolDescriptor {
            let read_only = matches!(self.capability, ToolCapability::ReadOnly);
            ToolDescriptor {
                name: self.name.into(),
                description: "probe".into(),
                input_schema: json!({"type": "object"}),
                capability: self.capability.clone(),
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
                require_approval_for_writes: false,
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
                require_approval_for_writes: false,
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
                require_approval_for_writes: false,
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
                require_approval_for_writes: false,
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
                require_approval_for_writes: true,
            },
        );
        let result = scheduler
            .execute_named(
                "write_d",
                req("write_d", json!({})),
                CancellationToken::new(),
                &DenyAll,
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
                require_approval_for_writes: false,
            },
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = scheduler
            .execute_named(
                "read_x",
                req("read_x", json!({})),
                cancel,
                &AutoApproveResolver,
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
                CancellationToken::new(),
                &AutoApproveResolver,
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::NotFound);
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
                require_approval_for_writes: false,
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
}
