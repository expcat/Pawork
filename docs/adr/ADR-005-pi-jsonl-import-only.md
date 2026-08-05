# ADR-005：Pi JSONL 仅支持导入

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

为保证向后兼容需保留历史会话，但继续以 JSONL 作为主存储会牺牲性能与一致性。

## 决策

仅提供 Pi JSONL 导入能力：扫描、解析 header、导入消息/工具调用/模型切换/Compaction/Branch/自定义 Entry，保存未知字段，产出迁移报告，且不修改原始 Pi 文件。导入后使用新数据库，不再继续双写。

## 后果

- 用户可迁移历史会话，但不承担 JSONL 全量扫描成本。
- 导入器需独立版本化、保存未知字段以兼容未来格式。

## 相关

- [sessions](../features/sessions.md) · [ADR-003 Event Store](ADR-003-sqlite-event-store.md) · [ROADMAP P5-9](../../ROADMAP.md)
