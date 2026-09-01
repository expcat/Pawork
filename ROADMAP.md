# Pawork 路线图

> 本文是当前任务与后续工作的唯一计划事实源，只保留未完成工作、验收标准和开放决策。已完成阶段与旧编号统一查阅 [docs/history.md](docs/history.md)；架构红线与冻结契约见 [docs/architecture.md](docs/architecture.md)。

## 1. 当前指针

| 字段 | 当前事实 |
| --- | --- |
| 活动线 | **无（P3 已收口；下一阶段 P4 Accessibility 与跨平台待用户开启）** |
| 状态 | 🟢 P3 已收口（2026-09-01）：片 1 审计 ✅、片 2 Terminal 三修复 ✅、片 3 真窗口验收 ✅、片 4 G4 [ADR-045](docs/adr/ADR-045-terminal-lifecycle-wire-evolution.md) ✅（实现 + 定向门禁 + 真窗口验收全过）。P1 / P2 / P3 均已收口，过程与证据归档 [docs/history.md](docs/history.md)。 |
| 本轮结果 | Changes / Resources 冻结契约内无缺口（真窗口冒烟确认权威数据、刷新、断线 stale 诚实）；Terminal 面 G1 尺寸 stepper 真实 resize（stty 24x80→28x88）、G2 exited 重建（New 入口同 workspace/cwd 新建、旧终端只读）、G3 cwd 诚实显示（快照 cwd 键 + unknown 兜底 + 根目录标签归一）、G4 完整生命周期（ADR-045：`terminal_close` + `TerminalExited` live 事件、API 1.3 按协商 minor 门控推送、Desktop Stop/Close 真实接线、live 终态即时刷新、Close 清理 GuiHost tombstone 与 PTY service 条目、Failed 终态可 Close 清理后回到 Start）全部落地并经真窗口验证；提交后 review 另补齐检入 TypeScript schema/typegen 门禁并修复 PTY 清理、Stop/Close 回执竞态与 Failed 锁死；门禁结果见片 4 与 history。 |
| 下一动作 | 待用户开启 P4（Accessibility 与跨平台）或指派阶段外任务；P2 / P3 遗留观察项见 §5。 |
| 本轮完成条件 | §4 P3 退出条件已满足：三面板只展示 Host 权威数据，关键动作与错误恢复完整。 |
| 当前阻塞 | 无。 |

状态：⚪ 未开始 · 🔵 进行中 · 🟢 已验证 · ⚠️ 阻塞。任何“已实现”“自动检查通过”“真窗口通过”“等待人工确认”必须分开记录。

## 2. 当前执行顺序

### E0 — 构建与启动入口 ✅

- 在 `scripts/` 提供一个最小脚本，支持构建和启动两个入口。
- `build` 只构建正式 `pawork` 与 `pawork-desktop` 二进制；不编译或运行 fixture、probe、测试 target。
- `start` 默认先构建，再启动正式 `pawork gui serve` 与 Desktop；Host 已运行时复用，不启动第二个实例。
- Desktop 退出时只关闭由本脚本启动的 Host；日志写入忽略目录，不把 token、凭证或运行数据写入仓库。
- README 给出用户可直接复制的命令和运行前提。

### E1 — 真实核心路径 ✅

严格使用正式 Host、真实数据库与真实 UI，不调用 `ui-fixture`、seed、probe 或测试 profile：

1. 启动空态并确认 Host 显示 Connected。
2. 从 UI 添加一个真实本地项目；项目必须由用户选择或输入的真实目录进入 Host，不能靠预置 workspace 冒充。
3. 在该项目中新建对话并发送消息。
4. 要求 Agent 在项目内创建一个文本文件，内容同时包含 `Hello world` 与执行时的本地日期时间标记。
5. 在 Changes 中确认文件名、状态与 diff 内容正确，且与命令行 `git status` / `git diff` 的真实结果一致。
6. 在 Terminal 中启动会话、执行只读命令、看到输出并验证输入/输出/resize 的基本生命周期；重连恢复单列为诚实性边界。

### E2 — 修复与复验 ✅

- 每个失败先记录可观察现象和最短复现，再读源码定位根因。
- 只修 E0/E1 主路径必需内容；不新增包、不演进 wire、不引入生产依赖，除非已证明现有契约无法承载且用户批准 ADR。
- 有现有定向测试能证明回归时复用；只有行为改动且现有测试无法捕获时，最多补一条主路径和一条关键失败路径。
- 修复后从失败步骤复跑，最后再完整走一遍 E1，避免用局部绿灯代替用户路径。

