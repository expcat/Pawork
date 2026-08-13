# P17-1：User Hooks（用户声明式事件钩子）

> Phase 17 · Ecosystem & Host Compatibility · 状态：✅已验收 · 交付成熟度：Accepted + L2 定向门禁 · 依赖：P10-3、P8-1、P4-9、P2-1、P9-3、P11-6、P15-8、P17-5

**最终目的**：为用户提供声明式（配置驱动）的事件钩子系统，按 trigger point 把 Agent / Run 生命周期事件桥接到多种 handler——`Command`（外部命令）、`Http`（webhook）、`PromptTransform`（改写 prompt）、`PromptEval`（模型判定）、`AgentEval`（受限 Agent 判定）、`McpTool`（MCP tool 作为 handler），并区分同步阻断与 async fire-and-forget。与 [P10-3](P10-3-plugin-registration.md) 的 WASM 插件内部 lifecycle hook 划清边界：P10-3 是**进程内、沙箱化、capability 门控**的插件 API 派发（`hook-runtime`）；本任务是**用户配置驱动、经 Policy/Sandbox 约束**的外部/进程外桥接。二者共享同一组 trigger point 词汇但走不同 dispatcher、不同运行时与不同信任边界，互不重复执行。

**涉及范围**：新增 `user-hooks`；复用 `agent-events`（事件源）、`policy-engine`、`http-runtime`、`pty-service`/`process-runtime`、`resource-loader`（配置加载）、`provider-api`（PromptEval/AgentEval 经 canonical provider，P15-8 协商能力）、`mcp-client`（McpTool handler）。新增 handler 类型只复用既有子系统，不重定义运行时语义。

## 细分步骤

1. **Trigger vocabulary 扩展** —— 目的：在 `agent-events` 既有生命周期事件上定义 hook trigger point，统一事件名与负载 schema，至少覆盖 `SessionStart` / `SessionEnd` / `RunStarted` / `RunCompleted` / `RunFailed` / `PromptAssembled` / `PreToolUse` / `PostToolUse` / `ToolFailed` / `PermissionRequest` / `SubagentStart` / `SubagentStop` / `TaskStarted` / `TaskCompleted` / `PreCompact` / `PostCompact` / `Notification`；本任务仅订阅，不修改 P10-3 的派发实现。
2. **HookHandler 统一模型** —— 目的：定义 `HookHandler { Command, Http, PromptTransform, PromptEval, AgentEval, McpTool }` 统一抽象，每个 handler 声明 trigger、permissions、lifecycle、required capability；handler 经依赖注入执行，自身不含 Provider/平台名称分支。
3. **与 P10-3 边界划分** —— 目的：明确 `hook-runtime`（P10-3）负责「WASM 组件内部派发」、本任务负责「用户配置驱动外部/进程外桥接」，二者经同一 trigger registry 注册但走独立 dispatcher，避免重复执行与权限混淆；WASM lifecycle hook 与 User Hook 始终是不同 trust boundary。
4. **Command / Http handler** —— 目的：`Command` 经 `pty-service`/`process-runtime` 执行外部命令、`Http` 经 `http-runtime` 发 webhook（复用重试/超时/错误处理），均受 `policy-engine` + sandbox（P11）约束；配置、Event 与日志只保存 secret 引用，运行前即时解析并仅注入获批的环境变量或 allowlisted HTTP header，执行链全程 redaction，结束后释放明文。**执行所有权约束**：Command handler 属于 Core-owned 本地进程，必须经 `Sandbox Runtime（P11）→ Process Runtime` 统一路径执行外部命令，禁止以 `tokio::process::Command`（或任何直连 spawn 方式）绕过沙箱；进程生命周期（cleanup / 进程树回收）与 policy 判定统一由 Sandbox/Process Runtime 承担，不另起一套 cleanup 或 policy 逻辑。
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

- [x] 六类 `HookHandler`（Command / Http / PromptTransform / PromptEval / AgentEval / McpTool）可按 trigger 触发（`crates/user-hooks/tests/user_hooks.rs` 全类覆盖）
- [x] Trigger vocabulary 覆盖 Session/Run/Prompt/Tool/Permission/Subagent/Task/Compact/Notification 等（17 项 + P10-3 canonical 映射单测）
- [x] PromptTransform 可审计、有 diff、有作用域，且不可绕过 system/security policy
- [x] PromptEval / AgentEval 经 canonical `provider-api`，不含 Provider 名分支；AgentEval 为受限 Agent
- [x] 与 P10-3 WASM lifecycle hook 共享 trigger 词汇但运行时与信任边界独立，不重复执行
- [x] 所有外部执行经 `policy-engine` + sandbox 约束，secret 不入库 / 日志；async 不阻塞 run loop

## 收尾回归（已落地）

