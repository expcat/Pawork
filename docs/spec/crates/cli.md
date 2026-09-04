# pawork-cli

> `pawork` 二进制的子命令层与三条程序化通道（`--json` / headless JSONL / ACP stdio）。位于宿主装配链中层：被 [apps/pawork](pawork.md) 独家消费，向下依赖 app / client / domain / engine / protocol / storage(session) / transport（依赖方向见 [../../architecture.md](../../architecture.md)）。

## 1. 职责与边界

- **职责**：clap 解析；六运行模式（chat / run / headless / acp / gui / service）与运维子命令；把 `AppCore`（[app.md](app.md)）装配成各通道所需宿主形态；终端渲染与终端审批交互。
- **同进程原则**：CLI 与 Core 同进程同二进制。`run()` 解析参数后加载 `AppCore` 再分发；`service` / `status` / `doctor` / `watch` / `shutdown` 在加载 Core **之前**运行（纯本机检查 / 系统服务操作，不需要 Provider 装配）。
- **边界**：ACP 通道不持有 Provider 凭证、不构造第二个 Core、不消费 GUI Connection Protocol frame；Core 执行统一走窄 port `AcpCommandHost`。`gui serve` 经 `pawork-app` 的 `GuiHostAdapter` / `GuiServer` 提供服务，本包不直接实现协议服务器。
- **不做**：Provider 调用、持久化实现、Policy 判定——全部由下层承载；本包只做参数解析、装配、翻译与呈现。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~1080 | `Cli`（全局参数）与 `Command` 及全部子命令 enum（`SessionsCommand` / `AuthCommand` / `GuiCommand` / `AcpCommand` / `ServiceCommand` / `McpCommand` / `TasksCommand` / `PlanCommand` / `AgentsCommand`）；`run()` / `run_inner()` 分发（GUI 分支把同一次 data-dir 解析同时注入 Core 与 `run_gui`）；`run_models`；`approval_host` 选择；`CliError`；clap 解析单元测试 |
| `src/chat.rs` | ~650 | `run_chat`（REPL + 单次）、`run_once`、`run_json`（`--json` 驱动）；REPL 斜杠命令处理；`drive_turn` Ctrl-C 取消；轮末 usage 行；`map_turn_error` |
| `src/sessions.rs` | ~700 | `sessions list/show/export/import/fork` 实现；`.jsonl` 首行签名嗅探（Codex 信封 / Claude 本地行 / Pi 默认）；`format_millis`（无时区库的 UTC 格式化） |
| `src/auth.rs` | ~120 | `auth list/set-key/login/logout`；OAuth 登录等待 5 分钟（`LOGIN_TIMEOUT`）；只显示掩码 |
| `src/gui.rs` | ~150 | `gui serve`：消费 `run_inner` 已解析的 Host data directory，派生单实例 socket/pid/token，向认证握手注入同一路径；单实例探测、`TokenStore`、socket 目录 0o700、pid 文件、`GuiServer` accept 循环；握手能力由 `pawork_protocol::app::registry::gui_supported_capabilities()` **派生**（无手写清单） |
| `src/headless.rs` | ~430 | `headless --json-stdio`：`HeadlessHandler`（hello 协商、capability gate、compat import/history、事件轮询）；`HOST_CAPABILITIES` 常量 |
| `src/acp.rs` | ~280 | `acp serve` 进程循环：stdin 逐行 JSON-RPC 解析、`session/prompt` 并发 inflight、事件泵任务、outbox 冲刷、EOF 后 30s drain 收尾 |
| `src/ops.rs` | ~360 | `status` / `watch` / `shutdown` / `doctor`；`gui_socket_path` / `gui_token_path` / `gui_pid_path` 命名；token 文件读取组装握手 proof；`InstanceReport` |
| `src/service.rs` | ~390 | `service install/start/stop`：三平台（launchd plist / systemd user unit / Windows SCM）定义生成；默认 dry-run；`--apply` 执行；stop 回收步骤（`TeardownStep`） |
| `src/vcs.rs` | ~200 | `diff`（分页 10 文件/页、git 概况走 stderr）与 `rollback`（列表 / 询问 / 确认 / Blob 还原） |
| `src/mcp.rs` | ~50 | `mcp list/test`：`McpServerStatus` 行渲染（name / transport / state / tools / last_error），`--json` 直接序列化数组 |
| `src/usage.rs` | ~50 | `usage`：`usage_overview` 投影（provider / session / LocalLedger / 配额窗口） |
| `src/tasks.rs` | ~90 | `tasks list/status/cancel/register` |
| `src/plan.rs` | ~100 | `plan show/create/replace/submit/approve/reject`；session 缺省 `latest`，create 无会话时新建 |
| `src/agents.rs` | ~35 | `agents demo`：多 Agent 编排演示报告输出 |
| `src/import.rs` | ~80 | `import <tool>`：compat 配置导入向导（预览 / 确认 / 应用） |
| `src/approval.rs` | ~125 | `InteractiveApprovals`：stderr 打印审批摘要 + 从 stdin 读 `y`/`a`/`n`；取消 token 优先（biased select） |
| `src/adapter.rs` | ~120 | `AppCore` → `GuiHostAdapter` 装配助手；`command_envelope` / `wrap_response` / `stamp_automation`（Automation 身份戳）；`CliAcpCommandHost`（实现 `AcpCommandHost` 窄 port） |
| `src/render.rs` | ~560 | `TextSink`（`AgentEventSink` 实现）：文本流 → stdout，thinking / 工具活动 / 审批往返 / 沙箱回退 notice / 截断提示 → stderr |
| `src/error.rs` | ~80 | `format_provider_error`：`ProviderErrorKind` → 中文可读错误（不重试、不打印 Secret） |
| `src/channels/mod.rs` | ~15 | 外部通道命名空间；本波仅激活 `acp`，re-export 通道 API |
| `src/channels/acp/mod.rs` | ~35 | ACP 子系统 re-export 与 `now_timestamp` |
| `src/channels/acp/host.rs` | ~2120 | `AcpHost` + `AcpActor`：单 actor 循环独占全部状态（occupancy / run_sessions / pending prompts / permissions / outbox / held_events）；普通信箱 + 紧急信箱；`OutboxItem`（Frame / FlushBarrier）；attach / reattach / disconnect 的 ownership 校验 |
| `src/channels/acp/adapter.rs` | ~660 | `AcpClientAdapter`（`ClientAdapter` 实现，纯翻译无状态）与 `AcpClientAdapterFactory`（能力协商：白名单外显式降级）；`CwdResolver` / `SessionResolver` port；registry `acp` 列准入门 `admit_acp_command` |
| `src/channels/acp/command_host.rs` | ~25 | `AcpCommandHost` trait（dispatch / query / subscribe）与 `AcpHostError`——ACP 触达 Core 的唯一执行面 |
| `src/channels/acp/map.rs` | ~285 | ACP ↔ canonical 显式映射表：错误码映射、`extract_user_message`（text / resource_link）、`translate_session_update`、权限选项（allow-once / reject-once） |
| `src/channels/acp/wire.rs` | ~650 | ACP v1 wire 类型：`PROTOCOL_VERSION = 1`、JSON-RPC 消息手工解析（规范精确的 -32700/-32600）、`ParamsExt::reject_unknown`（除 `_meta` 外未知字段显式拒绝）、`SessionUpdate` / `StopReason` 等 schema 类型 |

