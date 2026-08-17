# S12 CR-05：持久化 / 事件 / 控制面账本审查报告

- CR 编号：CR-05
- 主审范围：foundation/sqlite、storage/session、storage/blob、foundation/domain 事件与存储类型、control-plane/core、control-plane/quota（含内嵌 tests）
- 审查日期：2026-08-17
- 主审模型：GLM（zai/glm-5.3）

## 实际审查路径

- foundation/sqlite/src/lib.rs、foundation/sqlite/src/migration.rs：Actor 串行模型、WAL/foreign_keys、backup/restore、命名空间迁移账本与迁移前备份。
- storage/session/src/lib.rs、migration.rs、event_store.rs、projection.rs、lifecycle.rs、session_tree.rs、catalog.rs：v1–v9 DDL、append-only 双触发器、事务内脱敏与投影、重放/分页、lease/生命周期、分支树与 catalog。
- storage/session/src/compaction/engine.rs、retention.rs、snapshot.rs：fork/replace range、保留策略、版本化快照与 golden。
- storage/session/src/import/persist_export.rs、persist_compat.rs、persist_pi.rs：导入导出单事务、identity、分支与 head 校验、Secret 扫描。
- storage/blob/src/artifact.rs、protected.rs、checkpoint.rs：内容寻址 put/get/range/release/gc/integrity、PWB1+AEAD 状态机、checkpoint-state-v1 原子持久化与回滚。
- foundation/domain/src/events.rs 与 foundation/domain/tests/events_golden.rs：事件信封 v1、32 变体、serde tag/content 与 golden。
- control-plane/core/src/usage.rs、audit.rs：UsageRecord v2、SQLite v2→v3、dedup 主键/部分唯一索引、内存/SQLite 幂等语义、audit JSONL append/启动校验/allowlist export。
- control-plane/core/src/tenant.rs、decision.rs、rbac.rs、identity.rs、identity_schema.rs：deny-first 策略、版本化决策、RBAC、身份 schema 抽样。
- control-plane/quota/src/ledger.rs、service.rs、domain.rs、adapter.rs、error.rs、util.rs：LocalLedger 计数、半开窗口 reconcile、cache/singleflight 与 abort 语义。
- 跨包消费面抽样：engine/engine/src/tool_loop.rs、host/app/src/lib.rs、control.rs、persist.rs、checkpoint.rs、gui_host.rs、orchestration_host.rs，以及 agents/orchestration/src/budget.rs 与 supervisor budget flush 入口。
- 基线：docs/design.md §3.2、plan/S10-serve-clients.md、plan/S11-workflow-control.md、plan/S5-context-usage.md、ROADMAP.md §3.2 K-01～K-10。未发现本报告 finding 与 K-01～K-10 直接重复。

## 未覆盖路径与原因

- 未执行 cargo test/build/clippy/fmt、pawork/GUI/protocol-probe、fault-injection 或真实多进程并发实验：S12 任务书明确只读审查、禁止运行测试与二进制。
- agents/orchestration 只审查 usage ledger 消费与 budget flush 接口，不展开 supervisor 全量正确性；该包主审归 CR-06。
- host/gui-server、protocol、Desktop 只追到 fork/resume/compact 的持久化输入输出，不做协议与 UI 全量审查；主审归 CR-07/CR-08。
- SQLite backup/restore、PWB1 AEAD 损坏注入、崩溃半行 audit JSONL 等只做源码与测试断言审查，未做真实故障注入。
- control-plane/core 与 control-plane/quota 的内嵌测试按核心契约抽查，不逐条复核全部测试名称。

## Findings

### S12-CR05-01 分支/Fork 消费面使用无 branch 维度的全 session 消息投影

