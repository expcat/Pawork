# pawork-policy

> 安全内核：把「工具能力 + 输入 + 信任状态 + 审批档位」映射为冻结的 `PolicyDecision`，并承载文件路径安全解析与 shell 风险分类。仅依赖 `pawork-domain`，被 tools / workspace / app 消费，不依赖也不执行任何进程。

## 1. 职责与边界

- **裁决**：`PolicyEngine::decide` 综合 `ToolCapability`、工具 JSON 输入、workspace 信任位、descriptor 的 `allowed_in_untrusted_workspace` 与 `ApprovalMode`，产出 `Allow` / `Deny` / `AskUser` / `AllowWithConstraints`。
- **路径安全**：`resolve_workspace_path` 是所有文件工具解析模型传入路径的**唯一入口**（`workspace_id + relative_path` 语义；函数本身收 `roots` 切片与相对路径字符串，与 workspace-service 解耦）。
- **shell 风险分类**：`classify_command` 用手写轻量 tokenizer（ADR-041 D4，不引 shell 解析库）+ 固定词表判定 `Safe | Dangerous`；`hits_danger_floor`（`pub(crate)`）单独判定灾难地板。
- **不做**：不执行命令、不碰 Git、不读写文件内容（只做 canonicalize / metadata 查询）、不依赖 `pawork-exec`。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~25 | crate 门面：5 个私有 `mod` + 全量 re-export，无 `pub mod`。 |
| `src/decision.rs` | ~105（含测试） | 类型定义：`PolicyDecision`（serde `tag="kind"` snake_case）、`ApprovalPrompt`、`RiskLevel`（Safe/Moderate/Dangerous）、`CommandRisk`（Safe/Dangerous）、`ExecutionConstraints{timeout_ms, max_output_bytes}`。默认值均 Safe。 |
| `src/mode.rs` | ~63（含测试） | `ApprovalMode` 枚举与 serde 形状（snake_case；`NeverAsk` 带 `alias="on_failure"` 只进不出）；默认 `ReadOnly`。 |
| `src/engine.rs` | ~550（逻辑 ~215 + 测试） | `PolicyEngine` / `PolicyInput` 与 `decide` 决策树；命令提取 `extract_command`（argv 优先）；进程默认约束常量（timeout 60_000ms、输出 1 MiB）。 |
| `src/path.rs` | ~510（逻辑 ~260 + 测试） | `resolve_workspace_path` / `ResolvedPath` / `PathSafetyError`；辅助 `canonicalize_platform`（dunce）、`path_within_root`、`relative_to_root`（Windows 大小写不敏感逐组件比较）；`canonicalize_deepest_existing` 支持尚不存在的嵌套新路径。 |
| `src/shell.rs` | ~1265（逻辑 ~840 + 测试） | 手写 Lexer（`Word`/`Tok`/`Cmd`）、`parse_commands`（语句→管道→命令）、升档分类 `classify_snippet`、灾难地板 `script_floor`、固定词表、`extract_shell_script`（`sh -c` / `cmd /c` / `powershell -Command` / POSIX 含 `c` 短选项簇）。 |

无 `tests/` 目录与 fixtures；全部回归内联在各文件 `#[cfg(test)]`。

## 3. 对外 API 面

### 3.1 审批模式与裁决类型

- `ApprovalMode`（serde snake_case，默认 `ReadOnly`），严格度递增：
  - `NeverAsk`：不询问；副作用能力直接放行（Process 附加默认约束）。反序列化接受旧档位别名 `"on_failure"`（只进不出，序列化永不输出）。
  - `AskForDangerous`：仅 `effective_risk == Dangerous` 时询问。
  - `AskForWrites`：一切副作用能力（除 ReadOnly / UserInteraction 外）询问。
  - `AlwaysAsk`：一切能力询问（只读能力除外，见 §4.1 步骤 3）。
  - `ReadOnly`：拒绝一切非只读能力。
- `PolicyDecision`（serde `tag="kind"`）：
  - `Allow`：无载荷，JSON 恰为 `{"kind":"allow"}`。
  - `Deny { reason: String }`：reason 可展示、可入日志，**禁止含 Secret**。
  - `AskUser { prompt: ApprovalPrompt }`；`ApprovalPrompt { message, risk: RiskLevel }`。
  - `AllowWithConstraints { constraints: ExecutionConstraints }`；`ExecutionConstraints { timeout_ms: Option<u64>, max_output_bytes: Option<u64> }`。
- `RiskLevel`：`Safe | Moderate | Dangerous`（prompt 展示用）；`CommandRisk`：`Safe | Dangerous`（`classify_command` 返回值）。两者默认值均 `Safe`。

### 3.2 PolicyEngine

