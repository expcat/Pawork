# P9-3：Tools / Resources / Prompts

> Phase 9 · MCP · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P9-1

**最终目的**：实现 MCP 能力发现（Tools / Resources / Prompts）并注册到 tool-runtime，让 MCP server 的工具可被 Agent 调用。

**涉及范围**：`mcp-client`

## 细分步骤

1. **能力发现** —— 目的：列出 server 提供的工具/资源/提示。
2. **注册到 tool-runtime** —— 目的：统一调度。
3. **descriptor 转换** —— 目的：符合 ToolDescriptor。
4. **测试** —— 目的：可调用。

## 主要产出物

- MCP 能力发现与注册

## 验收标准

- [x] MCP tools 可注册并被调用

## 验证记录（2026-08-09）

- 能力发现遵循 initialize 声明，覆盖 Tools / Resources / Resource Templates / Prompts；Tool 以 `{server}.{tool}` 注册到 `ToolRegistry` 并可调用。
- 非 object 输入 fail closed，`structuredContent` 与普通内容共用输出预算；`cargo test -p mcp-client`：48 passed，定向 Clippy 通过。

**相关文档**：[mcp](../docs/features/mcp.md) · [tools](../docs/features/tools.md) · [ROADMAP](../ROADMAP.md)
