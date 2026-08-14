# V1 全量 Review 与迁移参考（原 ROADMAP_V2.md）

> 本文档是 V2 重构的**原始事实源与迁移词典**：V1 全量 Review 结论、V2 目标与原则、终局目录结构、V1→V2 模块映射总表、发布策略、开发期测试策略、风险与缓解、Review 方法附录。前身为仓库根 `ROADMAP_V2.md`（基于 2026-08-14 对 V1 仓库的全量 Review 撰写，后曾并入 V2 ROADMAP §13），文档体系整合后独立成文。除相对路径与章节编号按新位置修正外，内容保持原文，作为**冻结参考**不随阶段推进改写（行数与依赖数据为 2026-08-14 快照）。
>
> 子节对照（原 ROADMAP_V2.md → 本文）：原 §1（V1 Review 结论）→ §1、原 §2（目标与原则）→ §2、原 §3（目录结构）→ §3、原 §4（模块映射 / 迁移词典）→ §4、原 §5（发布策略）→ §5、原 §6（测试与验证）→ §6、原 §8（风险与缓解）→ §7、原 §10（Review 方法附录）→ §8。原 §7（里程碑）已被增量式路线图（[../ROADMAP.md](../ROADMAP.md)）取代；原 §9（未决事项）已并入 ROADMAP §4。归档计划（[../plan/archive/](../plan/archive/README.md)）中对「ROADMAP_V2 §N」或「ROADMAP §13.N」的引用按此对照定位。
>
> 文中 M0–M8 编号均指已归档的旧里程碑（新旧对照见 [../plan/archive/README.md](../plan/archive/README.md)）；其中 M8 的门禁与发布职责由 S12 承接（[../plan/S12-release-hardening.md](../plan/S12-release-hardening.md)）。
>
> 关联文档：[../ROADMAP.md](../ROADMAP.md)（V2 任务索引）· [design.md](design.md)（V2 设计文档）· [../../ROADMAP.md](../../ROADMAP.md)（V1 路线图）· [../../REVIEW.md](../../REVIEW.md)（V1 各 Phase 评审记录）· [../../docs/architecture/workspace-layout.md](../../docs/architecture/workspace-layout.md)（V1 crate 注册表）· [../../AGENTS.md](../../AGENTS.md)（工作约定）。

## 1. V1 全量 Review 结论

### 1.1 规模与结构现状（2026-08-14 实测）

| 维度 | 数据 |
| --- | --- |
| workspace 成员 | 86 crate + 2 app + benches（另有 `client-codex-app-server`、`client-claude-gateway` 两个磁盘存在、经 path 依赖参与构建但未登记 members 的 crate，实际 88 crate） |
| 代码量 | 572 个 `.rs` 文件，约 236,000 行 Rust |
| 头部集中 | `app-service` 21.5k 行（29 个内部依赖，全仓组装枢纽）、`quota-service` 14.1k、`provider-control` 13.5k、`session-store` 9.7k |
| 尾部碎片 | 12 个 crate 不足 500 行：`transport-api` 165、`cli-renderer` 171、`schema-typegen` 217、`tool-api` 253、`subscription-hub` 270、`agent-events` 338、`client-auth` 346、`transport-memory` 387、`snapshot-service` 432、`workspace-service` 440、`cli-command` 443、`connection-manager` 450 |
| 完成度 | ROADMAP 计数 219 任务 / 188 完成，但大量任务为「有界完成」——library 层交付、生产宿主接线登记延期（Phase 15/16/17/18 各有 5–6 项延期落点） |

### 1.2 四类系统性问题

**问题一：crate 过度碎片化。** 88 个 crate 承载 23.6 万行（平均 2.7k 行/crate，中位数约 1.4k）。大量 crate 单独存在只为表达依赖方向（`tool-api` 253 行、一半是 re-export；`agent-events` 393 行、事件载荷类型实际定义在 `agent-domain`；`transport-api` 165 行）。每次跨域改动动辄触碰 5–10 个 crate，任务写入集约定（「收敛到单一 crate」）在此结构下频繁失效。

**问题二：组件齐全、主干未通电（最严重）。** [../../REVIEW.md](../../REVIEW.md) §0.2 自 P2 起持续记录该模式，至今未根治。全 workspace 依赖扫描（2026-08-14）确认零生产消费者的 crate：

