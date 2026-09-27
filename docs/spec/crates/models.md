# pawork-models

> 宿主与 Provider 共用的模型目录、能力证据、能力协商和计价。2026-09-27 从 providers 原位迁移；内部 API 路径直接改为 `pawork_models`，不保留旧路径代理。

## 1. 职责与边界

维护模型定义、别名、能力来源、探测缓存、逐型号默认能力与定点费率。生产内部依赖仅 [domain](domain.md)，不依赖具体 Provider、HTTP、凭证后端、存储或 GUI。

`probe_provider` 调用调用方注入的 `ModelProvider`，可能触发外部 IO；本包不实现网络请求，也不管理凭证生命周期。wire 字段归一、SSE、厂商端点及认证头属于 [providers](providers.md)。

## 2. 模块树

| 文件 | 职责 |
| --- | --- |
| `src/lib.rs` | 四个模块与公共类型导出 |
| `src/registry.rs` | `ModelRegistry`、`CatalogEntry`、三源能力证据、别名、共享探测槽、默认能力表 |
| `src/negotiate.rs` | `CapabilityNegotiator`、请求能力 gate、reasoning clamp、嵌套视频检测 |
| `src/pricing.rs` | `ModelPricing`、`estimate_cost`、内置费率卡与版本 |
| `src/error.rs` | `RegistryError` |

## 3. 对外 API 面

- `ModelRegistry::{empty,builtin,register,try_register,extend_with,resolve,list,filter,validate_context,estimate_cost}`：目录、别名、上下文预算与计价。`CatalogEntry::to_definition` 转为 domain 模型定义。
- `merge_provider_models` / `merge_provider_source`：合并提供方目录；`ProviderCapabilitySource` 是注入目录端口。
- `probe_provider` / `record_probe` / `clear_probe`、`ProviderProbe` / `ProbeError`：按 Provider 缓存发现结果及失败，并发共享探测槽，取消后的槽可重试。
- `set_override` / `remove_override`、`capability_evidence` / `capability_snapshot`：读写能力证据。`CapabilityEvidence::merged` / `merge_capabilities` 对已出现来源取保守交集。
- `registry::{default_supported_efforts,default_image_input,default_image_output,default_hosted_web_search}` 与 `apply_default_*`：逐型号默认能力，远端显式声明优先；未知保持未知。端点限制仍由 providers 执行。
- `CapabilityNegotiator::negotiate`、`negotiate::capability_gate`、`clamp_reasoning_to_thinking`、`negotiate::content_part_has_video`：模型证据与请求相交；图片和视频递归覆盖工具结果。
- `ModelPricing` / `estimate_cost` / `BUILTIN_RATE_CARD` / `BUILTIN_RATE_VERSION`：micro-unit 整数计价；不把未知价格编造成零费用。

## 4. 关键行为与语义

真实 model id 优先于 alias，替换条目清理旧别名；不同 Provider 同名目录不会自动跨通道借用证据。Host 从本包建目录，providers 将远端目录转换为 domain 定义后合并；Agent Engine 始终只看 domain 契约。

能力来源 `Static / Probe / Override` 用于溯源，合并是已出现来源逐字段交集，override 只能收窄。请求协商满足 `requested == supported ∪ unsupported`，不支持能力明确拒绝或记录降级。transport 由能力声明驱动；reasoning 显式配置优先于旧 thinking 配置，无法表达的强度按既有规则 clamp。

目录探测以 Provider 为键，同一轮并发共享结果，不持锁跨 await。静态默认表与费率卡随产品更新；厂商协议能力声明仍须有实际 wire 支持，不能仅靠目录标签开放功能。

## 5. 依赖与 feature

生产：`pawork-domain`、`serde`、`serde_json`、`thiserror`。开发：`async-trait`、`tokio`。无 feature。真实生产消费者为 `pawork-app` 与 `pawork-providers`。

## 6. 红线与不变量

不引入 Provider 实现依赖、HTTP 客户端或持久化后端；模型能力不得放大已有证据。不改变 domain 的 Provider 契约、JSON、事件或数据库格式。计价单一实现位于本包，wire usage 解析留在 providers。

## 7. 测试与 golden 资产

原 registry / negotiate / pricing 内联回归完整迁移，覆盖别名冲突、并发探测/取消、能力收窄、图片/视频与 hosted tools 拒绝、推理强度和整数计价。执行 `bash scripts/test.sh models`；跨适配器联动执行 `bash scripts/test.sh models providers`。本次不新增镜像测试。

## 8. 相关文档

[架构与本次决策](../../architecture.md#2-包布局与依赖方向29-包) · [providers](providers.md) · [app](app.md) · [跨包链路](../flows.md) · [验证](../verification.md)。
