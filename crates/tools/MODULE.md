# pawork-tools

八个内置工具、最小调度器、MCP client。依赖 domain / workspace / policy / exec / auth。

## 职责

实现 `AgentTool`：只读四件、写入三件、`run_command`；经 `ToolScheduler` 接 `PolicyEngine` 与审批回调。MCP（R1 并入）负责 stdio/HTTP 传输、工具登记到同一 `ToolRegistry`、独立 `mcp-auth.json` 域。`tool_search` 不在本包。

## 模块树

```
src/
  lib.rs  common.rs  scheduler.rs
  read_file.rs  list_directory.rs  find_files.rs  search_text.rs
  write_file.rs  edit_file.rs  apply_patch.rs  run_command.rs
  mcp/{mod,capabilities,config,manager,oauth,sandbox,security}.rs
  mcp/{codec,transport}.rs          # 私有；rmcp 隔离在 codec
```

无 `tests/` 目录。

## 对外入口/API 面

八工具（均 `::new(WorkspaceService)`，实现 `AgentTool`）：

| 类型 | descriptor `.name` |
| --- | --- |
| `ReadFileTool` | `read_file` |
| `ListDirectoryTool` | `list_directory` |
| `FindFilesTool` | `find_files` |
| `SearchTextTool` | `search_text` |
| `WriteFileTool` | `write_file` |
| `EditFileTool` | `edit_file` |
| `ApplyPatchTool` | `apply_patch` |
| `RunCommandTool` | `run_command` |

调度：`ToolRegistry`、`ToolScheduler` / `ToolSchedulerConfig`、`ApprovalResolver` / `AutoApproveResolver`、`ApprovalOutcome`。`common` 为 `pub mod`（路径解析、原子写），不 glob 到根。

`pub mod mcp`：`ManagedMcpClient`、`McpPeer`、`McpToolAdapter`、`TransportSpec::{Stdio,Http}`、`begin_pkce_login` / `complete_pkce_login`、`SecretRef`。

## 依赖与被依赖

- **依赖**：`pawork-domain`、`pawork-workspace`、`pawork-policy`、`pawork-exec`、`pawork-auth`。外部 `rmcp = "=3.1.3"`。无 feature。
- **被依赖**：仅 `pawork-app`（`extensions.rs` 装配八工具 + MCP；`loop_ctx` / `approval` 消费调度器）。engine **不**依赖本包。

## 红线与注意事项

- 路径一律 `context.workspace_id` + 工具 JSON 相对 `path`（`run_command` 用相对 `cwd`），经 `pawork_policy::resolve_workspace_path`。禁止调用方传入任意绝对路径。
- 写工具走 policy 审批；`run_command` 的 env/Secret 路径白名单以 `pawork-exec` 为权威。
- MCP Secret 与主 `auth.json` 隔离（`pawork.mcp.*`）。
- `rmcp` 只允许出现在 `mcp/codec.rs`（`public_sources_do_not_mention_rmcp` 断言）。
- 取消须能杀掉整棵进程树（exec）；本包不自己实现 Job Object。

## 相关文档

- [docs/design.md](../../docs/design.md) §3.2 工具契约 / §4 S2–S4 / S9
- [AGENTS.md](../../AGENTS.md) §8
- [代码地图总索引](../../docs/code-map/README.md)
