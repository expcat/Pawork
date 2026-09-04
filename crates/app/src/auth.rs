//! pawork auth 的应用操作面（S6 波 C）：凭证状态、set-key、OAuth 登录编排。
//!
//! 明文 secret 只在 SecretBackend 与短暂的栈上存在；本模块所有返回值与
//! 打印输出只含掩码与来源标注。

use std::time::Duration;

use pawork_auth::{
    exchange_pkce_code, poll_device_token, request_device_authorization,
    start_pkce_flow_with_callback, store_default_oauth_token, CallbackServer, DeviceFlowConfig,
    DeviceUserPrompt, PkceSession, StoredCredential,
};
use pawork_domain::ProviderId;

use crate::channels::{self, OAuthFlow};
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

/// 一次进行中的 OAuth 登录。
pub enum OAuthLogin {
    /// PKCE：callback server 已监听，等待浏览器回调后换 token。
    Pkce {
        provider: String,
        auth_url: String,
        session: PkceSession,
        server: CallbackServer,
    },
    /// Device Flow（RFC 8628）：已取得 device_code，轮询 token endpoint。
    Device {
        provider: String,
        config: DeviceFlowConfig,
        prompt: DeviceUserPrompt,
    },
}

impl AppCore {
    /// 首发通道 + config 自定义 provider 的凭证状态（无网络、无明文）。
    /// 双认证通道（xAI）按实际存储形态展示：先查 api key 凭证，再查 OAuth
    /// meta（SET-4 A3：显示 method 与实际凭证一致，不按 kind 猜）。
    pub fn auth_status(&self) -> Result<Vec<AuthChannelStatus>, AppError> {
        let mut rows = Vec::new();
        for channel in channels::FIRST_PARTY_CHANNELS.iter() {
            let methods = channel.auth_methods();
            if methods.contains(&"api_key") {
                match pawork_auth::resolve_provider_credential(
                    self.auth_backend().as_ref(),
                    channel.id,
                )? {
                    pawork_auth::CredentialSource::AuthFile(stored) => {
                        rows.push(AuthChannelStatus {
                            provider: channel.id.into(),
                            kind: "api-key",
                            source: AuthSource::File,
                            masked: Some(stored.masked.as_str().to_string()),
                            expires_at_ms: None,
                        });
                        continue;
                    }
                    pawork_auth::CredentialSource::EnvFallback(_) => {
                        rows.push(AuthChannelStatus {
                            provider: channel.id.into(),
                            kind: "api-key",
                            source: AuthSource::Env,
                            masked: None,
                            expires_at_ms: None,
                        });
                        continue;
                    }
                    pawork_auth::CredentialSource::None => {}
                }
            }
            if methods.contains(&"oauth") {
                let provider = ProviderId::new(channel.id);
                let meta =
                    pawork_auth::load_default_oauth_meta(self.auth_backend().as_ref(), &provider)?;
                rows.push(AuthChannelStatus {
                    provider: channel.id.into(),
                    kind: "oauth",
                    source: if meta.is_some() {
                        AuthSource::File
                    } else {
                        AuthSource::None
                    },
                    masked: meta.as_ref().map(|meta| meta.masked.as_str().to_string()),
                    expires_at_ms: meta.as_ref().and_then(|meta| meta.expires_at_ms),
                });
                continue;
            }
            rows.push(AuthChannelStatus {
                provider: channel.id.into(),
                kind: method_label(methods.first().copied()),
                source: AuthSource::None,
                masked: None,
                expires_at_ms: None,
            });
        }
        Ok(rows)
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
        let stored =
            pawork_auth::store_default_api_key(self.auth_backend().as_ref(), &provider, secret)?;
        // 替换语义（SET-4 A3）：一切换认证方式 = 替换连接；声明 oauth 的
        // 通道写入 api key 后移除旧 OAuth 条目（删除失败 fail-closed 上报）。
        if channels::first_party_channel(provider_id)
            .map(|channel| channel.auth_methods().contains(&"oauth"))
            .unwrap_or(false)
        {
            pawork_auth::delete_default_oauth_token(self.auth_backend().as_ref(), &provider)?;
        }
        Ok(stored.masked)
    }

