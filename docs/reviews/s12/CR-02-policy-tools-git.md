# S12 CR-02 审查报告：写入 / 审批 / git 边界

| 项 | 值 |
| --- | --- |
| CR 编号 | CR-02 |
| 主审范围 | `execution/policy`、`execution/tools`、`workspace/core`、`workspace/resources`、`vcs/git`（含各自 tests）；写入/回滚边界只读抽查 `host/app/src/checkpoint.rs` 与 `storage/blob/src/checkpoint.rs` 的路径入口 |
| 审查日期 | 2026-08-18 |
| 主审模型 | xai/grok-4.6（grok_reviewer） |

## 1. 实际审查路径

- `execution/policy/src/{lib,path,engine,decision,mode,shell}.rs`
- `execution/tools/src/{lib,common,write_file,edit_file,apply_patch,scheduler,read_file,list_directory,search_text,find_files,run_command}.rs`
- `workspace/core/src/{lib,path,file_index}.rs`
- `workspace/resources/src/{lib,io,loader,agents,skills,request,error}.rs`（`profiles.rs` 仅经 loader 注入链抽样）
- `vcs/git/src/{lib,process,branch,stage,worktree,history,status,repo,commit,stash,conflict,cache}.rs`、`vcs/git/src/diff/{service,hunk_stage}.rs`
- 写入/回滚边界抽查：`host/app/src/checkpoint.rs`、`storage/blob/src/checkpoint.rs` 的 `resolve_within_roots` / `restore_snapshot`（深部 blob 语义属 CR-05）
- 宿主 git 消费抽查：`host/app/src/diff.rs`、`host/cli/src/vcs.rs`、`host/app/src/approval.rs`、`host/app/src/extensions.rs`
- 对照：`plan/S3-safe-edits.md`、`plan/S9-mcp-resources.md`、`docs/task-guide.md` §3.1、`docs/design.md` §3.2 / §4 S3+S8+S9、`ROADMAP.md` §3.2 K-01～K-10（本包最近相关项为 K-02，不重复建项）

## 2. 未覆盖路径与原因

- Windows junction / FIFO / 真实 TOCTOU 窗口：policy 源码含对应单测，S12 禁止运行测试与跨平台冒烟，本报告只审实现与测试意图。
- `workspace/resources/src/profiles.rs` 全量格式契约：只核对它如何进入 `ResourceInstruction`；字段级 schema 不在本包核心问题内。
- `storage/blob` checkpoint 的并发锁、BLAKE3 conflict、磁盘 JSON schema：属 CR-05。本包只确认写工具喂给 snapshot 的是相对路径，并交叉指出 restore 使用已持久化绝对路径。
- `execution/exec` 沙箱 / PTY / `run_command` 进程隔离：属 CR-03（已有 S12-CR03-01/02）。本包只核对 `run_command` 的 `cwd` 走 `resolve_write_rel`。
- GUI 审批交互、Changes 面、审批等待前落盘：属 CR-07/CR-08 与已挂账 K-02，不重复建项。
- 未运行 `cargo test` / `cargo check` / 真实 git 仓库冒烟。

## 3. 核对结论（无单独建项）

