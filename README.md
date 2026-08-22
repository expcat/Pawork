# Pawork

> 纯 Rust 编码智能体核心平台 —— CLI 与 Core 同进程同二进制，无 Node / Bun / JavaScript Runtime。

Pawork 用 Rust 从零实现一个编码智能体（Coding Agent）平台核心：以 Pi 的功能与工作流为参考，但**不复用**其 TypeScript 实现。二进制 `pawork` 是 Core 的唯一正式宿主；Desktop GUI（GPUI，`apps/desktop`）作为独立进程，经 CLI 暴露的 GUI Connection Protocol 连接 Core。

当前仓库处于 **V3 重构线（R0–R9，见 [ROADMAP.md](ROADMAP.md)）**：V2 增量开发（S0–S13）已于 2026-08-18 完成并总结于 [docs/v2-summary.md](docs/v2-summary.md)；V3 不加新功能，聚焦结构收敛（R0 裁决后 37→21 包，R1 已收口）、依赖治理、补丁根因重构与 GUI 组件化。V1 全量实现（88 crate）已于 2026-08-17 归档至仓库外同级目录 `../Pawork_v1/`：移出 git 管理，仅作为迁移参照与历史快照保留。

## 项目状态（2026-08-19）

V2（S0–S13）全部 🟢：最小对话 → 会话持久化 → 工具循环 → 写入审批 → 命令执行与沙箱 → 上下文预算与用量 → 六通道 Provider 与 OAuth → Agent GUI（三栏工作台）→ Git/Diff/Checkpoint → MCP 与资源 → 服务化与客户端（headless/ACP/SDK）→ 工作流与多 Agent → 全项目 Code Review → 整改收口。唯一挂账：OAuth 自然临期 refresh 人工验收（并入 R9）。

| V3 阶段 | 主题 | 状态 |
| --- | --- | --- |
| [R0](plan/R0-inventory-decisions.md) | 决策收口与休眠库存裁决（ADR-038） | 🟢 |
| [R1](plan/R1-package-consolidation.md) | 包合并 37→21（ADR-039） | 🟢 |
| [R2](plan/R2-dependency-governance.md) | 依赖治理（本地化 / 升级 / 去重） | ⚪ |
| [R3](plan/R3-protocol-unification.md) | 协议与投影同源化 | ⚪ |
| [R4](plan/R4-host-decomposition.md) | 宿主拆解与可靠性内核 | ⚪ |
| [R5](plan/R5-provider-neutrality.md) | Provider 中立化与凭证收口 | ⚪ |
| [R6](plan/R6-session-branching.md) | 会话分支模型原生化（ADR-040） | ⚪ |
| [R7](plan/R7-sandbox-isolation.md) | 执行面真隔离（ADR-041） | ⚪ |
| [R8](plan/R8-gui-components.md) | GUI 组件化与 Desktop 收口 | ⚪ |
| [R9](plan/R9-consistency-closeout.md) | 一致性收口 | ⚪ |

状态符号：⚪未开始 · 🔵进行中 · 🟢已完成。阶段明细、依赖与验收要点见 [ROADMAP.md](ROADMAP.md) §2；开发开启方式见 [v3_plan.md](v3_plan.md)。

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
├── crates/              # 19 个库（ADR-039 扁平布局，目录 = 包名去 pawork- 前缀）
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
├── design/              # GUI v3 定稿视觉基准
├── docs/                # 设计、规范、参照、迁移词典、V2 总结、ADR
└── plan/                # 阶段任务书 R0–R9
```

以上为 V3 定稿布局（21 成员：19 库 + 2 应用，R1 收口 2026-08-19，ADR-039 D1）。包清单与依赖方向见 [docs/design.md](docs/design.md) §2；冻结契约与「追加不重写」三道保险见 §3；R1 合并映射见 [plan/R1-package-consolidation.md](plan/R1-package-consolidation.md)。

## 文档导航

| 文档 | 职责 |
| --- | --- |
| [ROADMAP.md](ROADMAP.md) | 任务总索引：阶段状态、阶段外任务、未决事项、风险 |
| [v3_plan.md](v3_plan.md) | V3 任务开启编排（当前指针、选波规则、子代理派发） |
| [plan/](plan/) | 阶段任务书 R0–R9（附件 [plan/archive/](plan/archive/README.md)：旧按域计划索引） |
| [docs/design.md](docs/design.md) | 设计与冻结契约 |
| [docs/gui-design.md](docs/gui-design.md) | Desktop GUI 设计（GUI v3 视觉基准） |
| [docs/references.md](docs/references.md) | 参照项目手册 |
| [docs/task-guide.md](docs/task-guide.md) | 任务实现规范（公共提示词） |
| [docs/v2-summary.md](docs/v2-summary.md) | V2（S0–S13）归档总结：交付、冻结契约、遗留债务 |
| [docs/v1-migration-reference.md](docs/v1-migration-reference.md) | V1→V2 迁移词典（冻结参考） |
| [docs/code-map/README.md](docs/code-map/README.md) | 三层代码地图（总索引 + 各 crate/app `MODULE.md`） |
| [AGENTS.md](AGENTS.md) | 工作约定（V3 版） |

V1 时期文档（架构、ADR-001~035、features、quality、REVIEW 等）随 V1 归档于 `../Pawork_v1/docs/`，仓库内链接以 `../Pawork_v1/...` 标注。

## 贡献

- 工作约定见 [AGENTS.md](AGENTS.md)；定向验证约定见 [docs/task-guide.md](docs/task-guide.md)。V3 期间不设全量门禁，发布须用户明确授权后另立任务。
- 架构决策须以 ADR 记录，编号续接 V1（现有 [ADR-037](docs/adr/ADR-037-s13-wave-b-contracts.md)；V3 将新增 ADR-038~041）。
- V3 期间**不新增包**，只做合并与收敛；包布局变更经 [plan/R1](plan/R1-package-consolidation.md) 与 ADR-039。

## 许可证

待定（见 [ROADMAP.md](ROADMAP.md) §4 未决事项）。
