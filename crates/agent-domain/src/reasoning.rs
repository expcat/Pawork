use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ProtectedBlobRef, ReasoningItemId};

/// Provider-neutral reasoning continuation state.
///
/// The provider credential itself is never stored here. It lives encrypted in
/// the Protected Blob Store and is represented by [`ProtectedBlobRef`]. Both
/// metadata maps are restricted to non-sensitive translation hints.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReasoningItem {
    pub id: ReasoningItemId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub protected_blob_ref: ProtectedBlobRef,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub opaque_metadata: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub continuation_metadata: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_item_round_trip_contains_only_safe_reference() {
        let item = ReasoningItem {
            id: ReasoningItemId::from("reasoning-1"),
            summary: Some("checked the constraints".into()),
            protected_blob_ref: ProtectedBlobRef::from("protected-1"),
            opaque_metadata: BTreeMap::from([(
                "item_type".into(),
                Value::String("reasoning".into()),
            )]),
            continuation_metadata: BTreeMap::new(),
        };

        let encoded = serde_json::to_string(&item).expect("serialize reasoning item");
        let decoded: ReasoningItem =
            serde_json::from_str(&encoded).expect("deserialize reasoning item");

        assert_eq!(decoded, item);
        assert!(encoded.contains("protected-1"));
        for forbidden in ["encrypted_content", "signature", "reasoning_content"] {
            assert!(!encoded.contains(forbidden));
        }
    }
}