- `PolicyEngine::new(mode)` / `.mode()`：构造档位仅作记录；`decide(&PolicyInput)` 一律以 `PolicyInput::approval_mode` 为准（支持按调用覆盖）。
- `PolicyInput { capability: ToolCapability, input: serde_json::Value, trusted: bool, allowed_in_untrusted_workspace: bool, approval_mode: ApprovalMode }`。
- 命令提取（Process 能力）：优先非空 `argv`（`argv[0]` 为 program、其余为 args，与 `run_command` 实际执行形状一致）；否则 `program` / `command` / `cmd` 字符串 + `args` 数组。

### 3.3 路径安全

- `resolve_workspace_path(roots: &[PathBuf], relative: &str) -> Result<ResolvedPath, PathSafetyError>`。
- `ResolvedPath`：
  - `absolute: PathBuf`——canonical、已复核在 root 内的真实路径。
  - `root: PathBuf`——命中的 canonical root。
  - `relative: String`——规范化后相对 root 的路径（不含 `.` / `..`）。
- `PathSafetyError` 变体：
  - `Empty`：空字符串输入。
  - `AbsolutePath`：拒绝一切绝对路径（含 Windows 盘符/UNC）。
  - `Traversal(String)`：`..` 越出 root 或出现 RootDir/Prefix 组件。
  - `SymlinkEscape`：canonicalize 后落在所有 root 之外。
  - `GitInternals`：任一组件为 `.git`（大小写不敏感）。
  - `NonRegular`：目标是 device / fifo / socket 等非常规文件。
  - `NoRoot`：roots 为空或全部 canonicalize 失败。
  - `Io(std::io::Error)`：其余文件系统错误。
- 辅助函数：`canonicalize_platform`（Windows 去 `\\?\` verbatim 前缀）、`path_within_root`、`relative_to_root`（Windows 盘符与组件大小写不敏感）。

### 3.4 shell 风险分类

- `classify_command(program: &str, args: &[String]) -> CommandRisk`：升档判定（是否需要 `AskForDangerous` 询问）。
- `hits_danger_floor` 为 `pub(crate)`，仅 engine 消费，**不是**公开 API。

## 4. 核心行为与数据流

### 4.1 `PolicyEngine::decide` 决策顺序

1. **信任硬门**：`!trusted && !allowed_in_untrusted_workspace` → `Deny`（descriptor 是未信任工作区的第一道门）。
2. **灾难地板**（仅 `ToolCapability::Process` 且 `hits_danger_floor` 命中）：`NeverAsk` / `ReadOnly` 档 → `Deny`（灾难命令不得静默执行）；三个 Ask 档 → `AskUser(risk=Dangerous)`。
3. **只读能力放行**：`ToolCapability::ReadOnly` 过信任门后一律 `Allow`（与档位无关，`AlwaysAsk` 也不问）。
4. **按档位分派**：
   - `ReadOnly` 档 → `Deny`（拒一切非只读能力）。
   - `NeverAsk` → `allow_or_constrained`：Process 附加默认约束（`timeout_ms=60_000`、`max_output_bytes=1_048_576`）返回 `AllowWithConstraints`，其余能力 `Allow`。
   - `AlwaysAsk` → `AskUser(effective_risk)`。
   - `AskForWrites` → 副作用能力（除 ReadOnly / UserInteraction）`AskUser`，否则 `Allow`。
   - `AskForDangerous` → `effective_risk == Dangerous` 时 `AskUser(Dangerous)`，否则 `allow_or_constrained`。
5. `effective_risk` 折算：ReadOnly / UserInteraction → Safe；Process → 按 `classify_command`（Dangerous → Dangerous、Safe → Moderate）；其余能力（WorkspaceWrite / GitWrite / Network / ExternalPlugin）→ Moderate。

### 4.2 `resolve_workspace_path` 解析流程

1. 空串 → `Empty`；绝对路径 → `AbsolutePath`。
2. 任一组件为 `.git`（Windows 大小写不敏感）→ `GitInternals`，先于一切文件系统访问；`.gitignore` 等 dotfile 不受影响。
3. 词法规范化：弹栈消解 `..`，越出栈底 → `Traversal`；出现 `RootDir` / Windows `Prefix` → `Traversal`。
4. `roots` 为空或全部 canonicalize 失败 → `NoRoot`。
5. 逐 root 尝试：对父目录链执行 `canonicalize_deepest_existing`（向上找最深已存在祖先 canonicalize 再拼回缺失组件——缺失组件尚不存在、不可能是 symlink，故支持写工具新建 `a/b/c.txt`）；拼回目标文件名后必须仍落在任一 canonical root 内，否则 `SymlinkEscape`。
6. 目标已存在时再次 canonicalize（缓解解析与使用之间被替换为 symlink 的 TOCTOU）并复核仍在 root 内；`symlink_metadata` 判定文件类型，device/fifo/socket → `NonRegular`（非 Unix 平台上非常规文件/目录一律保守拒绝）。
7. 命中 root → 返回 `ResolvedPath`；多 root 逐个尝试，全部失败返回最后一个错误。root 内部 symlink（目标仍在 root 内）允许，`absolute` 指向解析后的真实路径。

### 4.3 shell 分类管线（升档与地板共用 tokenizer）

1. `extract_shell_script` 识别 shell 包装并提取内层脚本重新走整条管线（递归，深度上限 `MAX_SCRIPT_DEPTH = 12`）：
   - POSIX 族：`sh|bash|zsh|dash|ksh|ash|fish|csh|tcsh -c`，含单 dash 组合短选项簇（`-lc`/`-cl` 等含 `c` 即按 `-c` 对待，取簇后首个非选项参数为脚本，宁可升档）。
   - Windows：`cmd /c`（`/c`/`-c` 大小写不敏感）、`powershell|pwsh -Command|/Command|-c|/c`。
2. 非 shell 调用时把 `program + args` 拼为脚本视图整体解析（覆盖「程序名含空白/分隔符」与 argv 形态）。
3. Lexer 认知：单引号（内无转义）、双引号（`\"` `\\` `\$` `` \` `` 与行续接）、反斜杠转义、`$VAR`/`${VAR}`/位置与特殊参数（标记 dynamic）、`$(...)` 与反引号命令替换（内层原文收集进 substitutions）、`&&`/`||`/`|`/`;`/换行分段、`#` 注释、重定向目标提取（`>` `>>` `2>` 等 fd 数字前缀、`&>`、`>&N`）。未闭合引号按已读内容收尾，不 panic。
4. `parse_commands` 把 token 流组装为「语句 → 管道 → 命令（program + args + redirect_targets）」。
5. 升档判定（每命令）：
   - 危险重定向目标：`/`、`/etc` 精确，及 `/dev/` `/etc/` `/usr/` `/proc/` `/sys/` `/boot/` `/var/` 前缀。
   - 命令替换内层脚本递归分类。
   - 程序位 dynamic（`$X` / `$(...)` 拼程序名）保守判 `Dangerous`（仅升档，不进地板）。
   - 同管道内 `curl`/`wget` 之后接 sh 族或 python/perl → 远程脚本执行升档。
   - 最后 `classify_single` 过固定词表。