模块可见性：仅 `pub mod channels`；其余模块私有，另有 crate 级 `pub use error::format_provider_error`。

## 3. 对外 API 面

### 3.1 Rust API（消费方仅 [apps/pawork](pawork.md) 与集成测试）

- `pub async fn run() -> ExitCode`——唯一入口；错误打印到 stderr 并返回 `FAILURE`。
- `pub struct Cli` / `pub enum Command` 及各子命令 enum（clap derive，测试可 `try_parse_from`）。
- `pub mod channels`（ACP 公开面）：
  - 宿主：`AcpHost`（信箱化公开 API：`handle_request` / `handle_notification` / `handle_response` / `pump_events` / `drain_and_pump` / `drain_outbox_items` / `take_outbox` / `release_drained_barriers` / `resolve_queued_prompts` / `fail_closed_all_prompts` / `has_active_runs` / `pending_run` / `degraded_capabilities` / `is_initialized` / `subscribe` / `registry` / `connection_id`）、`OutboxItem`、`PromptResolution`；
  - Core 执行 port：`AcpCommandHost` trait 与 `AcpHostError`（宿主注入实现为私有 `CliAcpCommandHost`）；
  - 翻译层：`AcpClientAdapter` / `AcpClientAdapterFactory` / `NegotiatedAcpAdapter` / `CwdResolver` / `SessionResolver` / `CancelTarget` / `PermissionDecision`；
  - wire：`JsonRpcMessage` / `JsonRpcError` / `JsonRpcId` / `PROTOCOL_VERSION` 及 ACP v1 schema 类型。
- `pub use format_provider_error`。

### 3.2 全局参数

| 参数 | 含义 | 默认 |
| --- | --- | --- |
| `--provider` / `-p` | 覆盖 config 的 default_provider（配置里的 provider id） | 配置值 |
| `--model` / `-m` | 覆盖 config 的 default_model（上游 model id） | 配置值 |
| `--instance` | 隔离实例名（数据目录子路径，影响 socket / token / pid / db 命名） | `default` |
| `--json` | 机器可读输出；chat/run 时 stdout 为 `HeadlessResponse` JSONL（无 hello） | 关 |
| `--approval-mode MODE` | 审批强度五档（见 §4.7） | `read-only`（沿用 V1：不改模式就不会写入） |
| `--trust-workspaces` | 显式信任本进程打开的 workspace；不写配置，仅供可信启动方使用 | 关 |

### 3.3 子命令（21 个顶级 clap 名，以源码 `Command` enum 为准）

**会话与对话**

