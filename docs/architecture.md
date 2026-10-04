# Pawork 架构

> 架构事实源：红线、包布局与依赖方向、冻结契约、安全语义。功能设计见 [design.md](design.md)；包内细节见 [包级 Spec](spec/README.md)；跨包链路见 [spec/flows.md](spec/flows.md)；Desktop 见 [gui-design.md](gui-design.md)。

---

## 1. 架构红线（不可违反）

- CLI 与 Core 同进程同二进制（`pawork` 是唯一正式宿主），纯 Rust 实现；不引入 Node / Bun / V8 / 嵌入式 JS Runtime；不做 TUI。
- GUI 以独立 GPUI 进程（`apps/desktop`）经 GUI Connection Protocol 连接 CLI，不嵌入 Core、不直接加载 Core crate；GUI 不得直接访问 Provider、数据库与工具。
- `pawork-domain` 不得依赖任何 GUI framework（包括 GPUI/Tauri）、SQLite、HTTP Client、OS Keychain、Git、任何具体 Provider（canonical 纯净红线，依赖树可断言）。
- 禁止包间循环依赖；依赖方向见 §2。
- Agent Engine 不得通过判断 Provider 名称走特例逻辑；能力差异一律经 registry / capability / `provider_hints` 数据表达。
- Secret（明文 Token）不写入数据库、日志、事件 payload 与任何可能提交到仓库的文件；`Debug`/`Display` 输出脱敏（`[REDACTED]` 语义）。
- 所有 Agent 事件必须可持久化、可重放；磁盘/线上格式是冻结契约（§3.2），演进须用户确认 + 版本化迁移 + 升级 golden。
- 文件操作输入必须基于 `workspace_id + relative_path`，拒绝绝对路径与越 root 的 `..`；子进程、网络、Secret 访问须经 Policy / Sandbox 约束（承载于 `pawork-policy` / `pawork-exec`）。

2026-09-21 用户要求无项目附件并确认继续：Desktop 可读取系统选择器中用户显式选定的普通文件，经 GUI 1.22 上传文件名与字节；Host 不接收本机绝对路径，不注册父目录，不扩大工具工作区权限。模型文件工具继续遵循 `workspace_id + relative_path`。

违反以上任意一条须先向用户确认。破坏式改动写入本文与对应 Spec，golden 先行。

---

2026-09-24 经用户确认的产品契约（现行形状见 [协议 Spec](spec/crates/protocol.md)）：API 1.23 新增跨进程 TasksCancel；1.24 新增 Plan/Goal/技能录制与远程 Video 引用。持续目标由现行 Host 驱动并持久化，重启必须显式恢复；Desktop 仍不加载 Core。文件夹与外部浏览器快照只作为显式选定的附件，不注册工作区或扩大工具权限。技能写入由 Host 基于 workspace_id 解析后安全新建，插件面只展示实际记录，不引入插件运行时。

## 2. 包布局与依赖方向（29 包）

Workspace 为 **29 成员（27 库 + 2 应用）**：27 个库平铺 `crates/<短名>`（目录 = 包名去 `pawork-` 前缀，包名保持 `pawork-` 前缀），2 个应用 `apps/{pawork,desktop}`。2026-09-16 按用户明确要求新增独立浏览器包；2026-09-17 按用户明确授权新增独立 computer use 包；2026-09-27 用户授权本次合拆评估与实施，新增 models / gui-server / acp / mcp / gateway；判断依据见 §2.1。

