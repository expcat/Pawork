# S12 CR-01 审查报告：manifest 与包布局契约

| 项 | 值 |
| --- | --- |
| CR 编号 | CR-01 |
| 主审范围 | workspace 根 `Cargo.toml`、全部 39 个成员 crate 的 `Cargo.toml`、`docs/design.md` §2 包布局与 §3.2 冻结契约、`docs/v1-migration-reference.md` §3/§4.1 依赖方向基准 |
| 审查日期 | 2026-08-17 |
| 主审模型 | GLM（zai/glm-5.3） |

## 实际审查路径

- `Cargo.toml`（workspace 根：members glob、workspace.package、workspace.dependencies 全文）
- 全部 39 个成员 `Cargo.toml`（`cargo metadata --no-deps` 枚举后逐个通读：agents/orchestration、apps/desktop、apps/pawork、apps/protocol-probe、clients/{compat,gui-client,sdk}、control-plane/{core,provider-control,quota}、engine/engine、execution/{exec,policy,tools}、extensions/mcp、foundation/{api,config,diagnostics,domain,protocol,sqlite,testkit}、host/{app,channels,cli,gui-server,transport}、net/net、providers/{adapters,auth,core}、storage/{blob,session}、vcs/git、workflow/{core,memory,review}、workspace/{core,resources}）
- `docs/design.md` §2（激活映射表）、§3.1/§3.2（终局布局与冻结契约）
- `docs/v1-migration-reference.md` §3（终局目录结构）、§4.1（40 包 + 2 应用映射总表，依赖方向基准）
- `docs/task-guide.md` §3.1（架构红线）、`ROADMAP.md` §3.2（K-01～K-10 已知基线）、§4（未决事项，feature 激活条件登记）
- `plan/S12-project-code-review.md` 全文（finding 格式与纪律）
- 依赖证据源码抽查：`storage/session/src/client_adapter.rs`、`foundation/diagnostics/src/lib.rs`、`foundation/api/src/`（lib.rs/tool.rs）、`foundation/config/src/{schema.rs,loader.rs}`（api_key 契约抽查）、`foundation/domain/src/events.rs`（AgentEvent 变体计数）
- 工具命令（仅 manifest 解析，未编译）：`cargo metadata --no-deps`、`cargo tree -p <crate> -e normal / -i ts-rs`、`rg --files`（全仓 Cargo.toml 清单核对）

## 核对结论（无违约项）

1. **布局与激活映射**：design §2 终局 40 包 + 3 应用，扣除 4 个明确不激活包（wasm-host/plugin/hooks/lsp）应为 39 个活跃 workspace 成员；`cargo metadata` 实测 39 个，目录、crate 命名（`pawork-` 前缀、`pawork`/`protocol-probe`/`pawork-desktop` 三应用）与 §2/§3 表逐项一致。`rg --files -g 'Cargo.toml'` 全仓仅 40 份（根 + 39 成员），无游离 manifest、无未登记成员。
2. **canonical 纯净（红线）**：`cargo tree -p pawork-domain -e normal` 闭包仅 serde/serde_json；`pawork-api` 闭包仅 pawork-domain + async-trait/serde/serde_json/thiserror。domain/api 均不依赖 GUI framework、SQLite、HTTP Client、Keychain、Git、任何具体 Provider。
3. **Desktop 不链 Core（红线）**：`cargo tree -p pawork-desktop -e normal` 中无 engine/providers/provider-core/auth/app/tools/policy/exec/mcp/session/sqlite/git/workflow/orchestration/control-plane 等 Core crate；Desktop 仅经 pawork-client 消费 protocol/transport。
4. **无循环依赖**：39 个成员的内部依赖边（metadata 全量导出审计）构成 DAG；cargo metadata 正常解析本身即排除 normal 依赖环，dev-dependencies 亦无环（gui-client dev→app 为单向）。
5. **引擎不依赖 workflow**：`pawork-engine` 仅依赖 domain/api，符合 design §2「S11 Plan gate 在 host，engine 不依赖 workflow」。
6. **provider-core 边界**：`pawork-provider-core` 仅依赖 domain/api，符合 §4.1 #9「砍掉对 blob store 的依赖」。
7. **冻结契约抽查**：`foundation/domain/src/events.rs:98` `AgentEvent` 实测 32 个变体，与 §3.2「32 变体」一致；`foundation/config/src/schema.rs` ProviderConfig 无 api_key 字段且 extra 反序列化时剥离（schema.rs:54-71、loader.rs:221/321/356），与「无 api_key 字段」契约一致。
8. **feature 门控与 ROADMAP §4 登记对齐**：diagnostics `experimental`（lib.rs:13-27 cfg 门控）、provider-control `account-control-v1`（host/app 以 default-features=false 关闭，激活条件已登记）、workflow `process-exec`、transport `remote`（S10 任务书 §明确不做：「remote feature 编译与本机回环测试为限」）、blob `protected`/session `compaction` 均为默认关闭或按需启用，未发现「静默库存」。

