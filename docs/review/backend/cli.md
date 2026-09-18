# pawork-cli Review

> CLI 命令面与外部通道：21 个顶层子命令 + ACP 四件套，把 `AppCore` 装配成终端 / headless / GUI serve / ACP stdio。27 个 src `.rs` + 3 个 tests `.rs`（含 `tests/common/mod.rs`），src 9,817 行 + tests 2,503 行 = 12,320 行；最大模块 `channels/acp/host.rs`（2,199）。另有 fixtures 14 个 JSON（v1 ×13 + v2 ×1）。只被 `apps/pawork` 消费。

## 1. 职责与边界

做什么：解析 clap、按命令决定是否加载 Core、选审批宿主、把 stdout 留给协议/JSON（日志与提示走 stderr）。六运行模式：chat / run / headless / acp / gui serve / service；其余是会话、凭证、运维与编排入口。

不做什么：不定义 wire 契约（在 `pawork-protocol`）；不实现 Provider / 工具 / SQLite；不嵌入 Core 到 Desktop；自身不安装 tracing subscriber（日志装配在宿主二进制）。关闭 GUI/ACP **不取消**已进入 Core 的 Run。`channels/` 注释写「本 crate 不依赖 pawork-app」——那是 ACP 通道模块的目标边界；crate 级仍依赖 app，Core 执行经 `CliAcpCommandHost` 注入。

装配顺序（`run()` → `run_inner()`，lib.rs:372 起）：

1. Service / Status / Doctor / Watch / Shutdown **不加载 Core**（lib.rs:390-397 pre-core 分支）。
2. `gui serve` 在加载 Core 前只解析一次 data directory，同一 `PathBuf` 同时写入 `AppLoadOptions.data_dir` 与 `run_gui`（lib.rs:400-403）。
3. 其余 `AppCore::load` 或 `load_for_catalog`（tolerant：Models/Sessions/Auth/Diff/Rollback/Mcp/Import/Headless/Acp/Gui/Usage/Tasks/Plan/Agents，lib.rs:418-437）。
4. `--json` 或非 TTY → `DenyAllApprovals`；Gui/Headless/Acp 强制 `GuiApprovalHost`；默认审批档 `read-only`。
5. 普通命令结果返回前 `core.shutdown()`；gui / headless / acp / json 快路径各自收尾。

全局旗标：`--provider/-p`、`--model/-m`、`--instance`（默认 `default`）、`--json`、`--approval-mode`、`--trust-workspaces`（只覆盖本次进程，不写配置）。

## 2. 依赖关系

| 方向 | crate | 用途 |
| --- | --- | --- |
| 依赖 | pawork-app | `AppCore` / `AppLoadOptions` / 审批宿主 / `GuiHostAdapter` / `gui_server` / 领域门面 |
| 依赖 | pawork-client | `status`/`watch`/`doctor`/`shutdown` 连本机 GUI serve |
| 依赖 | pawork-domain | ID、事件、审批决策 |
| 依赖 | pawork-engine | chat 文本路径的 `AgentEventSink` / compact |
| 依赖 | pawork-protocol（feature `adapter`） | 命令信封、ACP ClientAdapter、headless 帧、GUI 能力表 |
| 依赖 | pawork-storage（`session`，关 default） | ACP `SqliteClientSessionRegistryStore` |
| 依赖 | pawork-transport | `gui serve` 本机 UDS / Named Pipe |
| 被依赖 | apps/pawork | 唯一正式宿主二进制 |

| 外部 crate | 用途 |
| --- | --- |
| clap | 21 子命令 |
| tokio | rt-multi-thread、signal、stdio、ACP actor |
| async-trait | ACP CommandHost / Adapter |
| serde / serde_json | JSON 输出与 JSON-RPC |
| thiserror / tracing | `CliError`、诊断 |
| tempfile（dev） | ACP / sessions 夹具 |

无 crate feature（`[[test]]` target `acp_fixtures` / `acp_floor` 无 required-features，默认死表内）。Desktop **不**依赖本包。

## 3. 文件清单

