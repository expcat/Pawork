//! Hook Dispatcher（P17-1 步骤 2、4-8）。
//!
//! 按 trigger point 派发匹配的 user hook，统一处理：
//! - secret 解析（短生命周期，[`crate::secret::SecretValue`] 用后清零）；
//! - 模板渲染 + 全程 redaction；
//! - 策略门控（每条 handler 一次裁决）；
//! - 同步阻断 vs async fire-and-forget；
//! - 超时降级与失败审计；
//! - 与 P10-3 lifecycle hook 派发器互不干扰（独立 registry / dispatcher）。
//!
//! 审计：所有派发产出 canonical、versioned 的 [`crate::audit::UserHookEvent`]，
//! 同步与 async（queued + terminal 两条）均持久化到注入的
//! [`crate::exec::AuditSink`]。

use crate::audit::{
    DispatchOutcome, HookEffect, PromptTransformDiff, UserHookEvent, UserHookEventPayload,
};
use crate::capability::HookCapability;
use crate::config::{
    EvalFallback, HandlerConfig, HandlerLifecycle, HookConfig, HookScope, McpFallback,
    PromptTarget, RenderContext,
};
use crate::error::{HookError, HookStatus};
use crate::exec::{
    CommandRequest, Executors, HookClock, JudgeDecision, JudgeMode, JudgeRequest, McpToolRequest,
    PolicyAction, PolicyOutcome, WebhookRequest,
};
use crate::handler::{HookHandler, HookId};
use crate::registry::TriggerRegistry;
use crate::secret::{redact, SecretValue};
use crate::trigger::{TriggerPayload, TriggerPoint};
use agent_domain::WorkspaceId;
use serde_json::Value;
use std::time::{Duration, Instant};

/// User Hook 派发器。持有独立 trigger registry；与 `hook-runtime`（P10-3）互不调用。
pub struct HookDispatcher {
    registry: TriggerRegistry,
}

impl Default for HookDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl HookDispatcher {
    pub fn new() -> Self {
        Self {
            registry: TriggerRegistry::new(),
        }
    }

    pub fn with_registry(registry: TriggerRegistry) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &TriggerRegistry {
        &self.registry
    }

    pub fn register(&mut self, handler: HookHandler) -> Result<(), HookError> {
        self.registry.register(handler)
    }

    /// 从用户配置解析并注册。
    pub fn register_config(&mut self, config: HookConfig) -> Result<(), HookError> {
        let handler = HookHandler::from_config(config)?;
        self.registry.register(handler)
    }

    pub fn unregister(&mut self, id: &HookId) -> Result<(), HookError> {
        self.registry.unregister(id)
    }

    /// 派发一次 trigger。同步 handler 的结果回灌进 [`DispatchOutcome`]；
    /// async handler 经 `Executors::async_runner` 投递后立即返回，不阻塞 run loop。
    pub async fn dispatch(
        &self,
        trigger: TriggerPoint,
        payload: &TriggerPayload,
        workspace: Option<&WorkspaceId>,
        exec: &Executors,
    ) -> DispatchOutcome {
        let mut outcome = DispatchOutcome::default();
        let matching: Vec<&HookHandler> = self
            .registry
            .handlers_for(trigger)
            .into_iter()
            .filter(|h| h.matches(trigger, workspace))
            .collect();
        for handler in matching {
            Self::dispatch_one(handler, trigger, payload, exec, &mut outcome).await;
        }
        outcome
    }

