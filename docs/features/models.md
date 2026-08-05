# Model Registry

## 职责

维护模型目录、能力、别名与定价，支持运行时按能力过滤与费用估算。

## 数据模型

```rust
pub struct ModelDefinition {
    pub id: String,
    pub provider: ProviderId,
    pub display_name: String,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub supports_tools: bool,
    pub supports_images: bool,
    pub supports_thinking: bool,
    pub supports_structured_output: bool,
    pub supports_prompt_cache: bool,
    pub thinking_levels: Vec<ThinkingLevel>,
    pub pricing: Option<ModelPricing>,
    pub aliases: Vec<String>,
}
```

## 功能要求

内置模型目录；Provider 动态发现；用户自定义模型；模型别名；默认模型；工作区模型覆盖；Session 模型覆盖；Fallback 模型；能力过滤；模型废弃提示；模型目录缓存；上下文窗口校验；Thinking Level 校验；费用估算。

## 验收标准

- 模型能力与实际 Provider 行为一致（由 Contract Tests 验证）
- 上下文窗口与 Thinking Level 校验有效
- 费用估算可追溯

## 相关文档

- [providers](providers.md) · [context（token 预算）](context.md)
- [ROADMAP P2-7](../../ROADMAP.md)
