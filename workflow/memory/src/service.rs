//! 长期记忆 service：依赖注入 canonical `EmbeddingProvider`，串联提炼 → 嵌入 → 存储 → 检索。
//!
//! **不感知 Provider 名称**：只持有 `Arc<dyn EmbeddingProvider>` trait 对象，
//! 请求构造走 canonical `EmbeddingRequest` 字段。`no_provider_branch` 测试守护该不变量。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use pawork_api::{EmbeddingProvider, EmbeddingRequest};
use pawork_domain::{CancellationToken, MemoryEvent, MemoryId, ModelId, WorkspaceId};

use crate::error::MemoryError;
use crate::extract::contains_secret;
use crate::model::{CandidateMemory, Memory};
use crate::store::MemoryStore;

pub struct MemoryService {
    provider: Arc<dyn EmbeddingProvider>,
    model: ModelId,
    store: MemoryStore,
    next_id: AtomicU64,
}

impl MemoryService {
    /// 依赖注入 embedding provider；`model` 决定 canonical embedding 模型。
    pub fn new(provider: Arc<dyn EmbeddingProvider>, model: ModelId) -> Self {
        Self {
            provider,
            model,
            store: MemoryStore::new(),
            next_id: AtomicU64::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }

    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    pub fn get(&self, id: &MemoryId) -> Option<&Memory> {
        self.store.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Memory> {
        self.store.iter()
    }

    fn alloc_id(&self) -> MemoryId {
        let n = self.next_id.fetch_add(1, Ordering::Relaxed);
        MemoryId::new(format!("mem-{n}"))
    }

    /// 嵌入候选文本并写入记忆，返回 canonical `Recorded` 事件以持久化。
    ///
    /// 空 / 含 Secret 的候选被拒绝（不进入记忆）。`workspace_id` 用于跨 workspace 隔离。
    pub async fn record(
        &mut self,
        candidate: CandidateMemory,
        workspace_id: Option<WorkspaceId>,
        cancel: CancellationToken,
    ) -> Result<MemoryEvent, MemoryError> {
        let CandidateMemory {
            summary,
            source_event_id,
            privacy,
            workspace_id: candidate_ws,
            confidence,
        } = candidate;

        let summary = summary.trim().to_owned();
        if summary.is_empty() {
            return Err(MemoryError::EmptySummary);
        }
        if contains_secret(&summary) {
            return Err(MemoryError::SecretDetected);
        }

        let response = self
            .provider
            .embed(
                EmbeddingRequest {
                    model: self.model.clone(),
                    inputs: vec![summary.clone()],
                    dimensions: None,
                },
                None,
                cancel,
            )
            .await?;
        let embedding = response.vectors.into_iter().next().unwrap_or_default();

        let memory_id = self.alloc_id();
        let resolved_ws = candidate_ws.or(workspace_id);
        let memory = Memory {
            memory_id: memory_id.clone(),
            summary: summary.clone(),
            source_event_id: source_event_id.clone(),
            confidence,
            privacy,
            workspace_id: resolved_ws.clone(),
            embedding: embedding.clone(),
            valid: true,
        };
        self.store.ingest(memory);

        Ok(MemoryEvent::Recorded {
            memory_id,
            summary,
            source_event_id,
            privacy,
            workspace_id: resolved_ws,
            embedding,
            confidence,
        })
    }

    /// 失效记忆：发 `Invalidated` 事件并标记 `valid=false`（不删除，可追溯）。
    pub fn invalidate(
        &mut self,
        memory_id: &MemoryId,
        reason: impl Into<String>,
    ) -> Result<MemoryEvent, MemoryError> {
        // 先以只读借用判定状态，再释放借用执行可变写入。
        let status = match self.store.get(memory_id) {
            None => InvalidationStatus::Missing,
            Some(memory) if !memory.valid => InvalidationStatus::AlreadyInvalid,
            Some(_) => InvalidationStatus::Valid,
        };
        match status {
            InvalidationStatus::Missing => Err(MemoryError::NotFound(memory_id.to_string())),
            InvalidationStatus::AlreadyInvalid => {
                Err(MemoryError::AlreadyInvalidated(memory_id.to_string()))
            }
            InvalidationStatus::Valid => {
                self.store.invalidate(memory_id);
                Ok(MemoryEvent::Invalidated {
                    memory_id: memory_id.clone(),
                    reason: reason.into(),
                })
            }
        }
    }

    /// 检索 Top-K（受 token 预算约束），按 workspace 归属与隐私标签隔离。
    pub fn retrieve(
        &self,
        query_vec: &[f32],
        workspace_id: Option<&WorkspaceId>,
        top_k: usize,
        budget_tokens: usize,
    ) -> Vec<&Memory> {
        self.store
            .retrieve(query_vec, workspace_id, top_k, budget_tokens)
    }

    /// 折叠 canonical 事件（replay / 重放入口，委托 store）。
    pub fn apply(&mut self, event: &MemoryEvent) {
        self.store.apply(event);
    }
}

enum InvalidationStatus {
    Missing,
    AlreadyInvalid,
    Valid,
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use pawork_api::{
        EmbeddingModelDefinition, EmbeddingProvider, EmbeddingRequest, EmbeddingResponse,
        EmbeddingUsage, ProviderError, ResolvedCredential,
    };

    /// 固定向量 embedder：每条输入映射为 [字节和, 长度]，便于检索断言。
    /// 仅用于测试——返回确定性向量，不访问网络。
    struct FixedEmbedder;

    #[async_trait]
    impl EmbeddingProvider for FixedEmbedder {
        fn id(&self) -> pawork_domain::ProviderId {
            pawork_domain::ProviderId::new("fixed-embedder")
        }

        async fn list_embedding_models(
            &self,
            _credential: Option<&ResolvedCredential>,
        ) -> Result<Vec<EmbeddingModelDefinition>, ProviderError> {
            Ok(Vec::new())
        }

        async fn embed(
            &self,
            request: EmbeddingRequest,
            _credential: Option<&ResolvedCredential>,
            _cancel: CancellationToken,
        ) -> Result<EmbeddingResponse, ProviderError> {
            let vectors = request
                .inputs
                .iter()
                .map(|input| {
                    let sum: f32 = input.bytes().map(|byte| byte as f32).sum();
                    vec![sum, input.len() as f32]
                })
                .collect();
            Ok(EmbeddingResponse {
                model: request.model,
                vectors,
                usage: EmbeddingUsage::default(),
            })
        }
    }

    fn svc() -> MemoryService {
        MemoryService::new(Arc::new(FixedEmbedder), ModelId::new("embed-test"))
    }

    /// 与 FixedEmbedder 一致的向量构造，用于检索查询。
    fn embed_of(input: &str) -> Vec<f32> {
        let sum: f32 = input.bytes().map(|byte| byte as f32).sum();
        vec![sum, input.len() as f32]
    }

    #[tokio::test]
    async fn record_and_retrieve_roundtrip() {
        let mut service = svc();
        let summary = "the user prefers concise answers";
        let event = service
            .record(
                CandidateMemory::new(summary),
                Some(WorkspaceId::new("ws1")),
                CancellationToken::new(),
            )
            .await
            .expect("record ok");
        let MemoryEvent::Recorded { memory_id, .. } = &event else {
            panic!("expected Recorded event");
        };
        assert_eq!(
            service.get(memory_id).unwrap().privacy,
            pawork_domain::MemoryPrivacy::WorkspaceLocal
        );
        // 用同一文本向量检索 → 命中该记忆（cosine = 1）。
        let got = service.retrieve(&embed_of(summary), Some(&WorkspaceId::new("ws1")), 5, 1024);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].memory_id, *memory_id);
        assert!(!got[0].embedding.is_empty());
    }

