# pawork-tools

> 工具层：八个内置 Agent 工具（读/列/找/搜/写/改/补丁/命令）、最小调度器（ToolRegistry + ToolScheduler：注册、Policy 闸门、审批解析、并发上限、超时）、MCP 客户端子系统（`mcp/`，rmcp SDK 隔离在 `codec.rs` 单文件）。依赖 domain / policy / exec / workspace / auth，被 `pawork-engine` 与 app 宿主消费。

## 1. 职责与边界

- **内置工具**：八个 `pawork_domain::AgentTool` 实现；一切路径输入 = `workspace_id + relative_path`，统一经 [`pawork-policy`](policy.md) `resolve_workspace_path` 解析——模型无法用绝对路径直达文件系统。
- **调度**：`ToolRegistry` 是唯一注册表（内置与 MCP 工具同表）；`ToolScheduler::execute_named` 串起「查表 → Policy 裁决 → 审批解析 → 并发信号量 → 超时 → 执行」。
- **MCP**：配置解析与校验、受管客户端（惰性连接/退避重连/超时/取消）、能力发现与 `{server}.{tool}` 命名空间注册、stdio 服务器强制经 Sandbox Runtime 托管、Secret 只存 locator（`SecretRef`）、PKCE OAuth。
- **不做**：不做风险分类与裁决（policy）；不实现进程/沙箱原语（exec）；不持久化事件（engine/store 侧）。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~30 | 门面：11 个模块声明 + re-export（八工具、`NoopToolEventSink`、registry/scheduler 全家、`pub mod mcp`）。 |
| `src/common.rs` | ~230 | 公共层：`BuiltinToolError` 与 → `ToolError` 集中映射；取参 `require_str`/`opt_str`/`opt_u64`/`opt_bool`；`workspace_roots`；`resolve_write_rel`（包 `resolve_workspace_path`）；`atomic_write`（同目录临时文件 + rename，覆盖保留既有 Unix mode）。 |
| `src/read_file.rs` | ~500（逻辑 ~260 + 测试） | `ReadFileTool`：行号视图、offset/limit、编码探测（chardetng + encoding_rs）、二进制检测（NUL + 控制字节占比）、4 MiB 读上限 / 256 KiB 输出上限。 |
| `src/list_directory.rs` | ~440（逻辑 ~300 + 测试） | `ListDirectoryTool`：目录优先字典序、BinaryHeap 单扫描取 offset+limit 窗口（内存 O(offset+limit)）、entry kind/size/mtime/symlink 目标（目标相对化，越 root 省略）。 |
| `src/find_files.rs` | ~380（逻辑 ~280 + 测试） | `FindFilesTool`：逗号分隔 glob（globset）、`ignore` walker（尊重 .gitignore、隐藏文件默认跳过、不 follow symlink）、file_type/max_depth/max_results、每项 `resolve_write_rel` 复核（逃逸与 `.git` 静默跳过）、`spawn_blocking` 执行。 |
| `src/search_text.rs` | ~500（逻辑 ~390 + 测试） | `SearchTextTool`：固定串/regex（regex crate）、context_lines、case_sensitive、glob 过滤、`hidden(false)`（搜隐藏文件但仍尊重 .gitignore）、`spawn_blocking`、每 64 个候选查取消、输出预算 256 KiB。 |
| `src/write_file.rs` | ~330（逻辑 ~160 + 测试） | `WriteFileTool`：整文件原子写、自动建父目录、覆盖保留 mode。 |
| `src/edit_file.rs` | ~530（逻辑 ~345 + 测试） | `EditFileTool`：单段（`old_string`/`new_string`）或多段 `edits[]`；全部替换先内存预演再一次原子写；可选 `allow_fuzzy`（行对齐 whitespace 归一化匹配）。 |
| `src/apply_patch.rs` | ~520（逻辑 ~365 + 测试） | `ApplyPatchTool`：多文件 `ops[]`（create/update/delete/rename）、`dry_run` 预演、执行前逐文件字节备份、失败自动恢复备份（含删除新建文件）；`ApplyPatchError::Partial` 报出 failed_op 与 applied 清单。 |
| `src/run_command.rs` | ~830（逻辑 ~430 + 测试） | `RunCommandTool`：argv 优先 / `command` 经平台 shell；cwd 相对解析；timeout/输出/资源 clamp；手工构造 `SandboxPolicy` + `SandboxSelector::pick`；domain↔exec 取消桥；`metadata.sandbox` 上报后端选择。 |
| `src/scheduler.rs` | ~1080（逻辑 ~400 + 测试） | `ToolRegistry` / `ToolRegistryError`、`ToolScheduler` / `ToolSchedulerConfig` / `SchedulerError`、审批接口 `ApprovalResolver` / `ApprovalOutcome` / `AutoApproveResolver`、闸门 `check_gate` 与约束注入、`NoopToolEventSink`。 |
| `src/mcp/mod.rs` | ~250（含测试） | MCP 边界类型：`McpError`（10 变体）、`McpServerCapabilities`、`McpToolInfo`、`McpToolCall`、`McpPeer` trait；re-export sandbox 三件套；「公开源码不得出现 rmcp」守卫测试。 |
| `src/mcp/capabilities.rs` | ~740（逻辑 ~300 + 测试） | 能力桥：`McpCapabilities::discover`、`McpToolAdapter`（MCP 工具 → `AgentTool`）、`register_server_tools` / `register_discovered_tools`、`namespaced_name`；workspace/工具白名单、非对象输入拒绝、输出预算、信任钳制。 |
| `src/mcp/codec.rs` | ~700（逻辑 ~430 + 测试） | **rmcp SDK 唯一隔离点**（crate 私有 mod）：`RunningClient`/`ClientPeer` 包装、initialize 握手、`Tool → McpToolInfo`（read_only_hint）、`CallToolResult → ToolResult`（structured_content 进 metadata、is_error 转 error context）、`apply_tool_result_budget`（UTF-8 安全截断）、`timed`/`should_retry`、streamable-http 构建、`test_support::InProcessConnector`。 |
| `src/mcp/config.rs` | ~875（逻辑 ~485 + 测试） | `McpConfig`（读 `ResolvedConfig.extra["mcp"]` 已合并层）、`McpServerConfig{transport, auto_start, timeout_ms, restart, permissions, trusted}`、`TransportSpec::{Stdio, Http}` 校验、`RestartPolicy`、`McpPermissions`、`StdioSandboxRuntime`、`SecretResolvingConnector`、服务器名禁 `.`。 |
| `src/mcp/manager.rs` | ~590（逻辑 ~375 + 测试） | `ManagedMcpClient`：惰性连接、指数退避有界重启（耗尽后冷却 4×max_delay）、请求超时、shutdown 取消在途、`HealthSnapshot`/`ConnectionState`；`should_retry` 触发单次强制重连重试；实现 `McpPeer`。 |
| `src/mcp/oauth.rs` | ~370（逻辑 ~150 + 测试） | PKCE：`begin_pkce_login` / `complete_pkce_login`（换码 + 存储）；`McpBearerProvider`（到期自动 refresh）；`OAuthHttpConnector`；测试与构造一律 `pawork_auth::http_client()`（`redirect(none)`），不使用 `Client::new()`。 |
| `src/mcp/sandbox.rs` | ~580（逻辑 ~390 + 测试） | stdio 托管：`StdioSpawner` trait / `SandboxedStdioSpawner`（唯一生产实现，走 `SandboxBackend::spawn_interactive`）/ `SpawnedStdio`（AsyncRead/AsyncWrite 适配，stdout 预算 8 MiB fail-closed 断连）；`apply_mcp_stdio_env_hygiene`（env_clear + untrusted allowlist + 追加 deny `PAWORK_API_KEY_*`，不改 network_mode）。 |
| `src/mcp/security.rs` | ~205（逻辑 ~115 + 测试） | `SecretRef{service, account}`（只序列化 locator；`resolve` 强制 `pawork.mcp.*` 前缀，Provider/OAuth 域 fail-closed）；`ResolvedSecret`（Debug/Display 恒 `[REDACTED]`）。 |
| `src/mcp/transport.rs` | ~400（逻辑 ~300 + 测试） | 传输配置（crate 私有 mod，类型 pub 但包外不可命名）：`StdioTransportConfig` / `HttpTransportConfig` / `TransportConfig`（携密字段 Debug 手写 redact、URL 打码 userinfo/query/fragment）、`McpConnector` trait（pub(crate)）、`DefaultConnector`。 |

