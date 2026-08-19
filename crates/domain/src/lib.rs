//! Pawork 的最底层领域类型与 Provider/Tool 契约面。
//!
//! 本 crate 只包含纯数据与基于标准库的协作式取消语义，不执行 IO，也不依赖
//! 数据库、HTTP、Git、任何 GUI framework（包括 GPUI/Tauri）、OS Keychain 或任何具体 Provider。
//! R1 起（ADR-039），原 `pawork-api` 的 Provider 契约与工具执行契约并入
//! `provider_api` / `tool_api` 两模块，纯净红线不变。

mod cancel;
mod client_session;
mod error;
mod events;
mod ids;
mod message;
mod profile;
mod provider_api;
mod reasoning;
mod server_tool;
mod tool;
mod tool_api;
mod workflow;

pub use cancel::{CancellationFuture, CancellationToken};
pub use client_session::*;
pub use error::{ErrorCategory, ErrorContext};
pub use events::*;
pub use ids::*;
pub use message::*;
pub use profile::*;
pub use provider_api::*;
pub use reasoning::*;
pub use server_tool::*;
pub use tool::*;
pub use tool_api::*;
pub use workflow::*;
