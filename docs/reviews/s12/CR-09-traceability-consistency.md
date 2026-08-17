# S12 CR-09 — 需求追踪 · 死代码 · 文档一致性 · 跨报告收口

| 字段 | 值 |
| --- | --- |
| CR 编号 | CR-09 |
| 主审范围 | ROADMAP.md、plan/（S0–S12）、docs/、README.md、v2_plan.md 与全仓 TODO / feature / call-site 索引；跨报告收口 CR-01～CR-08 与 cross-review 裁定表 |
| 审查日期 | 2026-08-18 |
| 主审模型 | zai/glm-5.3（reasoning effort: high） |
| 审查方式 | 只读静态审查：文档 ↔ 源码 ↔ git 历史交叉核对；rg 全仓调用点索引；未运行构建 / 测试 / 二进制 / GUI |

## 1. 实际审查路径

- 必读基线：plan/S12-project-code-review.md 全文；ROADMAP.md §3.2（K-01～K-10）；docs/task-guide.md §3.1 架构红线；docs/design.md §3.2 冻结契约。
- 需求侧：ROADMAP.md 全文；plan/S0–S11 共 12 份任务书；v2_plan.md 全文；README.md 全文；AGENTS.md。
- 源码侧（调用点索引）：host/cli/src（lib.rs Command 枚举、chat.rs、sessions.rs、service.rs、ops.rs、render.rs）；host/app/src（lib.rs、plan_host.rs、tasks_host.rs、gui_host.rs）；engine/engine/src（tool_loop.rs、context/token.rs）；execution/tools/src/scheduler.rs；execution/policy/src/path.rs；workspace/core/src/path.rs；workspace/resources/src/io.rs；workflow/core/src/lib.rs；workflow/review/src/anchor.rs；vcs/git/src（lib.rs、diff/hunk_stage.rs）；control-plane/quota/src（util.rs、error.rs）；providers/auth/src/lib.rs。
- 报告侧：docs/reviews/s12/ CR-01～CR-08 共 8 份主报告与全部 5 份 cross-review 裁定文件逐条通读。

## 2. S0–S11 需求追踪（计划 → 生产调用点 / 用户界面 → 既有证据）

| 阶段 | 主承诺（计划口径） | 生产调用点 / 用户界面（本轮已核实） | 判定 |
| --- | --- | --- | --- |
| S0 | domain 事件与会话模型 | `AgentEvent` 在 engine / host / session 持久化全链消费；docs/design.md §2 登记包布局 | ✅ 落地 |
| S1 | `sessions` CLI 与 resume | host/cli/src/lib.rs:84 `Command::Sessions`；sessions.rs:83 `list_sessions` | ✅ 落地 |
| S2 | chat REPL + 只读工具 + `@` 引用 | `Command::Chat`；chat.rs:64 `expand_at_refs` | ✅ 落地（轮数上限 Mock-only 已由 CR-06 披露） |
| S3 | policy 写三件 + 审批 | scheduler.rs:278/350 `check_gate`；tool_loop.rs:44 `ApprovalGate` | ✅ 落地；读路径分叉见 S12-CR09-05；S3 假完成归 S12-CR02-02（链接不重号） |
| S4 | exec / 沙箱 / run_command | host/app/src/lib.rs:722 注册只读四件 + 写三件 + run_command | ✅ 落地（K-09 登记 allowlist 语义） |
| S5 | usage / context / compaction | tool_loop.rs:475/768/1331 `reply_primer_tokens` 生产消费；ContextMeter UI | ✅ 落地 |
| S6 | 六通道 + auth 文件 | host/app/src/lib.rs:411 `FileBackend::new()` 生产装配 | ✅ 落地；OAuth 临期 refresh 在 v2_plan.md:49 挂账 🔵，属已披露延期非假完成 |
| S7 | GUI serve + Desktop | `Command::Gui`；apps/desktop 四层结构 | ✅ 落地；人工窗口验收 K-03 未做（已登记） |
| S8 | diff / rollback / checkpoint | `Command::Diff` / `Command::Rollback`；tool_loop.rs:275-279 `snapshot_write_tools` → `CheckpointCreated` | ✅ 落地；Desktop Changes 面 K-04；`HunkStageService` 见 S12-CR09-04 |
| S9 | MCP / 资源 / 兼容导入 | `Command::Mcp` / `Command::Import`；REPL `@file` | ✅ 落地；Desktop `@` / Resources 面 K-06（已登记） |
| S10 | headless / ACP / service / fork | `Command::Headless` / `Command::Acp` / `Command::Service`；apps/protocol-probe | ✅ 落地；`stop --apply` 文档失真见 S12-CR09-02；Windows Service 降级登记 |
| S11 | plan gate / tasks / usage / agents | plan_host.rs:138-152 `is_approved_for_execution` 拦截未批准计划；`Command::Usage` / `Command::Tasks` / `Command::Agents`；gui_host.rs:823 `QuotaOverview` | ✅ 落地；workflow 三域零消费见 S12-CR09-03；Desktop Workflow / quota 条延期已登记 |

