# P10-3：Tool / command registration + hooks

> Phase 10 · WASM Plugin · 状态：🟡未开始 · 依赖：P10-2
> **边界与扩展**：本任务交付 **WASM 插件**的工具/命令注册与**进程内、沙箱化、capability 门控**的 lifecycle hook 派发（`hook-runtime`）。**用户配置驱动**的事件钩子（Command / Http / PromptTransform / PromptEval / AgentEval / McpTool 外部桥接，见 [P17-1](P17-1-user-hooks.md)）**不在本任务范围**，由 P17-1 独立承接；二者共享同一组 trigger point 词汇但走不同 dispatcher、不同运行时与不同信任边界，互不重复执行。勿把插件 lifecycle hook 误认为已覆盖用户外部钩子。

**最终目的**：实现插件注册工具/命令与生命周期 hook 派发，让 WASM 插件可扩展 Agent 行为。

**涉及范围**：`wasm-plugin-host`、`hook-runtime`

## 细分步骤

1. **注册工具/命令** —— 目的：插件提供能力。
2. **生命周期 hook 派发** —— 目的：插件响应事件。
3. **与 tool-runtime 集成** —— 目的：统一调度。
4. **测试** —— 目的：注册与 hook 生效。

## 主要产出物

- 插件注册 + hooks

## 验收标准

- [ ] 插件可注册工具/命令并响应 hook

**相关文档**：[plugins](../docs/features/plugins.md) · [tools](../docs/features/tools.md) · [ROADMAP](../ROADMAP.md)
