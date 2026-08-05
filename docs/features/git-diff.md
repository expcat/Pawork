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

## 相关文档

- [checkpoint](checkpoint.md) · [process](process.md) · [api-surface（diff.*）](../architecture/api-surface.md)
- [ADR-007 系统 Git](../adr/ADR-007-system-git.md)
- [ROADMAP Phase 7](../../ROADMAP.md)
