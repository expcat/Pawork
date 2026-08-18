# S13：S12 finding 整改

> 阶段 S13 · 整改与收口 · 状态：🔵进行中（2026-08-18 波 A ✅ · 波 B ✅；波 C 未开） · 依赖：S12 🟢（60 条 finding 已合并登记为 57 项 S12-F01～F57，见 [ROADMAP](../ROADMAP.md) §3.2） · 规模：大（57 项整改，波 A 安全 → 波 B Bug → 波 C 收口）

## 目标

把 S12 全项目 Code Review 登记的 57 项 Confirmed finding（[docs/reviews/s12/](../docs/reviews/s12/) 九份报告 + 五份交叉复核）按优先级与写入集分波整改收口：安全与数据风险先行，功能缺口与 Bug 其次，性能与维护性随同写入集簇同批消化。S13 不交付新功能、不发布、不设全量门禁；逐项验收证据以 [ROADMAP](../ROADMAP.md) §3.2 登记行为准，本任务书补充执行边界、同批约束与决策点。

## 事实源与登记修正

- finding 事实源优先级：审查报告原文 > §3.2 登记行 > 本任务书。三者冲突时以报告原文为准并回写登记行。
- 2026-08-18 立项核对（四路并行重读十四份报告 + 登记行对照）发现以下登记失真，已随立项回写 ROADMAP §3.2：
  - **F13**：验收方向按交叉复核纠偏——应断言按 `RunStart.provider` 命中用户所选通道（catalog 首项会误中），原登记「选择后者」表述错误。
  - **F31**：报告原文中 Automation 通道连接前缀为可选建议，登记误写为必做，已改回。
  - **F09**：写入集补齐 `compact_history` 钉死 `DEFAULT_BRANCH_ID`、CLI/`timeline()`/`replay_events` 消费面；补「冻结线性模型」备选与拍板标记。
  - **F05**：验收补 MCP 子进程 env 继承（`PAWORK_API_KEY_*`）收口。
  - **F17**：验收补 Skills/profile（原文不止 AGENTS.md）。
  - **F30 / F49**：二选一任务验收补齐「report-only / §4 登记」支，原登记只写实现支。
- 其余边界以报告原文为准，不再逐行回写 §3.2：F01 不改写工具/审批矩阵、`search_text` 用不跟随的 `file_type`；F15 含 host 装配点、不改 GUI 帧语义；F16 不碰 K-02、不静默映射 AskForWrites；F22 `session_usage_inner` 只累加 completed、日后热路径硬门禁则回升 High；F25 现网 host 长度正确只防回归；F26 host 无 revise 入口；F28 inbox 视图已幂等；F29 demo 已知 parent、生产 API 才暴露；F33 对象级 ACL。

## 波次拆分

排队规则：波 A（High）→ 波 B（Medium）→ 波 C（收口），与 §3.2 分桶一致。**同写入集或强依赖的 Low 项随动入簇**：仍为独立任务、独立验收、独立状态回写，只是与同簇任务同批执行，避免同一文件二次改动；§3.2 的编号与分桶不变。

### 波 A：安全与数据风险（F01–F14，随动 F45/F50/F52）

| 簇 | finding | 写入集 | 内部约束 |
| --- | --- | --- | --- |
| A1 路径内核 | F01（含 `.git` 只读拍板）；随动 F45、F50 | `execution/policy`、`execution/tools`、`workspace/core`、`workflow/review`（F50） | 四套路径校验以 `policy::path` 为单一事实源（CR09-05 不另立写入集，不拉入 resources `canonical_within`）；F45/F50 在内核落地后跟随；F17（B2）不堵 F01 |
| A2 exec 沙箱与命令 | F02（拍板）、F03、F04 | `execution/exec`、`execution/policy`（shell.rs）、`host/app`（data_dir.rs）、`host/cli`（service.rs） | 禁顺带 K-09；auth.json deny 归 F02 不归 F05；F03 文档措辞归 F39；Windows SCM 本机无法验收按 S10 口径降级登记 |
| A3 Secret 与 MCP 边界 | F05+F08 同批（均拍板）、F06（拍板）、F07 | `foundation/config`、`net/net`、`extensions/mcp`、`host/app`（extensions.rs） | F05 与 F08 同写 extensions.rs 必须同批；F07 必须在 `classify_status` 生成点脱敏；禁顺带 K-09；与 A2 在 `host/app` 文件级不重叠（extensions.rs vs data_dir.rs） |
| A4 会话分支投影 | F09（契约级拍板） | `storage/session`、`engine`（loop_ctx.compact_history）、`host/app`、`host/cli` | golden 先行；不改事件信封 v1 与 append-only 事实表；必须改 `compact_history` 的 `DEFAULT_BRANCH_ID` 钉死点，不能只改 `resume_messages` |
| A5 协议与连接安全 | F10+F52 同批、F11 | `foundation/protocol`、`host/transport`、`host/gui-server`、`clients/gui-client`、`host/cli`（gui.rs）、`apps/protocol-probe` | F52（TOKEN_SCHEME）随 F10 同批；F11 勿接 K-07 rate_limit、勿改 EventHub 容量；F11 与 F32（B7）的 ReplayUnavailable/SnapshotRequired 信号需归一协调；禁碰 K-08；与 A6 在 protocol 上文件级不重叠（client_auth vs RunStart 帧） |
| A6 Desktop 安全链 | F12、F13（链式）、F14 | `apps/desktop`（projection/ui）、F13 另涉 `foundation/protocol`（RunStart）+ `host/app`（gui_host.rs） | F13 三处一条链（协议→Host→Desktop），不可只改 Desktop；F14 人工键盘走查证据并入 K-03；F12 只动锚点不改 UI |

