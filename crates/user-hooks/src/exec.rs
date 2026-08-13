//! 依赖注入的执行器接口（P17-1 步骤 2、4-7）。
//!
//! 本 crate **不直接依赖** policy-engine / http-runtime / process-runtime /
//! sandbox-runtime / provider-api / mcp-client。所有外部执行经这里定义的
//! trait 由消费层（app-service）注入实现：
//! - [`PolicyGate`]：策略裁决（接线 policy-engine）；
//! - [`CommandExecutor`]：Command handler 的 Sandbox→Process 执行器
//!   （接线 sandbox-runtime → process-runtime）；
//! - [`HttpExecutor`]：Http handler 的 webhook 执行器（接线 http-runtime）；
//! - [`ProviderJudge`]：PromptEval / AgentEval 的 canonical 模型判定
//!   （接线 provider-api，不按 Provider 名分支）；
//! - [`McpToolInvoker`]：McpTool handler 的 MCP 调用（接线 mcp-client，P9-5）；
//! - [`AuditSink`]：审计事件持久化；
//! - [`SecretResolver`]：运行时 secret 解析；
//! - [`AsyncRunner`]：async fire-and-forget 的投递。
//!
//! 这些 trait 让 user-hooks 在隔离单测中可用 mock 实现驱动，且运行时不被
//! 任何具体 Provider / 平台绑定。

use crate::audit::UserHookEvent;
use crate::capability::HookCapability;
use crate::config::{BudgetLimit, EvalFallback, PromptTarget};
use crate::error::HookError;
use crate::secret::{SecretRef, SecretString, SecretValue};
use crate::trigger::TriggerPayload;
use agent_domain::{EventId, RunId, Timestamp, WorkspaceId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// 注入执行器时携带的执行上下文（hook id、trigger、payload、超时）。
#[derive(Clone, Debug)]
pub struct ExecContext<'a> {
    pub hook_id: &'a str,
    pub capability: HookCapability,
    pub trigger_payload: &'a TriggerPayload,
    /// 该 handler 生效的超时（同步模式）；执行器应据此裁剪执行。
    pub timeout: Option<Duration>,
}

/// 策略裁决结果（与 policy-engine 的 PolicyDecision 解耦的 hook 视图）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolicyOutcome {
    Allow,
    Deny { reason: String },
    AllowWithConstraints { timeout_ms: Option<u64> },
}

impl PolicyOutcome {
    pub fn is_allowed(&self) -> bool {
        !matches!(self, Self::Deny { .. })
    }
}

/// 一次 hook 动作请求交给策略门裁决。
#[derive(Clone, Debug)]
pub struct PolicyAction<'a> {
    pub capability: HookCapability,
    pub hook_id: &'a str,
    /// 当前动作所属 workspace；缺失或宿主无法解析 trust 时必须按 untrusted。
    pub workspace_id: Option<&'a WorkspaceId>,
    /// 已 redaction 的动作描述（命令、URL、tool 名等；非 secret）。
    pub description: Value,
    /// 对 PromptTransform：目标；其他能力为 None。
    pub prompt_target: Option<PromptTarget>,
    /// 是否显式请求 system override（仅 PromptTransform 有意义）。
    pub allow_system_override: bool,
}

/// 策略门：裁决一次 hook 动作是否可执行。app-service 接线 policy-engine。
#[async_trait]
pub trait PolicyGate: Send + Sync {
    async fn evaluate(&self, action: PolicyAction<'_>) -> PolicyOutcome;

    /// 显式 fail-open eval fallback 是否可用于该 workspace。默认 false；生产
    /// 实现只可在真实 workspace trust 为 trusted 时返回 true。
    fn allows_eval_fail_open(&self, _workspace: Option<&WorkspaceId>) -> bool {
        false
    }
}

/// Command 执行请求（已渲染、secret 明文仅在此结构内存活）。
#[derive(Clone, Debug)]
pub struct CommandRequest {
    pub program: String,
    pub args: Vec<String>,
    /// 允许注入的环境变量（名→明文值 wrapper）；仅 allowlisted 项。
    /// 明文用 [`SecretString`]，请求 Drop 时所有副本清零。
    pub env: Vec<(String, SecretString)>,
    pub working_directory: Option<String>,
}

/// Command 执行结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_code: i32,
    /// 已 redaction 的 stdout（执行器负责按 secret 明文 redact）。
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Command handler 的执行器。**必须**接线 Sandbox Runtime → Process Runtime；
/// 实现方不得在 trait 实现内直接 `tokio::process::Command::spawn`。
#[async_trait]
pub trait CommandExecutor: Send + Sync {
    async fn run(
        &self,
        request: CommandRequest,
        timeout: Option<Duration>,
    ) -> Result<CommandResult, HookError>;
}

/// Http 执行请求。
#[derive(Clone, Debug)]
pub struct WebhookRequest {
    pub url: String,
    pub method: String,
    /// allowlisted header（名→明文值 wrapper）。明文用 [`SecretString`]，Drop 清零。
    pub headers: Vec<(String, SecretString)>,
    pub body: Option<String>,
}

/// Http 执行结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebhookResult {
    pub status: u16,
    pub body: String,
    pub timed_out: bool,
}

