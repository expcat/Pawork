//! 首发通道静态注册表（R5 波 A 轨 b）：通道 preset 数据化单点登记。
//!
//! 注册表是纯数据：一行 = 一条首发通道（id / 凭证形态 / 默认 endpoint /
//! feature 门 / OAuth 预设）。行本身不带 cfg——pawork models 的六行语义
//! 由数据承载；feature 是否启用由 is_enabled 在唯一的 cfg 求值点判定
//! （fail-closed）。app 层的通道 facade 由本注册表派生；新增通道 = 加一行。

/// 首发通道凭证与 adapter 形态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    /// 四条 API-key 通道（复用 OpenAI-compatible transport，可逐模型切 Responses）。
    ApiKey,
    /// ChatGPT OAuth（Responses transport）。
    ChatGptOAuth,
    /// xAI Grok OAuth（按模型 capability 选 Chat/Responses）。
    XaiOAuth,
    /// Kimi Code OAuth（固定 Chat Completions）。
    KimiOAuth,
}

/// OAuth 授权流形态（预设与 config [oauth.<id>] 覆盖共用；运行期 String 形态）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthFlow {
    /// Authorization Code + PKCE（浏览器授权 + 本地回调；ChatGPT）。
    Pkce {
        auth_url: String,
        redirect_uri: String,
        /// 授权 URL 附加参数（如 ChatGPT 的 codex_cli_simplified_flow）。
        extra_auth_params: Vec<(String, String)>,
    },
    /// Device Flow（RFC 8628；xAI）。
    Device { device_auth_url: String },
}

/// OAuth 端点预设（运行期 String 形态）。ChatGPT 使用 Codex 公开 client 参数；
/// xAI 使用 auth.x.ai 公开 Device Flow 端点与 grok-cli 公共 client。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthPreset {
    pub client_id: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub flow: OAuthFlow,
}

/// 注册表内的 const 友好 OAuth 数据镜像（&'static str 形态）。
///
/// static 初始化不允许堆分配，因此注册表行存本镜像；运行期经
/// OAuthPresetData::to_preset 转成与 config 覆盖共用的 OAuthPreset。
#[derive(Clone, Copy, Debug)]
pub enum OAuthFlowData {
    Pkce {
        auth_url: &'static str,
        redirect_uri: &'static str,
        extra_auth_params: &'static [(&'static str, &'static str)],
    },
    Device {
        device_auth_url: &'static str,
    },
}

/// 注册表内的 const 友好 OAuth 预设镜像。
#[derive(Clone, Copy, Debug)]
pub struct OAuthPresetData {
    pub client_id: &'static str,
    pub token_url: &'static str,
    pub scopes: &'static [&'static str],
    pub flow: OAuthFlowData,
}

impl OAuthPresetData {
    /// 转运行期 String 形态（与 [oauth.<id>] 覆盖叠加共用同一形状）。
    pub fn to_preset(&self) -> OAuthPreset {
        OAuthPreset {
            client_id: self.client_id.into(),
            token_url: self.token_url.into(),
            scopes: self.scopes.iter().map(|scope| (*scope).into()).collect(),
            flow: match self.flow {
                OAuthFlowData::Pkce {
                    auth_url,
                    redirect_uri,
                    extra_auth_params,
                } => OAuthFlow::Pkce {
                    auth_url: auth_url.into(),
                    redirect_uri: redirect_uri.into(),
                    extra_auth_params: extra_auth_params
                        .iter()
                        .map(|(key, value)| ((*key).into(), (*value).into()))
                        .collect(),
                },
                OAuthFlowData::Device { device_auth_url } => OAuthFlow::Device {
                    device_auth_url: device_auth_url.into(),
                },
            },
        }
    }
}

/// 一条首发通道的装配元数据（纯数据行；feature 是数据，不是 cfg）。
#[derive(Clone, Copy, Debug)]
pub struct ChannelPreset {
    pub id: &'static str,
    pub kind: ChannelKind,
    pub default_base_url: &'static str,
    /// Settings 面板展示名（Host 声明，Desktop 不按品牌硬编码）。
    pub display_name: &'static str,
    pub feature: &'static str,
    pub oauth: Option<OAuthPresetData>,
    /// Host 声明的可用认证方法（构造期纯数据；SET-4 起不再按 kind 派生，
    /// xAI 同时声明 oauth 与 api_key）。
    pub auth_methods: &'static [&'static str],
}

impl ChannelPreset {
    /// OAuth 端点预设（String 形态；API-key 通道返回 None）。
    pub fn oauth_preset(&self) -> Option<OAuthPreset> {
        self.oauth.map(|data| data.to_preset())
    }
}

