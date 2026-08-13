//! Claude Code Gateway `ClientAdapter`（P18-12 §0）。
//!
//! 本层是纯协议翻译与能力协商：`claude.*` `ClientFrame` ↔ canonical 帧，
//! 并给宿主提供 fail-closed 的流状态工厂（能力协商 + signed thinking
//! protector 注入）。不持有 Provider credential、不做权限决策、不构造 Core；
//! session 归属沿用 `client-adapter-api` 的 authoritative registry 契约
//! （由宿主接线，本 crate 不自建 ownership）。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use client_adapter_api::{
    AdapterError, CanonicalClientRequest, CanonicalCoreFrame, CapabilitySnapshot, ClientAdapter,
    ClientAdapterFactory, ClientCapability, ClientFrame, ClientProtocol,
    CLIENT_ADAPTER_SCHEMA_VERSION,
};

use crate::reasoning::SignedThinkingProtector;
use crate::stream::ClaudeStreamState;

/// Claude Code Gateway 协议名（registry / capability snapshot 的权威标识）。
pub const CLAUDE_GATEWAY_PROTOCOL: &str = "claude-gateway";

/// 支持的线协议版本。
pub const CLAUDE_GATEWAY_PROTOCOL_VERSION: &str = "1";

/// signed thinking continuity 能力名（capability negotiation key）。
///
/// 协商成功 → 线协议中的 `thinking.signature` / `redacted_thinking.data`
/// 经 Protected Blob 引用处理；未协商 → 收到 signed 材料显式失败。
pub const REASONING_SIGNED_CONTINUITY_CAPABILITY: &str = "reasoning.signed_continuity";

/// 首轮由网关支持的客户端能力白名单。
///
/// - `events`：canonical 事件下发；
/// - `reasoning.signed_continuity`：signed thinking 续传（ADR-032 / P15-7）。
pub const DEFAULT_SUPPORTED_CAPABILITIES: &[&str] =
    &["events", REASONING_SIGNED_CONTINUITY_CAPABILITY];

/// 能力名 → [`ClientCapability`] 便捷构造。
pub fn capability() -> ClientCapability {
    ClientCapability::new(REASONING_SIGNED_CONTINUITY_CAPABILITY)
}

pub const METHOD_COMMAND: &str = "claude.command";
pub const METHOD_QUERY: &str = "claude.query";
pub const METHOD_ATTACH: &str = "claude.attach";
pub const METHOD_REATTACH: &str = "claude.reattach";
pub const METHOD_DISCONNECT: &str = "claude.disconnect";

pub const METHOD_RESPONSE: &str = "claude.response";
pub const METHOD_EVENT: &str = "claude.event";
pub const METHOD_SESSION_STATE: &str = "claude.session_state";
pub const METHOD_ERROR: &str = "claude.error";

/// 协商产物：concrete adapter + 被显式降级的客户端能力清单。
///
/// 与 acp-host 同型：白名单之外的能力**显式降级**（记录而非静默丢弃），
/// 使用点（如 signed thinking 映射）再按最终 snapshot fail-closed。
#[derive(Clone)]
pub struct NegotiatedClaudeAdapter {
    pub adapter: Arc<ClaudeGatewayAdapter>,
    pub degraded: Vec<ClientCapability>,
}

/// Claude Gateway adapter factory：协议名 + 显式能力 allowlist +
/// 可注入的 signed thinking 保护器（生产由宿主桥接
/// `ProtectedBlobStoreProtector`，ADR-032）。
pub struct ClaudeGatewayAdapterFactory {
    supported_capabilities: BTreeSet<ClientCapability>,
    protector: Option<Arc<dyn SignedThinkingProtector>>,
}

impl ClaudeGatewayAdapterFactory {
    pub fn new(
        supported_capabilities: impl IntoIterator<Item = ClientCapability>,
        protector: Option<Arc<dyn SignedThinkingProtector>>,
    ) -> Self {
        Self {
            supported_capabilities: supported_capabilities.into_iter().collect(),
            protector,
        }
    }

    /// 以默认白名单构造（含 `reasoning.signed_continuity`）。
    pub fn with_defaults(protector: Option<Arc<dyn SignedThinkingProtector>>) -> Self {
        Self::new(
            DEFAULT_SUPPORTED_CAPABILITIES
                .iter()
                .map(|name| ClientCapability::new(*name)),
            protector,
        )
    }

