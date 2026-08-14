//! Usage / stop reason 归一（从 V1 `provider-runtime::usage` 内联）。
//!
//! 只保留 openai-compatible 流解析所需的 `normalize_usage` / `map_stop_reason`，
//! 不迁入 `estimate_cost` / `ModelPricingRef`。

use pawork_domain::{StopReason, TokenUsage};
use serde_json::Value;

/// 归一化 token usage。兼容主流 Provider 的字段命名：
/// - OpenAI：`prompt_tokens` / `completion_tokens`；
/// - Anthropic：`input_tokens` / `output_tokens`，以及
///   `cache_read_input_tokens` / `cache_creation_input_tokens`；
/// - 嵌套 `prompt_tokens_details.cached_tokens`（OpenAI 缓存）。
///
/// 缺失字段按 0 处理，绝不 panic。
pub fn normalize_usage(raw: &Value) -> TokenUsage {
    // token 字段可能位于 `raw` 顶层，也可能嵌套在 "usage" 下（OpenAI / Anthropic
    // 流式均把 usage 嵌在 "usage" 键内）。先解析出有效 usage 视图，再回退到顶层。
    let view = raw
        .get("usage")
        .filter(|value| value.is_object())
        .unwrap_or(raw);

    TokenUsage {
        input_tokens: pick_u64(view, raw, &["input_tokens", "prompt_tokens"]).unwrap_or(0),
        output_tokens: pick_u64(view, raw, &["output_tokens", "completion_tokens"]).unwrap_or(0),
        cache_read_tokens: pick_u64(view, raw, &["cache_read_input_tokens", "cache_read_tokens"])
            .or_else(|| prompt_cached_tokens(view))
            .or_else(|| prompt_cached_tokens(raw))
            .unwrap_or(0),
        cache_write_tokens: pick_u64(
            view,
            raw,
            &[
                "cache_creation_input_tokens",
                "cache_write_tokens",
                "cache_write_input_tokens",
            ],
        )
        .unwrap_or(0),
    }
}

/// 把 Provider 的 finish_reason 字符串映射为 canonical [`StopReason`]。
/// `has_tool_calls` 为 true 时优先返回 [`StopReason::ToolUse`]；协议已正常收尾但未提供
/// finish reason 时按 [`StopReason::Completed`] 处理。
pub fn map_stop_reason(finish: Option<&str>, has_tool_calls: bool) -> StopReason {
    if has_tool_calls {
        return StopReason::ToolUse;
    }
    match finish {
        None => StopReason::Completed,
        Some(reason) => match reason.to_ascii_lowercase().as_str() {
            "stop" | "end_turn" | "ended" => StopReason::Completed,
            "length" | "max_tokens" | "max_output_tokens" => StopReason::MaxTokens,
            "tool_calls" | "function_call" | "tool_use" | "functioncall" | "toolcalls" => {
                StopReason::ToolUse
            }
            "content_filter" | "content_filtered" | "safety" => StopReason::ContentFiltered,
            "cancelled" | "canceled" => StopReason::Cancelled,
            other => StopReason::Other(other.to_string()),
        },
    }
}

fn read_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    for key in keys {
        if let Some(number) = value.get(*key).and_then(|v| v.as_u64()) {
            return Some(number);
        }
    }
    None
}

/// 先从 `primary` 读字段，失败再从 `fallback` 读。
fn pick_u64(primary: &Value, fallback: &Value, keys: &[&str]) -> Option<u64> {
    read_u64(primary, keys).or_else(|| read_u64(fallback, keys))
}

/// 读取 OpenAI 的 `prompt_tokens_details.cached_tokens`（缓存命中）。
fn prompt_cached_tokens(view: &Value) -> Option<u64> {
    view.get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_openai_field_names() {
        let raw = serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "prompt_tokens_details": { "cached_tokens": 20 }
        });
        let usage = normalize_usage(&raw);
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 20);
        assert_eq!(usage.cache_write_tokens, 0);
    }

    #[test]
    fn normalizes_anthropic_field_names() {
        let raw = serde_json::json!({
            "input_tokens": 200,
            "output_tokens": 80,
            "cache_read_input_tokens": 30,
            "cache_creation_input_tokens": 10
        });
        let usage = normalize_usage(&raw);
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 80);
        assert_eq!(usage.cache_read_tokens, 30);
        assert_eq!(usage.cache_write_tokens, 10);
    }

    #[test]
    fn normalizes_nested_usage_container() {
        let raw = serde_json::json!({
            "usage": { "input_tokens": 5, "output_tokens": 2 }
        });
        let usage = normalize_usage(&raw);
        assert_eq!(usage.input_tokens, 5);
        assert_eq!(usage.output_tokens, 2);
    }

    #[test]
    fn missing_fields_default_to_zero() {
        let usage = normalize_usage(&serde_json::json!({}));
        assert_eq!(usage, TokenUsage::default());
    }

    #[test]
    fn maps_finish_reasons() {
        assert_eq!(map_stop_reason(Some("stop"), false), StopReason::Completed);
        assert_eq!(
            map_stop_reason(Some("end_turn"), false),
            StopReason::Completed
        );
        assert_eq!(
            map_stop_reason(Some("length"), false),
            StopReason::MaxTokens
        );
        assert_eq!(map_stop_reason(Some("STOP"), false), StopReason::Completed);
        assert_eq!(
            map_stop_reason(Some("content_filter"), false),
            StopReason::ContentFiltered
        );
        // tool_calls 优先（即使 finish 不是 tool）
        assert_eq!(map_stop_reason(Some("stop"), true), StopReason::ToolUse);
        assert_eq!(map_stop_reason(None, false), StopReason::Completed);
        assert_eq!(
            map_stop_reason(Some("weird"), false),
            StopReason::Other("weird".into())
        );
    }
}