/// 首发通道（顺序即 pawork models / auth list 展示顺序；SET-4 起为八行）。
pub static CHANNEL_REGISTRY: &[ChannelPreset] = &[
    ChannelPreset {
        id: "chatgpt",
        kind: ChannelKind::ChatGptOAuth,
        default_base_url: "https://chatgpt.com/backend-api/codex",
        display_name: "ChatGPT",
        feature: "chatgpt-oauth",
        auth_methods: &["oauth"],
        oauth: Some(OAuthPresetData {
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
            token_url: "https://auth.openai.com/oauth/token",
            scopes: &[
                "openid",
                "profile",
                "email",
                "offline_access",
                // 上游 Codex CLI 同款 scope：connectors 权限是后端把
                // 会话识别为 Codex 会话的一部分，缺失时 /models 返回空。
                "api.connectors.read",
                "api.connectors.invoke",
            ],
            flow: OAuthFlowData::Pkce {
                auth_url: "https://auth.openai.com/oauth/authorize",
                // redirect URI 必须与 Codex CLI 的 Hydra allow-list 精确匹配：
                // host 固定 localhost、path 固定 /auth/callback（端口 1455）。
                redirect_uri: "http://localhost:1455/auth/callback",
                extra_auth_params: &[
                    ("id_token_add_organizations", "true"),
                    ("codex_cli_simplified_flow", "true"),
                ],
            },
        }),
    },
    ChannelPreset {
        id: "xai",
        kind: ChannelKind::XaiOAuth,
        default_base_url: "https://api.x.ai/v1",
        display_name: "xAI Grok",
        feature: "xai-oauth",
        // SET-4 A3：xAI 双认证——OAuth 订阅与 API key 可切换（互斥替换）。
        auth_methods: &["oauth", "api_key"],
        oauth: Some(OAuthPresetData {
            client_id: "b1a00492-073a-47ea-816f-4c329264a828",
            token_url: "https://auth.x.ai/oauth2/token",
            scopes: &[
                "openid",
                "profile",
                "email",
                "offline_access",
                // grok-cli:access 是订阅级 CLI 推理访问；api:access 覆盖
                // api.x.ai REST 调用（xAI 官方文档的 Agentic CLI scope 组）。
                "grok-cli:access",
                "api:access",
            ],
            flow: OAuthFlowData::Device {
                device_auth_url: "https://auth.x.ai/oauth2/device/code",
            },
        }),
    },
    ChannelPreset {
        id: "glm-coding",
        kind: ChannelKind::ApiKey,
        default_base_url: "https://api.z.ai/api/coding/paas/v4",
        display_name: "GLM Coding",
        feature: "glm-coding",
        auth_methods: &["api_key"],
        oauth: None,
    },
    ChannelPreset {
        id: "opencode-go",
        kind: ChannelKind::ApiKey,
        default_base_url: "https://opencode.ai/zen/go/v1",
        display_name: "OpenCode Go",
        feature: "opencode-go",
        auth_methods: &["api_key"],
        oauth: None,
    },
    ChannelPreset {
        id: "qwen-token-plan",
        kind: ChannelKind::ApiKey,
        default_base_url: "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        display_name: "Qwen Token Plan",
        feature: "qwen-token-plan",
        auth_methods: &["api_key"],
        oauth: None,
    },
    ChannelPreset {
        id: "deepseek",
        kind: ChannelKind::ApiKey,
        default_base_url: "https://api.deepseek.com",
        display_name: "DeepSeek",
        feature: "deepseek",
        auth_methods: &["api_key"],
        oauth: None,
    },
    ChannelPreset {
        id: "kimi-platform",
        kind: ChannelKind::ApiKey,
        default_base_url: "https://api.moonshot.ai/v1",
        display_name: "Kimi Platform",
        feature: "kimi-platform",
        auth_methods: &["api_key"],
        oauth: None,
    },
    ChannelPreset {
        id: "kimi-code",
        kind: ChannelKind::KimiOAuth,
        default_base_url: "https://api.kimi.com/coding/v1",
        display_name: "Kimi Code",
        feature: "kimi-code",
        auth_methods: &["oauth"],
        // Kimi Code 公开 Device Flow 端点（MoonshotAI/kimi-cli auth/oauth.py
        // 与 moonshotai.github.io/kimi-code 文档一致，SET-4 web 核对）。
        oauth: Some(OAuthPresetData {
            client_id: "17e5f671-d194-4dfb-9706-5516cb48c098",
            token_url: "https://auth.kimi.com/api/oauth/token",
            scopes: &["kimi-code"],
            flow: OAuthFlowData::Device {
                device_auth_url: "https://auth.kimi.com/api/oauth/device_authorization",
            },
        }),
    },
];

