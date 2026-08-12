//! Pawork P16-7 Long-term Memory：只读提炼、canonical EmbeddingProvider 检索注入（Phase 16）。
//!
//! 跨会话长期记忆 service。记忆从历史 canonical 事件只读提炼，**绝不修改 / 删除
//! 任何输入事件**；记忆文本经 canonical [`EmbeddingProvider`](provider_api::EmbeddingProvider)
//! 取向量，检索走自实现余弦相似度（不引入向量数据库，存储为纯内存）。
//!
//! 设计约束（与服务级 `AGENTS.md` 红线一致）：
//! - `MemoryService` 只持有 `dyn EmbeddingProvider` trait 对象，**禁止按 Provider
//!   名称分支、禁止用 `provider_options`、禁止自造 Provider-specific 请求**。
//! - 含明显 Secret / 敏感关键词的内容不进入记忆（启发式过滤）。
//! - 记忆记录 / 失效为 canonical [`MemoryEvent`](agent_domain::MemoryEvent)，可持久化、
//!   可重放；失效为 `valid=false` 而非删除，保留可追溯。
//! - 跨 workspace 默认不共享：`WorkspaceLocal` 受 `workspace_id` 过滤，`Shareable` 可跨。
//!
//! 具体领域类型与事件载荷由 `agent-domain::workflow` 与 `agent_events::AgentEvent` 提供。

mod error;
mod extract;
mod model;
mod service;
mod similarity;
mod store;

pub use error::MemoryError;
pub use extract::{contains_secret, extract, extract_from_events, SECRET_MARKERS};
pub use model::{CandidateMemory, Memory};
pub use service::MemoryService;
pub use similarity::{cosine_similarity, estimate_tokens};
pub use store::MemoryStore;
