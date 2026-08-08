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

生成模型与 embedding 模型使用独立定义，避免把维度等向量能力挂到不支持 embedding 的生成模型上：

```rust
pub struct EmbeddingModelDefinition {
    pub id: String,
    pub provider: ProviderId,
    pub display_name: String,
    pub capabilities: EmbeddingCapabilities,
    pub aliases: Vec<String>,
}

pub struct EmbeddingCapabilities {
    pub dimensions: Vec<u32>,
    pub max_input_tokens: u32,
    pub max_batch_size: u32,
    pub supports_dimension_override: bool,
}
```

`model-registry` 分别维护生成模型与 embedding 模型目录；二者可共享 Provider ID、别名和发现基础设施，但不得假定同一模型 ID 或同一能力集合。向量只经 [P16-7](../../plan/P16-7-long-term-memory.md) 的 canonical `EmbeddingProvider`（`provider-api`）获取，memory-service 不按 Provider 名分支。

## 功能要求

内置模型目录；Provider 动态发现；用户自定义模型；模型别名；默认模型；工作区模型覆盖；Session 模型覆盖；Fallback 模型；能力过滤；模型废弃提示；模型目录缓存；上下文窗口校验；Thinking Level 校验；费用估算。

能力过滤扩展覆盖独立 embedding 目录（按维度、输入上限、batch 上限筛选），供 memory-service 等子系统选择 embedding 模型。

## 验收标准

- 模型能力与实际 Provider 行为一致（由 Contract Tests 验证）
- 上下文窗口与 Thinking Level 校验有效
- 费用估算可追溯
- `EmbeddingModelDefinition` 目录与 Provider 逐模型能力一致，不污染生成模型字段

## 相关文档

- [providers](providers.md) · [context（token 预算）](context.md)
- [ROADMAP P2-7 / Phase 15–16（embedding canonical）](../../ROADMAP.md)