波 A 完成即做一次安全红线定向回归汇总（路径内核、命令分类、Seatbelt、SecretRef、跨域凭证、错误脱敏、gui serve 认证、Lagged fail-closed）。

### 波 B：功能缺口与 Bug（F15–F40，随动 Low 入簇）

| 簇 | finding | 写入集 | 内部约束 |
| --- | --- | --- | --- |
| B1 manifests 与 trait | F15（红线级拍板）；随动 F41（拍板）、F42、F43、F44（拍板） | `storage/session`（client_adapter.rs）、`foundation/api`、各 Cargo.toml | F15 涉包依赖方向红线，先 ADR/用户确认；随动项不升级依赖版本 |
| B2 策略与注入语义 | F16（拍板）、F17（拍板） | `execution/policy`、`host/app`（approval.rs、extensions.rs）、`host/cli`、`workspace/resources` | 禁碰 K-02；F16 禁止静默映射成 AskForWrites；F17 不实现通用注入分类器、不跑 Skills scripts |
| B3 进程与 PTY | F18（拍板）+F20 同批、F19（拍板）；随动 F46 | `execution/exec`（pty/tree）、`host/app`（gui_host.rs）、`host/cli`（gui.rs、render.rs） | F18 与 F20 同属进程回收面同批；kill -9 会话 seal 归 K-02；F19 勿同时改三平台 profile、勿做 K-09；与 B7 在 gui_host.rs 有交集，串行或同人 |
| B4 provider 凭证闸门 | F21 | `providers/adapters`（anthropic）、`net/net`（Debug 脱敏） | 不改 Anthropic 请求体能力（K-10 / CR04-06） |
| B5 持久化与账本 | F22（随动 F47 同路径）、F23（随动 F48 同 crate 分文件） | `engine`（tool_loop 收口）、`host/app`（usage 记录）、`storage/blob` | F47 接 F22 同一 usage 构造路径；F23/F48 勿绑迁移、勿改内容寻址；与 B6 在 engine/tool_loop 有交集，串行或同人 |
| B6 engine / workflow / orchestration | F24（契约级拍板，先于 F49）、F25、F26（契约级拍板）、F27、F28（契约级拍板）、F29（先于 F30）、F30（拍板）；随动 F49（拍板，依赖 F24 契约）、F51（只改测试） | `foundation/api`、`foundation/domain`、`engine`、`workflow/*`、`pawork-memory`、`agents/orchestration` | 契约改动 golden 先行；F24 不修 K-08；F25 不顺带 K-02；F29 先于 F30 重建支；F49 接线支不得让 engine 依赖 blob store |
| B7 协议幂等与能力 | F31、F32（拍板）、F33（拍板） | `host/app`（idempotency.rs、gui_host.rs）、`clients/gui-client`、`host/gui-server`、`host/cli`（headless.rs） | F31 勿改信封、勿持久化 SQLite；F32 与 F11 信号归一；F33 勿改 GUI capabilities（K-08）与 ACP method 表；与 B3 在 gui_host.rs 串行 |
| B8 Desktop 体验 | F34、F35、F36、F37（拍板）；随动 F53、F54、F55、F56（拍板） | `apps/desktop`（ui/projection）、F37/F56 或涉 `design/README.md`+`docs/gui-design.md` | F35/F56 人工证据并入 K-03；F37 不与 K-03 合并；F56 有意差异先改基准再留现状；quota「—」归 S11 延期不立项 |
| B9 文档一致性 | F38、F39、F40（拍板）；随动 F57（拍板） | `README.md`、`AGENTS.md`、`plan/S10-serve-clients.md`、`v2_plan.md`、`ROADMAP.md` §4 | F39 行为补齐归 F03、本条只改文档，复验在 F03 后；F40/F57 的 §4 登记支在本簇内即可完成；F38 状态表已随 S13 立项部分对齐（README S12 🟢 + S13 行），剩结构图与 AGENTS §3 补域 |