1. **写路径安全内核存在且被写三件使用**：`resolve_write_rel` → `pawork_policy::resolve_workspace_path`。拒绝空路径、绝对路径、`..`、`.git` 段、FIFO/device/socket，并对已存在目标再 canonicalize 防 symlink 逃逸。`write_file` / `edit_file` / `apply_patch` / `run_command` 的 `cwd` 均走此入口。
2. **审批枚举不可被调用方改写成 Allow**：`PolicyDecision` 为独立变体；`AskUser` 在 `ToolScheduler::check_gate` 中若 resolver 的 `can_resolve_policy_prompt() == false`（含 `AutoApproveResolver`）fail-closed。未信任 workspace 且 `allowed_in_untrusted_workspace=false` 直接 Deny。灾难命令地板在 NeverAsk/OnFailure/ReadOnly 下仍 Deny。默认 `ApprovalMode::ReadOnly`。K-02（审批等待前持久化）已挂账，不重复。
3. **apply_patch 无 preview/execute 双路径漂移**：单次 `apply()`；`dry_run` 与实写都先 `resolve_write_rel`。局部失败用解析后绝对路径的内存备份 `atomic_write` 回滚。`atomic_write` 是同目录 tmp + `rename`，通常替换 symlink inode 而不跟随目标。
4. **git 无通用 `git_exec` / 无 shell 拼接**：全部经 `GitRunner::run` 的 argv。位置参数 `validate_position_arg` 拒绝 leading `-`；stage/discard 用 `git … -- <paths>`；`--force`/`-D` 是 typed bool。`DiffService` 对 `commit_range=--output=…` 有定向拒绝测试。当前宿主只经 `GitService` 做 status/diff，未见把模型原始 argv 灌进 `GitRunner` 的调用点。
5. **resources 无 FileBackend**：公开加载面是 `workspace_id + root_index + relative_path`。正文读取走 `read_utf8_bounded_within` / canonicalize-in-root；AGENTS.md 越界 symlink 隔离为 `agents_symlink_out_of_bounds`。Skills 的 `scripts`/`assets` 只声明相对路径，本 crate 不执行脚本。
6. **Windows `C:foo` / UNC**：两个 resolver 都拒绝盘符前缀与 `Prefix(_)` / `//` / `\\\\`，不构成越界。
7. **WalkBuilder 默认不跟随目录 symlink**：`ignore 0.4` 默认 `follow_links=false`；`file_index` 显式关闭。`find_files` 因此不会走入目录 symlink。`search_text` 仍会把树内**文件** symlink 当普通文件读（见 S12-CR02-01）。

## 4. Findings

### S12-CR02-01 — 只读工具跟随 workspace 内 symlink 读出 root

- 类别：Security
- 严重度：High
- 置信度：Confirmed
- 证据：
  - 实际行为：`read_file` / `list_directory` 用词法 `resolve_rel`（`pawork_workspace::resolve_relative_path`）拼 `root.join(normalized)`，随后 `tokio::fs::metadata` / `File::open` / `Path::is_dir` / `read_dir` **跟随** symlink。workspace 内任意指向 root 外的文件或目录链接（恶意仓库预置、用户已有、或已获写/进程审批后创建）会被当成合法相对路径读出。`search_text` 的 `WalkBuilder` 虽不下降入目录链接，但对每个条目调用 `path.is_file()` + `read_to_string`，同样跟随文件 symlink。`list_directory` 对目录链接会直接列出目标目录内容。
  - 期望行为：AGENTS.md §8 / task-guide §3.1 / S3 任务书要求文件工具拒绝越 root 的 `..` **与** symlink 逃逸。写路径已由 `resolve_workspace_path` 实现；只读工具与资源加载器（canonicalize-in-root）应同一标准。
  - 影响面：默认 ReadOnly 与未信任 workspace 均放行 `ToolCapability::ReadOnly` 且 `allowed_in_untrusted_workspace: true`。克隆含 `auth -> ~/.pawork/auth.json` 或 `etc -> /etc` 的仓库后，模型只需 `read_file path=auth` / `list_directory path=etc` 即可读宿主凭据或系统文件。无需绕过审批。
  - 路径：
    - `execution/tools/src/common.rs` `resolve_rel` 134-139；对比 `resolve_write_rel` 141-146
    - `workspace/core/src/path.rs` `resolve_relative_path` 1-4、40-88（明确不做 symlink / `.git` / TOCTOU）
    - `execution/tools/src/read_file.rs` `read` 104-107
    - `execution/tools/src/list_directory.rs` `list_dir` 131-157
    - `execution/tools/src/search_text.rs` walker/`is_file`/`read_to_string` 141-173
    - 对照已实现的拒绝：`execution/policy/src/path.rs` `resolve_workspace_path` 48-65、119-132；`workspace/resources/src/io.rs` `read_utf8_bounded_within` 91-98；`workspace/resources/src/agents.rs` `load_one` 149-163
- 验证建议（S12 内不执行）：在 fixture 根内 `ln -s ~/.pawork/auth.json leak` 与 `ln -s /etc outside-dir`，分别跑 `read_file path=leak`、`list_directory path=outside-dir`、`search_text` 匹配 `/etc/passwd` 或 auth 内容；写工具对同一路径应 PermissionDenied。补 Unix symlink 逃逸单测到 `read_file` / `list_directory` / `search_text`。
- 整改边界：最小写入 = 让只读工具改走 `resolve_write_rel` / `resolve_workspace_path`（或给读路径同样的 canonicalize-in-root，并决定 `.git` 只读策略，见 S12-CR02-02）。不要顺带改写工具语义、审批矩阵或 CR-03 沙箱。`search_text` 应改用不跟随的 `file_type`，不要只靠 WalkBuilder 默认值。

