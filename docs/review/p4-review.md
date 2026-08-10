# Phase 4 Review：核心工具与权限

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（工作树除 `REVIEW-P2.md` / `REVIEW-P3.md` 未跟踪外干净）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 4「核心工具与权限」的 12 个任务（P4-1 ~ P4-12）的完成情况、所引入包是否合适、基线偏差；漏洞与优化点一并列出。Phase 4 是关键路径中「Built-in Tools → Policy」一环，上游承接 Phase 3 Agent Loop，下游被 Phase 5（Compaction 引用 checkpoint）、Phase 11（Sandbox 复用 ProcessRuntime / ExecutionConstraints）、Phase 12（Worker 写隔离）依赖，受影响处在文中标注「传播面」。

### 1. 结论摘要

1. **测试全绿，但「绿」的含金量与 Phase 3 同档——单元自测充分、端到端接线缺失**：4 个交付 crate（`builtin-tools` / `policy-engine` / `checkpoint-service` / `process-runtime`）复跑共 **99 passed / 0 failed**（builtin-tools 31、checkpoint-service 13、policy-engine 50、process-runtime 5）；`clippy -D warnings`、`fmt --check` 干净。但 99 项测试全部是「单工具/单模块自测」，没有任何一项覆盖「Scheduler → PolicyEngine → Tool → Checkpoint 回滚」的真实链路。
2. **核心问题与 REVIEW-P3 §2 同源：组件齐全，主干未接线**。`PolicyEngine::decide()`（P4-9 的全部决策逻辑：6 种 ApprovalMode、Shell 风险分类、信任闸门、ExecutionConstraints）**全仓库零生产调用**——唯一调用方是 policy-engine 自己的 13 个单测（[engine.rs:212-376](../../crates/policy-engine/src/engine.rs)）。执行路径上的 `tool-runtime` 调度器只用一个 `require_approval_for_writes: bool` 布尔（[scheduler.rs:274-283](../../crates/tool-runtime/src/scheduler.rs)）替代了整套策略引擎；工具描述符里的 `allowed_in_untrusted_workspace` 字段**全仓库无任何强制点**。结果是：P4-10 的「未信任工作区默认限制写/命令」在运行时并不存在闸门。
3. **执行上下文注入假值，checkpoint 上下文断链**：调度器把 `workspace_id` / `run_id` 硬编码为 `"default"`（[scheduler.rs:261-262](../../crates/tool-runtime/src/scheduler.rs)），导致所有写工具的 checkpoint 都挂在 `"default"` run 下、与真实 Agent run 无关，回滚键全局碰撞。这与 REVIEW-P3 V8/V9（上下文注入假值）同根。
4. **一项数据完整性缺陷**：apply_patch 部分失败回滚不完整——对 `create` 覆盖既有文件、`update`、`delete` 三类操作，错误路径只回滚 `create`（新建）与 `rename`（反向），内容型操作依赖 checkpoint 但**从不调用 `rollback_tool_call`**；尤其 `create` 覆盖既有文件时 `rollback_done` 直接 `remove_file`，原内容丢失。验收「部分失败回滚」仅覆盖 create-new 单一情形（V3）。
5. **包选型总体合理，无「应自实现替换」命中**：regex（线性时间、ReDoS 安全）、ignore+globset（ripgrep 同源）、chardetng+encoding_rs（Mozilla 编码检测）、libc（Unix 进程组）、blake3（checkpoint 内容寻址）使用面都覆盖核心价值区。按基线「参考+自实现」落地的 edit_file / apply_patch 匹配器方向正确，但**违反基线自定的「需完整 fuzz 与审计」标准——零属性/fuzz 测试**（基线原文见 ROADMAP「完全自实现」与 §3.3）。
6. **基线管理优于 Phase 1/2/6**：无「引入未登记」依赖；唯一的「声明未引用」是 `content-inspector`（基线记 P4-1，但 read_file 实际只用 chardetng+encoding_rs，全仓库零引用）。另有 4 个 crate 内死依赖（agent-domain×2、bytes、futures）与 `atomic_write` 四处重复实现，属清理项。
7. **流程合规**：12 篇 `plan/P4-*.md` 状态均为 `🟢已完成`、验收框全部已勾，提交 `da1c260` 同步更新了 ROADMAP——**纠正了 Phase 2/3 的 plan 停留 🟡、验收未勾的流程偏差**（见 REVIEW-P2 §1-5、REVIEW-P3 §1-5）。docs/features（tools/policy/checkpoint/process）与 ADR-009/010 均已就位。
8. **四个「mock 过得去、真实运行会暴露」的中危项**：① run_command 非真流式（缓冲全集后一次性 emit 单 delta，长构建用户全程无输出，V8）；② Windows env allowlist 缺 SYSTEMROOT/TEMP 等，env_clear 后复杂工具链行为异常（V5）；③ edit_file 模糊匹配吞文件尾换行（V6）；④ list_directory 单个 dangling symlink 致整目录列出失败（V7）。
9. **checkpoint 崩溃恢复缺口**：checkpoint 元数据（run→change→blob 映射）纯内存（[lib.rs:151/155](../../crates/checkpoint-service/src/lib.rs)），进程崩溃后索引丢失、无法回滚——与 P3-10「崩溃后 <1s 恢复」目标及 ADR-010「所有改动可撤销」矛盾（V9）。blob 本身持久，但映射不持久。
10. **路径安全是全 Phase 4 最扎实的一环**：`resolve_workspace_path`（[path.rs](../../crates/policy-engine/src/path.rs)）防穿越/绝对路径/`.git`/symlink 跳出/设备文件/TOCTOU，逻辑完备、15 项测试，且被 builtin-tools 实际复用——是少数真正接线的策略能力。