### 波 C：收口

1. 57 项状态全量回写：§3.2 逐行 🟢（日期 + 证据链接）或经确认转 §4 延期（激活条件登记）。
2. 文档一致性复核：README / ROADMAP / v2_plan / AGENTS / task-guide 状态符号与结构清单一致（F38 验收口径）。
3. §3.1 归档一条 S13 记录（57 行压缩 + 链接本任务书与审查目录）。
4. 向用户报告整改完成度与剩余挂账（K-01～K-10、S6 OAuth refresh、发布类决策），不自动开启任何后续任务。

## 决策点（拍板清单）

**契约 / 红线级**（动手前先出 ADR 或用户确认，涉冻结契约与架构红线）：

| 项 | 决策 |
| --- | --- |
| F01 | **已拍板（2026-08-18）**：读写均拒绝 `.git`（与 V1 写路径一致）；不设「允许审计」开关 |
| F09 | **已拍板（2026-08-18）**：支 A — schema v10 附加式迁移 + `ancestor_lineage` API；不改事件信封 v1、append-only 事实表、`UNIQUE(session_id, sequence)` |
| F15 | **已拍板（2026-08-18）**：trait + 记录归 domain（[ADR-037](../docs/adr/ADR-037-s13-wave-b-contracts.md)）；session 去掉 protocol 依赖 |
| F19 | **已拍板（2026-08-18）**：维持 ADR-031 可观测回退，写回 design/S4，CLI/GUI 必须展示 fallback |
| F24 | **已拍板（2026-08-18）**：扩 `ToolResultContent.artifacts`（附加式）；不新增 AgentEvent 变体 |
| F26 | **已拍板（2026-08-18）**：`Revised` 加 `title`/`steps`（附加式）；拒绝重复 version |
| F28 | **已拍板（2026-08-18）**：`ResultArchived` 加 `task_id`；幂等键 `(automation_id, task_id)` |

**任务内二选一**（簇内拍板并记录理由；默认倾向以报告建议为准）：

F02（移除整盘只读 vs 诚实降级标签）· F05（SecretRef 域形态）· F06（fail-closed vs 剥尽凭证头）· F08（workspace MCP 剥离形态）· F16（实现 vs 收窄口径）· F17（拒仓库层 vs loader trust 开关）· F18（接沙箱 vs 显式降级「本机不受控终端」）· F30（重建 vs report-only）· F32（附带 Snapshot 归一方向）· F33（fail-closed vs 补映射）· F37（多窗口实现 vs 文档修正）· F40（消费面 vs §4 登记）· F41（占位 feature vs 改文档）· F44（门控 vs 词典措辞）· F49（接线 vs §4 登记）· F56（按基准改 vs 改基准）· F57（并入 K-04 / 独立 / 冻结候审）

波 A 任务内已拍板（2026-08-18）：

- **F02**：支 B — 保留 `(allow file-read* (subpath "/"))`（Darwin 25.5），新增 `IsolationLevel::HardWritesAndNetwork`，并扩大 `default_secret_paths`。
- **F05**：SecretRef 必须 `pawork.mcp.*`；MCP 用独立 `{auth-dir}/mcp-auth.json`；stdio `env_clear` + 拒绝 `PAWORK_API_KEY_*`。
- **F06**：剥离 workspace `proxy_url` / 非回环 `providers[].base_url`，且出站 `redirect(Policy::none())`。
- **F07**：丢弃错误 body；`classify_status` 消息仅 `HTTP {status}`。
- **F08**：剥离 workspace `trusted`/`auto_start`；宿主 clamp；未信任 workspace 不自动启动。
- **F11**：Lagged 发 `ReplayUnavailable` Error 帧后停转发；不发假 Resume / `SnapshotRequired`（F32 已在波 B 消费附带 Snapshot）。

波 B 任务内已拍板（2026-08-18）：

