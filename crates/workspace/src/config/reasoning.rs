//! Global-only per-model reasoning effort preferences (`[reasoning]`).
//!
//! ADR-063：模型级推理强度偏好。`default_effort` 是选择该模型时的默认
//! 推理强度；`supported_efforts` 是目录/探测未声明时的手动可选范围声明
//! （目录已声明时以目录为准，手动值仅作覆盖）。effort 名在此不校验，
//! 由 Host 写路径 fail-closed 校验（canonical ReasoningEffort 词汇）。
//!
//! 2026-09-22 合并语义：同一 model_id 跨 Provider 共用一套偏好（如
//! `deepseek/deepseek-flash` 与 `opencode-go/deepseek-flash`），查找只按
//! model_id 匹配；`provider_id` 字段仅作写入来源记录，不参与匹配。

use serde::{Deserialize, Serialize};

/// `[reasoning]` 表，仅 Builtin/Global 层生效。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReasoningSettings {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelReasoningConfig>,
}
impl ReasoningSettings {
    /// 按 model_id 查显式偏好（跨 Provider 合并：同 id 模型共用一套配置）。
    pub fn model(&self, model_id: &str) -> Option<&ModelReasoningConfig> {
        self.models.iter().find(|entry| entry.model_id == model_id)
    }
}

/// 单模型的推理强度偏好。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelReasoningConfig {
    /// 写入来源 Provider（旧配置兼容字段；不参与匹配，同 id 跨 Provider 合并）。
    pub provider_id: String,
    pub model_id: String,
    /// 选择该模型时的默认推理强度（canonical effort 名）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// 手动声明的可选推理强度范围（目录未声明时使用；None = 未手动声明）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_efforts: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_settings_round_trip_skips_empty_keys() {
        let settings = ReasoningSettings {
            models: vec![ModelReasoningConfig {
                provider_id: "glm-coding".into(),
                model_id: "glm-5.3-flash".into(),
                default_effort: Some("high".into()),
                supported_efforts: None,
            }],
        };
        let value = toml::to_string(&settings).expect("serialize");
        assert!(value.contains("default_effort = \"high\""));
        assert!(!value.contains("supported_efforts"));
        let decoded: ReasoningSettings = toml::from_str(&value).expect("deserialize");
        assert_eq!(decoded, settings);
        assert_eq!(
            settings
                .model("glm-5.3-flash")
                .map(|m| m.default_effort.as_deref()),
            Some(Some("high"))
        );
        // 跨 Provider 合并：provider_id 不参与匹配。
        assert!(settings.model("other").is_none());
        let merged = ReasoningSettings {
            models: vec![ModelReasoningConfig {
                provider_id: "deepseek".into(),
                model_id: "deepseek-flash".into(),
                default_effort: Some("medium".into()),
                supported_efforts: None,
            }],
        };
        assert_eq!(
            merged
                .model("deepseek-flash")
                .map(|m| m.provider_id.as_str()),
            Some("deepseek")
        );
    }
}
