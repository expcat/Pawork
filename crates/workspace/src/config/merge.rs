//! 通用配置值合并：递归 object 合并，标量/数组整体替换。
//!
//! 合并语义与 `config-rs` 的设计相参照，但优先级语义自实现：
//! - object（map）：按键递归合并，子层值覆盖父层同键。
//! - 标量（bool / 数字 / 字符串）与数组：整体替换，不逐元素拼接。

use serde_json::Value;

/// 可参与合并的配置值。
///
/// 这是一个类型擦除的 JSON 值包装，便于实现统一的合并算法与来源追溯，
/// 最终再投影到强类型 [`super::PaworkConfig`]。
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

    /// 可变访问内部 JSON 值（供 loader 在合并前就地剥离受限键）。
    pub(crate) fn as_value_mut(&mut self) -> &mut Value {
        &mut self.value
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
}
