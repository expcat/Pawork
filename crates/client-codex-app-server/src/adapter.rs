//! Codex App Server `ClientAdapter`：官方 Thread/Turn/Item 线协议 ↔ canonical。
//!
//! 本层是纯协议翻译：不持有 Provider 凭证、不做业务决策、不构造 Core。
//! session 映射只读复用 [`SessionRegistry`]；cwd / 事件归属经注入的
//! [`CwdResolver`] / [`SessionResolver`] 完成。未协商能力（compaction /
//! tool.namespace / experimentalApi）在使用点 `require()` 显式失败。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use agent_domain::{
    CommandId, EventId, ModelId, QueryId, RunId, SessionId, ToolCallId, WorkspaceId,
};
use async_trait::async_trait;
use client_adapter_api::{
    AdapterError, CanonicalClientRequest, CanonicalCoreFrame, CapabilitySnapshot, ClientAdapter,
    ClientAdapterFactory, ClientCapability, ClientFrame, ClientProtocol, ClientSessionId,
    ClientSessionRecord, ClientSessionState, SessionRegistry, CLIENT_ADAPTER_SCHEMA_VERSION,
};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppEvent, AppQuery, AppQueryEnvelope,
    CommandSource, API_VERSION,
};
use serde_json::Value;

use crate::map::{self, ThreadLineage};
use crate::now_timestamp;
use crate::wire::{
    self, ApprovalDecisionResult, ParamsExt, ThreadCompactParams, ThreadForkParams,
    ThreadListParams, ThreadResumeParams, ThreadStartParams, ThreadUnsubscribeParams,
    TurnInterruptParams, TurnStartParams, TurnSteerParams, DEPRECATED_THREAD_COMPACTED,
};
use crate::{HOST_AGENT_NAME, PROTOCOL_NAME, PROTOCOL_VERSION};

/// 压缩能力：`thread/compact/start` → [`AppCommand::SessionCompact`]。
pub const CAP_COMPACTION: &str = "compaction";
/// 工具命名空间：`thread/start.dynamicTools`。本 adapter 无 canonical 映射。
pub const CAP_TOOL_NAMESPACE: &str = "tool.namespace";
/// 实验 API：`initialize.capabilities.experimentalApi` 与实验过滤器。
pub const CAP_EXPERIMENTAL_API: &str = "experimentalApi";

/// 工厂默认白名单：compaction 与 experimentalApi 可协商；tool.namespace 不在列。
pub const DEFAULT_SUPPORTED_CAPABILITIES: &[&str] = &[CAP_COMPACTION, CAP_EXPERIMENTAL_API];

/// cwd → workspace 解析（宿主注入）。
#[async_trait]
pub trait CwdResolver: Send + Sync {
    async fn resolve(&self, cwd: &str) -> Result<WorkspaceId, AdapterError>;
}

/// Core 事件 → Codex thread id（宿主注入）。
#[async_trait]
pub trait SessionResolver: Send + Sync {
    async fn resolve_client_session(
        &self,
        event: &core_api::AppEventEnvelope,
    ) -> Option<ClientSessionId>;
}

/// 协商产物：concrete adapter + 被显式降级的客户端能力清单。
#[derive(Clone)]
pub struct NegotiatedCodexAdapter {
    pub adapter: Arc<CodexAppServerAdapter>,
    pub degraded: Vec<ClientCapability>,
}

/// Codex adapter factory。白名单之外的能力**显式降级**（记录而非静默丢弃），
/// 使用点再 `require()` 拒绝。
pub struct CodexAppServerAdapterFactory {
    supported_capabilities: BTreeSet<ClientCapability>,
    registry: Arc<SessionRegistry>,
    cwd_resolver: Arc<dyn CwdResolver>,
    session_resolver: Arc<dyn SessionResolver>,
}

impl CodexAppServerAdapterFactory {
    pub fn new(
        supported_capabilities: impl IntoIterator<Item = ClientCapability>,
        registry: Arc<SessionRegistry>,
        cwd_resolver: Arc<dyn CwdResolver>,
        session_resolver: Arc<dyn SessionResolver>,
    ) -> Self {
        Self {
            supported_capabilities: supported_capabilities.into_iter().collect(),
            registry,
            cwd_resolver,
            session_resolver,
        }
    }

