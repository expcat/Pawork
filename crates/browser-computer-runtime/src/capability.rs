use std::sync::{Arc, Mutex};

use agent_domain::{CancellationToken, ServerToolEvent, ToolCallId, WorkspaceId};
use artifact_store::ArtifactStore;
use policy_engine::{ApprovalMode, PolicyEngine};
use sandbox_runtime::SandboxBackend;
use serde_json::Value;

use crate::action::{BrowserComputerAction, BrowserComputerSnapshot};
use crate::artifact::{self, DEFAULT_LARGE_PAYLOAD_BYTES};
use crate::audit::{AuditRecord, AuditSink};
use crate::backend::{reject_non_client_function_for_local, BrowserComputerBackend, ExecutionSite};
use crate::backends::provider_hosted::HostedComputerEventEmitter;
use crate::error::BrowserComputerError;
use crate::policy::{self, BrowserComputerAudit};
use crate::selector::{self, BackendSelection, SelectionPolicy};

/// Browser / Computer 统一能力 facade（P17-10）。
///
/// 收敛 Local / Playwright / MCP / ProviderHosted 四档后端，按 canonical 执行位点
/// 路由（不读 Provider 名）：
///
/// - **ClientFunction**（Local / Playwright / 本地 MCP）→ `act_local`，走 Core 工具调度；
/// - **ProviderHosted**（provider computer use）→ `hosted_request`，走 `ServerToolEvent`，
///   **绝不**进入本地 `AgentTool::execute()`；
/// - **ProviderExtension**（Provider-mediated MCP）→ 由 Provider transcript 续接，不在本地执行。
///
/// 所有操作经 `policy-engine` 审批与审计；本地（Core-owned）子进程经注入的
/// `SandboxBackend` 隔离执行；跨 trust boundary 的降级必须显式且符合 Policy。
pub struct BrowserComputerCapability {
    inner: Arc<Inner>,
}

struct Inner {
    backends: Vec<Arc<dyn BrowserComputerBackend>>,
    hosted_emitter: Option<Arc<dyn HostedComputerEventEmitter>>,
    policy_engine: PolicyEngine,
    trusted: bool,
    approval_mode: ApprovalMode,
    selection: SelectionPolicy,
    artifact_store: Option<ArtifactStore>,
    large_payload_bytes: u64,
    sandbox: Option<Arc<dyn SandboxBackend>>,
    audit_sink: Option<Arc<dyn AuditSink>>,
    last_audit: Mutex<Option<BrowserComputerAudit>>,
}
impl Inner {
    /// 记录审计。配置了 durable sink 时，append 失败即失败（fail-closed）：
    /// 调用方（allow 路径）必须在副作用前调用并传播错误。
    fn record_audit(&self, audit: BrowserComputerAudit) -> Result<(), BrowserComputerError> {
        if let Some(sink) = self.audit_sink.as_ref() {
            match sink.append(&audit) {
                Ok(record) => {
                    let AuditRecord { audit, .. } = record;
                    self.set_last_audit(audit);
                    return Ok(());
                }
                Err(err) => {
                    // 内存最近记录仍保留（进程内可观测），但错误必须向上传播。
                    self.set_last_audit(audit);
                    return Err(BrowserComputerError::AuditSink(err.to_string()));
                }
            }
        }
        self.set_last_audit(audit);
        Ok(())
    }

    /// 尽力记录（deny / 错误路径使用）：失败仅告警，不改变既有结果。
    /// 这些路径本身已无副作用（deny 或执行已失败），不存在「成功副作用未审计」。
    fn record_audit_best_effort(&self, audit: BrowserComputerAudit) {
        if let Err(err) = self.record_audit(audit) {
            tracing::error!(
                target: "pawork.browser_computer.audit",
                error = %err,
                "audit append failed on best-effort path"
            );
        }
    }

    fn set_last_audit(&self, audit: BrowserComputerAudit) {
        if let Ok(mut slot) = self.last_audit.lock() {
            *slot = Some(audit);
        }
    }
}

impl BrowserComputerCapability {
    /// 装配构造器。
    pub fn builder() -> BrowserComputerCapabilityBuilder {
        BrowserComputerCapabilityBuilder::default()
    }

    /// 已注册的后端（按位点路由；审计用）。
    pub fn backends(&self) -> &[Arc<dyn BrowserComputerBackend>] {
        &self.inner.backends
    }

