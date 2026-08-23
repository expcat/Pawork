# R6 — 会话分支模型原生化(T4,ADR-040)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R6 行。根因:S1 按线性会话设计(messages 无 branch 维度、sequence 全局),S10 Fork 后补,S13-F09 只能 `ALTER TABLE ADD COLUMN branch_id DEFAULT 'main'` + 事件反查回填 + `ancestor_lineage` API 外挂。分支语义靠补列外挂维持,压缩/投影/resume 各自处理分支边界。两个高风险契约阶段之一,必须 ADR 先行。
>
> **2026-08-22 波 0 三路核查回写(实态修正)**:`session_events.branch_id` 自 v1 即 `NOT NULL` 一等列 + FK 到 `session_branches`(`crates/storage/src/session/migration.rs:30-42`),并非 F09 后补;F09/v10 补的是 `messages` 投影列(`migration.rs:247-269`,原引用路径 `storage/session/src/migration.rs:252-260` 已随 R1 扁平化失效)。当前 `CURRENT_SCHEMA_VERSION = 11`(v11 = R4 波 B `command_ledger`)。压缩结构隐患实态:host `loop_ctx.rs` 用 `events_on_lineage(active)` 算 `compacted_through`,storage `CompactionEngine` 却用 `events_by_branch`(本支)读压缩输入,投影删除按事件所属 branch DELETE——三处语义不一致。`ancestor_lineage` 公开 API 几乎无生产消费者(生产走内部 `load_ancestor_lineage`/`events_on_lineage`)。无检入的真实 v10/v11 库文件升级 golden(现有升级测试全是临时库种子)。
>
> **2026-08-23 波 A 回写(实态)**:波 A 收口,写入集仅 crates/storage。① 落地:`CURRENT_SCHEMA_VERSION` 11→12;v12 迁移 = 孤儿 fail-closed(TEMP 触发器 RAISE ABORT,整批事务回滚)+ 按事件所属 branch 重建 `messages` 去 `DEFAULT 'main'` + 按原名恢复两索引;检入 4 个升级 golden(v10 fork 树 / v11 交错 / v10 压缩折叠 / v11 孤儿负例),fixture(`src/session/fixtures/` 7 JSONL)由真实写入路径落盘字节生成、`PAWORK_WRITE_STORAGE_GOLDEN=1` 门控再生;删除公开 `ancestor_lineage`(全仓零生产消费者);`create_session` 显式写 active_branch。reviewer verdict=pass。② 核查新发现实态(登记波 B):compaction 引擎 `filter_retention_inputs` 对 host lineage 输入按本支 event_id 二次过滤(engine.rs:141-145),祖先链条目被「读取+过滤」双重排除,比波 0 所述三处口径不一致更硬;`fork_from_event` 对同 fork 点重复 branch_id 一律拒绝,与 `create_branch` 同 `(parent, fork point)` 幂等语义不对称;Pi 导入事件全落 main 但 Branch payload 照插元数据行(零事件归属)。③ 偏差 2 项:既有 v9 升级正例种子的孤儿行 m-orphan 移出(旧断言「孤儿静默归 main」与 D4 fail-closed 直接冲突),孤儿负例由专项 v11 golden 承接;fixture 先落空占位使 crate 可编译、后由生成器写入真实字节(终态无手写字节)。④ 波 B 清单:压缩三处口径合一(含 `filter_retention_inputs`)、fork turn 边界(ADR D5)、`fork_from_event` 幂等不对称、Pi 导入分支归属;golden 生成器与被测读路径共享 lineage 实现,波 B 若动 lineage 保持 fixture 不再生。
>
> **2026-08-23 波 B 开工前 GLM 三路核查回写(实态与设计冻结)**:① 波次表漏列 storage——host 已用 `events_on_lineage(active)`,但 storage compact 仍按本支读取并二次过滤；投影按事件 branch 物理 DELETE 既会让「子支压缩」漏折叠祖先,也会让「父支后压缩」删除早先 fork 仍应可见的祖先行；`loop_ctx` 还以 `.ok()?` 吞 compact 错误,engine 无 outcome 时以摘要自身 sequence 作水位。② 无 schema/wire 变更的收口形态冻结为：compact 输入与保留过滤统一为 active lineage；v12 `messages` 物化表继续按事件 branch 执行冻结的 branch-local fold，但 branch snapshot 改从 append-only `session_events` 重建消息，再按目标 lineage 可见的 `CompactionCompleted` 最大水位折叠，避免跨视图不可逆丢失；无 outcome 水位 fail-safe 为 0。③ `fork_from_event` 只接受 `RunCompleted` / `RunCancelled` / `RunFailed` turn 终止事件,同 `(parent, fork point)` 重试直接复用 `create_branch` 幂等,其余事件拒绝。④ Pi Branch marker 显式折叠为 main 上的单分支导入元数据,不得创建零事件 branch。⑤ protocol 只在非 wire `TimelineEntry.fork_boundary` 标记上述终止边界,Desktop 仅对该类条目开放 Fork；切支继续沿用 snapshot + `reset_baseline`,补同 session 换 branch 的基线回归。schema v12、信封 v1、GUI 帧、export v3、波 A fixture 字节均不动；`SessionCompact` 悬空命令与全局 `head_sequence` 语义不在本波扩线。
>
> **DSH 主参考现态复核补充**:`ctx.sessions.fork` 接受「闭合 turn 之后的稳定位置」,包括 `turn/end` 或其后的 standalone log-only event,并拒绝 open turn 内部边界；映射到 Pawork 后，storage 白名单为 `RunCompleted` / `RunCancelled` / `RunFailed` 与 standalone `CompactionCompleted`。`MessageCommitted`（含 user message）不单独证明 turn 已闭合,仍拒绝；Desktop 当前只投影前三类 run 终态,故其 Fork 菜单边界不扩 wire、不为 compaction 伪造条目。
>
> **2026-08-23 波 B 回写(代码与自动门禁收口)**:① compact 读取与 retention 二次过滤统一到 active lineage；snapshot 从 append-only event ledger 重建消息并按 lineage 可见水位折叠，父支晚压缩、压缩后从旧边界 late-fork、兄弟支隔离均有回归；host compact 错误显式上抛，无持久化 outcome 水位固定为 0。host 仍先读 lineage 组装 retention inputs，storage 再读同 lineage 做权威范围校验，这是跨 crate 输入边界的有意双检，不是 Provider/branch 特判。② `fork_from_event` 白名单为三类 run 终态 + standalone `CompactionCompleted`，同 tuple 重试幂等；Pi marker 全部折叠为 main 上 `pi.branch_collapsed`（含无 branch_id 的 null 追溯形态），不造零事件 branch。③ protocol 增非 wire `ForkBoundary`，Desktop 渲染与动作入口双重 gate，并在同 session 切 branch 时重置 timeline/seen/锚点基线。④ GLM reviewer 未发现源码 P0–P2；首轮唯一 P1 是漏跑 opt-in compaction tests，补跑 `--features compaction` 后 125 passed / 1 ignored + 5 integration passed；P3 Pi 无 ID marker 已补断言。schema/wire/export v3/波 A fixture 均零 diff。默认 Desktop 构建受本机缺 Metal Toolchain 阻断，改用 `gpui/runtime_shaders` 后 28/28；`cargo check -p pawork` 通过。真实 Provider fork/compact 冒烟未执行，留 §5 阶段人工验收，不阻塞波 C。
>
> **2026-08-23 波 C 回写(K-05 收口)**:① 样本取得——本机两格式真实存在,主代理结构采样(只读键名/类型分布,不取内容)后由 worker 合成脱敏 fixture,非 fail-closed。② 落地:compat 解析双形态——`parse_claude` 自动判定 claude.ai 导出 JSON 与 Claude Code 本地 JSONL(sidechain/thinking/queue-operation/last-prompt 跳过并计数,标题取真实键 `aiTitle`/`customTitle`,未知行 type 落 Raw);`parse_codex` 自动判定平铺 typed entry 与 rollout 信封 `{timestamp,type,payload}`(session_meta.payload.id 取 identity,response_item 含 agent_message/user_message 映射,developer/reasoning/event_msg 镜像跳过,event_msg 仅 token_count→Usage);旧路径逐字节不变;损坏文件(零 record 且有 unparseable 行)fail-closed。③ workspace 新增 `session_scan` 只读发现原语(有界、不跟 symlink、根缺失为空、只取元数据;Claude 排除 `agent-*.jsonl` sidecar,因其 sessionId 复用父会话);CLI `sessions import --from claude|codex` 批量导入经 app facade(design.md §2 依赖边不变),文件级 fail-continue 聚合报告;.jsonl 格式嗅探签名化(codex 信封 / claude sessionId,首行整行读取——8KiB 截断曾被真实 session_meta 大首行证伪)。④ fork 往返回归走真实 `fork_from_event` 生产路径。⑤ 写入集实态扩为 storage(import)+workspace(import)+app facade+cli(接线裁决:config 导入惯例 cli→app→workspace,不加 cli→workspace 依赖边),已回写。验证:storage 108+5 / workspace session_scan 定向 / app 135+6+13+2 / cli 39+16+25 全绿,`cargo check -p pawork` 绿;隔离数据目录真实样本导入 + export 还原 + `--from` 幂等通过。收口审查用本机键级统计坐实 Claude `agent-*.jsonl` sidecar 与父会话共用 sessionId,扫描层已排除;P3 登记 ROADMAP §4。