| 命令 | 用途 | 关键参数 | 输出 / 交互形态 | 安全 / 审批语义 |
| --- | --- | --- | --- | --- |
| `chat` | 流式多轮对话（REPL）或单次提问 | `--prompt`（单次后退出）、`--resume`（完整 id / 唯一前缀 / `latest`）、`--branch`（需 `--resume`，进入前切 branch） | 文本：assistant 增量 → stdout，thinking / 工具活动 / 提示 → stderr；非 TTY 无 `--prompt` 时读 stdin 一行；`--json` 需 `--prompt` | TTY 用 `InteractiveApprovals`（y/a/n）；`--json` 或 stdin 非 TTY 一律 `DenyAllApprovals` |
| `run <prompt>` | 非交互单次任务 | 位置参数 prompt | 同 `chat --prompt`；`--json` 输出 `HeadlessResponse` JSONL | 同上（fail-closed） |
| `sessions list` | 按更新时间列未归档会话 | — | 文本 tab 分隔（id / 更新时间 / 标题）；`--json` 数组 | 只读 |
| `sessions show <session>` | 会话元数据 + 投影消息 | 位置参数支持前缀 / `latest` | 元数据、usage、逐条消息、model switches（读 `model.switched` Diagnostic 事件投影 from/to） | 只读 |
| `sessions export` | 导出 export v3 JSON | `--session`、`--out`（默认 `{session_id}.export.json`） | 写文件并回显路径；`--json` 且无 `--out` 时文档直接打 stdout | 只读源会话 |
| `sessions import` | 导入会话 | `<path>` 或 `--from claude\|codex`（与 path/format/source 互斥）；`--format export\|compat\|pi`；`--source claude\|codex\|grok\|cursor` | 省略 format 按文件名 + `.jsonl` 首行签名嗅探；批量模式输出逐文件状态（imported / deduplicated / error）与汇总，有失败则以错误退出 | 只读源文件；写本机 session 库 |
| `sessions fork <event>` | 从历史事件分叉新 branch | `--session`、`--no-switch`（默认切换，与 Host SessionFork 一致） | branch id 形如 `fork-{millis}-{事件前8字符}` | 只写本会话分支元数据 |

**目录与凭证**

| 命令 | 用途 | 关键参数 | 输出 / 交互形态 | 安全 / 审批语义 |
| --- | --- | --- | --- | --- |
| `models` | 列 provider 模型目录 | 全局 `-p` 可切目录视角 | 文本按六首发通道顺序聚合 + config 自定义 provider，无静态条目的通道提示「login/set-key 后运行期探测」；定价按 micros → 每 M token 货币展示；`--json` 形状标注 unstable | 目录兜底装配（允许默认 provider 缺凭证） |
| `auth list` | 各通道凭证状态 | — | 表格：provider / kind / source / **掩码** / 过期时间 | 不显示明文 |
| `auth set-key <provider>` | 写入 API key | — | key 从 stdin 单行读入；结果只回显掩码 | 明文只经 stdin 进 auth 文件，不回显、不落日志 |
| `auth login <provider>` | OAuth 登录 | — | PKCE 回调或 Device Flow；URL / user code 走 stderr；最长等待 5 分钟 | token 落 auth 文件 default 条目 |
| `auth logout <provider>` | 删除凭证 | — | 删 auth 文件 default 条目 | env fallback 不受影响 |

**改动、回滚与扩展**

| 命令 | 用途 | 关键参数 | 输出 / 交互形态 | 安全 / 审批语义 |
| --- | --- | --- | --- | --- |
| `diff` | 会话累计改动（结构化 hunk） | `--session`、`--page`（每页 10 文件，从 1 起） | 文本渲染或 `--json` 对象（含分页元数据）；git 概况（branch / dirty / worktree）始终走 stderr | 只读 |
| `rollback` | 回滚到写前 checkpoint | `[checkpoint]`（`{run_id}` 或 `{run_id}/{tool_call_id}`）、`--session`、`--yes` | 无 id：TTY 打印列表并询问；`--yes` 自动取最近一个 run；确认 `y/N` 后 Blob 还原并逐文件回显（不用 `git reset --hard`） | `--json` / 非 TTY 必须显式 checkpoint id（拒绝隐式选择） |
| `mcp list` | 列已配置 MCP server 与已发现工具 | — | 表格：name / transport / state / tools / 错误 | 只读 |
| `mcp test [name]` | ping / list_tools 探测 | 省略 name 探测全部 | 同上（带探测结果） | 只发探测请求 |
| `import <tool>` | 导入外部工具配置 | `claude\|codex\|grok\|cursor\|pi`；`--yes`（跳过确认）；`--dry-run`（只预览） | 预览 → 确认 → 应用报告（applied / skipped / plan 路径 / 源文件未变提示） | 只读源文件；写 `.pawork/`；非交互必须 `--yes` 或 `--dry-run` |

**程序化通道与 GUI / 服务**

| 命令 | 用途 | 关键参数 | 输出 / 交互形态 | 安全 / 审批语义 |
| --- | --- | --- | --- | --- |
| `headless` | JSONL 协议入口（SDK / 编程驱动） | `--json-stdio`（必须显式给出，否则 Usage 错误） | stdout 只写 JSONL 协议帧（含 hello 握手）；见 §4.4 | 审批走 `GuiApprovalHost`；capability gate fail-closed |
| `acp serve` | ACP 编辑器通道（JSON-RPC stdio） | 裸 `acp`（无 `serve`）是解析错误 | stdio JSON-RPC（wire protocolVersion 1）；见 §4.5 | 审批转成 `session/request_permission`；registry `acp` 列准入 |
| `gui serve` | 本机 GUI 协议服务（单客户端切片） | `--socket`（覆盖默认路径） | 前台阻塞 accept 循环；Ctrl-C 退出；bind 前探测拒绝双实例 | Token 认证（`gui.token`）；socket 目录 0o700；审批走 `GuiApprovalHost` 转发给 GUI 客户端 |
| `service install` | 生成（或写入）开机常驻定义 | `--apply`（缺省 dry-run 只打印 plan 与激活提示） | plan 文本（macOS launchd plist / Linux systemd user unit / Windows `sc create`）；`--json` 输出对象（service / action / dry_run / plan / platform） | 默认不改系统；服务入口硬编码为 `<exe> --instance <i> gui serve` |
| `service start` | 启动已安装服务 | `--apply` | `launchctl load` / `systemctl --user start` / `sc start` | 同上 |
| `service stop` | 停止并回收服务 | `--apply` | 回收步骤序列：前缀命令尽力执行，**最后一步（删单元文件 / `sc delete`）必须落地**，避免 KeepAlive / 登录再拉起 | 同上 |