    /// 宿主使用的具体协商入口（返回 concrete adapter + 降级清单）。
    pub fn create_concrete(
        &self,
        negotiated: CapabilitySnapshot,
    ) -> Result<NegotiatedClaudeAdapter, AdapterError> {
        negotiated.validate()?;
        if negotiated.protocol != ClientProtocol::new(CLAUDE_GATEWAY_PROTOCOL) {
            return Err(AdapterError::ProtocolUnsupported(
                negotiated.protocol.0.clone(),
            ));
        }
        if negotiated.protocol_version != CLAUDE_GATEWAY_PROTOCOL_VERSION {
            return Err(AdapterError::ProtocolUnsupported(format!(
                "claude-gateway protocol version {} (only version {} is supported)",
                negotiated.protocol_version, CLAUDE_GATEWAY_PROTOCOL_VERSION
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
        Ok(NegotiatedClaudeAdapter {
            adapter: Arc::new(ClaudeGatewayAdapter {
                protocol: ClientProtocol::new(CLAUDE_GATEWAY_PROTOCOL),
                capabilities: snapshot,
                protector: self.protector.clone(),
            }),
            degraded,
        })
    }
}

impl ClientAdapterFactory for ClaudeGatewayAdapterFactory {
    fn protocol(&self) -> &ClientProtocol {
        static PROTOCOL: std::sync::LazyLock<ClientProtocol> =
            std::sync::LazyLock::new(|| ClientProtocol::new(CLAUDE_GATEWAY_PROTOCOL));
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

/// Claude Gateway 线协议 ↔ canonical 的翻译 adapter（无内部可变状态）。
pub struct ClaudeGatewayAdapter {
    protocol: ClientProtocol,
    capabilities: CapabilitySnapshot,
    protector: Option<Arc<dyn SignedThinkingProtector>>,
}

impl ClaudeGatewayAdapter {
    pub fn capabilities_snapshot(&self) -> &CapabilitySnapshot {
        &self.capabilities
    }

    /// 是否已协商 signed thinking continuity 能力。
    pub fn reasoning_supported(&self) -> bool {
        self.capabilities.supports(&ClientCapability::new(
            REASONING_SIGNED_CONTINUITY_CAPABILITY,
        ))
    }

    /// 构造 fail-closed 流状态：能力协商结果 + protector 注入一体。
    pub fn stream_state(&self) -> ClaudeStreamState {
        let mut state = ClaudeStreamState::new(self.reasoning_supported());
        if let Some(protector) = &self.protector {
            state.set_protector(Arc::clone(protector));
        }
        state
    }
}

#[async_trait]
impl ClientAdapter for ClaudeGatewayAdapter {
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
        if !matches!(
            frame.method.as_str(),
            METHOD_COMMAND | METHOD_QUERY | METHOD_ATTACH | METHOD_REATTACH | METHOD_DISCONNECT
        ) {
            return Err(AdapterError::ProtocolUnsupported(frame.method.clone()));
        }
        if !frame.extensions.is_empty() {
            return Err(AdapterError::InvalidFrame(format!(
                "unsupported fields: {}",
                frame
                    .extensions
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(",")
            )));
        }
        let canonical: CanonicalClientRequest = serde_json::from_value(frame.payload)
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?;
        let method_matches = matches!(
            (&canonical, frame.method.as_str()),
            (CanonicalClientRequest::Command(_), METHOD_COMMAND)
                | (CanonicalClientRequest::Query(_), METHOD_QUERY)
                | (CanonicalClientRequest::Attach(_), METHOD_ATTACH)
                | (CanonicalClientRequest::Reattach { .. }, METHOD_REATTACH)
                | (CanonicalClientRequest::Disconnect { .. }, METHOD_DISCONNECT)
        );
        if !method_matches {
            return Err(AdapterError::InvalidFrame(format!(
                "method `{}` does not match payload type",
                frame.method
            )));
        }
        Ok(canonical)
    }

    async fn encode_payload(&self, frame: CanonicalCoreFrame) -> Result<ClientFrame, AdapterError> {
        let (request_id, method) = match &frame {
            CanonicalCoreFrame::Response(envelope) => {
                (envelope.request_id.as_str().to_string(), METHOD_RESPONSE)
            }
            CanonicalCoreFrame::Event(envelope) => {
                (envelope.event_id.as_str().to_string(), METHOD_EVENT)
            }
            CanonicalCoreFrame::SessionState(record) => {
                (record.client_session_id.0.clone(), METHOD_SESSION_STATE)
            }
            CanonicalCoreFrame::Error(_) => ("claude-error".into(), METHOD_ERROR),
        };
        Ok(ClientFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id,
            method: method.into(),
            payload: serde_json::to_value(&frame)
                .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?,
            extensions: BTreeMap::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{ConnectionId, SessionId, Timestamp};
    use client_adapter_api::{ClientSessionId, ClientSessionRecord, ClientSessionState};

    fn snapshot(protocol: &str, capabilities: &[&str]) -> CapabilitySnapshot {
        CapabilitySnapshot {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new(protocol),
            protocol_version: CLAUDE_GATEWAY_PROTOCOL_VERSION.into(),
            client_version: "2.0".into(),
            revision: 1,
            capabilities: capabilities
                .iter()
                .map(|name| ClientCapability::new(*name))
                .collect(),
        }
    }

    fn record() -> ClientSessionRecord {
        ClientSessionRecord {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new(CLAUDE_GATEWAY_PROTOCOL),
            client_session_id: ClientSessionId::new("client-session"),
            core_session_id: SessionId::from("core-session"),
            connection_id: ConnectionId::from("connection-1"),
            ownership_epoch: 1,
            revision: 1,
            state: ClientSessionState::Loaded,
            capabilities: snapshot(CLAUDE_GATEWAY_PROTOCOL, &["events"]),
            updated_at: Timestamp::from_unix_millis(1),
        }
    }

    #[test]
    fn factory_rejects_protocol_and_version_mismatch() {
        let factory = ClaudeGatewayAdapterFactory::with_defaults(None);
        let mut wrong_protocol = snapshot("acp", &["events"]);
        wrong_protocol.protocol = ClientProtocol::new("acp");
        assert!(matches!(
            factory.create_concrete(wrong_protocol),
            Err(AdapterError::ProtocolUnsupported(protocol)) if protocol == "acp"
        ));

        let mut wrong_version = snapshot(CLAUDE_GATEWAY_PROTOCOL, &["events"]);
        wrong_version.protocol_version = "9".into();
        assert!(matches!(
            factory.create_concrete(wrong_version),
            Err(AdapterError::ProtocolUnsupported(_))
        ));
    }

    #[test]
    fn unknown_capabilities_are_degraded_not_dropped() {
        let factory = ClaudeGatewayAdapterFactory::with_defaults(None);
        let negotiated = factory
            .create_concrete(snapshot(
                CLAUDE_GATEWAY_PROTOCOL,
                &[
                    "events",
                    REASONING_SIGNED_CONTINUITY_CAPABILITY,
                    "future.cap",
                ],
            ))
            .expect("negotiate");
        assert_eq!(
            negotiated.degraded,
            vec![ClientCapability::new("future.cap")]
        );
        assert!(negotiated.adapter.reasoning_supported());
        assert!(negotiated
            .adapter
            .capabilities_snapshot()
            .supports(&ClientCapability::new("events")));
        assert!(!negotiated
            .adapter
            .capabilities_snapshot()
            .supports(&ClientCapability::new("future.cap")));
    }

    #[test]
    fn reasoning_support_is_fail_closed_without_capability() {
        let factory = ClaudeGatewayAdapterFactory::with_defaults(None);
        let negotiated = factory
            .create_concrete(snapshot(CLAUDE_GATEWAY_PROTOCOL, &["events"]))
            .expect("negotiate");
        assert!(!negotiated.adapter.reasoning_supported());
        let state = negotiated.adapter.stream_state();
        assert!(!state.reasoning_supported);
    }

    #[tokio::test]
    async fn frames_encode_decode_round_trip() {
        let factory = ClaudeGatewayAdapterFactory::with_defaults(None);
        let negotiated = factory
            .create_concrete(snapshot(CLAUDE_GATEWAY_PROTOCOL, &["events"]))
            .expect("negotiate");
        let adapter = negotiated.adapter;

        // canonical → claude.* 帧。
        let encoded = adapter
            .encode(CanonicalCoreFrame::SessionState(record()))
            .await
            .expect("encode");
        assert_eq!(encoded.method, METHOD_SESSION_STATE);
        assert_eq!(encoded.request_id, "client-session");

        // claude.attach 帧 → canonical，且 method 与 payload 类型必须一致。
        let payload = serde_json::to_value(CanonicalClientRequest::Attach(record()))
            .expect("serialize attach");
        let decoded = adapter
            .decode(ClientFrame {
                schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
                request_id: "request-1".into(),
                method: METHOD_ATTACH.into(),
                payload,
                extensions: BTreeMap::new(),
            })
            .await
            .expect("decode");
        assert!(matches!(decoded, CanonicalClientRequest::Attach(_)));
    }

    #[tokio::test]
    async fn unknown_method_and_extensions_fail_explicitly() {
        let factory = ClaudeGatewayAdapterFactory::with_defaults(None);
        let adapter = factory
            .create_concrete(snapshot(CLAUDE_GATEWAY_PROTOCOL, &["events"]))
            .expect("negotiate")
            .adapter;
        let frame = ClientFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: "request-1".into(),
            method: "claude.nope".into(),
            payload: serde_json::Value::Null,
            extensions: BTreeMap::new(),
        };
        assert_eq!(
            adapter.decode(frame).await,
            Err(AdapterError::ProtocolUnsupported("claude.nope".into()))
        );

        let mut with_extension = ClientFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: "request-2".into(),
            method: METHOD_ATTACH.into(),
            payload: serde_json::to_value(CanonicalClientRequest::Attach(record()))
                .expect("serialize"),
            extensions: BTreeMap::new(),
        };
        with_extension
            .extensions
            .insert("future_field".into(), serde_json::json!(true));
        assert!(matches!(
            adapter.decode(with_extension).await,
            Err(AdapterError::InvalidFrame(_))
        ));
    }
}
