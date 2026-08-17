# S11：工作流、多 Agent 与控制面

> 阶段 S11 · 编排与治理 · 状态：🔵进行中（波 A ✅ · 波 B ✅）· 依赖：S10（app/cli 正式化、Event Hub）· 规模：大

## 目标（本阶段结束时用户能做什么）

单 Agent 工具升级为可编排、可治理的系统：Plan 模式（计划步骤经审批 gate 拦截未批准的 turn）；automation/monitor 后台任务经 CLI 列出/查看/取消；多 Agent 编排最小闭环（supervisor 派发 ≥2 个子 Agent——**真实场景：GLM 与 OpenCode Go 各驱动一个子 Agent 并行处理独立子任务**、cancel-tree、budget-gate 限流）；`pawork usage` 查询用量与配额；评审（行锚点 re-anchor + resolution 生命周期）与记忆抽象就位。「无消费者不合入」逐包执行，接不上真实消费者的以 `experimental` feature 登记。

## 涉及包与 V1 资产

全部细则以 [archive/M7](archive/README.md) 为主文档，本表只记增量决策：

| V2 包（目录） | 本阶段动作 | 真实消费者 |
| --- | --- | --- |
| `pawork-workflow`（workflow/core） | plan/goal/task/automation/monitor 五合一迁移，各域独立 reducer；`process-exec` feature 门控 | Plan 审批 gate（engine turn 组装前拦截）+ 后台任务 CLI 可见 |
| `pawork-orchestration`（agents/orchestration） | orchestration + teams 迁移；supervisor.rs 拆 spawn/registry/cancel-tree/recovery/budget-gate 五模块；budget 依赖 trait 化注入 | 多 Agent 最小编排 demo（双通道双子 Agent） |
| `pawork-control-plane`（control-plane/core） | tenant/usage-ledger/audit-log 三合一；`dedup_key` 索引与 audit JSONL golden 先行；usage 投影 trait（quota 与 budget-gate 的消费源） | S5 的会话用量聚合接入 ledger；audit 记录审批/通道事件 |
| `pawork-quota`（control-plane/quota) | 核心迁移（domain/service/ledger 投影 + LocalLedger）；**远端适配器约 8k 行冻结候审不迁** | `pawork usage` 子命令 |
| `pawork-provider-control`（control-plane/provider-control） | `account-control` feature 分层迁移；控制面 schema 迁移收回本包（`pawork-sqlite` 纯化承诺兑现）；lease/binding 经 Provider factory 消费，account/routing/health 无消费者则 experimental 登记 | Provider 绑定/租约 |
| `pawork-review`（workflow/review） | re-anchor + resolution 生命周期 + 平台无关 ForgeAdapter trait | 会话内评审最小流（对 S8 diff 的评论→re-anchor→resolve），Forge 实接可 experimental |
| `pawork-memory`（workflow/memory） | Provider 无关记忆抽象迁移；EmbeddingProvider（`provider-api` 已有 trait）若无真实实现则 **experimental 登记** | 待定（登记激活条件） |

## 关键任务

1. **控制面契约 golden 先行**：usage `dedup_key`、audit JSONL 形状逐字节一致（V1 golden 随迁）。
2. **Plan gate**：plan step 审批状态在 turn 组装前校验，决策经 policy/approval 位点、事件化（`Plan` 事件变体 S1 起在位）。
3. **多 Agent demo**：两个子 Agent 分别配置 GLM 与 OpenCode Go，父 Agent 分派两个独立文件任务 → 并行执行 → 汇总；中途 cancel-tree 一键全停；budget-gate 在 Mock ledger 限额下触发限流。
4. **`pawork usage`**：会话/累计用量 + 配额余量（LocalLedger），与 S5 的显示同源。
5. **experimental 纪律**：接不上消费者的包/层显式 feature 门控 + 在 [../ROADMAP.md](../ROADMAP.md) §4 登记激活条件（严禁静默库存）。

## 真实测试与评估（冒烟清单）

- [ ] Plan 模式真实任务：「先列计划再执行」→ 未批准步骤被 gate 拦截 → 逐步批准推进 → 完成；中途改计划重新走审批。
- [ ] 多 Agent demo（双通道）：并行完成、事件流按子 Agent 隔离可读、`TeamEvent` 双通道语义正确；cancel-tree 后无残留进程/运行。
- [ ] 后台 automation 任务（如定时跑测试）：`pawork tasks list/status/cancel` 可见可控。
- [ ] `pawork usage`：数字与本阶段真实消耗对得上（与 S5 显示一致、与厂商侧抽查一致）。
- [ ] **评估记录**：两模型作为「子 Agent worker」的可靠性对比（任务完成率、越权尝试率）；多 Agent 并行的 token 成本与收益。

## 定向自动化测试

- `cargo test -p pawork-workflow`：各 reducer 独立单测 + 事件重放投影一致；`process-exec` 两档构建。
- `cargo test -p pawork-orchestration`：supervisor 拆分后行为等价（spawn/cancel-tree/recovery/budget-gate）、`TeamEvent` golden。
- `cargo test -p pawork-control-plane`：`dedup_key`/audit JSONL golden、feature 门控两档、usage 投影 trait round-trip。
- `cargo test -p pawork-quota`：LocalLedger 累计/触限。
- `cargo test -p pawork-provider-control`：feature 两档、schema 迁移 round-trip（V1 库可升级）。
- `cargo test -p pawork-review` / `pawork-memory`：re-anchor 漂移修正、resolution 状态机；Mock Embedding 写入→召回。

## 退出标准

- [ ] Plan gate、后台任务可见、`pawork usage`、多 Agent demo 四个真实消费点全部通电。
- [ ] 控制面冻结契约 golden 全绿（dedup_key/audit JSONL 与 V1 逐字节一致）。
- [ ] 七包「无消费者不合入」逐包核对：接线或 experimental 登记，无静默库存。
- [ ] quota 远端适配器确未迁移（冻结清单核对）；supervisor 拆分行为等价回归通过。

## GUI 增量

按 [gui-design.md](../docs/gui-design.md) §5：Workflow / 用量条 / 子 Agent 时间线分组。无对应 Core 事件则不做按钮。

## 为后续阶段预留 / 明确不做

- 预留：tenant 多租户词表在位（单机默认单租户）；ForgeAdapter 真实平台实现按需求激活。
- 不做：quota 远端六厂商适配器 + WebScrape（冻结候审）；分布式编排；插件编排面。

## 并行拆分建议

沿用 [archive/M7](archive/README.md) 四波：波 A 控制面三包（trait 先行）∥ 波 B 工作流三包（✅）→ 波 C orchestration（依赖波 A trait）→ 波 D host 接线（Plan gate/usage/demo，单一 owner 串行）。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [archive/M7-workflow-control.md](archive/README.md)（本阶段主文档：七包迁移细则、四波派发、experimental 纪律）