无 `tests/` 目录与 fixture 文件；全部回归内联 `#[cfg(test)]`（edit_file / apply_patch 各含 proptest）。

## 3. 对外 API 面

### 3.1 八个内置工具

统一形状：实现 `AgentTool`（`descriptor()` + `execute(request, context, sink, cancel)`）；输入 JSON object；路径参数一律 workspace 相对（绝对/穿越/`.git`/symlink 逃逸由 policy 内核拒绝）；构造函数均 `new(Arc<WorkspaceService>)`。**全部 descriptor `requires_approval=false`**——是否询问完全由 scheduler 按 `ApprovalMode` + `ToolCapability` 裁决（§4.1），descriptor 该字段只是给 MCP 等外部工具用的叠加闸。

| 工具 | capability | untrusted 可用 | default_timeout / max_output | 输入 |
| --- | --- | --- | --- | --- |
| `read_file` | ReadOnly | 是 | 10s / 256 KiB | `path`；`offset`（1 起行号）、`limit`（默认 2000 行） |
| `list_directory` | ReadOnly | 是 | 10s / 256 KiB | `path`（`"."` = root，空串报错）；`limit`（默认 500）、`offset`（默认 0） |
| `find_files` | ReadOnly | 是 | 30s / 256 KiB | `pattern`（逗号分隔 glob）；`file_type`（file 默认/dir/any）、`max_depth`、`max_results`（默认 200） |
| `search_text` | ReadOnly | 是 | 30s / 256 KiB | `pattern`；`is_regex`（默认 false）、`glob`、`context_lines`（默认 2）、`max_results`（默认 100）、`case_sensitive`（默认 true） |
| `write_file` | WorkspaceWrite | 否 | 10s / 16 KiB | `path`、`content` |
| `edit_file` | WorkspaceWrite | 否 | 10s / 32 KiB | `path` + （`old_string`/`new_string`）或 `edits[]`；`allow_fuzzy`（默认 false） |
| `apply_patch` | WorkspaceWrite | 否 | 15s / 64 KiB | `ops[]`（`op`=create/update/delete/rename + `path`/`content`/`to`）、`dry_run` |
| `run_command` | Process | 否 | descriptor 不设（超时自带） | 见下方专列 |