- `app-service`（`user_hook.rs` 测试模块）：
  - pre_prompt 携带真实完整 payload，同时分别保存 System / User / Injected 的目标原文；`replace` / `prefix` / `suffix` 仅基于目标角色自身原文计算，三目标均回归且无全量 prompt 双写；
  - PromptTransform 先作用于请求副本，再校验 message identity、role、metadata、非目标 content 与非文本 security content；校验失败拒绝且原请求不变；
  - PromptEval / AgentEval 的 `Transform` 真实回灌到 User message，并再次经过 PromptTransform policy 与同一 post-validation；二次策略拒绝不会留下 transform effect；
  - pre_prompt PromptEval 判定 deny → 返回 `Err`（run 收敛 Failed）；`DispatchOutcome::is_denied` 同时尊重显式 policy denial 与 Judge deny effect；
  - Eval timeout / error 默认 fail-closed；仅配置显式 `on_failure=allow` 且当前 workspace 经真实 trust resolver 判定为 trusted 时允许 fail-open；
  - 生产默认 `ReadOnly + untrusted`，每次 policy action 从 `WorkspaceList` 的真实 trust 状态解析；未知 workspace / 无 workspace 均 fail-closed，且 capability 永不声明 `allowed_in_untrusted_workspace=true`；
  - AgentEval 的 `restricted_profile` 按 workspace 从 P17-5 `ResourceLoader.profiles_v2` 解析；未知 profile / workspace、缺模型、未声明 restricted/container isolation 均拒绝，不回退 default；独立 profile 的 system/instructions、model、effort、tool rules、max turns、isolation 均进入执行配置；
  - handler token/time 预算必须显式且为正；与 profile 全维预算取更严格上限；工具必须同时命中 handler allowlist 与 profile 显式 allowlist，deny 优先；
  - pre_tool 经真实 `register_mcp_server`（minimal McpPeer → discovery → McpToolAdapter）注册的 MCP tool deny → 调用从执行列表移除；
  - SubagentStart / SubagentStop 映射（实例级 task_kind 记录，非 Agent 任务收敛 TaskStarted/TaskCompleted）；
  - `SqliteHookAuditSink` 写失败可见（`failure_count`）+ 同 event_id 重放去重（INSERT OR IGNORE）；
  - 真实 Command（Sandbox→Process）超时收敛 `timed_out` + stdout 按 env 明文 redaction；
  - 真实 HTTP（静默 listener）超时收敛。
- `user-hooks`：Command/HTTP 超时审计状态 `Timeout` 且生效 timeout 传给执行器；Http URL（query）在 policy 描述中 redact、body 模板渲染后无明文、审计全程无明文。
- `core-runtime`：`CoreRuntimeConfig.user_hooks` 注入经 `with_config` 到达 `AppService`（`user_hooks_active`），默认配置不注入。
- `apps/pawork`：无 workspace（headless/serve）时 global hooks 仍加载并注册进宿主 dispatcher。

## 验证记录（2026-08-12）

- `cargo check -p user-hooks -p app-service -p core-runtime -p pawork`：通过。
- `cargo test -p user-hooks`：5 个 unit + 25 个 integration 全部通过。
- `cargo test -p app-service --lib user_hook::tests`：17/17 通过。
- `cargo test -p pawork --lib user_hooks::tests`：5/5 通过。
- `cargo test -p core-runtime user_hooks_config_injection_reaches_service`：1/1 通过。
- `cargo clippy -p user-hooks --all-targets --no-deps -- -D warnings`：通过。
- `cargo clippy -p app-service --lib --no-deps -- -D warnings`：通过。
- `cargo clippy -p core-runtime --lib --no-deps -- -D warnings`：通过。
- `cargo clippy -p pawork --lib --no-deps -- -D warnings`：通过。
- P17-1 写集合内 Rust 文件定向 `rustfmt --check`：通过；未运行 workspace full gate（未命中升级条件，且工作区含并行 P17-7/P17-11 改动）。

> 注：macOS Seatbelt 后端在本机探测通过但真实 spawn（sandbox-exec + 生成 profile）即 abort（exit 134），属平台既有问题（不在本任务范围）；Command 回归用永远可用的 `NativeRestricted` 软沙箱后端走同一执行链验证。

**相关文档**：[P10-3 WASM lifecycle hooks](P10-3-plugin-registration.md) · [P15-8 Capability Discovery](P15-8-capability-discovery.md) · [plugins](../docs/features/plugins.md) · [policy](../docs/features/policy.md) · [sandbox](../docs/features/sandbox.md) · [mcp](../docs/features/mcp.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；复用 `agent-events` / `policy-engine` / `http-runtime` / `pty-service` / `provider-api` / `mcp-client`。新 crate `user-hooks` 依赖方向：`agent-domain → user-hooks → app-service`；与 `hook-runtime`（P10-3）并列、互不依赖，仅共享 trigger point 词汇。
