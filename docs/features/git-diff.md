# Git、Diff 与 Worktree

## 职责

封装系统 Git、产出结构化 Diff、管理 Worktree，供工具、Checkpoint 与 GUI 复用。

## Git 实现策略

第一版优先调用**系统 Git**而非完全依赖 libgit2，以保持 Worktree、LFS、Submodule、`.gitattributes`、用户配置、Credential Helper、textconv、rename detection 等行为一致。Rust 负责：参数构造；Process 监督；输出解析；timeout；cancel；缓存；安全策略。

## Git 功能优先级

- **P0**：Repository 检测；Branch；HEAD；status；changed files；staged/unstaged/untracked；unified diff；stage；unstage；discard；Worktree 创建删除；Git 错误归一化。
- **P1**：commit；branch create/delete；checkout；stash；log；show；merge-base；conflict 状态；hunk stage；line stage。
- **P2**：rebase 辅助；merge 辅助；PR 上下文；remote 操作；push；force push（独立高风险审批）。

## 结构化 Diff

```rust
pub struct DiffFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub status: FileStatus,
    pub staged: bool,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    pub hunks: Vec<DiffHunk>,
}

pub struct DiffHunk {
    pub id: HunkId,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
    pub lines: Vec<DiffLine>,
}
```

## Diff 功能

Unified Diff 解析；文件摘要；分页 Hunk；Context Lines；binary；rename；copied；deleted；added；CRLF；无末尾换行；Unicode 文件名；submodule；大文件保护；Ignore whitespace；word-level diff；缓存；内容指纹；Hunk stage；Hunk discard。

核心直接输出结构化文件和 Hunk，避免 pi-gui 那样先拿完整 diff 字符串再由前端拆分渲染。

## 验收标准

- rename / binary / untracked / submodule 测试通过
- Diff 可分页
- 100,000 行 Diff 解析 < 500ms
- 已缓存 Diff 切换 < 50ms
- Worktree 清理不删用户数据

## 实现状态（Phase 7 已交付 P7-1 ~ P7-8）

- `git-service` crate：系统 Git 封装（`GitRunner` 调用入口 + `GitError` 归一）、repo 检测 / branch / HEAD（`GitService`）、status / changed files（`StatusService`）、stage / unstage / discard（`StageService`，discard 标记高风险）、Worktree 创建/删除（`WorktreeService`，remove 先校验受管理、绝不递归删除用户数据）、status 缓存 + notify watcher 失效（`StatusCache` / `CachedStatusService`，命中纯内存读 < 50ms）。
- `diff-service` crate：结构化 Diff（`DiffFile`/`DiffHunk`/`DiffLine`），解析 `--raw -z` + `--numstat -z` 文件清单与 unified patch hunks，支持 rename/binary/无末尾换行、`paginate` 分页；unified 解析器为纯字符串状态机，100k 行 < 500ms。
- P1 已交付（P7-7 / P7-8）：commit 含 amend / allow-empty（`CommitService`）、branch 创建/删除/checkout（`BranchService`）、stash push/list/pop/apply/drop（`StashService`）、log / show / merge-base（`HistoryService`）、未合并路径与 merge 状态识别（`ConflictService`）；hunk / line 级暂存与取消暂存（`diff-service` 的 `HunkStageService`，基于结构化 Diff 生成精确 patch 并经 `git apply --cached` 应用，binary/rename/unmerged 等不可部分暂存的场景显式报错）。

## 相关文档

- [checkpoint](checkpoint.md) · [process](process.md) · [api-surface（diff.*）](../architecture/api-surface.md)
- [ADR-007 系统 Git](../adr/ADR-007-system-git.md)
- [ROADMAP Phase 7](../../ROADMAP.md)
