# Model Registry

## 职责

维护模型目录、能力、别名与定价，支持运行时按能力过滤与费用估算。

## 数据模型

```rust
pub struct CatalogEntry {
    pub id: ModelId,
    pub provider: ProviderId,
    pub display_name: String,
    pub context_window_tokens: u64,
    pub max_output_tokens: u64,
    pub capabilities: ModelCapabilities,
    pub pricing: Option<ModelPricing>,
    pub aliases: Vec<String>,
}

pub struct CapabilityEvidence {
    pub model: ModelId,
    pub provider: Option<ProviderId>,
    pub static_declared: Option<ModelCapabilities>,
    pub probe_declared: Option<ModelCapabilities>,
    pub override_declared: Option<ModelCapabilities>,
}
```

`ModelCapabilities` 同时保留 Phase 6 的兼容布尔字段，并以 P15-8 的枚举集合表达 transport、Provider Hosted / Extension 工具、reasoning effort / continuation、citation/source、structured output 与 prompt cache 等现代能力。旧目录与旧序列化数据缺少 v2 字段时按空集合处理，不据此推断 Provider 支持。

能力证据有三种来源：内置或动态目录的静态声明（Static）、`ModelProvider::list_models` 的运行时探测（Probe）、用户或测试配置覆盖（Override）。有效能力是所有**已出现来源的逐项交集**；缺失来源不施加约束，出现的 override 只能继续收紧能力，不能凭配置创造 Provider 未声明的支持。选择 transport 或工具 fallback 前必须先得到该交集，禁止把三源做 union。

探测结果按 `provider_id` 缓存。同一 Provider 的并发发现共享一次 `list_models` 调用，成功与失败都完成同一槽位；等待期间不跨 `await` 持锁。需要重新探测时由宿主显式清除该 Provider 的缓存，不能在每个请求前重复访问远端。

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

内置模型目录；Provider 动态发现；用户自定义模型；模型别名；默认模型；工作区模型覆盖；Session 模型覆盖；Fallback 模型；三源能力证据；保守能力协商；Provider 级探测缓存；能力过滤；模型废弃提示；上下文窗口校验；Reasoning Effort 校验；费用估算。

能力过滤扩展覆盖独立 embedding 目录（按维度、输入上限、batch 上限筛选），供 memory-service 等子系统选择 embedding 模型。

## 验收标准

- 模型能力与实际 Provider 行为一致（由 Contract Tests 验证）
- Static / Probe / Override 只取交集，配置不能把不支持的能力改成支持
- 同一 Provider 的并发能力探测只调用一次，且锁不跨 `await`
- 上下文窗口与 Reasoning Effort 校验有效
- 费用估算可追溯
- `EmbeddingModelDefinition` 目录与 Provider 逐模型能力一致，不污染生成模型字段

## 相关文档

- [providers](providers.md) · [context（token 预算）](context.md)
- [ROADMAP P2-7 / Phase 15–16（embedding canonical）](../../ROADMAP.md)