**运维与其他**（`service` / 下列前四条在加载 `AppCore` 之前运行）

| 命令 | 用途 | 关键参数 | 输出 / 交互形态 | 安全 / 审批语义 |
| --- | --- | --- | --- | --- |
| `status` | 本机 gui serve 状态 | 全局 `--instance` | `InstanceReport`：instance / data_dir / socket(+listening) / pid_file / session_db / pid | 只做 300ms socket 探测 |
| `watch` | 订阅本机 gui serve 事件直到 Ctrl-C | 全局 `--instance` | `--json` 输出 `HeadlessResponse::Event` JSONL；文本模式事件提示行走 stderr | 需 token 文件组装握手 proof，缺失显式失败 |
| `shutdown` | 停止本机 gui serve | 全局 `--instance` | 读 pid 文件，Unix `kill -TERM`、Windows `taskkill /F`；无 pid 文件报错 | 只对 pid 文件记录的进程操作 |
| `doctor` | 本机装配自检 | 全局 `--instance` | status 全量 + 监听时追加握手探测（`GuiClient` 3s 超时，报告 client id 与能力数；失败原因附于 `handshake:` 行） | 同 `watch` |
| `usage` | 用量与本地配额（LocalLedger） | `--session` | provider / session 用量 / ledger 累计 / 配额窗口（used / limit / remaining / confidence） | 只读 |
| `tasks list/status/cancel/register` | 后台任务 | `register --kind`（默认 `automation`）、`status`/`cancel` 接 task id | 表格或 `--json`；cancel 回显级联取消的 id 列表 | — |
| `plan show/create/replace/submit/approve/reject` | Plan 审批流 | `--session`（默认 `latest`）、`--title`、`--step`（可重复、create/replace 必填）、`reject --reason` | `plan_id@version 标题 评审状态` + 步骤清单 | 评审状态机由 app 层承载；create 无既有会话时新建 |
| `agents demo` | 多 Agent 编排演示 | `--cancel`、`--budget-tokens` | 报告：parent / workers / cancelled / budget-gate / 事件序列 | — |

### 3.4 REPL 斜杠命令（`chat` 交互模式）

| 命令 | 行为 |
| --- | --- |
| `/exit`、`/quit` | 退出（空闲时连按两次 Ctrl-C 亦可） |
| `/compact` | 手动压缩当前会话：与自动链同一 engine 函数与事件序，回显 `before → after` 消息数 |
| `/model [name]` | 无参列当前 provider 静态目录；有参切换并落 `model.switched` 事件 |
| `/provider <id> [model]` | 切换 provider（可选同时切模型），事件流记录变更 |
| `/plan show\|create\|replace\|submit\|approve\|reject` | Plan 操作；create/replace 语法 `Title \| step1 \| step2` |
| `@file` | 消息内的工作区文件引用，经 `expand_at_refs` 展开后进入本轮内容 |

## 4. 核心行为与数据流

1. **启动与装配**（`run_inner`）：
   - clap 解析 → `normalize_instance`；`service` / `status` / `doctor` / `watch` / `shutdown` 五命令直接进入 pre-core 分支返回。
   - `gui serve` 在加载 Core 前只解析一次实际 data directory：同一个 `PathBuf` 同时写入 `AppLoadOptions.data_dir` 并传给 `run_gui`；Core 存储、socket/pid/token 派生和 Accepted 握手元数据因此不会分叉。`--socket` 只覆盖 endpoint，不改变 data directory。
   - 组装 `AppLoadOptions`（provider / model / instance / `parse_approval_mode` / `trust_workspaces` / approval_host）；`--trust-workspaces` 只设置本进程显式覆盖，不修改配置；gui / headless / acp 三命令强制换 `GuiApprovalHost`。
   - 目录与协议入口类命令（models / sessions / auth / diff / rollback / mcp / import / headless / acp / gui / usage / tasks / plan / agents）用 `AppCore::load_for_catalog`（容忍默认 provider 缺凭证的目录兜底装配），其余（chat / run）用 `AppCore::load`。
   - 普通命令路径在结果返回前执行 `core.shutdown()`，快路径命令（gui / headless / acp / json 驱动）各自负责收尾。
2. **交互式 chat 一轮**：
   - REPL 启动横幅（provider / model / 快捷键说明）走 stderr；`> ` 提示后读行，斜杠命令就地处理；首条普通消息触发 `create_session` 并回显 `session <id>`。
   - `expand_at_refs` 展开 `@file` → 追加 user `Message` → `core.chat_turn` 驱动，事件经 `TextSink` 渲染：assistant 文本增量进 **stdout**；`thinking: …` 前缀行、`⚙ 工具 detail (字节数)` / `✗ 工具 detail (原因)` 活动行、`run_command` 的 `（Ctrl-C 取消当轮）` 提示、`? approve 工具: 原因` 与决策回执行、`[stderr]` 分段（彩色终端红色）、沙箱回退 notice、`已截断` 提示全部进 **stderr**。
   - 需要审批时 `InteractiveApprovals` 在 stderr 打印 `approve <tool> <path> [risk]` + message + preview（写类工具带 diff/hunk 预览），从 stdin 读 `y`（一次）/ `a`（本 run）/ `n`（拒绝）；等待期间收到取消 token 立即返回 `Cancelled`。
   - Ctrl-C 取消当轮（`CancelHandle.cancel(CancelReason::User)` 后等 turn 收尾）；轮末从事件流重放同步本地 history，并打 `tokens: turn in/out | session in/out (cache read/write) | ~费用` 行（registry 无定价条目不编造费用）。取消在 REPL 中不中断循环，单次模式（`--prompt` / `run`）向上抛 `Cancelled`。
