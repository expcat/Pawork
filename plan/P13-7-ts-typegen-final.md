# P13-7：TS 类型生成落地

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P0-10、P13-3

**最终目的**：将 TS 类型生成从脚手架落地为自动化管线，覆盖 GUI Connection Protocol 全部 Command/Query/Event/Snapshot 类型，保证生成结果与 Rust 类型一致，供非 Rust 客户端与协议兼容性工具使用；GPUI Desktop 直接消费 Rust `gui-client`。

**涉及范围**：`schemas/gui-protocol`、`schemas/core-api`、`core-api`、`gui-protocol`

## 细分步骤

1. **生成管线落地** —— 目的：自动产出 `.d.ts`。
2. **一致性校验** —— 目的：与 Rust 一致。
3. **CI 强制** —— 目的：防漂移。
4. **覆盖全部协议类型** —— 目的：完整。

## 主要产出物

- TS 类型生成落地（覆盖 GUI Connection Protocol）

## 验收标准

- [x] 生成 TS 类型与 Rust 一致
- [x] 覆盖 Command/Query/Event/Snapshot

## 实现记录（2026-08-10）

- `schema-typegen` 以 `core-api` 的四个信封（AppCommandEnvelope /
  AppQueryEnvelope / AppResponseEnvelope / AppEventEnvelope）与
  `gui-protocol` 的 ClientFrame / ServerFrame 为根做 ts-rs 全量导出，覆盖
  Command / Query / Event / Snapshot / Resume / ArtifactChunk / 错误帧等全部
  协议类型；`versions.ts` 由生成器直接落盘
  （API_VERSION / SUPPORTED_API_VERSIONS，P13-10），非 Rust 客户端拥有与
  Rust 侧一致的版本协商基线。
- `--check` 模式比对 `schemas/` 与 scratch 生成物，漂移即失败；CI 已含
  "TypeScript schema drift check" 步骤（.github/workflows/ci.yml）。
- 验证：`cargo run -p schema-typegen -- --check` 通过，`schemas/core-api` /
  `schemas/gui-protocol` 与 Rust 类型一致。

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [ADR-006](../docs/adr/ADR-006-tauri-via-app-service.md) · [ROADMAP](../ROADMAP.md)
