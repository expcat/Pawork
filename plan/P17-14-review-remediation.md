# P17-14：Phase 17 评审修复（REVIEW remediation）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟢已完成 · 交付成熟度：TargetVerified（八项review修复+两项门禁衍生可靠性修复已落地；定向门禁数字已回填：9 crate test 371/0/0、clippy 0 warnings、`rustfmt --edition 2021 --check --config skip_children=true`（26 文件）通过、38 个 test/doc summaries 已核对）· 依赖：P17-1 ~ P17-13

**最终目的**：按 [docs/review/p17-review.md](../docs/review/p17-review.md) §3（P0 确定性产品错误）、§4（主流程缺口与状态失真）、§5（冗余与过度设计）、§7（收敛顺序），先让所有成功响应都代表真实、可持续的副作用：修复 P17-11 remote 生命周期（endpoint / token / credentials 由执行 `remote publish` 的长驻 pawork 进程持有，publish 长驻至 SIGINT 并清理 token；跨进程仅 connect / reconnect，独立 unpublish/revoke 无共享控制面一律 fail-closed 并延后 P19-14），消除假成功（placeholder 命令、Plugin/MCP 列表、隔离下的 no-op 工具执行一律 fail-closed），收敛状态失真（Profile 未支持引用 fail-closed、Teams 不再在正式宿主启动时无条件打开 DB），删除重复权威与死抽象（placeholder remote contract 收回 `transport-api`、Browser 纯别名 helper 删除、Compat `apply` 更名收缩为名实相符的 `export_plan`），并固化 JSON stdout 契约（日志一律写 stderr，协议帧独占 stdout）。未闭环的纵向能力（Teams ingress、Marketplace/Plugin 真实资源、Profile 引用维度真实消费、远程外部可达、ACP 降级审计）显式延后到 P18-13 / P18-15 / P19-11 / P19-13 / P19-14 落点。本任务不新增 facade / crate，不扩 schema；定向门禁已复跑并回填数字（9 crate `cargo test` 371 passed / 0 failed / 0 ignored，clippy `--all-targets -- -D warnings` 0 warnings，26 个 Rust 文件 `rustfmt --edition 2021 --check --config skip_children=true` 通过，`git diff --check` 通过，38 个 test/doc summaries 已核对），任务状态 `🟢已完成 · TargetVerified`。

**涉及范围**：`apps/pawork`（pawork 进程 remote 装配（serve 与 `remote publish` 共享同一 transport）、JSON stdout 契约、teams.sqlite 不随启动创建）、`cli-host`（placeholder 命令 fail-closed、`--json` 错误帧）、`app-service`（`ServiceOperation::Placeholder` / `PluginList` / `McpList` fail-closed、Profile 未支持引用、隔离 no-op 工具失败）、`transport-api`（收回 remote provider/connector 契约）、`transport-remote-placeholder`（收缩为短期 re-export 兼容层 + Mock/测试支持）、`transport-remote`（endpoint/token 生命周期与 drop 清理）、`browser-computer-runtime`（删纯别名 helper）、`compat-loader`（`apply` → `export_plan`）；新增 `apps/pawork/tests/remote.rs`（跨进程 publish/长驻/清理 e2e）与 `apps/pawork/tests/teams_state.rs`（正常 CLI 启动不创建 teams.sqlite）；新增 [docs/review/p17-review.md](../docs/review/p17-review.md) 与本 plan，并在 [plan/README.md](README.md) 增量登记 Phase 17 延期落点。本任务写入集为 `plan/P17-14-review-remediation.md`、`docs/review/p17-review.md`、`ROADMAP.md`、`REVIEW.md` 与 `plan/README.md`（代码修复为既有工作区改动，不在本任务重复变更）。

## 处置策略（按评审 §7 矩阵）