3. **`--json` 单次驱动**（`chat --prompt --json` / `run --json`）：
   - 不走 `TextSink`，改经 `GuiHostAdapter`：`SessionOpen`（resume）或 `SessionCreate` → `RunStart`（每个命令响应各打一行 `type=response` 帧）。
   - 订阅事件流，逐事件打 `type=event` 帧，直到本 run 的 `RunChanged` 终态（Completed / Cancelled / Failed / Interrupted）。
   - Ctrl-C 发 `RunCancel`（不退出，等终态帧）；事件订阅滞后打 `Backpressure` 错误帧并以错误退出。REPL 模式不支持 `--json`（缺 `--prompt` 直接 Usage 错误）。
4. **headless 会话驱动**（`--json-stdio` 必须显式）：
   - `hello` 握手：`negotiate_api_version_with` 协商 api version，失败回 `IncompatibleApiVersion` 错误帧；授予能力 = 客户端请求 ∩ `HOST_CAPABILITIES`（Sessions / Runs / Streaming / CompatImport / CompatHistory）。
   - 每条命令 / 查询按 **protocol registry** 的 `headless` 列映射到 capability 做 gate：未授予 → `UnsupportedCapability`；registry 未映射（`None`）→ 同样拒绝（fail-closed）。
   - `SessionClientContextReplace` 额外要求目标 session 是本连接经 SessionCreate / SessionOpen 打开的（`owned_sessions`），否则 `CompatRejected`。
   - `CompatImport` / `CompatHistory` 直连 `SessionStore`（支持 dry-run 与游标分页）。授予 Streaming 后 `poll_event` 持续转发事件帧，滞后发 `Backpressure`；未授予时仅节流休眠。协议循环本体（帧解析 / hello 时序）由 `pawork-protocol::headless::stdio::run_loop` 承载（见 [protocol.md](protocol.md)）。
5. **ACP 会话驱动**（`acp serve`）：
   - 进程循环逐行解析 JSON-RPC：`session/prompt` 请求 spawn 为 inflight 任务（prompt 阻塞至 run 终态），其余请求就地串行；通知与响应回执转交 `AcpHost`；每帧后冲刷 outbox。
   - `AcpHost` 内部是**单 actor**（独立 OS 线程 + current_thread runtime）独占全部可变状态：普通信箱（Mail）串行处理请求 / 事件泵 / outbox drain；`session/cancel` 与 `$/cancel_request` 走**紧急信箱**（UrgentMail，biased 优先），不被队头 prompt 或 Core dispatch 阻塞——Core 调用经 `interruptible_core_call` 挂起期间仍处理紧急件（其余邮件暂存 deferred 队列）。
   - **prompt 串行**只覆盖建立临界区：`reserve_prompt_occupancy`（同 session 已有活跃 prompt → `ERROR_INVALID_REQUEST` 拒绝）→ adapter decode → `RunStart` dispatch → 绑定 run id；绑定后 turn 执行期跨会话可并发。占用窗口内到达的 cancel 记入 `early_session_cancel` / `early_request_cancel` 标志，绑定完成后立即重放。
   - 事件回译：`RunChanged` 终态结算 prompt（映射见 §5），经 outbox 末尾的 `FlushBarrier` 保证此前全部帧写出后才释放；`ToolApprovalRequired` → `session/request_permission` 请求（选项固定 allow-once / reject-once），客户端响应回译为 `ToolApprove` 命令；其余可表示事件 → `session/update` 通知；`Diagnostic` 有意丢弃（不新增 update 臂）。run 归属未知的事件暂存 `held_events`，绑定后冲刷。
   - 订阅滞后 → `fail_closed_all_prompts`：清空 occupancy / pending / run 映射 / held_events，全部未决 prompt 以 Failed 释放，并释放 outbox 中全部屏障；清账本前先拍下已绑定 run 与挂起权限，清空后对每个 run 补发 `RunCancel`、对每个 pending permission 补发 `ToolApprove Deny`（best-effort 补偿，避免 Core 侧悬挂）。stdin EOF 后最多 drain 30 秒（`ACP_DRAIN_TIMEOUT`）等待活跃 run 收尾，inflight 任务 join 超时 2 秒后 abort。
6. **gui serve 生命周期**：
   - bind 前向目标 socket 发 300ms 探测连接，能连上即报错拒绝双实例（Unix bind 会清理 stale socket 文件，探测是唯一在线判定）。
   - 以 `run_inner` 传入的同一 data directory 加载或生成 `gui.token`（`TokenStore`）→ 写 pid 文件 → `HandshakeService`（能力由 registry `gui_supported_capabilities()` 派生，并用 `with_host_data_dir` 注入该目录）+ `TokenAuthenticator` → 认证成功的 Accepted 握手发布可选只读元数据 → accept 循环持有连接句柄（`SessionHandle` 提前 drop 会令客户端握手 Broken pipe）。
   - Ctrl-C 关闭监听、删 pid 文件、关闭 pty 与 Core；关闭不取消已进入 Core 的 run（进程内 run 随进程结束，跨进程存活语义归 service）。