| 相对路径 | 行数 | 职责 |
| --- | ---: | --- |
| src/lib.rs | 1092 | `Cli` / `Command` 21 变体、`run`/`run_inner` 装配、clap 矩阵测试 |
| src/chat.rs | 661 | REPL / 单次 / `--json` HeadlessResponse；`cli-json` 身份戳 |
| src/sessions.rs | 695 | list/show/export/import/fork；jsonl 完整首行嗅探 |
| src/render.rs | 645 | `TextSink`：assistant→stdout，thinking/工具→stderr；SEARCH-1 引用活动行 |
| src/headless.rs | 512 | `--json-stdio` Handler + capability gate + compat import/history |
| src/service.rs | 380 | install/start/stop；默认 dry-run；TeardownStep 回收 |
| src/ops.rs | 372 | status/watch/shutdown/doctor；socket/token/pid 命名（300ms 探测） |
| src/acp.rs | 278 | `acp serve` stdio 循环 + 事件泵 + EOF 后 30s drain |
| src/channels/acp/host.rs | 2199 | `AcpHost` actor（独立 OS 线程 + current_thread runtime） |
| src/channels/acp/adapter.rs | 658 | ACP ↔ canonical ClientAdapter |
| src/channels/acp/wire.rs | 651 | JSON-RPC 类型，`PROTOCOL_VERSION=1` |
| src/channels/acp/map.rs | 286 | crate-private：错误码 / StopReason / 权限选项 |
| src/channels/acp/command_host.rs | 28 | `AcpCommandHost` 窄 port |
| src/channels/acp/mod.rs | 35 | ACP 模块导出 + `now_timestamp` |
| src/channels/mod.rs | 13 | 外部通道入口（本波只激活 acp） |
| src/vcs.rs | 194 | diff / rollback（Blob 还原） |
| src/gui.rs | 151 | `gui serve` 单实例绑定；能力由 registry 派生 |
| src/approval.rs | 124 | TTY `InteractiveApprovals` |
| src/auth.rs | 123 | list/set-key/login/logout |
| src/adapter.rs | 258 | CLI→`GuiHostAdapter` 胶水、command_id 进程命名空间、`CliAcpCommandHost` |
| src/plan.rs | 97 | Plan 审批子命令 |
| src/tasks.rs | 82 | 后台任务 |
| src/import.rs | 80 | 本机工具配置导入 |
| src/error.rs | 73 | `format_provider_error` |
| src/usage.rs | 48 | 用量与 LocalLedger 报表 |
| src/mcp.rs | 44 | MCP list/test |
| src/agents.rs | 34 | S11 多 Agent demo |
| tests/common/mod.rs | 650 | ACP TestHarness / MockScript |
| tests/fixtures.rs | 444 | ACP golden 16 |
| tests/floor.rs | 1409 | ACP 地板 27 |

fixtures：`fixtures/v1/` 13 个 JSON + `fixtures/v2/` 1 个（initialize-request-v2，仅断言实验 v2 显式拒绝）。

## 4. 类型与方法功能列表

### 4.1 lib.rs — 入口与 21 子命令

| 名称 | 种类 | 功能 / 语义 |
| --- | --- | --- |
| `CliError` | enum | `App` / `Turn` / `Usage` / `Cancelled` / `Io`。失败走 stderr + `ExitCode::FAILURE` |
| `Cli` | struct | 全局旗标 + `command: Command` |
| `run` | async fn | 唯一对外入口，`apps/pawork` 调用 |
| `format_provider_error` | re-export | 见 error.rs |
| `channels` | pub mod | ACP 对外出口（`AcpHost` / Adapter / CommandHost / `PROTOCOL_VERSION` 等） |

模块可见性：仅 `pub mod channels` 与 `pub use error::format_provider_error`；其余模块私有（lib.rs:7-27）。

`Command` 21 变体及入口：