### 2. P4 任务完成情况核对表

| 任务 | 交付 crate/模块 | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P4-1 read_file | `builtin-tools/read_file.rs` | 🟢（有 V11） | 行号/offset/limit、二进制+chardetng 编码检测、路径经 policy-engine；但整文件 `std::fs::read` 后再切片（[read_file.rs:93](../../crates/builtin-tools/src/read_file.rs)） |
| P4-2 write_file | `builtin-tools/write_file.rs` | 🟢 | 原子写 tmp+sync+rename、建父目录、保留 unix mode、写前 checkpoint（[write_file.rs](../../crates/builtin-tools/src/write_file.rs)） |
| P4-3 edit_file | `builtin-tools/edit_file.rs` | 🟢（有 V6/V13） | 精确替换/多段预演原子/uniqueness 冲突/模糊匹配；但模糊模式吞尾换行（[edit_file.rs:317](../../crates/builtin-tools/src/edit_file.rs)） |
| P4-4 apply_patch | `builtin-tools/apply_patch.rs` | 🟡部分（有 V3） | create/update/delete/rename、dry run、原子提交；但部分失败回滚不完整（[apply_patch.rs:313-329](../../crates/builtin-tools/src/apply_patch.rs)），验收仅覆盖 create-new |
| P4-5 run_command | `builtin-tools/run_command.rs` | 🟡部分（有 V5/V8） | exit code/timeout/cancel/进程树终止正确；但「流式」名不副实（[run_command.rs:151-160](../../crates/builtin-tools/src/run_command.rs)），Windows env 缺关键变量（[run_command.rs:31](../../crates/builtin-tools/src/run_command.rs)） |
| P4-6 search_text | `builtin-tools/search_text.rs` | 🟢（有 V10/V11） | 固定串/正则（regex 线性时间无 ReDoS）、glob+ignore、上下文行、字节预算；但入口单点 cancel、整文件入内存 |
| P4-7 find_files | `builtin-tools/find_files.rs` | 🟢（有 V10） | glob/类型/深度/ignore/稳定排序、结果受限；但入口单点 cancel |
| P4-8 list_directory | `builtin-tools/list_directory.rs` | 🟡部分（有 V7/V12） | 类型/大小/mtime/symlink/分页；但 dangling symlink 致整目录失败（[list_directory.rs:113](../../crates/builtin-tools/src/list_directory.rs)）、全收集后分页 |
| P4-9 Policy Engine | `policy-engine` | 🟡部分（有 V1/V4） | 6 种 ApprovalMode、路径安全、Shell 分类、信任闸门、50 测试；但 `decide()` **零生产调用**，执行路径未接线 |
| P4-10 Workspace Trust | `policy-engine`+`workspace-service` | 🟡部分（见 V1/V2） | `PolicyInput.trusted` + `requires_trust` 模型在；但信任来源未接线、`allowed_in_untrusted_workspace` 不强制、调度器上下文为假值 |
| P4-11 Checkpoint 与回滚 | `checkpoint-service` | 🟢（有 V9/V11） | 写前 snapshot、按 tool_call/run 逆序回滚、冲突检测（BLAKE3）、不 `git reset --hard`；但元数据纯内存（崩溃不可回滚） |
| P4-12 Process Runtime | `process-runtime` | 🟢（有 V14） | Unix 进程组 `setpgid`+`killpg`、Windows `taskkill /T`、无死锁并发读、max_output 截断、timeout+cancel；但 `spawn_stream` 返回死句柄（[lib.rs:258](../../crates/process-runtime/src/lib.rs)） |

**门禁证据（2026-08-08 复核）**：

- `cargo test -p builtin-tools -p policy-engine -p checkpoint-service -p process-runtime`：**99 passed / 0 failed**（builtin-tools 31、checkpoint-service 13、policy-engine 50、process-runtime 5）。
- `cargo clippy -p <同上> --all-targets -- -D warnings`：干净。
- `cargo fmt -p <同上> -- --check`：干净（退出码 0）。
- 各 `plan/P4-*.md` 验收项均已勾选；ROADMAP Phase 4 计数 12/12 🟢。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本 | 使用点 | 使用面评估 | 结论 |
| --- | --- | --- | --- | --- |
| `regex` | 1 | P4-6 search_text | 线性时间引擎，从结构上消除 ReDoS，满足 P4-6「无 ReDoS」验收；`RegexBuilder` 控制大小写 | **保留** |
| `ignore` + `globset` | 0.4 / 0.4 | P4-6、P4-7 | WalkBuilder+GitignoreBuilder、GlobSet 多模式匹配，ripgrep 同源 | **保留** |
| `chardetng` + `encoding_rs` | 0.1 / 0.8 | P4-1 read_file | Mozilla 编码检测 + 解码，`decode` 损失式兜底并标注 | **保留** |
| `libc` | 0.2 | P4-12 process-runtime | Unix `setpgid`/`killpg`，`unsafe` 边界最小（仅进程组信号） | **保留** |
| `blake3` | 1 | P4-11 checkpoint | 冲突检测重算哈希、blob 内容寻址，SIMD 加速自实现不可企及 | **保留** |
| `tokio`（process/io-util/sync/time） | 1 | P4-5、P4-12 | 子进程/管道/超时/cancel 标准原语 | **保留** |
| `serde`/`serde_json`/`thiserror`/`async-trait`/`tracing` | 基线版本 | 全局 | 基础设施，无争议 | **保留** |