7. **审批五档与宿主选择**：
   - `--approval-mode` 五档：`always-ask` / `ask-for-writes` / `ask-for-dangerous` / `never-ask` / `read-only`（kebab 与 snake 拼写均可；缺省 `read-only`）。已移除的旧档 `on-failure` 保持拼写兼容，**映射为 `NeverAsk`**；未知档报错并列出合法值。
   - `--trust-workspaces` 与审批档正交：它只声明启动宿主对当前 workspace 的信任，不自动降低审批档，也不让 workspace 内容自我提权。
   - 宿主选择与档位正交：`--json` 或 stdin 非 TTY → `DenyAllApprovals`（fail-closed）；TTY 交互 → `InteractiveApprovals`；gui / headless / acp → `GuiApprovalHost`（审批经通道转发给客户端决策）。
8. **sessions import 格式嗅探**（省略 `--format` 时的决策序）：
   - 文件名以 `.jsonl` 结尾 → 逐行读到首个**完整**非空行（不截断，超长首行也不误判）解析 JSON 签名：含 `timestamp` + `type` + `payload` → compat / Codex 信封；含 `sessionId` + `type` 且**无** `payload` → compat / Claude 本地行（首行常为 ai-title / queue-operation 等无 message 行）；签名不明确 → Pi 默认。
   - 文件名以 `.export.json` 结尾 → export。
   - 其他文件读全文：JSON 对象含 `"schema_version"` → export；否则 → compat（来源待 `--source` 指定）。
   - 均不匹配 → 显式报错要求 `--format export|compat|pi`，不猜测。
   - `--from claude|codex` 批量路径跳过嗅探：扫描本机会话目录后逐文件按 compat + 对应来源导入。

## 5. 契约与不变量

- **stdout 协议纪律**：`--json` 与 `headless --json-stdio` 下 stdout 只承载 JSONL 协议帧；`acp serve` 的 stdout 只承载 JSON-RPC 帧。文本说明、URL、审批提示、git 概况、日志一律走 stderr。
- **HeadlessResponse 形状**（冻结，见 [../contracts.md](../contracts.md)）：`type = event | response | error`（headless 通道另有 hello / hello_ack 握手对；`--json` 模式无 hello）。错误帧 `kind` 取 `ProtocolErrorKind`（本包用到 IncompatibleApiVersion / UnsupportedCapability / CompatRejected / Backpressure / Internal）。编码统一走 `pawork-protocol::headless::translate::encode_protocol_response`，本包不得自拼 JSON 帧。
- **headless capability gate**：`HOST_CAPABILITIES` 五项由测试钉死快照；registry `headless` 列必须 ⊆ `HOST_CAPABILITIES`（测试断言）；**未映射命令 fail-closed**——`command_entry().headless == None` 一律 `UnsupportedCapability`，不得静默放行（`WorkspaceAdd` 有定向回归）。
- **ACP wire 契约**：`PROTOCOL_VERSION = 1`（整数）；实验 v2 握手显式拒绝（-32602，错误信息点名 experimental v2）；`initialize` 每连接一次。JSON-RPC 错误码固定：-32700 Parse / -32600 InvalidRequest / -32601 MethodNotFound / -32602 InvalidParams / -32603 Internal / -32800 RequestCancelled / -32000 AuthRequired / -32002 ResourceNotFound。params 未知字段（除保留 `_meta`）一律 -32602；`session/request_permission` 响应是嵌套 outcome 形状，扁平形状与未知字段拒绝。`fixtures/v1` 的 golden **先于实现改动**（冻结契约清单见 [../../design.md](../../design.md)）。
- **RunState → StopReason 映射**（ACP prompt 结果）：Completed → `end_turn`；Cancelled / Interrupted → `cancelled`；Failed → JSON-RPC internal error（"prompt turn failed in Core"）。
- **GUI 握手能力派生**：`gui serve` 对外宣告的 capability 集不手写，恒等于 `pawork_protocol::app::registry::gui_supported_capabilities()` 的派生结果——registry 增删命令映射时 GUI 宣告自动跟随；禁止在本包重新出现字面量能力清单。
- **GUI 数据目录单源**：Core 加载、socket/pid/token 路径和 API 1.9 Accepted `host_data_dir` 必须消费 `run_inner` 的同一个解析结果；不得在 `run_gui` 二次读取环境或按 endpoint 反推。`host_data_dir` 只作当前认证客户端的只读展示元数据，不进日志、事件、ledger 或文件操作输入。
- **ACP 命令准入**：registry `acp` 列即 ACP 可达命令全集（当前 `session_create` / `run_start` / `run_cancel` / `tool_approve`，测试钉死）；adapter decode 产物与宿主自构命令都过 `admit_acp_command`，列外命令 `ProtocolUnsupported`。禁止在本包另维护一份命令名字表——三通道可用性一律查 protocol registry（headless 列 / acp 列 / `gui_supported_capabilities()`）。
- **审批 fail-closed**：`--json` 或 stdin 非 TTY 时任何审批请求都被拒绝（`DenyAllApprovals`）；ACP 权限选项固定 `allow-once` / `reject-once`，未知 option id 拒绝；客户端错误响应视为 Deny，`-32800` 视为 Cancel。
- **通道身份戳**：三条程序化通道进 Core 的命令 / 查询一律 `CommandSource::Automation` + `ActorIdentity::Automation`（名称分别 `cli-json` / `headless` / `acp:pawork-acp`），command id 前缀 `cli-<name>-<n>` / `acp-<request_id>`——事件与审计侧可区分来源。
- **ACP 错误映射为显式表**：`AdapterError` → JSON-RPC 码、`AdapterErrorFrame.code` 字符串 → 码、canonical `ErrorContext.category` → 码三张映射都在 `map.rs` 落表（NotFound → -32002、Authentication/Authorization → -32000、InvalidRequest → -32602、Cancelled → -32800、其余 → -32603）；`Artifact` 响应在 ACP 通道不支持（-32603）。
- **凭证红线**：明文 key 只经 stdin 进 auth 文件；`auth list` 只显示掩码与来源；`format_provider_error` 对认证错误不透传上游消息原文。
- **单实例与文件权限**：`gui serve` bind 前探测防双实例；socket 父目录（位于数据目录内时）强制 0o700；token 文件缺失 / 空内容显式失败，不回退为无认证。
- **实例命名契约**（`ops.rs` 定义、lib.rs 测试钉死 default 命名不因 instance 参数化回归漂移）：
  - default instance：socket `pawork-gui.sock`、token `gui.token`、系统服务名 `pawork`；
  - 命名 instance `<i>`：socket `pawork-gui-<i>.sock`、token `gui-<i>.token`、服务名 `pawork.<i>`；
  - pid 文件固定为 `<data_dir>/<instance>/gui-serve.pid`（`instance_dir` 来自 `pawork-app`，default 实例同样落在 `default/` 子目录）；`status` / `watch` / `doctor` / `shutdown` 与 `gui serve` 共用这一组命名函数，不得各自拼路径。
