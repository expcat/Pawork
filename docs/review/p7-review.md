# Phase 7 Review：Git、Diff 与 Worktree

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（HEAD）；工作树含用户未提交的 docs/ROADMAP/plan 改动与本评审产物，均不影响 Phase 7 代码结论
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 7 的 8 个任务（P7-1 ~ P7-8，主题「Git、Diff 与 Worktree」）的完成情况、所引入包是否合适、是否存在更优替代或自实现替换的必要；另含「优先级 P1」标签任务（P7-7/P7-8）现状。安全漏洞与优化点一并列出。

### 1. 结论摘要

1. **完成度可信**：P7-1 ~ P7-8 全部 🟢。2026-08-08 复跑 `git-service`（51 项）+ `diff-service`（21 项）共 **72 项测试全部通过**（均为真实 git 仓库集成测试）；`cargo clippy --all-targets -- -D warnings` 与 `cargo fmt --check`（两 crate）干净。
2. **包选型总体合理**：实际引用面落在 `notify` / `notify-debouncer-full`（P7-6 watcher）、`parking_lot`（缓存锁）、`tempfile`（测试 + stage patch 临时文件）上，使用面都覆盖其核心价值，**不建议自实现替换**。但「直接采用」表把 `notify-debouncer-full` 归到 P1-8，真实使用者是 P7-6 的 git 缓存失效器，归属应补 P7-6。
3. **`similar` 仍为「声明未引用」**：P7-3 与基线原计划用 `similar` 做 word-level diff，但 `diff-service` 实际解析 git 结构化输出（`--raw`/`--numstat`/unified patch），全仓库零真实引用（仅 [parser.rs:8](../../crates/diff-service/src/parser.rs) 注释出现 `similarity` 字样）。`docs/features/git-diff.md:44` 却把「word-level diff / Ignore whitespace / Hunk discard / 内容指纹」列为能力——文档承诺与实现存在缺口。
4. **基线偏差**：`similar`（声明未引用）、`parking_lot`/`tempfile`（Phase 7 引入但未回填 workspace 基线，REVIEW.md §4 已点名，仍未处理）持续存在；**新增** `diff-service` 把 `serde_json`、`thiserror` 声明为直接依赖却零使用。
5. **两个应优先处理的安全点**：(a) `apply_patch_to_index` 用可预测路径（`pawork-hunk-stage-{pid}-{counter}.patch`）在系统 temp 目录写 patch，多用户/共享主机下存在符号链接竞争与源码外泄面；(b) `history`/`branch` 的 `rev`/`range`/`name`/`start_point` 作为位置参数直传 git，未防「以 `-` 开头」的选项注入。
6. **一个语义缺口**：`CacheScope::Staged` 未实现——`refresh` 用 `let _ = scope;` 忽略 scope，`Staged` 实际返回与 `Worktree` 完全相同的全量视图，API 具误导性。

### 2. P7 任务完成情况核对表

| 任务 | 交付模块 | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P7-1 Repo 检测 / branch / HEAD | `git-service::repo` + `process` | 🟢 | [repo.rs](../../crates/git-service/src/repo.rs)：`open`/`current_head`/`repo_info`；错误归一 [error.rs:45-58](../../crates/git-service/src/error.rs)；非仓库 → `NotARepository` |
| P7-2 status / changed files | `git-service::status` | 🟢 | [status.rs:84-96](../../crates/git-service/src/status.rs)：`--porcelain=v1 -z`，解析 rename `previous_path`；`changed_files` 剔除未跟踪 |
| P7-3 结构化 Diff | `diff-service` | 🟢 | [service.rs](../../crates/diff-service/src/service.rs) `diff_summary`/`diff`；rename/binary/无末尾换行测试；100k 行解析基准 < 500ms（[parser.rs:214-231](../../crates/diff-service/src/parser.rs)） |
| P7-4 stage / unstage / discard | `git-service::stage` | 🟢 | [stage.rs:65-108](../../crates/git-service/src/stage.rs)；discard 标 `Dangerous` 供审批 |
| P7-5 Worktree | `git-service::worktree` | 🟢 | [worktree.rs](../../crates/git-service/src/worktree.rs)；`remove` 先 `list()` 校验受管理，删除只交 `git worktree remove`，**绝不** `std::fs` 递归删——红线遵守 |
| P7-6 Git 缓存 / watcher | `git-service::cache` | 🟢 | [cache.rs](../../crates/git-service/src/cache.rs)：`StatusCache`(parking_lot RwLock) + `CachedStatusService` + notify-debouncer 失效；缓存命中 1000 次 < 50ms 测试 |
| P7-7 Hunk / Line stage（P1） | `diff-service::hunk_stage` | 🟢 | [hunk_stage.rs](../../crates/diff-service/src/hunk_stage.rs)：`build_hunk_patch`/`build_line_patch` + `git apply --cached [--reverse]`，hunk/line 级 stage/unstage 真实 git 测试 |
| P7-8 commit/branch/stash/log/show（P1） | `git-service::{commit,branch,stash,history,conflict}` | 🟢 | 51 项真实 git 测试含 conflict/merge-base/未合并检测；plan 验收勾选 |

