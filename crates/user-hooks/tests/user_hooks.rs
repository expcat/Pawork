//! P17-1 定向测试：六类 handler、policy 拒绝、async 不阻塞、P10-3 隔离、
//! secret 不泄露、PromptTransform 审计 diff。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use user_hooks::config::{
    AgentEvalHandler, BudgetLimit, CommandHandler, EvalFallback, HandlerConfig, HandlerLifecycle,
    HookConfig, HookScope, HttpHandler, McpToolHandler, PromptEvalHandler, PromptTarget,
    PromptTransformHandler,
};
use user_hooks::exec::{
    AsyncRunner, AuditSink, CommandExecutor, CommandRequest, CommandResult, ExecutorsBuilder,
    HttpExecutor, JudgeDecision, JudgeMode, JudgeRequest, McpToolInvoker, McpToolRequest,
    McpToolResult, PolicyAction, PolicyGate, PolicyOutcome, ProviderJudge, SecretResolver,
    WebhookRequest, WebhookResult,
};
use user_hooks::secret::{SecretRef, SecretValue};
use user_hooks::trigger::{TriggerPayload, TriggerPoint};
use user_hooks::{
    HookCapability, HookDispatcher, HookEffect, HookStatus, SystemHookClock, TransformRequest,
    TransformResult, UserHookEvent,
};

// —— 录制容器 ——

#[derive(Clone)]
struct Recorder<T: Clone + Send + 'static>(Arc<Mutex<Vec<T>>>);

impl<T: Clone + Send + 'static> Default for Recorder<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
}
impl<T: Clone + Send + 'static> Recorder<T> {
    fn snapshot(&self) -> Vec<T> {
        self.0.lock().unwrap().clone()
    }
    fn push(&self, v: T) {
        self.0.lock().unwrap().push(v);
    }
    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

// —— Mock 执行器 ——

struct AllowPolicy;
#[async_trait]
impl PolicyGate for AllowPolicy {
    async fn evaluate(&self, _action: PolicyAction<'_>) -> PolicyOutcome {
        PolicyOutcome::Allow
    }
}

struct TrustedAllowPolicy;
#[async_trait]
impl PolicyGate for TrustedAllowPolicy {
    async fn evaluate(&self, _action: PolicyAction<'_>) -> PolicyOutcome {
        PolicyOutcome::Allow
    }

    fn allows_eval_fail_open(&self, _workspace: Option<&agent_domain::WorkspaceId>) -> bool {
        true
    }
}

struct ErrJudge;
#[async_trait]
impl ProviderJudge for ErrJudge {
    async fn judge(
        &self,
        _request: JudgeRequest,
        _timeout: Option<std::time::Duration>,
    ) -> Result<JudgeDecision, user_hooks::HookError> {
        Err(user_hooks::HookError::executor("test", "judge failed"))
    }
}

struct DenyPolicy;
#[async_trait]
impl PolicyGate for DenyPolicy {
    async fn evaluate(&self, action: PolicyAction<'_>) -> PolicyOutcome {
        // PromptTransform 改写 system 且未显式 allow → 拒绝（不可绕过 security policy）。
        if action.capability == HookCapability::PromptTransform
            && matches!(action.prompt_target, Some(PromptTarget::System))
            && !action.allow_system_override
        {
            return PolicyOutcome::Deny {
                reason: "system prompt override not permitted".into(),
            };
        }
        // 其余一律拒绝（用于 policy 拒绝测试）。
        if action.capability == HookCapability::Process {
            return PolicyOutcome::Deny {
                reason: "command blocked by policy".into(),
            };
        }
        PolicyOutcome::Allow
    }
}

struct DenyJudgeTransformPolicy;
#[async_trait]
impl PolicyGate for DenyJudgeTransformPolicy {
    async fn evaluate(&self, action: PolicyAction<'_>) -> PolicyOutcome {
        if action.capability == HookCapability::PromptTransform {
            PolicyOutcome::Deny {
                reason: "judge transform rejected by post-policy".into(),
            }
        } else {
            PolicyOutcome::Allow
        }
    }
}

struct MockCommand {
    requests: Recorder<CommandRequest>,
    /// 收到的生效超时（用于验证 timeout 传递）。
    timeouts: Recorder<Option<std::time::Duration>>,
}
#[async_trait]
impl CommandExecutor for MockCommand {
    async fn run(
        &self,
        request: CommandRequest,
        timeout: Option<std::time::Duration>,
    ) -> Result<CommandResult, user_hooks::HookError> {
        self.requests.push(request);
        self.timeouts.push(timeout);
        Ok(CommandResult {
            exit_code: 0,
            stdout: "ok".into(),
            stderr: String::new(),
            timed_out: false,
        })
    }
}

struct MockHttp {
    requests: Recorder<WebhookRequest>,
    /// 收到的生效超时（用于验证 timeout 传递）。
    timeouts: Recorder<Option<std::time::Duration>>,
}
#[async_trait]
impl HttpExecutor for MockHttp {
    async fn send(
        &self,
        request: WebhookRequest,
        timeout: Option<std::time::Duration>,
    ) -> Result<WebhookResult, user_hooks::HookError> {
        self.requests.push(request);
        self.timeouts.push(timeout);
        Ok(WebhookResult {
            status: 200,
            body: "ok".into(),
            timed_out: false,
        })
    }
}

