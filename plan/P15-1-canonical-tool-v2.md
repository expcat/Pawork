# P15-1：Canonical Tool v2（三执行位点统一）

> Phase 15 · Provider Native Capabilities · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P0-4、P0-5、P3-4、P4-9、P5-7

**最终目的**：把现有「只有客户端函数工具」的 canonical tool 模型升级为 v2，统一表达三种执行位点——`ClientFunction`（Core 本地执行，如 read_file）、`ProviderHosted`（Provider 服务端内置工具，如 web_search）、`ProviderExtension`（Provider 中介的外部工具/连接器/远程 MCP）。让 Tool Scheduler 与 CanonicalModelRequest 按位点分流，并把本地 `ToolResult` 与 Provider transcript 严格分道，为 P15-2/3/4 的现代 Provider API 与 P15-5 的 server tool 事件打下唯一数据底座，Agent Core 仍不感知 Provider 名称。

**涉及范围**：`agent-domain`（ToolKind / ContinuationMode / ToolDescriptor v2 / CanonicalModelRequest hosted tools 字段）、`tool-runtime`（scheduler 路由分流）、`provider-runtime`（请求侧声明 hosted tools 与 transcript continuation）、`builtin-tools`（ClientFunction 行为不变，仅标注）；新增/扩展类型须在 [workspace 结构](../docs/architecture/workspace-layout.md) §2 登记，不新增 crate。

## 细分步骤

1. **ToolKind 三态枚举与执行位点** —— 目的：在 `agent-domain` 新增 `ToolKind { ClientFunction, ProviderHosted, ProviderExtension }` 表达三种执行位点（「谁执行」直接由 `ToolKind` 承载，早期计划的 `ExecutionOwner` 冗余枚举已按 [P15-10](P15-10-review-remediation.md) 删除）。三种执行位点与回填语义必须严格区分：
   - **Core 执行（ClientFunction）**：Provider 发出 tool_call → Core Tool Scheduler 执行 → `ToolResult` → Provider；这是唯一由 Pawork 本地执行的位点。
   - **Provider Hosted Tool（ProviderHosted）**：Provider 自己执行 → Provider 返回结果 → Pawork 只记录 / 归一 / 重放，**绝不尝试本地 `AgentTool::execute`**。
   - **Provider Extension（ProviderExtension）**：由 Provider 中介的 MCP / Connector / Remote extension 执行，拥有明确的 approval / audit / execution ownership，Core 参与审批与审计但不持有执行体。
2. **ToolDescriptor v2** —— 目的：在既有 `ToolDescriptor` 上补 `kind: ToolKind`、`hosting`（执行位点细节，如 hosted tool 名 `web_search`）、`capabilities`（能力标签，供 P15-8 协商）、`requires_approval`（与 PolicyEngine 对齐）；ClientFunction 既有字段语义不变。
3. **本地 ToolResult 与 ContinuationMode 分离** —— 目的：冻结 `ContinuationMode { CoreSuppliedResult, ProviderTranscript }`。`ToolResult` 只表达 `ClientFunction` 的 Core 执行结果，可包含本地工具产生的普通 content / artifact / metadata；`Citation` / `Source` 是共享内容类型，但 Provider hosted/extension 的调用 id、状态、引用和结果只进入 P15-5 `ServerToolEvent` / Provider transcript envelope，**不得**塞入 `ToolResult.metadata` 或伪造成客户端函数结果。只有 `CoreSuppliedResult` 由适配器翻译为 Provider 的 function-result 字段；`ProviderTranscript` 由适配器续接 Provider 原生 output item / cursor / transcript reference。
4. **CanonicalModelRequest 声明 hosted tools** —— 目的：在请求结构新增 `hosted_tools: Vec<HostedToolRequest>`（如 `{ kind: WebSearch, ... }`）与 `extensions`，供 P15-2/3/4 适配器翻译为各 Provider 内置工具参数；不携带 Provider 名称。
5. **Scheduler 三位点路由** —— 目的：`tool-runtime` 调度器按 `ToolKind` 分流——`ClientFunction` 走既有本地执行+checkpoint+审批；`ProviderHosted` 的 tool_call 不在本地执行，直接归一为 P15-5 的 ServerToolEvent（本任务仅留 trait/接口，实际事件落 P15-5）；`ProviderExtension` 走审批闸门后由 Provider 中介通道回填结果。
6. **ProviderExtension 审批与审计** —— 目的：复用 P4-9 PolicyEngine，对 `ProviderExtension` 的注册与首次调用要求显式审批（未信任工作区默认拒绝），所有调用写入审计日志（不落 Secret）。
7. **能力标签与协商占位** —— 目的：为 `ToolDescriptor.capabilities` 定义稳定枚举，至少覆盖 Web Search / Web Fetch / File or Collection Search / X Search / Code Execution / Hosted Shell / Provider Apply Patch / Computer Use / Image Generation / server-side MCP / Tool Search / Memory / Programmatic Tool Calling / server-side Multi-Agent；reasoning effort 与 citation/source 由同一 capability vocabulary 表达。匹配逻辑落 P15-8。
8. **Mock smoke 三位点** —— 目的：用 Mock Provider + Mock Tool 覆盖三种位点各一条最小链路（ClientFunction 本地执行、ProviderHosted 仅声明+事件回填、ProviderExtension 审批后中介回填），验证路由分流正确、不串味。