- **ACP 单 actor 不变量**：全部可变状态由 actor 独占，公开 API 经信箱进出（快照经 `watch` 只读发布）；禁止 `std::sync::Mutex` / `RwLock`；同 session 同时至多一个 prompt；prompt 结果必须经 `FlushBarrier` 在此前帧全部写出后才释放；cwd 必须位于已登记 workspace root 内（组件级前缀匹配 + 两侧 canonicalize），不静默 `WorkspaceAdd`。

## 6. 依赖关系

**Cargo 依赖**（内部 7 包）：

- `pawork-app`：`AppCore` / `AppLoadOptions` / 审批宿主（`ApprovalPromptHost` / `DenyAllApprovals` / `GuiApprovalHost`）/ `GuiHostAdapter` / `gui_server::{GuiHost, GuiServer}` / 各领域投影 API（[app.md](app.md)）。
- `pawork-protocol`（feature `adapter`）：`AppCommand` / `AppEvent` / envelope 家族、headless wire 与 `stdio::run_loop`、`app::registry`（命令三通道可达性）、`adapter::{ClientAdapter, SessionRegistry, …}`、`client_auth`、握手服务（[protocol.md](protocol.md)）。
- `pawork-client`：`GuiClient`（`watch` / `doctor` 的握手与订阅，[client.md](client.md)）。
- `pawork-storage`（`default-features = false`, feature `session`）：`SessionStore` compat import / `SqliteClientSessionRegistryStore`（[storage.md](storage.md)）。
- `pawork-transport`：`LocalTransport` / `TransportEndpoint`（gui serve 监听与运维探测，[transport.md](transport.md)）。
- `pawork-domain` / `pawork-engine`：id 与事件类型 / `AgentEventSink`、`CancelHandle`（[domain.md](domain.md)、[engine.md](engine.md)）。

**外部依赖**：clap（derive）、tokio（macros / rt-multi-thread / signal / io-std / io-util / sync / time）、serde / serde_json、async-trait、thiserror、tracing。无 crate feature。

**dev 依赖**：tempfile（集成测试的临时 workspace root / 会话文件）。

**被依赖**：仅 `apps/pawork`（[pawork.md](pawork.md)）。集成测试经 `pawork_cli::channels` 消费 ACP 公开 API。

## 7. 测试与验证资产

默认验证命令：`cargo test -p pawork-cli --offline --lib --tests`