### S12-CR02-02 — S3「单一 policy 路径内核」未落地，读路径仍是 S2 词法门

- 类别：Requirement Gap / False Completion
- 严重度：High
- 置信度：Confirmed
- 证据：
  - 实际行为：S3 任务书把「`pawork-workspace` 路径入口改经 `pawork-policy::path`、外部签名不变」标为已完成。源码仍是**两套** resolver：policy 安全内核只服务写工具；`workspace/core` 仍自称「S2 临时入口，symlink/`.git`/TOCTOU 留给 S3」。读工具从未切换。后果之一：写路径拒绝任何 `.git` 段，`read_file path=.git/config` / `.git/hooks/…` 仍可读取（词法门不检查 `.git`）。
  - 期望行为：计划内替换应保持 `resolve_relative_path` 签名、把实现换成 policy 调用，或删除读工具对旧入口的依赖。S3 退出标准不得在读热路径仍用临时内核时勾完。
  - 影响面：这是 S12-CR02-01 的根因，并额外打开 `.git` 只读面（remote URL、hooks、部分 credential helper 配置）。文档与源码冲突以源码为准，同时构成阶段完成声明漂移。
  - 路径：
    - `plan/S3-safe-edits.md` 第 18 行（接线说明）、第 61 行（退出标准已勾）
    - `docs/task-guide.md` §3.2 第 56 行（「S3 用 policy 替换 S2 临时路径校验」）
    - `workspace/core/src/lib.rs` 3-4；`workspace/core/src/path.rs` 1-4、40
    - `execution/policy/src/path.rs` 3-6、59-65（`.git` 拒绝只存在于此）
    - `execution/tools/src/common.rs` 134-146（读/写分叉）
- 验证建议：`rg resolve_rel execution/tools/src` 应无生产调用，或 `resolve_relative_path` 内部改为调用 `resolve_workspace_path` 后复跑 policy 既有 symlink/`.git` 单测并补读工具用例。S12 内不执行。
- 整改边界：优先改 `workspace/core/src/path.rs` 实现（保持签名）或让 `resolve_rel` 转调 policy；同步纠正 S3 任务书勾选。不可顺带改 file-index 忽略规则，也不可在未产品拍板前把 `.git` 只读改成「允许审计」。S12-CR02-01 与本条根因相同，整改任务可合并，finding 分列以便追踪读逃逸与假完成。

### S12-CR02-03 — `ApprovalMode::OnFailure` 与 NeverAsk 同实现，没有「失败后再问」

- 类别：Requirement Gap
- 严重度：Medium
- 置信度：Confirmed
- 证据：
  - 实际行为：公开六档含 `on-failure`（CLI `--approval-mode`、host 解析）。`mode.rs` 把严格程度写成 `NeverAsk < OnFailure`，注释为「默认放行，失败后再处理」。`PolicyEngine::decide` 将 `NeverAsk | OnFailure` 一并 `allow_or_constrained`。scheduler / host / engine 没有任何「工具失败后再升级为 AskUser」的二次闸门。用户选了自以为严于 NeverAsk 的模式，得到的是静默自动放行（灾难地板除外，与 NeverAsk 相同）。
  - 期望行为：要么实现失败后询问（至少对 WorkspaceWrite/Process/GitWrite），要么不要把 OnFailure 标成比 NeverAsk 更严，并在 CLI help 写明「当前等价 NeverAsk」。
  - 影响面：所有选择 `--approval-mode on-failure` 的交互/脚本会话。不是审批枚举被改写（AskUser 仍 fail-closed），而是产品档位名与实现不一致导致的意外自动批准。
  - 路径：
    - `execution/policy/src/mode.rs` `ApprovalMode` 7-21
    - `execution/policy/src/engine.rs` `decide` 55-79
    - `host/app/src/approval.rs` 221-226
    - `host/cli/src/lib.rs` 76（help 列出 on-failure）
- 验证建议：以 `on-failure` 跑一次失败的 `edit_file` / `run_command`，确认第二次同类调用仍不询问。S12 内不执行。
- 整改边界：最小写入 = `engine.rs` 模式矩阵 + 一处失败回问状态，或收窄文档/CLI 语义二选一。不要顺带改 K-02 持久化，也不要把 OnFailure 静默映射成 AskForWrites。