- **已修（落地）**
  - **§3.1 / §7-P0-1 Remote 长驻生命周期 + token 清理（P17-11）**：`RealRemoteTransport` 与 `TokenStore`（instance 目录 `remote.token`）在 [`apps/pawork/src/main.rs`](../apps/pawork/src/main.rs) 装配，端点由执行 `remote publish` 的长驻 pawork 进程经 `ServeGuiHost::bind_remote` 实际 bind / accept，publish 保持长驻直至收到 SIGINT；跨进程仅 connect / reconnect，SIGINT 触发端点关闭与 token 清理。独立 unpublish / revoke 命令没有共享控制面，无长驻 host 时一律 fail-closed；不声称可跨进程操控运行中的 publish 进程（跨进程 unpublish / revoke），共享控制面与外部可达地址一并延后 → P19-14。`EndpointState::drop` 幂等删除其自建 endpoint token 文件，transport drop 只清本实例登记过的端点（`transport_drop_deletes_owned_endpoint_token` 回归）。跨真实进程 e2e：[`apps/pawork/tests/remote.rs`](../apps/pawork/tests/remote.rs)（长驻 → 跨进程 connect / reconnect → SIGINT → token 清理、清理后同名再发布不冲突、无长驻 host 的 unpublish/revoke fail-closed）。边界：bind 仍为 loopback 临时端口（开发/测试默认）；**外部可达地址（NAT 穿透 / relay）与共享控制面显式延后 → P19-14**。
  - **§3.2 / §7-P0-2 假成功消除（fail-closed）**：`ServiceOperation::Placeholder` 改为失败响应（不再 `ok: true`）；`AppQuery::PluginList` / `McpList` 返回 `AppServiceError::Unavailable`（不再固定空数组）；隔离 profile（Restricted/Container）下的 P13-1 no-op 工具执行返回 `ToolResult::failure(ErrorCategory::Unavailable)`，不再生成成功 `ToolExecutionCompleted`。CLI 层 `placeholder_commands_fail_closed_with_json_error`：plugin / mcp / import 等未接线命令退出码非 0，`--json` 输出 `ok=false` + 错误帧。
  - **§4.2 Profile 未支持引用 fail-closed**：run 解析时 `unsupported_profile_refs` 按固定顺序汇总 `skills` / `mcp` / `permissions` / `hooks`，任一非空即返回 `Unavailable`，绝不静默携带未兑现的配置维度。工具 rule deny-first 与 Restricted/Container 拒绝执行是评审认定的正确行为，保留。
  - **§4.3 Teams 不持久装配**：`team_db_path` 保持 `CoreRuntimeConfig` 默认 `None`，正式宿主启动不再无条件打开 `teams.sqlite`（`normal_cli_startup_does_not_create_teams_sqlite` 回归）；TeamService / SQLite append-replay / EventHub 镜像保留为 durable library，canonical TeamCommand/TeamQuery ingress **显式延后 → P19-13**。
  - **§5 remote placeholder contract 归 `transport-api`**：provider / connector 契约（`RemoteGuiTransportProvider` / `RemotePublishHandle` / `RemotePublishRequest` 等）收回 [`transport-api`](../crates/transport-api/src/lib.rs)；`transport-remote-placeholder` 收缩为评审允许的「短期 re-export 兼容层 + `MockRemoteTransport` 测试支持」（crate 描述改为「契约见 transport-api；P13-6 / P17-14」），不再承载生产契约定义。
  - **§5 Browser 纯别名 helper 删除**：`reject_hosted_for_local` 等无生产调用、无独立语义的别名 helper 已删除（全仓无引用）；三执行位点、Sandbox-before-driver、ProviderHosted 不进本地 execute 的安全不变量保留。四套 backend wrapper 的进一步收缩待第一个真实 backend 出现（评审 §7-P2-8）。
  - **§5 Compat `apply` → `export_plan`**：[`compat-loader`](../crates/compat-loader/src/apply.rs) 入口更名收缩为显式幂等的 `export_plan`（计划文件名 + 幂等指纹），只把 canonical 计划写入调用方指定输出目录，绝不执行 hook / MCP、不应用任何资源；「真实应用」留给未来 ResourceLoader/AppService composition 消费（落点并入 P19-11）。
  - **JSON stdout 契约（日志一律 stderr）**：`pawork` `--json` / ACP / Headless 路径 stdout 只承载协议帧，tracing / 日志统一写 stderr，`--json` 输出可整体解析为纯 JSON（cli.rs 断言）；这是 remote publish JSON 输出可机读与 §3.1/§3.2 错误语义可被自动化正确消费的前置。