### P1 — 项目与会话生命周期 ✅

- **片 1 ✅**：ADR-043 / schema v13 Session→Workspace 弱引用持久化；storage/app 定向门禁与正式 Host/Desktop 重启复验均通过。
- **片 2A ✅**：ADR-044 已由用户 Accepted，冻结 stable workspace identity、本地注册表持久化、legacy `ws-default` 与按 session 路由边界。
- **片 2B ✅**：schema v14 `workspaces` 注册表 + AppCore/GuiHost 按 session workspace 路由（Run / 资源 / `@` 展开 / diff / terminal cwd）；`workspace_add` 幂等登记、`workspace_list` 返回注册表全集合；不改 wire，storage/workspace/app/cli 定向门禁通过。
- **片 2C ✅**：审计确认添加 / 切换 / 重开项目、新建 / 续聊会话五流程在既有 Desktop 已完整接线（零代码改动）；正式 Host/Desktop 真窗口验收通过（隔离实例双粒度重开 + 按项目归属绑定核对），Desktop 定向门禁 147/147；P1 收口。

### P2 — Agent 主路径可靠性 ✅

退出条件（§4）：发送、审批、取消、失败恢复、重放与文件写入形成一条可靠闭环。切片如下，每片数小时内可验收：

- **片 1 ✅**：六链路（发送 / 审批 / 取消 / 失败恢复 / 重放 / 文件写入）可靠性缺口审计（glm_explorer，只读零改动）。结论：发送 / 取消（三相位均有测试钉住，Desktop Cancel 全相位可达）/ 审批（含重启恢复）/ 重放（三态 + lagged）四链路闭环；两个真实缺口同根——①无终态事件的 run（Host 崩溃 / sink 持久化失败）在重放侧永远悬空（runs 表停 "running"，timeline 工具行永 "running"，启动无清扫，合成终态只上 wire 不落库）；②checkpoint 止步于持久化层（快照失败仅 warn 静默跳过，Desktop 零消费）。
- **片 2A ✅（已实现 + 定向门禁通过，待片 3 真窗口验证）**：悬空 run 诚实收口。open_store 启动清扫：state=running 的 run 追加持久化 ToolExecutionCompleted(is_error)（非 waiting 悬空工具）+ RunFailed(Internal)，waiting 审批保持 pending 可决议、幂等、单 session 失败不阻断启动；live 合成终态闸改 persist-first（持久化失败才退回 publish_raw 合成兜底）。写入集 crates/app 四文件，新增测试 4 条，cargo test -p pawork-app --offline --lib --tests 全绿；零 wire/schema 演进；app.md Spec 已同批回写（§4.1/§4.6/§5/§7）。
- **片 2B ✅（已实现 + 定向门禁通过，待片 3 真窗口验证）**：checkpoint 失败诚实化。LoopContext::snapshot_write_tools 增 LoopEventEmitter 参数，快照失败经 emitter 发可持久化 Diagnostic{checkpoint.snapshot_failed}（写入继续）；protocol 投影两臂沿用 sandbox.fallback 模式渲染提示行。写入集 engine/app/protocol，新增测试 2 条，cargo test -p pawork-engine -p pawork-app -p pawork-protocol --offline --lib --tests 全绿；零 wire/schema 演进；engine.md/protocol.md Spec 已同批回写。不加回滚 UI（归候选池 A3）。
- **片 3 ✅**：真窗口闭环验收通过（隔离实例 p2-3，glm_worker）：六链路一轮走通，UI（AX）+ SQLite/磁盘/git 双证据；取消两相位、kill -9 两相位失败恢复（诚实 RunFailed + 清扫幂等）、续聊重放、checkpoint 失败诊断真实触发落库；Desktop 门禁 147/147。三条观察项登记 §5。

不改动范围：不演进 wire/schema；不新增包与生产依赖；Terminal stop/close 与 live exit/failure 仍归 P3 边界；不改 Provider 通道与多 Agent orchestration。

### P3 — Changes / Terminal / Resources 完整性 ✅（2026-09-01 收口，细节归档 [docs/history.md](docs/history.md)）

退出条件（§4）：三面板只展示 Host 权威数据，关键动作与错误恢复完整。切片如下，每片数小时内可验收：