#### 3.2 需要重新评估的项

| 项 | 现状 | 建议 |
| --- | --- | --- |
| `content-inspector = "0.2"` | 基线记 P4-1 使用，但 read_file 实际只用 `chardetng`，**全仓库零引用**（`rg content_inspector` 无命中） | **移出基线**（声明虚置，与 REVIEW.md 的 uuid/tracing-appender/similar 同类） |
| `agent-domain`（policy-engine） | [policy-engine/Cargo.toml](../../crates/policy-engine/Cargo.toml) 声明，源码零引用（policy-engine 只用 tool_api） | 删除该依赖；不影响接口 |
| `agent-domain`（checkpoint-service） | [checkpoint-service/Cargo.toml](../../crates/checkpoint-service/Cargo.toml) 声明，源码零引用 | 删除该依赖 |
| `bytes`、`futures`（process-runtime） | [process-runtime/Cargo.toml](../../crates/process-runtime/Cargo.toml) 声明，源码零引用（实际只用 tokio 原语） | 删除；REVIEW-P2 曾把 futures/bytes 列为「引入未登记」，现已回填基线但在此 crate 仍是死引用 |

#### 3.3 「自实现替换包」总体判断

针对「引用面小 → 自实现」命题：**P4 范围内没有命中**。每个被引用包使用面都覆盖核心价值区。反向看，按基线「参考+自实现」落地的 edit_file / apply_patch 精确匹配与 fuzzy 匹配器方向正确（安全关键路径需可控语义），但**违反基线自定的验收标准**——ROADMAP「完全自实现」表对 apply_patch/edit_file 明确要求「需完整 fuzz 与审计」，而 builtin-tools 与 checkpoint-service **零 proptest/arbitrary/cargo-fuzz 目标**（`rg arbitrary|proptest|fuzz` 仅误命中 fuzzy 特性名）。建议补属性测试：随机 `old_string`/`new_string`/文件内容组合，断言不 panic、`occurrences` 计数与最终内容一致、回滚后与原文逐字节相等。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填基线表。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 声明未引用 | `content-inspector` | [Cargo.toml:103](../../Cargo.toml) | 基线记 P4-1，实际零引用，见 §3.2 |
| crate 内死依赖 | `agent-domain` | [policy-engine/Cargo.toml](../../crates/policy-engine/Cargo.toml)、[checkpoint-service/Cargo.toml](../../crates/checkpoint-service/Cargo.toml) | 两 crate 源码均零引用 |
| crate 内死依赖 | `bytes`、`futures` | [process-runtime/Cargo.toml](../../crates/process-runtime/Cargo.toml) | 源码零引用 |

**对比**：Phase 4 **无「引入未登记」**（所有外部依赖均在 workspace 基线内），基线卫生优于 Phase 1（6 个未登记）/Phase 2（futures/bytes）/Phase 6（base64 等）。`cargo build`/`clippy` 不会报死依赖（路径依赖会被链接），需 `cargo machete`/`cargo udeps` 才能检出——建议在 CI 增加一道。

**建议**：一次小型清理——移出 `content-inspector` 基线声明、删 3 个 crate 的 4 个死依赖、把四处重复的 `atomic_write` 下沉到 `builtin-tools/common`（或 checkpoint-service 导出复用），CI 增加 machete/udep 门禁。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V14）。

#### V1 [安全·高] PolicyEngine 主干未接线，`allowed_in_untrusted_workspace` 完全不强制

`PolicyEngine::decide()` 的全部 13 处调用都在 policy-engine 自测内（[engine.rs:212-376](../../crates/policy-engine/src/engine.rs)），**全仓库无任何生产调用方**（`rg "\.decide\("` 仅命中 policy-engine 测试与 agent-engine 的 `RetryPolicy.decide`，后者是重试策略、无关）。执行路径上 `tool-runtime` 调度器自带的 `requires_approval` 只看 `config.require_approval_for_writes` 布尔 + capability 类型（[scheduler.rs:274-283](../../crates/tool-runtime/src/scheduler.rs)），不查信任、不分类 Shell、不用 ApprovalMode。工具描述符里的 `allowed_in_untrusted_workspace`（read 工具 `true`、写工具 `false`）**全仓库零强制点**（`rg allowed_in_untrusted_workspace` 排除赋值后无命中）。