## 1. ADR-040 决策点(波 0;推荐已列,须用户确认)

| 决策 | 推荐 | 备选 |
| --- | --- | --- |
| 分支模型去留 | **原生化**:events/投影按 branch lineage 一等建模 | 冻结线性模型 + 删除 Fork(产品倒退:`sessions fork`/`chat --branch`/Desktop Fork 均为 S10 已交付能力,不推荐) |
| 建模方式 | 事件账本保持 append-only 单表,`branch_id` + `parent_branch`/fork 点成为**写入时必填**的一等列(不再 DEFAULT 回填);投影按分支物化;lineage 由 storage 层维护,删除 API 外挂式 `ancestor_lineage` 拼装 | 每分支独立事件流(重放/导出复杂度高,不推荐) |
| 迁移 | schema v12(v11 已由 R4 波 B command_ledger 占用):v10/v11 数据一次性迁移(回填即校验),旧库升级 golden;信封 wire 形状不变(envelope v1 保持) | — |
| 压缩语义 | compaction 按分支水位独立计算(修 fork 后压缩误删祖先上下文的结构隐患) | — |

## 2. 目标设计

1. **storage**:`session_events`/投影表 branch 一等化;`fork_from_event` 语义收编(fork = 新 branch 记录 + 祖先引用,不复制事件);lineage 查询单点实现;v12 迁移 + 「V2 真实库文件直接打开升级」golden。
2. **storage/engine**:compact 读取、保留过滤与投影折叠统一按 active lineage；engine 触发保持 branch-neutral,无持久化 outcome 时不得用新发摘要事件作删除水位。
3. **host**:resume/fork/`PersistThenRender` 直接消费 storage lineage 语义；保留既有 `RunStart` 续聊回归并补 fork + compact 变体。
4. **desktop/protocol**:非 wire reducer 标记合法 fork turn 边界；切 branch 必经 snapshot + `reset_baseline`,Timeline 锚点仍只用 `event_id`。
5. **K-05 并入(波 C)**:本机会话导入——`~/.claude/projects/**/*.jsonl` 与 Codex rollout `{timestamp,type,payload}` 两格式,导入为单分支会话;源文件只读、外部源 Secret 前缀拒绝(S9 既有红线);需脱敏样本,缺样本则该波 fail-closed 登记。

