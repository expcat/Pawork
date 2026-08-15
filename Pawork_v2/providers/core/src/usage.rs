//! Usage / stop reason 归一与会话级 usage 聚合（S5 波 A）。
//!
//! 归一函数迁自 V1 `provider-runtime::usage`（经 V2 `pawork-providers` 内联版回收），
//! 不包含 `ModelPricingRef` / `estimate_cost`——定价统一在 [`crate::pricing`]，
//! 避免双轨。`UsageAccumulator` 对齐 `ProviderStreamEvent::UsageUpdated`：
//! 请求内按「最新快照」语义，跨请求按「累加」语义。

use pawork_domain::{RequestId, StopReason, TokenUsage};
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

/// 请求内最新快照、跨请求累加的 usage 聚合器。
///
/// Provider 在单个请求的流式过程中可能上报多次 usage（渐进累计快照），
/// [`Self::record`] 对同一请求按「最新快照覆盖」语义处理——后者覆盖前者，
/// 不在请求内重复累加；请求 id 变化（或调用 [`Self::finish_request`]）时，
/// 把上一请求的最终快照累加进会话累计。
///
/// 假设会话内 provider 请求按顺序发起、同一时刻只有一个进行中请求；
/// 交错回放（A→B→A）会把 A 的迟到快照当作新请求，不在支持范围。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageAccumulator {
    /// 已结算请求的累加（每请求取其最终快照）。
    finalized: TokenUsage,
    /// 当前请求 id；`None` 表示尚无（或刚结算完）进行中请求。
    current_request: Option<RequestId>,
    /// 当前请求内最近一次快照。
    current: TokenUsage,
}

impl UsageAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次 usage 快照（对应 `ProviderStreamEvent::UsageUpdated`）。
    ///
    /// 同一请求 id 内多次记录取最新快照；请求 id 变化时先结算上一请求。
    pub fn record(&mut self, request: &RequestId, usage: TokenUsage) {
        if self.current_request.as_ref() != Some(request) {
            self.settle();
            self.current_request = Some(request.clone());
        }
        self.current = usage;
    }

    /// 会话累计 = 已结算请求累加 + 进行中请求的最新快照。
    pub fn total(&self) -> TokenUsage {
        let mut total = self.finalized.clone();
        add_usage(&mut total, &self.current);
        total
    }

    /// 进行中请求的最新快照；无任何记录时为全零。
    pub fn current(&self) -> &TokenUsage {
        &self.current
    }

    /// 显式结算当前请求。请求正常结束时调用；不调用则由下一次换请求的
    /// [`Self::record`] 触发，语义等价。重复调用无额外效果。
    pub fn finish_request(&mut self) {
        self.settle();
    }

    /// 结算当前请求快照进累计；无进行中请求时是 no-op。
    fn settle(&mut self) {
        if self.current_request.take().is_some() {
            add_usage(&mut self.finalized, &self.current);
            self.current = TokenUsage::default();
        }
    }
}

fn add_usage(dst: &mut TokenUsage, src: &TokenUsage) {
    dst.input_tokens = dst.input_tokens.saturating_add(src.input_tokens);
    dst.output_tokens = dst.output_tokens.saturating_add(src.output_tokens);
    dst.cache_read_tokens = dst
        .cache_read_tokens
        .saturating_add(src.cache_read_tokens);
    dst.cache_write_tokens = dst
        .cache_write_tokens
        .saturating_add(src.cache_write_tokens);
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
        assert_eq!(map_stop_reason(Some("length"), false), StopReason::MaxTokens);
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

    fn usage(input: u64, output: u64) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }
    }

    #[test]
    fn latest_snapshot_wins_within_one_request() {
        let request = RequestId::new("request-1");
        let mut accumulator = UsageAccumulator::new();
        // Provider 流式过程中的渐进快照：后者覆盖前者。
        accumulator.record(&request, usage(120, 10));
        accumulator.record(&request, usage(150, 40));
        accumulator.record(&request, usage(150, 60));

        assert_eq!(*accumulator.current(), usage(150, 60), "请求内取最新快照");
        assert_eq!(
            accumulator.total(),
            usage(150, 60),
            "total 含进行中请求的最新快照，不在请求内重复累加"
        );
    }

    #[test]
    fn requests_accumulate_across_request_boundaries() {
        let mut accumulator = UsageAccumulator::new();
        accumulator.record(&RequestId::new("request-1"), usage(100, 20));
        accumulator.record(&RequestId::new("request-1"), usage(120, 30));
        accumulator.record(&RequestId::new("request-2"), usage(50, 5));

        assert_eq!(*accumulator.current(), usage(50, 5));
        assert_eq!(
            accumulator.total(),
            usage(170, 35),
            "跨请求累加：request-1 最终快照 + request-2 最新快照"
        );
    }

    #[test]
    fn finish_request_settles_current_snapshot_once() {
        let mut accumulator = UsageAccumulator::new();
        let request = RequestId::new("request-1");
        accumulator.record(&request, usage(80, 8));
        accumulator.finish_request();
        assert_eq!(accumulator.total(), usage(80, 8));

        // 重复结算无额外效果；结算后继续记录同一请求按新请求处理。
        accumulator.finish_request();
        accumulator.record(&request, usage(10, 1));
        assert_eq!(
            accumulator.total(),
            usage(90, 9),
            "结算后的同请求新快照按新请求处理"
        );
    }

    #[test]
    fn accumulator_starts_from_zero() {
        let accumulator = UsageAccumulator::new();
        assert_eq!(accumulator.total(), TokenUsage::default());
        assert_eq!(*accumulator.current(), TokenUsage::default());
    }
}
