//! xAI Responses wire → canonical ReasoningItem / 回灌映射（P15-7 夹具）。
//!
//! 只冻结 xAI Responses `reasoning` output item 的已确认字段映射，不接入现有
//! Chat Completions transport，也不在 Core 走 Provider 名称分支：
//!
//! | Responses `reasoning` output item | canonical `ReasoningItem` | 去向 |
//! |---|---|---|
//! | `type` = `"reasoning"` | 不映射 | 校验 |
//! | `id` | `id` | 安全引用 |
//! | `summary[]`（单个 `summary_text` 条目） | `summary` | 安全文本 |
//! | `encrypted_content`（opaque 字符串） | 不进入 item | 提取为 [`ProtectedContinuation`]，由调用方加密入 Protected Blob Store |
//!
//! 安全红线（P15-7）：`encrypted_content` 原文只作为不透明回灌材料存在，绝不
//! 进入 [`ReasoningItem`] / Debug / 日志 / 错误详情；[`ParsedResponsesReasoning`]
//! 的 Debug 输出对受保护内容脱敏。缺失或无法确认的字段一律返回
//! [`ReasoningMappingError::unsupported`]，不猜值、不伪造 continuation。

use std::collections::BTreeMap;
use std::fmt;

use agent_domain::{ProtectedBlobRef, ReasoningItem, ReasoningItemId};
use provider_api::ReasoningMappingError;
use serde_json::Value;

/// 已确认的 xAI Responses `reasoning` output item 的受保护 continuation 原文。
///
/// 刻意不实现 `Serialize`，`Debug` 输出固定为 `<redacted>`，防止原文经格式化或
/// 日志泄漏。调用方经 [`ProtectedContinuation::as_str`] 读取后写入 Protected
/// Blob Store，再以返回的 blob ref 组装 [`ReasoningItem`]。
#[derive(Clone, PartialEq, Eq)]
pub struct ProtectedContinuation(String);

impl ProtectedContinuation {
    /// 以只读方式暴露 opaque continuation 原文（仅供写入 Protected Blob Store）。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 移出 opaque continuation 原文（仅供写入 Protected Blob Store）。
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for ProtectedContinuation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

/// 校验并提取一个 xAI Responses `reasoning` output item。
///
/// 三个已确认字段独立验证：`id` 必填字符串；`summary` 可选，仅接受空数组或单个
/// `summary_text` 条目（多条目未确认，返回 unsupported）；`encrypted_content`
/// 可选，仅接受字符串。其余字段（如 `status`、`content`）不消费、不猜值。
pub fn parse_responses_reasoning(
    item: &Value,
) -> Result<ParsedResponsesReasoning, ReasoningMappingError> {
    let item_type = required_str(item, "type", "reasoning item without `type`")?;
    if item_type != "reasoning" {
        return Err(ReasoningMappingError::unsupported(format!(
            "unmapped xAI Responses item type `{item_type}`"
        )));
    }

    let id =
        ReasoningItemId::new(required_str(item, "id", "reasoning item without `id`")?.to_owned());

    let summary = match item.get("summary") {
        None => None,
        Some(Value::Array(entries)) => match entries.as_slice() {
            [] => None,
            [entry] => {
                let entry_type =
                    required_str(entry, "type", "reasoning summary entry without `type`")?;
                if entry_type != "summary_text" {
                    return Err(ReasoningMappingError::unsupported(format!(
                        "unmapped xAI reasoning summary entry type `{entry_type}`"
                    )));
                }
                Some(
                    required_str(entry, "text", "reasoning summary entry without `text`")?
                        .to_owned(),
                )
            }
            _ => {
                return Err(ReasoningMappingError::unsupported(
                    "unmapped xAI reasoning summary with multiple entries",
                ))
            }
        },
        Some(_) => {
            return Err(ReasoningMappingError::unsupported(
                "reasoning summary is not an array",
            ))
        }
    };

    let protected = match item.get("encrypted_content") {
        None => None,
        Some(Value::String(content)) => Some(ProtectedContinuation(content.clone())),
        Some(_) => {
            return Err(ReasoningMappingError::unsupported(
                "reasoning encrypted_content is not a string",
            ))
        }
    };

    Ok(ParsedResponsesReasoning {
        id,
        summary,
        protected,
    })
}

/// 校验后的 xAI Responses `reasoning` output item 分解结果。
///
/// 受保护 continuation 以 [`ProtectedContinuation`] 持有，Debug 输出脱敏；字段
/// 保持私有，避免原文被意外写入 metadata 或日志。
#[derive(Clone)]
pub struct ParsedResponsesReasoning {
    id: ReasoningItemId,
    summary: Option<String>,
    protected: Option<ProtectedContinuation>,
}

impl ParsedResponsesReasoning {
    /// Responses reasoning item 的 `id`。
    pub fn id(&self) -> &ReasoningItemId {
        &self.id
    }