| 未通电资产 | 行数 | 状态 |
| --- | --- | --- |
| `builtin-tools`（read/write/edit/apply_patch/run_command/search/find/list 八个内置工具） | 3,655 | **全 workspace 零消费者。生产装配链（`apps/pawork` → `app-service`）只注册了 MCP 工具，正式二进制的 Agent Loop 没有接入任何内置文件工具** |
| `pty-service` | 1,289 | 零消费者 |
| `compaction-engine` | 1,301 | 零消费者 |
| `file-index` | 930 | 零消费者 |
| `context-engine` | 1,469 | 仅被零消费者的 compaction-engine 引用，主循环未接入 |
| Phase 16 五件套：`goal-service` / `automation-service` / `monitor-service` / `memory-service` / `review-engine` | 6,904 | 全部零消费者（`plan-service` 仅被 teams 部分消费） |
| Phase 17 批量：`wasm-plugin-host` / `lsp-runtime` / `plugin-package` / `marketplace` / `ide-host-adapter` / `compat-loader` / `remote-control-adapter` / `browser-computer-runtime`（driver 全为 Stub） | 约 28,000 | 扩展生态 14 crate 中仅 5 个有生产接线 |

合计约 4.5 万行「测试全绿但系统不可用」的库存代码。**V1 的根本失衡：横向铺功能面优先于纵向打通可用主干。**

**问题三：文档注册表漂移。** [../../docs/architecture/workspace-layout.md](../../docs/architecture/workspace-layout.md) 与源码实态多处不符：`checkpoint-service`「依赖 git-service」不实（实际仅依赖 artifact-store）；`usage-ledger`「SQLite 待 P18-8」过时（SqliteUsageLedger 已落地）；`provider-control`「最小契约」实为 13.5k 行完整实现；`app-database` 已吸收控制面/identity/lease 三套具体 schema，偏离「不依赖具体 schema」登记；`agent-api`、`provider-bedrock`、`provider-mistral` 登记但从未创建；`user-hooks`/`compat-loader`/`browser-computer-runtime`/`acp-host` 等的依赖清单与 Cargo.toml 实态不符。手工注册表已不可信。

**问题四：验证流程过重。** L0–L3 分级验证、每 Phase 的 review + remediation 循环、4 个专用门禁脚本（`scripts/p15-gate.sh` 等）、schema drift 检查、Workspace Full Gate 升级条件审议——流程本身消耗了大量开发时间，且没能阻止问题二（门禁验证的是「库正确」，不验证「系统可用」）。

**其余结构性发现**（V2 需修复）：
- 重复实现：`provider-openai` 与 `provider-xai` 各有一份约 1,300 行的 Responses 流组装器；Ed25519+blake3 验签逻辑在 `wasm-plugin-host`/`marketplace` 三处重复；SQLite schema 与迁移散布 5+ 处。
- 类型泄漏：`mcp-client` 的 pub trait `McpPeer` 直接用 `rmcp::model::*` 做签名（「不泄漏 SDK 类型」只兑现一半）；`session-store` 反向依赖 `client-adapter-api`（存储层依赖服务契约，分层瑕疵）。
- `http-runtime` 规划已久未抽离，导致 `marketplace` 无真实 HTTP 源、`quota-service` 直接借 `provider-runtime` 的 HTTP 层。

### 1.3 可复用资产盘点

V1 代码质量整体过硬（每 Phase 均经评审修复、安全红线项有回归测试），V2 是**重组而非重写**。分四档：

- **高外部价值（发布主打）**：进程执行链（process/sandbox/pty，跨平台进程树+Landlock/Seatbelt/Job Object+PTY 重连，Rust 生态稀缺）；SSE/JSONL/partial-JSON 流解析器（零依赖纯函数）；LSP Client 运行时（生态缺 async client 侧）；SQLite Actor 模式库；层级配置合并；`gui-client`/`agent-sdk` 接入 SDK；多厂商 Provider 适配层。
- **平台核心（随平台走）**：agent-engine、app-service、session-store、git/diff、policy-engine、tool 链、GUI 协议栈。
- **冻结候审（不急迁移，先决定砍留）**：quota-service 六厂商远端适配器 + WebScrape + refresh scheduler（约 8k 行未接线）；browser-computer-runtime（driver 全 Stub）；tool-runtime 的 tool_search（1,216 行 feature 门控、无消费者）。
- **不迁移**：`transport-remote-placeholder`（已被 transport-remote 取代，trait 上移后删除）；`benches`（全部为 no-op 占位，V2 需要时重建）；注册表中从未创建的 `agent-api`/`provider-bedrock`/`provider-mistral`；各 Phase 门禁脚本；`apps/pawork` 中的 TEMP-P17-7-VERIFY 临时代码。