后果：P4-9 的审批模式、P4-10 的「未信任工作区默认限制写/命令」（ADR-009）在运行时**不存在闸门**——与 REVIEW.md V1（`trust_workspaces` 未消费）同一攻击面的延续：一旦接线就有自我提权风险，但当前根本未接线，所以是「安全能力未生效」而非「漏洞被利用」。传播面：Phase 11 Sandbox 引用了 `policy_engine::ExecutionConstraints`（[sandbox-runtime/src/lib.rs:56-57](../../crates/sandbox-runtime/src/lib.rs)），但只取约束类型、不走 decide。

#### V2 [正确性/安全·高] 调度器硬编码上下文，checkpoint 上下文断链

[scheduler.rs:261-262](../../crates/tool-runtime/src/scheduler.rs) 构造 `ToolExecutionContext` 时 `workspace_id` / `run_id` 均写死 `"default"`、`working_directory: None`。后果：① write_file/edit_file/apply_patch 调 checkpoint 时传入 `run_id="default"`，所有 run 的改动挂在同一 key 下，回滚键全局碰撞、跨 run 互相污染；② 真实 Agent run 的 run_id 永远到不了工具。与 REVIEW-P3 V8/V9（上下文注入假值、Scheduler 未与 ProviderLoop 桥接）同根。传播面：Phase 5 Compaction、Phase 12 Worker 写隔离都依赖 run 级 checkpoint 正确归属。

#### V3 [数据完整性·高] apply_patch 部分失败回滚不完整，create 覆盖既有文件会丢原内容

部分失败路径调用 `rollback_done`（[apply_patch.rs:313-329](../../crates/builtin-tools/src/apply_patch.rs)），其语义为：`Create` → `remove_file`；`Delete`/`Update` → 空操作（注释「由 checkpoint rollback 恢复」）；`Rename` → 反向 rename。问题：① 错误路径**从不调用 `checkpoint_service::rollback_tool_call`**，注释承诺的 checkpoint 恢复无人触发，Update/Delete/Create-over-existing 的已应用改动**留在半应用状态**；② 特别地，`create` 覆盖既有文件时（[apply_patch.rs:182-188](../../crates/builtin-tools/src/apply_patch.rs) 已为其拍了 snapshot），`rollback_done` 仍走 `Create => remove_file`（[apply_patch.rs:319-321](../../crates/builtin-tools/src/apply_patch.rs)），**直接删除文件、原内容丢失**。验收「部分失败回滚」的测试 `partial_failure_rolls_back` 只覆盖 create-new + rename-fail，未覆盖 update/delete/create-over-existing。**建议**：rollback_done 对已 snapshot 的路径改用 checkpoint 内容恢复；或在错误路径追加一次 `rollback_tool_call` 调用，并补三类回归测试。

#### V4 [安全·中] NeverAsk/OnFailure 完全跳过 Shell 风险分类，无硬拒绝地板

[engine.rs:63](../../crates/policy-engine/src/engine.rs) 对 `NeverAsk | OnFailure` 直接 `allow_or_constrained(cap)`，不调用 `effective_risk`/`classify_command`——即 trusted + NeverAsk 下 `rm -rf /`、`dd of=/dev/sda` 被 `AllowWithConstraints`（仅附 timeout/输出上限）放行。Shell 分类器（[shell.rs](../../crates/policy-engine/src/shell.rs)）只在 AlwaysAsk/AskForWrites/AskForDangerous 三种模式下才生效。当前因 V1 引擎未接线而仅具理论风险，但一旦接线，最宽松模式对最具破坏性命令也无「硬拒绝地板」。**建议**：增加一个无视 ApprovalMode 的 denylist 地板（如 `rm -rf /`、`mkfs`、`dd of=/dev/` 恒 Deny 或恒 AskUser），把 Shell 分类从「装饰」提升为「底线」。

#### V5 [安全/正确性·中] Windows env allowlist 缺失关键变量

[run_command.rs:31](../../crates/builtin-tools/src/run_command.rs) `ENV_ALLOWLIST = ["PATH","HOME","LANG","LC_ALL","TERM"]`，配合 `spec.env_clear = true`（[run_command.rs:131](../../crates/builtin-tools/src/run_command.rs)）。Windows 上：① 缺 `SYSTEMROOT`（cmd.exe / 多数程序加载 ntdll 等系统 DLL 依赖它）、`TEMP`/`TMP`（写临时文件的工具失败）、`USERPROFILE`、`COMSPEC`、`PATHEXT`；② `HOME` 在 Windows 通常不存在（用 `USERPROFILE`），`LANG`/`LC_ALL`/`TERM` 多为空，实际只透传了 PATH。本机 smoke test（`echo hello`）能过是因为 cmd 内建命令不触系统 DLL 加载，但 `cargo build`、`git`、PowerShell 脚本等真实工具链会异常或行为偏差。allowlist 还硬编码、Unix 中心、无工作区配置透传。**建议**：按平台分桶（Windows 额外含 SYSTEMROOT/TEMP/TMP/USERPROFILE/COMSPEC/PATHEXT），并允许配置层追加透传变量。

