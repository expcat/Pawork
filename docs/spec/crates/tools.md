# pawork-tools

> 工具层：九个内置 Agent 工具（读/列/找/搜/写/改/补丁/命令/桌面）、最小调度器（ToolRegistry + ToolScheduler：注册、Policy 闸门、审批解析、并发上限、超时）。依赖 domain / policy / exec / workspace / computer-use；被依赖方为 `pawork-app` 与 `pawork-mcp`（engine 不依赖本包，工具由宿主装配进 LoopContext）。

## 1. 职责与边界

- **内置工具**：九个 `pawork_domain::AgentTool` 实现；文件工具的路径输入 = `workspace_id + relative_path`，统一经 [`pawork-policy`](policy.md) `resolve_workspace_path` 解析——模型无法用绝对路径直达文件系统。
- **调度**：`ToolRegistry` 是唯一注册表（内置与 MCP 工具同表）；`ToolScheduler::execute_named` 串起「查表 → Policy 裁决 → 审批解析 → 并发信号量 → 超时 → 执行」。
- **扩展工具**：MCP 适配器在 [mcp](mcp.md)，通过本包 ToolRegistry 注册；本包不包含网络客户端或 MCP SDK。
- **不做**：不做风险分类与裁决（policy）；不实现进程/沙箱原语（exec）；不持久化事件（engine/store 侧）。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~30 | 门面：12 个模块声明 + re-export（九工具、`NoopToolEventSink`、registry/scheduler 全家）。 |
| `src/computer.rs` | — | `ComputerTool`：进程共享隔离桌面会话、按动作必填的 JSON 参数、跨 run 观察隔离、阻塞工作取消、JPEG → canonical Image；复用 Policy / 审批。 |
| `src/common.rs` | ~230 | 公共层：`BuiltinToolError` 与 → `ToolError` 集中映射；取参 `require_str`/`opt_str`/`opt_u64`/`opt_bool`（opt_* 缺省或 `null` → None，类型不符报 InvalidField）；`workspace_roots`；`resolve_write_rel`（包 `resolve_workspace_path`）；`atomic_write`（同目录临时文件经 `create_new` **独占创建**——同名路径已存在（含预置 symlink）即换名重试、不跟随——+ rename，覆盖保留既有 Unix mode）。 |
| `src/read_file.rs` | ~500（逻辑 ~260 + 测试） | `ReadFileTool`：行号视图、offset/limit、编码探测（chardetng + encoding_rs）、二进制检测（NUL + 控制字节占比）、4 MiB 读上限 / 256 KiB 输出上限。 |
| `src/list_directory.rs` | ~440（逻辑 ~300 + 测试） | `ListDirectoryTool`：路径解析与目录扫描均在 `spawn_blocking` 内；目录优先字典序、BinaryHeap 单扫描取 offset+limit 窗口（内存 O(offset+limit)）、entry kind/size/mtime/symlink 目标（目标相对化，越 root 省略）。 |
| `src/find_files.rs` | ~380（逻辑 ~280 + 测试） | `FindFilesTool`：逗号分隔 glob（globset）、`ignore` walker（尊重 .gitignore、隐藏文件默认跳过、不 follow symlink）、file_type/max_depth/max_results、每项 `resolve_write_rel` 复核（逃逸与 `.git` 静默跳过）、`spawn_blocking` 执行。 |
| `src/search_text.rs` | ~530（逻辑 ~410 + 测试） | `SearchTextTool`：固定串/regex（regex crate）、context_lines、case_sensitive、glob 过滤、`hidden(false)`（搜隐藏文件但仍尊重 .gitignore）、`spawn_blocking`、每 64 个候选查取消、输出预算 256 KiB；R-14 单文件 4 MiB 读取上限——超限文件跳过不装入内存，metadata `skipped_oversize` 计数且 `truncated=true` 如实标记结果不完整。 |
| `src/write_file.rs` | ~330（逻辑 ~160 + 测试） | `WriteFileTool`：整文件原子写、自动建父目录、覆盖保留 mode；`spawn_blocking` 承载阻塞 IO。 |
| `src/edit_file.rs` | ~530（逻辑 ~345 + 测试） | `EditFileTool`：单段（`old_string`/`new_string`）或多段 `edits[]`；全部替换先内存预演再一次原子写；可选 `allow_fuzzy`（行对齐 whitespace 归一化匹配）；`spawn_blocking` 承载阻塞 IO。 |
| `src/apply_patch.rs` | ~520（逻辑 ~365 + 测试） | `ApplyPatchTool`：多文件 `ops[]`（create/update/delete/rename）、`dry_run` 预演、执行前逐文件字节备份、失败自动恢复备份（含删除新建文件）；`ApplyPatchError::Partial` 报出 failed_op 与 applied 清单；`spawn_blocking` 承载阻塞 IO。 |
| `src/run_command.rs` | ~830（逻辑 ~430 + 测试） | `RunCommandTool`：argv 优先 / `command` 经平台 shell；cwd 相对解析；timeout/输出/资源 clamp；手工构造 `SandboxPolicy` + `SandboxSelector::pick`；domain↔exec 取消桥；`metadata.sandbox` 上报后端选择。 |
| `src/scheduler.rs` | ~1080（逻辑 ~400 + 测试） | `ToolRegistry` / `ToolRegistryError`、`ToolScheduler` / `ToolSchedulerConfig`、审批接口 `ApprovalResolver` / `ApprovalOutcome` / `AutoApproveResolver`、闸门 `check_gate` 与约束注入、`NoopToolEventSink`。 |

