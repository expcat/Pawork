# AGENTS.md — Pawork 工作指南（V2 版）

本文件是面向在 Pawork 仓库中工作的代理与人类协作者的工程约定。它与服务级 `AGENTS.md`（Codex 全局行为）叠加生效，冲突时以本文件为准。本版于 2026-08-17 随「V1 归档 + V2 升为仓库根」重建；V1 版归档于仓库外同级目录 [../Pawork_v1/AGENTS.md](../Pawork_v1/AGENTS.md)。

## 1. 核心原则

- **事实源优先**：以当前分支、工作区差异、源码、生成物、运行日志与真实远程状态为准；历史结论只作检索线索，使用前重新验证。
- **最小写入集**：保留用户已有未提交改动；新增改动只触碰任务必需的文件。
- **先确认已落地的内容，再补缺口**：避免重复规划或重做已完成的工作。
- **范围明确的实现 / 修复任务，定位后直接执行**：不把简单任务过度规划。

## 2. 架构红线（不可违反）

- CLI 与 Core 同进程同二进制（`pawork` 是唯一正式宿主），纯 Rust 实现；不引入 Node / Bun / V8 / 嵌入式 JS Runtime。GUI 以独立 GPUI 进程（`apps/desktop`）经 GUI Connection Protocol 连接 CLI，不嵌入 Core；Desktop 构建链同样保持纯 Rust。
- `pawork-domain`（foundation/domain）不得依赖任何 GUI framework（包括 GPUI/Tauri）、SQLite、HTTP Client、OS Keychain、Git、任何具体 Provider。
- 禁止包间循环依赖；包布局与依赖方向见 [docs/design.md](docs/design.md) §2。
- Agent Engine 不得通过判断 Provider 名称走特例逻辑（统一走 canonical domain）。
- Secret（明文 Token）不写入数据库与日志。
- 所有 Agent 事件必须可持久化、可重放。
- GUI 不得直接访问 Provider、数据库与工具；只能通过 GUI Connection Protocol 连接 CLI，经 CLI 宿主访问 Core（不直接加载 Core crate）。

违反以上任意一条需先升级为 ADR 讨论或向用户确认。V1 时期 ADR（ADR-001 ~ ADR-035）随 V1 归档于 [../Pawork_v1/docs/adr/](../Pawork_v1/docs/adr/)，其原则在本仓库继续有效；新决策仍以 ADR 记录（编号续接 V1）。

## 3. 命名与结构约定

- 项目名：`Pawork`；CLI 二进制名：`pawork`。
- `pawork`（apps/pawork）是 Core 的唯一正式宿主；不存在独立的 daemon / rpc 入口。
- 仓库根即 Cargo workspace 根（2026-08-17 已把原 `Pawork_v2/` 摊平，不再嵌套外层目录）。
- 包按功能域分目录：foundation（domain/api/protocol/config/sqlite/testkit/diagnostics）、engine、execution（exec/policy/tools）、providers（core/adapters/auth）、storage（session/blob）、host（app/cli/gui-server/transport/channels）、clients（gui-client/sdk/compat）、agents（orchestration）、control-plane（core/quota/provider-control）、workflow（core/memory/review）、net、vcs、extensions（mcp）、workspace（core/resources）；应用入口在 apps/（pawork、desktop、protocol-probe）；协议 schema 在 schemas/。
- crate 统一 `pawork-` 前缀（`pawork-domain`、`pawork-engine`……）；新增包须在 [docs/design.md](docs/design.md) §2 包布局登记并明确依赖方向。

## 4. 任务粒度

- 每个任务应在数小时内可独立完成、独立验收。
- 写入集尽量收敛到单一包或一组紧相关文件；不同任务不修改同一文件。
- 任何任务完成后，对应文档与 ROADMAP 状态须同步更新（状态回写约定见 [ROADMAP.md](ROADMAP.md) §6）。
- 任务开启 / 进行 / 收尾的公共规范见 [docs/task-guide.md](docs/task-guide.md)；阶段任务书见 [plan/](plan/)；阶段外任务登记见 [ROADMAP.md](ROADMAP.md) §3。

## 5. 验证决策（当前 S0–S13 路线）