    #[tokio::test]
    async fn secret_summary_rejected() {
        let mut service = svc();
        let err = service
            .record(
                CandidateMemory::new("the api_key is sk-xxxx"),
                None,
                CancellationToken::new(),
            )
            .await
            .expect_err("secret rejected");
        assert!(matches!(err, MemoryError::SecretDetected));
        assert_eq!(service.len(), 0);
    }

    #[tokio::test]
    async fn empty_summary_rejected() {
        let mut service = svc();
        let err = service
            .record(CandidateMemory::new("   "), None, CancellationToken::new())
            .await
            .expect_err("empty rejected");
        assert!(matches!(err, MemoryError::EmptySummary));
    }

    #[tokio::test]
    async fn invalidate_marks_invalid_and_emits_event() {
        let mut service = svc();
        let recorded = service
            .record(
                CandidateMemory::new("fact one"),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let MemoryEvent::Recorded { memory_id, .. } = &recorded else {
            panic!("recorded");
        };

        let invalidated = service
            .invalidate(memory_id, "superseded")
            .expect("invalidate ok");
        assert!(matches!(invalidated, MemoryEvent::Invalidated { .. }));
        assert!(!service.get(memory_id).unwrap().valid);

        // retrieve 不再返回已失效记忆。
        let got = service.retrieve(&embed_of("fact one"), None, 5, 1024);
        assert!(got.is_empty());

        // 重复失效报错。
        let err = service.invalidate(memory_id, "again").unwrap_err();
        assert!(matches!(err, MemoryError::AlreadyInvalidated(_)));
    }

    #[tokio::test]
    async fn invalidate_missing_errors() {
        let mut service = svc();
        let err = service
            .invalidate(&MemoryId::new("nope"), "gone")
            .unwrap_err();
        assert!(matches!(err, MemoryError::NotFound(_)));
    }

    #[tokio::test]
    async fn replay_via_apply_matches_live_path_completely() {
        let mut service = svc();
        let recorded = service
            .record(
                CandidateMemory::new("persisted fact"),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let MemoryEvent::Recorded { memory_id, .. } = &recorded else {
            panic!("recorded");
        };
        let memory_id = memory_id.clone();

        // 用 apply 重放到全新 service：embedding / confidence / 全字段完整一致。
        let mut replay = svc();
        replay.apply(&recorded);
        let live = service.get(&memory_id).unwrap();
        let replayed = replay.get(&memory_id).unwrap();
        assert_eq!(replayed.memory_id, live.memory_id);
        assert_eq!(replayed.summary, live.summary);
        assert_eq!(replayed.source_event_id, live.source_event_id);
        assert_eq!(replayed.confidence, live.confidence);
        assert_eq!(replayed.privacy, live.privacy);
        assert_eq!(replayed.workspace_id, live.workspace_id);
        assert_eq!(replayed.embedding, live.embedding);
        assert_eq!(replayed.valid, live.valid);
        // replay 后 embedding 不再为空（ADR-016）。
        assert!(!replayed.embedding.is_empty());
    }

    #[tokio::test]
    async fn shareable_crosses_workspaces() {
        let mut service = svc();
        let mut candidate = CandidateMemory::new("global convention note");
        candidate.privacy = pawork_domain::MemoryPrivacy::Shareable;
        service
            .record(
                candidate,
                Some(WorkspaceId::new("ws1")),
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // 从另一 workspace 检索仍可见。
        let ws2 = WorkspaceId::new("ws2");
        let got = service.retrieve(&embed_of("global convention note"), Some(&ws2), 5, 1024);
        assert_eq!(got.len(), 1);
    }

    /// 守护「service 只持有 trait 对象、不含任何 Provider 名分支」不变量。
    ///
    /// forbidden 名以分片形式枚举，避免测试自身把完整名字写进被扫描的源码。
    #[test]
    fn no_provider_branch() {
        let sources = [
            include_str!("error.rs"),
            include_str!("model.rs"),
            include_str!("extract.rs"),
            include_str!("similarity.rs"),
            include_str!("store.rs"),
            include_str!("service.rs"),
        ];
        // 具体供应商名（分片拼接，源码中不出现完整连续串）。
        const FORBIDDEN_PARTS: &[(&str, &str)] = &[
            ("anth", "ropic"),
            ("clau", "de"),
            ("open", "ai"),
            ("zhi", "pu"),
            ("gl", "m-"),
            ("moon", "shot"),
            ("ki", "mi"),
            ("qw", "en"),
            ("tong", "yi"),
            ("deep", "seek"),
            ("gr", "ok"),
            ("x", "ai"),
            ("gem", "ini"),
            ("goo", "gle"),
        ];
        for source in sources {
            let lower = source.to_ascii_lowercase();
            for (a, b) in FORBIDDEN_PARTS {
                let name = format!("{a}{b}");
                assert!(
                    !lower.contains(&name),
                    "provider name `{name}` appears in memory-service source — forbidden branch/smell"
                );
            }
        }
        // 关键不变量：provider 字段为 canonical trait 对象。
        assert!(
            include_str!("service.rs").contains("Arc<dyn EmbeddingProvider>"),
            "MemoryService must hold a canonical dyn EmbeddingProvider trait object"
        );
    }
}
