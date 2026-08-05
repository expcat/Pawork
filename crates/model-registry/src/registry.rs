//! 模型目录、别名解析、能力过滤、上下文校验与费用估算。

use std::collections::BTreeMap;

use agent_domain::{Cost, ModelId, ProviderId, TokenUsage};
use provider_api::ModelCapabilities;
use serde::{Deserialize, Serialize};

use crate::error::RegistryError;
use crate::pricing::{estimate_cost, ModelPricing};

/// 目录中的单个模型条目。比 [`provider_api::ModelDefinition`] 多了 provider、定价与别名。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: ModelId,
    pub provider: ProviderId,
    pub display_name: String,
    pub context_window_tokens: u64,
    pub max_output_tokens: u64,
    pub capabilities: ModelCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<ModelPricing>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

impl CatalogEntry {
    /// 转换为 Provider 协议的 [`provider_api::ModelDefinition`]（丢弃 provider/定价/别名）。
    pub fn to_definition(&self) -> provider_api::ModelDefinition {
        provider_api::ModelDefinition {
            id: self.id.clone(),
            display_name: self.display_name.clone(),
            context_window_tokens: self.context_window_tokens,
            max_output_tokens: self.max_output_tokens,
            capabilities: self.capabilities.clone(),
        }
    }
}

/// 模型目录。内置模型 + provider 动态发现 + 用户自定义覆盖，统一按 id 索引。
#[derive(Clone, Debug, Default)]
pub struct ModelRegistry {
    entries: BTreeMap<ModelId, CatalogEntry>,
    alias_to_id: BTreeMap<String, ModelId>,
}

impl ModelRegistry {
    /// 创建空目录。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 创建带内置常用模型目录的注册表。
    pub fn builtin() -> Self {
        let mut registry = Self::empty();
        for entry in builtin_entries() {
            // 内置目录自身假定无冲突，直接写入（覆盖语义）。
            for alias in &entry.aliases {
                registry.alias_to_id.insert(alias.clone(), entry.id.clone());
            }
            registry.entries.insert(entry.id.clone(), entry);
        }
        registry
    }

    /// 注册单个条目（含别名）。注册时若别名已被其它模型占用则返回错误。
    pub fn register(&mut self, entry: CatalogEntry) -> Result<(), RegistryError> {
        self.try_register(entry)
    }

    /// 注册并在别名冲突时返回错误（别名预检后写入）。
    pub fn try_register(&mut self, entry: CatalogEntry) -> Result<(), RegistryError> {
        for alias in &entry.aliases {
            if let Some(existing) = self.alias_to_id.get(alias) {
                if *existing != entry.id {
                    return Err(RegistryError::DuplicateAlias {
                        alias: alias.clone(),
                        existing: existing.to_string(),
                    });
                }
            }
        }
        for alias in &entry.aliases {
            self.alias_to_id.insert(alias.clone(), entry.id.clone());
        }
        self.entries.insert(entry.id.clone(), entry);
        Ok(())
    }

    /// 合并 provider 动态发现或用户自定义的模型；同 id 覆盖、别名冲突时跳过旧映射。
    pub fn extend_with(&mut self, entries: Vec<CatalogEntry>) {
        for entry in entries {
            // 覆盖语义：同 id 直接替换；别名以新条目为准（覆盖旧映射）。
            for alias in &entry.aliases {
                self.alias_to_id.insert(alias.clone(), entry.id.clone());
            }
            self.entries.insert(entry.id.clone(), entry);
        }
    }

    /// 按 id 或别名解析条目。
    pub fn resolve(&self, id_or_alias: &str) -> Option<&CatalogEntry> {
        let id = self.alias_to_id.get(id_or_alias).cloned();
        if let Some(id) = id {
            return self.entries.get(&id);
        }
        // 也允许直接用真实 model id（不区分大小写匹配别名表后回退精确 id）
        self.entries.get(&ModelId::new(id_or_alias))
    }

    /// 列出全部条目（按 id 排序）。
    pub fn list(&self) -> Vec<&CatalogEntry> {
        self.entries.values().collect()
    }

    /// 按能力过滤：`required` 中为 `true` 的能力，候选条目必须同时满足。
    pub fn filter(&self, required: &ModelCapabilities) -> Vec<&CatalogEntry> {
        self.entries
            .values()
            .filter(|entry| caps_satisfied(&entry.capabilities, required))
            .collect()
    }

    /// 校验输入 token 数是否在模型的上下文窗口内。
    pub fn validate_context(&self, id_or_alias: &str, input_tokens: u64) -> bool {
        match self.resolve(id_or_alias) {
            Some(entry) => input_tokens <= entry.context_window_tokens,
            None => false,
        }
    }

