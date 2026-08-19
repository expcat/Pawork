//! # Pawork Headless JSON（NDJSON）协议
//!
//! 语言无关的 stdin/stdout JSONL 接入层：脚本/CI 通过
//! `pawork headless --json-stdio` 收发 NDJSON。本 crate 提供协议帧定义、
//! 请求翻译与运行循环；Host 接线（`cli-host` 的 headless 模式装配）不在本
//! crate 内，而是消费 [`translate`] 与 [`stdio`] 提供的入口。
//!
//! ## 协议约定（稳定面）
//!
//! - 每行一个 JSON 帧（UTF-8，`\n` 结尾）；单帧上限
//!   [`MAX_FRAME_BYTES`](wire::MAX_FRAME_BYTES)。
//! - 请求帧：`hello` / `command` / `query` / `compat_import` /
//!   `compat_history`（见 [`HeadlessRequest`]）。
//! - 响应帧：`hello_ack` / `response` / `event` / `compat_import_result` /
//!   `compat_history_result` / `error`（见 [`HeadlessResponse`]）。
//! - Command / Query / Event 帧**直接承载 `core-api` 信封类型**，不另造协议；
//!   帧定义复用 [`AppCommandEnvelope`]、[`AppQueryEnvelope`]、
//!   [`AppEventEnvelope`]。
//! - `compat_import` / `compat_history` 是本层定义的稳定协议入口，Host 接线
//!   映射到 `session-store` 的 compat 导入实现（P16-10 已收敛存储语义；
//!   本层不重做存储，只做协议翻译）。
//! - 所有 unknown / unsupported / malformed 情况都以显式 `error` 帧返回
//!   （[`ProtocolErrorKind`]），不静默忽略。
//! - 输出流式语义：事件/响应帧逐行写出；批量模式下由
//!   [`StdioWriter`](stdio::StdioWriter) 的待写上限提供背压。
//!
//! ## 与 GUI Connection Protocol 的边界
//!
//! 本协议与 GUI Connection Protocol 正交：GUI 帧不向本协议泄漏，反之亦然；
//! 两者都经 `app-service` 访问 Core，本协议不取代也不嵌入 GUI 通道。
//!
//! ## 模块
//!
//! - [`wire`]：协议帧与错误类型。
//! - [`translate`]：请求行翻译与响应/事件编码（纯函数）。
//! - [`stdio`]：stdin/stdout 运行循环与有界输出写入器。

pub mod json_mapping;
#[cfg(feature = "headless")]
pub mod stdio;
pub mod translate;
pub mod wire;

pub use wire::{
    CompatHistoryEntry, CompatImportOptions, CompatImportReport, CompatImportRequest, CompatSource,
    HeadlessRequest, HeadlessResponse, HelloRequest, HeadlessError, ProtocolErrorKind,
    SdkCapability, TranslatedRequest, MAX_FRAME_BYTES,
};