- **F16**：收窄为「当前等价 NeverAsk」；compat OnFailure 与 NeverAsk 同映射（Ask）。
- **F17**：host 按 `workspace_trusted` 过滤 Workspace origin 注入。
- **F18**：显式降级「本机不受控终端」+ 退出路径 `pty.shutdown()`；不接沙箱。
- **F30**：report-only，不假装重建 Supervisor。
- **F32**：客户端消费服务端附带 Snapshot（gui-design §4.1）。
- **F33**：未映射 headless 命令 fail-closed；不改 GUI capabilities（K-08）。
- **F37**：文档修正（不实现多窗口），§4 登记激活条件。
- **F40 / F49 / F57**：§4 登记（F57 并入 K-04）。
- **F41**：空 `plugin = []`。
- **F44**：改词典 #30（tokio 保持无条件）。
- **F56**：按 v3 基准改（Inspector ~440px、折叠归零、Fork 进条目操作）。

## 与 K-01～K-10 基线的关系

S13 不吸收任何 K 项；仅登记证据共享与禁止顺带边界：

- 证据共享：F14 / F35 / F56 的人工界面证据并入 K-03（Desktop 人工验收）一次采集；F57 登记时挂钩 K-04；F39 文档复验在 F03 行为补齐后执行。
- 禁止顺带：K-02（F16 / F18 / F25 相邻）、K-07（F11）、K-08（F10 / F24 / F33 / F49）、K-09（F02 / F06 / F08 / F19）；F21 相邻 K-10 但不收口 Anthropic 能力表。

## 验证约定

- 沿用 S0–S11 定向验证（[docs/task-guide.md](../docs/task-guide.md) §6）：`cargo check -p <crate>` / `cargo test -p <crate>`，多包重复 `-p`，不改用 `--workspace`。
- 三类关键测试不推迟：安全红线定向回归（波 A 每项必带）、持久化与重放契约 golden（F09/F22/F23/F26/F28 等）、协议与解析 golden（F13/F24/F32 等契约改动 golden 先行）。
- GUI 项真实界面证据：F12/F14/F34/F35/F36/F53–F56 的实现验收 + 人工证据并入 K-03；不以 probe/源码替代真实窗口。
- 平台项：F02/F20（macOS）在当前平台实测；F03 的 Windows SCM 本机无法验收，按 S10 口径降级登记。
- 不设 Workspace Full Gate、clippy/fmt 门禁、三平台矩阵、覆盖率；不发布。任务结束报告沿用根 AGENTS.md §5 模板。

## ROADMAP 回写规则

- 每项 S12-Fxx 完成即在 [ROADMAP](../ROADMAP.md) §3.2 状态列标 🟢（日期 + 证据链接）；证据不足的不得标绿。
- 二选一任务无论选哪支，验收按所选支执行并在登记行记录所选口径。
- 波 C 收口时：57 行压缩为一条 §3.1 归档记录（链接本任务书与 [docs/reviews/s12/](../docs/reviews/s12/)），未收口项与决策延期逐行转 §4 激活条件。
- 整改引入的新发现不得顺手改：登记为新 finding 行追加 §3.2，排队另行授权。

## 退出标准

- [x] 波 A：F01–F14（含随动 F45/F50/F52）全部验收通过，安全红线定向回归汇总全绿。F14 真实窗口键盘走查并入 K-03；F03 Windows SCM 按 S10 降级。
- [x] 波 B：F15–F40（含随动 Low 项）全部验收通过或经确认转 §4 延期。
- [x] 契约 / 红线级决策（F01/F09/F15/F19/F24/F26/F28）均有 ADR 或用户确认记录（ADR-037）。
- [ ] 波 C：§3.2 状态全量回写、文档一致性复核（F38/F39 口径）、§3.1 归档完成。
- [x] 全程未实现新功能、未设全量门禁、未发布、未吸收 K 项。（波 A/B 迄今成立；波 C 继续守）

## 明确不做

- 不交付新功能（多窗口、Desktop Workflow 面、多账户 factory 等仍属各自挂账项）。
- 不执行 Workspace Full Gate、fuzz、基准（F55 任务内渲染基准除外）、三平台矩阵、发布/打包/签名/部署。
- 不吸收 K-01～K-10；不处理 S6 OAuth 临期 refresh 挂账。
- 不做 V1 归档资产改动；不顺带修复审查范围外问题。

## 参考

- [../ROADMAP.md](../ROADMAP.md) §2/§3.2/§4/§6 · [../docs/reviews/s12/](../docs/reviews/s12/)（九份报告 + 五份交叉复核）
- [S12 审查任务书](S12-project-code-review.md)（finding 记录格式与回写规则来源）
- [../docs/task-guide.md](../docs/task-guide.md) §6（定向验证约定） · [../docs/design.md](../docs/design.md) §2–§3（包布局与冻结契约）
