//! Anthropic thinking continuation 纯映射（P15-7）。
//!
//! Anthropic 的 extended-thinking 续传以两类 content block 出现：
//!
//! - `{"type":"thinking","thinking":"<明文推理>","signature":"<签名>"}` —— `signature`
//!   是续传凭证，属受保护材料；`thinking` 文本是用户可见推理，由
//!   [`ThinkingContent`](agent_domain::ThinkingContent) 独立承载，**不**进受保护载荷。
//! - `{"type":"redacted_thinking","data":"<Base64>"}` —— `data` 是被服务端遮蔽的
//!   续传凭证，整体受保护，无关联文本。
//!
//! 本模块只做三件无副作用的纯映射，绝不猜测缺失值：
//!
//! 1. [`extract_thinking_payload`]：从原始 block JSON 抽取受保护字符串，产出
//!    [`AnthropicThinkingPayload`]（待加密进 Protected Blob Store）。
//! 2. [`build_reasoning_item`]：给定调用方合成的 [`ReasoningItemId`] 与**已存**
//!    [`ProtectedBlobRef`]，产出仅含安全引用 + 非敏感 kind 提示的
//!    [`ReasoningItem`]。
//! 3. [`reconstruct_block`]：用 `ReasoningItem` + 关联 `ThinkingContent` + 解密
//!    载荷，逐字段精确重建原始 Anthropic block。
//!
//! 任何缺字段、缺关联或未知结构都返回
//! [`ReasoningMappingError::unsupported`](provider_api::ReasoningMappingError)；
//! 受保护字符串永不进入 canonical 事件 / `Debug` / 日志。

use std::collections::BTreeMap;
use std::fmt;

use agent_domain::{ProtectedBlobRef, ReasoningItem, ReasoningItemId, ThinkingContent};
use provider_api::ReasoningMappingError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// [`ReasoningItem::continuation_metadata`] 中记录 Anthropic block kind 的键。
///
/// 值为 `"thinking"` 或 `"redacted_thinking"`，是纯结构性翻译提示，不含任何
/// 凭证材料，重建时用于与解密载荷做一致性校验。
pub const ANTHROPIC_BLOCK_KIND_KEY: &str = "anthropic_block_kind";

/// 从 Anthropic thinking / redacted_thinking block 抽取的受保护载荷。
///
/// 这是写入 Protected Blob Store、经 XChaCha20-Poly1305 加密前的明文结构。
/// `signature` / `data` 是续传凭证，**不可**进入 canonical 事件、日志、GUI 或
/// OS Keychain；故本类型提供脱敏的 [`fmt::Debug`] 实现，且不实现
/// [`std::fmt::Display`]。
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "anthropic_block_kind", content = "protected")]
pub enum AnthropicThinkingPayload {
    /// `thinking` block 的受保护部分：仅 `signature`。
    /// `thinking` 文本由调用方经 [`ThinkingContent`] 独立关联。
    #[serde(rename = "thinking")]
    Thinking {
        /// Anthropic 下发的 `signature`，原样保留以供逐字节重建。
        signature: String,
    },
    /// `redacted_thinking` block 的受保护部分：服务端遮蔽后的 `data`。
    /// 不存在关联文本。
    #[serde(rename = "redacted_thinking")]
    Redacted {
        /// Anthropic 下发的 Base64 `data`，原样保留以供逐字节重建。
        data: String,
    },
}

impl AnthropicThinkingPayload {
    /// 返回对应的 Anthropic block `type` 字符串，用于一致性校验与元数据记录。
    pub fn kind(&self) -> &'static str {
        match self {
            AnthropicThinkingPayload::Thinking { .. } => "thinking",
            AnthropicThinkingPayload::Redacted { .. } => "redacted_thinking",
        }
    }
}