#### V6 [正确性·中] edit_file 模糊匹配吞文件尾换行

`replace_fuzzy` 用 `content.lines()`（剥终止换行）收集后 `out.join("\n")` 重建（[edit_file.rs:298-317](../../crates/builtin-tools/src/edit_file.rs)）。`str::lines()` 不保留结尾 `\n`，`join("\n")` 不补回——因此当模糊匹配窗口含末行时，源文件结尾的 `\n` 被静默吞掉。非模糊 `replacen` 路径操作原始字符串、不受影响。后果：对源文件做一次末行模糊编辑即丢失 POSIX 文本文件的结尾换行（可能触发 lint/格式门禁）。现有测试 `fuzzy_match_normalizes_whitespace` 用单行无尾换行样本，未覆盖此情形。**建议**：重建时根据原文是否以 `\n` 结尾补回；或记录并保留尾换行。

#### V7 [健壮性·中] list_directory 单个 dangling symlink 致整目录列出失败

[list_directory.rs:111-115](../../crates/builtin-tools/src/list_directory.rs) 对每个 entry 调 `entry.metadata()?`——`DirEntry::metadata` **跟随符号链接**，dangling symlink 返回 `NotFound` 并经 `?` 传播，**整目录列出失败**。验收「symlink 信息正确」的测试只造了有效 symlink。后果：工作区里一个失效链接就让 list_directory 整体报错，Agent 无法浏览。**建议**：改用 `symlink_metadata`（不跟随）判类型/大小，对跟随失败的目标降级为「broken symlink」而非整体失败。

#### V8 [可用性·中] run_command 非真流式

[run_command.rs:148-160](../../crates/builtin-tools/src/run_command.rs) 先 `runtime.run(spec, cancel).await`（缓冲全集 stdout/stderr），完成后一次性 `sink.emit(OutputDelta{...})` 各发一个 delta——注释自承「结果以事件回放保证流式可见」。对长构建/测试，用户全程看不到增量输出，直到进程结束才一次性涌入。process-runtime **已有**真流式 `spawn_stream`（[process-runtime/src/lib.rs:219](../../crates/process-runtime/src/lib.rs)）但未被 run_command 采用。与 REVIEW-P3 V2（流式增量被 LoopSink 全量缓冲、从不广播）同类。**建议**：run_command 改用 `spawn_stream`，边读边 emit。

#### V9 [健壮性·中] checkpoint 元数据纯内存，崩溃后不可回滚

checkpoint 的 run→change→blob 映射存于 `Arc<Mutex<BTreeMap<...>>>`（[lib.rs:151](../../crates/checkpoint-service/src/lib.rs)）与 `paths` 映射（[lib.rs:155](../../crates/checkpoint-service/src/lib.rs)），**不持久化**。blob 本身经 ArtifactStore 落盘，但索引纯内存。进程崩溃后（正是 P3-10「Interrupted Run 恢复」要处理的场景）映射丢失，rollback_run/rollback_tool_call 找不到记录，ADR-010「所有 Agent 改动可审查与撤销」在崩溃路径上不成立。传播面：Phase 5 Compaction、Phase 12 Worker 回滚均依赖 checkpoint 可恢复。**建议**：把 RunCheckpoint（或其投影）写入 session-store / Event Store，崩溃恢复时重建映射。

#### V10 [健壮性·低] search_text / find_files 仅入口单点 cancel

二者各只在 `execute` 入口检查一次 `cancel.is_cancelled()`（[search_text.rs:81](../../crates/builtin-tools/src/search_text.rs)、[find_files.rs:81](../../crates/builtin-tools/src/find_files.rs)），随后进入同步 `WalkBuilder` 遍历 + 逐文件读取，全程不再查 cancel。大仓库（数十万文件）长扫描无法中途取消。**建议**：遍历循环内每 N 个 entry 或每文件检查一次 cancel。

#### V11 [性能·中] 多工具在 async 中做阻塞 std::fs 且整文件入内存

- read_file：`std::fs::read(&absolute)` 整文件入内存后再 offset/limit 切片（[read_file.rs:93](../../crates/builtin-tools/src/read_file.rs)）；`MAX_OUTPUT_BYTES` 只限渲染输出、不限读取，多 GB 日志会撑爆内存。
- search_text：`std::fs::read_to_string(path)` 逐文件全量读（search_text.rs `scan_file` 前），大文件内存与阻塞风险。
- checkpoint：`std::fs::read(&absolute)` 全量读后存 blob（[lib.rs:193](../../crates/checkpoint-service/src/lib.rs)）。
- 三者均在 `async fn execute` 内同步阻塞调用（无 `spawn_blocking`），慢盘/大文件会卡住 tokio worker 线程（与 REVIEW.md P1-6 cli-host shell 阻塞同型）。

**建议**：读路径改 `tokio::fs` + 流式/分块（read_file 按行流读至 limit）；重 IO 包 `spawn_blocking`。

#### V12 [性能·低] list_directory 全收集后分页，分页名不副实

