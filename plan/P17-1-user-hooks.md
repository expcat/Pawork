# P17-1：User Hooks（用户声明式事件钩子）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P10-3、P8-1、P4-9、P2-1、P11-6

**最终目的**：为用户提供声明式（配置驱动）的事件钩子系统，将 Agent 与 Run 生命周期事件桥接到外部动作——执行 Shell 命令、调用 HTTP / Webhook、注入或改写 Prompt，并支持 async（非阻塞 fire-and-forget）。与 [P10-3](P10-3-plugin-registration.md) 的 WASM 插件内部 lifecycle hook 划清边界：P10-3 是**进程内、沙箱化、capability 门控**的插件 API 派发（`hook-runtime`）；本任务是**进程外、用户配置驱动、经 Policy/Sandbox 约束**的外部桥接。二者共享同一组生命周期事件源（trigger point），但走不同 dispatcher、不同运行时与不同信任边界，互不重复执行。

**涉及范围**：新增 `user-hooks`；复用 `agent-events`（事件源）、`policy-engine`、`http-runtime`、`pty-service`/`process-runtime`、`resource-loader`（配置加载）

## 细分步骤

1. **事件源与触发点定义** —— 目的：在 `agent-events` 既有生命周期事件（RunStarted / ToolCall / PromptAssembled / RunCompleted 等）上定义 hook trigger point，统一事件名与负载 schema；本任务仅订阅，不修改 P10-3 的派发实现。
2. **与 P10-3 边界划分** —— 目的：明确 `hook-runtime`（P10-3）负责「WASM 组件内部派发」、本任务负责「进程外用户桥接」，二者经同一 trigger registry 注册但走独立 dispatcher，避免重复执行与权限混淆。
3. **Shell hook** —— 目的：用户配置 `on<event> → 执行外部命令`，经 `pty-service`/`process-runtime` 执行，受 `policy-engine` + sandbox（P11）约束；明文 token 不写入环境变量与日志。
4. **HTTP hook** —— 目的：`on<event> → POST/GET` 指定 URL（webhook），复用 `http-runtime` 的重试 / 超时 / 错误处理（P2），支持 header 注入与 secret 引用（引用不落库）。
5. **Prompt hook** —— 目的：`on PromptAssembled → 注入/改写 prompt 片段`（system / user 段），是唯一可改写 Agent 输入的 hook 类型，受策略限定可改范围，并记录 diff 作为 canonical event 以便审计。
6. **async 与同步语义** —— 目的：区分同步阻断（需等待结果，如 Prompt hook）与 async fire-and-forget（如通知类 HTTP/Shell），async 不阻塞 run loop，失败仅记录不中断。
7. **定向 / Mock 测试** —— 目的：四类 hook 触发、secret 不泄露、策略拒绝生效、async 不阻塞主循环、与 P10-3 派发互不干扰。仅定向 + Mock smoke。

## 主要产出物

- `user-hooks`：trigger registry + 四类 dispatcher + 配置 schema
- 定向测试（Shell / HTTP / Prompt / async / 策略拒绝 / 边界隔离）

## 验收标准

- [ ] 四类 hook（Shell / HTTP / Prompt / async）可按事件触发
- [ ] 与 P10-3 WASM lifecycle hook 共享事件源但运行时与信任边界独立，不重复执行
- [ ] 所有外部执行经 `policy-engine` + sandbox 约束，secret 不入库 / 日志
- [ ] async hook 不阻塞 run loop；Prompt hook 改写可追溯

**相关文档**：[P10-3 WASM lifecycle hooks](P10-3-plugin-registration.md) · [plugins](../docs/features/plugins.md) · [policy](../docs/features/policy.md) · [sandbox](../docs/features/sandbox.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；复用 `agent-events` / `policy-engine` / `http-runtime` / `pty-service`。新 crate `user-hooks` 依赖方向：`agent-domain → user-hooks → app-service`；与 `hook-runtime`（P10-3）并列、互不依赖，仅共享 trigger point 定义。