    /// 最近一次后端选择 / action 的审计记录（可观测回退与跨 trust 降级的证据）。
    pub fn last_audit(&self) -> Option<BrowserComputerAudit> {
        self.inner
            .last_audit
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// 装配时注入的 Core-owned sandbox（若配置）。
    pub fn sandbox(&self) -> Option<&Arc<dyn SandboxBackend>> {
        self.inner.sandbox.as_ref()
    }

    /// 为本地（ClientFunction）执行选择后端。
    ///
    /// `ProviderHosted` 后端从不得被选中；选择失败返回 [`BrowserComputerError::NoLocalBackend`]。
    pub fn select_for_local(&self) -> Result<BackendSelection, BrowserComputerError> {
        selector::select_for_local(&self.inner.backends)
            .map_err(|attempted| selector::no_local_backend_error(&attempted))
    }

    /// 本地执行一个动作（ClientFunction 路径）。
    ///
    /// 流程：policy 审批 → 选择本地后端（ProviderHosted 永不选中）→ **副作用前**
    /// 落盘「allow」审计（sink 失败即失败，不执行）→ 后端执行 → 大 payload 归一为
    /// artifact 引用。执行失败追加 best-effort 失败审计。本地无后端时按 Policy
    /// 决定是否显式跨 trust 降级到 hosted（返回 `HostedFallbackRequired`）。
    pub async fn act_local(
        &self,
        action: BrowserComputerAction,
        workspace_id: &WorkspaceId,
        cancel: CancellationToken,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        if cancel.is_cancelled() {
            return Err(BrowserComputerError::Cancelled);
        }

        let input_value = serde_json::to_value(&action).unwrap_or(Value::Null);
        let policy_input = policy::policy_input_for(
            &action,
            &input_value,
            self.inner.trusted,
            self.inner.approval_mode,
        );
        let decision = self.inner.policy_engine.decide(&policy_input);
        let _constraints = match policy::enforce_decision(decision.clone()) {
            Ok(constraints) => constraints,
            Err(err) => {
                self.inner.record_audit_best_effort(BrowserComputerAudit {
                    action: action.label().into(),
                    backend: None,
                    site: None,
                    trust: None,
                    cross_trust_fallback: false,
                    policy: "deny".into(),
                    note: err.to_string(),
                });
                return Err(err);
            }
        };

        let selection = match self.select_for_local() {
            Ok(selection) => selection,
            Err(BrowserComputerError::NoLocalBackend) => {
                return self.resolve_no_local(action.label());
            }
            Err(other) => return Err(other),
        };

        // 运行期硬门：防御性地拒绝任何非 ClientFunction 位点进入本地路径
        // （selector 已保证，此处二次断言；ProviderExtension / ProviderHosted 均拒绝）。
        let Some(backend) = self
            .inner
            .backends
            .iter()
            .find(|b| b.descriptor_name() == selection.route.descriptor_name)
        else {
            return Err(BrowserComputerError::NoLocalBackend);
        };
        reject_non_client_function_for_local(backend.as_ref())?;

        // 副作用前落盘「allow」审计：sink 失败 → 操作失败，driver 不可达。
        self.inner.record_audit(selection.audit(
            action.label(),
            "allow",
            "local execution (durable audit before side effect)",
        ))?;

        match backend.act(action.clone(), workspace_id, cancel).await {
            Ok(snapshot) => {
                let snapshot = artifact::normalize_snapshot(
                    snapshot,
                    self.inner.artifact_store.as_ref(),
                    self.inner.large_payload_bytes,
                )
                .await;
                Ok(snapshot)
            }
            Err(err) => {
                // 执行失败：追加 best-effort 失败审计（成功副作用已不可能发生）。
                self.inner.record_audit_best_effort(BrowserComputerAudit {
                    action: action.label().into(),
                    backend: Some(selection.route.kind.as_str().into()),
                    site: Some(selection.route.site.as_str().into()),
                    trust: Some(selection.route.trust.as_str().into()),
                    cross_trust_fallback: false,
                    policy: "allow".into(),
                    note: format!("execution failed: {err}"),
                });
                Err(err)
            }
        }
    }

    /// 本地读取快照（ClientFunction 路径；便捷封装）。
    pub async fn snapshot_local(
        &self,
        workspace_id: &WorkspaceId,
        cancel: CancellationToken,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        if cancel.is_cancelled() {
            return Err(BrowserComputerError::Cancelled);
        }

        // 快照以只读 canonical action 走同一 Policy 审批（policy + audit 强制）。
        let action = BrowserComputerAction::Screenshot;
        let input_value = serde_json::to_value(&action).unwrap_or(Value::Null);
        let policy_input = policy::policy_input_for(
            &action,
            &input_value,
            self.inner.trusted,
            self.inner.approval_mode,
        );
        let decision = self.inner.policy_engine.decide(&policy_input);
        let _constraints = match policy::enforce_decision(decision.clone()) {
            Ok(constraints) => constraints,
            Err(err) => {
                self.inner.record_audit_best_effort(BrowserComputerAudit {
                    action: "snapshot_local".into(),
                    backend: None,
                    site: None,
                    trust: None,
                    cross_trust_fallback: false,
                    policy: "deny".into(),
                    note: err.to_string(),
                });
                return Err(err);
            }
        };

        let selection = match self.select_for_local() {
            Ok(selection) => selection,
            Err(BrowserComputerError::NoLocalBackend) => {
                return self.resolve_no_local("snapshot_local");
            }
            Err(other) => return Err(other),
        };
        let backend = self
            .inner
            .backends
            .iter()
            .find(|b| b.descriptor_name() == selection.route.descriptor_name)
            .ok_or(BrowserComputerError::NoLocalBackend)?;
        reject_non_client_function_for_local(backend.as_ref())?;

        // 副作用前落盘「allow」审计：sink 失败 → 操作失败，driver 不可达。
        self.inner.record_audit(selection.audit(
            "snapshot_local",
            "allow",
            "local snapshot (read-only; durable audit before side effect)",
        ))?;

        match backend.snapshot(workspace_id, cancel).await {
            Ok(snapshot) => {
                let snapshot = artifact::normalize_snapshot(
                    snapshot,
                    self.inner.artifact_store.as_ref(),
                    self.inner.large_payload_bytes,
                )
                .await;
                Ok(snapshot)
            }
            Err(err) => {
                self.inner.record_audit_best_effort(BrowserComputerAudit {
                    action: "snapshot_local".into(),
                    backend: Some(selection.route.kind.as_str().into()),
                    site: Some(selection.route.site.as_str().into()),
                    trust: Some(selection.route.trust.as_str().into()),
                    cross_trust_fallback: false,
                    policy: "allow".into(),
                    note: format!("execution failed: {err}"),
                });
                Err(err)
            }
        }
    }

    /// 本地无可用后端时，按 Policy 决定是否显式跨 trust 降级到 hosted。
    fn resolve_no_local(
        &self,
        action_label: &'static str,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        let hosted = selector::find_hosted(&self.inner.backends);
        match (hosted, self.inner.selection.allow_cross_trust_fallback) {
            (Some(route), true) => {
                self.inner.record_audit_best_effort(BrowserComputerAudit {
                    action: action_label.into(),
                    backend: Some(route.kind.as_str().into()),
                    site: Some(route.site.as_str().into()),
                    trust: Some(route.trust.as_str().into()),
                    cross_trust_fallback: true,
                    policy: "cross_trust_fallback".into(),
                    note: "no local backend; degrading to provider-hosted via ServerToolEvent"
                        .into(),
                });
                tracing::info!(
                    target: "pawork.browser_computer",
                    action = action_label,
                    hosted = route.descriptor_name,
                    "cross-trust fallback to provider-hosted (explicit, policy-permitted)"
                );
                Err(BrowserComputerError::HostedFallbackRequired {
                    attempted: route.descriptor_name.to_string(),
                })
            }
            (Some(route), false) => {
                self.inner.record_audit_best_effort(BrowserComputerAudit {
                    action: action_label.into(),
                    backend: Some(route.kind.as_str().into()),
                    site: Some(route.site.as_str().into()),
                    trust: Some(route.trust.as_str().into()),
                    cross_trust_fallback: true,
                    policy: "deny".into(),
                    note: "cross-trust fallback available but disallowed by policy".into(),
                });
                Err(BrowserComputerError::CrossTrustFallbackDenied {
                    attempted: route.descriptor_name.to_string(),
                })
            }
            (None, _) => Err(BrowserComputerError::NoLocalBackend),
        }
    }

    /// Provider-hosted 路径：把 canonical action 翻译为 `ServerToolEvent` 序列。
    ///
    /// **不**经过本地 `AgentTool::execute()`；由 provider 适配器 / agent loop 在
    /// hosted 调用处直接调用，事件注入 transcript。与本地路径一样经 Policy
    /// 审批并落审计。
    pub fn hosted_request(
        &self,
        action: &BrowserComputerAction,
        tool_call_id: &ToolCallId,
    ) -> Result<Vec<ServerToolEvent>, BrowserComputerError> {
        let input_value = serde_json::to_value(action).unwrap_or(Value::Null);
        let policy_input = policy::policy_input_for(
            action,
            &input_value,
            self.inner.trusted,
            self.inner.approval_mode,
        );
        let decision = self.inner.policy_engine.decide(&policy_input);
        let _constraints = match policy::enforce_decision(decision.clone()) {
            Ok(constraints) => constraints,
            Err(err) => {
                self.inner.record_audit_best_effort(BrowserComputerAudit {
                    action: action.label().into(),
                    backend: Some("provider_hosted".into()),
                    site: Some(ExecutionSite::ProviderHosted.as_str().into()),
                    trust: Some("externally_owned".into()),
                    cross_trust_fallback: false,
                    policy: "deny".into(),
                    note: err.to_string(),
                });
                return Err(err);
            }
        };

        let emitter =
            self.inner
                .hosted_emitter
                .as_ref()
                .ok_or_else(|| BrowserComputerError::Backend {
                    backend: "provider_hosted",
                    message: "no hosted emitter configured".into(),
                })?;
        // 返回事件（副作用）之前落盘「hosted」审计：sink 失败 → 调用方拿不到事件。
        self.inner.record_audit(BrowserComputerAudit {
            action: action.label().into(),
            backend: Some("provider_hosted".into()),
            site: Some(ExecutionSite::ProviderHosted.as_str().into()),
            trust: Some("externally_owned".into()),
            cross_trust_fallback: false,
            policy: "hosted".into(),
            note: "lifecycle via ServerToolEvent; never enters local execute".into(),
        })?;
        let events = emitter.emit_action(action, tool_call_id);
        Ok(events)
    }
}
/// [`BrowserComputerCapability`] 装配器。
#[derive(Default)]
pub struct BrowserComputerCapabilityBuilder {
    backends: Vec<Arc<dyn BrowserComputerBackend>>,
    hosted_emitter: Option<Arc<dyn HostedComputerEventEmitter>>,
    approval_mode: Option<ApprovalMode>,
    trusted: Option<bool>,
    selection: Option<SelectionPolicy>,
    artifact_store: Option<ArtifactStore>,
    large_payload_bytes: Option<u64>,
    sandbox: Option<Arc<dyn SandboxBackend>>,
    audit_sink: Option<Arc<dyn AuditSink>>,
}

impl BrowserComputerCapabilityBuilder {
    /// 追加一个后端（位点由后端自身声明）。
    pub fn backend(mut self, backend: Arc<dyn BrowserComputerBackend>) -> Self {
        self.backends.push(backend);
        self
    }

