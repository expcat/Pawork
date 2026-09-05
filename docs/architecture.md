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

违反以上任意一条须先向用户确认。破坏式改动写入本文与对应 Spec，golden 先行。

---

## 2. 包布局与依赖方向（21 包）

Workspace 为 **21 成员（19 库 + 2 应用）**：19 个库平铺 `crates/<短名>`（目录 = 包名去 `pawork-` 前缀，包名保持 `pawork-` 前缀），2 个应用 `apps/{pawork,desktop}`。不新增包；新能力只往既有包加模块。包布局变更须向用户确认。

| 包 | 目录 | 依赖方向 | 备注 |
| --- | --- | --- | --- |
| `pawork-domain` | `crates/domain` | 无内部依赖 | canonical 纯净红线；`provider_api/`（ModelProvider、CanonicalModelRequest、ProviderStreamEvent 13 变体、ProviderError、ResolvedCredential）与 `tool_api/`（AgentTool、ToolResult）；事件信封 v1 与契约字节 golden 在本包 tests/ |
| `pawork-protocol` | `crates/protocol` | → domain | GUI 帧 / headless-json / core-api / typegen（检入 `schemas/` 三产物）；`app/registry`（三通道登记单源）+ `projection/`（共享投影 reducer） |
| `pawork-testkit` | `crates/testkit` | → domain | dev-only：MockProvider/MockTool/契约断言 |
| `pawork-policy` | `crates/policy` | → domain | 安全内核；`PolicyDecision`/`ApprovalMode` 冻结契约与红线回归锚；shell 风险分类；`path` 内核 |
| `pawork-exec` | `crates/exec` | → policy（仅路径 helper） | process/sandbox/pty；不直接依赖 domain；CancellationToken 仍为本包类型 |
| `pawork-tools` | `crates/tools` | → domain、exec、policy、workspace、auth | 八工具 + scheduler + `mcp/`（rmcp 隔离断言为模块级测试） |
| `pawork-workspace` | `crates/workspace` | → domain、policy | `service/`+`path/`+`file_index/`、`resources/`、`config/`（六层矩阵）、`import/`（五来源导入 + session_scan） |
| `pawork-storage` | `crates/storage` | → domain | `sqlite/`（Actor+migration 框架）、`session/`（DDL/迁移/export）、`blob/`（artifact + 共用 `atomic_write_bytes` + PWB1/checkpoint/protected）；`default = ["session","blob"]`，compaction/checkpoint/protected opt-in |
| `pawork-providers` | `crates/providers` | → domain | `net/`（http/sse/retry）+ `registry/`/`pricing/`/`usage/`/`negotiate/`/`reasoning/` + `channels/`（六通道，feature 门控；通道登记单点 `channels/registry.rs` `CHANNEL_REGISTRY`，app 侧为 facade）；core 不依赖 net 为模块纪律 + 源扫描测试 |
| `pawork-auth` | `crates/auth` | → domain | Secret 后端/OAuth/脱敏/解析链 + `locator` 单一事实源（Secret 审计边界） |
| `pawork-git` | `crates/git` | → domain、exec | Diff/Status/GitService/GitRunner/HunkStage/worktree；单一 `FileStatus` |
| `pawork-engine` | `crates/engine` | → domain（唯一 pawork-* 生产依赖，`tests/domain_only.rs` 断言护航） | tool_loop/session_turn/context/cancel/appender |
| `pawork-workflow` | `crates/workflow` | → domain | plan/task 纯 reducer |
| `pawork-orchestration` | `crates/orchestration` | → domain、control-plane（default-features = false）、git(opt) | supervisor/budget/lifecycle/merge/task_graph/worktree/identity；不依赖 workflow（装配在 app） |
| `pawork-control-plane` | `crates/control-plane` | → domain（rusqlite optional，自开连接） | 控制面 core + `quota/` + `credential/`（lease/pool）；租户裁决 `TenantPolicyDecision`（与 policy 的 `PolicyDecision` 不同名）；usage `dedup_key`/audit JSONL golden |
| `pawork-transport` | `crates/transport` | 无内部依赖（帧长度常量与 protocol 对齐，但不依赖该 crate） | local（UDS/named pipe）+ memory |
| `pawork-app` | `crates/app` | 领域宿主依赖 + transport | 装配宿主 + `gui_server/`（GuiServer/ConnectionManager/GuiHost trait）+ `gui_host/`（分发表） |
| `pawork-cli` | `crates/cli` | 原 cli 依赖（GuiHost 经 app） | 21 子命令 + `channels/acp/`（AcpHost 四件套） |
| `pawork-client` | `crates/client` | → domain、protocol、transport | framed 连接面 + `headless/`；probe 场景为本包 tests/，live 模式 `examples/probe.rs` |
| `pawork`（bin） | `apps/pawork` | → cli | composition root + `redact.rs`（Redactor/RedactingFmtLayer） |
| `pawork-desktop`（bin） | `apps/desktop` | → client、gpui；macOS platform-only → cocoa、objc、raw-window-handle | 四层 ui/projection/controller/platform；业务依赖仅 pawork-client（deny-list 断言）；AX 走应用侧原生 bridge，不扩张业务依赖 |

