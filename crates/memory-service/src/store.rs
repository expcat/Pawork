//! 纯内存记忆存储：canonical 事件折叠（apply / replay）+ 隐私过滤检索。
//!
//! 存储选择为纯内存 `BTreeMap`（不引入向量数据库）；embedding 作为 `Vec<f32>`
//! 直接存于 [`Memory`]。`apply` 是 replay 入口——它不重新嵌入（事件不携带向量），
//! 故 replay 出的记忆 `embedding` 为空，但记录 / 失效状态与实时路径一致。

use std::collections::BTreeMap;

use agent_domain::{MemoryEvent, MemoryId, MemoryPrivacy, WorkspaceId};

use crate::model::Memory;
use crate::similarity::{cosine_similarity, estimate_tokens};

#[derive(Debug, Default)]
pub struct MemoryStore {
    memories: BTreeMap<MemoryId, Memory>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.memories.len()
    }

    pub fn is_empty(&self) -> bool {
        self.memories.is_empty()
    }

    pub fn get(&self, id: &MemoryId) -> Option<&Memory> {
        self.memories.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Memory> {
        self.memories.values()
    }

    /// 折叠 canonical [`MemoryEvent`]（replay 入口）。
    ///
    /// - `Recorded`：插入记忆（`embedding` 为空，因为事件不携带向量）。
    /// - `Invalidated`：标记 `valid=false`（不删除）。
    ///
    /// 幂等：重复 apply 同一事件结果一致。
    pub fn apply(&mut self, event: &MemoryEvent) {
        match event {
            MemoryEvent::Recorded {
                memory_id,
                summary,
                source_event_id,
                privacy,
                workspace_id,
            } => {
                self.memories.insert(
                    memory_id.clone(),
                    Memory {
                        memory_id: memory_id.clone(),
                        summary: summary.clone(),
                        source_event_id: source_event_id.clone(),
                        confidence: 0.0,
                        privacy: *privacy,
                        workspace_id: workspace_id.clone(),
                        embedding: Vec::new(),
                        valid: true,
                    },
                );
            }
            MemoryEvent::Invalidated {
                memory_id,
                reason: _reason,
            } => {
                if let Some(memory) = self.memories.get_mut(memory_id) {
                    memory.valid = false;
                }
            }
        }
    }

    /// 实时写入路径：直接存入含 embedding 的完整记忆（service.record 使用）。
    pub(crate) fn ingest(&mut self, memory: Memory) {
        self.memories.insert(memory.memory_id.clone(), memory);
    }

    /// 标记失效；返回先前是否有效（`false` 表示不存在或本已失效）。
    pub fn invalidate(&mut self, memory_id: &MemoryId) -> bool {
        if let Some(memory) = self.memories.get_mut(memory_id) {
            let was_valid = memory.valid;
            memory.valid = false;
            was_valid
        } else {
            false
        }
    }

    /// 按余弦相似度 + token 预算检索 Top-K（自实现，无向量数据库）。
    ///
    /// 隐私过滤：`WorkspaceLocal` 仅在 `workspace_id` 与记忆归属一致时可见；
    /// `Shareable` 跨 workspace 可见。`valid=false` 与空 embedding 的记忆被排除。
    pub fn retrieve(
        &self,
        query_vec: &[f32],
        workspace_id: Option<&WorkspaceId>,
        top_k: usize,
        budget_tokens: usize,
    ) -> Vec<&Memory> {
        let mut scored: Vec<(f32, &Memory)> = self
            .memories
            .values()
            .filter(|memory| memory.valid)
            .filter(|memory| visible(memory, workspace_id))
            .filter(|memory| !memory.embedding.is_empty())
            .map(|memory| (cosine_similarity(query_vec, &memory.embedding), memory))
            .collect();

        // 相似度降序；并列按 memory_id 升序以保证确定性。
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.memory_id.cmp(&b.1.memory_id))
        });

        let mut spent = 0_usize;
        let mut out: Vec<&Memory> = Vec::new();
        for (score, memory) in scored {
            if out.len() >= top_k {
                break;
            }
            // 无正相关性（<=0）的记忆不纳入检索结果。
            if score <= 0.0 {
                continue;
            }
            let cost = estimate_tokens(&memory.summary);
            // 超预算则跳过，继续尝试更小 / 更低相似度的记忆以充分利用预算。
            if spent.saturating_add(cost) > budget_tokens {
                continue;
            }
            spent += cost;
            out.push(memory);
        }
        out
    }
}

