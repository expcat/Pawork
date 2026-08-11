//! OpenAI Responses reasoning item wire to/from canonical 映射（P15-7）。
//!
//! 本模块只冻结 P15-2 将消费的字段映射：从 Responses API reasoning output item
//! 提取 `encrypted_content`（Protected Blob Store 材料），并把归一后的 reasoning
//! continuation 翻译为 canonical [`ReasoningItem`]；回灌时根据 canonical item +
//! 已解密凭证重建 Responses input reasoning item。
//!
//! 安全红线（ADR-032）：`encrypted_content` 原文只能进入 Protected Blob Store；
//! 绝不进入 canonical item / Debug / 日志 / GUI / OS Keychain。无法无损映射的
//! 字段统一返回 [`ReasoningMappingError::unsupported`]，绝不猜值。
//!
//! 口径依据：OpenAI Responses reasoning item `id` / `summary[]` /
//! `encrypted_content`。`id` 与安全 summary 进入 [`ReasoningItem`]；
//! `encrypted_content` 原文加密入 Protected Blob；缺 `encrypted_content` 时
//! 不伪造 continuation（见 docs/features/providers.md）。

use std::collections::BTreeMap;
use std::fmt;

use agent_domain::{ProtectedBlobRef, ReasoningItem, ReasoningItemId};
use provider_api::ReasoningMappingError;
use serde_json::Value;

/// Responses `reasoning.encrypted_content` 的不透明回灌材料。
///
/// 调用方拿到后必须直接交给 Protected Blob Store；明文不暴露到 [`fmt::Debug`]、
/// 事件、日志或 GUI。仅 Provider 受信运行时可在构造回灌请求时读取。
#[derive(Clone, PartialEq, Eq)]
pub struct EncryptedContent(String);

impl EncryptedContent {
    /// 仅 Protected Blob Store 写入路径读取明文。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 消耗 self 并返回明文所有权，便于直接交给 Protected Blob Store。
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for EncryptedContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedContent")
            .field("byte_len", &self.0.len())
            .finish()
    }
}

/// `opaque_metadata` 中镜像原始 summary 条目数组的 key。
///
/// 仅用于 provider crate 内 round-trip 保真；Core 不按 key 名分支。条目只含
/// 安全字段 `type` / `text`，绝不含凭证。
const SUMMARY_ENTRIES_KEY: &str = "openai.responses.summary_entries";

/// Responses reasoning item 的固定 `type`。
const REASONING_ITEM_TYPE: &str = "reasoning";

/// 从一个 Responses reasoning output item 校验并提取 optional
/// `encrypted_content`。
///
/// - 字段缺省或为 `null`：返回 `Ok(None)`（caller 视作无 continuation 材料）；
/// - 字段为非空字符串：返回 `Ok(Some(_))`；
/// - 字段为空字符串、数组、对象、数字、布尔等不能无损表达的口径：返回
///   [`ReasoningMappingError::unsupported`]，绝不猜值。
///
/// 返回值只允许进入 Protected Blob Store；不可进入事件 / 日志 / GUI / Keychain。
pub fn extract_encrypted_content(
    item: &Value,
) -> Result<Option<EncryptedContent>, ReasoningMappingError> {
    validated_encrypted_content(item)
        .map(|content| content.map(|value| EncryptedContent(value.to_owned())))
}

