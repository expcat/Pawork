# 全项目 Review 主报告（2026-09-17）

> 审查基线：`main / e87b0487` + 工作区在途改动（computer-use 三文件：rfb.rs / computer-use.md / ROADMAP.md，审查避让回写）。执行：主代理横切分析 + 8 个并行包簇审查代理（沿用同会话模型），覆盖 24 个包 × 六类声明（模块树 / 对外 API / `pawork-*` 依赖边 / feature 门 / 红线行为 / 测试资产），每包均有结论（含「无发现」）。本报告是当日审查的汇总与处置记录；处置状态截至 2026-09-17 修复会话结束。

## 1. 结论概览

| 级别 | 数量 | 处置 |
| --- | --- | --- |
| P0 红线 / 契约 | 2 | 全部已修复，含定向回归 |
| P1 spec 漂移 | 60+ | 全部以源码为准同批回写 |
| P2 冗余 / 低效 | 22 | 已修 13、部分修 3、登记 ROADMAP 4、明确不修 2 |
| P3 建议 | ~25 | 已处理约 10；其余登记 ROADMAP 或标注不修 / 观察 |
| 红线核查 | — | 全部通过，无违反 |

红线核查通过项：domain 纯净（生产依赖仅 serde / serde_json / thiserror / async-trait + optional ts-rs）；Secret 不落盘、不脱敏面外泄（`ResolvedCredential` 私有字段 + Debug `[REDACTED]` + 无 Serialize；bin 层 `RedactingFmtLayer` 字段名 + 值双通道脱敏）；Seatbelt（deny-default + 整盘读 allow + secret 挖洞，raw+canonical 双形态）/ Landlock 形态与 spec 一致；冻结契约锚在位（events_golden、migration v9→v14 分段 golden、`PolicyDecision` tag="kind"、shell 反引号 / 命令替换 danger+floor 双断言）；事件可持久化可重放（`session_events` `UNIQUE(session_id, sequence)` + `CHECK(sequence > 0)` + append-only 触发器）；protocol 三通道 registry 单源 + 穷举 match + fail-closed；desktop 生产 `pawork-*` 依赖恰为 {client, terminal, browser} 且 deny-list 断言在位；Agent Engine 无 Provider 名特判（守护测试在）。

## 2. P0（红线 / 契约）

### P0-1 orchestration merge 主路径越界防护未接线 —— 已修复

- 发现：`detect_conflicts` / `merge` 直接 `parent_path.join(file)`（`Path::join` 遇绝对路径整体替换 base、遇 `..` 可穿越），`resolve_relative` 校验被 `cfg(any(test, feature="git"))` 门控且默认关闭；`PatchMerger` 为公开 API。证据：merge.rs（原 :239/:301/:316）。可达性限制：app 未注入 PatchMerger 且 git feature 关闭，当时产品路径不可达。
- 修复（修复会话）：`resolve_relative` 改无条件编译，detect_conflicts / merge 循环内全部路径经其校验（merge.rs:143/217/240/303）；补越界定向回归。

### P0-2 spawn 的 TaskGraph 失败路径未注册 Failed —— 已修复

- 发现：spawn.rs `add_task / assign / start` 失败直接上抛，不发 `WorkerFailed`、不注册 WorkerEntry，children 边与 cancel_token 成悬挂项，lease Drop 按默认 Failed 惩罚账号健康；触发面为 task_deps 跨租户依赖，无测试覆盖。证据：spawn.rs（原 :480）。
- 修复：新增 `abort_spawn_as_failed`（spawn.rs:491）统一收口全部失败路径（含 TaskGraph 注册失败，释放 lease、worktree 与并发槽位），同时消除原三段同构失败注册块（P2-2）；补注册失败收口定向回归。

两项验证：`cargo test -p pawork-orchestration --offline --lib --tests` 87 通过。

## 3. P1 spec 漂移（全部已回写）

回写只改事实性表述（模块树 / API / 依赖边 / feature / 测试资产），不改 spec 结构与设计表述。按包计数：