## 2. V2 目标与原则

### 2.1 目标

1. 在仓库根创建 `Pawork_v2/`（独立 Cargo workspace），按功能域分子目录，把 88 crate 重组为 **约 40 个包 + 2 个应用**。
2. 可独立发布：所有包满足发布卫生（无循环依赖、无内部类型泄漏、元数据齐全）；其中约 15 个高外部价值包按波次发布到 crates.io（`pawork-*` 前缀）。
3. **纵向优先**：先交付一个内置工具真实接线、能在真实仓库完成编码任务的 CLI Coding Agent（V1 从未达成），再横向合入扩展生态与控制面。
4. 开发期只做关键测试、不做门禁；全部门禁推迟到功能完备后的一次性 Release Hardening（M8，现由 S12 承接）。

### 2.2 保留的架构红线（不变）

- CLI 与 Core 同进程同二进制，`pawork` 是唯一正式宿主；纯 Rust，无 Node/Bun/JS Runtime；GUI 独立进程经 GUI Connection Protocol 连接。
- canonical domain 纯净：`pawork-domain` 与 `pawork-api` 不依赖 GUI framework、SQLite、HTTP Client、OS Keychain、Git、具体 Provider。
- 所有 Agent 事件可持久化、可重放；**序列化形状与磁盘格式是冻结契约**（见 §7 风险 1）。
- Secret 不落库、不入日志；Agent Engine 无 Provider 名称特例分支；禁止循环依赖。

### 2.3 新增规则（针对 V1 病灶）

- **无消费者不合入**：任何包/能力合入 V2 主干时，同批必须接到 `pawork` 装配链上有真实调用点，否则标记 `experimental` feature 并在 ROADMAP 登记。禁止再产生「库完成、零接线」库存。
- **注册表自动化**：不再手工维护 crate 注册表；依赖图用 `cargo metadata` 派生，文档只写域级职责。
- **依赖方向的执法工具从「crate 边界」放宽为「包内模块 + feature 门」**；跨包方向约束在 M8 补 workspace lint 兜底。合并不等于放弃分层。

### 2.4 明确放宽（V2 开发期）

- 不做 L0–L3 分级与升级审批、不做每功能簇 review+remediation 循环、不维护门禁脚本。
- 允许包内先用 `todo!()`/feature 残缺合入（但必须编译通过且不在默认 feature 路径上）。
- 文档同步要求降为：里程碑收尾时更新状态表，不逐任务同步。

## 3. V2 目录结构

```text
Pawork_v2/
├── Cargo.toml               # 独立 workspace 根（resolver 2，成员按域目录 glob）
├── foundation/              # 基座域：类型、契约、协议、通用基础设施
│   ├── domain/              #   pawork-domain
│   ├── api/                 #   pawork-api
│   ├── protocol/            #   pawork-protocol
│   ├── sqlite/              #   pawork-sqlite
│   ├── config/              #   pawork-config
│   ├── diagnostics/         #   pawork-diagnostics
│   └── testkit/             #   pawork-testkit
├── net/                     # 网络域
│   └── net/                 #   pawork-net
├── providers/               # Provider 域
│   ├── core/                #   pawork-provider-core
│   ├── adapters/            #   pawork-providers
│   └── auth/                #   pawork-auth
├── storage/                 # 存储域
│   ├── blob/                #   pawork-blob-store
│   └── session/             #   pawork-session
├── workspace/               # 工作区域
│   ├── core/                #   pawork-workspace
│   └── resources/           #   pawork-resources
├── execution/               # 执行域
│   ├── exec/                #   pawork-exec
│   ├── policy/              #   pawork-policy
│   └── tools/               #   pawork-tools
├── vcs/                     # Git 域
│   └── git/                 #   pawork-git
├── engine/                  # 引擎域
│   └── engine/              #   pawork-engine
├── extensions/              # 扩展域
│   ├── mcp/                 #   pawork-mcp
│   ├── wasm-host/           #   pawork-wasm-host
│   ├── plugin/              #   pawork-plugin
│   ├── hooks/               #   pawork-hooks
│   └── lsp/                 #   pawork-lsp
├── workflow/                # 工作流域
│   ├── core/                #   pawork-workflow
│   ├── memory/              #   pawork-memory
│   └── review/              #   pawork-review
├── agents/                  # 多 Agent 域
│   └── orchestration/       #   pawork-orchestration
├── control-plane/           # 控制面域
│   ├── core/                #   pawork-control-plane
│   ├── provider-control/    #   pawork-provider-control
│   └── quota/               #   pawork-quota
├── host/                    # 宿主域
│   ├── app/                 #   pawork-app
│   ├── transport/           #   pawork-transport
│   ├── gui-server/          #   pawork-gui-server
│   ├── channels/            #   pawork-channels
│   └── cli/                 #   pawork-cli
├── clients/                 # 客户端域
│   ├── gui-client/          #   pawork-client
│   ├── sdk/                 #   pawork-sdk
│   └── compat/              #   pawork-compat
├── apps/
│   ├── pawork/              # 唯一正式宿主二进制
│   └── protocol-probe/      # 协议自检工具（原 protocol-test-gui，不发布）
├── schemas/                 # 生成的 .d.ts 与 JSON Schema（typegen 输出）
└── fixtures/                # 跨包共享测试夹具（包内夹具就近放包内）
```