各工具输出与行为要点：

- `read_file`：`{行号:>6}\t行文本` 视图；metadata 报 encoding/offset/limit/total_lines/truncated；读上限 4 MiB（超出截断续报）；NUL/控制字节占比判定二进制，只报类型与大小不吐内容。
- `list_directory`：目录优先字典序，行格式 `{size:>8}  {kind}  {name}[ -> target]`，kind ∈ `file/dir/symlink/broken_symlink`；metadata.entries 结构化（name/kind/size/mtime_ms/is_symlink/symlink_target）；symlink 目标相对化、越 root 时省略（不泄漏宿主路径）；dangling symlink 不致败。
- `find_files`：相对路径列表（字典序稳定，不按 mtime）；尊重 .gitignore、跳过隐藏文件；每 64 项检查取消；`.git` 与逃逸 symlink 被 `resolve_write_rel` 复核静默跳过。
- `search_text`：匹配块（`path:line:` + 前后 context）；搜隐藏文件但尊重 .gitignore；非法 regex → InvalidInput；逐文件 `read_to_string`（非 UTF-8 文件跳过）。
- `write_file`：原子写 + 自动建父目录 + 覆盖保留 mode；metadata `{path, bytes}`。
- `edit_file`：内存预演全段——0 命中 NotFound、>1 命中 Conflict（报命中数）、`old==new` Conflict、无净变化 NotFound，任一失败整体不落盘；成功一次原子写；metadata 报 replacements。fuzzy 匹配为行对齐 whitespace 归一化，仍要求唯一命中。
- `apply_patch`：`dry_run` 只报计划不落盘；执行前对受影响文件做字节备份，op 失败自动恢复（改写还原、新建删除、删除恢复），以 `Partial` 报 failed_op + applied 清单；metadata.changes 逐 op 记录。
- `run_command` 输入专列：
  - `argv[]`（优先）或 `command`（Unix 包 `sh -c`，Windows 包 `cmd /d /s /c`）。
  - `cwd`：workspace 相对，经 `resolve_write_rel` 解析（默认第一 root）。
  - `timeout_ms`：clamp 100..600_000，默认 30_000；`max_output_bytes`：≤8 MiB，默认 8 MiB。
  - `env{}`：显式注入对（仍过 denylist）；资源四项 `cpu_seconds`（默认 60/上限 600）、`memory_mb`（2048/8192）、`open_fds`（1024/4096）、`max_procs`（64/256）。
  - 输出：stdout 文本 + 非空时 `[stderr]` 段 + 非零退出时 `[exit N]`；metadata：exit_code / timed_out / truncated / limits / **sandbox**（backend/isolation/fallback/note/attempted）。

四个只读工具 `allowed_in_untrusted_workspace=true`；四个副作用工具为 false（untrusted workspace 里被 policy 信任门直接 Deny）。

### 3.2 公共层（common）

- `BuiltinToolError` 变体：`MissingField(&'static str)` / `InvalidField{field, detail}` / `Path(WorkspacePathError)` / `PolicyPath(PathSafetyError)` / `Io(std::io::Error)` / `Workspace(WorkspaceError)` / `Process(String)` / `Other(String)`。
- 映射规则（`From<BuiltinToolError> for ToolError`）：
  - MissingField / InvalidField → `InvalidInput`。
  - `PathSafetyError::Empty`、`WorkspacePathError::Empty` → `InvalidInput`；`NoRoot` → `NotFound`。
  - **其余一切路径安全违规（绝对/穿越/`.git`/symlink 逃逸/NonRegular）→ `PermissionDenied`**。
  - Io 的 NotFound → `NotFound`，其余 Io → `ExecutionFailed`（retryable）；Workspace(NotFound) → `NotFound`。
