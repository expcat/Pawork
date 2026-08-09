# P3-9：事件流式分发

> Phase 3 · Agent Loop · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P3-1

**最终目的**：实现事件广播（bounded channel + backpressure），让 GUI/CLI 能低延迟订阅 Run 事件而不拖累核心。

**涉及范围**：`agent-engine`、`agent-events`

## 细分步骤

1. **广播订阅模型** —— 目的：多订阅者。
2. **bounded channel + backpressure** —— 目的：慢消费者不拖垮核心。
3. **分发延迟基准** —— 目的：低延迟。
4. **背压策略测试** —— 目的：可控丢弃/阻塞。

## 主要产出物

- 事件广播

## 验收标准

- [x] 事件分发延迟 < 2ms

**相关文档**：[agent-engine](../docs/features/agent-engine.md) · [observability](../docs/features/observability.md) · [ROADMAP](../ROADMAP.md)
