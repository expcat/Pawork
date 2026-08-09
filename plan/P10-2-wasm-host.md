# P10-2：WASM Host

> Phase 10 · WASM Plugin · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P10-1

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

- [x] 可加载/卸载 WASM 组件

**实现**：新增 `wasm-plugin-host`，以 Wasmtime 27 Component Model 加载固定 `invoke(string) -> string` ABI；每插件使用独立 Store，load/unload、调用、trap 与畸形 ABI 均收敛为结构化 `PluginError`，不会把组件失败传播为 Core panic。unload 与 load 串行，并等待旧实例完整的 snapshot→invoke→state apply 事务结束后返回。

## 验证记录（2026-08-09）

- `cargo test -p wasm-plugin-host`：40 passed，0 failed；真实 inline Component WAT 覆盖 load/invoke、并发同 ID load、async unload（含 retained handle 失效、等待 state apply、阻止同 ID 抢先重载）、未知 import、无导出与 trap 隔离。

**相关文档**：[plugins](../docs/features/plugins.md) · [ADR-012](../docs/adr/ADR-012-wasm-first-plugin.md) · [ADR-013 无 native dylib](../docs/adr/ADR-013-no-native-dylib-plugin.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：用 wasmtime + wit-bindgen（Component Model 成熟、采用率高）；fuel / 内存上限直接实现 ADR-012 的预算隔离，与 P10-5 capability 门控保持一致。

> 落地说明：host runtime 只直接链接 Wasmtime；`wit-bindgen` 面向插件 guest/SDK 生成，v1 WIT 事实源已入库。测试使用 inline Component WAT，避免 CI 额外安装 wasm target，也不把 guest toolchain 带入 `pawork`。
