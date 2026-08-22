# ADR-040:会话分支模型原生化(schema v12)

- **状态**:Accepted(用户 2026-08-23 确认)
- **日期**:2026-08-22(2026-08-23 Accepted)

## 背景

V2 S1 按线性会话设计;S10 交付 Fork(`sessions fork`/`chat --branch`/Desktop Fork)后,分支语义由两处后补机制维持:S13-F09 以 v10 迁移给 `messages` 投影补 `branch_id TEXT NOT NULL DEFAULT 'main'` + 按 `session_events` 反查回填(orphan 静默归 `'main'`),并以 `ancestor_lineage` 公开 API 外挂拼装祖先链。R6 任务书 [plan/R6-session-branching.md](../../plan/R6-session-branching.md) 判定其为结构性债务;两个高风险契约阶段之一(R6/R7),必须 ADR 先行。

2026-08-22 波 0 三路只读核查(实态,已回写任务书):

- `session_events.branch_id` 自 schema v1 即 `NOT NULL` 一等列并带 FK 到 `session_branches`([`crates/storage/src/session/migration.rs`](../../crates/storage/src/session/migration.rs) 30-42);F09/v10 后补的是 `messages` 投影列(247-269)。任务书「后补 `branch_id` 列」的准确对象是投影,不是事件账本。
- sequence 为 session 全局:`UNIQUE(session_id, sequence)`、`append_event` 以 `MAX(sequence) WHERE session_id` 续号;fork 只插 `session_branches` 不复制事件(`session_tree.rs` `fork_from_event`)。
- `ancestor_lineage` 公开 API 几乎无生产消费者;生产读路径为内部 `load_ancestor_lineage` / `events_on_lineage`(resume、Timeline、host 压缩输入)。
- **压缩三处语义不一致(结构隐患实态)**:host `crates/app/src/loop_ctx.rs` 用 `events_on_lineage(active_branch)`(含祖先链)算 `compacted_through`;storage `CompactionEngine` 用 `events_by_branch`(仅本支)读压缩输入(`compaction/engine.rs`);投影删除按事件所属 branch `DELETE ... WHERE branch_id AND sequence <= through`(`projection.rs` 180-187)。fork 后压缩的水位、读取、删除三者口径互不对齐。
- `DEFAULT 'main'` 与分支特判散落:v3 `sessions.active_branch`、v10 `messages.branch_id` 回填、`DEFAULT_BRANCH_ID` 常量(`event_store.rs:10`)、compat/Pi 导入一律落 main、export 读 v1 降级 main。
- 信封 v1 不含 branch 字段:`AgentEventEnvelope{schema_version=1, event_id, session_id, run_id, sequence, timestamp, parent_event_id?, payload}`(`crates/domain/src/events.rs` 44-54),32 变体字节 golden 在位;branch 只存在于表列、`AppendReceipt` 与 export v3 sidecar(`ExportedEvent{branch_id, event}`)。
- 无检入的真实 v10/v11 `.sqlite` 升级 golden;现有升级测试全为程序化临时库种子。
- `command_ledger`(v11,R4 波 B)与会话事件同库共存、无 FK;open 时回收 inflight。

参照:DeepSeek Harness `ctx.sessions.fork`(fork 只许切在 turn 边界,`(parentSession, seedLength)` lineage)与 Pi per-entry `parentId` 树([references.md](../references.md) §7.1 R6 行);反面教材 Claude Code 跨文件 DAG 重建(昂贵且脆弱)。Pawork 单表 `branch_id` 引用零拷贝方案优于 DSH 深拷贝 seed。

## 决策

### D1 — 分支模型去留:原生化

events/投影按 branch lineage 一等建模,保留并正式化 S10 已交付的 Fork 能力。

- 否决支:冻结线性模型 + 删除 Fork——`sessions fork`/`chat --branch`/Desktop Fork 均为已交付产品能力,删除属产品倒退,不接受。

### D2 — 事件账本:append-only 单表 + session 全局 sequence 保持

`session_events` 保持 append-only 单表(v2 触发器不动),fork 不复制事件的零拷贝语义不动;`UNIQUE(session_id, sequence)` 与全局续号保持——全局序列是跨分支重放、export 与 Timeline 锚点(`event_id`/`sequence`)的唯一事实序。

- 否决支:每分支独立事件流 / per-branch sequence——须迁移 `UNIQUE` 并重建跨分支排序,重放/导出/投影复杂度显著上升,收益不抵成本。

### D3 — lineage 单点收编,消灭外挂与静默回退

- 祖先链查询收编为 storage 层单点:`events_on_lineage` 语义成为 resume/Timeline/压缩的唯一祖先链读路径;公开 `ancestor_lineage` 外挂 API 删除或收窄为内部实现细节(实态已几乎无生产消费者)。
- `messages` 投影分支维度正式化:作为可重建投影,由 `session_events` 重建时按事件所属 branch 物化;`DEFAULT 'main'` 回填路径与「orphan 静默归 main」语义消灭——投影行无事件背书即迁移/重建校验失败,不静默兜底。
- 写入路径显式化:建 session/切换分支/append 显式携带 branch,不依赖 DDL DEFAULT;import/PI 落 main 改为显式单分支语义而非隐式默认。
- export v3 sidecar(`ExportedEvent{branch_id}`)与 `AppendReceipt.branch_id` 已是 branch 的正式表达面,形状不动。