- **片 1 ✅**：三面板完整性缺口审计（glm_explorer，只读零改动）。结论：Changes 面（latest-session 三重 fail-closed、mismatch banner、断线 stale、重连刷新）与 Resources 面（mcp_list 权威数据、epoch 拒旧、stale 标记）在冻结契约内无真实缺口；缺口集中 Terminal 面——G1 resize 不可变参实为 no-op（A）、G2 gate 单一化断 exited 重建与瞬态失败重试（A）、G3 cwd 展示伪造（A）；G4 完整终端生命周期（stop/close + live exit）为已登记 B 边界须 ADR；G5 mcp_test 类动作归候选池；P2 遗留 ①② 不归 P3（①建议独立任务：Host 取消收口补 ToolCompleted；②engine 文案分支）。
- **片 2 ✅（已实现 + 定向门禁通过，待片 3 真窗口验证）**：G1 Terminal 尺寸 stepper（−W/+W/−H/+H 本地草稿钳制 20–500 列 / 6–200 行，apply 走冻结 `terminal_resize`，可见/键盘/AX 三路径同 gate，匹配回执或终端切换后草稿复位）；G2 已知 exited/killed 终端 Start 单槽变 New（同 workspace/cwd 新建，旧终端只读保留不伪造生命周期）、瞬态 write/resize 失败在 runtime running 时不锁死仅 status_hint 报错；G3 Host `terminal_snapshots()` 补 `cwd` 键（注册表值 `owner\0cwd` 编码，快照段不透明 JSON 零 wire/golden 演进，缺键省略）、Desktop 缺键诚实显示 unknown。主代理收口与 review：回滚 worker 越界的 20 文件 import 重排（纯 churn）；修复 create 失败/断连后的 cwd 残留、New 在途仍可重复触发、AX 未播报尺寸草稿、空 cwd 显示空白，以及 resize 迟到回执跨 workspace 清错草稿/新终端误用当前终端尺寸；`cargo test -p pawork-app --offline --lib --tests`（187）与 `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`（151，新增 4 条且扩展既有 gate 测试）复跑全绿；desktop.md / app.md Spec 已同批回写。AX 侧 stepper 节点 bounds 为固定偏移近似，精确几何归 P4 签字。
- **片 3 ✅**：真窗口验收通过（隔离实例 p3-3，glm_worker，六场景 PASS，UI（AX）+ stty/pwd/git/SQLite/快照双证据）：G1 stepper Apply 后 PTY 真实 `stty size` 24x80→28x88、AX 四节点 press 可达；G2 `exit` 后经断连重连快照得 exited → Start 变 New → 新终端同 workspace/cwd（pwd 一致）、旧输出只读保留、无伪造 Stop（Host 重启变体不适用：注册表进程内，快照无终端如实 not started）；G3 外部客户端以 `working_directory=src` 建终端，重连后面板 cwd=src 与 pwd 一致；Changes（diff 与 git status 一致）/ Resources（mcp_list 权威、断线 stale 不伪装、Reconnect 恢复刷新）冒烟通过；Desktop 门禁 151/151 复跑。片 3 发现一真实缺陷已由主代理同波修复：Desktop 首次 Start 传 `.`，策略层归一为空串记账导致重连后面板 cwd 空白——`terminal_cwd_label` 根目录标签归一 + 定向测试 1 条（app 门禁 187 全绿）。

- **片 4 ✅**：G4 Terminal 完整生命周期经 [ADR-045](docs/adr/ADR-045-terminal-lifecycle-wire-evolution.md)（用户 2026-09-01 Accepted）演进 wire——`terminal_close` 命令（`PtyService::cleanup` 幂等终止并移除 PTY service 条目，再注销 GuiHost 注册表；未知/重复 id 报 `not_found`）；`TerminalExited` live 事件（Exited/Killed/Failed 三态，waiter 将权威终态随内部 `PtyEvent::Exit` 传给 forwarder 唯一广播点，cleanup 与异步广播无竞态，IO 异常诚实 Failed 不臆造退出码）；API minor 1.2→1.3（新事件按协商 minor 门控推送，老连接仍从快照 `state` 获知终态）；golden 先行（34 fixture）。Desktop Stop/Close 同槽按钮（视觉/键盘/AX 三路径同源 gate）：exited/killed 可 Close 或 New；failed 只开放 Close，清理后回到 Start，避免 forwarder 断流时遗留仍运行的旧进程；live 终态即时刷新，Close 回执本地移除复位 not started。真窗口验收（隔离实例 adr045，glm_worker）发现并修复两个真实缺陷：cli ACP `app_event_kind` 漏新变体臂致 Host 编译失败（补臂）；Stop 后 Close 报 not registered 面板卡死（宿主 not_found 映射 `RequestNotFound` 可观察 + Desktop 按清理已达成收敛）。复验四场景全过：Stop→无重连即时 killed（ps 进程组证据）、Close→复位 not started 快照清空、exit 7→即时 exited、断线 stale 不回归。提交后 review 再修复 typegen 生成物遗漏、PTY service 会话泄漏、Stop/Close 回执先后竞态与 failed 终端无恢复入口；顺带修复 client contract harness 预存失败（ADR-044 后未登记 ws-default，base 复现确认）；protocol/app/client/exec/desktop Spec 同批回写。

