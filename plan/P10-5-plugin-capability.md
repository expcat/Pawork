# P10-5：Capability / fuel / memory / 时间

> Phase 10 · WASM Plugin · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P10-2

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

- [x] 无默认文件/网络/进程；越权被拒

**实现**：宿主不链接 WASI 且 linker 不注入文件、网络、进程能力；组件、调用输入/输出、Fuel、Store linear memory 与 wall-clock timeout/cancel 均有硬限制。越权 import 在实例化期拒绝，资源耗尽和无限循环只终止对应插件调用。

## 验证记录（2026-08-09）

- `cargo test -p wasm-plugin-host`：40 passed，0 failed；真实 Component 覆盖未知 import、component/input/output 限额、Fuel、memory growth、timeout、cancel、trap 后其他插件存活与非法 host config 拒绝。

**相关文档**：[plugins](../docs/features/plugins.md) · [sandbox](../docs/features/sandbox.md) · [ADR-012](../docs/adr/ADR-012-wasm-first-plugin.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)