无 `tests/` 目录与 fixture 文件；全部回归内联 `#[cfg(test)]`（edit_file / apply_patch 各含 proptest）。

## 3. 对外 API 面

### 3.1 八个文件与命令工具

统一形状：实现 `AgentTool`（`descriptor()` + `execute(request, context, sink, cancel)`）；输入 JSON object；路径参数一律 workspace 相对（绝对/穿越/`.git`/symlink 逃逸由 policy 内核拒绝）；构造函数均 `new(Arc<WorkspaceService>)`。**全部 descriptor `requires_approval=false`**——是否询问完全由 scheduler 按 `ApprovalMode` + `ToolCapability` 裁决（§4.1），descriptor 该字段用于 computer / MCP 的显式审批叠加闸。

| 工具 | capability | untrusted 可用 | default_timeout / max_output | 输入 |
| --- | --- | --- | --- | --- |
| `read_file` | ReadOnly | 是 | 10s / 256 KiB | `path`；`offset`（1 起行号）、`limit`（默认 2000 行） |
| `list_directory` | ReadOnly | 是 | 10s / 128 KiB | `path`（`"."` = root，空串报错）；`limit`（默认 500）、`offset`（默认 0） |
| `find_files` | ReadOnly | 是 | 10s / 256 KiB | `pattern`（逗号分隔 glob）；`file_type`（file 默认/dir/any）、`max_depth`、`max_results`（默认 200） |
| `search_text` | ReadOnly | 是 | 15s / 256 KiB | `pattern`；`is_regex`（默认 false）、`glob`、`context_lines`（默认 2）、`max_results`（默认 100）、`case_sensitive`（默认 true） |
| `write_file` | WorkspaceWrite | 否 | 10s / 16 KiB | `path`、`content` |
| `edit_file` | WorkspaceWrite | 否 | 10s / 32 KiB | `path` + （`old_string`/`new_string`）或 `edits[]`；`allow_fuzzy`（默认 false） |
| `apply_patch` | WorkspaceWrite | 否 | 15s / 64 KiB | `ops[]`（`op`=create/update/delete/rename + `path`/`content`/`to`）、`dry_run` |
| `run_command` | Process | 否 | descriptor 30s（scheduler 层；与 exec 层 timeout_ms 语义不同，见 §4.2） | 见下方专列 |

各工具输出与行为要点：

