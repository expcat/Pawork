# P13-8：大型 payload Artifact API

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟡未开始 · 依赖：P1-6

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

- [ ] 大型 payload 通过 Artifact ID 传递，不内联
- [ ] 100,000 行 Diff 不需一次复制到 GUI

**相关文档**：[artifacts](../docs/features/artifacts.md) · [ADR-004 Blob Store](../docs/adr/ADR-004-blob-store.md) · [ADR-018](../docs/adr/ADR-018-large-payload-artifact-id.md) · [ROADMAP](../ROADMAP.md)