[list_directory.rs:111-157](../../crates/builtin-tools/src/list_directory.rs) 先把 `read_dir` 全部 entry 收进 `Vec<Entry>`（含每项两次 metadata 系统调用），排序后 `skip(offset).take(limit)`。超大目录（如 node_modules、构建产物）O(N) 内存与时间，offset/limit 不省成本。**建议**：对稳定排序需求可接受现状，否则记录总数后单次扫描取页。

#### V13 [性能·低] edit_file 模糊匹配 O(L·n) 行拼接

`count_fuzzy`/`replace_fuzzy` 对 `content.lines().windows(n)` 每个窗口做 `join("\n")` + `normalize_ws`（[edit_file.rs:277-317](../../crates/builtin-tools/src/edit_file.rs)），每窗口 O(n) 拼接，整体 O(L·n)。大文件 + 大 `old_string` 块时偏慢。**建议**：滚动哈希或规范化后单次扫描匹配。

#### V14 [健壮性·低] process-runtime `spawn_stream` 返回死句柄且不限输出

[lib.rs:258](../../crates/process-runtime/src/lib.rs) `let handle = ProcessHandle { child: None }`——child 已 move 进 spawned task，句柄内无 child，故 `handle.kill()`/`handle.id()` 恒空操作；唯一停止路径是 `cancel` token。同时 `spawn_stream` 的 `stream_lines` **不执行 `max_output_bytes`**（见 [process-runtime/src/lib.rs stream_lines](../../crates/process-runtime/src/lib.rs)），`Exit` 事件恒 `truncated: false`（[lib.rs:253](../../crates/process-runtime/src/lib.rs)）。缓冲 `run()` 路径有截断、流式路径无——语义不一致。**建议**：句柄持有发送端或 child id 以支持外部 kill；流式路径补输出上限与截断标记。

### 6. 优化建议（按优先级）

#### P0（建议在下一阶段开工前处理）

1. **V1**：把 `PolicyEngine::decide()` 接入 `tool-runtime` 调度器，用 `PolicyDecision` 替换 `require_approval_for_writes` 布尔；同时让调度器强制 `allowed_in_untrusted_workspace`（未信任工作区 + 该字段 false → Deny）。这是 Phase 4 安全边界的「通电」动作，成本最低时点就是现在（未接线阶段）。
2. **V2**：调度器从真实 `ToolExecutionContext`（workspace_id / run_id / working_directory）注入，消除 `"default"` 假值；与 REVIEW-P3 V8/V9 合并处理。
3. **V3**：apply_patch 部分失败回滚补全（内容型操作走 checkpoint 恢复，create-over-existing 不删除原文件）+ 三类回归测试。

#### P1（近期排期）

4. **V9**：checkpoint 元数据持久化（session-store / Event Store 投影），支撑崩溃后回滚——ADR-010 与 P3-10 的前置。
5. **V8**：run_command 改用 `spawn_stream` 实现真流式。
6. **V5**：env allowlist 按平台分桶 + 配置可透传。
7. **V6**：edit_file 模糊匹配保留尾换行 + 回归测试。
8. **V7**：list_directory 改 `symlink_metadata`、dangling 降级。
9. **V11**：read_file/search_text/checkpoint 改流式或 `spawn_blocking`，避免 worker 阻塞与整文件入内存。
10. **V4**：增加无视 ApprovalMode 的危险命令硬拒绝地板。
11. **§3.3**：为 edit_file/apply_patch 匹配器补 proptest/arbitrary 属性测试（满足基线「需完整 fuzz」标准），断言不 panic、计数一致、回滚逐字节相等。

#### P2（顺手/评估项）

12. **基线清理**（§4）：移 `content-inspector` 基线声明、删 4 个 crate 内死依赖、`atomic_write` 四处下沉复用、CI 增 `cargo machete`/`cargo udeps` 门禁（2026-08-10 更新：门禁工作流已移除，转为文档记录的 L3 维护检查项）。
13. **V10**：search/find 遍历内周期性查 cancel。
14. **V12**：list_directory 大目录分页优化。
15. **V13**：edit_file 模糊匹配滚动匹配。
16. **V14**：spawn_stream 句柄可 kill + 流式输出上限。

### 7. 附录

#### 7.1 Phase 4 与「优先级 P1」标签任务

ROADMAP 中 Phase 4 的 12 个任务**均无 P1 标签**（全部为 P0），故无跨 Phase 的 P1 任务需在此追踪。涉及 P4 产物的 P1 任务：P9-7（MCP OAuth，⚪）会复用 auth 能力，与 P4 无直接耦合。

#### 7.2 文档与 ADR 现状

- `docs/features/`：tools.md / policy.md / checkpoint.md / process.md 均已存在（Phase 4 模块文档齐全）。
- ADR：ADR-009（默认工作区信任）、ADR-010（全写 checkpoint）均已就位并被 plan 引用。
- 注：本文档未逐字审阅 docs 内容，仅确认存在性；如需文档级评审另开任务。

### 8. 建议的后续动作（本次未执行，供研究）

