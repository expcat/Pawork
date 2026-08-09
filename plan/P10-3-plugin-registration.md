# P10-3：Tool / command registration + hooks

> Phase 10 · WASM Plugin · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P10-2
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

- [x] 插件可注册工具/命令并响应 hook

**实现**：manifest 冻结 tool/command/hook 注册契约；宿主以 `<plugin_id>::<local_name>` 注册工具与命令，冲突 fail closed，插件工具统一映射为 `ExternalPlugin` 进入 canonical `ToolRegistry`。新增 `hook-runtime`，按 plugin id 确定性派发、订阅/capability 双门控，并把错误、取消与 panic 隔离为可序列化 outcome。`PluginRuntime` 以单一事务边界协调 component load、三类注册、Start/Stop 与完整撤销；注册集合仅可在 stopped 状态变更。

## 验证记录（2026-08-09）

- `wasm-plugin-host` 40 项、`hook-runtime` 12 项、`tool-runtime` 15 项测试通过；覆盖 `PluginRuntime` load→注册→派发→unload、真实 ToolScheduler/command invoke、具体注册名门控、确定性顺序、完整注销，以及错误/manifest accessor panic/执行 panic/cancel 隔离。

> P10 交付可供组合层调用的插件子系统；`app-service` / `core-runtime` / `pawork` 的正式进程装配属于 P13-1/P13-2，不在本任务中提前接线。

**相关文档**：[plugins](../docs/features/plugins.md) · [tools](../docs/features/tools.md) · [ROADMAP](../ROADMAP.md)