结论：S0–S11 主承诺均落到生产调用点或已登记延期项；除下列 finding 外，未发现其他假完成 / 零消费者主路径。

## 3. Findings

### S12-CR09-01 — README / AGENTS 状态与结构清单滞后

- 类别：Requirement Gap（文档状态漂移）　严重度：Medium　置信度：Confirmed
- 证据：
  - README.md:18-19 状态表 S10 🔵、S11 ⚪；ROADMAP.md:64-65 与 v2_plan.md:49 均为 S10 🟢、S11 🟢。同一事实三个文档两种口径。
  - README.md:38-56 仓库结构图缺 `agents/`、`control-plane/`、`workflow/`、`schemas/` 四个真实存在的顶层目录（均已 ls 核实）；`apps/` 只列 pawork、desktop，漏 protocol-probe；`clients/` 只列 gui-client，漏 sdk、compat。
  - AGENTS.md:29 包功能域清单同样缺 agents、control-plane、workflow 三域。
- 实际行为：README 状态与结构图落后于仓库真实状态；AGENTS.md 的目录契约不完整。
- 期望行为：状态表与 ROADMAP / v2_plan 对齐；结构图与 AGENTS.md §3 补齐上述目录与子包。
- 影响面：S12 整改任务按 README 定位包会漏四个目录；新会话按 AGENTS.md 路由会把 workflow 等域当成不存在。
- 验证建议（S12 内不执行）：rg 核对三份文档的 S10 / S11 状态符号；ls 对照结构图目录清单。
- 整改边界：README.md、AGENTS.md 两文件的状态行与目录清单；不触碰 ROADMAP / v2_plan；不顺带改其他文档内容。

### S12-CR09-02 — S10 任务书记录「stop --apply 删 plist」与源码从未相符

- 类别：False Completion（验收证据失真）　严重度：Medium　置信度：Confirmed
- 证据：
  - plan/S10-serve-clients.md:48 勾选项写「`stop --apply` 后未监听、进程退出并删 plist」；v2_plan.md:62 冒烟记录同文「stop，已删 plist」。
  - host/cli/src/service.rs:144 macOS stop 只执行 `launchctl unload`，其后无任何删除逻辑。
  - `git log -S remove_file -- host/cli/src/service.rs` 零命中：该文件历史上从未包含 remove_file，删除 plist 行为从未存在。
  - 全 cli 目录唯一 `remove_file` 是 ops.rs:289 的 pid 文件清理，与 plist 无关。
- 实际行为：任务书与活动计划把「删 plist」记为已验收事实，但源码从未实现。
- 期望行为：文档与源码一致——行为侧补齐归 S12-CR03-03（同查 stop 不删定义的持久化残留，链接不重号）；本条要求 plan/S10 与 v2_plan 的记录改为真话（要么删除该句，要么改写为「stop 未删 plist，残留行为见 S12-CR03-03」）。
- 影响面：S10 验收记录失真；若按文档信任，残留 plist 在 RunAtLoad / KeepAlive 下会于重启后复活服务。
- 验证建议：整改 CR03-03 后重跑 install→start→stop 并 `ls` plist 路径；本轮静态证据已足够定稿。
- 整改边界：plan/S10-serve-clients.md:48 与 v2_plan.md:62 两处措辞；不重复登记行为缺陷编号。

### S12-CR09-03 — workflow 五域中 goal / automation / monitor 零生产消费者

- 类别：Requirement Gap（零消费者合入）　严重度：Medium　置信度：Confirmed
- 证据：
  - workflow/core/src/lib.rs:10-14 声明 `pub mod automation / goal / monitor / plan / task` 五域，各自带状态机与测试。
  - host/ 全域 rg `pawork_workflow::` 仅命中 plan（plan_host.rs:5）与 task（tasks_host.rs:7、lib.rs:109-110 / 357 / 672）；goal / automation / monitor 三域无任何生产调用点。
  - ROADMAP.md §4:127-132 只登记 workflow 的 `process-exec` feature 激活条件，没有这三域的延期 / 冻结登记行。
  - plan/S11-workflow-control.md:52 写「七包『无消费者不合入』逐包核对」，但核对粒度是包不是域：pawork-workflow 因 plan / task 两域有宿主而整包放行，掩盖了三域缺口。
