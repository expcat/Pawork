# ADR-010：所有写操作建立 Checkpoint

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

Agent 可能批量修改文件，若无快照则无法按 Tool Call 或 Run 粒度回滚，用户难以撤销 Agent 改动。

## 决策

每次可能修改文件的 Agent Run 创建逻辑 Checkpoint（HEAD、Git Index fingerprint、修改文件列表、修改前内容 Blob、新增/删除文件、权限、时间戳），支持回滚单次 Tool Call 与整个 Run。不默认自动执行 `git reset --hard`。

## 后果

- 所有 Agent 改动可审查与回滚。
- 增加 Blob Store 与元数据成本，由引用计数与 GC 平衡。
- 须检测用户在 Run 后的修改以避免覆盖。

## 相关

- [checkpoint](../features/checkpoint.md) · [tools](../features/tools.md) · [ADR-004 Blob Store](ADR-004-blob-store.md)