| 包 | 目录 | 依赖方向 | 备注 |
| --- | --- | --- | --- |
| `pawork-domain` | `crates/domain` | 无内部依赖 | canonical 纯净红线；`provider_api/`（ModelProvider、CanonicalModelRequest、ProviderStreamEvent 13 变体、ProviderError、ResolvedCredential）与 `tool_api/`（AgentTool、ToolResult）；事件信封 v1 与契约字节 golden 在本包 tests/ |
| `pawork-protocol` | `crates/protocol` | → domain | GUI 帧 / headless-json / core-api / typegen（检入 `schemas/` 三产物）；`app/registry`（三通道登记单源）+ `projection/`（共享投影 reducer） |
| `pawork-testkit` | `crates/testkit` | → domain | dev-only：MockProvider/MockTool/契约断言 |
| `pawork-policy` | `crates/policy` | → domain | 安全内核；`PolicyDecision`/`ApprovalMode` 冻结契约与红线回归锚；shell 风险分类；`path` 内核 |
| `pawork-exec` | `crates/exec` | → policy（仅路径 helper） | process/sandbox/pty；不直接依赖 domain；CancellationToken 仍为本包类型 |
| `pawork-tools` | `crates/tools` | → domain、exec、policy、workspace、computer-use | 九个内置工具 + registry/scheduler；不依赖 MCP SDK 或认证客户端 |
| `pawork-workspace` | `crates/workspace` | → domain、policy | `service/`+`path/`+`file_index/`、`resources/`、`config/`（六层矩阵）、`import/`（五来源导入 + session_scan） |
| `pawork-storage` | `crates/storage` | → domain | `sqlite/`（Actor+migration 框架）、`session/`（DDL/迁移/export）、`blob/`（artifact + 共用 `atomic_write_bytes` + PWB1/checkpoint/protected）；`default = ["session","blob"]`，compaction/checkpoint/protected opt-in |
| `pawork-models` | `crates/models` | → domain | registry/能力证据与默认表、negotiate、pricing；无具体 Provider、HTTP、存储或 GUI 依赖 |
| `pawork-providers` | `crates/providers` | → domain、models | HTTP/SSE + Chat/Responses/Messages wire、八通道适配、usage/error 归一、reasoning 保护端口；通道静态注册单源 |
| `pawork-auth` | `crates/auth` | → domain | Secret 后端/OAuth/脱敏/解析链 + `locator` 单一事实源（Secret 审计边界） |
| `pawork-git` | `crates/git` | → domain、exec | Diff/Status/GitService/GitRunner/HunkStage/worktree；单一 `FileStatus` |
| `pawork-engine` | `crates/engine` | → domain（唯一 pawork-* 生产依赖，`tests/domain_only.rs` 断言护航） | tool_loop/session_turn/context/cancel/appender |
| `pawork-workflow` | `crates/workflow` | → domain | plan/task 纯 reducer |
| `pawork-orchestration` | `crates/orchestration` | → domain、policy（路径内核）、control-plane（default-features = false）、git(opt) | supervisor/budget/lifecycle/merge/task_graph/worktree/identity；不依赖 workflow（装配在 app） |
| `pawork-control-plane` | `crates/control-plane` | → domain（rusqlite optional，自开连接） | 控制面 core + `quota/` + `credential/`（lease/pool）；租户裁决 `TenantPolicyDecision`（与 policy 的 `PolicyDecision` 不同名）；usage `dedup_key`/audit JSONL golden |
| `pawork-transport` | `crates/transport` | 无内部依赖（帧长度常量与 protocol 对齐，但不依赖该 crate） | local（UDS/named pipe）+ memory |
| `pawork-mcp` | `crates/mcp` | → domain、tools、exec、workspace、auth | MCP 协议/连接/认证；rmcp 隔离于 codec；复用 tools 注册表和调度 |
| `pawork-gui-server` | `crates/gui-server` | → domain、protocol、transport | GuiServer / ConnectionManager / GuiHost 端口，无 Core 装配依赖 |
| `pawork-acp` | `crates/acp` | → domain、protocol | ACP wire、映射、会话 actor、AcpCommandHost 端口；无 CLI / app 依赖 |
| `pawork-gateway` | `crates/gateway` | → domain、auth | loopback HTTP/SSE、token、GatewayBackend；不依赖 Core/Provider/数据库 |
| `pawork-app` | `crates/app` | → domain、engine、models、providers、auth、tools、mcp、policy、workspace、exec、storage、git、workflow、orchestration、control-plane、protocol、transport、gui-server、gateway | AppCore / gui_host / GatewayBackend 实现；账户、租约、账本、工具与事件装配 |
| `pawork-cli` | `crates/cli` | → app、client、domain、engine、protocol、storage、transport、gui-server、acp、gateway | 子命令、REPL、stdio / listener / 进程生命周期装配 |
| `pawork-client` | `crates/client` | → domain、protocol、transport | framed 连接面 + `headless/`；probe 场景为本包 tests/，live 模式 `examples/probe.rs` |
| `pawork-terminal` | `crates/terminal` | 无内部依赖 | 终端显示核心：行缓冲解析（CR/退格/擦除/光标/折行/SGR 16 色）、按键→PTY 字节映射、面板像素→列×行估算；不是完整 VT emulator |
| `pawork-computer-use` | `crates/computer-use` | 无内部依赖 | 独立虚拟桌面截图和输入（本地 RFB / 容器内 Xvnc），不使用本机 HID；不依赖 GUI / Core / Provider，tools 适配后由 Host 执行 |
| `pawork-browser` | `crates/browser` | 无内部依赖 | 系统 WebView、HTTP(S) 导航、页面状态、显示与释放；macOS 使用 WebKit，不依赖 GPUI、Core 或协议 |
| `pawork`（bin） | `apps/pawork` | → cli | composition root + `redact.rs`（Redactor/RedactingFmtLayer） |
| `pawork-desktop`（bin） | `apps/desktop` | → client、terminal、browser、gpui、raw-window-handle；macOS platform-only → cocoa、objc | 四层 ui/projection/controller/platform；pawork-* 依赖 = client + terminal + browser（deny-list 断言）；浏览器手动导航与经 client 的 Host 授权自动操作，不直接访问 Core / Provider |

