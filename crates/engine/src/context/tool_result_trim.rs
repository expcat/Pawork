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
//!
//! CU-12 请求侧观测图像裁剪（[`trim_observation_images`]）：多轮 computer-use
//! 循环里每张截图都会以 base64 留在工具消息中，token 估算器对图像只计 85
//! placeholder token，既有压缩 / 截断感知不到图像字节膨胀。本模块因此在请求
//! 副本上单独按字节收敛截图历史：最新观测无条件保留，旧图像按
//! [`observation_image_budget_bytes`] 折算的预算保留，被裁剪图像原位替换为
//! 「图像存在但已裁剪」的文本说明。只改请求副本，持久事件 / 重放不受影响；
//! 不使用 `ArtifactRef` 占位（Chat / Responses 会丢弃 Artifact 类型图像来源）。

use pawork_domain::{
    ArtifactId, ArtifactReference, ContentPart, ImageSource, Message, MessageRole, TextContent,
    ToolResultContent,
};

use super::budget::ContextBudget;

/// 分级裁剪的字节阈值。
///
/// 阈值为闭区间边界（`<= small` 为小结果，`<= medium` 为中等，`<= large` 为大，
/// 否则为超大）。默认值参考常见 Coding Agent 体量：
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
        ContentPart::Video(video) => video.url.len() as u64,
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

// ---------- CU-12 请求侧观测图像裁剪 ----------

/// base64 图像文本按 4 字符 ≈ 1 token 折算（与 HeuristicEstimator 默认口径一致），
/// 把 [`ContextBudget`] 的 token 上限折算为请求体图像字节上限。
const OBSERVATION_IMAGE_CHARS_PER_TOKEN: u64 = 4;

/// 请求侧旧观测图像的保留预算（字节，按 base64 编码长度计）。
///
/// token 估算器把每张图像按 85 placeholder token 计数，反映不了 base64 在请求体中的
/// 真实体量，因此图像单独按字节设限：取输入硬上限的等值文本量
///（max_input_tokens × 4 字符/token）。最新一轮观测无条件保留、不占此预算。
pub fn observation_image_budget_bytes(budget: &ContextBudget) -> u64 {
    budget
        .max_input_tokens
        .saturating_mul(OBSERVATION_IMAGE_CHARS_PER_TOKEN)
}

/// [`trim_observation_images`] 的裁剪统计（字节均为 base64 编码长度，即请求体真实体量）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObservationImageTrim {
    /// 被替换为文本说明的图像数。
    pub trimmed_images: u64,
    /// 被裁剪图像的 base64 编码总字节。
    pub trimmed_bytes: u64,
    /// 请求中保留的图像数（含无条件保留的最新观测）。
    pub retained_images: u64,
    /// 保留图像的 base64 编码总字节。
    pub retained_bytes: u64,
}

/// CU-12 请求侧截图历史裁剪：只改请求副本，不动持久历史与事件。
///
/// 语义：
/// - 只处理 Tool 角色消息中 ToolResultContent 内的 [`ImageSource::Base64`] 图像
///   （用户附图、Url / Artifact 来源不动；Tool 消息顶层直接挂的图像也不动——
///   生产循环把工具结果统一包装为 ToolResult，顶层图像不出现，
///   既不裁剪也不参与最新观测判定）；
/// - 最后一条含图像的 Tool 消息（最新观测）的全部图像无条件保留、不占预算；
/// - 更早的图像从新到旧在 `retained_image_bytes` 预算内保留；
/// - 被裁剪的图像原位替换为文本说明（图像存在但已裁剪、完整图像留在会话历史），
///   同一 tool result 的文本部分（如观测 metadata）原样保留；
/// - 不使用 ArtifactRef 占位：Chat / Responses 会丢弃 Artifact 类型图像来源。
pub fn trim_observation_images(
    messages: &mut [Message],
    retained_image_bytes: u64,
) -> ObservationImageTrim {
    // 第一趟：按消息顺序枚举全部候选图像（枚举序即 occurrence 序号）。
    let mut occurrence_message: Vec<usize> = Vec::new();
    let mut occurrence_bytes: Vec<u64> = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        if message.role != MessageRole::Tool {
            continue;
        }
        // 只从顶层 ToolResult 进入：顶层直接挂的 Image 不在候选内。
        for part in &message.content {
            let ContentPart::ToolResult(result) = part else {
                continue;
            };
            collect_observation_images(
                &result.content,
                index,
                &mut occurrence_message,
                &mut occurrence_bytes,
            );
        }
    }
    if occurrence_bytes.is_empty() {
        return ObservationImageTrim::default();
    }

    let mut retained = vec![false; occurrence_bytes.len()];
    // 最新观测：最后一条含图像的 Tool 消息的全部图像无条件保留。
    let latest_message = occurrence_message[occurrence_message.len() - 1];
    for (ordinal, message_index) in occurrence_message.iter().enumerate() {
        if *message_index == latest_message {
            retained[ordinal] = true;
        }
    }
    // 更早的图像从新到旧在预算内保留（最新观测不占预算）。
    let mut budgeted_bytes = 0_u64;
    for ordinal in (0..occurrence_bytes.len()).rev() {
        if retained[ordinal] {
            continue;
        }
        let bytes = occurrence_bytes[ordinal];
        if budgeted_bytes.saturating_add(bytes) <= retained_image_bytes {
            retained[ordinal] = true;
            budgeted_bytes += bytes;
        }
    }

    // 第二趟：按同一枚举顺序把未保留图像原位替换为文本说明。
    let mut stats = ObservationImageTrim::default();
    let mut ordinal = 0_usize;
    for message in messages.iter_mut() {
        if message.role != MessageRole::Tool {
            continue;
        }
        for part in &mut message.content {
            let ContentPart::ToolResult(result) = part else {
                continue;
            };
            replace_trimmed_images(&mut result.content, &retained, &mut ordinal, &mut stats);
        }
    }
    stats
}

