use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ProtectedBlobRef, ReasoningItemId};

/// Canonical reasoning effort（P15-8，P17-5 起由 pawork-domain 定义）。
///
/// `AgentProfileV2.effort` 以本枚举为一等字段，经本 crate `provider_api`
/// 模块的 `ReasoningConfig` → `CapabilityNegotiator` → Provider Adapter 翻译；禁止
/// Profile 或 Agent Core 按 Provider 名分支。显式 `ReasoningConfig` 优先；
/// 旧 `ThinkingConfig.level` 仅在缺省时派生；`XHigh / Max` 进入旧 P6 adapter
/// 时显式 clamp 为 `High`，不形成双轨。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

impl ReasoningEffort {
    /// 是否要求模型声明 reasoning 能力（任何非 None effort）。
    pub fn requires_reasoning_support(self) -> bool {
        !matches!(self, Self::None)
    }
}

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

    #[test]
    fn reasoning_effort_is_canonical_and_serde_stable() {
        // Canonical 枚举序列化名稳定：Profile v2 / provider_api 契约面共用同一词汇。
        assert_eq!(
            serde_json::to_string(&ReasoningEffort::XHigh).expect("serialize"),
            r#""x_high""#
        );
        assert_eq!(ReasoningEffort::default(), ReasoningEffort::Medium);
        assert!(!ReasoningEffort::None.requires_reasoning_support());
        assert!(ReasoningEffort::Max.requires_reasoning_support());
    }
}