**不合并清单**（保持独立包）：`policy`、`exec`、`auth`、`git`、`engine`、`protocol`、`testkit`、`transport`、`orchestration`、`workflow`。

理由：这些包对应不同安全、执行、契约或状态职责，合并会破坏单向依赖或独立消费者边界。GUI 编译闭包可以出现 domain/protocol **纯类型**，红线指 GUI 不加载 Core 运行时装配。新增包需要现有职责、真实消费者与可以强制的依赖边界，不按文件大小或厂商品牌拆包。

归档资产以 git tag `v2-final` 兜底；复活条件登记 [产品候选](spec/backlog.md)；不得把归档代码复制回仓库其它位置。`pawork-domain` 的 `plugin = []` 仅作复活锚点。

---

### 2.1 2026-09-27 包边界决策

本次以长期职责内聚、依赖方向、协议/SDK 升级隔离及真实消费者为依据，不把实施成本或包数量作为目标。结论为 **5 处拆分、0 处合并**，24 → 29 成员。用户已授权分析后按推荐方案实施，覆盖此前“当前不新增包”的布局限制。

| 原边界 | 决策与现有证据 | 新依赖方向 |
| --- | --- | --- |
| providers 的 registry / negotiate / pricing / error | 抽出 models。app 与 adapter 共同使用目录、能力 gate 和计价；模型逻辑不应依赖 HTTP/厂商实现，迁出模块由 Cargo 边界隔离，留下的 usage/reasoning 保留 net 禁入守卫 | app → models；providers → models → domain |
| app 的 gui_server | 抽出 gui-server。已有 GuiHost 端口，握手/背压/心跳/重放不需要 Core 私有状态，MockHost 测试随迁；GUI 业务实现留在 app | cli → gui-server；app 实现 GuiHost；gui-server → protocol/transport/domain |
| cli 的 channels/acp | 抽出 acp。已有 AcpCommandHost，wire/会话 actor/权限映射只依赖 canonical 协议；CLI 承载 stdio 和进程生命周期 | cli → acp → protocol/domain |
| tools 的 mcp | 抽出 mcp。SDK、远端连接、OAuth 和 stdio 生命周期与本地文件工具不同；直接复用 ToolRegistry，不另建调度 | app → mcp → tools；tools 不依赖 rmcp/auth/reqwest |
| app 的 gateway_server / gateway_tokens | 抽出 gateway 并定义 GatewayBackend。HTTP/token 不加载 Core；Completion 保存宿主冻结请求，保持准备失败在 SSE 成功头之前返回 | cli → gateway；app 实现 GatewayBackend；gateway → domain/auth |

