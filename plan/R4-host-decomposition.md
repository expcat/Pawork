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
- 行数:`lib.rs` 4131→1413(<1500 ✅);`gui_host/mod.rs` 679(<800 ✅)。整阶段审计时(2026-08-22)lib.rs 实态 1514(波 B/C/D 接线吸收),审计修复后 1458;gui_host/mod.rs 审计后 812,略超 800 目标(增量为有界等待/重试与 hazard 注释,见 §2.6 登记)。
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

## 2.5 波 D 实态进度(2026-08-21,波 D 已收口,grok_explorer ×3 核查 + glm_worker 单 owner + glm_reviewer)

- **核查漂移回写**:hub 实态位于 `crates/app/src/hub.rs`(EventHub,核查时 425 行,波 D hub 简化后 412 行),非 §1 所指 gui-server 内;host 域非测试 `let _` 基线 58 处(app 24 + cli 36),分类:通道唤醒 25 / 资源清理 11 / 吞 Result 12 / 非 Result 弃绑定 6(测试内 2 处除外)。
- **`let _` 全量清零**:58 处逐点处理——通道唤醒/资源清理/断连 send_frame 等常态竞态改 `tracing::debug!` 结构化带上下文;tasks_start_agent、RunCancel、flush_outbox、dispatch_request、dispatch_attached 四 fail-closed 路径升 `tracing::warn!`;非 Result 弃绑定改定义处 `_` 前缀或签名级参数改名;收尾 `rg 'let _ =' crates/app/src crates/cli/src` 非测试命中为 0(整阶段审计复核实态:残留 4 处全在 #[cfg(test)] 内,原「3 处」为漏计)。acp/host.rs:644 回执 payload 双 trait 缺失,按同文件 `wait_std` 先例用 `is_err()` + 上下文 debug,未为日志扩大类型面。
- **HOME 回退告警升级**:`resolve_data_dir_outcome` / `default_data_dir()` 保持纯路径选择(内部 warn 移除,反向断言钉住);单一结构化出口 `consume_data_dir_outcome` 发 `tracing::warn!`(code=degrade.home_dir_fallback/severity/path/message)。生产消费者:`AppCore::load_with`(会话库落地)与 `ops::inspect_instance`(不经 load_with 的早退路径)。`attach_workspace` / GUI / extension 继续走静默 `default_data_dir()`,避免同一进程重复告警。pin:`consume_data_dir_outcome_warns_once_and_path_helper_stays_silent` + `load_with_home_fallback_consumes_degrade_and_warns_once`(cfg(test) 测试缝 `data_dir_outcome_for_test` 透传私有纯函数)。
- **usage 哨兵按 D1 收敛**:账本写入值零变化;control.rs 补三段 doc 钉死 ADR-038 D1 哨兵语义(LEDGER_ACCOUNT="local/default"、upstream_attempt=Some(1)、trace_id=None——无上游重试跟踪、哨兵宇宙不扩张),新增 pin 测试断言三字段。登记:control-plane legacy v1 JSON 默认 `upstream_attempt=None` 与 host `Some(1)` 口径差异,留 R9 复查(ROADMAP §4)。
- **hub 序列简化**:单字段 `RingInner` 拆除直用 VecDeque;零消费公开 API `subscriber_count` 删除;`publish_with_envelope` 收窄 pub(crate)(全仓无外部调用方);过时 rate limiter 测试注释修正;序列连续性/replay 窗口/容量淘汰/lagged 不变量零变化(8 条 hub 测试断言未动)。
- **死码删除**:acp/map.rs `stop_reason_for`/`cancel_request` 删除(全仓零引用,host.rs 有内联等价表),连带清理未用 import。
- **写入集实态**:crates/app 10 文件 + crates/cli 8 文件(357+/124-),与设计一致;审查(glm_reviewer)verdict=pass,P2-1(load_with pin 未端到端驱动,接线 4 行已人工核实,接受登记)、P2-2(登记项提醒,已落 ROADMAP §4)。
- **验证**:app 121+6+13+2 / cli 35+16+25 / protocol golden 5 / domain events_golden 3 全绿零 diff,`cargo check -p pawork` 通过;client 45 条(9+22+9+1+3+1,含 probe 场景与 spawn_e2e)主代理复跑全绿——R4 阶段收口。

## 2.6 整阶段审计(2026-08-22,grok_explorer ×4 只读分域审计 + 主代理逐项裁决 + grok_worker 修复 + grok_reviewer 双轮复核)

模式同 R3 整阶段审计:基线门禁全绿后四路只读分域审计(波 A 服务拆分/lib.rs、波 B CommandLedger 与幂等接线、波 C ACP actor 化与降级契约、波 D let-underscore 收口面与 hub),主代理逐项裁决,确认缺陷交 grok_worker 修复(写集合 7 文件),grok_reviewer 首轮 changes-needed 三项(R-1 record 失败重试次序、R-2 hazard 测试拆分、R-3 超时/断连区分),修复后复核 verdict=pass(findings=0)。

- **确认缺陷与修复(7 项)**:
  - B-1(P1)`gui_host/mod.rs` command() InFlight 臂:同 idempotency_key 不同 command_id 占位时 waiter 按自身 command_id 注册永不被唤醒,叠加 notify_waiters 丢唤醒竞态 → 50ms 有界等待(select notified/sleep)后回 loop 重查 SQLite 权威 CAS;回归拆 hazard1/hazard2 两个独立测试(hazard2 无 sleep,确定性触发)。
  - B-2(P1)`persist_command_response`:record 失败后行仍 inflight 不释放(同进程重试挂死、重启 reclaim 重入)→ release 兜底;复核进一步改 DB 类错误(Closed/Other/StoreUnavailable)先重试 record 一次(幂等 UPDATE WHERE status='inflight'),重试仍失败或 KeyConflict/DuplicateCommand 才 release;回归追加带键 check 得 Replay(键持有行)断言,证明带键重试不重执行。
  - D-1(P1)`services/run.rs` tasks_start_agent `.ok()` 吞错 → fail-closed `tracing::warn!`(对标 orchestration_host 先例)。
  - A-4(P2)lib.rs 残留 compact_session 内联 63 行 → 逐字搬入 RunService,门面委托,lib.rs 1458(<1500)。
  - D-3(P2)`cli/acp.rs` pump/teardown/frame loop 三处 flush_outbox 失败无 warn → 补 warn,与 drain 路径一致。
  - C-1(P2)`acp/host.rs` wait_std 无界 recv → recv_timeout(2s),drain_outbox_items/fail_closed_all_prompts 区分 Timeout/Disconnected 分别 report_acp_state,公开签名与空向量/返回兜底行为不变。
  - B-3(P2)storage 缺 open_read_only 不 reclaim 回归 → 新增 open_read_only_does_not_reclaim_inflight 测试。
- **驳回误报(代表性)**:B 初版 P0(引用虚构路径 crates/storage/src/command_ledger.rs,实为 session/ 子目录;check 经 DatabaseActor 单 call 原子);A-2/A-3(引用行号与 canonical_wire_name 符号不存在);C-2(host.rs 无 Mutex map 残留);C-3(floor.rs pin 实态在 :1147 且正确);C-5(host.rs 无 eprintln)。
- **验证**:主代理独立复跑 cargo check -p pawork-app -p pawork-cli → cargo test -p pawork-app -p pawork-cli -p pawork-storage(355 passed / 0 failed / 1 ignored)→ cargo check -p pawork 全绿;冻结契约零触碰(protocol/domain/engine/golden/schemas/wire 无 diff)。
- **行数基线漂移**:审计时 lib.rs 1514(波 A 基线 1413,波 B/C/D 接线吸收),修复后 1458;gui_host/mod.rs 812,略超波 A 800 目标(增量为审计修复的有界等待、重试逻辑与 hazard 注释),登记为已知偏差,不阻塞。
- **登记**:复核残留两项(record 失败计数单次计数、50ms 轮询延迟上界)均非阻塞,维持现状;K-02 kill -9 冒烟与 GUI/Zed 人工验收仍留 ROADMAP §4,本次不变。


## 3. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| A | 服务拆分(纯代码组织,行为零变化;每拆一块跑 app 契约测试) | host/app(R1 后 `pawork-app`) | 串行(单一 owner;心脏手术不并行) |
| B | ✅ CommandLedger 持久化 + K-02 审批落盘语义(2026-08-21 收口,见 §2.2) | storage(新迁移)、app(idempotency/approval);实态含 engine(tool_loop.rs emit 时序) | 串行(依赖波 A 的 ApprovalService 边界) |
| C | ✅ ACP actor 化 ∥ 降级事件契约(2026-08-21 收口,见 §2.4) | cli `channels/acp/` ∥ domain degrade.rs(主代理先行)+ protocol app/event.rs(From 转换)+ app(五接点);实态含 cli/tests 并发种子 | 并行 ×2(写入集不相交;DegradeEvent 契约面由主代理先定形状) |
| D | ✅ 收口(2026-08-21,见 §2.5):host 域 `let _` 58 处清零、HOME 回退告警升级(单一 consume_data_dir_outcome 出口,load_with/ops 消费,路径 helper 静默)、usage 哨兵按 D1 钉死(doc+pin,值零变化)、hub 简化(RingInner 拆除/死 API 清理)、acp map.rs 死码删除 | app、cli | 串行 |

## 4. 验证

- app 契约测试(V2 已有 88+ 条)全绿是波 A 的硬门;拆分前后 `--json`/GUI 帧行为快照对比。
- 幂等:双进程/重启重放定向测试;K-02 的 kill -9 → resume → 不重复执行回归。
- ACP:Zed 冒烟 + actor 化后的并发 prompt 压测种子(两客户端交错)。
- 降级:每类 DegradeEvent 一条触发测试(HOME 缺失、无凭证、Lagged)。
- 真实冒烟(矩阵一组):chat/审批/取消/resume/fork/usage 对账。

## 5. 退出标准

- [x] AppCore 拆为领域服务;巨 match 消失(registry 分发);行数目标达成(波 A,2026-08-21)
- [x] 幂等持久化 + K-02 语义落地并有崩溃回归;内存 CAS 删除(波 B,2026-08-21;持久态以 SQLite 为准,进程内仅余 Notify 唤醒表)
- [x] ACP 无 Mutex map/`expect` 热点;降级事件契约生效(波 C,2026-08-21;host 域 `let _` 全量清零于波 D 完成)
- [x] app/cli/storage 定向测试全绿;probe+spawn_e2e 冒烟通过;v3_plan §3 更新(波 D,2026-08-21;K-02 kill -9 冒烟、GUI/Zed 人工验收与双连接压测登记 ROADMAP §4)
