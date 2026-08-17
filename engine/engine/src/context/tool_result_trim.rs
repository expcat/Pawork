//! Tool Result 分级裁剪（自 V1 `context-engine::tool_result_trim` 整体迁入，形状不变）。
//!
//! 目的：避免超大 tool 输出无限进入上下文。按体量将 `ToolResultContent` 分为
//! 小 / 中 / 大 / 超大四级并确定性裁剪：小结果完整保留；中等结果保留头部 + 尾部 +
//! 截断说明；大结果转为摘要文本 + `ArtifactReference` 占位；超大结果仅保留元数据与
//! `ArtifactReference`。
//!
//! 完整原文通过 [`TrimmedToolResult::retained_full`] 暂存，便于按需回溯；真正写入
//! Blob Store 由调用方负责（本模块不依赖 `artifact-store`）。`ArtifactReference`
//! 中的 `ArtifactId` 由调用方提供的 [`TrimStrategy`] 决定（默认占位）。

use pawork_domain::{
    ArtifactId, ArtifactReference, ContentPart, ImageSource, TextContent, ToolResultContent,
};

/// 分级裁剪的字节阈值。
///
/// 阈值为闭区间边界（`<= small` 为小结果，`<= medium` 为中等，`<= large` 为大，
/// 否则为超大）。默认值参考已归档 V1 `../Pawork_v1/docs/features/context.md` 与常见 Coding Agent 体量：
/// 小 < 2 KiB，中等 < 16 KiB，大 < 256 KiB，超大 >= 256 KiB。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrimThresholds {
    pub small: u64,
    pub medium: u64,
    pub large: u64,
}

impl Default for TrimThresholds {
    fn default() -> Self {
        Self {
            small: 2 * 1024,
            medium: 16 * 1024,
            large: 256 * 1024,
        }
    }
}

/// 单条 tool result 的体量等级。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultSize {
    /// 小：完整保留。
    Small,
    /// 中：头部 + 尾部 + 截断说明。
    Medium,
    /// 大：摘要文本 + ArtifactReference 占位。
    Large,
    /// 超大：仅元数据 + ArtifactReference。
    Huge,
}

impl ResultSize {
    /// 按字节数与阈值分级。
    pub fn classify(byte_len: u64, thresholds: &TrimThresholds) -> Self {
        if byte_len <= thresholds.small {
            Self::Small
        } else if byte_len <= thresholds.medium {
            Self::Medium
        } else if byte_len <= thresholds.large {
            Self::Large
        } else {
            Self::Huge
        }
    }
}

/// 裁剪后的 tool result。原始字段从 `ToolResultContent` 透传，`content` 被替换为
/// 裁剪后版本；超大输出原文经 `retained_full` 暂存以便回溯（写入 Blob 由调用方负责）。
#[derive(Clone, Debug, PartialEq)]
pub struct TrimmedToolResult {
    pub tool_call_id: pawork_domain::ToolCallId,
    pub tool_name: Option<String>,
    /// 裁剪后进入上下文的内容（可能含 `ArtifactRef` 占位）。
    pub content: Vec<ContentPart>,
    pub is_error: bool,
    pub metadata: serde_json::Value,
    /// 分级结果。
    pub size: ResultSize,
    /// 原始字节长度（裁剪前）。
    pub original_byte_len: u64,
    /// 被折叠进 Artifact 的完整载荷（仅大 / 超大有值）；纯文本保持原文，含非文本
    /// content 时保存 content parts 的 JSON，调用方可据此写 Blob。
    pub retained_full: Option<String>,
}

/// 控制 Artifact 占位如何生成。
///
/// 默认 [`TrimStrategy::Placeholder`] 使用占位 `ArtifactId`，调用方可在写完 Blob 后
/// 替换为真实 id 与哈希。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrimStrategy {
    #[default]
    /// 使用占位 ArtifactId（`artifact:trimmed-tool-result`）。
    Placeholder,
}

