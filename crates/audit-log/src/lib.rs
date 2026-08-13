//! Tenant-scoped canonical audit events and allowlist-only export (P18-13).
//!
//! This crate deliberately stores identifiers and structured decisions only. Prompt text,
//! tool input/output, plaintext credentials, secret references and protected blobs are not
//! representable in [`AuditEventV1`] or [`ExportRecord`].

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use agent_domain::{
    AccountId, AgentId, EventId, PrincipalId, ProviderId, SessionId, TenantId, Timestamp,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current immutable canonical audit schema version.
pub const AUDIT_SCHEMA_VERSION: u32 = 1;

/// Stable audit action vocabulary. Free-form payloads are intentionally excluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    IdentityResolved,
    PolicyEvaluated,
    RouteEvaluated,
    LeaseAcquired,
    LeaseReleased,
    LeaseRebound,
    AgentLifecycle,
    ApprovalEvaluated,
    ToolLifecycle,
    ClientLifecycle,
    ConfigurationChanged,
    QuotaRefreshed,
    QuotaAlerted,
    AuditExported,
}

/// Stable decision vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditDecision {
    Allow,
    Deny,
    Limit,
    Fallback,
    Observe,
    Error,
}

/// Target category without target contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditTargetKind {
    Identity,
    Policy,
    Route,
    Lease,
    Agent,
    Approval,
    Tool,
    Client,
    Configuration,
    Quota,
    Audit,
}

/// Optional correlation dimensions shared by control-plane producers. The type contains
/// identifiers only; free-form request or response payloads are deliberately impossible.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditDimensions {
    pub session_id: Option<SessionId>,
    pub agent_id: Option<AgentId>,
    pub provider_id: Option<ProviderId>,
    pub account_id: Option<AccountId>,
    pub client_id: Option<String>,
    pub trace_id: Option<String>,
}

/// Canonical immutable audit event v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEventV1 {
    pub schema_version: u32,
    pub event_id: EventId,
    pub occurred_at: Timestamp,
    pub tenant_id: TenantId,
    pub principal_id: PrincipalId,
    pub action: AuditAction,
    pub target_kind: AuditTargetKind,
    pub decision: AuditDecision,
    /// Stable, redacted reason code; never a raw provider/tool/user message.
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<ProviderId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<AccountId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Monotonic version of the policy/config/state that produced the decision.
    pub decision_version: u64,
}

impl AuditEventV1 {
    /// Constructs a canonical event with no optional correlation dimensions.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_id: EventId,
        occurred_at: Timestamp,
        tenant_id: TenantId,
        principal_id: PrincipalId,
        action: AuditAction,
        target_kind: AuditTargetKind,
        decision: AuditDecision,
        reason_code: impl Into<String>,
        decision_version: u64,
    ) -> Self {
        Self {
            schema_version: AUDIT_SCHEMA_VERSION,
            event_id,
            occurred_at,
            tenant_id,
            principal_id,
            action,
            target_kind,
            decision,
            reason_code: reason_code.into(),
            session_id: None,
            agent_id: None,
            provider_id: None,
            account_id: None,
            client_id: None,
            trace_id: None,
            decision_version,
        }
    }

    /// Adds the allowlisted identifier dimensions to an event.
    pub fn with_dimensions(mut self, dimensions: AuditDimensions) -> Self {
        self.session_id = dimensions.session_id;
        self.agent_id = dimensions.agent_id;
        self.provider_id = dimensions.provider_id;
        self.account_id = dimensions.account_id;
        self.client_id = dimensions.client_id;
        self.trace_id = dimensions.trace_id;
        self
    }

    pub fn validate(&self) -> Result<(), AuditError> {
        if self.schema_version != AUDIT_SCHEMA_VERSION {
            return Err(AuditError::UnsupportedSchema(self.schema_version));
        }
        if self.reason_code.is_empty() || !safe_label(&self.reason_code) {
            return Err(AuditError::UnsafeLabel("reason_code"));
        }
        for (name, value) in [
            ("client_id", self.client_id.as_deref()),
            ("trace_id", self.trace_id.as_deref()),
        ] {
            if value.is_some_and(|value| !safe_label(value)) {
                return Err(AuditError::UnsafeLabel(name));
            }
        }
        Ok(())
    }
}