    async fn dispatch_one(
        handler: &HookHandler,
        trigger: TriggerPoint,
        payload: &TriggerPayload,
        exec: &Executors,
        outcome: &mut DispatchOutcome,
    ) {
        let started = Instant::now();
        let hook_id = handler.id.as_str().to_string();

        // 1) secret 解析（短生命周期，用后随作用域 drop 清零）。失败 → 审计并跳过，
        //    不泄露引用细节。
        let secrets_owned = match resolve_secrets(handler, &*exec.secret) {
            Ok(v) => v,
            Err(_) => {
                record(
                    outcome,
                    exec,
                    &hook_id,
                    trigger,
                    &handler.scope,
                    handler.capability,
                    handler.lifecycle,
                    Some(HookStatus::Failed(
                        "a required secret is unavailable".into(),
                    )),
                    started,
                    None,
                    None,
                )
                .await;
                return;
            }
        };
        let secret_refs: Vec<&SecretValue> = secrets_owned.iter().collect();

        // 2) 渲染 + redaction。
        let trigger_json = redact(
            &serde_json::to_string(payload).unwrap_or_default(),
            &secret_refs,
        );
        let details_json = payload
            .details
            .as_ref()
            .map(|v| redact(&v.to_string(), &secret_refs))
            .unwrap_or_default();
        let render_ctx = RenderContext {
            trigger_json,
            details_json,
            secrets: secret_refs.clone(),
            vars: Default::default(),
        };

        // 3) 策略门控。
        let policy_desc = describe_action(&handler.config.handler, &secret_refs);
        let (prompt_target, allow_system_override) = match handler.config.handler {
            HandlerConfig::PromptTransform(ref p) => (Some(p.target), p.allow_system_override),
            _ => (None, false),
        };
        let action = PolicyAction {
            capability: handler.capability,
            hook_id: &hook_id,
            workspace_id: payload.workspace_id.as_ref(),
            description: policy_desc,
            prompt_target,
            allow_system_override,
        };
        let decision = exec.policy.evaluate(action).await;
        if let PolicyOutcome::Deny { reason } = &decision {
            outcome.denied.push((hook_id.clone(), reason.clone()));
            record(
                outcome,
                exec,
                &hook_id,
                trigger,
                &handler.scope,
                handler.capability,
                handler.lifecycle,
                Some(HookStatus::Denied(reason.clone())),
                started,
                None,
                None,
            )
            .await;
            return;
        }
        let handler_timeout = handler.config.handler.timeout_ms();
        let effective_timeout = match decision {
            PolicyOutcome::AllowWithConstraints { timeout_ms } => {
                handler_timeout.or(timeout_ms).map(Duration::from_millis)
            }
            _ => handler_timeout.map(Duration::from_millis),
        };

        // 4) 执行（按 lifecycle 区分同步 / async）。
        match handler.capability {
            HookCapability::Process => {
                let req = build_command_request(&handler.config.handler, &render_ctx);
                if handler.lifecycle == HandlerLifecycle::Async {
                    spawn_async(
                        exec,
                        handler,
                        trigger,
                        AsyncJob::Command(req, effective_timeout),
                        outcome,
                    )
                    .await;
                    outcome.fired_async.push(hook_id);
                } else {
                    let (status, summary) = match exec.command.run(req, effective_timeout).await {
                        Ok(result) if result.timed_out => {
                            (HookStatus::Timeout, Some("command timed out".into()))
                        }
                        Ok(result) => (
                            HookStatus::Success,
                            Some(format!("exit_code={}", result.exit_code)),
                        ),
                        Err(HookError::Timeout { .. }) => {
                            (HookStatus::Timeout, Some("command timed out".into()))
                        }
                        Err(HookError::Cancelled { .. }) => (HookStatus::Cancelled, None),
                        Err(e) => (HookStatus::Failed(e.to_string()), None),
                    };
                    record(
                        outcome,
                        exec,
                        &hook_id,
                        trigger,
                        &handler.scope,
                        handler.capability,
                        handler.lifecycle,
                        Some(status),
                        started,
                        summary,
                        None,
                    )
                    .await;
                    outcome.push_effect(hook_id, HookEffect::None);
                }
            }
            HookCapability::Network => {
                let req = build_webhook_request(&handler.config.handler, &render_ctx);
                if handler.lifecycle == HandlerLifecycle::Async {
                    spawn_async(
                        exec,
                        handler,
                        trigger,
                        AsyncJob::Http(req, effective_timeout),
                        outcome,
                    )
                    .await;
                    outcome.fired_async.push(hook_id);
                } else {
                    let (status, summary) = match exec.http.send(req, effective_timeout).await {
                        Ok(result) if result.timed_out => {
                            (HookStatus::Timeout, Some("http request timed out".into()))
                        }
                        Ok(result) => (
                            HookStatus::Success,
                            Some(format!("status={}", result.status)),
                        ),
                        Err(HookError::Timeout { .. }) => {
                            (HookStatus::Timeout, Some("http request timed out".into()))
                        }
                        Err(HookError::Cancelled { .. }) => (HookStatus::Cancelled, None),
                        Err(e) => (HookStatus::Failed(e.to_string()), None),
                    };
                    record(
                        outcome,
                        exec,
                        &hook_id,
                        trigger,
                        &handler.scope,
                        handler.capability,
                        handler.lifecycle,
                        Some(status),
                        started,
                        summary,
                        None,
                    )
                    .await;
                    outcome.push_effect(hook_id, HookEffect::None);
                }
            }
            HookCapability::PromptTransform => {
                let target = prompt_target.expect("PromptTransform target validated above");
                let original_prompt = current_target_prompt(payload, outcome, target, &secret_refs);
                let effect =
                    apply_prompt_transform(&handler.config.handler, &render_ctx, &original_prompt);
                let diff = match &effect {
                    HookEffect::Transform { diff, .. } => Some(diff.clone()),
                    _ => None,
                };
                record(
                    outcome,
                    exec,
                    &hook_id,
                    trigger,
                    &handler.scope,
                    handler.capability,
                    handler.lifecycle,
                    None,
                    started,
                    None,
                    diff,
                )
                .await;
                outcome.push_effect(hook_id, effect);
            }
            HookCapability::PromptEval | HookCapability::AgentEval => {
                let mode = if handler.capability == HookCapability::AgentEval {
                    JudgeMode::ConstrainedAgent
                } else {
                    JudgeMode::SingleTurn
                };
                let req = build_judge_request(&handler.config.handler, mode, &render_ctx, payload);
                let configured_fallback = eval_fallback(&handler.config.handler);
                let fallback = if configured_fallback == EvalFallback::Deny
                    || exec
                        .policy
                        .allows_eval_fail_open(payload.workspace_id.as_ref())
                {
                    configured_fallback
                } else {
                    EvalFallback::Deny
                };
                let (status, decided) = match exec.judge.judge(req, effective_timeout).await {
                    Ok(d) => (HookStatus::Success, d),
                    Err(HookError::PolicyDenied { reason, .. }) => (
                        HookStatus::Denied(reason.clone()),
                        JudgeDecision::Deny { reason },
                    ),
                    Err(_) => {
                        let fb = crate::exec::fallback_decision(fallback, &hook_id);
                        (
                            HookStatus::Failed("eval failed; applied fallback".into()),
                            fb,
                        )
                    }
                };
                record(
                    outcome,
                    exec,
                    &hook_id,
                    trigger,
                    &handler.scope,
                    handler.capability,
                    handler.lifecycle,
                    Some(status),
                    started,
                    None,
                    None,
                )
                .await;
                apply_judge_decision(
                    outcome,
                    exec,
                    &hook_id,
                    trigger,
                    &handler.scope,
                    handler.lifecycle,
                    started,
                    payload,
                    &secret_refs,
                    decided,
                )
                .await;
            }
            HookCapability::McpTool => {
                let req = build_mcp_request(&handler.config.handler, &render_ctx, payload);
                let fallback = mcp_fallback(&handler.config.handler);
                let (status, decided) = match exec.mcp.invoke(req, effective_timeout).await {
                    Ok(r) if r.success => (HookStatus::Success, parse_mcp_decision(&r.text)),
                    Ok(_) => (
                        HookStatus::Failed("mcp tool reported failure".into()),
                        fallback_decision(fallback, "mcp tool reported failure"),
                    ),
                    Err(_) => (
                        HookStatus::Failed("mcp invoke failed".into()),
                        fallback_decision(fallback, "mcp invoke failed"),
                    ),
                };
                record(
                    outcome,
                    exec,
                    &hook_id,
                    trigger,
                    &handler.scope,
                    handler.capability,
                    handler.lifecycle,
                    Some(status),
                    started,
                    None,
                    None,
                )
                .await;
                apply_judge_decision(
                    outcome,
                    exec,
                    &hook_id,
                    trigger,
                    &handler.scope,
                    handler.lifecycle,
                    started,
                    payload,
                    &secret_refs,
                    decided,
                )
                .await;
            }
        }
    }
}

