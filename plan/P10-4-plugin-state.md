# P10-4：Plugin state

> Phase 10 · WASM Plugin · 状态：🟡未开始 · 依赖：P10-2

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

- [ ] 插件状态可跨调用保存

**相关文档**：[plugins](../docs/features/plugins.md) · [ROADMAP](../ROADMAP.md)
