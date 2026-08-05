//! 通用配置值合并：递归 object 合并，标量/数组整体替换。
//!
//! 合并语义与 `config-rs` 的设计相参照，但优先级语义自实现：
//! - object（map）：按键递归合并，子层值覆盖父层同键。
//! - 标量（bool / 数字 / 字符串）与数组：整体替换，不逐元素拼接。

use std::collections::BTreeMap;

use serde_json::Value;

/// 可参与合并的配置值。
///
/// 这是一个类型擦除的 JSON 值包装，便于实现统一的合并算法与来源追溯，
/// 最终再投影到强类型 [`crate::PaworkConfig`]。
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigValue {
    value: Value,
}

impl ConfigValue {
    pub fn new(value: Value) -> Self {
        Self { value }
    }

    pub fn into_inner(self) -> Value {
        self.value
    }

    pub fn as_value(&self) -> &Value {
        &self.value
    }

    /// 是否为 object。只有 object 才会递归合并。
    pub fn is_object(&self) -> bool {
        self.value.is_object()
    }
}

impl From<Value> for ConfigValue {
    fn from(value: Value) -> Self {
        Self::new(value)
    }
}

/// 合并语义：把 `other` 合并进 `self`，`other` 的值优先（更高层级覆盖更低层级）。
pub trait Merge {
    /// 用 `other`（更高优先级）合并覆盖 `self`，原地更新。
    fn merge(&mut self, other: &Self);
}

impl Merge for ConfigValue {
    fn merge(&mut self, other: &Self) {
        merge_json(&mut self.value, &other.value);
    }
}

/// 递归合并两个 JSON 值：object 按键递归，其余整体替换。
///
/// `higher` 的优先级高于 `lower`。
pub fn merge_json(lower: &mut Value, higher: &Value) {
    match (lower, higher) {
        (Value::Object(lower_map), Value::Object(higher_map)) => {
            for (key, higher_value) in higher_map {
                match lower_map.get_mut(key) {
                    Some(lower_value) if lower_value.is_object() && higher_value.is_object() => {
                        merge_json(lower_value, higher_value);
                    }
                    _ => {
                        lower_map.insert(key.clone(), higher_value.clone());
                    }
                }
            }
        }
        // 任一非 object：整体替换。
        (slot, replacement) => {
            *slot = replacement.clone();
        }
    }
}

/// 把多个按优先级升序排列的值合并为单个值。
///
/// 数组顺序即优先级：靠后的元素覆盖靠前的。这是合并的基础原语，
/// 由上层按层级顺序调用。
pub fn merge_ordered(values: impl IntoIterator<Item = ConfigValue>) -> ConfigValue {
    let mut iter = values.into_iter();
    let Some(mut acc) = iter.next() else {
        return ConfigValue::new(Value::Object(BTreeMap::new().into_iter().collect()));
    };
    for next in iter {
        acc.merge(&next);
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn objects_merge_recursively() {
        let mut lower = ConfigValue::new(json!({
            "a": { "x": 1, "y": 2 },
            "b": 1
        }));
        let higher = ConfigValue::new(json!({
            "a": { "y": 20, "z": 30 },
            "c": 3
        }));
        lower.merge(&higher);
        assert_eq!(
            lower.into_inner(),
            json!({ "a": { "x": 1, "y": 20, "z": 30 }, "b": 1, "c": 3 })
        );
    }

    #[test]
    fn arrays_are_replaced_not_concatenated() {
        let mut lower = ConfigValue::new(json!({ "items": [1, 2, 3] }));
        let higher = ConfigValue::new(json!({ "items": [9] }));
        lower.merge(&higher);
        assert_eq!(lower.into_inner(), json!({ "items": [9] }));
    }

    #[test]
    fn scalars_are_replaced() {
        let mut lower = ConfigValue::new(json!({ "n": 1, "s": "a", "flag": true }));
        let higher = ConfigValue::new(json!({ "n": 2, "s": "b", "flag": false }));
        lower.merge(&higher);
        assert_eq!(
            lower.into_inner(),
            json!({ "n": 2, "s": "b", "flag": false })
        );
    }

    #[test]
    fn higher_object_replaces_lower_scalar() {
        let mut lower = ConfigValue::new(json!({ "k": 5 }));
        let higher = ConfigValue::new(json!({ "k": { "nested": true } }));
        lower.merge(&higher);
        assert_eq!(lower.into_inner(), json!({ "k": { "nested": true } }));
    }

    #[test]
    fn merge_ordered_respects_input_order() {
        let merged = merge_ordered([
            ConfigValue::new(json!({ "a": 1 })),
            ConfigValue::new(json!({ "a": 2 })),
            ConfigValue::new(json!({ "a": 3 })),
        ]);
        assert_eq!(merged.into_inner(), json!({ "a": 3 }));
    }
}
