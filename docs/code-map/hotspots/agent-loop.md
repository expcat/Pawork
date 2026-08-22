# Agent loop

一次对话轮次如何跑完工具循环。Engine 不落库、不选通道、不按 Provider 名分支。

## 调用链

1. CLI `chat` / `run` 或 GUI `run_start` → `AppCore::chat_turn*`（实现在 `crates/app/src/services/run.rs`）。
2. 宿主装配 `SessionLoopCtx`（`crates/app/src/loop_ctx.rs`）实现 `pawork_engine::LoopContext`。
3. `pawork_engine::run_session`（`crates/engine/src/tool_loop.rs`）调 `ModelProvider::stream`，把 `ProviderStreamEvent` 映射为 `AgentEvent`。
4. 收集到 tool call 后：`request_approval`（**等待前**必须 emit `ToolApprovalRequested`）→ `execute_tools` → `ToolScheduler`（`pawork-tools`）→ 各 `AgentTool`。
5. 轮数上限 `DEFAULT_MAX_TOOL_ROUNDS = 20`。压缩走 `LoopContext::compact_history`（host 负责 session fork/snapshot）。

## 审批

- 预判：`PolicyEngine::decide`（`pawork-policy`）。`Allow` 不再进 scheduler 的 resolve 钩子。
- `AskUser`：CLI 终端宿主 / GUI `GuiApprovalHost`。`--json` 或非 TTY → `DenyAllApprovals`。
- GUI resume 保留待审批（`resume_messages_keep_pending`）；CLI resume seal `Denied`。
- 文件路径：工具 JSON 相对路径 + `workspace_id` → `policy::resolve_workspace_path`。

## 不要做的

- 在 engine 里 `match provider_id`（见 `crates/engine/tests/no_provider_branch.rs`）。
- 让 engine 依赖 tools / exec / storage。
- 把 hosted / extension 工具的结果收成 `ToolResult`（那是 Provider transcript）。

模块图：[engine](../../../crates/engine/MODULE.md) · [app](../../../crates/app/MODULE.md) · [tools](../../../crates/tools/MODULE.md) · [policy](../../../crates/policy/MODULE.md)
