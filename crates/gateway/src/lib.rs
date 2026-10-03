//! 本机 OpenAI HTTP 接入与可撤销客户端令牌。
//!
//! 不加载 AppCore、Agent Engine、存储或具体 Provider；宿主通过
//! [`GatewayBackend`] 注入模型路由、账户快照、租约和用量结算。

mod backend;
mod server;
pub mod tokens;

pub use backend::{
    GatewayBackend, GatewayChatRequest, GatewayContent, GatewayContentPart, GatewayContentUrl,
    GatewayError, GatewayMessage, GatewayModel, GatewayModelCapabilities, GatewayStreamOptions,
    GatewayVideoRequest,
};
pub use server::{gateway_is_running, serve_gateway};
pub use tokens::{GatewayTokenInfo, GatewayTokenStore};

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