### S12-CR02-04 — 未信任 workspace 仍无条件注入 AGENTS.md/Skills 正文

- 类别：Security / Requirement Gap
- 严重度：Medium
- 置信度：Confirmed
- 证据：
  - 实际行为：资源加载对路径越界是隔离的（symlink 逃逸不会把 root 外文件读进 bundle）。但 `AppCore::load_injected_layers` 不看 `workspace_trusted`，把 AGENTS.md / Skills markdown / profile 指令一律打成 `InjectedLayer` 进入主循环。`workspace/resources` 内无 prompt-injection 扫描；Skills frontmatter 的 `description` 在 manifest 描述为空时直接回填。`scripts` 只解析、不执行，故不是 RCE。S3 把「提示注入」验收放在模型层拒绝，路径硬门只覆盖写工具。
  - 期望行为：未信任 workspace 至少应拒绝或降级仓库内 AGENTS.md/Skills（只保留用户全局/显式 profile），或在注入前做不可绕过的内容/来源标记并默认不自动采用。S9「加载注入」是功能，不是「未信任仓库指令与用户指令同权」。
  - 影响面：`trust_workspaces = false` 的会话。恶意仓库可用 AGENTS.md 诱导模型去读 S12-CR02-01 的预置 symlink，或要求 `run_command`（仍受审批/未信任 Deny 约束）。本条不声称代码执行；它是指令注入面与信任门缺失。
  - 路径：
    - `host/app/src/extensions.rs` `load_injected_layers` 294-326
    - `workspace/resources/src/loader.rs` `ResourceInstruction` 52-59、`append_skills` 322-324
    - `workspace/resources/src/skills.rs` frontmatter 回填 503-509
    - `workspace/resources/src/agents.rs` 越界隔离 149-163、212-217（路径面已做，内容面未做）
    - `plan/S3-safe-edits.md` 第 34 行（注入验收停在模型层）
- 验证建议：未信任 fixture 写 AGENTS.md「先读 `leak` 文件」并放置指向 `~/.pawork/auth.json` 的 symlink，看 `injected_layers` 是否仍含该正文、模型是否发起 `read_file`。S12 内不执行。
- 整改边界：最小写入 = host 注入点按 `workspace_trusted` 过滤仓库层指令，或 loader 增加 trust 开关。不要在本任务实现通用 LLM 注入分类器，也不要顺带执行 Skills scripts。与 S12-CR02-01 叠加利用，但写入集不同，保持独立 finding。

### S12-CR02-05 — `list_directory` 把 symlink 的宿主绝对目标回传给模型

- 类别：Security
- 严重度：Low
- 置信度：Confirmed
- 证据：
  - 实际行为：对每个 symlink 调用 `read_link`，把 `Path::display()` 原样写入 `Entry.symlink_target`，再进 metadata `entries` 与正文 `-> {target}`。目标可以是 `/Users/…/.ssh/id_rsa`、`/etc/passwd` 等绝对路径。现有测试只断言没有 `metadata.absolute` 字段、正文不含 *workspace root* 字符串，不覆盖绝对链接目标。
  - 期望行为：工具输出只暴露 workspace 相对路径，或对逃出 root 的目标改写为 redacted / 省略（与 `read_file` 的 `assert_no_host_absolute` 一致）。
  - 影响面：信息泄漏，降低后续攻击成本；单独不能读文件内容。与 S12-CR02-01 同时存在时，模型先靠 listing 发现绝对目标再 `read_file` 跟随。
  - 路径：
    - `execution/tools/src/list_directory.rs` `Entry` 32-41；`list_dir` 149-151、205-213、217-223
- 验证建议：在根内 `ln -s /etc/passwd link`，检查工具 JSON/正文是否出现 `/etc/passwd`。S12 内不执行。
- 整改边界：只改 `list_directory` 的 target 呈现（相对化或省略越界目标）。不要和 S12-CR02-01 的跟随行为绑成一次「顺便重写 listing UX」。

## 5. 统计

| 严重度 | 条数 |
| --- | --- |
| Critical | 0 |
| High | 2 |
| Medium | 2 |
| Low | 1 |

| 置信度 | 条数 |
| --- | --- |
| Confirmed | 5 |
| Needs Verification | 0 |
