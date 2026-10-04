//! 与费用账本共用连接的调用日志。未知用量也持久化，费用账本仍只追加。
use crate::usage::{InMemoryUsageLedger, UsageLedgerError};
use pawork_domain::*;
use std::collections::BTreeMap;

pub(crate) fn unsupported() -> UsageLedgerError {
    storage("task usage journal is unavailable")
}
fn storage(error: impl std::fmt::Display) -> UsageLedgerError {
    UsageLedgerError::Storage {
        reason: error.to_string(),
    }
}
fn invalid(reason: &str) -> UsageLedgerError {
    UsageLedgerError::InvalidRecord {
        reason: reason.into(),
    }
}
fn conflict(record: &TaskUsageRecord) -> UsageLedgerError {
    UsageLedgerError::Conflict {
        record_id: record.id.clone(),
    }
}

fn validate(record: &TaskUsageRecord) -> Result<(), UsageLedgerError> {
    if !valid_usage_id(&record.id)
        || !valid_usage_id(&record.client)
        || record.started_at_ms == 0
        || record.started_at_ms > i64::MAX as u64
        || record
            .finished_at_ms
            .is_some_and(|t| t < record.started_at_ms || t > i64::MAX as u64)
        || record
            .context
            .as_ref()
            .is_some_and(|c| !c.validate() || c.operation != record.operation)
        || [
            record.provider.as_deref(),
            record.model.as_deref(),
            record.upstream_task_id.as_deref(),
            record.related_call_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|v| !valid_usage_id(v))
        || record.error_code.as_ref().is_some_and(|v| {
            v.is_empty()
                || v.len() > 128
                || !v
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        })
    {
        return Err(invalid("invalid task usage identity, time or context"));
    }
    if let Some(cost) = &record.cost {
        if cost.value.currency.len() != 3
            || cost.value.currency == "XXX"
            || !cost.value.currency.bytes().all(|b| b.is_ascii_uppercase())
        {
            return Err(invalid("task usage cost requires a known ISO currency"));
        }
    }
    if record.source == TaskUsageSource::ClientReport
        && (!record.operation.is_client_operation()
            || record.tokens.is_some()
            || record.cost.is_some()
            || record.output_images.is_some()
            || record.planned_video_seconds.is_some()
            || record.provider.is_some()
            || record.model.is_some())
    {
        return Err(invalid(
            "client operation reports cannot claim provider usage",
        ));
    }
    if (record.output_images.is_some() && record.operation != TaskUsageOperation::Image)
        || (record.planned_video_seconds.is_some() && record.operation != TaskUsageOperation::Video)
    {
        return Err(invalid(
            "media units must belong to the generation operation",
        ));
    }
    Ok(())
}

fn update(
    existing: &TaskUsageRecord,
    next: &TaskUsageRecord,
    finishing: bool,
) -> Result<(), UsageLedgerError> {
    if existing == next {
        return Ok(());
    }
    if !finishing {
        return Err(conflict(next));
    }
    let mut fixed = next.clone();
    fixed.status = existing.status;
    fixed.finished_at_ms = existing.finished_at_ms;
    fixed.tokens = existing.tokens.clone();
    fixed.cost = existing.cost.clone();
    fixed.output_images = existing.output_images;
    fixed.upstream_task_id = existing.upstream_task_id.clone();
    fixed.upstream_status = existing.upstream_status;
    fixed.error_code = existing.error_code.clone();
    if &fixed != existing
        || !matches!(
            existing.status,
            TaskUsageStatus::Running | TaskUsageStatus::Submitted | TaskUsageStatus::Unknown
        )
        || existing
            .finished_at_ms
            .is_some_and(|t| next.finished_at_ms != Some(t))
        || existing
            .tokens
            .as_ref()
            .is_some_and(|v| next.tokens.as_ref() != Some(v))
        || existing
            .cost
            .as_ref()
            .is_some_and(|v| next.cost.as_ref() != Some(v))
        || existing
            .upstream_task_id
            .as_ref()
            .is_some_and(|v| next.upstream_task_id.as_ref() != Some(v))
    {
        return Err(conflict(next));
    }
    Ok(())
}

