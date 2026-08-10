# P0-10：TS 类型生成脚手架

> Phase 0 · 架构与协议冻结 · 状态：🟢已完成 · 依赖：P0-8

**最终目的**：建立「Rust 类型 → TypeScript 类型」生成管线占位，确保 GUI Connection Protocol 契约（`core-api` / `gui-protocol`）可自动同步到非 Rust 客户端与契约工具（[ADR-006](../docs/adr/ADR-006-tauri-via-app-service.md)），避免手写 `.d.ts` 与 Rust 实现漂移。Phase 19 的 GPUI Desktop 直接使用 Rust `gui-client`，不依赖该生成物。

**涉及范围**：`crates/schema-typegen`、`schemas/core-api`、`schemas/gui-protocol`、`.github/workflows/ci.yml`

## 细分步骤

1. **选定生成方案** —— `ts-rs` derive + `schema-typegen` workspace 工具；`cargo run -p schema-typegen` 是唯一生成入口。目的：固化生成入口。
2. **从 CoreCommand/CoreEvent 产出示例 .d.ts** —— 目的：验证管线可用。
3. **CI 加入生成一致性检查** —— 目的：防止手改 `.d.ts` 与 Rust 漂移。

## 主要产出物

- 生成管线占位 + 示例 `.d.ts` + CI 检查

## 验收标准

- [x] 能从 Rust 类型产出 `.d.ts`
- [x] CI 通过 `cargo run -p schema-typegen -- --check` 校验生成结果与提交一致

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [gui-connection](../docs/features/gui-connection.md) · [ADR-006](../docs/adr/ADR-006-tauri-via-app-service.md) · [ROADMAP](../ROADMAP.md)
