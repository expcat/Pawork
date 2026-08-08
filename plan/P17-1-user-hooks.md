# P17-1：User Hooks（用户声明式事件钩子）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P10-3、P8-1、P4-9、P2-1、P9-3、P11-6、P15-8

**最终目的**：为用户提供声明式（配置驱动）的事件钩子系统，按 trigger point 把 Agent / Run 生命周期事件桥接到多种 handler——`Command`（外部命令）、`Http`（webhook）、`PromptTransform`（改写 prompt）、`PromptEval`（模型判定）、`AgentEval`（受限 Agent 判定）、`McpTool`（MCP tool 作为 handler），并区分同步阻断与 async fire-and-forget。与 [P10-3](P10-3-plugin-registration.md) 的 WASM 插件内部 lifecycle hook 划清边界：P10-3 是**进程内、沙箱化、capability 门控**的插件 API 派发（`hook-runtime`）；本任务是**用户配置驱动、经 Policy/Sandbox 约束**的外部/进程外桥接。二者共享同一组 trigger point 词汇但走不同 dispatcher、不同运行时与不同信任边界，互不重复执行。

**涉及范围**：新增 `user-hooks`；复用 `agent-events`（事件源）、`policy-engine`、`http-runtime`、`pty-service`/`process-runtime`、`resource-loader`（配置加载）、`provider-api`（PromptEval/AgentEval 经 canonical provider，P15-8 协商能力）、`mcp-client`（McpTool handler）。新增 handler 类型只复用既有子系统，不重定义运行时语义。

## 细分步骤

1. **Trigger vocabulary 扩展** —— 目的：在 `agent-events` 既有生命周期事件上定义 hook trigger point，统一事件名与负载 schema，至少覆盖 `SessionStart` / `SessionEnd` / `RunStarted` / `RunCompleted` / `RunFailed` / `PromptAssembled` / `PreToolUse` / `PostToolUse` / `ToolFailed` / `PermissionRequest` / `SubagentStart` / `SubagentStop` / `TaskStarted` / `TaskCompleted` / `PreCompact` / `PostCompact` / `Notification`；本任务仅订阅，不修改 P10-3 的派发实现。
2. **HookHandler 统一模型** —— 目的：定义 `HookHandler { Command, Http, PromptTransform, PromptEval, AgentEval, McpTool }` 统一抽象，每个 handler 声明 trigger、permissions、lifecycle、required capability；handler 经依赖注入执行，自身不含 Provider/平台名称分支。
3. **与 P10-3 边界划分** —— 目的：明确 `hook-runtime`（P10-3）负责「WASM 组件内部派发」、本任务负责「用户配置驱动外部/进程外桥接」，二者经同一 trigger registry 注册但走独立 dispatcher，避免重复执行与权限混淆；WASM lifecycle hook 与 User Hook 始终是不同 trust boundary。
4. **Command / Http handler** —— 目的：`Command` 经 `pty-service`/`process-runtime` 执行外部命令、`Http` 经 `http-runtime` 发 webhook（复用重试/超时/错误处理），均受 `policy-engine` + sandbox（P11）约束；配置、Event 与日志只保存 secret 引用，运行前即时解析并仅注入获批的环境变量或 allowlisted HTTP header，执行链全程 redaction，结束后释放明文。
5. **PromptTransform handler** —— 目的：`on PromptAssembled → 改写 system / user / injected context`，是可改写 Agent 输入的 handler；必须可审计、记录 diff、有作用域（workspace/global），且**不允许绕过 system / security policy**；改写以 canonical event 记录以便审计与重放。
6. **PromptEval / AgentEval handler** —— 目的：`PromptEval` 调用模型做 hook 判定（是否允许继续 / 是否满足条件 / 是否阻断）；`AgentEval` 用一个受限 Agent（独立 profile、受限 tools、受限预算）执行 hook 判定；二者经 canonical `provider-api`（P15-8 协商能力），不按 Provider 名分支，结果（allow/deny/transform）回灌决策。
7. **McpTool handler** —— 目的：调用 MCP tool 作为 hook handler（复用 `mcp-client` P9），经 P9-5 每 server 独立审批与输出限制，handler 不获额外特权。
8. **async 与同步语义** —— 目的：区分同步阻断（需等待结果，如 PromptTransform/PromptEval/AgentEval/McpTool）与 async fire-and-forget（如通知类 Command/Http），async 不阻塞 run loop，失败仅记录不中断；同步阻断超时按策略降级。
9. **定向 / Mock 测试** —— 目的：六类 handler 各一条触发、PromptTransform diff 可审计、PromptEval/AgentEval 经 mock provider 判定、McpTool 经 mock server、策略拒绝生效、secret 不泄露、async 不阻塞主循环、与 P10-3 派发互不干扰。仅定向 + Mock smoke。

## 主要产出物

- `user-hooks`：trigger registry + 六类 `HookHandler` dispatcher + 配置 schema
- PromptTransform 审计（diff / canonical event）+ PromptEval/AgentEval/McpTool 接线
- 定向测试（Command / Http / PromptTransform / PromptEval / AgentEval / McpTool / 策略拒绝 / 边界隔离）

## 验收标准

- [ ] 六类 `HookHandler`（Command / Http / PromptTransform / PromptEval / AgentEval / McpTool）可按 trigger 触发
- [ ] Trigger vocabulary 覆盖 Session/Run/Prompt/Tool/Permission/Subagent/Task/Compact/Notification 等
- [ ] PromptTransform 可审计、有 diff、有作用域，且不可绕过 system/security policy
- [ ] PromptEval / AgentEval 经 canonical `provider-api`，不含 Provider 名分支；AgentEval 为受限 Agent
- [ ] 与 P10-3 WASM lifecycle hook 共享 trigger 词汇但运行时与信任边界独立，不重复执行
- [ ] 所有外部执行经 `policy-engine` + sandbox 约束，secret 不入库 / 日志；async 不阻塞 run loop

**相关文档**：[P10-3 WASM lifecycle hooks](P10-3-plugin-registration.md) · [P15-8 Capability Discovery](P15-8-capability-discovery.md) · [plugins](../docs/features/plugins.md) · [policy](../docs/features/policy.md) · [sandbox](../docs/features/sandbox.md) · [mcp](../docs/features/mcp.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；复用 `agent-events` / `policy-engine` / `http-runtime` / `pty-service` / `provider-api` / `mcp-client`。新 crate `user-hooks` 依赖方向：`agent-domain → user-hooks → app-service`；与 `hook-runtime`（P10-3）并列、互不依赖，仅共享 trigger point 词汇。