const PLACEHOLDER_ARTIFACT_ID: &str = "artifact:trimmed-tool-result";
const PLACEHOLDER_MEDIA_TYPE: &str = "text/plain";
/// 中等结果头部 / 尾部分别保留的字节数（各占可用窗口的一半）。
const MEDIUM_HALF_WINDOW: u64 = 2 * 1024;
/// 无法得知实际字节数的图片采用保守固定成本，避免二进制为主的结果误判为 Small。
const IMAGE_ESTIMATED_BYTES: u64 = 64 * 1024;

/// 估算一条 `ToolResultContent` 的载荷字节数。
///
/// 文本按 UTF-8 字节数统计；`ArtifactRef` 使用其声明长度；图片使用固定成本并为
/// base64 加上近似解码长度。这样即使结果几乎不含文本，仍能进入正确裁剪等级。
pub fn byte_len_of_tool_result(result: &ToolResultContent) -> u64 {
    let mut total = 0u64;
    for part in &result.content {
        total = total.saturating_add(content_part_byte_len(part));
    }
    total
}

fn content_part_byte_len(part: &ContentPart) -> u64 {
    match part {
        ContentPart::Text(text) => u64::try_from(text.text.len()).unwrap_or(u64::MAX),
        ContentPart::Thinking(thinking) => u64::try_from(thinking.text.len()).unwrap_or(u64::MAX),
        ContentPart::Reasoning(reasoning) => serde_json::to_vec(reasoning)
            .map(|encoded| u64::try_from(encoded.len()).unwrap_or(u64::MAX))
            .unwrap_or(0),
        ContentPart::Image(image) => {
            let encoded_payload = match &image.source {
                ImageSource::Base64(value) => u64::try_from(value.len())
                    .unwrap_or(u64::MAX)
                    .saturating_mul(3)
                    .div_ceil(4),
                ImageSource::Artifact(_) | ImageSource::Url(_) => 0,
            };
            IMAGE_ESTIMATED_BYTES.saturating_add(encoded_payload)
        }
        ContentPart::ToolCall(call) => {
            let arguments = serde_json::to_string(&call.arguments).unwrap_or_default();
            u64::try_from(call.name.len())
                .unwrap_or(u64::MAX)
                .saturating_add(u64::try_from(arguments.len()).unwrap_or(u64::MAX))
                .saturating_add(
                    call.raw_arguments
                        .as_ref()
                        .map(|raw| u64::try_from(raw.len()).unwrap_or(u64::MAX))
                        .unwrap_or(0),
                )
        }
        ContentPart::ToolResult(nested) => byte_len_of_tool_result(nested),
        ContentPart::ArtifactRef(reference) => reference.byte_length,
    }
}

/// 提取一条 tool result 内所有文本内容的合并字符串（按出现顺序拼接）。
fn collect_text(result: &ToolResultContent) -> String {
    let mut buf = String::new();
    for part in &result.content {
        collect_text_part(part, &mut buf);
    }
    buf
}

fn collect_text_part(part: &ContentPart, buf: &mut String) {
    match part {
        ContentPart::Text(text) => buf.push_str(&text.text),
        ContentPart::ToolResult(nested) => buf.push_str(&collect_text(nested)),
        _ => {}
    }
}

fn retained_full_payload(result: &ToolResultContent, text: String) -> String {
    if result.content.iter().any(contains_non_text_part) {
        serde_json::to_string(&result.content).unwrap_or(text)
    } else {
        text
    }
}

fn contains_non_text_part(part: &ContentPart) -> bool {
    match part {
        ContentPart::Text(_) => false,
        ContentPart::ToolResult(nested) => nested.content.iter().any(contains_non_text_part),
        _ => true,
    }
}

/// 依据阈值与策略裁剪一条 tool result，确定性地产出 [`TrimmedToolResult`]。
///
/// 边界：阈值取闭区间；`byte_len` 取自 [`byte_len_of_tool_result`]。空内容视为小结果
/// 完整保留。
pub fn trim_tool_result(
    result: &ToolResultContent,
    thresholds: &TrimThresholds,
) -> TrimmedToolResult {
    trim_tool_result_with(result, thresholds, TrimStrategy::default())
}

