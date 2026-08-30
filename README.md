# Pawork

> 纯 Rust 编码智能体核心平台 —— CLI 与 Core 同进程同二进制，无 Node / Bun / JavaScript Runtime。

Pawork 用 Rust 从零实现一个编码智能体（Coding Agent）平台核心。二进制 `pawork` 是 Core 的唯一正式宿主；Desktop GUI（GPUI，`apps/desktop`）作为独立进程，经 CLI 暴露的 GUI Connection Protocol 连接 Core。

**当前状态（2026-08-25）**：既有功能与结构阶段已归档；当前唯一主线从新 R1 开始，按 v3 定稿图完成 Desktop UI 99% 视觉还原、全组件真实交互与模拟操作测试（R1–R8），其余未完成工作顺延至 R9–R11（R11 为 UI 终局比对与优化文档；发布准备见 ROADMAP §5）。当前指针见 [ROADMAP.md](ROADMAP.md)，阶段细节见 [plan/](plan/)，历史沿革见 [docs/history.md](docs/history.md)。V1 全量实现归档于仓库外 `../Pawork_v1/`。

## 快速开始

```bash
cargo build                      # workspace dev 构建
./target/debug/pawork chat       # 流式多轮对话
./target/debug/pawork models     # 各通道聚合的模型列表
./target/debug/pawork sessions list
./target/debug/pawork gui serve  # 启动 GUI 连接服务
```

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
├── design/              # GUI v3 定稿视觉基准（设计图）
├── docs/                # 架构、设计、Spec、参照、存档、ADR
└── plan/                # 进行中阶段的任务书
```

21 成员（19 库 + 2 应用，ADR-039 定稿）。包清单、依赖方向与冻结契约见 [docs/architecture.md](docs/architecture.md)。

## 文档导航

| 文档 | 职责 |
| --- | --- |
| [ROADMAP.md](ROADMAP.md) | 任务事实源：当前指针、剩余任务、未决登记、候选池、任务约定 |
| [plan/](plan/) | 当前 R1–R11 任务书与 Agent UI/测试方法调研；已完成任务不保留在此目录 |
| [docs/architecture.md](docs/architecture.md) | 架构事实源：红线、包布局与依赖方向、冻结契约、S13 安全拍板、ADR 索引 |
| [docs/design.md](docs/design.md) | 功能设计事实源：功能域 ↔ 参照项目映射、已确认扩展功能族（G1–G7）、候选功能池 |
| [docs/spec/README.md](docs/spec/README.md) | 产品与包级 Spec 总索引：产品范围/能力/契约/安全/Desktop/验证/运维 + 21 包逐包 Spec + 跨包链路 |
| [docs/spec/crates/](docs/spec/README.md#12-包级-spec) | **包级 Spec**：每包一篇，读文档即可了解该包全部功能与行为（agent 辅助阅读主入口） |
| [docs/spec/flows.md](docs/spec/flows.md) | 跨包核心链路：Agent loop / GUI 连接 / 事件持久化与重放 / 凭证与脱敏 |
| [docs/gui-design.md](docs/gui-design.md) | Desktop GUI 设计（配套 [design/README.md](design/README.md) 视觉基准与设计图） |
| [docs/references.md](docs/references.md) | 参照项目手册 + 调研附录（多账户/配额/缓存） |
| [docs/history.md](docs/history.md) | 历史存档：V1 迁移、V2（S0–S13）交付、V3（R0–R9）各阶段收口记录、已关闭登记项 |
| [docs/adr/](docs/adr/) | 架构决策记录（ADR-037~041 现存；001~036 随 V1 归档） |
| [AGENTS.md](AGENTS.md) | 工程约定（代理与人类协作者） |

V1 时期文档随 V1 归档于 `../Pawork_v1/docs/`，仓库内链接以 `../Pawork_v1/...` 标注。

## 贡献

- 工程约定见 [AGENTS.md](AGENTS.md)；任务开启/进行/收尾约定见 [ROADMAP.md](ROADMAP.md) §7。当前不设全量门禁，发布须用户明确授权后另立任务。
- 架构决策以 ADR 记录（[docs/adr/](docs/adr/)，编号续接，下一个 ADR-042）。
- 当前**不新增包**，只往既有包加模块；包布局变更须先过 ADR。

## 许可证

待定（见 [ROADMAP.md](ROADMAP.md) §5 发布准备候选）。