    /// pawork auth logout：删除 default 条目（OAuth 三账户或 API key default）。
    /// env fallback 不受影响（取消导出对应 PAWORK_API_KEY_* 即可）。
    /// 双认证通道（xAI）两类条目都清理（删除幂等）。
    pub fn auth_logout(&self, provider_id: &str) -> Result<(), AppError> {
        let provider = ProviderId::new(provider_id);
        let backend = self.auth_backend();
        let methods = channels::first_party_channel(provider_id)
            .map(|channel| channel.auth_methods())
            .unwrap_or(&["api_key"]);
        if methods.contains(&"oauth") {
            pawork_auth::delete_default_oauth_token(backend.as_ref(), &provider)?;
        }
        if methods.contains(&"api_key") {
            pawork_auth::delete_default_api_key(backend.as_ref(), &provider)?;
        }
        Ok(())
    }

    /// 开始一次 OAuth 登录：PKCE 返回授权 URL（callback 已监听）；Device Flow
    /// 已请求设备码，返回 verification_uri / user_code 供用户在浏览器确认。
    pub async fn oauth_begin(&self, provider_id: &str) -> Result<OAuthLogin, AppError> {
        let preset = channels::oauth_override(self.config(), provider_id)
            .or_else(|| {
                channels::first_party_channel(provider_id).and_then(|c| c.oauth_preset())
            })
            .ok_or_else(|| {
                AppError::OAuthLogin(format!(
                    "provider {provider_id} has no OAuth endpoint preset; configure [oauth.{provider_id}] (client_id/token_url + auth_url/redirect_uri or device_auth_url) first"
                ))
            })?;
        match preset.flow {
            OAuthFlow::Pkce {
                auth_url,
                redirect_uri,
                extra_auth_params,
            } => {
                let config = pawork_auth::PkceFlowConfig {
                    client_id: preset.client_id,
                    auth_url,
                    token_url: preset.token_url,
                    redirect_uri,
                    scopes: preset.scopes,
                    provider: ProviderId::new(provider_id),
                    extra_auth_params,
                };
                let (session, server) = start_pkce_flow_with_callback(config)?;
                Ok(OAuthLogin::Pkce {
                    auth_url: session.auth_url.clone(),
                    provider: provider_id.to_string(),
                    session,
                    server,
                })
            }
            OAuthFlow::Device { device_auth_url } => {
                let config = DeviceFlowConfig {
                    client_id: preset.client_id,
                    device_auth_url,
                    token_url: preset.token_url,
                    scopes: preset.scopes,
                    provider: ProviderId::new(provider_id),
                };
                let prompt = request_device_authorization(&config, &self.http).await?;
                Ok(OAuthLogin::Device {
                    provider: provider_id.to_string(),
                    config,
                    prompt,
                })
            }
        }
    }

    /// 等待用户授权并完成 token 交换，写入 default OAuth 条目（含 meta）。
    pub async fn oauth_complete(
        &self,
        login: OAuthLogin,
        timeout: Duration,
    ) -> Result<StoredCredential, AppError> {
        oauth_finish(login, self.auth_backend().as_ref(), &self.http, timeout).await
    }
}

/// oauth_complete 的不持锁版本（SET-2 GUI 后台认证任务专用）：等待授权
/// 可能耗时数分钟，任务内不得长期持有 core 读锁阻塞写操作。
pub(crate) async fn oauth_finish(
    login: OAuthLogin,
    backend: &dyn pawork_auth::SecretBackend,
    http: &reqwest::Client,
    timeout: Duration,
) -> Result<StoredCredential, AppError> {
    let provider = match &login {
        OAuthLogin::Pkce { provider, .. } | OAuthLogin::Device { provider, .. } => provider.clone(),
    };
    let tokens = match login {
        OAuthLogin::Pkce {
            session, server, ..
        } => {
            let (code, state) = server.wait_for_code(timeout).await?;
            exchange_pkce_code(&session, &code, &state, http).await?
        }
        OAuthLogin::Device { config, prompt, .. } => {
            poll_device_token(&config, &prompt, http, timeout).await?
        }
    };
    let stored = store_default_oauth_token(backend, ProviderId::new(&provider), &tokens)?;
    // 替换语义（SET-4 A3）：OAuth 登录成功写入后移除旧 API key 条目
    //（幂等；删除失败 fail-closed 上报，不静默）。
    pawork_auth::delete_default_api_key(backend, &ProviderId::new(&provider))?;
    Ok(stored)
}