**门禁证据（2026-08-08 复核）**：

- `cargo test -p git-service -p diff-service`：**git-service 51 passed / diff-service 21 passed / 0 failed**（含真实 git 仓库集成与 100k 行 diff 基准）。
- `cargo clippy -p git-service -p diff-service --all-targets -- -D warnings`：干净。
- `cargo fmt -p git-service -p diff-service -- --check`：干净。
- 各 plan 验收项：P7-7/P7-8 已勾选；P7-1~P7-6 验收点（解析稳定、不删用户数据、切换 < 50ms、100k 行 < 500ms）均有对应测试。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本 | 使用点 | 使用面评估 | 结论 |
| --- | --- | --- | --- | --- |
| `notify` | 7 | P7-6 [cache.rs:151-173](../../crates/git-service/src/cache.rs) | 跨平台文件监听，缓存失效核心；自实现 ReadDirectoryChangesW/inotify/FSEvents 成本高 | **保留** |
| `notify-debouncer-full` | 0.5 | P7-6 [cache.rs:137-162](../../crates/git-service/src/cache.rs) | 300ms 去抖 + RecommendedCache 事件合并，大 checkout 事件风暴下显著降噪 | **保留**；建议把基线归属补 P7-6 |
| `parking_lot` | 0.12 | P7-6 [cache.rs:45](../../crates/git-service/src/cache.rs) `RwLock<HashMap>` | 无毒化锁，命中路径纯内存读，满足 < 50ms | **保留**；但需回填 workspace 基线（见 §4） |
| `tempfile` | 3 | P7-3/4/5/7/8 测试 + P7-4 stage patch 临时文件 | 真实 git 隔离仓库与确定性内容断言基座 | **保留**；dev 已用，runtime 见 V1 |
| `serde` / `thiserror` / `tokio` / `tracing` | 基线 | 全局 | 基础设施，无争议 | **保留** |

#### 3.2 需要重新评估的项

| 项 | 现状 | 选项 | 建议 |
| --- | --- | --- | --- |
| `similar` | 基线声明（[Cargo.toml:127](../../Cargo.toml)，P7-3）但全仓库**零真实引用**，仅 [parser.rs:8](../../crates/diff-service/src/parser.rs) 注释；word-level diff 计划未落地 | a) 移出基线，关闭文档承诺；b) 实现 word-level diff 再保留 | **建议 a**。diff-service 已用 git 结构化输出完成 hunk/line 暂存；进程内 word diff 无强需求出现前不引入（与 REVIEW.md §3.2 结论一致） |
| `serde_json`（diff-service 直接依赖） | [diff-service/Cargo.toml](../../crates/diff-service/Cargo.toml) 声明但**零使用**（模型仅 `#[derive(serde::Serialize/Deserialize)]`，无 `serde_json::` 调用） | 移除该 crate 的直接依赖 | **移除** |
| `thiserror`（diff-service 直接依赖） | 同上声明但**零使用**（diff-service 复用 `git_service::GitError`，未自定义错误类型） | 移除该 crate 的直接依赖 | **移除** |
| `notify-debouncer-full` 归属 | 基线记 P1-8；file-index 实际自实现去抖，真实使用者是 P7-6 git 缓存 | 在基线「关联任务」补 P7-6 | **订正基线描述**（低优先） |