| 变体 | 入口 | 语义 |
| --- | --- | --- |
| `Chat { prompt, resume, branch }` | `chat::run_chat` / `run_json` | REPL 或单次；`--json` 需 `--prompt` |
| `Sessions { List/Show/Export/Import/Fork }` | `sessions::run_sessions` | 落盘会话；Import 可 `--from claude|codex`（与 path/format/source 互斥） |
| `Run { prompt }` | `chat::run_once` / `run_json` | 非交互单次 |
| `Models` | `run_models` | 目录排序后列出（六首发通道顺序 + config 自定义） |
| `Auth { List/SetKey/Login/Logout }` | `auth::run_auth` | 凭证；key 走 stdin；login 5min PKCE/device；logout 不清 env fallback |
| `Gui { Serve { socket } }` | `gui::run_gui` | 本机 GUI 协议服务 |
| `Diff { session, page }` | `vcs::run_diff` | 会话累计 hunk，每页 10 文件 |
| `Rollback { checkpoint, session, yes }` | `vcs::run_rollback` | **Blob 还原**，不是 `git reset --hard`。json/非 TTY 必须显式 checkpoint |
| `Mcp { List/Test }` | `mcp::run_mcp` | 已配置 server |
| `Import { tool, yes, dry_run }` | `import::run_import` | 只读源：claude/codex/grok/cursor/pi |
| `Headless { json_stdio }` | `headless::run_headless` | 必须 `--json-stdio` |
| `Acp { Serve }` | `acp::run_acp_serve` | ACP JSON-RPC stdio |
| `Service { Install/Start/Stop }` | `service::run_service` | 默认 dry-run，`--apply` 才改系统；模板硬编码 `gui serve` |
| `Status` | `ops::run_status` | 不加载 Core |
| `Watch` | `ops::run_watch` | 订阅事件直到 Ctrl-C |
| `Shutdown` | `ops::run_shutdown` | SIGTERM 本机 serve |
| `Doctor` | `ops::run_doctor` | 装配自检 + 握手探测 |
| `Usage { session }` | `usage::run_usage` | 用量 / LocalLedger |
| `Tasks { List/Status/Cancel/Register }` | `tasks::run_tasks` | 后台任务 |
| `Plan { Show/Create/Replace/Submit/Approve/Reject }` | `plan::run_plan` | Plan 闸门 |
| `Agents { Demo { cancel, budget_tokens } }` | `agents::run_agents_demo` | 多 Agent 演示 |

子枚举：`SessionsCommand` / `AuthCommand` / `GuiCommand` / `McpCommand` / `AcpCommand` / `ServiceCommand` / `TasksCommand` / `PlanCommand` / `AgentsCommand`。

`approval_host`：`--json` 或 stdin 非 TTY → `DenyAllApprovals`；否则 TTY `InteractiveApprovals`；Gui/Headless/Acp 覆盖为 `GuiApprovalHost`（lib.rs:619）。

### 4.2 chat.rs / render.rs / approval.rs

| 名称 | 种类 | 功能 / 语义 |
| --- | --- | --- |
| `run_chat` | async fn | 有 `--prompt` 单次；非 TTY 读 stdin 一行；否则 REPL |
| `run_once` | async fn | `pawork run` |
| `run_json` | async fn | stdout 只打 `HeadlessResponse` JSONL（无 hello）；`cli-json` 身份（chat.rs:170,204,220） |
| `open_or_create` / `switch_branch_if_requested` | async fn | 会话装配；`--branch` 进 REPL 前切分支 |
| `drive_turn` | async fn | Ctrl-C → `CancelHandle.cancel(User)` 后等 turn 收尾 |
| `print_usage_line` | async fn | 轮末 tokens/费用行；registry 无定价不编造 |
| `handle_model_command` / `handle_provider_command` / `handle_plan_command` | async fn | REPL 斜杠命令 |
| `compact_now` | async fn | `/compact`：与自动压缩同一 engine 函数 |
| `TextSink` | struct | `AgentEventSink`：`AssistantTextDelta`→stdout，thinking/工具/失败→stderr。`--json` 不经本模块 |
| ServerTool 活动行（render.rs:226-300） | 逻辑 | SEARCH-1：Started 登记、CitationAdded 计数（晚于 Completed 仍显示）、Completed `⚙ name · N citations`、Failed `✗`；中间进度帧不刷屏 |
| `InteractiveApprovals` | struct | stderr 提示，stdin `y` 一次 / `a` 本 run / `n` 拒绝；取消 → `Cancelled`（biased select） |

