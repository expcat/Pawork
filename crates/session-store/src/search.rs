//! Session 搜索与标签（P5-4）。
//!
//! 目的：在大量 session 中按标题 / 标签 / 内容快速定位。
//!
//! 设计要点：
//! - 标签：`session_tags(session_id, tag)` 唯一、不区分大小写（归一为小写存储），
//!   附 `(session_id, tag)` 主键与 `tag` 索引。
//! - 搜索：命中标题、标签或内容（messages 抽取文本），按 `updated_at_ms` 倒序。
//!   `sessions` 主键为 `session_id TEXT`（无单调整数 rowid），FTS5 外部内容表
//!   （`content_rowid`）难以正确映射且易产生同步错误，故标题/标签采用确定性 `LIKE`，
//!   内容反序列化后只匹配 `Text` part；迁移到整数 rowid 后可再引入 FTS5。

use agent_domain::{ContentPart, Message, SessionId};
use rusqlite::{params, Connection};

use crate::{SessionStore, SessionStoreError};

/// 搜索命中的来源维度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMatch {
    Title,
    Tag,
    Content,
}

/// 一次搜索命中。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSearchHit {
    pub session_id: String,
    pub title: String,
    pub matched_on: SearchMatch,
    pub snippet: Option<String>,
}

impl SessionStore {
    /// 为 session 添加标签（去重、小写归一）。重复标签静默忽略。
    pub async fn add_tags(
        &self,
        session_id: &SessionId,
        tags: &[&str],
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        let normalized: Vec<String> = tags
            .iter()
            .map(|tag| tag.trim().to_ascii_lowercase())
            .filter(|tag| !tag.is_empty())
            .collect();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                for tag in normalized {
                    connection.execute(
                        "INSERT OR IGNORE INTO session_tags(session_id, tag) VALUES (?1, ?2)",
                        params![session_id, tag],
                    )?;
                }
                Ok(())
            })
            .await??;
        Ok(())
    }

    /// 用给定标签集合完全替换 session 的标签。
    pub async fn set_tags(
        &self,
        session_id: &SessionId,
        tags: &[&str],
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        let normalized: Vec<String> = tags
            .iter()
            .map(|tag| tag.trim().to_ascii_lowercase())
            .filter(|tag| !tag.is_empty())
            .collect();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                let transaction = connection.transaction()?;
                transaction.execute(
                    "DELETE FROM session_tags WHERE session_id=?1",
                    [&session_id],
                )?;
                for tag in normalized {
                    transaction.execute(
                        "INSERT OR IGNORE INTO session_tags(session_id, tag) VALUES (?1, ?2)",
                        params![session_id, tag],
                    )?;
                }
                transaction.commit()?;
                Ok(())
            })
            .await??;
        Ok(())
    }

    /// 删除 session 的单个标签（不存在则静默忽略）。
    pub async fn remove_tag(
        &self,
        session_id: &SessionId,
        tag: &str,
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        let tag = tag.trim().to_ascii_lowercase();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                connection.execute(
                    "DELETE FROM session_tags WHERE session_id=?1 AND tag=?2",
                    params![session_id, tag],
                )?;
                Ok(())
            })
            .await??;
        Ok(())
    }

    /// 列出 session 的全部标签（按字母序）。
    pub async fn list_tags(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<String>, SessionStoreError> {
        let session_id = session_id.to_string();
        let tags = self
            .database()
            .call(move |connection| -> rusqlite::Result<Vec<String>> {
                let mut statement = connection
                    .prepare("SELECT tag FROM session_tags WHERE session_id=?1 ORDER BY tag")?;
                let rows = statement
                    .query_map([&session_id], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await??;
        Ok(tags)
    }

    /// 按关键字搜索 session（标题 / 标签 / 内容），按 `updated_at_ms` 倒序。
    pub async fn search_sessions(
        &self,
        query: &str,
    ) -> Result<Vec<SessionSearchHit>, SessionStoreError> {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let query_for_call = query.clone();
        let hits = self
            .database()
            .call(
                move |connection| -> Result<Vec<SessionSearchHit>, SessionStoreError> {
                    search_with_like(connection, &query_for_call)
                },
            )
            .await??;
        Ok(hits)
    }
}

/// 确定性 LIKE 搜索。优先级：标题 > 标签 > 内容；同 session 去重保留最高优先级命中。
fn search_with_like(
    connection: &Connection,
    query: &str,
) -> Result<Vec<SessionSearchHit>, SessionStoreError> {
    let like = format!("%{query}%");
    let mut hits: Vec<SessionSearchHit> = Vec::new();

    {
        let mut statement = connection.prepare(
            "SELECT session_id, title FROM sessions \
             WHERE title LIKE ?1 ORDER BY updated_at_ms DESC, session_id",
        )?;
        let rows = statement
            .query_map([&like], |row| {
                Ok(SessionSearchHit {
                    session_id: row.get(0)?,
                    title: row.get(1)?,
                    matched_on: SearchMatch::Title,
                    snippet: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        hits.extend(rows);
    }

    {
        let mut statement = connection.prepare(
            "SELECT DISTINCT s.session_id, s.title \
             FROM session_tags t JOIN sessions s ON s.session_id = t.session_id \
             WHERE t.tag LIKE ?1 ORDER BY s.updated_at_ms DESC, s.session_id",
        )?;
        let rows = statement
            .query_map([&like], |row| {
                Ok(SessionSearchHit {
                    session_id: row.get(0)?,
                    title: row.get(1)?,
                    matched_on: SearchMatch::Tag,
                    snippet: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        hits.extend(rows);
    }

    {
        let mut statement = connection.prepare(
            "SELECT m.session_id, s.title, m.message_json \
             FROM messages m JOIN sessions s ON s.session_id = m.session_id \
             ORDER BY s.updated_at_ms DESC, s.session_id, m.sequence",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let normalized_query = query.to_lowercase();
        for (session_id, title, message_json) in rows {
            let message: Message = serde_json::from_str(&message_json)?;
            let text = message_plain_text(&message);
            if text.to_lowercase().contains(&normalized_query) {
                hits.push(SessionSearchHit {
                    session_id,
                    title,
                    matched_on: SearchMatch::Content,
                    snippet: Some(readable_snippet(&text)),
                });
            }
        }
    }

    Ok(merge_and_dedupe(hits))
}

/// 只抽取用户可见的 `Text` content；role、metadata 与 JSON 字段名不参与内容搜索。
fn message_plain_text(message: &Message) -> String {
    let mut text = String::new();
    for part in &message.content {
        collect_text_part(part, &mut text);
    }
    text
}

fn collect_text_part(part: &ContentPart, output: &mut String) {
    match part {
        ContentPart::Text(text) => {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&text.text);
        }
        ContentPart::ToolResult(result) => {
            for nested in &result.content {
                collect_text_part(nested, output);
            }
        }
        _ => {}
    }
}

fn readable_snippet(text: &str) -> String {
    let mut snippet: String = text.chars().take(120).collect();
    if text.chars().count() > 120 {
        snippet.push('…');
    }
    snippet
}

/// 合并来源、按 session 去重（标题命中优先保留）。
fn merge_and_dedupe(mut hits: Vec<SessionSearchHit>) -> Vec<SessionSearchHit> {
    let order = |m: SearchMatch| match m {
        SearchMatch::Title => 0,
        SearchMatch::Tag => 1,
        SearchMatch::Content => 2,
    };
    hits.sort_by(|a, b| match a.session_id.cmp(&b.session_id) {
        std::cmp::Ordering::Equal => order(a.matched_on).cmp(&order(b.matched_on)),
        other => other,
    });
    hits.dedup_by(|a, b| a.session_id == b.session_id);
    hits
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use agent_domain::{
        ContentPart, EventId, Message, MessageId, MessageMetadata, MessageRole, RunId, SessionId,
        TextContent, Timestamp,
    };
    use agent_events::{AgentEvent, AgentEventEnvelope, EventSequence};

    use crate::{SessionStore, DEFAULT_BRANCH_ID};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path() -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-search-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    fn committed_event(session: &SessionId, seq: u64, text: &str) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from(format!("event-{seq}")),
            session.clone(),
            RunId::from("run-1"),
            EventSequence::new(seq),
            Timestamp::from_unix_millis(seq),
            AgentEvent::MessageCommitted {
                message: Message {
                    id: MessageId::from(format!("msg-{seq}")),
                    role: MessageRole::User,
                    content: vec![ContentPart::Text(TextContent { text: text.into() })],
                    metadata: MessageMetadata::default(),
                },
            },
        )
    }

    #[tokio::test]
    async fn tags_are_normalized_deduped_and_listed() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-tags");
        store
            .create_session(&session, "tagged", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        store
            .add_tags(&session, &["Rust", " rust ", "Coding"])
            .await
            .expect("tags");
        let tags = store.list_tags(&session).await.expect("list");
        assert_eq!(tags, vec!["coding", "rust"]);

        store.remove_tag(&session, "RUST").await.expect("remove");
        let tags = store.list_tags(&session).await.expect("list");
        assert_eq!(tags, vec!["coding"]);

        store
            .set_tags(&session, &["alpha", "Beta"])
            .await
            .expect("set");
        assert_eq!(
            store.list_tags(&session).await.expect("list"),
            vec!["alpha", "beta"]
        );

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn search_hits_title_tag_and_content() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let title_session = SessionId::from("session-title");
        store
            .create_session(
                &title_session,
                "Refactor parser",
                Timestamp::from_unix_millis(1),
            )
            .await
            .expect("session");
        let tag_session = SessionId::from("session-tag");
        store
            .create_session(
                &tag_session,
                "unrelated title",
                Timestamp::from_unix_millis(2),
            )
            .await
            .expect("session");
        store
            .add_tags(&tag_session, &["parser"])
            .await
            .expect("tag");
        let content_session = SessionId::from("session-content");
        store
            .create_session(
                &content_session,
                "another title",
                Timestamp::from_unix_millis(3),
            )
            .await
            .expect("session");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                committed_event(&content_session, 1, "discuss the parser bug"),
            )
            .await
            .expect("content");

        let hits = store.search_sessions("parser").await.expect("search");
        let ids: Vec<&str> = hits.iter().map(|h| h.session_id.as_str()).collect();
        assert!(ids.contains(&"session-title"));
        assert!(ids.contains(&"session-tag"));
        assert!(ids.contains(&"session-content"));

        let content_hit = hits
            .iter()
            .find(|hit| hit.session_id == "session-content")
            .expect("content hit");
        assert_eq!(content_hit.matched_on, super::SearchMatch::Content);
        assert_eq!(
            content_hit.snippet.as_deref(),
            Some("discuss the parser bug")
        );

        let mut sorted = ids.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 3);

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn content_search_ignores_role_and_json_field_names() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-search-noise");
        store
            .create_session(&session, "quiet title", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                committed_event(&session, 1, "only visible words"),
            )
            .await
            .expect("content");

        assert!(store
            .search_sessions("user")
            .await
            .expect("role search")
            .is_empty());
        assert!(store
            .search_sessions("metadata")
            .await
            .expect("field search")
            .is_empty());
        let hits = store.search_sessions("visible").await.expect("text search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].snippet.as_deref(), Some("only visible words"));
        assert!(!hits[0].snippet.as_deref().unwrap().contains('{'));

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn empty_query_returns_no_hits() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-empty-q");
        store
            .create_session(&session, "anything", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        assert!(store
            .search_sessions("   ")
            .await
            .expect("search")
            .is_empty());
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