// —— secret 解析 ——

/// 按 handler 配置中的 secret 引用顺序解析明文。任一失败统一归一为
/// `SecretUnavailable`，避免泄露具体引用。
fn resolve_secrets(
    handler: &HookHandler,
    resolver: &dyn crate::exec::SecretResolver,
) -> Result<Vec<SecretValue>, HookError> {
    let refs: Vec<&crate::secret::SecretRef> = match handler.config.handler {
        HandlerConfig::Command(ref c) => c.env_secret_refs.iter().collect(),
        HandlerConfig::Http(ref h) => h.header_secret_refs.iter().collect(),
        _ => return Ok(Vec::new()),
    };
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        if resolver.resolve(r).is_err() {
            return Err(HookError::SecretUnavailable {
                hook_id: handler.id.as_str().to_string(),
            });
        }
        // resolve 返回 SecretValue；这里取出放入容器（明文副本由 Zeroizing 管理）。
        out.push(resolver.resolve(r).expect("resolved above"));
    }
    Ok(out)
}

// —— 渲染 / 描述 ——

fn render_template(template: &str, ctx: &RenderContext) -> String {
    let mut out = template
        .replace("{trigger}", &ctx.trigger_json)
        .replace("{details}", &ctx.details_json);
    for (k, v) in &ctx.vars {
        out = out.replace(&format!("{{var:{k}}}"), v);
    }
    redact(&out, &ctx.secrets)
}

