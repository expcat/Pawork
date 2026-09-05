# pawork-app

> 应用装配门面（assembly host）：把 domain / engine / providers / tools / policy / workspace / exec / storage / git / workflow / orchestration / control-plane 焊成 `AppCore`，并实现 `GuiHost` 服务 GUI Connection Protocol。处于库依赖图顶端，生产上仅被 [pawork-cli](cli.md) 消费（`pawork` 二进制经 cli 间接使用）。

## 1. 职责与边界

**做什么**

- 装配 `AppCore`：配置发现（Builtin → Global → Workspace → CLI 覆盖）、凭证链（auth 文件 → env）、协议中立 provider、内建读写工具 + `run_command`、session store、checkpoint/artifact/protected 存储、usage/quota/audit 控制面。
- 承载一次 run 的宿主编排：`chat_turn` → `pawork_engine::run_session`，事件 persist-first 落库再渲染；审批、压缩、检查点由 `SessionLoopCtx` 桥接进 engine loop。
- 实现 GUI 宿主侧：`gui_server`（连接/心跳/订阅/resume 帧循环）+ `gui_host`（`GuiHost` trait 适配 `AppCore`，query/command 静态分发、幂等、timeline 投影分页、事件总线）。
- 提供 CLI 命令背后的领域门面：auth/OAuth、模型目录与切换、diff/checkpoint/rollback、MCP、compat import、tasks、plan gate、usage 报表、多 Agent demo。

**不做什么**

- 不定义 wire 契约：GUI 帧形状、`AppCommand`/`AppQuery`/`AppEvent`、timeline 投影规则全部在 [pawork-protocol](protocol.md)；本包只消费。
- 不实现 Provider 协议、工具、持久化、Policy 判定本体（分别在 [providers](providers.md) / [tools](tools.md) / [storage](storage.md) / [policy](policy.md)）。
- 不做终端/GUI 渲染（cli 与 desktop 的职责）。Desktop 进程**禁止**依赖本包，只经 protocol + transport 连 CLI（架构红线，见 [../../design.md](../../design.md) §2）。

R4 已把早期巨 match 拆为 `services/` 七个领域服务 + `gui_host/handlers/` 静态分发表；`services/` 与 `CatalogOnlyProvider` 均非公开 API。

## 2. 模块与文件地图

全包约 2 万行（含内嵌测试与 tests/）。src/ 共 58 个 `.rs`，tests/ 4 个。

