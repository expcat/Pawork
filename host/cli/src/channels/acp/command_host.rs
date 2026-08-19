//! ACP 宿主对 Core 执行面的窄 port。
//!
//! 禁止依赖 `pawork-app::EventHub`：事件只经 [`AcpCommandHost::subscribe`]
//! 扇出。审计 / 控制面留给 S11，本层不做。

use async_trait::async_trait;
use pawork_protocol::{AppCommandEnvelope, AppEventEnvelope, AppQueryEnvelope, AppResponseEnvelope};
use thiserror::Error;

/// ACP 通道调用 Core 失败。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AcpHostError {
    #[error("ACP command host unavailable: {0}")]
    Unavailable(String),
}

/// ACP Host 对命令 / 查询 / 事件订阅的唯一执行入口。
#[async_trait]
pub trait AcpCommandHost: Send + Sync {
    async fn dispatch(
        &self,
        command: AppCommandEnvelope,
    ) -> Result<AppResponseEnvelope, AcpHostError>;
    async fn query(&self, query: AppQueryEnvelope) -> Result<AppResponseEnvelope, AcpHostError>;
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AppEventEnvelope>;
}