- 类别：Requirement Gap
- 严重度：High
- 置信度：Confirmed
- 证据：
  - storage/session/src/migration.rs:30-42（session_events 含 branch_id，sequence 是 session 全局唯一）；storage/session/src/migration.rs:45-53（messages 投影表没有 branch_id，索引按 session_id+sequence）。
  - storage/session/src/event_store.rs:339-368（events_by_branch 明确按 branch 过滤，并说明与全 session replay 相对）；但 host/app/src/lib.rs:1034-1037 的 resume_messages 直接消费 projection_snapshot(session_id)，未读取 active branch 或祖先链。
  - storage/session/src/projection.rs:169-177（CompactionCompleted 以 session 全局 sequence <= compacted_through 删除该 session 全部 messages，不区分发出事件的 branch）。
  - plan/S10-serve-clients.md:29 要求补齐分支/Fork 消费面与投影；host/app/src/gui_host.rs:1225-1232 已把 GUI fork 后切换 active branch 作为用户操作暴露。
  - 实际行为：从较早事件 fork 并切换后，resume_messages / compact_session 仍读到其它 branch（尤其父分支 fork 点之后）的消息；任一 branch 压缩时还会按全局水位删除所有更低 sequence branch 的消息投影。
  - 期望行为：active branch 的祖先前缀 + 本 branch 事件形成 Timeline/resume 上下文；压缩删除只作用于该分支语义下的消息，或明确冻结并实现会话级线性投影模型。
  - 影响：Fork/Resume、GUI Timeline、后续 turn 组装和手动/自动 compaction 会混入未来分支消息或丢失早期分支消息；事件流仍 append-only，rebuild_projection 可重建，但重建结果复现同一错误投影。
- 验证建议（S12 不执行）：新增确定性测试：main 追加事件 1–3，从事件 1 fork 并 switch，再调用 resume_messages；预期只含 fork 祖先前缀，实际会含 2–3。再在 fork branch 压缩，断言 main 中低于全局水位的消息是否被删。
- 整改边界：最小写入集宜在 storage/session 增加分支/祖先投影语义（可能需要 v10 附加式迁移）并让 host resume/compact 显式使用该语义；配套 storage 与 host 定向测试。不得顺手改变事件信封 v1、append-only 事实表或协议帧。

### S12-CR05-02 失败/取消 run 已发生用量不进入 usage ledger

> **交叉复核裁定**（2026-08-18 主代理回写，Grok 复核，详见 [CR-05-08-cross-review-grok.md](CR-05-08-cross-review-grok.md)）：**adjust-severity → Medium**。不入账属实，但 chat 热路径未用该 ledger 做 quota/budget 门禁；影响是计量低估，不是控制面绕过。

- 类别：Requirement Gap
- 严重度：High
- 置信度：Confirmed
- 证据：
  - foundation/domain/src/events.rs:111-115：UsageUpdated 的冻结语义明确“确保失败/取消时已发生用量不丢失”。
  - engine/engine/src/tool_loop.rs:330-366：多轮工具循环只把累计 usage 放进成功 RunCompleted 的 summary；超过最大轮数、provider error、cancelled 都直接返回 Err，终态事件不携带累计 usage。
  - host/app/src/lib.rs:1287-1320：host 仅在 run_session 返回 Ok(summary) 时调用 record_completed_usage；Err(_) 分支只结束 task，不扫描已持久化的 UsageUpdated 事件。
  - 实际行为：provider 已流式返回 usage 后 run 失败或取消时，token/cost 不写入 ledger；事件流中虽有 UsageUpdated，但没有账本消费者兜底。
  - 期望行为：失败/取消 run 的已观测累计用量进入 UsageLedger，保持幂等 dedup key。
  - 影响：pawork usage 与 quota/budget gate 系统性低估；长时间运行后取消或失败的请求可绕过预算事实来源，重复失败会持续免费化。
- 验证建议（S12 不执行）：用 MockProvider 先发 UsageUpdated 再发 error/cancel，断言事件流与 ledger 记录一致；再验证重试/重放不重复计数。
- 整改边界：可在 engine 错误返回中携带累计 usage，或 host 在 Err 后按 run_id 重放已持久化 UsageUpdated 聚合入账；只改 engine/engine 或 host/app 的 run 收口与定向测试，不改变事件 serde。

### S12-CR05-03 重复写前快照先增加 blob 引用再命中去重返回，引用计数泄漏

- 类别：Bug
- 严重度：Medium
- 置信度：Confirmed
- 证据：
  - storage/blob/src/checkpoint.rs:196-227：snapshot_before_write 先读取当前文件并调用 store.put(bytes)；storage/blob/src/artifact.rs:230-260 显示 put 每次都会 ref_count + 1。
  - storage/blob/src/checkpoint.rs:237-288：之后才在 state 中查找同 run_id+tool_call_id+relative_path 快照；264-276 命中后直接返回 existing，没有把本次新增引用记录到 checkpoint，也没有调用 release。
  - storage/blob/src/checkpoint.rs:882-900 的测试只断言 change/files 数量去重，没有断言 blob ref_count 仍为 1。
  - 实际行为：同一 tool call 对同一已存在文件重复调用快照时，返回旧快照但 ref_count 多增一次。
  - 期望行为：去重路径不产生额外引用，或为临时 put 显式 release。
  - 影响：引用计数永久虚高，gc 永远不能回收该 blob；大量同路径写入会放大磁盘占用并使预算统计失真。
