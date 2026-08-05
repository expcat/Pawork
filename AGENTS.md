# AGENTS.md — Pawork 工作指南

本文件是面向在 Pawork 仓库中工作的代理与人类协作者的工程约定。它与服务级 `AGENTS.md`（Codex 全局行为）叠加生效，冲突时以本文件为准。

## 1. 核心原则

- **事实源优先**：以当前分支、工作区差异、源码、生成物、运行日志与真实远程状态为准；历史结论只作检索线索，使用前重新验证。
- **最小写入集**：保留用户已有未提交改动；新增改动只触碰任务必需的文件。
- **先确认已落地的内容，再补缺口**：避免重复规划或重做已完成的工作。
- **范围明确的实现 / 修复任务，定位后直接执行**：不把简单任务过度规划。

## 2. 架构红线（不可违反）

- CLI 与 Core 同进程同二进制（`pawork` 是唯一正式宿主），纯 Rust 实现；不引入 Node / Bun / V8 / 嵌入式 JS Runtime。GUI 作为独立进程经 GUI Connection Protocol 连接 CLI，不嵌入 Core。
- `agent-domain` 不得依赖 Tauri、SQLite、HTTP Client、OS Keychain、Git、任何具体 Provider。
- 禁止 crate 间循环依赖；依赖方向见 [workspace 结构](docs/architecture/workspace-layout.md)。
- Agent Engine 不得通过判断 Provider 名称走特例逻辑（统一走 canonical domain）。
- Secret（明文 Token）不写入数据库与日志。
- 所有 Agent 事件必须可持久化、可重放。
- GUI 不得直接访问 Provider、数据库与工具；只能通过 GUI Connection Protocol 连接 CLI，经 `app-service` 访问 Core（不直接加载 Core crate）。

违反以上任意一条需先升级为 ADR 讨论或向用户确认。详见 [docs/adr/](docs/adr/)。

## 3. 命名与结构约定

- 项目名：`Pawork`；CLI 二进制名：`pawork`。
- `pawork` 是 Core 的唯一正式宿主；不存在独立的 `core-daemon` / `core-cli` / `core-rpc` 入口。
- 仓库根即 Cargo workspace 根，不再嵌套外层目录。
- crate 名沿用设计文档原名（`agent-domain`、`provider-runtime`…）；首次发布到 crates.io 前可选加 `pawork-` 前缀（作为可选后续任务）。
- 新增 crate 须在 [workspace 结构](docs/architecture/workspace-layout.md) 登记，并明确依赖方向。

## 4. 任务粒度

- 每个任务应在数小时内可独立完成、独立验收。
- 写入集尽量收敛到单一 crate 或一组紧相关文件；不同任务不修改同一文件。
- 任何任务完成后，对应模块文档与 ROADMAP 状态须同步更新。
- 详细阶段任务见 [ROADMAP.md](ROADMAP.md)。

## 5. 验证顺序（从低成本到高成本）

1. 存在性 / 差异检查
2. 定向测试（单元 / contract / golden）
3. 构建 / 全量门禁（`cargo build`、`cargo clippy`、`cargo test`）
4. 性能与安全门槛（见 [性能目标](docs/quality/performance-targets.md)、[安全验收](docs/quality/security-acceptance.md)）
5. 模型审查或人工视觉验收

前一层失败时先收敛原因，不盲目推进。

## 6. 文档约定

- 中文撰写，保留关键术语英文。
- 每个功能模块对应一篇 `docs/features/<topic>.md`，结构：职责 / 设计要点 / 接口或数据模型 / 优先级（P0–P2）/ 验收标准 / 相关文档。
- 架构决策用 ADR 记录，编号 `ADR-0xx`，状态字段：Proposed / Accepted / Superseded。
- 交叉引用使用仓库内相对路径链接。

## 7. 提交与分支

- 分支前缀默认 `codex/`，用户另有要求时遵从用户。
- 提交、推送、发布仅在用户请求或已确认任务链明确包含时执行。
- 不使用 `git reset --hard` / `git checkout --` 清理用户改动，除非用户明确要求。
- 优先非交互式 git 命令。

## 8. 安全与权限

- 不执行递归删除、覆盖 workspace 外路径、`$HOME` / 根目录等宽范围破坏性操作。
- 文件操作输入必须基于 `workspace_id + relative_path`，禁止模型直接传任意绝对路径。
- 子进程、网络、Secret 访问须经 Policy / Sandbox 约束（见 [policy](docs/features/policy.md)、[sandbox](docs/features/sandbox.md)）。

## 9. 子代理使用

- 文档等一致性关键产物由主代理直接撰写。
- 实现阶段：边界清晰、写入集互不重叠的任务可并行派发，遵循服务级 `AGENTS.md` 的路由与并发上限。
- 确定性检查先于模型审查；每个门禁只调用一个审查者。

## 10. 常用命令（实现阶段补充）

实现开始后在此补充构建、测试、基准命令。当前仅有文档，无构建命令。