可见性布局：`gui_server` 是唯一 `pub mod`；`gui_host` 与 `services` 目录私有，公开类型统一经 `lib.rs` re-export；`testsupport` 仅 `cfg(test)` 编译。`AppCore` 本体在 `app_core.rs`。

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~80 | 模块声明与 crate 根 re-export 单点（CLI 消费面不变） |
| `src/app_core.rs` | ~1790 | `AppLoadOptions`、`AppError`（30+ 变体错误汇聚）、`CatalogOnlyProvider`（缺凭证 fail-closed 占位 provider）、`AppCore` 结构体与装配（`load*`/`from_config`/`from_parts*`）、会话/运行/usage/diff/checkpoint 门面方法、`SessionTokenEstimatorBridge`、`session_title_from_text`；`from_parts_with_protocol` 的 HTTP 客户端为 `pawork_auth::http_client()`（F06 `redirect(Policy::none())`），带 proxy 的路径仍走 `http_from_config` |
| `src/provider_assembly.rs` | ~1250 | provider 装配单点：`assemble_provider`/`assemble_registry`、通道→协议解析（KimiOAuth→ChatCompletions 装配 `KimiCodeProvider`；xAI SET-4 双认证按存储形态解析凭证——api key 优先、无则 OAuth 含刷新）、OAuth 刷新装配、`switch_model`/`switch_provider`（含 ModelSwitched 诊断事件）、`model_catalog`/`models_overview`/`provider_models` 目录聚合（builtin + config 覆盖 + 运行期探测，kimi-code 静态目录合并）、`is_credential_pending`、ADR-052 `provider_proxy` 按 provider 解析生效代理（Global `proxy_url` 统一生效，仅当该 provider 显式 `use_proxy = false` 时绕过；六个装配点统一接入，不按 provider 名称特判） |
| `src/idempotency.rs` | ~660 | `IdempotencyStore`：以 storage `CommandLedger`（SQLite）为权威 CAS 持久态，内存 `Notify` 做 InFlight 有界等待；`IdempotencyCheck`{New/Replay/InFlight}、`should_cache`、容量逐出、`IdempotencyStats` |
| `src/protected.rs` | ~620 | Reasoning 保护：`SwappableReasoningProtector`（内存 ↔ 持久动态绑定）、`ProtectedBlobStore` + `FileKeyResolver`（`master.key`）注入；instance 级 `BlobScope` `instance-reasoning` |
| `src/approval.rs` | ~520 | `ApprovalAsk`/`ApprovalResolve`、`ApprovalPromptHost` trait、`GuiApprovalHost`（pending/queued 单锁决议池 + `ToolApprovalRequired` 事件发布）、`DenyAllApprovals`、`PreApprovedResolver`、`parse_approval_mode`、写工具预览（`relative_path_from_input`/`preview_for_tool`） |
| `src/loop_ctx.rs` | ~430 | `SessionLoopCtx` 实现 `pawork_engine::LoopContext`：审批请求转宿主、工具执行经 `ToolScheduler`、写前 checkpoint、压缩（fork recovery branch + snapshot）、message/request id 发号、事件 emit |
| `src/extensions.rs` | ~420 | 内建工具注册表、MCP 装配（auto_start/untrusted 拒绝/stdio 沙箱 + env 卫生）、`mcp_list`/`mcp_test`、`@token` 词法 `at_tokens`、`AT_FILE_MAX_BYTES`（64 KiB）、skill 目录发现 |
| `src/auth.rs` | ~440 | `auth_status`（只报来源 file/env/none，不回显 secret；SET-4 起按 auth_methods 数据判定，xAI 双认证先查 api key 再查 OAuth meta，显示 method 与实际凭证一致）、`auth_set_key`（声明 oauth 的通道写入后移除旧 OAuth 条目）/`auth_logout`（双认证通道两类条目幂等清理）、`oauth_begin`/`oauth_complete`（PKCE 与 Device Flow 编排）、`oauth_finish`（pub(crate)：不持 AppCore 锁的 OAuth 收尾，供 GUI Device Flow 后台轮询任务复用；store 成功后对称移除旧 api key 条目，删除失败 fail-closed）、`AuthChannelStatus`/`AuthSource`/`OAuthLogin` |
| `src/hub.rs` | ~410 | `EventHub`：全局序 + ring buffer（默认 4096）+ `tokio::broadcast` 有界订阅；`global_sequence` 连续重写、`replay_from`、越界→`HubError::ReplayUnavailable`、慢订阅者 `Lagged` |
| `src/testsupport.rs` | ~390 | 仅 `cfg(test)`：`RecordingEvents`/`ScriptedProvider`/`mock_core*`、`RecordingSubscriber`、`RecordingCapture`（双注册 Dispatch 钉住 tracing-core interest 缓存，防投毒）及其回归测试 |
| `src/diff.rs` | ~380 | 会话累计 diff：git 仓走 `pawork-git` 且按 session 改动路径过滤，非 git 回退写前快照对比；`SessionDiff`/`GitDiffHeader`、`paginate_diff`、`render_session_diff`、`git_status_note`（注入 provider 请求，失败静默省略） |
| `src/control.rs` | ~370 | `ControlPlaneRuntime`（in_memory/persistent：`SqliteUsageLedger` + `QuotaService` + `FileAuditStore`）；usage 记录哨兵字段（`record_id = "rec-<run_id>"`、单机 tenant/principal 哨兵）；`ledger_totals`/`quota_windows`/`append_audit`；`UsageOverview` 等报表行类型 |
| `src/checkpoint.rs` | ~320 | 写工具识别（write_file/edit_file/apply_patch）、写前快照、`list_checkpoints`/`CheckpointSummary`、`rollback`/`perform_rollback`（恢复文件 + 持久化 `CheckpointRolledBack`） |
| `src/import_host.rs` | ~310 | compat 导入宿主包装：`CompatTool::parse`（claude/codex/grok/cursor/pi）、`SessionImportFormat`、payload 落盘（instructions/skill/MCP merge/profile）、源文件指纹快照（`snapshots_match` 防 TOCTOU） |
| `src/data_dir.rs` | ~300 | 数据目录解析：`PAWORK_DATA_DIR` → `%LOCALAPPDATA%\pawork`(win) → `~/.pawork` → temp 回退；`DataDirOutcome`（HOME 回退附 `DegradeEvent`）、`consume_data_dir_outcome`（唯一告警点）、`normalize_instance` 白名单校验、各实例文件路径 helper |
| `src/devfixture.rs` | ~1350 | `cfg(any(test, feature = "ui-fixture"))` + `#[doc(hidden)]` dev-only（R1 Wave B）：UI fixture 种子器。默认 feature 关闭，不进入生产编译；声明式数据集在写入前校验引用、枚举、相对路径与时间锚点，拒绝绝对路径 / `.` / `..`、默认数据目录与仓库重叠、Unix socket 路径超限以及时间戳溢出/越界；git 基线隔离用户/系统配置与 `GIT_*` 路由环境；seed 先写 `preparing` marker，完整收口后改 `ready`，失败可安全重试且 serve fail-closed。数据经 SessionStore / CheckpointService 公开 API + git/文件写入隔离 root；不依赖 testkit |
| `src/channels.rs` | ~210 | 首发通道 facade：从 providers `CHANNEL_REGISTRY` 派生 `FIRST_PARTY_CHANNELS`/`first_party_channel`/`is_first_party`/`ChannelKind`，`oauth_override` 允许配置覆盖 OAuth preset；通道登记单点在 providers |
| `src/orchestration_host.rs` | ~210 | S11 多 Agent demo：`run_multi_agent_demo`（Supervisor spawn 双 worker / cancel-tree / budget-gate），固定样例 provider/model id，非通用编排 API |
| `src/plan_host.rs` | ~190 | Plan 审批 gate：`plan_snapshot/create/replace/submit/approve/reject`（事件重放构建 `PlanService`，决议落 audit）；`ensure_plan_allows_execution`（无 Plan 放行，有未批准版本拦截 run 并 audit Deny；重放失败原样上抛，禁止 fail-open） |
| `src/protocol.rs` | ~140 | 适配器协议选择：`AdapterProtocol`{ChatCompletions/Messages/Responses}，`extra.provider_protocols[id]` → 样例默认表 → ChatCompletions；未知值 fail-closed（`ProtocolError::Unknown`）；纯配置数据，engine 不读 |
| `src/tasks_host.rs` | ~90 | tasks 门面转发 + `parse_task_kind`；`tasks.json` 快照 load/replay 与原子写（tmp + rename） |
| `src/persist.rs` | ~20 | `PersistThenRender`：先 `append_event`（用 session 当前 active branch）成功再交渲染 sink |
| `src/services/mod.rs` | ~10 | 七服务模块声明（全 `pub(crate)`） |
| `src/services/session.rs` | ~730 | `SessionService`：会话生命周期、workspace 绑定（ADR-043：初始绑定与 session/main 分支同事务落盘，启动时读取全部绑定并以 `replace_workspace_cache` 原子替换；`bind_session_workspace` 保持仅内存供 devfixture）、事件序号 `next_sequence`、`resolve_session`（前缀/序号）、`resume_messages`（CLI：`seal_orphaned_approvals` 落 Denied）与 `resume_messages_keep_pending`（GUI：保留待审批）；`resolve_waiting_tool_call` 返回落库 envelope 序列供宿主补广播 |
| `src/services/run.rs` | ~600 | `RunService`：`chat_turn`/`chat_turn_with_run_id`（Plan gate → quota 预检 → TurnContext 装配含 `git_status_note` → engine `run_session` → usage 落账）、`compact_session` 手动压缩、`append_payload` 事件追加（返回 envelope 供补广播） |
| `src/services/approval.rs` | ~500 | `ApprovalService`：审批模式与宿主装配（启动 `configure`、运行时 `set_mode`）；ADR-053 由 Host 先落盘再更新审批快照，逐项目信任由 AppCore 配置解析、`ApprovalPromptHost` 委派、模式变更时重建 `ToolScheduler` 配置 |
| `src/services/usage.rs` | ~400 | `UsageService`：持有 `ControlPlaneRuntime`；`projected_run_usage` 预算预检、`record_completed_usage` 落 `usage-ledger.sqlite3`、`usage_overview`/`session_usage`/`last_run_usage`/`estimate_cost_for` |
| `src/services/extension.rs` | ~330 | `ExtensionService`：workspace roots/file-index、`expand_at_refs`（命中 file-index 的 `@` 附件展开为独立 Text part，64 KiB 截断标记）、`complete_at`、注入层加载（instructions/skills/profiles）、MCP slot 持有与关停 |
| `src/services/import.rs` | ~310 | `ImportService`：本机会话扫描、compat 预览/应用（指纹校验 `sources_unchanged`）、`export_session_doc`/`import_session_file`（export/compat/pi 三格式） |
| `src/services/tasks.rs` | ~260 | `TaskService`：`TaskManager` 状态机 + `tasks.json` 持久化；注册/查询/取消/收尾；persist 失败发 degrade 不吞错 |
| `src/gui_server/mod.rs` | ~180 | `GuiHost` trait（snapshot/timeline/query/command）、`GuiHostError`、`GuiServer`/`GuiServerConfig`（bind endpoint、accept 循环、按连接 spawn 会话任务）；re-export 连接层常量 |
| `src/gui_server/connection.rs` | ~550 | `ConnectionManager`：客户端注册/心跳（`DEFAULT_HEARTBEAT_TIMEOUT` 30s idle 清理）/事件订阅；每连接有界 mpsc 队列（`DEFAULT_QUEUE_CAPACITY` 1024），慢客户端标记 `lagged` 丢新事件不阻塞发布者；断连**不**取消 run |
| `src/gui_server/session.rs` | ~1000 | 单连接握手与帧循环：协议版本检查、command 盖 client 戳、capability 门（未授予在宿主前拒绝）、Resume 三态调度（replay / SnapshotRequired / up-to-date）、Heartbeat→Pong、订阅确认、lagged→ReplayUnavailable 帧；ADR-045 `deliverable_to_negotiated` 按协商 minor 门控推送——`TerminalExited`（since 1.3）不推给协商 <1.3 的连接（老客户端 serde 遇未知变体会 decode 失败断流），该连接仍可从快照 `terminal_sessions` 的 `state` 获知终态；`host_error_to_protocol` 把宿主 `not_found` 映射为既有 `RequestNotFound` 码（其余维持 Internal），ADR-045 的幂等边界在 wire 上可观察 |
| `src/gui_host/mod.rs` | ~950 | `GuiHostAdapter`：实现 `GuiHost`；`QUERY_HANDLERS`/`COMMAND_HANDLERS` 静态分发表（与 protocol registry `gui.available` 双射，SET-2 起六个 Settings 入口，SET-6a 再增 `general_settings` / `set_proxy_url`，SET-6b 再增 `permissions_settings` / `set_approval_mode` / `workspace_trust`、SET-6c 再增 `mcp_test` / `mcp_server_remove`、SET-6 终端页再增 `terminal_settings` / `set_terminal_settings`（ADR-050）、ADR-052 再增 `set_provider_use_proxy`）、幂等 wrap（scope 隔离 + begin/record）、snapshot 组装（含重启后 pending approvals 重建，Workspaces 段输出 v14 注册表全集合）、timeline 分页（limit 默认 200、clamp 1..=500，游标跨未投影事件推进）；SET-2 `auth_flights` 按 provider_id 单飞守卫（auth_start / auth_set_api_key / auth_cancel 共用，Arc 身份防误删他人 flight） |
| `src/gui_host/bus.rs` | ~315 | `GuiEventBus`（内部 `EventHub` 赋全局序 + replay；engine 终态上流时登记 run_id，供宿主合成终态兜底去重；`publish_raw` 合成事件序号从 `SYNTHETIC_SEQUENCE_BASE`=2^60 递增自取，不占真实持久化号段且排在既有时间线内容之后）、`GuiBroadcastSink`（AgentEvent→AppEvent 映射后广播）、`publish_provider_auth`（SET-2：Global 流广播 `AuthChanged`，`EventSource::Provider`，hub 重写全局序）、`GuiRunRegistry`（活跃 GUI run 与 `CancellationToken` 登记） |
| `src/gui_host/events.rs` | ~190 | `AgentEventEnvelope`→`AppEvent` 投影助手、诊断事件映射、幂等 client scope 推导 |
| `src/gui_host/handlers/mod.rs` | ~10 | handler 子模块声明 |
| `src/gui_host/handlers/mcp.rs` | ~230 | SET-6c / ADR-049：`mcp_test`（复用 `AppCore::mcp_test`，未知 server `unknown_mcp_server`）与 `mcp_server_remove`（定序：合并配置校验存在且跨层唯一（同名 server 亦定义于非 Global 层即 Error fail-closed）→ `write_mcp_server_remove` Global 原子写 → `pawork.mcp.<name>` SecretRef 幂等清理（非该命名空间 fail-closed）→ 内存同步（shutdown slot client → 删 slot → 重建 registry）；写盘成功后清密失败仍同步内存，再以 `secret_cleanup` Error 回执；回执 `Data({servers:[...]})` 同 mcp_list 形状） |
| `src/gui_host/handlers/terminal.rs` | ~565 | TerminalCreate/Write/Resize/Close：经 `PtyService`；`terminal_create` 过 PolicyEngine（capability=Process；NeverAsk/ReadOnly 直拒，AskUser fail-closed 落 Deny）；ADR-050 D4：构造 `PtyCreateSpec` 处读生效配置 `[terminal]` 填 `spec.shell` 与 `spec.size`（未设回落 exec 平台默认 80×24、pixel 0；只影响之后创建的终端），策略闸 `classification_shell(spec.shell)` 自动跟随、gate 语义不变；`resolve_terminal_cwd` 按注册表严格解析目标 workspace（未登记 fail-closed）并保留 workspace 相对 cwd 记账（注册表值编码 owner+cwd，字段声明在 gui_host/mod.rs 不动；根目录 canonical 空串经 `terminal_cwd_label` 归一为 `"."`，防面板 cwd 空白）；`terminal_snapshots` 回报该相对 `cwd` 键（SnapshotSection.data 为不透明 JSON，非 golden 帧；记账缺失省略键，Desktop 显示 unknown）；输出经事件广播，需 terminal-streaming capability。ADR-045：`terminal_close` 经 `PtyService::cleanup` 终止进程组并移除 PTY service 条目，再由 `forget_terminal` 从 GuiHost 注册表注销（快照节不再出现），未知/重复 id 报 `not_found`；forwarder 是终态事件唯一广播点——`PtyEvent::Exit` 自带 waiter 已写入的权威 `state`，即使 cleanup 已移除 service map 条目仍无竞态地区分 `Killed`/`Exited` 并携带真实 `exit_code`/`signal`；转发链路 `Err(_)` 广播 `Failed`（不臆造退出码），close 路径自身不广播（与自发 Exit 天然去重） |
| `src/gui_host/handlers/run_start.rs` | ~290 | RunStart：provider/model 切换校验（未知 fail-closed）→ 按 session 归属 workspace `expand_at_refs` 展开 → 登记 `ActiveGuiRun`（快照 workspace id 与 roots）→ spawn `chat_turn`；模型切换发诊断事件；engine 未报终态即死时才补发合成 `RunChanged{Failed}` + `run.failed` |
| `src/gui_host/handlers/query.rs` | ~240 | WorkspaceList（v14 持久注册表全集合，含 roots）/SessionGet/ModelList（聚合 overview）/RunStatus/DiffListFiles/DiffGet（按目标 workspace 的最新会话解析，该 workspace 无会话时返回空 files；latest session 已解析时，路径缺失的空结果仍携 `session_id`，供客户端 fail-closed 判 scope）/QuotaOverview/McpList（`{"servers":[...]}`） |
| `src/gui_host/handlers/session.rs` | ~120 | SessionCreate（建会话 + 绑当前 workspace）/SessionOpen/SessionFork（自指定事件建分支并切换 active branch；无绑定会话诚实 Unassigned，不回退当前 workspace） |
| `src/gui_host/handlers/approval.rs` | ~90 | ToolApprove：协议决定→domain 决定；live 决议 pending，非 live 走 session store 落 queued 决议、落库成功后经 `GuiBroadcastSink` 补广播（仅 `ToolCompleted` 上 wire）；写工具附预览 |
| `src/gui_host/handlers/command.rs` | ~40 | WorkspaceAdd（持久幂等登记入 v14 注册表，同 canonical root 复用 stable id）、RunCancel（翻转注册的 `CancellationToken`） |
| `src/gui_host/handlers/settings/` | 见下行 | SET-2/SET-6a/SET-6b/SET-6 终端页 Host Settings 门面（ADR-046/047/048/050）。查询与完整回执经 protocol `Data` 结构 `serde_json::to_value`（`GeneralSettingsData` / `TerminalSettingsData` / `PermissionsSettingsData` / `ProviderAuthStatusData` / `AuthStartData` / `DefaultModelPair`）；无独立 DTO 的命令回执仍为窄 JSON（`set_approval_mode` 的 `approval_mode` 值为 `ApprovalModeWire`）。明文 key 只在 handler 内存短暂停留，事件/响应仅携带 masked_credential |
| `src/gui_host/handlers/settings/mod.rs` | ~150 | `AuthFlight` 单飞守卫、ISO-8601 helper、`settings_data`、`ApprovalModeWire` ↔ `ApprovalMode`（不再有私有 `approval_mode_wire` 字符串表） |
| `src/gui_host/handlers/settings/catalog.rs` | ~260 | `provider_auth_status`：通道 descriptor 从 `CHANNEL_REGISTRY` 派生 display_name / endpoint_label / auth_methods（八通道）；auth 四态 none/connecting/connected{method,masked}/error，connecting 由 auth_flights 推导，env 命中报 connected 且 `masked_credential: null`；SET-4 A3 按 auth_methods 数据判定（声明 api_key 先查 api key 再查 OAuth meta）；catalog 三态 remote/fixed_fallback/unavailable 经 `models_overview` + `join_all` 并行探测，4s 超时，无持久缓存；SET-5 顶层 `default` 为 `DefaultModelPair` 或 null；`providers[].use_proxy` 输出生效值（未显式 `false` 即 `true`）。`set_provider_use_proxy`（ADR-052）：provider 已知（第一方通道或 config 登记）→ `write_provider_use_proxy` 写 Global 配置 → 短写锁 `AppCore::set_provider_use_proxy`，回执 `ProviderUseProxyData`；未知 provider fail-closed `unknown_provider`，Global 配置目录不可得 `config_unavailable`。`set_default_model`：provider 已知且 model 属可运行目录→`write_default_model_pair`→短写锁 `AppCore::set_default_model_pair` |
| `src/gui_host/handlers/settings/auth.rs` | ~330 | `auth_set_api_key`（trim→`verify_api_key` 10s→`store_default_api_key` 原子替换；声明 oauth 的通道移除旧 OAuth，删除失败 fail-closed；验证失败 Failed 且旧凭证保留）、`auth_start`（仅 oauth；Device Flow 回 `AuthStartData`；后台 `oauth_finish` 与 CancellationToken select）、`auth_cancel`（仅 OAuth 等待可取消；api_key 验证拒绝取消，D3）、`auth_remove`（按 auth_methods 清理；env 命中仍清已存条目，无可删项提示 unset） |
| `src/gui_host/handlers/settings/general.rs` | ~65 | Desktop Network 页沿用兼容 wire `general_settings` / `set_proxy_url`：`GeneralSettingsData`。校验用 `http_from_config` 预构 client；非法 URL `invalid_proxy_url`（文案只带解析类别，不含原文 URL）；代理原子写入 workspace 外的用户 Global `config.toml`，写锁内 `set_proxy_url` 禁止 `merge_with` |
| `src/gui_host/handlers/settings/permissions.rs` | ~90 | `permissions_settings` 输出 `PermissionsSettingsData`。`set_approval_mode`：`ApprovalModeWire::from_str`（不收 kebab/`on_failure`），未知值 `invalid_approval_mode` 保旧；ADR-053 先原子写 Global 配置再更新内存。`workspace_trust`：id 必须匹配 attached workspace，同一把写锁内校验 attached id、解析 canonical 根路径、写 Global `workspace_trust`、更新内存 |
| `src/gui_host/handlers/settings/terminal.rs` | ~100 | `terminal_settings` / `set_terminal_settings`：`TerminalSettingsData`。shell trim 非空且可解析、columns/rows ∈ 2..=1000，非法 `invalid_terminal_settings` 保旧；`shell: null` 清回平台默认；未设尺寸回落 `PtyWindowSize::default()` = 80×24 |
| `src/gui_host/tests/` | ~4000 | `cfg(test)` 原 `tests.rs` 按域拆为 `mod.rs` + `run` / `session` / `approval` / `idempotency` / `terminal` / `settings`（约 60 条）：双射 pin、timeline 分页、`@` 展开三态、幂等、审批三态与重启后广播收口、合成终态闸门、fork、provider 切换、bus lagged、ADR-045 `terminal_close`、SET-2/4/5/6 settings（含脱敏、xAI 双认证、proxy/approval/terminal fail-closed） |
| `tests/smoke.rs` | ~110 | env 门控真实 API 冒烟（`--ignored`），不进默认测试路径（`live-smoke` feature 显式启用） |
| `tests/timeline_projection_host.rs` | ~160 | host `timeline()` 与 protocol 投影 golden 对拍 |
| `tests/gui_server/session.rs` | ~1000 | 具名 test bin `gui_server_session`：握手/版本/capability/resume/心跳/慢消费 |
| `tests/gui_server/multi_gui_runtime.rs` | ~830 | 具名 test bin `gui_server_multi_gui_runtime`：多 GUI 一致性/重连 replay/慢客户端隔离 |