    /// 设置 provider-hosted 事件发射器（hosted 路径所必需）。
    pub fn hosted_emitter(mut self, emitter: Arc<dyn HostedComputerEventEmitter>) -> Self {
        self.hosted_emitter = Some(emitter);
        self
    }

    /// 审批模式（缺省 `NeverAsk`，仅用于 Policy 输入默认值）。
    pub fn approval_mode(mut self, mode: ApprovalMode) -> Self {
        self.approval_mode = Some(mode);
        self
    }

    /// 工作区是否可信（缺省 `false`）。
    pub fn trusted(mut self, trusted: bool) -> Self {
        self.trusted = Some(trusted);
        self
    }

    /// 选择策略（跨 trust 降级闸门；缺省禁止）。
    pub fn selection(mut self, selection: SelectionPolicy) -> Self {
        self.selection = Some(selection);
        self
    }

    /// 注入 artifact-store（大 payload 归一为引用）。
    pub fn artifact_store(mut self, store: ArtifactStore) -> Self {
        self.artifact_store = Some(store);
        self
    }

    /// 大 payload 阈值（缺省 16 KiB）。
    pub fn large_payload_bytes(mut self, bytes: u64) -> Self {
        self.large_payload_bytes = Some(bytes);
        self
    }

    /// 注入 Core-owned sandbox：`build()` 时统一注入所有进程型后端
    /// （Local / Playwright / 本地 MCP）的 [`crate::process::SandboxGate`]。
    pub fn sandbox(mut self, sandbox: Arc<dyn SandboxBackend>) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    /// 注入 durable audit sink：每次后端选择 / 执行 / hosted 事件落盘，可跨重启 replay。
    pub fn audit_sink(mut self, sink: Arc<dyn AuditSink>) -> Self {
        self.audit_sink = Some(sink);
        self
    }

    /// 装配 facade。
    pub fn build(self) -> BrowserComputerCapability {
        let approval_mode = self.approval_mode.unwrap_or_default();
        // 统一注入：装配期把 sandbox 注入全部后端（进程型后端覆盖注入，其余 no-op）。
        if let Some(sandbox) = self.sandbox.as_ref() {
            for backend in &self.backends {
                backend.inject_sandbox(sandbox.clone());
            }
        }
        BrowserComputerCapability {
            inner: Arc::new(Inner {
                backends: self.backends,
                hosted_emitter: self.hosted_emitter,
                policy_engine: PolicyEngine::new(approval_mode),
                trusted: self.trusted.unwrap_or(false),
                approval_mode,
                selection: self.selection.unwrap_or_default(),
                artifact_store: self.artifact_store,
                large_payload_bytes: self
                    .large_payload_bytes
                    .unwrap_or(DEFAULT_LARGE_PAYLOAD_BYTES),
                sandbox: self.sandbox,
                audit_sink: self.audit_sink,
                last_audit: Mutex::new(None),
            }),
        }
    }
}