/// 脱敏 Debug：暴露 kind 以便排障，但 `signature` / `data` 永不出现。
impl fmt::Debug for AnthropicThinkingPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, field) = match self {
            AnthropicThinkingPayload::Thinking { .. } => ("thinking", "signature"),
            AnthropicThinkingPayload::Redacted { .. } => ("redacted_thinking", "data"),
        };
        formatter
            .debug_struct("AnthropicThinkingPayload")
            .field("kind", &kind)
            .field(field, &"[REDACTED]")
            .finish()
    }
}

/// 从一条 Anthropic content block 抽取受保护载荷。
///
/// 仅读取续传凭证字段（`thinking` → `signature`；`redacted_thinking` → `data`），
/// **不**读取 `thinking` 文本——文本由调用方经 `ThinkingDelta` /
/// [`ThinkingContent`] 独立流转。缺失 `type`、缺失凭证字段、字段类型不符或
/// 未知 `type` 一律返回 [`ReasoningMappingError::unsupported`]，绝不猜测。
pub fn extract_thinking_payload(
    block: &Value,
) -> Result<AnthropicThinkingPayload, ReasoningMappingError> {
    let kind = block
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ReasoningMappingError::unsupported("anthropic block missing `type`"))?;
    match kind {
        "thinking" => {
            let signature = block
                .get("signature")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ReasoningMappingError::unsupported(
                        "anthropic thinking block missing `signature`",
                    )
                })?;
            Ok(AnthropicThinkingPayload::Thinking {
                signature: signature.to_string(),
            })
        }
        "redacted_thinking" => {
            let data = block.get("data").and_then(Value::as_str).ok_or_else(|| {
                ReasoningMappingError::unsupported(
                    "anthropic redacted_thinking block missing `data`",
                )
            })?;
            Ok(AnthropicThinkingPayload::Redacted {
                data: data.to_string(),
            })
        }
        other => Err(ReasoningMappingError::unsupported(format!(
            "unsupported anthropic thinking block type `{other}`"
        ))),
    }
}

/// 给定调用方合成的 [`ReasoningItemId`] 与**已存** [`ProtectedBlobRef`]，产出
/// 仅含安全引用 + 非敏感 kind 提示的 [`ReasoningItem`]。
///
/// 不读取也不记录 `signature` / `data` 明文——它们只存在于已加密的 blob 中。
/// `kind` 来自 `payload`，写入 [`ReasoningItem::continuation_metadata`] 供重建时
/// 与解密载荷做一致性校验。
pub fn build_reasoning_item(
    id: ReasoningItemId,
    blob_ref: ProtectedBlobRef,
    payload: &AnthropicThinkingPayload,
) -> ReasoningItem {
    let mut continuation_metadata = BTreeMap::new();
    continuation_metadata.insert(
        ANTHROPIC_BLOCK_KIND_KEY.to_string(),
        Value::String(payload.kind().to_string()),
    );
    ReasoningItem {
        id,
        summary: None,
        protected_blob_ref: blob_ref,
        opaque_metadata: BTreeMap::new(),
        continuation_metadata,
    }
}

