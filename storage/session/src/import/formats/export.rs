//! Session 导出 JSON 形状（`EXPORT_SCHEMA_VERSION = 3`）。
//!
//! 解析 / 校验纯函数；`SessionStore::{export_session, import_session}` 在 persist 侧。

use pawork_domain::{AgentEventEnvelope, PrincipalId, TenantId};
use serde::{Deserialize, Serialize};

use crate::{SessionStoreError, DEFAULT_BRANCH_ID};

/// 当前导出 schema 版本。
pub const EXPORT_SCHEMA_VERSION: u32 = 3;

pub(crate) const LEGACY_TENANT: &str = "local/default";
pub(crate) const LEGACY_PRINCIPAL: &str = "local/user";

/// 一个分支的导出表示。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedBranch {
    pub branch_id: String,
    pub parent_branch_id: Option<String>,
    pub forked_from_event_id: Option<String>,
    pub head_sequence: u64,
}

/// 一条事件及其原始 branch 归属。
///
/// `branch_id` 属于 Event Store 的存储维度，并非 [`AgentEventEnvelope`] 的 canonical
/// 字段，因此必须由导出 schema 显式携带，才能在多分支往返时无损恢复。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExportedEvent {
    pub branch_id: String,
    pub event: AgentEventEnvelope,
}

impl<'de> Deserialize<'de> for ExportedEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WireEvent {
            V2 {
                branch_id: String,
                event: AgentEventEnvelope,
            },
            /// v1 只携带 envelope，历史导出无法恢复分支归属，安全降级到 main。
            V1(AgentEventEnvelope),
        }

        Ok(match WireEvent::deserialize(deserializer)? {
            WireEvent::V2 { branch_id, event } => Self { branch_id, event },
            WireEvent::V1(event) => Self {
                branch_id: DEFAULT_BRANCH_ID.to_string(),
                event,
            },
        })
    }
}

/// 一个 session 的完整导出（稳定 schema）。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionExport {
    /// 导出 schema 版本；当前写 v3，读取兼容 v1～[`EXPORT_SCHEMA_VERSION`]。
    pub schema_version: u32,
    pub session_id: String,
    /// v3 起显式携带身份；v1/v2 经 [`Self::from_json`] 安全回填 legacy 默认。
    pub tenant_id: TenantId,
    pub principal_id: PrincipalId,
    pub title: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub archived: bool,
    pub active_branch: String,
    /// 分支树（含 main）。
    pub branches: Vec<ExportedBranch>,
    /// 全部事件（事实来源），按 sequence 升序，并携带原始 branch 归属。
    pub events: Vec<ExportedEvent>,
    /// 标签（小写归一）。
    #[serde(default)]
    pub tags: Vec<String>,
}

impl<'de> Deserialize<'de> for SessionExport {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireSessionExport {
            schema_version: u32,
            session_id: String,
            #[serde(default)]
            tenant_id: Option<TenantId>,
            #[serde(default)]
            principal_id: Option<PrincipalId>,
            title: String,
            created_at_ms: u64,
            updated_at_ms: u64,
            archived: bool,
            active_branch: String,
            branches: Vec<ExportedBranch>,
            events: Vec<ExportedEvent>,
            #[serde(default)]
            tags: Vec<String>,
        }

        let wire = WireSessionExport::deserialize(deserializer)?;
        let (tenant_id, principal_id) = if (1..=2).contains(&wire.schema_version) {
            // 历史版本没有可信 identity：忽略可能夹带的未知同名字段，固定回填。
            (
                TenantId::new(LEGACY_TENANT),
                PrincipalId::new(LEGACY_PRINCIPAL),
            )
        } else {
            let tenant_id = wire
                .tenant_id
                .filter(|value| !value.as_str().trim().is_empty())
                .ok_or_else(|| serde::de::Error::custom("session export tenant_id is missing"))?;
            let principal_id = wire
                .principal_id
                .filter(|value| !value.as_str().trim().is_empty())
                .ok_or_else(|| {
                    serde::de::Error::custom("session export principal_id is missing")
                })?;
            (tenant_id, principal_id)
        };

        Ok(Self {
            schema_version: wire.schema_version,
            session_id: wire.session_id,
            tenant_id,
            principal_id,
            title: wire.title,
            created_at_ms: wire.created_at_ms,
            updated_at_ms: wire.updated_at_ms,
            archived: wire.archived,
            active_branch: wire.active_branch,
            branches: wire.branches,
            events: wire.events,
            tags: wire.tags,
        })
    }
}

impl SessionExport {
    /// 序列化为紧凑 JSON 字符串。
    pub fn to_json(&self) -> Result<String, SessionStoreError> {
        serde_json::to_string(self).map_err(SessionStoreError::from)
    }