    /// 归一后的安全 summary 文本。
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    /// 提取到的 optional 受保护 continuation（`encrypted_content` 原文）。
    pub fn protected(&self) -> Option<&ProtectedContinuation> {
        self.protected.as_ref()
    }
}

impl fmt::Debug for ParsedResponsesReasoning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParsedResponsesReasoning")
            .field("id", &self.id)
            .field("summary", &self.summary)
            .field("protected", &"<redacted>")
            .finish()
    }
}

/// 以已存入 Protected Blob Store 的 blob ref 组装安全的 [`ReasoningItem`]。
///
/// 仅当解析结果确实携带受保护 continuation 时允许组装：缺少
/// `encrypted_content` 时返回 unsupported，绝不伪造 blob 引用指向不存在的
/// continuation。metadata 保持为空，不携带任何原文或猜测字段。
pub fn to_reasoning_item(
    parsed: ParsedResponsesReasoning,
    protected_blob_ref: ProtectedBlobRef,
) -> Result<ReasoningItem, ReasoningMappingError> {
    if parsed.protected.is_none() {
        return Err(ReasoningMappingError::unsupported(
            "reasoning item without encrypted continuation",
        ));
    }
    Ok(ReasoningItem {
        id: parsed.id,
        summary: parsed.summary,
        protected_blob_ref,
        opaque_metadata: BTreeMap::new(),
        continuation_metadata: BTreeMap::new(),
    })
}

/// 用 [`ReasoningItem`] 与从 Protected Blob Store 取回的明文重建 Responses
/// input reasoning item，用于跨轮回灌。
///
/// 只重建已确认的 input 字段：`type` / `id` / `summary`（存在时重建为单个
/// `summary_text` 条目）/ `encrypted_content`（解密值原样回填，不解析、不落
/// 日志）。调用方负责解密，本函数为纯映射。
pub fn to_responses_input_reasoning(item: &ReasoningItem, decrypted_content: &str) -> Value {
    let mut input = serde_json::json!({
        "type": "reasoning",
        "id": item.id.as_str(),
        "encrypted_content": decrypted_content,
    });
    if let Some(summary) = &item.summary {
        input["summary"] = serde_json::json!([{ "type": "summary_text", "text": summary }]);
    }
    input
}

