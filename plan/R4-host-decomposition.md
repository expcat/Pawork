# R4 — 宿主拆解与可靠性内核(T2 + T8 + T9)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R4 行。根因:V2 增量顺序把每阶段能力都挂在 `AppCore` 一个结构体上,形成 `host/app/src/lib.rs` 4,057 行 + `gui_host.rs` 2,594 行单体;并发/幂等问题就地打点(内存 CAS、9 张 Mutex map、序列补洞);降级路径静默吞错(全仓 323 处 `let _`/`.ok()`)。本阶段在 R3 的 registry 与协议 golden 护航下拆解宿主,并建立幂等持久化与降级可观测两个可靠性内核。

## 1. 现状证据(执行时重验;路径为 R1 合并后位置)

- **单体**:AppCore 承载 resume/compact/usage/checkpoint/task/approval/idempotency 全部;`CatalogOnlyProvider` 兜底假 provider(原 `host/app/src/lib.rs:265`);`RETAINED_MESSAGES` 等横切常量散置。
- **幂等**(波 B 已落地):`command_ledger` SQLite v11 表,作用域 `(tenant, client_scope, command_id)`;进程内仅 Notify 唤醒。历史路径:`idempotency.rs` 曾为内存 CAS;`gui_host.rs:930`/`gui_host/mod.rs:447` 曾 `let _ = record(...)` 吞错。
- **吞错热点**:`lib.rs:1325,1334` `let _ = tasks_finish(...)`;`data_dir.rs:22-26` HOME 缺失静默回退 `temp_dir()`(会话库落临时目录无告警);gui-server 断连清理 20+ 处零观测。
- **ACP host**:40 处 `.expect("…mutex")`(毒锁 panic 整通道)、`prompt_gate` 全局串行锁、9 张独立 Mutex map、Reserved/Active 手搓状态机(R1 后位于 cli `channels/acp/`)。
- **usage 哨兵**:`control.rs:150-176` `upstream_attempt: Some(1)`/`trace_id: None` 硬填(D1 单机决议后收敛语义)。
- **K-02**(波 B 已落地):等待前落盘 `ToolApprovalRequested`;GUI resume 呈现待审批、决策落盘不重跑;CLI resume 维持 seal Denied。

## 2. 目标设计

1. **领域服务拆分**:AppCore → `SessionService` / `RunService` / `ApprovalService` / `UsageService` / `TaskService` / `ImportService` / `ExtensionService`(MCP/resources),每服务自持状态与横切常量;`gui_host.rs` 巨 match 改 R3 registry 分发,目标 `lib.rs` <1,500 行、`gui_host.rs` <800 行。`CatalogOnlyProvider` 兜底改显式「无凭证」状态(配合降级事件)。
2. **幂等持久化(CommandLedger)**(波 B 已落地):幂等表入 SQLite(与会话库同 Actor 栈,storage v11 `command_ledger`——新表不动既有 DDL);作用域 `(tenant, client_scope, command_id)`;重启后可查;record 失败不再吞错。**K-02 并入**:`ToolApprovalRequested` 在进入等待前持久化;GUI 崩溃后 resume 呈现待审批、决策不重复执行(CLI 维持 seal Denied)。定向回归:审批中 drop+reopen → resume keep-pending → deny → 工具未执行。
3. **ACP actor 化**:单 actor 循环 + 消息信箱替换 9 张 Mutex map;`expect` 清零(错误进降级事件);prompt 串行语义由 actor 队列天然保证。
4. **降级可观测契约(T8)**:定义 `DegradeEvent`(或复用 Diagnostic 通道):HOME→temp 回退、无凭证兜底、Lagged 断流、tasks_finish 失败、幂等冲突等一律事件化(进事件流或 stderr 诊断,按敏感度分级);建立「副作用 Result 禁 `let _`」清单——本阶段清理 host 域全部命中点,其余包登记到 R9 复查。

## 2.1 波 A 实态进度(2026-08-21,波 A 已收口)

