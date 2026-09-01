# Pawork

> 纯 Rust 编码智能体核心平台 —— CLI 与 Core 同进程同二进制，无 Node / Bun / JavaScript Runtime。

Pawork 用 Rust 从零实现一个编码智能体（Coding Agent）平台核心。二进制 `pawork` 是 Core 的唯一正式宿主；Desktop GUI（GPUI，`apps/desktop`）作为独立进程，经 CLI 暴露的 GUI Connection Protocol 连接 Core。

**当前状态（2026-09-02）**：正式 Host/Desktop 的项目、对话、文件、Git Changes 和 Terminal 真实核心路径已经验收；旧阶段已从活动路线图移出。当前活动线是 Desktop Settings，先完成 Z.AI/GLM、Kimi、DeepSeek、xAI/Grok 的连接认证、模型发现与默认项；[SET-0 立项与 ADR-046（协议 API 1.4）](docs/spec/settings.md)已拍板，SET-1 协议词汇与 SET-2 Host settings 门面已实现并通过定向测试，Desktop UI（SET-3）与四家真实认证验收未开始。当前指针见 [ROADMAP.md](ROADMAP.md)，执行任务书见 [plan/settings.md](plan/settings.md)，历史沿革见 [docs/history.md](docs/history.md)。V1 全量实现归档于仓库外 `../Pawork_v1/`。

## 快速开始

```bash
./scripts/pawork-desktop.sh start # 构建正式 Host/Desktop 并打开 UI
./scripts/pawork-desktop.sh build # 仅构建正式 Host/Desktop
./target/debug/pawork chat       # 流式多轮对话
./target/debug/pawork models     # 各通道聚合的模型列表
./target/debug/pawork sessions list
./target/debug/pawork gui serve  # 启动 GUI 连接服务
```

Desktop 启动脚本不加载 fixture、seed 或测试 profile。它默认使用独立的真实实例 `desktop`，避免把日常 CLI 会话混入 UI 检查；可用 `PAWORK_DESKTOP_INSTANCE=<name>` 覆盖。脚本为本次 Host 进程显式信任 UI 选择的 workspace（不写配置），并默认使用 `ask-for-dangerous`：普通写入和默认 shell 可运行，危险命令仍需审批；这是现有 Terminal Policy 闸允许真实 PTY 的档位。可用 `PAWORK_DESKTOP_APPROVAL_MODE=ask-for-writes` 改为逐写审批（此时 Terminal 会按既定策略 fail-closed），或用 `PAWORK_DESKTOP_TRUST_WORKSPACES=0` 关闭进程级信任。上述参数只在脚本新启 Host 时生效；若复用已运行实例，则沿用该 Host 的设置。Desktop 退出时只关闭脚本自己启动的 Host，日志位于 `target/pawork-desktop-runtime/host.log`。

凭证经 `pawork auth` 写入 `~/.pawork/auth.json`；env 变量仅作遗留 fallback。Secret 红线：key/token 不入日志、事件与任何可提交文件。

## 仓库结构