fn validate_links(
    record: &TaskUsageRecord,
    records: &[TaskUsageRecord],
) -> Result<(), UsageLedgerError> {
    for (id, retry) in [
        (record.related_call_id.as_deref(), false),
        (
            record.context.as_ref().and_then(|c| c.retry_of.as_deref()),
            true,
        ),
    ] {
        if let Some(id) = id {
            let parent = records
                .iter()
                .find(|r| r.client == record.client && r.id == id)
                .ok_or_else(|| invalid("related call is unavailable in this client"))?;
            if parent.id == record.id
                || (retry
                    && parent.context.as_ref().map(|c| &c.task.id)
                        != record.context.as_ref().map(|c| &c.task.id))
            {
                return Err(invalid("related call must belong to the same task"));
            }
        }
    }
    Ok(())
}

impl InMemoryUsageLedger {
    pub(crate) fn journal_write(
        &self,
        record: TaskUsageRecord,
        finishing: bool,
    ) -> Result<(), UsageLedgerError> {
        validate(&record)?;
        let mut records = self.task_usage.lock().map_err(storage)?;
        if let Some(existing) = records
            .iter_mut()
            .find(|r| r.client == record.client && r.id == record.id)
        {
            update(existing, &record, finishing)?;
            *existing = record;
        } else {
            if finishing {
                return Err(invalid("cannot finish a missing call"));
            }
            validate_links(&record, &records)?;
            records.push(record);
        }
        Ok(())
    }
    pub(crate) fn journal_get(
        &self,
        client: &str,
        id: &str,
    ) -> Result<Option<TaskUsageRecord>, UsageLedgerError> {
        Ok(self
            .task_usage
            .lock()
            .map_err(storage)?
            .iter()
            .find(|r| r.client == client && r.id == id)
            .cloned())
    }
    pub(crate) fn journal_video(
        &self,
        client: Option<&str>,
        id: &str,
    ) -> Result<Option<TaskUsageRecord>, UsageLedgerError> {
        Ok(self
            .task_usage
            .lock()
            .map_err(storage)?
            .iter()
            .find(|r| {
                client.is_none_or(|c| r.client == c)
                    && r.operation == TaskUsageOperation::Video
                    && r.upstream_task_id.as_deref() == Some(id)
            })
            .cloned())
    }
    pub(crate) fn journal_report(
        &self,
        query: &TaskUsageQuery,
    ) -> Result<TaskUsageReport, UsageLedgerError> {
        report(
            self.task_usage
                .lock()
                .map_err(storage)?
                .iter()
                .filter(|r| matches(r, query))
                .cloned()
                .collect(),
            query,
        )
    }
}

fn matches(r: &TaskUsageRecord, q: &TaskUsageQuery) -> bool {
    q.client.as_ref().is_none_or(|v| &r.client == v)
        && q.task_id
            .as_deref()
            .is_none_or(|v| r.context.as_ref().is_some_and(|c| c.task.id == v))
        && q.group_id.as_deref().is_none_or(|v| {
            r.context
                .as_ref()
                .and_then(|c| c.group.as_ref())
                .is_some_and(|g| g.id == v)
        })
        && q.subtask_id.as_deref().is_none_or(|v| {
            r.context
                .as_ref()
                .and_then(|c| c.subtask.as_ref())
                .is_some_and(|s| s.id == v)
        })
        && q.operation.is_none_or(|v| r.operation == v)
        && q.provider
            .as_ref()
            .is_none_or(|v| r.provider.as_ref() == Some(v))
        && q.model.as_ref().is_none_or(|v| r.model.as_ref() == Some(v))
        && q.status.is_none_or(|v| r.status == v)
        && q.started_after_ms.is_none_or(|v| r.started_at_ms >= v)
        && q.started_before_ms.is_none_or(|v| r.started_at_ms < v)
}