- `workspace_roots(&WorkspaceService, &WorkspaceId)`：未知 workspace → `WorkspaceError::NotFound`。
- `resolve_write_rel(roots, rel)`：全部八工具（含只读）解析路径的统一入口。
- `atomic_write(path, bytes)`：同目录临时文件 + rename；目标已存在时保留其 permissions。

### 3.3 调度层

- `ToolRegistry`：`new` / `register(Arc<dyn AgentTool>)` / `extend` / `get` / `descriptor` / `descriptors` / `len` / `is_empty`。仅接受 `ToolKind::ClientFunction` 且 descriptor 合法（`ToolRegistryError::InvalidDescriptor` / `UnsupportedKind`）；同名注册为覆盖语义（MCP 重连刷新用）。
- `ToolSchedulerConfig { max_concurrent: 8, approval_mode: ApprovalMode::ReadOnly, workspace_trusted: false }`——**默认即最保守档**。
- `ToolScheduler::new(registry, config)` / `tool_count()` / `approval_mode()` / `workspace_trusted()` / `with_approval_snapshot(mode, trusted)`（克隆工具表到新 scheduler，旧实例不变，供宿主 Arc-swap） / `execute_named(name, request, context, cancel, approval: Option<&dyn ApprovalResolver>, sink) -> Result<ToolResult, ToolError>`。
- `ApprovalResolver`（async trait）：`resolve(&[ToolRequest]) -> Vec<ApprovalOutcome{approved: bool, reason: Option<String>}>`；`can_resolve_policy_prompt() -> bool`（默认 true；`AutoApproveResolver` 覆写为 **false**——自动批准器只能过 descriptor 叠加闸，不能替用户回答 policy `AskUser`）。
- 错误面：未知工具 → `ToolError{kind: NotFound}`；policy `Deny` 与审批拒绝**不是 Err**，而是 `Ok(ToolResult{success: false, error: Authorization})`（对模型可见的失败结果，Agent loop 可继续）；超时 → `kind: Timeout`。
- 内部 `SchedulerError` 只是 `check_gate` 的中间形态，公开面统一为 `ToolResult` / `ToolError`。
- `NoopToolEventSink`：丢事件 sink，测试与最小宿主用。
- 装配约定：宿主构造 `Arc<WorkspaceService>` → 八工具 `new` → `ToolRegistry::register` → `ToolScheduler::new`；MCP 工具经 `register_server_tools` 进同一 registry，两类工具走同一 `execute_named` 闸门，无旁路。

### 3.4 MCP 子系统

- **配置入口**：`McpConfig::from_resolved(&ResolvedConfig)` / `from_value(&Value)`（读已按 global→workspace→session→run 合并后的 `extra["mcp"]`）；`servers: BTreeMap<name, McpServerConfig>`；服务器名非空且禁 `.`（进入工具命名空间）。
- `McpServerConfig::build_client(name, Arc<dyn SecretBackend>, Option<StdioSandboxRuntime>) -> Result<ManagedMcpClient, McpError>`：
  - stdio 传输缺 runtime 直接 `McpError::Config`（fail-closed）；http 不需要 runtime。
  - `runtime_options` 暴露请求超时（默认 30s）与 `RestartPolicy`（默认 max_attempts=1、base 200ms、cap 10s）。
- **传输规格**：`TransportSpec::Stdio{command, args, env: BTreeMap<String, SecretRef>}` / `Http{url, headers: BTreeMap<String, SecretRef>}`（serde tag=`kind`）。校验：仅 http/https scheme、拒 URL userinfo 与 fragment、明文 http 携密 header 仅 loopback 允许。
- **权限**：`McpPermissions { allowed_tools: BTreeSet<String>, allowed_workspaces: BTreeSet<String>, max_output_bytes: u64（默认 1 MiB） }`：
  - 空集 = 不限制；非空 = 白名单。
  - `allowed_tools` 双重生效：注册期过滤（不在名单的工具不进 registry）+ 调用期复核。
  - `allowed_workspaces` 调用期按 `context.workspace_id` 校验，违规返回 Authorization 失败结果。
  - `max_output_bytes` 是 codec 输出预算（`apply_tool_result_budget` 的上限来源）。