REPL 斜杠：`/exit` `/quit`；`/compact`；`/model`；`/provider`；`/plan`；`@file` 工作区引用（`expand_at_refs`）。空闲连按两次 Ctrl-C 退出。

### 4.3 sessions.rs / import.rs / vcs.rs

`sniff_jsonl_session`（sessions.rs:496）：**读完首个完整非空行**再 JSON 解析，禁止 8KiB 截断（真实 session_meta 可超 8KiB）。签名：`timestamp+type+payload` → Codex 信封；`sessionId+type` 且无 `payload` → Claude 本地行；否则 `None`。`detect_session_format` 对 `.jsonl` 在 sniff 为 None 时默认 Pi。测试钉死超 8K 首行不误判。

rollback 走 checkpoint Blob，json/非 TTY 无 id 则 Usage 错误（不能交互询问）。

### 4.4 headless.rs

| 名称 | 种类 | 功能 / 语义 |
| --- | --- | --- |
| `HOST_CAPABILITIES` | const | `Sessions` / `Runs` / `Streaming` / `CompatImport` / `CompatHistory`（headless.rs:27-32）。测试钉死快照，且 registry headless 列 ⊆ 本表 |
| `run_headless` | async fn | 无 `--json-stdio` → Usage 错误。Lagged 停流 fail-closed（`stream_halted`，headless.rs:219-247） |
| `HeadlessHandler` | struct | hello 用 `negotiate_api_version_with`（headless.rs:96）；授予 = 请求 ∩ HOST_CAPABILITIES；`command_entry().headless == None` → `UnsupportedCapability`（headless.rs:131，含 `WorkspaceAdd` 定向回归） |
| `SessionClientContextReplace` gate | 逻辑 | 目标 session 须是本连接 opened（`owned_sessions`），否则 `CompatRejected` |
| compat 直连 `SessionStore` | 逻辑 | CompatImport / CompatHistory 支持 dry-run 与游标分页 |
| 协议循环本体 | 边界 | 帧解析 / hello 时序由 `pawork_protocol::headless::stdio::run_loop` 承载 |
| 身份戳 | 常量 | `HEADLESS_NAME = "headless"`（headless.rs:25） |

### 4.5 gui.rs / ops.rs / service.rs

| 名称 | 种类 | 功能 / 语义 |
| --- | --- | --- |
| `run_gui` | async fn | bind 前探测拒绝双实例（gui.rs:48）；父目录 `0o700`（gui.rs:42）；capabilities = `gui_supported_capabilities()`（gui.rs:74，不是本地列表）；`with_host_data_dir` 注入同一 data_dir（gui.rs:76）；token 缺失则 generate，空 token 失败无匿名回退；`core.set_approval_host` 只接线 GUI 审批回调（gui.rs:32，OPT-1/ADR-053） |
| `gui_socket_path` | fn | default：`pawork-gui.sock`；命名 instance：`pawork-gui-<i>.sock`（ops.rs:30） |
| `gui_token_path` | fn | `gui.token` / `gui-<i>.token`（ops.rs:39） |
| `service_name` | fn | `pawork` / `pawork.<i>`（ops.rs:22） |
| `gui_pid_path` | fn | `<data_dir>/<instance>/gui-serve.pid`（ops.rs:80） |
| `probe_socket` / `probe_handshake` | async fn | 300ms 探测（ops.rs:227）；doctor 握手探测报告 client id 与能力数 |
| `run_status` / `run_doctor` / `run_watch` / `run_shutdown` | async fn | 不加载 Core；watch/doctor 需 token 组装握手 proof |
| `TeardownStep` | enum | service.rs:203；三平台 stop 回收（macOS unload+删 plist、Linux stop+disable+删 unit、Windows sc stop+delete），最后一步必须落地 |
| `launchd_plist` / systemd unit / `sc create` | 模板 | 入口硬编码 `<exe> --instance <i> gui serve` |

