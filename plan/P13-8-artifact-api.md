# P13-8：大型 payload Artifact API

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P1-6

**最终目的**：提供通过 Artifact ID 传递大型 payload 的 API（查询/流式读取），保证 GUI Connection Protocol 事件流与 CLI/GUI 轻量，远程与大 diff 不内联传输（[ADR-018](../docs/adr/ADR-018-large-payload-artifact-id.md)）。

**涉及范围**：`app-service`、`artifact-store`、`gui-protocol`

## 细分步骤

1. **Artifact ID 查询/流式读取 API** —— 目的：按需读取大内容。
2. **事件不内联大数据** —— 目的：轻量事件流。
3. **生命周期与可达性** —— 目的：引用安全。
4. **测试** —— 目的：可按 ID 读取。

## 主要产出物

- Artifact API（查询/流式读取，经 GUI Connection Protocol 暴露）

## 验收标准

- [x] 大型 payload 通过 Artifact ID 传递，不内联
- [x] 100,000 行 Diff 不需一次复制到 GUI

## 实现记录（2026-08-10）

- `artifact-store` 补 `BlobId` serde（64-hex 字符串，非法 hex 拒绝）与
  `read_range(offset, limit)` / `byte_length`：读取时全量重读重算 BLAKE3 校验，
  错误语义明确（UnknownBlob / EmptyRange / RangeOffsetOutOfBounds / 损坏）。
- `app-service`：可选注入 `Arc<ArtifactStore>`（`with_artifact_store`，
  `new` 保持 None）；新增 `async artifact_read(artifact_id, offset, limit)`
  流式读取路径：aggregate 记录校验 → 无 store 返回 Unavailable → 非 64-hex
  返回 NotFound → limit==0 读到文件尾 → offset 超尾返回空片 eof=true。
- `gui-server` 会话层 `artifact_chunks` 改 async：经 app-service 读真实
  payload，按 ≤64 KiB 连续分片回 `ArtifactChunk`，末片 eof=true；错误映射
  NotFound→RequestNotFound、其余→Internal。事件流始终只携带 Artifact ID，
  不内联大 payload（[ADR-018]）。
- 测试：app-service 8 项（分片重组 / limit=0 / 无 store / 超尾 / 非 hex /
  损坏 blob）+ gui-server 集成 4 项（约 5.8MiB、100k 行 diff 文本经 memory
  transport 流式读取，80+ 分片重组一致）。
- 已知取舍：`read_range` 每次全量重读校验，分片循环整体为 O(n²/chunk)
  读量；流式校验优化（Merkle / 缓存）留待后续 ADR。

**相关文档**：[artifacts](../docs/features/artifacts.md) · [ADR-004 Blob Store](../docs/adr/ADR-004-blob-store.md) · [ADR-018](../docs/adr/ADR-018-large-payload-artifact-id.md) · [ROADMAP](../ROADMAP.md)