- **边界类型**：`McpServerCapabilities { tools, resources, prompts: bool }`（服务器 initialize 广播；未广播 tools 的服务器跳过工具注册）；`McpToolInfo { name, description, input_schema, read_only }`；`McpToolCall { name, arguments }`；`McpPeer` trait（`server_capabilities` / `list_tools` / `call_tool`）是 manager 与 capabilities 之间的抽象缝，测试用 in-process peer 替换。
- **受管客户端**：`ManagedMcpClient` 实现 `McpPeer`；另有 `ping()`、`health() -> HealthSnapshot{state: ConnectionState, transport, last_error, last_connected_at, restart_attempts, max_restart_attempts}`、`shutdown()`（5s 优雅关闭）。
- **能力桥**：`register_server_tools(registry, server, peer, permissions, trusted, host_trusted)` → `McpCapabilities::discover`（握手能力 + list_tools）+ 白名单过滤 + 注册，返回 descriptors；`register_discovered_tools` 供已有发现结果复用。`McpToolAdapter` descriptor 规则：
  - 注册名 = `namespaced_name(server, tool)` = `{server}.{tool}`（服务器名禁 `.` 保证无歧义）。
  - `read_only_hint=true` → `ToolCapability::ReadOnly` + `requires_approval=false`。
  - 否则 → `ExternalPlugin` + `requires_approval=true`（descriptor 叠加闸生效，policy 放行后仍需 resolver 确认）。
  - `allowed_in_untrusted_workspace = read_only || trusted`，且注册期 `trusted &&= host_trusted`（MCP 配置的 trusted 不得越过宿主信任地板）。
- **Secret 域**：`SecretRef::new(service, account)` / `.resolve(&dyn SecretBackend) -> Result<ResolvedSecret, McpError>`；service 必须 `pawork.mcp.*` 前缀（Provider/OAuth 命名空间 fail-closed）；`ResolvedSecret` 与全部 transport 配置 Debug/Display 手写 redact；`McpError` 文案不含明文。
- **OAuth**：`begin_pkce_login(PkceFlowConfig) -> PkceSession`；`complete_pkce_login(session, code, state, http, backend, display_name) -> StoredCredential`；`McpBearerProvider::bearer()`（到期自动 refresh）；`OAuthHttpConnector`（transport 层注入 `Authorization: Bearer`，拒绝配置里已有 Authorization header；token 轮换要求重建 transport）。
- **stdio 托管**：`StdioSpawner` trait + `SandboxedStdioSpawner` + `SpawnedStdio`（`pub use` 于 `mcp`）；`apply_mcp_stdio_env_hygiene(&mut SandboxPolicy)`。
- `McpError` 变体：`Config / Transport / Protocol / Disconnected / Timeout(Duration) / Cancelled / PermissionDenied / Secret / OAuth / Registry(ToolRegistryError)`。
- 内部但值得知道：`codec.rs` 私有；`StdioTransportConfig`/`HttpTransportConfig` 在私有 `mod transport`——以其为参数的公开函数实际只能由 crate 内装配。

## 4. 核心行为与数据流

### 4.1 `execute_named` 一次调度

1. 查 `ToolRegistry`，未知名 → `NotFound` 错误。
2. `check_gate`：组装 `PolicyInput{capability=descriptor.capability, input=request.input, trusted=config.workspace_trusted, allowed_in_untrusted_workspace, approval_mode}` → `PolicyEngine::decide`。
3. 裁决处置：
   - `Deny{reason}` → 返回 `Ok(失败 ToolResult(Authorization))`。
   - `AskUser{prompt}` → 仅当有 resolver 且 `can_resolve_policy_prompt()==true` 才转交（approved 放行 / 拒绝 → 失败结果）；否则 fail-closed 拒绝。
   - `AllowWithConstraints` → 把 `timeout_ms`/`max_output_bytes` 注入 `request.input`（与已有值取更严者）后放行。
   - `Allow` → 继续。
4. 叠加闸：policy 放行但 `descriptor.requires_approval=true`（MCP 写工具）时仍需 resolver 确认一次（无 resolver → 拒绝；`AutoApproveResolver` 在此闸有效）。
5. 获全局 `Semaphore` 许可（`max_concurrent`）→ descriptor 有 `default_timeout_ms` 则 `tokio::time::timeout` 包裹 → `tool.execute(...)`；超时 → `Timeout`；取消由各工具协作检查。

### 4.2 `run_command` 全流程