- **门禁衍生可靠性修复（两项；定向门禁复跑暴露，非评审 §3–§5 原始八项）**
  - **RateLimiter 自动冲刷返回值消费**：`RateLimiter::push` 在窗口到期 / 缓冲超限时返回的自动冲刷结果，曾被三个生产调用点（quota alert 广播、Team 事件镜像、worker 事件观察）直接丢弃——跨窗事件静默丢失；新增 [`rate_limit.rs`](../crates/app-service/src/rate_limit.rs) `enqueue`：本次入队触发的自动冲刷结果重排入有界 ready 队列、由下一次 `flush` 发出（不丢不重；ready 与合并缓冲同界，极端过载丢最旧并计入 `dropped_events`）；[`supervisor.rs`](../crates/app-service/src/supervisor.rs) 三个生产调用点全部改走 `enqueue`；zero-window（`Duration::ZERO` 每次入队先冲刷已缓冲事件）语义一并固化回归。
  - **remote 认证后同步 subscribe 传 pump + carrier 联合捕获**：[`service.rs`](../crates/remote-control-adapter/src/service.rs) 认证完成后同步 `subscribe()` 并把订阅传入 pump 任务，Replay 帧严格晚于订阅建立（确定性订阅时点，锁死「认证成功到订阅之间丢失事件」竞态）；[`transport_remote_carrier.rs`](../crates/remote-control-adapter/tests/transport_remote_carrier.rs) 新增 `rpc_capture` carrier helper：单一接收循环联合捕获 RPC 响应与 RunFinished 通知（到达顺序任意仅指 RPC 响应与命中通知；capture 谓词命中的 RunFinished 帧会缓存，不被等待 RPC 响应的循环丢弃）。修前压力测试有失败；修后 30/30 exact + 10/10 整 target 通过。

- **状态 / 事实纠正（评审 §0 / §4.1 / §8）**
  - Phase 17 不再以「13/13 Accepted / 统一绿色」自述：按评审 §8 四态矩阵终态校准——HostWired：P17-1 / P17-7 / P17-8（模型 Run 受 Provider 宿主前置条件限制）；PartialWired：P17-5 / P17-11（P17-5 收敛为部分字段生效；P17-11 修复后解除 lifecycle blocked，但仅 loopback 且无共享控制面，保持 Partial）；LibraryBuilt：P17-2 / P17-3 / P17-4 / P17-6 / P17-10 / P17-13（P17-6 收敛为 durable library）；AdapterBuilt：P17-9 / P17-12。
  - ROADMAP、REVIEW、docs/review/p17-review.md 的状态矩阵已按上述终态同步（Phase 17 14/14，总计 220/189）；本 plan 与 plan/README.md 登记同步落地。

- **保留（评审允许 / 判定不改）**
  - `transport-remote-placeholder` 保留为短期 re-export 兼容层（评审 §5 明示「必要时只保留短期 re-export 兼容层」），待消费者迁移完成后删除。
  - Restricted / Container 无真实执行器时拒绝运行（§4.2「正确行为，应保留」）。
  - §5.1 不建议合并的边界全部维持：User Hooks / WASM Hook Runtime、ACP / Headless / GUI 三 adapter、Plugin Package / Marketplace 分层、LSP Client / IDE adapter、P16-9 Session Import 与 P17-13 Config Import。