**不合并清单**（保持独立包）：`policy`、`exec`、`auth`、`git`、`engine`、`protocol`、`testkit`、`transport`、`orchestration`、`workflow`。

理由（布局经验）：`policy` 并入含 tools 的包即成环；`exec` 零内部依赖自含；`auth` 是 Secret 审计边界。GUI 编译闭包可以出现 domain/protocol **纯类型**，不违反「GUI 不加载 Core」——红线指运行时装配，不指类型入编译图。对照外部布局只抄纪律不抄粒度：微 crate 增殖会把跨域改动摊到十几份 Cargo 清单上。

归档资产以 git tag `v2-final` 兜底；复活条件登记 [产品候选](spec/backlog.md)；不得把归档代码复制回仓库其它位置。`pawork-domain` 的 `plugin = []` 仅作复活锚点。

---

## 3. 冻结契约与「追加不重写」

### 3.1 终局包布局先行

- 现行终局布局为 §2 的 21 成员；新能力 = 已有包内新模块（不新增包）；**禁止**「先写在 bin 里、以后再抽包」。
- 包间依赖方向遵守 §2 表与不合并清单；canonical 纯净红线不变。

### 3.2 冻结契约（激活即采用完整形状；golden 先于实现改动）

每个契约在激活时直接采用完整形状，宁可字段暂时闲置，也不做「先简后改」；golden 测试先于消费实现。