6. 固定词表（大小写折叠、剥 `.exe` 后匹配）：
   - 危险程序：`sudo su dd shutdown reboot halt poweroff format reg mkfs mkfs.* remove-item del erase osascript diskpart schtasks launchctl`。
   - 解释器内联代码：`python*/python3 -c`、`perl -e`。
   - `rm`：带递归 flag（`-r/-R/--recursive` 或组合簇如 `-rf`）或宽目标（`/ ~ $HOME * . .. /etc /usr /var /home /boot`）。
   - `chmod`/`chown` 带递归 flag。
   - `git push --force|-f`（`--force-with-lease` 不算）与 `git branch -D|-d|--delete`。
7. 灾难地板独立管线 `script_floor`：只认完全静态可判定形式——`mkfs`/`mkfs.*`、`dd of=/dev` 或 `of=/dev/...`、`rm` 同时具备递归 + force + 目标 `/`。程序位 dynamic 或未知形态**绝不**进地板（`NeverAsk` 下误拒是事故）；引号剥离后匹配（`'r'm -rf '/'` 仍命中）。

## 5. 契约与不变量

- **冻结契约**：`PolicyDecision`（`kind` tag + 四变体载荷形状）与 `ApprovalMode`（五值 snake_case + `on_failure` 只进不出）是跨包 wire 形状，由 `decision.rs` / `mode.rs` 内联 serde 回归钉死；变更须走 ADR。
- **灾难地板集合不变**：`mkfs*`、`dd of=/dev/*`、`rm -rf /`（含 `sh -c` 嵌套与静态可判定的命令替换内层）。即使 `trusted + NeverAsk` 也 `Deny`。
- **`.git` 永拒**：路径含 `.git` 组件一律 `GitInternals`，先于文件系统访问；dotfile（`.gitignore` 等）不受影响。
- **ReadOnly 双语义不可混淆**：`ReadOnly` **能力**过信任门后恒 `Allow`；`ReadOnly` **档位**拒绝一切非只读能力。
- **`Deny.reason` 不得含 Secret**：engine 产出的 reason 均为固定英文短语，不回显输入。
- **升档只紧不松**：tokenizer 归一化（引号拼接程序名、转义、命令替换递归、fd 重定向）只会让原本漏判的形态升档；参数位变量维持字面匹配不升级（见 §8）。
- 无 golden/fixture 文件；契约由内联测试向量承载（`serializes_snake_case`、`deny_roundtrips_with_kind_tag`、`danger_floor_only_matches_catastrophic_forms` 等）。

