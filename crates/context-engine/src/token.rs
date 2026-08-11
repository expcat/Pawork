//! Token 估算：OpenAI 系精确计数（tiktoken-rs），其它模型启发式估算（chars/token）。
//!
//! 设计要点：
//! - [`TokenEstimator`] 以 `count_text` 为唯一核心方法；`count_message` /
//!   `count_content_part` / `count_tool_schemas` 均为基于 `count_text` 的默认实现，
//!   保证两种实现口径一致，且可在 `&dyn TokenEstimator` 上调用。
//! - OpenAI 系模型用 tiktoken 精确分词；无法获得精确 tokenizer 时走
//!   [`HeuristicEstimator`]（拉丁等脚本默认 chars/4，CJK/Hangul/Kana 保守按 1 字符/token）。

use agent_domain::{ContentPart, Message, MessageRole};
use serde::{Deserialize, Serialize};

use crate::error::ContextBuildError;

/// 单条消息的结构开销（OpenAI cl100k/o200k 约定每条消息 +4）。
const MESSAGE_FRAMING_TOKENS: u64 = 4;
/// 助手回复的起始占用（消息序列末尾的 primer）。
const REPLY_PRIMER_TOKENS: u64 = 3;
/// 每个工具定义的额外开销。
const TOOL_FRAMING_TOKENS: u64 = 8;
/// 单张图片占用的近似 token（OpenAI low-detail ≈ 85）。
const IMAGE_PLACEHOLDER_TOKENS: u64 = 85;

/// 可计数的工具定义表示。
///
/// 镜像 `provider-api::ToolDefinition` 的 JSON 形状，但**独立定义**，以保持
/// `context-engine` 仅依赖 `agent-domain`（见 `docs/architecture/workspace-layout.md`）。
/// 调用方（如 agent-engine）可将 canonical `ToolDefinition` 序列化后反序列化为本类型，
/// 字段一一对应，无需手动转换。
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

/// 基于 tiktoken 的精确估算器（OpenAI 系模型）。
pub struct TiktokenEstimator {
    bpe: tiktoken_rs::CoreBPE,
    model: String,
}

impl TiktokenEstimator {
    /// 按模型名构造；模型无法识别或 BPE 加载失败时返回错误。
    pub fn for_model(model: &str) -> Result<Self, ContextBuildError> {
        let bpe = tiktoken_rs::get_bpe_from_model(model)
            .map_err(|err| ContextBuildError::tokenizer_unavailable(model, err.to_string()))?;
        Ok(Self {
            bpe,
            model: model.to_string(),
        })
    }

    /// 显式按 tokenizer 构造（测试或手动指定）。
    pub fn with_tokenizer(
        tokenizer: tiktoken_rs::tokenizer::Tokenizer,
    ) -> Result<Self, ContextBuildError> {
        let bpe = tiktoken_rs::get_bpe_from_tokenizer(tokenizer)
            .map_err(|err| ContextBuildError::tokenizer_unavailable("explicit", err.to_string()))?;
        Ok(Self {
            bpe,
            model: "explicit".to_string(),
        })
    }

    /// 构造时使用的模型名。
    pub fn model(&self) -> &str {
        &self.model
    }
}

impl TokenEstimator for TiktokenEstimator {
    fn count_text(&self, text: &str) -> u64 {
        self.bpe.encode_ordinary(text).len() as u64
    }

    fn estimator_kind(&self) -> &'static str {
        "tiktoken"
    }
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

/// 选择默认估算器：OpenAI 系（tiktoken 可识别）走精确计数，否则启发式。
pub fn default_estimator_for(model: &str) -> Box<dyn TokenEstimator> {
    if tiktoken_rs::tokenizer::get_tokenizer(model).is_some() {
        if let Ok(estimator) = TiktokenEstimator::for_model(model) {
            return Box::new(estimator);
        }
    }
    Box::new(HeuristicEstimator::default())
}

fn role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

pub(crate) const fn message_framing_tokens() -> u64 {
    MESSAGE_FRAMING_TOKENS
}

pub(crate) const fn reply_primer_tokens() -> u64 {
    REPLY_PRIMER_TOKENS
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{MessageId, MessageMetadata, TextContent};

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
    fn tiktoken_counts_openai_model_precisely() {
        let est = TiktokenEstimator::for_model("gpt-4o").expect("gpt-4o tokenizer");
        assert_eq!(est.count_text("hello world"), 2);
        assert_eq!(est.estimator_kind(), "tiktoken");
    }

    #[test]
    fn default_estimator_routes_by_model() {
        assert_eq!(default_estimator_for("gpt-4o").estimator_kind(), "tiktoken");
        assert_eq!(
            default_estimator_for("claude-3-5-sonnet").estimator_kind(),
            "heuristic"
        );
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
        let part = ContentPart::ToolResult(agent_domain::ToolResultContent {
            tool_call_id: agent_domain::ToolCallId::from("c"),
            tool_name: Some("read_file".into()),
            content: vec![ContentPart::Text(TextContent {
                text: "abcdefgh".into(),
            })], // 2 tokens
            is_error: false,
            metadata: serde_json::Value::Null,
        });
        // tool_name(3) + nested text(2) + metadata "null"(1)
        assert_eq!(est.count_content_part(&part), 3 + 2 + 1);
    }

    #[test]
    fn reasoning_counts_only_safe_summary_not_protected_reference() {
        let est = HeuristicEstimator::new(4);
        let part = ContentPart::Reasoning(agent_domain::ReasoningItem {
            id: agent_domain::ReasoningItemId::from("reasoning-1"),
            summary: Some("abcdefgh".into()),
            protected_blob_ref: agent_domain::ProtectedBlobRef::from(
                "a-very-long-protected-reference-that-must-not-affect-token-count",
            ),
            opaque_metadata: Default::default(),
            continuation_metadata: Default::default(),
        });

        assert_eq!(est.count_content_part(&part), 2);
    }
}
