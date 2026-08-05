# P1-6：Blob Store

> Phase 1 · 基础设施 · 状态：🟢已完成 · 依赖：P1-2

**最终目的**：实现内容寻址 Blob Store（BLAKE3 + 引用计数 + GC），为大 payload（tool output / diff / artifact）提供去重存储（ADR-004/018）。

**涉及范围**：`artifact-store`

## 细分步骤

1. **BLAKE3 寻址写入** —— 目的：内容去重。
2. **引用计数** —— 目的：跟踪引用，安全 GC。
3. **完整性校验** —— 目的：防损坏。
4. **GC 与磁盘预算** —— 目的：回收无引用 blob。

## 主要产出物

- `artifact-store` crate

## 验收标准

- [x] 相同内容去重
- [x] GC 可回收无引用 blob
- [x] integrity check 可用

**相关文档**：[artifacts](../docs/features/artifacts.md) · [ADR-004 Blob Store](../docs/adr/ADR-004-blob-store.md) · [ADR-018](../docs/adr/ADR-018-large-payload-artifact-id.md) · [ROADMAP](../ROADMAP.md)