关闭窗口/Ctrl-C 只关监听，不取消 in-Core Run。

### 4.6 adapter.rs / acp.rs — CLI 胶水与 ACP 装配

| 名称 | 种类 | 功能 / 语义 |
| --- | --- | --- |
| `adapter_from_locked` / `adapter_with_gui_approvals` | fn | 包 `GuiHostAdapter` |
| `stamp_automation` / `stamp_query` | fn | 盖 `CommandSource::Automation` + 指定名字（**无条件覆盖**既有 identity） |
| `command_namespace` | fn | 进程级命名空间：`{pid:x}-{nanos:x}`，`OnceLock` 单次初始化（adapter.rs:28-36）。command_ledger 以 (tenant, scope, command_id) 持久幂等，裸计数器会跨进程撞键重放旧响应（`pawork run` 挂死实证） |
| `command_envelope` | fn | id 形如 `cli-{name}-{namespace}-{n}`（n 为进程级 `AtomicU64`，adapter.rs:63-80）；与 client 的 `new_request_namespace` 同形态 |
| `wrap_response` | fn | 给 query/command 回包加 `request_id` 与时间戳 |
| `now_timestamp` | fn | Unix 毫秒 `Timestamp` |
| `CliAcpCommandHost` | struct | `AcpCommandHost`：dispatch/query/subscribe；dispatch/query 强制 `source=Automation`；合法 `acp:<client>` Automation 身份（含 `acp:pawork-acp`）原样保留，伪装 User 或非 ACP 身份回退为 `acp`（adapter.rs `retain_acp_automation_identity`），command_id / idempotency_key 不改写 |
| `run_acp_serve` | async fn | `SqliteClientSessionRegistryStore` + `AcpHost` + stdio 循环（acp.rs:24-41）。事件泵任务批处理 + flush；Lagged → `fail_closed_all_prompts`（acp.rs:60）；EOF 后最多 drain 30s（`ACP_DRAIN_TIMEOUT`），inflight join 超时 2s abort |

### 4.7 ACP 四件套（`channels/acp/`）

**1. `AcpHost`（host.rs）**

独立 OS 线程 `pawork-acp-actor` + `current_thread` runtime（host.rs:297，同步 drain 不能挂在调用方 ambient runtime 上）。信箱 API：`handle_request` / `handle_notification` / `handle_response` / `drain_and_pump` / `pump_events` / `drain_outbox_items` / `take_outbox` / `resolve_queued_prompts` / `fail_closed_all_prompts` / `release_drained_barriers` / `has_active_runs` / `pending_run` / `degraded_capabilities` / `is_initialized` / `subscribe` / `registry` / `connection_id`。

- 同 session 同时只占一个 prompt occupancy；占用窗口内 early cancel 记标志、绑定后重放。
- `handle_request` 对 `session/prompt` 经 `FlushBarrier` 等到 run 终态才返回。
- `fail_closed_all_prompts` 清账后对已绑定 run 补发 `RunCancel`、对 pending permission 补发 Deny（best-effort 补偿）。
- cwd 必须在已登记 workspace root 内（canonicalize 两侧），**禁止静默 `WorkspaceAdd`**。
- `protocolVersion=1` 整数；实验 v2 → `-32602`。未知方法 `-32601`，未知参数 `-32602`。
- `ACP_SUPPORTED_CAPABILITIES = []`（客户端能力全记入 degraded，使用点再拒）。
- `ACP_AGENT_NAME="pawork-acp"`（host.rs:60）；host 自构信封身份 `acp:{ACP_AGENT_NAME}`（host.rs:242）。
- `Diagnostic` 事件有意不发射 update；run 归属未知事件暂存 `held_events`。

**2. `AcpCommandHost`（command_host.rs）**

`dispatch` / `query` / `subscribe`。禁止依赖 `EventHub`；事件只经 subscribe 扇出。错误 `AcpHostError::Unavailable`。

