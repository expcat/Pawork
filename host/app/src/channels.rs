//! 首发通道装配表（S6 波 C）：装配层的数据驱动 Provider 选择。
//!
//! 这是 host 装配代码，不是 Engine 分支——Engine 仍只消费
//! `ModelProvider` trait；本表只回答「这个 provider id 用哪个 adapter、
//! 哪种凭证 kind、默认 endpoint 是什么」。默认 endpoint 可被 config 的
//! `[[providers]] base_url` 覆盖；OAuth 端点可被 `[oauth.<id>]` 覆盖。


/// 首发通道凭证与 adapter 形态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    /// 四条 API-key 通道（复用 OpenAI-compatible transport，可逐模型切 Responses）。
    ApiKey,
    /// ChatGPT OAuth（Responses transport）。
    ChatGptOAuth,
    /// xAI Grok OAuth（按模型 capability 选 Chat/Responses）。
    XaiOAuth,
}

/// OAuth 授权流形态（预设与 config `[oauth.<id>]` 覆盖共用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthFlow {
    /// Authorization Code + PKCE（浏览器授权 + 本地回调；ChatGPT）。
    Pkce {
        auth_url: String,
        redirect_uri: String,
        /// 授权 URL 附加参数（如 ChatGPT 的 `codex_cli_simplified_flow`）。
        extra_auth_params: Vec<(String, String)>,
    },
    /// Device Flow（RFC 8628；xAI）。
    Device {
        device_auth_url: String,
    },
}

/// OAuth 端点预设。ChatGPT 使用 Codex 公开 client 参数；xAI 使用 auth.x.ai
/// 公开 Device Flow 端点与 grok-cli 公共 client。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthPreset {
    pub client_id: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub flow: OAuthFlow,
}

/// 一条首发通道的装配元数据。
#[derive(Clone, Debug)]
pub struct FirstPartyChannel {
    pub id: &'static str,
    pub kind: ChannelKind,
    pub default_base_url: &'static str,
}

impl FirstPartyChannel {
    pub fn oauth_preset(&self) -> Option<OAuthPreset> {
        match self.kind {
            ChannelKind::ChatGptOAuth => Some(OAuthPreset {
                client_id: "app_EMoamEEZ73f0CkXaXp7hrann".into(),
                token_url: "https://auth.openai.com/oauth/token".into(),
                scopes: vec![
                    "openid".into(),
                    "profile".into(),
                    "email".into(),
                    "offline_access".into(),
                    // 上游 Codex CLI 同款 scope：connectors 权限是后端把
                    // 会话识别为 Codex 会话的一部分，缺失时 /models 返回空。
                    "api.connectors.read".into(),
                    "api.connectors.invoke".into(),
                ],
                flow: OAuthFlow::Pkce {
                    auth_url: "https://auth.openai.com/oauth/authorize".into(),
                    // redirect URI 必须与 Codex CLI 的 Hydra allow-list 精确匹配：
                    // host 固定 localhost、path 固定 /auth/callback（端口 1455）。
                    redirect_uri: "http://localhost:1455/auth/callback".into(),
                    extra_auth_params: vec![
                        ("id_token_add_organizations".into(), "true".into()),
                        ("codex_cli_simplified_flow".into(), "true".into()),
                    ],
                },
            }),
            // xAI Device Flow（RFC 8628）：端点与公共 client 与上游 grok CLI、
            // cc-switch 等第三方实现一致；仍可用 config `[oauth.xai]` 覆盖。
            ChannelKind::XaiOAuth => Some(OAuthPreset {
                client_id: "b1a00492-073a-47ea-816f-4c329264a828".into(),
                token_url: "https://auth.x.ai/oauth2/token".into(),
                scopes: vec![
                    "openid".into(),
                    "profile".into(),
                    "email".into(),
                    "offline_access".into(),
                    // grok-cli:access 是订阅级 CLI 推理访问；api:access 覆盖
                    // api.x.ai REST 调用（xAI 官方文档的 Agentic CLI scope 组）。
                    "grok-cli:access".into(),
                    "api:access".into(),
                ],
                flow: OAuthFlow::Device {
                    device_auth_url: "https://auth.x.ai/oauth2/device/code".into(),
                },
            }),
            ChannelKind::ApiKey => None,
        }
    }
}

/// 六条首发通道（顺序即 `pawork models` / `auth list` 展示顺序）。
pub const FIRST_PARTY_CHANNELS: &[FirstPartyChannel] = &[
    FirstPartyChannel {
        id: "chatgpt",
        kind: ChannelKind::ChatGptOAuth,
        default_base_url: "https://chatgpt.com/backend-api/codex",
    },
    FirstPartyChannel {
        id: "xai",
        kind: ChannelKind::XaiOAuth,
        default_base_url: "https://api.x.ai/v1",
    },
    FirstPartyChannel {
        id: "glm-coding",
        kind: ChannelKind::ApiKey,
        default_base_url: "https://api.z.ai/api/coding/paas/v4",
    },
    FirstPartyChannel {
        id: "opencode-go",
        kind: ChannelKind::ApiKey,
        default_base_url: "https://opencode.ai/zen/go/v1",
    },
    FirstPartyChannel {
        id: "qwen-token-plan",
        kind: ChannelKind::ApiKey,
        default_base_url: "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
    },
    FirstPartyChannel {
        id: "deepseek",
        kind: ChannelKind::ApiKey,
        default_base_url: "https://api.deepseek.com",
    },
];