/// 按 id 查找首发通道 preset。
pub fn channel_preset(id: &str) -> Option<&'static ChannelPreset> {
    CHANNEL_REGISTRY.iter().find(|preset| preset.id == id)
}

/// 唯一的 feature cfg 求值点。
///
/// 注册表行不带 cfg（保 pawork models 六行数据语义）；某行 feature 未启用
/// 时装配必须 fail-closed。未知 feature 名一律返回 false。
pub fn is_enabled(preset: &ChannelPreset) -> bool {
    match preset.feature {
        "chatgpt-oauth" => cfg!(feature = "chatgpt-oauth"),
        "xai-oauth" => cfg!(feature = "xai-oauth"),
        "glm-coding" => cfg!(feature = "glm-coding"),
        "opencode-go" => cfg!(feature = "opencode-go"),
        "qwen-token-plan" => cfg!(feature = "qwen-token-plan"),
        "deepseek" => cfg!(feature = "deepseek"),
        "kimi-platform" => cfg!(feature = "kimi-platform"),
        "kimi-code" => cfg!(feature = "kimi-code"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_first_party_channels_in_product_order() {
        let ids: Vec<&str> = CHANNEL_REGISTRY.iter().map(|preset| preset.id).collect();
        assert_eq!(
            ids,
            [
                "chatgpt",
                "xai",
                "glm-coding",
                "opencode-go",
                "qwen-token-plan",
                "deepseek",
                "kimi-platform",
                "kimi-code",
            ]
        );
    }

    #[test]
    fn api_key_rows_carry_default_endpoints_and_feature_gates() {
        let expected = [
            (
                "glm-coding",
                "https://api.z.ai/api/coding/paas/v4",
                "glm-coding",
            ),
            (
                "opencode-go",
                "https://opencode.ai/zen/go/v1",
                "opencode-go",
            ),
            (
                "qwen-token-plan",
                "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
                "qwen-token-plan",
            ),
            ("deepseek", "https://api.deepseek.com", "deepseek"),
            (
                "kimi-platform",
                "https://api.moonshot.ai/v1",
                "kimi-platform",
            ),
        ];
        for (id, url, feature) in expected {
            let preset = channel_preset(id).expect(id);
            assert_eq!(preset.kind, ChannelKind::ApiKey);
            assert_eq!(preset.default_base_url, url);
            assert_eq!(preset.feature, feature);
            assert!(preset.oauth.is_none());
            assert_eq!(preset.auth_methods, &["api_key"]);
        }
    }

    #[test]
    fn oauth_rows_convert_to_runtime_presets() {
        let chatgpt = channel_preset("chatgpt")
            .and_then(|preset| preset.oauth_preset())
            .expect("chatgpt preset");
        assert!(matches!(chatgpt.flow, OAuthFlow::Pkce { .. }));
        assert_eq!(chatgpt.token_url, "https://auth.openai.com/oauth/token");
        assert!(chatgpt.scopes.contains(&"api.connectors.read".to_string()));

        let xai = channel_preset("xai")
            .and_then(|preset| preset.oauth_preset())
            .expect("xai preset");
        assert_eq!(
            xai.flow,
            OAuthFlow::Device {
                device_auth_url: "https://auth.x.ai/oauth2/device/code".into(),
            }
        );
        let xai_row = channel_preset("xai").expect("xai row");
        assert_eq!(xai_row.auth_methods, &["oauth", "api_key"]);
    }

    #[test]
    fn kimi_code_preset_uses_official_device_flow_endpoints() {
        let preset = channel_preset("kimi-code").expect("kimi-code row");
        assert_eq!(preset.kind, ChannelKind::KimiOAuth);
        assert_eq!(preset.default_base_url, "https://api.kimi.com/coding/v1");
        assert_eq!(preset.auth_methods, &["oauth"]);
        let oauth = preset.oauth_preset().expect("kimi-code oauth preset");
        assert_eq!(oauth.client_id, "17e5f671-d194-4dfb-9706-5516cb48c098");
        assert_eq!(oauth.token_url, "https://auth.kimi.com/api/oauth/token");
        assert_eq!(
            oauth.flow,
            OAuthFlow::Device {
                device_auth_url: "https://auth.kimi.com/api/oauth/device_authorization".into(),
            }
        );
        assert_eq!(oauth.scopes, vec!["kimi-code".to_string()]);
    }

    #[test]
    fn unknown_feature_name_fails_closed() {
        let preset = ChannelPreset {
            id: "unknown-channel",
            kind: ChannelKind::ApiKey,
            default_base_url: "https://example.test",
            display_name: "Unknown Channel",
            feature: "not-a-feature",
            auth_methods: &["api_key"],
            oauth: None,
        };
        assert!(!is_enabled(&preset));
    }
}