不改动范围：不再演进 wire schema（仅补齐 ADR-045 已接受版本的检入 TypeScript 生成物）；Terminal stop/close 与 live exit/failure 已经 ADR-045 拍板实施，其余 wire 冻结不变；Changes 面维持只读（git_stage 接线仍属 ADR 候选）；不新增包与生产依赖；Timeline / Composer / 审批主链路非本片范围；不跑全量门禁。

## 3. 本轮验收矩阵

| 能力 | 通过条件 | 证据 |
| --- | --- | --- |
| 构建/启动 | 脚本可从仓库根构建两个正式二进制并打开真窗口；重复启动不产生双 Host。 | ✅ `./scripts/pawork-desktop.sh build/start`；独立 `desktop` 实例；Connected 真窗口 |
| 项目 | UI 可将一个真实目录注册为 workspace/project，并显示可辨识项目名。 | ✅ `Add project…` 选择仓库根；Host `workspace_add`；Scope 显示 `Pawork`。✅ P1 片 2C：系统选择器登记第二真实项目 `p1-2c-proj`，scope 切换与双粒度重开复现项目集合 |
| 对话与文件 | 消息实际发送；Agent 完成文件写入；磁盘文件含 `Hello world` 与本轮日期时间。 | ✅ 真实 Provider Run + 显式写入审批；两行标记文件实测生成（一次性产物，已随清理移除） |
| Git Changes | UI 文件状态、diff 与仓库命令行事实一致；空态/非 Git 目录诚实显示。 | ✅ UI `untracked · +2 / −0`；`git status --short` 与定向 diff 一致 |
| Terminal | UI 可创建 Terminal、执行只读命令并显示真实 stdout；错误与断线不伪装成功。 | ✅ 真实 PTY 执行 `pwd` 与 `terminal-ok`；可见文本/AX 不再暴露 ANSI/VT 控制串 |
| 恢复与诚实性 | 重新打开任务或重连后，项目、对话、Changes 与 Terminal 的可恢复部分符合现有协议；不可恢复能力明确说明。 | ✅ P1 片 1：schema v13 `workspace_id=ws-default` 与 Task/Timeline/Changes 跨 Host 重启恢复；Terminal 进程不恢复但 workspace/cwd 仍正确，新 PTY `pwd` 为同一仓库。✅ P1 片 2C：项目集合（schema v14 注册表）与会话归属跨 Desktop 重启、Host 重启 + Reconnect 双粒度复现；断线诚实显示 Disconnected + Reconnect。 |

真窗口证据只用于本轮报告，不重新堆入 `docs/ui-review/`。长期视觉基准只保留 [design/README.md](design/README.md) 所列三张初始设计图。

## 4. 后续计划

E0–E2、P1、P2 与 P3 已完成。后续按以下顺序推进；每项在开启前再拆成数小时内可验收的小任务，不预建兼容层或第二套实现。

| 优先级 | 主题 | 进入条件 | 退出条件 |
| --- | --- | --- | --- |
| ~~P2~~ ✅ | Agent 主路径可靠性（已收口，2026-09-01） | P1 可稳定复现真实 Run | 发送、审批、取消、失败恢复、重放与文件写入形成一条可靠闭环 |
| ~~P3~~ ✅ | Changes / Terminal / Resources 完整性（已收口，2026-09-01） | P2 产出真实工具与文件事件（已满足） | 三面板只展示 Host 权威数据，关键动作与错误恢复完整 |
| P4 | Accessibility 与跨平台 | macOS 核心路径稳定 | 键盘/AX/VoiceOver 主路径通过；Linux/Windows 能力和缺口有真实平台证据 |
| P5 | 发布准备 | P1–P4 完成且用户授权发布任务 | License、供应链、安装/升级/回滚和三平台发布门禁另立任务并通过 |

## 5. 开放边界