| 包 | 条数 | 要点 |
| --- | --- | --- |
| tools | 9 | find_files 10s / search_text 15s / list_directory 128 KiB 实值；run_command descriptor 30s 与 exec 层超时的两层语义；`ApprovalOutcome` 无字段枚举实形；`GateOutcome`；审批流水线实际五步（requires_approval 非 Deny 即升级 AskUser、S2 钩子再问一次）；被依赖方仅 pawork-app；灾难地板测试归 policy |
| protocol | 12 | 42 命令 / 18 查询；GuiCapability 6（BrowserControl）；CONTROL_PLANE_SCHEMA_VERSION=2 与默认 scope 常量；QuotaUnit 4 形态；golden 93 与 projection 4 组；json_mapping 33 行 / 12 Some；§2 行数；补载 `project_server_tool_event`；删 storage dev-dep 残留 |
| storage | 8 | Actor 改 tokio mpsc 有界通道；`schema_migrations` 表名；ProjectionSnapshot 五字段；`get_session_identity` 返回 Option；compat 游标键 `updated_at_ms`；`SessionStoreError` 补 6 变体；删 PEM 扫描表述；统计口径 |
| desktop | 13 | 能力面五项（BrowserControl）与 minor gate；deny-list 三件套；settings 账号 / 配额命令面；theme 数值（880 / 30）、timeline 行距、tool group 默认折叠、inspector §4.8 按终端修订重写、MenuKind 八值、§4 补 7 文件条目、§7 测试计数 |
| app | 4 | 35,860 行 / 70 文件；`let _` 非测试 4 处白名单化；文件地图刷新；AppError 46 变体 |
| cli / client | 6+2 | API 版本 1.19 与 1.0–1.19；ACP 身份表述；headless API 清单补全（SdkErrorKind::Protocol、PaworkOptions 四字段、re-export 9 项）；行数。21 子命令与 ACP 四件套核实无漂移 |
| providers / control-plane | 5 | providers 28 文件 12.7k；`LegacyCredentialPicker` 恒 default；未知租户返回 `TenantPolicy::default()`；QuotaUnit 补 Percent；测试 205 |
| orchestration / workflow | 3 | PlanError 14 变体；「BFS」改「栈式深度优先」（两包同形，集合等价）；SupervisorError 11 变体 |
| auth / policy / git / domain / terminal | 各 1–2 | auth §2 accounts 行入表 + 总量 6.0k；shell.rs ~1.3k；`with_call_count` 测试钩子；CanonicalModelRequest 补 `session_id`（ADR-057）；terminal 行数与按键映射 |

回写执行：审查线程回写 16 篇（tools / protocol / providers / control-plane / workflow / orchestration / domain / terminal / storage / workspace / app / cli / client / auth / policy / git）；desktop.md 由修复会话补写；修复会话并对代码改动涉及的包做二次同步（行数、测试计数、行为描述跟随修复后源码）。

## 4. P2（冗余 / 低效）与处置

| # | 发现 | 证据 | 处置 |
| --- | --- | --- | --- |
| 1 | merge 三主路径越界未校验 | orchestration merge.rs | 已修（P0-1） |
| 2 | spawn 三段同构失败注册块 | spawn.rs | 已修（`abort_spawn_as_failed`，P0-2） |
| 3 | `optional_usage` / `last_stream_usage` 双份私有实现 | engine tool_loop/mod.rs、session_turn.rs | 已修（合一） |
| 4 | 截断估算 O(n²) + truncate 后双重重算 | engine compaction.rs | 已修（一次计数 O(n)） |
| 5 | `events_on_lineage` 全量物化后内存过滤 | storage session_tree.rs | 已修（lineage 谓词下推 SQL 游标） |
| 6 | import「流式」注释与 `read_to_string` 实现不符 | storage persist_compat.rs | 已修（删伪流式表述；真流式登记 R-08） |
| 7 | `merge_with` 不合并 naming 对 | workspace config/schema.rs | 已修（补 naming_provider / naming_model） |
| 8 | workspace 信任语义双入口分叉 | app query.rs vs app_core.rs | 已修（workspace_list 逐 root 判定） |
| 9 | files.rs 在 async 内做阻塞 std::fs | app handlers/files.rs | 已修（read / write 走 spawn_blocking；SAVE_LOCK 保留为 revision-CAS 串行机制） |
| 10 | ACP 客户端细粒度身份成死数据 | cli adapter.rs | 已修（合法 `acp:<client>` 身份保留，伪装形态回退重盖） |
| 11 | `check_gate` 每次整体 `request.input.clone()` | tools scheduler.rs | 已修（`mem::take` 转移） |
| 12 | 「六通道」注释滞后 | providers lib/error_table/channels、app lib/provider_assembly | 已修（八通道口径） |
| 13 | 未使用依赖：auth/async-trait、policy/regex、storage/proptest、workflow tokio rt | 各 Cargo.toml | 已修（移除 / 收窄为 sync） |
| 14 | Timeline 热路径每帧全量重算 + Markdown 同帧最多 3 次 parse | desktop timeline.rs、markdown.rs | 部分修：MessageMeasure 一次 parse、timeline_rows 同帧复用、render_item 四分支 wrapper 合一；render 路径仍每帧 parse，跨帧缓存登记 R-03 |
| 15 | 异步上下文同步 IO 不一致 | tools list_directory / edit_file / write_file / apply_patch | 部分修：list_directory 已 spawn_blocking；其余三件登记 R-02 |
| 16 | 会话解析样板约 6 处 | cli plan / vcs / usage / sessions / chat | 部分修：plan / vcs / sessions 已收敛 `resolve_session`；chat 与 usage 登记 R-04 |
| 17 | 分发表 44 个 `Box::pin` 包装函数（约 330 行） | app gui_host/mod.rs:687-1094 | 登记 R-01 |
| 18 | quota dead_code 抑制面积偏大 | control-plane quota/util.rs、quota/error.rs | 登记 R-05 |
| 19 | EventHint 双轨映射（仅自测引用，与手工 emit 并存） | orchestration lifecycle.rs:190-197 | 登记 R-06 |
| 20 | `run_session` 每轮 `current.clone()` | engine tool_loop/mod.rs:228 | 登记 R-07（受 provider.stream 按值接口约束） |
| 21 | artifact / protected 平行脚手架 | storage artifact.rs、protected.rs | 不修：已知设计取舍，演进时防第三套 |
| 22 | 包内两套原子写 | workspace config/writer.rs、import/io.rs | 不修：错误类型与目标不同，可接受重复 |

