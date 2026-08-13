//! Trigger Registry（P17-1 步骤 1、3）。
//!
//! 按 trigger point 确定性注册 user hook（按 hook id 升序派发，与注册顺序无关）。
//! 与 P10-3 `hook-runtime` 的插件 lifecycle 派发器**互不调用**：二者共享同一
//! 组 trigger point 词汇概念，但运行时与信任边界独立，dispatcher 也不同。

use crate::error::HookError;
use crate::handler::{HookHandler, HookId};
use crate::trigger::TriggerPoint;
use std::collections::BTreeMap;

/// 按 trigger 索引的 hook 注册表。同 trigger 内按 hook id 升序。
pub struct TriggerRegistry {
    by_trigger: BTreeMap<TriggerPoint, Vec<HookHandler>>,
    ids: BTreeMap<String, ()>,
}

impl Default for TriggerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TriggerRegistry {
    pub fn new() -> Self {
        Self {
            by_trigger: BTreeMap::new(),
            ids: BTreeMap::new(),
        }
    }

    /// 注册一个 handler；重复 id 拒绝。
    pub fn register(&mut self, handler: HookHandler) -> Result<(), HookError> {
        if self.ids.contains_key(&handler.id.0) {
            return Err(HookError::Conflict {
                hook_id: handler.id.0.clone(),
            });
        }
        self.ids.insert(handler.id.0.clone(), ());
        let list = self.by_trigger.entry(handler.trigger).or_default();
        list.push(handler);
        // 保持按 id 升序，确保确定性派发。
        list.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        Ok(())
    }

    /// 注销。
    pub fn unregister(&mut self, id: &HookId) -> Result<(), HookError> {
        if self.ids.remove(&id.0).is_none() {
            return Err(HookError::NotFound {
                hook_id: id.0.clone(),
            });
        }
        for list in self.by_trigger.values_mut() {
            list.retain(|h| &h.id != id);
        }
        Ok(())
    }

    /// 返回某 trigger 下所有匹配 handler（已按 id 升序）。
    pub fn handlers_for(&self, trigger: TriggerPoint) -> Vec<&HookHandler> {
        self.by_trigger
            .get(&trigger)
            .map(|list| list.iter().collect())
            .unwrap_or_default()
    }

    /// 已注册 handler 总数。
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HandlerConfig, HookConfig, HookScope, PromptEvalHandler};

    fn eval_config(id: &str, trigger: TriggerPoint) -> HookConfig {
        HookConfig {
            id: id.into(),
            trigger,
            scope: HookScope::Global,
            lifecycle: None,
            enabled: true,
            handler: HandlerConfig::PromptEval(PromptEvalHandler {
                prompt_template: "is this safe?".into(),
                response_schema: None,
                on_failure: Default::default(),
            }),
        }
    }

    #[test]
    fn register_and_lookup_is_deterministic_by_id() {
        let mut reg = TriggerRegistry::new();
        // 故意逆序注册，期望按 id 升序返回。
        let z = HookHandler::from_config(eval_config("z-hook", TriggerPoint::RunStarted)).unwrap();
        let a = HookHandler::from_config(eval_config("a-hook", TriggerPoint::RunStarted)).unwrap();
        let m = HookHandler::from_config(eval_config("m-hook", TriggerPoint::RunStarted)).unwrap();
        for h in [z, a, m] {
            reg.register(h).unwrap();
        }
        let order: Vec<&str> = reg
            .handlers_for(TriggerPoint::RunStarted)
            .into_iter()
            .map(|h| h.id.as_str())
            .collect();
        assert_eq!(order, vec!["a-hook", "m-hook", "z-hook"]);
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let mut reg = TriggerRegistry::new();
        let h = HookHandler::from_config(eval_config("dup", TriggerPoint::RunStarted)).unwrap();
        reg.register(h.clone()).unwrap();
        let err = reg.register(h).unwrap_err();
        assert!(matches!(err, HookError::Conflict { .. }));
    }

    #[test]
    fn unregister_removes_from_all_triggers() {
        let mut reg = TriggerRegistry::new();
        let h = HookHandler::from_config(eval_config("x", TriggerPoint::RunStarted)).unwrap();
        reg.register(h.clone()).unwrap();
        reg.unregister(&h.id).unwrap();
        assert!(reg.handlers_for(TriggerPoint::RunStarted).is_empty());
        assert!(matches!(
            reg.unregister(&HookId::new("x")),
            Err(HookError::NotFound { .. })
        ));
    }
}