fn add(t: &mut TaskUsageTotals, r: &TaskUsageRecord) {
    t.records = t.records.saturating_add(1);
    t.generation_calls = t
        .generation_calls
        .saturating_add(u64::from(r.operation.is_generation()));
    t.failed = t
        .failed
        .saturating_add(u64::from(r.status == TaskUsageStatus::Failed));
    t.cancelled = t
        .cancelled
        .saturating_add(u64::from(r.status == TaskUsageStatus::Cancelled));
    t.unfinished = t.unfinished.saturating_add(u64::from(matches!(
        r.status,
        TaskUsageStatus::Running | TaskUsageStatus::Submitted | TaskUsageStatus::Unknown
    )));
    let metered = r.operation.is_generation() || r.operation == TaskUsageOperation::Coding;
    if let Some(u) = &r.tokens {
        t.tokens.input_tokens = t.tokens.input_tokens.saturating_add(u.input_tokens);
        t.tokens.output_tokens = t.tokens.output_tokens.saturating_add(u.output_tokens);
        t.tokens.cache_read_tokens = t
            .tokens
            .cache_read_tokens
            .saturating_add(u.cache_read_tokens);
        t.tokens.cache_write_tokens = t
            .tokens
            .cache_write_tokens
            .saturating_add(u.cache_write_tokens);
    } else if metered {
        t.unknown_token_calls = t.unknown_token_calls.saturating_add(1);
    }
    if let Some(cost) = &r.cost {
        let index = t
            .currencies
            .iter()
            .position(|c| c.currency == cost.value.currency)
            .unwrap_or_else(|| {
                t.currencies.push(TaskUsageCurrencyTotal {
                    currency: cost.value.currency.clone(),
                    ..Default::default()
                });
                t.currencies.len() - 1
            });
        let c = &mut t.currencies[index];
        match cost.kind {
            TaskUsageCostKind::Actual => {
                c.actual_micros = c.actual_micros.saturating_add(cost.value.amount_micros);
                c.actual_records = c.actual_records.saturating_add(1);
            }
            TaskUsageCostKind::Estimated => {
                c.estimated_micros = c.estimated_micros.saturating_add(cost.value.amount_micros);
                c.estimated_records = c.estimated_records.saturating_add(1);
            }
        }
        t.currencies.sort_by(|a, b| a.currency.cmp(&b.currency));
    } else if metered {
        t.unknown_cost_calls = t.unknown_cost_calls.saturating_add(1);
    }
    if r.operation == TaskUsageOperation::Image {
        t.output_images = t.output_images.saturating_add(r.output_images.unwrap_or(0));
    }
    if r.operation == TaskUsageOperation::Video {
        t.planned_video_seconds = t
            .planned_video_seconds
            .saturating_add(r.planned_video_seconds.unwrap_or(0));
    }
}

fn group(r: &TaskUsageRecord, by: TaskUsageGroupBy) -> (String, String) {
    let task = r.context.as_ref().map(|c| &c.task);
    match by {
        TaskUsageGroupBy::Task => task
            .map(|t| (t.id.clone(), t.title.clone()))
            .unwrap_or(("".into(), "未关联任务".into())),
        TaskUsageGroupBy::Group | TaskUsageGroupBy::Subtask => {
            let item = r.context.as_ref().and_then(|c| {
                if by == TaskUsageGroupBy::Group {
                    c.group.as_ref()
                } else {
                    c.subtask.as_ref()
                }
            });
            // 编码长度前缀，避免 task / child ID 中的分隔符产生碰撞。
            let root = task.map(|t| t.id.as_str()).unwrap_or("");
            let child = item.map(|t| t.id.as_str()).unwrap_or("");
            (
                format!("{}:{root}{child}", root.len()),
                item.map(|t| t.title.clone()).unwrap_or("未分组".into()),
            )
        }
        TaskUsageGroupBy::Operation => (
            serde_json::to_value(r.operation)
                .expect("enum")
                .as_str()
                .expect("enum string")
                .into(),
            format!("{:?}", r.operation),
        ),
        TaskUsageGroupBy::Model => {
            let p = r.provider.as_deref().unwrap_or("");
            let m = r.model.as_deref().unwrap_or("");
            (
                format!("{}:{p}{m}", p.len()),
                if p.is_empty() && m.is_empty() {
                    "未指定模型".into()
                } else {
                    format!("{p}/{m}")
                },
            )
        }
        TaskUsageGroupBy::Client => (r.client.clone(), r.client.clone()),
    }
}

