# ADR-007：使用系统 Git

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

完全依赖 libgit2 难以完整覆盖 Worktree、LFS、Submodule、`.gitattributes`、用户配置、Credential Helper、textconv、rename detection 等行为。

## 决策

第一版优先调用系统 Git。Rust 负责参数构造、Process 监督、输出解析、timeout、cancel、缓存与安全策略。

## 后果

- 与用户本地 Git 行为一致，Worktree/LFS/Submodule 可靠。
- 产生子进程依赖，需 Process Runtime 管理与缓存。
- 须对系统 Git 版本差异做输出解析容错。

## 相关

- [git-diff](../features/git-diff.md) · [process](../features/process.md) · [ROADMAP Phase 7](../../ROADMAP.md)