1. scheduler 闸门中 policy 已做 shell 风险分类与灾难地板判定（[policy.md](policy.md) §4.3）；`NeverAsk` 下注入的执行约束与显式输入取更严者。
2. 工具内解析输入：非空 `argv` 优先；否则 `command` 包平台 shell（Unix `sh -c`，Windows `cmd /d /s /c`）；`cwd` 相对解析进 workspace root（默认第一 root）；clamp timeout / 输出 / 资源四项。
3. 手工构造 `SandboxPolicy`：read/write roots = workspace roots、deny = `default_secret_paths()`、`NetworkMode::Enforce`、**`allow_spawn=true`**（区别于 exec 的 `untrusted_default`——命令执行本身已过 policy 闸门）、`max_procs` = clamp 值、`env_clear=true`、allowlist = `default_env_allowlist()` ∪ 显式 `env` 键、denylist = `["*TOKEN*","*KEY*","*SECRET*"]`——显式传入的 env 若命中通配同样被剥除，宿主 Secret 与模型注入的 Secret 都进不了子进程。
4. `SandboxSelector::new().pick()` 选后端 → `spawn_stream`（exec 管线：软限制 → 平台翻译 → 进程树守卫，见 [exec.md](exec.md) §4.3）。
5. 取消桥：spawn 一个任务监听 domain `CancellationToken`，触发即 exec token `.cancel()`。
6. 收集事件流：stdout/stderr 分别累积（exec 层已按合计预算截断）；组装文本（stdout + `[stderr]` 段 + 非零 `[exit N]`）与 `metadata.sandbox = {backend, isolation, fallback, note, attempted[], limits}`（内联 golden 钉形状）——回退可观测地呈现给上层与用户。
7. 退出/超时/取消路径由 exec 保证 5s 内整树回收；`timed_out` → 失败结果（Timeout error context），非零 exit → `success: false`（ExecutionFailed）。

### 4.3 文件写路径（write / edit / apply_patch 共通）

1. `workspace_roots` 取 roots → `resolve_write_rel(roots, path)`（拒绝绝对/穿越/`.git`/逃逸/非常规文件，错误映射见 §3.2）。
2. 内存预演全部变更（edit 的段替换、patch 的 op 计划）；任何一段失败整体失败，不触盘。
3. 落盘：`atomic_write`（写类）；apply_patch 执行前对受影响文件做字节备份，op 失败即恢复备份（改写还原、新建删除），并以 `Partial` 报出 failed_op 与 applied 清单（proptest 断言恢复字节精确）。

### 4.4 MCP 服务器接入与调用

1. `McpConfig::from_resolved` 解析校验；每个 server `build_client`：stdio 必须携 `StdioSandboxRuntime{backend, policy, workspace_roots}`（缺失 fail-closed），构造 `SecretResolvingConnector`。
2. 首次请求触发惰性 connect：`SecretRef` 逐项 `resolve`（仅 `pawork.mcp.*`）→ 组装 transport → stdio 经 `SandboxedStdioSpawner.spawn`（`apply_mcp_stdio_env_hygiene`：env_clear + untrusted allowlist + 追加 deny `PAWORK_API_KEY_*`；`spawn_interactive` 进沙箱；stdout 预算 8 MiB 超限断连）→ codec initialize 握手。
3. `register_server_tools` 发现工具 → 白名单过滤 → `McpToolAdapter` 以 `{server}.{tool}` 注册进同一 `ToolRegistry`（与内置工具同表同闸门）。
4. 调用：scheduler 闸门（ExternalPlugin 走 descriptor 叠加审批）→ adapter 校验 workspace/tool 白名单（违规 → Authorization 失败结果而非异常）→ 非对象输入拒绝 → `ManagedMcpClient::call_tool`（超时/取消/`should_retry` 单次强制重连重试）→ codec 转换 → `apply_tool_result_budget` 按 `max_output_bytes` UTF-8 安全截断（structured_content 超预算整体丢弃并标记 truncated）。
5. 断连恢复：指数退避（base×2^n 封顶 max_delay）至 `max_attempts` 耗尽 → `Disconnected`；冷却 4×max_delay 后允许再试；crash 重启复用同一 spawner（沙箱保证不降级）。

### 4.5 取消与超时的传播路径

1. 取消源头是 domain `CancellationToken`（engine/宿主持有），经 `execute_named` 原样传入工具。
2. 只读四工具：走 `spawn_blocking` 的（find/search）每 64 个候选检查一次并在进入阻塞前检查；read_file 在读文件前后检查。命中即返回 `ToolError::cancelled`（kind=Cancelled）。
3. `run_command`：桥接任务把 domain 取消翻译为 exec token cancel → exec 监督循环 kill 整树（[exec.md](exec.md) §4.1）。
4. 超时双层：descriptor `default_timeout_ms` 由 scheduler 的 `tokio::time::timeout` 强制（工具无感知）；`run_command` 的 `timeout_ms` 由 exec 层强制（`timed_out` 标记）。两层语义不同：前者报 `Timeout` 错误，后者是带上下文的失败结果。

