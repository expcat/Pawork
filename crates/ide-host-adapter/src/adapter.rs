//! IDE 协议的 `ClientFrame` ↔ canonical 翻译层（复用 P18-10 契约）。
//!
//! [`IdeClientAdapter`] 实现 `client_adapter_api::ClientAdapter`：只做协议
//! 翻译（方法名 ↔ canonical 变体），未知方法显式 `ProtocolUnsupported`、
//! 未知字段显式 `InvalidFrame`，禁止静默丢字段。不承载业务决策。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use client_adapter_api::{
    AdapterError, CanonicalClientRequest, CanonicalCoreFrame, CapabilitySnapshot, ClientAdapter,
    ClientAdapterFactory, ClientCapability, ClientFrame, ClientProtocol,
    CLIENT_ADAPTER_SCHEMA_VERSION,
};

use crate::contract::{IdeCapability, IDE_PROTOCOL};

pub const METHOD_COMMAND: &str = "ide.command";
pub const METHOD_QUERY: &str = "ide.query";
pub const METHOD_ATTACH: &str = "ide.attach";
pub const METHOD_REATTACH: &str = "ide.reattach";
pub const METHOD_DISCONNECT: &str = "ide.disconnect";

pub const METHOD_RESPONSE: &str = "ide.response";
pub const METHOD_EVENT: &str = "ide.event";
pub const METHOD_SESSION_STATE: &str = "ide.session_state";
pub const METHOD_ERROR: &str = "ide.error";

/// IDE 协议适配器：`ClientFrame` ↔ canonical。
#[derive(Clone, Debug)]
pub struct IdeClientAdapter {
    protocol: ClientProtocol,
    capabilities: CapabilitySnapshot,
}

impl IdeClientAdapter {
    pub fn new(capabilities: CapabilitySnapshot) -> Result<Self, AdapterError> {
        capabilities.validate()?;
        Ok(Self {
            protocol: capabilities.protocol.clone(),
            capabilities,
        })
    }
}

#[async_trait]
impl ClientAdapter for IdeClientAdapter {
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
            CanonicalCoreFrame::Error(_) => ("ide-error".into(), METHOD_ERROR),
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

/// IDE 协议工厂：协议名 + 显式能力 allowlist（fail-closed）。
#[derive(Clone, Debug)]
pub struct IdeClientAdapterFactory {
    supported: BTreeSet<ClientCapability>,
    protocol: ClientProtocol,
}

impl IdeClientAdapterFactory {
    pub fn new() -> Self {
        Self {
            supported: IdeCapability::ALL
                .iter()
                .map(|capability| ClientCapability::new(capability.as_str()))
                .collect(),
            protocol: ClientProtocol::new(IDE_PROTOCOL),
        }
    }
}

impl Default for IdeClientAdapterFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientAdapterFactory for IdeClientAdapterFactory {
    fn protocol(&self) -> &ClientProtocol {
        &self.protocol
    }

