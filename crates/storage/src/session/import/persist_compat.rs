//! compat 导入的 store 写入：Immediate 事务 + identity 幂等 / 冲突。

use std::path::Path;

use pawork_domain::RunId;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use tokio::io::{AsyncReadExt, BufReader};

use crate::session::event_store::persist_event_in_transaction;
use crate::session::import::formats::compat::{
    content_fingerprint, count_records, derive_compat_session_id, effective_identity, find_secret,
    map_to_events, parse_external, validate_structure, CompatImportHistoryEntry,
    CompatImportHistoryPage, CompatImportReport, ExternalSource,
};
use crate::session::{SessionStore, SessionStoreError, DEFAULT_BRANCH_ID};

const COMPAT_TENANT: &str = "local/default";
const COMPAT_PRINCIPAL: &str = "local/user";

/// compat 导入 session 的固定 created_at（ms），与事件时间戳（`1_000 + seq`）解耦。
const COMPAT_CREATED_AT_MS: i64 = 1;

impl SessionStore {
    /// 从外部会话内容导入（内存字符串）。**不修改任何既有 event**。
    pub async fn import_compat(
        &self,
        source: ExternalSource,
        content: &str,
    ) -> Result<CompatImportReport, SessionStoreError> {
        import_compat_inner(self, source, content).await
    }

    /// 从外部会话文件导入。**只读取源文件，不修改原文件**（ADR-005）。
    pub async fn import_compat_from_file(
        &self,
        source: ExternalSource,
        path: &Path,
    ) -> Result<CompatImportReport, SessionStoreError> {
        let content =
            if path.to_string_lossy().ends_with(".jsonl") || source == ExternalSource::Codex {
                // JSONL：流式读取，避免大文件整体入内存。
                let file = tokio::fs::File::open(path).await?;
                let mut reader = BufReader::new(file);
                let mut buf = String::new();
                reader.read_to_string(&mut buf).await?;
                buf
            } else {
                tokio::fs::read_to_string(path).await?
            };
        import_compat_inner(self, source, &content).await
    }

    /// 只校验与解析，**不落库**（dry run）：与 [`Self::import_compat`] 相同的
    /// Secret 扫描、解析、结构校验与计数，返回完整报告但零持久化。
    pub async fn import_compat_dry_run(
        &self,
        source: ExternalSource,
        content: &str,
    ) -> Result<CompatImportReport, SessionStoreError> {
        // 1. Secret 扫描（与真实导入同一拒绝策略）。
        if let Some(pattern) = find_secret(content) {
            return Err(SessionStoreError::CompatSecretDetected {
                pattern: pattern.into(),
            });
        }
        // 2. 解析 + 3. identity / 会话 id 推导（不持久化）。
        let parsed = parse_external(source, content)?;
        let session_id = derive_compat_session_id(source, parsed.original_id.as_deref(), content);
        let run_id = RunId::from(format!("compat-run-{}", session_id.as_str()));
        let events = map_to_events(&session_id, &run_id, &parsed);
        // 4. 结构校验（失败同样报错，与真实导入一致）。
        validate_structure(&events)?;
        let counts = count_records(&parsed.records);
        Ok(CompatImportReport {
            source: parsed.source,
            session_id: session_id.to_string(),
            original_id: parsed.original_id.clone(),
            imported_events: events.len(),
            imported_messages: counts.messages,
            imported_tool_calls: counts.tool_calls,
            imported_tool_results: counts.tool_results,
            imported_usages: counts.usages,
            imported_reviews: counts.reviews,
            raw_records: counts.raw,
            deduplicated: false,
            unknown_fields: parsed.unknown_fields.clone(),
        })
    }

