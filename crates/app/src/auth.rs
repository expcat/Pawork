//! pawork auth 的应用操作面（S6 波 C）：凭证状态、set-key、OAuth 登录编排。
//!
//! 明文 secret 只在 SecretBackend 与短暂的栈上存在；本模块所有返回值与
//! 打印输出只含掩码与来源标注。

use std::time::Duration;

use pawork_auth::{
    exchange_pkce_code, poll_device_token, request_device_authorization,
    start_pkce_flow_with_callback, store_default_oauth_token, AuthError, CallbackServer,
    DeviceFlowConfig, DeviceUserPrompt, PkceSession, StoredCredential,
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
    /// ADR-056 D6：双形态通道（同时声明 api_key 与 oauth，如 xAI）api key
    /// 命中后不再提前收束，继续输出 OAuth 行——两种已存凭证各占一行；
    /// 未存储的 kind 维持「声明即出行、无 meta 则 source None」口径。
    pub fn auth_status(&self) -> Result<Vec<AuthChannelStatus>, AppError> {
        let mut rows = Vec::new();
        for channel in channels::FIRST_PARTY_CHANNELS.iter() {
            let methods = channel.auth_methods();
            let inventory = pawork_auth::list_provider_accounts(
                self.auth_backend().as_ref(),
                &ProviderId::new(channel.id),
            )?;
            if !inventory.accounts.is_empty() {
                for account in &inventory.accounts {
                    rows.push(AuthChannelStatus {
                        provider: channel.id.into(),
                        kind: method_label(Some(account.kind.as_str())),
                        source: AuthSource::File,
                        masked: Some(account.stored.masked.as_str().into()),
                        expires_at_ms: account.stored.expires_at.map(|time| time.as_unix_millis()),
                    });
                }
                if methods.contains(&"api_key")
                    && !inventory
                        .accounts
                        .iter()
                        .any(|a| a.kind == pawork_auth::ProviderAccountKind::ApiKey)
                    && pawork_auth::locator::read_api_key_from_env(channel.id).is_some()
                {
                    rows.push(AuthChannelStatus {
                        provider: channel.id.into(),
                        kind: "api-key",
                        source: AuthSource::Env,
                        masked: None,
                        expires_at_ms: None,
                    });
                }
                continue;
            }

            // api key 行是否已出行；单 api_key 通道据此跳过兜底行。
            let mut api_key_row_emitted = false;
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
                        api_key_row_emitted = true;
                    }
                    pawork_auth::CredentialSource::EnvFallback(_) => {
                        rows.push(AuthChannelStatus {
                            provider: channel.id.into(),
                            kind: "api-key",
                            source: AuthSource::Env,
                            masked: None,
                            expires_at_ms: None,
                        });
                        api_key_row_emitted = true;
                    }
                    pawork_auth::CredentialSource::None => {}
                }
            }
            if methods.contains(&"oauth") {
                // 声明即出行：无 meta 时仍输出 source None 的 oauth 行。
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
            if api_key_row_emitted {
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
        // ADR-056 D1 共存语义：写入 api key 不删除该 provider 的 OAuth
        // default 条目；替换缩窄为同 kind 覆盖（store_default_api_key
        // 同账户覆盖写），跨 kind 清理只属于 auth_logout。
        Ok(stored.masked)
    }

    /// pawork auth logout：删除 default 条目（OAuth 三账户或 API key default）。
    /// env fallback 不受影响（取消导出对应 PAWORK_API_KEY_* 即可）。
    /// 双认证通道（xAI）两类条目都清理（删除幂等）。
    pub fn auth_logout(&self, provider_id: &str) -> Result<(), AppError> {
        let provider = ProviderId::new(provider_id);
        let backend = self.auth_backend();
        pawork_auth::remove_all_provider_accounts(backend.as_ref(), &provider)?;
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
                let http = crate::provider_assembly::provider_http(self.config(), provider_id)?;
                let prompt = request_device_authorization(&config, &http).await?;
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
        let provider = match &login {
            OAuthLogin::Pkce { provider, .. } | OAuthLogin::Device { provider, .. } => provider,
        };
        let http = crate::provider_assembly::provider_http(self.config(), provider)?;
        oauth_finish(login, self.auth_backend().as_ref(), &http, timeout).await
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
    let (provider, tokens) = oauth_exchange(login, http, timeout).await?;
    Ok(store_default_oauth_token(backend, provider, &tokens)?)
}

pub(crate) async fn oauth_exchange(
    login: OAuthLogin,
    http: &reqwest::Client,
    timeout: Duration,
) -> Result<(ProviderId, pawork_auth::TokenSet), AppError> {
    let provider = match &login {
        OAuthLogin::Pkce { provider, .. } | OAuthLogin::Device { provider, .. } => {
            ProviderId::new(provider)
        }
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
    Ok((provider, tokens))
}

/// Single effective account policy for assembly, Settings and request refresh.
/// None denotes env fallback or no stored credential, never an invented account.
pub(crate) fn effective_provider_account(
    backend: &dyn pawork_auth::SecretBackend,
    provider: &ProviderId,
) -> Result<Option<pawork_auth::ProviderAccount>, AppError> {
    let inventory = pawork_auth::list_provider_accounts(backend, provider)?;
    let methods = channels::first_party_channel(provider.as_str())
        .map(|c| c.auth_methods())
        .unwrap_or(&["api_key"]);
    if let Some(account) = inventory.selected() {
        if !methods.contains(&account.kind.as_str()) {
            return Err(AppError::Auth(AuthError::MalformedMetadata(
                "selected account kind is unsupported by provider".into(),
            )));
        }
        return Ok(Some(account.clone()));
    }
    if methods.contains(&"api_key") {
        if let Some(account) = inventory
            .accounts
            .iter()
            .find(|a| a.credential_id == pawork_auth::LEGACY_API_KEY_ID)
        {
            return Ok(Some(account.clone()));
        }
        if pawork_auth::locator::read_api_key_from_env(provider.as_str()).is_some() {
            return Ok(None);
        }
    }
    Ok(inventory
        .accounts
        .into_iter()
        .find(|a| methods.contains(&"oauth") && a.credential_id == pawork_auth::LEGACY_OAUTH_ID))
}

pub(crate) fn activate_first_account(provider: &str) -> bool {
    let uses_key = channels::first_party_channel(provider)
        .is_none_or(|c| c.auth_methods().contains(&"api_key"));
    !uses_key || pawork_auth::locator::read_api_key_from_env(provider).is_none()
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

    fn core_with_backend(backend: Arc<MemoryBackend>) -> AppCore {
        AppCore::from_parts(
            Arc::new(NoopProvider),
            None,
            ModelId::from("grok-4"),
            ProviderId::from("xai"),
            None,
        )
        .with_state(PaworkConfig::default(), backend)
    }

    /// 直接构造 Device 登录（跳过设备码端点），供 oauth_finish 单测驱动。
    fn device_login(token_url: String) -> OAuthLogin {
        OAuthLogin::Device {
            provider: "xai".into(),
            config: DeviceFlowConfig {
                client_id: "test-client".into(),
                device_auth_url: format!("{token_url}/device"),
                token_url,
                scopes: Vec::new(),
                provider: ProviderId::new("xai"),
            },
            prompt: DeviceUserPrompt {
                user_code: "USER-CODE".into(),
                verification_uri: "https://example.test/device".into(),
                verification_uri_complete: None,
                device_code: "DEVICE-SECRET".into(),
                expires_in: 600,
                interval: 1,
            },
        }
    }

    /// ADR-056 D1：双形态通道（xAI）api key 与 OAuth default 条目共存
    /// roundtrip——set key 后 OAuth meta 仍在；再次 OAuth 写入后 api key 仍在。
    #[tokio::test]
    async fn xai_api_key_and_oauth_default_entries_coexist() {
        let server = MockServer::start().await;
        crate::testsupport::token_mock(
            "/oauth2/token",
            ResponseTemplate::new(200).set_body_json(crate::testsupport::token_success_json(
                "xai-access-secret",
                Some("xai-refresh-secret"),
                None,
            )),
        )
            .expect(2)
        .mount(&server)
        .await;
        let backend = Arc::new(MemoryBackend::new());
        let http = pawork_auth::http_client().expect("http client");
        let provider = ProviderId::new("xai");

        oauth_finish(
            device_login(format!("{}/oauth2/token", server.uri())),
            backend.as_ref(),
            &http,
            Duration::from_secs(30),
        )
        .await
        .expect("first oauth finish");
        assert!(
            pawork_auth::load_default_oauth_meta(backend.as_ref(), &provider)
                .expect("load meta")
                .is_some()
        );

        // set key 不再跨删 OAuth default 条目（同 kind 覆盖由 store 自身保证）。
        let core = core_with_backend(backend.clone());
        core.auth_set_key("xai", "xai-coexist-key-00000001")
            .expect("set key");
        assert!(
            pawork_auth::load_default_oauth_meta(backend.as_ref(), &provider)
                .expect("load meta after set key")
                .is_some(),
            "oauth meta must survive api key write"
        );

        // 再次 OAuth 写入同样不跨删 api key default 条目。
        oauth_finish(
            device_login(format!("{}/oauth2/token", server.uri())),
            backend.as_ref(),
            &http,
            Duration::from_secs(30),
        )
        .await
        .expect("second oauth finish");
        assert!(
            matches!(
                pawork_auth::resolve_provider_credential(backend.as_ref(), "xai")
                    .expect("resolve after oauth finish"),
                pawork_auth::CredentialSource::AuthFile(_)
            ),
            "api key must survive oauth write"
        );
        assert!(
            pawork_auth::load_default_oauth_meta(backend.as_ref(), &provider)
                .expect("load meta after second finish")
                .is_some()
        );
        server.verify().await;
    }

    /// ADR-056 D6：双形态通道两种已存凭证各占一行（api-key 行在前）。
    #[test]
    fn auth_status_lists_both_rows_for_dual_form_channel() {
        let backend = Arc::new(MemoryBackend::new());
        let provider = ProviderId::new("xai");
        pawork_auth::store_default_api_key(backend.as_ref(), &provider, "xai-stored-key-00000001")
            .expect("seed api key");
        pawork_auth::store_default_oauth_token(
            backend.as_ref(),
            provider,
            &pawork_auth::TokenSet {
                access_token: "xai-oauth-access-0001".into(),
                refresh_token: Some("xai-oauth-refresh-0001".into()),
                id_token: None,
                expires_in: Some(3600),
                token_type: "Bearer".into(),
                scope: None,
            },
        )
        .expect("seed oauth token");

        let xai_rows: Vec<AuthChannelStatus> = core_with_backend(backend)
            .auth_status()
            .expect("auth status")
            .into_iter()
            .filter(|row| row.provider == "xai")
            .collect();
        assert_eq!(xai_rows.len(), 2, "dual-form channel emits two rows");
        assert_eq!(xai_rows[0].kind, "api-key");
        assert_eq!(xai_rows[0].source, AuthSource::File);
        assert!(xai_rows[0].masked.is_some());
        assert_eq!(xai_rows[1].kind, "oauth");
        assert_eq!(xai_rows[1].source, AuthSource::File);
        assert!(xai_rows[1].masked.is_some());
        assert!(xai_rows[1].expires_at_ms.is_some());
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
            .respond_with(ResponseTemplate::new(400).set_body_json(
                crate::testsupport::token_error_json("authorization_pending", None)))
            .up_to_n_times(1)
            .expect(1)
            .named("authorization_pending once")
            .mount(&server)
            .await;
        crate::testsupport::token_mock(
            "/oauth2/token",
            ResponseTemplate::new(200).set_body_json(crate::testsupport::token_success_json(
                "xai-access-secret",
                Some("xai-refresh-secret"),
                None,
            )),
        )
            .expect(1)
        .mount(&server)
        .await;

        let mut core = core_with_device_override(server.uri());
        core.config.proxy_url = Some("http://[invalid-proxy".into());
        core.set_provider_use_proxy("xai", false);
        // 旧共享客户端故意指向不可达代理；开始与完成均须按 provider 重建。
        core.http = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:1").expect("proxy"))
            .build()
            .expect("http client");
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
