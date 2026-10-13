# B22 · 逐包架构审查与清理波

> 状态：实施完成（模拟验证）；独立终审 FAIL → T12 修正 → 复审代码项全部 ok，唯一文档项已按复审修法改正；改动全部暂存未提交，未做真窗口复验。更新：2026-10-13。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

用户 2026-10-13 要求逐包 Review，优化架构、清理死代码、去掉过度设计；指挥（主代理）分析与裁决，`grok-4.7-xhigh-fast` 子代理执行细节。本波按 [architecture](../architecture.md) §3.3「无消费者不合入」删除零消费者公开面，合并包内重复实现，修正 Spec 漂移；不改冻结契约（§3.2），不新增包、依赖或抽象，不重提 B1–B21 已裁决项。

## 基线与流程

`main`，HEAD `8490acb3ff9acf42340a364d9eb781ce7353ad66`（2026-10-12）。开始时工作区干净。

1. 九个只读审查子代理按组写发现（contracts / security / persistence / providers / control / platform / app / edge / desktop），每条附消费者核查证据。
2. 指挥逐条裁决：执行、改写或否决；触及冻结契约或产品取舍的列入「待用户裁决」。
3. 实施子代理串行执行（全会话同一时刻一个 Cargo 进程），只格式化自己改动的文件，同批更新包级 Spec。
4. 指挥审 diff、补漏、跑下游编译后 `git add` 暂存，再派下一组。

不读用户 auth / config，不调用真实 Provider。

## 裁决与改动

### T1 · 契约层（protocol / transport / workflow / testkit）

- protocol：删除零消费者的 `app/limits.rs`（control-plane 同名类型不动）；`MockClientAdapter` / `MockClientAdapterFactory` 收进 `#[cfg(test)]`，端口 trait 与 `InMemorySessionRegistryStore` 保持公开。
- transport：删除远程传输占位（`RemoteGuiTransportProvider`、`RemoteGuiConnector` 及其 DTO、`TransportEndpoint::Remote`、`ConnectionLocality::Remote`）和无构造点的错误变体；本地四个 trait 不动。复活条件改写在 [backlog](../spec/backlog.md) BK-REMOTE-01。
- workflow：`TaskManager` 删除广播订阅面并去掉生产 `tokio`；命令面收窄到 Host 实际调用的方法。`PlanState::apply` / `TaskManagerState::apply` 继续折叠全部历史事件变体，重放测试改为直接喂事件序列。
- testkit：删除 `count_variant` 与 `MockTool::assert_called_with`。
- 文档：domain Spec 下游名单按 Cargo.toml 重数；[architecture](../architecture.md) 的 `ProviderStreamEvent` 变体数改为 14。

### T2 · 安全层（git / exec / mcp / tools / policy）

- git：删除 `RepoInfo` / `repo_info` / `git_dir` / `is_bare` / `current_branch` / `read_status` / `changed_files` / `with_runtime`。**行为修正**：`current_head` 先 `symbolic-ref --short HEAD` 取分支名，再 `rev-parse --verify HEAD` 验证提交存在——有名且可解析为 `Branch`，有名但无提交为 `Unborn`，无名可解析为 `Detached(sha)`。旧实现把 detached 判成 unborn、把 unborn 判成分支名；Desktop 状态显示随之改为 `detached:<sha>` / `unborn`。
- exec：删除零引用的 `pub(crate) use` 重导出与 Windows AppContainer 映射（probe 仍冻结为不可用）；Windows 交叉类型检查通过。
- mcp：删除 `auth_token` / `with_auth_token`，明文 HTTP 携密拒绝改按 `headers` 判定，原有拒绝与脱敏断言改用 `headers` 构造；`McpServerCapabilities` 只留 `tools`。
- tools：12 个模块改为私有，保留 `pub use` 门面；删除 `RunCommandTool::with_runtime` 与 `tracing` 依赖；`encoding_rs` 有真实消费者，保留。
- 依赖：git 去掉 `serde_json`、生产 tokio 收到 `rt`；policy 去掉 dev `tempfile`。