ADR-053 启动：显式 `AppLoadOptions.approval_mode` > Global 审批 > ReadOnly；显式 `trust_workspaces` 仅当次进程，缺省为当前根路径选择 > Global 全项目信任默认。`set_approval_host` 只替换交互入口，GUI/CLI 装配不再把已解析 trust 伪装成显式启动覆盖。Settings 修改当前信任时清除当次启动 trust 覆盖，以保存选择为准；其他项目回到各自配置。Host 持写锁跨写盘和内存更新，失败不改变 scheduler。现有 Settings 测试扩充为落盘/重载、项目隔离、未知模式拒绝与损坏 Global 文件保旧。

## 3. 对外 API 面

### 3.1 装配与生命周期

- `AppLoadOptions { workspace_root, provider, model, data_dir, approval_mode, trust_workspaces, approval_host, auth_backend, instance }`；`AppLoadOptions::from_cli(provider, model)` 是 CLI 覆盖入口。`trust_workspaces: Option<bool>` 只供可信宿主显式覆盖本进程，`None` 沿用已解析全局配置；不写回配置。`auth_backend` 缺省为 `FileBackend`（auth 文件），测试可注入 `MemoryBackend`。
- `AppCore::load(options) -> Result<AppCore, AppError>`：发现配置（Builtin → Global → Workspace → CLI 覆盖）→ 装配默认 provider（缺凭证即 `AppError::Auth` 失败）→ 打开 `session.db`、checkpoint/artifact/protected 存储与控制面。
- `AppCore::load_for_catalog(options)`：同路径但缺凭证**不失败**，退回 `CatalogOnlyProvider`——目录、凭证、导入等命令可用；chat 在请求时报 `ProviderErrorKind::Authentication`（fail-closed，错误文案引导 `pawork auth set-key/login <id>`）。`provider_pending() -> bool` 暴露该状态。
- `load_from` / `from_resolved` / `from_config`：跳过发现、注入已解析配置的低层装配入口（cli 测试与特殊装配路径用）。
- `from_parts(provider, credential, model, provider_id, store)`：用注入件直接拼 Core，测试与 smoke 专用；测试内部另有 `from_parts_with_protocol` 可再注入 `AdapterProtocol` 与 `ModelRegistry`。
- 装配后增量开口（`&mut self`）：
  - `attach_workspace(root)`：以目录 basename 作为可见名称，通过 `WorkspaceService` 注册并保存 canonical roots，再重建内建工具注册表；空库首个 workspace 固定 `ws-default`，其后由 `allocate_workspace_id` 发号（ADR-044）；
  - `open_store(path)`：打开会话库跑迁移后读 v14 项目注册表、对 legacy 启动目录补登记（注册表为空时固定 `ws-default`）、按注册表重建 WorkspaceService 与内建工具并预载全部 session 归属绑定；`open_checkpoints(root)` / `open_control_plane(dir)`：分别打开检查点服务、usage/quota/audit 运行时；
  - `register_workspace(root) -> WorkspaceRecord`：持久幂等登记（同 canonical root 复用既有 stable id，同 id 异 root fail-closed），GUI `workspace_add` 入口；
  - `configure_approval(mode, trusted)`：设置审批模式与 workspace 信任并重建 `ToolScheduler` 配置（启动装配专用）；
  - `set_approval_mode(mode)` / `set_workspace_trusted(root, trusted)`（ADR-053，pub(crate)）：Host handler 写盘成功后更新配置内存与 scheduler 快照；进行中 run 不变。`workspace_trusted_for_roots` 按实际目标根路径读取逐项目信任，Run/Terminal 不借用其他项目信任；
  - `prime_extensions()`：file-index 扫描（失败仅 warn）+ MCP auto-start（失败不拖垮装配）。