- 验证建议（S12 不执行）：扩展 snapshot_dedupes_same_path_in_call，两次调用后断言 metadata(pre_blob).ref_count == 1，再 release/run rollback 后验证 gc 可回收。
- 整改边界：仅调整 storage/blob/src/checkpoint.rs 的检查/put/release 顺序并补定向测试；不要改变 checkpoint-state-v1 JSON 形状或 ArtifactStore API。

### S12-CR05-04 无定价记录被静默标成 USD

- 类别：Bug
- 严重度：Low
- 置信度：Confirmed
- 证据：
  - host/app/src/lib.rs:1323-1340：record_completed_usage 在 estimate_cost_for 返回 None 时传入 cost_micros=0、currency 为空。
  - host/app/src/control.rs:129-172：usage_record 对非 3 位大写币种一律替换为 USD，且不设置 cost_confidence / cost_provenance。
  - plan/S5-context-usage.md:21-25、32-33 明确“无定价不编造费用/定价”。
  - 实际行为：无定价模型虽然金额为 0，但持久化账本记录声明币种 USD。
  - 期望行为：无定价时保留“成本未知/无定价”口径（例如仅在 cost>0 时要求币种，或引入显式 unknown/none 附加式表示），不得静默选择真实币种。
  - 影响：当前 0 金额对聚合数值影响有限，但账本导出、审计和多币种统计会把“未知成本”误归类为 USD，后续接入实收或非 USD 定价时会污染口径。
- 验证建议（S12 不执行）：为无 pricing 条目构造 usage record，断言记录不会声明 USD；再验证有定价记录仍保持原币种。
- 整改边界：仅调整 host/app/src/control.rs 构造与必要的基础表示/校验，补 host/control 定向测试；不得为无定价模型填入估算费率。

### S12-CR05-05 崩溃窗口留下的 final blob 孤儿不会被 gc 回收

- 类别：Maintainability
- 严重度：Low
- 置信度：Confirmed
- 证据：
  - storage/blob/src/artifact.rs:273-293：新 blob 先 atomic_write 到最终内容寻址路径，再插入 SQLite 元数据；进程在 rename 后、INSERT 前崩溃会留下磁盘 final 文件但无 DB 记录。
  - storage/blob/src/artifact.rs:502-552：integrity_check 能列出磁盘有、DB 无的 orphan；273-286 只有后续同内容 put 才会采纳该文件。
  - storage/blob/src/artifact.rs:555-589：gc 只删除 DB 中 ref_count=0 的 blob，并只额外清理过期 .tmp- 文件，不回收 final orphan。
  - 实际行为：该崩溃窗口产生的 final 文件会一直占用磁盘，并让 integrity_check().is_ok() 持续为 false，除非之后恰好 put 同一内容。
  - 期望行为：崩溃恢复策略应能在安全延迟/哈希校验后回收 DB 无记录的 final blob，或提供显式 repair/reclaim 路径。
  - 影响：不丢已有引用数据，但长寿命实例可能出现不可自动收敛的磁盘泄漏和持续非绿完整性报告。
- 验证建议（S12 不执行）：注入“rename 成功、DB insert 失败/进程终止”的故障，断言下次 gc/repair 可删除 orphan 且不影响有 DB 记录 blob。
- 整改边界：仅扩展 storage/blob/src/artifact.rs 的 gc/repair 与测试；不要改变内容寻址路径格式或 PWB1 格式。

## 统计

- 严重度：Critical 0 · High 2 · Medium 1 · Low 2
- 置信度：Confirmed 5 · Needs Verification 0
- 已核对且未立项的要点：session append-only 双触发器与事务内投影、migration 单事务/备份、PWB1 AAD/状态机、UsageLedger SQLite 与内存 dedup 语义、quota 半开窗口 reconcile、audit JSONL fail-closed append。这些路径未发现达到 finding 门槛的新缺陷。
