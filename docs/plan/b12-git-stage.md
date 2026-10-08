# B12 · Git Stage / HunkStage 产品面裁决

> 状态：✅ 完成。完成日期：2026-10-08。更新：2026-10-08。独立 review / 复审 PASS。未推送、未发布。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

在生产接线和归档之间裁决 Git 写操作。裁决是归档，不接线。

## 事实

基线 `main` / `44a8848ed21caeb02fa6318332f4f882df951e3e`。

全仓没有 `StageService` / `HunkStageService` 的生产调用。App [diff.rs](../../crates/app/src/diff.rs) 只用 `GitService`、`StatusService`、`DiffService`、`paginate` 和 diff 模型。orchestration 的默认关闭 feature `git` 只用 `GitRunner`、`WorktreeService`、`DiffService`。Desktop Changes 保持只读。

冻结 `AppCommand::GitStage` 自 API 1.0 存在。registry 中 GUI `available=false`、headless `None`、ACP `false`。[gui-server](../../crates/gui-server/src/session.rs) 的 `gui_channel_gate` 在 `available=false` 时于进 Host 前拒绝。App、CLI、ACP 没有处理分支。protocol 只把它当信封往返夹具。本任务不改这条冻结命令，也不开放它。

[BK-GIT-02](../spec/backlog.md) 仍是候选。激活需要双向 wire、Policy、审批/回滚和 Accepted ADR。归档不是禁止模型经现行 Policy 约束的 `run_command` 执行 git。

## 逐项裁决

| 对象 | 裁决 | 依据 |
| --- | --- | --- |
| `StageService` 及 `StageOp` / `StageRequest` / `StageRisk` | 删除 | 无生产调用。只服务 index 写入。 |
| `HunkStageService`、`build_hunk_patch`、`build_line_patch` | 删除 | 无生产调用。Desktop 未接线。 |
| `GitError::PatchDoesNotApply` | 删除 | 只由已删除的 stage / hunk 路径构造。`GitError` 现为 8 个 variant。 |
| 生产依赖 `tempfile`、`tracing` | 移出 | `tempfile` 只给 `apply_patch_to_index` 写临时 patch；`tracing` 只记录该临时文件删除失败。`tempfile` 留在 dev-dependencies。 |
| `serde_json` | 保留 | 源码无直接引用，但是既有依赖。本次不顺手删。 |
| `AppCommand::GitStage` | 不改 | 冻结契约。三通道仍不可用，不是实现。 |
| Desktop Changes | 保持只读 | 不从 UI 直接调用 Git。接线仍是 BK-GIT-02。 |
| 只读 diff / status、parser golden / proptest、`RepoInfo`、`WorktreeService` | 保留 | 有生产或默认关闭 feature 的真实消费者。 |

## 删除测试的无效性

删掉的测试只覆盖已退出 API，不能证明保留下来的只读面。包括 `stage.rs` 的 stage / unstage / discard / `stage_all`、空路径、`classify`、`apply_patch_to_index`、过期 index 的 `PatchDoesNotApply`、临时文件独占随机名与 0600、可预测 symlink 截获，以及名为 `--force` 的文件经 `--` stage。`hunk_stage.rs` 的 hunk / 行级暂存、rename 不支持、过期 diff、patch 纯函数与 no-newline 标记同样随 API 退出。

仍在的注入锚点是 `validate_position_arg`（`rejects_option_like_position_arguments`）、worktree 的 option 形 ref 拒绝（`option_like_worktree_refs_are_rejected`），以及 diff 的 option 形 commit range 拒绝（`option_like_commit_range_is_rejected`）。Git 写测试只用临时仓库。本任务没有新增测试。

## 恢复锚点

当前布局的恢复锚点是 `44a8848ed21caeb02fa6318332f4f882df951e3e` 的 `crates/git/src/stage.rs` 与 `crates/git/src/diff/hunk_stage.rs`。tag `v2-final`（`088b539bfa35e4466f8fe7c3550c730b74c551d4`）未包含当前目录布局：旧路径 `vcs/git/src/stage.rs` 与该基线逐字节相同，`vcs/git/src/diff/hunk_stage.rs` 仅有 import 迁移差异，不作为当前布局恢复点。引入提交只作历史：stage `5e7a72333725151e11fc464e00409ad92b78e90b`，hunk `1af699d88a33c380a1efa8e407548bc0981f3d2b`，都是 2026-08-06。不另存副本，禁止把归档源码拷回主干库存。

改前 `cargo tree -p pawork-git --edges normal --depth 1` 在 `/tmp/pawork-b12-git-stage/tree-git-before.txt`。当时直接依赖含 `tempfile` 和 `tracing`。

## 写入集

代码：删除 `crates/git/src/stage.rs`、`crates/git/src/diff/hunk_stage.rs`；改 `crates/git/src/lib.rs`、`crates/git/src/diff/mod.rs`、`crates/git/src/error.rs`、`crates/git/Cargo.toml`。`Cargo.lock` 里 pawork-git 去掉生产依赖 `tracing`；`tempfile` 仍因 dev-dependency 留在锁文件。`lib.rs` 写明恢复锚点。

