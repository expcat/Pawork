# P6-8：结构化输出

> Phase 6 · 主要 Provider · 状态：🟡未开始 · 依赖：P6-1、P6-2、P6-3

**最终目的**：实现 JSON / 结构化输出，让 Agent 可要求模型返回符合 schema 的结构化结果。

**涉及范围**：`provider-*`

## 细分步骤

1. **JSON schema 约束** —— 目的：要求结构化输出。
2. **provider 差异处理** —— 目的：各 provider 对齐。
3. **校验与失败回退** —— 目的：不合规可处理。
4. **测试** —— 目的：输出符合 schema。

## 主要产出物

- 结构化输出能力

## 验收标准

- [ ] 可要求并校验 JSON 结构化输出

**相关文档**：[providers](../docs/features/providers.md) · [ROADMAP](../ROADMAP.md)