### T3 · 持久化（storage / workspace）

- workspace：新增私有 `fs::atomic_write_bytes`（同目录 `create_new` 临时文件 + `write_all` + `sync_all` + `rename`，失败清理；临时名含 pid、进程级计数器与纳秒）。配置写盘改用它（**修正**：原实现没有 fsync，也不是独占临时文件）；compat 导出在其上保留 rename 前的目标 symlink 拒绝。
- workspace：删除对外 v1 `AgentProfile`、`compat_view`、`ResourceBundle.profiles` / `profiles_v2` / `resolved_instructions`、`byte_len` 与 `memory_available`；v1→v2 迁移、Secret 扫描、规则校验与 prompt 注入优先级不变。
- storage：删除 `SessionStore::add_tags`；`CheckpointService::new`、`InMemoryKeyResolver` 收为测试可见；compat 解析函数收为 `pub(crate)`，PWB1 纯函数保持公开。
- protocol：RunStart `profile` 字段注释如实写明 Host 丢弃该冻结字段。
- [architecture](../architecture.md) §5 原子写一条改为「各包私有实现」，并写明 storage 与 workspace 各自的边界。

### T4 · Provider 与认证（auth / providers / models）

- auth：删除通用 OAuth 写入与刷新包装（`store_oauth_token`、`update_oauth_token`、`refresh_oauth_credential_if_needed`、`resolve_oauth_credential_for_request`），`refresh_oauth_credential_with` 的 `reload` 改为必填。
- providers：**修正**非 2xx 时不再缓冲上游错误正文（直接丢弃 response，`classify_status` 去掉正文参数，消息仍是 `HTTP {status}`）；删除 `UsageAccumulator` 与 `event_to_events`；`drive_stream` 两段分发合成一处；两份 `redact_url_origin` 合并为 `net` 内一份；Kimi 与 OpenAI 目录回填共用四字段函数（Kimi 仍不认 `input_modalities`）；渠道差异收进 `ApiKeyChannelPolicy`；根重导出收窄，删除别名 `anthropic_builtin_models` 与 `serde` 依赖。
- models：`content_part_has_image` 公开，Anthropic 删除本地副本。
- **测试修正**：`native_file_lock_excludes_another_backend_instance` 偶发失败，原因是同进程其它测试 spawn 子进程时，子进程在 exec 关闭 CLOEXEC 描述符前短暂共享同一 flock；释放后改为 2 秒有界重试。

### T5 · 控制面与编排（control-plane / orchestration / engine）

- control-plane：删除候审日历 / 脱敏工具、`merge_dual_failures` 家族、策略决策环、`IdentityResolver` / `LocalIdentityResolver`、未接线的并发与审计导出检查及其配置字段、`AuditStore` 查询 trait；保留 `AuditSink`、冻结审计 fixture 与 `max_concurrent_agents`。
- orchestration：删除 `record_policy_allow` / `record_policy_denial` 与 `default_pool_concurrency`；闸门回归改为断言 `PolicyDenied` 原因与 `WorkerAdmitted` 事件。
- engine：删除通用分级裁剪 `trim_tool_result` 家族（CU-12 的 `trim_observation_images` 保留）；`CancelHandle::new` 只收 `run_id`，删除 `ProcessTreeCleaner` 与 `processes_killed`，杀树仍由取消令牌桥到 exec。
- protocol：删除零调用的 `ActorIdentity::canonical_principal`。

### T6 · 平台（computer-use / browser）

- computer-use：删除零消费者的 `platform_probe()` 与 `impl NativeProbe for Box<dyn NativeProbe>`，生产入口是 `NativeHost::host()` → `HostSurface::local()`；`ProbeSnapshot` 不再从根导出。
- browser：数据目录与 origin 归一化的注释和 Spec 如实写明与 app / Desktop、CU-16 的差异；否决把 browser 的数据目录解析改成四段对齐（桥进程为 `cfg(unix)`，Windows 分支无消费者）。

### T7 · App 装配层

