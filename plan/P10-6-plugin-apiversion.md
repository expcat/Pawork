# P10-6：API version 兼容测试

> Phase 10 · WASM Plugin · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P10-3

**最终目的**：建立插件 API 版本兼容测试套件，保证 Plugin API 小而稳定、向前兼容。

**涉及范围**：`wasm-plugin-host`、`test-support`

## 细分步骤

1. **版本兼容矩阵** —— 目的：覆盖 api_version 组合。
2. **兼容测试套件** —— 目的：回归保护。
3. **不兼容拒绝** —— 目的：明确报错。
4. **CI 接入** —— 目的：持续验证。

## 主要产出物

- API version 兼容测试

## 验收标准

- [x] 插件 API 版本兼容可验证

**实现**：Plugin API v1 固定为 `1.0.0`，WIT 事实源位于 `schemas/plugin-api/pawork-plugin-v1.wit`；manifest 以 `semver::VersionReq` 显式声明兼容范围。`test-support::plugin_contract` 提供 11 项 host/plugin 矩阵与复用断言，覆盖 exact/caret/tilde/range、minor 演进、跨 major、不满足最低版本与 prerelease；`plugin-api` 同时冻结 v1 JSON golden，并通过 `wit-bindgen` 编译 WIT guest binding，WIT/JSON 漂移会在 workspace test 中失败。

## 验证记录（2026-08-09）

- Phase 10 相关 7 个 crate（`plugin-api` / `wasm-plugin-host` / `hook-runtime` / `test-support` / `tool-runtime` / `policy-engine` / `agent-engine`）合计 216 项测试通过，0 failed。
- `cargo build --workspace --all-targets`、`cargo test --workspace` 与 `cargo clippy --workspace --all-targets -- -D warnings` 全量通过；`cargo fmt --all -- --check`、`cargo run -p schema-typegen -- --check` 与 `git diff --check` 也均通过。三平台远程发布门禁仍按 L3 节奏执行。

**相关文档**：[plugins](../docs/features/plugins.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)