## 5. 契约与不变量

- **路径红线**：所有文件类工具输入 = `workspace_id + relative_path`，唯一解析入口 `resolve_workspace_path`；`.git` 与 symlink 逃逸永拒（PermissionDenied），错误信息不回显宿主绝对路径。工具层无绕行通道。
- **rmcp 隔离**：SDK 类型只出现在 `mcp/codec.rs`；守卫测试 `public_sources_do_not_mention_rmcp` 扫描 `src/mcp/*.rs`（除 codec）断言无 `rmcp::` / `use rmcp`。SDK 升级不外溢。
- **MCP Secret 域**：配置只持久化 `SecretRef{service, account}`；service 强制 `pawork.mcp.*`（Provider/OAuth 命名空间 fail-closed）；`ResolvedSecret` 与 transport 配置 Debug 全 redact；`McpError` 文案无明文。
- **stdio 必沙箱**：本地 MCP stdio 服务器唯一 spawn 路径是 `SandboxBackend::spawn_interactive`；无 runtime 即拒建；`PAWORK_API_KEY_*` 永不进子进程环境。
- **信任地板**：MCP `trusted` 注册期被 `host_trusted` 钳制；写类 MCP 工具在 untrusted workspace 恒不可用。
- **调度默认最保守**：`ToolSchedulerConfig::default` = `ReadOnly` 档 + untrusted + 并发 8；policy `AskUser` 无有权 resolver 时 fail-closed 拒绝；`AutoApproveResolver` 不能回答 policy prompt。
- **原子性**：write/edit 单文件原子写；edit 多段与 apply_patch 多文件「全成或全滚」，回滚字节精确（proptest 钉死）。
- **`metadata.sandbox` 形状**：run_command 必带后端选择证据（backend/isolation/fallback/note/attempted/limits），`metadata_sandbox_shape_and_limits_golden` 钉死——「fail-closed 可观测回退」在工具层的落点。
- **run_command 环境卫生**：`env_clear=true` + denylist `*TOKEN*/*KEY*/*SECRET*`（优先于 allowlist），宿主与显式注入的命中项都被剥除。
- 无独立 golden 文件；上述契约全部由内联测试承载。

## 6. 依赖关系

- **workspace 内**：`pawork-domain`（AgentTool/ToolResult/CancellationToken 等 canonical 类型）、`pawork-policy`（路径内核 + PolicyEngine）、`pawork-exec`（Process/Sandbox Runtime）、`pawork-workspace`（WorkspaceService、ResolvedConfig）、`pawork-auth`（SecretBackend、OAuth 原语、`http_client()`）。
- **外部**：`tokio`、`async-trait`、`serde/serde_json`、`thiserror`、`tracing`、`ignore`、`globset`、`regex`、`chardetng`、`encoding_rs`、`rmcp`（仅 codec）、`reqwest`（OAuth）、`url`。dev：`tempfile`、`proptest`、`wiremock`。无 cargo feature。
- **被依赖**：`pawork-engine`（Agent loop 工具执行）、app 宿主（注册与调度装配）。

## 7. 测试与验证资产

默认验证命令：`cargo test -p pawork-tools --offline --lib --tests`（无 `tests/` 目录，用例全部在 `--lib`）。