- `read_file`：`{行号:>6}\t行文本` 视图；metadata 报 encoding/offset/limit/total_lines/truncated；读上限 4 MiB（超出截断续报）；NUL/控制字节占比判定二进制，只报类型与大小不吐内容。
- `list_directory`：目录优先字典序，行格式 `{size:>8}  {kind}  {name}[ -> target]`，kind ∈ `file/dir/symlink/broken_symlink`；metadata.entries 结构化（name/kind/size/mtime_ms/is_symlink/symlink_target）；symlink 目标相对化、越 root 时省略（不泄漏宿主路径）；dangling symlink 不致败。
- `find_files`：相对路径列表（字典序稳定，不按 mtime）；尊重 .gitignore、跳过隐藏文件；每 64 项检查取消；`.git` 与逃逸 symlink 被 `resolve_write_rel` 复核静默跳过。
- `search_text`：匹配块（`path:line:` + 前后 context）；搜隐藏文件但尊重 .gitignore；非法 regex → InvalidInput；逐文件有界读取（非 UTF-8 文件跳过；最多读取上限 + 1 字节，以识别读取期间增长的文件）；R-14：超过 4 MiB 的文件不读取直接跳过并计数（`skipped_oversize`），有任何跳过时结果标 `truncated`。
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

### 3.2 Computer use

`ComputerTool::default()` 使用进程共享 `Computer::isolated()`；`new(Arc<Computer>)` 用于注入 backend。工具名 `computer`，`ExternalPlugin` / `ClientFunction` / `Local`，能力标签 `ComputerUse`，禁止 untrusted 与 ReadOnly，`requires_approval=true`（包括截图与权限查询），禁并发。Host 明确批准本轮时仍沿用既有本轮授权。参数与错误见 [computer-use](computer-use.md)，JSON 严格拒绝未知字段，输入上限 16 KiB。工具 schema 按 action 声明必填字段：所有输入必须有新截图的 observation_id，click 还必须带 x / y / button / clicks；解析失败返回参数要求，便于模型纠正，且不触达 backend。调用 `spawn_blocking`；future 取消/超时通过局部 token 停止工作，锁直到底层释放输入才归还。成功截图返回 Text 元数据及 `ImageSource::Base64` JPEG，整个输出预算 768 KiB；不写模型指定路径。每次输入返回投递事实，效果需新截图复验。

### 3.3 公共层（common）

- `BuiltinToolError` 变体：`MissingField(&'static str)` / `InvalidField{field, detail}` / `Path(WorkspacePathError)` / `PolicyPath(PathSafetyError)` / `Io(std::io::Error)` / `Workspace(WorkspaceError)` / `Process(String)` / `Other(String)`。
- 映射规则（`From<BuiltinToolError> for ToolError`）：
  - MissingField / InvalidField → `InvalidInput`。
  - `PathSafetyError::Empty`、`WorkspacePathError::Empty` → `InvalidInput`；`NoRoot` → `NotFound`。
  - **其余一切路径安全违规（绝对/穿越/`.git`/symlink 逃逸/NonRegular）→ `PermissionDenied`**。
  - Io 的 NotFound → `NotFound`，其余 Io → `ExecutionFailed`（retryable）；Workspace(NotFound) → `NotFound`。
- `workspace_roots(&WorkspaceService, &WorkspaceId)`：未知 workspace → `WorkspaceError::NotFound`。
- 取参语义：`require_str` 缺失或类型不符按 `MissingField` 报错；`opt_str`/`opt_u64`/`opt_bool` 可选字段缺省或显式 `null` 视为未提供，字段存在但类型不符（如字符串传给整数位）→ `InvalidField`（映射 `InvalidInput`），模型得到纠错信号而非静默默认值。
- `resolve_write_rel(roots, rel)`：全部八工具（含只读）解析路径的统一入口。
- `atomic_write(path, bytes)`：同目录临时文件 + rename；临时文件以 `create_new` 独占创建（同名路径已存在——含预置 symlink——即换名重试，有界 16 次），失败只清理本次创建的文件；目标已存在时保留其 permissions。

### 3.4 调度层

