use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ArtifactId, MessageId, ModelId, ProviderId, Timestamp, ToolCallId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub role: MessageRole,
    pub content: Vec<ContentPart>,
    #[serde(default)]
    pub metadata: MessageMetadata,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ContentPart {
    Text(TextContent),
    Image(ImageContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCallContent),
    ToolResult(ToolResultContent),
    ArtifactRef(ArtifactReference),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextContent {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageContent {
    pub source: ImageSource,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ImageSource {
    Artifact(ArtifactId),
    Url(String),
    Base64(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingContent {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default)]
    pub redacted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCallContent {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_arguments: Option<String>,
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResultContent {
    pub tool_call_id: ToolCallId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub content: Vec<ContentPart>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactReference {
    pub id: ArtifactId,
    pub media_type: String,
    pub byte_length: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(default)]
    pub incomplete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
}

impl TokenUsage {
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// 以微单位保存费用，避免浮点数在持久化与跨语言传输时产生误差。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cost {
    pub currency: String,
    pub amount_micros: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum StopReason {
    Completed,
    StopSequence,
    MaxTokens,
    ToolUse,
    ContentFiltered,
    Cancelled,
    Error,
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_content_part_round_trips_through_json() {
        let artifact = ArtifactReference {
            id: ArtifactId::from("artifact-1"),
            media_type: "text/plain".into(),
            byte_length: 42,
            content_hash: Some("blake3:abc".into()),
            label: Some("output".into()),
        };
        let parts = vec![
            ContentPart::Text(TextContent {
                text: "hello".into(),
            }),
            ContentPart::Image(ImageContent {
                source: ImageSource::Artifact(artifact.id.clone()),
                media_type: "image/png".into(),
                alt_text: Some("preview".into()),
            }),
            ContentPart::Thinking(ThinkingContent {
                text: "reasoning".into(),
                signature: Some("sig".into()),
                redacted: false,
            }),
            ContentPart::ToolCall(ToolCallContent {
                id: ToolCallId::from("call-1"),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "README.md"}),
                raw_arguments: None,
                complete: true,
            }),
            ContentPart::ToolResult(ToolResultContent {
                tool_call_id: ToolCallId::from("call-1"),
                tool_name: Some("read_file".into()),
                content: vec![ContentPart::Text(TextContent {
                    text: "body".into(),
                })],
                is_error: false,
                metadata: Value::Null,
            }),
            ContentPart::ArtifactRef(artifact),
        ];
        let message = Message {
            id: MessageId::from("message-1"),
            role: MessageRole::Assistant,
            content: parts,
            metadata: MessageMetadata {
                model: Some(ModelId::from("model-1")),
                provider: Some(ProviderId::from("provider-1")),
                usage: Some(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_read_tokens: 2,
                    cache_write_tokens: 1,
                }),
                cost: Some(Cost {
                    currency: "USD".into(),
                    amount_micros: 10,
                }),
                timestamp: Some(Timestamp::from_unix_millis(123)),
                stop_reason: Some(StopReason::ToolUse),
                incomplete: true,
                ..MessageMetadata::default()
            },
        };

        let json = serde_json::to_string(&message).expect("serialize message");
        let decoded: Message = serde_json::from_str(&json).expect("deserialize message");
        assert_eq!(decoded, message);
    }
}