**3. `AcpClientAdapter` + `AcpClientAdapterFactory`（adapter.rs）**

纯协议翻译：decode `session/new|prompt|cancel|permission`；admit 走 protocol registry acp 列（`session_create` / `run_start` / `run_cancel` / `tool_approve`，测试钉死）。`CwdResolver` / `SessionResolver` 由宿主注入；`AcpClientAdapterFactory::create_concrete` 得到 `NegotiatedAcpAdapter`。`decode_cancel` → `CancelTarget`；permission 响应 → `PermissionDecision`（嵌套 outcome 形态，扁平/未知字段拒绝）。能力白名单外 **显式降级** 而非拒握手；`mcpServers` 非空、`additionalDirectories`、image/audio/resource content block、`session/load` 显式拒绝；`resource_link` 映射为 `[name](uri)` 文本引用。adapter 自构信封身份 `acp:{client_info.name}`、command id `acp-{request_id}`。

**4. `wire.rs` + crate-private `map.rs`**

| 名称 | 种类 | 功能 / 语义 |
| --- | --- | --- |
| `PROTOCOL_VERSION` | const | `1` |
| JSON-RPC 错误码 | const | PARSE -32700 / INVALID_REQUEST -32600 / METHOD_NOT_FOUND -32601 / INVALID_PARAMS -32602 / INTERNAL -32603 / REQUEST_CANCELLED -32800 / AUTH_REQUIRED -32000 / RESOURCE_NOT_FOUND -32002 |
| `JsonRpcMessage` | enum | parse/to_value |
| `InitializeParams/Result` 等 | struct | ACP v1 方法形状；`ParamsExt::reject_unknown`（除 `_meta` 外未知字段 -32602） |
| `StopReason` | enum | map：`Completed`→`end_turn`；`Cancelled`/`Interrupted`→`cancelled`；`Failed`→internal |
| `PERMISSION_OPTION_ALLOW_ONCE` / `REJECT_ONCE` | const | 未知 option id 拒绝。客户端错误响应视为 Deny，`-32800` 视为 Cancel |
| map.rs 三张错误映射表 | 表 | `AdapterError` / `AdapterErrorFrame.code` / `ErrorContext.category` → JSON-RPC 码（NotFound→-32002、Auth→-32000、InvalidRequest→-32602、Cancelled→-32800、其余→-32603） |

`channels/mod.rs` re-export：`AcpHost` / `AcpHostError` / `AcpCommandHost` / `AcpClientAdapter` / `AcpClientAdapterFactory` / `CwdResolver` / `SessionResolver` / `JsonRpcMessage` / `PROTOCOL_VERSION`。`OutboxItem` / `PromptResolution` 经 `channels::acp`。`codex` / `claude` / `remote-control` 不迁入。

### 4.8 其余命令模块

| 文件 | 入口 | 语义 |
| --- | --- | --- |
| auth.rs | `run_auth` | List 掩码（ADR-056 双形态通道两类凭证各一行）；SetKey stdin；Login 5 分钟；Logout 只删 auth 文件 default |
| mcp.rs | `run_mcp` | List 已发现工具；Test ping/list_tools |
| usage.rs | `run_usage` | session 可选；provider / session / ledger / 配额窗口 |
| tasks.rs | `run_tasks` | list/status/cancel/register（cancel 回显级联 id） |
| plan.rs | `run_plan` | show/create/replace/submit/approve/reject；session 缺省 latest，create 无会话时新建 |
| agents.rs | `run_agents_demo` | 可选 cancel 与 token 预算 |
| error.rs | `format_provider_error` | 把 `ProviderError` 打成可读中文短句；认证错误不透传上游原文 |

## 5. 关键行为与契约

