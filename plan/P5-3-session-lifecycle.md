# P5-3：Resume / 归档 / 删除 / 重命名

> Phase 5 · Session、Branch 与 Compaction · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P1-4

**最终目的**：实现 session 完整生命周期（resume/归档/删除/重命名）与 session lease，含损坏检测与只读恢复，让会话管理稳健。

**涉及范围**：`session-store`

## 细分步骤

1. **resume/归档/删除/重命名** —— 目的：完整生命周期。
2. **session lease（并发占用保护）** —— 目的：避免重复占用。
3. **损坏检测 + 只读恢复** —— 目的：损坏可降级读取。
4. **生命周期测试** —— 目的：边界正确。

## 主要产出物

- session 生命周期 + lease

## 验收标准

- [x] 损坏可检测并只读恢复

**相关文档**：[sessions](../docs/features/sessions.md) · [ROADMAP](../ROADMAP.md)
