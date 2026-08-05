# ADR-004：大型内容使用 Blob Store

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

Tool Output、图片、文件快照、Provider 原始响应、Diff 等大型内容若直接存入事件或数据库行，会带来复制、膨胀与查询性能问题。

## 决策

大型内容以 BLAKE3 内容寻址存入 Blob Store（`blobs/ab/cd/<hash>`），事件与 Projection 只保存引用。支持去重、引用计数、完整性校验、可配置保留、GC、最大磁盘预算。

## 后果

- 事件流轻量；GUI 通过引用读取大型内容。
- 需要 GC 与引用计数避免磁盘膨胀。
- 完整性校验成为持久化不变量。

## 相关

- [sessions](../features/sessions.md) · [artifacts](../features/artifacts.md) · [ADR-018 大 payload 用 Artifact ID](ADR-018-large-payload-artifact-id.md)