/// 录制每次策略裁决的 action（供 URL/动作描述 redaction 断言）。
struct RecordingPolicy(Arc<Mutex<Vec<PolicyAction<'static>>>>);
#[async_trait]
impl PolicyGate for RecordingPolicy {
    async fn evaluate(&self, action: PolicyAction<'_>) -> PolicyOutcome {
        // 描述已 redaction、可安全克隆留档（测试内 PolicyAction 字段均为
        // 非 secret 的 owned 值；hook_id/description 短暂借用）。
        let snapshot = PolicyAction {
            capability: action.capability,
            hook_id: "recorded",
            workspace_id: None,
            description: action.description.clone(),
            prompt_target: action.prompt_target,
            allow_system_override: action.allow_system_override,
        };
        self.0.lock().unwrap().push(snapshot);
        PolicyOutcome::Allow
    }
}

/// 可编程判定执行器：按 mode 返回不同决策。
struct MockJudge {
    single: Mutex<JudgeDecision>,
    agent: Mutex<JudgeDecision>,
    saw: Recorder<JudgeMode>,
}
#[async_trait]
impl ProviderJudge for MockJudge {
    async fn judge(
        &self,
        _request: JudgeRequest,
        _timeout: Option<std::time::Duration>,
    ) -> Result<JudgeDecision, user_hooks::HookError> {
        let mode = _request.mode;
        self.saw.push(mode);
        let decision = match mode {
            JudgeMode::SingleTurn => self.single.lock().unwrap().clone(),
            JudgeMode::ConstrainedAgent => self.agent.lock().unwrap().clone(),
        };
        Ok(decision)
    }
}

struct MockMcp {
    text: String,
    saw: Recorder<McpToolRequest>,
}
#[async_trait]
impl McpToolInvoker for MockMcp {
    async fn invoke(
        &self,
        request: McpToolRequest,
        _timeout: Option<std::time::Duration>,
    ) -> Result<McpToolResult, user_hooks::HookError> {
        self.saw.push(request);
        Ok(McpToolResult {
            success: true,
            text: self.text.clone(),
        })
    }
}

#[derive(Default)]
struct MockAudit(Arc<Mutex<Vec<UserHookEvent>>>);
#[async_trait]
impl AuditSink for MockAudit {
    async fn record(&self, event: UserHookEvent) {
        self.0.lock().unwrap().push(event);
    }
}

struct MapSecret(Arc<Mutex<std::collections::HashMap<String, String>>>);
impl SecretResolver for MapSecret {
    fn resolve(&self, reference: &SecretRef) -> Result<SecretValue, user_hooks::HookError> {
        self.0
            .lock()
            .unwrap()
            .get(reference.as_str())
            .cloned()
            .map(SecretValue::new)
            .ok_or_else(|| user_hooks::HookError::SecretUnavailable {
                hook_id: "test".into(),
            })
    }
}

/// 录制但不执行 future；测试可显式取回并驱动，验证 fire-and-forget。
type BoxedFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
#[derive(Clone)]
struct PendingAsync(Arc<Mutex<Vec<BoxedFuture>>>);

impl Default for PendingAsync {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
}
impl AsyncRunner for PendingAsync {
    fn spawn(&self, future: BoxedFuture) {
        self.0.lock().unwrap().push(future);
    }
}
impl PendingAsync {
    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
    async fn run_all(self) {
        let futures: Vec<_> = self.0.lock().unwrap().drain(..).collect();
        for f in futures {
            tokio::spawn(f).await.unwrap();
        }
    }
}

// —— 工具：构造 executors ——

struct Mocks {
    policy_allow: Arc<AllowPolicy>,
    policy_deny: Arc<DenyPolicy>,
    command: Arc<MockCommand>,
    http: Arc<MockHttp>,
    judge: Arc<MockJudge>,
    mcp: Arc<dyn McpToolInvoker>,
    audit: Arc<MockAudit>,
    /// 与 MockMcp 共享的调用录制（trait 对象不暴露 saw）。
    mcp_saw: Recorder<McpToolRequest>,
    secret: Arc<MapSecret>,
    pending: Arc<PendingAsync>,
    clock: Arc<SystemHookClock>,
}

fn setup_mocks() -> Mocks {
    let mcp_saw: Recorder<McpToolRequest> = Recorder::default();
    Mocks {
        policy_allow: Arc::new(AllowPolicy),
        policy_deny: Arc::new(DenyPolicy),
        command: Arc::new(MockCommand {
            requests: Recorder::default(),
            timeouts: Recorder::default(),
        }),
        http: Arc::new(MockHttp {
            requests: Recorder::default(),
            timeouts: Recorder::default(),
        }),
        judge: Arc::new(MockJudge {
            single: Mutex::new(JudgeDecision::Allow),
            agent: Mutex::new(JudgeDecision::Allow),
            saw: Recorder::default(),
        }),
        mcp: Arc::new(MockMcp {
            text: "allow".into(),
            saw: mcp_saw.clone(),
        }) as Arc<dyn McpToolInvoker>,
        mcp_saw,
        audit: Arc::new(MockAudit::default()),
        secret: Arc::new(MapSecret(Arc::new(Mutex::new(
            std::collections::HashMap::new(),
        )))),
        pending: Arc::new(PendingAsync::default()),
        clock: Arc::new(SystemHookClock::default()),
    }
}

fn executors(m: &Mocks, deny: bool) -> user_hooks::exec::Executors {
    ExecutorsBuilder::default()
        .policy(if deny {
            m.policy_deny.clone()
        } else {
            m.policy_allow.clone()
        })
        .command(m.command.clone())
        .http(m.http.clone())
        .judge(m.judge.clone())
        .mcp(m.mcp.clone())
        .audit(m.audit.clone())
        .secret(m.secret.clone())
        .async_runner(m.pending.clone())
        .clock(m.clock.clone())
        .build()
}

fn payload() -> TriggerPayload {
    TriggerPayload::builder()
        .prompt("please help")
        .system_prompt("system rules")
        .user_prompt("please help")
        .injected_prompt("")
        .details(serde_json::json!({"tool": "edit"}))
        .build()
}

// —— 测试：六类 handler 各自触发 ——

#[tokio::test]
async fn six_handlers_each_fire() {
    let m = setup_mocks();
    let exec = executors(&m, false);
    let mut d = HookDispatcher::new();

    d.register_config(HookConfig {
        id: "cmd".into(),
        trigger: TriggerPoint::PostToolUse,
        scope: HookScope::Global,
        lifecycle: Some(HandlerLifecycle::Sync), // 改同步以便直接观察 executor 调用
        enabled: true,
        handler: HandlerConfig::Command(CommandHandler {
            program: "echo".into(),
            args: vec!["hi".into()],
            allowed_env: vec![],
            env_secret_refs: vec![],
            working_directory: None,
            timeout_ms: None,
        }),
    })
    .unwrap();
    d.register_config(HookConfig {
        id: "http".into(),
        trigger: TriggerPoint::RunCompleted,
        scope: HookScope::Global,
        lifecycle: Some(HandlerLifecycle::Sync),
        enabled: true,
        handler: HandlerConfig::Http(HttpHandler {
            url: "https://example.com/hook".into(),
            method: "POST".into(),
            allowed_headers: vec![],
            header_secret_refs: vec![],
            body_template: Some("{trigger}".into()),
            timeout_ms: None,
        }),
    })
    .unwrap();
    d.register_config(HookConfig {
        id: "pt".into(),
        trigger: TriggerPoint::PromptAssembled,
        scope: HookScope::Global,
        lifecycle: None,
        enabled: true,
        handler: HandlerConfig::PromptTransform(PromptTransformHandler {
            target: PromptTarget::User,
            rewrite_kind: "prefix".into(),
            template: "BE_CONCISE".into(),
            allow_system_override: false,
        }),
    })
    .unwrap();
    d.register_config(HookConfig {
        id: "pe".into(),
        trigger: TriggerPoint::PreToolUse,
        scope: HookScope::Global,
        lifecycle: None,
        enabled: true,
        handler: HandlerConfig::PromptEval(PromptEvalHandler {
            prompt_template: "safe? {trigger}".into(),
            response_schema: None,
            on_failure: EvalFallback::Allow,
        }),
    })
    .unwrap();
    d.register_config(HookConfig {
        id: "ae".into(),
        trigger: TriggerPoint::PreToolUse,
        scope: HookScope::Global,
        lifecycle: None,
        enabled: true,
        handler: HandlerConfig::AgentEval(AgentEvalHandler {
            restricted_profile: "restricted".into(),
            tool_allowlist: vec!["read".into()],
            budget: Some(BudgetLimit {
                max_tokens: Some(100),
                timeout_ms: Some(500),
            }),
            prompt_template: "judge {trigger}".into(),
            on_failure: EvalFallback::Deny,
        }),
    })
    .unwrap();
    d.register_config(HookConfig {
        id: "mcp".into(),
        trigger: TriggerPoint::PermissionRequest,
        scope: HookScope::Global,
        lifecycle: None,
        enabled: true,
        handler: HandlerConfig::McpTool(McpToolHandler {
            server_id: "srv".into(),
            tool_name: "decide".into(),
            arg_template: Some(serde_json::json!({"x": 1})),
            on_failure: user_hooks::McpFallback::default(),
        }),
    })
    .unwrap();

    // Command
    let _ = d
        .dispatch(TriggerPoint::PostToolUse, &payload(), None, &exec)
        .await;
    // Http
    let _ = d
        .dispatch(TriggerPoint::RunCompleted, &payload(), None, &exec)
        .await;
    // PromptTransform
    let o = d
        .dispatch(TriggerPoint::PromptAssembled, &payload(), None, &exec)
        .await;
    // PromptEval + AgentEval (同 trigger)
    let o_eval = d
        .dispatch(TriggerPoint::PreToolUse, &payload(), None, &exec)
        .await;
    // McpTool
    let o_mcp = d
        .dispatch(TriggerPoint::PermissionRequest, &payload(), None, &exec)
        .await;

    assert_eq!(m.command.requests.len(), 1, "command handler ran once");
    assert_eq!(m.http.requests.len(), 1, "http handler ran once");
    assert_eq!(m.judge.saw.len(), 2, "prompt_eval + agent_eval judged");
    assert_eq!(m.mcp_saw.len(), 1, "mcp handler invoked once");

    // PromptTransform 回灌 + diff 审计。
    let transform = o
        .effects
        .iter()
        .find(|(id, _)| id == "pt")
        .and_then(|(_, e)| match e {
            HookEffect::Transform {
                new_prompt, diff, ..
            } => Some((new_prompt, diff)),
            _ => None,
        });
    let (new_prompt, diff) = transform.expect("prompt transform effect present");
    assert!(new_prompt.starts_with("BE_CONCISE"));
    assert!(new_prompt.contains("please help"));
    assert_eq!(diff.target, "User");
    assert!(!diff.before_digest.is_empty());
    assert_ne!(diff.before_digest, diff.after_digest);

    // Eval 两条决策都被回灌。
    assert_eq!(o_eval.effects.len(), 2);
    assert!(o_eval.effects.iter().any(|(id, _)| id == "pe"));
    assert!(o_eval.effects.iter().any(|(id, _)| id == "ae"));

    // McpTool 决策（默认 allow）。
    assert!(matches!(
        o_mcp.effects.as_slice(),
        [(_, HookEffect::Decision(JudgeDecision::Allow))]
    ));

    // 全部 handler 产出审计记录。
    assert!(m.audit.0.lock().unwrap().len() >= 6);
}

// —— 测试：policy 拒绝 ——

#[tokio::test]
async fn policy_denial_blocks_command_and_records_reason() {
    let m = setup_mocks();
    let exec = executors(&m, true); // DenyPolicy
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "blocked-cmd".into(),
        trigger: TriggerPoint::PostToolUse,
        scope: HookScope::Global,
        lifecycle: Some(HandlerLifecycle::Sync),
        enabled: true,
        handler: HandlerConfig::Command(CommandHandler {
            program: "rm".into(),
            args: vec!["-rf".into(), "/".into()],
            allowed_env: vec![],
            env_secret_refs: vec![],
            working_directory: None,
            timeout_ms: None,
        }),
    })
    .unwrap();

    let outcome = d
        .dispatch(TriggerPoint::PostToolUse, &payload(), None, &exec)
        .await;

    assert_eq!(
        m.command.requests.len(),
        0,
        "command must NOT run when denied"
    );
    assert_eq!(outcome.denied.len(), 1);
    assert_eq!(outcome.denied[0].0, "blocked-cmd");
    assert!(outcome.denied[0].1.contains("policy"));

    let rec = &m.audit.0.lock().unwrap()[0];
    assert!(matches!(rec.dispatch_status(), Some(HookStatus::Denied(_))));
}

