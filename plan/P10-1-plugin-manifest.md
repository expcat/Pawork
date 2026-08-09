# P10-1：Plugin Manifest + signature

> Phase 10 · WASM Plugin · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P0-6

**最终目的**：定义 Plugin Manifest 与签名校验，为 WASM 代码插件提供元数据与可信来源验证（ADR-012）。

**涉及范围**：`plugin-api`、`wasm-plugin-host`

## 细分步骤

1. **Manifest 字段定型** —— id/version/api_version/permissions/capabilities。目的：完整元数据。
2. **签名校验** —— 目的：可信来源。
3. **版本兼容声明** —— 目的：与 api_version 对齐。
4. **测试** —— 目的：非法签名被拒。

## 主要产出物

- Manifest + 签名校验

## 验收标准

- [x] 签名校验生效

**实现**：`plugin-api` 冻结 v1 manifest、注册、权限与签名 envelope；canonical signing payload 使用域分离、稳定 JSON 与 component BLAKE3 摘要。`wasm-plugin-host` 以宿主 trust store 的 Ed25519 key 严格验签，未知 key、畸形签名及 manifest/component 任一篡改均拒绝。

## 验证记录（2026-08-09）

- `plugin-api` 17 项与 `wasm-plugin-host` 40 项测试通过；覆盖 canonical 顺序/大小、component 与 manifest 篡改、未知 key、畸形 Base64 签名和 API 不兼容拒绝。

**相关文档**：[plugins](../docs/features/plugins.md) · [ADR-012 WASM](../docs/adr/ADR-012-wasm-first-plugin.md) · [ROADMAP](../ROADMAP.md)
