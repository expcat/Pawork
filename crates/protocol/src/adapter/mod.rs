//! 外部 Agent Client 的统一协议适配契约（P18-10）。
//!
//! 本 crate 只定义协议翻译、能力协商和 authoritative ownership registry。
//! Adapter 不持有 Provider credential，也不消费 GUI Connection Protocol frame。
//!
//! [`ExternalAgentIdentity`] 是协议中立的 session/agent/parent-agent 归属契约
//! （P18-12）：tenant 只来自宿主注入的 [`TrustedTenantContext`]，身份字段不得
//! 作为跨 tenant affinity key。

mod identity;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::{AppCommandEnvelope, AppEventEnvelope, AppQueryEnvelope, AppResponseEnvelope};
use async_trait::async_trait;
use pawork_domain::{ConnectionId, SessionId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::Mutex;

pub use identity::{
    bind_tenant, ExternalAgentIdentity, IdentityError, TenantBinding, TrustedTenantContext,
};
pub use pawork_domain::{
    CapabilitySnapshot, ClientCapability, ClientProtocol, ClientSessionId, ClientSessionRecord,
    ClientSessionState, RegistryWriteOutcome, SessionRegistryError, SessionRegistryStore,
    CLIENT_ADAPTER_SCHEMA_VERSION,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdapterWireFrame {
    pub schema_version: u32,
    pub request_id: String,
    pub method: String,
    #[serde(default)]
    pub payload: Value,
    /// Adapter 未理解的客户端字段必须原样保留，禁止静默吞掉。
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl AdapterWireFrame {
    pub fn validate(&self) -> Result<(), AdapterError> {
        if self.schema_version != CLIENT_ADAPTER_SCHEMA_VERSION {
            return Err(AdapterError::UnsupportedSchema {
                found: self.schema_version,
                supported: CLIENT_ADAPTER_SCHEMA_VERSION,
            });
        }
        if self.request_id.trim().is_empty() || self.method.trim().is_empty() {
            return Err(AdapterError::InvalidFrame(
                "request_id and method must be non-empty".into(),
            ));
        }
        const RESERVED: &[&str] = &["schema_version", "request_id", "method", "payload"];
        if let Some(field) = self
            .extensions
            .keys()
            .find(|field| RESERVED.contains(&field.as_str()))
        {
            return Err(AdapterError::InvalidFrame(format!(
                "extension shadows reserved field `{field}`"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum CanonicalClientRequest {
    Command(AppCommandEnvelope),
    Query(AppQueryEnvelope),
    Attach(ClientSessionRecord),
    Reattach {
        client_session_id: ClientSessionId,
        ownership_epoch: u64,
        revision: u64,
        connection_id: ConnectionId,
        state: ClientSessionState,
        updated_at: Timestamp,
    },
    Disconnect {
        client_session_id: ClientSessionId,
        ownership_epoch: u64,
        revision: u64,
        updated_at: Timestamp,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum CanonicalCoreFrame {
    Response(AppResponseEnvelope),
    Event(AppEventEnvelope),
    SessionState(ClientSessionRecord),
    Error(AdapterErrorFrame),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterErrorFrame {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<ClientCapability>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AdapterError {
    #[error("protocol method is unsupported: {0}")]
    ProtocolUnsupported(String),
    #[error("client capability is unsupported: {0:?}")]
    CapabilityUnsupported(ClientCapability),
    #[error("invalid client frame: {0}")]
    InvalidFrame(String),
    #[error("adapter schema {found} is unsupported (expected {supported})")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("unknown client session: {0:?}")]
    UnknownSession(ClientSessionId),
    #[error("client session {0:?} is not attached")]
    SessionNotAttached(ClientSessionId),
    #[error("core session {0:?} does not exist in Core")]
    CoreSessionNotFound(SessionId),
    #[error("client session already exists: {0:?}")]
    SessionConflict(ClientSessionId),
    #[error("client session ownership counter exhausted: {0:?}")]
    RevisionExhausted(ClientSessionId),
    #[error(
        "stale owner for {client_session_id:?}: expected epoch/revision {expected_epoch}/{expected_revision}, got {actual_epoch}/{actual_revision}"
    )]
    StaleOwner {
        client_session_id: ClientSessionId,
        expected_epoch: u64,
        expected_revision: u64,
        actual_epoch: u64,
        actual_revision: u64,
    },
    #[error("adapter host unavailable: {0}")]
    HostUnavailable(String),
}

impl AdapterError {
    pub fn frame(&self) -> AdapterErrorFrame {
        let capability = match self {
            Self::CapabilityUnsupported(capability) => Some(capability.clone()),
            _ => None,
        };
        AdapterErrorFrame {
            code: match self {
                Self::ProtocolUnsupported(_) => "protocol_unsupported",
                Self::CapabilityUnsupported(_) => "capability_unsupported",
                Self::InvalidFrame(_) => "invalid_frame",
                Self::UnsupportedSchema { .. } => "unsupported_schema",
                Self::UnknownSession(_) => "unknown_session",
                Self::SessionNotAttached(_) => "session_not_attached",
                Self::CoreSessionNotFound(_) => "core_session_not_found",
                Self::SessionConflict(_) => "session_conflict",
                Self::RevisionExhausted(_) => "revision_exhausted",
                Self::StaleOwner { .. } => "stale_owner",
                Self::HostUnavailable(_) => "host_unavailable",
            }
            .into(),
            message: self.to_string(),
            capability,
        }
    }
}

#[async_trait]
pub trait ClientAdapter: Send + Sync {
    fn protocol(&self) -> &ClientProtocol;
    fn capabilities(&self) -> &CapabilitySnapshot;
    fn require(&self, capability: &ClientCapability) -> Result<(), AdapterError> {
        if self.capabilities().supports(capability) {
            Ok(())
        } else {
            Err(AdapterError::CapabilityUnsupported(capability.clone()))
        }
    }
    async fn decode_payload(
        &self,
        frame: AdapterWireFrame,
    ) -> Result<CanonicalClientRequest, AdapterError>;
    async fn encode_payload(
        &self,
        frame: CanonicalCoreFrame,
    ) -> Result<AdapterWireFrame, AdapterError>;

    async fn decode(
        &self,
        frame: AdapterWireFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
        frame.validate()?;
        self.decode_payload(frame).await
    }

    async fn encode(&self, frame: CanonicalCoreFrame) -> Result<AdapterWireFrame, AdapterError> {
        let encoded = self.encode_payload(frame).await?;
        encoded.validate()?;
        Ok(encoded)
    }
}

pub trait ClientAdapterFactory: Send + Sync {
    fn protocol(&self) -> &ClientProtocol;
    fn create(
        &self,
        negotiated: CapabilitySnapshot,
    ) -> Result<Arc<dyn ClientAdapter>, AdapterError>;
}

/// Host 侧已协商上下文：协议宿主在 factory 创建 adapter 后构造，随每次
/// dispatch 提交给 Host。Host 只信任该上下文与 authoritative registry
/// 记录一致；Command/Query 要求 session 已 attach 且 protocol、capability
/// snapshot、ownership epoch/revision 全部匹配，异常 adapter 无法用伪造的
/// protocol/capability/ownership 绑定会话。
#[derive(Clone)]
pub struct AdapterSessionContext {
    /// factory 协商出的 adapter（negotiated protocol + capability snapshot
    /// 的权威来源；Host 不信任客户端自报的 protocol/capability）。
    pub adapter: Arc<dyn ClientAdapter>,
    pub client_session_id: ClientSessionId,
    pub connection_id: ConnectionId,
    /// 调用方声明的 ownership 位置；以 registry 权威记录为准核对。
    pub ownership_epoch: u64,
    pub revision: u64,
}

/// 协议 contract 测试使用的最小 adapter；它只翻译 canonical JSON，未知字段
/// 显式失败，不包含账号、Provider 或业务决策。
pub struct MockClientAdapter {
    protocol: ClientProtocol,
    capabilities: CapabilitySnapshot,
}

/// Contract tests and protocol hosts use this factory to exercise the same
/// fail-closed negotiation path as concrete adapters. A factory is scoped to
/// one protocol and an explicit capability allowlist; it never infers support
/// from a client name or silently drops an unknown capability.
pub struct MockClientAdapterFactory {
    protocol: ClientProtocol,
    supported_capabilities: BTreeSet<ClientCapability>,
}

impl MockClientAdapterFactory {
    pub fn new(
        protocol: ClientProtocol,
        supported_capabilities: impl IntoIterator<Item = ClientCapability>,
    ) -> Self {
        Self {
            protocol,
            supported_capabilities: supported_capabilities.into_iter().collect(),
        }
    }
}

impl ClientAdapterFactory for MockClientAdapterFactory {
    fn protocol(&self) -> &ClientProtocol {
        &self.protocol
    }

    fn create(
        &self,
        negotiated: CapabilitySnapshot,
    ) -> Result<Arc<dyn ClientAdapter>, AdapterError> {
        negotiated.validate()?;
        if negotiated.protocol != self.protocol {
            return Err(AdapterError::ProtocolUnsupported(
                negotiated.protocol.0.clone(),
            ));
        }
        if let Some(unsupported) = negotiated
            .capabilities
            .iter()
            .find(|capability| !self.supported_capabilities.contains(*capability))
        {
            return Err(AdapterError::CapabilityUnsupported(unsupported.clone()));
        }
        Ok(Arc::new(MockClientAdapter::new(negotiated)?))
    }
}

impl MockClientAdapter {
    pub fn new(capabilities: CapabilitySnapshot) -> Result<Self, AdapterError> {
        capabilities.validate()?;
        Ok(Self {
            protocol: capabilities.protocol.clone(),
            capabilities,
        })
    }
}

#[async_trait]
impl ClientAdapter for MockClientAdapter {
    fn protocol(&self) -> &ClientProtocol {
        &self.protocol
    }

    fn capabilities(&self) -> &CapabilitySnapshot {
        &self.capabilities
    }

    async fn decode_payload(
        &self,
        frame: AdapterWireFrame,
    ) -> Result<CanonicalClientRequest, AdapterError> {
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
        if frame.method != "canonical.request" {
            return Err(AdapterError::ProtocolUnsupported(frame.method));
        }
        serde_json::from_value(frame.payload)
            .map_err(|error| AdapterError::InvalidFrame(error.to_string()))
    }

    async fn encode_payload(
        &self,
        frame: CanonicalCoreFrame,
    ) -> Result<AdapterWireFrame, AdapterError> {
        let (request_id, method) = match &frame {
            CanonicalCoreFrame::Response(envelope) => (
                envelope.request_id.as_str().to_string(),
                "canonical.response",
            ),
            CanonicalCoreFrame::Event(envelope) => {
                (envelope.event_id.as_str().to_string(), "canonical.event")
            }
            CanonicalCoreFrame::SessionState(record) => (
                record.client_session_id.0.clone(),
                "canonical.session_state",
            ),
            CanonicalCoreFrame::Error(_) => ("adapter-error".into(), "canonical.error"),
        };
        Ok(AdapterWireFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id,
            method: method.into(),
            payload: serde_json::to_value(frame)
                .map_err(|error| AdapterError::InvalidFrame(error.to_string()))?,
            extensions: BTreeMap::new(),
        })
    }
}

impl From<SessionRegistryError> for AdapterError {
    fn from(error: SessionRegistryError) -> Self {
        match error {
            SessionRegistryError::Unavailable(message) => Self::HostUnavailable(message),
            SessionRegistryError::InvalidRecord(message) => Self::InvalidFrame(message),
            SessionRegistryError::UnsupportedSchema { found, supported } => {
                Self::UnsupportedSchema { found, supported }
            }
        }
    }
}

#[derive(Default)]
pub struct InMemorySessionRegistryStore {
    records: Mutex<BTreeMap<ClientSessionId, ClientSessionRecord>>,
}

#[async_trait]
impl SessionRegistryStore for InMemorySessionRegistryStore {
    async fn load_all(&self) -> Result<Vec<ClientSessionRecord>, SessionRegistryError> {
        Ok(self.records.lock().await.values().cloned().collect())
    }

    async fn insert(
        &self,
        record: &ClientSessionRecord,
    ) -> Result<RegistryWriteOutcome, SessionRegistryError> {
        let mut records = self.records.lock().await;
        if let Some(current) = records.get(&record.client_session_id) {
            return Ok(RegistryWriteOutcome::Conflict(Box::new(Some(
                current.clone(),
            ))));
        }
        records.insert(record.client_session_id.clone(), record.clone());
        Ok(RegistryWriteOutcome::Applied)
    }

    async fn compare_and_swap(
        &self,
        expected_epoch: u64,
        expected_revision: u64,
        record: &ClientSessionRecord,
    ) -> Result<RegistryWriteOutcome, SessionRegistryError> {
        let mut records = self.records.lock().await;
        let Some(current) = records.get(&record.client_session_id) else {
            return Ok(RegistryWriteOutcome::Conflict(Box::new(None)));
        };
        if current.ownership_epoch != expected_epoch || current.revision != expected_revision {
            return Ok(RegistryWriteOutcome::Conflict(Box::new(Some(
                current.clone(),
            ))));
        }
        records.insert(record.client_session_id.clone(), record.clone());
        Ok(RegistryWriteOutcome::Applied)
    }

    async fn remove_if_owner(
        &self,
        client_session_id: &ClientSessionId,
        expected_epoch: u64,
        expected_revision: u64,
    ) -> Result<RegistryWriteOutcome, SessionRegistryError> {
        let mut records = self.records.lock().await;
        let Some(current) = records.get(client_session_id) else {
            return Ok(RegistryWriteOutcome::Conflict(Box::new(None)));
        };
        if current.ownership_epoch != expected_epoch || current.revision != expected_revision {
            return Ok(RegistryWriteOutcome::Conflict(Box::new(Some(
                current.clone(),
            ))));
        }
        records.remove(client_session_id);
        Ok(RegistryWriteOutcome::Applied)
    }
}

pub struct SessionRegistry {
    store: Arc<dyn SessionRegistryStore>,
    records: Mutex<BTreeMap<ClientSessionId, ClientSessionRecord>>,
}

impl SessionRegistry {
    pub async fn new(store: Arc<dyn SessionRegistryStore>) -> Result<Self, AdapterError> {
        let mut records = BTreeMap::new();
        for record in store.load_all().await? {
            validate_record(&record)?;
            let id = record.client_session_id.clone();
            if records.insert(id.clone(), record).is_some() {
                return Err(AdapterError::SessionConflict(id));
            }
        }
        Ok(Self {
            store,
            records: Mutex::new(records),
        })
    }

    pub async fn register(&self, record: ClientSessionRecord) -> Result<(), AdapterError> {
        validate_record(&record)?;
        let mut records = self.records.lock().await;
        match self.store.insert(&record).await? {
            RegistryWriteOutcome::Applied => {
                records.insert(record.client_session_id.clone(), record);
                Ok(())
            }
            RegistryWriteOutcome::Conflict(current) => {
                if let Some(current) = *current {
                    records.insert(current.client_session_id.clone(), current);
                }
                Err(AdapterError::SessionConflict(record.client_session_id))
            }
        }
    }

    pub async fn get(&self, id: &ClientSessionId) -> Option<ClientSessionRecord> {
        self.records.lock().await.get(id).cloned()
    }

    pub async fn claim(
        &self,
        id: &ClientSessionId,
        expected_epoch: u64,
        expected_revision: u64,
        connection_id: ConnectionId,
        state: ClientSessionState,
        updated_at: Timestamp,
    ) -> Result<ClientSessionRecord, AdapterError> {
        self.update_checked(id, expected_epoch, expected_revision, |record| {
            record.ownership_epoch = record
                .ownership_epoch
                .checked_add(1)
                .ok_or_else(|| AdapterError::RevisionExhausted(id.clone()))?;
            record.revision = record
                .revision
                .checked_add(1)
                .ok_or_else(|| AdapterError::RevisionExhausted(id.clone()))?;
            record.connection_id = connection_id;
            record.state = state;
            record.updated_at = updated_at;
            Ok(())
        })
        .await
    }

    pub async fn transition(
        &self,
        id: &ClientSessionId,
        expected_epoch: u64,
        expected_revision: u64,
        state: ClientSessionState,
        updated_at: Timestamp,
    ) -> Result<ClientSessionRecord, AdapterError> {
        self.update_checked(id, expected_epoch, expected_revision, |record| {
            record.revision = record
                .revision
                .checked_add(1)
                .ok_or_else(|| AdapterError::RevisionExhausted(id.clone()))?;
            record.state = state;
            record.updated_at = updated_at;
            Ok(())
        })
        .await
    }

    pub async fn remove(
        &self,
        id: &ClientSessionId,
        expected_epoch: u64,
        expected_revision: u64,
    ) -> Result<ClientSessionRecord, AdapterError> {
        let mut records = self.records.lock().await;
        let record = records
            .get(id)
            .cloned()
            .ok_or_else(|| AdapterError::UnknownSession(id.clone()))?;
        ensure_owner(&record, expected_epoch, expected_revision)?;
        match self
            .store
            .remove_if_owner(id, expected_epoch, expected_revision)
            .await?
        {
            RegistryWriteOutcome::Applied => {
                records.remove(id);
                Ok(record)
            }
            RegistryWriteOutcome::Conflict(current) => reconcile_conflict(
                &mut records,
                id,
                *current,
                expected_epoch,
                expected_revision,
            ),
        }
    }

    async fn update_checked(
        &self,
        id: &ClientSessionId,
        expected_epoch: u64,
        expected_revision: u64,
        update: impl FnOnce(&mut ClientSessionRecord) -> Result<(), AdapterError>,
    ) -> Result<ClientSessionRecord, AdapterError> {
        let mut records = self.records.lock().await;
        let record = records
            .get(id)
            .cloned()
            .ok_or_else(|| AdapterError::UnknownSession(id.clone()))?;
        ensure_owner(&record, expected_epoch, expected_revision)?;
        let mut candidate = record;
        update(&mut candidate)?;
        match self
            .store
            .compare_and_swap(expected_epoch, expected_revision, &candidate)
            .await?
        {
            RegistryWriteOutcome::Applied => {
                records.insert(id.clone(), candidate.clone());
                Ok(candidate)
            }
            RegistryWriteOutcome::Conflict(current) => reconcile_conflict(
                &mut records,
                id,
                *current,
                expected_epoch,
                expected_revision,
            ),
        }
    }
}

fn validate_record(record: &ClientSessionRecord) -> Result<(), AdapterError> {
    record.capabilities.validate()?;
    if record.schema_version != CLIENT_ADAPTER_SCHEMA_VERSION {
        return Err(AdapterError::UnsupportedSchema {
            found: record.schema_version,
            supported: CLIENT_ADAPTER_SCHEMA_VERSION,
        });
    }
    if record.protocol != record.capabilities.protocol {
        return Err(AdapterError::InvalidFrame(
            "session protocol differs from capability snapshot".into(),
        ));
    }
    if record.client_session_id.0.trim().is_empty()
        || record.core_session_id.as_str().trim().is_empty()
        || record.connection_id.as_str().trim().is_empty()
    {
        return Err(AdapterError::InvalidFrame(
            "session and connection ids must be non-empty".into(),
        ));
    }
    Ok(())
}

fn reconcile_conflict(
    records: &mut BTreeMap<ClientSessionId, ClientSessionRecord>,
    id: &ClientSessionId,
    current: Option<ClientSessionRecord>,
    actual_epoch: u64,
    actual_revision: u64,
) -> Result<ClientSessionRecord, AdapterError> {
    match current {
        Some(current) => {
            records.insert(id.clone(), current.clone());
            Err(AdapterError::StaleOwner {
                client_session_id: id.clone(),
                expected_epoch: current.ownership_epoch,
                expected_revision: current.revision,
                actual_epoch,
                actual_revision,
            })
        }
        None => {
            records.remove(id);
            Err(AdapterError::UnknownSession(id.clone()))
        }
    }
}

fn ensure_owner(
    record: &ClientSessionRecord,
    expected_epoch: u64,
    expected_revision: u64,
) -> Result<(), AdapterError> {
    if record.ownership_epoch == expected_epoch && record.revision == expected_revision {
        Ok(())
    } else {
        Err(AdapterError::StaleOwner {
            client_session_id: record.client_session_id.clone(),
            expected_epoch: record.ownership_epoch,
            expected_revision: record.revision,
            actual_epoch: expected_epoch,
            actual_revision: expected_revision,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> CapabilitySnapshot {
        CapabilitySnapshot {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new("mock"),
            protocol_version: "1".into(),
            client_version: "1.0".into(),
            revision: 1,
            capabilities: [ClientCapability::new("events")].into_iter().collect(),
        }
    }

    fn record() -> ClientSessionRecord {
        ClientSessionRecord {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new("mock"),
            client_session_id: ClientSessionId::new("client-session"),
            core_session_id: SessionId::from("core-session"),
            connection_id: ConnectionId::from("connection-1"),
            ownership_epoch: 1,
            revision: 1,
            state: ClientSessionState::Loaded,
            capabilities: snapshot(),
            updated_at: Timestamp::from_unix_millis(1),
        }
    }

    #[tokio::test]
    async fn stale_owner_is_rejected_and_reattach_advances_epoch() {
        let store: Arc<dyn SessionRegistryStore> =
            Arc::new(InMemorySessionRegistryStore::default());
        let registry = SessionRegistry::new(store).await.expect("registry");
        registry.register(record()).await.expect("register");

        let claimed = registry
            .claim(
                &ClientSessionId::new("client-session"),
                1,
                1,
                ConnectionId::from("connection-2"),
                ClientSessionState::Subscribed,
                Timestamp::from_unix_millis(2),
            )
            .await
            .expect("claim");
        assert_eq!(claimed.ownership_epoch, 2);
        assert_eq!(claimed.revision, 2);

        let error = registry
            .transition(
                &claimed.client_session_id,
                1,
                1,
                ClientSessionState::Executing,
                Timestamp::from_unix_millis(3),
            )
            .await
            .expect_err("old owner must fail");
        assert!(matches!(error, AdapterError::StaleOwner { .. }));
    }

    #[tokio::test]
    async fn persisted_records_are_reloaded() {
        let store: Arc<dyn SessionRegistryStore> =
            Arc::new(InMemorySessionRegistryStore::default());
        let registry = SessionRegistry::new(store.clone()).await.expect("registry");
        registry.register(record()).await.expect("register");
        let reloaded = SessionRegistry::new(store).await.expect("reload");
        assert_eq!(
            reloaded.get(&ClientSessionId::new("client-session")).await,
            Some(record())
        );
    }

    #[tokio::test]
    async fn competing_registries_reject_stale_store_write_and_resync() {
        let store: Arc<dyn SessionRegistryStore> =
            Arc::new(InMemorySessionRegistryStore::default());
        let first = SessionRegistry::new(store.clone()).await.expect("first");
        first.register(record()).await.expect("register");
        let second = SessionRegistry::new(store).await.expect("second");

        let claimed = first
            .claim(
                &ClientSessionId::new("client-session"),
                1,
                1,
                ConnectionId::from("connection-2"),
                ClientSessionState::Subscribed,
                Timestamp::from_unix_millis(2),
            )
            .await
            .expect("first claim");
        let error = second
            .transition(
                &ClientSessionId::new("client-session"),
                1,
                1,
                ClientSessionState::Executing,
                Timestamp::from_unix_millis(3),
            )
            .await
            .expect_err("stale store CAS must fail");
        assert!(matches!(error, AdapterError::StaleOwner { .. }));
        assert_eq!(
            second.get(&ClientSessionId::new("client-session")).await,
            Some(claimed)
        );
    }

    #[tokio::test]
    async fn concurrent_claims_across_registries_advance_epoch_exactly_once() {
        let store: Arc<dyn SessionRegistryStore> =
            Arc::new(InMemorySessionRegistryStore::default());
        let first = Arc::new(SessionRegistry::new(store.clone()).await.expect("first"));
        first.register(record()).await.expect("register");
        let second = Arc::new(SessionRegistry::new(store).await.expect("second"));

        // 两个 registry 各自持有独立缓存，共享同一 store：并发 claim 必须
        // 由 store 的原子 CAS 裁决，恰好一个成功，其余全部 StaleOwner。
        let mut tasks = Vec::new();
        for _ in 0..8 {
            for registry in [Arc::clone(&first), Arc::clone(&second)] {
                tasks.push(tokio::spawn(async move {
                    registry
                        .claim(
                            &ClientSessionId::new("client-session"),
                            1,
                            1,
                            ConnectionId::from("connection-2"),
                            ClientSessionState::Subscribed,
                            Timestamp::from_unix_millis(2),
                        )
                        .await
                }));
            }
        }

        let mut applied = 0usize;
        for task in tasks {
            match task.await.expect("join") {
                Ok(claimed) => {
                    applied += 1;
                    assert_eq!(claimed.ownership_epoch, 2);
                    assert_eq!(claimed.revision, 2);
                    assert_eq!(claimed.connection_id.as_str(), "connection-2");
                }
                Err(AdapterError::StaleOwner { .. }) => {}
                Err(other) => panic!("unexpected error: {other:?}"),
            }
        }
        assert_eq!(applied, 1, "exactly one claim may win the CAS");
        let current = first
            .get(&ClientSessionId::new("client-session"))
            .await
            .expect("authoritative record");
        assert_eq!((current.ownership_epoch, current.revision), (2, 2));
        assert_eq!(
            second.get(&ClientSessionId::new("client-session")).await,
            Some(current)
        );
    }

    #[tokio::test]
    async fn remove_after_other_registry_removed_surfaces_conflict_none() {
        let store: Arc<dyn SessionRegistryStore> =
            Arc::new(InMemorySessionRegistryStore::default());
        let first = SessionRegistry::new(store.clone()).await.expect("first");
        first.register(record()).await.expect("register");
        let second = SessionRegistry::new(store.clone()).await.expect("second");
        let id = ClientSessionId::new("client-session");

        let removed = first
            .remove(&id, 1, 1)
            .await
            .expect("first remove succeeds");
        assert_eq!((removed.ownership_epoch, removed.revision), (1, 1));
        assert_eq!(first.get(&id).await, None);

        // store 行已删除：remove_if_owner 必须返回 Conflict(None) 而非假装成功。
        let outcome = store.remove_if_owner(&id, 1, 1).await.expect("store op");
        assert_eq!(outcome, RegistryWriteOutcome::Conflict(Box::new(None)));

        // second 的本地缓存仍持有旧记录（epoch/revision 匹配），但 store 行
        // 已消失：registry 层应同步清空缓存并返回 UnknownSession。
        assert!(matches!(
            second.remove(&id, 1, 1).await,
            Err(AdapterError::UnknownSession(_))
        ));
        assert_eq!(second.get(&id).await, None);
    }

    #[tokio::test]
    async fn mock_adapter_encodes_all_canonical_frames() {
        use crate::{
            AppEvent, AppEventEnvelope, AppResponse, AppResponseEnvelope, EventSource, EventStream,
            GlobalSequence, API_VERSION,
        };
        use pawork_domain::{CoreInstanceId, QueryId};

        let adapter = MockClientAdapter::new(snapshot()).expect("adapter");
        let response = CanonicalCoreFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from("query-1"),
            responded_at: Timestamp::from_unix_millis(2),
            response: AppResponse::Data(serde_json::json!({ "ok": true })),
        });
        let event = CanonicalCoreFrame::Event(AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("instance-1"),
            event_id: pawork_domain::EventId::from("event-1"),
            global_sequence: GlobalSequence(1),
            stream: EventStream::Session(SessionId::from("core-session")),
            stream_sequence: 1,
            timestamp: Timestamp::from_unix_millis(1),
            source: EventSource::Core,
            payload: AppEvent::SessionChanged {
                session_id: SessionId::from("core-session"),
                revision: 1,
            },
        });
        let state = CanonicalCoreFrame::SessionState(record());
        let error = CanonicalCoreFrame::Error(
            AdapterError::UnknownSession(ClientSessionId::new("missing")).frame(),
        );

        let cases = [
            (response, "query-1", "canonical.response"),
            (event, "event-1", "canonical.event"),
            (state, "client-session", "canonical.session_state"),
            (error, "adapter-error", "canonical.error"),
        ];
        for (frame, request_id, method) in cases {
            let encoded = adapter.encode(frame.clone()).await.expect("encode");
            assert_eq!(encoded.method, method);
            assert_eq!(encoded.request_id, request_id);
            let decoded: CanonicalCoreFrame =
                serde_json::from_value(encoded.payload).expect("payload round-trip");
            assert_eq!(decoded, frame);
        }
    }

    #[test]
    fn client_frame_validates_schema_and_preserves_extensions() {
        let raw = serde_json::json!({
            "schema_version": CLIENT_ADAPTER_SCHEMA_VERSION,
            "request_id": "request-1",
            "method": "session/query",
            "payload": {},
            "future_field": {"kept": true}
        });
        let frame: AdapterWireFrame = serde_json::from_value(raw.clone()).expect("decode");
        frame.validate().expect("valid");
        assert_eq!(frame.extensions["future_field"]["kept"], true);
        assert_eq!(serde_json::to_value(frame).expect("encode"), raw);
    }

    #[tokio::test]
    async fn mock_adapter_rejects_unknown_fields_and_unsupported_methods() {
        let adapter = MockClientAdapter::new(snapshot()).expect("adapter");
        let unknown = AdapterWireFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: "request-1".into(),
            method: "canonical.request".into(),
            payload: serde_json::Value::Null,
            extensions: [("future_field".into(), serde_json::json!(true))]
                .into_iter()
                .collect(),
        };
        assert!(matches!(
            adapter.decode(unknown).await,
            Err(AdapterError::InvalidFrame(_))
        ));

        let unsupported = AdapterWireFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: "request-2".into(),
            method: "future.method".into(),
            payload: serde_json::Value::Null,
            extensions: BTreeMap::new(),
        };
        assert_eq!(
            adapter.decode(unsupported).await,
            Err(AdapterError::ProtocolUnsupported("future.method".into()))
        );
    }

    #[test]
    fn unsupported_capability_has_explicit_error_frame() {
        let missing = ClientCapability::new("approval");
        assert!(!snapshot().supports(&missing));
        let error = AdapterError::CapabilityUnsupported(missing.clone());
        let frame = error.frame();
        assert_eq!(frame.code, "capability_unsupported");
        assert_eq!(frame.capability, Some(missing));
    }

    #[test]
    fn mock_factory_rejects_protocol_and_capability_mismatch() {
        let factory = MockClientAdapterFactory::new(
            ClientProtocol::new("mock"),
            [ClientCapability::new("events")],
        );

        let mut wrong_protocol = snapshot();
        wrong_protocol.protocol = ClientProtocol::new("acp");
        assert!(matches!(
            factory.create(wrong_protocol),
            Err(AdapterError::ProtocolUnsupported(protocol)) if protocol == "acp"
        ));

        let mut unsupported = snapshot();
        unsupported
            .capabilities
            .insert(ClientCapability::new("approval"));
        assert!(matches!(
            factory.create(unsupported),
            Err(AdapterError::CapabilityUnsupported(capability))
                if capability == ClientCapability::new("approval")
        ));

        let adapter = factory.create(snapshot()).expect("negotiated adapter");
        assert_eq!(adapter.protocol(), factory.protocol());
    }
}