## Findings

### S12-CR01-01

- **类别**：Maintainability（冻结契约/迁移词典漂移）
- **严重度**：Medium　**置信度**：Confirmed
- **证据**：
  - `storage/session/Cargo.toml:18`：`pawork-protocol = { path = "../../foundation/protocol", default-features = false, features = ["adapter"] }`
  - `storage/session/src/client_adapter.rs:3-8`：直接 `use pawork_protocol::adapter::{AdapterError, CapabilitySnapshot, ClientProtocol, ClientSessionId, ClientSessionRecord, ClientSessionState, RegistryWriteOutcome, SessionRegistryStore}`，`SqliteClientSessionRegistryStore` 以 protocol 所属类型实现 `SessionRegistryStore` 并持久化 `client_adapter_sessions` 表。
  - 基准：`docs/v1-migration-reference.md:175`（§4.1 #13 关键动作）：「对 client-adapter-api 的反向依赖用 **trait 倒置**修复」。
  - **实际行为**：V1 的 session-store → client-adapter-api 反向依赖未被 trait 倒置消除，而是随 client-adapter-api 并入 `pawork-protocol::adapter` 后原样保留为 storage/session → foundation/protocol 的直接依赖与类型耦合。
  - **期望行为**：按 §4.1 #13，trait 与持久化记录类型应由 session（或 domain）侧定义，adapter/protocol 层实现映射；storage 层不应依赖客户端协议层的记录形状。
  - **影响面**：GUI 协议的 client session record / capability snapshot 形状变更会直接波及 session store 的表结构与序列化代码；迁移词典承诺的解耦动作未兑现，后续按 §4.1 审计会持续产生偏差。无直接安全/数据风险（内部包，功能正常）。
- **验证建议**（S12 内不执行）：整改任务中先以 `rg "pawork_protocol" storage/session/src` 确认耦合面收敛于 client_adapter.rs，再补一条「protocol 类型变更不触碰 storage 代码」的定向回归（trait 倒置后以编译边界断言）。
- **整改边界**：最小写入集 = `storage/session/src/client_adapter.rs` + trait/记录类型新归属 crate + host 装配点适配；不可顺带改 GUI 协议帧语义或 session DDL 之外的存储行为。需先拍板 trait 归属（session vs domain），如维持现状则应改写 §4.1 #13 动作并留 ADR。

### S12-CR01-02

- **类别**：Requirement Gap（文档承诺的预留面缺失）
- **严重度**：Low　**置信度**：Confirmed
- **证据**：
  - `ROADMAP.md:124`（§4 扩展生态整族）：「`pawork-api` 预留 `plugin` feature 但不激活」；`docs/design.md:30`：「`plugin` feature **预留不激活**」；`docs/v1-migration-reference.md:164`（§4.1 #2）：「feature：`provider`/`tool`/`plugin`」。
  - **实际行为**：`foundation/api/Cargo.toml` 的 `[features]` 仅 `default = ["provider", "tool"]`，不存在 `plugin` feature；`foundation/api/src/` 仅 lib.rs 与 tool.rs。
  - **期望行为**：按三处文档，api 应保留 `plugin` feature 占位（默认不激活）。
  - **影响面**：类型级预留已兑现（`foundation/domain/src/ids.rs:68` `PluginId`、`foundation/domain/src/tool.rs:144` `ToolCapability::ExternalPlugin`、policy 审批文案存在），实际风险仅是「预留 feature」承诺与 manifest 不符；重启 plugin-api 时需补 feature（机械成本极低）。
- **验证建议**：无需运行验证；整改时 `cargo metadata --no-deps` 断言 feature 列表即可。
- **整改边界**：最小写入集 = `foundation/api/Cargo.toml`（加空 `plugin = []`）或改 ROADMAP/design 措辞为「届时再加 feature」；二选一，不可顺带迁移 plugin-api 类型。

### S12-CR01-03