- **修正**：新增私有 `ids::process_unique(prefix)`（`{prefix}-{pid:x}-{nanos:x}-{n}`，进程级 static 计数器），session / run / 压缩 run / GUI run / workspace / 审计 / 账本 request / 子代理 id 共用。旧实现是每份 `AppCore` 从 1 起的计数器加毫秒（审计只有毫秒），两个宿主同毫秒会撞 `sessions` / `runs` / `workspaces` 主键，同毫秒两次审计会被 `DuplicateEvent` 拒绝。与 [AGENTS.md](../../AGENTS.md) §11「CLI command_id 裸计数器」同族。`next_run` / `next_session` / `next_workspace` / `next_gui_run` 删除；主循环消息 id `msg-{run_id}-{n}` 随 run id 唯一，不改（终审补漏见 T12）；gateway 随机 id 与实例锁文件名（十进制 pid 前缀是解析契约）不并入。
- 根导出收窄：`render_diff_file`、存储路径函数、`project_timeline_item`、`PersistThenRender`、`ApprovalResolve`、`PendingToolApproval`、`GoalSnapshot`、`first_party_channel`、`is_first_party`、`CompatExternalSource`、`EXPORT_SCHEMA_VERSION`、`GuiEventBus`、`GuiBroadcastSink` 收为 `pub(crate)`；删除零调用的 `GuiHostAdapter::bus()`，`adapter_protocol` 收进测试构建。`base64_encode` / `image_media_type` 从根公开供 CLI 复用。
- 生产依赖去掉 `pawork-transport`（只有测试与 `ui_fixture` example 使用，dev-dependency 已有）。
- 否决：把 browser 数据目录并入 app 规则（见 T6）。

### T8 · 边缘包（client / acp / cli / gui-server）与统一时钟

- client **修正**：headless `PaworkClient` 的 command / query / compat id 原为实例计数器 `cmd-1`…，Automation scope 固定，两次 `pawork headless` 的首条命令会命中持久 `command_ledger` 并重放旧响应。现每个 client 构造时取一份 `new_request_namespace()`（pid + 纳秒 + 进程序号），id 为 `{prefix}-{namespace}-{n}`；不与 GuiClient 每连接命名空间或 CLI `command_namespace` 合并。`MockTransport` 把脚本响应已有的 `request_id` 改写为对应请求的实际 id，不带 id 的帧原样交付。
- acp：持久化的 `connection_id` 追加纳秒段（`acp-connection-{pid}-{nanos:x}-{seq}`），覆盖进程重启后 pid 复用；JSON-RPC `acp-<request_id>` 不变。
- domain 新增 `Timestamp::now()`；engine `now_timestamp` 及 client / acp / gui-server / cli 的私有副本删除，调用改为 `Timestamp::now()`。headless SDK 可达的 `pawork_client::headless::now_timestamp` 按 B18 口径保留为一行转发。
- cli：`--gui-host` 客户端路径从 `chat.rs` 移到私有 `chat_host.rs`（行多重集合比对确认为纯移动）；本地 `base64_encode` / `image_media_type` 删除，改用 app 的同一实现（逐字相同），附件拒绝、8 MiB 上限与描述符二次检查不动；去掉未使用的 `serde` 依赖；Spec 内部依赖列为 11 包。
- gui-server：删除无调用的 `GuiServer::host` / `handshake` / `connections`；测试 `RecordedQuery` 只用于判空，改为计数，消除 dead_code 警告。

### T9 · Desktop 小项

- **安全收紧**：`--instance` 原样拼进 socket 文件名，`../../../x` 这类名字经 `Path::join` 会让 socket 落到数据目录上级。现在 desktop 内按 app `normalize_instance` 同一允许集校验（trim 后非空、只允许 `[A-Za-z0-9._-]`、不得含 `..`），非法名字 usage 失败、exit 2；合法名字与 `default` 的 socket / token 路径不变。回归：`instance_names_reject_path_escape_and_blank`、`instance_names_keep_socket_and_token_paths`。
- 子代理工具行删除与主时间线逐字相同的参数 / 预览函数，改用 `timeline_entry` 的 `arguments_secondary` / `preview_result_text`。
- `EmptyState::labeled` 与 `stale_notice` 收拢 Changes / Resources / Computer / 子代理四处空态配方和三处过期条，逐属性照搬；首页 Timeline 空态配方不同，不并入。
- 删除无视图调用的 `Icon::Collapse` / `RadioDot` / `RadioRing` 及三个 SVG、目录词条 `inspector.stop`。
- 否决：扩展桥数据目录与 GUI socket 对齐（见 T6）。