| 契约 | 形状要点 | golden / 锚位置 |
| --- | --- | --- |
| Provider 契约 | `ModelProvider`（`id`/`list_models`/`stream`）、`CanonicalModelRequest`、`ProviderStreamEvent`（13 变体，tag=`type`/content=`data`）、`ModelResponseSummary`、`ResolvedCredential`（Debug 脱敏、无 Serialize）、`ProviderError` | `crates/domain`（`provider_api`）+ tests/ 契约 golden |
| 事件信封 | `AgentEventEnvelope`（`schema_version = 1`、`event_id/session_id/run_id/sequence/timestamp/parent_event_id/payload`）、`AgentEvent` 32 变体（含 `Diagnostic`）；与 SQLite migration 版本相互独立 | 信封字节 golden `crates/domain/tests/events_golden.rs` |
| 会话存储 | `session_events` DDL（`UNIQUE(session_id, sequence)`、`CHECK(sequence > 0)`）、append-only 双触发器、`AppendReceipt`；DB `CURRENT_SCHEMA_VERSION = 14`（v11 = `command_ledger` 宿主幂等表，纯新增不进 export；v12 = 分支 lineage 原生化，`messages` 整表重建去 `DEFAULT 'main'`、回填即校验孤儿行 fail-closed；v13 = `sessions.workspace_id` 归属弱引用列，纯追加不回填；v14 = `workspaces` 持久项目注册表，空表不回填、`root_path` UNIQUE）；import/export v3；fork 分支（`fork_from_event`） | DDL/迁移锚 `crates/storage/src/session/migration.rs`；升级 golden `crates/storage/src/session/fixtures/`（v9→v14 链；lineage 期望文件沿用 `v12_*`） |
| 工具契约 | `AgentTool`（`descriptor`/`execute`）、`ToolEventSink`、`ToolExecutionContext`（`workspace_id` + 相对 `working_directory`）、`ToolDescriptor`（含 `requires_approval`/`read_only`/`allowed_in_untrusted_workspace`） | `crates/domain::tool_api` |
| Policy 契约 | `PolicyDecision`（`Allow/Deny/AskUser/AllowWithConstraints`）、`ApprovalPrompt`+`RiskLevel`、`ApprovalMode`（默认 `ReadOnly`；旧 `on-failure` 仅兼容读入并映射 `NeverAsk`） | `crates/policy` 安全红线回归 |
| 引擎语义 | 审批经 `ApprovalResolver` await（`ToolApprovalRequested/Responded` 事件对；Requested 在等待前落盘）、`CancelHandle`+`CancelReason`、`LoopContext` 工具执行注入点 | `crates/engine` 定向回归 |
| 配置 schema | TOML、`ConfigTier`（Builtin<Global<Profile<Workspace<Session<Run）、`PaworkConfig`/`ProviderConfig{id, base_url}`（**无 api_key 字段**）；ADR-053 追加 Global-only `approval_mode` / `workspace_trust`，非 Global 高层剥离 | `crates/workspace::config` 六层矩阵测试 |
| blob 格式 | `PWB1` + protected AEAD 边界；artifact/protected/checkpoint 三区 | `crates/storage::blob` golden |
| GUI 协议 | 帧格式带版本协商；`SUPPORTED_API_VERSIONS` 1.0–1.10（1.3 Terminal 生命周期 `terminal_close` + `TerminalExited`，按协商 minor 门控推送；1.4 Settings 认证；1.5 通用页；1.6 权限与审批；1.7 工具与 MCP；1.8 终端设置；1.9 Accepted 握手可选 `host_data_dir`；1.10 供应商级代理开关 `set_provider_use_proxy`）；typegen 检入 [`schemas/`](../schemas/)（core-api/gui-protocol/headless-json）；三通道可用性单源 `protocol::app::registry`，未登记 fail-closed | 帧 golden + typegen 断言（`crates/protocol`） |
| headless JSON | `HeadlessResponse`（`type=event|response`）；`run`/`chat --prompt --json` 已对齐；stdout 仅 JSONL；`--json` → 正式 headless 映射见 [spec/contracts.md](spec/contracts.md) | `crates/protocol` headless golden |
| 控制面 | usage `dedup_key`；audit JSONL | `fixtures/audit/event-v1.jsonl` + `crates/control-plane` golden |
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
- **持久项目注册表**：`workspaces` 表按 canonical root 幂等登记，`root_path` UNIQUE；同 id 不同 root fail-closed。有可用项目时未绑定/未登记会话 fail-closed。
- **Terminal 生命周期**：`terminal_close` 注销注册表；`TerminalExited` live 事件按协商 minor 门控；重复 close 报 `not_found`（对客户端是「清理目标已达成」）。
- **Settings wire**：API key 明文只走非重放单帧 `ApiKeySecret`（Debug 恒 `[REDACTED]`，无 Display）；`SetApprovalMode` 保存 Global `approval_mode` 默认，`WorkspaceTrust` 保存 Global `workspace_trust` canonical 根路径布尔项（[ADR-053](spec/settings.md#adr-053opt-1-设置持久化2026-09-05)）；先落盘后更新后续 Run，进行中 Run 不变；`SetProxyUrl` 写 workspace 外标准用户配置目录的 Global `config.toml`，`SetTerminalSettings` / MCP remove 同写 Global 层；About 只在握手提供非空 `host_data_dir` 时显示，不从 endpoint 反推。
- **Desktop AX**：GPUI 锁定 `=0.2.2`；显式语义树 + AppKit 虚拟 AX 元素；AX action 回到既有 AppView handler 与 enable gate。
- **CancellationToken**：exec 与其它包仍双轨，不借路径依赖合并类型。
- **配置写盘**：Global 层入口共用单一 RMW 内核（`CONFIG_WRITE_LOCK` + tmp/rename）；OAuth/MCP 测试与可注入默认 HTTP 客户端均为 `redirect(Policy::none())`。
- **原子写**：blob / auth / config 共用「同目录临时文件 + rename」；storage 以 `atomic_write_bytes` 为单源。