厂商通道继续在 providers 内按模块组织。八条通道共用三种 wire 协议，按品牌拆 crate 会使相同协议的更新跨越多个相互关联的包；没有独立 SDK、依赖闭包或真实消费者要求按品牌分割。HTTP/SSE 也不单独成为通用网络包：当前只是 providers 内部实现，认证/MCP 各有不同安全策略。reasoning 保护端口与 wire usage 归一留在 providers。

以下边界保留：domain 的纯契约；protocol 的共享 wire/投影、client 的客户端行为、transport 的字节 IO；policy 的裁决、exec 的执行与沙箱、auth 的 Secret；workspace 的根/配置/资源、storage 的事件与 blob 持久化、git 的系统 Git；engine 的 domain-only 执行核；workflow 的可重放状态机、orchestration 的 worker 生命周期、control-plane 的租约/配额/账本；testkit 的开发依赖；browser、computer-use、terminal 的独立平台/显示职责；两个应用的进程边界。当前没有职责重复到值得合包的生产实现，也不按文件长度进一步拆 workspace/storage/app。

实施是原实现及现有回归的单次迁移，消费者直接指向新包，不保留旧模块 re-export 或双轨实现。GUI/core-api、ACP、网关 HTTP、数据库和持久事件格式均不变；不新增第三方生产依赖。服务仍只有 `pawork` 正式宿主，Desktop 直接内部依赖仍为 client/terminal/browser。

竞品依据为 2026-09-27 获取的官方源码（固定 commit，具体机制并非照搬）：

