//! 适配器协议选择：读配置数据，不按 Provider 名称做启发式。
//!
//! `ProviderConfig` 冻结形状只有 `id` / `base_url` / `default`。协议落在
//! `PaworkConfig.extra["provider_protocols"]`（顶层 TOML 表），再加本 crate
//! 对样例三条 id 的默认表。engine 不读本模块。

use pawork_workspace::config::PaworkConfig;

/// host 装配用的协议枚举（不是冻结契约字段）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterProtocol {
    ChatCompletions,
    Messages,
    /// Responses transport（ChatGPT/xAI 首发 OAuth 通道装配用标记）。
    Responses,
}

/// 解析 provider 应使用的适配器协议。
///
/// 顺序：`extra.provider_protocols[id]` → 样例默认表 → `ChatCompletions`。
/// extra 里出现无法识别的值则 fail-closed。
pub fn resolve_adapter_protocol(
    config: &PaworkConfig,
    provider_id: &str,
) -> Result<AdapterProtocol, ProtocolError> {
    if let Some(value) = extra_protocol(config, provider_id) {
        return parse_protocol(value).ok_or_else(|| ProtocolError::Unknown {
            provider: provider_id.to_string(),
            value: value.to_string(),
        });
    }
    Ok(default_protocol(provider_id))
}

fn extra_protocol<'a>(config: &'a PaworkConfig, provider_id: &str) -> Option<&'a str> {
    config
        .extra
        .get("provider_protocols")?
        .get(provider_id)?
        .as_str()
}

fn parse_protocol(value: &str) -> Option<AdapterProtocol> {
    match value {
        "chat_completions" | "openai-compatible" => Some(AdapterProtocol::ChatCompletions),
        "messages" | "anthropic-messages" => Some(AdapterProtocol::Messages),
        _ => None,
    }
}

fn default_protocol(provider_id: &str) -> AdapterProtocol {
    match provider_id {
        "glm-coding-anthropic" => AdapterProtocol::Messages,
        _ => AdapterProtocol::ChatCompletions,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    Unknown { provider: String, value: String },
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown { provider, value } => write!(
                formatter,
                "unknown adapter protocol `{value}` for provider `{provider}`"
            ),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pawork_workspace::config::PaworkConfig;
    use serde_json::json;

    use super::*;

    fn config_with_protocols(map: serde_json::Value) -> PaworkConfig {
        let mut extra = BTreeMap::new();
        extra.insert("provider_protocols".into(), map);
        PaworkConfig {
            extra,
            ..PaworkConfig::default()
        }
    }

    #[test]
    fn extra_overrides_default_table() {
        let config = config_with_protocols(json!({
            "glm-coding": "messages",
            "custom": "chat_completions",
        }));
        assert_eq!(
            resolve_adapter_protocol(&config, "glm-coding").expect("ok"),
            AdapterProtocol::Messages
        );
        assert_eq!(
            resolve_adapter_protocol(&config, "custom").expect("ok"),
            AdapterProtocol::ChatCompletions
        );
    }

    #[test]
    fn fixture_ids_have_defaults_without_extra() {
        let config = PaworkConfig::default();
        assert_eq!(
            resolve_adapter_protocol(&config, "glm-coding").expect("ok"),
            AdapterProtocol::ChatCompletions
        );
        assert_eq!(
            resolve_adapter_protocol(&config, "glm-coding-anthropic").expect("ok"),
            AdapterProtocol::Messages
        );
        assert_eq!(
            resolve_adapter_protocol(&config, "opencode-go").expect("ok"),
            AdapterProtocol::ChatCompletions
        );
        assert_eq!(
            resolve_adapter_protocol(&config, "unknown-vendor").expect("ok"),
            AdapterProtocol::ChatCompletions
        );
    }

    #[test]
    fn unknown_extra_value_is_fail_closed() {
        let config = config_with_protocols(json!({ "glm-coding": "responses" }));
        let err = resolve_adapter_protocol(&config, "glm-coding").expect_err("bad");
        assert!(matches!(
            err,
            ProtocolError::Unknown { ref provider, ref value }
                if provider == "glm-coding" && value == "responses"
        ));
    }
}