## 4. V1 → V2 模块映射

### 4.1 映射总表（40 包 + 2 应用）

「发布」列：**W1–W4** = crates.io 发布波次（见 §5.2）；**内部** = 保持发布卫生但不主动发布。行数为迁移前 V1 实测约数。

| # | V2 包 | 目录 | 合并自（V1） | 约行数 | 发布 | 关键动作 |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `pawork-domain` | foundation/domain | agent-domain + agent-events | 2.3k | W1 | events 作为 `events` 模块并入，保留 `schema_version` 语义与 serde 形状；`typegen` feature 保持可选 |
| 2 | `pawork-api` | foundation/api | provider-api + tool-api + plugin-api | 2.0k | W2 | 扩展契约三合一，feature：`provider`/`tool`/`plugin`；tool-api 的 re-export 薄壳消除 |
| 3 | `pawork-protocol` | foundation/protocol | core-api + gui-protocol + client-adapter-api + client-auth + headless-json + schema-typegen | 6.5k | W2 | 应用/GUI/Headless 协议单一 schema source；typegen 改为 `[[bin]]` + feature，重写路径发现（V1 硬编码 `../..`）；core-api 单文件 2k 行拆模块 |
| 4 | `pawork-sqlite` | foundation/sqlite | app-database（纯化） | 2.0k | W1 | 控制面/identity/lease 三套 schema 迁移移交各 owner 包，回归纯 SQLite Actor + 备份/恢复 + 通用 migration 框架 |
| 5 | `pawork-config` | foundation/config | config-service | 1.1k | W1 | PaworkConfig schema 泛化或 feature 化，层级合并引擎通用化 |
| 6 | `pawork-diagnostics` | foundation/diagnostics | diagnostics | 1.0k | W2 | 脱敏 tracing layer 是卖点；接线到所有域（V1 仅 resource-loader 消费） |
| 7 | `pawork-testkit` | foundation/testkit | test-support | 1.2k | W4 | Mock Provider/Tool/Plugin + contract 断言，供第三方扩展作者使用 |
| 8 | `pawork-net` | net/net | provider-runtime 的 http/retry/sse/jsonl/partial_json | 1.8k | W1 | 补齐规划已久的 http-runtime；feature：`parsers`（默认，零重依赖）/`http`（reqwest）；marketplace/hooks/quota 共用 |
| 9 | `pawork-provider-core` | providers/core | provider-runtime 剩余（组装/usage/协商/capability/reasoning trait）+ model-registry | 2.9k | W3 | `ProtectedBlobStoreProtector` 实现移交宿主组装层，砍掉对 blob store 的依赖（trait 保留） |
| 10 | `pawork-providers` | providers/adapters | provider-openai-compatible + openai + anthropic + google + xai + zhipu + qwen + moonshot | 15k | W3 | 每厂商一个 feature；openai/xai 两份 Responses 组装器（约 1.3k 行 × 2）下沉为共享模块；厂商错误码归一做成数据表 |
| 11 | `pawork-auth` | providers/auth | auth-service | 2.0k | W3 | Keychain + OAuth（PKCE/Device/refresh/callback）通用 LLM 认证包 |
| 12 | `pawork-blob-store` | storage/blob | artifact-store + protected-blob-store + checkpoint-service | 3.3k | W3 | feature：`protected`（AEAD 加密层）/`checkpoint`（写快照回滚）；数据库文件与安全语义保持分离（ADR-032 不破） |
| 13 | `pawork-session` | storage/session | session-store + compaction-engine | 11k | 内部 | compaction 以 `TokenEstimator` trait 注入解耦后并入（feature `compaction`）；对 client-adapter-api 的反向依赖用 trait 倒置修复；P16-9 四来源导入解析器收为 `import::formats` 内部模块 |
| 14 | `pawork-workspace` | workspace/core | workspace-service + file-index | 1.4k | 内部 | file-index 首次接线（`@file` 搜索消费者在 M4 落地） |
| 15 | `pawork-resources` | workspace/resources | resource-loader | 6.7k | 内部 | 包内分两层：loader 基础设施 / profiles+skills 格式契约 |
| 16 | `pawork-exec` | execution/exec | process-runtime + sandbox-runtime + pty-service | 4.9k | W1 | **对外发布主打包**（进程树 + Job Object/进程组 + Landlock/Seatbelt/AppContainer + PTY 重连）；agent-domain 类型换中性类型；按 `os/{linux,macos,windows}.rs` 重排 |
| 17 | `pawork-policy` | execution/policy | policy-engine | 1.4k | W2 | 安全内核独立小包（路径穿越/symlink/TOCTOU、shell 风险分类、审批决策）；decision 类型本地化剥离 tool-api |
| 18 | `pawork-tools` | execution/tools | tool-runtime + builtin-tools | 6.2k | 内部 | **M4 必须接入正式装配链（V1 最大缺口）**；tool_search 冻结候审（§4.4） |
| 19 | `pawork-git` | vcs/git | git-service + diff-service | 4.8k | W3 | roots 参数化解开 workspace-service 依赖；async 系统 git 封装 + worktree + 结构化 diff 对外发布 |
| 20 | `pawork-engine` | engine/engine | agent-engine + context-engine | 7.7k | 内部 | **context-engine 正式接入主循环（V1 未接）**；provider_loop.rs（3.5k 行单文件）拆子模块（turn 组装/工具派发/流事件/审批暂停恢复） |
| 21 | `pawork-mcp` | extensions/mcp | mcp-client | 4.2k | W4 | `McpPeer` 泄漏的 rmcp 类型 canonical 化，rmcp 收口到内部模块；否则独立发布承诺失效 |
| 22 | `pawork-wasm-host` | extensions/wasm-host | wasm-plugin-host + hook-runtime | 3.9k | W4 | hook-runtime（394 行、唯一消费者是它）降为 `lifecycle` 模块；wasmtime 体量大，保持独立包不并入聚合包 |
| 23 | `pawork-plugin` | extensions/plugin | plugin-package + marketplace | 7.5k | 内部 | feature `market`；三处 Ed25519+blake3 验签收敛为单一签名模块；真实 HTTP 源走 pawork-net |
| 24 | `pawork-hooks` | extensions/hooks | user-hooks | 3.7k | W4 | 注入式执行器设计保留；与 wasm-host 的 lifecycle hook 信任域不同，不合并 |
| 25 | `pawork-lsp` | extensions/lsp | lsp-runtime | 5.4k | W3 | 生态缺 async LSP client，高发布价值；resource-loader/sandbox 依赖改注入 |
| 26 | `pawork-workflow` | workflow/core | plan-service + goal-service + task-manager + automation-service + monitor-service | 6.5k | 内部 | 五合一、各域保留独立模块与独立 reducer；process 真实执行用 feature `process-exec` 门控（默认纯 reducer 不拉 exec 链）；canonical 事件类型仍在 pawork-domain，重放兼容天然安全 |
| 27 | `pawork-memory` | workflow/memory | memory-service | 0.8k | 内部 | Provider 无关记忆抽象；等真实 EmbeddingProvider 接线 |
| 28 | `pawork-review` | workflow/review | review-engine | 2.2k | 内部 | 行锚点 re-anchor + resolution 生命周期；ForgeAdapter 平台无关保留 |
| 29 | `pawork-orchestration` | agents/orchestration | orchestration + teams | 9.5k | 内部 | supervisor.rs（3.4k 行）拆模块；budget 对 ledger/tenant 的依赖 trait 化注入；TeamEvent 双通道语义保持 |
| 30 | `pawork-control-plane` | control-plane/core | tenant-service + usage-ledger + audit-log | 4.5k | 内部 | 三者同为控制面词表 + 多后端存储；rusqlite/tokio 用 feature 门控；dedup_key 索引、JSONL 审计格式保持 |
| 31 | `pawork-provider-control` | control-plane/provider-control | provider-control | 13.5k | 内部 | 保留现有 `account-control` feature 边界（lease/binding 始终可用层 vs account/routing/health 完整层）；schema 迁移从 app-database 收回本包 |
| 32 | `pawork-quota` | control-plane/quota | quota-service（核心） | 约 6k | 内部 | 只迁 domain/service/ledger 投影 + LocalLedger 适配器；约 8k 行远端适配器冻结候审（§4.4） |
| 33 | `pawork-app` | host/app | app-service + core-runtime + subscription-hub | 23k | 内部 | 应用门面 + 生命周期装配 + Event Hub 合一；aggregate/router/supervisor/team/user_hook 按模块整理；SQLite sink 下沉存储层 |
| 34 | `pawork-transport` | host/transport | transport-api + transport-local + transport-memory + transport-remote | 6.0k | W3 | feature：`local`（默认）/`memory`/`remote`（rustls 系严格锁在 remote 后）；**transport-remote-placeholder 删除**，Remote trait 上移入本包，Mock 移入 memory/testkit |
| 35 | `pawork-gui-server` | host/gui-server | gui-server + connection-manager + snapshot-service | 5.0k | 内部 | 多 GUI 运行时合一；Resume/Replay、慢客户端隔离语义与测试原样迁移 |
| 36 | `pawork-channels` | host/channels | acp-host + remote-control-adapter + client-codex-app-server + client-claude-gateway | 14.4k | 内部 | Host 侧外部通道四合一，feature：`acp`/`remote-control`/`codex`/`claude`；共享审计/gate 模式；两个未登记 members 的 crate 借此转正 |
| 37 | `pawork-cli` | host/cli | cli-host + cli-command + cli-renderer | 4.9k | 内部 | 三合一（cli-command 443 行、cli-renderer 171 行无独立理由）；六种运行模式保留 |
| 38 | `pawork-client` | clients/gui-client | gui-client | 1.8k | W4 | 外部 GUI（含 Phase 19 GPUI Desktop）唯一接入 SDK，依赖面干净，高发布价值 |
| 39 | `pawork-sdk` | clients/sdk | agent-sdk + ide-host-adapter | 6.1k | W4 | IDE adapter 是 SDK 的高级用法（feature `ide`）；连接 `pawork headless --json-stdio` |
| 40 | `pawork-compat` | clients/compat | compat-loader | 3.5k | 内部 | Claude/Codex/Grok/Cursor/Pi 配置只读导入；MCP 声明类型改薄类型依赖，摆脱 mcp→rmcp 拖带 |
| A1 | `pawork`（binary） | apps/pawork | apps/pawork | 2.7k | cargo install | composition root；清理 TEMP-P17-7-VERIFY；工具注册补 builtin |
| A2 | `protocol-probe` | apps/protocol-probe | apps/protocol-test-gui | 1.2k | 不发布 | 协议契约自检工具 |