    fn create(
        &self,
        negotiated: CapabilitySnapshot,
    ) -> Result<Arc<dyn ClientAdapter>, AdapterError> {
        negotiated.validate()?;
        if negotiated.protocol != *self.protocol() {
            return Err(AdapterError::ProtocolUnsupported(
                negotiated.protocol.0.clone(),
            ));
        }
        if let Some(unsupported) = negotiated
            .capabilities
            .iter()
            .find(|capability| !self.supported.contains(*capability))
        {
            return Err(AdapterError::CapabilityUnsupported(unsupported.clone()));
        }
        Ok(Arc::new(IdeClientAdapter::new(negotiated)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{CommandId, QueryId, SessionId, Timestamp};
    use client_adapter_api::{ClientSessionId, ClientSessionRecord, ClientSessionState};
    use core_api::{
        ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, CommandSource,
        API_VERSION,
    };
    use serde_json::json;

    fn snapshot() -> CapabilitySnapshot {
        CapabilitySnapshot {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new(IDE_PROTOCOL),
            protocol_version: crate::IDE_PROTOCOL_VERSION.into(),
            client_version: "0.0.0".into(),
            revision: 1,
            capabilities: [ClientCapability::new("lifecycle")].into_iter().collect(),
        }
    }

    fn adapter() -> IdeClientAdapter {
        IdeClientAdapter::new(snapshot()).expect("valid snapshot")
    }

    fn command_frame() -> ClientFrame {
        let envelope = AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from("cmd-1"),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: "ide-test".into(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(1),
            command: AppCommand::RunCancel {
                run_id: "run-1".into(),
            },
        };
        ClientFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: "cmd-1".into(),
            method: METHOD_COMMAND.into(),
            payload: serde_json::to_value(CanonicalClientRequest::Command(envelope)).unwrap(),
            extensions: Default::default(),
        }
    }

    #[tokio::test]
    async fn decodes_command_query_and_session_frames() {
        let ide = adapter();
        let canonical = ide.decode(command_frame()).await.expect("command decodes");
        assert!(matches!(
            canonical,
            CanonicalClientRequest::Command(ref envelope)
                if matches!(envelope.command, AppCommand::RunCancel { .. })
        ));

        let query = ClientFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: "q-1".into(),
            method: METHOD_QUERY.into(),
            payload: serde_json::to_value(CanonicalClientRequest::Query(AppQueryEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from("q-1"),
                source: CommandSource::Automation,
                identity: ActorIdentity::Automation {
                    name: "ide-test".into(),
                },
                issued_at: Timestamp::from_unix_millis(1),
                query: AppQuery::RunStatus {
                    run_id: "run-1".into(),
                },
            }))
            .unwrap(),
            extensions: Default::default(),
        };
        assert!(matches!(
            ide.decode(query).await.expect("query decodes"),
            CanonicalClientRequest::Query(_)
        ));

        let record = ClientSessionRecord {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new(IDE_PROTOCOL),
            client_session_id: ClientSessionId::new("ide:s-1"),
            core_session_id: SessionId::from("s-1"),
            connection_id: "conn-1".into(),
            ownership_epoch: 1,
            revision: 1,
            state: ClientSessionState::Loaded,
            capabilities: snapshot(),
            updated_at: Timestamp::from_unix_millis(1),
        };
        let attach = ClientFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: "a-1".into(),
            method: METHOD_ATTACH.into(),
            payload: serde_json::to_value(CanonicalClientRequest::Attach(record)).unwrap(),
            extensions: Default::default(),
        };
        assert!(matches!(
            ide.decode(attach).await.expect("attach decodes"),
            CanonicalClientRequest::Attach(_)
        ));
    }

    #[tokio::test]
    async fn unknown_method_and_extensions_fail_explicitly() {
        let ide = adapter();
        let unknown = ClientFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: "x".into(),
            method: "ide.frobnicate".into(),
            payload: json!({"type": "command", "data": {}}),
            extensions: Default::default(),
        };
        assert!(matches!(
            ide.decode(unknown).await,
            Err(AdapterError::ProtocolUnsupported(_))
        ));

        let mut frame = command_frame();
        frame.extensions.insert("extra".into(), json!(1));
        assert!(matches!(
            ide.decode(frame).await,
            Err(AdapterError::InvalidFrame(_))
        ));

        let mut mismatch = command_frame();
        mismatch.method = METHOD_QUERY.into();
        assert!(matches!(
            ide.decode(mismatch).await,
            Err(AdapterError::InvalidFrame(_))
        ));
    }

    #[tokio::test]
    async fn encodes_canonical_core_frames() {
        let ide = adapter();
        let frame = ide
            .encode(CanonicalCoreFrame::Error(
                AdapterError::ProtocolUnsupported("ide.nope".into()).frame(),
            ))
            .await
            .expect("encode");
        assert_eq!(frame.method, METHOD_ERROR);
        assert_eq!(frame.request_id, "ide-error");

        let encoded = ide
            .encode(CanonicalCoreFrame::Error(
                AdapterError::ProtocolUnsupported("ide.nope".into()).frame(),
            ))
            .await
            .expect("encode again");
        assert_eq!(encoded.method, METHOD_ERROR);
    }

    #[test]
    fn factory_is_fail_closed() {
        let factory = IdeClientAdapterFactory::new();
        let ok = factory
            .create(snapshot())
            .expect("allowlisted capability negotiates");
        assert_eq!(ok.protocol().0, IDE_PROTOCOL);

        let mut wrong = snapshot();
        wrong.protocol = ClientProtocol::new("acp");
        assert!(matches!(
            factory.create(wrong),
            Err(AdapterError::ProtocolUnsupported(_))
        ));

        let mut unknown = snapshot();
        unknown
            .capabilities
            .insert(ClientCapability::new("account_management"));
        assert!(matches!(
            factory.create(unknown),
            Err(AdapterError::CapabilityUnsupported(_))
        ));
    }
}
