//! 能力校验（P6-5 / P6-8）。
//!
//! 按 [`ModelCapabilities`] 约束 thinking level 与结构化输出：当请求要求的能力
//! 超出模型支持范围时，给出明确、可归一化的 [`ProviderError`]，避免把不合法
//! 请求直接发给远端。

use provider_api::{
    ModelCapabilities, ProviderError, ProviderErrorKind, ResponseFormat, ThinkingConfig,
    ThinkingLevel,
};

/// 校验 thinking 配置与模型能力是否匹配。
///
/// `Off` 永远合法；其余 level 仅在模型声明 `thinking` 能力时允许，否则返回
/// [`ProviderErrorKind::InvalidRequest`]（不可重试，调用方应切换模型或关闭 thinking）。
pub fn validate_thinking(
    config: &ThinkingConfig,
    caps: &ModelCapabilities,
) -> Result<(), ProviderError> {
    if config.level != ThinkingLevel::Off && !caps.thinking {
        return Err(ProviderError::new(
            ProviderErrorKind::InvalidRequest,
            format!(
                "model does not support thinking/reasoning (requested level: {:?})",
                config.level
            ),
        ));
    }
    Ok(())
}

/// 结构化输出是否被模型原生支持（JSON / JSON Schema）。
///
/// 不直接报错：不支持时调用方应降级（如改用文本指令 + 自行校验），由各 Provider
/// adapter 决定回退策略。
pub fn structured_output_supported(caps: &ModelCapabilities) -> bool {
    caps.structured_output
}

/// 把 canonical [`ResponseFormat`] 归一为「是否需要结构化输出」。
pub fn requires_structured_output(format: &ResponseFormat) -> bool {
    !matches!(format, ResponseFormat::Text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(thinking: bool, structured: bool) -> ModelCapabilities {
        ModelCapabilities {
            text: true,
            image_input: true,
            tool_calls: true,
            parallel_tool_calls: true,
            thinking,
            structured_output: structured,
            prompt_cache: true,
            ..ModelCapabilities::default()
        }
    }

    #[test]
    fn off_level_always_allowed() {
        let config = ThinkingConfig {
            level: ThinkingLevel::Off,
            budget_tokens: None,
        };
        validate_thinking(&config, &caps(false, false)).expect("off is always allowed");
    }

    #[test]
    fn thinking_rejected_when_unsupported() {
        let config = ThinkingConfig {
            level: ThinkingLevel::Medium,
            budget_tokens: None,
        };
        let err = validate_thinking(&config, &caps(false, false)).expect_err("unsupported");
        assert_eq!(err.kind, ProviderErrorKind::InvalidRequest);
        assert!(!err.retryable);
    }

    #[test]
    fn thinking_allowed_when_supported() {
        let config = ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: Some(1024),
        };
        validate_thinking(&config, &caps(true, false)).expect("supported");
    }

    #[test]
    fn structured_output_flag_matches_capability() {
        assert!(structured_output_supported(&caps(false, true)));
        assert!(!structured_output_supported(&caps(false, false)));
    }

    #[test]
    fn response_format_requires_structured_helper() {
        assert!(!requires_structured_output(&ResponseFormat::Text));
        assert!(requires_structured_output(&ResponseFormat::Json));
        assert!(requires_structured_output(&ResponseFormat::JsonSchema {
            name: "x".into(),
            schema: serde_json::json!({})
        }));
    }
}
