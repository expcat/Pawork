//! 身份认证与 Secret 管理（P2-6）。
//!
//! 提供 API Key 认证方式与 OS Keychain 后端：明文 token 只存于 OS Keychain
//! （或内存），不写入数据库与日志。SQLite 仅记录 Credential 元数据与脱敏状态。
//!
//! ## 核心红线
//!
//! - 明文 token **绝不**进入 [`StoredCredential`] / [`ApiKeyCredential`]，只存在
//!   于 [`SecretBackend`]（Keychain / 内存）中。
//! - [`MaskedCredential`] 的 `Debug` / `Display` / `Serialize` 输出永不含明文。
//! - 自动测试只用 [`MemoryBackend`]，不依赖真实 OS Keychain。

mod backend;
mod credential;
mod error;
mod masked;
pub mod oauth;

pub use backend::{KeychainBackend, MemoryBackend, SecretBackend};
pub use credential::{ApiKeyCredential, CredentialId, StoredCredential};
pub use error::AuthError;
pub use masked::MaskedCredential;
pub use oauth::{
    exchange_pkce_code, needs_refresh, poll_device_token, random_state, read_refresh_token,
    refresh_access_token, request_device_authorization, resolve_oauth_credential, start_pkce_flow,
    store_oauth_token, CallbackServer, DeviceFlowConfig, DeviceUserPrompt, Pkce, PkceFlowConfig,
    PkceSession, TokenSet,
};