- 实际行为：三域状态机 + 测试已合入主干，无用户可见面、无宿主装配、无延期登记。
- 期望行为：遵守 docs/task-guide.md §3.2「无消费者不合入」——要么补消费面（GUI Workflow 面已整体延期，见 ROADMAP §4），要么在 ROADMAP §4 为三域各登记激活条件与冻结名义。
- 影响面：后续维护成本与「假库存」风险——读者以为 workflow 引擎可用，实际五分之三不可达。
- 验证建议：整改后 rg `pawork_workflow::(goal|automation|monitor)` 应命中生产路径，或 ROADMAP §4 出现三行登记。
- 整改边界：ROADMAP.md §4 增加登记行（或另立消费面任务后回写）；不改 workflow 代码。

### S12-CR09-04 — HunkStageService 迁入后承诺的两个消费时点均已过期

- 类别：Maintainability（零消费者）　严重度：Low　置信度：Confirmed
- 证据：
  - vcs/git/src/diff/hunk_stage.rs:26 定义 `pub struct HunkStageService`；vcs/git/src/lib.rs:45 对外 re-export。
  - 全仓调用点仅 hunk_stage.rs 自身 tests（342-700 行区间的 `HunkStageService::new`）；host / engine / desktop 均无消费者。
  - plan/S8-git-checkpoint.md:56 承诺「消费者（GUI 交互式暂存）随本阶段 Changes 或 S10 补齐」——Changes 面未做（K-04 登记），S10 已收口 🟢，两个时点均过期且 ROADMAP §4 无对应行。
- 实际行为：hunk / line 级暂存能力作为公共 API 迁入并测试，但从未接任何交互面。
- 期望行为：在 ROADMAP §4 为其登记激活条件（并入 K-04 Changes 面或独立 GUI 暂存任务），或纳入冻结候审清单挂名。
- 影响面：低——纯 Rust API 无运行时开销；主要是契约漂移与库存误导。
- 验证建议：整改后 rg `HunkStageService` 的生产命中，或 ROADMAP §4 出现登记行。
- 整改边界：ROADMAP.md §4 一行登记；不改 vcs 代码。

### S12-CR09-05 — 工作区路径校验四处分叉，无单一事实源

- 类别：Maintainability（重复实现）　严重度：Low　置信度：Confirmed
- 证据：
  - execution/policy/src/path.rs:48 `resolve_workspace_path`：canonical 复核 + root 收敛，语义最强，服务写路径。
  - workspace/core/src/path.rs:41 `resolve_relative_path`：纯词法，注释自述「S2 不做存在性、symlink 或 `.git` 检查」，是读工具逃逸根因（见 S12-CR02-01 / CR02-02，链接不重号）。
  - workspace/resources/src/io.rs:129 `canonical_within`：canonicalize 后比前缀，服务资源加载。
  - workflow/review/src/anchor.rs:95 `safe_path`：纯词法（绝对路径 + `..` 拒绝），服务 review 锚点（缺口见 S12-CR06-09）。
- 实际行为：同一「工作区相对路径安全解析」职责存在四套语义强度不同的实现，分散在四个包。
- 期望行为：以 policy::path 为单一事实源收口读 / 写两侧，或至少在 docs/design.md 写明四套实现的语义矩阵与收口计划。
- 影响面：新调用点（如未来 GUI 写路径、LSP 注入）容易复用错弱实现；CR02-01 的逃逸正是分叉的直接后果。
- 验证建议：整改时以 policy 实现替换 workspace/core 词法入口后，重跑 CR02-01 的 symlink 回归。
- 整改边界：随 S12-CR02 整改统一处理，本条只登记分叉全景与收口方向，不另立写入集。

## 4. 跨报告收口（CR-01～CR-08 + cross-review）

### 4.1 覆盖总量与口径

- 实际存在 5 份 cross-review 文件：CR-02 / CR-03 / CR-04 / CR-07 各一份（GLM）+ CR-05-08 一份（Grok）。派发口径为「4 份裁定表」，少计一份；父代理收口统计应按 5 份计。
- 8 份主报告自有 finding：CR-01 5 / CR-02 5 / CR-03 7 / CR-04 6 / CR-05 5 / CR-06 10 / CR-07 6 / CR-08 11，合计 55，全部 Confirmed、NV 0。
- High 共 18 条，全部被 cross-review 复核（CR-02 2、CR-03 4、CR-04 4、CR-05 2、CR-07 3、CR-08 3），每条均有裁定行 + 逐条复核记录，程序合规。
- 裁定后有效严重度：High 15 / Medium 24 / Low 16（三条 High 降级 Medium）。