S0–S11 的实现任务以 [docs/task-guide.md](docs/task-guide.md) §6 为准——少测试、无全量门禁：只做能证明本任务核心行为的关键定向测试（`cargo check -p <crate>` / `cargo test -p <crate>`，多包重复 `-p`，不因包多改用 `--workspace`）。三类关键测试不推迟：安全红线定向回归、持久化与重放契约 golden、协议与解析 golden/种子。

S12 是只读全项目 Code Review：按 [plan/S12-project-code-review.md](plan/S12-project-code-review.md) 审查和登记 finding，不修改生产代码，不运行测试、构建、格式化、fuzz、三平台矩阵或真实冒烟。Workspace Full Gate 与发布不在当前 S0–S13 排期；未来若获明确授权，须在 S13 整改收口后另立任务和门禁。

S13 是 S12 finding 整改阶段：按 [plan/S13-s12-remediation.md](plan/S13-s12-remediation.md) 分波执行（波 A 安全 → 波 B Bug → 波 C 收口），验证沿用 S0–S11 定向约定（三类关键测试不推迟、契约改动 golden 先行）；契约/红线级决策先 ADR 或用户确认；不设全量门禁、不发布。

沿用的硬约束：

- 禁止 `cargo clean`；复用默认 `target/` 增量缓存，仅清理本任务临时输出。
- 文档或不影响构建行为的配置改动只做链接、格式与 diff 检查，不为形式完整跑编译。
- 前一层失败先收敛原因，不盲目扩大范围。
- Secret、Policy、路径越界、持久化/重放、破坏性文件/进程操作等高风险改动必须带对应定向回归。

任务结束报告至少包含：

```text
Validated: <实际命令 / tests / checks，或 none 及理由>
Targeted regressions: <实际覆盖，或 none>
Full workspace gate: NOT RUN（当前 S0–S13 未设置全量门禁）
```

## 6. 文档约定

- 中文撰写，保留关键术语英文。
- 常设文档体系：[ROADMAP.md](ROADMAP.md)（任务总索引）· [plan/](plan/)（阶段任务书 S0–S12）· [docs/design.md](docs/design.md)（设计与冻结契约）· [docs/gui-design.md](docs/gui-design.md)（Desktop GUI 设计）· [docs/references.md](docs/references.md)（参照项目手册）· [docs/task-guide.md](docs/task-guide.md)（任务实现规范）· [docs/v1-migration-reference.md](docs/v1-migration-reference.md)（V1 迁移词典，冻结参考）。
- 架构决策用 ADR 记录，编号续接 V1（ADR-0xx），状态字段：Proposed / Accepted / Superseded。
- 交叉引用使用仓库内相对路径链接；指向已归档 V1 资产时用 `../Pawork_v1/...` 并注明归档。

## 7. 提交与分支

- 分支前缀默认 `codex/`，用户另有要求时遵从用户。
- 提交、推送、发布仅在用户请求或已确认任务链明确包含时执行。
- 不使用 `git reset --hard` / `git checkout --` 清理用户改动，除非用户明确要求。
- 优先非交互式 git 命令。

## 8. 安全与权限

- 不执行递归删除、覆盖 workspace 外路径、`$HOME` / 根目录等宽范围破坏性操作。
- 文件操作输入必须基于 `workspace_id + relative_path`，禁止模型直接传任意绝对路径。
- 子进程、网络、Secret 访问须经 Policy / Sandbox 约束：写入工具与审批见 [plan/S3-safe-edits.md](plan/S3-safe-edits.md)，命令执行与沙箱见 [plan/S4-exec-sandbox.md](plan/S4-exec-sandbox.md)；实现承载于 `pawork-policy` / `pawork-exec`。

## 9. 子代理使用

- 文档等一致性关键产物由主代理直接撰写。
- 实现阶段：边界清晰、写入集互不重叠的任务可并行派发，遵循服务级 `AGENTS.md` 的路由与并发上限。
- 确定性检查先于模型审查；每个门禁只调用一个审查者。

## 10. 验证命令模板

S0–S11 普通实现任务从以下命令选择最小有效子集；多个相关包追加 `-p`：

```bash
cargo check -p <crate>
cargo test -p <crate>
```

S12 不运行上述命令。未来发布任务的全量门禁必须在该任务书中重新定义，不沿用已移除的旧 S12 默认动作。