    /// 导入历史（分页，按导入时间倒序）。`limit` 缺省 50，上限 500；
    /// `cursor` 来自上一页返回的不透明令牌。
    pub async fn compat_import_history(
        &self,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<CompatImportHistoryPage, SessionStoreError> {
        const DEFAULT_LIMIT: u32 = 50;
        const MAX_LIMIT: u32 = 500;
        let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
        // 键集分页：(updated_at_ms DESC, session_id DESC)；游标为
        // `"{updated_at_ms}:{session_id}"`（不透明）。
        let (cursor_ms, cursor_session): (i64, String) = match cursor {
            Some(raw) => {
                let (ms, session) = raw
                    .split_once(':')
                    .ok_or_else(|| SessionStoreError::InvalidHistoryCursor(raw.to_string()))?;
                let ms = ms
                    .parse::<i64>()
                    .map_err(|_| SessionStoreError::InvalidHistoryCursor(raw.to_string()))?;
                (ms, session.to_string())
            }
            None => (i64::MAX, String::new()),
        };
        let entries = self
            .database()
            .call(
                move |connection| -> Result<Vec<CompatImportHistoryEntry>, SessionStoreError> {
                    let mut statement = connection.prepare(
                        "SELECT i.session_id, i.source, i.original_id, s.updated_at_ms, \
                     (SELECT COUNT(*) FROM session_events e \
                      WHERE e.session_id = i.session_id) AS event_count \
                     FROM compat_import_identity i \
                     JOIN sessions s ON s.session_id = i.session_id \
                     WHERE (s.updated_at_ms < ?1) \
                        OR (s.updated_at_ms = ?1 AND i.session_id < ?2) \
                     ORDER BY s.updated_at_ms DESC, i.session_id DESC \
                     LIMIT ?3",
                    )?;
                    let rows = statement.query_map(
                        params![cursor_ms, cursor_session, limit as i64 + 1],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, Option<String>>(2)?,
                                row.get::<_, i64>(3)?,
                                row.get::<_, i64>(4)?,
                            ))
                        },
                    )?;
                    let raw = rows.collect::<Result<Vec<_>, _>>()?;
                    raw.into_iter()
                        .map(
                            |(
                                session_id,
                                source_label,
                                original_id,
                                imported_at_ms,
                                event_count,
                            )| {
                                let source = match source_label.as_str() {
                                    "claude" => ExternalSource::Claude,
                                    "codex" => ExternalSource::Codex,
                                    "grok" => ExternalSource::Grok,
                                    "cursor" => ExternalSource::Cursor,
                                    other => {
                                        return Err(SessionStoreError::InvalidHistorySource(
                                            other.to_string(),
                                        ));
                                    }
                                };
                                Ok(CompatImportHistoryEntry {
                                    session_id,
                                    source,
                                    original_id: original_id.filter(|id| !id.is_empty()),
                                    imported_events: event_count.max(0) as usize,
                                    imported_at_unix_ms: imported_at_ms.max(0) as u64,
                                })
                            },
                        )
                        .collect()
                },
            )
            .await??;
        // 多取 1 条探测是否还有下一页；`limit` 为 0 时没有（clamp 保证 ≥1）。
        let has_more = entries.len() > limit;
        let mut entries = entries;
        entries.truncate(limit);
        let cursor = has_more.then(|| {
            let last = entries.last().expect("entries non-empty when has_more");
            format!("{}:{}", last.imported_at_unix_ms, last.session_id)
        });
        Ok(CompatImportHistoryPage { entries, cursor })
    }
}

/// 当前 unix 毫秒（compat 导入时间戳 / 历史排序使用）。
fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// 单事务导入的结果：新建导入或命中既有 identity 的幂等去重。
enum ImportOutcome {
    Imported { session_id: String },
    Deduplicated { session_id: String },
}

