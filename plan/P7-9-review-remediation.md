# P7-9：Phase 7 评审修复（REVIEW remediation）

> Phase 7 · Git、Diff 与 Worktree · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P7-1 ~ P7-8

**最终目的**：消除 [REVIEW.md](../REVIEW.md) §7（Phase 7）评审发现的安全面、语义缺口与基线/文档漂移——让 hunk stage 临时文件不可预测、git 位置参数不被注入、`CacheScope::Staged` 不再是 API 谎言、watcher 不全量递归监听，并收敛 `similar` 零引用、diff-service 多余依赖与文档能力清单超前于实现。

**涉及范围**：`git-service`（stage/history/branch/commit/cache/worktree/process）、`diff-service`（service/hunk_stage/Cargo.toml）、根 `Cargo.toml`、ROADMAP「依赖选型基线」、`docs/features/git-diff.md`

## 细分步骤（分组）

### A. 安全（V1 / V2）

1. **V1 临时文件外泄面**：把 `tempfile` 提升为 runtime 依赖，`apply_patch_to_index` 的 patch 改用 `tempfile::NamedTempFile`（随机名、0600、独占创建），写完保留句柄传给 git、用后删除。目的：消除可预测路径在共享 temp 目录的符号链接竞争与源码外泄/污染面。
2. **V2 git 参数注入**：在服务边界统一拒绝前导 `-` 的 rev/range/branch/start_point/commit_range（或对允许语法白名单校验），覆盖 history/branch/diff 全部位置参数点，补注入回归测试。目的：模型/Agent 输出（受 prompt 注入影响）不可触发 git 选项注入。

### B. 语义正确性（V3 / V4）

3. **V3 CacheScope::Staged**：实现 staged-only 过滤（`git diff --cached` 视图），或移除/重命名 `Staged` 变体并在文档写明语义。目的：消除「Staged 与 Worktree 返回完全相同」的 API 谎言。
4. **V4 watcher 范围**：watch 前复用 ignore 规则过滤；`.git` 路径改由 `git rev-parse --git-dir`（repo.rs 已有 `git_dir`）解析后监听。目的：大仓库不全量递归监听，linked worktree 的 `.git` 变更也被捕获。

### C. 健壮性（V5 / V6 / V7）

5. **V5 StatusCache 无界**：接上已存储的 `computed_at` 做 TTL 失效或 LRU 容量上限。目的：多 worktree 切换下缓存不无界增长。
6. **V6 verbatim cwd**：git-service 出口对 cwd 应用 `dunce::simplified`（与 P1-13 V3 / P11-8 同根，本任务收口 git 子进程侧）。目的：Windows `\\?\` 路径不流入 git 子进程 cwd。
7. **V7 commit 判据**：收紧「nothing to commit」判据——仅在确认空暂存（如先 `diff --cached --quiet`）时归类，否则保留 stderr 上下文返回 `GitFailed`。目的：退出码 1 + 空 stderr 的真实失败不被误判。

### D. 文档与边界（V8 / V9 / V10）

8. **V8 git-diff.md 能力清单**：把 word-level diff / Ignore whitespace / Hunk discard / 内容指纹等未实现项标注「计划/未交付」或从清单移除。目的：文档承诺与实现一致（与 `similar` 未引用同源）。
9. **V9 no-newline 标志歧义**：拆分 `old_no_newline`/`new_no_newline`，或补对应回归测试锁定 `build_line_patch` 行为。目的：消除「旧侧删除行转 context 后标记 no-newline」语义偏离。
10. **V10 repo_info 合并调用**：用单条 `git rev-parse --show-toplevel --absolute-git-dir --is-bare-repository HEAD` + `symbolic-ref` 合并，减少进程 spawn。目的：降低 repo_info 的 3–4 次进程往返。

### E. 基线/包清理

11. **声明未引用/未登记**：从根 `Cargo.toml` 移除 `similar`（零引用）；回填 `parking_lot`/`tempfile`（Phase 7 引入未登记）；移除 `diff-service` 多余的 `serde_json`/`thiserror` 直接依赖。目的：基线一致、crate 依赖卫生。
12. **debouncer 归属**：与 P1-13 一致，ROADMAP 基线把 `notify-debouncer-full` 关联任务补 P7-6。目的：基线归属名副其实。

## 主要产出物

- stage patch 改 `NamedTempFile`；git 位置参数前导 `-` 校验 + 注入回归；CacheScope staged-only/重命名；watcher ignore 过滤 + git-dir 解析
- StatusCache TTL/LRU；verbatim cwd 收口；commit 判据收紧；git-diff.md 对齐；no-newline 标志拆分；repo_info 合并调用
- similar 移除；parking_lot/tempfile 回填；diff-service 多余依赖移除；debouncer 归属订正

## 验收标准（保留 REVIEW 追踪编号）

- [ ] **V1**：stage patch 用 `NamedTempFile`（随机名、独占），无可预测路径（符号链接竞争测试）
- [ ] **V2**：以 `-` 开头的 rev/range/branch/start_point 被拒绝（注入回归测试覆盖各位置参数点）
- [ ] **V3**：`CacheScope::Staged` 返回 staged-only 视图，或变体已移除/重命名并文档化
- [ ] **V4**：watcher 接 ignore 过滤；linked worktree 的 `.git` 变更被监听（用例）
- [ ] **V5**：StatusCache 有 TTL 或 LRU 上限（多 worktree 切换不无界增长，测试）
- [ ] **V6**：git 子进程 cwd 不含 `\\?\` 前缀（Windows 路径测试）
- [ ] **V7**：「退出码 1 + 空 stderr」的非空暂存失败不被误判 `NothingToCommit`（用例）
- [ ] **V8**：`git-diff.md` 未实现能力已标注/移除，与实现对齐
- [ ] **V9**：no-newline 标志语义明确（拆分或回归测试锁定）
- [ ] **V10**：`repo_info` 进程 spawn 次数减少（基准/审查）
- [ ] **基线**：`similar` 移除；`parking_lot`/`tempfile` 回填；diff-service `serde_json`/`thiserror` 移除；`notify-debouncer-full` 关联 P7-6
- [ ] **快速验证**：Git 参数、临时文件、watcher/cache 与路径风险立即跑定向回归；workspace 全量与三平台门禁延后到 Core 主干 L2/L3

**相关文档**：[REVIEW.md](../REVIEW.md) §7 · [ADR-007 系统 Git](../docs/adr/ADR-007-system-git.md) · [git-diff](../docs/features/git-diff.md) · [ROADMAP 依赖选型基线](../ROADMAP.md#依赖选型基线)

> 跨任务协调（2026-08 review）：V6 verbatim cwd 与 P1-13 V3、P11-8 同根，三任务各自收口出口（workspace-service / git 子进程 / sandbox）；基线 `similar`/`parking_lot`/`tempfile` 与 P1-13、P6-14 在根 `Cargo.toml` 与 ROADMAP 基线表上分行归属，序列执行不撞车。
