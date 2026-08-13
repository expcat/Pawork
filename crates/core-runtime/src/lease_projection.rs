//! P18-4 组合层适配：`app-database::LeaseRowRepository` → `LeaseProjection`。
//!
//! `app-database` 刻意不依赖 `provider-control`（存储层不反向拉入控制面行为
//! 类型，见 `app-database::lease` 模块注释）；本模块位于依赖双方的宿主组合层
//! （`core-runtime`），在 canonical [`LeaseRecord`]/[`LeaseEvent`] 与扁平
//! [`LeaseSnapshotRow`]/[`LeaseEventRow`] 之间做双向映射，并实现对象安全的
//! [`LeaseProjection`] 供持久 `CredentialPool` 注入。
//!
//! 映射约定（与 `app-database::lease` 的 DB 字符串词表对齐）：
//! - `state`：`LeaseState::as_db_str`（requested/acquired/released/expired/reclaimed）；
//! - `outcome`：`completed`/`cancelled`/`failed`/`released`（与
//!   [`LeaseOutcome`] 的 serde snake_case 一致，此处显式冻结不依赖 serde 推导）；
//! - 事件 `payload`：canonical [`LeaseEvent`] 的 serde JSON（仓库不解析）。
//!
//! 逆向解析（`load_outstanding`）遇到未知 state/outcome 或事件 JSON 解码失败
//! 一律返回 [`LeaseProjectionError::Backend`]（fail-closed：损坏行不得静默
//! 丢弃或伪造状态进入恢复）。

use agent_domain::Timestamp;
use app_database::{LeaseEventRow, LeaseRowRepository, LeaseSnapshotRow};
use async_trait::async_trait;
use provider_control::{
    LeaseEvent, LeaseId, LeaseOutcome, LeaseProjection, LeaseProjectionError, LeaseRecord,
    LeaseState,
};

/// `LeaseRowRepository` 的 `LeaseProjection` 适配器（生产持久化 sink）。
#[derive(Clone)]
pub struct SqliteLeaseProjection {
    repository: LeaseRowRepository,
}

impl SqliteLeaseProjection {
    /// 以行仓库构造（`LeaseRowRepository` 内部为 `Arc<DatabaseActor>`，clone 廉价）。
    pub fn new(repository: LeaseRowRepository) -> Self {
        Self { repository }
    }

    /// 借用底层仓库（诊断 / 测试复用）。
    pub fn repository(&self) -> &LeaseRowRepository {
        &self.repository
    }
}

#[async_trait]
impl LeaseProjection for SqliteLeaseProjection {
    async fn apply(
        &self,
        snapshot: &LeaseRecord,
        events: &[LeaseEvent],
    ) -> Result<(), LeaseProjectionError> {
        let row = lease_record_to_row(snapshot)?;
        let rows = events
            .iter()
            .map(lease_event_to_row)
            .collect::<Result<Vec<_>, _>>()?;
        self.repository
            .apply(&row, &rows)
            .await
            .map_err(map_repository_error)
    }

    async fn settle(&self, lease_id: &LeaseId) -> Result<(), LeaseProjectionError> {
        self.repository
            .settle(lease_id.as_str())
            .await
            .map_err(map_repository_error)
    }

    async fn load_outstanding(&self) -> Result<Vec<LeaseRecord>, LeaseProjectionError> {
        let rows = self
            .repository
            .load_outstanding()
            .await
            .map_err(map_repository_error)?;
        rows.into_iter()
            .map(|row| lease_row_to_record(&row))
            .collect()
    }
}

fn map_repository_error(error: app_database::LeaseProjectionError) -> LeaseProjectionError {
    LeaseProjectionError::Backend(error.to_string())
}

/// canonical 记录 → 扁平快照行（应用 `state`/`outcome` DB 字符串映射）。
fn lease_record_to_row(record: &LeaseRecord) -> Result<LeaseSnapshotRow, LeaseProjectionError> {
    Ok(LeaseSnapshotRow {
        lease_id: record.lease_id.as_str().to_string(),
        schema_version: record.schema_version,
        version: record.version,
        state: record.state.as_db_str().to_string(),
        tenant_id: record.tenant_id.as_str().to_string(),
        account_id: record.account_id.as_str().to_string(),
        provider_id: record.provider_id.as_str().to_string(),
        credential_id: record.credential_id.as_str().to_string(),
        principal_id: record.principal_id.as_str().to_string(),
        agent_id: record.agent_id.as_str().to_string(),
        session_id: record.session_id.as_str().to_string(),
        acquired_at_ms: record.acquired_at.as_unix_millis(),
        ttl_ms: record.ttl_ms,
        expires_at_ms: record.expires_at.as_unix_millis(),
        outcome: record.outcome.map(outcome_db_str).map(str::to_string),
        trace_id: record.trace_id.clone(),
    })
}