fn visible(memory: &Memory, workspace_id: Option<&WorkspaceId>) -> bool {
    match memory.privacy {
        MemoryPrivacy::Shareable => true,
        MemoryPrivacy::WorkspaceLocal => match (workspace_id, &memory.workspace_id) {
            (Some(query), Some(owner)) => query == owner,
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{MemoryId, WorkspaceId};

    fn mem(
        id: &str,
        summary: &str,
        privacy: MemoryPrivacy,
        ws: Option<&str>,
        emb: Vec<f32>,
    ) -> Memory {
        Memory {
            memory_id: MemoryId::new(id),
            summary: summary.to_owned(),
            source_event_id: None,
            confidence: 0.0,
            privacy,
            workspace_id: ws.map(WorkspaceId::new),
            embedding: emb,
            valid: true,
        }
    }

    #[test]
    fn privacy_filters_workspace_local() {
        let mut store = MemoryStore::new();
        store.ingest(mem(
            "a",
            "local note",
            MemoryPrivacy::WorkspaceLocal,
            Some("ws1"),
            vec![1.0],
        ));
        store.ingest(mem(
            "b",
            "shared note",
            MemoryPrivacy::Shareable,
            Some("ws1"),
            vec![1.0],
        ));

        let ws2 = WorkspaceId::new("ws2");
        let got: Vec<String> = store
            .retrieve(&[1.0], Some(&ws2), 10, 1024)
            .into_iter()
            .map(|m| m.memory_id.as_str().to_owned())
            .collect();
        // workspace_local(ws1) 不可见于 ws2；shareable 可见。
        assert_eq!(got, vec!["b".to_owned()]);

        let ws1 = WorkspaceId::new("ws1");
        assert_eq!(store.retrieve(&[1.0], Some(&ws1), 10, 1024).len(), 2);
    }

    #[test]
    fn budget_caps_results() {
        let mut store = MemoryStore::new();
        // 每条 summary 4 chars -> 1 token；embedding 2 维以获得真实相似度梯度。
        store.ingest(mem(
            "a",
            "aaaa",
            MemoryPrivacy::Shareable,
            None,
            vec![1.0, 0.0],
        ));
        store.ingest(mem(
            "b",
            "bbbb",
            MemoryPrivacy::Shareable,
            None,
            vec![0.0, 1.0],
        ));
        store.ingest(mem(
            "c",
            "cccc",
            MemoryPrivacy::Shareable,
            None,
            vec![1.0, 1.0],
        ));

        // query=[1,0]：a=1.0，c≈0.707，b=0（无正相关性，排除）。
        let got1 = store.retrieve(&[1.0, 0.0], None, 10, 1);
        assert_eq!(got1.len(), 1);
        assert_eq!(got1[0].memory_id.as_str(), "a");

        let got2 = store.retrieve(&[1.0, 0.0], None, 10, 2);
        assert_eq!(got2.len(), 2);
        assert_eq!(got2[0].memory_id.as_str(), "a");
        assert_eq!(got2[1].memory_id.as_str(), "c");
    }

    #[test]
    fn apply_replay_matches_direct_validity() {
        let mut direct = MemoryStore::new();
        direct.ingest(mem(
            "m1",
            "hello world",
            MemoryPrivacy::WorkspaceLocal,
            Some("ws1"),
            vec![1.0],
        ));

        // Replay 路径：仅事件，无 embedding。
        let mut replayed = MemoryStore::new();
        replayed.apply(&MemoryEvent::Recorded {
            memory_id: MemoryId::new("m1"),
            summary: "hello world".to_owned(),
            source_event_id: None,
            privacy: MemoryPrivacy::WorkspaceLocal,
            workspace_id: Some(WorkspaceId::new("ws1")),
        });
        replayed.apply(&MemoryEvent::Invalidated {
            memory_id: MemoryId::new("m1"),
            reason: "stale".to_owned(),
        });
        // 直接路径同样失效。
        assert!(direct.invalidate(&MemoryId::new("m1")));

        let direct_mem = direct.get(&MemoryId::new("m1")).unwrap();
        let replayed_mem = replayed.get(&MemoryId::new("m1")).unwrap();
        assert_eq!(direct_mem.memory_id, replayed_mem.memory_id);
        assert_eq!(direct_mem.summary, replayed_mem.summary);
        assert_eq!(direct_mem.valid, replayed_mem.valid);
        assert!(!replayed_mem.valid);
        // replay 不携带 embedding（事件不存向量）。
        assert!(replayed_mem.embedding.is_empty());
    }
}