fn cursor(r: &TaskUsageRecord) -> TaskUsageCursor {
    TaskUsageCursor {
        started_at_ms: r.started_at_ms,
        client: r.client.clone(),
        id: r.id.clone(),
    }
}
fn group_filter(r: &TaskUsageRecord, q: &TaskUsageQuery) -> Option<TaskUsageQuery> {
    let mut f = q.clone();
    f.cursor = None;
    match q.group_by {
        TaskUsageGroupBy::Task => {
            f.task_id = Some(r.context.as_ref()?.task.id.clone());
        }
        TaskUsageGroupBy::Group => {
            let c = r.context.as_ref()?;
            f.task_id = Some(c.task.id.clone());
            f.group_id = Some(c.group.as_ref()?.id.clone());
        }
        TaskUsageGroupBy::Subtask => {
            let c = r.context.as_ref()?;
            f.task_id = Some(c.task.id.clone());
            f.subtask_id = Some(c.subtask.as_ref()?.id.clone());
        }
        TaskUsageGroupBy::Operation => {
            f.operation = Some(r.operation);
        }
        TaskUsageGroupBy::Model => {
            f.provider = Some(r.provider.clone()?);
            f.model = Some(r.model.clone()?);
        }
        TaskUsageGroupBy::Client => {
            f.client = Some(r.client.clone());
        }
    }
    Some(f)
}
fn report(
    mut records: Vec<TaskUsageRecord>,
    query: &TaskUsageQuery,
) -> Result<TaskUsageReport, UsageLedgerError> {
    if !query.validate() {
        return Err(invalid("invalid task usage query"));
    }
    let mut report = TaskUsageReport::default();
    let mut groups = BTreeMap::<String, TaskUsageGroup>::new();
    for r in &records {
        add(&mut report.totals, r);
        let (key, title) = group(r, query.group_by);
        let g = groups.entry(key.clone()).or_insert(TaskUsageGroup {
            key,
            title,
            totals: Default::default(),
            filter: group_filter(r, query),
        });
        add(&mut g.totals, r);
    }
    report.groups = groups.into_values().collect();
    records.sort_by(|a, b| {
        (b.started_at_ms, &b.client, &b.id).cmp(&(a.started_at_ms, &a.client, &a.id))
    });
    if let Some(c) = &query.cursor {
        records
            .retain(|r| (r.started_at_ms, &r.client, &r.id) < (c.started_at_ms, &c.client, &c.id));
    }
    if records.len() > query.limit as usize {
        records.truncate(query.limit as usize);
        report.next_cursor = records.last().map(cursor);
    }
    report.records = records;
    Ok(report)
}

#[cfg(feature = "sqlite")]
use crate::usage::SqliteUsageLedger;
#[cfg(feature = "sqlite")]
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

