# P3-8：取消

> Phase 3 · Agent Loop · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P3-3、P3-4

**最终目的**：实现取消传播（取消 provider、取消 tool），并保证不遗留进程，让用户能可靠地中断 Agent。

**涉及范围**：`agent-engine`、`tool-runtime`

## 细分步骤

1. **取消 provider 流** —— 目的：中断模型调用。
2. **取消 tool** —— 目的：中断工具执行。
3. **进程树清理** —— 目的：不遗留子进程。
4. **取消测试** —— 目的：无悬挂进程。

## 主要产出物

- 取消传播 + 进程清理

## 验收标准

- [x] Cancel 不留下运行进程

**相关文档**：[agent-engine](../docs/features/agent-engine.md) · [process](../docs/features/process.md) · [ROADMAP](../ROADMAP.md)