- **显式延后（纵向闭环与再认定，不在本任务）**
  - **ACP 降级能力审计 → [P18-13](P18-13-audit-otel.md)**（§1 P17-7 / §7-P2-7）：`ACP_SUPPORTED_CAPABILITIES` 为空导致的降级结果目前只留在内存查询，写入 canonical audit event / structured trace 由 P18-13 落地，不新增协商服务。
  - **Phase 17 host-wired 再认定与功能簇门禁 → [P18-15](P18-15-control-plane-gate.md)**（§4.1 / §8）：模型 Run 前置条件（正式宿主 Provider 注册，P15-10 延期项 ② → P18-3）闭合后，P17-1 / P17-7 / P17-8 的 Product usable 再认定、历史 L1/L2 证据与当前可达性对账、跨 crate 不变量集中验证由 P18-15（依赖含 P17-7）作为 `MaintenanceGated` 收口。
  - **Marketplace / Plugin 真实纵向 + Profile 引用维度消费 → [P19-11](P19-11-resources-extensions.md)**（§4.2 / §4.4 / §7-P1-4 / §7-P1-5）：一个真实 source + 一种资源的最小闭环、Plugin/MCP 真实列表接线、`profile.skills/mcp/permissions/hooks` 由 ResourceLoader 结果映射到既有 Hook/MCP/Skill/Policy 入口；Compat `export_plan` 的真实应用（ResourceLoader 消费 canonical plan）一并落此。
  - **Teams canonical ingress → [P19-13](P19-13-multi-agent-teams.md)**（§4.3 / §7-P1-4）：最薄 TeamCommand / TeamQuery + worker presence 桥（`observe_worker_events` 生产调用者）；有真实 ingress 后再收敛 18 变体 public event 镜像。
  - **Remote 外部可达 + Remote Control pairing → [P19-14](P19-14-multi-window-remote.md)**（§3.1 / §4.5 / §7-P1-6）：外部可达地址 / relay 作为 transport 配置接入（loopback 保留测试/开发模式）；pairing credential 签发 / 持久化 / 撤销复用 `client-auth` / auth-service 与长生命周期 transport owner，adapter 只做 capability gate。

## 细分步骤（分组）

### A. Remote 生命周期与 token 清理（评审 §3.1 / §7-P0-1）

1. **长驻 publish 进程装配**：`RealRemoteTransport` + `TokenStore` 在 `pawork` 进程创建，执行 `remote publish` 时经 `ServeGuiHost::bind_remote` 承载 publish / accept；publish 长驻至 SIGINT，跨进程仅 connect / reconnect。目的：消除「命令返回即进程退出、端点随之消失」的生命周期错误。
2. **token drop 清理**：`EndpointState::drop` 幂等删除自建 token 文件，transport drop 只清本实例登记端点。目的：消除遗留凭证与同名再发布撞 `create_new` 的确定性故障。
3. **跨进程 e2e**：`apps/pawork/tests/remote.rs` 三条回归（长驻 → 跨进程 connect / reconnect → SIGINT → token 清理、同名再发布不冲突、无 host 的 unpublish/revoke fail-closed）。目的：覆盖评审指出的「现有 e2e 只在单测试进程内持有 transport」盲区。

### B. 假成功消除：fail-closed（评审 §3.2 / §7-P0-2）

4. **命令面**：placeholder 命令与 Plugin/MCP 列表一律 `Unavailable` / 非零退出码。目的：调用方（CLI / SDK / 自动化 / 模型）不再把「无副作用」误认成「已完成」。
5. **工具面**：隔离 profile 下的 no-op 工具执行返回失败 ToolResult。目的：成功 `ToolExecutionCompleted` 只对应真实副作用。

### C. 状态失真收敛（评审 §4.2 / §4.3）

6. **Profile 引用 fail-closed**：run 解析汇总非空 skills / mcp / permissions / hooks 即拒。目的：未兑现配置维度不静默携带。
7. **Teams 不持久装配**：`team_db_path` 默认 `None` + 启动不创建 teams.sqlite 回归。目的：durable library 保留、无条件打开 DB 的副作用消除。

### D. 重复权威与死抽象删除（评审 §5）

8. **contract 归 transport-api**：契约上移、placeholder 收缩为 re-export + Mock。目的：概念与事实一致，生产依赖不再指向名为 placeholder 的契约 crate。
9. **Browser 别名 helper / Compat `export_plan`**：删纯别名、更名收缩。目的：API 表面积与名实相符。

### E. JSON stdout 契约与文档登记

10. **日志 stderr / 协议帧 stdout**：`--json` / ACP / Headless 全路径固化，测试断言 stdout 纯 JSON。目的：机读契约稳定。
11. **文档登记**：本 plan + README 延期落点登记；门禁数字回填前不标绿。目的：状态可追溯、不提前声明验收。

