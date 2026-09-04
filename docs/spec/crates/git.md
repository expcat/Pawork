# pawork-git

> 系统 Git 封装与结构化 Diff：以系统 `git` 二进制为唯一后端、统一经 `pawork-exec` 执行子进程；上承 `pawork-app`（默认闭包内的只读 diff/status 面）与 `pawork-orchestration`（feature `git`，默认关闭），下依 `pawork-domain` / `pawork-exec`，属基础设施层。

## 1. 职责与边界

- 提供五类能力：仓库检测与 HEAD 元信息（`GitService`）、工作区状态（`StatusService`）、暂存操作（`StageService`）、worktree 管理（`WorktreeService`）、结构化 Diff 与 hunk/行级暂存（`DiffService` / `HunkStageService`）。
- 唯一后端是系统 `git`：不引入 libgit2 / gitoxide；所有命令经 `GitRunner` → `pawork_exec::ProcessRuntime` 执行，统一 cwd、超时、输出上限与取消归一。
- **不做**：commit / branch / stash / conflict / history / cache 六个零消费服务已于旧 R0（ADR-038 D16，2026-08-18 波 C）归档删除，git tag `v2-final` 可找回，复活条件见 [产品候选](../backlog.md)。
- 不做持久化、不做协议、不做审批决策；`discard` 等危险操作只做风险分级（`StageRisk`），审批由上层 Policy 执行。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~40 | crate 门面：7 个模块声明与全部公开 re-export；`FileStatus` 唯一定义在 `status`，`diff::FileStatus` 为同一类型 re-export |
| `src/error.rs` | ~70 | `GitError` 统一错误枚举（17 个 variant）；`From<pawork_exec::ProcessError>` 归一：Spawn 且 `NotFound`→`GitNotFound`、`KillTimeout`→`Timeout`、其余 Spawn/ProcessTree/Isolation→`Other`、IO 透传 |
| `src/process.rs` | ~200 | `GitRunner`（默认 `git` 路径、30s 超时、16MB 输出上限）；`validate_position_arg` 注入防护；domain→exec 取消令牌桥接（已取消立即 cancel，否则后台任务等待）；Windows `\\?\` verbatim cwd 经 `dunce` 简化 |
| `src/repo.rs` | ~410 | `GitService`：`open`（`rev-parse --show-toplevel` 定位 work tree，失败→`NotARepository`）、`git_dir` / `is_bare` / `current_branch` / `current_head` / `repo_info`（合并 rev-parse，固定两次 spawn，unborn 回退路径）；`Head::{Branch(name), Detached(sha), Unborn}` / `RepoInfo` |
| `src/status.rs` | ~370 | `StatusService` 与自由函数 `read_status`：`git status --porcelain=v1 -z --untracked-files=all` 解析为 `FileChange`（X 列 index / Y 列 worktree 双状态 + `untracked` 标记 + rename/copy 原路径）；`FileStatus` 九态状态码映射；`changed_files` 过滤未跟踪与全未修改条目 |
| `src/stage.rs` | ~450 | `StageService`：`stage` / `unstage` / `discard` / `stage_all` / `classify`（仅 `Discard`=`StageRisk::Dangerous`）；`apply_patch_to_index` 用临时 patch 文件走 `git apply --cached [-R]`，失败归一 `PatchDoesNotApply`；路径一律经 `--` 传递 |
| `src/worktree.rs` | ~360 | `WorktreeService`：`list`（`worktree list --porcelain` 解析）/ `add` / `remove` / `prune`；`remove` 先经 `list()` 校验目标为 git 受管 worktree，绝不调用 `std::fs` 递归删除用户数据 |
| `src/diff/mod.rs` | ~23 | diff 子模块声明与 re-export（`hunk_stage` / `model` / `parser` / `service`）；`FileStatus` 来自 `status` |
| `src/diff/model.rs` | ~70 | 结构化 diff 数据模型：`DiffFile`（path / previous_path / status:`FileStatus` / staged / binary / additions / deletions / hunks，`changed_lines()`）、`DiffHunk`、`DiffLine`、`HunkId`（全局自增 u64）、`LineKind`（Context / Addition / Deletion） |
| `src/diff/parser.rs` | ~270 | unified diff 文本状态机解析：`parse_unified` / `parse_unified_with_start`（延续 HunkId 起点）；`@@` 头解析、`\ No newline at end of file` 标记按上一行类型作用到旧侧/新侧（Context 两侧一致）；容忍任意输入不 panic |
| `src/diff/service.rs` | ~650 | `DiffService`：`diff_summary`（`--raw -z` + `--numstat -z` 按 NUL 记录合并，含多文件与 rename/copy old/new 双路径，工作区视角补 `ls-files --others --exclude-standard -z` 未跟踪文件）、`diff`（逐文件 `-U<n> --no-color -- <path>` 解析 hunks）；`DiffOptions` / `paginate` / `DiffPage`；`--raw` 头解析（状态字母 M/A/D/T/U、R/C 带相似度、mode 160000 gitlink） |
| `src/diff/hunk_stage.rs` | ~715 | `HunkStageService`：`stage_hunks` / `unstage_hunks` / `stage_lines` / `unstage_lines`（底层 `git apply --cached [-R]`）；纯函数 `build_hunk_patch`（重建 `diff --git` 头 + 选中 hunks）与 `build_line_patch`（按 bool 选择行、重算 hunk 行数） |
| `tests/parser_contract.rs` | ~65 | parser golden 契约 + proptest（任意输入不 panic）；夹具在 `tests/golden/*.diff` |

## 3. 对外 API 面

**命令执行（process）**
- `GitRunner::new()`：`git` 路径 + 30s 默认超时 + 新建 `ProcessRuntime`；`with_runtime(runtime, git_path, timeout)` 供测试与定制注入；`Clone` 廉价共享。
- `run(cwd, args, cancel) -> Result<String, GitError>` / `run_with_stderr -> (stdout, stderr)`：输出 lossy UTF-8；非零退出→`GitFailed{code, stderr}`、超时→`Timeout`、取消→`Cancelled`。**不清空环境**（保留用户 git config 与 credential helper）。
- `validate_position_arg(name, value)`：值以 `-` 开头即 `InvalidPositionArgument{name, value}`。适用于 revision / range / branch 等会被 git 当位置参数解析的值；路径参数不走此校验，改用 `--` 分隔。

**仓库（repo）**
- `GitService::open(path, cancel)`：从任意子目录向上定位 work tree；非仓库→`NotARepository`。
- 访问器 `work_dir()` / `runner()`（借出 `GitRunner` 供其它 Service 复用同一 runtime）。
- `git_dir` / `is_bare`（各一次 rev-parse）；`current_branch -> Option<String>`（detached/unborn 为 `None`）；`current_head -> Head`：`symbolic-ref --short HEAD` 成功→`Branch`，stderr 含 `detached`→`Detached(rev-parse HEAD)`，其余失败（如 unborn 的 ref 不存在）→`Unborn`。
- `repo_info -> RepoInfo`：一次合并 rev-parse（toplevel + git-dir + bare + HEAD SHA）+ 一次 symbolic-ref，固定两次 spawn；unborn HEAD 走无 HEAD 的回退查询。

**状态（status）**
- `StatusService::new(&runner, work_dir)`；`status(cancel) -> StatusSnapshot{changes}`：`FileChange{path, previous_path, index_status, worktree_status, untracked}`，porcelain v1 `-z` + `--untracked-files=all`，NUL 分隔保证含空格/非 ASCII 文件名安全，rename/copy 附原路径。
- `changed_files(cancel)`：剔除未跟踪与两列均 `Unmodified` 的条目，供 diff / UI。
- 自由函数 `read_status(work_dir, cancel)`：内部临时构造 `GitRunner` 的一次性入口。
- `FileStatus` 九态：`' '`/M/A/D/R/C/U/T/`?` → Unmodified/Modified/Added/Deleted/Renamed/Copied/Unmerged/TypeChanged/Untracked；未识别字符保守映射 Unmodified。

**暂存（stage）**
- `StageRequest::new(paths)`；`StageService::stage/unstage/discard(&StageRequest, cancel)`、`stage_all(cancel)`。
- `classify(StageOp) -> StageRisk`：仅 `StageOp::Discard` 返回 `Dangerous`，其余 `Safe`——上层审批依据。
- `apply_patch_to_index(patch, reverse, cancel)`：hunk/行级暂存底层入口；index 自 diff 后已变化→`PatchDoesNotApply`。

**Worktree**
- `WorktreeService::new(&runner, main_work_dir)`；`list(cancel) -> Vec<Worktree>`（porcelain 解析：path / branch / HEAD 等）。
- `add(path, branch, base, cancel)`：branch/base 走 `validate_position_arg`，路径经 `--` 传递并拒绝 option 形路径（如 `--force`）；成功后重新 `list()` 返回新条目。
- `remove(path, force, cancel)`：先 `list()` 验证受管身份（canonical 比较），`force=true` 时加 `--force`；`prune(cancel)` 只清 git 元数据。

**结构化 Diff**
- `DiffOptions{staged, context(默认 3), detect_renames(默认 true → `-M`), commit_range: Option<String>}`；`commit_range` 过 `validate_position_arg`；`staged=true` 加 `--cached`。
- `DiffService::new(GitRunner, work_dir)`；`diff_summary(opts, cancel) -> Vec<DiffFile>`：仅文件清单（binary 标记与 add/del 行数已填，hunks 为空）——大 diff 的轻量入口；`diff(opts, cancel)`：在 summary 基础上逐个非 binary / 非 Untracked 文件补 hunks，`HunkId` 全局自增。
- `paginate(files, page, page_size) -> DiffPage{files, total_files, page, page_size}`：`page` 从 1 起、`page_size == 0` 返回全部、越界返回空页。
- `parse_unified(patch)` / `parse_unified_with_start(patch, start) -> (Vec<DiffHunk>, next_id)`：可独立用于解析任意 unified diff 文本。
- `HunkStageService::new(GitRunner, work_dir)`：`stage_hunks/stage_lines`（输入必须来自 worktree-vs-index 的 diff）与 `unstage_hunks/unstage_lines`（输入必须来自 `staged: true` 的 diff）；rename / binary / untracked 文件不支持 hunk 级暂存。`build_hunk_patch` / `build_line_patch` 为纯函数（行级选择全 false 时返回 `None`）。

**错误语义速查（当前有构造点的 variant）**

| variant | 触发条件 | 调用方处置 |
| --- | --- | --- |
| `GitNotFound(program)` | spawn 失败且 `ErrorKind::NotFound`（git 未安装） | 提示安装 git，不重试 |
| `NotARepository(path)` | `GitService::open` 的 rev-parse 失败 | 目录不在仓库内 |
| `GitFailed{code, stderr}` | 命令非零退出 | 按 stderr 呈现；stderr 已 trim |
| `InvalidPositionArgument{name, value}` | 位置参数以 `-` 开头 | 拒绝输入，不发起子进程 |
| `PatchDoesNotApply` | `git apply --cached` 失败（index 已变化） | 重新 diff 后重试 |
| `Timeout` | 超过默认 30s 或 `KillTimeout` | 可放宽超时后重试 |
| `Cancelled` | 取消令牌触发、进程被杀且无退出码 | 正常取消路径 |
| `Io` / `Other` | IO 错误 / 罕见 runtime 失败 | 透传诊断 |

无 feature 门控：本包不声明任何 feature；消费侧 `pawork-orchestration` 以 `git = ["dep:pawork-git"]` 可选接入。

## 4. 核心行为与数据流

1. **一次 git 调用**：domain `CancellationToken` 桥接为 exec 令牌（已取消立即 cancel，否则 spawn 后台等待任务、调用结束 abort）→ 构造 `CommandSpec`（cwd 经 `dunce::simplified` 去 Windows verbatim 前缀、timeout=30s、max_output_bytes=16MB、`env_clear=false`）→ `ProcessRuntime::run` → 归一：`timed_out`→`Timeout`；`killed` 且无退出码→`Cancelled`；退出码 0→Ok；其余→`GitFailed{code, stderr}`。
2. **一次结构化 diff**：`base_args`（`diff` + 可选 range / `--cached` / `-M`）→ `--raw -z` 解析文件清单（`:<oldmode> <newmode> <sha> <sha> <STATUS>\0<path>\0[<origpath>\0]`；R/C 带相似度尾数、mode 160000 标 gitlink）→ `--numstat -z` 按 NUL 记录合并行数与 binary 标记（普通记录内联 path；R/C 空 path 后消费 old/new 两字段并匹配 new path）→ 非 staged 视角追加 `ls-files --others --exclude-standard -z` 未跟踪文件 → （`diff` 入口）逐文件 `git diff -U<n> --no-color -- <path>`，`parse_unified_with_start` 解析 hunks 并延续全局 `HunkId`。binary / gitlink / untracked 不跑文本 hunk（避免把 submodule 工作树当内容解析）。
3. **hunk/行级暂存**：调用方从 `DiffService` 拿 `DiffFile` → `build_hunk_patch`（选中 hunks）或 `build_line_patch`（`selection: &[bool]` 按行保留：未选 Added 行剔除、未选 Removed 行降级为 Context，并重算两侧行数）→ `StageService::apply_patch_to_index`（临时文件 + `git apply --cached [-R]`）→ index 期间被改动则 `PatchDoesNotApply`，调用方应重新 diff 后重试。
4. **status 解析**：`--porcelain=v1 -z` 按 `\0` 分段；每条 `XY PATH`，X→`index_status`、Y→`worktree_status`；`??` 置 `untracked=true`；X 为 R/C 时下一段为原路径存入 `previous_path`。
5. **HEAD 判定**：`symbolic-ref --short HEAD` 三分支——成功且非空→`Branch`；`GitFailed` 且 stderr 含 `detached`→`rev-parse HEAD` 取 SHA→`Detached`；其余失败→`Unborn`（新仓库无提交）。
6. **worktree 生命周期**：`add` 校验 branch / base / 路径 → `git worktree add [--] <path> …` → 重新 `list()` 定位新条目（找不到→`Other`）；`remove` 先 `list()` 确认路径在受管清单内 → `git worktree remove [--force] <path>`；脏 worktree 无 force 删除失败且数据保留。

## 5. 契约与不变量

- **注入防护**：一切来自上层（含模型输出）的 revision / range / branch / worktree 路径若以 `-` 开头一律拒绝（`InvalidPositionArgument`）；文件路径一律放在 `--` 之后传递（名为 `--force` 的文件可被安全 stage，有定向回归）。
- **worktree 删除红线**（ADR-007 延续）：`remove` 只删除经 `list()` 验证的 git 受管 worktree，本 crate 永不执行 `std::fs` 递归删除。
- **风险分级契约**：`StageService::classify` 仅 `Discard` 返回 `Dangerous`；上层审批依赖该分级，不得静默放宽。
- **parser 契约**（golden + proptest 锁定）：任意输入不 panic；`\ No newline at end of file` 语义按上一行类型归属旧侧/新侧；`tests/golden/*.diff` 为冻结夹具，先改 golden 再改实现。
- **资源上限**：单次命令输出 ≤16MB、默认超时 30s，防巨型 diff / log 打爆内存。
- **环境继承**：`GitRunner` 不清空环境变量，git 的用户配置与 credential helper 照常生效——这是行为契约而非疏漏。

## 6. 依赖关系

- **依赖**：`pawork-domain`（`CancellationToken`）、`pawork-exec`（`ProcessRuntime` / `CommandSpec` / `ProcessError` / exec 侧 `CancellationToken`）、`tokio`（sync / macros / rt / time / fs）、`serde` / `serde_json`、`thiserror`、`tracing`、`dunce`（Windows 路径简化）、`tempfile`（**正式依赖**：`apply_patch_to_index` 写临时 patch 文件）。
- **被依赖**：
  - `pawork-app`：默认闭包内唯一生产消费者，仅用 `GitService` / `StatusService` / `DiffService` / `paginate`（`diff.rs` 只读面）。
  - `pawork-orchestration`：`worktree.rs`（`GitRunner` + `WorktreeService`）与 `merge.rs`（`DiffService` + `DiffOptions`）在 feature `git` 之后；该 feature 默认关闭（`default = []`），且 `pawork-app` 以 `default-features = false` 依赖 orchestration，因此默认二进制闭包不含这些路径。
- 全仓分层总览见 [../../architecture.md](../../architecture.md)；依赖方向与包布局见 [../../design.md](../../design.md) §2；Agent loop / 事件持久化等跨包流程见 [../flows.md](../flows.md)；工作区路径与配置事实层见 [workspace.md](workspace.md)。

## 7. 测试与验证资产

- 内嵌单元测试（各模块 `#[cfg(test)]`，多为真实临时仓库集成测试）：
  - `process.rs`：option 形位置参数拒绝、相对 cwd 不变、Windows verbatim 简化。
  - `repo.rs`：open / HEAD 三态（Branch / Detached / Unborn）、repo_info 两次 spawn 断言（`call_count` 测试钩子）。
  - `status.rs`：porcelain 双列状态、rename 原路径、untracked 过滤。
  - `stage.rs`：stage / unstage / discard 真实仓库行为、`--force` 文件名经 `--` 安全 stage、`PatchDoesNotApply` 归一。
  - `worktree.rs`：add / list / remove 生命周期、脏 worktree 无 force 删除失败且数据保留、option 形路径与 branch 拒绝。
  - `diff/service.rs` / `diff/hunk_stage.rs`：diff 清单与 hunks、多文件 NUL numstat 计数、分页、hunk/行级暂存端到端与失败路径。
- `tests/parser_contract.rs`：golden 契约（`tests/golden/basic_hunk.diff` / `no_newline.diff` / `context_no_newline.diff`）+ proptest 任意输入不 panic。
- 默认验证命令：`cargo test -p pawork-git --offline --lib --tests`（需要系统 git 可用）。

## 8. 注意事项与已知限制

- **单一 `FileStatus`**：定义在 `status.rs`（porcelain 九态，crate root re-export）；`diff::FileStatus` 是同一类型。`--raw` 的相似度数字只用于 rename/copy 检测，不进枚举。
- **`HunkStageService` / `StageService` / `WorktreeService` 当前零默认闭包消费者**：HunkStage 与 Stage 在整个 workspace 无生产调用点（Desktop 未接线）；Worktree 仅被 orchestration 的默认关闭 feature 使用。API 保留但演进时无下游回归网。
- `GitError` 中 `NothingToCommit` / `BranchAlreadyExists` / `BranchNotFound` / `BranchNotMerged` / `ReferenceNotFound` / `Conflict` / `DetachedHead` / `LocalChangesWouldBeOverwritten` 等 variant 是 V2 归档服务的遗留，当前全 workspace 无构造点。
- hunk 级暂存对输入来源敏感：`stage_*` 只接受 worktree-vs-index diff、`unstage_*` 只接受 staged diff，混用会 `PatchDoesNotApply`；rename / binary / untracked 不支持。
- 输出为 lossy UTF-8：非 UTF-8 文件名 / 内容会被替换字符污染而非报错。
- rename 检测、porcelain 输出细节依赖系统 git 版本行为，本包未固定最低 git 版本。
- `read_status` 每次调用新建 `GitRunner`（含新 `ProcessRuntime`）；高频调用应改用 `StatusService` 复用 runner。