fn describe_action(handler: &HandlerConfig, secrets: &[&SecretValue]) -> Value {
    let desc = match handler {
        HandlerConfig::Command(c) => serde_json::json!({
            "program": c.program,
            "args": c.args,
            "working_directory": c.working_directory,
        }),
        HandlerConfig::Http(h) => serde_json::json!({
            "url": crate::secret::redact_url(&h.url),
            "method": h.method,
        }),
        HandlerConfig::PromptTransform(p) => serde_json::json!({
            "target": format!("{:?}", p.target),
            "rewrite_kind": p.rewrite_kind,
            "allow_system_override": p.allow_system_override,
        }),
        HandlerConfig::PromptEval(_) | HandlerConfig::AgentEval(_) => {
            serde_json::json!({ "mode": "eval" })
        }
        HandlerConfig::McpTool(m) => serde_json::json!({
            "server_id": m.server_id,
            "tool_name": m.tool_name,
        }),
    };
    crate::secret::redact_value(&desc, secrets)
}

// —— 请求构造（含明文注入：allowlist 位置与 ctx.secrets 对齐）——

fn build_command_request(handler: &HandlerConfig, ctx: &RenderContext) -> CommandRequest {
    let HandlerConfig::Command(c) = handler else {
        return CommandRequest {
            program: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            working_directory: None,
        };
    };
    // ctx.secrets 与 env_secret_refs(=allowed_env) 位置对齐（见 resolve_secrets）。
    let env = c
        .allowed_env
        .iter()
        .zip(ctx.secrets.iter())
        .map(|(name, sv)| (name.clone(), sv.to_secret_string()))
        .collect();
    CommandRequest {
        program: c.program.clone(),
        args: c.args.clone(),
        env,
        working_directory: c.working_directory.clone(),
    }
}

fn build_webhook_request(handler: &HandlerConfig, ctx: &RenderContext) -> WebhookRequest {
    let HandlerConfig::Http(h) = handler else {
        return WebhookRequest {
            url: String::new(),
            method: "POST".into(),
            headers: Vec::new(),
            body: None,
        };
    };
    let headers = h
        .allowed_headers
        .iter()
        .zip(ctx.secrets.iter())
        .map(|(name, sv)| (name.clone(), sv.to_secret_string()))
        .collect();
    let body = h.body_template.as_ref().map(|t| render_template(t, ctx));
    WebhookRequest {
        url: h.url.clone(),
        method: h.method.clone(),
        headers,
        body,
    }
}

