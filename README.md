# Pawork

> 纯 Rust 编码智能体核心平台 —— CLI 与 Core 同进程同二进制，无 Node / Bun / JavaScript Runtime。

Pawork 用 Rust 从零实现一个编码智能体（Coding Agent）平台核心：以 Pi 的功能与工作流为参考，但**不复用**其 TypeScript 实现。二进制 `pawork` 是 Core 的唯一正式宿主；Desktop GUI（GPUI，`apps/desktop`）作为独立进程，经 CLI 暴露的 GUI Connection Protocol 连接 Core。

当前仓库为 V2 重构后的增量开发主线（S0–S13，见 [ROADMAP.md](ROADMAP.md)）。V1 全量实现（88 crate）已于 2026-08-17 归档至仓库外同级目录 `../Pawork_v1/`：移出 git 管理，仅作为迁移参照与历史快照保留。

## 项目状态（2026-08-18）

| 阶段 | 主题 | 状态 |
| --- | --- | --- |
| S0–S5 | 最小对话 → 会话持久化 → 工具循环 → 写入审批 → 命令执行与沙箱 → 上下文预算与用量 | 🟢 |
| [S6](plan/S6-providers-auth.md) | 首发 Provider 与认证（六通道、OAuth、auth 文件） | 🔵 |
| [S7](plan/S7-gui-agent.md) | 最小 Agent GUI（v3 三栏工作台已交付） | 🟢 |
| [S8](plan/S8-git-checkpoint.md) | Git、Diff 与 Checkpoint（rollback 一键回滚） | 🟢 |
| [S9](plan/S9-mcp-resources.md) | MCP、资源与兼容导入 | 🟢 |
| [S10](plan/S10-serve-clients.md) | 服务化与客户端补齐 | 🟢 |
| [S11](plan/S11-workflow-control.md) | 工作流、多 Agent 与控制面 | 🟢 |
| [S12](plan/S12-project-code-review.md) | 全项目 Code Review 与整改拆分（只读） | 🟢 |
| [S13](plan/S13-s12-remediation.md) | S12 finding 整改（波 A ✅ · 波 B Bug → 波 C 收口） | 🔵 |

状态符号：⚪未开始 · 🔵进行中 · 🟢已完成。阶段明细与真实验收要点见 [ROADMAP.md](ROADMAP.md) §2。

## 快速开始

```bash
cargo build                      # workspace dev 构建
./target/debug/pawork chat       # 流式多轮对话
./target/debug/pawork models     # 各通道聚合的模型列表
./target/debug/pawork sessions list
./target/debug/pawork gui serve  # 启动 GUI 连接服务（S7）
```

凭证经 `pawork auth`（S6）写入 `~/.pawork/auth.json`；env 变量仅作遗留 fallback。Secret 红线：key/token 不入日志、事件与任何可提交文件。

## 仓库结构

```text
Pawork/                  # 仓库根 = Cargo workspace 根
├── apps/                # 可执行入口：pawork（CLI 宿主）、desktop（GPUI GUI）
├── foundation/          # domain、api、protocol、config、sqlite、testkit、diagnostics
├── engine/              # Agent Engine（工具循环、上下文、事件）
├── execution/           # exec（进程/沙箱）、policy、tools
├── providers/           # core、adapters（多通道）、auth
├── storage/             # session（持久化/重放）、blob
├── host/                # app、cli、gui-server、transport
├── clients/             # gui-client
├── net/                 # HTTP/传输基础
├── vcs/                 # git（diff/checkpoint/rollback）
├── extensions/          # mcp（S9）
├── workspace/           # core（workspace 服务）、resources（AGENTS.md/Skills 加载）
├── fixtures/            # 测试夹具
├── design/              # GUI v3 定稿视觉基准
├── docs/                # 设计、规范、参照、迁移词典
└── plan/                # 阶段任务书 S0–S13
```

包布局与激活映射（40 包 + 3 应用）见 [docs/design.md](docs/design.md) §2；冻结契约与「追加不重写」三道保险见 §3。

## 文档导航

| 文档 | 职责 |
| --- | --- |
| [ROADMAP.md](ROADMAP.md) | 任务总索引：阶段状态、阶段外任务、未决事项、风险 |
| [plan/](plan/) | 阶段任务书（附件 [plan/archive/](plan/archive/README.md)：旧按域计划索引） |
| [docs/design.md](docs/design.md) | 设计与冻结契约 |
| [docs/gui-design.md](docs/gui-design.md) | Desktop GUI 设计（v3 基准） |
| [docs/references.md](docs/references.md) | 参照项目手册 |
| [docs/task-guide.md](docs/task-guide.md) | 任务实现规范（公共提示词） |
| [docs/v1-migration-reference.md](docs/v1-migration-reference.md) | V1→V2 迁移词典（冻结参考） |
| [AGENTS.md](AGENTS.md) | 工作约定（V2 版） |

V1 时期文档（架构、ADR-001~035、features、quality、REVIEW 等）随 V1 归档于 `../Pawork_v1/docs/`，仓库内链接以 `../Pawork_v1/...` 标注。

## 贡献

- 工作约定见 [AGENTS.md](AGENTS.md)；V2 当前路线的定向验证约定见 [docs/task-guide.md](docs/task-guide.md) §6。S12 只做审查与任务登记，S13 按波次整改；全量门禁和发布需在 S13 收口后另立任务。
- 架构决策须以 ADR 记录，编号续接 V1。
- 新增包须在 [docs/design.md](docs/design.md) §2 登记并明确依赖方向。

## 许可证

待定（见 [ROADMAP.md](ROADMAP.md) §4 未决事项）。