| 资产 | 覆盖点 |
| --- | --- |
| `src/lib.rs` 内嵌 tests | 21 个子命令的 clap 解析矩阵（含 `sessions import` 三组互斥断言、`--branch`、`--instance`）；`--approval-mode` kebab 解析、`--trust-workspaces` 全局解析、`on-failure → NeverAsk` 兼容、未知档拒绝；default instance 的 socket / token 命名稳定性 |
| `src/approval.rs` tests | 审批提示格式（tool / path / risk / preview；edit 与 apply_patch 的 hunk preview） |
| `src/render.rs` tests | 工具活动行（成功字节数 / 失败原因）、`run_command` 取消提示、stderr 红色仅限彩色终端、`已截断` 检测、沙箱回退 notice（现行 / 旧版 Diagnostic 形状、空 note、message 直传与默认串） |
| `src/sessions.rs` tests | `.jsonl` 首行签名嗅探（Codex / Claude / Pi、首行无 message、首行超 8K 不误判）、本地源白名单（拒绝 grok）、`format_millis` epoch 断言 |
| `src/service.rs` tests | 三平台 stop 回收计划（macOS unload + 删 plist、Linux stop + disable + 删 unit、Windows sc stop + delete）；`apply_teardown` 真删文件 |
| `src/headless.rs` tests | `WorkspaceAdd` 未映射 fail-closed；已授予 capability 放行；registry headless 列 ⊆ `HOST_CAPABILITIES`；`HOST_CAPABILITIES` 快照钉死 |
| `src/error.rs` tests | 认证 / 限流 / 超时 / 网络四类错误文案（认证错误不透传上游消息） |
| `src/channels/acp/adapter.rs` tests | registry `acp` 列 = 四命令钉死；准入门放行 / 拒绝 |
| `tests/fixtures.rs`（target `acp_fixtures`） | versioned golden：v1 initialize 握手响应逐字节比对、session/new / prompt / cancel fixture 解析、session/update text 与 tool_call 回译 golden、permission selected / cancelled、未知方法与 `session/set_model` 错误 golden、v2 握手拒绝、未知 params 字段拒绝、`mcpServers` 必填 vs 空数组放行、resume 缺省字段、JSON-RPC 非对象 / 坏版本拒绝 |
| `tests/floor.rs`（target `acp_floor`） | 全链路（mock host）：握手协商与降级记录、未初始化拒绝、cwd 越界拒绝与规范化别名匹配、prompt 流式回译与终态、权限请求往返、`session/cancel` / `$/cancel_request`、close → resume 重挂、跨连接 resume 走 authoritative registry claim、同 session 二 prompt 拒绝、注册窗口 early cancel 重放、fail-closed 释放 inflight、事件滞后 fail-closed、部分写出后屏障必须释放、Diagnostic 不发射 update、双客户端交错保持会话内串行且 cancel 不被阻塞 |
| `tests/common/mod.rs` | `TestHarness` / `MockScript`（脚本化 `AcpCommandHost`）与 outbox 收集工具 |
| `fixtures/v1/`（13 个 JSON） | `initialize-request/response`、`session-new-request`、`session-prompt-request`、`session-resume-minimal`、`session-cancel-notification`、`session-set-model-request`、`session-update-text`、`session-update-tool-call`、`permission-response-selected/cancelled`、`error-unknown-method`、`error-unknown-set-model` |
| `fixtures/v2/`（1 个 JSON） | `initialize-request-v2`——仅用于断言实验 v2 显式拒绝 |

Cargo `[[test]]` 把 target 命名为 `acp_fixtures` / `acp_floor`（文件为 `tests/fixtures.rs` / `tests/floor.rs`）。交互式 REPL、gui serve 网络路径与真实 Provider 不在本包测试范围（验证策略见 [../verification.md](../verification.md)）。

2026-09-03 SET-6g 与 client 合并运行默认门禁，CLI 84/84、client 41/41 通过；`cargo check -p pawork --offline` 通过。GUI data directory 的同源装配由类型/调用链编译覆盖，握手字段的 wire 与透传由 protocol/client 定向回归锁定。

## 8. 注意事项与已知限制

- 裸 `pawork acp`（无 `serve`）是 clap 解析错误；`headless` 不带 `--json-stdio` 是显式 Usage 错误（防止把协议帧混进人类终端）。
- REPL 不支持 `--json`；`--branch` 必须与 `--resume` 同用；非 TTY 管道进 `chat` 只读 stdin 第一行作为单次提问（多轮驱动请改用 headless）。
- headless 未授予 Streaming 时 `poll_event` 只做 25ms 节流休眠、不转发事件也不报错；客户端要流式必须在 hello 里请求该能力。
- CLI `resume` 会把中途被杀、停在 waiting_for_approval 的孤儿审批 seal 为 Denied——该语义承载于 `pawork-app`（GUI resume 走 keep-pending，差异是有意的）。
- `models --json` 的 models 数组形状标注 unstable，随 registry 目录演进。
- `rollback` 交互式输入的 checkpoint id 不在列表内时不在 CLI 层拦截，由 core 侧回滚失败兜底。
- ACP actor 运行在独立 OS 线程（current_thread runtime）：公开同步 API（drain / fail-closed）带 2s 超时回执，超时降级为 degrade 告警不阻塞 teardown；actor 线程 spawn 失败只记 degrade 事件，宿主进入不可用态（请求回 "ACP host actor is unavailable"）。
- ACP 首轮能力白名单为空（`ACP_SUPPORTED_CAPABILITIES = []`）：客户端声明的能力全部降级记录（可经 `degraded_capabilities()` 审计），`mcpServers` 非空、`additionalDirectories`、image / audio / resource content block、`session/load` 均显式拒绝；`resource_link` 映射为安全文本引用 `[name](uri)`，不拉取资源。
- `service` 定义模板硬编码 `gui serve` 为服务入口；不支持的平台显式报错。macOS / Linux 的 `install --apply` 只写定义文件，激活需按提示手动执行或 `start --apply`。
- `watch` / `doctor` 的握手探测需要 token 文件本机可读；`doctor` 报告 `handshake: failed: …` 而不中断其余检查。
- 本包自身不安装 tracing subscriber：日志装配与全字段脱敏（`Redactor` / `RedactingFmtLayer`）由宿主二进制承载（见 [pawork.md](pawork.md)），本包只发 `tracing` 事件。
- 状态回写与任务登记见 [AGENTS.md](../../../AGENTS.md)；跨包时序图见 [../flows.md](../flows.md)；产品能力口径见 [../README.md](../README.md)。
