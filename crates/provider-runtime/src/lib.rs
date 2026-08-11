//! Provider 共用的运行时基础设施。
//!
//! 本 crate 不包含任何具体 Provider 的业务逻辑，只提供：
//! - [`http`]：统一的 HTTP 客户端（超时 / 代理 / 自定义 header / trace / 取消）；
//! - [`sse`] / [`jsonl`]：流式响应解析（跨 chunk、Unicode 边界、提前断开）；
//! - [`partial_json`]：跨 chunk 的 tool arguments 增量 JSON 拼接；
//! - [`retry`]：错误归一与可重试判定（生产退避由 `agent-engine` 单点负责）；
//! - [`reasoning`]：受保护 reasoning continuation 的统一存取桥；
//! - [`usage`]：token / 费用 / stop reason 归一；
//! - [`stream_assembly`]：`ProviderStreamEvent` → 领域消息组装。

pub mod capability;
pub mod http;
pub mod jsonl;
pub mod partial_json;
pub mod reasoning;
pub mod retry;
pub mod sse;
pub mod stream_assembly;
pub mod usage;