/// 把 Responses reasoning output item 翻译为 canonical [`ReasoningItem`]。
///
/// `blob_ref` 由调用方在把 [`extract_encrypted_content`] 的结果写入 Protected
/// Blob Store 后取得；本函数绝不读取或拷贝 `encrypted_content`。`id` 与安全
/// summary 进入 canonical item；summary 条目按原结构（仅 `type` / `text`）镜像
/// 到 `opaque_metadata`，保证 round-trip 保真。无法无损映射的结构返回
/// [`ReasoningMappingError::unsupported`]。
pub fn responses_reasoning_to_canonical(
    item: &Value,
    blob_ref: ProtectedBlobRef,
) -> Result<ReasoningItem, ReasoningMappingError> {
    let item_type = required_str(item, "type", "reasoning item without `type`")?;
    if item_type != REASONING_ITEM_TYPE {
        return Err(ReasoningMappingError::unsupported(format!(
            "unmapped Responses item type `{item_type}`"
        )));
    }

    let id = required_str(item, "id", "reasoning item without `id`")?;
    let summary_entries = collect_summary_entries(item)?;
    if validated_encrypted_content(item)?.is_none() {
        return Err(ReasoningMappingError::unsupported(
            "reasoning item without encrypted continuation",
        ));
    }

    let summary = if summary_entries.is_empty() {
        None
    } else {
        let mut joined = String::new();
        for (index, entry) in summary_entries.iter().enumerate() {
            // collect_summary_entries 已保证 `text` 是非空字符串。
            let text = entry
                .get("text")
                .and_then(Value::as_str)
                .expect("summary entry text validated by collect_summary_entries");
            if index > 0 {
                joined.push('\n');
            }
            joined.push_str(text);
        }
        Some(joined)
    };

    let mut opaque_metadata = BTreeMap::new();
    if !summary_entries.is_empty() {
        opaque_metadata.insert(SUMMARY_ENTRIES_KEY.into(), Value::Array(summary_entries));
    }

    Ok(ReasoningItem {
        id: ReasoningItemId::from(id.to_owned()),
        summary,
        protected_blob_ref: blob_ref,
        opaque_metadata,
        // OpenAI 回灌只需 id + summary + encrypted_content，无需非敏感续传提示。
        continuation_metadata: BTreeMap::new(),
    })
}

/// 把 canonical [`ReasoningItem`] + 已解密的 `encrypted_content` 重建为
/// Responses input reasoning item。
///
/// 只能由 Provider 受信运行时调用：解密后的凭证只出现在返回的 JSON 中，由
/// adapter 直接放进下一次 Responses 请求的 `input` 数组；不进入事件 / 日志 /
/// GUI / Keychain。
///
/// `summary[]` 严格来自 [`responses_reasoning_to_canonical`] 镜像在
/// `opaque_metadata` 的结构化条目；canonical `summary` 字段不参与重建，避免
/// 把单字符串猜成数组形状。空 `decrypted_content` 或 `opaque_metadata` 中
/// summary 条目结构损坏时返回 [`ReasoningMappingError::unsupported`]。
pub fn canonical_reasoning_to_responses_input(
    item: &ReasoningItem,
    decrypted_content: &str,
) -> Result<Value, ReasoningMappingError> {
    if decrypted_content.is_empty() {
        return Err(ReasoningMappingError::unsupported(
            "cannot rehydrate reasoning item with empty decrypted content",
        ));
    }

    let summary_entries = match item.opaque_metadata.get(SUMMARY_ENTRIES_KEY) {
        None => Vec::new(),
        Some(Value::Array(entries)) => validate_summary_entries(entries)?,
        Some(_) => {
            return Err(ReasoningMappingError::unsupported(
                "reasoning opaque_metadata summary entries is malformed",
            ))
        }
    };

    Ok(serde_json::json!({
        "type": REASONING_ITEM_TYPE,
        "id": item.id.as_str(),
        "summary": summary_entries,
        "encrypted_content": decrypted_content,
    }))
}

fn collect_summary_entries(item: &Value) -> Result<Vec<Value>, ReasoningMappingError> {
    match item.get("summary") {
        None => Ok(Vec::new()),
        Some(Value::Array(entries)) => validate_summary_entries(entries),
        Some(_) => Err(ReasoningMappingError::unsupported(
            "reasoning.summary must be an array when present",
        )),
    }
}

