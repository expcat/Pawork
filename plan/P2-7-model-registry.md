# P2-7：Model Registry

> Phase 2 · 首个真实 Provider · 状态：🟡未开始 · 依赖：P0-4

**最终目的**：实现模型目录（内置 + 动态发现）、别名、能力过滤、上下文窗口校验与费用估算，让 Agent 与 UI 能正确选择模型。

**涉及范围**：`model-registry`

## 细分步骤

1. **内置模型目录** —— 目的：常用模型开箱即用。
2. **动态发现** —— 目的：从 provider 拉取可用模型。
3. **别名与能力过滤** —— 目的：按能力（vision/tool/thinking）筛选。
4. **上下文窗口校验 + 费用估算** —— 目的：防越界 + 可预估成本。

## 主要产出物

- `model-registry` crate

## 验收标准

- [ ] 别名解析正确
- [ ] 上下文窗口校验生效
- [ ] 费用估算可用

**相关文档**：[models](../docs/features/models.md) · [ROADMAP](../ROADMAP.md)