    pub fn with_defaults(
        registry: Arc<SessionRegistry>,
        cwd_resolver: Arc<dyn CwdResolver>,
        session_resolver: Arc<dyn SessionResolver>,
    ) -> Self {
        Self::new(
            DEFAULT_SUPPORTED_CAPABILITIES
                .iter()
                .map(|name| ClientCapability::new(*name)),
            registry,
            cwd_resolver,
            session_resolver,
        )
    }

    pub fn create_concrete(
        &self,
        negotiated: CapabilitySnapshot,
    ) -> Result<NegotiatedCodexAdapter, AdapterError> {
        negotiated.validate()?;
        if negotiated.protocol != ClientProtocol::new(PROTOCOL_NAME) {
            return Err(AdapterError::ProtocolUnsupported(
                negotiated.protocol.0.clone(),
            ));
        }
        if negotiated.protocol_version != PROTOCOL_VERSION {
            return Err(AdapterError::ProtocolUnsupported(format!(
                "codex-app-server protocol version {} (only version {PROTOCOL_VERSION} is supported)",
                negotiated.protocol_version
            )));
        }
        let mut degraded = Vec::new();
        let mut capabilities = BTreeSet::new();
        for capability in &negotiated.capabilities {
            if self.supported_capabilities.contains(capability) {
                capabilities.insert(capability.clone());
            } else {
                degraded.push(capability.clone());
            }
        }
        let snapshot = CapabilitySnapshot {
            capabilities,
            ..negotiated
        };
        Ok(NegotiatedCodexAdapter {
            adapter: Arc::new(CodexAppServerAdapter {
                protocol: ClientProtocol::new(PROTOCOL_NAME),
                capabilities: snapshot,
                registry: Arc::clone(&self.registry),
                cwd_resolver: Arc::clone(&self.cwd_resolver),
                session_resolver: Arc::clone(&self.session_resolver),
            }),
            degraded,
        })
    }
}

impl ClientAdapterFactory for CodexAppServerAdapterFactory {
    fn protocol(&self) -> &ClientProtocol {
        static PROTOCOL: std::sync::LazyLock<ClientProtocol> =
            std::sync::LazyLock::new(|| ClientProtocol::new(PROTOCOL_NAME));
        &PROTOCOL
    }

    fn create(
        &self,
        negotiated: CapabilitySnapshot,
    ) -> Result<Arc<dyn ClientAdapter>, AdapterError> {
        self.create_concrete(negotiated)
            .map(|negotiated| negotiated.adapter as Arc<dyn ClientAdapter>)
    }
}

/// Codex 线协议 ↔ canonical 的翻译 adapter（无内部可变状态）。
pub struct CodexAppServerAdapter {
    protocol: ClientProtocol,
    capabilities: CapabilitySnapshot,
    registry: Arc<SessionRegistry>,
    cwd_resolver: Arc<dyn CwdResolver>,
    session_resolver: Arc<dyn SessionResolver>,
}

impl CodexAppServerAdapter {
    pub fn capabilities_snapshot(&self) -> &CapabilitySnapshot {
        &self.capabilities
    }