### D4 — 迁移:schema v12(v10/v11 一次性迁移,回填即校验)

- `CURRENT_SCHEMA_VERSION` 11→12;v12 内容按波 A 设计落任务书,原则是:重建 `messages` 投影为无 `DEFAULT` 的显式 branch 物化(可整表重建,因其为可重建投影),回填过程即校验——任何无法从 `session_events` 获得 branch 背书的投影行使迁移失败,不静默归 main。
- v1–v10 DDL 与 v11 `command_ledger`(含部分唯一索引 `idx_command_ledger_idempotency_key`)不动;不回滚、不重排历史迁移。
- **golden 先行**:检入真实形状的 v10/v11 库文件(或等价的程序化全量种子库)「直接打开升级 v12」golden,升级后逐事件重放一致;fork 树、多分支交错、压缩折叠三类种子随迁移落地。这是现有测试基建的缺口,波 A 必须先补。
- 信封 v1 wire 零 diff(32 变体 + parent golden 不动);GUI 帧/headless JSON 不新增 branch 字段——分支切换语义沿用现有快照/reset_baseline 机制与 `event_id` 锚点表达。若波 B 实态证明必须动 wire,回本 ADR 补决议后再改。

### D5 — 压缩语义:按分支水位独立计算,三处口径合一

- compaction 触发与水位按 active branch 的祖先链(lineage)独立计算;host 水位(`loop_ctx`)、storage `CompactionEngine` 读取范围、投影删除区间三处统一为同一线性语义——消除「fork 后压缩误删/漏读祖先上下文」的结构隐患。
- 投影删除不得波及兄弟分支:压缩只物化/折叠本支 lineage 覆盖的投影区间,fork 点之后其他分支的投影行不受影响。
- fork 点约束参照 DSH 不变量:fork 只许切在事件(turn)边界,越界即拒绝;`(parent_branch_id, forked_from_event_id)` 即 Pawork 对 DSH `(parentSession, seedLength)` 的零拷贝对应。

### D6 — K-05 并入范围(波 C,登记而非本 ADR 主体)

本机会话导入(`~/.claude/projects/**/*.jsonl` 与 Codex rollout `{timestamp,type,payload}`)导入为**单分支**会话;源文件只读、外部源 Secret 前缀拒绝(S9 红线)不变。实态提示:现有 compat 导入器解析的是 Claude 导出 JSON 对象而非 projects jsonl,且 `.jsonl` 无 `--format` 时误判为 Pi——波 C 需新增逐行容错解析与格式探测;两格式非稳定契约,未知 type 落为不透明扩展事件。缺脱敏样本则该波 fail-closed 登记。

## 迁移方案与回滚不可行性

- 迁移在单事务内执行,失败整批回滚(沿用 sqlite 迁移框架既有语义);`open_read_only` 闸门随 `CURRENT_SCHEMA_VERSION` 升至 12。
- **回滚不可行**:v12 库一旦被新版本写入(新投影物化/分支元数据重构),旧版本 `open_read_only` 要求恰好 v11 将拒绝打开;不支持原地降级。回滚路径 = 恢复迁移前备份文件;升级前应由 `SessionStore::open` 迁移路径留档原库副本或提示用户备份(波 A 设计定细节)。append-only 事件本体不丢,可重建投影,但投影/分支元数据的重构不可逆。
- v10/v11 库为本次迁移的支持输入;更早版本(V1/V2 真库)经既有 v1–v11 序列升到 v11 后再升 v12,同序列保证可打开升级。

## 后果

- 波 A(storage,v12 + golden)、波 B(engine compact 分支水位 ∥ host resume/fork 收编 + desktop 投影分支语义)、波 C(K-05 导入)按任务书波次拆分执行;ADR Accepted 是开工前提。
- `DEFAULT 'main'` 回填路径、公开 `ancestor_lineage` 外挂、resume/compact/projection 的分支特判在波 A/B 逐步消失(退出标准见任务书 §5)。
- storage schema 与信封版本继续独立演进:信封 v1 不动,schema 11→12。
- desktop 投影(R3 已同源)仅需在 reducer 增加分支切换语义,wire 不变;desktop deny-list(client-only)不受 R6 影响。

## 相关

- [plan/R6-session-branching.md](../../plan/R6-session-branching.md)(任务书:决策点、波次拆分、验证、退出标准)
- [ROADMAP.md](../../ROADMAP.md) §2 R6 行、§3.2 K-05
- [v2-summary.md](../v2-summary.md) §4(冻结契约:信封 v1、会话存储、export v3)、§5(F11/F32 Timeline 锚点 `event_id`/`sequence`)
- [design.md](../design.md) §3.2(信封/会话存储契约表)
- [references.md](../references.md) §7.1 R6 行(DSH fork 不变量、Pi 树、Claude Code 反面教材、K-05 导入映射要点)
