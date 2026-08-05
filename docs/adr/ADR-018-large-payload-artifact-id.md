# ADR-018：大型 Payload 通过 Artifact ID 传递

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

Tool Output、Diff、Provider 原始响应等大型内容若直接进事件流，会膨胀内存与渲染负担（如 100,000 行 Diff 不应一次复制到 GUI）。

## 决策

大型内容经 Blob Store / Artifact 持久化，事件与 API 响应只返回 Artifact ID，按需流式读取。

## 后果

- 事件流与 GUI 轻量。
- 需 Artifact 生命周期、磁盘预算与 Secret 扫描。
- 须保证 Artifact 引用的可达性与 GC 安全。

## 相关

- [artifacts](../features/artifacts.md) · [ADR-004 Blob Store](ADR-004-blob-store.md) · [ADR-017 GUI 不直接访问](ADR-017-gui-no-direct-access.md)
