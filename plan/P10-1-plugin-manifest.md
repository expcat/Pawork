# P10-1：Plugin Manifest + signature

> Phase 10 · WASM Plugin · 状态：🟡未开始 · 依赖：P0-6

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

- [ ] 签名校验生效

**相关文档**：[plugins](../docs/features/plugins.md) · [ADR-012 WASM](../docs/adr/ADR-012-wasm-first-plugin.md) · [ROADMAP](../ROADMAP.md)
