# AGENTS.md — Pawork 工作指南

本文件是面向在 Pawork 仓库中工作的代理与人类协作者的工程约定。它与服务级 `AGENTS.md`（全局行为）叠加生效，冲突时以本文件为准。本版于 2026-08-25 随文档体系重构更新（包级 Spec 取代 code-map，历史存档收敛至 `docs/history.md`）；V1 版归档于仓库外同级目录 [../Pawork_v1/AGENTS.md](../Pawork_v1/AGENTS.md)。

## 1. 核心原则

- **事实源优先**：以当前分支、工作区差异、源码、生成物、运行日志与真实远程状态为准；历史结论只作检索线索，使用前重新验证。
- **Spec vs 源码**：[docs/spec/crates/](docs/spec/README.md) 各包 Spec 是理解包内功能的**首选读物**（目标：读文档即可了解该包全部功能，尽量少读代码），但**不是**事实源——公开 API 与行为以源码 / rustdoc / golden 为准；架构布局与冻结契约以 [docs/architecture.md](docs/architecture.md) 为准。冲突以源码为准并**同批回写 Spec**，禁止按过期 Spec 改代码。
- **按写入集加载 Spec**：进某包前读 [docs/spec/crates/](docs/spec/README.md) 该包一篇；不要一次读完 21 份。跨包链路（Agent loop / GUI 连接 / 事件持久化与重放 / 凭证与脱敏）读 [docs/spec/flows.md](docs/spec/flows.md) 对应一条。
- **最小写入集**：保留用户已有未提交改动；新增改动只触碰任务必需的文件。
- **先确认已落地的内容，再补缺口**：避免重复规划或重做已完成的工作。
- **范围明确的实现 / 修复任务，定位后直接执行**：不把简单任务过度规划。

## 2. 架构红线（不可违反）

- CLI 与 Core 同进程同二进制（`pawork` 是唯一正式宿主），纯 Rust 实现；不引入 Node / Bun / V8 / 嵌入式 JS Runtime。GUI 以独立 GPUI 进程（`apps/desktop`）经 GUI Connection Protocol 连接 CLI，不嵌入 Core；Desktop 构建链同样保持纯 Rust。
- `pawork-domain`（crates/domain）不得依赖任何 GUI framework（包括 GPUI/Tauri）、SQLite、HTTP Client、OS Keychain、Git、任何具体 Provider。
- 禁止包间循环依赖；包布局与依赖方向见 [docs/architecture.md](docs/architecture.md) §2。
- Agent Engine 不得通过判断 Provider 名称走特例逻辑（统一走 canonical domain）。
- Secret（明文 Token）不写入数据库与日志。
- 所有 Agent 事件必须可持久化、可重放。
- GUI 不得直接访问 Provider、数据库与工具；只能通过 GUI Connection Protocol 连接 CLI，经 CLI 宿主访问 Core（不直接加载 Core crate）。

违反以上任意一条需先升级为 ADR 讨论或向用户确认。V1 时期 ADR（ADR-001 ~ ADR-035）随 V1 归档于 [../Pawork_v1/docs/adr/](../Pawork_v1/docs/adr/)，其原则在本仓库继续有效；新决策仍以 ADR 记录（编号续接，现存 037–041，见 [docs/architecture.md](docs/architecture.md) §5）。

## 3. 命名与结构约定

- 项目名：`Pawork`；CLI 二进制名：`pawork`。
- `pawork`（apps/pawork）是 Core 的唯一正式宿主；不存在独立的 daemon / rpc 入口。
- 仓库根即 Cargo workspace 根。
- 当前布局为 21 成员（19 库 + 2 应用）：19 库平铺 `crates/<短名>`（目录 = 包名去 `pawork-` 前缀），2 应用 `apps/{pawork,desktop}`（ADR-039 定稿）；包清单与依赖方向见 [docs/architecture.md](docs/architecture.md) §2。
- crate 统一 `pawork-` 前缀（`pawork-domain`、`pawork-engine`……）。**当前不新增包**，只往既有包加模块；任何包布局变更须先过 ADR。
- 归档资产以 git tag `v2-final` 兜底，复活条件登记在 [docs/spec/backlog.md](docs/spec/backlog.md)；不得把归档代码复制回仓库其它位置。