fn required_str<'a>(
    value: &'a Value,
    key: &str,
    error: &str,
) -> Result<&'a str, ReasoningMappingError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ReasoningMappingError::unsupported(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "type": "reasoning",
        "id": "rs_67df8af07d3d4c43a4915c7dba2e0c1d",
        "status": "completed",
        "summary": [{"type": "summary_text", "text": "checked the constraints"}],
        "encrypted_content": "enc:opaque-continuation-bytes"
    }"#;

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).expect("fixture is valid JSON")
    }

    #[test]
    fn fixture_parses_only_confirmed_fields() {
        let parsed = parse_responses_reasoning(&fixture()).expect("parse fixture");
        assert_eq!(parsed.id().as_str(), "rs_67df8af07d3d4c43a4915c7dba2e0c1d");
        assert_eq!(parsed.summary(), Some("checked the constraints"));
        assert_eq!(
            parsed.protected().map(ProtectedContinuation::as_str),
            Some("enc:opaque-continuation-bytes")
        );
    }

    #[test]
    fn missing_encrypted_content_yields_no_protected_string() {
        let item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": []
        });
        let parsed = parse_responses_reasoning(&item).expect("parse without encrypted_content");
        assert!(parsed.protected().is_none());
        assert_eq!(parsed.summary(), None);

        // 没有受保护材料时，不得用 blob ref 伪造 continuation。
        let error = to_reasoning_item(parsed, ProtectedBlobRef::from("blob-1"))
            .expect_err("must not fabricate a continuation");
        assert!(error.to_string().contains("encrypted continuation"));
    }

    #[test]
    fn assembled_item_never_contains_the_secret() {
        let parsed = parse_responses_reasoning(&fixture()).expect("parse fixture");
        let item =
            to_reasoning_item(parsed, ProtectedBlobRef::from("blob-1")).expect("assemble item");
        assert_eq!(item.id.as_str(), "rs_67df8af07d3d4c43a4915c7dba2e0c1d");
        assert_eq!(item.summary.as_deref(), Some("checked the constraints"));
        assert_eq!(item.protected_blob_ref.as_str(), "blob-1");
        assert!(item.opaque_metadata.is_empty());
        assert!(item.continuation_metadata.is_empty());

        let serialized = serde_json::to_string(&item).expect("serialize item");
        for needle in ["enc:opaque-continuation-bytes", "opaque-continuation"] {
            assert!(!serialized.contains(needle));
            assert!(!format!("{item:?}").contains(needle));
        }
    }

    #[test]
    fn parsed_debug_redacts_the_protected_continuation() {
        let parsed = parse_responses_reasoning(&fixture()).expect("parse fixture");
        let debug = format!("{parsed:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("opaque-continuation"));
        assert!(!format!("{:?}", parsed.protected()).contains("opaque-continuation"));
    }

    #[test]
    fn rehydrate_rebuilds_confirmed_input_item_exactly() {
        let original = fixture();
        let parsed = parse_responses_reasoning(&original).expect("parse fixture");
        let item =
            to_reasoning_item(parsed, ProtectedBlobRef::from("blob-1")).expect("assemble item");
        let decrypted = "enc:opaque-continuation-bytes";

        let rebuilt = to_responses_input_reasoning(&item, decrypted);
        let expected = serde_json::json!({
            "type": "reasoning",
            "id": "rs_67df8af07d3d4c43a4915c7dba2e0c1d",
            "summary": [{"type": "summary_text", "text": "checked the constraints"}],
            "encrypted_content": "enc:opaque-continuation-bytes",
        });
        assert_eq!(rebuilt, expected);
        assert_eq!(rebuilt["encrypted_content"].as_str(), Some(decrypted));
    }

    #[test]
    fn rehydrate_without_summary_omits_the_summary_field() {
        let item = ReasoningItem {
            id: ReasoningItemId::from("rs_2"),
            summary: None,
            protected_blob_ref: ProtectedBlobRef::from("blob-2"),
            opaque_metadata: BTreeMap::new(),
            continuation_metadata: BTreeMap::new(),
        };
        let rebuilt = to_responses_input_reasoning(&item, "enc:opaque-continuation-bytes");
        assert_eq!(
            rebuilt,
            serde_json::json!({
                "type": "reasoning",
                "id": "rs_2",
                "encrypted_content": "enc:opaque-continuation-bytes",
            })
        );
    }

    #[test]
    fn missing_or_unknown_structures_are_unsupported() {
        let unknown_type = parse_responses_reasoning(&serde_json::json!({
            "type": "function_call",
            "id": "fc_1"
        }))
        .expect_err("unmapped item type");
        assert!(unknown_type.to_string().contains("function_call"));

        let missing_id = parse_responses_reasoning(&serde_json::json!({"type": "reasoning"}))
            .expect_err("missing id");
        assert!(missing_id.to_string().contains("id"));

        let non_string_secret = parse_responses_reasoning(&serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "encrypted_content": 42
        }))
        .expect_err("encrypted_content must be a string");
        assert!(non_string_secret.to_string().contains("encrypted_content"));

        let non_array_summary = parse_responses_reasoning(&serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": "nope"
        }))
        .expect_err("summary must be an array");
        assert!(non_array_summary.to_string().contains("summary"));

        let unknown_entry_type = parse_responses_reasoning(&serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{"type": "other", "text": "x"}]
        }))
        .expect_err("unmapped summary entry type");
        assert!(unknown_entry_type.to_string().contains("other"));

        let multiple_entries = parse_responses_reasoning(&serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [
                {"type": "summary_text", "text": "a"},
                {"type": "summary_text", "text": "b"}
            ]
        }))
        .expect_err("multiple summary entries are not confirmed");
        assert!(multiple_entries.to_string().contains("multiple entries"));
    }

    #[test]
    fn error_messages_never_carry_the_continuation_secret() {
        let item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "encrypted_content": {"junk": "TOP-SECRET-CONTENT"}
        });
        let error = parse_responses_reasoning(&item).expect_err("non-string secret");
        assert!(!error.to_string().contains("TOP-SECRET-CONTENT"));
    }
}
