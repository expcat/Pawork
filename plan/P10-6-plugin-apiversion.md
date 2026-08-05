# P10-6：API version 兼容测试

> Phase 10 · WASM Plugin · 状态：🟡未开始 · 依赖：P10-3

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

- [ ] 插件 API 版本兼容可验证

**相关文档**：[plugins](../docs/features/plugins.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)
