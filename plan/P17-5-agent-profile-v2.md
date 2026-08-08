# P17-5：Agent Profile v2（智能体配置档案 v2）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P8-5、P8-3、P9-6、P4-9、P17-1、P3-6、P15-7、P15-8、P16-4、P16-7、P11-1

**最终目的**：升级 Agent Profile 为 v2，统一描述一个可复用 Agent 的完整配置：prompt（system / instructions）、model、effort / reasoning 强度、tools（含显式 denied 清单）、skills、MCP、permissions、hooks、memory、max turns、background、isolation。让 Agent 可被一键实例化、复用、共享，且所有维度可被 policy 与运行时校验。v1 profile 可平滑迁移。

**涉及范围**：扩展 `agent-domain`（profile v2 类型）、`resource-loader`（加载 / 校验）；复用各被引用子系统

## 细分步骤

1. **Profile v2 schema** —— 目的：在 `agent-domain` 定义 v2 profile 类型，涵盖 prompt / model / effort / tools(denied) / skills / mcp / permissions / hooks / memory / max-turns / background / isolation；提供 v1→v2 迁移路径。
2. **tools 与 denied** —— 目的：声明允许工具清单与显式 denied 清单（deny 优先），与 tool-runtime + `policy-engine` 协作，确保 denied 不可被任何方式绕过。
3. **skills / MCP / permissions / hooks 引用** —— 目的：profile 通过引用（id + version / pin）挂载 skills（[P8-3](P8-3-skills.md)）、MCP（[P9-6](P9-6-mcp-config.md)）、permissions（[P4-9](P4-9-policy-engine.md)）、hooks（[P17-1](P17-1-user-hooks.md)），引用解析失败或越权时降级 / 报错。
4. **memory / max turns / canonical effort** —— 目的：memory（接入 [P16-7](P16-7-long-term-memory.md) 长期记忆）、max turns（接入 [P3-6](P3-6-budget-control.md) 预算控制）。effort / reasoning 改为**canonical 一等字段**：`AgentProfile.effort` 经 `ReasoningConfig` → [P15-8](P15-8-capability-discovery.md) `CapabilityNegotiator` → Provider Adapter 翻译，不再经 P6-9 `provider_options`；`ReasoningEffort { None, Low, Medium, High, XHigh, Max }` 由 P15-8 定义为 canonical 枚举。**Profile 不得包含 Provider-specific reasoning 字段**；Provider-specific 剩余特殊配置仍可经 extension/options 旁路，但 canonical effort 必须是一等字段，Agent Core 不按 Provider 名分支。
5. **background / isolation** —— 目的：background（接入 [P16-4](P16-4-background-task-manager.md) 后台任务）声明该 agent 可后台运行；isolation（接入 P11 sandbox）声明运行隔离等级（none / restricted / container）。
6. **校验与迁移** —— 目的：加载时做完整性 / 越权 / 冲突校验（与 `policy-engine` 协作），v1→v2 自动迁移；profile 本身不携带明文 secret。
7. **定向 / Mock 测试** —— 目的：v2 profile 加载与校验、denied 生效、引用解析失败降级、v1 迁移、isolation / background 正确传递。仅定向 + Mock。

## 主要产出物

- `agent-domain` profile v2 类型
- `resource-loader` 的 v2 加载 / 校验 / 迁移
- 定向测试

## 验收标准

- [ ] v2 profile 覆盖 prompt / model / effort / tools(denied) / skills / mcp / permissions / hooks / memory / max-turns / background / isolation 全部维度
- [ ] `effort` 为 canonical 一等字段（`ReasoningEffort`），经 P15-8 协商翻译，不经 `provider_options`；Profile 不含 Provider-specific reasoning 字段（`no_provider_branch` 断言）
- [ ] denied 工具不可被任何方式绕过
- [ ] 引用解析失败 / 越权时安全降级或报错
- [ ] v1 profile 可迁移到 v2；profile 不含明文 secret

**相关文档**：[P8-5 Profiles v1](P8-5-profiles.md) · [skills](../docs/features/skills.md) · [mcp](../docs/features/mcp.md) · [policy](../docs/features/policy.md) · [sandbox](../docs/features/sandbox.md) · [P16-7 Long-term Memory](P16-7-long-term-memory.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；扩展 `agent-domain` + `resource-loader`，依赖方向不变。profile v2 类型只引用各子系统 domain 类型，不依赖 infra，保持 `agent-domain` 纯净。