/// 扁平快照行 → canonical 记录（未知 state/outcome fail-closed）。
fn lease_row_to_record(row: &LeaseSnapshotRow) -> Result<LeaseRecord, LeaseProjectionError> {
    let state = LeaseState::from_db_str(&row.state).ok_or_else(|| {
        LeaseProjectionError::Backend(format!(
            "unknown lease state `{}` in persisted row {}",
            row.state, row.lease_id
        ))
    })?;
    let outcome = match row.outcome.as_deref() {
        None => None,
        Some(value) => Some(outcome_from_db_str(value).ok_or_else(|| {
            LeaseProjectionError::Backend(format!(
                "unknown lease outcome `{value}` in persisted row {}",
                row.lease_id
            ))
        })?),
    };
    Ok(LeaseRecord {
        lease_id: LeaseId::new(row.lease_id.clone()),
        schema_version: row.schema_version,
        state,
        version: row.version,
        tenant_id: row.tenant_id.clone().into(),
        account_id: row.account_id.clone().into(),
        provider_id: row.provider_id.clone().into(),
        credential_id: row.credential_id.clone().into(),
        principal_id: row.principal_id.clone().into(),
        agent_id: row.agent_id.clone().into(),
        session_id: row.session_id.clone().into(),
        acquired_at: Timestamp::from_unix_millis(row.acquired_at_ms),
        ttl_ms: row.ttl_ms,
        expires_at: Timestamp::from_unix_millis(row.expires_at_ms),
        outcome,
        trace_id: row.trace_id.clone(),
    })
}

/// canonical 事件 → 扁平事件行（payload 为 serde JSON，仓库不解析）。
fn lease_event_to_row(event: &LeaseEvent) -> Result<LeaseEventRow, LeaseProjectionError> {
    Ok(LeaseEventRow {
        seq: None,
        lease_id: event.lease_id().as_str().to_string(),
        version: event.version(),
        kind: event_kind_db_str(event).to_string(),
        payload: serde_json::to_string(event).map_err(|error| {
            LeaseProjectionError::Backend(format!("lease event serialization failed: {error}"))
        })?,
        at_ms: event_at_ms(event),
    })
}

/// 事件 kind 的 DB 字符串（与快照 `state` 同词表）。
fn event_kind_db_str(event: &LeaseEvent) -> &'static str {
    match event {
        LeaseEvent::Requested { .. } => "requested",
        LeaseEvent::Acquired { .. } => "acquired",
        LeaseEvent::Released { .. } => "released",
        LeaseEvent::Expired { .. } => "expired",
        LeaseEvent::Reclaimed { .. } => "reclaimed",
    }
}

/// 事件发生时刻（各变体字段名不同，统一提取）。
fn event_at_ms(event: &LeaseEvent) -> u64 {
    match event {
        LeaseEvent::Requested { at_ms, .. }
        | LeaseEvent::Released { at_ms, .. }
        | LeaseEvent::Expired { at_ms, .. }
        | LeaseEvent::Reclaimed { at_ms, .. } => *at_ms,
        LeaseEvent::Acquired { acquired_at_ms, .. } => *acquired_at_ms,
    }
}

/// `LeaseOutcome` → DB 字符串（冻结词表，与 serde snake_case 一致）。
fn outcome_db_str(outcome: LeaseOutcome) -> &'static str {
    match outcome {
        LeaseOutcome::Completed => "completed",
        LeaseOutcome::Cancelled => "cancelled",
        LeaseOutcome::Failed => "failed",
        LeaseOutcome::Released => "released",
    }
}