    /// 按定价估算费用；模型未注册或无定价时返回 `None`。
    pub fn estimate_cost(&self, id_or_alias: &str, usage: &TokenUsage) -> Option<Cost> {
        let entry = self.resolve(id_or_alias)?;
        let pricing = entry.pricing.as_ref()?;
        Some(estimate_cost(usage, pricing))
    }
}

fn caps_satisfied(have: &ModelCapabilities, required: &ModelCapabilities) -> bool {
    (!required.text || have.text)
        && (!required.image_input || have.image_input)
        && (!required.tool_calls || have.tool_calls)
        && (!required.parallel_tool_calls || have.parallel_tool_calls)
        && (!required.thinking || have.thinking)
        && (!required.structured_output || have.structured_output)
        && (!required.prompt_cache || have.prompt_cache)
}

/// 构造能力集合的便捷函数。
#[allow(clippy::too_many_arguments)]
pub fn caps(
    text: bool,
    image_input: bool,
    tool_calls: bool,
    parallel_tool_calls: bool,
    thinking: bool,
    structured_output: bool,
    prompt_cache: bool,
) -> ModelCapabilities {
    ModelCapabilities {
        text,
        image_input,
        tool_calls,
        parallel_tool_calls,
        thinking,
        structured_output,
        prompt_cache,
    }
}

