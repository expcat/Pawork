# R4 — 宿主拆解与可靠性内核(T2 + T8 + T9)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R4 行。根因:V2 增量顺序把每阶段能力都挂在 `AppCore` 一个结构体上,形成 `host/app/src/lib.rs` 4,057 行 + `gui_host.rs` 2,594 行单体;并发/幂等问题就地打点(内存 CAS、9 张 Mutex map、序列补洞);降级路径静默吞错(全仓 323 处 `let _`/`.ok()`)。本阶段在 R3 的 registry 与协议 golden 护航下拆解宿主,并建立幂等持久化与降级可观测两个可靠性内核。

## 1. 现状证据(执行时重验;路径为 R1 合并后位置)

- **单体**:AppCore 承载 resume/compact/usage/checkpoint/task/approval/idempotency 全部;`CatalogOnlyProvider` 兜底假 provider(原 `host/app/src/lib.rs:265`);`RETAINED_MESSAGES` 等横切常量散置。
- **幂等**:`idempotency.rs` 内存 CAS(F31 修竞态但重启失忆;tenant 冻结 `local/default`);`gui_host.rs:930` `let _ = idempotency.record(...)` 吞 DuplicateCommand/KeyConflict;幂等作用域按 GUI `client_id`(S10 修的撞车问题)。
- **吞错热点**:`lib.rs:1325,1334` `let _ = tasks_finish(...)`;`data_dir.rs:22-26` HOME 缺失静默回退 `temp_dir()`(会话库落临时目录无告警);gui-server 断连清理 20+ 处零观测。
- **ACP host**:40 处 `.expect("…mutex")`(毒锁 panic 整通道)、`prompt_gate` 全局串行锁、9 张独立 Mutex map、Reserved/Active 手搓状态机(R1 后位于 cli `channels/acp/`)。
- **usage 哨兵**:`control.rs:150-176` `upstream_attempt: Some(1)`/`trace_id: None` 硬填(D1 单机决议后收敛语义)。
- **K-02**:`ToolApprovalRequested` 进入用户等待前不落盘,崩溃后 resume 语义缺失(时序注释在 gui_host)。

## 2. 目标设计

1. **领域服务拆分**:AppCore → `SessionService` / `RunService` / `ApprovalService` / `UsageService` / `TaskService` / `ImportService` / `ExtensionService`(MCP/resources),每服务自持状态与横切常量;`gui_host.rs` 巨 match 改 R3 registry 分发,目标 `lib.rs` <1,500 行、`gui_host.rs` <800 行。`CatalogOnlyProvider` 兜底改显式「无凭证」状态(配合降级事件)。
2. **幂等持久化(CommandLedger)**:幂等表入 SQLite(与会话库同 Actor 栈,storage 新增 `command_ledger` 迁移——新表不动既有 DDL);作用域 `(client_id, command_id)`;重启后可查;record 失败不再吞错。**K-02 并入**:`ToolApprovalRequested` 在进入等待前持久化,定义崩溃/`kill -9` 后 seal → resume 呈现待审批 → 决策不重复执行的语义(定向回归:审批中 kill -9 → resume → deny → 工具未执行)。
3. **ACP actor 化**:单 actor 循环 + 消息信箱替换 9 张 Mutex map;`expect` 清零(错误进降级事件);prompt 串行语义由 actor 队列天然保证。
4. **降级可观测契约(T8)**:定义 `DegradeEvent`(或复用 Diagnostic 通道):HOME→temp 回退、无凭证兜底、Lagged 断流、tasks_finish 失败、幂等冲突等一律事件化(进事件流或 stderr 诊断,按敏感度分级);建立「副作用 Result 禁 `let _`」清单——本阶段清理 host 域全部命中点,其余包登记到 R9 复查。

## 3. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| A | 服务拆分(纯代码组织,行为零变化;每拆一块跑 app 契约测试) | host/app(R1 后 `pawork-app`) | 串行(单一 owner;心脏手术不并行) |
| B | CommandLedger 持久化 + K-02 审批落盘语义(storage 迁移 + app 接线 + resume 语义测试) | storage(新迁移)、app(idempotency/approval) | 串行(依赖波 A 的 ApprovalService 边界) |
| C | ACP actor 化 ∥ 降级事件契约(DegradeEvent 定义在 protocol/domain 侧 + host 全部接点) | cli `channels/acp/` ∥ protocol/domain(事件定义)+ app(接点) | 并行 ×2(写入集不相交;DegradeEvent 契约面由主代理先定形状) |
| D | 收口:`let _` 清理(host 域)、HOME 回退告警、usage 哨兵语义按 D1 收敛、hub 序列逻辑简化(rate_limit 已删) | app、cli | 串行 |

## 4. 验证

- app 契约测试(V2 已有 88+ 条)全绿是波 A 的硬门;拆分前后 `--json`/GUI 帧行为快照对比。
- 幂等:双进程/重启重放定向测试;K-02 的 kill -9 → resume → 不重复执行回归。
- ACP:Zed 冒烟 + actor 化后的并发 prompt 压测种子(两客户端交错)。
- 降级:每类 DegradeEvent 一条触发测试(HOME 缺失、无凭证、Lagged)。
- 真实冒烟(矩阵一组):chat/审批/取消/resume/fork/usage 对账。

## 5. 退出标准

- [ ] AppCore 拆为领域服务;巨 match 消失(registry 分发);行数目标达成
- [ ] 幂等持久化 + K-02 语义落地并有崩溃回归;内存 CAS 删除
- [ ] ACP 无 Mutex map/`expect` 热点;降级事件契约生效且 host 域 `let _` 清零
- [ ] app/cli/storage 定向测试全绿;冒烟通过;v3_plan §3 更新
