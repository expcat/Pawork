//! pawork auth 的应用操作面（S6 波 C）：凭证状态、set-key、OAuth 登录编排。
//!
//! 明文 secret 只在 SecretBackend 与短暂的栈上存在；本模块所有返回值与
//! 打印输出只含掩码与来源标注。

use std::time::Duration;

use pawork_auth::{
    exchange_pkce_code, start_pkce_flow_with_callback, store_default_oauth_token,
    CallbackServer, PkceSession, StoredCredential,
};
use pawork_domain::ProviderId;

use crate::channels::{self, ChannelKind};
use crate::{AppCore, AppError};

/// 凭证来源标注（auth list 展示；不含任何明文）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthSource {
    File,
    Env,
    None,
}

impl AuthSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Env => "env",
            Self::None => "none",
        }
    }
}

/// 一个通道的凭证状态行。
#[derive(Clone, Debug)]
pub struct AuthChannelStatus {
    pub provider: String,
    pub kind: &'static str,
    pub source: AuthSource,
    /// 存储命中时的掩码展示；env/none 不展示（绝不打印明文或部分明文）。
    pub masked: Option<String>,
    pub expires_at_ms: Option<u64>,
}

/// 一次进行中的 OAuth PKCE 登录（callback server 已监听）。
pub struct OAuthLogin {
    pub auth_url: String,
    pub provider: String,
    session: PkceSession,
    server: CallbackServer,
}

impl AppCore {
    /// 六通道 + config 自定义 provider 的凭证状态（无网络、无明文）。
    pub fn auth_status(&self) -> Vec<AuthChannelStatus> {
        let mut rows = Vec::new();
        for channel in channels::FIRST_PARTY_CHANNELS {
            let kind = match channel.kind {
                ChannelKind::ApiKey => "api-key",
                ChannelKind::ChatGptOAuth | ChannelKind::XaiOAuth => "oauth",
            };
            match channel.kind {
                ChannelKind::ApiKey => {
                    let source = match pawork_auth::resolve_provider_credential(
                        self.auth_backend().as_ref(),
                        channel.id,
                    ) {
                        pawork_auth::CredentialSource::Keychain(stored) => AuthChannelStatus {
                            provider: channel.id.into(),
                            kind,
                            source: AuthSource::File,
                            masked: Some(stored.masked.as_str().to_string()),
                            expires_at_ms: None,
                        },
                        pawork_auth::CredentialSource::EnvFallback(_) => AuthChannelStatus {
                            provider: channel.id.into(),
                            kind,
                            source: AuthSource::Env,
                            masked: None,
                            expires_at_ms: None,
                        },
                        pawork_auth::CredentialSource::None => AuthChannelStatus {
                            provider: channel.id.into(),
                            kind,
                            source: AuthSource::None,
                            masked: None,
                            expires_at_ms: None,
                        },
                    };
                    rows.push(source);
                }
                ChannelKind::ChatGptOAuth | ChannelKind::XaiOAuth => {
                    let provider = ProviderId::new(channel.id);
                    let meta = pawork_auth::load_default_oauth_meta(
                        self.auth_backend().as_ref(),
                        &provider,
                    )
                    .ok()
                    .flatten();
                    rows.push(AuthChannelStatus {
                        provider: channel.id.into(),
                        kind,
                        source: if meta.is_some() {
                            AuthSource::File
                        } else {
                            AuthSource::None
                        },
                        masked: meta
                            .as_ref()
                            .map(|meta| meta.masked.as_str().to_string()),
                        expires_at_ms: meta.as_ref().and_then(|meta| meta.expires_at_ms),
                    });
                }
            }
        }
        rows
    }

    /// pawork auth set-key：明文从 stdin 读入后立即写 auth 文件（0600），不回显、不落日志。
    pub fn auth_set_key(
        &self,
        provider_id: &str,
        secret: &str,
    ) -> Result<pawork_auth::MaskedCredential, AppError> {
        let secret = secret.trim();
        if secret.is_empty() {
            return Err(AppError::Auth(pawork_auth::AuthError::InvalidSecret(
                "API key is empty".into(),
            )));
        }
        let provider = ProviderId::new(provider_id);
        let stored = pawork_auth::store_default_api_key(
            self.auth_backend().as_ref(),
            &provider,
            secret,
        )?;
        Ok(stored.masked)
    }

    /// pawork auth logout：删除 default 条目（OAuth 三账户或 API key default）。
    /// env fallback 不受影响（取消导出对应 PAWORK_API_KEY_* 即可）。
    pub fn auth_logout(&self, provider_id: &str) -> Result<(), AppError> {
        let provider = ProviderId::new(provider_id);
        let backend = self.auth_backend();
        match channels::first_party_channel(provider_id).map(|c| c.kind.clone()) {
            Some(ChannelKind::ChatGptOAuth) | Some(ChannelKind::XaiOAuth) => {
                pawork_auth::delete_default_oauth_token(backend.as_ref(), &provider)?;
            }
            _ => {
                pawork_auth::delete_default_api_key(backend.as_ref(), &provider)?;
            }
        }
        Ok(())
    }

    /// 开始一次 OAuth PKCE 登录：返回授权 URL（打印给用户），callback 已监听。
    pub fn oauth_begin(&self, provider_id: &str) -> Result<OAuthLogin, AppError> {
        let preset = channels::oauth_override(self.config(), provider_id)
            .or_else(|| {
                channels::first_party_channel(provider_id).and_then(|c| c.oauth_preset())
            })
            .ok_or_else(|| {
                AppError::OAuthLogin(format!(
                    "provider {provider_id} has no OAuth endpoint preset; configure [oauth.{provider_id}] (client_id/auth_url/token_url/redirect_uri) first"
                ))
            })?;
        let config = pawork_auth::PkceFlowConfig {
            client_id: preset.client_id,
            auth_url: preset.auth_url,
            token_url: preset.token_url,
            redirect_uri: preset.redirect_uri,
            scopes: preset.scopes,
            provider: ProviderId::new(provider_id),
            extra_auth_params: preset.extra_auth_params,
        };
        let (session, server) = start_pkce_flow_with_callback(config)?;
        Ok(OAuthLogin {
            auth_url: session.auth_url.clone(),
            provider: provider_id.to_string(),
            session,
            server,
        })
    }

    /// 等待回调并完成 token 交换，写入 default OAuth 条目（含 meta）。
    pub async fn oauth_complete(
        &self,
        login: OAuthLogin,
        timeout: Duration,
    ) -> Result<StoredCredential, AppError> {
        let (code, state) = login.server.wait_for_code(timeout).await?;
        let tokens = exchange_pkce_code(&login.session, &code, &state, &self.http).await?;
        let stored = store_default_oauth_token(
            self.auth_backend().as_ref(),
            ProviderId::new(&login.provider),
            &tokens,
        )?;
        Ok(stored)
    }

}