- 阶段1(2026-08-21 早):抽出 `UsageService` / `TaskService` / `ImportService` / `ExtensionService`,AppCore 对外 pub API 形状不变(门面委托);测试随迁 12 条 + 共享 `testsupport.rs`。
- 阶段2(2026-08-21,glm_worker 单 owner):Session/Run/Approval 三服务抽取(`services/{session,run,approval}.rs`)+ provider 装配移 `provider_assembly.rs` + lib.rs 33 条内联测试随迁;`gui_host.rs`(2407)目录化为 `gui_host/`(mod.rs 679 + bus/events/handlers/tests),巨 match 改 `QUERY_HANDLERS` 7 / `COMMAND_HANDLERS` 10 静态分发表(wire 名查表,与 protocol registry `gui.available` 双射,新 pin 测试锁定),幂等 wrap 留在分发前,fallback 文案逐字保留。
- 行数:`lib.rs` 4131→1413(<1500 ✅);`gui_host/mod.rs` 679(<800 ✅)。
- 验证:`cargo test -p pawork-app` 122 绿(1 ignored;+1 为授权 pin 测试);protocol golden / domain events_golden / typegen / client probe+spawn_e2e / desktop 27 / `cargo check -p pawork` 全绿;26 帧 golden 零 diff。
- 审查(glm_reviewer)verdict=pass;P2-1 状态文档滞后与 P2-2 搬迁丢 1 条 doc 注释 + 若干反引号均同波闭环(主代理补齐 12 处)。
- 留后续波:CatalogOnlyProvider 显式「无凭证」状态配合 DegradeEvent(波 C/D,T8);`let _` 清理(波 D)。K-02 已由波 B 落地。
- 环境备注:worker 首跑 desktop 与 `cargo check -p pawork` 遇 idle-rustc 环境异常(挂起/被终止),主代理串行重跑均绿,非源码问题。

## 2.2 波 B 实态进度(2026-08-21,波 B 已收口)

- CommandLedger 持久化:storage 追加 v11 `command_ledger` 迁移(纯新增表 + 部分唯一索引,不动 v1–v10 DDL;`CURRENT_SCHEMA_VERSION` 10→11);`SessionStore::open` 迁移后 reclaim 残留 inflight(`open_read_only` 不动);check 的 SELECT+INSERT 在单次 actor call 内原子;record 冲突映射 `DuplicateCommand`/`KeyConflict`;容量淘汰全局 4096(与内存前身一致,注释 + 跨 tenant/scope pin 测试)。R6 预定的 v11 编号被本波占用,R6 迁移已顺延 v12(ROADMAP 与 R6 任务书已回写)。
- app 幂等接线:`IdempotencyStore` 改以 ledger 为持久态,进程内仅余 Notify 唤醒表;client 作用域由 `{client_id}/` 前缀串改为 (tenant, client_scope, command_id) 列式;record 失败 `tracing::error!` + 计数,不再 `let _` 吞错;release 失败改 log。
- K-02:engine `LoopContext::request_approval` 加 emitter 参数,`ToolApprovalRequested` 在每次阻塞等待(含 batch 短路)前 emit 落盘,engine 只在闸门应用后补 `Responded`;GUI resume 改 `resume_messages_keep_pending` 不 seal(CLI `resume_messages` 维持 seal Denied,fail-closed 语义不变);snapshot PendingToolApprovals 只读合并投影 waiting(全局,对齐 `host.pending()` 语义);`ToolApprove` 对非 live run(GuiRunRegistry 判活)且投影 waiting 的调用走 durable resolve(Responded + ToolExecutionCompleted is_error + MessageCommitted,工具不重跑),live run 维持 queued 竞态语义。
- 写入集实态:storage(migration/mod/command_ledger 新)+ engine(tool_loop.rs emit 时序,任务书原写集未列,已实态修正)+ app(idempotency/loop_ctx/approval/services/{session,approval}/gui_host 相关),共 15 文件。
- 验证:storage 90+5 / engine 65+2+1 / app 110+6+13+2 / domain / protocol(45+10+7+5+15+16+3+16+8+8+7)/ client(9+22+9+1+3+1) 全绿,`cargo check -p pawork` 绿,events_golden 与 26 帧 golden 零 diff。
- 审查(grok_reviewer 双轮):首轮 changes-needed——P0(Queued 竞态误封 live 等待调用,已修:Queued 先判 run 活,live 不落盘)+ P1(record 失败断言改走生产 persist helper;K-02 回归升级为经 GuiHostAdapter.command(ToolApprove) 端到端)+ P2×2(snapshot 合并全局语义、淘汰全局语义,均以注释+pin 登记);修复后复核 verdict=pass。
- 登记:K-02 崩溃回归为 app 层 drop+reopen 模拟;真实 kill -9 进程冒烟与 GUI 人工验收留待人工验收;protocol/client 整 suite 曾卡 dyld 启动,分拆 `--test` 全绿,判定宿主抖动非本波问题。

## 2.3 波 C 开启核查(2026-08-21,grok_explorer ×3,证据漂移回写)