#### 3.3 「自实现替换包」总体判断

P7 范围内**没有命中「应自实现替换」的包**：notify/debouncer/parking_lot/tempfile 的使用面都覆盖核心价值，自实现无收益。反向看，唯一「只用一小部分/零引用」的是 `similar`（应移出基线）与 diff-service 多余的 `serde_json`/`thiserror`（应删除）。当前自实现部分（porcelain/raw/numstat/unified 解析状态机、hunk/line patch 构造、缓存与去抖失效编排）质量高、边界正确，无需替换为第三方。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填基线表。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 声明未引用 | `similar` | [Cargo.toml:127](../../Cargo.toml) | P7-3 计划项，word-level diff 未实现，零引用；见 §3.2 |
| 引入未登记 | `parking_lot = "0.12"` | [git-service/Cargo.toml:22](../../crates/git-service/Cargo.toml) | Phase 7 引入（REVIEW.md §4 已点名，未处理） |
| 引入未登记 | `tempfile = "3"` | [git-service/Cargo.toml:26](../../crates/git-service/Cargo.toml)（dev）、[diff-service/Cargo.toml:20](../../crates/diff-service/Cargo.toml)（dev） | dev 依赖也应登记 |
| crate 多余直接依赖 | `serde_json`、`thiserror` | [diff-service/Cargo.toml](../../crates/diff-service/Cargo.toml) | 零使用，应从该 crate 移除 |
| 基线归属 | `notify-debouncer-full` | [Cargo.toml:108](../../Cargo.toml) | 基线记 P1-8，真实首用为 P7-6，建议补关联任务 |

**建议**：并入 REVIEW.md §4 的「基线清理小任务」一次性处理（删除 `similar`、回填 `parking_lot`/`tempfile`、移除 diff-service 两个多余依赖、订正 debouncer 归属）。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V10）。

#### V1 [安全·中] hunk stage 用可预测临时文件写 patch（符号链接竞争 / 源码外泄面）

[stage.rs:133-143](../../crates/git-service/src/stage.rs) 把 patch 写入 [stage.rs:175-183](../../crates/git-service/src/stage.rs) 生成的 `std::env::temp_dir().join("pawork-hunk-stage-{pid}-{counter}.patch")`：路径名可猜（pid 可观测、counter 从 0 起），位于共享系统 temp 目录。多用户主机或共享 CI 上，攻击者可预先在该路径建符号链接 → `std::fs::write` 跟随符号链接，把 patch 内容（即模型选中的源码片段）写到攻击者指定位置（外泄），或在 write 与 `git apply` 读取之间替换为攻击者 patch（污染暂存）。**建议**：把 `tempfile` 提升为 runtime 依赖，用 `tempfile::NamedTempFile`（随机名、0600、独占创建）；写完保留句柄传给 git，用后删除。

#### V2 [安全·中] git 参数注入（位置参数未防前导 `-`）

`history`/`branch`/`diff` 把 `rev`/`range`/`name`/`start_point`/`a`/`b`/`commit_range` 作为位置参数直传 git，未校验前导 `-`：
- [history.rs:87-89](../../crates/git-service/src/history.rs)（`range`）、[history.rs:109-123](../../crates/git-service/src/history.rs)（`show` 的 `rev`）、[history.rs:153-161](../../crates/git-service/src/history.rs)（`merge_base` 的 `a`/`b`）
- [branch.rs:41-49](../../crates/git-service/src/branch.rs)（`create` 的 `name`/`start_point`）、[branch.rs:101-104](../../crates/git-service/src/branch.rs)（`checkout` 的 `name`）、[branch.rs:130-136](../../crates/git-service/src/branch.rs)（`checkout_new`）
- [service.rs:166-169](../../crates/diff-service/src/service.rs)（`commit_range`）

