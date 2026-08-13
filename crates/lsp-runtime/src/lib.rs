//! P17-4 LSP Client Runtime。
//!
//! Pawork 作为 LSP Client：启动、托管、调用现有 Language Server，把代码智能收敛为
//! Agent 可经统一接口消费的 canonical 能力。
//!
//! 架构约束（见 plan/P17-4-lsp-runtime.md 与 docs/adr）：
//! - 语言服务子进程属 Core-owned 本地进程，spawn / restart 一律经注入的
//!   [`transport::ServerSpawner`]（生产侧桥接 sandbox-runtime → process-runtime）；
//!   本 crate 禁止 `tokio::process` / 直接 spawn / 自建进程树清理。
//! - `Content-Length` framing 自实现严格状态机，不复用 SSE / JSONL / partial-json。
//! - rename / code_action 等写操作经注入的 [`write_policy::WriteEditPolicy`] 审批，
//!   不直接写盘。

#![forbid(unsafe_code)]

pub mod artifact;
pub mod capabilities;
pub mod client;
pub mod descriptor;
pub mod doc;
pub mod error;
pub mod framing;
pub mod jsonrpc;
pub mod protocol;
pub mod runtime;
pub mod transport;
pub mod write_policy;

pub use agent_domain::CancellationToken;
pub use artifact::{ArtifactSink, ArtifactSink as LspArtifactSink, ARTIFACT_INLINE_THRESHOLD};
pub use capabilities::{ClientCapabilities, ServerCapabilities};
pub use client::{LspClient, Phase, MAX_BUFFERED_NOTIFICATIONS};
pub use descriptor::{
    builtin_presets, clangd, from_resource, gopls, pyright, rust_analyzer,
    typescript_language_server, LanguageServerDescriptor, LspTransport, WorkspaceFolder,
};
pub use error::{FrameError, LspError, LspResult};
pub use framing::{encode_message, FrameEvent, LspFrameDecoder, MAX_FRAME_BYTES_HARD_LIMIT};
pub use protocol::*;
pub use runtime::LanguageClient;
pub use transport::{
    SandboxServerSpawner, ServerLifecycle, ServerReader, ServerSpawnConfig, ServerSpawner,
    ServerWriter, SharedSpawner, SpawnedServer,
};
pub use write_policy::{
    AllowThenApplyPolicy, DenyAllPolicy, EditApplier, EditOrigin, EditOutcome, EditRequest,
    PolicyVerdict, WriteEditPolicy,
};