pub fn first_party_channel(id: &str) -> Option<&'static FirstPartyChannel> {
    FIRST_PARTY_CHANNELS.iter().find(|channel| channel.id == id)
}

pub fn is_first_party(id: &str) -> bool {
    first_party_channel(id).is_some()
}

/// 该 id 对应的 ApiKeyChannel 枚举（仅在对应 feature 启用时可用）。
pub fn api_key_channel(id: &str) -> Option<pawork_providers::ApiKeyChannel> {
    match id {
        "glm-coding" => Some(pawork_providers::ApiKeyChannel::GlmCoding),
        "opencode-go" => Some(pawork_providers::ApiKeyChannel::OpenCodeGo),
        "qwen-token-plan" => Some(pawork_providers::ApiKeyChannel::QwenTokenPlan),
        "deepseek" => Some(pawork_providers::ApiKeyChannel::DeepSeek),
        _ => None,
    }
}

/// config `[oauth.<id>]` 覆盖预设；返回 None 表示「必须配置但缺失」或 id 非OAuth。
///
/// Device Flow 只需 `device_auth_url`；PKCE 需要 `auth_url` + `redirect_uri`。
/// 两者同时提供时 device 优先（Device Flow 无回调端口要求）。
pub fn oauth_override(config: &pawork_config::PaworkConfig, id: &str) -> Option<OAuthPreset> {
    let table = config.extra.get("oauth")?.get(id)?;
    let string_field = |key: &str| -> Option<String> {
        table.get(key).and_then(|value| value.as_str()).map(String::from)
    };
    let client_id = string_field("client_id")?;
    let token_url = string_field("token_url")?;
    let scopes = table
        .get("scopes")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let flow = if let Some(device_auth_url) = string_field("device_auth_url") {
        OAuthFlow::Device { device_auth_url }
    } else {
        OAuthFlow::Pkce {
            auth_url: string_field("auth_url")?,
            redirect_uri: string_field("redirect_uri")?,
            extra_auth_params: Vec::new(),
        }
    };
    Some(OAuthPreset {
        client_id,
        token_url,
        scopes,
        flow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_config::PaworkConfig;
    use serde_json::json;

    #[test]
    fn xai_preset_is_device_flow_with_public_endpoints() {
        let preset = first_party_channel("xai")
            .and_then(|channel| channel.oauth_preset())
            .expect("xai preset");
        assert_eq!(preset.client_id, "b1a00492-073a-47ea-816f-4c329264a828");
        assert_eq!(preset.token_url, "https://auth.x.ai/oauth2/token");
        assert_eq!(
            preset.flow,
            OAuthFlow::Device {
                device_auth_url: "https://auth.x.ai/oauth2/device/code".into()
            }
        );
        assert_eq!(
            preset.scopes,
            vec![
                "openid",
                "profile",
                "email",
                "offline_access",
                "grok-cli:access",
                "api:access",
            ]
        );
    }

    #[test]
    fn chatgpt_preset_stays_pkce() {
        let preset = first_party_channel("chatgpt")
            .and_then(|channel| channel.oauth_preset())
            .expect("chatgpt preset");
        assert!(matches!(preset.flow, OAuthFlow::Pkce { .. }));
    }

    #[test]
    fn oauth_override_supports_device_flow_fields() {
        let mut config = PaworkConfig::default();
        config.extra.insert(
            "oauth".into(),
            json!({
                "xai": {
                    "client_id": "custom-client",
                    "device_auth_url": "https://auth.example.test/device/code",
                    "token_url": "https://auth.example.test/token",
                    "scopes": ["openid", "api:access"],
                }
            }),
        );
        let preset = oauth_override(&config, "xai").expect("device override");
        assert_eq!(preset.client_id, "custom-client");
        assert_eq!(preset.scopes, vec!["openid", "api:access"]);
        assert_eq!(
            preset.flow,
            OAuthFlow::Device {
                device_auth_url: "https://auth.example.test/device/code".into()
            }
        );
    }

    #[test]
    fn oauth_override_pkce_still_requires_auth_url_and_redirect() {
        let mut config = PaworkConfig::default();
        config.extra.insert(
            "oauth".into(),
            json!({
                "chatgpt": {
                    "client_id": "c",
                    "token_url": "https://example.test/token",
                }
            }),
        );
        assert!(oauth_override(&config, "chatgpt").is_none());

        config.extra.insert(
            "oauth".into(),
            json!({
                "chatgpt": {
                    "client_id": "c",
                    "auth_url": "https://example.test/authorize",
                    "token_url": "https://example.test/token",
                    "redirect_uri": "http://localhost:1455/auth/callback",
                }
            }),
        );
        let preset = oauth_override(&config, "chatgpt").expect("pkce override");
        assert!(matches!(preset.flow, OAuthFlow::Pkce { .. }));
    }
}