以 `-` 开头的值会被 git 解释为选项（历史上有 `--upload-pack`/`-c core.xxx`/`--output` 等参数注入 CVE 类）。这些值最终来自模型/Agent 输出（处理不可信内容时受 prompt 注入影响），并非用户终端手敲。对比之下 `stage`/`stash` 的 `paths` 与 `run_file_patch` 的 `path` 已正确用 `--` 分隔（[stage.rs:73](../../crates/git-service/src/stage.rs)、[stash.rs:64-66](../../crates/git-service/src/stash.rs)、[service.rs:161](../../crates/diff-service/src/service.rs)）。**建议**：在服务边界统一拒绝前导 `-` 的 rev/range/branch/start_point（或对允许的语法白名单校验），并补注入回归测试。

#### V3 [正确性·中] `CacheScope::Staged` 语义未实现

[cache.rs:118-131](../../crates/git-service/src/cache.rs) 的 `refresh` 用 `let _ = scope;` 显式忽略 `scope`，注释自承「当前 status 解析器统一返回 staged+unstaged+untracked 视角；scope 仅影响缓存槽位区分」。结果 `CacheScope::Staged` 与 `CacheScope::Worktree` 返回**完全相同**的全量视图，调用方若按字面理解为「仅暂存区」会被误导。**建议**：要么实现 staged-only 过滤（`git diff --cached` 视图），要么把 `Staged` 变体移除/重命名并在文档写明语义，避免 API 谎言。

#### V4 [性能/正确性·中] watcher 全量递归监听 + `.git` 路径假设

[cache.rs:165-173](../../crates/git-service/src/cache.rs) 对 `work_dir` 递归监听（`RecursiveMode::Recursive`）并附加 `work_dir.join(".git")`。两点问题：(1) 大仓库（`node_modules`/构建产物）下递归监听事件量极大，即便 debouncer 去抖仍是高开销，且**未像 file-index 那样接 ignore 规则**过滤；(2) `work_dir.join(".git")` 假设标准布局，linked worktree（git dir 在主仓 `.git/worktrees/<name>`）或 gitfile 布局下该路径不存在/是文件，`.git` 内部变更监听失效（缓存可能不及时）。**建议**：watch 前复用 ignore 过滤；`.git` 路径改由 `git rev-parse --git-dir`（repo.rs 已有 `git_dir`）解析后监听。

#### V5 [健壮性·低] StatusCache 无界增长 + 死字段

[cache.rs:38-40](../../crates/git-service/src/cache.rs) 的 `computed_at: Instant` 被 `#[allow(dead_code)]` 标记、从未读取；`invalidate` 仅按 work_dir 删除、无 TTL/容量上限。长生命周期进程在多 worktree 间切换时，缓存条目只增不减。**建议**：接上 `computed_at` 做 TTL 失效或 LRU 容量上限。

#### V6 [健壮性·低] Windows verbatim 路径流入 git 子进程 cwd

