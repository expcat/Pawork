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

/// OAuth 端点预设（PKCE）。ChatGPT 使用 Codex 公开 client 参数；xAI 无公开
/// 稳定端点，必须由 config `[oauth.xai]` 提供后才能登录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthPreset {
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
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
                auth_url: "https://auth.openai.com/authorize".into(),
                token_url: "https://auth.openai.com/oauth/token".into(),
                redirect_uri: "http://localhost:1455/auth".into(),
                scopes: vec![
                    "openid".into(),
                    "profile".into(),
                    "email".into(),
                    "offline_access".into(),
                ],
            }),
            // xAI 没有公开稳定 OAuth 端点：login 必须由 config 提供（fail-closed）。
            ChannelKind::XaiOAuth => None,
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
pub fn oauth_override(config: &pawork_config::PaworkConfig, id: &str) -> Option<OAuthPreset> {
    let table = config.extra.get("oauth")?.get(id)?;
    let string_field = |key: &str| -> Option<String> {
        table.get(key).and_then(|value| value.as_str()).map(String::from)
    };
    let client_id = string_field("client_id")?;
    let auth_url = string_field("auth_url")?;
    let token_url = string_field("token_url")?;
    let redirect_uri = string_field("redirect_uri")?;
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
    Some(OAuthPreset {
        client_id,
        auth_url,
        token_url,
        redirect_uri,
        scopes,
    })
}