// —— 测试：PromptTransform 改 system 在未授权时被 policy 拒绝（不可绕过 security policy）——

#[tokio::test]
async fn prompt_transform_system_override_blocked_without_allow() {
    let m = setup_mocks();
    let exec = executors(&m, true); // DenyPolicy 拒绝未授权 system override
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "sys-rewrite".into(),
        trigger: TriggerPoint::PromptAssembled,
        scope: HookScope::Global,
        lifecycle: None,
        enabled: true,
        handler: HandlerConfig::PromptTransform(PromptTransformHandler {
            target: PromptTarget::System,
            rewrite_kind: "replace".into(),
            template: "EVIL".into(),
            allow_system_override: false,
        }),
    })
    .unwrap();

    let outcome = d
        .dispatch(TriggerPoint::PromptAssembled, &payload(), None, &exec)
        .await;

    assert!(
        outcome.effects.is_empty(),
        "no transform effect when denied"
    );
    assert_eq!(outcome.denied.len(), 1);
}

#[tokio::test]
async fn prompt_transforms_use_each_targets_own_text_and_chain_without_double_write() {
    let m = setup_mocks();
    let exec = executors(&m, false);
    let mut dispatcher = HookDispatcher::new();
    for (id, target, rewrite_kind, template, allow_system_override) in [
        (
            "01-system",
            PromptTarget::System,
            "suffix",
            "SYS-SUFFIX",
            true,
        ),
        (
            "02-user-prefix",
            PromptTarget::User,
            "prefix",
            "USER-PREFIX",
            false,
        ),
        (
            "03-user-suffix",
            PromptTarget::User,
            "suffix",
            "USER-SUFFIX",
            false,
        ),
        (
            "04-injected",
            PromptTarget::Injected,
            "prefix",
            "INJECTED",
            false,
        ),
    ] {
        dispatcher
            .register_config(HookConfig {
                id: id.into(),
                trigger: TriggerPoint::PromptAssembled,
                scope: HookScope::Global,
                lifecycle: None,
                enabled: true,
                handler: HandlerConfig::PromptTransform(PromptTransformHandler {
                    target,
                    rewrite_kind: rewrite_kind.into(),
                    template: template.into(),
                    allow_system_override,
                }),
            })
            .unwrap();
    }

    let outcome = dispatcher
        .dispatch(TriggerPoint::PromptAssembled, &payload(), None, &exec)
        .await;
    assert_eq!(
        outcome.transformed_prompt("System", "system rules"),
        "system rules\nSYS-SUFFIX"
    );
    assert_eq!(
        outcome.transformed_prompt("User", "please help"),
        "USER-PREFIX\nplease help\nUSER-SUFFIX"
    );
    assert_eq!(outcome.transformed_prompt("Injected", ""), "INJECTED");
    assert_eq!(
        outcome
            .transformed_prompt("User", "please help")
            .matches("please help")
            .count(),
        1,
        "target original must not be written twice"
    );
    assert!(!outcome
        .transformed_prompt("User", "")
        .contains("system rules"));
}