`git-service`/`diff-service` 均未依赖 `dunce`；[process.rs:76-77](../../crates/git-service/src/process.rs) 直接把 `cwd` 设为调用方传入路径，[worktree.rs:41-43](../../crates/git-service/src/worktree.rs) 的 `canon` 用 `std::fs::canonicalize`（Windows 产出 `\\?\` 前缀）。当 cwd 来自 `workspace-service` 的 canonicalize 根（REVIEW.md V3）时，verbatim 路径流入 git 子进程，部分 git 版本对 `\\?\` 路径 cwd 处理不佳。属跨阶段问题，归并 P11-8 统一在出口 `dunce::simplified`。

#### V7 [健壮性·低] commit「nothing to commit」判据过宽

[commit.rs:64-71](../../crates/git-service/src/commit.rs) 在「stderr 不含 nothing to commit」时，以 `code == Some(1) && stderr.trim().is_empty()` 兜底归一为 `NothingToCommit`。git 其他「退出码 1 + 空 stderr」的失败会被误判为无可提交，掩盖真实错误。**建议**：收紧为仅在确认空暂存（如先 `diff --cached --quiet`）时归类，或保留 stderr 上下文返回 `GitFailed`。

#### V8 [文档/实现·低] docs/features/git-diff.md 能力清单超前于实现

[docs/features/git-diff.md:44](../features/git-diff.md) 列出「word-level diff / Ignore whitespace / Hunk discard / 内容指纹」等能力，但 P7-3 实现仅做 unified diff 解析 + hunk/line stage（无 word-level、无 ignore-whitespace 选项、无 hunk discard、无内容指纹）。与 `similar` 声明未引用（§4）同源。**建议**：把未实现项标注为「计划/未交付」，或从能力清单移除，避免误导下游任务。

#### V9 [健壮性·低] build_line_patch 复用 `new_no_newline` 的语义歧义

[hunk_stage.rs:201-206](../../crates/diff-service/src/hunk_stage.rs) 把未选中的 `Deletion` 转 context 时复用 `push_line`，其依据 `line.new_no_newline`（[hunk_stage.rs:249-256](../../crates/diff-service/src/hunk_stage.rs)）决定是否输出 `\ No newline`。但该标志在 [parser.rs:59-99](../../crates/diff-service/src/parser.rs) 中对「旧行无末尾换行」与「新行无末尾换行」均置 true，命名 `new_no_newline` 有歧义；旧侧删除行转 context 后再标记 no-newline 语义偏离。属边界情形，目前测试未覆盖。**建议**：拆分 `old_no_newline`/`new_no_newline`，或补对应回归测试锁定行为。

#### V10 [性能·低] repo_info 串行多次 git 调用

[repo.rs:155-164](../../crates/git-service/src/repo.rs) 的 `repo_info` 顺序触发 `current_head`（最多 2 次）+ `is_bare` + `git_dir`，共 3-4 次进程 spawn。**建议**：可用单条 `git rev-parse --show-toplevel --absolute-git-dir --is-bare-repository HEAD` + `symbolic-ref` 合并，减少往返。

### 6. 优化建议（按优先级）

#### P0（建议尽早处理）

1. **V1**：stage patch 临时文件改用 `tempfile::NamedTempFile`（安全红线相关，多用户/CI 场景有外泄面）。
2. **V3**：`CacheScope::Staged` 要么实现 staged-only 视图，要么删除/重命名（低成本消除 API 谎言）。

#### P1（近期排期）

3. **V2**：服务边界统一校验/拒绝前导 `-` 的 rev/range/branch/start_point，补注入回归测试。
4. **V4**：watcher 接 ignore 过滤 + 用 `git rev-parse --git-dir` 解析 `.git`（修正 linked worktree 监听）。
5. **基线清理**：§4 清单并入 REVIEW.md §4 的统一小任务——删 `similar`、回填 `parking_lot`/`tempfile`、移除 diff-service 多余 `serde_json`/`thiserror`、订正 `notify-debouncer-full` 归属。
6. **V8**：对齐 `docs/features/git-diff.md` 能力清单与实现。

#### P2（顺手/评估项）

7. **V5**：StatusCache 接 TTL/LRU（复用已存储的 `computed_at`）。
8. **V6**：Windows verbatim cwd 归并 P11-8 统一 `dunce::simplified`。
9. **V7**：收紧 commit「nothing to commit」判据。
10. **V9**：拆分 no-newline 标志或补回归测试。
11. **V10**：`repo_info` 合并 git 调用减少进程 spawn。

### 7. 附录：「优先级 P1」标签任务与跨阶段依赖

| 任务 | 状态 | 说明 |
| --- | --- | --- |
| P7-7 Hunk/Line stage（P1） | 🟢 已交付 | 21 项 diff-service 测试覆盖；rename/copy/typechange/unmerged/binary 已显式拒绝并回退整文件 stage（[hunk_stage.rs:133-152](../../crates/diff-service/src/hunk_stage.rs)） |
| P7-8 commit/branch/...（P1） | 🟢 已交付 | commit/branch/stash/log/show/merge-base/conflict 真实 git 测试齐全 |
| 与 REVIEW.md 的承接 | — | REVIEW.md §3.2/§4 点名的 `similar` 零引用、`parking_lot`/`tempfile` 未登记在 P7 依然成立，本评审补充 diff-service 多余依赖与 P7-6 debouncer 归属；REVIEW.md V3（verbatim cwd）在 P7 范围内仍未消除（见 V6） |

### 8. 建议的后续动作（本次未执行，供研究）

1. 对 V1/V2 立项（安全优先；V1 涉及源码外泄，V2 涉及 git 参数注入，二者均可能被 prompt 注入触发）。
2. 基线清理小任务（§4，并入 REVIEW.md §4 一次性提交）。
3. `CacheScope` 语义决策（实现 staged-only 或删变体）。
4. watcher ignore 过滤 + linked worktree `.git` 解析方案（影响 P7-6 大仓库体验）。
5. `docs/features/git-diff.md` 能力清单与实现对齐。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 `git-service`/`diff-service` 的测试与静态门禁；文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更。*

---

## 修复记录（review-remediation）

> Phase 7 · Git、Diff 与 Worktree · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P7-1 ~ P7-8

**最终目的**：消除 [REVIEW.md](../../REVIEW.md) §7（Phase 7）评审发现的安全面、语义缺口与基线/文档漂移——让 hunk stage 临时文件不可预测、git 位置参数不被注入、`CacheScope::Staged` 不再是 API 谎言、watcher 不全量递归监听，并收敛 `similar` 零引用、diff-service 多余依赖与文档能力清单超前于实现。

**涉及范围**：`git-service`（stage/history/branch/commit/cache/worktree/process）、`diff-service`（service/hunk_stage/Cargo.toml）、根 `Cargo.toml`、ROADMAP「依赖选型基线」、`docs/features/git-diff.md`

### 细分步骤（分组）

#### A. 安全（V1 / V2）

1. **V1 临时文件外泄面**：把 `tempfile` 提升为 runtime 依赖，`apply_patch_to_index` 的 patch 改用 `tempfile::NamedTempFile`（随机名、0600、独占创建），写完保留句柄传给 git、用后删除。目的：消除可预测路径在共享 temp 目录的符号链接竞争与源码外泄/污染面。
2. **V2 git 参数注入**：在服务边界统一拒绝前导 `-` 的 rev/range/branch/start_point/commit_range（或对允许语法白名单校验），覆盖 history/branch/diff 全部位置参数点，补注入回归测试。目的：模型/Agent 输出（受 prompt 注入影响）不可触发 git 选项注入。

#### B. 语义正确性（V3 / V4）

3. **V3 CacheScope::Staged**：实现 staged-only 过滤（`git diff --cached` 视图），或移除/重命名 `Staged` 变体并在文档写明语义。目的：消除「Staged 与 Worktree 返回完全相同」的 API 谎言。
4. **V4 watcher 范围**：watch 前复用 ignore 规则过滤；`.git` 路径改由 `git rev-parse --git-dir`（repo.rs 已有 `git_dir`）解析后监听。目的：大仓库不全量递归监听，linked worktree 的 `.git` 变更也被捕获。

#### C. 健壮性（V5 / V6 / V7）

5. **V5 StatusCache 无界**：接上已存储的 `computed_at` 做 TTL 失效或 LRU 容量上限。目的：多 worktree 切换下缓存不无界增长。
6. **V6 verbatim cwd**：git-service 出口对 cwd 应用 `dunce::simplified`（与 P1-13 V3 / P11-8 同根，本任务收口 git 子进程侧）。目的：Windows `\\?\` 路径不流入 git 子进程 cwd。
7. **V7 commit 判据**：收紧「nothing to commit」判据——仅在确认空暂存（如先 `diff --cached --quiet`）时归类，否则保留 stderr 上下文返回 `GitFailed`。目的：退出码 1 + 空 stderr 的真实失败不被误判。

#### D. 文档与边界（V8 / V9 / V10）

8. **V8 git-diff.md 能力清单**：把 word-level diff / Ignore whitespace / Hunk discard / 内容指纹等未实现项标注「计划/未交付」或从清单移除。目的：文档承诺与实现一致（与 `similar` 未引用同源）。
9. **V9 no-newline 标志歧义**：拆分 `old_no_newline`/`new_no_newline`，或补对应回归测试锁定 `build_line_patch` 行为。目的：消除「旧侧删除行转 context 后标记 no-newline」语义偏离。
10. **V10 repo_info 合并调用**：用单条 `git rev-parse --show-toplevel --absolute-git-dir --is-bare-repository HEAD` + `symbolic-ref` 合并，减少进程 spawn。目的：降低 repo_info 的 3–4 次进程往返。

#### E. 基线/包清理

11. **声明未引用/未登记**：从根 `Cargo.toml` 移除 `similar`（零引用）；回填 `parking_lot`/`tempfile`（Phase 7 引入未登记）；移除 `diff-service` 多余的 `serde_json`/`thiserror` 直接依赖。目的：基线一致、crate 依赖卫生。
12. **debouncer 归属**：与 P1-13 一致，ROADMAP 基线把 `notify-debouncer-full` 关联任务补 P7-6。目的：基线归属名副其实。

### 主要产出物

- stage patch 改 `NamedTempFile`；git 位置参数前导 `-` 校验 + 注入回归；CacheScope staged-only/重命名；watcher ignore 过滤 + git-dir 解析
- StatusCache TTL/LRU；verbatim cwd 收口；commit 判据收紧；git-diff.md 对齐；no-newline 标志拆分；repo_info 合并调用
- similar 移除；parking_lot/tempfile 回填；diff-service 多余依赖移除；debouncer 归属订正

### 验收标准（保留 REVIEW 追踪编号）

- [x] **V1**：stage patch 用 `NamedTempFile`（随机名、独占），无可预测路径（符号链接竞争测试）
- [x] **V2**：以 `-` 开头的 rev/range/branch/start_point 被拒绝（注入回归测试覆盖各位置参数点）
- [x] **V3**：`CacheScope::Staged` 返回 staged-only 视图，或变体已移除/重命名并文档化
- [x] **V4**：watcher 接 ignore 过滤；linked worktree 的 `.git` 变更被监听（用例）
- [x] **V5**：StatusCache 有 TTL 或 LRU 上限（多 worktree 切换不无界增长，测试）
- [x] **V6**：git 子进程 cwd 不含 `\\?\` 前缀（Windows 路径测试）
- [x] **V7**：「退出码 1 + 空 stderr」的非空暂存失败不被误判 `NothingToCommit`（用例）
- [x] **V8**：`git-diff.md` 未实现能力已标注/移除，与实现对齐
- [x] **V9**：no-newline 标志语义明确（拆分或回归测试锁定）
- [x] **V10**：`repo_info` 进程 spawn 次数减少（基准/审查）
- [x] **基线**：`similar` 移除；`parking_lot`/`tempfile` 回填；diff-service `serde_json`/`thiserror` 移除；`notify-debouncer-full` 关联 P7-6
- [x] **快速验证**：Git 参数、临时文件、watcher/cache 与路径风险立即跑定向回归；workspace 全量与三平台门禁延后到 Core 主干 L2/L3

**相关文档**：[REVIEW.md](../../REVIEW.md) §7 · [ADR-007 系统 Git](../../docs/adr/ADR-007-system-git.md) · [git-diff](../../docs/features/git-diff.md) · [ROADMAP 依赖选型基线](../../ROADMAP.md#依赖选型基线)

> 跨任务协调（2026-08 review）：V6 verbatim cwd 与 P1-13 V3、P11-8 同根，三任务各自收口出口（workspace-service / git 子进程 / sandbox）；基线 `similar`/`parking_lot`/`tempfile` 与 P1-13、P6-14 在根 `Cargo.toml` 与 ROADMAP 基线表上分行归属，序列执行不撞车。

### 验证记录（2026-08-09）

- `cargo test -p git-service -p diff-service`：89 passed（git-service 64、diff-service 25），0 failed；覆盖前导 `-` 注入、随机临时文件、staged-only、TTL/LRU、ignore + linked git-dir、Windows verbatim cwd、silent hook、no-newline 与 repo_info 两次 spawn。
- `cargo clippy -p git-service -p diff-service --all-targets -- -D warnings`：通过。
- `cargo fmt -p git-service -p diff-service -- --check` 与 `git diff --check`：通过。
- `cargo tree -p diff-service --depth 1`：直接依赖中已无 `serde_json` / `thiserror`；根基线已移除 `similar` 并登记 `parking_lot` / `tempfile`。
- 按本任务门禁节奏只执行受影响 crate 的定向门禁；workspace 全量、三平台与发布门禁留待 Core 主干 L2/L3。