### 4.2 拆分动作清单（包内模块级）

| 对象 | 动作 |
| --- | --- |
| `provider-runtime` | 一拆三：http/解析 → pawork-net；canonical 组装 → pawork-provider-core；reasoning protector 实现 → 宿主组装层 |
| `agent-engine::provider_loop`（3,539 行单文件） | 拆 turn 组装 / 工具派发 / 流事件处理 / 审批暂停恢复四个子模块 |
| `orchestration::supervisor`（3,440 行单文件） | 拆 spawn / registry / cancel-tree / recovery / budget-gate |
| `core-api`（2,006 行单文件） | 拆 version / command / query / event / quota / limits 六模块 |
| `resource-loader`（6.7k 行） | 包内分 loader 基础设施层与 profiles+skills 格式契约层 |
| `quota-service` | 核心与远端适配器分离（后者冻结候审） |
| `app-database` | 三套业务 schema 迁移移交 owner 包，本体纯化为 pawork-sqlite |
| `session-store` 导入器 | 四来源解析器（纯函数）收为 `import::formats` 模块；事务写入留在 store 内 |

### 4.3 删除与不迁移清单

- `transport-remote-placeholder`（842 行）：trait 上移 pawork-transport 后删除。
- `benches/` no-op 占位（7 文件）：不迁移，V2 需要基准时重建。
- 注册表幽灵项：`agent-api`、`provider-bedrock`、`provider-mistral`（从未创建，V2 注册表不再出现）。
- `scripts/p15-gate.sh`、`p16-gate.sh`、`p18-gate.sh`、`phase17-host-gate.sh`：门禁脚本不迁移（§6）。
- 各 crate 中已知死代码：V1 评审已标记的 deferred-consumer API 在迁移时逐项决定「接线或删除」，不允许原样搬运。