1. 对 V1/V2 立项（安全边界通电 + 上下文接线）——与 REVIEW-P3 §8 的「主干接线」合并为一个跨 Phase 任务。
2. 对 V3 立项（apply_patch 回滚补全 + 回归测试）。
3. checkpoint 持久化（V9）作为 P3-10/P5 的前置评估。
4. 基线清理小任务（§4 + §3.2），一次提交完成。
5. 匹配器属性测试补全（§3.3 / P1-11），满足基线自定 fuzz 标准。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 4 个 Phase-4 crate 的测试与静态门禁；文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更。*

---

## 修复记录（review-remediation）

> Phase 4 · 核心工具与权限 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P4-1 ~ P4-12、P3-11 V8（scheduler 上下文注入先于本任务的策略接线）

**最终目的**：消除 [REVIEW.md](../../REVIEW.md) §4（Phase 4）评审发现的安全边界未接线、上下文断链、数据完整性缺陷与基线卫生问题——把 `PolicyEngine::decide()` 与 `allowed_in_untrusted_workspace` 接入执行路径，消除调度器 `"default"` 假值，补全 apply_patch 部分失败回滚，让 checkpoint 可崩溃恢复，并清理 crate 内死依赖与缺位的匹配器 fuzz 测试。

**涉及范围**：`policy-engine`、`builtin-tools`、`checkpoint-service`、`process-runtime`、`tool-runtime`（scheduler）、根 `Cargo.toml`、ROADMAP「依赖选型基线」、各 `plan/P4-*.md`

### 细分步骤（分组）

#### A. 安全边界通电（V1 / V4）

1. **V1 PolicyEngine 接线**：把 `PolicyEngine::decide()` 接入 `tool-runtime` 调度器，用 `PolicyDecision` 替换 `require_approval_for_writes` 布尔；强制 `allowed_in_untrusted_workspace`（未信任工作区 + false → Deny）。目的：P4-9/P4-10 的信任闸门在运行时生效。
2. **V4 危险命令硬拒绝地板**：增加无视 ApprovalMode 的 denylist 地板（`rm -rf /`、`mkfs`、`dd of=/dev/` 恒 Deny/AskUser），让 Shell 分类成为底线。目的：最宽松模式对最破坏性命令也有地板。

#### B. 上下文注入（V2）

3. **V2 checkpoint 上下文断链**：调度器从真实 `ToolExecutionContext`（workspace_id/run_id/working_directory）注入（依赖 P3-11 V8 已完成），消除所有 run 的改动挂在同一 `"default"` key、回滚键全局碰撞。目的：Phase 5 Compaction / Phase 12 Worker 写隔离依赖 run 级 checkpoint 正确归属。

#### C. 数据完整性（V3）

4. **V3 apply_patch 回滚补全**：错误路径对已 snapshot 的路径改用 checkpoint 内容恢复（而非 `remove_file`），或追加一次 `rollback_tool_call`；`create` 覆盖既有文件不删除原内容；补 create-over-existing / update / delete 三类回归测试。目的：消除半应用状态与原内容丢失。

#### D. 健壮性/可用性（V5 / V6 / V7 / V8）

5. **V5 Windows env allowlist**：按平台分桶（Windows 额外含 SYSTEMROOT/TEMP/TMP/USERPROFILE/COMSPEC/PATHEXT），允许配置层追加透传变量。目的：env_clear 后复杂工具链行为正常。
6. **V6 edit_file 尾换行**：模糊匹配重建时按原文是否以 `\n` 结尾补回；补回归测试。目的：消除 POSIX 文本文件结尾换行被吞。
7. **V7 list_directory dangling symlink**：改用 `symlink_metadata`（不跟随）判类型/大小，跟随失败降级为「broken symlink」而非整目录失败。目的：单个失效链接不阻断目录浏览。
8. **V8 run_command 真流式**：改用 `process-runtime` 的 `spawn_stream` 边读边 emit。目的：长构建/测试用户可见增量输出。

#### E. 持久化与性能（V9 / V11）

9. **V9 checkpoint 元数据持久化**：Run→change→blob/path 映射以版本化状态文件原子写入 Artifact Store 根目录，`CheckpointService::open` 在崩溃恢复时重建；避免 `checkpoint-service` 反向依赖 `session-store`，后续组合层可再投影为事件。目的：ADR-010「所有改动可撤销」在崩溃路径成立。
10. **V11 阻塞 IO**：read_file/search_text/checkpoint 改 `tokio::fs` + 流式/分块或 `spawn_blocking`，read_file 读取受预算约束。目的：避免 worker 线程阻塞与整文件入内存。

#### F. 其余健壮性（V10 / V12 / V13 / V14）

11. **V10 周期 cancel**：search_text/find_files 遍历内每 N entry 检查一次 cancel。目的：大仓库长扫描可中途取消。
12. **V12 list_directory 分页**：大目录单次扫描取页，记录总数。目的：分页名副其实。
13. **V13 edit_file 滚动匹配**：模糊匹配改滚动哈希或规范化后单次扫描。目的：消除 O(L·n) 行拼接。
14. **V14 spawn_stream 句柄/上限**：句柄持有发送端或 child id 支持外部 kill；流式路径补 `max_output_bytes` 与截断标记。目的：消除死句柄与流式/缓冲语义不一致。

