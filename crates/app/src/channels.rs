//! 首发通道装配 facade（S6 波 C → R5 波 A 轨 b）：转发 providers 单点注册表。
//!
//! 这是 host 装配代码，不是 Engine 分支——Engine 仍只消费
//! ModelProvider trait；本表只回答「这个 provider id 用哪个 adapter、
//! 哪种凭证 kind、默认 endpoint 是什么」。默认 endpoint 可被 config 的
//! [[providers]] base_url 覆盖；OAuth 端点可被 [oauth.<id>] 覆盖。
//!
//! 通道数据自 R5 波 A 起单点登记在 pawork-providers 的 CHANNEL_REGISTRY；
//! 本模块保留 app 公开名（FIRST_PARTY_CHANNELS 等）与 config 叠加逻辑
//! （oauth_override 依赖 workspace config，providers 不得反向依赖）。

use std::sync::LazyLock;

pub use pawork_providers::channels::registry::{ChannelKind, OAuthFlow, OAuthPreset};

use pawork_providers::channels::registry::{channel_preset, ChannelPreset};

/// 一条首发通道的装配元数据（由 providers CHANNEL_REGISTRY 派生）。
#[derive(Clone, Debug)]
pub struct FirstPartyChannel {
    pub id: &'static str,
    pub kind: ChannelKind,
    pub default_base_url: &'static str,
    preset: &'static ChannelPreset,
}

impl FirstPartyChannel {
    pub fn oauth_preset(&self) -> Option<OAuthPreset> {
        self.preset.oauth_preset()
    }
}

/// 六条首发通道（顺序即 pawork models / auth list 展示顺序；
/// 与 providers CHANNEL_REGISTRY 单点同源派生）。
pub static FIRST_PARTY_CHANNELS: LazyLock<Vec<FirstPartyChannel>> = LazyLock::new(|| {
    pawork_providers::CHANNEL_REGISTRY
        .iter()
        .map(|preset| FirstPartyChannel {
            id: preset.id,
            kind: preset.kind,
            default_base_url: preset.default_base_url,
            preset,
        })
        .collect()
});

pub fn first_party_channel(id: &str) -> Option<&'static FirstPartyChannel> {
    FIRST_PARTY_CHANNELS.iter().find(|channel| channel.id == id)
}

pub fn is_first_party(id: &str) -> bool {
    first_party_channel(id).is_some()
}

/// 该 id 对应的 API-key 通道 preset（仅注册表内 kind == ApiKey 的行；
/// feature 门由装配层用 is_enabled fail-closed 判定）。
pub fn api_key_channel(id: &str) -> Option<&'static ChannelPreset> {
    let preset = channel_preset(id)?;
    (preset.kind == ChannelKind::ApiKey).then_some(preset)
}

/// config [oauth.<id>] 覆盖预设；返回 None 表示「必须配置但缺失」或 id 非OAuth。
///
/// Device Flow 只需 device_auth_url；PKCE 需要 auth_url + redirect_uri。
/// 两者同时提供时 device 优先（Device Flow 无回调端口要求）。
pub fn oauth_override(config: &pawork_workspace::config::PaworkConfig, id: &str) -> Option<OAuthPreset> {
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
    use pawork_workspace::config::PaworkConfig;
    use serde_json::json;

    #[test]
    fn first_party_channels_derive_from_provider_registry() {
        let ids: Vec<&str> = FIRST_PARTY_CHANNELS.iter().map(|channel| channel.id).collect();
        assert_eq!(
            ids,
            [
                "chatgpt",
                "xai",
                "glm-coding",
                "opencode-go",
                "qwen-token-plan",
                "deepseek",
            ]
        );
        for channel in FIRST_PARTY_CHANNELS.iter() {
            let preset = channel_preset(channel.id).expect("registry row");
            assert_eq!(channel.default_base_url, preset.default_base_url);
        }
    }

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