- 凭证只从 Pawork 正式 auth store 或显式环境 fallback 读取，不进入脚本、截图、日志、数据库事件或提交文件。
- 文件与命令操作继续受 Workspace、Policy、Sandbox 与审批约束；检查脚本只对该次 Host 进程显式启用 workspace trust 与 `ask-for-dangerous`，不修改持久配置，写文件仍经显式审批，危险命令仍受闸。
- Desktop 仍是独立 GPUI 进程，只经 GUI Connection Protocol 访问 Core；不直连 Provider、Git、数据库或 PTY。
- 若“添加项目”在现有 Desktop 不可达，优先复用冻结的 `workspace_add` 命令；任何需要新增 wire 的方案先停在 ADR 决策。
- Terminal Stop/Close 与 live exit/failure 事件已经 [ADR-045](docs/adr/ADR-045-terminal-lifecycle-wire-evolution.md) 落地（API 1.3：`terminal_close` + `TerminalExited`，按协商 minor 门控推送），UI Stop/Close 为真实 wire 能力；写入 `exit` 文本冒充终止的伪造路径仍禁止。
- 发布、提交、推送、生产部署与真实账户变更不在本轮授权范围。
- P2 遗留观察项（不阻塞；经 P3 片 1 审计判定 ①② 不归 P3，各立独立小任务——①冻结 wire 内 Host 取消收口补 `ToolCompleted{success:false}`，②engine 侧按写入被拒/继续分支措辞）：①审批等待相位取消 run 后，waiting tool call 的工具行显示 "running"，只剩用户决议一条闭合路径；②`checkpoint.snapshot_failed` 文案 "write proceeded without rollback point" 在越界写被工具层拒绝的场景与实际不符；③Desktop 进程被 SIGSTOP ≥30s 后 AX 树永久退化（与 P1 片 2C 窗口异常同类，倾向 macOS 环境非产品缺陷）；④Host 启动 chatgpt probe 401 warn 与重启后 usage ledger record id conflict warn（既有现象）。
- P3 遗留观察项（不阻塞，归后续任务评估）：①多终端时面板粘住当前终端，外部客户端建的终端需任务往返切换才浮出（running 优先 + 最小 session_id 的选择设计，P4 或独立任务评估）；②Changes Files 清单为 session-diff 语义——无 run 的会话显示 0 files 并如实标注 latest-session，终端直接写入只体现在 Summary 的 dirty_files（设计事实，非缺口）；③Terminal AX stepper 节点 bounds 为固定偏移近似，精确几何归 P4 签字。

## 6. 计划事实源

- 当前不保留进行中阶段任务书；`plan/` 为空，后续确有需要时再按数小时可验收粒度新建。
- 活动目标、顺序、状态与候选统一登记在本文；已完成过程只进入 [docs/history.md](docs/history.md)，不回填旧计划。
- 事实优先级：当前工作区 / 真实运行状态 > 源码与冻结契约 > 本文 > 历史记录。

## 7. 执行与收尾纪律

### 7.1 任务开启

- 先写清目标、非目标、验收标准与不改动范围；进包前只读该写入集对应的 `docs/spec/crates/<pkg>.md`。
- 先确认已有实现和未提交改动，再补剩余缺口；不为未来候选预建抽象、兼容层或第二套实现。

### 7.2 实现

- 保留用户未提交改动；写入集只覆盖当前主路径修复及必要文档/Spec。
- 涉及 wire/schema/架构红线、生产依赖或发布动作时先停下走 ADR/用户授权。

### 7.3 验证

- 按存在性与 diff → 写入集定向测试 → 正式二进制构建 → 真窗口主路径推进，前一层失败先收敛原因。
- 默认命令为 `cargo test -p <crate> --offline --lib --tests`；无测试或只需类型检查时用 `cargo check -p <crate> --offline`。多包仍只开一个 Cargo 进程。
- 不运行 `cargo clean` 或 workspace 全量门禁；只有发布任务另行定义全量门禁。

### 7.4 证据

- “已实现”“自动门禁通过”“真窗口通过”“等待人工验收”“已发布”分别表述。
- 真窗口结论同时提供 UI 状态与至少一个源码外事实（文件、Git、Host、PTY 或真实 Provider）。

### 7.5 回写与报告

- 每次收尾更新 §1、§3 与 §4；包行为或边界变化同批回写对应 Spec。
- 完成细节归入 [docs/history.md](docs/history.md)，ROADMAP 不累积过程日志。
- 最终报告列出实际实现、实际命令/场景、定向回归、真窗口证据、未验证项，并固定声明 `Full workspace gate: NOT RUN（当前未设置全量门禁）`。
