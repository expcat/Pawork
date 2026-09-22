//! Parent-owned, persisted child runs. All tool work still goes through SessionLoopCtx.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use pawork_domain::*;
use pawork_engine::{
    AgentEventSink, EngineError, LoopEventEmitter, PendingToolInvocation, ToolCallResult,
};
use pawork_orchestration::{AgentSupervisor, SpawnRequest, SupervisorConfig};
use pawork_protocol::{SubagentInfo, SubagentListData};
use pawork_workspace::config::{SubagentConfig, SubagentModelConfig};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

use crate::{AppCore, AppError};

pub(crate) const PERMISSIONS: &[&str] = &[
    "read", "write", "terminal", "network", "mcp", "browser", "computer",
];
pub(crate) type ActiveSubagents = Arc<Mutex<BTreeMap<String, ActiveChild>>>;
pub(crate) struct ActiveChild {
    pub parent_session: SessionId,
    pub parent_run: RunId,
    pub token: CancellationToken,
    pub child_run: RunId,
}

pub(crate) fn rule(config: &SubagentConfig, provider: &str, model: &str) -> SubagentModelConfig {
    config
        .models
        .iter()
        .find(|r| r.provider_id == provider && r.model_id == model)
        .cloned()
        .unwrap_or_else(|| SubagentModelConfig {
            provider_id: provider.into(),
            model_id: model.into(),
            ..Default::default()
        })
}

/// ADR-063：子代理生效 effort 解析。
///
/// 顺序：子代理规则 `default_effort`（`allowed_efforts` 非空时须落在
/// 其中，越界 / 非法名视为未配置）> 模型级 `[reasoning]` 默认 > None
/// （Provider 默认，与未配置时行为一致）。
fn resolve_effort(
    config: &SubagentConfig,
    global: &pawork_workspace::config::PaworkConfig,
    provider: &str,
    model: &str,
) -> Option<pawork_domain::ReasoningEffort> {
    let rule = rule(config, provider, model);
    if let Some(name) = rule.default_effort.as_deref() {
        let parsed = pawork_domain::ReasoningEffort::from_wire_name(name);
        if let Some(effort) = parsed {
            if rule.allowed_efforts.is_empty()
                || rule.allowed_efforts.iter().any(|item| item == name)
            {
                return Some(effort);
            }
        }
    }
    global
        .reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.model(model))
        .and_then(|entry| entry.default_effort.as_deref())
        .and_then(pawork_domain::ReasoningEffort::from_wire_name)
        .filter(|effort| {
            rule.allowed_efforts.is_empty()
                || rule
                    .allowed_efforts
                    .iter()
                    .any(|name| name == effort.as_wire_name())
        })
}

pub(crate) fn allows_tool(rule: &SubagentModelConfig, tool: &ToolDescriptor) -> bool {
    let permission = match tool.name.as_str() {
        "browser" => "browser",
        "computer" => "computer",
        name if name.starts_with("mcp.") || name.starts_with("mcp__") => "mcp",
        _ => match tool.capability {
            ToolCapability::ReadOnly => "read",
            ToolCapability::WorkspaceWrite | ToolCapability::GitWrite => "write",
            ToolCapability::Process => "terminal",
            ToolCapability::Network => "network",
            ToolCapability::ExternalPlugin => "mcp",
            ToolCapability::UserInteraction => return true,
        },
    };
    rule.permissions.iter().any(|p| p == permission)
}

pub(crate) fn is_agent_tool(name: &str) -> bool {
    matches!(name, "spawn_agent" | "wait_agent" | "close_agent")
}