- `ToolRegistry`：`new` / `register(Arc<dyn AgentTool>)` / `extend` / `get` / `descriptor` / `descriptors` / `len` / `is_empty`。仅接受 `ToolKind::ClientFunction` 且 descriptor 合法（`ToolRegistryError::InvalidDescriptor` / `UnsupportedKind`）；同名注册为覆盖语义（MCP 重连刷新用）。
- `ToolSchedulerConfig { max_concurrent: 8, approval_mode: ApprovalMode::ReadOnly, workspace_trusted: false }`——**默认即最保守档**。
- `ToolScheduler::new(registry, config)` / `tool_count()` / `approval_mode()` / `workspace_trusted()` / `with_approval_snapshot(mode, trusted)`（克隆工具表到新 scheduler，旧实例不变，供宿主 Arc-swap） / `execute_named(name, request, context, cancel, approval: Option<&dyn ApprovalResolver>, sink) -> Result<ToolResult, ToolError>`。
- `ApprovalResolver`（async trait）：`resolve(&[ToolRequest]) -> Vec<ApprovalOutcome>`（`ApprovalOutcome` 为无字段枚举 `Approved` / `Denied`，拒绝文案由闸门统一生成）；`can_resolve_policy_prompt() -> bool`（默认 true；`AutoApproveResolver` 覆写为 **false**——它无法批准任何 `requires_approval` 工具或 policy `AskUser`，只能放行 S2 钩子的例行确认）。
- 错误面：未知工具 → `ToolError{kind: NotFound}`；policy `Deny` 与审批拒绝**不是 Err**，而是 `Ok(ToolResult{success: false, error: Authorization})`（对模型可见的失败结果，Agent loop 可继续）；超时 → `kind: Timeout`。
- 内部 `GateOutcome`（私有）只是 `check_gate` 的中间形态，公开面统一为 `ToolResult` / `ToolError`。
- `NoopToolEventSink`：丢事件 sink，测试与最小宿主用。
- 装配约定：宿主构造 `Arc<WorkspaceService>` → 八工具 `new` → `ToolRegistry::register` → `ToolScheduler::new`；MCP 工具经 `register_server_tools` 进同一 registry，两类工具走同一 `execute_named` 闸门，无旁路。

MCP 的独立 API、连接与认证见 [mcp](mcp.md)，注册适配器仍使用本包唯一工具表。

## 4. 核心行为与数据流

### 4.1 `execute_named` 一次调度

1. 查 `ToolRegistry`，未知名 → `NotFound` 错误。
2. `check_gate`：组装 `PolicyInput{capability=descriptor.capability, input=request.input, trusted=config.workspace_trusted, allowed_in_untrusted_workspace, approval_mode}` → `PolicyEngine::decide`。
3. 裁决处置：
   - `Deny{reason}` → 返回 `Ok(失败 ToolResult(Authorization))`。
   - `AskUser{prompt}` → 仅当有 resolver 且 `can_resolve_policy_prompt()==true` 才转交（approved 放行 / 拒绝 → 失败结果）；否则 fail-closed 拒绝。
   - `AllowWithConstraints` → 把 `timeout_ms`/`max_output_bytes` 注入 `request.input`（与已有值取更严者）后放行。
   - `Allow` → 继续。
4. 叠加闸：`descriptor.requires_approval=true`（computer、MCP 写工具）且 policy 非 Deny 时升级为 `AskUser`，必须由 `can_resolve_policy_prompt()==true` 的 resolver 放行（`AutoApproveResolver` 在此闸无效、一律拒绝；无 resolver 同样拒绝）。policy 直接放行（Allow / AllowWithConstraints）的工具，只要调用方传入 resolver，S2 钩子会再确认一次（AutoApprove 恒过、DenyAll 全拒；check_gate 已问过用户则跳过）。
5. 获全局 `Semaphore` 许可（`max_concurrent`）——槽位等待与取消 `select`，排队期间取消立即返回 `Cancelled` 且不调用 executor（R-09）→ descriptor 有 `default_timeout_ms` 则按 deadline 包裹 `tool.execute(...)`；超时 → 先触发派生执行令牌取消、**等待工具协作收口后**再回执 `Timeout`（R-10：响应返回后不会再启动新写操作，阻塞闭包的最终结果已被等待而非丢弃失控）。工具收到的令牌是 scheduler 派生令牌：调用方取消经桥接任务原样传播，scheduler 超时单独触发，互不误伤。

### 4.2 `run_command` 全流程