fn build_judge_request(
    handler: &HandlerConfig,
    mode: JudgeMode,
    ctx: &RenderContext,
    payload: &TriggerPayload,
) -> JudgeRequest {
    let (prompt_template, response_schema, profile, allowlist, budget) = match handler {
        HandlerConfig::PromptEval(p) => (
            p.prompt_template.clone(),
            p.response_schema.clone(),
            None,
            Vec::new(),
            None,
        ),
        HandlerConfig::AgentEval(a) => (
            a.prompt_template.clone(),
            None,
            Some(a.restricted_profile.clone()),
            a.tool_allowlist.clone(),
            a.budget,
        ),
        _ => (String::new(), None, None, Vec::new(), None),
    };
    JudgeRequest {
        mode,
        workspace_id: payload.workspace_id.clone(),
        prompt: render_template(&prompt_template, ctx),
        response_schema,
        restricted_profile: profile,
        tool_allowlist: allowlist,
        budget,
    }
}

fn current_target_prompt(
    payload: &TriggerPayload,
    outcome: &DispatchOutcome,
    target: PromptTarget,
    secrets: &[&SecretValue],
) -> String {
    let original = match target {
        PromptTarget::System => payload.system_prompt.as_deref(),
        PromptTarget::User => payload.user_prompt.as_deref(),
        PromptTarget::Injected => payload.injected_prompt.as_deref(),
    }
    .map(|value| redact(value, secrets))
    .unwrap_or_default();
    outcome.transformed_prompt(&format!("{target:?}"), &original)
}

#[allow(clippy::too_many_arguments)]
async fn apply_judge_decision(
    outcome: &mut DispatchOutcome,
    exec: &Executors,
    hook_id: &str,
    trigger: TriggerPoint,
    scope: &HookScope,
    lifecycle: HandlerLifecycle,
    started: Instant,
    payload: &TriggerPayload,
    secrets: &[&SecretValue],
    decision: JudgeDecision,
) {
    let JudgeDecision::Transform { new_prompt } = decision else {
        outcome.push_effect(hook_id, HookEffect::Decision(decision));
        return;
    };

    // Eval / MCP 返回的 transform 与声明式 PromptTransform 走同一 policy 门，
    // 并产出同一种可回放 diff。Judge 只能改写 User target，不能借结果文本
    // 绕过 system/security 保护。
    let new_prompt = redact(&new_prompt, secrets);
    let action = PolicyAction {
        capability: HookCapability::PromptTransform,
        hook_id,
        workspace_id: payload.workspace_id.as_ref(),
        description: serde_json::json!({
            "source": "judge_transform",
            "target": "User",
        }),
        prompt_target: Some(PromptTarget::User),
        allow_system_override: false,
    };
    if let PolicyOutcome::Deny { reason } = exec.policy.evaluate(action).await {
        outcome.denied.push((hook_id.to_string(), reason.clone()));
        record(
            outcome,
            exec,
            hook_id,
            trigger,
            scope,
            HookCapability::PromptTransform,
            lifecycle,
            Some(HookStatus::Denied(reason)),
            started,
            None,
            None,
        )
        .await;
        return;
    }

    let original = current_target_prompt(payload, outcome, PromptTarget::User, secrets);
    let diff = PromptTransformDiff::new("User", &original, &new_prompt, false);
    record(
        outcome,
        exec,
        hook_id,
        trigger,
        scope,
        HookCapability::PromptTransform,
        lifecycle,
        None,
        started,
        None,
        Some(diff.clone()),
    )
    .await;
    outcome.push_effect(
        hook_id,
        HookEffect::Transform {
            target: "User".into(),
            new_prompt,
            diff,
        },
    );
}

fn build_mcp_request(
    handler: &HandlerConfig,
    ctx: &RenderContext,
    payload: &TriggerPayload,
) -> McpToolRequest {
    let HandlerConfig::McpTool(m) = handler else {
        return McpToolRequest {
            server_id: String::new(),
            tool_name: String::new(),
            arguments: Value::Null,
            workspace_id: payload.workspace_id.clone(),
            run_id: payload.run_id.clone(),
        };
    };
    let arguments = m
        .arg_template
        .clone()
        .map(|v| crate::secret::redact_value(&v, &ctx.secrets))
        .unwrap_or(Value::Null);
    McpToolRequest {
        server_id: m.server_id.clone(),
        tool_name: m.tool_name.clone(),
        arguments,
        workspace_id: payload.workspace_id.clone(),
        run_id: payload.run_id.clone(),
    }
}

