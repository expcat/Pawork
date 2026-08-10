# P13-10：GUI Protocol schema 版本化

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P13-3

**最终目的**：完成 GUI Connection Protocol schema 版本化与向后兼容策略，保证 GUI（独立进程、可能独立发布）与 CLI/Core 可独立演进。

**涉及范围**：`schemas/gui-protocol`、`core-api`、`gui-protocol`

## 细分步骤

1. **API version 机制与协商** —— 目的：版本可协商。
2. **向后兼容策略** —— 目的：演进不破坏。
3. **废弃与迁移流程** —— 目的：可控演进。
4. **测试** —— 目的：兼容可验证。

## 主要产出物

- GUI Protocol schema 版本化与兼容策略

## 验收标准

- [x] GUI Protocol schema 完成版本化与兼容策略
- [x] GUI API 有版本与 Contract Tests

## 实现记录（2026-08-10）

- 版本机制：`core-api` 新增版本助手 `ApiVersion::new` / `ApiVersion::bump_minor` 与
  常量表 `SUPPORTED_API_VERSIONS`；`gui-protocol` 提供表式协商
  `negotiate_api_version_with` 与单版本 `negotiate_api_version`，握手接入协商。
- 兼容策略：新增 [ADR-036](../docs/adr/ADR-036-gui-protocol-versioning.md)（minor
  只增、字段级 serde default 双向兼容、枚举变体仅 major bump、废弃流程与删除
  策略、golden + schema-typegen 格式锁定）。
- schema 版本化：schema-typegen 生成 `schemas/core-api/versions.d.ts`
  （`API_VERSION` / `SUPPORTED_API_VERSIONS` 常量表）并汇入 index；非 Rust 客户端
  获得与 Rust 侧一致的协商基线。
- Contract Tests：信封 api_version 校验（`ensure_compatible_api_version` /
  `decode_*_frame_checked`，IncompatibleVersion 产生路径）、协商边界（空列表 /
  全不兼容 / major 不同）、golden JSON fixture（`gui-protocol/tests/fixtures/*.json`，
  锁定线上格式）。
- 定向验证：`cargo test -p gui-protocol -p core-api -p schema-typegen` 全绿；
  `cargo run -p schema-typegen -- --check` 通过；`cargo fmt` 与 `cargo clippy`
  （三包 `--all-targets -- -D warnings`）通过。

### Deferred items（建议/跟踪，本任务不做）

- 服务端按协商 minor 裁剪帧/事件生产的行为随 gui-server（P13-4 起）接线。
- 未来 major bump 的迁移流程与网关方案在真实 major 升级前按 ADR-036 附录登记。

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [ADR-006](../docs/adr/ADR-006-tauri-via-app-service.md) · [ADR-036](../docs/adr/ADR-036-gui-protocol-versioning.md) · [ROADMAP](../ROADMAP.md)