- **类别**：Performance / Maintainability（feature 组合缺陷）
- **严重度**：Low　**置信度**：Confirmed
- **证据**：
  - `foundation/protocol/Cargo.toml:24`：`pawork-domain = { path = "../domain", features = ["typegen"] }` —— 无条件开启 domain 的 typegen；`foundation/protocol/Cargo.toml:28`：`ts-rs.workspace = true` 非可选。
  - `foundation/domain/Cargo.toml:12,17`：typegen 设计为可选（`ts-rs` optional）；基准 `docs/v1-migration-reference.md:163`（§4.1 #1）：「`typegen` feature 保持可选」。
  - `cargo tree -p pawork -i ts-rs`：ts-rs v11.1.0 经 pawork-domain 进入生产 `pawork` 二进制图（protocol 无条件激活 domain:typegen 所致）。
  - **实际行为**：凡引入 `pawork-protocol` 的图（pawork、desktop、sdk、client、gui-server 等全 workspace）都强制编译 ts-rs 与 domain 的 typegen 展开代码；protocol 自己的 `typegen` feature（Cargo.toml:16）反而为空壳。
  - **期望行为**：domain「typegen 保持可选」；生产装配链默认不带 ts-rs，仅 typegen bin 场景启用。
  - **影响面**：运行时与二进制体积影响经死代码消除后接近零，主要是全 workspace 编译时间与 feature 语义混乱（可选性被架空、空 feature 误导）。
- **验证建议**：整改后跑 `cargo tree -p pawork -i ts-rs`（应无结果）与 `cargo tree -p pawork-protocol --features typegen -i ts-rs`（应出现）；S12 内不执行。
- **整改边界**：最小写入集 = `foundation/protocol/Cargo.toml`（`typegen = ["dep:ts-rs", "pawork-domain/typegen"]`，domain 依赖去掉无条件 features）；不可顺带改 typegen 输出路径或 schemas/ 再生成逻辑。

### S12-CR01-04

- **类别**：Maintainability（workspace 依赖集中化被绕过）
- **严重度**：Low　**置信度**：Confirmed
- **证据**：
  - `execution/policy/Cargo.toml:16`：`regex = "1"`；`execution/tools/Cargo.toml:21,23`：`ignore = "0.4"`、`regex = "1"`；`providers/adapters/Cargo.toml:30,34`：`tracing = "0.1"`、dev `wiremock = "0.6"`。
  - 以上四项均已在根 `Cargo.toml` workspace.dependencies 登记（`Cargo.toml:51,59,60,64`）。
  - **实际行为**：成员 manifest 内联版本声明，绕过 workspace 统一管理；当前版本号与根声明一致，尚无实际版本漂移。
  - **期望行为**：已进入 workspace.dependencies 的依赖由成员以 `workspace = true` 引用。
  - **影响面**：未来根版本升级不会传播到这五处，存在静默分叉风险；纯维护性，无行为差异。
- **验证建议**：整改后 `cargo metadata --no-deps` diff 确认依赖解析不变。
- **整改边界**：最小写入集 = 上述三个 Cargo.toml 共五行改 `workspace = true`；不可顺带升级版本或新增依赖。

### S12-CR01-05

- **类别**：Maintainability（manifest 与迁移词典漂移）
- **严重度**：Low　**置信度**：Confirmed
- **证据**：
  - `docs/v1-migration-reference.md:192`（§4.1 #30 关键动作）：「rusqlite/tokio 用 feature 门控」。
  - `control-plane/core/Cargo.toml:22-23`：rusqlite 已 optional（`sqlite` feature 门控），tokio 为无条件依赖。
  - **实际行为**：仅 rusqlite 被门控，tokio 始终编译。
  - **期望行为**：按词典动作两者均应 feature 门控。
  - **影响面**：极小——该 crate 为 async 设计，全门控实用性低；属文档与 manifest 的低风险不一致，宜以修订词典措辞收口而非强行门控。
- **验证建议**：无需运行验证。
- **整改边界**：最小写入集 = `docs/v1-migration-reference.md` §4.1 #30 措辞（或 `control-plane/core/Cargo.toml` 补门控，二选一）；不可顺带改 audit/ledger 行为。

## 未覆盖路径与原因

- 各 crate `src/` 内部实现逻辑（安全、持久化、协议行为等）：属 CR-02～CR-08 主审范围；本包仅读取定位依赖与契约证据所需的最小源码片段。
- `Cargo.lock` 逐条第三方版本/漏洞/许可证审计：S12 任务书未授权 cargo audit 类检查；`cargo tree` 仅用作依赖图事实源。
- feature 矩阵的编译级验证（如 `--no-default-features` 组合）：S12 禁止 build/check，本报告止于 metadata 解析级证据。
- design.md §3.2 冻结契约的字段级全量核对（Provider 13 变体、会话 DDL、PWB1、GUI 帧字段）：分别属 CR-04/05/07 主审；本包仅完成布局级抽查（AgentEvent 32 变体、config 无 api_key，均通过）。
- `docs/gui-design.md` 与 design/ 视觉资产：属 CR-08。

## 统计

| 严重度 | 条数 |
| --- | --- |
| Critical | 0 |
| High | 0 |
| Medium | 1 |
| Low | 4 |

| 置信度 | 条数 |
| --- | --- |
| Confirmed | 5 |
| Needs Verification | 0 |