fn apply_prompt_transform(
    handler: &HandlerConfig,
    ctx: &RenderContext,
    original_prompt: &str,
) -> HookEffect {
    let HandlerConfig::PromptTransform(p) = handler else {
        return HookEffect::None;
    };
    let rendered = render_template(&p.template, ctx);
    let new_prompt = match p.rewrite_kind.as_str() {
        "replace" => rendered.clone(),
        "suffix" => join_prompt(original_prompt, &rendered),
        _ => join_prompt(&rendered, original_prompt),
    };
    let target_str = format!("{:?}", p.target);
    let diff = PromptTransformDiff::new(
        &target_str,
        original_prompt,
        &new_prompt,
        p.target == PromptTarget::System,
    );
    HookEffect::Transform {
        target: target_str,
        new_prompt,
        diff,
    }
}

fn join_prompt(first: &str, second: &str) -> String {
    match (first.is_empty(), second.is_empty()) {
        (true, _) => second.to_string(),
        (_, true) => first.to_string(),
        (false, false) => format!("{first}\n{second}"),
    }
}

fn eval_fallback(handler: &HandlerConfig) -> EvalFallback {
    match handler {
        HandlerConfig::PromptEval(p) => p.on_failure,
        HandlerConfig::AgentEval(a) => a.on_failure,
        _ => EvalFallback::Deny,
    }
}

fn mcp_fallback(handler: &HandlerConfig) -> McpFallback {
    match handler {
        HandlerConfig::McpTool(m) => m.on_failure,
        _ => McpFallback::Deny,
    }
}

/// McpTool 失败降级：根据显式配置产出 JudgeDecision。
fn fallback_decision(fallback: McpFallback, reason: &str) -> JudgeDecision {
    match fallback {
        McpFallback::Allow => JudgeDecision::Allow,
        McpFallback::Deny => JudgeDecision::Deny {
            reason: reason.to_string(),
        },
    }
}

// —— async fire-and-forget（queued + terminal 两条均持久化）——

enum AsyncJob {
    Command(CommandRequest, Option<Duration>),
    Http(WebhookRequest, Option<Duration>),
}

/// 投递 async handler：先持久化 queued 事件到 [`crate::exec::AuditSink`]（并放进
/// outcome），再 spawn 一个 future，其内部执行 job 后持久化 terminal 事件。
/// 失败仅记录，不阻塞 run loop。
async fn spawn_async(
    exec: &Executors,
    handler: &HookHandler,
    trigger: TriggerPoint,
    job: AsyncJob,
    outcome: &mut DispatchOutcome,
) {
    let queued = make_event(
        &*exec.clock,
        &handler.id.0,
        trigger,
        &handler.scope,
        handler.capability,
        HandlerLifecycle::Async,
        UserHookEventPayload::Dispatch {
            status: HookStatus::Success,
            duration_ms: 0,
            summary: Some("queued for async execution".into()),
        },
    );
    // queued 持久化（必须进 AuditSink，不只是 outcome）。
    exec.audit.record(queued.clone()).await;
    outcome.audit.push(queued);

    let command = exec.command.clone();
    let http = exec.http.clone();
    let audit = exec.audit.clone();
    let clock = exec.clock.clone();
    let hid = handler.id.0.clone();
    let scope = handler.scope.clone();
    let capability = handler.capability;
    let future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        Box::pin(async move {
            let started = Instant::now();
            let (status, summary) = match job {
                AsyncJob::Command(req, t) => match command.run(req, t).await {
                    Ok(result) if result.timed_out => {
                        (HookStatus::Timeout, Some("command timed out".into()))
                    }
                    Ok(result) => (
                        HookStatus::Success,
                        Some(format!("exit_code={}", result.exit_code)),
                    ),
                    Err(HookError::Timeout { .. }) => {
                        (HookStatus::Timeout, Some("command timed out".into()))
                    }
                    Err(HookError::Cancelled { .. }) => (HookStatus::Cancelled, None),
                    Err(e) => (HookStatus::Failed(e.to_string()), None),
                },
                AsyncJob::Http(req, t) => match http.send(req, t).await {
                    Ok(result) if result.timed_out => {
                        (HookStatus::Timeout, Some("http request timed out".into()))
                    }
                    Ok(result) => (
                        HookStatus::Success,
                        Some(format!("status={}", result.status)),
                    ),
                    Err(HookError::Timeout { .. }) => {
                        (HookStatus::Timeout, Some("http request timed out".into()))
                    }
                    Err(HookError::Cancelled { .. }) => (HookStatus::Cancelled, None),
                    Err(e) => (HookStatus::Failed(e.to_string()), None),
                },
            };
            // terminal 持久化。
            let terminal = make_event(
                &*clock,
                &hid,
                trigger,
                &scope,
                capability,
                HandlerLifecycle::Async,
                UserHookEventPayload::Dispatch {
                    status,
                    duration_ms: started.elapsed().as_millis() as u64,
                    summary,
                },
            );
            audit.record(terminal).await;
        });
    exec.async_runner.spawn(future);
}