/// 内置常用模型目录。定价为近似基线（仅用于估算演示，真实定价以 provider 为准），
/// 上下文窗口取公开文档的保守值。本地兼容服务（Ollama/vLLM/LM Studio）的模型
/// 在连接后通过 `extend_with` 动态补充。
fn builtin_entries() -> Vec<CatalogEntry> {
    let text_vision_tools = caps(true, true, true, true, false, true, true);
    let tools_only = caps(true, false, true, true, false, true, true);
    let text_only = caps(true, false, false, false, false, false, false);

    vec![
        // OpenAI 系
        CatalogEntry {
            id: ModelId::new("gpt-4o"),
            provider: ProviderId::new("openai"),
            display_name: "GPT-4o".into(),
            context_window_tokens: 128_000,
            max_output_tokens: 16_384,
            capabilities: text_vision_tools.clone(),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 2_500_000,
                output_per_mtoken_micros: 10_000_000,
                cache_read_per_mtoken_micros: 1_250_000,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["gpt4o".into(), "4o".into()],
        },
        CatalogEntry {
            id: ModelId::new("gpt-4o-mini"),
            provider: ProviderId::new("openai"),
            display_name: "GPT-4o mini".into(),
            context_window_tokens: 128_000,
            max_output_tokens: 16_384,
            capabilities: text_vision_tools.clone(),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 150_000,
                output_per_mtoken_micros: 600_000,
                cache_read_per_mtoken_micros: 75_000,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["mini".into()],
        },
        // Anthropic 系
        CatalogEntry {
            id: ModelId::new("claude-3-5-sonnet"),
            provider: ProviderId::new("anthropic"),
            display_name: "Claude 3.5 Sonnet".into(),
            context_window_tokens: 200_000,
            max_output_tokens: 8_192,
            capabilities: caps(true, true, true, true, true, true, true),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 3_000_000,
                output_per_mtoken_micros: 15_000_000,
                cache_read_per_mtoken_micros: 300_000,
                cache_write_per_mtoken_micros: 3_750_000,
                currency: "USD".into(),
            }),
            aliases: vec!["sonnet".into(), "claude".into()],
        },
        // Google 系
        CatalogEntry {
            id: ModelId::new("gemini-1.5-pro"),
            provider: ProviderId::new("google"),
            display_name: "Gemini 1.5 Pro".into(),
            context_window_tokens: 1_000_000,
            max_output_tokens: 8_192,
            capabilities: caps(true, true, true, true, false, true, false),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 1_250_000,
                output_per_mtoken_micros: 5_000_000,
                cache_read_per_mtoken_micros: 0,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["gemini".into(), "gemini-pro".into()],
        },
        // 本地兼容服务占位（实际由 provider 动态发现覆盖）
        CatalogEntry {
            id: ModelId::new("llama-3.1-8b"),
            provider: ProviderId::new("openai-compatible"),
            display_name: "Llama 3.1 8B (local)".into(),
            context_window_tokens: 128_000,
            max_output_tokens: 4_096,
            capabilities: tools_only,
            pricing: None,
            aliases: vec!["llama".into()],
        },
        // 纯文本小模型示例（无工具/无视觉）
        CatalogEntry {
            id: ModelId::new("gpt-3.5-turbo"),
            provider: ProviderId::new("openai"),
            display_name: "GPT-3.5 Turbo".into(),
            context_window_tokens: 16_385,
            max_output_tokens: 4_096,
            capabilities: text_only,
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 500_000,
                output_per_mtoken_micros: 1_500_000,
                cache_read_per_mtoken_micros: 0,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["3.5".into()],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::ModelPricing;

    #[test]
    fn builtin_catalog_resolves_real_ids_and_aliases() {
        let registry = ModelRegistry::builtin();
        assert!(registry.resolve("gpt-4o").is_some());
        assert!(registry.resolve("gpt4o").is_some(), "别名须可解析");
        assert!(registry.resolve("sonnet").is_some());
        assert!(registry.resolve("gemini-pro").is_some());
        assert!(registry.resolve("nonexistent").is_none());
        assert!(!registry.list().is_empty());
    }

    #[test]
    fn alias_conflict_is_reported() {
        let mut registry = ModelRegistry::empty();
        let first = CatalogEntry {
            id: ModelId::new("a"),
            provider: ProviderId::new("p"),
            display_name: "A".into(),
            context_window_tokens: 1000,
            max_output_tokens: 100,
            capabilities: caps(true, false, false, false, false, false, false),
            pricing: None,
            aliases: vec!["shared".into()],
        };
        registry.try_register(first).expect("首次注册成功");

        let conflicting = CatalogEntry {
            id: ModelId::new("b"),
            provider: ProviderId::new("p"),
            display_name: "B".into(),
            context_window_tokens: 1000,
            max_output_tokens: 100,
            capabilities: caps(true, false, false, false, false, false, false),
            pricing: None,
            aliases: vec!["shared".into()],
        };
        let err = registry
            .try_register(conflicting)
            .expect_err("重复别名必须报错");
        assert!(matches!(err, RegistryError::DuplicateAlias { .. }));
    }

    #[test]
    fn capability_filter_selects_only_matching_models() {
        let registry = ModelRegistry::builtin();
        let with_tools = caps(true, false, true, false, false, false, false);
        let filtered: Vec<ModelId> = registry
            .filter(&with_tools)
            .into_iter()
            .map(|entry| entry.id.clone())
            .collect();
        assert!(filtered.contains(&ModelId::new("gpt-4o")));
        assert!(filtered.contains(&ModelId::new("claude-3-5-sonnet")));
        // gpt-3.5-turbo 无工具能力，应被排除
        assert!(!filtered.contains(&ModelId::new("gpt-3.5-turbo")));
    }

    #[test]
    fn context_validation_respects_window() {
        let registry = ModelRegistry::builtin();
        assert!(registry.validate_context("gpt-3.5-turbo", 16_000));
        assert!(!registry.validate_context("gpt-3.5-turbo", 100_000));
        assert!(!registry.validate_context("unknown-model", 10));
    }

    #[test]
    fn cost_estimate_matches_manual_integer_math() {
        let registry = ModelRegistry::builtin();
        let usage = TokenUsage {
            input_tokens: 2_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let cost = registry
            .estimate_cost("gpt-4o", &usage)
            .expect("有定价的模型可估算");
        assert_eq!(cost.currency, "USD");
        // 2M input * $2.5/M + 1M output * $10/M = 5M + 10M = 15_000_000 micros = $15.00
        assert_eq!(cost.amount_micros, 15_000_000);
    }

    #[test]
    fn extend_with_overrides_same_id() {
        let mut registry = ModelRegistry::builtin();
        let discovered = vec![CatalogEntry {
            id: ModelId::new("llama-3.1-8b"),
            provider: ProviderId::new("ollama"),
            display_name: "Llama 3.1 8B (discovered)".into(),
            context_window_tokens: 32_000,
            max_output_tokens: 2_048,
            capabilities: caps(true, false, true, false, false, false, false),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 0,
                output_per_mtoken_micros: 0,
                cache_read_per_mtoken_micros: 0,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["llama".into()],
        }];
        registry.extend_with(discovered);

        let entry = registry.resolve("llama-3.1-8b").expect("覆盖后仍可解析");
        assert_eq!(entry.provider, ProviderId::new("ollama"));
        assert_eq!(entry.context_window_tokens, 32_000);
        assert!(entry.pricing.is_some(), "动态发现的定价覆盖了无定价占位");
        assert_eq!(
            registry.resolve("llama").map(|entry| entry.id.clone()),
            Some(ModelId::new("llama-3.1-8b"))
        );
    }

    #[test]
    fn models_without_pricing_cannot_estimate_cost() {
        let registry = ModelRegistry::builtin();
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 10,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        // llama 内置占位无定价
        assert!(registry.estimate_cost("llama", &usage).is_none());
    }
}
