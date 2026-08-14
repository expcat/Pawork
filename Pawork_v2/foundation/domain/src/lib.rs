//! Pawork 的最底层领域类型。
//!
//! 本 crate 只包含纯数据与基于标准库的协作式取消语义，不执行 IO，也不依赖
//! 数据库、HTTP、Git、任何 GUI framework（包括 GPUI/Tauri）、OS Keychain 或任何具体 Provider。

mod cancel;
mod error;
mod events;
mod ids;
mod message;
mod profile;
mod reasoning;
mod server_tool;
mod tool;
mod workflow;

pub use cancel::{CancellationFuture, CancellationToken};
pub use error::{ErrorCategory, ErrorContext};
pub use events::*;
pub use ids::*;
pub use message::*;
pub use profile::*;
pub use reasoning::*;
pub use server_tool::*;
pub use tool::*;
pub use workflow::*;
