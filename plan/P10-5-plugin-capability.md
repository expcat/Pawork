# P10-5：Capability / fuel / memory / 时间

> Phase 10 · WASM Plugin · 状态：🟡未开始 · 依赖：P10-2

**最终目的**：实现插件能力检查与资源限制（capability/fuel/memory/时间），默认无文件/网络/进程，保证插件不可越权（ADR-012）。

**涉及范围**：`wasm-plugin-host`

## 细分步骤

1. **capability 检查** —— 目的：按声明授权。
2. **fuel / memory / 时间限制** —— 目的：防资源滥用。
3. **默认无文件/网络/进程** —— 目的：默认安全。
4. **越权测试** —— 目的：被拒。

## 主要产出物

- Capability + 资源限制

## 验收标准

- [ ] 无默认文件/网络/进程；越权被拒

**相关文档**：[plugins](../docs/features/plugins.md) · [sandbox](../docs/features/sandbox.md) · [ADR-012](../docs/adr/ADR-012-wasm-first-plugin.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)
