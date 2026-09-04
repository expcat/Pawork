//! Token 估算（自 V1 `context-engine::token` 迁入；`TiktokenEstimator` 与
//! `default_estimator_for` 不迁——V2 engine 不引入 tiktoken 依赖，精确分词
//! 实现由宿主在需要时另行装配）。
//!
//! 设计要点：
//! - [`TokenEstimator`] 以 `count_text` 为唯一核心方法；`count_message` /
//!   `count_content_part` / `count_content_parts` / `count_tool_schemas` 均为
//!   基于 `count_text` 的默认实现，保证实现口径一致，且可在 `&dyn TokenEstimator`
//!   上调用。
//! - 无法获得精确 tokenizer 时走 [`HeuristicEstimator`]（拉丁等脚本默认
//!   chars/4，CJK/Hangul/Kana 保守按 1 字符/token）。

use pawork_domain::{ContentPart, Message, MessageRole};
use serde::{Deserialize, Serialize};

/// 单条消息的结构开销（业界 cl100k/o200k 分词约定每条消息 +4）。
const MESSAGE_FRAMING_TOKENS: u64 = 4;
/// 助手回复的起始占用（消息序列末尾的 primer）。
const REPLY_PRIMER_TOKENS: u64 = 3;
/// 每个工具定义的额外开销。
const TOOL_FRAMING_TOKENS: u64 = 8;
/// 单张图片占用的近似 token（业界 low-detail 档位约定 ≈ 85）。
const IMAGE_PLACEHOLDER_TOKENS: u64 = 85;

/// 可计数的工具定义表示。
///
/// 镜像 `pawork-domain/provider_api` 的 `ToolDefinition` JSON 形状，但**独立定义**，以保持
/// context 模块仅依赖 `pawork-domain`。调用方（engine 循环）可将 canonical
/// `ToolDefinition` 序列化后反序列化为本类型，字段一一对应，无需手动转换。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Token 估算器。除 `count_text` 与 `estimator_kind` 外，其余方法均有默认实现，
/// 可直接在 `&dyn TokenEstimator` 上调用。
pub trait TokenEstimator: Send + Sync {
    /// 估算纯文本 token 数。
    fn count_text(&self, text: &str) -> u64;

    /// 估算单个 [`ContentPart`]（递归处理 `ToolResult` 嵌套内容）。
    fn count_content_part(&self, part: &ContentPart) -> u64 {
        match part {
            ContentPart::Text(text) => self.count_text(&text.text),
            ContentPart::Thinking(thinking) => self.count_text(&thinking.text),
            // Protected continuation bytes are intentionally unavailable to the
            // estimator; only the safe summary contributes a local estimate.
            ContentPart::Reasoning(reasoning) => reasoning
                .summary
                .as_deref()
                .map(|summary| self.count_text(summary))
                .unwrap_or(0),
            ContentPart::Image(image) => {
                IMAGE_PLACEHOLDER_TOKENS
                    + image
                        .alt_text
                        .as_ref()
                        .map(|alt| self.count_text(alt))
                        .unwrap_or(0)
            }
            ContentPart::ToolCall(call) => {
                self.count_text(&call.name)
                    + self.count_text(&call.arguments.to_string())
                    + call
                        .raw_arguments
                        .as_ref()
                        .map(|raw| self.count_text(raw))
                        .unwrap_or(0)
            }
            ContentPart::ToolResult(result) => {
                let mut tokens = result
                    .tool_name
                    .as_ref()
                    .map(|name| self.count_text(name))
                    .unwrap_or(0);
                tokens += self.count_content_parts(&result.content);
                tokens += self.count_text(&result.metadata.to_string());
                tokens
            }
            ContentPart::ArtifactRef(reference) => {
                self.count_text(reference.id.as_str())
                    + self.count_text(&reference.media_type)
                    + reference
                        .label
                        .as_ref()
                        .map(|label| self.count_text(label))
                        .unwrap_or(0)
            }
        }
    }

    /// 估算一组 [`ContentPart`]。
    fn count_content_parts(&self, parts: &[ContentPart]) -> u64 {
        parts.iter().map(|part| self.count_content_part(part)).sum()
    }

    /// 估算单条消息（角色 + 内容 + 结构开销）。
    fn count_message(&self, message: &Message) -> u64 {
        MESSAGE_FRAMING_TOKENS
            + self.count_text(role_label(&message.role))
            + self.count_content_parts(&message.content)
    }

    /// 估算工具定义 schema 的总占用（序列化为 JSON 后计数 + 每个工具结构开销）。
    fn count_tool_schemas(&self, schemas: &[ToolSchema]) -> u64 {
        schemas
            .iter()
            .map(|schema| {
                let json = serde_json::to_string(schema).unwrap_or_default();
                self.count_text(&json) + TOOL_FRAMING_TOKENS
            })
            .sum()
    }

    /// 估算器标识，用于诊断。
    fn estimator_kind(&self) -> &'static str;
}

/// 启发式估算器：无法获得精确 tokenizer 时使用。
///
/// `chars_per_token` 适用于非 CJK 字符；CJK ideograph、Kana 与 Hangul 独立按
/// 1 字符/token 估算，避免使用统一 chars/4 时严重低估东亚文本。
#[derive(Clone, Debug)]
pub struct HeuristicEstimator {
    chars_per_token: u32,
}

impl Default for HeuristicEstimator {
    fn default() -> Self {
        Self { chars_per_token: 4 }
    }
}

impl HeuristicEstimator {
    /// 按 chars/token 构造（最小为 1）。
    pub fn new(chars_per_token: u32) -> Self {
        Self {
            chars_per_token: chars_per_token.max(1),
        }
    }

