# S12 CR-02 交叉复核（GLM → Grok 报告）

| 项 | 值 |
| --- | --- |
| 复核对象 | [CR-02-policy-tools-git.md](CR-02-policy-tools-git.md) 的 2 条 High finding |
| 复核日期 | 2026-08-18 |
| 复核模型 | zai/glm-5.3（glm_reviewer） |
| 复核方法 | 逐条独立打开源码证据（路径+符号+行号）核对实际行为，不采信报告转述；本任务只复核、不新建 finding |

## 裁定表

| 编号 | 原级别/置信度 | 裁定 | 复核后级别/置信度 | 一行理由 |
| --- | --- | --- | --- | --- |
| S12-CR02-01 | High / Confirmed | **uphold** | High / Confirmed | read_file/list_directory 用纯词法 `resolve_rel` 后直接 `metadata`/`File::open`/`is_dir`/`read_dir` 跟随 symlink，search_text 对条目 `is_file()`+`read_to_string` 跟随文件 symlink，全程无 canonical 复核；ReadOnly 工具免审批且未信任 workspace 放行，越 root 读成立。 |
| S12-CR02-02 | High / Confirmed | **uphold** | High / Confirmed | S3 任务书第 18/61 行声明「路径校验入口已替换为 policy 实现且已勾退出标准」，但 `workspace/core/src/path.rs` 仍是 S2 词法实现（自称临时入口、无 symlink/`.git` 检查），读工具生产调用仍走 `resolve_rel`，`.git` 只读面未被任何门拦截。 |

## 重点问题的独立核对结论

### 1. symlink 解析发生在路径校验之前还是之后

**之后，且之后没有任何补救校验**——这是逃逸成立的机制：

- 读路径顺序：`resolve_rel`（纯词法：空/绝对/`..`/Windows 保留名，见 `workspace/core/src/path.rs` 41-88、106-123）→ 使用点跟随 symlink（`execution/tools/src/read_file.rs` 104-107 的 `tokio::fs::metadata` + `File::open`；`execution/tools/src/list_directory.rs` 131-157 的 `Path::is_dir` + `std::fs::read_dir`；`execution/tools/src/search_text.rs` 158/173 的 `path.is_file()` + `read_to_string`）。词法校验先做但它不感知 symlink；symlink 在文件系统调用时被内核解析，返回的已是 root 外目标，之后无 within-root 复核。
- 对照组证明正确模式已存在于同仓库：`workspace/resources/src/io.rs` 91-98、129-142 的 `read_utf8_bounded_within` → `canonical_within` 先 canonicalize（此时解析 symlink）再 `path_within_root` 校验，然后才读；`execution/policy/src/path.rs` 100-132 对父目录与已存在目标 canonicalize 后做 `within_any_root` 复核并拒绝非普通文件。
- 细节确认：`search_text` 的 `WalkBuilder`（141-146）未调用 `follow_links`，ignore crate 默认 `false`，因此不下降入目录 symlink（与报告一致）；但文件 symlink 仍作为条目产出，`is_file()` 跟随后 `read_to_string` 读出目标。`list_directory` 对 `path` 本身是目录 symlink 的场景，`is_dir()` 跟随返回 true，`read_dir` 直接列出 root 外目标目录内容。

### 2. 读路径是否经过 PolicyEngine

**经过，但只是能力/信任级闸门，不涉及路径安全**：

- `execution/tools/src/scheduler.rs` 345-362：`check_gate` 把 `capability + trusted + allowed_in_untrusted_workspace + approval_mode + 原始 input` 交给 `PolicyEngine::decide`；`execution/policy/src/engine.rs` 70-73 对 `ToolCapability::ReadOnly` 在信任检查通过后一律 `Allow`。PolicyEngine 不解析路径、不调用 `resolve_workspace_path`；路径解析发生在各工具 `execute` 内部的 `resolve_rel`。
- `read_file` 描述符（`execution/tools/src/read_file.rs` 59-68）与 `list_directory`（100-105）均为 `read_only: true`、`requires_approval: false`、`allowed_in_untrusted_workspace: true`——默认 ReadOnly 模式与未信任 workspace 下都免审批放行。报告「无需绕过审批」的影响面描述成立。