#[tokio::test]
async fn judge_transform_is_reinjected_as_user_transform_after_second_policy_check() {
    let m = setup_mocks();
    *m.judge.single.lock().unwrap() = JudgeDecision::Transform {
        new_prompt: "safer user prompt".into(),
    };
    let exec = executors(&m, false);
    let mut dispatcher = HookDispatcher::new();
    dispatcher
        .register_config(HookConfig {
            id: "eval-transform".into(),
            trigger: TriggerPoint::PromptAssembled,
            scope: HookScope::Global,
            lifecycle: None,
            enabled: true,
            handler: HandlerConfig::PromptEval(PromptEvalHandler {
                prompt_template: "judge {trigger}".into(),
                response_schema: None,
                on_failure: EvalFallback::Deny,
            }),
        })
        .unwrap();

    let outcome = dispatcher
        .dispatch(TriggerPoint::PromptAssembled, &payload(), None, &exec)
        .await;
    assert!(!outcome.is_denied());
    assert_eq!(
        outcome.transformed_prompt("User", "please help"),
        "safer user prompt"
    );
    assert!(outcome.effects.iter().any(|(_, effect)| matches!(
        effect,
        HookEffect::Transform { target, .. } if target == "User"
    )));
}

#[tokio::test]
async fn judge_transform_denied_by_second_policy_check_blocks_without_transform_effect() {
    let m = setup_mocks();
    *m.judge.single.lock().unwrap() = JudgeDecision::Transform {
        new_prompt: "policy bypass attempt".into(),
    };
    let exec = ExecutorsBuilder::default()
        .policy(Arc::new(DenyJudgeTransformPolicy))
        .command(m.command.clone())
        .http(m.http.clone())
        .judge(m.judge.clone())
        .mcp(m.mcp.clone())
        .audit(m.audit.clone())
        .secret(m.secret.clone())
        .async_runner(m.pending.clone())
        .clock(m.clock.clone())
        .build();
    let mut dispatcher = HookDispatcher::new();
    dispatcher
        .register_config(HookConfig {
            id: "eval-transform-post-policy".into(),
            trigger: TriggerPoint::PromptAssembled,
            scope: HookScope::Global,
            lifecycle: None,
            enabled: true,
            handler: HandlerConfig::PromptEval(PromptEvalHandler {
                prompt_template: "judge".into(),
                response_schema: None,
                on_failure: EvalFallback::Deny,
            }),
        })
        .unwrap();

    let outcome = dispatcher
        .dispatch(TriggerPoint::PromptAssembled, &payload(), None, &exec)
        .await;
    assert!(outcome.is_denied());
    assert_eq!(outcome.denied.len(), 1);
    assert!(!outcome
        .effects
        .iter()
        .any(|(_, effect)| matches!(effect, HookEffect::Transform { .. })));
}

#[tokio::test]
async fn eval_error_is_fail_closed_unless_allow_is_explicit_and_policy_trusted() {
    fn dispatcher() -> HookDispatcher {
        let mut dispatcher = HookDispatcher::new();
        dispatcher
            .register_config(HookConfig {
                id: "eval-error".into(),
                trigger: TriggerPoint::PromptAssembled,
                scope: HookScope::Global,
                lifecycle: None,
                enabled: true,
                handler: HandlerConfig::PromptEval(PromptEvalHandler {
                    prompt_template: "judge".into(),
                    response_schema: None,
                    on_failure: EvalFallback::Allow,
                }),
            })
            .unwrap();
        dispatcher
    }

    let m = setup_mocks();
    let untrusted = ExecutorsBuilder::default()
        .policy(Arc::new(AllowPolicy))
        .command(m.command.clone())
        .http(m.http.clone())
        .judge(Arc::new(ErrJudge))
        .mcp(m.mcp.clone())
        .audit(m.audit.clone())
        .secret(m.secret.clone())
        .async_runner(m.pending.clone())
        .clock(m.clock.clone())
        .build();
    assert!(dispatcher()
        .dispatch(TriggerPoint::PromptAssembled, &payload(), None, &untrusted)
        .await
        .is_denied());

    let trusted = ExecutorsBuilder::default()
        .policy(Arc::new(TrustedAllowPolicy))
        .command(m.command.clone())
        .http(m.http.clone())
        .judge(Arc::new(ErrJudge))
        .mcp(m.mcp.clone())
        .audit(m.audit.clone())
        .secret(m.secret.clone())
        .async_runner(m.pending.clone())
        .clock(m.clock.clone())
        .build();
    assert!(!dispatcher()
        .dispatch(TriggerPoint::PromptAssembled, &payload(), None, &trusted)
        .await
        .is_denied());
}

#[test]
fn agent_eval_rejects_missing_or_partial_budget_at_registration() {
    for budget in [
        None,
        Some(BudgetLimit {
            max_tokens: Some(10),
            timeout_ms: None,
        }),
    ] {
        let config = HookConfig {
            id: "bounded-agent".into(),
            trigger: TriggerPoint::PromptAssembled,
            scope: HookScope::Global,
            lifecycle: None,
            enabled: true,
            handler: HandlerConfig::AgentEval(AgentEvalHandler {
                restricted_profile: "restricted".into(),
                tool_allowlist: vec![],
                budget,
                prompt_template: "judge".into(),
                on_failure: EvalFallback::Deny,
            }),
        };
        assert!(user_hooks::HookHandler::from_config(config).is_err());
    }
}

// —— 测试：async 不阻塞主循环 ——

#[tokio::test]
async fn async_command_does_not_block_run_loop() {
    let m = setup_mocks();
    let exec = executors(&m, false);
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "notify".into(),
        trigger: TriggerPoint::RunCompleted,
        scope: HookScope::Global,
        lifecycle: None, // Command 默认 Async
        enabled: true,
        handler: HandlerConfig::Command(CommandHandler {
            program: "notify-send".into(),
            args: vec![],
            allowed_env: vec![],
            env_secret_refs: vec![],
            working_directory: None,
            timeout_ms: None,
        }),
    })
    .unwrap();

    let outcome = d
        .dispatch(TriggerPoint::RunCompleted, &payload(), None, &exec)
        .await;

    // dispatch 立即返回；命令尚未执行（future 仍 pending）。
    assert_eq!(outcome.fired_async, vec!["notify".to_string()]);
    assert_eq!(
        m.command.requests.len(),
        0,
        "async command must not run synchronously"
    );
    assert_eq!(m.pending.count(), 1, "one future spawned");

    // 投递的 future 包含成功审计；驱动后命令执行 + 审计补记。
    // （fire-and-forget 语义：失败仅记录，不阻断。）
    let pending = (*m.pending).clone();
    pending.run_all().await;
    assert_eq!(
        m.command.requests.len(),
        1,
        "command ran after future driven"
    );
}

// —— 测试：PromptEval 经 mock provider 判定 deny ——

#[tokio::test]
async fn prompt_eval_can_deny_run() {
    let m = setup_mocks();
    *m.judge.single.lock().unwrap() = JudgeDecision::Deny {
        reason: "unsafe".into(),
    };
    let exec = executors(&m, false);
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "gate".into(),
        trigger: TriggerPoint::PreToolUse,
        scope: HookScope::Global,
        lifecycle: None,
        enabled: true,
        handler: HandlerConfig::PromptEval(PromptEvalHandler {
            prompt_template: "ok?".into(),
            response_schema: None,
            on_failure: EvalFallback::Allow,
        }),
    })
    .unwrap();

    let outcome = d
        .dispatch(TriggerPoint::PreToolUse, &payload(), None, &exec)
        .await;
    assert!(outcome.is_denied(), "deny decision should deny the run");
    assert!(
        outcome.denied.is_empty(),
        "judge deny is represented as an effect"
    );
}

