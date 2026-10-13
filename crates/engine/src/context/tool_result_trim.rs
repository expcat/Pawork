//! CU-12 请求侧观测图像裁剪。
//!
//! 多轮 computer-use 循环里每张截图都会以 base64 留在工具消息中，token
//! 估算器对图像只计 85 placeholder token，既有压缩 / 截断感知不到图像字节膨胀。
//! 本模块在请求副本上按字节收敛截图历史：最新观测无条件保留，旧图像按
//! [`observation_image_budget_bytes`] 折算的预算保留，被裁剪图像原位替换为
//! 「图像存在但已裁剪」的文本说明。只改请求副本，持久事件 / 重放不受影响；
//! 不使用 `ArtifactRef` 占位（Chat / Responses 会丢弃 Artifact 类型图像来源）。

use pawork_domain::{ContentPart, ImageSource, Message, MessageRole, TextContent};

use super::budget::ContextBudget;

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
        assert!(
            matches!(&latest[1], ContentPart::Image(image) if image.source == ImageSource::Base64("cccc".into()))
        );
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
        assert!(matches!(
            &tool_result_parts(&messages[1])[1],
            ContentPart::Image(_)
        ));
        assert!(matches!(
            &tool_result_parts(&messages[2])[1],
            ContentPart::Image(_)
        ));
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
