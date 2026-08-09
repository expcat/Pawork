# P10-4：Plugin state

> Phase 10 · WASM Plugin · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P10-2

**最终目的**：实现插件状态保存，让插件可在调用间保持必要状态。

**涉及范围**：`wasm-plugin-host`

## 细分步骤

1. **状态存储抽象** —— 目的：插件可持久化状态。
2. **作用域隔离** —— 目的：插件间互不干扰。
3. **大小限制** —— 目的：防滥用。
4. **测试** —— 目的：状态可保存恢复。

## 主要产出物

- Plugin state

## 验收标准

- [x] 插件状态可跨调用保存

**实现**：宿主提供可注入 `PluginStateStore` 与默认内存 backend，按 `plugin_id + session/workspace/global scope` 隔离；snapshot 携带 revision，成功调用后才原子应用 mutation，并执行 key、单值、总量与冲突限制。无 `PersistentState` capability 时 mutation 被拒绝。

## 验证记录（2026-08-09）

- `cargo test -p wasm-plugin-host`：40 passed，0 failed；覆盖跨调用、跨 plugin/scope 隔离、revision/配额、并发 snapshot→invoke→apply 串行事务、unload 等待 apply，以及无 `PersistentState` 时不读取 snapshot 且拒绝 mutation。

**相关文档**：[plugins](../docs/features/plugins.md) · [ROADMAP](../ROADMAP.md)