#### G. 基线/包清理与 fuzz

15. **死依赖清理**：移除 `content-inspector` 基线声明；删 `policy-engine`/`checkpoint-service` 的 `agent-domain`、`process-runtime` 的 `bytes`/`futures` 死依赖；`builtin-tools` 三处 `atomic_write` 已下沉到 `common`，`checkpoint-service` 与 `artifact-store` 因依赖方向和持久语义不同保留各自实现。目的：基线与 crate 依赖卫生。
16. **维护期死依赖检查**：把 `cargo machete`/`cargo udeps` 放入依赖升级、发布候选或定期维护的 L3 工作流，不加入每次开发提交的阻塞链。目的：在功能簇稳定后防止死依赖再生，同时避免前期频繁依赖调整拖慢实现。（2026-08-10 更新：不在本项目配置自动执行 Actions，`dependency-hygiene.yml` 已移除；检查项保留为文档记录，随 L3 维护人工执行）
17. **匹配器属性测试**：为 edit_file/apply_patch 匹配器补 proptest 策略属性测试（随机 old_string/new_string/文件内容组合，断言不 panic、计数一致、回滚后逐字节相等），满足基线「需完整 fuzz 与审计」标准；`arbitrary` 若需使用，必须按基线流程重新引入。目的：安全关键路径覆盖属性测试。

#### H. 文档同步

18. Phase 4 的 12 篇 `plan/P4-*.md` 已勾选（无 drift），本任务确保新验收项随修复勾选。目的：文档与实现一致。

### 主要产出物

- PolicyEngine 接线 + 危险命令地板 + 调度器真实上下文；apply_patch 回滚补全 + checkpoint 版本化持久状态
- Windows env 分桶、edit_file 尾换行、list_directory symlink、run_command 真流式；阻塞 IO 改造
- 死依赖清理 + machete/udeps 维护检查（2026-08-10 起为文档记录项，工作流已移除）+ 匹配器属性测试

### 验收标准（保留 REVIEW 追踪编号）

- [x] **V1**：`PolicyEngine::decide()` 被调度器调用；未信任工作区 + `allowed_in_untrusted_workspace=false` → Deny（测试）
- [x] **V4**：trusted + NeverAsk 下 `rm -rf /` 等被硬拒绝/询问（用例）
- [x] **V2**：写工具的 checkpoint 挂在真实 run_id 下，跨 run 不碰撞（测试）
- [x] **V3**：apply_patch create-over-existing 不删原文件；update/delete 部分失败可恢复（三类回归测试）
- [x] **V5**：Windows env 透传含 SYSTEMROOT/TEMP/TMP/USERPROFILE/COMSPEC/PATHEXT（用例）
- [x] **V6**：模糊匹配末行编辑后文件保留结尾 `\n`（回归测试）
- [x] **V7**：含 dangling symlink 的目录可列出（broken symlink 降级，测试）
- [x] **V8**：run_command 长命令增量输出可见（流式测试）
- [x] **V9**：进程崩溃后 checkpoint 映射可由持久状态重建、可回滚（崩溃恢复测试）
- [x] **V11**：read_file/search_text/checkpoint 不在 async 中同步阻塞读整文件（审查/基准）
- [x] **V10**：search/find 遍历中可中途取消（测试）
- [x] **V12 / V13 / V14**：list_directory 分页省成本；edit_file 模糊匹配不退化 O(L·n)；spawn_stream 句柄可 kill 且流式有输出上限
- [x] **基线**：`content-inspector` 移出；4 个 crate 死依赖删除；L3 维护工作流含 machete/udeps（不阻塞普通开发提交；2026-08-10 起工作流移除，转为文档记录项，不在本项目配置自动执行 Actions）
- [x] **fuzz**：edit_file/apply_patch 有 proptest 属性测试（不 panic、计数一致、回滚逐字节相等）
- [x] **快速验证**：安全红线、Policy、checkpoint、路径与 patch 立即跑定向回归；workspace 全量 build/test/clippy 延后到 Core 主干 L2

### 验证记录（2026-08-09）

- `cargo test -p policy-engine -p tool-runtime -p builtin-tools -p checkpoint-service -p process-runtime`
- `cargo clippy -p policy-engine -p tool-runtime -p builtin-tools -p checkpoint-service -p process-runtime --all-targets -- -D warnings`
- 属性测试覆盖模糊匹配总函数/计数一致与 apply_patch 失败后逐字节恢复；checkpoint 重开与跨 Run 同 ToolCall ID 隔离均有回归测试。

**相关文档**：[REVIEW.md](../../REVIEW.md) §4 · [ADR-009 默认工作区信任](../../docs/adr/ADR-009-default-workspace-trust.md) · [ADR-010 全写 Checkpoint](../../docs/adr/ADR-010-checkpoint-all-writes.md) · [ROADMAP 依赖选型基线](../../ROADMAP.md#依赖选型基线)

> 跨任务协调（2026-08 review）：V2 上下文注入依赖 P3-11 V8 先落地；本任务负责调度器策略接线（V1）与 checkpoint 归属验证。`tool-runtime/scheduler.rs` 由两任务序列修改，不并行。
