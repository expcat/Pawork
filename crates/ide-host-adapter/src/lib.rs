//! # P17-9 IDE Host Adapter
//!
//! 把 IDE（VS Code / JetBrains 等）的编辑器生命周期、诊断与交互桥接到
//! Pawork。接入链路统一为：
//!
//! ```text
//! IDE Extension -> IDE Host Adapter -> Agent SDK / Headless Protocol -> pawork Host -> Core
//! ```
//!
//! 本 crate 是 Host Adapter / Client Channel：
//!
//! - 复用 `client_adapter_api` 的 `ClientAdapter` 契约与 `SessionRegistry`
//!   （能力协商、client/core session 绑定、ownership epoch/revision）；
//! - 复用 `agent_sdk` 的 `PaworkClient` 与 `headless-json` NDJSON 通道连接
//!   **唯一正式宿主** `pawork`；
//! - 只做协议翻译：能力协商、session/run/event、取消、重连、IDE 诊断与
//!   可选 LSP 输出映射；**不承载业务决策、不构造第二 Core、不取代 GUI
//!   Connection Protocol**（ADR-021 / ADR-025 / ADR-030）。
//!
//! ## 模块
//!
//! - [`contract`]：最小「IDE 扩展 ↔ Adapter」契约（`IdeRequest` / `IdeEvent`
//!   消息子集，serde 可序列化，IDE 扩展实现该契约即可接入）。
//! - [`adapter`]：`ClientFrame` ↔ canonical 的协议翻译层（`IdeClientAdapter`，
//!   实现 `client_adapter_api::ClientAdapter`）。
//! - [`lifecycle`]：IDE 适配 trait（编辑器打开/关闭/激活、选区、可见范围、
//!   保存）与 `EditorContext` 状态。
//! - [`diagnostics`]：诊断双向映射（P17-4 LSP Client 聚合结果 → IDE；IDE
//!   诊断变化 → canonical 变更记录），只映射、不绕过 Policy。
//! - [`host`]：`IdeHostAdapter` 连接器——能力协商、session/run/event、取消、
//!   重连，驱动 SDK/Headless 通道。
//! - [`sdk_channel`]：`SdkChannel` 抽象（真实 `PaworkClient` 与 mock 可替换）。
//! - [`lsp_output`]：可选 LSP Server 输出映射（复用 P17-4 聚合结果，不改变
//!   P17-4 作为 LSP Client 的主定位）。
//!
//! ## 边界
//!
//! 本 crate 不依赖 `gui-protocol` / `gui-server` / `core-runtime` / `app-service`，
//! 不构造 Core；IDE 与独立 GUI 经各自通道并存。

pub mod adapter;
pub mod contract;
pub mod diagnostics;
pub mod error;
pub mod host;
pub mod lifecycle;
pub mod lsp_output;
pub mod sdk_channel;

pub use adapter::{IdeClientAdapter, IdeClientAdapterFactory};
pub use contract::{
    IdeCapability, IdeDiagnostic, IdeEvent, IdeRequest, LspQueryKind, IDE_CONTRACT_SCHEMA_VERSION,
    IDE_PROTOCOL, IDE_PROTOCOL_VERSION,
};
pub use diagnostics::{DiagnosticBoard, IdeDiagnosticSet};
pub use error::IdeAdapterError;
pub use host::{IdeHostAdapter, IdeHostOptions};
pub use lifecycle::{EditorContext, EditorLifecycleEvent, IdeLifecycle, LifecycleMapper};
pub use lsp_output::{LspOutputEncoder, LspResultProvider};
pub use sdk_channel::{PaworkSdkChannel, SdkChannel};