## 主要产出物

- `agent-domain`：`ToolKind`、`ContinuationMode`、`ToolDescriptor v2`、`CanonicalModelRequest.hosted_tools`；本地 `ToolResult` 边界冻结
- `tool-runtime`：三位点路由分流 + ProviderExtension 审批接线
- Mock smoke 三位点用例

## 验收标准

- [x] `ToolKind` 三态枚举落地（P15-10 删除冗余的 `ExecutionOwner`），三类工具可在同一 registry 共存
- [x] ClientFunction 行为与 P4-* 既有语义完全一致（回归不退化）
- [x] ProviderHosted tool_call 不触发本地 `AgentTool::execute`，Core 只记录/归一/重放（断言 + Mock smoke）
- [x] ProviderHosted / ProviderExtension 结果走 `ProviderTranscript` 语义（归一为 ServerToolEvent），不被伪装成本地 `ToolResult`
- [x] ProviderExtension 在未信任工作区被默认拒绝，需显式审批（用例）
- [x] `CanonicalModelRequest.hosted_tools` 不含 Provider 名称（`no_provider_branch` 风格断言）
- [x] 不新增第三方依赖；定向/Mock smoke 通过，不要求 workspace 全量门禁

## 验证记录（2026-08-11）

- 领域与路由：`ToolKind` / owner / continuation 映射、legacy descriptor 默认值、三位点 registry、descriptor-only hosted/extension、request 声明权威分类与本地执行负向断言通过。
- Policy 与恢复：Hosted descriptor gate、Extension 显式审批与未信任拒绝、deny fail-closed、dispatch cancel、`ProviderTranscriptContinued` 持久化与 recovery 续接回归通过。
- 兼容消费者：builtin tools、MCP、WASM plugin host 与现有 Provider request 构造均已适配；registry 注册错误显式返回，不再 panic；未新增第三方依赖。
- Windows 上额外尝试的完整 `host_wat` 文件被既有 `fuel_exhaustion_is_reported` 非 unwind abort 中断；与本任务改动直接相关的 3 个用例均单独通过，因此不把该非定向环境问题作为 P15-1 门禁。

```text
Validation Level: L1
Affected crates: agent-domain、agent-events、tool-api、provider-api、tool-runtime、agent-engine、app-service、builtin-tools、mcp-client、wasm-plugin-host、test-support、现有 Provider adapter request/contract 直接消费者
Validated: cargo test（7 crate lib，174 passed）/ wasm-plugin-host 3 个受影响 host_wat 用例 / cargo clippy（8 crate，all-targets，0 warning）/ 定向 cargo fmt --check / 直接消费者 cargo check / git diff --check
Targeted regressions: 三位点路由、ToolResult 边界、request 无 Provider 名称、Hosted policy、Extension approve/deny/cancel、all-hosted 与 mixed transcript 路径、崩溃回放、registry fallible API、MCP/WASM 直接消费者
Full workspace gate: NOT RUN（未命中升级条件）
```

**相关文档**：[tools](../docs/features/tools.md) · [providers](../docs/features/providers.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-008 capability 分类](../docs/adr/ADR-008-builtin-tools-capability.md) · [ADR-016 事件持久化重放](../docs/adr/ADR-016-core-event-persist-replay.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：不新增依赖；`ToolKind` 与 hosted tools 字段为纯领域扩展，先于 P15-5/P15-8 落地，避免下游返工。
