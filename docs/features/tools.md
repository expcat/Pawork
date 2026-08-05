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

## P0 工具

- `read_file`：文本读取；offset/limit；行号；编码检测；二进制检测；文件大小限制；图片作为 Attachment；Workspace 路径检查。
- `write_file`：创建文件；原子写入；自动创建父目录；保留权限；保留换行风格；覆盖审批；写入前 Checkpoint。
- `edit_file`：精确文本替换；多段替换；Unified Patch；上下文校验；模糊匹配；冲突报告；保留编码；原子提交；生成结构化 Diff。
- `apply_patch`：多文件 Patch；create/delete/rename；dry run；原子操作；部分失败回滚；路径安全。
- `run_command`：非 PTY 命令；流式 stdout/stderr；cwd；timeout；环境变量白名单；最大输出；cancel；exit code；进程树终止。
- `search_text`：固定字符串；正则；文件过滤；ignore；结果限制；上下文行；Unicode。
- `find_files`：glob；文件类型；ignore；最大深度；最大结果；排序。
- `list_directory`：文件类型；大小；修改时间；symlink 信息；分页。

## P1 工具

`git_status` / `git_diff` / `git_stage` / `git_unstage` / `git_commit` / `git_log` / `git_show` / `create_worktree` / `remove_worktree` / `read_many_files` / `file_metadata` / `diagnostics` / `open_url` / `http_request` / `ask_user` / `select_option` / `request_confirmation`。

## 调度

写操作串行；Shell 默认串行；只读文件可并发；搜索可并发；同文件操作串行；Git Index 操作串行；审批期间暂停相关调用；所有调用可取消。

## 验收标准

- 所有写操作可回滚（Checkpoint）
- 路径 / symlink 安全测试通过
- 高风险命令请求审批
- 大型输出走 Artifact 引用

## 相关文档

- [控制流（调度）](../architecture/control-flow.md) · [policy](policy.md) · [process](process.md) · [checkpoint](checkpoint.md) · [artifacts](artifacts.md)
- [ADR-008 capability 分类](../adr/ADR-008-builtin-tools-capability.md) · [ADR-010 写操作 Checkpoint](../adr/ADR-010-checkpoint-all-writes.md)
- [ROADMAP Phase 4](../../ROADMAP.md)
