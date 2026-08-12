//! 只读提炼：从历史 canonical 事件产出候选记忆。
//!
//! **绝不修改 / 删除任何输入事件**——所有入口只接受 `&` 引用。
//! 含明显 Secret / 敏感关键词的内容不进入记忆（简单启发式）。

use std::collections::HashMap;

use agent_domain::{EventId, MemoryPrivacy};
use agent_events::{AgentEvent, AgentEventEnvelope};

use crate::model::CandidateMemory;

/// 简单 Secret 启发式关键词（小写匹配）。非穷举，仅为默认防线；
/// 真正的脱敏应由 Policy / redaction 在更外层保障。
pub const SECRET_MARKERS: &[&str] = &[
    "secret",
    "password",
    "passwd",
    "token",
    "api_key",
    "apikey",
    "access_key",
    "private_key",
    "private key",
    "credential",
    "bearer",
    "authorization",
];

/// 命中任一敏感关键词即视为含 Secret（大小写不敏感）。
pub fn contains_secret(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    SECRET_MARKERS.iter().any(|marker| lower.contains(marker))
}

/// 从持久化的 canonical 事件信封（只读）提炼候选记忆。
///
/// 把同一 `message_id` 下的 `AssistantTextDelta` 折叠为一条完整助手发言，
/// 去空白 / 过滤 Secret 后产出候选。输入仅借用，绝不修改。
pub fn extract(envelopes: &[AgentEventEnvelope]) -> Vec<CandidateMemory> {
    extract_from_iter(
        envelopes
            .iter()
            .map(|env| (Some(env.event_id.clone()), &env.payload)),
    )
}

/// 从 canonical 事件负载切片（只读）提炼候选记忆。
///
/// 与 [`extract`] 的区别：不携带信封，故 `source_event_id` 恒为 `None`。
pub fn extract_from_events(events: &[AgentEvent]) -> Vec<CandidateMemory> {
    extract_from_iter(events.iter().map(|payload| (None, payload)))
}

fn extract_from_iter<'a, I>(iter: I) -> Vec<CandidateMemory>
where
    I: IntoIterator<Item = (Option<EventId>, &'a AgentEvent)>,
{
    // message_id -> (首个 source_event_id, 累积文本)；order 记录首次出现顺序。
    let mut turns: HashMap<String, (Option<EventId>, String)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for (source, payload) in iter {
        if let AgentEvent::AssistantTextDelta { message_id, delta } = payload {
            let key = message_id.as_str().to_owned();
            let entry = turns.entry(key).or_insert_with(|| {
                order.push(message_id.as_str().to_owned());
                (source.clone(), String::new())
            });
            entry.1.push_str(delta);
        }
    }

    let mut out = Vec::new();
    for key in order {
        let (source_event_id, text) = turns.remove(&key).expect("key inserted once above");
        let summary = text.trim();
        if summary.is_empty() || contains_secret(summary) {
            continue;
        }
        out.push(CandidateMemory {
            summary: summary.to_owned(),
            source_event_id,
            privacy: MemoryPrivacy::WorkspaceLocal,
            workspace_id: None,
            confidence: 0.5,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{EventId, MessageId, RunId, SessionId, Timestamp};
    use agent_events::{AgentEvent, AgentEventEnvelope, EventSequence};

    fn delta_envelope(eid: &str, mid: &str, delta: &str, seq: u64) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::new(eid),
            SessionId::new("s1"),
            RunId::new("r1"),
            EventSequence::new(seq),
            Timestamp::from_unix_millis(seq),
            AgentEvent::AssistantTextDelta {
                message_id: MessageId::new(mid),
                delta: delta.to_owned(),
            },
        )
    }

    #[test]
    fn extract_is_readonly_and_groups() {
        let envelopes = vec![
            delta_envelope("e1", "m1", "Hello ", 1),
            delta_envelope("e2", "m1", "world", 2),
            delta_envelope("e3", "m2", "the api_key is leaked", 3),
        ];
        let before = envelopes.clone();
        let candidates = extract(&envelopes);
        // 只读：输入未被修改。
        assert_eq!(envelopes, before);
        // m1 折叠为一条；m2 命中 Secret 被丢弃。
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].summary, "Hello world");
        assert_eq!(
            candidates[0].source_event_id.as_ref().unwrap().as_str(),
            "e1"
        );
    }

    #[test]
    fn extract_from_events_has_no_source() {
        let events = vec![AgentEvent::AssistantTextDelta {
            message_id: MessageId::new("m1"),
            delta: "clean fact".to_owned(),
        }];
        let got = extract_from_events(&events);
        assert_eq!(got.len(), 1);
        assert!(got[0].source_event_id.is_none());
    }

    #[test]
    fn contains_secret_cases() {
        assert!(contains_secret("my password is 123"));
        assert!(contains_secret("Bearer xyz"));
        assert!(contains_secret("set API_KEY=..."));
        assert!(contains_secret("a real secret here"));
        assert!(!contains_secret("the weather is nice"));
    }
}