/// 同 [`trim_tool_result`]，但允许指定 [`TrimStrategy`]。
pub fn trim_tool_result_with(
    result: &ToolResultContent,
    thresholds: &TrimThresholds,
    _strategy: TrimStrategy,
) -> TrimmedToolResult {
    let original_byte_len = byte_len_of_tool_result(result);
    let size = ResultSize::classify(original_byte_len, thresholds);

    let (content, retained_full) = match size {
        ResultSize::Small => (result.content.clone(), None),
        ResultSize::Medium => {
            let full = collect_text(result);
            let window = std::cmp::min(MEDIUM_HALF_WINDOW, full.len() as u64);
            let window = usize::try_from(window).unwrap_or(0);
            let head: String = full.chars().take(window).collect();
            let tail: String = full
                .chars()
                .rev()
                .take(window)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let note = format!(
                "\n\n[output truncated: {} bytes total, showing first/last {} chars each]",
                original_byte_len, window
            );
            let combined = format!("{head}\n…\n{tail}{note}");
            (
                vec![ContentPart::Text(TextContent { text: combined })],
                None,
            )
        }
        ResultSize::Large | ResultSize::Huge => {
            let full = collect_text(result);
            let retained_full = retained_full_payload(result, full.clone());
            let reference = ArtifactReference {
                id: ArtifactId::from(PLACEHOLDER_ARTIFACT_ID),
                media_type: PLACEHOLDER_MEDIA_TYPE.into(),
                byte_length: original_byte_len,
                content_hash: None,
                label: Some(if size == ResultSize::Huge {
                    "trimmed tool output (metadata only)".into()
                } else {
                    "trimmed tool output".into()
                }),
            };
            let mut content = Vec::new();
            // 大结果保留一段摘要文本 + Artifact 引用；超大结果仅保留引用。
            if size == ResultSize::Large {
                let summary_window = std::cmp::min(MEDIUM_HALF_WINDOW, full.len() as u64);
                let summary_window = usize::try_from(summary_window).unwrap_or(0);
                let summary: String = full.chars().take(summary_window).collect();
                let summary_text = format!(
                    "{summary}…\n\n[full output ({} bytes) moved to artifact]",
                    original_byte_len
                );
                content.push(ContentPart::Text(TextContent { text: summary_text }));
            }
            content.push(ContentPart::ArtifactRef(reference));
            (content, Some(retained_full))
        }
    };

    TrimmedToolResult {
        tool_call_id: result.tool_call_id.clone(),
        tool_name: result.tool_name.clone(),
        content,
        is_error: result.is_error,
        metadata: result.metadata.clone(),
        size,
        original_byte_len,
        retained_full,
    }
}

#[cfg(test)]
mod tests {
    use pawork_domain::{ImageContent, ToolCallId, ToolResultContent};
    use serde_json::Value;

    use super::*;

    fn text_part(s: &str) -> ContentPart {
        ContentPart::Text(TextContent { text: s.into() })
    }

    fn result_with(content: Vec<ContentPart>) -> ToolResultContent {
        ToolResultContent {
            tool_call_id: ToolCallId::from("call-1"),
            tool_name: Some("run_command".into()),
            content,
            is_error: false,
            metadata: Value::Null,
        }
    }

    fn kb(k: usize) -> String {
        "x".repeat(k * 1024)
    }

    #[test]
    fn small_result_is_kept_intact() {
        let thresholds = TrimThresholds::default();
        let result = result_with(vec![text_part("hello")]);
        let trimmed = trim_tool_result(&result, &thresholds);
        assert_eq!(trimmed.size, ResultSize::Small);
        assert_eq!(trimmed.content, result.content);
        assert!(trimmed.retained_full.is_none());
        assert_eq!(trimmed.original_byte_len, 5);
    }

    #[test]
    fn exactly_small_boundary_is_small() {
        let thresholds = TrimThresholds::default();
        let body = kb(2); // == small threshold
        let result = result_with(vec![text_part(&body)]);
        let trimmed = trim_tool_result(&result, &thresholds);
        assert_eq!(trimmed.size, ResultSize::Small);
        assert!(trimmed.retained_full.is_none());
    }

