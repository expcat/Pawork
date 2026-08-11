# Built-in Tools

## 职责

提供编码 Agent 的核心工具能力，统一通过 `AgentTool` 接口与 Tool Scheduler 调度。

## Tool API

```rust
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        sink: &dyn ToolEventSink,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError>;
}
```

`ToolDescriptor` 包括：名称；描述；JSON Schema；权限类别；是否只读；是否可并发；默认 timeout；最大输出；是否允许在未信任工作区运行。

## Tool Result

```rust
pub struct ToolResult {
    pub content: Vec<ContentPart>,
    pub artifacts: Vec<ArtifactReference>,
    pub metadata: serde_json::Value,
    pub truncated: bool,
    pub success: bool,
    pub error: Option<ErrorContext>,
}
```

## ToolKind 三执行位点（Phase 15 起）

自 Phase 15（[P15-1](../../plan/P15-1-canonical-tool-v2.md)）起，工具按 `ToolKind` / `ExecutionOwner` 分流，三类位点互不串味：

```text
ClientFunction    → Core 本地执行（read_file / write_file / run_command …）→ ToolResult(CoreSuppliedResult)
ProviderHosted    → Provider 自执行（web_search / code_execution …）→ ServerToolEvent(ProviderTranscript)，不入本地 execute
ProviderExtension → Provider 中介外部通道（MCP / Connector / Remote）→ 审批+审计后回填(ProviderTranscript)
```

本地 P0/P1 工具均为 `ClientFunction`；`ProviderHosted` / `ProviderExtension` 的 tool_call 不触发本地 `AgentTool::execute()`，结果归一为 [P15-5](../../plan/P15-5-server-tool-events.md) `ServerToolEvent`。详见 [控制流 §5.1](../architecture/control-flow.md)。

`ToolResult` 是 `CoreSuppliedResult` 的本地结果类型，不承载 hosted/extension 调用状态。后两者的 citations / sources / progress / output 只走 `ServerToolEvent` 与 `ProviderTranscript`；Provider adapter 不得把它们翻译成客户端 function-result 字段。

三类工具共享同一个 registry，但只有 `ClientFunction` 条目持有本地 `AgentTool` executor；`ProviderHosted` 与 `ProviderExtension` 仅登记 descriptor。请求中的 `hosted_tools` / `extensions` 声明是本轮执行位点的权威来源，registry 只补充宿主侧 descriptor，避免把 Provider-owned 调用误降级为本地执行。

Provider-owned 调用仍受 Core policy 约束：Hosted 工具按 descriptor 的 `requires_approval` 与未信任工作区许可执行闸门；Extension 首次调用始终要求真实、可审计的显式审批，并在未信任工作区 fail closed。授权后的调用只记录 provider-neutral transcript continuation，不追加空 `Tool` 消息，也不产生本地 `ToolResult`。

## P0 工具

- `read_file`：异步、有界文本读取；offset/limit；行号；编码检测；二进制检测；文件大小限制；图片作为 Attachment；Workspace 路径检查。
- `write_file`：创建文件；原子写入；自动创建父目录；保留权限；保留换行风格；覆盖审批；写入前 Checkpoint。
- `edit_file`：精确文本替换；多段替换；上下文校验；线性时间空白模糊匹配；保留尾换行；冲突报告；原子提交。
- `apply_patch`：多文件 Patch；create/delete/rename；dry run；原子操作；部分失败按 Checkpoint 逐字节回滚（含 create-over-existing）；路径安全。
- `run_command`：非 PTY 命令；实时 stdout/stderr；cwd；timeout；平台环境变量白名单与配置追加；流式总输出上限；cancel；exit code；可 kill 的进程树句柄。
- `search_text`：固定字符串；正则；文件过滤；ignore；结果限制；上下文行；Unicode；遍历期取消；阻塞扫描隔离到 blocking worker。
- `find_files`：glob；文件类型；ignore；最大深度；最大结果；排序；遍历期取消。
- `list_directory`：文件类型；大小；修改时间；symlink / broken symlink；有界内存分页与准确总数。

## P1 工具

`git_status` / `git_diff` / `git_stage` / `git_unstage` / `git_commit` / `git_log` / `git_show` / `create_worktree` / `remove_worktree` / `read_many_files` / `file_metadata` / `diagnostics` / `open_url` / `http_request` / `ask_user` / `select_option` / `request_confirmation`。

## 调度

写操作串行；Shell 默认串行；只读文件可并发；搜索可并发；同文件操作串行；Git Index 操作串行；审批期间暂停相关调用；所有调用可取消。每次调用先经 `PolicyEngine::decide()`，未信任 Workspace 还需通过 descriptor gate；`rm -rf /`、`mkfs*` 与 `dd of=/dev/*` 在 `NeverAsk` 下仍必须拒绝或显式审批。Scheduler 只接受显式工具名，并原样传递真实 `ToolExecutionContext`。

## WASM Plugin 工具（Phase 10）

`wasm-plugin-host` 把签名 manifest 中的 tool registration 转为 `AgentTool`，注册名固定为
`<plugin_id>::<local_name>`；已有名称与同批重复名称均拒绝，不允许覆盖 built-in/MCP/其他插件工具。
`PluginRuntime` 在同一事务中发布/撤销插件工具、命令与 lifecycle hook；调度器从它取得 canonical
`ToolRegistry` 快照，卸载后的旧 adapter 也会被 host active gate 拒绝。
无论插件声明了哪些底层需求，交给 Scheduler 的 capability 始终是 `ExternalPlugin`，并固定
`read_only=false`、`supports_concurrency=false`、`allowed_in_untrusted_workspace=false`，因此插件不能
通过自报 descriptor 绕过审批或串行语义。

调用仍走 `ToolScheduler`，随后由 adapter 封装为 versioned `PluginInvocation` JSON 进入受 Fuel、内存、
时间、输入/输出限制的 Component Store；完整 canonical `ToolResult` 会原样保留，普通 JSON result 则
安全包装为单个 Text content。取消令牌会同时传到 Scheduler 与 WASM epoch interruption。详见
[Plugin 系统](plugins.md)。

## 验收标准

- 所有写操作可回滚（Checkpoint）
- 路径 / symlink 安全测试通过
- 高风险命令请求审批
- 大型输出走 Artifact 引用
- ProviderHosted / ProviderExtension 不触发本地执行，结果走 ServerToolEvent（Phase 15 起）

## 相关文档

- [控制流（调度）](../architecture/control-flow.md) · [policy](policy.md) · [process](process.md) · [checkpoint](checkpoint.md) · [artifacts](artifacts.md)
- [ADR-008 capability 分类](../adr/ADR-008-builtin-tools-capability.md) · [ADR-010 写操作 Checkpoint](../adr/ADR-010-checkpoint-all-writes.md)
- [ROADMAP Phase 4](../../ROADMAP.md)
- [ROADMAP Phase 10（WASM Plugin）](../../ROADMAP.md) · [ROADMAP Phase 15（Canonical Tool v2）](../../ROADMAP.md)