#[cfg(feature = "sqlite")]
pub(crate) fn ensure_schema(conn: &Connection) -> Result<(), UsageLedgerError> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS task_usage_calls (
        client TEXT NOT NULL, id TEXT NOT NULL, started_at_ms INTEGER NOT NULL,
        upstream_task_id TEXT, operation TEXT NOT NULL, payload TEXT NOT NULL,
        PRIMARY KEY(client,id));
        CREATE INDEX IF NOT EXISTS idx_task_usage_time ON task_usage_calls(client,started_at_ms);
        CREATE INDEX IF NOT EXISTS idx_task_usage_video ON task_usage_calls(upstream_task_id,operation);")
        .map_err(storage)
}
#[cfg(feature = "sqlite")]
fn decode(payload: String) -> Result<TaskUsageRecord, UsageLedgerError> {
    let r = serde_json::from_str(&payload).map_err(storage)?;
    validate(&r).map_err(storage)?;
    Ok(r)
}
#[cfg(feature = "sqlite")]
fn get(
    conn: &Connection,
    client: &str,
    id: &str,
) -> Result<Option<TaskUsageRecord>, UsageLedgerError> {
    conn.query_row(
        "SELECT payload FROM task_usage_calls WHERE client=?1 AND id=?2",
        params![client, id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(storage)?
    .map(decode)
    .transpose()
}
#[cfg(feature = "sqlite")]
impl SqliteUsageLedger {
    pub(crate) fn journal_write(
        &self,
        record: TaskUsageRecord,
        finishing: bool,
    ) -> Result<(), UsageLedgerError> {
        validate(&record)?;
        let mut conn = self.conn.lock().map_err(storage)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        if let Some(existing) = get(&tx, &record.client, &record.id)? {
            update(&existing, &record, finishing)?;
        } else {
            if finishing {
                return Err(invalid("cannot finish a missing call"));
            }
            let mut parents = Vec::new();
            for id in [
                record.related_call_id.as_deref(),
                record.context.as_ref().and_then(|c| c.retry_of.as_deref()),
            ]
            .into_iter()
            .flatten()
            {
                if let Some(r) = get(&tx, &record.client, id)? {
                    parents.push(r);
                }
            }
            validate_links(&record, &parents)?;
        }
        let operation = serde_json::to_value(record.operation).map_err(storage)?;
        tx.execute("INSERT INTO task_usage_calls (client,id,started_at_ms,upstream_task_id,operation,payload)
            VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(client,id) DO UPDATE SET upstream_task_id=excluded.upstream_task_id,payload=excluded.payload",
            params![record.client,record.id,record.started_at_ms as i64,record.upstream_task_id,operation.as_str(),serde_json::to_string(&record).map_err(storage)?]).map_err(storage)?;
        tx.commit().map_err(storage)
    }
    pub(crate) fn journal_get(
        &self,
        client: &str,
        id: &str,
    ) -> Result<Option<TaskUsageRecord>, UsageLedgerError> {
        let conn = self.conn.lock().map_err(storage)?;
        get(&conn, client, id)
    }
    pub(crate) fn journal_video(
        &self,
        client: Option<&str>,
        id: &str,
    ) -> Result<Option<TaskUsageRecord>, UsageLedgerError> {
        let conn = self.conn.lock().map_err(storage)?;
        conn.query_row("SELECT payload FROM task_usage_calls WHERE upstream_task_id=?1 AND operation='video' AND (?2 IS NULL OR client=?2)",params![id,client],|row|row.get::<_,String>(0))
            .optional().map_err(storage)?.map(decode).transpose()
    }
    pub(crate) fn journal_report(
        &self,
        q: &TaskUsageQuery,
    ) -> Result<TaskUsageReport, UsageLedgerError> {
        if !q.validate() {
            return Err(invalid("invalid task usage query"));
        }
        let conn = self.conn.lock().map_err(storage)?;
        let mut stmt=conn.prepare("SELECT payload FROM task_usage_calls WHERE (?1 IS NULL OR client=?1) AND (?2 IS NULL OR started_at_ms>=?2) AND (?3 IS NULL OR started_at_ms<?3)").map_err(storage)?;
        let a = q
            .started_after_ms
            .map(i64::try_from)
            .transpose()
            .map_err(storage)?;
        let b = q
            .started_before_ms
            .map(i64::try_from)
            .transpose()
            .map_err(storage)?;
        let rows = stmt
            .query_map(params![q.client, a, b], |row| row.get::<_, String>(0))
            .map_err(storage)?;
        let mut records = Vec::new();
        for row in rows {
            let r = decode(row.map_err(storage)?)?;
            if matches(&r, q) {
                records.push(r);
            }
        }
        report(records, q)
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use crate::usage::{UsageLedger, UsageQuery, UsageRecord, SCHEMA_VERSION};
    fn call(id: &str, client: &str, operation: TaskUsageOperation) -> TaskUsageRecord {
        TaskUsageRecord {
            id: id.into(),
            client: client.into(),
            source: TaskUsageSource::Gateway,
            context: Some(TaskUsageContext {
                task: UsageTaskRef {
                    id: "novel".into(),
                    title: "作品".into(),
                },
                group: Some(UsageTaskRef {
                    id: "part-1".into(),
                    title: "第一部".into(),
                }),
                subtask: Some(UsageTaskRef {
                    id: "chapter-1".into(),
                    title: "第一章".into(),
                }),
                operation,
                retry_of: None,
            }),
            operation,
            provider: Some("provider".into()),
            model: Some("model".into()),
            started_at_ms: 10,
            finished_at_ms: None,
            status: TaskUsageStatus::Running,
            tokens: None,
            cost: None,
            output_images: None,
            planned_video_seconds: None,
            upstream_task_id: None,
            upstream_status: None,
            related_call_id: None,
            error_code: None,
        }
    }
    #[tokio::test]
    async fn journal_migrates_preserves_usage_and_resumes_video_without_double_counting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        {
            let ledger = SqliteUsageLedger::open(&path).unwrap();
            ledger
                .record(UsageRecord {
                    record_id: "legacy".into(),
                    tenant_id: TenantId::new("local/default"),
                    account_id: "account".into(),
                    provider_id: ProviderId::new("provider"),
                    model_id: ModelId::new("model"),
                    input_tokens: 1,
                    currency: "XXX".into(),
                    occurred_at_ms: 1,
                    ..Default::default()
                })
                .await
                .unwrap();
        }
        // 原 v3 账本没有调用表；历史费用行必须保留。
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE task_usage_calls; PRAGMA user_version=3;")
            .unwrap();
        drop(conn);
        let ledger = SqliteUsageLedger::open(&path).unwrap();
        assert_eq!(ledger.query(&UsageQuery::default()).await.unwrap().len(), 1);
        let mut video = call("video-1", "yingmai", TaskUsageOperation::Video);
        video.planned_video_seconds = Some(5);
        ledger.start_task_usage(video.clone()).await.unwrap();
        ledger.start_task_usage(video.clone()).await.unwrap();
        video.status = TaskUsageStatus::Submitted;
        video.finished_at_ms = Some(12);
        video.upstream_task_id = Some("account.task-1".into());
        video.upstream_status = Some(VideoTaskStatus::Pending);
        ledger.finish_task_usage(video.clone()).await.unwrap();
        drop(ledger);
        let ledger = SqliteUsageLedger::open(&path).unwrap();
        assert_eq!(
            ledger
                .find_task_usage_video(Some("yingmai"), "account.task-1")
                .await
                .unwrap(),
            Some(video.clone())
        );
        assert!(ledger
            .find_task_usage_video(Some("momai"), "account.task-1")
            .await
            .unwrap()
            .is_none());
        video.status = TaskUsageStatus::Succeeded;
        video.upstream_status = Some(VideoTaskStatus::Succeeded);
        ledger.finish_task_usage(video.clone()).await.unwrap();
        ledger.finish_task_usage(video.clone()).await.unwrap();
        let mut query = call("query-1", "yingmai", TaskUsageOperation::Query);
        query.related_call_id = Some(video.id.clone());
        query.status = TaskUsageStatus::Succeeded;
        query.finished_at_ms = Some(12);
        ledger.start_task_usage(query).await.unwrap();
        let report = ledger
            .task_usage_report(&TaskUsageQuery::default())
            .await
            .unwrap();
        assert_eq!(
            (
                report.totals.records,
                report.totals.generation_calls,
                report.totals.planned_video_seconds
            ),
            (2, 1, 5)
        );
        assert_eq!(
            (
                report.totals.unknown_token_calls,
                report.totals.unknown_cost_calls
            ),
            (1, 1)
        );
        let mut altered = video;
        altered.context.as_mut().unwrap().task.id = "other".into();
        assert!(matches!(
            ledger.finish_task_usage(altered).await,
            Err(UsageLedgerError::Conflict { .. })
        ));
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        conn.execute(
            "UPDATE task_usage_calls SET payload='broken' WHERE id='query-1'",
            [],
        )
        .unwrap();
        assert!(matches!(
            ledger.task_usage_report(&TaskUsageQuery::default()).await,
            Err(UsageLedgerError::Storage { .. })
        ));
    }
    #[tokio::test]
    async fn journal_report_scopes_retries_and_aggregates_all_pages_and_currencies() {
        let dir = tempfile::tempdir().unwrap();
        let stores: Vec<Box<dyn UsageLedger>> = vec![
            Box::new(InMemoryUsageLedger::new()),
            Box::new(SqliteUsageLedger::open(dir.path().join("usage.sqlite3")).unwrap()),
        ];
        for ledger in stores {
            let mut failed = call("first", "momai", TaskUsageOperation::Text);
            failed.status = TaskUsageStatus::Failed;
            failed.finished_at_ms = Some(12);
            ledger.start_task_usage(failed).await.unwrap();
            let mut retry = call("retry", "momai", TaskUsageOperation::Text);
            retry.context.as_mut().unwrap().retry_of = Some("first".into());
            retry.status = TaskUsageStatus::Succeeded;
            retry.finished_at_ms = Some(13);
            retry.tokens = Some(TokenUsage {
                input_tokens: 7,
                output_tokens: 3,
                ..Default::default()
            });
            retry.cost = Some(TaskUsageCost {
                value: Cost {
                    currency: "CNY".into(),
                    amount_micros: 12,
                },
                kind: TaskUsageCostKind::Actual,
            });
            ledger.start_task_usage(retry).await.unwrap();
            let mut other = call("other", "momai", TaskUsageOperation::Text);
            other.context.as_mut().unwrap().task.id = "another-novel".into();
            other.status = TaskUsageStatus::Succeeded;
            other.finished_at_ms = Some(14);
            other.cost = Some(TaskUsageCost {
                value: Cost {
                    currency: "USD".into(),
                    amount_micros: 4,
                },
                kind: TaskUsageCostKind::Estimated,
            });
            ledger.start_task_usage(other).await.unwrap();
            let mut foreign = call("foreign", "yingmai", TaskUsageOperation::Video);
            foreign.context.as_mut().unwrap().retry_of = Some("first".into());
            assert!(ledger.start_task_usage(foreign.clone()).await.is_err());
            foreign.context.as_mut().unwrap().retry_of = None;
            ledger.start_task_usage(foreign).await.unwrap();
            let mut query = TaskUsageQuery {
                client: Some("momai".into()),
                limit: 1,
                group_by: TaskUsageGroupBy::Subtask,
                ..Default::default()
            };
            let first = ledger.task_usage_report(&query).await.unwrap();
            assert_eq!(first.records.len(), 1);
            assert_eq!(
                (
                    first.totals.records,
                    first.totals.generation_calls,
                    first.totals.failed,
                    first.totals.tokens.input_tokens
                ),
                (3, 3, 1, 7)
            );
            assert_eq!(
                (
                    first.totals.unknown_token_calls,
                    first.totals.unknown_cost_calls
                ),
                (2, 1)
            );
            assert_eq!(
                first.groups.len(),
                2,
                "same chapter id in different novels cannot merge"
            );
            assert_eq!(first.totals.currencies.len(), 2);
            query.cursor = first.next_cursor.clone();
            let next = ledger.task_usage_report(&query).await.unwrap();
            assert_eq!(first.totals, next.totals);
            assert_ne!(first.records[0].id, next.records[0].id);
            query.cursor = None;
            query.task_id = Some("novel".into());
            query.group_id = Some("part-1".into());
            query.operation = Some(TaskUsageOperation::Text);
            query.group_by = TaskUsageGroupBy::Task;
            let filtered = ledger.task_usage_report(&query).await.unwrap();
            assert_eq!(filtered.totals.records, 2);
            let drill = filtered.groups[0].filter.as_ref().unwrap();
            assert_eq!(drill.group_id, query.group_id);
            assert_eq!(
                ledger.task_usage_report(drill).await.unwrap().totals,
                filtered.totals
            );
            query.limit = 0;
            assert!(ledger.task_usage_report(&query).await.is_err());
            query.limit = 1;
            query.started_after_ms = Some(u64::MAX);
            assert!(ledger.task_usage_report(&query).await.is_err());
        }
    }
}