## 3. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| 0 | ADR-040 起草(含 v10/v11→v12 迁移方案与回滚不可行性说明)→ 用户确认 | docs/adr/ | 串行 |
| A | storage 原生化 + v12 迁移 + 升级 golden(含 fork 树、多分支交错、压缩折叠种子) | storage | 串行(契约面) |
| B | lineage compact / fork 边界与幂等 / Pi 单分支收编 ∥ desktop 投影分支语义 | storage、engine、app ∥ protocol projection、desktop | 并行 ×2(B1 storage+engine+app / B2 protocol+desktop；schema/wire/fixture 冻结) |
| C | K-05 导入(取得样本后);`sessions import/export` 分支往返回归 | workspace(import)、storage(formats) | 串行 |

## 4. 验证

- **重放 golden 全套**:v10/v11 库升级 v12 后逐事件重放一致;fork 分支 resume 上下文连续;压缩后祖先链读取正确。
- 信封 v1 serde 零 diff(wire 不变的证明);export v3 往返。
- 定向:`cargo test -p pawork-storage --features compaction --offline --lib --tests`；`cargo test -p pawork-engine -p pawork-app -p pawork-protocol --offline --lib --tests`；Desktop unit 走 `cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders`（无 Metal Toolchain 的本机闭环）；宿主接线有改动时追加一次 `cargo check -p pawork --offline`。
- 真实冒烟(矩阵一组):`sessions fork --no-switch` 后 main/fork 两分支 `chat --resume` 各自续聊到指定口令(S10 同款)+ fork 后 `/compact` 再 resume。

## 5. 退出标准

- [x] ADR-040 Accepted;schema v12 迁移 + 升级 golden 绿(波 A,2026-08-23)
- [x] `ancestor_lineage` 外挂、`DEFAULT 'main'` 回填路径与 host compact 吞错回退消失；host/storage 双读保留为 retention 输入组装 + 权威范围校验边界(波 A/B,2026-08-23)
- [x] compact 读取/过滤/投影折叠按同一 lineage；父/子/兄弟双向回归绿且 fixture 字节零 diff(波 B,2026-08-23)
- [x] fork 只接受闭合 turn 后的稳定事件(run 终态 / compaction 完成)、同点重试幂等；Pi 仅产出 main 单分支；Desktop 切支 reset 回归绿(波 B,2026-08-23)
- [x] fork/resume/export v3 自动回归绿(波 B,2026-08-23)
- [x] K-05 完成(两格式真实冒烟通过,2026-08-23);冒烟通过;v3_plan §3 更新