- stdout 只给协议/JSON；日志、审批、REPL 提示走 stderr（`--json` / `--json-stdio` / `acp serve` 三通道）。
- 默认审批 `read-only`；`--json` / 非 TTY fail-closed DenyAll；`on-failure` 旧档拼写兼容映射 NeverAsk。
- jsonl 嗅探读完整首行，禁止 8KiB 截断。
- headless 未映射命令 `UnsupportedCapability`，不得静默放行。
- GUI token 缺失/空失败，无匿名回退。socket 目录 0o700。GUI 能力集恒等于 registry `gui_supported_capabilities()` 派生，禁止本包字面量清单。
- GUI 数据目录单源：Core 加载、socket/pid/token、Accepted `host_data_dir` 消费 `run_inner` 同一解析结果。
- 关闭 GUI/ACP 不取消 in-Core Run。
- ACP：protocolVersion=1；Lagged fail-closed；cwd 不自动 WorkspaceAdd；同 session 同时至多一个 prompt；prompt 结果经 FlushBarrier 释放。
- rollback 不是 `git reset --hard`。
- service 默认 dry-run；stop 最后一步（删单元文件）必须落地。
- **通道身份戳现状**：`cli-json`（chat.rs:170,204,220）与 `headless`（headless.rs:25）按 spec 落地；ACP 通道 `CliAcpCommandHost` 强制 `source=Automation`，合法 `acp:<client>` Automation 身份（含 `acp:pawork-acp`）原样保留进 Core，伪装 User 或非 ACP 身份回退为 `acp`；`command_id` / `idempotency_key` 不改写。事件与审计侧可区分 cli-json / headless / acp 及 ACP 细粒度客户端来源。
- command id 防撞键：CLI 路径 `cli-{name}-{pid+纳秒命名空间}-{n}`（进程级 OnceLock + AtomicU64，ADR-061 波收口，含定向回归）；ACP 路径 `acp-{request_id}`。

## 6. 测试资产

| 文件 | 数量 | 验证点 |
| --- | ---: | --- |
| src/lib.rs | 12 | clap 矩阵（全局旗标、子命令、sessions import 互斥、socket 命名） |
| src/approval.rs | 2 | 提示格式 |
| src/render.rs | 12 | TextSink 分流、SEARCH-1 引用活动行 |
| src/sessions.rs | 5 | jsonl 嗅探（含超 8K）、format_millis |
| src/service.rs | 4 | dry-run / 平台模板 / apply_teardown 真删文件 |
| src/headless.rs | 5 | capability gate、HOST_CAPABILITIES 快照、WorkspaceAdd fail-closed |
| src/error.rs | 1 | provider 错误格式 |
| src/adapter.rs | 2 | **command_id 含进程命名空间且进程内唯一**（ADR-061 防撞键回归）；ACP 身份戳（合法 `acp:<client>` 保留、伪装 User 回退、command_id / idempotency_key 不改写） |
| src/channels/acp/adapter.rs | 3 | decode/admit；registry acp 列 = 四命令钉死 |
| tests/fixtures.rs（target `acp_fixtures`） | 16 | ACP golden：v1 握手逐字节、update/permission 回译、v2 拒绝、未知字段拒绝等 |
| tests/floor.rs（target `acp_floor`） | 27 | ACP 地板：occupancy、Lagged、cwd 越界、early cancel、屏障释放、双客户端交错 |
| tests/common/mod.rs | 装配 | TestHarness / MockScript，被 fixtures+floor 共用 |

默认验证：`cargo test -p pawork-cli --offline --lib --tests`（当前合计 89：unit 46 + integration 43）。交互式 REPL、gui serve 网络路径与真实 Provider 不在本包测试范围。

## 7. 协作关系

```mermaid
graph LR
  app[pawork-app] --> cli[pawork-cli]
  client[pawork-client] --> cli
  proto[pawork-protocol] --> cli
  transport[pawork-transport] --> cli
  engine[pawork-engine] --> cli
  storage[pawork-storage] --> cli
  cli --> bin[apps/pawork]
  desktop[apps/desktop] --> client
```

Desktop 只依赖 client，经 GUI Connection Protocol 连本 crate 拉起的 `gui serve`。ACP 编辑器经 stdio 连 `acp serve`，Core 仍是同一 `AppCore`。
