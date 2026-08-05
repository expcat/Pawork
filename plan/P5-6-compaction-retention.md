# P5-6：压缩保留策略

> Phase 5 · Session、Branch 与 Compaction · 状态：🟡未开始 · 依赖：P5-5

**最终目的**：实现压缩保留策略（最近 N 轮、未解决任务、用户约束、修改文件、失败/待处理 tool call），避免压缩后遗忘关键约束导致 Agent 跑偏。

**涉及范围**：`compaction-engine`

## 细分步骤

1. **保留最近 N 轮** —— 目的：近期上下文。
2. **保留未解决任务与用户约束** —— 目的：不遗忘目标。
3. **保留修改文件与待处理 tool call** —— 目的：不丢执行态。
4. **Golden Session 回归** —— 目的：压缩品质可验证。

## 主要产出物

- 压缩保留策略

## 验收标准

- [ ] 压缩后保留任务与关键约束

**相关文档**：[context](../docs/features/context.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)