- `shutdown(self) -> Result<(), AppError>`：关停 MCP 客户端、落 tasks 快照、关闭 store；消费 self。
- 只读访问器：`provider_id()` / `model()` / `adapter_protocol()` / `config()` / `auth_backend()` / `store()`（无 store 时 `Err`）/ `workspace_id()` / `workspace_name()` / `workspace_trusted()` / `approval_mode()` / `approval_host()` / `tool_names()` / `turn_context()`；workspace 注册表查询 `registered_workspaces()` / `workspace_by_id()` / `workspace_for_session()` / `latest_session_for_workspace()`。
- `AppError`：30+ 变体的错误汇聚层，粗分为透传（Config/Auth/Provider/Engine/Session/Io/Workspace/Tools/Git/Mcp/Checkpoint/…，`#[from]`）与本包语义（MissingDefaultProvider/MissingCredential/OAuthLoginRequired/UnknownModel/StoreNotOpen/AmbiguousSession/EmptyTurn/PlanNotApproved/…）两类；`Display` 文案面向终端用户（含修复指引），任何变体不携带明文 secret。

### 3.2 会话与运行

- `create_session(title) -> SessionId`：以当前 workspace 归属建会话；`create_session_with_workspace(title, workspace_id)` 显式指定归属（ADR-043 起与 session/main 分支同事务持久化，跨 Host 重启保留；历史无绑定会话仍归 Unassigned）。
- `list_sessions() -> Vec<SessionRecord>`；`get_session(&SessionId) -> SessionRecord`（含 `active_branch` 与可空 `workspace_id`）。
- `resolve_session(spec)`：接受 id 前缀或列表序号，歧义/未命中报 `AppError`。
- `next_sequence(&SessionId) -> u64`：该会话下一事件序号（宿主外持久化事件时用）。
- `bind_session_workspace`（仅进程内缓存，devfixture 用）/ `session_workspace` / `session_workspace_for_record`：会话 ↔ workspace 映射维护与查询；持久化绑定在开库时全量读取并替换缓存，重复开库不泄漏旧绑定；NULL 归 Unassigned，尚未登记的 canonical workspace id 原样保留。
- `resume_messages(&SessionId) -> Vec<Message>`：**CLI resume 语义**——重放 active branch 构造消息序列，孤儿待审批工具调用持久 seal 为 Denied。
- `resume_messages_keep_pending(&SessionId)`：**GUI resume 语义**——同重放但保留待审批，另返回 pending 列表供 snapshot 重建审批卡片。
- `chat_turn(session, messages, sink, cancel) -> ModelResponseSummary`：单轮执行（详见 §4.2）；`sink` 是调用方渲染 sink，宿主内部自动套 `PersistThenRender`，调用方**不要**自己先落库。`chat_turn_with_run_id` 允许外部指定 `RunId`（GUI 路径用，保证响应与事件可关联）。
- `compact_session(session_id, sink)`：手动压缩历史（§4.5）。
- `session_title_from_text(text) -> String`：自由函数，从首条用户文本派生会话标题。

### 3.3 模型、凭证与通道

- `list_models()`：当前 provider 实时列举（走网络，缺凭证时经 `CatalogOnlyProvider` 返回静态目录）。
- `model_catalog()` / `models_overview()` / `provider_models()`：聚合 `CatalogEntry`（builtin 目录 + config 覆盖 + 运行期探测缓存），不发网络请求，GUI ModelList 即消费 overview。
- `switch_model(model)`：同 registry 内切换默认 model；未知 id fail-closed 报错，不静默沿用。
- `switch_provider(provider, model)`：重装配 provider（重新走凭证链与协议解析），成功后发 ModelSwitched 诊断事件；带 provider 而 model 未指明时不得静默保留旧 model id。
- `auth_status() -> Vec<AuthChannelStatus>`：逐通道报 `AuthSource`{File/Env/None}，永不回显 key 本体。
- `auth_set_key(provider_id, key)` / `auth_logout(provider_id)`：写/删 auth 文件后端。
- `oauth_begin(provider_id) -> OAuthLogin` / `oauth_complete(...)`：PKCE 与 Device Flow 两形态编排；token 落 auth 后端，不回显。
- 通道 facade：`FIRST_PARTY_CHANNELS` / `first_party_channel(id)` / `is_first_party(id)` / `ChannelKind`；事实源是 providers `CHANNEL_REGISTRY`，本包不自建通道表；`oauth_override` 允许配置覆盖默认 OAuth preset。