pub(crate) fn definitions() -> Vec<ToolDescriptor> {
    let pair = json!({"type":"string"});
    [
        ("spawn_agent", "Delegate an independent task to a subagent. It has its own conversation and shares this workspace and approval restrictions. Specify a complete task and expected result. Omit provider/model to inherit this model. Avoid concurrent edits to the same files. Child agents cannot delegate. Use wait_agent to collect its result before answering.", json!({"type":"object","properties":{"message":pair,"title":pair,"provider":pair,"model":pair},"required":["message"],"additionalProperties":false})),
        ("wait_agent", "Wait for selected child agents to finish (up to 60 seconds) and collect their status and final result. Repeat while running if their results are needed.", json!({"type":"object","properties":{"agent_ids":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":16},"timeout_ms":{"type":"integer","minimum":0,"maximum":60000}},"required":["agent_ids"],"additionalProperties":false})),
        ("close_agent", "Stop one of this task's child agents. Completed results remain available in history.", json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"],"additionalProperties":false})),
    ].into_iter().map(|(name, description, input_schema)| ToolDescriptor {
        name: name.into(), description: description.into(), input_schema,
        capability: ToolCapability::UserInteraction, kind: ToolKind::ClientFunction,
        hosting: ToolHosting::Local, capabilities: vec![], requires_approval: false,
        read_only: true, supports_concurrency: false, default_timeout_ms: Some(60_000),
        max_output_bytes: 128 * 1024, allowed_in_untrusted_workspace: true,
    }).collect()
}

pub(crate) fn denied_call(call: PendingToolInvocation, message: &str) -> ToolCallResult {
    reply(call, Err(AppError::Orchestration(message.into())))
}
fn reply(call: PendingToolInvocation, outcome: Result<Value, AppError>) -> ToolCallResult {
    let result = match outcome {
        Ok(value) => ToolResult::success(vec![ContentPart::Text(TextContent {
            text: value.to_string(),
        })]),
        Err(error) => {
            let message = error.to_string();
            let mut result = ToolResult::failure(ErrorContext {
                category: ErrorCategory::Authorization,
                message: message.clone(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            });
            result
                .content
                .push(ContentPart::Text(TextContent { text: message }));
            result
        }
    };
    ToolCallResult {
        tool_call_id: call.tool_call_id,
        tool_name: call.name,
        arguments: call.arguments,
        result,
    }
}
fn invalid(message: impl Into<String>) -> AppError {
    AppError::Orchestration(message.into())
}
fn unique(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "{prefix}-{:x}-{:x}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) struct SubagentRun<'a> {
    core: &'a AppCore,
    session: SessionId,
    run: RunId,
    cancel: CancellationToken,
    config: SubagentConfig,
    supervisor: OnceCell<(Arc<AgentSupervisor>, AgentId)>,
    jobs: AsyncMutex<Vec<tokio::task::JoinHandle<()>>>,
    finished: std::sync::atomic::AtomicBool,
}
impl<'a> SubagentRun<'a> {
    pub fn new(
        core: &'a AppCore,
        session: &SessionId,
        run: &RunId,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            core,
            session: session.clone(),
            run: run.clone(),
            cancel,
            config: core.config.subagents.clone().unwrap_or_default(),
            supervisor: OnceCell::new(),
            jobs: AsyncMutex::new(vec![]),
            finished: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn enabled(&self) -> bool {
        self.config.enabled
            && !self.session.as_str().starts_with("child-")
            && rule(
                &self.config,
                self.core.provider_id.as_str(),
                self.core.model.as_str(),
            )
            .allow_spawn
    }
    async fn supervisor(&self) -> Result<&(Arc<AgentSupervisor>, AgentId), AppError> {
        self.supervisor
            .get_or_try_init(|| async {
                let control = &self.core.usage.control;
                let supervisor = Arc::new(AgentSupervisor::new(
                    control.pool.clone(),
                    control.policy.clone(),
                    control.ledger.clone(),
                    SupervisorConfig {
                        max_agent_concurrency: u64::from(self.config.max_concurrent.clamp(1, 16))
                            + 1,
                        max_worker_depth: Some(1),
                        ..Default::default()
                    },
                ));
                let parent = supervisor
                    .spawn(spawn_request(&self.session, None, None))
                    .await
                    .map_err(|e| invalid(e.to_string()))?;
                supervisor
                    .start_worker(&parent)
                    .await
                    .map_err(|e| invalid(e.to_string()))?;
                Ok((supervisor, parent))
            })
            .await
    }
    pub async fn execute(
        &self,
        call: PendingToolInvocation,
        events: LoopEventEmitter<'_>,
        cancel: CancellationToken,
    ) -> ToolCallResult {
        if !self.enabled() {
            return denied_call(
                call,
                "subagents are disabled for this model or this is already a child agent",
            );
        }
        let outcome = match call.name.as_str() {
            "spawn_agent" => self.spawn(&call.arguments, events).await,
            "wait_agent" => self.wait(&call.arguments, cancel).await,
            "close_agent" => match call.arguments.get("agent_id").and_then(Value::as_str) {
                Some(id) => self.close(id).await,
                None => Err(invalid("agent_id is required")),
            },
            _ => Err(invalid("unknown subagent tool")),
        };
        reply(call, outcome)
    }
    async fn spawn(&self, input: &Value, events: LoopEventEmitter<'_>) -> Result<Value, AppError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            message: String,
            title: Option<String>,
            provider: Option<String>,
            model: Option<String>,
        }
        let input: Input = serde_json::from_value(input.clone())
            .map_err(|_| invalid("invalid spawn_agent arguments"))?;
        if input.message.trim().is_empty() || input.message.len() > 64 * 1024 {
            return Err(invalid("task must contain 1..65536 bytes"));
        }
        // Serialize admission with finish; no child can be registered after its join drain.
        let mut jobs = self.jobs.lock().await;
        if self.finished.load(Ordering::Acquire) || self.cancel.is_cancelled() {
            return Err(invalid("parent run has finished or was cancelled"));
        }
        let provider = input
            .provider
            .as_deref()
            .unwrap_or(self.core.provider_id.as_str());
        let model = input.model.as_deref().unwrap_or(self.core.model.as_str());
        let child_rule = rule(&self.config, provider, model);
        if !child_rule.allow_as_subagent || !self.core.config.is_model_enabled(provider, model) {
            return Err(invalid("this model is not allowed as a subagent"));
        }
        let mut child = self.core.child_core(&self.session, provider, model).await?;
        // ADR-063：子代理 effort = 子代理规则默认（须在规则 allowed_efforts
        // 内，越界视为未配置）> 模型级 `[reasoning]` 默认 > None（Provider
        // 默认）。配置名非法按未配置处理（写路径已 fail-closed 校验）。
        let child_effort = resolve_effort(&self.config, &self.core.config, provider, model);
        child.effort = child_effort;
        // RunService falls back to this default when effort is None; keep that
        // fallback consistent with the child's allowed range as well.
        if let Some(reasoning) = &mut child.config.reasoning {
            for entry in &mut reasoning.models {
                if entry.provider_id == provider && entry.model_id == model {
                    entry.default_effort = child_effort.map(|effort| effort.as_wire_name().into());
                }
            }
        }
        child.parent_tool_run = Some(self.run.clone());
        // A child cannot obtain tools the parent model was not allowed to use.
        let parent_rule = rule(
            &self.config,
            self.core.provider_id.as_str(),
            self.core.model.as_str(),
        );
        child
            .descriptors
            .retain(|d| allows_tool(&parent_rule, d) && allows_tool(&child_rule, d));
        child
            .tool_defs
            .retain(|d| child.descriptors.iter().any(|t| t.name == d.name));
        let mut config = self.config.clone();
        config.enabled = false;
        child.config.subagents = Some(config);
        if !parent_rule.permissions.iter().any(|p| p == "network")
            || !child_rule.permissions.iter().any(|p| p == "network")
        {
            child.config.web_search = Some(false);
        }
        if self.cancel.is_cancelled() {
            return Err(invalid("parent run was cancelled"));
        }
        let (supervisor, parent) = self.supervisor().await?;
        let worker = supervisor
            .spawn(spawn_request(
                &self.session,
                Some(parent.clone()),
                Some(model.into()),
            ))
            .await
            .map_err(|e| invalid(e.to_string()))?;
        let child_session = SessionId::from(unique("child"));
        let child_run = RunId::from(unique("run-child"));
        let title: String = input
            .title
            .as_deref()
            .unwrap_or(&input.message)
            .chars()
            .take(80)
            .collect();
        let info = SubagentInfo {
            agent_id: child_session.as_str().into(),
            session_id: child_session.as_str().into(),
            parent_run_id: self.run.as_str().into(),
            title: title.clone(),
            provider_id: provider.into(),
            model_id: model.into(),
            status: "running".into(),
            result: None,
            effort: child_effort.map(|effort| effort.as_wire_name().to_string()),
        };
        supervisor
            .start_worker(&worker)
            .await
            .map_err(|e| invalid(e.to_string()))?;
        let token = supervisor
            .cancel_token(&worker)
            .ok_or_else(|| invalid("missing child cancellation token"))?;
        self.core.subagents.lock().unwrap().insert(
            info.agent_id.clone(),
            ActiveChild {
                parent_session: self.session.clone(),
                parent_run: self.run.clone(),
                token: token.clone(),
                child_run: child_run.clone(),
            },
        );
        let prepare = async {
            let ws = self.core.workspace_for_session_or_unbound(&self.session)?;
            if ws.id.as_str() == "ws-unbound" {
                child
                    .store()?
                    .create_session(&child_session, &title, pawork_engine::now_timestamp())
                    .await?;
            } else {
                child
                    .store()?
                    .create_session_with_workspace(
                        &child_session,
                        &title,
                        pawork_engine::now_timestamp(),
                        &ws.id,
                    )
                    .await?;
                child.bind_session_workspace(&child_session, ws.id);
            }
            events
                .emit(AgentEvent::Diagnostic {
                    code: "subagent.spawned".into(),
                    details: serde_json::to_value(&info).unwrap(),
                })
                .await?;
            Ok::<_, AppError>(())
        }
        .await;
        if let Err(error) = prepare {
            self.core.subagents.lock().unwrap().remove(&info.agent_id);
            supervisor
                .fail(&worker, "child setup failed".into())
                .await
                .map_err(|e| invalid(e.to_string()))?;
            return Err(error);
        }
        let active = self.core.subagents.clone();
        let id = info.agent_id.clone();
        let supervisor = supervisor.clone();
        let parent_cancel = self.cancel.clone();
        let render = self
            .core
            .subagent_render
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| Arc::new(SilentSink));
        let job = tokio::spawn(async move {
            let future = child.chat_turn_with_run_id(
                child_run.clone(),
                &child_session,
                vec![Message {
                    id: MessageId::from("pending"),
                    role: MessageRole::User,
                    content: vec![ContentPart::Text(TextContent {
                        text: input.message,
                    })],
                    metadata: Default::default(),
                }],
                render.as_ref(),
                token.clone(),
                None,
            );
            tokio::pin!(future);
            let outcome = tokio::select! {
                result = &mut future => result,
                _ = parent_cancel.cancelled() => { token.cancel(); future.await }
            };
            if let Err(error) = &outcome {
                if let Err(seal_error) = seal_child(
                    &child,
                    &child_session,
                    &child_run,
                    render.as_ref(),
                    error,
                    token.is_cancelled(),
                )
                .await
                {
                    tracing::error!(%seal_error,"failed to persist subagent terminal state");
                }
            }
            let terminal = if token.is_cancelled() {
                supervisor.cancel_tree(&worker).await.map(|_| ())
            } else if outcome.is_ok() {
                supervisor.complete(&worker).await
            } else {
                supervisor.fail(&worker, "child run failed".into()).await
            };
            if let Err(error) = terminal {
                tracing::warn!(%error,"subagent supervisor completion failed");
            }
            active.lock().unwrap().remove(&id);
        });
        jobs.push(job);
        Ok(serde_json::to_value(info).unwrap())
    }
    async fn wait(&self, input: &Value, cancel: CancellationToken) -> Result<Value, AppError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            agent_ids: Vec<String>,
            timeout_ms: Option<u64>,
        }
        let input: Input = serde_json::from_value(input.clone())
            .map_err(|_| invalid("invalid wait_agent arguments"))?;
        if input.agent_ids.is_empty() || input.agent_ids.len() > 16 {
            return Err(invalid("select 1..16 child agents"));
        }
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(input.timeout_ms.unwrap_or(30_000).min(60_000));
        loop {
            let list = self
                .core
                .query_subagents(&self.session, Some(&input.agent_ids))
                .await?;
            let selected: Vec<_> = list
                .agents
                .into_iter()
                .filter(|a| {
                    input.agent_ids.contains(&a.agent_id) && a.parent_run_id == self.run.as_str()
                })
                .collect();
            if selected.len() != input.agent_ids.iter().collect::<BTreeSet<_>>().len() {
                return Err(invalid("agent does not belong to this parent run"));
            }
            let done = selected
                .iter()
                .all(|a| !matches!(a.status.as_str(), "running" | "waiting"));
            if done || tokio::time::Instant::now() >= deadline {
                return Ok(json!({"agents":selected,"timed_out":!done}));
            }
            tokio::select! { _ = cancel.cancelled() => return Err(invalid("wait cancelled")), _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {} }
        }
    }
    /// close_agent 只作用于本 run 的子代理：session 内其它 run 的 child
    /// 拒绝取消（GUI 的 session 级 subagent_cancel 保留宽语义）。
    async fn close(&self, id: &str) -> Result<Value, AppError> {
        let list = self
            .core
            .query_subagents(&self.session, Some(&[id.to_string()]))
            .await?;
        let Some(agent) = list.agents.iter().find(|a| a.agent_id == id) else {
            return Err(invalid("agent does not belong to this session"));
        };
        if agent.parent_run_id != self.run.as_str() {
            return Err(invalid("agent does not belong to this parent run"));
        }
        let child = self
            .core
            .subagents
            .lock()
            .unwrap()
            .get(id)
            .map(|c| (c.parent_run.clone(), c.token.clone()));
        if let Some((parent_run, token)) = child {
            if parent_run != self.run {
                return Err(invalid("agent does not belong to this parent run"));
            }
            token.cancel();
        }
        Ok(json!({"agent_id":id,"cancellation_requested":true}))
    }
    pub fn cancel_children(&self) {
        self.cancel.cancel();
        for child in self
            .core
            .subagents
            .lock()
            .unwrap()
            .values()
            .filter(|c| c.parent_run == self.run)
        {
            child.token.cancel();
        }
    }
    pub async fn finish(&self) {
        let jobs = {
            let mut jobs = self.jobs.lock().await;
            if self.finished.swap(true, Ordering::AcqRel) {
                return;
            }
            std::mem::take(&mut *jobs)
        };
        for job in jobs {
            if let Err(error) = job.await {
                tracing::error!(%error,"subagent task terminated");
            }
        }
        if let Some((supervisor, parent)) = self.supervisor.get() {
            let outcome = if self.cancel.is_cancelled() {
                supervisor.cancel_tree(parent).await.map(|_| ())
            } else {
                supervisor.complete(parent).await
            };
            if let Err(error) = outcome {
                tracing::warn!(%error,"parent supervisor completion failed");
            }
        }
    }
}
impl Drop for SubagentRun<'_> {
    fn drop(&mut self) {
        for child in self
            .core
            .subagents
            .lock()
            .unwrap()
            .values()
            .filter(|c| c.parent_run == self.run)
        {
            child.token.cancel();
        }
    }
}
fn spawn_request(
    session: &SessionId,
    parent_id: Option<AgentId>,
    model: Option<ModelId>,
) -> SpawnRequest {
    SpawnRequest {
        tenant_id: pawork_control_plane::default_tenant(),
        principal_id: pawork_control_plane::default_principal(),
        parent_id,
        session_id: session.clone(),
        model,
        worktree_path: None,
        budget: None,
        acquire: None,
        task_deps: vec![],
        task_description: None,
        task_max_retries: None,
    }
}
struct SilentSink;
#[async_trait]
impl AgentEventSink for SilentSink {
    async fn emit(&self, _: AgentEventEnvelope) -> Result<(), EngineError> {
        Ok(())
    }
}
async fn seal_child(
    core: &AppCore,
    session: &SessionId,
    run: &RunId,
    render: &dyn AgentEventSink,
    error: &AppError,
    cancelled: bool,
) -> Result<(), AppError> {
    let snapshot = core.store()?.projection_snapshot(session).await?;
    if snapshot.runs.iter().any(|r| {
        r.run_id == *run && matches!(r.state.as_str(), "completed" | "failed" | "cancelled")
    }) {
        return Ok(());
    }
    let mut sequence = core.next_sequence(session).await?;
    if !snapshot.runs.iter().any(|r| r.run_id == *run) {
        let started = core
            .append_payload(
                session,
                run,
                &mut sequence,
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("pending"),
                },
            )
            .await?;
        render.emit(started).await?;
    }
    let event = core
        .append_payload(
            session,
            run,
            &mut sequence,
            if cancelled {
                AgentEvent::RunCancelled {
                    reason: Some("subagent cancelled".into()),
                    usage: None,
                }
            } else {
                AgentEvent::RunFailed {
                    error: ErrorContext {
                        category: ErrorCategory::Internal,
                        message: error.to_string(),
                        retryable: false,
                        retry_after_ms: None,
                        diagnostics: Default::default(),
                    },
                    usage: None,
                }
            },
        )
        .await?;
    render.emit(event).await?;
    Ok(())
}

impl AppCore {
    async fn child_core(
        &self,
        parent: &SessionId,
        provider: &str,
        model: &str,
    ) -> Result<AppCore, AppError> {
        let mut child = if provider == self.provider_id.as_str() {
            let mut child = Self::from_parts_with_protocol(
                self.request_provider_snapshot().await?,
                self.credential.clone(),
                model.into(),
                provider.into(),
                self.adapter_protocol,
                Some(self.store()?.clone()),
                (*self.registry).clone(),
            );
            // Reused adapters retain this protector; fresh cross-provider adapters
            // retain the one created by from_config_inner instead.
            child.reasoning_protector = self.reasoning_protector.clone();
            child
        } else {
            let mut child = Self::from_config_inner(
                self.config.clone(),
                Some(provider),
                Some(model),
                self.backend.clone(),
                false,
            )
            .await?;
            child.store = Some(self.store()?.clone());
            child
        };
        // Unknown explicit targets must never fall back to another provider/model.
        if provider != self.provider_id.as_str() || model != self.model.as_str() {
            let mut registry = (*child.registry).clone();
            crate::provider_assembly::resolve_provider_model(
                &mut registry,
                child.provider.as_ref(),
                child.credential.as_ref(),
                &child.provider_id,
                model,
                &self.config,
            )
            .await?;
            child.registry = Arc::new(registry);
        }
        child.config = self.config.clone();
        child.backend = self.backend.clone();
        child.protected_store = self.protected_store.clone();
        child.rebind_persistent_protector();
        child.scheduler = self.scheduler.clone();
        child.descriptors = self.descriptors.clone();
        child.tool_defs = self.tool_defs.clone();
        child.extensions.workspaces = self.extensions.workspaces.clone();
        child.extensions.workspace_catalog = self.extensions.workspace_catalog.clone();
        child.extensions.resource_loader = Some(
            crate::services::extension::ExtensionService::resource_loader_for(
                child.extensions.workspaces.clone(),
            ),
        );
        child.checkpoints = self.checkpoints.clone();
        child.artifacts = self.artifacts.clone();
        child.usage.control.ledger = self.usage.control.ledger.clone();
        child.subagents = self.subagents.clone();
        let workspace = self.workspace_for_session_or_unbound(parent)?;
        child.configure_approval(
            self.approval_mode(),
            self.workspace_trusted_for_roots(&workspace.roots),
            self.approval_host(),
        );
        Ok(child)
    }
    pub(crate) async fn list_subagents(
        &self,
        session: &SessionId,
    ) -> Result<SubagentListData, AppError> {
        self.query_subagents(session, None).await
    }

    async fn query_subagents(
        &self,
        session: &SessionId,
        agent_ids: Option<&[String]>,
    ) -> Result<SubagentListData, AppError> {
        let branch = self.session_active_branch(session).await?;
        let events = self
            .store()?
            .events_on_lineage(session, &branch, 1, usize::MAX)
            .await?;
        let mut agents = BTreeMap::new();
        for event in events {
            if let AgentEvent::Diagnostic { code, details } = event.payload {
                if code == "subagent.spawned" {
                    if let Ok(info) = serde_json::from_value::<SubagentInfo>(details) {
                        if agent_ids.is_none_or(|ids| ids.contains(&info.agent_id)) {
                            agents.insert(info.agent_id.clone(), (event.sequence, info));
                        }
                    }
                }
            }
        }
        let mut agents: Vec<_> = agents.into_values().collect();
        if agent_ids.is_none() && agents.len() > 64 {
            // Display bounds must not hide active children or select by process ID.
            // Targeted wait/close queries retain every requested child on this lineage.
            let active = self.subagents.lock().unwrap();
            agents.sort_by_key(|(sequence, info)| (active.contains_key(&info.agent_id), *sequence));
            agents.drain(..agents.len() - 64);
        }
        agents.sort_by_key(|(sequence, _)| *sequence);
        let mut agents: Vec<_> = agents.into_iter().map(|(_, info)| info).collect();
        for info in &mut agents {
            let child_session = SessionId::from(info.session_id.as_str());
            let snapshot = self.store()?.projection_snapshot(&child_session).await?;
            if let Some(run) = snapshot.runs.last() {
                info.status = run.state.clone();
            }
            if matches!(info.status.as_str(), "running" | "waiting")
                && !self.subagents.lock().unwrap().contains_key(&info.agent_id)
            {
                info.status = "failed".into();
            }
            info.result = snapshot
                .messages
                .iter()
                .rev()
                .filter(|m| m.role == MessageRole::Assistant)
                .map(|m| {
                    m.content
                        .iter()
                        .filter_map(|p| match p {
                            ContentPart::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .find(|s| !s.is_empty())
                .map(|s| s.chars().take(2_000).collect());
            if info.result.is_none() && info.status == "failed" {
                info.result = Some("子代理未完成，请打开任务查看错误。".into());
            }
        }
        Ok(SubagentListData { agents })
    }
    pub(crate) async fn cancel_subagent(
        &self,
        parent: &SessionId,
        id: &str,
    ) -> Result<(), AppError> {
        let list = self
            .query_subagents(parent, Some(&[id.to_string()]))
            .await?;
        if !list.agents.iter().any(|a| a.agent_id == id) {
            return Err(invalid("agent does not belong to this session"));
        }
        if let Some(child) = self.subagents.lock().unwrap().get(id) {
            if child.parent_session != *parent {
                return Err(invalid("child ownership mismatch"));
            }
            child.token.cancel();
        }
        Ok(())
    }
}

/// A parent cannot publish its terminal event while its children still execute.
pub(crate) struct ParentSink<'a> {
    pub inner: &'a dyn AgentEventSink,
    pub subagents: &'a SubagentRun<'a>,
}
#[async_trait]
impl AgentEventSink for ParentSink<'_> {
    async fn emit(&self, event: AgentEventEnvelope) -> Result<(), EngineError> {
        if matches!(
            event.payload,
            AgentEvent::RunFailed { .. } | AgentEvent::RunCancelled { .. }
        ) {
            self.subagents.cancel_children();
        }
        if matches!(
            event.payload,
            AgentEvent::RunCompleted { .. }
                | AgentEvent::RunFailed { .. }
                | AgentEvent::RunCancelled { .. }
        ) {
            self.subagents.finish().await;
        }
        self.inner.emit(event).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::{user_hello, RecordingEvents};
    use pawork_storage::session::SessionStore;
    use pawork_testkit::{MockProvider, MockScript};

    struct DelegatingProvider {
        wait: bool,
        target: Option<(&'static str, &'static str)>,
    }
    #[async_trait]
    impl ModelProvider for DelegatingProvider {
        fn id(&self) -> ProviderId {
            "mock".into()
        }
        async fn list_models(
            &self,
            _: Option<&ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            Ok(vec![])
        }
        async fn stream(
            &self,
            request: &CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            let session = request.session_id.as_ref().unwrap().as_str();
            let child = session.starts_with("child-");
            let tools = request
                .messages
                .iter()
                .filter(|m| m.role == MessageRole::Tool)
                .count();
            let call = |name: &str, args: Value| {
                MockScript::new()
                    .tool_call_chunks(
                        ToolCallId::from(format!("{session}-{name}")),
                        name,
                        [args.to_string()],
                    )
                    .complete_with(StopReason::ToolUse)
            };
            let script = if child {
                assert!(
                    request.reasoning.is_none(),
                    "child must not bypass allowed efforts via the global default"
                );
                assert!(!request
                    .tools
                    .iter()
                    .any(|t| is_agent_tool(&t.name) || t.name == "write_file"));
                if self.wait {
                    MockScript::new().wait_for_cancellation()
                } else if tools == 0 {
                    MockScript::new()
                        .tool_call_chunks(
                            ToolCallId::from(format!("{session}-read")),
                            "read_file",
                            [json!({"path":"note.txt"}).to_string()],
                        )
                        .tool_call_chunks(
                            ToolCallId::from(format!("{session}-write")),
                            "write_file",
                            [json!({"path":"note.txt","content":"overwrite"}).to_string()],
                        )
                        .tool_call_chunks(
                            ToolCallId::from(format!("{session}-spawn")),
                            "spawn_agent",
                            [json!({"message":"grandchild"}).to_string()],
                        )
                        .complete_with(StopReason::ToolUse)
                } else {
                    MockScript::new().text("child result: original").complete()
                }
            } else if tools == 0 {
                let mut arguments =
                    json!({"message":"Read note.txt and report it","title":"Reader"});
                if let Some((provider, model)) = self.target {
                    arguments["provider"] = json!(provider);
                    arguments["model"] = json!(model);
                }
                MockScript::new()
                    .tool_call_chunks(
                        ToolCallId::from(format!("{session}-spawn_agent")),
                        "spawn_agent",
                        [arguments.to_string()],
                    )
                    .tool_call_chunks(
                        ToolCallId::from(format!("{session}-parent-read")),
                        "read_file",
                        [json!({"path":"note.txt"}).to_string()],
                    )
                    .complete_with(StopReason::ToolUse)
            } else if tools == 2 {
                let id = request
                    .messages
                    .iter()
                    .flat_map(|m| &m.content)
                    .find_map(|p| match p {
                        ContentPart::ToolResult(result) => {
                            result.content.iter().find_map(|p| match p {
                                ContentPart::Text(t) => serde_json::from_str::<Value>(&t.text)
                                    .ok()
                                    .and_then(|v| v["agent_id"].as_str().map(str::to_owned)),
                                _ => None,
                            })
                        }
                        _ => None,
                    })
                    .expect("spawn result includes child id");
                call("wait_agent", json!({"agent_ids":[id],"timeout_ms":60000}))
            } else {
                MockScript::new().text("collected child result").complete()
            };
            MockProvider::new(script)
                .stream(request, sink, cancel)
                .await
        }
    }
    async fn setup(wait: bool) -> (AppCore, tempfile::TempDir, SessionId) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), "original").unwrap();
        let (store, _) = SessionStore::open(dir.path().join("session.db"))
            .await
            .unwrap();
        let mut core = AppCore::from_parts(
            Arc::new(DelegatingProvider { wait, target: None }),
            None,
            "model-1".into(),
            "mock".into(),
            Some(store),
        );
        core.configure_approval(
            pawork_policy::ApprovalMode::ReadOnly,
            true,
            Arc::new(crate::DenyAllApprovals),
        );
        core.attach_workspace(dir.path()).unwrap();
        core.config.subagents = Some(SubagentConfig {
            models: vec![SubagentModelConfig {
                provider_id: "mock".into(),
                model_id: "model-1".into(),
                permissions: vec!["read".into()],
                ..Default::default()
            }],
            ..Default::default()
        });
        let session = core.create_session("parent").await.unwrap();
        (core, dir, session)
    }
    #[tokio::test]
    async fn subagent_result_replays_and_tool_restrictions_cannot_be_bypassed() {
        let (mut core, dir, session) = setup(false).await;
        core.config.reasoning = Some(pawork_workspace::config::ReasoningSettings {
            models: vec![pawork_workspace::config::ModelReasoningConfig {
                provider_id: "mock".into(),
                model_id: "model-1".into(),
                default_effort: Some("high".into()),
                ..Default::default()
            }],
        });
        core.config.subagents.as_mut().unwrap().models[0].allowed_efforts = vec!["low".into()];
        core.chat_turn(
            &session,
            vec![user_hello()],
            &RecordingEvents::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("note.txt")).unwrap(),
            "original"
        );
        let parent_events = core
            .store()
            .unwrap()
            .replay_events(&session, 1, 1000)
            .await
            .unwrap();
        let spawned = parent_events.iter().position(|e| matches!(&e.payload, AgentEvent::Diagnostic { code, .. } if code == "subagent.spawned")).unwrap();
        let read = parent_events.iter().position(|e| matches!(&e.payload, AgentEvent::ToolExecutionCompleted { tool_call_id, .. } if tool_call_id.as_str().ends_with("parent-read"))).unwrap();
        assert!(
            spawned < read,
            "the first serial tool must complete before following parallel tools"
        );
        let list = core.list_subagents(&session).await.unwrap();
        assert_eq!(list.agents.len(), 1);
        let child = &list.agents[0];
        assert_eq!(child.status, "completed");
        assert_eq!(child.result.as_deref(), Some("child result: original"));
        assert_eq!(core.list_sessions().await.unwrap().len(), 1);
        let child_session = SessionId::from(child.session_id.as_str());
        let events = core
            .store()
            .unwrap()
            .replay_events(&child_session, 1, 1000)
            .await
            .unwrap();
        assert_eq!(events.iter().filter(|e|matches!(&e.payload,AgentEvent::ToolExecutionCompleted{result,..} if result.is_error)).count(),2);
        let path = core.store().unwrap().path().to_path_buf();
        core.shutdown().await.unwrap();
        let (store, _) = SessionStore::open(path).await.unwrap();
        let replay = AppCore::from_parts(
            Arc::new(DelegatingProvider {
                wait: false,
                target: None,
            }),
            None,
            "model-1".into(),
            "mock".into(),
            Some(store),
        );
        let restored = replay.list_subagents(&session).await.unwrap();
        assert_eq!(restored.agents[0].status, "completed");
        assert_eq!(restored.agents[0].result, child.result);
        let unrelated = replay.create_session_unbound("other").await.unwrap();
        assert!(replay
            .cancel_subagent(&unrelated, &child.agent_id)
            .await
            .is_err());
    }
    #[tokio::test]
    async fn display_limit_keeps_active_and_recent_children_without_limiting_control() {
        let (core, _dir, session) = setup(false).await;
        let parent_run = RunId::from("display-limit");
        let mut sequence = core.next_sequence(&session).await.unwrap();
        let mut ids = Vec::new();
        for index in 0..65 {
            // IDs deliberately sort opposite to creation order (e.g. after a restart).
            let id = format!("child-{:02}", 64 - index);
            let child_session = SessionId::from(id.as_str());
            core.store()
                .unwrap()
                .create_session(&child_session, "child", pawork_engine::now_timestamp())
                .await
                .unwrap();
            let info = SubagentInfo {
                agent_id: id.clone(),
                session_id: id.clone(),
                parent_run_id: parent_run.as_str().into(),
                title: id.clone(),
                provider_id: "mock".into(),
                model_id: "model-1".into(),
                status: "running".into(),
                result: None,
                effort: None,
            };
            core.append_payload(
                &session,
                &parent_run,
                &mut sequence,
                AgentEvent::Diagnostic {
                    code: "subagent.spawned".into(),
                    details: serde_json::to_value(info).unwrap(),
                },
            )
            .await
            .unwrap();
            let child_run = RunId::from(format!("run-{id}"));
            if index == 0 {
                core.subagents.lock().unwrap().insert(
                    id.clone(),
                    ActiveChild {
                        parent_session: session.clone(),
                        parent_run: parent_run.clone(),
                        token: CancellationToken::new(),
                        child_run,
                    },
                );
            } else {
                seal_child(
                    &core,
                    &child_session,
                    &child_run,
                    &SilentSink,
                    &invalid("cancelled"),
                    true,
                )
                .await
                .unwrap();
            }
            ids.push(id);
        }
        let displayed = core.list_subagents(&session).await.unwrap().agents;
        assert_eq!(displayed.len(), 64);
        assert_eq!(displayed.first().unwrap().agent_id, ids[0]);
        assert_eq!(displayed.last().unwrap().agent_id, ids[64]);
        assert!(!displayed.iter().any(|a| a.agent_id == ids[1]));
        let run = SubagentRun::new(&core, &session, &parent_run, CancellationToken::new());
        let result = run
            .wait(
                &json!({"agent_ids":[ids[1]], "timeout_ms":0}),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result["agents"][0]["status"], "cancelled");
        assert_eq!(result["timed_out"], false);
        run.close(&ids[1]).await.unwrap();
        core.cancel_subagent(&session, &ids[1]).await.unwrap();
        let other_run = SubagentRun::new(
            &core,
            &session,
            &RunId::from("other"),
            CancellationToken::new(),
        );
        assert!(other_run.close(&ids[1]).await.is_err());
        let other_session = core.create_session_unbound("other").await.unwrap();
        assert!(core.cancel_subagent(&other_session, &ids[1]).await.is_err());
        core.cancel_subagent(&session, &ids[0]).await.unwrap();
        assert!(core.subagents.lock().unwrap()[&ids[0]].token.is_cancelled());
    }

    #[tokio::test]
    async fn cross_provider_subagents_select_models_and_persist_reasoning() {
        use pawork_providers::ReasoningProtector;
        use pawork_workspace::config::ProviderConfig;
        use wiremock::{
            matchers::{body_partial_json, method, path},
            Mock, MockServer, ResponseTemplate,
        };

        for (provider, model, protocol, endpoint, chunks) in [
            (
                "custom-chat",
                "qwen-test",
                "chat_completions",
                "/chat/completions",
                vec![
                    json!({"id":"chat","choices":[{"index":0,"delta":{"content":"child answer"},"finish_reason":null}]}),
                    json!({"id":"chat","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
                ],
            ),
            (
                "custom-messages",
                "claude-3-5-sonnet",
                "messages",
                "/v1/messages",
                vec![
                    json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":10,"output_tokens":1}}}),
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","id":"thought"}}),
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}),
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"test-signature"}}),
                    json!({"type":"content_block_stop","index":0}),
                    json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"child answer"}}),
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}),
                    json!({"type":"message_stop"}),
                ],
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/models"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":model}]})),
                )
                .mount(&server)
                .await;
            let body: String = chunks
                .into_iter()
                .map(|chunk| format!("data: {chunk}\n\n"))
                .collect();
            Mock::given(method("POST"))
                .and(path(endpoint))
                .and(body_partial_json(json!({"model":model})))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(body),
                )
                .expect(1)
                .mount(&server)
                .await;
            let (mut core, dir, session) = setup(false).await;
            core.provider = Arc::new(DelegatingProvider {
                wait: false,
                target: Some((provider, model)),
            });
            core.config.providers.push(ProviderConfig {
                id: provider.into(),
                base_url: Some(server.uri()),
                ..Default::default()
            });
            core.config
                .extra
                .insert("provider_protocols".into(), json!({provider:protocol}));
            pawork_auth::store_default_api_key(
                core.backend.as_ref(),
                &ProviderId::from(provider),
                "test-key",
            )
            .unwrap();
            core.open_protected(dir.path().join("protected"))
                .await
                .unwrap();
            core.chat_turn(
                &session,
                vec![user_hello()],
                &RecordingEvents::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
            let list = core.list_subagents(&session).await.unwrap();
            let child = &list.agents[0];
            assert_eq!((&*child.provider_id, &*child.model_id), (provider, model));
            assert_eq!(child.status, "completed");
            assert_eq!(child.result.as_deref(), Some("child answer"));
            if protocol == "messages" {
                let snapshot = core
                    .store()
                    .unwrap()
                    .projection_snapshot(&SessionId::from(child.session_id.as_str()))
                    .await
                    .unwrap();
                let blob = snapshot
                    .messages
                    .iter()
                    .flat_map(|m| &m.content)
                    .find_map(|part| match part {
                        ContentPart::Reasoning(item) => Some(&item.protected_blob_ref),
                        _ => None,
                    })
                    .expect("child reasoning was persisted");
                let protector = crate::protected::PersistentReasoningProtector::new(
                    core.protected_store.clone().unwrap(),
                    provider.into(),
                );
                let payload = protector
                    .resolve(blob)
                    .await
                    .expect("child signature must survive its in-memory adapter");
                assert!(String::from_utf8(payload)
                    .unwrap()
                    .contains("test-signature"));
            }
        }
    }

    #[tokio::test]
    async fn cancellation_stops_children_and_concurrency_rejects_extra_workers() {
        let (core, _dir, session) = setup(true).await;
        let core = Arc::new(core);
        let token = CancellationToken::new();
        let task = {
            let core = core.clone();
            let session = session.clone();
            let token = token.clone();
            tokio::spawn(async move {
                core.chat_turn(
                    &session,
                    vec![user_hello()],
                    &RecordingEvents::default(),
                    token,
                )
                .await
            })
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if !core.subagents.lock().unwrap().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        token.cancel();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(10), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert!(core.subagents.lock().unwrap().is_empty());
        let list = core.list_subagents(&session).await.unwrap();
        assert_eq!(list.agents[0].status, "cancelled");
        let run = SubagentRun::new(
            &core,
            &session,
            &RunId::from("concurrency"),
            CancellationToken::new(),
        );
        let (sup, parent) = run.supervisor().await.unwrap();
        for _ in 0..4 {
            sup.spawn(spawn_request(&session, Some(parent.clone()), None))
                .await
                .unwrap();
        }
        assert!(sup
            .spawn(spawn_request(&session, Some(parent.clone()), None))
            .await
            .is_err());
        sup.cancel_tree(parent).await.unwrap();
        run.finish().await;
        assert!(run.finished.load(Ordering::Acquire));
        let early_run = RunId::from("early-cancel");
        seal_child(
            &core,
            &session,
            &early_run,
            &SilentSink,
            &invalid("cancelled before start"),
            true,
        )
        .await
        .unwrap();
        let snapshot = core
            .store()
            .unwrap()
            .projection_snapshot(&session)
            .await
            .unwrap();
        assert!(snapshot
            .runs
            .iter()
            .any(|r| r.run_id == early_run && r.state == "cancelled"));
    }
}