fn collect_observation_images(
    parts: &[ContentPart],
    message_index: usize,
    occurrence_message: &mut Vec<usize>,
    occurrence_bytes: &mut Vec<u64>,
) {
    for part in parts {
        match part {
            ContentPart::ToolResult(result) => collect_observation_images(
                &result.content,
                message_index,
                occurrence_message,
                occurrence_bytes,
            ),
            ContentPart::Image(image) => {
                if let ImageSource::Base64(value) = &image.source {
                    occurrence_message.push(message_index);
                    occurrence_bytes.push(u64::try_from(value.len()).unwrap_or(u64::MAX));
                }
            }
            _ => {}
        }
    }
}

fn replace_trimmed_images(
    parts: &mut [ContentPart],
    retained: &[bool],
    ordinal: &mut usize,
    stats: &mut ObservationImageTrim,
) {
    for part in parts.iter_mut() {
        match part {
            ContentPart::ToolResult(result) => {
                replace_trimmed_images(&mut result.content, retained, ordinal, stats)
            }
            ContentPart::Image(image) => {
                let ImageSource::Base64(value) = &image.source else {
                    continue;
                };
                let bytes = u64::try_from(value.len()).unwrap_or(u64::MAX);
                let keep = retained.get(*ordinal).copied().unwrap_or(false);
                *ordinal += 1;
                if keep {
                    stats.retained_images += 1;
                    stats.retained_bytes = stats.retained_bytes.saturating_add(bytes);
                } else {
                    stats.trimmed_images += 1;
                    stats.trimmed_bytes = stats.trimmed_bytes.saturating_add(bytes);
                    let decoded_bytes = bytes.saturating_mul(3) / 4;
                    let note = format!("[image trimmed: earlier tool result image ({}, ~{} bytes) removed from this request to fit the context budget; the full image is retained in session history]", image.media_type, decoded_bytes);
                    *part = ContentPart::Text(TextContent { text: note });
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use pawork_domain::{ImageContent, MessageId, ToolCallId, ToolResultContent};
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
            artifacts: Vec::new(),
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

    // ---------- CU-12 请求侧观测图像裁剪 ----------

    fn tool_message_with_image(id: &str, call_id: &str, text: &str, b64: &str) -> Message {
        Message {
            id: MessageId::from(id),
            role: MessageRole::Tool,
            content: vec![ContentPart::ToolResult(ToolResultContent {
                tool_call_id: ToolCallId::from(call_id),
                tool_name: Some("computer".into()),
                content: vec![
                    ContentPart::Text(TextContent { text: text.into() }),
                    ContentPart::Image(ImageContent {
                        source: ImageSource::Base64(b64.into()),
                        media_type: "image/jpeg".into(),
                        alt_text: None,
                    }),
                ],
                is_error: false,
                metadata: Value::Null,
                artifacts: Vec::new(),
            })],
            metadata: Default::default(),
        }
    }

    fn tool_result_parts(message: &Message) -> &[ContentPart] {
        match &message.content[0] {
            ContentPart::ToolResult(result) => &result.content,
            other => panic!("expected tool result, got {other:?}"),
        }
    }

    fn assert_trim_note(part: &ContentPart) {
        let ContentPart::Text(text) = part else {
            panic!("expected trim note, got {part:?}")
        };
        assert!(text.text.contains("image trimmed"));
        assert!(text.text.contains("retained in session history"));
    }

    #[test]
    fn observation_trim_keeps_latest_and_replaces_older_with_note() {
        // 三条 Tool 消息；第一条含嵌套 tool result 图像（子代理结果形态）。
        // 预算 0 → 只有最新一条的图像保留，其余（含嵌套）原位变文本说明。
        let mut nested = tool_message_with_image("ignored", "c1-nested", "nested obs", "dddd");
        let ContentPart::ToolResult(nested_result) = nested.content.remove(0) else {
            unreachable!()
        };
        let mut first = tool_message_with_image("m1", "c1", "obs one", "aaaa");
        match &mut first.content[0] {
            ContentPart::ToolResult(result) => {
                result.content.push(ContentPart::ToolResult(nested_result));
            }
            _ => unreachable!(),
        }
        let mut messages = vec![
            first,
            tool_message_with_image("m2", "c2", "obs two", "bbbb"),
            tool_message_with_image("m3", "c3", "obs three", "cccc"),
        ];

        let stats = trim_observation_images(&mut messages, 0);

        assert_eq!(stats.trimmed_images, 3);
        assert_eq!(stats.retained_images, 1);
        assert_eq!(stats.retained_bytes, 4);
        // m1：文本保留，顶层图像与嵌套图像都变成说明。
        let parts = tool_result_parts(&messages[0]);
        assert!(matches!(&parts[0], ContentPart::Text(t) if t.text == "obs one"));
        assert_trim_note(&parts[1]);
        let ContentPart::ToolResult(nested) = &parts[2] else {
            panic!("nested result lost")
        };
        assert_trim_note(&nested.content[1]);
        // m2：图像变说明。
        assert_trim_note(&tool_result_parts(&messages[1])[1]);
        // m3：最新观测原样保留。
        let latest = tool_result_parts(&messages[2]);
        assert!(matches!(&latest[1], ContentPart::Image(image) if image.source == ImageSource::Base64("cccc".into())));
    }

    #[test]
    fn observation_trim_retains_older_images_within_budget() {
        let mut messages = vec![
            tool_message_with_image("m1", "c1", "obs one", &"a".repeat(100)),
            tool_message_with_image("m2", "c2", "obs two", &"b".repeat(100)),
            tool_message_with_image("m3", "c3", "obs three", &"c".repeat(40)),
        ];

        // 预算 100：最新观测（m3，不占预算）+ m2（恰好 100）保留；m1 超预算被裁。
        let stats = trim_observation_images(&mut messages, 100);

        assert_eq!(stats.trimmed_images, 1);
        assert_eq!(stats.trimmed_bytes, 100);
        assert_eq!(stats.retained_images, 2);
        assert_eq!(stats.retained_bytes, 140);
        assert_trim_note(&tool_result_parts(&messages[0])[1]);
        assert!(matches!(&tool_result_parts(&messages[1])[1], ContentPart::Image(_)));
        assert!(matches!(&tool_result_parts(&messages[2])[1], ContentPart::Image(_)));
    }

    #[test]
    fn observation_trim_ignores_user_and_url_images() {
        let user = Message {
            id: MessageId::from("u1"),
            role: MessageRole::User,
            content: vec![ContentPart::Image(ImageContent {
                source: ImageSource::Base64("user-image".into()),
                media_type: "image/png".into(),
                alt_text: None,
            })],
            metadata: Default::default(),
        };
        let mut url_tool = tool_message_with_image("m1", "c1", "obs", "ffff");
        match &mut url_tool.content[0] {
            ContentPart::ToolResult(result) => {
                result.content.push(ContentPart::Image(ImageContent {
                    source: ImageSource::Url("https://example.test/ref.png".into()),
                    media_type: "image/png".into(),
                    alt_text: None,
                }));
            }
            _ => unreachable!(),
        }
        // Tool 消息顶层直接挂的图像不在候选内：不裁剪，也不参与最新观测判定。
        // 这条消息位置更靠后——若顶层图像被枚举，它会抢走「最新观测」并导致
        // m1 的 ToolResult 图像被裁（trimmed=1），断言随之失败。
        let top_level_only = Message {
            id: MessageId::from("m2"),
            role: MessageRole::Tool,
            content: vec![ContentPart::Image(ImageContent {
                source: ImageSource::Base64("top-level-image".into()),
                media_type: "image/jpeg".into(),
                alt_text: None,
            })],
            metadata: Default::default(),
        };
        let mut messages = vec![user, url_tool, top_level_only];
        let before = messages.to_vec();

        // 预算 0：唯一候选（m1 ToolResult 内的 Base64）即最新观测，保留；
        // 用户附图、Url 图像与顶层图像均不动。
        let stats = trim_observation_images(&mut messages, 0);

        assert_eq!(stats.trimmed_images, 0);
        assert_eq!(stats.retained_images, 1);
        assert_eq!(messages, before);
    }

    #[test]
    fn observation_image_budget_scales_with_input_ceiling() {
        let budget = ContextBudget::from_context_window(10_000, 2_000, 0);
        assert_eq!(observation_image_budget_bytes(&budget), 8_000 * 4);
        let saturated = ContextBudget::from_context_window(1_000, 2_000, 0);
        assert_eq!(observation_image_budget_bytes(&saturated), 0);
    }
}