## 逐条复核证据（本人打开核对过的位置）

### S12-CR02-01 — uphold（High / Confirmed）

| 报告证据 | 复核结果 |
| --- | --- |
| `execution/tools/src/common.rs` `resolve_rel` 134-139 / `resolve_write_rel` 141-146 | 属实：读走 `pawork_workspace::resolve_relative_path`，写走 `pawork_policy::resolve_workspace_path`，分叉清晰。 |
| `workspace/core/src/path.rs` 1-4、40-88 | 属实：模块头自称「S2 临时入口」，40 行明说不做 symlink/`.git` 检查；函数体仅词法规则。 |
| `execution/tools/src/read_file.rs` `read` 104-107 | 属实：`resolve_rel` 后 `metadata`/`File::open` 跟随 symlink。 |
| `execution/tools/src/list_directory.rs` `list_dir` 131-157 | 属实：`is_dir()`/`read_dir` 跟随目录 symlink；147-157 对条目也用 `std::fs::metadata` 跟随。 |
| `execution/tools/src/search_text.rs` 141-173 | 属实：不下降目录链接但 `is_file()`+`read_to_string` 跟随文件 symlink。 |
| 对照 `execution/policy/src/path.rs` 48-65、119-132；`workspace/resources/src/io.rs` 91-98 | 属实：写内核与资源加载器均已实现 canonicalize-in-root，读工具未对齐。 |

严重度评估：High 恰当。利用前提是 workspace root 内存在指向外部的 symlink（git clone 保留 symlink，恶意/被污染仓库可预置；用户或已审批的 `run_command` 亦可创建），随后默认 ReadOnly、免审批即可读出宿主任意文件（含 `~/.pawork/auth.json`）。未达 Critical（需要 root 内已有/可造链接，非无条件直接越界）。

### S12-CR02-02 — uphold（High / Confirmed，Requirement Gap / False Completion）

| 报告证据 | 复核结果 |
| --- | --- |
| `plan/S3-safe-edits.md` 第 18 行 | 属实：明确「路径校验入口改经 pawork-policy::path……S2 入口签名不变，实现换成 policy 调用」。 |
| `plan/S3-safe-edits.md` 第 61 行 | 属实：退出标准已勾「S2 的路径校验入口已替换为 policy 实现且外部签名未变」。 |
| `workspace/core/src/lib.rs` 3-4；`workspace/core/src/path.rs` 1-4、40 | 属实：源码仍标注等待 S3 接线，S2 实现原样在位，与任务书勾选直接矛盾。 |
| `execution/policy/src/path.rs` 3-6、59-65 | 属实：`.git` 段拒绝只存在于 policy 内核；模块文档自称「所有文件工具……唯一入口」与实际调用面不符，进一步佐证假完成。 |
| `execution/tools/src/common.rs` 134-146 + `rg resolve_rel` | 属实：`resolve_rel` 生产调用仅 `read_file.rs:105`、`list_directory.rs:132`；写三件与 `run_command` cwd 均已走 `resolve_write_rel`。 |
| `.git` 只读面 | 属实：词法入口无 `.git` 检查，`read_file path=.git/config` 可解析打开；`search_text` 141-146 `hidden(false)` 关闭隐藏过滤后 `.git` 条目进入遍历。 |
| `docs/task-guide.md` §3.2 第 56 行 | 属实：把 S3 替换 S2 临时校验列为「计划内替换不是返工」的标准示例。 |

严重度评估：High 恰当。它是 S12-CR02-01 的根因，额外打开 `.git` 只读面（remote URL、hooks、credential helper 配置），并构成安全关键阶段（S3）退出标准的完成声明与源码不符。

## 复核附注

- 原报告两条 finding 的证据链、行为描述、影响面与整改边界均与源码一致，未发现需要降级、升级或修正的事实性错误。
- 一处精确化（不改变裁定）：读路径并非完全「不经过 PolicyEngine」——scheduler 的能力级闸门在链路上，但 ReadOnly 恒放行且不校验路径；报告「读路径仍是 S2 词法门」指的是路径安全层面，表述成立。