/// 用 [`ReasoningItem`] + 关联 [`ThinkingContent`] + 解密载荷，逐字段精确重建
/// 原始 Anthropic content block。
///
/// 三项输入各自贡献：item 提供 `kind`（须与解密载荷一致），`thinking` 文本来自
/// 关联的 `ThinkingContent`（`redacted_thinking` 无关联文本），`signature` /
/// `data` 来自解密载荷。任何缺关联、kind 不一致、redacted 误带关联文本或未知
/// kind 都返回 [`ReasoningMappingError::unsupported`]，绝不猜值。
pub fn reconstruct_block(
    item: &ReasoningItem,
    thinking: Option<&ThinkingContent>,
    payload: &AnthropicThinkingPayload,
) -> Result<Value, ReasoningMappingError> {
    let kind = item
        .continuation_metadata
        .get(ANTHROPIC_BLOCK_KIND_KEY)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ReasoningMappingError::unsupported(
                "reasoning item missing `anthropic_block_kind` continuation hint",
            )
        })?;
    if kind != payload.kind() {
        return Err(ReasoningMappingError::unsupported(
            "reasoning item kind does not match decrypted anthropic payload",
        ));
    }
    match payload {
        AnthropicThinkingPayload::Thinking { signature } => {
            let thinking = thinking.ok_or_else(|| {
                ReasoningMappingError::unsupported(
                    "anthropic thinking block requires an associated ThinkingContent for its text",
                )
            })?;
            Ok(json!({
                "type": "thinking",
                "thinking": thinking.text,
                "signature": signature,
            }))
        }
        AnthropicThinkingPayload::Redacted { data } => {
            if thinking.is_some() {
                return Err(ReasoningMappingError::unsupported(
                    "anthropic redacted_thinking block has no associated thinking text",
                ));
            }
            Ok(json!({
                "type": "redacted_thinking",
                "data": data,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::ThinkingContent;

    // ---- 真实形态 fixture ----

    fn thinking_fixture() -> Value {
        json!({
            "type": "thinking",
            "thinking": "Let me weigh the constraints before answering.",
            "signature": "EuYBCkQYAjJgWE4gNEZqT7s0m1p2V3a4b5c6signature==",
        })
    }

    fn redacted_fixture() -> Value {
        json!({
            "type": "redacted_thinking",
            "data": "EmwKAggBEhRKS2Lr7vN9o3wqZ+base64redacted==",
        })
    }

    // ---- extract：fixture ----

    #[test]
    fn extract_thinking_block_yields_signature_only() {
        let payload = extract_thinking_payload(&thinking_fixture()).expect("thinking extracts");
        assert_eq!(payload.kind(), "thinking");
        match &payload {
            AnthropicThinkingPayload::Thinking { signature } => {
                assert_eq!(signature, "EuYBCkQYAjJgWE4gNEZqT7s0m1p2V3a4b5c6signature==");
            }
            AnthropicThinkingPayload::Redacted { .. } => panic!("expected Thinking variant"),
        }
    }

    #[test]
    fn extract_redacted_block_yields_data_only() {
        let payload = extract_thinking_payload(&redacted_fixture()).expect("redacted extracts");
        assert_eq!(payload.kind(), "redacted_thinking");
        match &payload {
            AnthropicThinkingPayload::Redacted { data } => {
                assert_eq!(data, "EmwKAggBEhRKS2Lr7vN9o3wqZ+base64redacted==");
            }
            AnthropicThinkingPayload::Thinking { .. } => panic!("expected Redacted variant"),
        }
    }

    // ---- extract：absence / 未知结构 ----

    #[test]
    fn extract_missing_type_is_unsupported() {
        let block = json!({"thinking": "x", "signature": "y"});
        let err = extract_thinking_payload(&block).expect_err("missing type rejects");
        assert!(err.to_string().contains("missing `type`"));
        assert_secret_absent(&err.to_string());
    }

    #[test]
    fn extract_unknown_type_is_unsupported() {
        let block = json!({"type": "text", "text": "hi"});
        let err = extract_thinking_payload(&block).expect_err("unknown type rejects");
        assert!(err
            .to_string()
            .contains("unsupported anthropic thinking block type `text`"));
        assert_secret_absent(&err.to_string());
    }

    #[test]
    fn extract_thinking_missing_signature_is_unsupported() {
        let block = json!({"type": "thinking", "thinking": "no sig"});
        let err = extract_thinking_payload(&block).expect_err("missing signature rejects");
        assert!(err.to_string().contains("missing `signature`"));
        // thinking 文本不得进入错误信息
        assert!(!err.to_string().contains("no sig"));
    }

    #[test]
    fn extract_thinking_signature_wrong_type_is_unsupported() {
        let block = json!({"type": "thinking", "thinking": "x", "signature": 42});
        let err = extract_thinking_payload(&block).expect_err("non-string signature rejects");
        assert!(err.to_string().contains("missing `signature`"));
    }

    #[test]
    fn extract_redacted_missing_data_is_unsupported() {
        let block = json!({"type": "redacted_thinking"});
        let err = extract_thinking_payload(&block).expect_err("missing data rejects");
        assert!(err.to_string().contains("missing `data`"));
        assert_secret_absent(&err.to_string());
    }

    // ---- build_reasoning_item：canonical 只含安全引用 ----

    #[test]
    fn build_reasoning_item_records_kind_and_no_secret() {
        let payload = extract_thinking_payload(&thinking_fixture()).expect("extract");
        let item = build_reasoning_item(
            ReasoningItemId::from("reasoning-7"),
            ProtectedBlobRef::from("blob-anthropic-7"),
            &payload,
        );

        assert_eq!(item.id.as_str(), "reasoning-7");
        assert_eq!(item.protected_blob_ref.as_str(), "blob-anthropic-7");
        assert_eq!(
            item.continuation_metadata[ANTHROPIC_BLOCK_KIND_KEY],
            Value::String("thinking".into()),
        );
        assert!(item.summary.is_none());

        // 序列化后的 canonical 项不得携带任何凭证字段。
        let encoded = serde_json::to_string(&item).expect("serialize item");
        assert!(encoded.contains("blob-anthropic-7"));
        assert!(encoded.contains("anthropic_block_kind"));
        assert_secret_absent(&encoded);
        // Debug 也不得泄露。
        assert_secret_absent(&format!("{item:?}"));
    }

    // ---- reconstruct：exact roundtrip ----

    #[test]
    fn thinking_block_round_trips_exactly() {
        let original = thinking_fixture();
        let payload = extract_thinking_payload(&original).expect("extract");
        let item = build_reasoning_item(
            ReasoningItemId::from("reasoning-1"),
            ProtectedBlobRef::from("blob-1"),
            &payload,
        );
        let associated = ThinkingContent {
            text: original
                .get("thinking")
                .and_then(Value::as_str)
                .unwrap()
                .to_string(),
            reasoning_item_id: Some(item.id.clone()),
            redacted: false,
        };

        let rebuilt = reconstruct_block(&item, Some(&associated), &payload).expect("reconstruct");
        assert_eq!(rebuilt, original);

        // 重建结果可被再次抽取，且再重建保持不变（幂等）。
        let payload2 = extract_thinking_payload(&rebuilt).expect("re-extract");
        let rebuilt2 =
            reconstruct_block(&item, Some(&associated), &payload2).expect("re-reconstruct");
        assert_eq!(rebuilt2, original);
    }

    #[test]
    fn redacted_block_round_trips_exactly_without_thinking() {
        let original = redacted_fixture();
        let payload = extract_thinking_payload(&original).expect("extract");
        let item = build_reasoning_item(
            ReasoningItemId::from("reasoning-2"),
            ProtectedBlobRef::from("blob-2"),
            &payload,
        );

        let rebuilt = reconstruct_block(&item, None, &payload).expect("reconstruct");
        assert_eq!(rebuilt, original);

        let payload2 = extract_thinking_payload(&rebuilt).expect("re-extract");
        let rebuilt2 = reconstruct_block(&item, None, &payload2).expect("re-reconstruct");
        assert_eq!(rebuilt2, original);
    }

    // ---- reconstruct：absence / 不一致 ----

    #[test]
    fn reconstruct_thinking_without_associated_thinking_is_unsupported() {
        let payload = extract_thinking_payload(&thinking_fixture()).expect("extract");
        let item = build_reasoning_item(
            ReasoningItemId::from("reasoning-3"),
            ProtectedBlobRef::from("blob-3"),
            &payload,
        );
        let err = reconstruct_block(&item, None, &payload).expect_err("missing thinking rejects");
        assert!(err
            .to_string()
            .contains("requires an associated ThinkingContent"));
        assert_secret_absent(&err.to_string());
    }

    #[test]
    fn reconstruct_redacted_with_associated_thinking_is_unsupported() {
        let payload = extract_thinking_payload(&redacted_fixture()).expect("extract");
        let item = build_reasoning_item(
            ReasoningItemId::from("reasoning-4"),
            ProtectedBlobRef::from("blob-4"),
            &payload,
        );
        let stray = ThinkingContent {
            text: "should not be here".into(),
            reasoning_item_id: Some(item.id.clone()),
            redacted: true,
        };
        let err =
            reconstruct_block(&item, Some(&stray), &payload).expect_err("stray thinking rejects");
        assert!(err.to_string().contains("no associated thinking text"));
        assert!(!err.to_string().contains("should not be here"));
    }

    #[test]
    fn reconstruct_missing_kind_hint_is_unsupported() {
        let payload = extract_thinking_payload(&thinking_fixture()).expect("extract");
        // 构造一个没有 kind 提示的 item（模拟元数据丢失 / 损坏）。
        let item = ReasoningItem {
            id: ReasoningItemId::from("reasoning-5"),
            summary: None,
            protected_blob_ref: ProtectedBlobRef::from("blob-5"),
            opaque_metadata: BTreeMap::new(),
            continuation_metadata: BTreeMap::new(),
        };
        let associated = ThinkingContent {
            text: String::new(),
            reasoning_item_id: Some(item.id.clone()),
            redacted: false,
        };
        let err = reconstruct_block(&item, Some(&associated), &payload)
            .expect_err("missing kind rejects");
        assert!(err.to_string().contains("missing `anthropic_block_kind`"));
        assert_secret_absent(&err.to_string());
    }

    #[test]
    fn reconstruct_kind_payload_mismatch_is_unsupported() {
        // item 记录的是 thinking kind，但传入 redacted 载荷。
        let thinking_payload =
            extract_thinking_payload(&thinking_fixture()).expect("extract thinking");
        let redacted_payload =
            extract_thinking_payload(&redacted_fixture()).expect("extract redacted");
        let item = build_reasoning_item(
            ReasoningItemId::from("reasoning-6"),
            ProtectedBlobRef::from("blob-6"),
            &thinking_payload,
        );
        let err = reconstruct_block(&item, None, &redacted_payload).expect_err("mismatch rejects");
        assert!(err
            .to_string()
            .contains("does not match decrypted anthropic payload"));
        assert_secret_absent(&err.to_string());
    }

    // ---- 受保护材料不进 Debug / 序列化 ----

    #[test]
    fn payload_debug_redacts_secret_material() {
        let thinking = extract_thinking_payload(&thinking_fixture()).expect("extract");
        let redacted = extract_thinking_payload(&redacted_fixture()).expect("extract");

        for debug in [format!("{thinking:?}"), format!("{redacted:?}")] {
            assert!(debug.contains("[REDACTED]"));
            assert_secret_absent(&debug);
        }
    }

    #[test]
    fn payload_serde_round_trips_for_blob_storage() {
        // 受保护载荷需可序列化进 Protected Blob Store 并无损取回（加密由存储层完成）。
        for payload in [
            extract_thinking_payload(&thinking_fixture()).expect("extract thinking"),
            extract_thinking_payload(&redacted_fixture()).expect("extract redacted"),
        ] {
            let encoded = serde_json::to_string(&payload).expect("serialize payload");
            let decoded: AnthropicThinkingPayload =
                serde_json::from_str(&encoded).expect("deserialize payload");
            assert_eq!(decoded.kind(), payload.kind());
        }
    }

    // 受保护明文片段：thinking 的 signature、redacted 的 data 都不得出现在
    // canonical / Debug / 错误信息中。
    fn assert_secret_absent(haystack: &str) {
        for forbidden in [
            "EuYBCkQYAjJgWE4gNEZqT7s0m1p2V3a4b5c6signature==",
            "EmwKAggBEhRKS2Lr7vN9o3wqZ+base64redacted==",
        ] {
            assert!(
                !haystack.contains(forbidden),
                "protected material leaked into: {haystack}"
            );
        }
    }
}