文档：[git Spec](../spec/crates/git.md)、[architecture](../architecture.md)、[spec 索引](../spec/README.md)、[backlog](../spec/backlog.md)、[desktop crate Spec](../spec/crates/desktop.md)、[capabilities](../spec/capabilities.md)、[product](../spec/product.md)、本文件、[ROADMAP](../ROADMAP.md)。git Spec 数据流现为 1–5，已去掉 hunk 暂存那一步。`src/diff/parser.rs` 行数量级仍写 `~270`，实测 168 行；这是既有量级，本次不顺手改。

## `run_command` 边界

归档的是 typed stage / hunk stage。模型仍可经现行 Policy、审批和 Sandbox 约束的 `run_command` 执行 git。本任务不放宽、不收紧这条路径，也不把库内删除当成 GUI 写入已实现或已禁止。

## 验证

日志目录 `/tmp/pawork-b12-git-stage`。Cargo 串行，每次 Cargo 前确认没有其它 `cargo` 进程。沿用仓库 kache，不 `cargo clean`。

| 命令 | 结果 | 日志 |
| --- | --- | --- |
| `bash scripts/test.sh --print git` | `cargo test --offline --tests -p pawork-git` | 终端记录 |
| `bash scripts/test.sh --log /tmp/pawork-b12-git-stage/git.log git` | 35 passed，0 failed，exit 0，Elapsed 201s。lib 30，parser_contract 5。实际 Cargo：`cargo test --offline --tests -p pawork-git` | `git.log` |
| `cargo check -p pawork --offline` | exit 0，Finished in 2m 55s。仅既有 `block` future-incompat 提示 | `pawork-check.log` |
| `cargo tree -p pawork-git --edges normal --depth 1` | exit 0。直接依赖去掉 `tempfile` 与 `tracing`，其余与改前相同。这只对照 git 包直接依赖，不是 `-p pawork` 闭包。 | `tree-git-before.txt`、`tree-git-after.txt` |
| `cargo tree -p pawork --offline --edges normal` | exit 0，709 行，stderr 空。`(*)` 是已展示节点折叠，没有 cycle 诊断。`pawork-git` 的 normal 子边是 dunce、domain、exec、serde、serde_json、thiserror、tokio，不含 tempfile / tracing。整棵宿主 normal 树没有 tempfile；tracing 仍被其它既有包引用。生产 manifest / lock diff 只有这两条依赖缩减，没有新增包或边，闭包未膨胀。 | `tree-pawork.log`、`tree-pawork.err`、`tree-pawork.exit` |
| `bash scripts/mock/gate.sh --level 0` | exit 0。ROADMAP 与本次改动 Markdown 共 9 个文件，相对链接存在；同一次包含 `git diff --check HEAD`。 | `links-l0.log`、`links-l0.err`、`links-l0.exit` |
| `rustfmt --edition 2021 --config skip_children=true --check crates/git/src/lib.rs crates/git/src/error.rs crates/git/src/diff/mod.rs` | exit 0。只这 3 个本次修改的 Rust 文件。不宣称 `pawork-git` 其余文件或全仓格式全绿。 | `rustfmt-check.log`、`rustfmt-check.err`、`rustfmt-check.exit` |

未跑真实 Provider、GUI 或人工验收。自动回归不表示产品人工验收，也不表示 BK-GIT-02 已实现。交接文件 `final.diff` 与 `commands.md` 在日志目录，于本计划定稿后生成；`final.diff` 是当时工作区的 `git diff --binary`。

```text
Validated: bash scripts/test.sh --log /tmp/pawork-b12-git-stage/git.log git（实际 cargo test --offline --tests -p pawork-git，35 passed，exit 0，证据 git.log）；cargo check -p pawork --offline exit 0；cargo tree -p pawork --offline --edges normal exit 0（无环，闭包未膨胀）；bash scripts/mock/gate.sh --level 0 exit 0；rustfmt --edition 2021 --config skip_children=true --check crates/git/src/lib.rs crates/git/src/error.rs crates/git/src/diff/mod.rs exit 0；git diff --check exit 0
Targeted regressions: pawork-git lib 30（含 validate_position_arg、worktree option 形 ref、diff option 形 commit range）与 parser_contract 5（golden + proptest）
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

## 完成记录

2026-10-08 完成。按归档裁决删除无消费者的 Stage / HunkStage，并完成上表必要检查与交接证据；`gpt-6.1-sol / max` 独立 review 与增量复审均 PASS，未发现剩余问题。审查 P3 已修正并复核：`v2-final` 未包含当前目录布局，源码在旧路径 `vcs/git/src/`；当前布局恢复锚点仍是 `44a8848ed21caeb02fa6318332f4f882df951e3e`。

P3 仅修改注释和文档，未重跑已通过的 Git 35 项回归、Host check 与 cargo tree；定向注释格式、9 个 Markdown 链接和 diff 检查通过。指挥核对受审 `final.diff` 与工作区逐字节一致后，仅同步本文件和 ROADMAP 的完成元数据。本任务完成不表示 BK-GIT-02 已激活或 GUI 写入通过人工验收；冻结 `git_stage` 仍不可用，Desktop Changes 保持只读。未推送、未发布。