// —— 测试：AgentEval 用受限 profile 走 ConstrainedAgent 模式 ——

#[tokio::test]
async fn agent_eval_uses_constrained_agent_mode() {
    let m = setup_mocks();
    *m.judge.agent.lock().unwrap() = JudgeDecision::Deny {
        reason: "agent veto".into(),
    };
    let exec = executors(&m, false);
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "agent-gate".into(),
        trigger: TriggerPoint::TaskStarted,
        scope: HookScope::Global,
        lifecycle: None,
        enabled: true,
        handler: HandlerConfig::AgentEval(AgentEvalHandler {
            restricted_profile: "limited".into(),
            tool_allowlist: vec!["read".into()],
            budget: Some(BudgetLimit {
                max_tokens: Some(50),
                timeout_ms: Some(500),
            }),
            prompt_template: "decide".into(),
            on_failure: EvalFallback::Deny,
        }),
    })
    .unwrap();

    let outcome = d
        .dispatch(TriggerPoint::TaskStarted, &payload(), None, &exec)
        .await;

    assert_eq!(m.judge.saw.snapshot(), vec![JudgeMode::ConstrainedAgent]);
    assert!(outcome.is_denied());
}

// —— 测试：McpTool deny 文本 → 决策 ——

#[tokio::test]
async fn mcp_tool_deny_text_maps_to_deny() {
    let mut m = setup_mocks();
    m.mcp = Arc::new(MockMcp {
        text: "deny: tool not approved".into(),
        saw: Recorder::default(),
    });
    let exec = executors(&m, false);
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "mcp-gate".into(),
        trigger: TriggerPoint::PermissionRequest,
        scope: HookScope::Global,
        lifecycle: None,
        enabled: true,
        handler: HandlerConfig::McpTool(McpToolHandler {
            server_id: "srv".into(),
            tool_name: "approve".into(),
            arg_template: None,
            on_failure: user_hooks::McpFallback::default(),
        }),
    })
    .unwrap();

    let outcome = d
        .dispatch(TriggerPoint::PermissionRequest, &payload(), None, &exec)
        .await;
    assert!(outcome.is_denied());
}

// —— 测试：secret 不泄露到审计，但正确注入执行器 ——

#[tokio::test]
async fn secret_redacted_from_audit_but_injected_to_executor() {
    let mut m = setup_mocks();
    m.secret = Arc::new(MapSecret(Arc::new(Mutex::new(
        std::collections::HashMap::from([(
            "API_KEY".to_string(),
            "sk-super-secret-12345".to_string(),
        )]),
    ))));
    // 用全新 command recorder 替换，避免借用冲突。
    let command = Arc::new(MockCommand {
        requests: Recorder::default(),
        timeouts: Recorder::default(),
    });
    let exec = ExecutorsBuilder::default()
        .policy(m.policy_allow.clone())
        .command(command.clone())
        .http(m.http.clone())
        .judge(m.judge.clone())
        .mcp(m.mcp.clone())
        .audit(m.audit.clone())
        .secret(m.secret.clone())
        .async_runner(m.pending.clone())
        .clock(m.clock.clone())
        .build();
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "secret-cmd".into(),
        trigger: TriggerPoint::PostToolUse,
        scope: HookScope::Global,
        lifecycle: Some(HandlerLifecycle::Sync),
        enabled: true,
        handler: HandlerConfig::Command(CommandHandler {
            program: "deploy".into(),
            args: vec![],
            allowed_env: vec!["API_KEY_ENV".into()],
            env_secret_refs: vec![SecretRef::new("API_KEY")],
            working_directory: None,
            timeout_ms: None,
        }),
    })
    .unwrap();

    let outcome = d
        .dispatch(TriggerPoint::PostToolUse, &payload(), None, &exec)
        .await;
    let _ = outcome;

    // 执行器确实拿到明文 env。
    let reqs = command.requests.snapshot();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].env[0].0, "API_KEY_ENV");
    assert_eq!(reqs[0].env[0].1.as_str(), "sk-super-secret-12345");

    // 审计记录里绝不出现明文。
    let records = m.audit.0.lock().unwrap().clone();
    assert!(!records.is_empty());
    for r in &records {
        let json = serde_json::to_string(r).expect("audit serializable");
        assert!(
            !json.contains("sk-super-secret-12345"),
            "secret leaked into audit: {json}"
        );
    }
}

// —— 测试：与 P10-3 隔离 ——

#[tokio::test]
async fn dispatcher_has_independent_registry_and_distinct_trigger_vocabulary() {
    // 本 crate 不依赖 hook-runtime；registry 独立、初始为空。
    let d = HookDispatcher::new();
    assert!(d.registry().is_empty());

    // TriggerPoint 词汇与 P10-3 PluginLifecycleEventKind 不同（独立枚举类型）。
    // 这里只验证 user hook 自己的词汇完整且可序列化。
    for tp in TriggerPoint::ALL {
        let json = serde_json::to_string(tp).expect("trigger serializable");
        let back: TriggerPoint = serde_json::from_str(&json).expect("trigger deserializable");
        assert_eq!(tp, &back);
    }

    // scope 隔离：workspace 级 hook 不在别的 workspace 触发。
    use agent_domain::WorkspaceId;
    let mut d2 = HookDispatcher::new();
    let ws_a = WorkspaceId::new("ws-a");
    let ws_b = WorkspaceId::new("ws-b");
    d2.register_config(HookConfig {
        id: "scoped".into(),
        trigger: TriggerPoint::RunStarted,
        scope: HookScope::Workspace {
            workspace_id: ws_a.clone(),
        },
        lifecycle: Some(HandlerLifecycle::Sync),
        enabled: true,
        handler: HandlerConfig::Http(HttpHandler {
            url: "https://example.com".into(),
            method: "POST".into(),
            allowed_headers: vec![],
            header_secret_refs: vec![],
            body_template: None,
            timeout_ms: None,
        }),
    })
    .unwrap();

    let m = setup_mocks();
    let exec = executors(&m, false);
    let in_other = d2
        .dispatch(TriggerPoint::RunStarted, &payload(), Some(&ws_b), &exec)
        .await;
    let in_scope = d2
        .dispatch(TriggerPoint::RunStarted, &payload(), Some(&ws_a), &exec)
        .await;
    assert_eq!(
        m.http.requests.len(),
        1,
        "only the in-scope workspace triggers"
    );
    assert!(in_other.effects.is_empty());
    assert_eq!(in_scope.effects.len(), 1);
}

// —— 测试：lifecycle 默认值 ——