### 3.4 usage、diff、checkpoint

- `usage_overview(window)` → `UsageOverview`（含 `LedgerTotals` 与 `QuotaWindowLine`）；`session_usage(&SessionId) -> TokenUsage`；`last_run_usage`；`estimate_cost_for(model, usage) -> Option<Cost>`（无定价数据返回 None）。
- `ControlPlaneRuntime::in_memory()` / `persistent(dir)`：usage/quota/audit 运行时装配（cli 直接消费 re-export 的报表行类型）。
- `session_diff(&SessionId) -> SessionDiff`：git 仓走 `pawork-git` 全量 diff 后按 session 改动路径过滤（含 rename 前路径、untracked 补 hunk）；工作区根取 session 归属 workspace，不使用进程当前 workspace；非 git / 无 git 二进制回退写前快照对比。`GitDiffHeader`（branch/work_dir/dirty_files）仅 git 路径返回。
- `paginate_diff(files, page, page_size) -> DiffPage`；`render_session_diff` / `render_diff_file`：统一 diff 文本渲染（二进制标注 `Binary files differ`）。
- `git_status_note(roots) -> Option<String>`：短 git 状态行（branch + dirty 数），任何失败返回 None 不阻断对话。
- `list_checkpoints(&SessionId) -> Vec<CheckpointSummary>`；`rollback(&SessionId, spec) -> RollbackOutcome`：按快照恢复文件并持久化 `CheckpointRolledBack` 事件。

### 3.5 扩展、MCP、导入、tasks/plan/编排

- `expand_at_refs(session_id, text) -> Vec<ContentPart>`（async，ADR-044 起按 session 归属 workspace 的 file-index 路由）：`@token` 命中时正文作为**独立** Text part 追加（不拼进 user text），单文件 64 KiB 截断并标记；无 `@` 时返回单 Text part。`complete_at(query, limit)` 补全候选（未索引 workspace 自动先扫描）。`workspace_root()`。
- `mcp_list() -> Vec<McpServerStatus>`（name/transport/state/tools/last_error）；`mcp_test(name?)`：真实建连 + ping + list_tools 并刷新 slot 状态；SET-6c 增 `remove_mcp_server(name)`（同会话生效：写盘后同步 extra、shutdown slot、删 slot、按既有 descriptors 重建 registry）与 `mcp_server_secrets_for_removal`（纯函数：收集本 server 的 `pawork.mcp.*` SecretRef，非该命名空间 Err，其它 server 跳过）。
- `scan_local_sessions(source, home_root?)`：只读发现本机会话文件。
- `preview_compat_import(tool, global_root?)` / `apply_compat_import(...)`：compat 配置导入两段式；apply 前用文件指纹快照校验 `sources_unchanged`，落盘 instructions / skills / MCP merge / profiles。`CompatTool::parse` 接受 claude/codex/grok/cursor/pi。
- `export_session_doc(spec?)` / `import_session_file(path, format, source?)`：会话文档导出/导入；`SessionImportFormat`{Export/Compat/Pi}，`parse_session_source` 解析来源名。
- `tasks_list` / `tasks_status(spec)` / `tasks_register(kind)` / `tasks_cancel(spec)` + `parse_task_kind`（agent/automation/monitor/process）。
- `plan_snapshot` / `plan_create` / `plan_replace` / `plan_submit` / `plan_approve` / `plan_reject` + `review_status_label`：Plan 事件持久化在会话事件流内，approve/reject 落 audit。
- `run_multi_agent_demo(options) -> MultiAgentDemoReport`：S11 演示（spawn 双 worker / cancel-tree / budget-gate），非通用编排 API。

### 3.6 GUI 宿主与复用组件

- `gui_server`（唯一 `pub mod`）：
  - trait `GuiHost`：`snapshot()` / `timeline(session, cursor, limit)` / `query(envelope)` / `command(envelope)`，是 cli 与测试注入宿主的接口；
  - `GuiServer::bind(config, host, transport)` + accept 循环，每连接 spawn 独立会话任务；`GuiServerConfig`；
  - `ConnectionManager` / `GuiSubscription` / `ManagerError`；常量 `DEFAULT_HEARTBEAT_TIMEOUT`（30s）与 `DEFAULT_QUEUE_CAPACITY`（1024）。
- `GuiHostAdapter::new(Arc<AppCore>)`：生产 `GuiHost` 实现；配套 `GuiEventBus`、`GuiBroadcastSink`、`GuiRunRegistry`、`project_timeline_item`。
- `EventHub` / `HubSubscription` / `HubError` / `DEFAULT_HUB_CAPACITY`（4096）：全局序 + ring + broadcast 的通用事件扇出。
- `IdempotencyStore` / `IdempotencyCheck`{New/Replay/InFlight} / `IdempotencyError` / `IdempotencyStats` / `should_cache` / `DEFAULT_IDEMPOTENCY_CAPACITY`（= storage `DEFAULT_COMMAND_LEDGER_CAPACITY`）。
- `PersistThenRender { store, render, branch_id }`：persist-first 组合子；`branch_id` 必须是 active branch。
- 审批宿主家族：`ApprovalPromptHost` trait / `GuiApprovalHost` / `DenyAllApprovals` / `PendingToolApproval` / `ApprovalAsk` / `ApprovalResolve` / `parse_approval_mode`。
- data_dir 家族：`default_data_dir(_outcome)`、`consume_data_dir_outcome`、`normalize_instance`、`instance_dir`、`session_db_path(_for)`、`artifact_store_path(_for)`、`protected_store_path_for`、`usage_ledger_path_for`、`audit_log_path_for`、`tasks_snapshot_path_for`、`DEFAULT_INSTANCE`（`"default"`）。
- 便捷 re-export：`AdapterProtocol`/`ProtocolError`；`ApprovalMode`/`RiskLevel`（policy）；`SessionRecord`/`SessionExport`/`EXPORT_SCHEMA_VERSION`（storage）；`PlanSnapshot`/`TaskSnapshot`（workflow）；`DiffFile`/`DiffPage`（git）；`CompatExternalSource`/`LocalSessionFile`/`LocalSessionSource`（workspace）。

## 4. 核心行为与数据流

### 4.1 GUI RunStart 全流程

1. GUI 帧到达 `gui_server::session` 帧循环。首帧必须是握手：做协议版本检查，返回 `HandshakeResponse` + 初始 `Snapshot`；非握手首帧直接拒绝并关闭连接。
2. 后续 command 帧被盖上 client 戳（连接身份）并做版本校验；client_context 替换尝试被拒绝。
3. capability 门：所需 capability 未在握手授予的 query/command 在进宿主**之前**被拒（terminal-streaming 相关的 snapshot 分区、事件、命令全路径同规则）。
4. 帧进入 `GuiHostAdapter::command`：由 envelope 推导幂等 scope（不同 GUI client 的相同 `command_id` 不冲突），`IdempotencyStore::begin` 判定三态——
   - `Replay`：直接返回缓存响应，副作用不重放；
   - `InFlight`：有界等待首个执行者（`Notify` + 超时轮询），唤醒丢失也收敛到 SQLite 权威结果；
   - `New`：领到执行权，继续。
5. `COMMAND_HANDLERS` 静态表分发到 `handlers/run_start.rs`：
   - 若请求携带 provider/model 切换，先经 `provider_assembly` 校验并装配——请求的通道直连装配（不是 catalog 顺位），未知 model fail-closed，带 provider 不得静默沿用旧 model id；切换成功发 ModelSwitched 诊断事件；
   - `expand_at_refs` 展开 `@file` 为独立 ContentPart；
   - **展开成功后才**在 `GuiRunRegistry` 登记 `ActiveGuiRun`（含 `CancellationToken`），任何前置失败不留幽灵 run。
