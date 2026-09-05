//! Pawork 配置 schema（强类型投影）。
//!
//! 该 schema 只覆盖当前阶段需要的、与配置合并直接相关的字段集合，作为
//! 配置系统自身的契约。其他 crate 在后续任务中以本 schema 为基础扩展，
//! 但本 crate 不引入对它们的依赖。

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// Pawork 顶层配置。
///
/// 所有字段 `Option`，缺省时退回更低层级或内置默认值，保证合并语义可叠加。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaworkConfig {
    /// 当前激活的 profile 名称（用于在 `[[profile]]` 中选择展开）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,

    /// 默认 provider。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,

    /// 默认 model。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    /// 自动命名 provider（ADR-054 D4；分层与 default_provider 相同）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub naming_provider: Option<String>,

    /// 自动命名 model（ADR-054 D4；分层与 default_model 相同）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub naming_model: Option<String>,

    /// provider 列表配置。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderConfig>,

    /// model 列表配置。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelConfig>,

    /// 用户自定义 profile 列表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<ProfileConfig>,

    /// 工作区信任默认值。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_workspaces: Option<bool>,

    /// Global 审批默认（ADR-053）；显式宿主启动参数可覆盖当次进程。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<pawork_policy::ApprovalMode>,

    /// Global 逐项目选择；键由 Host canonical 根路径生成，false 可撤销全局信任。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub workspace_trust: BTreeMap<String, bool>,

    /// 全局出站代理（如 `http://127.0.0.1:38081`）。
    ///
    /// 参照 CLIProxyAPI `proxy-url`：应用于 Provider/OAuth 出站请求；
    /// 回环与 `.local` 目标直连（`pawork-providers::net::loopback_aware_proxy`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,

    /// 终端默认设置（ADR-050 D1：仅 Global 层可写入，其余层整段剥离）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<TerminalConfig>,

    /// 任意扩展字段，按 key 递归合并。为未在 schema 显式声明的配置保留向后兼容入口。
    ///
    /// 顶层 `api_key` 不得经 extra 绕过「配置不含凭证」红线，反序列化时剥离。
    #[serde(
        flatten,
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "deserialize_extra_without_api_key"
    )]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// 终端默认设置段（`[terminal]`，ADR-050 D1）。
///
/// 三字段独立可选：`shell: None` 跟随平台默认（exec 兜底链），
/// columns/rows 未设时由消费方回落既有默认（80×24）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// 默认 shell 程序（含路径分隔符时为路径，否则按 PATH 解析）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// 默认列数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub columns: Option<u16>,
    /// 默认行数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
}

fn deserialize_extra_without_api_key<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    let mut extra = BTreeMap::<String, serde_json::Value>::deserialize(deserializer)?;
    extra.remove("api_key");
    Ok(extra)
}

/// Provider 配置。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// provider 标识（例如 `openai`）。
    pub id: String,
    /// 自定义 base URL（OpenAI 兼容端点）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// 是否为默认 provider。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
    /// 该 provider 是否跟随 Global `proxy_url`（ADR-052 SET-6h）。
    /// `None`/`Some(true)`：配置了全局代理时经代理出站；
    /// `Some(false)`：该 provider 出站绕过全局代理。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_proxy: Option<bool>,
}

/// Model 配置。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// model 标识。
    pub id: String,
    /// 上下文窗口大小（token）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// 最大输出 token。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
}

/// 用户 Profile：一组可命名的预设。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileConfig {
    /// profile 名称。
    pub name: String,
    /// 该 profile 覆盖的配置片段。
    #[serde(flatten)]
    pub overrides: ProfileOverrides,
}

/// Profile 覆盖片段。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Session 级别覆盖。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// 单次 Run 参数覆盖，优先级最高。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl PaworkConfig {
    /// 内置默认配置（优先级最低）。
    pub fn builtin() -> Self {
        PaworkConfig {
            trust_workspaces: Some(false),
            ..Self::default()
        }
    }

    /// 用更高优先级的片段合并覆盖自身（强类型语义：Option 取 `other` 的非空值）。
    pub fn merge_with(&mut self, other: &Self) {
        if other.profile.is_some() {
            self.profile = other.profile.clone();
        }
        if other.default_provider.is_some() {
            self.default_provider = other.default_provider.clone();
        }
        if other.default_model.is_some() {
            self.default_model = other.default_model.clone();
        }
        if other.trust_workspaces.is_some() {
            self.trust_workspaces = other.trust_workspaces;
        }
        if other.approval_mode.is_some() {
            self.approval_mode = other.approval_mode;
        }
        self.workspace_trust.extend(other.workspace_trust.clone());
        if other.proxy_url.is_some() {
            self.proxy_url = other.proxy_url.clone();
        }
        if other.terminal.is_some() {
            self.terminal = other.terminal.clone();
        }
        if !other.providers.is_empty() {
            self.providers = other.providers.clone();
        }
        if !other.models.is_empty() {
            self.models = other.models.clone();
        }
        if !other.profiles.is_empty() {
            self.profiles = other.profiles.clone();
        }
        merge_extra(&mut self.extra, &other.extra);
    }
}