#[test]
fn lifecycle_defaults_by_capability() {
    use user_hooks::HookHandler;
    let mk = |handler: HandlerConfig| {
        HookHandler::from_config(HookConfig {
            id: "x".into(),
            trigger: TriggerPoint::Notification,
            scope: HookScope::Global,
            lifecycle: None,
            enabled: true,
            handler,
        })
        .unwrap()
        .lifecycle
    };
    assert_eq!(
        mk(HandlerConfig::Command(CommandHandler {
            program: "e".into(),
            args: vec![],
            allowed_env: vec![],
            env_secret_refs: vec![],
            working_directory: None,
            timeout_ms: None,
        })),
        HandlerLifecycle::Async
    );
    assert_eq!(
        mk(HandlerConfig::PromptEval(PromptEvalHandler {
            prompt_template: "p".into(),
            response_schema: None,
            on_failure: EvalFallback::Allow,
        })),
        HandlerLifecycle::Sync
    );
}

// —— 占位实现：保持 trait 对象完整（未使用的 transform 类型）——
#[allow(dead_code)]
fn _types_present(_a: TransformRequest, _b: TransformResult) {}

// —— Finding 1：与 P10-3 共享 canonical trigger 词汇（一一映射 contract）——

#[test]
fn shared_vocabulary_maps_one_to_one_to_plugin_lifecycle_kind() {
    use plugin_api::PluginLifecycleEventKind;
    use std::collections::HashSet;
    use user_hooks::TriggerPoint;

    // 重叠点 1-1 映射（单射），扩展点 None。
    let mut targets: HashSet<PluginLifecycleEventKind> = HashSet::new();
    let mut shared = 0usize;
    let mut extensions = 0usize;
    for tp in TriggerPoint::ALL {
        match tp.to_lifecycle_kind() {
            Some(kind) => {
                shared += 1;
                assert!(targets.insert(kind), "non-injective mapping at {tp:?}");
            }
            None => extensions += 1,
        }
    }
    assert!(
        shared >= 8,
        "expected >=8 shared canonical points, got {shared}"
    );
    assert!(
        extensions >= 9,
        "expected >=9 P17 extensions, got {extensions}"
    );

    // 显式 1-1 断言（user hook 点 ↔ canonical kind）。
    assert_eq!(
        TriggerPoint::SessionStart.to_lifecycle_kind(),
        Some(PluginLifecycleEventKind::SessionOpen)
    );
    assert_eq!(
        TriggerPoint::SessionEnd.to_lifecycle_kind(),
        Some(PluginLifecycleEventKind::SessionClose)
    );
    assert_eq!(
        TriggerPoint::RunStarted.to_lifecycle_kind(),
        Some(PluginLifecycleEventKind::RunStart)
    );
    assert_eq!(
        TriggerPoint::RunCompleted.to_lifecycle_kind(),
        Some(PluginLifecycleEventKind::RunEnd)
    );
    assert_eq!(
        TriggerPoint::PromptAssembled.to_lifecycle_kind(),
        Some(PluginLifecycleEventKind::ContextBuild)
    );
    assert_eq!(
        TriggerPoint::PreToolUse.to_lifecycle_kind(),
        Some(PluginLifecycleEventKind::ToolCall)
    );
    assert_eq!(
        TriggerPoint::PostToolUse.to_lifecycle_kind(),
        Some(PluginLifecycleEventKind::ToolResult)
    );
    assert_eq!(
        TriggerPoint::PreCompact.to_lifecycle_kind(),
        Some(PluginLifecycleEventKind::Compaction)
    );

    // P17 专有扩展（无 canonical 等价）。
    for ext in [
        TriggerPoint::RunFailed,
        TriggerPoint::ToolFailed,
        TriggerPoint::PermissionRequest,
        TriggerPoint::SubagentStart,
        TriggerPoint::SubagentStop,
        TriggerPoint::TaskStarted,
        TriggerPoint::TaskCompleted,
        TriggerPoint::PostCompact,
        TriggerPoint::Notification,
    ] {
        assert_eq!(ext.to_lifecycle_kind(), None, "{ext:?} should be extension");
    }
}

// —— Finding 2：UserHookEvent versioned envelope 可 serde replay ——

#[test]
fn user_hook_event_round_trips_with_schema_version() {
    use agent_domain::{EventId, Timestamp};
    use user_hooks::{
        PromptTransformDiff, UserHookEvent, UserHookEventPayload, USER_HOOK_EVENT_SCHEMA_VERSION,
    };

    let dispatch = UserHookEvent::new(
        EventId::from("evt-1"),
        Timestamp::from_unix_millis(42),
        "hook-1".into(),
        TriggerPoint::PostToolUse,
        user_hooks::HookScope::Global,
        "Process".into(),
        "Sync".into(),
        UserHookEventPayload::Dispatch {
            status: HookStatus::Success,
            duration_ms: 7,
            summary: Some("ok".into()),
        },
    );
    let transform = UserHookEvent::new(
        EventId::from("evt-2"),
        Timestamp::from_unix_millis(43),
        "hook-2".into(),
        TriggerPoint::PromptAssembled,
        user_hooks::HookScope::Global,
        "PromptTransform".into(),
        "Sync".into(),
        UserHookEventPayload::Transform {
            diff: PromptTransformDiff::new("User", "a", "b", false),
            duration_ms: 3,
        },
    );

    for evt in [dispatch, transform] {
        let json = serde_json::to_string(&evt).expect("serialize");
        let back: UserHookEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, evt);
        assert_eq!(back.schema_version, USER_HOOK_EVENT_SCHEMA_VERSION);
        // envelope 字段持久化。
        assert!(json.contains("\"schema_version\""));
        assert!(json.contains("\"event_id\""));
    }
}

// —— Finding 2：AuditSink 接收的是 canonical UserHookEvent ——

#[tokio::test]
async fn audit_sink_receives_canonical_user_hook_event() {
    let m = setup_mocks();
    let exec = executors(&m, false);
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "evt-check".into(),
        trigger: TriggerPoint::PostToolUse,
        scope: HookScope::Global,
        lifecycle: Some(HandlerLifecycle::Sync),
        enabled: true,
        handler: HandlerConfig::Command(CommandHandler {
            program: "echo".into(),
            args: vec![],
            allowed_env: vec![],
            env_secret_refs: vec![],
            working_directory: None,
            timeout_ms: None,
        }),
    })
    .unwrap();

    let outcome = d
        .dispatch(TriggerPoint::PostToolUse, &payload(), None, &exec)
        .await;

    let sink_records = m.audit.0.lock().unwrap().clone();
    assert_eq!(sink_records.len(), 1, "sink received the canonical event");
    assert_eq!(sink_records[0].hook_id, "evt-check");
    assert_eq!(sink_records[0].schema_version, 1);
    assert_eq!(
        sink_records[0].dispatch_status(),
        Some(&HookStatus::Success)
    );
    // outcome 里的审计与 sink 一致。
    assert_eq!(outcome.audit.len(), 1);
    assert_eq!(outcome.audit[0], sink_records[0]);
}

// —— Finding 4：McpTool 默认 fail-closed（success=false → Deny；invoke error → Deny）——