6. spawn 异步任务执行 `RunService::chat_turn`（§4.2），响应帧先行返回 run 受理；run 事件在宿主内经 `PersistThenRender` 先落库，再交 `GuiBroadcastSink` 映射为 `AppEvent` 发进 `GuiEventBus`。
7. `EventHub` 为每条事件赋连续 `global_sequence` 并写 ring buffer；`ConnectionManager` 将事件推入各订阅连接的有界 mpsc 队列；订阅 GUI 收到 Event 帧。
8. 慢客户端队满被标记 lagged 并丢新事件——发布者与其它 GUI 不被阻塞；lagged 连接随后收到 ReplayUnavailable，走 snapshot 恢复（§4.3）。
9. Run 收尾终态闸门（P2 片 2A 起 persist-first）：fail/cancel 路径 engine 在 Err 返回前已经 sink 广播真实终态（`RunChanged{Failed}` / `RunChanged{Cancelled}`），宿主据 `GuiEventBus` 的终态登记（`terminal_reported`）去重，不再补发；只在 engine 未报终态即死（plan 闸门拒绝、宿主侧早退等）时由 `seal_run_without_terminal` 收口——**先 best-effort 持久化真实 `RunFailed`**（`ErrorCategory::Internal`），成功后经 `GuiBroadcastSink` 正常映射补广播（携带真实持久化 sequence）；持久化失败（如该 run 尚无任何已落库事件）才退回 `publish_raw` 合成兜底——合成 `stream_sequence` 从 `SYNTHETIC_SEQUENCE_BASE`（2^60）递增自取：真实持久化 sequence 从 1 单调递增不会到达该段，既不触发 reducer seen 去重吞掉真实事件，也让合成条目有序插入落在既有时间线内容（含用户消息乐观回显）之后（seq-0 旧行为曾把合成 "Run failed" 插到时间线顶端，R4 Wave B 评审 P2 修复）；语义仍是宿主兜底而非 engine 事实。两条路径都补发 `run.failed` 诊断供 GUI 展示原因。随后 `GuiRunRegistry` 摘除该 run 并清理终态登记（防无界增长）；成功响应按 `should_cache` 写回幂等 ledger；record 失败计数并释放 inflight（同 `command_id` 可重入），不吞错挂死。

### 4.2 CLI chat_turn 单轮

1. 调用方（cli REPL / exec）持 `Vec<Message>` 调 `AppCore::chat_turn(session, messages, sink, cancel)`。
2. Plan gate：`ensure_plan_allows_execution`——会话存在未批准 Plan 版本则拒（`AppError::PlanNotApproved`，audit 记 Deny）；无 Plan 或已批准放行。事件重放失败（含 StoreNotOpen）原样上抛，不得吞成 Ok 后继续执行。
3. quota 预检：`projected_run_usage` 估算本轮输入预算并询问 `QuotaService`，超限直接拒绝，不发请求。
4. 装配 `TurnContext`：system prompt、注入层（instructions / skills / profiles / AGENTS 文件，经 `load_injected_layers`）、工具定义、`git_status_note` 短状态行（任何 git 失败静默省略，不阻断）。
5. 进入 `pawork_engine::run_session`。`SessionLoopCtx` 作为 `LoopContext` 提供：
   - 审批：转 `ApprovalPromptHost`（CLI 为终端 ask，GUI 为 `GuiApprovalHost`）；
   - 工具执行：经 `ToolScheduler`（并发上限 8，Policy/审批模式约束），写工具执行前先落 checkpoint 快照；
   - 压缩：超阈值时 fork recovery branch 并产快照；
   - id 发号与事件 emit。
6. 全部 agent 事件先 `append_event`（active branch）成功再进调用方渲染 sink（persist-first）。
7. 收尾：`record_completed_usage` 把 `TokenUsage` 写 `usage-ledger.sqlite3`，`record_id` 用哨兵 `"rec-<run_id>"`（单机形态无上游账号，tenant/principal 亦为默认哨兵，`upstream_attempt = 1`）；返回 `ModelResponseSummary`。

### 4.3 断线 resume / Replay / SnapshotRequired

1. GUI 重连并重新握手后发 Resume（带 last seen `global_sequence` 与 ack 状态）。
2. 宿主对照 `EventHub` 分三态：
   - 已最新 → up-to-date，无补发；
   - 缺口仍在 ring 内 → 逐条补发缺失事件（replay）；
   - 缺口越界（`HubError::ReplayUnavailable`）→ 回 SnapshotRequired，客户端改拉全量 `snapshot()`。
3. snapshot 含会话列表、活跃 run、以及重启后由 waiting 投影重建的 pending approvals（`snapshot_rebuilds_pending_approvals_after_restart` 钉住）。
4. 连接存续期间队列 lagged 同样收敛到 ReplayUnavailable 帧，客户端按 snapshot 路径恢复。
5. 心跳：客户端周期发 Heartbeat 得 Pong；超过 `DEFAULT_HEARTBEAT_TIMEOUT`（30s）无活动由 `ConnectionManager` 清理连接。
6. **断连与心跳超时都不取消 run**——run 继续执行并持续落库，取消只能来自显式 RunCancel 命令（翻转登记的 `CancellationToken`）。

### 4.4 审批等待与恢复（K-02）

1. live 路径：engine 发 `ToolApprovalRequested` → `SessionLoopCtx` 挂起等待决议 → `GuiApprovalHost` 在同一把锁内查 queued / 插 pending 并广播审批事件（先到的 ToolApprove 入队，后到的 decide 立即消费，避免双锁窗口挂死）。
2. GUI 发 ToolApprove 命令（协议 `ApprovalDecision` 译为 domain 决定；写工具附 `preview_for_tool` 生成的预览）→ live 决议唤醒等待中的 run，放行或拒绝该工具。live 等待期间**不**做持久 seal（决议由 run 自身事件流落库，避免双写）。
3. 非 live（进程重启、run 不在内存）：
   - session 有 waiting 投影 → ToolApprove 决议持久化落库（durable seal），下次 resume 可见；落库成功后经 `GuiBroadcastSink` 逐事件补广播（persist-first），`broadcast_event` 过滤后仅 `ToolExecutionCompleted` 映射 `AppEvent::ToolCompleted`（success=false）上 wire，GUI 借此清 pending 并把 tool 行显示为 failed；`ToolApprovalResponded`/`MessageCommitted` 仍不进实时流，wire 契约不变（`tool_approve_non_live_waiting_broadcasts_tool_completed` 钉住）；
   - 无 waiting 投影 → 决议进 queued 池，等待 run 恢复时消费，不落库。
4. resume 分叉：CLI `resume_messages` 把孤儿待审批 seal 为 Denied（持久化）；GUI `resume_messages_keep_pending` 保留 pending 并随 snapshot 重建审批卡片，用户可继续决议。

### 4.5 compact_session 手动压缩

1. 校验会话存在且有可压缩历史；fork recovery branch 保留原始完整历史（可回溯）。
2. 生成摘要快照，token 统计经 `SessionTokenEstimatorBridge`（engine `HeuristicEstimator` 桥接到 storage 窄口 `TokenEstimator`）。
3. emit 并持久化 `CompactionStarted` / `CompactionCompleted`；active branch 切到压缩后分支。
4. 后续 resume / fork 直接消费 storage lineage：fork 后 resume 只能看到祖先前缀；压缩过程中的存储错误显式上抛，禁止降级为"未发生"。

### 4.6 启动清扫：悬空 run 诚实收口（P2 片 2A）

宿主进程在终态前结束（崩溃 / sink 持久化失败）会在存储留下停在 `running` 的 run，重放侧永不闭合。`open_store` 装配末尾执行 `seal_interrupted_runs`：

1. 遍历全部 session 的 projection，筛出 `runs.state == 'running'` 的 run。
2. 对该 run 的悬空 tool call（非 `completed` 且非 `waiting_for_approval`）逐个追加持久化 `ToolExecutionCompleted{is_error:true, "run interrupted before completion"}`；`waiting_for_approval` 的调用不动——pending 重建只查 `tool_calls` 状态、与 runs 表无关，审批仍可经非 live durable seal 决议。
3. 追加 `RunFailed{ErrorCategory::Internal, "host process ended before the run reached a terminal state"}`——含 waiting 审批的 run 同样收口（run 已是孤儿，审批决议只是后续清理）。
4. 幂等：收口后状态不再是 `running`，重复清扫自然早退；单 session 失败只 warn 后继续，不阻断启动。

前提：单个库只由一个宿主进程写入（默认拓扑 Desktop 与 CLI 各用独立 instance、各持独立库）。若两进程共享同一库，后启动者的清扫会把先启动者仍活跃的 run 收口为 failed，先启动者的下一次落库将因序号不连续显式报错——这是未支持的拓扑，不在清扫的存活判定范围内。

以上流程的跨包端到端视角（CLI ↔ GUI ↔ engine ↔ storage）见 [../flows.md](../flows.md)。

## 5. 契约与不变量