#[async_trait]
pub trait HttpExecutor: Send + Sync {
    async fn send(
        &self,
        request: WebhookRequest,
        timeout: Option<Duration>,
    ) -> Result<WebhookResult, HookError>;
}

/// PromptTransform 改写请求。
#[derive(Clone, Debug)]
pub struct TransformRequest {
    pub target: PromptTarget,
    pub rewrite_kind: String,
    /// 渲染后的改写内容（已 redaction）。
    pub rendered_template: String,
    /// 原始 prompt（已 redaction）。
    pub original_prompt: String,
}

/// PromptTransform 改写结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformResult {
    /// 改写后的 prompt（已 redaction）。
    pub new_prompt: String,
}

/// 模型 / Agent 判定的请求（PromptEval 与 AgentEval 共用）。
#[derive(Clone, Debug)]
pub struct JudgeRequest {
    /// 判定模式：单轮模型判定 vs 受限 Agent 判定。
    pub mode: JudgeMode,
    /// 判定所属 workspace；受限 profile 与 fail-open policy 必须据此解析，
    /// 缺失时按未知 / untrusted 处理。
    pub workspace_id: Option<WorkspaceId>,
    /// 渲染后的判定 prompt（已 redaction）。
    pub prompt: String,
    /// 期望响应 schema（PromptEval）。
    pub response_schema: Option<Value>,
    /// 受限 profile（AgentEval）。
    pub restricted_profile: Option<String>,
    /// 工具 allowlist（AgentEval）。
    pub tool_allowlist: Vec<String>,
    /// 预算（AgentEval）。
    pub budget: Option<BudgetLimit>,
}

/// 判定模式。trait 实现据此选择 provider 单轮调用或受限 Agent 执行，
/// **不按 Provider 名分支**。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JudgeMode {
    /// 单轮模型判定（PromptEval）。
    SingleTurn,
    /// 受限 Agent 判定（AgentEval）。
    ConstrainedAgent,
}

/// 判定结果：允许 / 阻断 / 改写。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JudgeDecision {
    Allow,
    Deny { reason: String },
    Transform { new_prompt: String },
}

/// PromptEval / AgentEval 判定执行器。接线 canonical provider-api（P15-8）。
#[async_trait]
pub trait ProviderJudge: Send + Sync {
    async fn judge(
        &self,
        request: JudgeRequest,
        timeout: Option<Duration>,
    ) -> Result<JudgeDecision, HookError>;
}

/// MCP tool 调用请求。
#[derive(Clone, Debug)]
pub struct McpToolRequest {
    pub server_id: String,
    pub tool_name: String,
    pub arguments: Value,
    /// 触发时的 workspace / run 上下文（来自 TriggerPayload；McpTool 执行器
    /// 用它构造工具执行上下文，缺失时回退宿主静态上下文）。
    pub workspace_id: Option<WorkspaceId>,
    pub run_id: Option<RunId>,
}

/// MCP tool 调用结果（已 redaction）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpToolResult {
    pub success: bool,
    /// 判定意图：McpTool 作为同步 handler 时，结果文本经解析得到决策。
    pub text: String,
}

/// McpTool handler 执行器。接线 mcp-client；P9-5 每 server 审批由实现承担。
#[async_trait]
pub trait McpToolInvoker: Send + Sync {
    async fn invoke(
        &self,
        request: McpToolRequest,
        timeout: Option<Duration>,
    ) -> Result<McpToolResult, HookError>;
}

/// 运行时 secret 解析。明文 [`SecretValue`] 仅在调用方作用域内存活。
pub trait SecretResolver: Send + Sync {
    fn resolve(&self, reference: &SecretRef) -> Result<SecretValue, HookError>;
}

/// async fire-and-forget 投递器。实现可用 tokio::spawn / 队列 / 线程池；
/// user-hooks 自身不绑定具体异步运行时。
pub trait AsyncRunner: Send + Sync {
    /// 投递一个 future，立即返回。失败仅由 future 内部记录审计。
    fn spawn(&self, future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>);
}

/// 审计事件持久化 sink。记录前 dispatcher 已对 secret redaction。
#[async_trait]
pub trait AuditSink: Send + Sync {
    async fn record(&self, event: UserHookEvent);
}

/// 审计记录所需的时间戳 / 事件 id 生成器。便于测试注入确定性时钟。
pub trait HookClock: Send + Sync {
    fn now(&self) -> Timestamp;
    fn next_event_id(&self) -> EventId;
}