## 5. P3（建议）与处置

已处理：providers / app 「六通道」注释；client 文档注释腐化（`API_VERSION = 1.2` 写死、Artifact 分片宣称）；transport remote feature 注释；orchestration budget 注释（`> limit` → `>= limit`，与实现及 spec 对齐）；workflow tokio feature 收窄；auth spec §2 表格结构；各包行数 / 测试计数（随 P1 回写）。

登记 ROADMAP（建议级，随邻近任务顺带处理）：

- desktop input_area.rs:159 truncate div 自带 `min_w_0`（工程经验警示形态，移除后需真窗口复验）。
- desktop 能力面测试改钉五项全集（controller/mod.rs:1844 现仅断言 TerminalStreaming）。
- desktop button.rs `disabled_text_color` 与 tools mcp/transport.rs 两个 HEAD 既有 dead-code 警告。
- tools `opt_str` / `opt_u64` / `opt_bool` 类型不符静默视为 None（common.rs:116），改 InvalidField。
- providers `require_bearer_credential`（xai.rs:210 / kimi.rs:157 逐字重复）提炼；xai.rs:152 `list_models` 循环内 `builtin_models()` 外提。
- app query 不支持文案统一（gui_host/mod.rs:533 残留波段名）；provider_quota.rs:62 `!= "opencode-go"` 特判改通道能力驱动；裸实例计数器（loop_ctx.rs:183、services/run.rs:301；单库单宿主拓扑下安全，改拓扑前必须处理）。
- client `FrameWant::Snapshot` 匹配任意快照帧（快照幂等影响低，并发 snapshot 会互取）。
- transport `unpublish` / `revoke` 语义重叠（api.rs:166-176），复活远程实现时合并或明确分工。
- desktop quick_search.rs 与 timeline_navigation.rs 平行模态输入骨架提炼；approval_card.rs 每帧小分配（轻微）。

不修 / 保持观察：storage artifact / protected 平行脚手架与 workspace 两套原子写（见 §4）；providers QuotaClock / LeaseClock 平行时钟（spec 声明有意独立）；desktop i18n 反向 import（projection → ui::i18n，若 i18n 引入 gpui 依赖将击穿投影纯度）；transport local / memory 双份约 12 行帮手；domain 事件变体穷举 match 样板（契约钉死的刻意设计）。

## 6. 横切分析（主代理）

- 依赖边：`cargo metadata` 全量核对 vs architecture §2，无环；desktop 直连白名单断言在位；orchestration `git` feature 默认关闭、app `default-features=false` 核实。
- 幂等键审计：两个持久去重账本键均安全——usage 用 `run_request_id`（pid + 纳秒 + 进程级 static，带跨重启回归 services/run.rs:401）；command ledger 用客户端 command_id + scope。裸实例计数器仅存于 engine 事件流身份（见 §5）。
- 文档间一致性：「六通道 → 八通道」口径已修；architecture ↔ spec/README ↔ 包级 spec 数量口径同步（24 成员）。
- 镜像总篇：README 由 21 包口径刷新到 24 包（约 25.2 万行），依赖图与分层表重生成。

## 7. 镜像刷新摘要

`docs/review/` 由 2026-09-07 的 21 包口径刷新到 24 包现状：backend 19 篇更新 + 新建 browser / computer-use / terminal 3 篇，frontend 4 篇更新，README 索引重统计。修复会话对代码改动涉及的镜像页（orchestration / engine / app / cli / storage / tools / workspace / views 等）按修复后源码再同步一轮。

## 8. 修复会话验证（2026-09-17）

- 定向测试全绿：pawork-cli 89、pawork-engine 70、pawork-orchestration 87（含两项 P0 新回归）、pawork-storage 166、pawork-tools 139；pawork-app 266、pawork-client 46；pawork-desktop 268。
- `cargo check -p pawork --offline` 通过；`git diff --check` 干净。
- 真窗口冒烟（隔离实例 reviewfix，`opencode-go / glm-5.3-flash` 两轮真实 Run）：Markdown 标题 / 列表 / 代码块渲染完整，溢出后滚轮脱钩与回底按钮正常。
- 环境备注：本机 `/usr/bin/git` 因 Xcode license 不可用，git 操作使用 fallback 二进制；cargo 全程单进程 `--offline`。

## 9. 后续项

全部待处理项登记于 [ROADMAP](../ROADMAP.md)「全仓库 Review 后续项（2026-09-17）」（R-01～R-08 + 建议级清单），完成一项即从 ROADMAP 移除。

```text
Validated: 审查为只读静态核查（rg / find / wc / 只读 git）；修复验证见 §8
Targeted regressions: P0-1 越界、P0-2 失败注册收口两项新回归随 orchestration 包测试通过
Full workspace gate: NOT RUN（当前未设置全量门禁）
```