## 主要产出物

- `apps/pawork`：pawork 进程 remote 装配（serve 与 `remote publish` 共享同一 transport）与 JSON stdout 契约；`tests/remote.rs`（跨进程 e2e 三条）、`tests/teams_state.rs`、`tests/cli.rs`（placeholder fail-closed + 纯 JSON stdout 断言）。
- `app-service`：Placeholder / PluginList / McpList fail-closed、`unsupported_profile_refs`、隔离 no-op 工具失败。
- `transport-api` + `transport-remote-placeholder` + `transport-remote`：契约归属、re-export 兼容层、endpoint/token 生命周期。
- `browser-computer-runtime`、`compat-loader`、`cli-host`：别名删除、`export_plan`、CLI 错误语义。
- [docs/review/p17-review.md](../docs/review/p17-review.md) + 本 plan + [plan/README.md](README.md) Phase 17 延期落点登记。

## 验收标准（保留 REVIEW 追踪编号）

- [x] **§3.1 / P0-1 Remote 生命周期**：端点由执行 `remote publish` 的长驻 pawork 进程持有、跨进程仅 connect / reconnect、SIGINT 触发 token 清理、同名再发布不冲突、无 host 的 unpublish/revoke fail-closed，跨进程 e2e 存在
- [x] **§3.2 / P0-2 fail-closed**：placeholder 命令 / PluginList / McpList / 隔离 no-op 工具一律失败语义，CLI 非零退出码 + `--json` `ok=false`
- [x] **§4.2 Profile 引用**：skills / mcp / permissions / hooks 任一非空 → run 解析 `Unavailable`
- [x] **§4.3 Teams**：`team_db_path` 默认 None，正常 CLI 启动不创建 `teams.sqlite`
- [x] **§5 contract 归属**：remote 契约归 `transport-api`，placeholder 仅 re-export + Mock
- [x] **§5 Browser 别名**：`reject_hosted_for_local` 等纯别名 helper 删除，全仓无引用
- [x] **§5 Compat**：`apply` → `export_plan`，只写计划不应用资源，幂等指纹
- [x] **JSON stdout 契约**：日志一律 stderr，`--json` / ACP / Headless stdout 可整体解析为纯 JSON
- [x] **验证门禁（已回填）**：9 crate `cargo test` 371 passed / 0 failed / 0 ignored；同 9 crate `cargo clippy --all-targets -- -D warnings` 0 warnings；26 个 Rust 文件 `rustfmt --edition 2021 --check --config skip_children=true` 通过；`git diff --check` 通过；38 个 test/doc summaries 已核对；文档链接（git diff/未跟踪共 27 个变更 Markdown、733 条本地相对链接、0 broken）
- [x] **ROADMAP / REVIEW / p17-review 状态矩阵同步**：按评审 §8 四态矩阵终态校准已同步（HostWired P17-1/7/8 · PartialWired P17-5/11 · LibraryBuilt P17-2/3/4/6/10/13 · AdapterBuilt P17-9/12；Phase 17 14/14，总计 220/189）
- [ ] **§1 P17-7 ACP 降级审计**（显式延后）→ P18-13
- [ ] **§4.1 / §8 成熟度再认定与功能簇门禁**（显式延后）→ P18-15
- [ ] **§4.4 Marketplace / Plugin 真实纵向 + §4.2 Profile 引用消费**（显式延后）→ P19-11
- [ ] **§4.3 Teams canonical ingress**（显式延后）→ P19-13
- [ ] **§3.1 外部可达 + §4.5 Remote Control pairing**（显式延后）→ P19-14

## 验证记录（已回填 · 2026-08-13）

