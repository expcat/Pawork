# AGENTS.md — Pawork 工作指南

本文件是面向在 Pawork 仓库中工作的代理与人类协作者的工程约定。它与服务级 `AGENTS.md`（Codex 全局行为）叠加生效，冲突时以本文件为准。

## 1. 核心原则

- **事实源优先**：以当前分支、工作区差异、源码、生成物、运行日志与真实远程状态为准；历史结论只作检索线索，使用前重新验证。
- **最小写入集**：保留用户已有未提交改动；新增改动只触碰任务必需的文件。
- **先确认已落地的内容，再补缺口**：避免重复规划或重做已完成的工作。
- **范围明确的实现 / 修复任务，定位后直接执行**：不把简单任务过度规划。

## 2. 架构红线（不可违反）

- CLI 与 Core 同进程同二进制（`pawork` 是唯一正式宿主），纯 Rust 实现；不引入 Node / Bun / V8 / 嵌入式 JS Runtime。GUI 以独立 GPUI 进程经 GUI Connection Protocol 连接 CLI，不嵌入 Core；Desktop 构建链同样保持纯 Rust。
- `agent-domain` 不得依赖任何 GUI framework（包括 GPUI/Tauri）、SQLite、HTTP Client、OS Keychain、Git、任何具体 Provider。
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

## 5. 验证决策（默认最小有效验证）

普通任务默认停在 L0/L1；先按实际 diff 决定受影响 crate 和定向回归，再选择命令。带 `--workspace` 的 Cargo build/test/clippy 是例外性的 Workspace Full Gate，不是任务收尾模板。前一层失败时先收敛原因，不盲目扩大范围。

### 5.1 受影响范围

1. 只统计本任务实际 diff；在脏工作区中区分用户原有改动、本任务改动、生成物与未跟踪文件。
2. 用路径和 `cargo metadata --format-version 1 --no-deps` 的 `manifest_path` 把改动映射到 changed crates。根 `Cargo.toml`、`Cargo.lock`、toolchain、`.cargo/config*`、build script 与共享生成配置单独判断影响面，不能仅因它们位于 workspace 根就自动全量验证。
3. 判断改动是否触及 `pub` API、feature、shared/canonical domain、GUI Connection Protocol、持久化格式、schema 或共享测试夹具。需要查看反向依赖时用 `cargo tree --workspace --invert <crate> --depth 1` 或 `cargo metadata` 的 dependency graph。
4. crate 私有实现通常只选该 crate；公共接口加入能实际消费改动面的关键直接 reverse dependents。canonical domain / protocol 改动加入主要 producer、consumer、serializer/typegen 与 contract crate；只有证据表明一层不足时才继续扩大。
5. Secret、Policy、路径越界、持久化/重放、破坏性文件/进程操作、协议兼容等高风险改动必须加入对应定向 regression，但不因此自动加入无关 crate 或升级 Workspace Full Gate。

Provider、GUI 和平台模块遵循同一算法：单个 Provider 只验证其 adapter/runtime/对应 contract；GUI 只验证改动的 projection/controller/protocol 消费链与必要视觉或平台回归；平台代码只验证相关 target/harness。不得按模块类别直接扩大到整个 workspace。

### 5.2 命令选择

- 文档或不影响构建行为的配置改动可以只做链接、格式、schema/配置解析与 diff 检查，不为“形式完整”运行 Cargo 编译。
- `cargo check -p <crate>` 用于只需类型/feature 编译反馈的改动；`cargo test -p <crate>` 用于行为验证且已覆盖所需编译时，不再机械追加 `cargo build`。
- 只有实际需要验证 binary/link、build script、examples、特定 target/profile 或产物生成行为时才运行 `cargo build`。
- Rust lint、contract、golden、schema、regression 按改动选择；不要机械执行 `check + build + test + clippy`。
- 多个相关 crate 使用重复的 `-p`，例如 `cargo test -p <crate-a> -p <crate-b>`；crate 数量多不是改用 `--workspace` 的理由。

### 5.3 层级与 Full Gate 升级

- **L0**：存在性、链接、diff、生成物与规则一致性检查。
- **L1（普通任务默认）**：changed crates + 必要关键 reverse dependents + 定向 regression，复用默认 `target/`。
- **L2**：功能簇收尾时对相关 crates 做 integration/contract/golden/schema、定向 clippy/fmt；范围仍可由多个 `-p` 表达。功能簇确实覆盖大部分 workspace 时可明确批准 Workspace Full Gate。
- **L3**：Maintenance/Release Gate，包括 workspace 全量、跨平台、安全、性能、fuzz/chaos/差分等。

只有以下明确条件可运行 Workspace Full Gate：功能簇整体收尾或专门 Gate 任务；大规模跨 crate 重构；workspace/resolver/toolchain/关键依赖重大变化；canonical protocol/domain 的大范围变更且无法用关键消费者集合充分覆盖；发布/维护门禁；用户明确要求。普通公共 API 变更应先扩到主要消费者，不自动全量。

“保险”“最终确认”“确保没有回归”“改动较多”或“已经到任务末尾”都不是升级理由。升级前必须在进度说明中指出命中的具体条件；未命中时，`Full workspace gate: NOT RUN` 是正常完成状态。

L0/L1 禁止默认 `cargo clean`，继续复用默认 `target/` 增量缓存。仅清理本任务临时输出；隔离的 L2/L3 `CARGO_TARGET_DIR` 可在对应 Gate 结束后定向清理。完整规则见 [测试体系](docs/quality/testing.md)。

任务结束报告至少包含：

```text
Validation Level: L1
Affected crates: <changed + selected reverse dependents，或 none>
Validated: <实际命令 / tests / checks>
Targeted regressions: <实际覆盖，或 none>
Full workspace gate: NOT RUN (<未命中升级条件>)
```

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

## 10. 验证命令模板

普通任务从以下命令中选择最小有效子集；多个相关 crate 继续追加 `-p`：

```bash
cargo check -p <crate>
cargo test -p <crate>
cargo clippy -p <crate> --all-targets -- -D warnings
```

只有 §5.3 的升级条件成立时，才运行以下 **Workspace Full Gate（L2/L3 Maintenance/Release）**：

```bash
cargo build --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

本地任务中，`cargo fmt --all -- --check` 仅在需要检查 Rust 格式时运行，`cargo run -p schema-typegen -- --check` 仅在 schema/protocol/typegen 可能受影响时运行；手动三平台 L3 CI 作为固定 Maintenance/Release Gate 同时包含两项。基准入口为 `cargo bench -p pawork-benches`，按性能任务范围选择具体 benchmark。