    pub fn command_envelope(&self, request_id: &str, command: AppCommand) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from(format!("codex-{request_id}")),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: HOST_AGENT_NAME.into(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: now_timestamp(),
            command,
        }
    }

    pub fn query_envelope(&self, request_id: &str, query: AppQuery) -> AppQueryEnvelope {
        AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from(format!("codex-{request_id}")),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: HOST_AGENT_NAME.into(),
            },
            issued_at: now_timestamp(),
            query,
        }
    }

    /// 客户端审批响应 → [`AppCommand::ToolApprove`]。
    pub fn decode_approval_response(
        &self,
        result: Value,
        run_id: &str,
        item_id: &str,
        request_id: &str,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let parsed: ApprovalDecisionResult = serde_json::from_value(result)
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        parsed
            .reject_unknown("item/commandExecution/requestApproval result")
            .map_err(AdapterError::InvalidFrame)?;
        Ok(CanonicalClientRequest::Command(self.command_envelope(
            request_id,
            AppCommand::ToolApprove {
                run_id: RunId::from(run_id),
                tool_call_id: ToolCallId::from(item_id),
                decision: map::approval_decision(&parsed.decision),
            },
        )))
    }

    pub async fn approval_request(
        &self,
        event: &AppEvent,
        thread_id: &str,
    ) -> Result<wire::CommandApprovalParams, AdapterError> {
        map::approval_request(event, thread_id)
    }

    /// 宿主把 Core 事件路由到 Codex thread id。
    pub async fn resolve_thread(
        &self,
        envelope: &core_api::AppEventEnvelope,
    ) -> Option<ClientSessionId> {
        self.session_resolver.resolve_client_session(envelope).await
    }

    async fn decode_thread_start(
        &self,
        frame: &ClientFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params: ThreadStartParams = serde_json::from_value(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("thread/start")
            .map_err(AdapterError::InvalidFrame)?;
        if params.dynamic_tools.is_some() {
            self.require(&ClientCapability::new(CAP_TOOL_NAMESPACE))?;
            return Err(AdapterError::ProtocolUnsupported(
                "thread/start.dynamicTools (tool.namespace has no canonical mapping)".into(),
            ));
        }
        let workspace_id = self
            .cwd_resolver
            .resolve(require_absolute_cwd(&params.cwd)?)
            .await?;
        Ok(CanonicalClientRequest::Command(self.command_envelope(
            &frame.request_id,
            AppCommand::SessionCreate {
                workspace_id,
                title: Some(params.cwd),
            },
        )))
    }

    async fn decode_thread_resume(
        &self,
        frame: &ClientFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params: ThreadResumeParams = serde_json::from_value(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("thread/resume")
            .map_err(AdapterError::InvalidFrame)?;
        if params.thread_id.trim().is_empty() {
            return Err(AdapterError::InvalidFrame(
                "threadId must be non-empty".into(),
            ));
        }
        let id = ClientSessionId::new(&params.thread_id);
        if let Some(record) = self.registry.get(&id).await {
            return Ok(CanonicalClientRequest::Reattach {
                client_session_id: record.client_session_id.clone(),
                ownership_epoch: record.ownership_epoch,
                revision: record.revision,
                connection_id: record.connection_id.clone(),
                state: ClientSessionState::Subscribed,
                updated_at: now_timestamp(),
            });
        }
        Ok(CanonicalClientRequest::Command(self.command_envelope(
            &frame.request_id,
            AppCommand::SessionOpen {
                session_id: SessionId::from(params.thread_id.as_str()),
            },
        )))
    }

    async fn decode_thread_fork(
        &self,
        frame: &ClientFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params: ThreadForkParams = serde_json::from_value(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("thread/fork")
            .map_err(AdapterError::InvalidFrame)?;
        if params.thread_id.trim().is_empty() {
            return Err(AdapterError::InvalidFrame(
                "threadId must be non-empty".into(),
            ));
        }
        let parent_event_id = EventId::from(
            params
                .last_turn_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .unwrap_or("codex-fork-head"),
        );
        Ok(CanonicalClientRequest::Command(self.command_envelope(
            &frame.request_id,
            AppCommand::SessionFork {
                session_id: SessionId::from(params.thread_id.as_str()),
                parent_event_id,
            },
        )))
    }

    async fn decode_thread_list(
        &self,
        frame: &ClientFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params: ThreadListParams = serde_json::from_value(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        if params.parent_thread_id.is_some() || params.ancestor_thread_id.is_some() {
            self.require(&ClientCapability::new(CAP_EXPERIMENTAL_API))?;
        }
        if params.parent_thread_id.is_some() && params.ancestor_thread_id.is_some() {
            return Err(AdapterError::InvalidFrame(
                "parentThreadId and ancestorThreadId are mutually exclusive".into(),
            ));
        }
        params
            .reject_unknown("thread/list")
            .map_err(AdapterError::InvalidFrame)?;
        Err(AdapterError::ProtocolUnsupported(
            "thread/list (no canonical session list query)".into(),
        ))
    }

    async fn decode_thread_unsubscribe(
        &self,
        frame: &ClientFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params: ThreadUnsubscribeParams = serde_json::from_value(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("thread/unsubscribe")
            .map_err(AdapterError::InvalidFrame)?;
        let record = self.attached_record(&params.thread_id).await?;
        Ok(CanonicalClientRequest::Disconnect {
            client_session_id: record.client_session_id.clone(),
            ownership_epoch: record.ownership_epoch,
            revision: record.revision,
            updated_at: now_timestamp(),
        })
    }

    async fn decode_thread_compact(
        &self,
        frame: &ClientFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        self.require(&ClientCapability::new(CAP_COMPACTION))?;
        let params: ThreadCompactParams = serde_json::from_value(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("thread/compact/start")
            .map_err(AdapterError::InvalidFrame)?;
        let record = self.attached_record(&params.thread_id).await?;
        Ok(CanonicalClientRequest::Command(self.command_envelope(
            &frame.request_id,
            AppCommand::SessionCompact {
                session_id: record.core_session_id,
            },
        )))
    }

    async fn decode_turn_start(
        &self,
        frame: &ClientFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params: TurnStartParams = serde_json::from_value(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("turn/start")
            .map_err(AdapterError::InvalidFrame)?;
        let record = self.attached_record(&params.thread_id).await?;
        let user_message = map::extract_user_message(&params.input)?;
        Ok(CanonicalClientRequest::Command(self.command_envelope(
            &frame.request_id,
            AppCommand::RunStart {
                session_id: record.core_session_id,
                user_message,
                model: params.model.map(ModelId::from),
                profile: None,
            },
        )))
    }

    async fn decode_turn_steer(
        &self,
        frame: &ClientFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params: TurnSteerParams = serde_json::from_value(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("turn/steer")
            .map_err(AdapterError::InvalidFrame)?;
        let _record = self.attached_record(&params.thread_id).await?;
        let _user_message = map::extract_user_message(&params.input)?;
        // Core 没有「向飞行中 turn 注入」命令。把 steer 伪装成新 RunStart
        // 会丢掉官方语义（同一 turn 追加输入），因此显式失败，禁止静默降级。
        Err(AdapterError::ProtocolUnsupported(
            "turn/steer has no canonical in-flight turn injection".into(),
        ))
    }

    async fn decode_turn_interrupt(
        &self,
        frame: &ClientFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params: TurnInterruptParams = serde_json::from_value(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("turn/interrupt")
            .map_err(AdapterError::InvalidFrame)?;
        self.attached_record(&params.thread_id).await?;
        if params.turn_id.trim().is_empty() {
            return Err(AdapterError::InvalidFrame(
                "turnId must be non-empty".into(),
            ));
        }
        Ok(CanonicalClientRequest::Command(self.command_envelope(
            &frame.request_id,
            AppCommand::RunCancel {
                run_id: RunId::from(params.turn_id.as_str()),
            },
        )))
    }

    async fn attached_record(
        &self,
        client_session_id: &str,
    ) -> Result<ClientSessionRecord, AdapterError> {
        let id = ClientSessionId::new(client_session_id);
        let record = self
            .registry
            .get(&id)
            .await
            .ok_or_else(|| AdapterError::UnknownSession(id.clone()))?;
        if record.state == ClientSessionState::Disconnected {
            return Err(AdapterError::SessionNotAttached(id));
        }
        Ok(record)
    }
}

fn require_absolute_cwd(cwd: &str) -> Result<&str, AdapterError> {
    if cwd.trim().is_empty() {
        return Err(AdapterError::InvalidFrame(
            "cwd must be a non-empty absolute path".into(),
        ));
    }
    if !Path::new(cwd).is_absolute() {
        return Err(AdapterError::InvalidFrame(format!(
            "cwd `{cwd}` must be an absolute path"
        )));
    }
    Ok(cwd)
}

fn reject_frame_extensions(frame: &ClientFrame) -> Result<(), AdapterError> {
    if frame.extensions.is_empty() {
        Ok(())
    } else {
        Err(AdapterError::InvalidFrame(format!(
            "unsupported fields: {}",
            frame
                .extensions
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(",")
        )))
    }
}

#[async_trait]
impl ClientAdapter for CodexAppServerAdapter {
    fn protocol(&self) -> &ClientProtocol {
        &self.protocol
    }

    fn capabilities(&self) -> &CapabilitySnapshot {
        &self.capabilities
    }

    async fn decode_payload(
        &self,
        frame: ClientFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        reject_frame_extensions(&frame)?;
        match frame.method.as_str() {
            "thread/start" => self.decode_thread_start(&frame).await,
            "thread/resume" => self.decode_thread_resume(&frame).await,
            "thread/fork" => self.decode_thread_fork(&frame).await,
            "thread/list" => self.decode_thread_list(&frame).await,
            "thread/unsubscribe" => self.decode_thread_unsubscribe(&frame).await,
            "thread/compact/start" => self.decode_thread_compact(&frame).await,
            "turn/start" => self.decode_turn_start(&frame).await,
            "turn/steer" => self.decode_turn_steer(&frame).await,
            "turn/interrupt" => self.decode_turn_interrupt(&frame).await,
            "initialize" | "initialized" => Err(AdapterError::InvalidFrame(
                "initialize/initialized is a handshake method handled by the Codex host, not a canonical request"
                    .into(),
            )),
            DEPRECATED_THREAD_COMPACTED => Err(AdapterError::ProtocolUnsupported(
                "thread/compacted is deprecated; use thread/compact/start + contextCompaction item"
                    .into(),
            )),
            other => Err(AdapterError::ProtocolUnsupported(other.into())),
        }
    }

    async fn encode_payload(&self, frame: CanonicalCoreFrame) -> Result<ClientFrame, AdapterError> {
        match frame {
            CanonicalCoreFrame::Response(envelope) => Ok(ClientFrame {
                schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
                request_id: envelope.request_id.as_str().to_string(),
                method: "codex.response".into(),
                payload: serde_json::to_value(&envelope)
                    .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?,
                extensions: Default::default(),
            }),
            CanonicalCoreFrame::Error(frame) => Ok(ClientFrame {
                schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
                request_id: "adapter-error".into(),
                method: "codex.error".into(),
                payload: serde_json::to_value(&frame)
                    .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?,
                extensions: Default::default(),
            }),
            CanonicalCoreFrame::SessionState(record) => Ok(ClientFrame {
                schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
                request_id: record.client_session_id.0.clone(),
                method: "codex.session_state".into(),
                payload: map::thread_result(
                    record.client_session_id.0.as_str(),
                    &ThreadLineage::default(),
                ),
                extensions: Default::default(),
            }),
            CanonicalCoreFrame::Event(envelope) => {
                if matches!(envelope.payload, AppEvent::ToolApprovalRequired { .. }) {
                    return Err(AdapterError::InvalidFrame(
                        "tool approval requires host request correlation; use CodexAppServerAdapter::approval_request"
                            .into(),
                    ));
                }
                let Some(client_session_id) = self
                    .session_resolver
                    .resolve_client_session(&envelope)
                    .await
                else {
                    return Err(AdapterError::HostUnavailable(format!(
                        "core event `{}` is not routable to a Codex thread",
                        map::app_event_kind(&envelope.payload)
                    )));
                };
                let Some((method, params)) =
                    map::translate_event(&envelope.payload, client_session_id.0.as_str())?
                else {
                    return Err(AdapterError::InvalidFrame(format!(
                        "core event `{}` has no Codex app-server representation",
                        map::app_event_kind(&envelope.payload)
                    )));
                };
                Ok(ClientFrame {
                    schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
                    request_id: envelope.event_id.as_str().to_string(),
                    method,
                    payload: params,
                    extensions: Default::default(),
                })
            }
        }
    }
}