/// 基于系统时钟与原子计数器的默认实现。
///
/// **事件 id 跨重启 / 并发唯一**：`SystemHookClock` 在构造时捕获本进程的
/// `pid` 与高精度启动时刻，事件 id 为 `hook-event-{pid}-{boot_nanos}-{n}`。
/// 不同进程（并发宿主）pid 不同；同一进程重启后 boot_nanos 不同；同一时钟
/// 实例内 `n` 单调递增——三者组合保证同一审计库中跨重启、跨进程不碰撞，
/// 碰撞时 `AuditSink` 的按 event_id 去重不会误吞真实事件。
pub struct SystemHookClock {
    counter: std::sync::atomic::AtomicU64,
    boot_nonce: String,
}

impl Default for SystemHookClock {
    fn default() -> Self {
        Self {
            counter: std::sync::atomic::AtomicU64::new(0),
            boot_nonce: format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_nanos())
                    .unwrap_or(0)
            ),
        }
    }
}

impl HookClock for SystemHookClock {
    fn now(&self) -> Timestamp {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Timestamp::from_unix_millis(ms)
    }
    fn next_event_id(&self) -> EventId {
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        EventId::from(format!("hook-event-{}-{n}", self.boot_nonce))
    }
}

/// 已注入的全部执行器集合。dispatcher 在派发时持有它；async handler 的
/// spawned future 通过 clone `Arc` 捕获所需执行器（'static）。
#[derive(Clone)]
pub struct Executors {
    pub policy: Arc<dyn PolicyGate>,
    pub command: Arc<dyn CommandExecutor>,
    pub http: Arc<dyn HttpExecutor>,
    pub judge: Arc<dyn ProviderJudge>,
    pub mcp: Arc<dyn McpToolInvoker>,
    pub audit: Arc<dyn AuditSink>,
    pub secret: Arc<dyn SecretResolver>,
    pub async_runner: Arc<dyn AsyncRunner>,
    pub clock: Arc<dyn HookClock>,
}

impl Executors {
    pub fn builder() -> ExecutorsBuilder {
        ExecutorsBuilder::default()
    }
}

/// [`Executors`] 构建器。
#[derive(Default)]
pub struct ExecutorsBuilder {
    pub policy: Option<Arc<dyn PolicyGate>>,
    pub command: Option<Arc<dyn CommandExecutor>>,
    pub http: Option<Arc<dyn HttpExecutor>>,
    pub judge: Option<Arc<dyn ProviderJudge>>,
    pub mcp: Option<Arc<dyn McpToolInvoker>>,
    pub audit: Option<Arc<dyn AuditSink>>,
    pub secret: Option<Arc<dyn SecretResolver>>,
    pub async_runner: Option<Arc<dyn AsyncRunner>>,
    pub clock: Option<Arc<dyn HookClock>>,
}

impl ExecutorsBuilder {
    pub fn policy(mut self, v: Arc<dyn PolicyGate>) -> Self {
        self.policy = Some(v);
        self
    }
    pub fn command(mut self, v: Arc<dyn CommandExecutor>) -> Self {
        self.command = Some(v);
        self
    }
    pub fn http(mut self, v: Arc<dyn HttpExecutor>) -> Self {
        self.http = Some(v);
        self
    }
    pub fn judge(mut self, v: Arc<dyn ProviderJudge>) -> Self {
        self.judge = Some(v);
        self
    }
    pub fn mcp(mut self, v: Arc<dyn McpToolInvoker>) -> Self {
        self.mcp = Some(v);
        self
    }
    pub fn audit(mut self, v: Arc<dyn AuditSink>) -> Self {
        self.audit = Some(v);
        self
    }
    pub fn secret(mut self, v: Arc<dyn SecretResolver>) -> Self {
        self.secret = Some(v);
        self
    }
    pub fn async_runner(mut self, v: Arc<dyn AsyncRunner>) -> Self {
        self.async_runner = Some(v);
        self
    }
    pub fn clock(mut self, v: Arc<dyn HookClock>) -> Self {
        self.clock = Some(v);
        self
    }

    pub fn build(self) -> Executors {
        Executors {
            policy: self.policy.expect("policy gate required"),
            command: self.command.expect("command executor required"),
            http: self.http.expect("http executor required"),
            judge: self.judge.expect("provider judge required"),
            mcp: self.mcp.expect("mcp invoker required"),
            audit: self.audit.expect("audit sink required"),
            secret: self.secret.expect("secret resolver required"),
            async_runner: self.async_runner.expect("async runner required"),
            clock: self.clock.expect("hook clock required"),
        }
    }
}

/// Eval 失败/超时降级帮助：按 [`EvalFallback`] 推断决策。
pub fn fallback_decision(on_failure: EvalFallback, hook_id: &str) -> JudgeDecision {
    match on_failure {
        EvalFallback::Allow => JudgeDecision::Allow,
        EvalFallback::Deny => JudgeDecision::Deny {
            reason: format!("eval hook {hook_id} failed and policy is deny"),
        },
        EvalFallback::SafeTransform => JudgeDecision::Transform {
            new_prompt: String::new(),
        },
    }
}