- **wire 契约不在本包**：GUI 帧、`AppCommand`/`AppQuery`/`AppEvent`、timeline 投影的 golden 全部钉在 [pawork-protocol](protocol.md)；本包 host 侧以 `tests/timeline_projection_host.rs` 与 protocol 投影 golden 对拍，保证分页路径与 reducer 历史臂同源。
- **分发表双射 pin**：`QUERY_HANDLERS`/`COMMAND_HANDLERS` 与 protocol registry `gui.available=true` 条目一一对应，由内嵌测试 `dispatch_tables_match_gui_available_registry_entries` 钉死；新增 GUI 能力必须两侧同批。
- **persist-first**：所有 agent 事件先 `append_event` 成功再渲染/广播（`PersistThenRender`）；`branch_id` 必须是 active branch。事件可持久化可重放是架构红线。
- **幂等权威在 SQLite**：`CommandLedger` CAS 是唯一权威；重复 `command_id` 返回缓存响应不重放副作用；key 冲突拒绝；`record` 失败必须计数并释放 inflight。幂等状态在进程重启后存活。
- **EventHub 序列连续**：发布时重写 `global_sequence` 保证连续单调；replay 越界必须显式 `ReplayUnavailable`，禁止静默丢段。
- **断连不取消 run**；慢客户端只降级自身（lagged），不得阻塞发布者或其它 GUI。
- **无终态 run 必收口**：任何持久化了 `RunStarted` 的 run 最终必须有持久化终态——进程内由 engine/合成闸（persist-first）保证，进程死亡遗留由启动清扫（§4.6）幂等收口；重放侧不得出现永远 `running` 的 run。
- **Secret 红线**：明文 key 不进 `AppError` 任何变体、不进日志与数据库；`auth_status` 只报来源；smoke 测试禁止打印 key。SET-2 settings 六入口同样只在内存验证路径短暂持有 key——`AuthChanged` 事件与响应只携带 masked_credential，验证失败保留旧凭证且不回显任何明文。
- **`let _` 非测试归零**：非测试代码不允许 `let _ =` 吞结果（当前全包仅 3 处且都在 `cfg(test)`）。
- **HOME 回退单点告警**：只有 `consume_data_dir_outcome` 打一次结构化 warn（`degrade.home_dir_fallback`），路径 helper 保持静默，禁止静默落 temp。
- **usage 哨兵**：单机形态 ledger 记录 `record_id = "rec-<run_id>"`、默认 tenant/principal、`upstream_attempt = 1`，消费方不得把哨兵当真实上游账号。
- **不按 Provider 名称走特例**（红线）：协议选择只读 `extra.provider_protocols` 配置与样例默认表（`protocol.rs` 是配置数据），未知值 fail-closed。
- **`@` 展开 fail-closed**：无 `@` 零行为变化；展开失败整个 run_start 失败且不留 ActiveGuiRun；附件正文独立 ContentPart，不拼进 user text，单文件上限 64 KiB。
- **workspace 注册表幂等**（ADR-044）：`workspace_add`/`register_workspace` 按 canonical root 幂等，同 root 重登复用 stable id，同 id 异 root fail-closed；Run/资源/diff/terminal cwd 按 session 归属 workspace 解析。注册表已有可用项目时，未绑定或未登记 workspace fail-closed，不得回退到当前/最近项目；仅无可用 root 的测试装配允许空的 `ws-unbound`。
- **MCP 信任边界**：untrusted workspace 拒绝 stdio auto-start/test；stdio 走沙箱 + env 卫生；MCP secret 独立文件（不复用 Provider auth 后端）。
- **Workspace 信任覆盖**：`AppLoadOptions::trust_workspaces` 只能由可信启动宿主显式提供，优先于全局配置且只存活于当前进程；workspace 内容不能通过自身配置提升信任，Secret/Policy/路径红线不因该覆盖取消。
- **instance 名白名单**：`[A-Za-z0-9._-]` 且禁 `..`，拒绝路径逃逸。
- **compat import 防 TOCTOU**：apply 前对预览时快照的源文件做字节 + mtime 指纹复核，源已变化则 `sources_unchanged = false` 不落盘。
- **tasks 快照原子写**：`tasks.json` 先写 `.json.tmp` 再 rename；persist 失败发 degrade 事件，不静默吞错。
- **工具调度**：`ToolScheduler` 并发上限 8；审批模式或 workspace trust 变更必须 Arc-swap 重建 scheduler 配置（`configure_approval` / `replace_registry` / SET-6b `set_approval_mode`/`set_workspace_trusted` 负责），禁止运行中就地改 PolicyEngine。

## 6. 依赖关系

**上游（15 个 pawork crate）**：[domain](domain.md)、[engine](engine.md)、[providers](providers.md)（features：anthropic / chatgpt-oauth / xai-oauth / glm-coding / opencode-go / qwen-token-plan / deepseek / kimi-platform / kimi-code，八通道全开）、[auth](auth.md)、[tools](tools.md)、[policy](policy.md)、[workspace](workspace.md)、[exec](exec.md)、[storage](storage.md)（features：compaction / checkpoint / protected）、[git](git.md)、[workflow](workflow.md)、[orchestration](orchestration.md)（`default-features = false`）、[control-plane](control-plane.md)、[protocol](protocol.md)、[transport](transport.md)。三方：tokio、reqwest、toml、blake3、getrandom、tracing、serde 系。

**下游**：生产仅 [pawork-cli](cli.md)（`pawork` 二进制 → cli → app）；[pawork-client](client.md) 以 dev-dependency 使用本包做集成测试。desktop **禁止**依赖本包。

**dev-dependencies**：pawork-testkit（MockProvider/MockScript）、pawork-transport（features local + memory，供 gui_server 集成测试起真实 endpoint）、wiremock、tempfile。

依赖方向与全局分层见 [../../architecture.md](../../architecture.md) 与 [../../design.md](../../design.md) §2。本包处在 `pawork` 二进制依赖闭包的最大层：合并/归档波以 `cargo tree -p pawork` 断言无环且闭包不膨胀，给本包新增上游依赖须先过对应任务书。

## 7. 测试与验证资产

默认验证命令：

```bash
cargo test -p pawork-app --offline --lib --tests
```

UI fixture 属显式 opt-in 验证资产；其集成测试与 example 均声明
`required-features = ["ui-fixture"]`，定向命令为：

```bash
cargo test -p pawork-app --offline --lib --tests --features ui-fixture
```

**内嵌 `#[cfg(test)]`（跑在 `--lib`）**，重点覆盖：

- `gui_host/tests/`（约 60 条，本包最大测试集）：
  - 分发表与 registry `gui.available` 双射 pin（`dispatch_tables_match_gui_available_registry_entries`）；
  - timeline 投影分页与 ModelList 聚合、McpList `{"servers":[...]}` 形状；
  - `run_start` 的 `@` 展开三态：展开成独立 part / 失败不留活跃 run / 无 `@` 单 Text part；二轮带历史；
  - 合成终态闸门：engine fail 恰一条 `RunChanged{Failed}` 无合成重复、cancel 恰一条 `RunChanged{Cancelled}` 不被谎报 Failed、无终态早死（Draft plan 闸门拒绝）兜底 `RunChanged{Failed}` + `run.failed` 诊断、persist-first 成功路径落真实 `RunFailed`（持久化失败才退回 ≥2^60 合成段）；
  - 启动清扫（§4.6）：悬空 run 收口幂等、waiting 审批保持 pending 且可决议闭合、tool 行诚实收口；
  - 幂等：replay 不重放副作用、跨 client 同 `command_id` 不撞、重启存活、record 失败计数不吞错、InFlight 共享 key 不挂死、唤醒丢失有界轮询收敛、失败释放 inflight 可重入；
  - ToolApprove 三态：live waiting 不 durable seal / 非 live 有 waiting 投影 durable / 无 waiting 保持 queued；
  - snapshot 重启后重建 pending approvals；SessionCreate / SessionFork（建分支并切换）；
  - provider/model 切换：请求通道直连不走 catalog 顺位、未知 fail-closed、不静默保留旧 id；
  - SET-2 settings：`auth_set_api_key` 主路径（wiremock 验证 Bearer 命中→原子替换→事件/响应/status 全脱敏）与失败路径（401→`auth_verify` Error、旧凭证保留、Failed 事件无明文）；SET-4 A3 xAI 双认证主路径与替换移除旧 OAuth 各一条；SET-5 `provider_auth_status` 顶层 `default`（已配置→对象取自生效配置而非 core 选中值 / 未配置→null）各一条，`set_default_model` 主路径串联（写盘成功→同会话重查 status `default` 即新 pair，HOME 重定向临时目录）一条；SET-6a `general_settings`/`set_proxy_url`：设置→重查一致、清除→重查 null、非法 URL fail-closed 保旧值且错误文案不含原文（HOME Drop 守卫）各一条；SET-6b 权限与审批：`set_approval_mode`→重查一致（含未知值 `invalid_approval_mode` 保旧）且 scheduler 同步 Arc-swap 到新 mode、`workspace_trust` 匹配 attached id→内存切换并重建 scheduler trust、id 不匹配→`unknown_workspace` fail-closed 保旧（含 scheduler）各一条；SET-6c MCP 定向回归：`mcp_server_remove` 盘/密/内存三处一致与未知 name 三处皆不动 fail-closed、`mcp_test` 未知 name Error 且 mcp_list 不变；SET-6 终端页 ADR-050：`set_terminal_settings` 设置→清除 shell→重查一致、非法 shell/尺寸 fail-closed 保旧、`terminal_create` 应用配置 shell/size 各一条；
  - `GuiRunRegistry` cancel 翻转 token；bus 经 EventHub 发布与 lagged degrade 帧。
