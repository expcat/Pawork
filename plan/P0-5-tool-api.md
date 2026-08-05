# P0-5：Tool 协议

> Phase 0 · 架构与协议冻结 · 状态：🟢已完成 · 依赖：P0-2

**最终目的**：冻结工具的统一契约（Trait/描述/结果/能力类别/取消），使调度器与权限引擎有统一依据（ADR-008）。内置工具、MCP、WASM 工具都实现同一接口。

**涉及范围**：`tool-api`

## 细分步骤

1. **定义 AgentTool Trait** —— `async fn execute(request, context, sink, cancel) -> ToolResult`，使用 `async-trait` 提供对象安全的异步接口。目的：所有工具的统一接口。
2. **定义 ToolDescriptor** —— name/description/input_schema(JSON Schema)/capability。目的：供模型与权限决策使用。
3. **定义 capability 类别枚举** —— ReadOnly/WorkspaceWrite/GitWrite/Process/Network/UserInteraction/ExternalPlugin。目的：调度（并发/串行）与审批的统一依据。
4. **定义 ToolResult** —— content/artifact_ref/error/is_error/metadata。目的：结果可承载大数据引用。
5. **定义 CancellationToken 语义** —— 协作式取消。目的：工具可被取消且不泄漏资源。

## 主要产出物

- `tool-api` crate：`AgentTool` + `ToolDescriptor` + `ToolResult` + capability + cancel

## 验收标准

- [x] 只读/可并发/timeout/maxOutput/capability 字段齐备
- [x] Trait 为 builtin/MCP/WASM 提供同一 canonical 接口

**相关文档**：[tools](../docs/features/tools.md) · [ADR-008 capability](../docs/adr/ADR-008-builtin-tools-capability.md) · [ROADMAP](../ROADMAP.md)