    /// 从 JSON 反序列化并校验 schema 版本。
    pub fn from_json(json: &str) -> Result<Self, SessionStoreError> {
        let export: SessionExport = serde_json::from_str(json).map_err(SessionStoreError::from)?;
        export.validate()?;
        Ok(export)
    }

    /// 校验 schema 版本与非空身份。v1 可读并按 main 分支迁移，新导出写 v3。
    pub fn validate(&self) -> Result<(), SessionStoreError> {
        if !(1..=EXPORT_SCHEMA_VERSION).contains(&self.schema_version) {
            return Err(SessionStoreError::ExportSchemaVersion {
                found: self.schema_version,
                supported: EXPORT_SCHEMA_VERSION,
            });
        }
        if self.tenant_id.as_str().trim().is_empty() || self.principal_id.as_str().trim().is_empty()
        {
            return Err(SessionStoreError::ExportIdentityMissing);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use pawork_domain::{
        AgentEvent, AgentEventEnvelope, EventId, EventSequence, RunId, SessionId, Timestamp,
    };

    use super::*;

    fn event(session: &SessionId, seq: u64, payload: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from(format!("event-{seq}")),
            session.clone(),
            RunId::from("run-1"),
            EventSequence::new(seq),
            Timestamp::from_unix_millis(1000 + seq),
            payload,
        )
    }

    #[test]
    fn import_rejects_unsupported_schema_version() {
        let mut export = SessionExport {
            schema_version: 999,
            session_id: "x".into(),
            tenant_id: TenantId::new(LEGACY_TENANT),
            principal_id: PrincipalId::new(LEGACY_PRINCIPAL),
            title: "t".into(),
            created_at_ms: 1,
            updated_at_ms: 1,
            archived: false,
            active_branch: DEFAULT_BRANCH_ID.into(),
            branches: vec![ExportedBranch {
                branch_id: DEFAULT_BRANCH_ID.into(),
                parent_branch_id: None,
                forked_from_event_id: None,
                head_sequence: 0,
            }],
            events: vec![],
            tags: vec![],
        };
        let json = serde_json::to_string(&export).expect("serialize");
        assert!(SessionExport::from_json(&json).is_err());
        export.schema_version = EXPORT_SCHEMA_VERSION;
        assert!(export.validate().is_ok());
    }

    #[test]
    fn schema_v1_json_migrates_events_to_main_branch() {
        let session = SessionId::from("legacy-session");
        let legacy = serde_json::json!({
            "schema_version": 1,
            "session_id": session.as_str(),
            "title": "legacy",
            "created_at_ms": 1,
            "updated_at_ms": 1,
            "archived": false,
            "active_branch": DEFAULT_BRANCH_ID,
            "branches": [{
                "branch_id": DEFAULT_BRANCH_ID,
                "parent_branch_id": null,
                "forked_from_event_id": null,
                "head_sequence": 1
            }],
            "events": [event(
                &session,
                1,
                AgentEvent::RunCancelled {
                    reason: None,
                    usage: None,
                },
            )],
            "tags": []
        });
        let decoded = SessionExport::from_json(&legacy.to_string()).expect("read v1");
        assert_eq!(decoded.schema_version, 1);
        assert_eq!(decoded.tenant_id.as_str(), LEGACY_TENANT);
        assert_eq!(decoded.principal_id.as_str(), LEGACY_PRINCIPAL);
        assert_eq!(decoded.events[0].branch_id, DEFAULT_BRANCH_ID);
    }

    #[test]
    fn schema_v2_json_backfills_frozen_legacy_identity() {
        let legacy = serde_json::json!({
            "schema_version": 2,
            "session_id": "legacy-v2",
            "title": "legacy",
            "created_at_ms": 1,
            "updated_at_ms": 1,
            "archived": false,
            "active_branch": DEFAULT_BRANCH_ID,
            "branches": [],
            "events": [],
            "tags": [],
            // v2 未定义这些字段；即便夹带也不得把旧包归到任意租户。
            "tenant_id": "tenant-attacker",
            "principal_id": "principal-attacker"
        });
        let decoded = SessionExport::from_json(&legacy.to_string()).expect("read v2");
        assert_eq!(decoded.tenant_id.as_str(), LEGACY_TENANT);
        assert_eq!(decoded.principal_id.as_str(), LEGACY_PRINCIPAL);
    }

    #[test]
    fn schema_v3_missing_identity_is_rejected() {
        let missing = serde_json::json!({
            "schema_version": EXPORT_SCHEMA_VERSION,
            "session_id": "missing-identity",
            "title": "missing",
            "created_at_ms": 1,
            "updated_at_ms": 1,
            "archived": false,
            "active_branch": DEFAULT_BRANCH_ID,
            "branches": [],
            "events": [],
            "tags": []
        });
        assert!(SessionExport::from_json(&missing.to_string()).is_err());
    }
}