- [Codex model-provider](https://github.com/openai/codex/tree/8f195c93d7e7acfef95acf273f0e49cce917e291/codex-rs/model-provider) 与 [models-manager](https://github.com/openai/codex/tree/8f195c93d7e7acfef95acf273f0e49cce917e291/codex-rs/models-manager)：协议请求、模型目录与 Provider 生命周期有独立职责；其 Provider 对 login/secrets/遥测的聚合不作为 Pawork 窄 adapter 的目标。
- [OpenCode provider](https://github.com/anomalyco/opencode/blob/b471c2b4495747353af768fbf2e0790c9d820ce2/packages/opencode/src/provider/provider.ts)：通用 SDK 协议与品牌配置可以复用；其 Bun/插件/应用服务形态不适合纯 Rust 约束。
- [Zed language_model_core](https://github.com/zed-industries/zed/tree/bda9c0bd43a8d235d82adb01ea5bc875b861ecfc/crates/language_model_core) 与 [open_ai](https://github.com/zed-industries/zed/blob/bda9c0bd43a8d235d82adb01ea5bc875b861ecfc/crates/open_ai/src/open_ai.rs)：共享语义与协议适配分离可参考；[language_model](https://github.com/zed-industries/zed/blob/bda9c0bd43a8d235d82adb01ea5bc875b861ecfc/crates/language_model/Cargo.toml) 的 GPUI 依赖不能进入 Pawork 模型/Provider 层。

---

## 3. 冻结契约与「追加不重写」

2026-09-25 用户授权的 [GW-1 本机模型网关](spec/model-gateway.md)：`pawork gateway serve` 在 127.0.0.1:17432 提供独立 OpenAI HTTP v1 子集，gateway 包完成 HTTP wire 翻译，通过 app 的 GatewayBackend 实现经既有 providers 调用模型，不走 Agent loop。使用按客户端可撤销的摘要 token、Host/Origin 校验及 thirdparty 租户账本；GUI/core-api 版本与既有 schema 不变。2026-09-27 已按职责抽为 gateway 库，不新增服务二进制，不开放远程账户池或凭证导出。

2026-10-03 用户授权对未发布功能直接修正 API：网关原生视频目录 / 任务使用 domain 的共享纯数据类型，`GatewayBackend` 显式实现目录、提交与按客户端查询三个端口，移除默认兼容实现。视频任务 ID 绑定提交账号，Secret 与身份同事务快照，查询不依赖当前选中账号；Chat / 视频共用渠道配置、JSON 有界读取及租约取消收尾。该媒体任务 API 不进入 Agent loop，不新增数据库或依赖边；生图仍归一为既有 `ImageOutput`，GUI / 事件冻结契约不变。


**右侧浏览器（2026-09-16，用户授权）**：`desktop → browser → 系统 WebKit` 承载网页视图及受限 DOM 操作；网页脚本运行于系统内容进程，不把 JS Runtime 嵌入 Agent / Core 或构建链。每任务一个内存页面，非持久网站数据互相隔离；不导入系统浏览器资料，不提供网页到 Rust 的工具桥。地址栏、链接与重定向限定 HTTP(S)，支持本地预览。聊天经 Host 的 `browser` 工具、Policy 与显式审批，通过 GUI 1.18 `browser_next` / `browser_respond` 操作当前任务页面，结果作为工具事件持久化；历史重放不派发操作。关闭释放，隐藏和切任务保留；尚无截图、多标签、下载与跨启动恢复。数据库 schema 不变。

### 3.1 终局包布局先行

- 现行布局为 §2 的 29 成员；browser 从首版即按用户要求独立。新能力按职责归属进入现有包；新增独立边界依 §2.1 的判断并取得任务授权；**禁止**「先写在 bin 里、以后再抽包」。
- 包间依赖方向遵守 §2 表与不合并清单；canonical 纯净红线不变。

### 3.2 冻结契约（激活即采用完整形状；golden 先于实现改动）

每个契约在激活时直接采用完整形状，宁可字段暂时闲置，也不做「先简后改」；golden 测试先于消费实现。

| 契约 | 形状要点 | golden / 锚位置 |
| --- | --- | --- |
| Provider 契约 | `ModelProvider`（`id`/`list_models`/`stream`）、`CanonicalModelRequest`（ADR-057 可选 `session_id`，缺省 None；真实会话入口传递）、`ProviderStreamEvent`（13 变体，tag=`type`/content=`data`）、`ModelResponseSummary`、`ResolvedCredential`（Debug 脱敏、无 Serialize）、`ProviderError` | `crates/domain`（`provider_api`）+ tests/ 契约 golden |
| 事件信封 | `AgentEventEnvelope`（`schema_version = 1`、`event_id/session_id/run_id/sequence/timestamp/parent_event_id/payload`）、`AgentEvent` 32 变体（含 `Diagnostic`）；与 SQLite migration 版本相互独立 | 信封字节 golden `crates/domain/tests/events_golden.rs` |
| 会话存储 | `session_events` DDL（`UNIQUE(session_id, sequence)`、`CHECK(sequence > 0)`）、append-only 双触发器、`AppendReceipt`；DB `CURRENT_SCHEMA_VERSION = 14`（v11 = `command_ledger` 宿主幂等表，纯新增不进 export；v12 = 分支 lineage 原生化，`messages` 整表重建去 `DEFAULT 'main'`、回填即校验孤儿行 fail-closed；v13 = `sessions.workspace_id` 归属弱引用列，纯追加不回填；v14 = `workspaces` 持久项目注册表，空表不回填、`root_path` UNIQUE）；import/export v3；fork 分支（`fork_from_event`） | DDL/迁移锚 `crates/storage/src/session/migration.rs`；升级 golden `crates/storage/src/session/fixtures/`（v9→v14 链；lineage 期望文件沿用 `v12_*`） |
| 工具契约 | `AgentTool`（`descriptor`/`execute`）、`ToolEventSink`、`ToolExecutionContext`（`workspace_id` + 相对 `working_directory`）、`ToolDescriptor`（含 `requires_approval`/`read_only`/`allowed_in_untrusted_workspace`） | `crates/domain::tool_api` |
| Policy 契约 | `PolicyDecision`（`Allow/Deny/AskUser/AllowWithConstraints`）、`ApprovalPrompt`+`RiskLevel`、`ApprovalMode`（默认 `ReadOnly`；旧 `on-failure` 仅兼容读入并映射 `NeverAsk`） | `crates/policy` 安全红线回归 |
| 引擎语义 | 审批经 `ApprovalResolver` await（`ToolApprovalRequested/Responded` 事件对；Requested 在等待前落盘）、`CancelHandle`+`CancelReason`、`LoopContext` 工具执行注入点 | `crates/engine` 定向回归 |
| 配置 schema | TOML、`ConfigTier`（Builtin<Global<Profile<Workspace<Session<Run）、`PaworkConfig`/`ProviderConfig{id, base_url}`（**无 api_key 字段**）；ADR-053 追加 Global-only `approval_mode` / `workspace_trust`，非 Global 高层剥离；ADR-054 追加 `naming_provider`/`naming_model` 自动命名对（分层同 `default_provider`/`default_model`） | `crates/workspace::config` 六层矩阵测试 |
| blob 格式 | `PWB1` + protected AEAD 边界；artifact/protected/checkpoint 三区 | `crates/storage::blob` golden |
| GUI 协议 | 帧格式带版本协商；`SUPPORTED_API_VERSIONS` 1.0–1.26（1.3 Terminal 生命周期 `terminal_close` + `TerminalExited`，按协商 minor 门控推送；1.4 Settings 认证；1.5 通用页；1.6 权限与审批；1.7 工具与 MCP；1.8 终端设置；1.9 Accepted 握手可选 `host_data_dir`；1.10 供应商级代理开关 `set_provider_use_proxy`；1.11 会话生命周期 `session_rename`/`session_archive`/`SessionMetaChanged`、`session_create.workspace_id` 可选化（ADR-054）；1.12 模型启用集 set_model_enabled/set_provider_models_enabled/set_default_role_model 与 role_defaults（ADR-055）；1.13 provider_auth_status 增 credentials 逐条凭证状态（ADR-056）；1.14 历史思考投影与可选 message_id/thinking_text，旧 minor 过滤新内容但保留分页游标（[ADR-057](spec/desktop.md#adr-057ui-3-思考投影与会话身份2026-09-08)）；1.15 命名账号新增/选择/删除及凭证 ID/名称/selected，旧 minor 剥离新字段（[ADR-059](spec/settings.md#adr-059ui-6b-命名账号与持久选择2026-09-08)）；1.16 逐账号 Percent 额度与耗尽切换模式，旧 minor 在发网前拒绝新查询并剥离模式字段（[ADR-060](spec/settings.md#adr-060ui-6b-g2-逐账号额度与耗尽切换2026-09-09)）；1.17 空 `display_name` 由 Host 生成默认账号名，并新增 GUI-only `auth_account_rename`（[ADR-061](spec/settings.md#adr-061账号默认名称与重命名2026-09-13)）；1.18 增加 GUI-only Browser 请求领取 / 回执与 BrowserControl capability；1.19 增加 GUI-only 项目目录 / 文本读取与带版本校验的手动文件保存；1.20 增加 GUI-only 子代理设置 `subagent_settings`/`set_subagent_settings` 与列表 / 取消 `subagent_list`/`subagent_cancel`；1.21 增加模型推理强度偏好 `RunStart.effort` / GUI-only `set_model_reasoning` 与 `ModelList` 条目能力 / 强度字段（ADR-063）；1.22 增加 GUI-only `attachment_upload` 与 `RunStart.attachment_ids` / `web_search`（本机附件分块暂存与本轮搜索覆盖，旧 minor 遇新字段 fail-closed）；1.25 `ModelList` 条目增 `text` / `image_output` 能力位（ADR-064 模型能力用途筛选，Gateway v1.1 与 CLI `--purpose` 同源 vocabulary）；1.26 新增 GUI-only 只读 `task_usage`，TaskUsage DTO 由 domain 共用）；typegen 检入 [`schemas/`](../schemas/)（core-api/gui-protocol/headless-json）；三通道可用性单源 `protocol::app::registry`，未登记 fail-closed | 帧 golden + typegen 断言（`crates/protocol`） |
| headless JSON | `HeadlessResponse`（`type=event|response`）；`run`/`chat --prompt --json` 已对齐；stdout 仅 JSONL；`--json` → 正式 headless 映射见 [spec/contracts.md](spec/contracts.md) | `crates/protocol` headless golden |
| 控制面 | usage `dedup_key`；audit JSONL；usage SQLite v4 纯新增 `task_usage_calls` 调用日志（task / group / subtask / operation，保留未知用量），既有费用 append-only / dedup 不变 | `fixtures/audit/event-v1.jsonl` + `crates/control-plane` golden |
| 缓存注解（附加式） | `CanonicalModelRequest` 缓存策略枚举（`Off/Auto/Explicit{retention}`）+ 前缀分段标注；`ModelResponseSummary`/usage 增 `cache_read`/`cache_write`；serde 向后兼容 | golden 先行；方案见 [references.md](references.md) 附录 B（F5-B） |
| 协议兼容表 | `PROTOCOL_CRATE_COMPATIBILITY` | `crates/protocol` |

### 3.3 消费面纪律与路径校验

- **无消费者不合入**：任何保留在主 workspace 的模块必须有真实装配点（生产调用链或已登记的激活条件）；零消费者代码归档，不以 experimental feature 库存。
- **合并不裁剪契约**：包合并时契约类型整组平移、零裁剪，golden/测试随迁。
- **破坏式改动边界**：允许破坏内部代码组织与 API；不允许静默破坏磁盘/线上格式、CLI 用户可见行为与安全语义（fail-closed 只紧不松）。
- **路径校验语义矩阵**：`pawork-policy` `path::resolve_workspace_path` 为写路径与读工具的唯一安全内核（canonical 复核 + root 收敛 + symlink/`.git`/TOCTOU 防护）；`pawork-workspace` `path::resolve_relative_path` 在平台词法前置拦截（盘符/UNC/设备名）后**委托** policy 内核。canonicalize / within-root / relative-to-root **只存在于 policy**：workspace resources 与 exec 沙箱必须调用这些函数，禁止包内再复制。新调用点一律复用 policy 内核。exec 可依赖 policy 仅为此 helper；不合并两包。

---

## 4. 安全语义

- **读写工具均拒 `.git`**（无审计开关）。
- **macOS Seatbelt**：写+网模式诚实标签 `HardWritesAndNetwork`。读 = 整盘 `(allow file-read* (subpath "/"))` + `default_secret_paths` 读写双拒挖洞（含 `.netrc` / `.git-credentials` / `.docker` / `.npmrc` / `.pypirc` / `.cargo/credentials.toml`）；写 = deny-default 白名单。隔离强度靠写闸 + 网络闸承担。tmp/`$TMPDIR` 白名单与 `.git`/`.env` 禁写洞一律 raw+canonical 双形态写入 profile（Seatbelt 按 canonical 路径匹配）。
- **MCP 凭证**走 SecretRef（仅 `pawork.mcp.*` 命名空间）+ 独立 `mcp-auth.json`；stdio 子进程 `env_clear` 且拒绝透传 `PAWORK_API_KEY_*`。
- **workspace 级配置**剥离 `proxy_url`/非回环 `base_url`、MCP `trusted`/`auto_start`；HTTP 错误只留 `HTTP {status}`；`redirect(Policy::none())`。
- **EventHub Lagged** → `ReplayUnavailable`；客户端收齐附带 Snapshot。禁止 seq-0 旁路直发。
- **未映射 headless 命令** fail-closed。
- 路径检查统一 `policy::path` 内核（读路径 symlink 同内核）；生产 `gui serve` 强制 token（UDS 0600）；Timeline 锚点用 `event_id`/`sequence`。
- 沙箱不可用时**可观测回退**：不是拒跑，CLI/GUI 必须展示 fallback。PTY 创建入 policy 闸（NeverAsk/ReadOnly 直拒，AskUser fail-closed 落 Deny）。
- shell 风险分类用手写 tokenizer（不引入外部 parser）；灾难地板（如 `` `rm -rf /` `` 与 `$(rm -rf /)`）必须命中。

---

## 5. 关键实现决策

这些是现行形状，不是待办：

- **会话分支**：append-only 单表全局 sequence；fork 只许切在闭合 turn 边界（`RunCompleted` / `RunCancelled` / `RunFailed`）；压缩按分支水位；父支晚写不得污染旧 fork。
- **Session→Workspace**：`sessions.workspace_id` 可空弱引用，写穿 + 启动预载；不回填历史；无 FK。
- **持久项目注册表**：`workspaces` 表按 canonical root 幂等登记，`root_path` UNIQUE；同 id 不同 root fail-closed。会话归属分两态（ADR-054 D1 修订 ADR-044 D3）：显式无项目会话（sessions.workspace_id 为 NULL）是合法产品状态，以空授权面（ws-unbound、无 roots）运行问答，文件类工具由 Policy 对空 roots fail-closed；绑定悬空（指向不可用 workspace）仍 fail-closed，仅测试与尚未登记任何 root 的进程允许 legacy ws-unbound 落空授权面。
- **聊天控制 Terminal / Browser（2026-09-16 用户要求）**：GUI Run 在 Host 注册工具并经过 Policy / 显式审批。Terminal 共用既有 PTY（非沙箱）；Browser 通过 GUI 1.18 browser_next / browser_respond 操作系统 WebView，请求绑定发起 Run 的 GUI 和 session、一次领取。工具结果持久化，历史不执行动作，没有新增 Core→GUI 依赖或 JS Runtime。
- **Terminal 生命周期**：`terminal_close` 注销注册表；`TerminalExited` live 事件按协商 minor 门控；重复 close 报 `not_found`（对客户端是「清理目标已达成」）。
- **Settings wire**：API key 明文只走非重放单帧 `ApiKeySecret`（Debug 恒 `[REDACTED]`，无 Display）；`SetApprovalMode` 保存 Global `approval_mode` 默认，`WorkspaceTrust` 保存 Global `workspace_trust` canonical 根路径布尔项（[ADR-053](spec/settings.md#adr-053opt-1-设置持久化2026-09-05)）；先落盘后更新后续 Run，进行中 Run 不变；`SetProxyUrl` 写 workspace 外标准用户配置目录的 Global `config.toml`，`SetTerminalSettings` / MCP remove 同写 Global 层；About 只在握手提供非空 `host_data_dir` 时显示，不从 endpoint 反推。
- **Desktop AX**：GPUI 锁定 `=0.2.2`；显式语义树 + AppKit 虚拟 AX 元素；AX action 回到既有 AppView handler 与 enable gate。
- **CancellationToken**：exec 与其它包仍双轨，不借路径依赖合并类型。
- **配置写盘**：Global 层入口共用单一 RMW 内核（`CONFIG_WRITE_LOCK` + tmp/rename）；OAuth/MCP 测试与可注入默认 HTTP 客户端均为 `redirect(Policy::none())`。
- **原子写**：blob / auth / config 共用「同目录临时文件 + rename」；storage 以 `atomic_write_bytes` 为单源。

- **手动文件编辑（2026-09-16 用户要求）**：Desktop Files 经 GUI 1.19 → Host 操作项目文件，输入仍为 workspace_id + relative_path；目录 / 读取不写库，保存只接受读取时的内容版本，冲突不覆盖。文件正文不进入命令账本或日志。只扩展手动 GUI 操作，不改变 Agent 工具 Policy、审批或事件重放语义。
