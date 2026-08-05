# P11-7：进程树清理

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟡未开始 · 依赖：P4-12

**最终目的**：实现三平台取消时的进程树清理，保证取消命令后不遗留子进程（chaos 测试可验证）。

**涉及范围**：`process-runtime`、`sandbox-runtime`

## 细分步骤

1. **进程组/Job 取消** —— 目的：整树终止。
2. **孤儿进程检测** —— 目的：兜底清理。
3. **chaos 测试** —— 目的：极端场景验证。
4. **三平台一致性** —— 目的：行为一致。

## 主要产出物

- 进程树清理（三平台）

## 验收标准

- [ ] 取消命令后无遗留进程（chaos 测试通过）

**相关文档**：[process](../docs/features/process.md) · [sandbox](../docs/features/sandbox.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)