/// Append-only event sink. Implementations must preserve event ids and order.
pub trait AuditSink: Send + Sync {
    fn append(&self, event: AuditEventV1) -> Result<(), AuditError>;
}

/// Query surface always requires an explicit tenant scope.
pub trait AuditStore: AuditSink {
    fn query_tenant(&self, tenant: &TenantId) -> Result<Vec<AuditEventV1>, AuditError>;
    fn replay(&self) -> Result<Vec<AuditEventV1>, AuditError>;
}

/// Deterministic append-only in-memory implementation used by the composition layer and tests.
#[derive(Clone, Default)]
pub struct InMemoryAuditStore {
    events: Arc<Mutex<Vec<AuditEventV1>>>,
}

impl AuditSink for InMemoryAuditStore {
    fn append(&self, event: AuditEventV1) -> Result<(), AuditError> {
        event.validate()?;
        let mut events = lock(&self.events);
        if events
            .iter()
            .any(|stored| stored.event_id == event.event_id)
        {
            return Err(AuditError::DuplicateEvent(event.event_id.to_string()));
        }
        events.push(event);
        Ok(())
    }
}

impl AuditStore for InMemoryAuditStore {
    fn query_tenant(&self, tenant: &TenantId) -> Result<Vec<AuditEventV1>, AuditError> {
        Ok(lock(&self.events)
            .iter()
            .filter(|event| event.tenant_id == *tenant)
            .cloned()
            .collect())
    }

    fn replay(&self) -> Result<Vec<AuditEventV1>, AuditError> {
        Ok(lock(&self.events).clone())
    }
}

/// Durable JSONL audit projection. Opening the store validates every historical line and rejects
/// duplicate ids before accepting new writes; append syncs the file before updating memory.
pub struct FileAuditStore {
    path: PathBuf,
    events: Mutex<Vec<AuditEventV1>>,
}