### T10 · Desktop 大文件机械拆分

只换文件位置，不改分支、几何、文案、AX identifier 或断言。搬运用脚本按行区间切片；指挥用去掉 `use` / `mod` 声明后的 token 多重集合比对复核，差异只允许是 `super::` 层级、`pub(super)`、`impl` 包装、新模块 `//!` 说明和 rustfmt 去掉的尾逗号 / 单表达式闭包花括号。

- `ui/accessibility/app.rs`（13546 行）：入口与共享助手留在父模块（641 行）；Press 白名单 `app/press.rs`、rail 与 header `app/rail.rs`、Timeline `app/timeline_ax.rs`（避开与 `ui::timeline` 重名）、Composer `app/composer.rs`、Inspector 一组 `app/inspector.rs`，各 500–900 行；约 9300 行测试整段移到 `app/tests.rs`。`ui/accessibility/settings.rs` 的测试移到 `settings/tests.rs`（去缩进后逐字一致）。
- `main.rs`：`--probe` / `--probe-smoke` 状态机与两个测试模块移到 `probe.rs`，`main.rs` 只剩 argv、分发与 `run_app`（260 行）；B20 行为不变。
- `ui/mod.rs`（6519 → 5260 行）：`ControllerEvent` 分发与事件泵移到 `ui/events.rs`，`MenuKind` 与菜单开关 / 高亮 / 激活移到 `ui/menus.rs`，焦点链停靠点 `RailStop` / `rail_stops` 归入 `task_rail.rs`。
- `ui/settings/providers.rs`（3244 → 708 行）：卡头、展开、代理 Switch 与 Usage 留下；账号与凭证 `provider_accounts.rs`、四角色菜单 `provider_roles.rs`、Manage models 与推理强度 `provider_models.rs`。
- `controller/mod.rs`（2653 → 1571 行）：Command / Query 构造与 `parse_*` 回执解析移到 `controller/wire.rs`；连接、事件泵、心跳与握手留下，测试仍在父模块。
- 可见性只放宽到 `pub(super)`；三组 token 比对的 `gone` 均为空，Desktop 306 个测试与改前一致，警告无新增。

### T11 · computer-use 大文件机械拆分

平台组审查时采纳、排在后面的一项。按文件里已有的分节注释拆，方法与 T10 相同，公开路径由父模块 `pub use` 保持不变。

- `macos.rs`（5957 行）→ `macos/`：`mod.rs`（公开类型、`MacosNative` 与 `NativeProbe` 实现，1456 行）、`discovery.rs`、`ax.rs`、`ax_tree.rs`、`actions.rs`、`input.rs`、`capture.rs`（内嵌 `sck`）与 `tests.rs`。
- `target.rs`（4333 行）→ `target/`：`mod.rs`（核心类型与 `NativeProbe`，405 行）、`ax_tree.rs`、`actions.rs`、`registry.rs`（`TargetRegistry`，1112 行）与 `tests.rs`。
- token 比对只少了测试模块外壳 `#[cfg(test)] mod tests { }`；多出的是 `pub(super)`（macos 113 处、target 6 处）、新模块 `//!` 和尾逗号。computer-use 104 + 1 个测试、Windows 目标检查的 22 条既有警告都与拆分前相同。

### T12 · 终审补漏

独立只读终审判 FAIL，指出发号统一漏了几条持久化写入。逐条核实后修正：

