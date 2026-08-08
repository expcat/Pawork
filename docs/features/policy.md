# Policy Engine

## 职责

在工具执行前判断是否允许、是否需要审批、是否施加约束。Pi 默认以启动用户系统权限运行、无内置完整权限隔离，新核心不复制该默认行为。

## Approval Mode

```rust
pub enum ApprovalMode {
    AlwaysAsk,
    AskForWrites,
    AskForDangerous,
    OnFailure,
    NeverAsk,
    ReadOnly,
}
```

## Policy 输入

工具；参数；Workspace；Session；当前 Agent；文件路径；Shell 命令；网络域名；Environment；Plugin；用户信任状态；之前的审批；风险级别。

## Policy 输出

```rust
pub enum PolicyDecision {
    Allow,
    Deny { reason: String },
    AskUser { prompt: ApprovalPrompt },
    AllowWithConstraints(ExecutionConstraints),
}
```

## 文件系统安全

必须防止：`../` 路径穿越；symlink 跳出 Workspace；junction；Windows UNC 路径；大小写差异；TOCTOU；覆盖 `.git` 内部文件；写入 Secret 文件；设备文件；FIFO 和 Socket；超大文件。

所有文件操作输入为 `workspace_id + relative_path`，绝不让模型直接传任意绝对路径。

## Shell 安全

高风险命令：递归删除；权限修改；磁盘格式化；系统关机；sudo；注册表修改；Credential 读取；网络上传；Git force push；删除分支；破坏工作区外路径。

不能只靠字符串黑名单，最终须结合：Sandbox；Workspace Boundary；Environment Filter；用户审批；Resource Limit。

## Phase 15–17 执行与 Hook 边界

- Policy 输入包含 `ToolKind` / `ExecutionOwner` / `ContinuationMode` 与后端 trust boundary。`ClientFunction` 可授权 Core 本地执行；`ProviderHosted` / `ProviderExtension` 只能授权声明、外部执行与 transcript continuation，授权不会把它们变成本地工具。
- 跨 Local / MCP / ProviderHosted 的 Browser/Computer fallback 必须重新评估 Policy、显式告知并写审计事件，不继承另一 trust boundary 的审批。
- User Hook 与 WASM lifecycle hook 是独立主体。`PromptTransform` 只能改写允许层，变换后必须重新验证不可变 system/security policy；Command/Http/McpTool/PromptEval/AgentEval 不因 hook 身份获得额外权限。
- Automation 外部 Trigger、Marketplace、Forge publish 与远程 Client Channel 都必须带已认证 `CommandSource`、幂等键和作用域；接收事件不等于授权外部副作用。

## 验收标准

- 路径穿越 / symlink escape / junction / UNC 测试通过
- Shell 注入与高风险命令可拦截或审批
- 未信任 Workspace 默认限制写与命令
- hosted/extension execution、跨 trust-boundary fallback 与 PromptTransform 均不能绕过审批或伪造本地隔离

## 相关文档

- [tools](tools.md) · [sandbox](sandbox.md) · [workspace-index](workspace-index.md)
- [ADR-008 capability 分类](../adr/ADR-008-builtin-tools-capability.md) · [ADR-009 默认 Workspace Trust](../adr/ADR-009-default-workspace-trust.md)
- [ROADMAP P4-9 / P4-10](../../ROADMAP.md)
