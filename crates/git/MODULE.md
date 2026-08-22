# pawork-git

系统 Git 封装与结构化 Diff。依赖 `pawork-domain`、`pawork-exec`。ADR-039 不合并清单成员。

## 职责

通过 `GitRunner`（底层 `pawork-exec::ProcessRuntime`）跑本机 `git`：仓库定位、status、unified diff 解析、index stage / hunk stage、worktree 增删。R0 已裁掉 GUI git 面板那一组服务。Merge 实现在 `pawork-orchestration::merge`（feature `git`），**不在本包**。

## 模块树

```
src/
  lib.rs  error.rs  process.rs  repo.rs  stage.rs  status.rs  worktree.rs
  diff/{mod,model,parser,service,hunk_stage}.rs
tests/
  parser_contract.rs
  golden/{basic_hunk,no_newline,context_no_newline}.diff
```

无 `branch.rs` / `stash.rs` / `conflict.rs` / `history.rs` / `commit.rs` / `merge.rs`。

## 对外入口/API 面

`pub mod diff` / `error` / `process` / `repo` / `stage` / `status` / `worktree`，crate 根再导出常用类型。

- `GitRunner`：`run` / `run_with_stderr`；`validate_position_arg` 拒绝以 `-` 开头的位置参数。
- `GitService::open`、`Head::{Branch, Detached, Unborn}`、`RepoInfo`。
- `StatusService` / `read_status`（porcelain v1 `-z`）、`FileStatus`。
- `DiffService`、`DiffFile` / `DiffHunk` / `HunkId`、`paginate`；`HunkStageService`（`git apply --cached`）。
- `StageService`：`Stage` / `Unstage` / `Discard`（Dangerous）/ `StageAll`。
- `WorktreeService::{list, add, remove, prune}`；`remove` 先 list，**不用**递归 `std::fs` 删除。

`GitError` 仍留 `BranchAlreadyExists` 等变体，但对应服务已归档。crate 根 `FileStatus` 是 `status::FileStatus`；diff 另有同名类型。

## 依赖与被依赖

- **依赖**：`pawork-domain`、`pawork-exec`。无 feature。
- **被依赖**：`pawork-app`（始终）；`pawork-orchestration` 的 optional feature `git`（当前 **没有任何** workspace 成员打开该 feature，含 `pawork-app` 以 `default-features = false` 依赖 orchestration）。

## 红线与注意事项

- 归档不存在：Branch / Stash / Conflict / History / CachedStatus / CommitService（ADR-038 D16）。复活见 ROADMAP §3.3「GUI git 面板」，tag `v2-final`。
- 注入防护：`validate_position_arg` 等随迁；不要把用户字符串当 extra git 开关。
- `design.md` §2 备注写「worktree/merge」——merge 的现行代码在 orchestration，本包只有 worktree。
- Desktop 不得直接依赖本包。

## 相关文档

- [docs/design.md](../../docs/design.md) §2 / §4 S8
- [ROADMAP.md](../../ROADMAP.md) §3.3
- [代码地图总索引](../../docs/code-map/README.md)
