# P13-7：TS 类型生成落地

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟡未开始 · 依赖：P0-10、P13-3

**最终目的**：将 TS 类型生成从脚手架落地为自动化管线，覆盖 GUI Connection Protocol 全部 Command/Query/Event/Snapshot 类型，保证生成结果与 Rust 类型一致，供 Tauri GUI 与协议测试端使用。

**涉及范围**：`schemas/gui-protocol`、`schemas/core-api`、`core-api`、`gui-protocol`

## 细分步骤

1. **生成管线落地** —— 目的：自动产出 `.d.ts`。
2. **一致性校验** —— 目的：与 Rust 一致。
3. **CI 强制** —— 目的：防漂移。
4. **覆盖全部协议类型** —— 目的：完整。

## 主要产出物

- TS 类型生成落地（覆盖 GUI Connection Protocol）

## 验收标准

- [ ] 生成 TS 类型与 Rust 一致
- [ ] 覆盖 Command/Query/Event/Snapshot

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [ADR-006](../docs/adr/ADR-006-tauri-via-app-service.md) · [ROADMAP](../ROADMAP.md)