以实态为准修正 §1 证据:

- ACP:「9 张独立 Mutex map」实为 **5 张 map + 7 把 std::sync::Mutex + 2 把 tokio Mutex**(`session_contexts`/`occupancy`/`run_sessions`/`pending_prompts`/`pending_permissions` + `negotiated`/`outbox` 两把非 map 锁,`prompt_gate`/`event_rx` 为 tokio 锁);毒锁 `.expect("…mutex")` 实为 host.rs **35 处**(host.rs `.expect(` 全计 40,另 635/705/803/1048/1098 非毒锁;wire.rs:175 一处;adapter.rs:638 在测试内)。Reserved/Active **不是枚举**,是 `PromptOccupancy{ run_id: Option<RunId> }` 占位/绑定(host.rs:179)。
- 降级接点实态:`CatalogOnlyProvider` 仍在 `app/src/lib.rs:259-292`(装配 534/558),不在 provider_assembly.rs;`tasks_finish` 吞错落点 `services/run.rs:179,188` + `services/tasks.rs:73,86`(`persist_tasks`);Lagged 主路径在 `gui_server/session.rs:665-684`、`hub.rs`、`acp host.rs:534-537`(gui_host bus/events 不直接处理);HOME→temp 仍在 `app/src/data_dir.rs:22-25` 静默。
- ACP 兼容面(须保形状):`AcpHost::new/handle_request/handle_notification/handle_response/drain_and_pump/pump_events/take_outbox/drain_outbox_items/fail_closed_all_prompts/pending_run/has_active_runs/is_initialized/subscribe/release_drained_barriers`;floor.rs:805 同 session 第二 prompt 拒绝文案 `already has an active prompt turn` 逐字保留;装配点 `cli/src/acp.rs:24` 不动。
- 契约决议(主代理定形状,已先行落地 `crates/domain/src/degrade.rs`):DegradeEvent 复用既有 `AgentEvent::Diagnostic`(persist-first 落盘)+ protocol `AppEvent::Diagnostic`(实时帧)双通道,**serde 形状零变更**(26 帧 golden / events_golden / typegen schemas 零 diff);code 命名空间 `degrade.<snake_kind>` 六类冻结;默认 sink 分级(TasksFinishFailed 落盘,其余帧/stderr);**ACP 不新增 session/update 臂**(丢 Diagnostic 维持现状并显式 pin),桌面投影对 degrade.* 安全忽略、不改。
- 写入集实态修正:C 轨协议面只加 `protocol/src/app/event.rs` 的 `From<&DegradeEvent> for AppEvent` 转换 + pin(domain→App 映射实态在 `app/gui_host/events.rs`,headless 对照表已有 diagnostic 行)。

## 2.4 波 C 实态进度(2026-08-21,波 C 已收口,grok_worker ×2 并行 + grok_reviewer 双轮)

