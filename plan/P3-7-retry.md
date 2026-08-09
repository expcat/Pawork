# P3-7：重试

> Phase 3 · Agent Loop · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P3-3

**最终目的**：实现 Agent 层重试（provider 断流重试、retry last call、retry run），让网络抖动与中断可恢复而不丢上下文。

**涉及范围**：`agent-engine`

## 细分步骤

1. **provider 断流重试** —— 目的：流中断可续。
2. **retry last call** —— 目的：重试上一次模型调用。
3. **retry run** —— 目的：从某点重跑。
4. **重试与事件一致性** —— 目的：可追溯。

## 主要产出物

- Agent 层重试

## 验收标准

- [x] 断流可重试、上下文不丢

**相关文档**：[agent-engine](../docs/features/agent-engine.md) · [ROADMAP](../ROADMAP.md)