#[tokio::test]
async fn mcp_tool_failure_is_fail_closed_by_default() {
    let mut m = setup_mocks();
    // success=false 的 mock。
    struct FailMcp;
    #[async_trait]
    impl McpToolInvoker for FailMcp {
        async fn invoke(
            &self,
            _request: McpToolRequest,
            _timeout: Option<std::time::Duration>,
        ) -> Result<McpToolResult, user_hooks::HookError> {
            Ok(McpToolResult {
                success: false,
                text: String::new(),
            })
        }
    }
    m.mcp = Arc::new(FailMcp) as Arc<dyn McpToolInvoker>;
    let exec = executors(&m, false);
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "mcp-fail".into(),
        trigger: TriggerPoint::PermissionRequest,
        scope: HookScope::Global,
        lifecycle: None,
        enabled: true,
        handler: HandlerConfig::McpTool(McpToolHandler {
            server_id: "srv".into(),
            tool_name: "decide".into(),
            arg_template: None,
            on_failure: user_hooks::McpFallback::default(), // Deny
        }),
    })
    .unwrap();

    let outcome = d
        .dispatch(TriggerPoint::PermissionRequest, &payload(), None, &exec)
        .await;
    assert!(
        outcome.is_denied(),
        "default fail-closed must deny on failure"
    );
}

#[tokio::test]
async fn mcp_tool_invoke_error_default_is_deny_but_allow_when_configured() {
    use user_hooks::HookError;
    // invoke error + 默认 Deny。
    struct ErrMcp;
    #[async_trait]
    impl McpToolInvoker for ErrMcp {
        async fn invoke(
            &self,
            _request: McpToolRequest,
            _timeout: Option<std::time::Duration>,
        ) -> Result<McpToolResult, HookError> {
            Err(HookError::executor("mcp-err", "boom"))
        }
    }
    let mut m = setup_mocks();
    m.mcp = Arc::new(ErrMcp) as Arc<dyn McpToolInvoker>;
    let exec = executors(&m, false);
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "mcp-err-deny".into(),
        trigger: TriggerPoint::PermissionRequest,
        scope: HookScope::Global,
        lifecycle: None,
        enabled: true,
        handler: HandlerConfig::McpTool(McpToolHandler {
            server_id: "srv".into(),
            tool_name: "t".into(),
            arg_template: None,
            on_failure: user_hooks::McpFallback::Deny,
        }),
    })
    .unwrap();
    let denied = d
        .dispatch(TriggerPoint::PermissionRequest, &payload(), None, &exec)
        .await;
    assert!(denied.is_denied());

    // invoke error + 显式 Allow。
    let mut m2 = setup_mocks();
    m2.mcp = Arc::new(ErrMcp) as Arc<dyn McpToolInvoker>;
    let exec2 = executors(&m2, false);
    let mut d2 = HookDispatcher::new();
    d2.register_config(HookConfig {
        id: "mcp-err-allow".into(),
        trigger: TriggerPoint::PermissionRequest,
        scope: HookScope::Global,
        lifecycle: None,
        enabled: true,
        handler: HandlerConfig::McpTool(McpToolHandler {
            server_id: "srv".into(),
            tool_name: "t".into(),
            arg_template: None,
            on_failure: user_hooks::McpFallback::Allow,
        }),
    })
    .unwrap();
    let allowed = d2
        .dispatch(TriggerPoint::PermissionRequest, &payload(), None, &exec2)
        .await;
    assert!(
        !allowed.is_denied(),
        "explicit Allow fallback must not deny"
    );
}

// —— Finding 5：async 持久化 queued 与 terminal 两条 UserHookEvent ——

#[tokio::test]
async fn async_persists_queued_and_terminal_events_to_sink() {
    let m = setup_mocks();
    let exec = executors(&m, false);
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "async-notify".into(),
        trigger: TriggerPoint::RunCompleted,
        scope: HookScope::Global,
        lifecycle: None, // Command 默认 Async
        enabled: true,
        handler: HandlerConfig::Command(CommandHandler {
            program: "notify".into(),
            args: vec![],
            allowed_env: vec![],
            env_secret_refs: vec![],
            working_directory: None,
            timeout_ms: None,
        }),
    })
    .unwrap();

    // dispatch 立即返回；queued 已持久化到 sink。
    let outcome = d
        .dispatch(TriggerPoint::RunCompleted, &payload(), None, &exec)
        .await;
    assert_eq!(outcome.fired_async, vec!["async-notify".to_string()]);

    let after_dispatch = m.audit.0.lock().unwrap().clone();
    assert_eq!(
        after_dispatch.len(),
        1,
        "queued event persisted to AuditSink immediately"
    );
    let queued_summary = match &after_dispatch[0].payload {
        user_hooks::UserHookEventPayload::Dispatch { summary, .. } => summary.clone(),
        _ => None,
    };
    assert_eq!(
        queued_summary.as_deref(),
        Some("queued for async execution")
    );

    // 驱动 future → terminal 事件持久化。
    (*m.pending).clone().run_all().await;
    let after_terminal = m.audit.0.lock().unwrap().clone();
    assert_eq!(after_terminal.len(), 2, "queued + terminal both persisted");
    // 两条都是 canonical UserHookEvent，schema_version 一致。
    for evt in &after_terminal {
        assert_eq!(evt.schema_version, 1);
        assert_eq!(evt.hook_id, "async-notify");
    }
    assert_eq!(
        m.command.requests.len(),
        1,
        "command ran in terminal future"
    );
}

// —— Finding 3：SecretString 不实现 Debug 泄露 / 请求字段 Drop 清零 ——

#[test]
fn secret_string_debug_never_leaks_plaintext() {
    use user_hooks::SecretString;
    let s = SecretString::new("sk-super-secret-12345");
    let dbg = format!("{s:?}");
    assert!(
        !dbg.contains("sk-super-secret-12345"),
        "Debug leaked: {dbg}"
    );
    assert!(dbg.contains("REDACTED"));
}

#[tokio::test]
async fn secret_in_request_field_is_zeroizing_wrapper() {
    // 验证 CommandRequest.env 携带 SecretString（Drop 清零），且明文可达。
    let mut m = setup_mocks();
    m.secret = Arc::new(MapSecret(Arc::new(Mutex::new(
        std::collections::HashMap::from([("TOKEN".to_string(), "plaintext-token-xyz".to_string())]),
    ))));
    let command = Arc::new(MockCommand {
        requests: Recorder::default(),
        timeouts: Recorder::default(),
    });
    let exec = ExecutorsBuilder::default()
        .policy(m.policy_allow.clone())
        .command(command.clone())
        .http(m.http.clone())
        .judge(m.judge.clone())
        .mcp(m.mcp.clone())
        .audit(m.audit.clone())
        .secret(m.secret.clone())
        .async_runner(m.pending.clone())
        .clock(m.clock.clone())
        .build();
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "tok".into(),
        trigger: TriggerPoint::PostToolUse,
        scope: HookScope::Global,
        lifecycle: Some(HandlerLifecycle::Sync),
        enabled: true,
        handler: HandlerConfig::Command(CommandHandler {
            program: "p".into(),
            args: vec![],
            allowed_env: vec!["TOK_ENV".into()],
            env_secret_refs: vec![SecretRef::new("TOKEN")],
            working_directory: None,
            timeout_ms: None,
        }),
    })
    .unwrap();
    let _ = d
        .dispatch(TriggerPoint::PostToolUse, &payload(), None, &exec)
        .await;
    let reqs = command.requests.snapshot();
    assert_eq!(reqs[0].env[0].1.as_str(), "plaintext-token-xyz");
}