### 4.4 冻结候审清单（留在 V1 目录，不迁移，按需激活）

| 资产 | 行数 | 激活条件 |
| --- | --- | --- |
| quota 六厂商远端适配器 + WebScrape + refresh scheduler | 约 8k | 远端额度监控有真实用户需求且 P18 账号归属落地 |
| `browser-computer-runtime`（driver 全 Stub） | 3.5k | 真实 Local/Playwright driver 落地 |
| `tool-runtime::tool_search`（feature 门控） | 1.2k | 工具目录规模达到需要动态发现的量级 |

## 5. 独立发布策略

### 5.1 命名与元数据

- 所有库包 `pawork-` 前缀；二进制名 `pawork` 不变。
- workspace 根 `[workspace.package]`：version 统一 `0.1.0` 起步、edition 2021、rust-version 1.85、license 待定（见 [../ROADMAP.md](../ROADMAP.md) §4 未决事项，crates.io 发布硬前置）、repository/keywords/description 逐包补齐。
- V1 的 `publish = false` 全局禁发不再沿用：V2 默认 `publish = false`，进入发布波次的包逐个翻转。

### 5.2 发布波次与前置链

发布顺序严格按依赖方向，每一波内部可并行：

| 波次 | 包 | 特征 |
| --- | --- | --- |
| W1 | `pawork-exec`、`pawork-net`、`pawork-sqlite`、`pawork-config`、`pawork-domain` | 零内部前置（exec/net/sqlite/config 完全自含；domain 仅 serde），通用价值最高，可最先占名 |
| W2 | `pawork-api`、`pawork-protocol`、`pawork-policy`、`pawork-diagnostics` | 前置 domain |
| W3 | `pawork-provider-core`、`pawork-providers`、`pawork-auth`、`pawork-git`、`pawork-lsp`、`pawork-blob-store`、`pawork-transport` | 前置 W1/W2；构成「用 Pawork 的件搭自己的 Agent」的最小材料包 |
| W4 | `pawork-client`、`pawork-sdk`、`pawork-mcp`、`pawork-wasm-host`、`pawork-hooks`、`pawork-testkit` | 接入生态；前置 protocol/api 稳定 |
| 不发布 | 其余「内部」包 | 保持发布卫生，等有外部需求再评估 |

