# P4-11：Checkpoint 与回滚

> Phase 4 · 核心工具与权限 · 状态：🟡未开始 · 依赖：P1-6、P1-7（「导出 patch / 固化为 commit」步骤另依赖 P7-1 git-service；Phase 7 完成前先交付基于 Blob Store 的 checkpoint / 回滚）

**最终目的**：实现 Run 写操作 Checkpoint 与回滚（单次 tool call / 整个 run、冲突检测、导出 patch、固化为 commit），让所有 Agent 改动可审查与撤销（ADR-010）。

**涉及范围**：`checkpoint-service`

## 细分步骤

1. **Run 快照** —— HEAD、Git Index fingerprint、修改文件列表、修改前内容 Blob、新增/删除、权限、时间戳。目的：完整可回滚。
2. **回滚单次 tool call / 整个 run** —— 目的：粒度可控。
3. **冲突检测（用户在 Run 后修改）** —— 目的：避免覆盖用户改动。
4. **导出 patch / 固化为 commit** —— 目的：可审查可留存。

## 主要产出物

- `checkpoint-service` crate

## 验收标准

- [ ] 可回滚单次 tool call 与整个 run
- [ ] 不默认 `git reset --hard`
- [ ] 冲突可检测

**相关文档**：[checkpoint](../docs/features/checkpoint.md) · [ADR-010 全写 checkpoint](../docs/adr/ADR-010-checkpoint-all-writes.md) · [ADR-004 Blob Store](../docs/adr/ADR-004-blob-store.md) · [ROADMAP](../ROADMAP.md)