- 代码修复已在工作区落地并经源码 / diff 核对（本 plan 各「已修」条目的证据指针）；定向门禁已于 2026-08-13 复跑并回填，范围为 9 crate：`app-service`、`browser-computer-runtime`、`compat-loader`、`transport-api`、`transport-remote-placeholder`、`transport-remote`、`cli-host`、`pawork`、`remote-control-adapter`（transport reverse dependent）：
  - `cargo test`（上述 9 crate）：**371 passed / 0 failed / 0 ignored**（含 `remote.rs` 跨进程 e2e、`teams_state.rs`、`cli.rs` fail-closed 回归）
  - `cargo clippy --all-targets -- -D warnings`（同 9 crate）：**0 warnings**
  - `rustfmt --edition 2021 --check --config skip_children=true`（本任务涉及的 26 个 Rust 文件）：**通过**
  - `git diff --check`：**通过**
  - 文档链接检查（git diff/未跟踪共 27 个变更 Markdown，733 条本地相对链接，0 broken）：**通过**
  - test/doc summaries：**38 个已核对**
- 两项门禁衍生可靠性修复（非评审 §3–§5 原始八项）已落地：RateLimiter `enqueue`（自动冲刷返回值三个生产调用点不再丢弃，有界 ready + zero-window 回归）、remote 认证后同步 subscribe 传 pump + `rpc_capture` 联合捕获 RPC 响应与命中的 RunFinished 通知（到达顺序任意）（修前压力测试有失败，修后 30/30 exact + 10/10 整 target 通过）。「验收标准」门禁项与 ROADMAP / REVIEW / p17-review 四态矩阵已同步；任务声明 🟢已完成 · TargetVerified。

```text
Validation Level: L1
Affected crates: app-service、browser-computer-runtime、compat-loader、transport-api、transport-remote-placeholder、transport-remote、cli-host、pawork、remote-control-adapter（transport reverse dependent）
Validated: cargo test -p × 9 crates（371/0/0）· cargo clippy --all-targets -- -D warnings × 9 crates（0 warnings）· rustfmt --edition 2021 --check --config skip_children=true × 26 文件 · git diff --check · 文档链接（git diff/未跟踪 27 个变更 Markdown / 733 条本地相对链接 / 0 broken）· test/doc summaries 38 个
Targeted regressions: remote 长驻 / 跨进程 connect / reconnect / token drop 清理 / 同名再发布 / 无 host fail-closed、placeholder fail-closed、teams.sqlite 不创建、纯 JSON stdout、RateLimiter enqueue 跨窗 / zero-window、carrier RPC 响应与命中通知任意顺序（30/30 exact + 10/10 整 target）
Full workspace gate: NOT RUN（未命中升级条件）
```

**相关文档**：[docs/review/p17-review.md](../docs/review/p17-review.md) · [plan/README.md（Phase 17 延期落点登记）](README.md) · [ADR-025 CLI 唯一宿主](../docs/adr/ADR-025-cli-is-sole-host.md) · [ADR-027 本地/远程同协议](../docs/adr/ADR-027-local-remote-same-protocol.md) · [ADR-028 可替换 Remote Transport](../docs/adr/ADR-028-replaceable-remote-transport.md) · [ADR-030 Core 唯一事实源](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [ROADMAP Phase 17](../ROADMAP.md)

> 延期决策（2026-08-13）：本任务只修复「成功语义可信（fail-closed）+ Remote 生命周期 + 状态失真收敛 + 重复权威 / 死抽象删除 + JSON stdout 契约」；外部可达地址 / relay、Teams canonical ingress、Marketplace / Plugin 真实纵向与 Profile 引用维度真实消费、ACP 降级审计、host-wired 成熟度再认定与功能簇门禁均不在本任务。延后落点按 [plan/README Phase 17 延期落点登记](README.md) 五项映射：ACP 降级审计 → P18-13、成熟度再认定与门禁收口 → P18-15、Marketplace / Plugin / Profile 消费 → P19-11、Teams ingress → P19-13、远程外部可达 / pairing → P19-14。代码门禁数字已回填（9 crate test 371/0/0、clippy 0 warnings、`rustfmt --edition 2021 --check --config skip_children=true` 26 文件通过、git diff --check 通过、文档链接 27 个变更 Markdown / 733 条本地相对链接 / 0 broken、38 个 test/doc summaries 已核对），任务为 🟢已完成 · TargetVerified。
