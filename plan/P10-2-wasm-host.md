# P10-2：WASM Host

> Phase 10 · WASM Plugin · 状态：🟡未开始 · 依赖：P10-1

**最终目的**：实现 WASM Component Model 宿主，支持加载/卸载插件，为代码级扩展提供沙箱化运行环境。

**涉及范围**：`wasm-plugin-host`

## 细分步骤

1. **component model 宿主集成** —— 目的：运行 WASM 组件。
2. **加载/卸载** —— 目的：生命周期管理。
3. **崩溃隔离** —— 目的：插件崩溃不致 Core 崩溃。
4. **测试** —— 目的：可加载卸载。

## 主要产出物

- WASM Host

## 验收标准

- [ ] 可加载/卸载 WASM 组件

**相关文档**：[plugins](../docs/features/plugins.md) · [ADR-012](../docs/adr/ADR-012-wasm-first-plugin.md) · [ADR-013 无 native dylib](../docs/adr/ADR-013-no-native-dylib-plugin.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：用 wasmtime + wit-bindgen（Component Model 成熟、采用率高）；fuel / 内存上限直接实现 ADR-012 的预算隔离，与 P10-5 capability 门控保持一致。