fn merge_extra(
    lower: &mut BTreeMap<String, serde_json::Value>,
    higher: &BTreeMap<String, serde_json::Value>,
) {
    use super::merge::merge_json;
    for (key, value) in higher {
        match lower.get_mut(key) {
            Some(slot) => {
                merge_json(slot, value);
            }
            None => {
                lower.insert(key.clone(), value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builtin_default_is_untrusted() {
        let cfg = PaworkConfig::builtin();
        assert_eq!(cfg.trust_workspaces, Some(false));
    }

    #[test]
    fn proxy_url_parses_and_merges() {
        let cfg: PaworkConfig =
            serde_json::from_value(json!({ "proxy_url": "http://127.0.0.1:38081" })).unwrap();
        assert_eq!(cfg.proxy_url.as_deref(), Some("http://127.0.0.1:38081"));

        let mut merged = PaworkConfig::default();
        merged.merge_with(&cfg);
        assert_eq!(merged.proxy_url, cfg.proxy_url);
    }

    #[test]
    fn merge_with_takes_higher_non_empty_values() {
        let mut base = PaworkConfig {
            default_provider: Some("a".into()),
            default_model: Some("m1".into()),
            ..PaworkConfig::default()
        };
        let higher = PaworkConfig {
            default_model: Some("m2".into()),
            ..PaworkConfig::default()
        };
        base.merge_with(&higher);
        assert_eq!(base.default_provider.as_deref(), Some("a"));
        assert_eq!(base.default_model.as_deref(), Some("m2"));
    }

    #[test]
    fn extra_fields_flatten_recursively() {
        let mut base = PaworkConfig {
            extra: BTreeMap::from([("section".into(), json!({ "a": 1 }))]),
            ..PaworkConfig::default()
        };
        let higher = PaworkConfig {
            extra: BTreeMap::from([("section".into(), json!({ "b": 2 }))]),
            ..PaworkConfig::default()
        };
        base.merge_with(&higher);
        assert_eq!(base.extra.get("section"), Some(&json!({ "a": 1, "b": 2 })));
    }

    #[test]
    fn merge_with_replaces_providers_table_not_by_id() {
        let mut base = PaworkConfig {
            providers: vec![ProviderConfig {
                id: "a".into(),
                base_url: Some("https://a.example/v1".into()),
                ..ProviderConfig::default()
            }],
            ..PaworkConfig::default()
        };
        let higher = PaworkConfig {
            providers: vec![ProviderConfig {
                id: "b".into(),
                ..ProviderConfig::default()
            }],
            ..PaworkConfig::default()
        };
        base.merge_with(&higher);
        assert_eq!(base.providers.len(), 1);
        assert_eq!(base.providers[0].id, "b");
        assert!(base.providers[0].base_url.is_none());
    }

    #[test]
    fn provider_config_has_no_api_key_field_in_toml_or_debug() {
        let provider = ProviderConfig {
            id: "glm-coding".into(),
            base_url: Some("https://example.test/v1".into()),
            default: Some(true),
            use_proxy: None,
        };
        let toml = toml::to_string(&provider).expect("serialize provider");
        let debug = format!("{provider:?}");
        assert!(
            !toml.contains("api_key"),
            "ProviderConfig TOML must not contain api_key: {toml}"
        );
        assert!(
            !debug.contains("api_key"),
            "ProviderConfig Debug must not contain api_key: {debug}"
        );
    }

    #[test]
    fn pawork_config_has_no_api_key_field_in_toml_or_debug() {
        let cfg = PaworkConfig {
            default_provider: Some("glm-coding".into()),
            default_model: Some("glm-5.2".into()),
            providers: vec![ProviderConfig {
                id: "glm-coding".into(),
                base_url: Some("https://example.test/v1".into()),
                ..ProviderConfig::default()
            }],
            ..PaworkConfig::default()
        };
        let toml = toml::to_string(&cfg).expect("serialize config");
        let debug = format!("{cfg:?}");
        assert!(
            !toml.contains("api_key"),
            "PaworkConfig TOML must not contain api_key: {toml}"
        );
        assert!(
            !debug.contains("api_key"),
            "PaworkConfig Debug must not contain api_key: {debug}"
        );
    }

    #[test]
    fn provider_toml_api_key_is_discarded() {
        let parsed: ProviderConfig = toml::from_str(
            r#"
id = "glm-coding"
api_key = "fake-key-should-be-dropped"
base_url = "https://example.test/v1"
"#,
        )
        .expect("parse provider");
        assert_eq!(parsed.id, "glm-coding");
        assert_eq!(parsed.base_url.as_deref(), Some("https://example.test/v1"));
        let toml = toml::to_string(&parsed).expect("serialize");
        let debug = format!("{parsed:?}");
        assert!(!toml.contains("api_key"));
        assert!(!debug.contains("api_key"));
        assert!(!toml.contains("fake-key-should-be-dropped"));
        assert!(!debug.contains("fake-key-should-be-dropped"));
    }

    #[test]
    fn top_level_api_key_is_stripped_from_extra() {
        let parsed: PaworkConfig = toml::from_str(
            r#"
default_provider = "glm-coding"
api_key = "fake-key-should-be-stripped"
other_extension = 1
"#,
        )
        .expect("parse config");
        assert_eq!(parsed.default_provider.as_deref(), Some("glm-coding"));
        assert!(
            !parsed.extra.contains_key("api_key"),
            "top-level api_key must not land in extra: {:?}",
            parsed.extra
        );
        assert_eq!(parsed.extra.get("other_extension"), Some(&json!(1)));
        let toml = toml::to_string(&parsed).expect("serialize");
        let debug = format!("{parsed:?}");
        assert!(!toml.contains("api_key"), "{toml}");
        assert!(!debug.contains("api_key"), "{debug}");
        assert!(!toml.contains("fake-key-should-be-stripped"));
        assert!(!debug.contains("fake-key-should-be-stripped"));
    }
}
