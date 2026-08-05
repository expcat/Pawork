# Artifact 系统

## 职责

以内容寻址方式管理大型内容，事件中只携带 Artifact ID，避免在事件流里内联数 MB 数据。

## Artifact 类型

```text
FileSnapshot
ToolOutput
Image
Diff
Patch
Log
ProviderRawResponse
Export
Report
TerminalCapture
```

## 功能

内容寻址；Metadata；MIME；文件名；Session/Run 关联；预览；导出；生命周期；清理；磁盘预算；Secret 扫描；大型 Artifact 流式读取。

GUI 通过 Artifact ID 获取内容，不在事件中直接携带数 MB 数据。

## Phase 1 Blob 基线

`artifact-store` 已实现 `blobs/ab/cd/<blake3-hash>` 内容寻址布局、原子写入、相同内容去重、SQLite 持久化引用计数、读取时哈希校验、missing / corrupt / orphan 完整性报告、磁盘预算，以及仅删除零引用 Blob 的 GC。MIME、Session / Run 关联、Secret 扫描与 GUI Artifact API 由后续任务补齐。

## 验收标准

- 大型 Tool Output / Provider 响应 / 文件快照走 Artifact
- 引用计数与 GC 可用
- Secret 扫描阻止敏感内容外泄

## 相关文档

- [sessions（Blob Store）](sessions.md) · [context（裁剪）](context.md) · [api-surface](../architecture/api-surface.md)
- [ADR-004 Blob Store](../adr/ADR-004-blob-store.md) · [ADR-018 大 payload 用 Artifact ID](../adr/ADR-018-large-payload-artifact-id.md)
- [ROADMAP P1-6 / P13-8](../../ROADMAP.md)
