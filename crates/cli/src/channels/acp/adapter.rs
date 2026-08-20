//! ACP ClientAdapter：把 ACP v1 请求/事件显式映射到 canonical
//! `pawork-protocol` adapter 帧。
//!
//! 本层是纯协议翻译：不持有 Provider 凭证、不做业务决策、不构造 Core。
//! session 映射只读复用 [`SessionRegistry`] 的 authoritative 记录（epoch/revision/
//! connection/state 全部来自 registry，不自建 ownership）；workspace 解析与
//! 事件路由分别经注入的 [`CwdResolver`] / [`SessionResolver`] 完成，由宿主胶水
//! 经 [`crate::channels::acp::AcpCommandHost`] 实现。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use pawork_domain::{CommandId, QueryId, WorkspaceId};
use pawork_protocol::adapter::{
    AdapterError, AdapterWireFrame, CanonicalClientRequest, CanonicalCoreFrame, CapabilitySnapshot,
    ClientAdapter, ClientAdapterFactory, ClientCapability, ClientProtocol, ClientSessionId,
    ClientSessionRecord, ClientSessionState, SessionRegistry, CLIENT_ADAPTER_SCHEMA_VERSION,
};
use pawork_protocol::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery,
    AppQueryEnvelope, CommandSource, API_VERSION,
};
use pawork_protocol::app::registry::command_entry;
use serde_json::Value;

use crate::channels::acp::map;
use crate::channels::acp::now_timestamp;
use crate::channels::acp::wire::{
    Implementation, ParamsExt, SessionCancelParams, SessionCloseParams, SessionNewParams,
    SessionPromptParams, SessionResumeParams, SessionUpdateParams,
};

/// ACP 协议名（registry / capability snapshot 的权威标识）。
pub const ACP_PROTOCOL: &str = "acp";

/// 首轮由宿主支持的客户端能力白名单：为空 = 客户端声明的能力全部显式降级记录，
/// 使用点（如 mcpServers）再显式拒绝。后续轮次在此扩展。
pub const ACP_SUPPORTED_CAPABILITIES: &[&str] = &[];

/// cwd → workspace 解析（宿主注入；规则：cwd 必须位于已登记 workspace root 内）。
#[async_trait]
pub trait CwdResolver: Send + Sync {
    async fn resolve(&self, cwd: &str) -> Result<WorkspaceId, AdapterError>;
}

/// Core 事件 → 归属的 ACP 客户端会话（宿主注入，读 pending 运行表）。
#[async_trait]
pub trait SessionResolver: Send + Sync {
    async fn resolve_client_session(&self, event: &AppEventEnvelope) -> Option<ClientSessionId>;
}

/// `session/cancel` 通知解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelTarget {
    pub client_session_id: ClientSessionId,
}

/// `session/request_permission` 响应解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    Selected { option_id: String },
    Cancelled,
}

/// 协商产物：adapter + 被显式降级的客户端能力清单。
#[derive(Clone)]
pub struct NegotiatedAcpAdapter {
    pub adapter: Arc<AcpClientAdapter>,
    pub degraded: Vec<ClientCapability>,
}

/// ACP adapter factory。能力白名单之外的能力**显式降级**（记录而非静默丢弃），
/// 与 mock factory 的硬拒绝互补：ACP 能力是可选扩展点，拒绝整个握手会破坏
/// 前向兼容，因此降级 + 使用点拒绝。
pub struct AcpClientAdapterFactory {
    supported_capabilities: BTreeSet<ClientCapability>,
    registry: Arc<SessionRegistry>,
    cwd_resolver: Arc<dyn CwdResolver>,
    session_resolver: Arc<dyn SessionResolver>,
    client_info: Implementation,
}

impl AcpClientAdapterFactory {
    pub fn new(
        supported_capabilities: impl IntoIterator<Item = ClientCapability>,
        registry: Arc<SessionRegistry>,
        cwd_resolver: Arc<dyn CwdResolver>,
        session_resolver: Arc<dyn SessionResolver>,
        client_info: Implementation,
    ) -> Self {
        Self {
            supported_capabilities: supported_capabilities.into_iter().collect(),
            registry,
            cwd_resolver,
            session_resolver,
            client_info,
        }
    }