1. scheduler 闸门中 policy 已做 shell 风险分类与灾难地板判定（[policy.md](policy.md) §4.3）；`NeverAsk` 下注入的执行约束与显式输入取更严者。
2. 工具内解析输入：非空 `argv` 优先；否则 `command` 包平台 shell（Unix `sh -c`，Windows `cmd /d /s /c`）；`cwd` 相对解析进 workspace root（默认第一 root）；clamp timeout / 输出 / 资源四项。
3. 手工构造 `SandboxPolicy`：read/write roots = workspace roots、deny = `default_secret_paths()`、`NetworkMode::Enforce`、**`allow_spawn=true`**（区别于 exec 的 `untrusted_default`——命令执行本身已过 policy 闸门）、`max_procs` = clamp 值、`env_clear=true`、allowlist = `default_env_allowlist()` ∪ 显式 `env` 键、denylist = `process_env_denylist()`（`*TOKEN*`/`*KEY*`/`*SECRET*`/`*PASS*`/`*PAT`/`*AUTH*`/`*COOKIE*`/`*CREDENTIAL*` 与 `BASH_ENV`/`ENV`/`BASH_FUNC_*`/`NODE_OPTIONS`/`PYTHONSTARTUP`/`PERL5OPT`/`LD_PRELOAD`/`DYLD_*`）——通配命中优先于 allowlist，显式 `env{}` 同样被剥除。
4. `SandboxSelector::new().pick()` 选后端 → `spawn_stream`（exec 管线：软限制 → 平台翻译 → 进程树守卫，见 [exec.md](exec.md) §4.3）。
5. 取消桥：spawn 一个任务监听 domain `CancellationToken`，触发即 exec token `.cancel()`。
6. 收集事件流：stdout/stderr 分别累积（exec 层已按合计预算截断）；组装文本（stdout + `[stderr]` 段 + 非零 `[exit N]`）与 `metadata.sandbox = {backend, isolation, fallback, note, attempted[], limits}`（内联 golden 钉形状）——回退可观测地呈现给上层与用户。
7. 退出/超时/取消路径由 exec 保证 5s 内整树回收；`timed_out` → 失败结果（Timeout error context），非零 exit → `success: false`（ExecutionFailed）。

### 4.3 文件写路径（write / edit / apply_patch 共通）

三个写工具的 `execute` 均以 `tokio::task::spawn_blocking` 承载全部同步文件 IO（与 list_directory / find / search 同形；`JoinError` → `Internal`）。路径与落盘步骤：

1. `workspace_roots` 取 roots → `resolve_write_rel(roots, path)`（拒绝绝对/穿越/`.git`/逃逸/非常规文件，错误映射见 §3.2）。
2. 内存预演全部变更（edit 的段替换、patch 的 op 计划）；任何一段失败整体失败，不触盘。
3. 落盘：`atomic_write`（写类）；apply_patch 执行前对受影响文件做字节备份，op 失败即恢复备份（改写还原、新建删除），并以 `Partial` 报出 failed_op 与 applied 清单（proptest 断言恢复字节精确）。

### 4.5 取消与超时的传播路径

1. 取消源头是 domain `CancellationToken`（engine/宿主持有）；scheduler 派生执行令牌传入工具——调用方取消经桥接传播，scheduler 超时也触发同一令牌（R-10）。
2. 只读四工具：走 `spawn_blocking` 的（find/search）每 64 个候选检查一次并在进入阻塞前检查；read_file 在读文件前后检查。命中即返回 `ToolError::cancelled`（kind=Cancelled）。
2a. 写三工具（write/edit/apply_patch）：`spawn_blocking` 闭包携带令牌——write/edit 在 `atomic_write` 提交边界检查一次；apply_patch 在每个操作边界检查，命中则回滚已应用操作（保持「全成或全滚」）后返回 `Cancelled`（R-10）。
3. `run_command`：桥接任务把 domain 取消翻译为 exec token cancel → exec 监督循环 kill 整树（[exec.md](exec.md) §4.1）。
4. 超时双层：descriptor `default_timeout_ms` 由 scheduler 强制——超时即取消派生令牌并等待工具收口，再报 `Timeout` 错误（R-10）；`run_command` 的 `timeout_ms` 由 exec 层强制（`timed_out` 标记）。两层语义不同：前者报 `Timeout` 错误，后者是带上下文的失败结果。

