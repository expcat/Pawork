//! 身份认证与 Secret 管理（S6 波 B 激活：`pawork-auth`）。
//!
//! 自 V1 `auth-service` 整包迁移：Secret 后端（OS Keychain / 内存）、API Key
//! credential、全局脱敏与 OAuth（PKCE / Device Flow / auto refresh / 本地回调）
//! 原样保留；新增 [`resolve_provider_credential`] 凭证解析链（Keychain →
//! env fallback → 无凭证）。
//!
//! 正式接线（`pawork auth`、config 凭证引用、六通道装配与 `auth list` 来源
//! 标注）属 S6 波 C，由 `pawork-config` / `pawork-app` / `pawork-cli` 完成。
//!
//! ## 核心红线
//!
//! - 明文 token **绝不**进入 [`StoredCredential`] / [`ApiKeyCredential`]，只存
//!   于 [`SecretBackend`]（Keychain / 内存）中。
//! - [`MaskedCredential`] 的 `Debug` / `Display` / `Serialize` 输出永不含明文。
//! - 自动测试只用 [`MemoryBackend`]，不依赖真实 OS Keychain。

mod backend;
mod credential;
mod error;
mod masked;
pub mod oauth;
mod resolve;

pub use backend::{KeychainBackend, MemoryBackend, SecretBackend};
pub use credential::{ApiKeyCredential, CredentialId, StoredCredential};
pub use error::AuthError;
pub use masked::MaskedCredential;
pub use oauth::{
    exchange_pkce_code, needs_refresh, poll_device_token, random_state, read_refresh_token,
    refresh_access_token, refresh_oauth_credential_if_needed, request_device_authorization,
    resolve_oauth_credential, resolve_oauth_credential_for_request, start_pkce_flow,
    start_pkce_flow_with_callback, store_oauth_token, update_oauth_token, CallbackServer,
    DeviceFlowConfig, DeviceUserPrompt, OAuthRefreshConfig, Pkce, PkceFlowConfig, PkceSession,
    TokenSet,
};
pub use resolve::{resolve_provider_credential, CredentialSource};