    /// 宿主使用的具体协商入口（返回 concrete adapter + 降级清单）。
    pub fn create_concrete(
        &self,
        negotiated: CapabilitySnapshot,
    ) -> Result<NegotiatedAcpAdapter, AdapterError> {
        negotiated.validate()?;
        if negotiated.protocol != ClientProtocol::new(ACP_PROTOCOL) {
            return Err(AdapterError::ProtocolUnsupported(
                negotiated.protocol.0.clone(),
            ));
        }
        if negotiated.protocol_version != "1" {
            return Err(AdapterError::ProtocolUnsupported(format!(
                "acp protocol version {} (only wire protocolVersion 1 is supported)",
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
        Ok(NegotiatedAcpAdapter {
            adapter: Arc::new(AcpClientAdapter {
                capabilities: snapshot,
                client_info: self.client_info.clone(),
                registry: Arc::clone(&self.registry),
                cwd_resolver: Arc::clone(&self.cwd_resolver),
                session_resolver: Arc::clone(&self.session_resolver),
            }),
            degraded,
        })
    }

    /// 解析 resume 的 `cwd` → workspace（P17-7 幂等 materialize 用；与
    /// `session/new` 的 decode 走同一解析路径：绝对路径校验 + 组件级前缀
    /// 匹配，不在此重建 workspace 状态）。
    pub async fn resolve_workspace(&self, cwd: &str) -> Result<WorkspaceId, AdapterError> {
        self.cwd_resolver.resolve(require_absolute_cwd(cwd)?).await
    }
}

impl ClientAdapterFactory for AcpClientAdapterFactory {
    fn protocol(&self) -> &ClientProtocol {
        static PROTOCOL: std::sync::LazyLock<ClientProtocol> =
            std::sync::LazyLock::new(|| ClientProtocol::new(ACP_PROTOCOL));
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

/// ACP 线协议 ↔ canonical 的翻译 adapter（无内部可变状态）。
pub struct AcpClientAdapter {
    capabilities: CapabilitySnapshot,
    client_info: Implementation,
    registry: Arc<SessionRegistry>,
    cwd_resolver: Arc<dyn CwdResolver>,
    session_resolver: Arc<dyn SessionResolver>,
}

impl AcpClientAdapter {
    pub fn capabilities_snapshot(&self) -> &CapabilitySnapshot {
        &self.capabilities
    }

    pub fn client_info(&self) -> &Implementation {
        &self.client_info
    }

    /// `session/cancel` 是 JSON-RPC 通知（无响应），经此入口解析。
    pub async fn decode_cancel(&self, params: Value) -> Result<CancelTarget, AdapterError> {
        let params = serde_json::from_value::<SessionCancelParams>(params)
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("session/cancel")
            .map_err(AdapterError::InvalidFrame)?;
        if params.session_id.trim().is_empty() {
            return Err(AdapterError::InvalidFrame(
                "sessionId must be non-empty".into(),
            ));
        }
        Ok(CancelTarget {
            client_session_id: ClientSessionId::new(params.session_id),
        })
    }

    /// `session/request_permission` 响应（client → agent 的 JSON-RPC response）解析。
    pub fn decode_permission_response(
        &self,
        result: Value,
    ) -> Result<PermissionDecision, AdapterError> {
        // ACP v1 官方响应是嵌套形状：
        //   `{"outcome":{"outcome":"selected","optionId":"..."}}`
        //   `{"outcome":{"outcome":"cancelled"}}`
        // serde 的 internally-tagged enum 无法表达 variant 专属字段（outcome
        // 标签在 enum 级），故手工解析并显式拒绝未知 outcome / 未知字段
        // （-32602）。
        fn reject_unknown(fields: &serde_json::Map<String, Value>) -> Result<(), AdapterError> {
            let unknown: Vec<&String> = fields
                .keys()
                .filter(|key| key.as_str() != crate::channels::acp::wire::META_FIELD)
                .collect();
            if unknown.is_empty() {
                Ok(())
            } else {
                Err(AdapterError::InvalidFrame(format!(
                    "unsupported request_permission response fields: {}",
                    unknown
                        .iter()
                        .map(|key| key.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                )))
            }
        }

        let Value::Object(mut fields) = result else {
            return Err(AdapterError::InvalidFrame(
                "request_permission response must be a JSON object".into(),
            ));
        };
        let outcome = fields
            .remove("outcome")
            .ok_or_else(|| AdapterError::InvalidFrame("missing `outcome` field".into()))?;
        reject_unknown(&fields)?;
        let Value::Object(mut outcome_fields) = outcome else {
            return Err(AdapterError::InvalidFrame(
                "`outcome` must be an object with a nested `outcome` field".into(),
            ));
        };
        let decision = outcome_fields
            .remove("outcome")
            .ok_or_else(|| AdapterError::InvalidFrame("missing nested `outcome` field".into()))?;
        let Some(decision) = decision.as_str() else {
            return Err(AdapterError::InvalidFrame(
                "nested `outcome` must be a string".into(),
            ));
        };
        match decision {
            "selected" => {
                let option_id = outcome_fields.remove("optionId").ok_or_else(|| {
                    AdapterError::InvalidFrame("`selected` outcome requires `optionId`".into())
                })?;
                let Some(option_id) = option_id.as_str() else {
                    return Err(AdapterError::InvalidFrame(
                        "`optionId` must be a string".into(),
                    ));
                };
                reject_unknown(&outcome_fields)?;
                Ok(PermissionDecision::Selected {
                    option_id: option_id.to_string(),
                })
            }
            "cancelled" => {
                reject_unknown(&outcome_fields)?;
                Ok(PermissionDecision::Cancelled)
            }
            other => Err(AdapterError::InvalidFrame(format!(
                "unknown outcome `{other}`"
            ))),
        }
    }

    /// `ToolApprovalRequired` → `session/request_permission` 参数（供宿主发射请求）。
    pub async fn permission_request(
        &self,
        event: &AppEvent,
        client_session_id: &ClientSessionId,
    ) -> Result<crate::channels::acp::wire::RequestPermissionParams, AdapterError> {
        map::permission_request(event, client_session_id.0.as_str())
    }

    /// 构造 canonical 命令信封（Automation 来源 + ACP 身份）。宿主胶水在
    /// 构造 RunCancel / ToolApprove 等命令时复用同一信封样式。
    pub fn command_envelope(&self, request_id: &str, command: AppCommand) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from(format!("acp-{request_id}")),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: format!("acp:{}", self.client_info.name),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: now_timestamp(),
            command,
        }
    }

    /// 构造 canonical 查询信封（Automation 来源 + ACP 身份）。
    pub fn query_envelope(&self, request_id: &str, query: AppQuery) -> AppQueryEnvelope {
        AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from(format!("acp-{request_id}")),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: format!("acp:{}", self.client_info.name),
            },
            issued_at: now_timestamp(),
            query,
        }
    }

    async fn decode_session_new(
        &self,
        frame: &AdapterWireFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params = serde_json::from_value::<SessionNewParams>(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("session/new")
            .map_err(AdapterError::InvalidFrame)?;
        reject_unsupported_session_fields(&params.mcp_servers, &params.additional_directories)?;
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

    async fn decode_session_prompt(
        &self,
        frame: &AdapterWireFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params = serde_json::from_value::<SessionPromptParams>(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("session/prompt")
            .map_err(AdapterError::InvalidFrame)?;
        let record = self.attached_record(&params.session_id).await?;
        let user_message = map::extract_user_message(&params.prompt)?;
        Ok(CanonicalClientRequest::Command(self.command_envelope(
            &frame.request_id,
            AppCommand::RunStart {
                session_id: record.core_session_id,
                user_message,
                model: None,
                provider: None,
                profile: None,
            },
        )))
    }

    async fn decode_session_resume(
        &self,
        frame: &AdapterWireFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params = serde_json::from_value::<SessionResumeParams>(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("session/resume")
            .map_err(AdapterError::InvalidFrame)?;
        reject_unsupported_session_fields(&params.mcp_servers, &params.additional_directories)?;
        require_absolute_cwd(&params.cwd)?;
        // resume 的语义是重新 claim 现有记录：记录可能是 Disconnected（close 之后
        // 再 resume），因此这里只要求记录存在，不要求当前已 attach。
        let id = ClientSessionId::new(&params.session_id);
        let record = self
            .registry
            .get(&id)
            .await
            .ok_or_else(|| AdapterError::UnknownSession(id.clone()))?;
        Ok(CanonicalClientRequest::Reattach {
            client_session_id: record.client_session_id.clone(),
            ownership_epoch: record.ownership_epoch,
            revision: record.revision,
            connection_id: record.connection_id.clone(),
            state: ClientSessionState::Subscribed,
            updated_at: now_timestamp(),
        })
    }

    async fn decode_session_close(
        &self,
        frame: &AdapterWireFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let params = serde_json::from_value::<SessionCloseParams>(frame.payload.clone())
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        params
            .reject_unknown("session/close")
            .map_err(AdapterError::InvalidFrame)?;
        let record = self.attached_record(&params.session_id).await?;
        Ok(CanonicalClientRequest::Disconnect {
            client_session_id: record.client_session_id.clone(),
            ownership_epoch: record.ownership_epoch,
            revision: record.revision,
            updated_at: now_timestamp(),
        })
    }

    /// 读取 authoritative registry 记录；未知 session 或已断连显式拒绝。
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

fn reject_unsupported_session_fields(
    mcp_servers: &[Value],
    additional_directories: &[String],
) -> Result<(), AdapterError> {
    if !mcp_servers.is_empty() {
        return Err(AdapterError::InvalidFrame(
            "mcpServers are not supported (mcp capability not negotiated)".into(),
        ));
    }
    if !additional_directories.is_empty() {
        return Err(AdapterError::InvalidFrame(
            "additionalDirectories are not supported (sessionCapabilities.additionalDirectories not advertised)"
                .into(),
        ));
    }
    Ok(())
}

/// registry ACP 准入门：Command 解码结果须登记为 ACP 可达（`acp: true`），
/// 否则 fail-closed。连接生命周期请求（Reattach/Disconnect）不经命令门。
fn admit_acp_command(method: &str, envelope: &AppCommandEnvelope) -> Result<(), AdapterError> {
    let entry = command_entry(&envelope.command);
    if entry.acp {
        Ok(())
    } else {
        Err(AdapterError::ProtocolUnsupported(format!(
            "{method} (registry command `{}` is not ACP-reachable)",
            entry.wire_name
        )))
    }
}

#[async_trait]
impl ClientAdapter for AcpClientAdapter {
    fn protocol(&self) -> &ClientProtocol {
        static PROTOCOL: std::sync::LazyLock<ClientProtocol> =
            std::sync::LazyLock::new(|| ClientProtocol::new(ACP_PROTOCOL));
        &PROTOCOL
    }

    fn capabilities(&self) -> &CapabilitySnapshot {
        &self.capabilities
    }

    async fn decode_payload(
        &self,
        frame: AdapterWireFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        let request = match frame.method.as_str() {
            "session/new" => self.decode_session_new(&frame).await?,
            "session/prompt" => self.decode_session_prompt(&frame).await?,
            "session/resume" => self.decode_session_resume(&frame).await?,
            "session/close" => self.decode_session_close(&frame).await?,
            "session/cancel" => return Err(AdapterError::InvalidFrame(
                "session/cancel is a JSON-RPC notification; use AcpClientAdapter::decode_cancel"
                    .into(),
            )),
            "$/cancel_request" => return Err(AdapterError::InvalidFrame(
                "$/cancel_request is a JSON-RPC notification handled by the ACP host, not a canonical request"
                    .into(),
            )),
            "initialize" => return Err(AdapterError::InvalidFrame(
                "initialize is a handshake method handled by the ACP host, not a canonical request"
                    .into(),
            )),
            "session/load" => return Err(AdapterError::ProtocolUnsupported(
                "session/load (loadSession capability is not advertised)".into(),
            )),
            other => return Err(AdapterError::ProtocolUnsupported(other.into())),
        };
        if let CanonicalClientRequest::Command(envelope) = &request {
            admit_acp_command(&frame.method, envelope)?;
        }
        Ok(request)
    }

    async fn encode_payload(
        &self,
        frame: CanonicalCoreFrame,
    ) -> Result<AdapterWireFrame, AdapterError> {
        match frame {
            CanonicalCoreFrame::Response(envelope) => Ok(AdapterWireFrame {
                schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
                request_id: envelope.request_id.as_str().to_string(),
                method: "acp.response".into(),
                payload: serde_json::to_value(&envelope)
                    .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?,
                extensions: Default::default(),
            }),
            CanonicalCoreFrame::Error(frame) => Ok(AdapterWireFrame {
                schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
                request_id: "adapter-error".into(),
                method: "acp.error".into(),
                payload: serde_json::to_value(&frame)
                    .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?,
                extensions: Default::default(),
            }),
            CanonicalCoreFrame::SessionState(_) => Err(AdapterError::InvalidFrame(
                "client session state is host-internal and never encoded to the ACP wire".into(),
            )),
            CanonicalCoreFrame::Event(envelope) => {
                if matches!(envelope.payload, AppEvent::ToolApprovalRequired { .. }) {
                    return Err(AdapterError::InvalidFrame(
                        "tool approval requires host permission correlation; use AcpClientAdapter::permission_request"
                            .into(),
                    ));
                }
                let Some(client_session_id) = self
                    .session_resolver
                    .resolve_client_session(&envelope)
                    .await
                else {
                    return Err(AdapterError::HostUnavailable(format!(
                        "core event `{}` is not routable to an ACP client session",
                        map::app_event_kind(&envelope.payload)
                    )));
                };
                let Some(update) = map::translate_session_update(&envelope.payload) else {
                    return Err(AdapterError::InvalidFrame(format!(
                        "core event `{}` has no ACP v1 session/update representation",
                        map::app_event_kind(&envelope.payload)
                    )));
                };
                Ok(AdapterWireFrame {
                    schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
                    request_id: envelope.event_id.as_str().to_string(),
                    method: "acp.notification".into(),
                    payload: serde_json::to_value(SessionUpdateParams {
                        session_id: client_session_id.0,
                        update,
                    })
                    .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?,
                    extensions: Default::default(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::SessionId;
    use pawork_protocol::app::registry::command_entries;

    fn envelope(command: AppCommand) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from("acp-test"),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: "acp:test".into(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: now_timestamp(),
            command,
        }
    }

    /// registry acp 列 = ACP 可达命令全集：四臂 decode 只产 session_create /
    /// run_start；run_cancel / tool_approve 由宿主复用 command_envelope 构造，
    /// 同列准入。列漂移（多登/漏登）必须在此 fail。
    #[test]
    fn registry_acp_column_matches_acp_reachable_commands() {
        let acp_commands: Vec<&str> = command_entries()
            .iter()
            .filter(|entry| entry.acp)
            .map(|entry| entry.wire_name)
            .collect();
        assert_eq!(
            acp_commands,
            vec!["session_create", "run_start", "run_cancel", "tool_approve"]
        );
    }

    #[test]
    fn acp_gate_admits_registry_acp_commands() {
        let commands = [
            AppCommand::SessionCreate {
                workspace_id: WorkspaceId::new("ws"),
                title: None,
            },
            AppCommand::RunStart {
                session_id: SessionId::new("session"),
                user_message: "hi".into(),
                model: None,
                provider: None,
                profile: None,
            },
        ];
        for command in commands {
            admit_acp_command("session/new", &envelope(command))
                .expect("registry acp:true command must be admitted");
        }
    }

    #[test]
    fn acp_gate_rejects_commands_outside_registry_acp_column() {
        let error = admit_acp_command(
            "session/new",
            &envelope(AppCommand::WorkspaceAdd {
                root_path: "/tmp".into(),
            }),
        )
        .expect_err("registry acp:false command must be rejected");
        assert_eq!(
            error,
            AdapterError::ProtocolUnsupported(
                "session/new (registry command `workspace_add` is not ACP-reachable)".into()
            )
        );
    }
}