## 5. 契约与不变量

- **路径红线**：所有文件类工具输入 = `workspace_id + relative_path`，唯一解析入口 `resolve_workspace_path`；`.git` 与 symlink 逃逸永拒（PermissionDenied），错误信息不回显宿主绝对路径。工具层无绕行通道。
- **MCP 边界**：SDK 隔离、Secret 域、stdio 沙箱与宿主信任钳制由 [mcp](mcp.md) 承担；本包仍负责全部工具的共同审批与调度闸门。
- **调度默认最保守**：`ToolSchedulerConfig::default` = `ReadOnly` 档 + untrusted + 并发 8；policy `AskUser` 无有权 resolver 时 fail-closed 拒绝；`AutoApproveResolver` 不能回答 policy prompt。
- **原子性**：write/edit 单文件原子写；edit 多段与 apply_patch 多文件「全成或全滚」，回滚字节精确（proptest 钉死）。
- **`metadata.sandbox` 形状**：run_command 必带后端选择证据（backend/isolation/fallback/note/attempted/limits），`metadata_sandbox_shape_and_limits_golden` 钉死——「fail-closed 可观测回退」在工具层的落点。
- **run_command 环境卫生**：`env_clear=true` + denylist 覆盖 Secret 通配与 shell/动态语言启动注入变量（优先于 allowlist），宿主与显式注入的命中项都被剥除。
- 无独立 golden 文件；上述契约全部由内联测试承载。

## 6. 依赖关系

- **workspace 内**：`pawork-domain`（AgentTool/ToolResult/CancellationToken 等 canonical 类型）、`pawork-policy`（路径内核 + PolicyEngine）、`pawork-exec`（Process/Sandbox Runtime）、`pawork-workspace`（WorkspaceService、ResolvedConfig）、`pawork-computer-use`（隔离虚拟桌面操作）。
- **外部**：`tokio`、`async-trait`、`serde/serde_json`、`thiserror`、`tracing`、`ignore`、`globset`、`regex`、`chardetng`、`encoding_rs`、`base64`（截图编码）。dev：`tempfile`、`proptest`。无 cargo feature。
- **被依赖**：`pawork-mcp`（注册外部工具）、`pawork-app`（注册与调度装配；engine 经 LoopContext 回调消费，不依赖本包）。

## 7. 测试与验证资产

2026-09-20 精简：HTTP 配置拒绝只保留 codec 的完整输入矩阵；auto-approve 标志并入实际写工具被拒绝且零调用的回归；环境白名单副本由 exec 权威测试和 run_command 的真实子进程环境检查承接。MCP 回归已随迁至 mcp 包。

`computer.rs` 三项回归：显式批准后的截图结果与序列化、缺 click 字段在触达 backend 前返回可纠正参数错误、未信任/只读/自动批准均不触碰虚拟桌面后端。输入与观察安全由 [computer-use](computer-use.md) 负责。

默认验证命令：`cargo test -p pawork-tools --offline --lib --tests`（无 `tests/` 目录，用例全部在 `--lib`）。