- `hub.rs`：ring 超容量逐出、replay 越界 ReplayUnavailable、全局序连续。
- `idempotency.rs`：容量逐出、SQLite CAS 权威、键冲突拒绝。
- `data_dir.rs`：HOME 回退 DegradeEvent 结构、告警单点（helper 静默 / consume 恰好一次 WARN）、instance 白名单拒路径逃逸。
- `protocol.rs`：extra 覆盖 / 样例默认表 / 未知值 fail-closed。
- `testsupport.rs`：`RecordingCapture` 治愈并屏蔽 tracing interest 缓存投毒的回归（探针双 callsite）。
- `services/*`、`loop_ctx.rs`、`checkpoint.rs`、`control.rs`、`protected.rs`、`approval.rs`、`extensions.rs`：各自带定向单测（resume seal 语义、压缩 lineage、usage 哨兵、审批模式解析、`at_tokens` 词法等）。

**tests/（integration）**——`tests/gui_server/` 下两个文件不是自动发现的，经 Cargo.toml `[[test]]` 声明为具名 test bin：

| 文件 | 形态 | 覆盖点 |
| --- | --- | --- |
| `tests/timeline_projection_host.rs` | 默认跑 | 真实 `GuiHostAdapter::timeline()` 与 protocol 投影 golden（`paged_interleave.jsonl`）逐条对拍；limit=0 收敛最小窗口、游标跨未投影事件推进 |
| `tests/ui_fixture_projection.rs` | `--features ui-fixture` | R1 Wave B Phase C：devfixture 把 `fixtures/ui/seed.json` 种到隔离 tempdir 后，经真实装配的 `GuiHostAdapter` `snapshot()`/`timeline()` 断言 3 workspaces、7 sessions、四日期桶分布、pending approval 重建、completed 会话条目构成（user/assistant/tool/approval/run 全量对拍 seed turns）、alpha diff 4 文件含 ≥200 字符长行；断言值取自 seed.json |
| `tests/gui_server/session.rs` | 具名 `[[test]]` `gui_server_session` | 握手往返、非握手首帧拒绝、command 盖戳与版本校验、SessionGet 字段透传、resume 三态与 ack、Heartbeat→Pong、断连不取消 run、lagged→ReplayUnavailable、慢消费不阻塞宿主、client_context 替换拒绝、capability 先于宿主拒绝、terminal-streaming capability 全路径、ADR-045 `TerminalExited` 按协商 minor 门控（1.2 连接跳过且不断流、1.3 连接送达） |
| `tests/gui_server/multi_gui_runtime.rs` | 具名 `[[test]]` `gui_server_multi_gui_runtime` | 三 GUI 收到相同事件序、重连 replay 缺失事件、replay 不可用回退 snapshot、慢客户端不拖累其它 GUI、断连/心跳超时均不触发 RunCancel |
| `tests/smoke.rs` | `live-smoke` feature + env 门控，默认忽略 | 真实 API 流式冒烟（AssistantTextDelta + RunCompleted）；`cargo test -p pawork-app --features live-smoke --test smoke -- --ignored --nocapture`，需 `PAWORK_SMOKE_BASE_URL/API_KEY/MODEL[/PROTOCOL]`，禁止打印 key |
| `examples/ui_fixture.rs` | `--features ui-fixture` dev-only example（非 test bin） | R1 Wave B UI fixture 工具（CLI 冻结）：`seed`（写隔离 root + manifest/ready marker）、`serve`（真实 GuiServer + 按首行前缀分派的 MockProvider；`drop_socket` 可重复触发）、`self-check`（握手+snapshot 校验+RunStart+Resume Replay，每轮先失效旧 `replay_complete`）、`snapshot-dump`（volatile 归一化 + seed 会话过滤）。数据集 `fixtures/ui/seed.json` 与确定性 PTY；验证链路：`seed → serve → self-check → snapshot-dump` |

验证约定总览见 [../verification.md](../verification.md)；degrade tracing 断言一律使用 `testsupport::RecordingCapture`，禁止裸 `tracing::subscriber::set_default`。

## 8. 注意事项与已知限制

- **读法建议**：本仓最大库。改 GUI 行为从 `gui_host/mod.rs` 分发表入手；改 run 语义从 `services/run.rs` + `loop_ctx.rs`；改装配从 `app_core.rs::load_with` + `provider_assembly.rs`；改 Settings Data 形状从 `gui_host/handlers/settings/` + [protocol Settings 载荷](protocol.md)。
- **`CatalogOnlyProvider` 语义**：`load_for_catalog` 下 chat 请求必然失败（Authentication），这是有意 fail-closed，不是缺陷；判断入口是 `provider_pending()`。
- **protected 无本包 feature gate**：任务惯称"feature protected"指 storage 侧 feature，由本包 Cargo.toml 常开；`ProtectedBlobStore` 用 instance 级 `BlobScope`（`instance-reasoning`），是已接受偏差（非按 session 隔离）。
- **`orchestration_host` 是 S11 demo**：固定样例 provider/model id 与固定 session id，仅供 `pawork` demo 命令演示 spawn/cancel-tree/budget-gate，不是通用多 Agent API。
- **`AdapterProtocol::Responses`** 是 ChatGPT/xAI OAuth 通道装配用标记（Kimi Code OAuth 固定 ChatCompletions），不可经 `extra.provider_protocols` 配置（解析表只认 chat_completions/messages，未知值报错）。
- **terminal 审批粒度**：`terminal_create` 在 AskUser 模式 fail-closed 落 Deny（命令级交互审批待 wire ADR，见 ADR-041 D2）；`AskForDangerous` 对默认 shell 返回 constrained allow，NeverAsk/ReadOnly 依 D2 拒绝；会话内容不逐条审批。
- **tracing interest 缓存投毒**：与无 subscriber 测试共享 callsite 的断言测试会间歇丢事件（tracing-core 0.1.36 `Interest::never()` 缓存）；`RecordingCapture::install` 以双注册 Dispatch 治愈并钉住，窗口结束调 `dismiss()`。
- **testsupport 环境写**：`set_env`/`remove_env` 直接写进程环境（unsafe），相关测试串行意识自负。
- **gui_server 集成测试依赖 dev-features**：需要 pawork-transport 的 `local` + `memory`；生产依赖不开这两个 feature。本包仅声明默认关闭的 `ui-fixture` feature，用于 opt-in 编译 devfixture / example / 对应集成测试；providers / storage 的 features 仍由本包 Cargo.toml 固定开启。
- **`mcp_test` 有副作用**：会真实建连并 ping 配置的 MCP server，untrusted workspace 下 stdio 直接报 PermissionDenied。
- **diff 回退路径是行级替换**：非 git 工作区的快照对比生成单 hunk 全量替换 diff（非最小编辑距离），二进制以 NUL 字节嗅探；`session_diff` 的改动集来自 checkpoint 服务，未 `open_checkpoints` 时返回空 diff 而非报错，git 判定只看首个 workspace root。
- **`AppCore` 字段全私有或 `pub(crate)`**：消费方只能走方法门面；`Debug` 输出经筛选（provider_id/model/协议/是否有 store 等），不含凭证本体。
- **内部测试装配件不可外用**：`testsupport` 的 `mock_core` / `ScriptedProvider` / `RecordingCapture` 均 `pub(crate)`；tests/ 集成测试改用 pawork-testkit 的 `MockProvider` / `MockScript`。

相关：产品能力总览 [../capabilities.md](../capabilities.md) · 跨包流程 [../flows.md](../flows.md) · 契约清单 [../contracts.md](../contracts.md) · Spec 索引 [../README.md](../README.md) · 任务状态 [AGENTS.md](../../../AGENTS.md)