impl FileAuditStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, AuditError> {
        let path = path.into();
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(AuditError::Io(error.to_string())),
        };
        let mut events = Vec::new();
        for (index, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event: AuditEventV1 =
                serde_json::from_str(line).map_err(|error| AuditError::CorruptLine {
                    line: index + 1,
                    message: error.to_string(),
                })?;
            event.validate()?;
            if events
                .iter()
                .any(|stored: &AuditEventV1| stored.event_id == event.event_id)
            {
                return Err(AuditError::DuplicateEvent(event.event_id.to_string()));
            }
            events.push(event);
        }
        Ok(Self {
            path,
            events: Mutex::new(events),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AuditSink for FileAuditStore {
    fn append(&self, event: AuditEventV1) -> Result<(), AuditError> {
        event.validate()?;
        let mut events = lock(&self.events);
        if events
            .iter()
            .any(|stored| stored.event_id == event.event_id)
        {
            return Err(AuditError::DuplicateEvent(event.event_id.to_string()));
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| AuditError::Io(error.to_string()))?;
        }
        let line =
            serde_json::to_string(&event).map_err(|error| AuditError::Json(error.to_string()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| AuditError::Io(error.to_string()))?;
        writeln!(file, "{line}").map_err(|error| AuditError::Io(error.to_string()))?;
        file.sync_data()
            .map_err(|error| AuditError::Io(error.to_string()))?;
        events.push(event);
        Ok(())
    }
}

impl AuditStore for FileAuditStore {
    fn query_tenant(&self, tenant: &TenantId) -> Result<Vec<AuditEventV1>, AuditError> {
        Ok(lock(&self.events)
            .iter()
            .filter(|event| event.tenant_id == *tenant)
            .cloned()
            .collect())
    }

    fn replay(&self) -> Result<Vec<AuditEventV1>, AuditError> {
        Ok(lock(&self.events).clone())
    }
}

/// OTel/SIEM-neutral allowlist record. It cannot contain prompt/tool/secret/blob fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportRecord {
    pub schema_version: u32,
    pub event_id: String,
    pub occurred_at_ms: u64,
    pub action: String,
    pub target_kind: String,
    pub decision: String,
    pub reason_code: String,
    pub attributes: BTreeMap<String, String>,
}

/// Destination abstraction implemented by an OTel/SIEM adapter outside this crate.
pub trait AuditExporter: Send + Sync {
    fn export(&self, records: &[ExportRecord]) -> Result<(), AuditError>;
}

/// Exports only events belonging to `tenant` and only identifier dimensions on the allowlist.
pub fn export_tenant(
    store: &dyn AuditStore,
    tenant: &TenantId,
    exporter: &dyn AuditExporter,
) -> Result<usize, AuditError> {
    let events = store.query_tenant(tenant)?;
    let records: Vec<_> = events.iter().map(ExportRecord::from).collect();
    exporter.export(&records)?;
    Ok(records.len())
}

impl From<&AuditEventV1> for ExportRecord {
    fn from(event: &AuditEventV1) -> Self {
        let mut attributes = BTreeMap::new();
        attributes.insert("tenant_id".into(), event.tenant_id.to_string());
        attributes.insert("principal_id".into(), event.principal_id.to_string());
        if let Some(value) = &event.session_id {
            attributes.insert("session_id".into(), value.to_string());
        }
        if let Some(value) = &event.agent_id {
            attributes.insert("agent_id".into(), value.to_string());
        }
        if let Some(value) = &event.provider_id {
            attributes.insert("provider_id".into(), value.to_string());
        }
        if let Some(value) = &event.account_id {
            attributes.insert("account_id".into(), value.to_string());
        }
        if let Some(value) = &event.client_id {
            attributes.insert("client_id".into(), value.clone());
        }
        if let Some(value) = &event.trace_id {
            attributes.insert("trace_id".into(), value.clone());
        }
        attributes.insert(
            "decision_version".into(),
            event.decision_version.to_string(),
        );
        Self {
            schema_version: event.schema_version,
            event_id: event.event_id.to_string(),
            occurred_at_ms: event.occurred_at.as_unix_millis(),
            action: snake_debug(event.action),
            target_kind: snake_debug(event.target_kind),
            decision: snake_debug(event.decision),
            reason_code: event.reason_code.clone(),
            attributes,
        }
    }
}

fn snake_debug(value: impl std::fmt::Debug) -> String {
    let input = format!("{value:?}");
    let mut output = String::new();
    for (index, character) in input.chars().enumerate() {
        if character.is_ascii_uppercase() && index > 0 {
            output.push('_');
        }
        output.push(character.to_ascii_lowercase());
    }
    output
}

fn safe_label(value: &str) -> bool {
    value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/')
        })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AuditError {
    #[error("unsupported audit schema version {0}")]
    UnsupportedSchema(u32),
    #[error("unsafe audit label in {0}")]
    UnsafeLabel(&'static str),
    #[error("duplicate audit event {0}")]
    DuplicateEvent(String),
    #[error("audit export failed: {0}")]
    Export(String),
    #[error("audit store I/O failed: {0}")]
    Io(String),
    #[error("audit JSON failed: {0}")]
    Json(String),
    #[error("corrupt audit line {line}: {message}")]
    CorruptLine { line: usize, message: String },
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn event(id: &str, tenant: &str) -> AuditEventV1 {
        AuditEventV1 {
            schema_version: AUDIT_SCHEMA_VERSION,
            event_id: EventId::new(id),
            occurred_at: Timestamp::from_unix_millis(42),
            tenant_id: TenantId::new(tenant),
            principal_id: PrincipalId::new(format!("{tenant}:user")),
            action: AuditAction::LeaseAcquired,
            target_kind: AuditTargetKind::Lease,
            decision: AuditDecision::Allow,
            reason_code: "lease_acquired".into(),
            session_id: Some(SessionId::new("session-1")),
            agent_id: Some(AgentId::new("agent-1")),
            provider_id: Some(ProviderId::new("provider-1")),
            account_id: Some(AccountId::new("account-1")),
            client_id: Some("client-1".into()),
            trace_id: Some("trace-1".into()),
            decision_version: 7,
        }
    }

    #[test]
    fn replay_rebuilds_order_and_duplicate_ids_fail_closed() {
        let store = InMemoryAuditStore::default();
        store.append(event("event-1", "tenant-a")).unwrap();
        store.append(event("event-2", "tenant-b")).unwrap();
        assert_eq!(store.replay().unwrap().len(), 2);
        assert!(matches!(
            store.append(event("event-1", "tenant-a")),
            Err(AuditError::DuplicateEvent(_))
        ));
    }

    #[test]
    fn tenant_queries_never_cross_boundaries() {
        let store = InMemoryAuditStore::default();
        store.append(event("event-1", "tenant-a")).unwrap();
        store.append(event("event-2", "tenant-b")).unwrap();
        let a = store.query_tenant(&TenantId::new("tenant-a")).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].event_id, EventId::new("event-1"));
    }

    #[derive(Default)]
    struct Capture(Mutex<Vec<ExportRecord>>);
    impl AuditExporter for Capture {
        fn export(&self, records: &[ExportRecord]) -> Result<(), AuditError> {
            lock(&self.0).extend_from_slice(records);
            Ok(())
        }
    }

    #[test]
    fn export_is_tenant_scoped_and_structurally_redacted() {
        let store = InMemoryAuditStore::default();
        store.append(event("event-1", "tenant-a")).unwrap();
        store.append(event("event-2", "tenant-b")).unwrap();
        let capture = Capture::default();
        assert_eq!(
            export_tenant(&store, &TenantId::new("tenant-a"), &capture).unwrap(),
            1
        );
        let json = serde_json::to_string(&*lock(&capture.0)).unwrap();
        for forbidden in ["prompt", "tool_output", "secret_ref", "protected_blob"] {
            assert!(!json.contains(forbidden), "export leaked field {forbidden}");
        }
        let allowed: BTreeSet<_> = lock(&capture.0)[0].attributes.keys().cloned().collect();
        assert_eq!(
            allowed,
            [
                "account_id",
                "agent_id",
                "client_id",
                "decision_version",
                "principal_id",
                "provider_id",
                "session_id",
                "tenant_id",
                "trace_id",
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        );
    }

    #[test]
    fn free_form_secret_bearing_reason_is_rejected() {
        let mut unsafe_event = event("event-1", "tenant-a");
        unsafe_event.reason_code = "Bearer sk-secret value".into();
        assert!(matches!(
            unsafe_event.validate(),
            Err(AuditError::UnsafeLabel("reason_code"))
        ));
    }

    #[test]
    fn durable_store_reopens_and_preserves_tenant_isolation() {
        let path = std::env::temp_dir().join(format!(
            "pawork-audit-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let store = FileAuditStore::open(&path).unwrap();
            store.append(event("event-1", "tenant-a")).unwrap();
            store.append(event("event-2", "tenant-b")).unwrap();
        }
        let reopened = FileAuditStore::open(&path).unwrap();
        assert_eq!(reopened.replay().unwrap().len(), 2);
        assert_eq!(
            reopened
                .query_tenant(&TenantId::new("tenant-a"))
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            reopened.append(event("event-1", "tenant-a")),
            Err(AuditError::DuplicateEvent(_))
        ));
        let _ = std::fs::remove_file(path);
    }
}