## 4. 任务粒度

- 每个任务应在数小时内可独立完成、独立验收。
- 写入集尽量收敛到单一包或一组紧相关文件；不同任务不修改同一文件。
- 任何任务完成后，对应文档与 ROADMAP 状态须同步更新（回写约定见 [ROADMAP.md](ROADMAP.md) §6–§7）。
- 任务开启 / 进行 / 收尾的公共规范见 [ROADMAP.md](ROADMAP.md) §6–§7；进行中阶段任务书见 [plan/](plan/)；阶段外候选登记见 [docs/spec/backlog.md](docs/spec/backlog.md)。

## 5. 验证决策

实现任务以 [ROADMAP.md](ROADMAP.md) §6–§7 为准——少测试、无全量门禁：只做能证明本任务核心行为的关键定向测试。默认死表为 `cargo test -p <crate> --offline --lib --tests`（多包可一次多个 `-p`，但仍是一个 Cargo 进程，不因包多改用 `--workspace`）。`cargo check -p <crate>` 仅在该包无测试或只需类型检查时使用。三类关键测试不推迟：安全红线定向回归、持久化与重放契约 golden、协议与解析 golden/种子；邻包 golden/probe/e2e/desktop/`cargo check -p pawork` 默认不跑，仅主代理收口且对应文件确有改动时加跑一次。

补充约定：

- **每波收口主干可用**：`pawork` 二进制可编译、可运行、既有冒烟行为不回退；合并/归档波补跑 `cargo tree` 断言（无环、`-p pawork` 闭包不膨胀）。
- **冻结契约不静默破坏**（清单见 [docs/architecture.md](docs/architecture.md) §3.2）：golden 先于实现改动；schema/wire 演进须 ADR Accepted。
- **ADR 闸门**：破坏式改动须用户确认 ADR 后执行；主代理不代替用户拍板。
- 全量门禁与发布不在当前排期；未来若获明确授权，须另立任务和门禁。

沿用的硬约束：

- 禁止 `cargo clean`；复用默认 `target/` 增量缓存，仅清理本任务临时输出。stale incremental 用 `python3 scripts/clean-stale-incremental.py` 按前缀清理，禁止 `rm -rf target`。
- 全会话同一时刻只允许一个 Cargo 进程；并行轨不得抢同一 `target/` 锁。审查者读 worker `/tmp` 日志，不再编译。
- 文档或不影响构建行为的配置改动只做链接、格式与 diff 检查，不为形式完整跑编译。
- 前一层失败先收敛原因，不盲目扩大范围。
- Secret、Policy、路径越界、持久化/重放、破坏性文件/进程操作等高风险改动必须带对应定向回归。

任务结束报告至少包含：

