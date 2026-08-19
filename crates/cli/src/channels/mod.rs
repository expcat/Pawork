//! Pawork 外部通道。
//!
//! 本波只激活 [`acp`]。`codex` / `claude` /
//! `remote-control` 不迁入。宿主执行面经 [`AcpCommandHost`] 窄 port 注入，
//! 本 crate 不依赖 `pawork-app`。

pub mod acp;

pub use acp::{
    AcpClientAdapter, AcpClientAdapterFactory, AcpCommandHost, AcpHost, AcpHostError, CwdResolver,
    SessionResolver,
    wire::{JsonRpcMessage, PROTOCOL_VERSION},
};
