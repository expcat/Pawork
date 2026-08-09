# P5-5：Compaction 引擎

> Phase 5 · Session、Branch 与 Compaction · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P1-5、P3-2

**最终目的**：实现自动/手动压缩与 `CompactionSnapshot`（版本化摘要），让长会话不超出上下文窗口且压缩前可恢复。

**涉及范围**：`compaction-engine`

## 细分步骤

1. **自动/手动压缩触发** —— 目的：按预算或手动。
2. **生成 `CompactionSnapshot`** —— 目的：结构化摘要。
3. **摘要版本化** —— 目的：可演进。
4. **压缩前 branch 可恢复** —— 目的：可回退。

## 主要产出物

- `compaction-engine` crate

## 验收标准

- [x] 摘要版本化
- [x] 压缩前 branch 可恢复

**相关文档**：[context](../docs/features/context.md) · [sessions](../docs/features/sessions.md) · [ROADMAP](../ROADMAP.md)
