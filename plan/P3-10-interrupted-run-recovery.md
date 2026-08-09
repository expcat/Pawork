# P3-10：Interrupted Run 恢复

> Phase 3 · Agent Loop · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P1-4、P3-1

**最终目的**：实现崩溃恢复——重启后识别 interrupted Run 并可 resume，让 Core 异常退出后能在 1s 内恢复，是「可重放」承诺的落地。

**涉及范围**：`agent-engine`

## 细分步骤

1. **重启识别 interrupted** —— 扫描未完成 Run。目的：发现需恢复项。
2. **从事件重放重建状态** —— 目的：恢复执行上下文。
3. **resume 接口** —— 目的：可继续执行。
4. **崩溃恢复测试** —— 目的：恢复 < 1s。

## 主要产出物

- Interrupted Run 恢复

## 验收标准

- [x] 重启后可识别 interrupted Run
- [x] 崩溃后恢复 < 1s

**相关文档**：[agent-engine](../docs/features/agent-engine.md) · [sessions](../docs/features/sessions.md) · [ADR-016](../docs/adr/ADR-016-core-event-persist-replay.md) · [ROADMAP](../ROADMAP.md)
