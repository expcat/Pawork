# S12 CR-06 审查报告：Engine、Workflow 与编排逻辑

| 项 | 值 |
| --- | --- |
| CR 编号 | CR-06 |
| 主审范围 | engine/engine、foundation/api、workflow/core、workflow/memory、workflow/review、agents/orchestration、foundation/testkit（含 tests；该 crate 当前无独立 tests/ 目录） |
| 审查日期 | 2026-08-18 |
| 主审模型 | GLM（zai/glm-5.3） |

## 实际审查路径

- engine/engine/src/：tool_loop.rs、appender.rs、event.rs、session_turn.rs、cancel.rs、lib.rs，以及 context/{mod,budget,compaction,token,tool_result_trim}.rs 全文；engine/engine/tests/no_provider_branch.rs。
- foundation/api/src/：lib.rs、tool.rs（AgentTool、ToolResult、ToolStreamEvent 契约）。
- workflow/core/src/：lib.rs；plan/{service,state,snapshot,error,mod}.rs；goal/{service,state,snapshot,error,mod}.rs；task/{manager,state,error,mod}.rs；automation/{engine,state,inbox,cron,automation,dispatcher,error,mod}.rs。
- workflow/memory/src/：service.rs、store.rs、model.rs、similarity.rs、extract.rs、error.rs、lib.rs。
- workflow/review/src/：anchor.rs、engine.rs、aggregate.rs、model.rs、error.rs、lib.rs。
- agents/orchestration/src/：supervisor/{mod,spawn,cancel_tree,recovery,budget_gate,registry}.rs、lifecycle.rs、task_graph.rs、budget.rs；teams/{approval,service}.rs 的公开服务面与状态边界；lib.rs。
- foundation/testkit/src/{lib,contract}.rs：按任务书采样核对 MockProvider / MockTool / contract assertion 与取消、失败脚本路径。
- 跨包调用面核对：host/app/src/loop_ctx.rs、host/app/src/plan_host.rs、host/app/src/lib.rs（turn_context）、host/app/src/orchestration_host.rs、execution/tools/src/run_command.rs、providers/adapters/src/responses.rs 与 providers/adapters/src/anthropic/provider.rs（Provider Error 后续是否返回 Err）。
- 基线与契约：plan/S12-project-code-review.md、ROADMAP.md §3.2 K-01～K-10、docs/task-guide.md §3.1、docs/design.md §3.2；另对照已落地的 CR-01/CR-03/CR-05 报告，避免跨包重复登记。

## 未覆盖路径与原因