## 6. 依赖关系

- **依赖**：`pawork-domain`（仅 `ToolCapability`）；外部 `serde` / `serde_json` / `thiserror` / `dunce` / `regex`（regex 在 Cargo.toml 声明但当前源码未直接使用）。无 cargo feature，无平台差异依赖。dev 依赖 `tempfile`。
- **被依赖**：`pawork-tools`（common 路径解析 + scheduler 闸门）、`pawork-workspace`、`pawork-app`。
- **刻意不依赖本包**：`pawork-exec`（其内部 `path.rs` 为三函数小复制，见 [exec.md](exec.md) §2）。

## 7. 测试与验证资产

默认验证命令：`cargo test -p pawork-policy --offline --lib --tests`（无 `tests/` 目录，`--tests` 为空集，全部用例在 `--lib`）。

| 文件 | 覆盖点 |
| --- | --- |
| `decision.rs` | `PolicyDecision` 四变体 serde 形状（`kind` tag、`allow` 无载荷、`deny_roundtrips_with_kind_tag`）、`RiskLevel`/`CommandRisk` 默认 Safe。 |
| `mode.rs` | 五档 snake_case 序列化 golden（`serializes_snake_case`）、全变体反序列化含 `on_failure → NeverAsk`、默认 `ReadOnly`。 |
| `engine.rs` | 信任门（untrusted 写拒绝 / descriptor 放行续走）、`ReadOnly` 档拒写允读、`trusted+NeverAsk` 放行写与带约束进程、AskForWrites / AskForDangerous / AlwaysAsk 行为、灾难地板三形态 Deny（含 argv 形态）、`git push --force` 升档、`extract_command` argv 优先、输入档位覆盖引擎档位。 |
| `path.rs` | 空/绝对/穿越拒绝、规范化后合法 `..`、`.git` 拒绝（含 `sub/.git`）、dotfile 放行、已存在/新建/深层不存在路径解析、`NoRoot`、root 边界（`root-other` 前缀不误判）、symlink 逃逸拒绝与 root 内 symlink 放行（Unix）、FIFO 拒绝（Unix）、junction 逃逸 / 大小写 / verbatim 前缀 / `.GIT`（Windows）。 |
| `shell.rs` | 固定词表全家（rm/sudo/dd/mkfs/shutdown 族/chmod -R/git/python -c/perl -e/PowerShell 与 cmd 命名命令/远程管道）、`-lc` 组合簇提取、嵌套 `sh -c`、引号拼接程序名归一化、命令替换递归（升档与地板双管线）、`danger_floor_only_matches_catastrophic_forms`、动态程序位升档不入地板、管道分段独立判定、重定向形态（`2>` `>>` `&>` 引号目标）、转义归一化、地板忽略变量形态、参数位变量保持字面语义、注释/未闭合引号健壮性。 |

三类不推迟的关键测试中，本包承载「安全红线定向回归」主体；tools 侧的路径红线消费另见 [tools.md](tools.md) §7。

## 8. 注意事项与已知限制

- **残余局限（有意保留，不静默扩大审批或误拒）**：
  - 参数位变量引用（`rm "$DIR"`）按 flag/字面匹配，不因未知变量升级。
  - `env` / `xargs` / `nohup` 等 launcher/包装器不解包内层命令。
  - 算术/进程替换 `<(...)` 与 heredoc 内容不参与分类。
  - PowerShell / cmd 语法按 POSIX 近似处理（反斜杠按转义消费）。
- `AllowWithConstraints` 仅对 `Process` 能力产生；约束注入到工具输入的动作发生在消费方（`pawork-tools` scheduler，见 [tools.md](tools.md) §4.1）。
- `resolve_workspace_path` 会真实访问文件系统（canonicalize / symlink_metadata）；对不存在路径不报 NotFound（留给上层 I/O），只保证安全性。
- PTY 创建入闸不在本包：闸在 app 宿主（capability=`Process`，AskUser fail-closed 落 Deny，R7 波 B / ADR-041 D2）；本包只提供裁决原语。
- 相关文档：Spec 索引 [../README.md](../README.md)；架构与冻结契约事实源 [../../design.md](../../design.md)；跨包运行时数据流 [../flows.md](../flows.md) 与 [../../architecture.md](../../architecture.md)；任务状态 [../../../ROADMAP.md](../../../ROADMAP.md)。