```text
Pawork/                  # 仓库根 = Cargo workspace 根
├── crates/              # 19 个库（扁平布局，目录 = 包名去 pawork- 前缀）
│   ├── domain/          # canonical 领域 + provider_api/tool_api 契约 + 事件信封 golden
│   ├── protocol/        # GUI 帧 / headless-json / core-api / typegen
│   ├── testkit/         # dev-only mock 与契约断言
│   ├── policy/          # 安全内核（PolicyDecision/ApprovalMode）
│   ├── exec/            # 进程/沙箱/PTY
│   ├── tools/           # 八工具 + scheduler + mcp/
│   ├── workspace/       # workspace 服务 + resources/ + config/ + import/
│   ├── storage/         # sqlite/ + session/ + blob/（PWB1）
│   ├── providers/       # net/ + registry/pricing/usage/negotiate/reasoning + channels/（六通道）
│   ├── auth/            # Secret 后端 / OAuth / 脱敏解析链
│   ├── git/             # Diff/Status/Checkpoint/worktree
│   ├── engine/          # Agent Engine（生产依赖仅 domain）
│   ├── workflow/        # plan/task 纯 reducer
│   ├── orchestration/   # supervisor/budget/lifecycle/task_graph
│   ├── control-plane/   # 控制面 core + quota/ + credential/
│   ├── transport/       # local（UDS）+ memory
│   ├── app/             # 装配宿主 + gui_server/
│   ├── cli/             # 21 子命令 + channels/acp/
│   └── client/          # framed 连接面 + headless/（原 sdk）
├── apps/                # pawork（CLI 宿主 + composition root）、desktop（GPUI GUI）
├── schemas/             # protocol typegen 检入的 .d.ts
├── fixtures/            # 测试夹具
├── scripts/             # 维护脚本（如 stale incremental 清理）
├── design/              # GUI 三张初始视觉基准图
├── docs/                # 架构、设计、Spec、参照、存档、ADR
├── plan/                # 当前活动线任务书（当前仅 Settings）
└── ROADMAP.md           # 当前任务与后续计划的唯一计划事实源
```

21 成员（19 库 + 2 应用，ADR-039 定稿）。包清单、依赖方向与冻结契约见 [docs/architecture.md](docs/architecture.md)。

## 文档导航

| 文档 | 职责 |
| --- | --- |
| [ROADMAP.md](ROADMAP.md) | 任务事实源：当前指针、Settings 阶段、开放决策与任务约定 |
| [plan/settings.md](plan/settings.md) | Settings 活动线的切片、写入集、验收和停止条件 |
| [docs/architecture.md](docs/architecture.md) | 架构事实源：红线、包布局与依赖方向、冻结契约、S13 安全拍板、ADR 索引 |
| [docs/design.md](docs/design.md) | 功能设计事实源：功能域 ↔ 参照项目映射、已确认扩展功能族（G1–G7）、候选功能池 |
| [docs/spec/README.md](docs/spec/README.md) | 产品与包级 Spec 总索引：产品范围/能力/契约/安全/Desktop/验证/运维 + 21 包逐包 Spec + 跨包链路 |
| [docs/spec/settings.md](docs/spec/settings.md) | Settings Feature Spec：供应商认证、模型发现、默认项、IA、安全和验证 |
| [docs/spec/crates/](docs/spec/README.md#12-包级-spec) | **包级 Spec**：每包一篇，读文档即可了解该包全部功能与行为（agent 辅助阅读主入口） |
| [docs/spec/flows.md](docs/spec/flows.md) | 跨包核心链路：Agent loop / GUI 连接 / 事件持久化与重放 / 凭证与脱敏 |
| [docs/gui-design.md](docs/gui-design.md) | Desktop GUI 设计（配套 [design/README.md](design/README.md) 视觉基准与设计图） |
| [docs/references.md](docs/references.md) | 参照项目手册 + 调研附录（多账户/配额/缓存） |
| [docs/history.md](docs/history.md) | 历史存档：V1 迁移、V2（S0–S13）交付、V3（R0–R9）各阶段收口记录、已关闭登记项 |
| [docs/adr/](docs/adr/) | 架构决策记录（ADR-037～045 现存；001～036 随 V1 归档） |
| [AGENTS.md](AGENTS.md) | 工程约定（代理与人类协作者） |

V1 时期文档随 V1 归档于 `../Pawork_v1/docs/`，仓库内链接以 `../Pawork_v1/...` 标注。

## 贡献

- 工程约定见 [AGENTS.md](AGENTS.md)；任务执行与收尾约定见 [ROADMAP.md](ROADMAP.md) §6–§7。当前不设全量门禁，发布须用户明确授权后另立任务。
- 架构决策以 ADR 记录（[docs/adr/](docs/adr/)，编号续接，下一个为 Settings 契约 ADR-046）。
- 当前**不新增包**，只往既有包加模块；包布局变更须先过 ADR。

## 许可证

待定。发布不在当前计划内；由用户后续单独立项时一并拍板。