// —— 回归：Command/HTTP 超时 ——

#[tokio::test]
async fn command_and_http_timeout_are_recorded_and_receive_effective_timeout() {
    let m = setup_mocks();
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "cmd-timeout".into(),
        trigger: TriggerPoint::PostToolUse,
        scope: HookScope::Global,
        lifecycle: Some(HandlerLifecycle::Sync),
        enabled: true,
        handler: HandlerConfig::Command(CommandHandler {
            program: "slow".into(),
            args: vec![],
            allowed_env: vec![],
            env_secret_refs: vec![],
            working_directory: None,
            timeout_ms: Some(75),
        }),
    })
    .unwrap();
    d.register_config(HookConfig {
        id: "http-timeout".into(),
        trigger: TriggerPoint::RunCompleted,
        scope: HookScope::Global,
        lifecycle: Some(HandlerLifecycle::Sync),
        enabled: true,
        handler: HandlerConfig::Http(HttpHandler {
            url: "https://example.com/hook".into(),
            method: "POST".into(),
            allowed_headers: vec![],
            header_secret_refs: vec![],
            body_template: None,
            timeout_ms: Some(120),
        }),
    })
    .unwrap();

    // 用“超时结果”变体：执行器返回 timed_out=true（Err(Timeout) 同样收敛为
    // Timeout 状态，已在 dispatcher 分支覆盖）。
    let timed_command = Arc::new(TimedOutCommand {
        inner: m.command.clone(),
    });
    let timed_http = Arc::new(TimedOutHttp {
        inner: m.http.clone(),
    });
    let exec = ExecutorsBuilder::default()
        .policy(m.policy_allow.clone())
        .command(timed_command)
        .http(timed_http)
        .judge(m.judge.clone())
        .mcp(m.mcp.clone())
        .audit(m.audit.clone())
        .secret(m.secret.clone())
        .async_runner(m.pending.clone())
        .clock(m.clock.clone())
        .build();

    let o = d
        .dispatch(TriggerPoint::PostToolUse, &payload(), None, &exec)
        .await;
    let o_http = d
        .dispatch(TriggerPoint::RunCompleted, &payload(), None, &exec)
        .await;

    // 生效超时确实传给了执行器。
    assert_eq!(
        m.command.timeouts.snapshot(),
        vec![Some(std::time::Duration::from_millis(75))]
    );
    assert_eq!(
        m.http.timeouts.snapshot(),
        vec![Some(std::time::Duration::from_millis(120))]
    );
    // 审计状态收敛为 Timeout，且不因超时产出失败审计之外的错误。
    assert!(matches!(
        o.audit[0].dispatch_status(),
        Some(HookStatus::Timeout)
    ));
    assert!(matches!(
        o_http.audit[0].dispatch_status(),
        Some(HookStatus::Timeout)
    ));
}

/// 执行器侧超时变体：返回 `timed_out=true` 的结果。
struct TimedOutCommand {
    inner: Arc<MockCommand>,
}
#[async_trait]
impl CommandExecutor for TimedOutCommand {
    async fn run(
        &self,
        request: CommandRequest,
        timeout: Option<std::time::Duration>,
    ) -> Result<CommandResult, user_hooks::HookError> {
        self.inner.run(request, timeout).await.map(|mut result| {
            result.timed_out = true;
            result
        })
    }
}

/// 执行器侧超时变体：返回 `timed_out=true` 的结果。
struct TimedOutHttp {
    inner: Arc<MockHttp>,
}
#[async_trait]
impl HttpExecutor for TimedOutHttp {
    async fn send(
        &self,
        request: WebhookRequest,
        timeout: Option<std::time::Duration>,
    ) -> Result<WebhookResult, user_hooks::HookError> {
        self.inner.send(request, timeout).await.map(|mut result| {
            result.timed_out = true;
            result
        })
    }
}

// —— 回归：Http URL / body 的 secret redaction ——

#[tokio::test]
async fn http_url_and_body_secrets_are_redacted_in_policy_and_audit() {
    const PLAIN: &str = "sk-http-secret-999";
    let mut m = setup_mocks();
    m.secret = Arc::new(MapSecret(Arc::new(Mutex::new(
        std::collections::HashMap::from([("WEBHOOK_KEY".to_string(), PLAIN.to_string())]),
    ))));
    let recorded: Arc<Mutex<Vec<PolicyAction<'static>>>> = Arc::default();
    let exec = ExecutorsBuilder::default()
        .policy(Arc::new(RecordingPolicy(Arc::clone(&recorded))))
        .command(m.command.clone())
        .http(m.http.clone())
        .judge(m.judge.clone())
        .mcp(m.mcp.clone())
        .audit(m.audit.clone())
        .secret(m.secret.clone())
        .async_runner(m.pending.clone())
        .clock(m.clock.clone())
        .build();
    let mut d = HookDispatcher::new();
    d.register_config(HookConfig {
        id: "http-redact".into(),
        trigger: TriggerPoint::RunCompleted,
        scope: HookScope::Global,
        lifecycle: Some(HandlerLifecycle::Sync),
        enabled: true,
        handler: HandlerConfig::Http(HttpHandler {
            // URL query 携带明文 secret（模拟配置内嵌 token）。
            url: format!("https://hooks.example.com/notify?token={PLAIN}&a=b"),
            method: "POST".into(),
            allowed_headers: vec!["X-Api-Key".into()],
            header_secret_refs: vec![SecretRef::new("WEBHOOK_KEY")],
            body_template: Some(format!("event={{trigger}} key={PLAIN}")),
            timeout_ms: None,
        }),
    })
    .unwrap();

    let o = d
        .dispatch(TriggerPoint::RunCompleted, &payload(), None, &exec)
        .await;

    // 策略裁决描述：URL 的 query/fragment 被遮蔽，且不含明文。
    let actions = recorded.lock().unwrap().clone();
    assert_eq!(actions.len(), 1);
    let url_in_policy = actions[0]
        .description
        .get("url")
        .and_then(|v| v.as_str())
        .expect("policy description carries url");
    assert!(
        url_in_policy.starts_with("https://hooks.example.com/notify?***REDACTED***"),
        "url query must be masked: {url_in_policy}"
    );
    assert!(!url_in_policy.contains(PLAIN));

    // 执行器拿到的 body 已 redaction（模板渲染后），明文不进入请求。
    let requests = m.http.requests.snapshot();
    assert_eq!(requests.len(), 1);
    let body = requests[0].body.as_deref().expect("body sent");
    assert!(!body.contains(PLAIN), "body leaked plaintext: {body}");
    // URL 本身必须保留真实值供执行器使用（secret 保护发生在审计/策略层）。
    assert!(requests[0].url.contains(PLAIN));

    // 审计记录全程无明文。
    assert!(!o.audit.is_empty());
    for event in &o.audit {
        let json = serde_json::to_string(event).expect("audit serializable");
        assert!(!json.contains(PLAIN), "audit leaked plaintext: {json}");
    }
}