    /// 每个估算 token 对应的字符数（即「可配 ratio」）。
    pub fn chars_per_token(&self) -> u32 {
        self.chars_per_token
    }
}

impl TokenEstimator for HeuristicEstimator {
    fn count_text(&self, text: &str) -> u64 {
        let (cjk_chars, other_chars) = text.chars().fold((0u64, 0u64), |counts, ch| {
            if is_cjk_like(ch) {
                (counts.0.saturating_add(1), counts.1)
            } else {
                (counts.0, counts.1.saturating_add(1))
            }
        });
        cjk_chars.saturating_add(other_chars.div_ceil(self.chars_per_token as u64))
    }

    fn estimator_kind(&self) -> &'static str {
        "heuristic"
    }
}

fn is_cjk_like(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x11FF // Hangul Jamo
            | 0x2E80..=0x2FFF // CJK radicals
            | 0x3000..=0x303F // CJK symbols and punctuation
            | 0x3040..=0x30FF // Hiragana and Katakana
            | 0x3100..=0x312F // Bopomofo
            | 0x3130..=0x318F // Hangul compatibility Jamo
            | 0x31A0..=0x31BF // Bopomofo extended
            | 0x31F0..=0x31FF // Katakana phonetic extensions
            | 0x3400..=0x4DBF // CJK unified ideographs extension A
            | 0x4E00..=0x9FFF // CJK unified ideographs
            | 0xA960..=0xA97F // Hangul Jamo extended-A
            | 0xAC00..=0xD7AF // Hangul syllables
            | 0xD7B0..=0xD7FF // Hangul Jamo extended-B
            | 0xF900..=0xFAFF // CJK compatibility ideographs
            | 0x20000..=0x2FA1F // CJK extensions and compatibility supplement
    )
}

fn role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

pub(crate) const fn reply_primer_tokens() -> u64 {
    REPLY_PRIMER_TOKENS
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::{MessageId, MessageMetadata, TextContent};

    #[test]
    fn heuristic_uses_chars_per_token_with_ceil() {
        let est = HeuristicEstimator::new(4);
        assert_eq!(est.count_text("hello world!"), 3); // 12 chars / 4 = 3
        assert_eq!(est.count_text("abcde"), 2); // ceil(5/4)
        assert_eq!(est.count_text(""), 0);
        assert_eq!(HeuristicEstimator::new(2).count_text("abcd"), 2);
        assert_eq!(HeuristicEstimator::new(0).count_text("abcd"), 4); // 最小为 1 → chars/1
    }

    #[test]
    fn heuristic_counts_cjk_separately_from_latin_ratio() {
        let est = HeuristicEstimator::default();
        assert_eq!(est.count_text("你好世界上下文压缩"), 9);
        assert_eq!(est.count_text("abcd你好"), 3); // latin 4/4 + CJK 2/1
        assert_eq!(est.count_text("こんにちは"), 5);
        assert_eq!(est.count_text("안녕하세요"), 5);
    }

    #[test]
    fn framing_constants_match_industry_conventions() {
        assert_eq!(MESSAGE_FRAMING_TOKENS, 4);
        assert_eq!(reply_primer_tokens(), 3);
        assert_eq!(TOOL_FRAMING_TOKENS, 8);
        assert_eq!(IMAGE_PLACEHOLDER_TOKENS, 85);
    }

    #[test]
    fn count_message_includes_framing_and_content() {
        let est = HeuristicEstimator::new(4);
        let msg = Message {
            id: MessageId::from("m"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "abcdefgh".into(),
            })], // 8 chars -> 2
            metadata: MessageMetadata::default(),
        };
        // framing(4) + role "user"(1) + content(2)
        assert_eq!(est.count_message(&msg), 4 + 1 + 2);
    }

    #[test]
    fn tool_schemas_counted_from_json() {
        let est = HeuristicEstimator::new(4);
        let schemas = vec![ToolSchema {
            name: "read_file".into(),
            description: "read a file".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        let n = est.count_tool_schemas(&schemas);
        // json 文本非空，故 n > 每工具结构开销 8
        assert!(n > TOOL_FRAMING_TOKENS);
    }

    #[test]
    fn count_content_part_recurses_into_tool_result() {
        let est = HeuristicEstimator::new(4);
        let part = ContentPart::ToolResult(pawork_domain::ToolResultContent {
            tool_call_id: pawork_domain::ToolCallId::from("c"),
            tool_name: Some("read_file".into()),
            content: vec![ContentPart::Text(TextContent {
                text: "abcdefgh".into(),
            })], // 2 tokens
            is_error: false,
            metadata: serde_json::Value::Null,
            artifacts: Vec::new(),
        });
        // tool_name(3) + nested text(2) + metadata "null"(1)
        assert_eq!(est.count_content_part(&part), 3 + 2 + 1);
    }

    #[test]
    fn reasoning_counts_only_safe_summary_not_protected_reference() {
        let est = HeuristicEstimator::new(4);
        let part = ContentPart::Reasoning(pawork_domain::ReasoningItem {
            id: pawork_domain::ReasoningItemId::from("reasoning-1"),
            summary: Some("abcdefgh".into()),
            protected_blob_ref: pawork_domain::ProtectedBlobRef::from(
                "a-very-long-protected-reference-that-must-not-affect-token-count",
            ),
            opaque_metadata: Default::default(),
            continuation_metadata: Default::default(),
        });

        assert_eq!(est.count_content_part(&part), 2);
    }
}