### 5.3 SemVer 与协议版本

- 0.x 阶段全线 minor 递进，不承诺 API 稳定；多厂商 feature 包接受「任一厂商 breaking 撞整包版本」（0.x 代价可接受）。
- 协议版本与 crate 版本解耦：`API_VERSION`/GUI 握手协商版本沿用 V1 机制（ADR-036），在 pawork-protocol 内维护「协议版本 × 包版本」映射表。
- rmcp（`=2.2.0` 锁定）与 wasmtime（27）这类重依赖只出现在各自独立包，升级不波及全 workspace。

## 6. 开发期测试与验证策略（关键测试、无门禁）

### 6.1 保留的三类关键测试（随代码迁移，缺失则补）

1. **安全红线定向回归**：路径越界/symlink/TOCTOU（policy）、shell 参数注入（git/exec）、Secret 不落库不入日志（session/auth/diagnostics）、进程树清理（exec）、sandbox fail-closed 降级。
2. **持久化与重放契约**：AgentEventEnvelope serde golden、`session_events` append-only、各 SQLite schema 升级迁移、blob `PWB1` 格式、usage dedup_key、audit JSONL——这些是磁盘契约，V2 只动代码组织不动格式，golden 测试**先于实现迁移**。
3. **协议与解析 golden/fuzz 种子**：SSE/JSONL/partial-JSON 解析（proptest 种子集原样迁）、gui-protocol 帧 golden、Provider contract 每厂商 1–2 条最小 golden、GUI Resume/Replay 语义测试。