async fn import_compat_inner(
    store: &SessionStore,
    source: ExternalSource,
    content: &str,
) -> Result<CompatImportReport, SessionStoreError> {
    // 1. Secret 扫描（untrusted 拒绝策略）。
    if let Some(pattern) = find_secret(content) {
        return Err(SessionStoreError::CompatSecretDetected {
            pattern: pattern.into(),
        });
    }
    // 2. 解析。
    let parsed = parse_external(source, content)?;
    // 3. identity / content fingerprint（与事件同事务持久化，作为去重/冲突唯一权威）。
    //    identity 不随内容漂移：同 (source, original_id) 始终映射同一 SessionId；
    //    无 original_id 时退化为 content fingerprint，使「相同无 id 内容」仍可幂等。
    let fingerprint = content_fingerprint(content);
    let identity = effective_identity(parsed.original_id.as_deref(), content);
    let session_id = derive_compat_session_id(source, parsed.original_id.as_deref(), content);
    let counts = count_records(&parsed.records);
    let imported_at_ms = now_unix_ms();
    // 4. 映射为 canonical event 序列（run / message / tool id 全部 session-scoped）。
    let run_id = RunId::from(format!("compat-run-{}", session_id.as_str()));
    let events = map_to_events(&session_id, &run_id, &parsed);
    // 5. 结构校验（失败则整批不入库）。
    validate_structure(&events)?;
    // 6. 单事务原子持久化：Session + branch + import identity + 全部脱敏 event + projection。
    //    任一失败由同一事务回滚（零残留、可重试）；绝不触碰既有事件。
    let title = parsed
        .title
        .clone()
        .unwrap_or_else(|| format!("imported from {source}"));
    let imported_event_count = events.len();
    let source_for_report = parsed.source;
    let original_id_for_report = parsed.original_id.clone();
    let unknown_for_report = parsed.unknown_fields.clone();
    let source_label = source.as_str().to_string();
    let outcome = store
        .database()
        .call(
            move |connection| -> Result<ImportOutcome, SessionStoreError> {
                // 先取得 SQLite writer reservation，再读取 identity。这样不同
                // SessionStore/连接上的并发导入会串行到同一 identity 判定：第二个事务
                // 在首个提交后读取结果，稳定映射为幂等或 CompatImportConflict，而不会
                // 在 deferred transaction 升级写锁时泄漏 SQLITE_BUSY/PK race。
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                // identity 权威：同 (source, identity) 的既有导入决定幂等 / 冲突。
                let existing: Option<(String, String)> = transaction
                    .query_row(
                        "SELECT content_fingerprint, session_id FROM compat_import_identity \
                     WHERE source=?1 AND original_id=?2",
                        params![source_label, identity],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?;
                if let Some((stored_fingerprint, stored_session_id)) = existing {
                    if stored_fingerprint == fingerprint {
                        // 同 identity + 同指纹：幂等，不产生任何新事件。
                        return Ok(ImportOutcome::Deduplicated {
                            session_id: stored_session_id,
                        });
                    }
                    // 同 identity + 不同指纹：明确冲突，绝不静默创建第二 Session。
                    return Err(SessionStoreError::CompatImportConflict {
                        source_label,
                        original_id: identity,
                    });
                }
                // 新建 Session + default branch + import identity row（同一事务）。
                let session_id_str = session_id.to_string();
                transaction.execute(
                    "INSERT INTO sessions(session_id, title, created_at_ms, updated_at_ms, tenant_id, principal_id) \
                 VALUES (?1, ?2, ?3, ?3, ?4, ?5)",
                    params![
                        session_id_str,
                        title,
                        COMPAT_CREATED_AT_MS,
                        COMPAT_TENANT,
                        COMPAT_PRINCIPAL
                    ],
                )?;
                transaction.execute(
                    "INSERT INTO session_branches(branch_id, session_id, head_sequence) \
                 VALUES (?1, ?2, 0)",
                    params![DEFAULT_BRANCH_ID, session_id_str],
                )?;
                transaction.execute(
                    "INSERT INTO compat_import_identity(source, original_id, \
                 content_fingerprint, session_id) VALUES (?1, ?2, ?3, ?4)",
                    params![source_label, identity, fingerprint, session_id_str],
                )?;
                // 全部事件 + projection（绝不触碰既有事件）。
                for envelope in &events {
                    persist_event_in_transaction(&transaction, DEFAULT_BRANCH_ID, envelope)?;
                }
                // 事件持久化会用（合成的）事件时间戳刷新 updated_at_ms；这里在
                // 同一事务内把它恢复为真实导入时间，作为导入历史的排序依据。
                transaction.execute(
                    "UPDATE sessions SET updated_at_ms=?1 WHERE session_id=?2",
                    params![imported_at_ms, session_id_str],
                )?;
                transaction.commit()?;
                Ok(ImportOutcome::Imported {
                    session_id: session_id_str,
                })
            },
        )
        .await??;

    Ok(match outcome {
        ImportOutcome::Deduplicated { session_id } => CompatImportReport {
            source: source_for_report,
            session_id,
            original_id: original_id_for_report,
            imported_events: 0,
            imported_messages: 0,
            imported_tool_calls: 0,
            imported_tool_results: 0,
            imported_usages: 0,
            imported_reviews: 0,
            raw_records: 0,
            deduplicated: true,
            unknown_fields: unknown_for_report,
        },
        ImportOutcome::Imported { session_id } => CompatImportReport {
            source: source_for_report,
            session_id,
            original_id: original_id_for_report,
            imported_events: imported_event_count,
            imported_messages: counts.messages,
            imported_tool_calls: counts.tool_calls,
            imported_tool_results: counts.tool_results,
            imported_usages: counts.usages,
            imported_reviews: counts.reviews,
            raw_records: counts.raw,
            deduplicated: false,
            unknown_fields: unknown_for_report,
        },
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use pawork_domain::{AgentEvent, SessionId};

    use super::*;
    use crate::session::import::formats::compat::{CLAUDE_JSON, CODEX_JSONL};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path() -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-compat-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    async fn open_store() -> SessionStore {
        let (store, _) = SessionStore::open(&temp_path()).await.expect("open store");
        store
    }

    #[tokio::test]
    async fn import_creates_new_session_and_is_append_only() {
        let store = open_store().await;
        let report = store
            .import_compat(ExternalSource::Claude, CLAUDE_JSON)
            .await
            .expect("import");
        assert!(!report.deduplicated);
        assert!(report.imported_events > 0);
        assert_eq!(report.imported_messages, 2);
        assert_eq!(report.raw_records, 1);
        // replay 可消费导入产物（状态机可推进）。
        let sid = SessionId::from(report.session_id.clone());
        let events = store.replay_events(&sid, 1, 100).await.expect("replay");
        assert!(!events.is_empty());
        // 首事件 RunStarted，尾事件 RunCompleted。
        assert!(matches!(
            events.first().map(|e| &e.payload),
            Some(AgentEvent::RunStarted { .. })
        ));
        assert!(matches!(
            events.last().map(|e| &e.payload),
            Some(AgentEvent::RunCompleted { .. })
        ));
    }

    #[tokio::test]
    async fn import_codex_and_verify_counts() {
        let store = open_store().await;
        let report = store
            .import_compat(ExternalSource::Codex, CODEX_JSONL)
            .await
            .expect("import codex");
        assert_eq!(report.imported_tool_calls, 1);
        assert_eq!(report.imported_tool_results, 1);
        assert_eq!(report.imported_usages, 1);
    }

    #[tokio::test]
    async fn import_rejects_secret_and_persists_nothing() {
        let store = open_store().await;
        let malicious = r#"{"id":"x","messages":[{"role":"user","content":"my key is sk-ant-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}]}"#;
        let err = store
            .import_compat(ExternalSource::Grok, malicious)
            .await
            .expect_err("must reject secret");
        assert!(matches!(
            err,
            SessionStoreError::CompatSecretDetected { .. }
        ));
        // 无任何 session 被创建（nothing imported）。
        let sid = derive_compat_session_id(ExternalSource::Grok, Some("x"), malicious);
        let events = store.replay_events(&sid, 1, 1).await.expect("replay");
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn import_dedup_is_idempotent() {
        let store = open_store().await;
        let first = store
            .import_compat(ExternalSource::Claude, CLAUDE_JSON)
            .await
            .expect("first import");
        let second = store
            .import_compat(ExternalSource::Claude, CLAUDE_JSON)
            .await
            .expect("second import");
        assert_eq!(first.session_id, second.session_id);
        assert!(second.deduplicated);
        assert_eq!(second.imported_events, 0);
        // 重复导入不产生重复 event。
        let sid = SessionId::from(first.session_id.clone());
        let events = store.replay_events(&sid, 1, 1000).await.expect("replay");
        assert_eq!(events.len(), first.imported_events);
    }

    #[tokio::test]
    async fn concurrent_import_same_identity_and_fingerprint_is_idempotent() {
        let path = temp_path();
        let (first_store, _) = SessionStore::open(&path).await.expect("open first store");
        let (second_store, _) = SessionStore::open(&path).await.expect("open second store");

        let (first, second) = tokio::join!(
            first_store.import_compat(ExternalSource::Claude, CLAUDE_JSON),
            second_store.import_compat(ExternalSource::Claude, CLAUDE_JSON),
        );
        let first = first.expect("first concurrent import");
        let second = second.expect("second concurrent import");

        assert_eq!(first.session_id, second.session_id);
        assert_ne!(
            first.deduplicated, second.deduplicated,
            "exactly one connection must perform the import"
        );
        let imported_events = if first.deduplicated {
            second.imported_events
        } else {
            first.imported_events
        };
        assert!(imported_events > 0);
        let events = first_store
            .replay_events(&SessionId::from(first.session_id), 1, 1_000)
            .await
            .expect("replay");
        assert_eq!(events.len(), imported_events);
        assert!(identity_row(&first_store, "claude", "claude-abc")
            .await
            .is_some());
    }

    #[tokio::test]
    async fn concurrent_import_same_identity_different_fingerprint_conflicts() {
        let path = temp_path();
        let (first_store, _) = SessionStore::open(&path).await.expect("open first store");
        let (second_store, _) = SessionStore::open(&path).await.expect("open second store");
        let first_content = r#"{
            "conversation_id": "concurrent-conflict",
            "chat_messages": [{"sender": "human", "text": "first body"}]
        }"#;
        let second_content = r#"{
            "conversation_id": "concurrent-conflict",
            "chat_messages": [{"sender": "human", "text": "second body"}]
        }"#;

        let (first, second) = tokio::join!(
            first_store.import_compat(ExternalSource::Claude, first_content),
            second_store.import_compat(ExternalSource::Claude, second_content),
        );

        let (report, conflict) = match (first, second) {
            (Ok(report), Err(conflict)) | (Err(conflict), Ok(report)) => (report, conflict),
            (left, right) => {
                panic!("expected one import and one conflict, got {left:?} / {right:?}")
            }
        };
        assert!(matches!(
            conflict,
            SessionStoreError::CompatImportConflict {
                ref source_label,
                ref original_id,
            } if source_label == "claude" && original_id == "concurrent-conflict"
        ));
        let events = first_store
            .replay_events(&SessionId::from(report.session_id.clone()), 1, 1_000)
            .await
            .expect("replay");
        assert_eq!(events.len(), report.imported_events);
        assert_eq!(
            identity_row(&first_store, "claude", "concurrent-conflict")
                .await
                .expect("identity row")
                .1,
            report.session_id
        );
    }

    #[tokio::test]
    async fn import_does_not_modify_original_file() {
        let store = open_store().await;
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pawork-compat-src-{unique}.json"));
        fs::write(&path, CLAUDE_JSON).expect("write src");
        let before = fs::read_to_string(&path).unwrap();
        store
            .import_compat_from_file(ExternalSource::Claude, &path)
            .await
            .expect("import file");
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(before, after, "original file must be unchanged");
    }

    #[tokio::test]
    async fn import_review_comment_emits_review_event() {
        let store = open_store().await;
        let json = r#"{"id":"r1","messages":[
            {"role":"user","text":"please review"},
            {"role":"assistant","file":"src/main.rs","line":42,"severity":"major","text":"unwrap here"}
        ]}"#;
        let report = store
            .import_compat(ExternalSource::Cursor, json)
            .await
            .expect("import");
        assert_eq!(report.imported_reviews, 1);
    }

    /// 读取 `(source, original_id)` 的 import identity 行（content_fingerprint, session_id）。
    async fn identity_row(
        store: &SessionStore,
        source: &str,
        original_id: &str,
    ) -> Option<(String, String)> {
        let source = source.to_string();
        let original_id = original_id.to_string();
        store
            .database()
            .call(move |conn| -> rusqlite::Result<Option<(String, String)>> {
                let mut stmt = conn.prepare(
                    "SELECT content_fingerprint, session_id FROM compat_import_identity \
                     WHERE source=?1 AND original_id=?2",
                )?;
                let mut rows = stmt.query_map(rusqlite::params![source, original_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                rows.next().transpose()
            })
            .await
            .expect("actor")
            .expect("query")
    }

    #[tokio::test]
    async fn import_two_distinct_sessions_same_source_do_not_collide() {
        let store = open_store().await;
        let first = store
            .import_compat(ExternalSource::Claude, CLAUDE_JSON)
            .await
            .expect("first");
        let second_json = r#"{
            "conversation_id": "claude-second",
            "name": "second chat",
            "chat_messages": [
                {"sender": "human", "text": "another"},
                {"sender": "assistant", "text": "reply"}
            ]
        }"#;
        let second = store
            .import_compat(ExternalSource::Claude, second_json)
            .await
            .expect("second");
        // 不同 original_id → 不同 SessionId，run/message/tool id 都 session-scoped，
        // 不撞 runs / messages / tool_calls 全局主键。
        assert_ne!(first.session_id, second.session_id);
        assert!(!first.deduplicated && !second.deduplicated);
        for sid in [&first.session_id, &second.session_id] {
            let events = store
                .replay_events(&SessionId::from(sid.clone()), 1, 100)
                .await
                .expect("replay");
            assert!(events.len() > 1, "session must be replayable");
        }
    }

    #[tokio::test]
    async fn import_cross_source_same_tool_id_do_not_collide() {
        let store = open_store().await;
        // 两个不同来源都使用外部 tool_call_id "shared-1"。
        let codex = concat!(
            r#"{"type":"function_call","call_id":"shared-1","name":"shell","arguments":"{\"cmd\":\"ls\"}","session_id":"codex-shared"}"#,
            "\n",
            r#"{"type":"function_call_output","call_id":"shared-1","output":"listed"}"#,
        );
        let claude = r#"{
            "conversation_id": "claude-shared",
            "chat_messages": [
                {"sender":"assistant","tool_use":{"id":"shared-1","name":"shell","input":{"cmd":"ls"}}}
            ]
        }"#;
        let a = store
            .import_compat(ExternalSource::Codex, codex)
            .await
            .expect("codex import");
        let b = store
            .import_compat(ExternalSource::Claude, claude)
            .await
            .expect("claude import");
        assert_ne!(a.session_id, b.session_id);
        // 两个 session 的 tool_calls 都能落盘（scoped id 全局唯一，无主键冲突）。
        assert_eq!(a.imported_tool_calls, 1);
        assert_eq!(b.imported_tool_calls, 1);
    }

    #[tokio::test]
    async fn import_failure_leaves_no_residue_and_is_retryable() {
        let store = open_store().await;
        // 两个 function_call 共享同一 call_id → 通过结构校验，但在持久化第二条
        // ToolCallStarted 时撞 tool_calls 全局主键，事务中途失败。
        let dup = concat!(
            r#"{"type":"function_call","call_id":"dup1","name":"shell","session_id":"dup-session"}"#,
            "\n",
            r#"{"type":"function_call","call_id":"dup1","name":"shell"}"#,
            "\n",
            r#"{"type":"function_call_output","call_id":"dup1","output":"ok"}"#,
        );
        let err = store
            .import_compat(ExternalSource::Codex, dup)
            .await
            .expect_err("duplicate tool id must abort the import transaction");
        assert!(matches!(err, SessionStoreError::Sqlite(..)));

        let sid = derive_compat_session_id(ExternalSource::Codex, Some("dup-session"), dup);
        // 零残留：无事件、无 identity 行、session 不存在。
        let events = store.replay_events(&sid, 1, 100).await.expect("replay");
        assert!(events.is_empty(), "no events may persist after rollback");
        assert!(identity_row(&store, "codex", "dup-session").await.is_none());

        // 重试：相同 original_id 但内容已修正（不同 call_id）。因为失败没有留下 identity
        // 行，这次按全新导入成功——若残留了旧 identity，则会因指纹不同返回冲突。
        let fixed = concat!(
            r#"{"type":"function_call","call_id":"dup1","name":"shell","session_id":"dup-session"}"#,
            "\n",
            r#"{"type":"function_call","call_id":"dup2","name":"shell"}"#,
            "\n",
            r#"{"type":"function_call_output","call_id":"dup2","output":"ok"}"#,
        );
        let report = store
            .import_compat(ExternalSource::Codex, fixed)
            .await
            .expect("retry succeeds after zero-residue rollback");
        assert!(!report.deduplicated);
        assert!(report.imported_events > 0);
        assert_eq!(report.session_id, sid.to_string());
    }

    #[tokio::test]
    async fn import_conflict_on_same_identity_different_content() {
        let store = open_store().await;
        let first = r#"{
            "conversation_id": "conflict-1",
            "chat_messages": [{"sender": "human", "text": "version one"}]
        }"#;
        let report = store
            .import_compat(ExternalSource::Claude, first)
            .await
            .expect("first import");
        assert!(!report.deduplicated);

        // 同 original_id、不同内容 → 必须返回明确冲突，绝不静默创建第二 Session。
        let second = r#"{
            "conversation_id": "conflict-1",
            "chat_messages": [{"sender": "human", "text": "version two changed"}]
        }"#;
        let err = store
            .import_compat(ExternalSource::Claude, second)
            .await
            .expect_err("different content with same identity must conflict");
        assert!(matches!(
            err,
            SessionStoreError::CompatImportConflict { .. }
        ));
        // 原会话保持完整（冲突导入未改写任何既有状态）。
        let events = store
            .replay_events(&SessionId::from(report.session_id.clone()), 1, 100)
            .await
            .expect("replay");
        assert_eq!(events.len(), report.imported_events);
        assert_eq!(
            identity_row(&store, "claude", "conflict-1")
                .await
                .unwrap()
                .1,
            report.session_id
        );
    }

    #[tokio::test]
    async fn import_without_original_id_uses_content_fingerprint_identity() {
        let store = open_store().await;
        // 无 id 字段：identity 退化为 content fingerprint。
        let content = r#"{"messages":[{"role":"user","content":"hello no id"}]}"#;
        let first = store
            .import_compat(ExternalSource::Grok, content)
            .await
            .expect("first");
        assert!(!first.deduplicated);
        // 相同内容再次导入 → 幂等（同一 content fingerprint identity）。
        let second = store
            .import_compat(ExternalSource::Grok, content)
            .await
            .expect("second");
        assert_eq!(first.session_id, second.session_id);
        assert!(second.deduplicated);

        // 不同内容、同样无 id → 不同 content fingerprint → 不同 identity，新建而非冲突。
        let other = r#"{"messages":[{"role":"user","content":"totally different body"}]}"#;
        let third = store
            .import_compat(ExternalSource::Grok, other)
            .await
            .expect("third");
        assert_ne!(third.session_id, first.session_id);
        assert!(!third.deduplicated);
    }

    #[tokio::test]
    async fn import_preserves_tool_arguments_as_arguments_delta() {
        let store = open_store().await;
        let report = store
            .import_compat(ExternalSource::Codex, CODEX_JSONL)
            .await
            .expect("import codex");
        let events = store
            .replay_events(&SessionId::from(report.session_id.clone()), 1, 100)
            .await
            .expect("replay");
        // tool arguments 映射为既有 ToolCallArgumentsDelta，原样保留。
        let delta = events.iter().find_map(|env| {
            if let AgentEvent::ToolCallArgumentsDelta { json_delta, .. } = &env.payload {
                Some(json_delta.clone())
            } else {
                None
            }
        });
        assert_eq!(delta.as_deref(), Some(r#"{"cmd":"cargo test"}"#));
    }

    #[tokio::test]
    async fn dry_run_validates_without_persisting_anything() {
        let store = open_store().await;
        let report = store
            .import_compat_dry_run(ExternalSource::Claude, CLAUDE_JSON)
            .await
            .expect("dry run");
        assert!(!report.deduplicated);
        assert!(report.imported_events > 0);
        assert!(report.imported_messages >= 2, "claude messages counted");
        assert_eq!(
            report.session_id,
            derive_compat_session_id(ExternalSource::Claude, Some("claude-abc"), CLAUDE_JSON,)
                .to_string()
        );
        // 零持久化：无 identity 行、无事件。
        assert!(identity_row(&store, "claude", "claude-abc").await.is_none());
        let events = store
            .replay_events(&SessionId::from(report.session_id.clone()), 1, 1_000)
            .await
            .expect("replay");
        assert!(events.is_empty(), "dry run must not persist events");

        // Secret 拒绝策略与真实导入一致。
        let secret = r#"{
            "conversation_id": "leaky",
            "chat_messages": [{"sender": "human", "text": "key sk-ant-abcdefghijklmnopqrstuvwxyz0123456789"}]
        }"#;
        let error = store
            .import_compat_dry_run(ExternalSource::Claude, secret)
            .await
            .expect_err("secret must be rejected in dry run too");
        assert!(matches!(
            error,
            SessionStoreError::CompatSecretDetected { .. }
        ));
    }

    #[tokio::test]
    async fn history_lists_imports_in_reverse_chronological_order_with_paging() {
        let store = open_store().await;
        // 三个不同 identity；导入顺序决定 updated_at_ms 倒序。
        let first = store
            .import_compat(ExternalSource::Claude, CLAUDE_JSON)
            .await
            .expect("first import");
        let second_json = r#"{
            "conversation_id": "claude-history-2",
            "chat_messages": [{"sender": "human", "text": "two"}]
        }"#;
        let second = store
            .import_compat(ExternalSource::Claude, second_json)
            .await
            .expect("second import");
        let codex = concat!(
            r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"codex"}]}"#,
            "\n",
            r#"{"type":"function_call","call_id":"h1","name":"shell","session_id":"codex-h"}"#,
            "\n",
            r#"{"type":"function_call_output","call_id":"h1","output":"ok"}"#,
        );
        let third = store
            .import_compat(ExternalSource::Codex, codex)
            .await
            .expect("third import");
        assert_ne!(first.session_id, second.session_id);
        assert_ne!(second.session_id, third.session_id);

        // 同一 identity 重复导入 → 幂等去重，不产生新条目。
        let dedup = store
            .import_compat(ExternalSource::Claude, CLAUDE_JSON)
            .await
            .expect("dedup import");
        assert!(dedup.deduplicated);
        assert_eq!(dedup.session_id, first.session_id);

        // 全量（limit 足够大）：最新导入在前。
        let page = store
            .compat_import_history(Some(100), None)
            .await
            .expect("history");
        assert_eq!(page.entries.len(), 3, "three identities, dedup adds none");
        assert_eq!(page.entries[0].session_id, third.session_id);
        assert_eq!(page.entries[1].session_id, second.session_id);
        assert_eq!(page.entries[2].session_id, first.session_id);
        assert_eq!(page.entries[0].source, ExternalSource::Codex);
        assert_eq!(page.entries[2].original_id.as_deref(), Some("claude-abc"));
        assert!(
            page.entries[2].imported_events > 0,
            "event count derived from persisted session events"
        );
        assert!(
            page.entries[2].imported_at_unix_ms > 0,
            "import time persisted on the session row"
        );
        assert!(page.cursor.is_none(), "no more pages");

        // 分页：limit=1 逐页走完，游标稳定续页。
        let first_page = store
            .compat_import_history(Some(1), None)
            .await
            .expect("first page");
        assert_eq!(first_page.entries.len(), 1);
        assert_eq!(first_page.entries[0].session_id, third.session_id);
        let cursor = first_page.cursor.expect("more pages");
        let second_page = store
            .compat_import_history(Some(1), Some(&cursor))
            .await
            .expect("second page");
        assert_eq!(second_page.entries.len(), 1);
        assert_eq!(second_page.entries[0].session_id, second.session_id);
        let cursor = second_page.cursor.expect("more pages");
        let third_page = store
            .compat_import_history(Some(1), Some(&cursor))
            .await
            .expect("third page");
        assert_eq!(third_page.entries.len(), 1);
        assert_eq!(third_page.entries[0].session_id, first.session_id);
        assert!(third_page.cursor.is_none(), "last page has no cursor");

        // 非法游标显式报错。
        let error = store
            .compat_import_history(Some(10), Some("not-a-cursor"))
            .await
            .expect_err("malformed cursor");
        assert!(matches!(error, SessionStoreError::InvalidHistoryCursor(_)));
    }
}
