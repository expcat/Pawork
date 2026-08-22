//! Responses reasoning item 与 canonical [`ReasoningItem`] 的安全映射。
//!
//! `encrypted_content` 只能短暂存在于本模块和 [`ReasoningProtector`] 调用边界，
//! 不进入 Debug、事件 payload 或日志。

use std::collections::BTreeMap;
use std::fmt;

use pawork_domain::{
    LEGACY_HINT_KEY_MAP, OPENAI_RESPONSES_SUMMARY_ENTRIES_HINT, ReasoningMappingError,
};
use pawork_domain::{ProtectedBlobRef, ReasoningItem, ReasoningItemId};
use serde_json::Value;

const SUMMARY_ENTRIES_KEY: &str = OPENAI_RESPONSES_SUMMARY_ENTRIES_HINT;
const REASONING_ITEM_TYPE: &str = "reasoning";

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct EncryptedContent(String);

impl EncryptedContent {
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.0.into_bytes()
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

pub(crate) fn extract_encrypted_content(
    item: &Value,
) -> Result<Option<EncryptedContent>, ReasoningMappingError> {
    match item.get("encrypted_content") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => {
            Ok(Some(EncryptedContent(value.clone())))
        }
        Some(Value::String(_)) => Err(ReasoningMappingError::unsupported(
            "reasoning encrypted continuation is empty",
        )),
        Some(_) => Err(ReasoningMappingError::unsupported(
            "reasoning encrypted continuation is not a string",
        )),
    }
}

