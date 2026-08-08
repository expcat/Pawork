# P15-1：Canonical Tool v2（三执行位点统一）

> Phase 15 · Provider Native Capabilities · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P0-4、P0-5、P3-4、P4-9、P5-7

**最终目的**：把现有「只有客户端函数工具」的 canonical tool 模型升级为 v2，统一表达三种执行位点——`ClientFunction`（Core 本地执行，如 read_file）、`ProviderHosted`（Provider 服务端内置工具，如 web_search）、`ProviderExtension`（Provider 中介的外部工具/连接器/远程 MCP）。让 Tool Scheduler、CanonicalModelRequest 与 ToolResult 都按位点分流，为 P15-2/3/4 的现代 Provider API 与 P15-5 的 server tool 事件打下唯一数据底座，Agent Core 仍不感知 Provider 名称。

**涉及范围**：`agent-domain`（ToolKind / ToolDescriptor v2 / ToolResult v2 / CanonicalModelRequest hosted tools 字段）、`tool-runtime`（scheduler 路由分流）、`provider-runtime`（请求侧声明 hosted tools）、`builtin-tools`（ClientFunction 行为不变，仅标注）；新增/扩展类型须在 [workspace 结构](../docs/architecture/workspace-layout.md) §2 登记，不新增 crate。

## 细分步骤

1. **ToolKind 三态枚举** —— 目的：在 `agent-domain` 新增 `ToolKind { ClientFunction, ProviderHosted, ProviderExtension }`，明确「谁执行」。`ClientFunction` 由 Core 经 `AgentTool::execute` 执行；`ProviderHosted` 由 Provider 服务端执行，Core 仅声明启用并归一输出；`ProviderExtension` 由 Provider 中介的外部通道执行，Core 参与审批与审计但不持有执行体。
2. **ToolDescriptor v2** —— 目的：在既有 `ToolDescriptor` 上补 `kind: ToolKind`、`hosting`（执行位点细节，如 hosted tool 名 `web_search`）、`capabilities`（能力标签，供 P15-8 协商）、`requires_approval`（与 PolicyEngine 对齐）；ClientFunction 既有字段语义不变。
3. **ToolResult v2 与 ServerTool 载荷** —— 目的：`ToolResult.content` 支持 `CitationPart` / `SourcePart`（指向 P15-5 的 Citation/Source 占位），`metadata` 增加 `server_tool` 段承载 hosted tool 返回的调用 id、状态、引用；为 hosted/extension 结果预留通道，不强制本地执行。
4. **CanonicalModelRequest 声明 hosted tools** —— 目的：在请求结构新增 `hosted_tools: Vec<HostedToolRequest>`（如 `{ kind: WebSearch, ... }`）与 `extensions`，供 P15-2/3/4 适配器翻译为各 Provider 内置工具参数；不携带 Provider 名称。
5. **Scheduler 三位点路由** —— 目的：`tool-runtime` 调度器按 `ToolKind` 分流——`ClientFunction` 走既有本地执行+checkpoint+审批；`ProviderHosted` 的 tool_call 不在本地执行，直接归一为 P15-5 的 ServerToolEvent（本任务仅留 trait/接口，实际事件落 P15-5）；`ProviderExtension` 走审批闸门后由 Provider 中介通道回填结果。
6. **ProviderExtension 审批与审计** —— 目的：复用 P4-9 PolicyEngine，对 `ProviderExtension` 的注册与首次调用要求显式审批（未信任工作区默认拒绝），所有调用写入审计日志（不落 Secret）。
7. **能力标签与协商占位** —— 目的：为 `ToolDescriptor.capabilities` 定义稳定枚举，至少覆盖 Web Search / Web Fetch / File or Collection Search / X Search / Code Execution / Hosted Shell / Provider Apply Patch / Computer Use / Image Generation / server-side MCP / Tool Search / Memory / Programmatic Tool Calling / server-side Multi-Agent；reasoning effort 与 citation/source 由同一 capability vocabulary 表达。匹配逻辑落 P15-8。
8. **Mock smoke 三位点** —— 目的：用 Mock Provider + Mock Tool 覆盖三种位点各一条最小链路（ClientFunction 本地执行、ProviderHosted 仅声明+事件回填、ProviderExtension 审批后中介回填），验证路由分流正确、不串味。

## 主要产出物

- `agent-domain`：`ToolKind`、`ToolDescriptor v2`、`ToolResult v2`（含 Citation/Source 占位）、`CanonicalModelRequest.hosted_tools`
- `tool-runtime`：三位点路由分流 + ProviderExtension 审批接线
- Mock smoke 三位点用例

## 验收标准

- [ ] `ToolKind` 三态枚举落地，三类工具可在同一 registry 共存
- [ ] ClientFunction 行为与 P4-* 既有语义完全一致（回归不退化）
- [ ] ProviderHosted tool_call 不触发本地执行，仅经事件/结果回填（Mock smoke）
- [ ] ProviderExtension 在未信任工作区被默认拒绝，需显式审批（用例）
- [ ] `CanonicalModelRequest.hosted_tools` 不含 Provider 名称（`no_provider_branch` 风格断言）
- [ ] 不新增第三方依赖；定向/Mock smoke 通过，不要求 workspace 全量门禁

**相关文档**：[tools](../docs/features/tools.md) · [providers](../docs/features/providers.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-008 capability 分类](../docs/adr/ADR-008-builtin-tools-capability.md) · [ADR-016 事件持久化重放](../docs/adr/ADR-016-core-event-persist-replay.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：不新增依赖；`ToolKind` 与 hosted tools 字段为纯领域扩展，先于 P15-5/P15-8 落地，避免下游返工。