fn validate_summary_entries(entries: &[Value]) -> Result<Vec<Value>, ReasoningMappingError> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let object = entry.as_object().ok_or_else(|| {
            ReasoningMappingError::unsupported("reasoning.summary entry must be an object")
        })?;
        if object.keys().any(|key| key != "type" && key != "text") {
            return Err(ReasoningMappingError::unsupported(
                "reasoning.summary entry contains an unmapped field",
            ));
        }
        let entry_type = required_str(entry, "type", "reasoning.summary entry missing `type`")?;
        if entry_type != "summary_text" {
            return Err(ReasoningMappingError::unsupported(
                "unmapped reasoning.summary entry type",
            ));
        }
        let text = required_str(
            entry,
            "text",
            "reasoning.summary entry missing string `text`",
        )?;
        out.push(serde_json::json!({"type": "summary_text", "text": text}));
    }
    Ok(out)
}

fn validated_encrypted_content(item: &Value) -> Result<Option<&str>, ReasoningMappingError> {
    match item.get("encrypted_content") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value)),
        Some(Value::String(_)) => Err(ReasoningMappingError::unsupported(
            "reasoning.encrypted_content is present but empty",
        )),
        Some(_) => Err(ReasoningMappingError::unsupported(
            "reasoning.encrypted_content must be a string when present",
        )),
    }
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

    /// 构造一条与 docs/features/providers.md OpenAI Responses reasoning 口径
    /// 对齐的 wire fixture；三家补齐后可横向比较 canonical 形状。
    ///
    /// `encrypted_content` 是受保护明文，仅出现在本 fixture 中用于验证提取与
    /// 隔离；绝不应进入 canonical item / Debug / 日志。
    fn responses_reasoning_fixture() -> Value {
        serde_json::json!({
            "type": "reasoning",
            "id": "rs_abc123",
            "summary": [
                {"type": "summary_text", "text": "考虑了天气约束"},
                {"type": "summary_text", "text": "确认了工具可用"}
            ],
            "encrypted_content": "opaque-continuation-bytes",
            "status": "completed"
        })
    }

    // ---------------- extract_encrypted_content ----------------

    #[test]
    fn extract_returns_some_for_nonempty_string() {
        let item = serde_json::json!({"encrypted_content": "abc"});
        let extracted = extract_encrypted_content(&item).expect("extract").unwrap();
        assert_eq!(extracted.as_str(), "abc");
    }

    #[test]
    fn extract_returns_none_when_field_absent() {
        let item = serde_json::json!({"type": "reasoning"});
        assert!(extract_encrypted_content(&item).expect("extract").is_none());
    }

    #[test]
    fn extract_returns_none_for_explicit_null() {
        let item = serde_json::json!({"encrypted_content": null});
        assert!(extract_encrypted_content(&item).expect("extract").is_none());
    }

    #[test]
    fn extract_rejects_empty_string_without_guessing() {
        let item = serde_json::json!({"encrypted_content": ""});
        let error = extract_encrypted_content(&item).expect_err("empty must be rejected");
        assert!(error.to_string().contains("empty"));
    }

    #[test]
    fn extract_rejects_non_string_shapes() {
        for value in [
            serde_json::json!({"encrypted_content": 42}),
            serde_json::json!({"encrypted_content": ["abc"]}),
            serde_json::json!({"encrypted_content": {"inner": "abc"}}),
            serde_json::json!({"encrypted_content": true}),
        ] {
            let error = extract_encrypted_content(&value).expect_err("non-string must reject");
            assert!(error.to_string().contains("string"));
        }
    }

    #[test]
    fn encrypted_content_debug_does_not_leak_plaintext() {
        let item = serde_json::json!({"encrypted_content": "super-secret-bytes"});
        let extracted = extract_encrypted_content(&item).expect("extract").unwrap();
        let debug = format!("{extracted:?}");
        assert!(!debug.contains("super-secret-bytes"));
        assert!(debug.contains("EncryptedContent"));
        assert!(debug.contains("byte_len"));
    }

    // ---------------- responses_reasoning_to_canonical ----------------

    #[test]
    fn canonical_item_maps_id_summary_and_blob_ref() {
        let item = responses_reasoning_fixture();
        let blob_ref = ProtectedBlobRef::from("pblob-1");
        let canonical =
            responses_reasoning_to_canonical(&item, blob_ref.clone()).expect("map to canonical");

        assert_eq!(canonical.id.as_str(), "rs_abc123");
        assert_eq!(canonical.protected_blob_ref.as_str(), "pblob-1");
        assert_eq!(
            canonical.summary.as_deref(),
            Some("考虑了天气约束\n确认了工具可用")
        );

        let mirrored = canonical
            .opaque_metadata
            .get(SUMMARY_ENTRIES_KEY)
            .and_then(Value::as_array)
            .expect("summary entries mirrored");
        assert_eq!(mirrored.len(), 2);
        assert_eq!(mirrored[0]["type"], "summary_text");
        assert_eq!(mirrored[0]["text"], "考虑了天气约束");
        assert_eq!(mirrored[1]["text"], "确认了工具可用");
        assert!(canonical.continuation_metadata.is_empty());
    }

    #[test]
    fn canonical_summary_is_none_when_wire_omits_summary() {
        let item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "encrypted_content": "opaque"
        });
        let canonical = responses_reasoning_to_canonical(&item, ProtectedBlobRef::from("pblob-1"))
            .expect("map");
        assert!(canonical.summary.is_none());
        assert!(!canonical.opaque_metadata.contains_key(SUMMARY_ENTRIES_KEY));
    }

    #[test]
    fn canonical_summary_is_none_for_empty_summary_array() {
        let item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [],
            "encrypted_content": "opaque"
        });
        let canonical = responses_reasoning_to_canonical(&item, ProtectedBlobRef::from("pblob-1"))
            .expect("map");
        assert!(canonical.summary.is_none());
    }

    #[test]
    fn canonical_mapping_rejects_summary_entry_without_type() {
        let item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{"text": "no type here"}]
        });
        let error = responses_reasoning_to_canonical(&item, ProtectedBlobRef::from("pblob-1"))
            .expect_err("missing summary type must reject");
        assert!(error.to_string().contains("type"));
    }

    #[test]
    fn canonical_mapping_rejects_missing_encrypted_continuation() {
        let item = serde_json::json!({"type": "reasoning", "id": "rs_1", "summary": []});
        let error = responses_reasoning_to_canonical(&item, ProtectedBlobRef::from("pblob-1"))
            .expect_err("missing continuation must reject");
        assert!(error.to_string().contains("continuation"));
    }

    #[test]
    fn canonical_mapping_rejects_missing_id() {
        let item = serde_json::json!({"type": "reasoning"});
        let error = responses_reasoning_to_canonical(&item, ProtectedBlobRef::from("pblob-1"))
            .expect_err("missing id must reject");
        assert!(error.to_string().contains("id"));
    }

    #[test]
    fn canonical_mapping_rejects_missing_and_wrong_type() {
        let missing_type = serde_json::json!({"id": "rs_1"});
        let error =
            responses_reasoning_to_canonical(&missing_type, ProtectedBlobRef::from("pblob-1"))
                .expect_err("missing type must reject");
        assert!(error.to_string().contains("type"));

        let wrong_type = serde_json::json!({"type": "message", "id": "rs_1"});
        let error =
            responses_reasoning_to_canonical(&wrong_type, ProtectedBlobRef::from("pblob-1"))
                .expect_err("wrong type must reject");
        assert!(error.to_string().contains("message"));
    }

    #[test]
    fn canonical_mapping_rejects_non_array_summary() {
        let item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": {"text": "wrong"}
        });
        let error = responses_reasoning_to_canonical(&item, ProtectedBlobRef::from("pblob-1"))
            .expect_err("non-array summary must reject");
        assert!(error.to_string().contains("array"));
    }

    #[test]
    fn canonical_mapping_rejects_summary_entry_missing_text() {
        let item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{"type": "summary_text"}]
        });
        let error = responses_reasoning_to_canonical(&item, ProtectedBlobRef::from("pblob-1"))
            .expect_err("entry without text must reject");
        assert!(error.to_string().contains("text"));
    }

    // ---------------- secret absence ----------------

    #[test]
    fn canonical_item_does_not_carry_encrypted_content_in_debug_or_serialize() {
        let item = responses_reasoning_fixture();
        let canonical = responses_reasoning_to_canonical(&item, ProtectedBlobRef::from("pblob-1"))
            .expect("map");

        let debug = format!("{canonical:?}");
        assert!(!debug.contains("opaque-continuation-bytes"));

        let serialized = serde_json::to_string(&canonical).expect("serialize");
        for forbidden in ["encrypted_content", "opaque-continuation-bytes", "status"] {
            assert!(
                !serialized.contains(forbidden),
                "canonical payload must not carry `{forbidden}`: {serialized}"
            );
        }
        assert!(serialized.contains("pblob-1"));
        assert!(serialized.contains("rs_abc123"));
    }

    #[test]
    fn canonical_mapping_rejects_unknown_summary_fields_so_secrets_cannot_hide() {
        let item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{
                "type": "summary_text",
                "text": "safe",
                "encrypted_content": "must-not-leak",
                "extra": "dropped"
            }]
        });
        let error = responses_reasoning_to_canonical(&item, ProtectedBlobRef::from("pblob-1"))
            .expect_err("unknown summary fields must reject");
        let rendered = format!("{error:?}");
        assert!(!rendered.contains("must-not-leak"));
        assert!(!rendered.contains("dropped"));
    }

    // ---------------- canonical_reasoning_to_responses_input ----------------

    #[test]
    fn exact_rehydrate_round_trips_safe_fields_and_decrypted_content() {
        let wire = responses_reasoning_fixture();
        let blob_ref = ProtectedBlobRef::from("pblob-1");
        let canonical = responses_reasoning_to_canonical(&wire, blob_ref).expect("map");

        let rehydrated = canonical_reasoning_to_responses_input(&canonical, "decrypted-bytes")
            .expect("rehydrate");

        assert_eq!(rehydrated["type"], "reasoning");
        assert_eq!(rehydrated["id"], "rs_abc123");
        assert_eq!(rehydrated["encrypted_content"], "decrypted-bytes");
        let summary = rehydrated["summary"].as_array().expect("summary array");
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0]["type"], "summary_text");
        assert_eq!(summary[0]["text"], "考虑了天气约束");
        assert_eq!(summary[1]["text"], "确认了工具可用");
    }

    #[test]
    fn rehydrate_rejects_empty_decrypted_content() {
        let item = ReasoningItem {
            id: ReasoningItemId::from("rs_1"),
            summary: None,
            protected_blob_ref: ProtectedBlobRef::from("pblob-1"),
            opaque_metadata: BTreeMap::new(),
            continuation_metadata: BTreeMap::new(),
        };
        let error = canonical_reasoning_to_responses_input(&item, "")
            .expect_err("empty content must reject");
        assert!(error.to_string().contains("decrypted"));
    }

    #[test]
    fn rehydrate_without_mirrored_summary_emits_empty_array() {
        let item = ReasoningItem {
            id: ReasoningItemId::from("rs_1"),
            summary: Some("display-only".into()),
            protected_blob_ref: ProtectedBlobRef::from("pblob-1"),
            opaque_metadata: BTreeMap::new(),
            continuation_metadata: BTreeMap::new(),
        };
        let rehydrated =
            canonical_reasoning_to_responses_input(&item, "decrypted").expect("rehydrate");
        let summary = rehydrated["summary"].as_array().expect("summary array");
        assert!(summary.is_empty());
        // canonical `summary` 显示字段不参与重建，避免把单字符串猜成数组条目。
        assert!(!serde_json::to_string(&rehydrated["summary"])
            .expect("serialize")
            .contains("display-only"));
    }

    #[test]
    fn rehydrate_rejects_malformed_summary_entries_metadata() {
        let mut opaque = BTreeMap::new();
        opaque.insert(
            SUMMARY_ENTRIES_KEY.into(),
            Value::String("not-an-array".into()),
        );
        let item = ReasoningItem {
            id: ReasoningItemId::from("rs_1"),
            summary: None,
            protected_blob_ref: ProtectedBlobRef::from("pblob-1"),
            opaque_metadata: opaque,
            continuation_metadata: BTreeMap::new(),
        };
        let error = canonical_reasoning_to_responses_input(&item, "decrypted")
            .expect_err("malformed metadata must reject");
        assert!(error.to_string().contains("malformed"));
    }

    #[test]
    fn rehydrate_rejects_unknown_fields_inside_summary_metadata() {
        let item = ReasoningItem {
            id: ReasoningItemId::from("rs_1"),
            summary: Some("safe".into()),
            protected_blob_ref: ProtectedBlobRef::from("pblob-1"),
            opaque_metadata: BTreeMap::from([(
                SUMMARY_ENTRIES_KEY.into(),
                serde_json::json!([{
                    "type": "summary_text",
                    "text": "safe",
                    "encrypted_content": "must-not-leak"
                }]),
            )]),
            continuation_metadata: BTreeMap::new(),
        };
        let error = canonical_reasoning_to_responses_input(&item, "decrypted")
            .expect_err("unknown summary metadata must reject");
        assert!(!format!("{error:?}").contains("must-not-leak"));
    }

    // ---------------- end-to-end + 三家可横比 fixture 形状 ----------------

    #[test]
    fn end_to_end_secret_only_reaches_protected_store_and_request_input() {
        let wire = responses_reasoning_fixture();
        let secret = extract_encrypted_content(&wire).expect("extract").unwrap();

        // 模拟 Protected Blob Store 写入：拿到一个不透明 ref，明文从此刻起只在
        // `secret` 句柄中；事件 / 日志 / GUI 都不应再见明文。
        let blob_ref = ProtectedBlobRef::from("pblob-stored");
        let decrypted = secret.into_inner();

        let canonical = responses_reasoning_to_canonical(&wire, blob_ref).expect("map");
        let rehydrated =
            canonical_reasoning_to_responses_input(&canonical, &decrypted).expect("rehydrate");

        // canonical 路径全程不携带明文。
        let canonical_debug = format!("{canonical:?}");
        let canonical_json = serde_json::to_string(&canonical).expect("serialize");
        for needle in ["opaque-continuation-bytes", "decrypted"] {
            assert!(!canonical_debug.contains(needle));
            assert!(!canonical_json.contains(needle));
        }

        // 明文只在回灌给 OpenAI 的 input item 中出现。
        let rehydrated_json = serde_json::to_string(&rehydrated).expect("serialize");
        assert!(rehydrated_json.contains("opaque-continuation-bytes"));
        assert_eq!(rehydrated["id"], "rs_abc123");
    }

    #[test]
    fn canonical_fixture_shape_is_stable_for_cross_provider_comparison() {
        // 锁定 canonical 形状：三家补齐 reasoning 映射后，等价输入应产出同样
        // 结构（id / summary / protected_blob_ref + 安全 metadata）。
        let wire = responses_reasoning_fixture();
        let canonical = responses_reasoning_to_canonical(&wire, ProtectedBlobRef::from("pblob-1"))
            .expect("map");

        assert_eq!(canonical.id.as_str(), "rs_abc123");
        assert_eq!(canonical.protected_blob_ref.as_str(), "pblob-1");
        assert!(canonical.summary.is_some());
        assert!(canonical.opaque_metadata.contains_key(SUMMARY_ENTRIES_KEY));
        assert!(canonical.continuation_metadata.is_empty());

        // 形状锁定：canonical 序列化后不含任何 Provider 凭证字段。
        let serialized = serde_json::to_string(&canonical).expect("serialize");
        for forbidden in [
            "encrypted_content",
            "signature",
            "reasoning_content",
            "opaque-continuation-bytes",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "canonical fixture leaked `{forbidden}`"
            );
        }
    }
}