pub(crate) fn to_canonical(
    item: &Value,
    blob_ref: ProtectedBlobRef,
) -> Result<ReasoningItem, ReasoningMappingError> {
    let item_type = required_str(item, "type", "reasoning item without type")?;
    if item_type != REASONING_ITEM_TYPE {
        return Err(ReasoningMappingError::unsupported(
            "Responses output item is not reasoning",
        ));
    }
    let id = required_str(item, "id", "reasoning item without id")?;
    if extract_encrypted_content(item)?.is_none() {
        return Err(ReasoningMappingError::unsupported(
            "reasoning item has no encrypted continuation",
        ));
    }

    let summary_entries = summary_entries(item)?;
    let summary = (!summary_entries.is_empty()).then(|| {
        summary_entries
            .iter()
            .filter_map(|entry| entry.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    });
    let opaque_metadata = if summary_entries.is_empty() {
        BTreeMap::new()
    } else {
        BTreeMap::from([(
            SUMMARY_ENTRIES_KEY.to_string(),
            Value::Array(summary_entries),
        )])
    };

    Ok(ReasoningItem {
        id: ReasoningItemId::from(id),
        summary,
        protected_blob_ref: blob_ref,
        opaque_metadata,
        continuation_metadata: BTreeMap::new(),
    })
}

pub(crate) fn to_input(
    item: &ReasoningItem,
    decrypted_content: &str,
) -> Result<Value, ReasoningMappingError> {
    if decrypted_content.is_empty() {
        return Err(ReasoningMappingError::unsupported(
            "cannot rehydrate empty reasoning continuation",
        ));
    }
    let summary = match summary_entries_metadata(item) {
        None => Vec::new(),
        Some(Value::Array(entries)) => validate_summary_entries(entries)?,
        Some(_) => {
            return Err(ReasoningMappingError::unsupported(
                "reasoning summary metadata is malformed",
            ))
        }
    };
    Ok(serde_json::json!({
        "type": REASONING_ITEM_TYPE,
        "id": item.id.as_str(),
        "summary": summary,
        "encrypted_content": decrypted_content,
    }))
}

/// 读规范命名空间键；兼容 R5 前落盘的旧拼写（从 domain 冻结映射表派生，
/// 生产者只写规范键）。
fn summary_entries_metadata(item: &ReasoningItem) -> Option<&Value> {
    item.opaque_metadata
        .get(SUMMARY_ENTRIES_KEY)
        .or_else(|| {
            LEGACY_HINT_KEY_MAP
                .iter()
                .filter(|(_, canonical)| *canonical == SUMMARY_ENTRIES_KEY)
                .find_map(|(legacy, _)| item.opaque_metadata.get(*legacy))
        })
}

fn summary_entries(item: &Value) -> Result<Vec<Value>, ReasoningMappingError> {
    match item.get("summary") {
        None => Ok(Vec::new()),
        Some(Value::Array(entries)) => validate_summary_entries(entries),
        Some(_) => Err(ReasoningMappingError::unsupported(
            "reasoning summary must be an array",
        )),
    }
}

fn validate_summary_entries(entries: &[Value]) -> Result<Vec<Value>, ReasoningMappingError> {
    let mut validated = Vec::with_capacity(entries.len());
    for entry in entries {
        let object = entry.as_object().ok_or_else(|| {
            ReasoningMappingError::unsupported("reasoning summary entry is not an object")
        })?;
        if object.keys().any(|key| key != "type" && key != "text") {
            return Err(ReasoningMappingError::unsupported(
                "reasoning summary entry has unmapped fields",
            ));
        }
        if required_str(entry, "type", "reasoning summary entry without type")?
            != "summary_text"
        {
            return Err(ReasoningMappingError::unsupported(
                "unsupported reasoning summary entry type",
            ));
        }
        let text = required_str(entry, "text", "reasoning summary entry without text")?;
        validated.push(serde_json::json!({"type": "summary_text", "text": text}));
    }
    Ok(validated)
}

fn required_str<'a>(
    value: &'a Value,
    key: &str,
    error: &str,
) -> Result<&'a str, ReasoningMappingError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ReasoningMappingError::unsupported(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire() -> Value {
        serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{"type": "summary_text", "text": "checked"}],
            "encrypted_content": "opaque-secret"
        })
    }

    #[test]
    fn encrypted_content_debug_is_redacted() {
        let encrypted = extract_encrypted_content(&wire()).unwrap().unwrap();
        let debug = format!("{encrypted:?}");
        assert!(!debug.contains("opaque-secret"));
        assert!(debug.contains("byte_len"));
    }

    #[test]
    fn canonical_round_trip_uses_only_protected_reference() {
        let item = to_canonical(&wire(), ProtectedBlobRef::new("blob-1")).unwrap();
        let encoded = serde_json::to_string(&item).unwrap();
        assert!(!encoded.contains("opaque-secret"));
        assert_eq!(to_input(&item, "opaque-secret").unwrap()["id"], "rs_1");
    }

    #[test]
    fn to_canonical_writes_namespaced_summary_hint_key() {
        let item = to_canonical(&wire(), ProtectedBlobRef::new("blob-2")).unwrap();
        assert_eq!(
            item.opaque_metadata
                .get(OPENAI_RESPONSES_SUMMARY_ENTRIES_HINT)
                .and_then(Value::as_array)
                .map(Vec::as_slice),
            Some(
                &[serde_json::json!({
                    "type": "summary_text",
                    "text": "checked",
                })][..]
            )
        );
    }

    #[test]
    fn to_input_reads_legacy_summary_hint_spellings() {
        for legacy in ["responses.summary_entries", "openai.responses.summary_entries"] {
            assert_eq!(
                pawork_domain::canonical_hint_key(legacy),
                Some(OPENAI_RESPONSES_SUMMARY_ENTRIES_HINT)
            );
            let item = ReasoningItem {
                id: ReasoningItemId::from("rs_legacy"),
                summary: None,
                protected_blob_ref: ProtectedBlobRef::new("blob-legacy"),
                opaque_metadata: BTreeMap::from([(
                    legacy.to_string(),
                    serde_json::json!([{"type": "summary_text", "text": "legacy"}]),
                )]),
                continuation_metadata: BTreeMap::new(),
            };
            let input = to_input(&item, "opaque-secret").unwrap();
            assert_eq!(input["summary"][0]["text"], "legacy");
        }
    }
}