- **契约先行(主代理)**:新增 `crates/domain/src/degrade.rs`——`DegradeKind` 六类(HomeDirFallback/MissingCredential/EventStreamLagged/TasksFinishFailed/IdempotencyConflict/AcpState)+ `DegradeSeverity` + `DegradeSink` 默认分级(TasksFinishFailed 落盘,其余帧/stderr)+ `DegradeEvent` 双出口(`to_agent_event()` → `AgentEvent::Diagnostic` 合并 kind/severity/message 三键;protocol `From<&DegradeEvent> for AppEvent`)。serde 形状零变更:26 帧 golden / events_golden / typegen schemas 零 diff;code 命名空间 `degrade.*` pin 冻结。
- **轨 a(ACP actor 化,grok_worker)**:`AcpHost` 改单 actor(独立 OS 线程 + current_thread runtime;证据:同步 drain/fail_closed API × #[tokio::test] current-thread 会冻死唯一 worker,注释在 `AcpHost::new`)+ mpsc 信箱独占 5 map/negotiated/outbox;std::sync::Mutex 与 35 处毒锁 expect 清零(wire.rs 序列化 expect 走 `DegradeKind::AcpState` tracing + JSON-RPC error);prompt 串行经 actor 队列,语义与 HEAD 一致(gate 只覆盖 reserve→dispatch→bind,主代理核实旧码 drop 点);urgent cancel 经 select! 插队 dispatch;DrainOutbox 纳入 interruptible 服务;fail_closed_all_prompts 改 ack 同步等待;公开 API 与 `cli/src/acp.rs` 装配零变化;拒绝文案逐字保留;floor.rs 追加两会话交错并发种子 + 「Diagnostic 不发 ACP」pin。
- **轨 b(降级接点,grok_worker)**:HOME 回退(data_dir.rs,新增 `DataDirOutcome`/`default_data_dir_outcome`,回退点真实 tracing::warn 外发,RecordingSubscriber 测试证外发);无凭证兜底(lib.rs 装配点 tracing::warn,details 只含 provider_id,AppCore.last_degrade 死存储已删);Lagged(gui_server/session.rs 删 seq-0 旁路,改经 hub 真序列取信封 + host_tx 直发受影响连接 + ReplayUnavailable,旧测试改断言递增序列帧);tasks_finish/persist_tasks 失败(TaskService→RunService 交接,run.rs 经 persist-first sink 落盘 `AgentEvent::Diagnostic`,无 sink 处 tracing::error,本接点 let _ 清零);幂等 record 失败(主代理契约拍板:客户端无可行动作且易误读为重试信号,**只** tracing::error 结构化 code,加「不发客户端帧」pin)。`gui_host/events.rs` 对 degrade.* code 特判 level/message 取 details 键,缺省回退现状。
- **验证**:domain 56 / protocol 141(frames+golden+projection_golden+typegen 全量)/ app 139 / cli acp 41(acp_fixtures 16 + acp_floor 25)全绿,`cargo check -p pawork` 通过;fixtures/golden/schemas 全部零 diff;合计 377 绿。
- **审查(grok_reviewer 双轮)**:首轮 changes-needed 四项 P1(HOME/无凭证只构造不外发、Lagged seq-0 旁路绕开 hub、ACP fail-closed/drain 阻塞与注释失真、幂等帧或误导重试),修复后复核 verdict=pass findings=0。
- **登记**:cli map.rs `stop_reason_for`/`cancel_request` 两条 dead_code 为 HEAD 既有(主代理 git grep HEAD 核实),留波 D 清理;`DataDirOutcome` pub 导出暂无生产消费者(预留 R7/CLI 采用);ACP 通道不承载 degrade 帧(只 tracing/JSON-RPC error),桌面投影对 degrade.* 安全忽略——均为本波显式决议;两客户端传输层交错压测(真实双连接)仍缺,种子为单 Host 双会话;Zed 真实冒烟留人工验收。

## 3. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| A | 服务拆分(纯代码组织,行为零变化;每拆一块跑 app 契约测试) | host/app(R1 后 `pawork-app`) | 串行(单一 owner;心脏手术不并行) |
| B | ✅ CommandLedger 持久化 + K-02 审批落盘语义(2026-08-21 收口,见 §2.2) | storage(新迁移)、app(idempotency/approval);实态含 engine(tool_loop.rs emit 时序) | 串行(依赖波 A 的 ApprovalService 边界) |
| C | ✅ ACP actor 化 ∥ 降级事件契约(2026-08-21 收口,见 §2.4) | cli `channels/acp/` ∥ domain degrade.rs(主代理先行)+ protocol app/event.rs(From 转换)+ app(五接点);实态含 cli/tests 并发种子 | 并行 ×2(写入集不相交;DegradeEvent 契约面由主代理先定形状) |
| D | 收口:`let _` 清理(host 域)、HOME 回退告警、usage 哨兵语义按 D1 收敛、hub 序列逻辑简化(rate_limit 已删) | app、cli | 串行 |

## 4. 验证

- app 契约测试(V2 已有 88+ 条)全绿是波 A 的硬门;拆分前后 `--json`/GUI 帧行为快照对比。
- 幂等:双进程/重启重放定向测试;K-02 的 kill -9 → resume → 不重复执行回归。
- ACP:Zed 冒烟 + actor 化后的并发 prompt 压测种子(两客户端交错)。
- 降级:每类 DegradeEvent 一条触发测试(HOME 缺失、无凭证、Lagged)。
- 真实冒烟(矩阵一组):chat/审批/取消/resume/fork/usage 对账。

## 5. 退出标准

- [x] AppCore 拆为领域服务;巨 match 消失(registry 分发);行数目标达成(波 A,2026-08-21)
- [x] 幂等持久化 + K-02 语义落地并有崩溃回归;内存 CAS 删除(波 B,2026-08-21;持久态以 SQLite 为准,进程内仅余 Notify 唤醒表)
- [x] ACP 无 Mutex map/`expect` 热点;降级事件契约生效(波 C,2026-08-21;host 域 `let _` 全量清零留波 D,本波已清零五接点)
- [ ] app/cli/storage 定向测试全绿;冒烟通过;v3_plan §3 更新
