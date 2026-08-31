# Pawork 路线图

> 本文是当前任务与后续工作的唯一计划事实源，只保留未完成工作、验收标准和开放决策。已完成阶段与旧编号统一查阅 [docs/history.md](docs/history.md)；架构红线与冻结契约见 [docs/architecture.md](docs/architecture.md)。

## 1. 当前指针

| 字段 | 当前事实 |
| --- | --- |
| 活动线 | **P2 Agent 主路径可靠性** |
| 状态 | 🟢 P1 已收口：E0–E2、片 1、片 2A、片 2B 与片 2C 全部完成。片 2C 审计确认五流程接线已完整落地（零代码改动），正式 Host/Desktop 真窗口验收通过，Desktop 定向门禁 147/147。 |
| 本轮结果 | 添加 / 切换 / 重开项目与新建 / 续聊会话五流程均经正式 UI 走通：真实目录经系统选择器注册为持久项目（schema v14 注册表），Desktop 重启与 Host 重启 + Reconnect 双粒度复现项目集合，按项目新建会话绑定各自 workspace，续聊会话重放持久化时间线，断线诚实显示 Disconnected + Reconnect。 |
| 下一动作 | 按 §4 开启 P2（Agent 主路径可靠性），开启前再拆成数小时内可验收的小任务。 |
| 本轮完成条件 | P1 退出条件「添加 / 切换 / 重开项目与新建 / 续聊会话均可通过正式 UI 完成」已满足（证据归档 [docs/history.md](docs/history.md)）。 |
| 当前阻塞 | 无。完整 Terminal stop/close 与 live exit/failure 仍以 UI 本地诚实实现绕过，归 P3 边界。 |

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

E0–E2 与 P1 已完成。后续按以下顺序推进；每项在开启前再拆成数小时内可验收的小任务，不预建兼容层或第二套实现。

| 优先级 | 主题 | 进入条件 | 退出条件 |
| --- | --- | --- | --- |
| P2 | Agent 主路径可靠性 | P1 可稳定复现真实 Run | 发送、审批、取消、失败恢复、重放与文件写入形成一条可靠闭环 |
| P3 | Changes / Terminal / Resources 完整性 | P2 产出真实工具与文件事件 | 三面板只展示 Host 权威数据，关键动作与错误恢复完整 |
| P4 | Accessibility 与跨平台 | macOS 核心路径稳定 | 键盘/AX/VoiceOver 主路径通过；Linux/Windows 能力和缺口有真实平台证据 |
| P5 | 发布准备 | P1–P4 完成且用户授权发布任务 | License、供应链、安装/升级/回滚和三平台发布门禁另立任务并通过 |

## 5. 开放边界

- 凭证只从 Pawork 正式 auth store 或显式环境 fallback 读取，不进入脚本、截图、日志、数据库事件或提交文件。
- 文件与命令操作继续受 Workspace、Policy、Sandbox 与审批约束；检查脚本只对该次 Host 进程显式启用 workspace trust 与 `ask-for-dangerous`，不修改持久配置，写文件仍经显式审批，危险命令仍受闸。
- Desktop 仍是独立 GPUI 进程，只经 GUI Connection Protocol 访问 Core；不直连 Provider、Git、数据库或 PTY。
- 若“添加项目”在现有 Desktop 不可达，优先复用冻结的 `workspace_add` 命令；任何需要新增 wire 的方案先停在 ADR 决策。
- Terminal 现有协议没有通用 Stop/Close 与 live exit 事件时，UI 必须诚实表达，不用写入 `exit` 冒充正式能力。
- 发布、提交、推送、生产部署与真实账户变更不在本轮授权范围。

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