```text
Validated: <实际命令 / tests / checks，或 none 及理由>
Targeted regressions: <实际覆盖，或 none>
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

## 6. 文档约定

- 中文撰写，保留关键术语英文。
- 常设文档体系（入口 [README.md](README.md)）：[ROADMAP.md](ROADMAP.md)（任务事实源：指针/阶段/开放决策/任务约定）· [plan/](plan/)（进行中阶段任务书）· [docs/architecture.md](docs/architecture.md)（架构事实源：红线/布局/冻结契约/ADR 索引）· [docs/design.md](docs/design.md)（功能设计事实源：功能映射/扩展功能族/候选池）· [docs/spec/README.md](docs/spec/README.md)（产品 Spec + 包级 Spec 总索引）· [docs/spec/backlog.md](docs/spec/backlog.md)（阶段外候选与复活条件）· [docs/spec/flows.md](docs/spec/flows.md)（跨包链路）· [docs/gui-design.md](docs/gui-design.md)（Desktop GUI 设计，配套 [design/](design/README.md) 视觉基准）· [docs/references.md](docs/references.md)（参照项目手册 + 调研附录）· [docs/history.md](docs/history.md)（历史存档）· [docs/adr/](docs/adr/)（架构决策记录）。
- **Spec 边界**：`docs/spec/` 产品篇是跨事实源的产品化汇总，包级 Spec 是包内功能的文档化镜像；均不替代源码/golden、`docs/architecture.md` 的布局与冻结契约、ROADMAP/任务书的状态事实源。用户可见能力、契约、安全、Desktop、验证或运维边界变化时，同批更新对应 Spec；「已实现」「已验证」「已人工验收」「已发布」必须分开表述。
- **包级 Spec 维护规则**：固定八节结构（见 [docs/spec/README.md](docs/spec/README.md)）。写入集改了模块树、对外 API、`pawork-*` 依赖边、feature 门、红线相关行为或测试资产时**同批**更新该包 `docs/spec/crates/<pkg>.md`；冲突以源码为准并回写。
- **历史存档纪律**：已完成阶段的收口细节、已关闭登记项、已拍板决策的过程记录进 [docs/history.md](docs/history.md)，不留在 ROADMAP/plan；存档只增不删。
- 架构决策用 ADR 记录，编号续接（ADR-0xx），状态字段：Proposed / Accepted / Superseded。
- 交叉引用使用仓库内相对路径链接；指向已归档 V1 资产时用 `../Pawork_v1/...` 并注明归档。

## 7. 提交与分支

- 分支前缀默认 `codex/`，用户另有要求时遵从用户。
- 提交、推送、发布仅在用户请求或已确认任务链明确包含时执行。
- 不使用 `git reset --hard` / `git checkout --` 清理用户改动，除非用户明确要求。
- 优先非交互式 git 命令。

## 8. 安全与权限

- 不执行递归删除、覆盖 workspace 外路径、`$HOME` / 根目录等宽范围破坏性操作。
- 文件操作输入必须基于 `workspace_id + relative_path`，禁止模型直接传任意绝对路径。
- 子进程、网络、Secret 访问须经 Policy / Sandbox 约束：红线与 S13 拍板见 [docs/architecture.md](docs/architecture.md) §1/§4；实现承载于 `pawork-policy` / `pawork-exec`（Spec：[policy](docs/spec/crates/policy.md) / [exec](docs/spec/crates/exec.md)）；凭证链路见 [docs/spec/flows.md](docs/spec/flows.md) §4。

## 9. 子代理使用

- 文档等一致性关键产物由主代理直接撰写。
- 实现阶段：边界清晰、写入集互不重叠的任务可并行派发，遵循服务级 `AGENTS.md` 的路由与并发上限。
- 派发实现 / 核查子代理时，提示词须点名写入集各包 `docs/spec/crates/<pkg>.md`（切片骨架见当前 [Settings 任务书](plan/settings.md) §3，后续任务书沿用同一字段）；不要让子代理先通读全部 Spec。
- 确定性检查先于模型审查；每个门禁只调用一个审查者。

## 10. 验证命令模板

普通实现任务默认只跑写入集 crate；多个相关包可一次追加 `-p`，但仍是一个 Cargo 进程：

```bash
cargo test -p <crate> --offline --lib --tests
```

仅在该包无测试或只需类型检查时改用 `cargo check -p <crate> --offline`。protocol golden、probe、spawn_e2e、desktop、`cargo check -p pawork` 默认不跑（probe/spawn_e2e/app smoke 已按 required-features 门控，默认死表不再编译，复跑命令见包级 Spec）。合并 / 归档波追加 `cargo tree` 断言（无环、`cargo tree -p pawork` 闭包对比）。未来发布任务的全量门禁必须在该任务书中重新定义，不沿用历史默认动作。
