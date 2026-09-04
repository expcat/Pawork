# Pawork

> 纯 Rust 编码智能体核心平台 —— CLI 与 Core 同进程同二进制，无 Node / Bun / JavaScript Runtime。

Pawork 用 Rust 从零实现一个编码智能体（Coding Agent）平台核心。二进制 `pawork` 是 Core 的唯一正式宿主；Desktop GUI（GPUI，`apps/desktop`）作为独立进程，经 CLI 暴露的 GUI Connection Protocol 连接 Core。

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
│   ├── providers/       # net/ + registry/pricing/usage/negotiate/reasoning + channels/
│   ├── auth/            # Secret 后端 / OAuth / 脱敏解析链
│   ├── git/             # Diff/Status/Checkpoint/worktree
│   ├── engine/          # Agent Engine（生产依赖仅 domain）
│   ├── workflow/        # plan/task 纯 reducer
│   ├── orchestration/   # supervisor/budget/lifecycle/task_graph
│   ├── control-plane/   # 控制面 core + quota/ + credential/
│   ├── transport/       # local（UDS）+ memory
│   ├── app/             # 装配宿主 + gui_server/
│   ├── cli/             # 21 子命令 + channels/acp/
│   └── client/          # framed 连接面 + headless/
├── apps/                # pawork（CLI 宿主 + composition root）、desktop（GPUI GUI）
├── schemas/             # protocol typegen 检入的 .d.ts
├── fixtures/            # 测试夹具
├── scripts/             # 维护脚本
├── design/              # GUI P0–P2 三张阶段视觉基准图
└── docs/                # 架构、设计、Spec、参照
```

21 成员（19 库 + 2 应用）。包清单、依赖方向与冻结契约见 [docs/architecture.md](docs/architecture.md)。

## 文档导航

| 文档 | 职责 |
| --- | --- |
| [docs/architecture.md](docs/architecture.md) | 架构：红线、包布局与依赖方向、冻结契约、安全语义 |
| [docs/design.md](docs/design.md) | 功能设计：能力域与参照项目映射、明确不做的形态 |
| [docs/spec/README.md](docs/spec/README.md) | 产品与包级 Spec 总索引 |
| [docs/spec/crates/](docs/spec/README.md#12-包级-spec) | 每包一篇 Spec（agent 辅助阅读主入口） |
| [docs/spec/flows.md](docs/spec/flows.md) | 跨包核心链路 |
| [docs/spec/settings.md](docs/spec/settings.md) | Settings Feature Spec |
| [docs/gui-design.md](docs/gui-design.md) | Desktop GUI 设计（配套 [design/README.md](design/README.md)） |
| [docs/gui-optimization.md](docs/gui-optimization.md) | Desktop UI / Design 全面优化方案（现状审计、竞品对比、逐区修改与验收） |
| [docs/gui-roadmap.md](docs/gui-roadmap.md) | Desktop UI 优化 Roadmap（P0–P2 子任务、依赖、写入集与阶段验收） |
| [docs/references.md](docs/references.md) | 参照项目手册与调研附录 |
| [AGENTS.md](AGENTS.md) | 工程约定与开发经验 |

## 贡献

工程约定见 [AGENTS.md](AGENTS.md)。当前不设全量门禁；发布须用户明确授权后另立任务。不新增包，只往既有包加模块；包布局、冻结契约或安全语义的破坏式改动须先向用户确认。

## 许可证

待定。