- **修正**：审批收口（`resolve_waiting_tool_call`）落库的工具消息 id 原为 `msg-{毫秒}-{实例计数}`，`messages.message_id` 是全局主键，两个宿主同毫秒会撞。这条路径处理已持久化的待审批调用，可能由重启后的另一份 `AppCore` 执行，所以不用 `msg-{run_id}-{n}`（新计数器从 1 起，仍会撞旧进程写过的消息），改为 `process_unique("msg")`。
- **修正**：计划 / 目标的旁路 run 原为 `run-plan-{序号}` / `run-goal-{序号}`，事件 id 为 `evt-resume-{run_id}-{序号}`，`session_events.event_id` 是全局主键。两个会话在同一序号上建计划时，第二个直接以 `UNIQUE constraint failed` 失败（新回归修复前实测）。改为 `process_unique("run-plan")` / `("run-goal")`；模型切换的 `run-switch-{毫秒}` 同改。
- GUI fork 分支 `fork-{毫秒}-{adapter 计数}` 改为 `process_unique("fork")`，删除 `next_fork`；`fork-` 前缀不变。CLI `sessions fork` 的 `fork-{毫秒}-{事件前 8 字符}` 不动。
- Desktop `--instance` 校验用 trim 后的值，但原来保存原文：`" dev "` 会产生带空格的 socket 名，而 app 收成 `dev`。现在保存 trim 后的名字。
- 回归：`plan_and_goal_events_persist_in_sessions_at_the_same_sequence`（两个新会话同序号各写计划与目标事件）；`instance_names_keep_socket_and_token_paths` 补带空白的名字。

## 历史文档引用

以下历史计划提到的符号已在本波删除或改名。历史正文不改，以本表为准：

| 历史文档 | 引用 | 现状 |
| --- | --- | --- |
| [B6](b6-auth.md)、[B13](b13-mcp-oauth.md) | `store_oauth_token_keeps_plaintext_out_of_metadata` | 改挂生产入口，现名 `store_default_oauth_token_keeps_plaintext_out_of_metadata` |
| [B13](b13-mcp-oauth.md) | `update_oauth_token_persists_rotated_refresh_and_expiry` | 现名 `update_default_oauth_token_persists_rotated_refresh_and_expiry`；其余三条 auth 锚点原名保留 |
| [B6](b6-auth.md) | `store_oauth_token` 空 refresh 拒绝 | 由 `default_credential.rs` 的 `store_rejects_empty_refresh_without_writes` 与 `update_rejects_empty_refresh_without_overwriting_valid_token` 覆盖 |
| [B13](b13-mcp-oauth.md)、[B14](b14-app-construction.md)、[B15](b15-app-small-cleanup.md)、[B16](b16-app-auth-revision.md)、CU-10 / CU-16 | `HttpTransportConfig::with_auth_token` dead_code 提示 | 方法已删除，提示消失 |
| [B12](b12-git-stage.md) | `RepoInfo` 保留 | 已删除；`current_head` 是唯一 HEAD 状态入口 |
| [B4](b4-prompt-template.md) | `AgentProfile` 资源 | 资源类别与注入仍在；对外 v1 视图类型已删除 |
| [B10](b10-engine.md) | 保留 engine `now_timestamp` 及其根导出 | 已删除；墙钟统一为 `pawork_domain::Timestamp::now()` |
| [CU-12](cu-12-screenshot-context.md) | `trim_tool_result` 未接线 | 已删除 |
| [CU-17](cu-17-gui-observation-protocol.md)、[CU-22](cu-22-windows-isolation.md) | `platform_probe()`、Box 转发 | 已删除；平台选择在 `HostSurface::local()` |

## 待用户裁决

- **`AppEvent::TeamEvent`**：GUI 协议里的 `TeamEvent`（18 个变体）仓内没有构造点，唯一引用是映射成字符串。它在冻结 schema 中，删除属于 wire 变更，需要授权、单独立项并先改 golden。本波未动。
- **CU-25 音频模拟状态机**：只在测试里使用，约 140 行；它是当时复审通过的交付物，删除与否由用户决定。本波未动。
- **全仓 rustfmt**：HEAD 约 50 个文件不符合 rustfmt。本波只格式化改动过的文件，没有做全仓格式化。