/// DB 字符串 → `LeaseOutcome`；未知值返回 `None`。
fn outcome_from_db_str(value: &str) -> Option<LeaseOutcome> {
    match value {
        "completed" => Some(LeaseOutcome::Completed),
        "cancelled" => Some(LeaseOutcome::Cancelled),
        "failed" => Some(LeaseOutcome::Failed),
        "released" => Some(LeaseOutcome::Released),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_domain::{
        AccountId, AgentId, CredentialId, PrincipalId, ProviderId, SessionId, TenantId,
    };
    use app_database::{DatabaseActor, LeaseEventRow};
    use provider_control::{
        AcquireRequest, CredentialPool, FixedLeaseClock, InMemoryCredentialPool, PoolConfig,
    };

    use super::*;

    fn sample_request() -> AcquireRequest {
        AcquireRequest {
            tenant_id: TenantId::new("tenant-a"),
            principal_id: PrincipalId::new("principal-a"),
            session_id: SessionId::new("session-a"),
            agent_id: AgentId::new("agent-a"),
            provider_id: Some(ProviderId::new("prov-1")),
            account_id: Some("acct-1".into()),
            trace_id: Some("trace-1".to_string()),
        }
    }

    fn opened_record(lease_id: &str) -> LeaseRecord {
        let clock = FixedLeaseClock::new(Timestamp::from_unix_millis(1_000));
        let (record, _, _) = LeaseRecord::open(
            &sample_request(),
            LeaseId::new(lease_id),
            CredentialId::new("cred-1"),
            &clock,
            5_000,
        );
        record
    }

    #[test]
    fn record_row_round_trip_preserves_all_fields() {
        let mut record = opened_record("lease-rt");
        let (released, _) = record
            .release(
                LeaseOutcome::Completed,
                &FixedLeaseClock::new(Timestamp::from_unix_millis(2_000)),
            )
            .expect("release");
        record = released;
        let row = lease_record_to_row(&record).expect("to row");
        let restored = lease_row_to_record(&row).expect("to record");
        assert_eq!(restored, record);
        assert_eq!(row.state, "released");
        assert_eq!(row.outcome.as_deref(), Some("completed"));
        assert_eq!(row.acquired_at_ms, 1_000);
        assert_eq!(row.expires_at_ms, 6_000);
    }

    #[test]
    fn event_row_round_trip_via_opaque_payload() {
        let clock = FixedLeaseClock::new(Timestamp::from_unix_millis(1_000));
        let (record, requested, acquired) = LeaseRecord::open(
            &sample_request(),
            LeaseId::new("lease-ev"),
            CredentialId::new("cred-1"),
            &clock,
            5_000,
        );
        let events = [requested.clone(), acquired.clone()];
        let rows: Vec<LeaseEventRow> = events
            .iter()
            .map(lease_event_to_row)
            .collect::<Result<_, _>>()
            .expect("to rows");
        assert_eq!(rows[0].kind, "requested");
        assert_eq!(rows[1].kind, "acquired");
        assert_eq!(rows[1].version, 2);
        assert_eq!(rows[1].at_ms, 1_000);
        // payload 是 opaque JSON，可反解回 canonical 事件（仓库不解析，此处验证映射忠实）。
        let decoded: LeaseEvent = serde_json::from_str(&rows[1].payload).expect("decode");
        assert_eq!(decoded, acquired);
        assert_eq!(decoded.lease_id(), &record.lease_id);
    }

    #[test]
    fn unknown_state_or_outcome_fails_closed() {
        let mut row = lease_record_to_row(&opened_record("lease-bad")).expect("to row");
        row.state = "bogus".to_string();
        assert!(matches!(
            lease_row_to_record(&row),
            Err(LeaseProjectionError::Backend(_))
        ));
        row.state = "released".to_string();
        row.outcome = Some("bogus".to_string());
        assert!(matches!(
            lease_row_to_record(&row),
            Err(LeaseProjectionError::Backend(_))
        ));
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-core-runtime-lease-projection-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn projection_persists_and_survives_actor_restart() {
        let path = temp_path("restart.sqlite3");
        {
            let actor = DatabaseActor::open(&path).await.expect("open");
            app_database::migrate_lease(&actor, &path, false)
                .await
                .expect("migrate");
            let projection = SqliteLeaseProjection::new(LeaseRowRepository::new(actor));
            let record = opened_record("lease-persist");
            let (released, event) = record
                .release(
                    LeaseOutcome::Completed,
                    &FixedLeaseClock::new(Timestamp::from_unix_millis(2_000)),
                )
                .expect("release");
            projection
                .apply(&released, std::slice::from_ref(&event))
                .await
                .expect("apply");
            let outstanding = projection.load_outstanding().await.expect("load");
            assert_eq!(outstanding.len(), 1);
            assert_eq!(outstanding[0], released);
            projection.settle(&released.lease_id).await.expect("settle");
            assert!(projection.load_outstanding().await.unwrap().is_empty());
        }
        // 模拟崩溃 / 重启：全新 Actor 重开同一文件，投影必须看到已持久化行。
        let actor = DatabaseActor::open(&path).await.expect("reopen");
        let projection = SqliteLeaseProjection::new(LeaseRowRepository::new(actor));
        let rows = projection
            .load_outstanding()
            .await
            .expect("load after restart");
        assert!(rows.is_empty(), "settle 后无 outstanding 行");
        // 仓库事件日志在 settle 后保留（append-only），并可读回。
        let events = projection
            .repository()
            .load_events("lease-persist")
            .await
            .expect("load events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "released");
        let decoded: LeaseEvent = serde_json::from_str(&events[0].payload).expect("decode");
        assert!(matches!(decoded, LeaseEvent::Released { .. }));
        let _ = std::fs::remove_file(path);
    }

    /// P18-4 审查补救：真实 `SqliteLeaseProjection` 跨「进程」重启回归——
    /// acquire 后崩溃（不释放）→ TTL 内重启 `restore` 恢复 active（canonical
    /// agent 身份跨重启保留）→ 时间越过 TTL 后重启 `restore` 回收过期孤儿
    /// （Expired + Reclaimed 持久化）→ 再次重启无复活、无额度泄漏。
    #[tokio::test]
    async fn restore_across_restart_recovers_then_reclaims_expired_orphans() {
        let path = temp_path("restore-expired.sqlite3");
        let t0 = Timestamp::from_unix_millis(1_000);
        let tenant = TenantId::new("tenant-a");
        let account = AccountId::new("acct-1");

        // 第一「进程」：真实 acquire（不释放）后崩溃——durable 只保留 Acquired 快照。
        {
            let actor = DatabaseActor::open(&path).await.expect("open");
            app_database::migrate_lease(&actor, &path, false)
                .await
                .expect("migrate");
            let pool = InMemoryCredentialPool::with_projection(
                PoolConfig::new(1).with_ttl_ms(100),
                Arc::new(FixedLeaseClock::new(t0)),
                Arc::new(SqliteLeaseProjection::new(LeaseRowRepository::new(actor))),
            );
            let lease = pool.acquire(sample_request()).await.expect("acquire");
            assert_eq!(pool.active_count_for(&tenant, &account), 1);
            // 模拟崩溃：不释放 lease（CredentialLease 无 Drop 副作用）。
            drop(lease);
        }

        // 第二「进程」：TTL 内重启 restore——孤儿恢复 active，agent 身份保留。
        {
            let actor = DatabaseActor::open(&path).await.expect("reopen");
            app_database::migrate_lease(&actor, &path, true)
                .await
                .expect("migrate idempotent");
            let projection = SqliteLeaseProjection::new(LeaseRowRepository::new(actor));
            let pool = InMemoryCredentialPool::with_projection(
                PoolConfig::new(1).with_ttl_ms(100),
                Arc::new(FixedLeaseClock::new(t0)),
                Arc::new(projection.clone()),
            );
            let report = pool.restore().await.expect("restore");
            assert_eq!((report.expired, report.reclaimed), (0, 0));
            assert_eq!(
                pool.active_count_for(&tenant, &account),
                1,
                "TTL 内孤儿在重启后恢复 active"
            );
            let rows = projection
                .load_outstanding()
                .await
                .expect("load after restore");
            assert_eq!(rows.len(), 1);
            assert_eq!(
                rows[0].agent_id.as_str(),
                "agent-a",
                "canonical agent 身份经真实投影跨重启保留"
            );
        }

        // 第三「进程」：时间越过 TTL 后重启 restore——过期孤儿直接回收并持久化。
        {
            let actor = DatabaseActor::open(&path).await.expect("reopen late");
            app_database::migrate_lease(&actor, &path, true)
                .await
                .expect("migrate idempotent");
            let projection = SqliteLeaseProjection::new(LeaseRowRepository::new(actor));
            let pool = InMemoryCredentialPool::with_projection(
                PoolConfig::new(1).with_ttl_ms(100),
                Arc::new(FixedLeaseClock::new(Timestamp::from_unix_millis(1_200))),
                Arc::new(projection.clone()),
            );
            let report = pool.restore().await.expect("restore expired orphan");
            assert_eq!(report.expired, 1, "过期孤儿标记 Expired");
            assert_eq!(report.reclaimed, 1, "过期孤儿回收为 Reclaimed");
            assert_eq!(
                pool.active_count_for(&tenant, &account),
                0,
                "过期孤儿不重建 active"
            );
            assert!(
                projection
                    .load_outstanding()
                    .await
                    .expect("load after reclaim")
                    .is_empty(),
                "终态快照已持久化并移出 outstanding"
            );
        }

        // 第四「进程」：Reclaimed 终态不复活——无 outstanding、无额度占用。
        {
            let actor = DatabaseActor::open(&path).await.expect("final reopen");
            app_database::migrate_lease(&actor, &path, true)
                .await
                .expect("migrate idempotent");
            let projection = SqliteLeaseProjection::new(LeaseRowRepository::new(actor));
            let pool = InMemoryCredentialPool::with_projection(
                PoolConfig::new(1).with_ttl_ms(100),
                Arc::new(FixedLeaseClock::new(Timestamp::from_unix_millis(1_200))),
                Arc::new(projection.clone()),
            );
            let report = pool.restore().await.expect("restore final");
            assert_eq!((report.expired, report.reclaimed), (0, 0));
            assert_eq!(pool.active_count_for(&tenant, &account), 0);
            assert!(projection
                .load_outstanding()
                .await
                .expect("load final")
                .is_empty());
        }
        let _ = std::fs::remove_file(path);
    }
}