执行方式：只跑触碰包 `cargo test -p <pkg>`（多包用多个 `-p`）；提交前无任何强制检查。

### 6.2 明确不做（开发期）

- 无 Workspace Full Gate；无 L0–L3 分级与升级审批；无 `clippy -D warnings` 强制；无 `rustfmt --check` 门禁；无 schema drift CI；无三平台矩阵（平台特定代码只在改动时于当前平台验证，交叉编译 check 可选）；无每簇 review + remediation 循环；无门禁脚本；无覆盖率要求；无 cargo-machete/udeps。

### 6.3 Release Hardening 一次性清单（原 M8，由 S12 承接）

功能完备核对通过后集中执行：workspace 全量 build/test/clippy/fmt；三平台矩阵（Windows/macOS/Linux 真实 runner，含 sandbox/PTY/Named Pipe 定向）；解析器与安全路径 fuzz 扩展（cargo-fuzz：路径解析、unified diff、shell 分类、SSE/partial-JSON）；schema/typegen 校验接回 CI；依赖卫生（machete/udeps/audit）；license inventory；crates.io 发布 dry-run；安全验收清单（沿用 [../../docs/quality/security-acceptance.md](../../docs/quality/security-acceptance.md) 框架裁剪）。

## 7. 风险与缓解（重构总体）

| # | 风险 | 缓解 |
| --- | --- | --- |
| 1 | **磁盘/线上契约破坏**：event envelope（schema_version=1）、gui-protocol 帧（ADR-036）、5+ 套 SQLite schema 与迁移序列、blob `PWB1`、audit JSONL、usage dedup_key | 全部 golden/迁移测试先于实现迁移；V2 明确「只动代码组织，不动 wire/存储格式」；M3 验收含「V1 库文件直接打开升级」 |
| 2 | **import 重写规模**：agent-domain 约 76 个反向依赖，合并改名波及全仓 | 机械替换脚本 + 逐包迁移即编译验证；合并映射表（§4.1）是唯一事实源 |
| 3 | **feature 组合爆炸**：providers 8 厂商、transport 3 通道、workflow process-exec 等 | 开发期只测 default 与 `--all-features` 两档；M8 再补关键组合矩阵 |
| 4 | **双线维护漂移**：V2 迁移期 V1 继续演进会导致移植目标漂移 | 本路线图生效后 V1 冻结为只收安全修复；P18/P19 新功能直接在 V2 做（Phase 19 GPUI Desktop 经 pawork-client 在 V2 启动） |
| 5 | **合并引入的边界腐蚀**：crate 边界消失后依赖方向靠自觉 | §2.3 新红线 + 包内模块可见性（`pub(crate)`）+ M8 workspace lint（如 cargo-deny/自定义 xtask 检查依赖方向） |
| 6 | **「无消费者不合入」拖慢横向进度** | 这是有意取舍：允许 experimental feature 门控合入，但必须显式登记，杜绝 V1 式静默库存 |
| 7 | **重依赖编译成本**（wasmtime、rmcp、rustls、scraper） | 各自锁在独立包/feature 后；默认构建路径不含它们 |

## 8. 附录：Review 方法与数据来源

- 原 ROADMAP_V2.md 基于 2026-08-14 的全仓审查：主代理完成依赖图扫描（全部 Cargo.toml path 依赖边）、逐 crate 行数统计（PowerShell 实测 236,177 行/572 文件）、零消费者验证与生产装配链核查；7 个并行子代理按功能域分组完成 88 crate 的逐个审查（职责验证、规模、依赖、测试、成熟度、合并/拆分/发布价值建议），主代理交叉验证后综合成文。
- 关键佐证文档：[../../REVIEW.md](../../REVIEW.md)（「组件齐全、主干未通电」系统性发现及各 Phase V 项）、[../../plan/README.md](../../plan/README.md)（V1 Phase 15–18 延期落点登记）、[../../docs/review/](../../docs/review/)（各 Phase 行号级证据）。
- 行数与依赖数据为 2026-08-14 快照；执行迁移前如 V1 有增量提交，以当时实态复核映射表。