## 未做

- computer-use `src/bin/computer_guest.rs`（1616 行）拆分暂缓：只在 Linux 编译，本机无法验证纯移动。
- CLI `sessions fork` 分支名 `fork-{毫秒}-{事件前 8 字符}` 未并入 `process_unique`：同一会话、同一事件、同一毫秒才会撞，且已有 Spec 写明格式。
- B16 裁决保留的 app hub / 幂等 dead_code 不重提。

## 验证

各批由实施子代理串行运行，日志在本机 `/tmp/pawork-review/logs/`（不入仓库）。数字取每批最后一次完整运行。

| 批次 | 命令（经 `bash scripts/test.sh`，feature 由脚本补齐） | 结果 |
| --- | --- | --- |
| T1 | `protocol transport workflow testkit`；下游 `cargo check --tests` app / cli / client / gui-server、`-p pawork`、desktop | 239 passed |
| T2 | `policy exec tools mcp git`；`cargo check -p pawork-exec --tests --target x86_64-pc-windows-msvc` | 326 passed；首轮 `current_head_distinguishes_detached_and_unborn` 失败，修正后重跑通过；Windows 检查无警告 |
| T3 | `storage workspace` | 337 passed |
| T4 | `models providers auth`（收尾时重跑） | 308 passed；auth 另连跑 6 次各 81 passed，锁测试未再复现 |
| T5 | `control-plane orchestration engine`；下游 `cargo check --tests` app / cli / gui-server / acp | 318 passed |
| T6 | `computer-use browser` | 132 passed |
| T7 | `app` | 311 + 2 + 1 passed |
| T8 | `domain engine client acp cli gui-server`；`app`；`--host`（`spawn_e2e`） | 303 passed；app 311 + 2 + 1；spawn_e2e 3 passed |
| T9 / T10 | `desktop`（每批各一次） | 306 passed，警告与改前相同 |
| T11 | `computer-use`（拆前 / 拆后）；`cargo check -p pawork-computer-use --tests --target x86_64-pc-windows-msvc`（拆前 / 拆后）；`cargo check -p pawork` | 104 + 1 passed，两次相同；Windows 检查通过，22 条既有警告不变 |
| T12 | 新回归先在修复前单跑；`app`；`desktop`；`cargo check -p pawork` | 修复前 `UNIQUE constraint failed: session_events.event_id`；修复后 app 312 + 2 + 1、desktop 306 passed，警告不变 |

收尾检查：

- `cargo check --offline -p pawork` 在 T10 后与 T11、T12 后各跑一次，都通过；警告只有 B16 已裁决保留的 5 条 dead_code，以及 `block v0.1.6` future-incompat。
- `cargo tree --offline -p pawork -e normal`：去重后 240 个包，与 HEAD 导出树逐行相同（只差本地路径）。
- `cargo metadata` 工作区依赖边：361 → 351，只删不增；删掉的是 app→transport、cli→serde、control-plane→url、git→serde_json、policy→tempfile（dev）、providers→serde、tools→tracing、workflow→tokio（生产与 dev）、workflow→async-trait（dev）。`pawork-*` 生产依赖图无环。
- `git diff --cached --check` 无空白错误。
- 未跑全 workspace、真实 Provider、真窗口；Desktop 改动只有小项与机械拆分，未做真窗口复验。

独立审查（只读、不编译，一个审查者）：

- 终审覆盖 id 唯一性、headless command_id、ACP connection_id、原子写、`current_head`、Provider 错误正文、MCP 携密拒绝、OAuth reload、Policy / 审计 / 杀树路径、Desktop 实例名、冻结契约与 domain 依赖。结论 FAIL：审批消息 id 撞号（blocker），旁路 run id 与 Spec 不符（should-fix），fork 分支与实例名 trim（nit）。其余各项 ok。
- 复审 T12：四项代码修正 ok，确认旁路 run id 只被当不透明字符串拼接；唯一未决是 desktop Spec 一句「原样保存」与 trim 矛盾，已按复审修法改正（纯文档，未再送审）。