/// auth list 展示标签：api_key 方法 → api-key，其余（oauth）→ oauth。
fn method_label(method: Option<&str>) -> &'static str {
    match method {
        Some("api_key") => "api-key",
        _ => "oauth",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::OAuthFlow;
    use async_trait::async_trait;
    use pawork_auth::{load_default_oauth_credential, resolve_oauth_credential, MemoryBackend};
    use pawork_domain::{CancellationToken, ModelId};
    use pawork_domain::{
        CanonicalModelRequest, ModelDefinition, ModelProvider, ModelResponseSummary, ProviderError,
        ProviderErrorKind, ProviderEventSink,
    };
    use pawork_workspace::config::PaworkConfig;
    use serde_json::json;
    use std::sync::Arc;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct NoopProvider;

    #[async_trait]
    impl ModelProvider for NoopProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("noop")
        }

        async fn list_models(
            &self,
            _credential: Option<&pawork_domain::ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            _request: CanonicalModelRequest,
            _sink: &dyn ProviderEventSink,
            _cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "noop",
            ))
        }
    }

    fn core_with_device_override(server_uri: String) -> AppCore {
        let mut config = PaworkConfig::default();
        config.extra.insert(
            "oauth".into(),
            json!({
                "xai": {
                    "client_id": "test-client",
                    "device_auth_url": format!("{server_uri}/oauth2/device/code"),
                    "token_url": format!("{server_uri}/oauth2/token"),
                }
            }),
        );
        AppCore::from_parts(
            Arc::new(NoopProvider),
            None,
            ModelId::from("grok-4"),
            ProviderId::from("xai"),
            None,
        )
        .with_state(config, Arc::new(MemoryBackend::new()))
    }

    #[tokio::test]
    async fn xai_device_flow_login_stores_oauth_credential_and_feeds_adapter() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_code": "DEVICE-SECRET",
                "user_code": "USER-CODE",
                "verification_uri": "https://example.test/device",
                "verification_uri_complete": "https://example.test/device?user_code=USER-CODE",
                "expires_in": 600,
                "interval": 1,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .and(body_string_contains("device_code"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": "authorization_pending",
            })))
            .up_to_n_times(1)
            .expect(1)
            .named("authorization_pending once")
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "xai-access-secret",
                "refresh_token": "xai-refresh-secret",
                "expires_in": 3600,
                "token_type": "Bearer",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let core = core_with_device_override(server.uri());
        let login = core.oauth_begin("xai").await.expect("device begin");
        let OAuthLogin::Device { prompt, .. } = &login else {
            panic!("xai login must be device flow");
        };
        assert_eq!(prompt.user_code, "USER-CODE");
        assert_eq!(prompt.verification_uri, "https://example.test/device");
        assert!(!format!("{prompt:?}").contains("DEVICE-SECRET"));

        let stored = core
            .oauth_complete(login, Duration::from_secs(30))
            .await
            .expect("device complete");
        assert!(!format!("{stored:?}").contains("xai-access-secret"));
        assert!(!stored.masked.as_str().contains("xai-access-secret"));

        // 登录产物落 default 条目：auth list 标注 file 来源。
        let xai_row = core
            .auth_status()
            .expect("auth status")
            .into_iter()
            .find(|row| row.provider == "xai")
            .expect("xai row");
        assert_eq!(xai_row.kind, "oauth");
        assert_eq!(xai_row.source, AuthSource::File);
        assert!(xai_row.expires_at_ms.is_some());

        // OAuth 登录 → 解析 → XaiProvider 构造链路（fail-closed 语义见 adapter 测试）。
        let stored =
            load_default_oauth_credential(core.auth_backend().as_ref(), &ProviderId::new("xai"))
                .expect("load default")
                .expect("present");
        let credential =
            resolve_oauth_credential(&stored, core.auth_backend().as_ref()).expect("resolve");
        let provider = pawork_providers::XaiProvider::new(
            pawork_providers::XaiConfig::new("https://api.x.ai/v1"),
            Some(credential),
        )
        .expect("construct xai provider");
        assert_eq!(provider.id(), ProviderId::new("xai"));
        server.verify().await;
    }

    #[test]
    fn xai_refresh_endpoint_resolves_from_preset() {
        let preset =
            crate::provider_assembly::oauth_refresh_endpoint(&PaworkConfig::default(), "xai")
                .expect("xai preset");
        assert_eq!(preset.token_url, "https://auth.x.ai/oauth2/token");
        assert_eq!(preset.client_id, "b1a00492-073a-47ea-816f-4c329264a828");
        assert!(matches!(preset.flow, OAuthFlow::Device { .. }));
    }
}