    #[test]
    fn medium_result_is_head_tail_with_note() {
        let thresholds = TrimThresholds::default();
        // 5 KiB：介于 small(2KiB) 与 medium(16KiB) 之间。
        let body = kb(5);
        let result = result_with(vec![text_part(&body)]);
        let trimmed = trim_tool_result(&result, &thresholds);
        assert_eq!(trimmed.size, ResultSize::Medium);
        assert!(trimmed.retained_full.is_none());
        assert_eq!(trimmed.content.len(), 1);
        let combined = match &trimmed.content[0] {
            ContentPart::Text(t) => &t.text,
            _ => panic!("expected text part"),
        };
        assert!(combined.contains("…"));
        assert!(combined.contains("[output truncated"));
    }

    #[test]
    fn large_result_becomes_summary_plus_artifact_ref() {
        let thresholds = TrimThresholds::default();
        // 64 KiB：介于 medium(16KiB) 与 large(256KiB) 之间。
        let body = kb(64);
        let result = result_with(vec![text_part(&body)]);
        let trimmed = trim_tool_result(&result, &thresholds);
        assert_eq!(trimmed.size, ResultSize::Large);
        // 文本摘要 + Artifact 引用
        assert_eq!(trimmed.content.len(), 2);
        assert!(trimmed.retained_full.is_some());
        assert_eq!(trimmed.retained_full.as_ref().unwrap().len(), body.len());
        let has_artifact = matches!(trimmed.content.last(), Some(ContentPart::ArtifactRef(_)));
        assert!(has_artifact, "expected ArtifactRef placeholder");
    }

    #[test]
    fn huge_result_is_metadata_only_with_artifact_ref() {
        let thresholds = TrimThresholds::default();
        // 1 MiB：超过 large(256KiB)。
        let body = kb(1024);
        let result = result_with(vec![text_part(&body)]);
        let trimmed = trim_tool_result(&result, &thresholds);
        assert_eq!(trimmed.size, ResultSize::Huge);
        assert_eq!(trimmed.content.len(), 1);
        assert!(matches!(trimmed.content[0], ContentPart::ArtifactRef(_)));
        assert_eq!(trimmed.retained_full.as_ref().unwrap().len(), body.len());
    }

    #[test]
    fn classification_is_deterministic_and_ordered() {
        let thresholds = TrimThresholds::default();
        assert_eq!(ResultSize::classify(0, &thresholds), ResultSize::Small);
        assert_eq!(
            ResultSize::classify(thresholds.small, &thresholds),
            ResultSize::Small
        );
        assert_eq!(
            ResultSize::classify(thresholds.small + 1, &thresholds),
            ResultSize::Medium
        );
        assert_eq!(
            ResultSize::classify(thresholds.medium, &thresholds),
            ResultSize::Medium
        );
        assert_eq!(
            ResultSize::classify(thresholds.medium + 1, &thresholds),
            ResultSize::Large
        );
        assert_eq!(
            ResultSize::classify(thresholds.large, &thresholds),
            ResultSize::Large
        );
        assert_eq!(
            ResultSize::classify(thresholds.large + 1, &thresholds),
            ResultSize::Huge
        );
    }

    #[test]
    fn empty_result_is_small_and_intact() {
        let thresholds = TrimThresholds::default();
        let result = result_with(vec![]);
        let trimmed = trim_tool_result(&result, &thresholds);
        assert_eq!(trimmed.size, ResultSize::Small);
        assert!(trimmed.content.is_empty());
        assert_eq!(trimmed.original_byte_len, 0);
    }

    #[test]
    fn image_only_result_is_not_misclassified_as_small() {
        let thresholds = TrimThresholds::default();
        let result = result_with(vec![ContentPart::Image(ImageContent {
            source: ImageSource::Base64("aGVsbG8=".into()),
            media_type: "image/png".into(),
            alt_text: None,
        })]);

        let trimmed = trim_tool_result(&result, &thresholds);
        assert_eq!(trimmed.size, ResultSize::Large);
        assert!(trimmed.original_byte_len >= IMAGE_ESTIMATED_BYTES);
        assert!(trimmed
            .retained_full
            .as_deref()
            .expect("serialized image payload")
            .contains("aGVsbG8="));
    }
}