- workflow/core/src/monitor/*：不在本轮主审核心问题（五合一 reducer 以 plan/goal/task/automation/inbox 为主），且时间预算优先覆盖已接宿主与事件折叠路径。
- workflow/review/src/{forge,patch}.rs 与 model.rs 深层实现：Forge 真实平台 adapter 未接宿主（ROADMAP §4 已登记激活条件），本轮重点为 re-anchor / resolution 生命周期。
- agents/orchestration/src/teams/ 的 mailbox.rs、presence.rs、peer.rs、store.rs、task_board.rs、state.rs、event.rs 内部，以及 merge.rs、worktree.rs、identity.rs、budget.rs 除终态 flush / budget-gate 关键路径外的内部实现：S11 明确 teams / 真实双子 run_session 未接生产闭环，本轮只审公开服务面与 Supervisor 集成。
- engine / api / workflow / orchestration 的测试未逐条全审；仅针对 finding 与状态机、取消、审批、重放行为读取相关测试。
- foundation/testkit 按任务书采样，未逐行审查所有断言辅助。
- 并发竞态、对抗性路径与 symlink/TOCTOU 的完整安全边界属 Grok / CR-02 主审范围；本报告只登记 review anchor 的确定性 lexical 校验缺口，不展开 openat/TOCTOU 设计。

## Findings

### S12-CR06-01 — Tool artifacts 在 engine 数据面被丢弃

- **类别**：Bug
- **严重度**：Medium　**置信度**：Confirmed
- **证据**：
  - foundation/api/src/tool.rs:49-64：ToolResult 明确携带 artifacts: Vec&lt;ArtifactReference&gt;。
  - engine/engine/src/appender.rs:193-205：tool_results_message() 构建 canonical ToolResultContent 时只保留 content / is_error / metadata，丢弃 result.result.artifacts。
  - engine/engine/src/tool_loop.rs:323-338、engine/engine/src/tool_loop.rs:941-948：ToolExecutionCompleted 事件与后续 tool message 均经 tool_result_content()，同样没有 artifact 字段。
  - engine/engine/src/event.rs:112-134：LoopEventEmitter::emit_tool_event() 对 ToolStreamEvent::ArtifactAvailable(_) 直接 Ok(())，不映射为任何 AgentEvent；foundation/api/src/tool.rs:94-109 定义了该事件但 engine 不消费。
  - **实际行为**：Core 工具返回的 artifact 引用既不进入 canonical tool result / message，也不进入可持久化 AgentEvent 流；运行期流式 artifact 通知被静默丢弃。
  - **期望行为**：ToolResult.artifacts 或 ArtifactAvailable 至少有一条可持久化、可重放的承载路径，满足工具契约与事件化语义。
  - **影响面**：artifact 契约在 engine 层断链，后续 GUI / replay / 大输出回溯无法从事件账本恢复引用。它与 K-08 的 GUI ArtifactStreaming 能力声明问题相邻，但根因在 engine 数据面而非 protocol capability 声明，不能由 K-08 修复顺带解决。
- **验证建议**（S12 内不执行）：用 MockTool 返回非空 artifacts 并同步发送 ArtifactAvailable，golden 断言事件流与 tool message 中至少一者保留引用，且 replay 后仍可取得。
- **整改边界**：先拍板 canonical 承载方式（扩展 ToolResultContent / ToolExecutionCompleted，或新增 AgentEvent 变体）并先改 golden；最小写入集为 domain/api 契约 + engine 映射 + 对应测试。不可顺带修 K-08 的 GUI capability 声明或 transport 行为。

### S12-CR06-02 — 审批闸门数组短缺时默认 NotRequired 并继续执行

- **类别**：Bug
- **严重度**：Medium　**置信度**：Confirmed
- **证据**：
  - engine/engine/src/tool_loop.rs:65-85：LoopContext::request_approval() 只约定返回 Vec&lt;ApprovalGate&gt;，未约定长度、缺失或错位语义。
  - engine/engine/src/tool_loop.rs:248-255：engine 直接把返回值交给 apply_approval_gates()。
  - engine/engine/src/tool_loop.rs:850-866：gates.get(index).cloned().unwrap_or(ApprovalGate::NotRequired)；缺失 gate 的 invocation 会被 push 进 to_run。
  - host/app/src/loop_ctx.rs:112-168：当前生产 host 逐 call push 一个 gate，长度正确；问题只在自定义 / 未来 LoopContext 或实现回归时触发。
  - **实际行为**：审批回调返回短数组时，engine 对缺失项 fail-open，按无需审批执行。
  - **期望行为**：gate 数量或 call 对齐失败应 fail-closed，例如 engine 校验 gates.len() == invocations.len() 并终止 run / 返回错误，而不是默认放行。
  - **影响面**：当前生产装配未触发，但这是审批契约的潜在绕过面；engine 测试替身或替代宿主一旦返回不完整 gates，就会无声越过审批。
- **验证建议**：新增 engine 定向测试——一个需审批 call 的 request_approval() 返回空数组，期望不执行工具并以错误 / denied result 收束。
- **整改边界**：最小修复在 engine 对 gate 数量与 invocation 对齐做校验；不改 LoopContext 的策略语义，不顺带处理 K-02 的 ToolApprovalRequested 等待前持久化时序。

### S12-CR06-03 — Plan Revised 事件无法携带修订后的内容

- **类别**：Requirement Gap
- **严重度**：Medium　**置信度**：Confirmed
- **证据**：
  - foundation/domain/src/workflow.rs:93-100：PlanEvent::Replaced 携带新 title / steps。
  - foundation/domain/src/workflow.rs:106-111：Revised 只有 plan_id / version / parent_version，没有内容字段。
  - workflow/core/src/plan/service.rs:228-261：revise() 只接受两个版本 ID；事件由调用方提供新 version，只校验 parent 是当前版本且新 ID 不同。
  - workflow/core/src/plan/state.rs:182-200：折叠 Revised 时把旧 state 的 title / steps 原样复制进新版本 history。
  - host/app/src/plan_host.rs:18-128：host 只暴露 create / replace / submit / approve / reject，没有 revise 入口；全仓 revise() 的非测试调用也仅落在 workflow/core 库层。
  - **实际行为**：changes_requested 后的修订链会创建一个指向 parent 的“新版本”，但内容与 parent 完全相同；caller-supplied version 还缺少与历史版本唯一性的完整校验。
  - **期望行为**：修订事件应能表达修改后的 title / steps，或明确由带内容的 Replaced 组合实现修订，并移除 / 收窄 hollow revise() API。
  - **影响面**：库层评审修订流语义不完整，replay 后无法恢复实际修订内容；后续接 GUI / CLI 评审工作流时会把空洞版本当成有效审批对象。
- **验证建议**：补 plan service + replay golden：请求修订时提供新内容，断言新版本 history、current snapshot 与重放结果一致；同时覆盖重复 version ID 被拒绝。
- **整改边界**：涉及 canonical PlanEvent 形状或 service API，需先做契约设计并 golden 先行；不可只改 reducer 复制逻辑，也不顺带改 host 的 Draft→InReview 自动提交产品行为。

### S12-CR06-04 — MemoryService replay 后新 ID 从 0 重发并覆盖历史记忆

- **类别**：Bug
- **严重度**：Medium　**置信度**：Confirmed
- **证据**：
  - workflow/memory/src/service.rs:17-32：next_id 从 0 初始化。
  - workflow/memory/src/service.rs:51-54：alloc_id() 生成 mem-{n}。
  - workflow/memory/src/service.rs:159-162：apply() 只委托 store.apply(event)，不根据 replay 的 memory_id 推进 next_id。
  - workflow/memory/src/store.rs:41-70：Recorded 以 memory_id 为 key insert，重复 ID 直接覆盖旧记忆。
  - **实际行为**：重放 mem-0 等历史事件后，下一次 record() 仍从 mem-0 分配并覆盖历史记录。
  - **期望行为**：事件重放后新写入 ID 不与历史 ID 碰撞；应从事件流派生 max / 下一 ID，或提供显式 from_events() 构造入口。
  - **影响面**：memory crate 当前未接生产宿主，但一旦激活，事件重建后的继续写入会静默破坏旧记忆，违反事件折叠后续写安全性。
- **验证建议**：定向测试先 apply Recorded { memory_id: "mem-7", .. }，再 record()，断言新 ID 不是 mem-0 且 mem-7 未被覆盖。
- **整改边界**：最小修复收敛在 workflow/memory/src/service.rs（apply 后推进计数或新增 from_events）；不得引入具体 Provider 名称 / 依赖，不改 embedding 契约。

### S12-CR06-05 — Automation record_result() 不幂等，重复上报污染事件与失败退避

- **类别**：Bug
- **严重度**：Medium　**置信度**：Confirmed
- **证据**：
  - workflow/core/src/automation/engine.rs:222-244：只校验 automation 存在、曾触发且该 task 曾被本 automation 触发，不检查该 task 是否已有归档结果。
  - workflow/core/src/automation/engine.rs:246-263：每次调用都构造并 apply 新 ResultArchived，再写 inbox。
  - workflow/core/src/automation/state.rs:76-85：每个 ResultArchived 都 append 到 archived，没有去重。
  - workflow/core/src/automation/inbox.rs:61-69：inbox 对同 (automation_id, task_id) 覆盖，是唯一幂等的视图。
  - workflow/core/src/automation/engine.rs:265-293：重复 Failed 结果还会重复累计 failure streak，可能提前触发 Suspended 并清空 next_at。
  - **实际行为**：同一 task 的结果重复记录会生成重复 canonical event / archived entry，并放大失败退避状态。
  - **期望行为**：同一 (automation_id, task_id) 的结果记录应 no-op、更新既有记录，或显式拒绝重复；重放语义需一致。
  - **影响面**：崩溃重试、GUI 双击或调用方重复上报会污染事件账本，并可能让 automation 被错误挂起。
- **验证建议**：对同一 triggered task 连续两次 record_result(..., Failed, ..)，断言第二次不新增事件 / archived entry，failure streak 不重复累计；再补 replay golden。
- **整改边界**：先定义幂等键与 canonical 字段（当前 ResultArchived 不含 task/status，可能需要契约决策）；最小修复限于 automation engine/state 与对应 golden，不重构 scheduler / cron。

### S12-CR06-06 — Supervisor spawn 接受不存在或不匹配的 parent_id

- **类别**：Bug
- **严重度**：Medium　**置信度**：Confirmed
- **证据**：
  - agents/orchestration/src/supervisor/spawn.rs:24-43：SpawnRequest.parent_id 完全由调用方提供。
  - agents/orchestration/src/supervisor/spawn.rs:86-110、agents/orchestration/src/supervisor/spawn.rs:617-633：深度计算沿 workers map 走 parent 链，但不存在时只是停止遍历，不报错；随机 parent 会被当作 depth 1。
  - agents/orchestration/src/supervisor/spawn.rs:223-238：不校验 parent 存在 / 同 tenant / 同 session / 状态可派生，直接以该 parent 创建 worker 实例。
  - agents/orchestration/src/supervisor/spawn.rs:433-445：无论 parent 是否在 workers 中，都写入 children[parent]；agents/orchestration/src/supervisor/mod.rs:119-145 显示 cancel-tree 依赖 workers / children / cancel_tokens 这些内部状态。
  - **实际行为**：不存在、跨租户或跨 session 的 parent 可以创建孤儿 child；取消真实 parent 时该 child 不在可达树中，depth limit 也可被无效 parent 链绕过。
  - **期望行为**：spawn 准入阶段校验 parent 存在、与本次请求 tenant/session 一致，且 parent 状态允许派生；失败返回 PolicyDenied 或专用错误。
  - **影响面**：编排 cancel-tree、层级深度限制与父子关系审计可被无效 parent 破坏。当前 demo 装配使用已知 parent，生产 API 一旦暴露即触发。
- **验证建议**：分别用不存在 parent、同 supervisor 不同 tenant/session parent 调 spawn()，断言均被拒绝且不写 children / workers。
- **整改边界**：最小修复在 spawn 前置校验与错误返回；不重构并发预约 / lease / worktree 流程，不处理 cancel-tree 的并发竞态（属对抗审查范围）。

### S12-CR06-07 — recover() 只生成报告，不重建 Supervisor 可操作状态

- **类别**：Requirement Gap
- **严重度**：Medium　**置信度**：Confirmed
- **证据**：
  - agents/orchestration/src/supervisor/mod.rs:119-145：Supervisor 的可操作状态在 workers / cancel_tokens / children / event_log / budget 等字段。
  - agents/orchestration/src/supervisor/recovery.rs:20-42：recover() 只调用 replay_workers(events) 得到局部 states，计算 RecoveryReport 后返回；不写回任何 supervisor 字段，也不 emit WorkerFailed 或持久化恢复事件。
  - agents/orchestration/src/supervisor/mod.rs:910-932：现有测试只断言 report 内容，不检查 recover() 后 supervisor 自身可列出 / 取消 / flush 的状态。
  - plan/S11-workflow-control.md:42：验收声明覆盖 recovery 行为等价，但当前入口无法把等价状态交回 Supervisor。
  - **实际行为**：调用 recover(events) 后 Supervisor 仍是空状态；报告只能诊断，不能作为继续 cancel / assign / usage flush 的恢复态。
  - **期望行为**：若文档与函数名声称“重放事件重建状态”，应重建最小 WorkerEntry、children、cancel token 等可操作状态并事件化孤儿 Fail；若产品决策只需诊断，应改名为 report-only 并修正文档与测试口径。
  - **影响面**：崩溃恢复入口语义漂移。当前无生产 caller，但 S11 库层恢复契约已经不可直接使用。
- **验证建议**：恢复含 active worker 的 events 后，断言 supervisor registry / cancel-tree / event log 能看到对应终态 worker，孤儿 worker 有 WorkerFailed 事实。
- **整改边界**：先拍板恢复语义；若选择重建，写入集限于 supervisor recovery/registry 相关状态与测试，不顺带改 teams、lease 或 worktree 恢复。

### S12-CR06-08 — Tool result 分级裁剪已迁移但零生产消费者

- **类别**：Requirement Gap / Performance
- **严重度**：Low　**置信度**：Confirmed
- **证据**：
  - engine/engine/src/context/tool_result_trim.rs:1-10：模块自述用于避免超大 tool 输出进入上下文，并把完整输出折叠到 ArtifactReference。
  - engine/engine/src/context/tool_result_trim.rs:182-267：完整实现小 / 中 / 大 / 超大四级裁剪与 retained full payload。
  - engine/engine/src/context/mod.rs:21-25：公开 re-export；全仓对 trim_tool_result / trim_tool_result_with 的调用点索引只命中该模块定义与自身单元测试，无 engine/host 生产调用。
  - host/app/src/lib.rs:1388-1421：生产宿主已启用通用 context budget、80% 软限压缩与硬限截断；execution/tools/src/run_command.rs:35-37、:184-186 另有默认 8 MiB 输出上限。
  - **实际行为**：专门的 tool result 分级裁剪没有接入生产热路径；大输出只能依赖整体消息截断或 run_command 的字节上限。
  - **期望行为**：按「无消费者不合入」纪律，该能力应接线到 engine/host 生产路径，或显式 feature gate / ROADMAP 登记激活条件，不能作为静默库存保留。
  - **影响面**：丢失设计的 head/tail 摘要与 artifact 引用能力，长期大输出上下文质量下降；由于通用预算路径存在，实际风险低于核心功能缺口。
- **验证建议**：整改后用超大 MockTool 输出断言分级裁剪在进入 provider request 前生效、artifact 引用可持久化；同时保留全仓调用点检查。
- **整改边界**：先决定接线位置（engine 或 host），注意 engine 不得直接依赖 blob store；完整原文写 blob 与 artifact 恢复应与 S12-CR06-01 的契约决策协同，但不顺带修 K-08 GUI 声明。

### S12-CR06-09 — Review anchor 的 lexical 路径校验可被 symlink 逃出 workspace

- **类别**：Security
- **严重度**：Low　**置信度**：Confirmed
- **证据**：
  - workflow/review/src/anchor.rs:94-114：safe_path() 只拒绝绝对路径与 .. 等 lexical component，然后 root.join(path)。
  - workflow/review/src/anchor.rs:123-126：resolve() 用 fs::read_to_string()，会跟随 workspace 内 symlink。
  - workflow/review/src/anchor.rs:184-194：reanchor() 同样直接读取该路径。
  - **实际行为**：workspace 内符号链接可让 anchor 指向 root 外文件；行数校验与 fingerprint 会基于外部文件计算。
  - **期望行为**：与文件工具的 workspace 边界一致，拒绝 symlink 逃逸。
  - **影响面**：review anchor 是只读面，当前暴露主要是外部文件存在性 / 行数 / 指纹 oracle，不直接返回文件内容，因此降为 Low。
- **验证建议**：临时 workspace 内创建指向外部文件的 symlink，resolve() / reanchor() 应返回路径拒绝而非读取成功。
- **整改边界**：最小修复在 AnchorResolver::safe_path() 做 canonicalize + root 前缀校验；完整 openat / TOCTOU 强化属 CR-02，不在本 finding 顺带处理。

### S12-CR06-10 — engine Provider 无特例回归的禁用名单过期

- **类别**：Maintainability
- **严重度**：Low　**置信度**：Confirmed
- **证据**：
  - engine/engine/tests/no_provider_branch.rs:9-21：禁用名单仅含 openai / anthropic / claude / google / gemini / bedrock / mistral / azure / ollama / vllm。
  - providers/adapters/Cargo.toml:10-18：当前首发与相关通道包括 chatgpt-oauth、xai-oauth、glm-coding、opencode-go、qwen-token-plan、deepseek；源码与配置中也存在 glm-coding、qwen、deepseek、xai 等名称。
  - workflow/memory/src/service.rs:386-400：memory 的同类红线测试已覆盖 anthropic / claude / openai / zhipu / glm / moonshot / kimi / qwen / tongyi / deepseek / grok / xai / gemini / google。
  - **实际行为**：engine 生产源码当前未发现 Provider 名称分支，但红线测试名单落后于真实 Provider 集，不能发现新增厂商名特例。
  - **期望行为**：禁用名单与当前 Provider / 通道集同步，或抽出共享测试清单，避免各 crate 手工漂移。
  - **影响面**：仅测试守护能力，不构成当前生产违约；一旦未来 engine 引入 glm、qwen、deepseek、xai 等字符串分支，现有回归不会失败。
- **验证建议**：更新名单或共享 helper 后运行该测试（S12 不执行），并故意注入一个新 Provider 名验证测试会失败。
- **整改边界**：只改测试清单 / 共享测试辅助，不改 engine 生产代码，不扩大为全仓 lint 任务。

## 统计

| 严重度 | 条数 |
| --- | --- |
| Critical | 0 |
| High | 0 |
| Medium | 7 |
| Low | 3 |

| 置信度 | 条数 |
| --- | --- |
| Confirmed | 10 |
| Needs Verification | 0 |

已知基线引用：K-02（审批请求等待前持久化 / 崩溃 resume）与 K-08（GUI ArtifactStreaming 能力声明不一致）均不重复登记；本报告的 engine artifact 数据面断链、审批 gate 数量 fail-open 是独立根因。