### 4.2 合规项（无违规）

- 无跨包重复建号：CR-04 对 CR-03 `default_secret_paths`、CR-07 对 K-07 / K-08 / S12-CR03-02 / S12-CR05-01 均以链接处理；CR-05-08 cross-review 明确 CR05-02 与 CR-06 账本不重复立项。
- K-01～K-10 未被误当新发现：各报告引用 K 编号时均标注为已登记基线（如 CR04-06 链接 K-10，CR-07 显式声明不计新 finding）。
- 本报告遵守同一纪律：S3 假完成链接 S12-CR02-02；stop 不删除 plist 的行为缺陷归 S12-CR03-03，本包只登记文档失真；读路径逃逸与替换声明链接 S12-CR02-01 / 02；review anchor 链接 S12-CR06-09；JsonlSink 链接 S12-CR03-07。

### 4.3 待回写差异（裁定与主报告原文的 7 项不一致）

| # | 编号 | 差异内容 | 裁定 |
| --- | --- | --- | --- |
| 1 | S12-CR03-02 | High → Medium：唯一写入方是本机用户手工输入，未找到模型到 TerminalWrite 的调用路径 | 降级 |
| 2 | S12-CR05-02 | High → Medium：热路径未用该 ledger 做 quota / budget 门禁，影响为计量低估而非控制面绕过 | 降级 |
| 3 | S12-CR07-03 | High → Medium：现实触发面仅 ACP 同 id 并发重试一条窄路径 | 降级 |
| 4 | S12-CR07-01 | 「umask 022 → 他人可 connect」括注证伪（socket 0755 下他人无写权限不能 connect）；同用户任意进程攻击面仍成立 | 维持 High，修正括注 |
| 5 | S12-CR08-02 | 方向写反：channels 顺序中 opencode-go 在 deepseek 之前，用户选 deepseek 时 find() 落到 opencode-go，非报告所写相反方向 | 维持 High，修正方向 |
| 6 | S12-CR08-03 | 行号校正：审批三按钮实际 1246-1268（原写 1325-1365）、全局新建实际 1119-1131（原写 1139-1151） | 维持 High，修正行号 |
| 7 | S12-CR04-04 | 「免于禁止执行」口径不准：trusted=true 只绕过未信任 workspace 硬门，写入类工具单次调用仍触发审批 | 维持 High，修正边界表述 |

按任务书回写规则，上述差异应由父代理在整改拆分前统一回写对应主报告；本报告不修改其他报告。

## 5. 未覆盖路径与原因

- 运行时行为：S12 只读纪律禁止运行二进制 / GUI / 构建 / 测试；所有结论基于源码与 git 静态证据。
- docs/references.md、docs/v1-migration-reference.md 仅按需抽查交叉引用，未逐条全文复核（冻结参照，非生产代码）。
- design/ 视觉基准图未逐张目检（归 CR-08 GUI 审查域）。
- 低信号 dead_code 观察（不构成 finding，留档）：
  - control-plane/quota/src/util.rs、error.rs 带 `#![allow(dead_code)]`，但文件头已写明「远端适配器冻结候审期间无生产调用方，仍保留为单一实现」，属已披露冻结候审。
  - engine/engine/src/context/token.rs:197-198 注释「S8 资源阶段接线前仅测试引用」已过期：`reply_primer_tokens` 现被 tool_loop.rs:475 / 768 / 1331 生产消费，仅 `message_framing_tokens` 仍限测试；建议随文档批次顺手修正。
  - host/cli/src/render.rs `JsonlSink` 输出形状问题归 S12-CR03-07，不重复登记。
- 记录口径观察：plan/S10-serve-clients.md:48 单条 checkbox 同时覆盖 install / start / stop 三个行为，粒度含混，是 S12-CR09-02 失真得以藏身的表单土壤；建议整改时把冒烟 checklist 按 action 拆条。

## 6. 统计

| 严重度 | 条数 | 编号 |
| --- | --- | --- |
| Critical | 0 | — |
| High | 0 | — |
| Medium | 3 | S12-CR09-01 / 02 / 03 |
| Low | 2 | S12-CR09-04 / 05 |

置信度：Confirmed 5 · Needs Verification 0。

跨包链接清单（均不重号）：S12-CR02-01、S12-CR02-02、S12-CR03-03、S12-CR03-07、S12-CR06-09、K-03、K-04、K-06、K-09。
