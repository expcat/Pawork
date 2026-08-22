# R6 — 会话分支模型原生化(T4,ADR-040)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R6 行。根因:S1 按线性会话设计(messages 无 branch 维度、sequence 全局),S10 Fork 后补,S13-F09 只能 `ALTER TABLE ADD COLUMN branch_id DEFAULT 'main'` + 事件反查回填 + `ancestor_lineage` API 外挂。分支语义靠补列外挂维持,压缩/投影/resume 各自处理分支边界。两个高风险契约阶段之一,必须 ADR 先行。
>
> **2026-08-22 波 0 三路核查回写(实态修正)**:`session_events.branch_id` 自 v1 即 `NOT NULL` 一等列 + FK 到 `session_branches`(`crates/storage/src/session/migration.rs:30-42`),并非 F09 后补;F09/v10 补的是 `messages` 投影列(`migration.rs:247-269`,原引用路径 `storage/session/src/migration.rs:252-260` 已随 R1 扁平化失效)。当前 `CURRENT_SCHEMA_VERSION = 11`(v11 = R4 波 B `command_ledger`)。压缩结构隐患实态:host `loop_ctx.rs` 用 `events_on_lineage(active)` 算 `compacted_through`,storage `CompactionEngine` 却用 `events_by_branch`(本支)读压缩输入,投影删除按事件所属 branch DELETE——三处语义不一致。`ancestor_lineage` 公开 API 几乎无生产消费者(生产走内部 `load_ancestor_lineage`/`events_on_lineage`)。无检入的真实 v10/v11 库文件升级 golden(现有升级测试全是临时库种子)。

## 1. ADR-040 决策点(波 0;推荐已列,须用户确认)

| 决策 | 推荐 | 备选 |
| --- | --- | --- |
| 分支模型去留 | **原生化**:events/投影按 branch lineage 一等建模 | 冻结线性模型 + 删除 Fork(产品倒退:`sessions fork`/`chat --branch`/Desktop Fork 均为 S10 已交付能力,不推荐) |
| 建模方式 | 事件账本保持 append-only 单表,`branch_id` + `parent_branch`/fork 点成为**写入时必填**的一等列(不再 DEFAULT 回填);投影按分支物化;lineage 由 storage 层维护,删除 API 外挂式 `ancestor_lineage` 拼装 | 每分支独立事件流(重放/导出复杂度高,不推荐) |
| 迁移 | schema v12(v11 已由 R4 波 B command_ledger 占用):v10/v11 数据一次性迁移(回填即校验),旧库升级 golden;信封 wire 形状不变(envelope v1 保持) | — |
| 压缩语义 | compaction 按分支水位独立计算(修 fork 后压缩误删祖先上下文的结构隐患) | — |

## 2. 目标设计

1. **storage**:`session_events`/投影表 branch 一等化;`fork_from_event` 语义收编(fork = 新 branch 记录 + 祖先引用,不复制事件);lineage 查询单点实现;v12 迁移 + 「V2 真实库文件直接打开升级」golden。
2. **engine**:compact 触发与水位按 active branch 计算;`ContextPrepared` 组装沿分支祖先链读取。
3. **host**:resume/fork/`PersistThenRender` 的分支处理从特判改为直接消费 storage 语义;`RunStart` 续聊历史按分支链(S10 修复的 `run_start_second_turn_includes_session_history` 回归保留)。
4. **desktop/protocol**:投影 reducer(R3 已同源)增加分支切换语义;Timeline fork 锚点显示沿用 `event_id`。
5. **K-05 并入(波 C)**:本机会话导入——`~/.claude/projects/**/*.jsonl` 与 Codex rollout `{timestamp,type,payload}` 两格式,导入为单分支会话;源文件只读、外部源 Secret 前缀拒绝(S9 既有红线);需脱敏样本,缺样本则该波 fail-closed 登记。

## 3. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| 0 | ADR-040 起草(含 v10/v11→v12 迁移方案与回滚不可行性说明)→ 用户确认 | docs/adr/ | 串行 |
| A | storage 原生化 + v12 迁移 + 升级 golden(含 fork 树、多分支交错、压缩折叠种子) | storage | 串行(契约面) |
| B | engine compact 分支水位 ∥ host resume/fork 收编 + desktop 投影分支语义 | engine ∥ app、protocol projection、desktop | 并行 ×2(B1 engine / B2 host+GUI;接缝为 storage API,波 A 已冻结) |
| C | K-05 导入(取得样本后);`sessions import/export` 分支往返回归 | workspace(import)、storage(formats) | 串行 |

## 4. 验证

- **重放 golden 全套**:v10/v11 库升级 v12 后逐事件重放一致;fork 分支 resume 上下文连续;压缩后祖先链读取正确。
- 信封 v1 serde 零 diff(wire 不变的证明);export v3 往返。
- 定向:`cargo test -p pawork-storage -p pawork-engine -p pawork-app -p pawork-desktop`。
- 真实冒烟(矩阵一组):`sessions fork --no-switch` 后 main/fork 两分支 `chat --resume` 各自续聊到指定口令(S10 同款)+ fork 后 `/compact` 再 resume。

## 5. 退出标准

- [ ] ADR-040 Accepted;schema v12 迁移 + 升级 golden 绿
- [ ] `ancestor_lineage` 外挂、`DEFAULT 'main'` 回填路径、分支特判消失
- [ ] 压缩按分支水位;fork/resume/导出回归绿
- [ ] K-05 完成或 fail-closed 登记(缺样本);冒烟通过;v3_plan §3 更新
