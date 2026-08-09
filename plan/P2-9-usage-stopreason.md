# P2-9：Usage 与 stop reason

> Phase 2 · 首个真实 Provider · 状态：🟢已完成 · 依赖：P2-5

**最终目的**：规范化 token / 费用信息与完成原因（stop reason），为预算控制与计费提供结构化数据。

**涉及范围**：`provider-runtime`

## 细分步骤

1. **token usage 归一** —— prompt/completion/cache。目的：统一计费口径。
2. **费用计算** —— 目的：可预估。
3. **stop reason 规范化** —— stop/tool/max_tokens/... 目的：上层可据此决策。
4. **测试** —— 目的：各 provider 行为一致。

## 主要产出物

- Usage / 费用 / stop reason 归一

## 验收标准

- [x] usage 与 stop reason 结构化

**相关文档**：[providers](../docs/features/providers.md) · [ROADMAP](../ROADMAP.md)