| 文件 | 覆盖点 |
| --- | --- |
| `common.rs` | 错误映射分流、取参辅助、atomic_write 保留 mode。 |
| `read_file.rs` | 行号/offset/limit、二进制拒吐、绝对与穿越路径拒绝、missing → NotFound、大文件读上限、symlink 逃逸与 `.git` 拒绝。 |
| `list_directory.rs` | 类型/symlink 列举、分页与 total、dangling symlink 容忍、逃逸 symlink 目标省略（不回显宿主路径）、非目录报错。 |
| `find_files.rs` | glob 匹配与字典序、max_results 截断、dir 过滤、遍历中取消、跳过逃逸 symlink 与 `.git`。 |
| `search_text.rs` | 固定串 + context、regex、glob 过滤、非法 regex → InvalidInput、取消、跳过逃逸与 `.git`。 |
| `write_file.rs` | 原子写/建父目录/覆盖保留 mode、路径拒绝。 |
| `edit_file.rs` | 精确单段、不唯一 Conflict、多段原子、预演失败不落盘、fuzzy 归一化与终止换行保留、fuzzy 唯一性计数、proptest（fuzzy 与精确替换一致性）。 |
| `apply_patch.rs` | 多文件 create、dry_run 不落盘、delete+rename、部分失败恢复（create/update/delete 各形态）、proptest 字节精确回滚、op 路径穿越拒绝。 |
| `run_command.rs` | 输出与 exit_code、非零失败、超时、流式先于退出、`platform_environment_allowlist_contains_runtime_basics`、descriptor 无网络旁路参数、clamp 上限、**`metadata_sandbox_shape_and_limits_golden`**、Seatbelt isolation 上报（macOS）、显式 Secret env 被剥除。 |
| `scheduler.rs` | 只读并发、全局并发上限、未知工具、上下文透传、取消（执行前/执行中）、超时映射、审批拒绝不执行、`auto_approve_cannot_resolve_policy_prompt`、registry kind/描述符校验、untrusted 写拒绝（NeverAsk 也拒）、AskForWrites 不可被 AutoApprove 绕过、ReadOnly 档拒写、`process_never_ask_trusted_injects_execution_constraints`、约束与显式输入取更严、灾难地板命令 Deny。 |
| `mcp/mod.rs` | rmcp 隔离守卫扫描、内置与 MCP 工具同表注册。 |
| `mcp/capabilities.rs` | 发现与命名空间注册、read_only 放行、写工具审批与 untrusted 地板、host_trusted 钳制、取消先于远程调用、输出预算截断、非对象输入拒绝、structured_content 保留、workspace/tool 白名单、is_error 转换、未广播 tools 能力跳过。 |
| `mcp/codec.rs` | http 配置校验、auth/header 注入、read_only_hint 往返、UTF-8 截断标记、input_required 状态 fail-closed。 |
| `mcp/config.rs` | keyed map 解析校验、stdio 无沙箱 fail-closed、http 免沙箱、重连复用 spawner、非法 transport/permissions、层级合并、SecretRef 内联明文拒绝、URL userinfo/fragment 拒绝、Debug 无泄漏、超时与重启语义。 |
| `mcp/manager.rs` | 握手/list/call/ping、调用与握手超时、退避重连、尝试耗尽与冷却、shutdown 取消在途、健康快照。 |
| `mcp/oauth.rs` | refresh 流（wiremock）、token 轮换触发重连、已有 Authorization 拒绝、PKCE 换码存储。 |
| `mcp/sandbox.rs` | stdio 经沙箱往返、stdout 预算有界（无半帧）、stderr 打爆预算干净失败、env 卫生化不动 network、`PAWORK_API_KEY_*` 不继承、空命令拒绝。 |
| `mcp/security.rs` | 只序列化 locator、Debug 无明文、resolve 往返、missing → Secret 错误、非 `pawork.mcp.*` 拒绝。 |
| `mcp/transport.rs` | stdio/http Debug 全 redact、URL 打码、非法配置拒绝。 |

## 8. 注意事项与已知限制

- 默认配置（`ReadOnly` 档 + untrusted）下只有四个只读工具可用；一切副作用工具被 policy 直接 Deny——宿主必须显式提升 `ApprovalMode` 并提供 `ApprovalResolver` 才能写盘/执行命令。
- `run_command` 的沙箱策略固定派生（Enforce 网络、deny secret 路径、env_clear），不随 workspace 信任度放宽；放宽属 R7 策略分层。真实隔离强度取决于平台后端（macOS Seatbelt 最强；无硬后端时 NativeRestricted 挡不住命令内部越权读，见 [exec.md](exec.md) §8）。
- `find_files` / `search_text` 尊重 `.gitignore`（被 ignore 的文件搜不到，有意行为）；`find_files` 还跳过隐藏文件，`search_text` 不跳过；`search_text` 逐文件全量读入内存，超大文件受进程内存约束而非显式上限。
- `edit_file` fuzzy 是行对齐 whitespace 归一化匹配，不做语义/缩进感知；替换文本按字面写入。
- MCP：HTTP transport 不经进程沙箱（无本地进程，凭 URL 校验与 Secret 域约束）；`ManagedMcpClient` 无后台心跳，断连在下次请求才被发现（`ping()` 供宿主探活）；`auto_start` 仅配置位，启动编排在宿主。
- `StdioTransportConfig` / `HttpTransportConfig` 位于私有 `mod transport`（类型 pub 但包外不可命名）——以其为参数的公开函数（如 `OAuthHttpConnector::new`）实际只能由 crate 内部装配，这是刻意的封装边界而非疏漏。
- `list_directory` 的 `path` 不接受空串（`PathSafetyError::Empty` → InvalidInput），列 root 用 `"."`；`read_file` 对超过 4 MiB 的文件只读前 4 MiB 并标记 truncated，不报错。
- 相关文档：[policy.md](policy.md)（裁决与路径内核）、[exec.md](exec.md)（执行原语）、[../flows.md](../flows.md)（跨包链路）、[../../architecture.md](../../architecture.md)、[../../design.md](../../design.md)、[../README.md](../README.md)、[AGENTS.md](../../../AGENTS.md)。
