# ADR-003：SQLite Event Store 是 Session 事实来源

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

Pi 与 pi-gui 以 JSONL Session 文件为会话记录来源，需承担大型 JSONL 全量扫描成本，难以高效支持分支、恢复、压缩与并发。

## 决策

新核心采用 SQLite Event Store + Materialized Projections + Content-addressed Blob Store。`session_events` 为事实来源，其余表为可重建 Projection。

## 后果

- 大型 Session 无需全量读取即可打开尾部；分支与重放低成本。
- Migration 须只向前、Event Store 不可破坏、Projection 可删后重建。
- 引入 SQLite Actor 模型以避免任意 Task 并发操作。

## 相关

- [sessions](../features/sessions.md) · [ADR-004 Blob Store](ADR-004-blob-store.md) · [ADR-005 Pi 导入](ADR-005-pi-jsonl-import-only.md) · [ADR-016 事件可重放](ADR-016-core-event-persist-replay.md)