// —— audit ——

/// 统一记录入口：`status=None` 表示 PromptTransform（Transform 载荷）；
/// 否则 Dispatch 载荷。同步派发与拒绝都走这里，事件同时进 outcome 与 AuditSink。
#[allow(clippy::too_many_arguments)]
async fn record(
    outcome: &mut DispatchOutcome,
    exec: &Executors,
    hook_id: &str,
    trigger: TriggerPoint,
    scope: &HookScope,
    capability: HookCapability,
    lifecycle: HandlerLifecycle,
    status: Option<HookStatus>,
    started: Instant,
    summary: Option<String>,
    diff: Option<PromptTransformDiff>,
) {
    let duration_ms = started.elapsed().as_millis() as u64;
    let payload = match diff {
        Some(d) => UserHookEventPayload::Transform {
            diff: d,
            duration_ms,
        },
        None => UserHookEventPayload::Dispatch {
            status: status.unwrap_or(HookStatus::Success),
            duration_ms,
            summary,
        },
    };
    let evt = make_event(
        &*exec.clock,
        hook_id,
        trigger,
        scope,
        capability,
        lifecycle,
        payload,
    );
    outcome.audit.push(evt.clone());
    exec.audit.record(evt).await;
}

#[allow(clippy::too_many_arguments)]
fn make_event(
    clock: &dyn HookClock,
    hook_id: &str,
    trigger: TriggerPoint,
    scope: &HookScope,
    capability: HookCapability,
    lifecycle: HandlerLifecycle,
    payload: UserHookEventPayload,
) -> UserHookEvent {
    UserHookEvent::new(
        clock.next_event_id(),
        clock.now(),
        hook_id.to_string(),
        trigger,
        scope.clone(),
        format!("{capability:?}"),
        format!("{lifecycle:?}"),
        payload,
    )
}

// —— helpers ——

/// McpTool 同步结果文本 → 判定决策（仅在 `success=true` 时调用）。
/// 约定：`allow`（默认）→ Allow；`deny[: reason]` → Deny；`transform: <text>` → Transform。
fn parse_mcp_decision(text: &str) -> JudgeDecision {
    let lower = text.trim().to_ascii_lowercase();
    if lower.starts_with("deny") {
        let reason = text.trim().trim_start_matches("deny").trim();
        let reason = reason.trim_start_matches(':').trim();
        JudgeDecision::Deny {
            reason: if reason.is_empty() {
                "denied by mcp tool hook".into()
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

/// 给 HandlerConfig 提供 timeout_ms 访问。
trait HandlerTimeoutMs {
    fn timeout_ms(&self) -> Option<u64>;
}
impl HandlerTimeoutMs for HandlerConfig {
    fn timeout_ms(&self) -> Option<u64> {
        match self {
            HandlerConfig::Command(c) => c.timeout_ms,
            HandlerConfig::Http(h) => h.timeout_ms,
            HandlerConfig::AgentEval(a) => a.budget.and_then(|budget| budget.timeout_ms),
            _ => None,
        }
    }
}
