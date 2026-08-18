//! # Pawork Rust Agent SDK
//!
//! 连接 **pawork Host**（`pawork` 二进制的 `headless --json-stdio` 模式）
//! 的 typed client。本 crate **不嵌入 Core、不实例化 Provider、不做业务决策**：
//! 它是纯 client，通过稳定 NDJSON framing（[`pawork_protocol::headless`]）驱动 Host，
//! 把 [`AppCommand`](pawork_protocol::AppCommand) / [`AppQuery`](pawork_protocol::AppQuery)
//! 映射为请求、把 [`AppEvent`](pawork_protocol::AppEvent) 流式暴露给调用方。
//!
//! ```text
//! Rust Application → pawork-sdk (PaworkClient)
//!     → pawork headless --json-stdio（唯一 Core 宿主）
//! ```
//!
//! ## 快速开始
//!
//! ```no_run
//! # async fn example() -> Result<(), pawork_sdk::SdkError> {
//! use pawork_sdk::{PaworkClient, PaworkOptions};
//! use pawork_domain::WorkspaceId;
//!
//! let client = PaworkClient::spawn(PaworkOptions::default()).await?;
//! let session = client
//!     .create_session(WorkspaceId::from("ws-1"), Some("demo".into()))
//!     .await?;
//! let run = client
//!     .run_start(session.session_id.clone(), "hello".into(), None)
//!     .await?;
//! client.cancel(run.run_id).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## 版本与稳定面（语义化策略）
//!
//! - SDK 自身的 crate 版本按 semver 演进；`SDK_API_VERSION` 声明 SDK 期望的
//!   协议版本（V2 `API_VERSION` = 1.2），握手时与 Host 协商，major 不兼容即显式失败。
//! - 稳定面：`PaworkClient`、`PaworkOptions`、`EventSubscription`、
//!   [`error::SdkError`]、`transport::Transport`、`mock::MockTransport`。
//!   稳定面内的行为改动需要 minor 版本与 CHANGELOG 记录。
//! - 实验面：`experimental` 模块内的辅助 API，可能在不发 major 的情况下
//!   调整；`is_stable` 标记用于区分。
//! - unknown / unsupported 情况全部落到 [`error::SdkErrorKind`] 的显式类别
//!   （`UnknownResponseType` / `UnsupportedCapability` /
//!   `IncompatibleApiVersion`），不静默忽略。
//!
//! ## 事件订阅与背压
//!
//! [`PaworkClient::subscribe`] 返回有界通道的 [`EventSubscription`]；消费者
//! 跟不上时按 [`BackpressurePolicy`] 丢弃并计数，或返回
//! [`SdkErrorKind::Backpressure`]。
//!
//! ## 模块
//!
//! - [`client`]：`PaworkClient` 与高层 API。
//! - [`transport`]：`Transport` 契约与 stdio 进程实现。
//! - [`mock`]：mock transport（测试与下游集成用）。
//! - [`stream`]：事件订阅（有界、可取消）。
//! - [`version`]：版本与稳定/实验面策略。
//! - [`error`]：`SdkError` 与类别。
//! - [`ide`]（feature `ide`）：门控占位；V1 adapter 缠住 LSP，本波不迁实现。

pub mod client;
pub mod error;
#[cfg(feature = "ide")]
pub mod ide;
pub mod mock;
pub mod stream;
pub mod transport;
pub mod version;

pub use client::PaworkClient;
pub use error::{SdkError, SdkErrorKind};
pub use stream::{BackpressurePolicy, EventSubscription};
pub use transport::{PaworkOptions, Transport};
pub use version::{SDK_API_VERSION, SDK_VERSION};

/// 便捷入口：`pawork_sdk::spawn_pawork(options)`。
pub use client::spawn_pawork;

/// 实验面：API 可能在不发 major 的情况下调整；使用前请核对版本说明。
pub mod experimental {
    pub use crate::client::CompatOutcome;
}

/// 便捷重导出：常用协议类型（与 `pawork-protocol` 同构，免去额外依赖）。
pub mod reexport {
    pub use pawork_protocol::{
        ApiHandle, ApiVersion, AppCommand, AppEvent, AppEventEnvelope, AppQuery, AppResponse,
        AppResponseEnvelope, CommandSource, EventStream, RunState, API_VERSION,
    };
}