| 文件 | 覆盖点 |
| --- | --- |
| `common.rs` | `opt_*` 取参语义（合法值 / 缺省与显式 `null` → `None`、类型误传 → `InvalidField`→`InvalidInput`）；R-01 预置同名 symlink 不被跟随、外部哨兵不变且写入仍成功；错误映射分流与 `atomic_write` 覆盖/权限行为由各工具用例承载。 |
| `read_file.rs` | 行号/offset/limit、二进制拒吐、绝对与穿越路径拒绝、missing → NotFound、大文件读上限、symlink 逃逸与 `.git` 拒绝。 |
| `list_directory.rs` | 类型/symlink 列举、分页与 total、dangling symlink 容忍、逃逸 symlink 目标省略（不回显宿主路径）、非目录报错。 |
| `find_files.rs` | glob 匹配与字典序、max_results 截断、dir 过滤、遍历中取消、跳过逃逸 symlink 与 `.git`。 |
| `search_text.rs` | 固定串 + context、regex、glob 过滤、非法 regex → InvalidInput、取消、跳过逃逸与 `.git`、R-14 超限文件跳过并报告（`oversize_file_is_skipped_and_reported`）。 |
| `write_file.rs` | 原子写/建父目录/覆盖保留 mode、路径拒绝。 |
| `edit_file.rs` | 精确单段、不唯一 Conflict、多段原子、预演失败不落盘、fuzzy 归一化与终止换行保留、fuzzy 唯一性计数、proptest（fuzzy 与精确替换一致性）。 |
| `apply_patch.rs` | 多文件 create、dry_run 不落盘、delete+rename、部分失败恢复（create/update/delete 各形态）、proptest 字节精确回滚、op 路径穿越拒绝、R-10 取消令牌下零写入（`cancelled_token_starts_no_ops`）。 |
| `run_command.rs` | 输出与 exit_code、非零失败、超时、流式先于退出、descriptor 无网络旁路参数、clamp 上限、**`metadata_sandbox_shape_and_limits_golden`**、macOS Seatbelt 必须上报 `sandbox_exec` / `hard_writes_and_network` / `fallback=false`（探测失败即失败）、显式 Secret env 被剥除。环境白名单由 exec 的权威清单与剥除断言承接。 |
| `scheduler.rs` | 只读并发、全局并发上限、未知工具、上下文透传、取消（执行前/执行中）、超时映射、审批拒绝不执行、auto-approve 不能绕过 AskForWrites（并入写工具零调用回归）、registry kind/描述符校验、untrusted 写拒绝（NeverAsk 也拒）、AskForWrites 不可被 AutoApprove 绕过、ReadOnly 档拒写、`process_never_ask_trusted_injects_execution_constraints`、约束与显式输入取更严、R-09 排队取消（`queued_call_cancelled_while_waiting_for_slot`）、R-10 超时协作收口与操作边界停写（`timeout_waits_for_cooperative_drain_before_responding`、`cancel_between_ops_stops_later_ops`）。 |

## 8. 注意事项与已知限制

- 默认配置（`ReadOnly` 档 + untrusted）下只有四个只读工具可用；一切副作用工具被 policy 直接 Deny——宿主必须显式提升 `ApprovalMode` 并提供 `ApprovalResolver` 才能写盘/执行命令。
- `run_command` 的沙箱策略固定派生（Enforce 网络、deny secret 路径、env_clear），不随 workspace 信任度放宽；放宽属 R7 策略分层。真实隔离强度取决于平台后端（macOS Seatbelt 最强；无硬后端时 NativeRestricted 挡不住命令内部越权读，见 [exec.md](exec.md) §8）。
- `find_files` / `search_text` 尊重 `.gitignore`（被 ignore 的文件搜不到，有意行为）；`find_files` 还跳过隐藏文件，`search_text` 不跳过；`search_text` 单文件读取上限 4 MiB（R-14），超限文件跳过并在 metadata 计数报告，结果标记不完整。
- `edit_file` fuzzy 是行对齐 whitespace 归一化匹配，不做语义/缩进感知；替换文本按字面写入。
- MCP 的连接、自动启动与 HTTP 限制见 [mcp](mcp.md)；本包只承接其工具适配器。
- `StdioTransportConfig` / `HttpTransportConfig` 位于私有 `mod transport`（类型 pub 但包外不可命名）——以其为参数的公开函数（如 `OAuthHttpConnector::new`）实际只能由 crate 内部装配，这是刻意的封装边界而非疏漏。
- `list_directory` 的 `path` 不接受空串（`PathSafetyError::Empty` → InvalidInput），列 root 用 `"."`；`read_file` 对超过 4 MiB 的文件只读前 4 MiB 并标记 truncated，不报错。
- 相关文档：[policy.md](policy.md)（裁决与路径内核）、[exec.md](exec.md)（执行原语）、[../flows.md](../flows.md)（跨包链路）、[../../architecture.md](../../architecture.md)、[../../design.md](../../design.md)、[../README.md](../README.md)、[AGENTS.md](../../../AGENTS.md)。
