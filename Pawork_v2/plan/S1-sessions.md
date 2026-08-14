# S1：会话持久化与恢复

> 阶段 S1 · 会话持久化 · 状态：⚪未开始 · 依赖：S0 · 规模：中

## 目标（本阶段结束时用户能做什么）

对话自动落盘为可重放的事件流：`pawork sessions list/show` 查看历史会话，`pawork chat --resume <session>` 续聊且模型记得上下文，进程被杀后恢复无损；新增 `pawork run "<prompt>"` 非交互单次模式与 `--json`（JSONL 事件流输出，**unstable**，S9 对齐正式 headless 协议），从本阶段起所有测试都可脚本化断言事件。

**本阶段把 V2 最重要的冻结契约立起来**：`AgentEventEnvelope`（`schema_version = 1`）与 `session_events` append-only 存储。此后一切功能（GUI 投影、编排、重放、导入导出）都建立在这条事件流上——这是「后期追加不重写」的地基。

## 涉及包与 V1 资产

| V2 包（目录） | 本阶段动作 | V1 来源与方式 |
| --- | --- | --- |
| `pawork-domain` | 增强：V1 `agent-events` 整包并入为 `events` 模块——`AgentEventEnvelope` 全字段 + `AgentEvent` **31 个变体一次迁入**（含暂未消费的 Plan/Goal/Task/Compaction/Checkpoint 等，为 S5/S7/S11 预留位）；`ApprovalDecision`/`ToolOutputStream`/`EventSequence` 辅助类型同迁。serde golden **先于任何消费实现**迁移并通过 | 直接迁移 |
| `pawork-sqlite`（foundation/sqlite) | 激活：V1 `app-database` 纯化版——SQLite Actor 模式 + backup/restore + 通用 migration 框架；不持任何业务 schema | 直接迁移（[archive/M0](archive/M0-skeleton-foundation.md) pawork-sqlite 节） |
| `pawork-session`（storage/session） | 激活（核心）：`event_store`（append + `sequence` 严格连续校验 + parent 校验 + `AppendReceipt`）、**V1 migration 序列全量复用**（v1→v9，`CURRENT_SCHEMA_VERSION = 9`；含 `sessions/session_branches/session_events/messages/runs/tool_calls` 等全部表与 append-only 双触发器）、`projection` 最小子集（从事件重建对话消息，供 resume）。branch 概念随 DDL 存在，UX 只用默认分支 | 直接迁移（[archive/M3](archive/M3-storage-session.md) 细则；compaction/导入器/lifecycle 高级能力分别留 S5/S8） |
| `pawork-engine` | 增强：turn 全程事件化——`RunStarted → AssistantTextDelta/ThinkingDelta → UsageUpdated → MessageCommitted → RunCompleted/RunCancelled/RunFailed`，经 appender 分配 `sequence` 落库，同时推给渲染端（双写）；resume 时从 projection 重建 `initial_messages` | appender 参考 V1 `agent-engine::appender` |
| `pawork-app` | 增强：装配 sqlite + session store；会话生命周期（新建/打开/续聊） | 新写（薄） |
| `pawork-cli` | 增强：`sessions list/show`、`chat --resume <id>`、`run "<prompt>"`（非交互单轮任务模式）、`--json`（每行一个 envelope JSON；stdout 只承载 JSONL，文本与日志走 stderr——stdout 协议纪律自此生效） | 新写 |

## 关键任务

1. **golden 先行**：从 V1 测试夹具原样迁移 `AgentEventEnvelope` serde golden（字节级比对），先绿再动其他。
2. **events 并入 domain**：31 变体 + 信封；`schema_version = 1` 不变；注意信封版本（1）与 DB migration 版本（9）是两个独立版本号，不得混用。
3. **sqlite 迁移**：Actor + migration 框架独立编译、round-trip 测试。
4. **session 核心迁移**：migration 全序列建库；append-only 触发器（`session_events_no_update`/`no_delete`）回归；`append_event` 连续性（`UNIQUE(session_id, sequence)`、`is_immediately_after`）与 parent 悬空校验。
5. **engine 事件化**：单轮对话的完整事件序列定义 + 落库/渲染双写；崩溃恢复 = 重启后 projection 重建上下文续聊。
6. **CLI 扩展**：sessions 子命令、resume、run、`--json`（标注 unstable）。
7. **secret 红线**：credential 及其派生值不进事件 payload、不落库（断言测试）。

## 真实测试与评估（冒烟清单）

- [ ] `pawork chat` 两轮 → 退出 → `pawork sessions list` 出现该会话（时间/摘要正确）→ `--resume` 第三轮提问引用第一轮内容，模型答对（两通道各测一次）。
- [ ] 对话进行中 `taskkill /F` 杀进程 → resume 后上下文完整、无重复/丢失轮次。
- [ ] `pawork run "用一句话解释这个 workspace 是干什么的" --json > events.jsonl`：每行合法 JSON、`sequence` 从 1 严格递增、首行 `RunStarted` 末行 `RunCompleted`。
- [ ] 用 SQLite 工具打开库文件：`session_events` 行数与事件数一致；手工 `UPDATE`/`DELETE` 被触发器拒绝。
- [ ] 观察两模型在 resume 场景下的上下文利用质量（长历史是否遗忘），记录。

## 定向自动化测试

- `cargo test -p pawork-domain`：envelope serde golden（V1 夹具字节等价）、31 变体 round-trip。
- `cargo test -p pawork-sqlite`：Actor、migration 框架 round-trip、backup/restore。
- `cargo test -p pawork-session`：append-only 触发器、sequence/parent 校验、projection 重建等价、**V1 库文件直接打开并升级**（用 V1 测试夹具库）。
- `cargo test -p pawork-engine`：mock provider 单轮事件序列 golden；崩溃点注入（写一半）后 resume 一致性。
- secret 不落库断言：构造含 credential 的运行，扫描库文件与事件 payload 无 key 片段。

## 退出标准

- [ ] envelope golden 与 append-only 契约测试全绿（此后任何阶段不得改动二者形状）。
- [ ] 冒烟清单全项通过（两通道）。
- [ ] V1 样例库文件可被 V2 打开并升级到 `CURRENT_SCHEMA_VERSION`。
- [ ] `--json` 输出可脚本断言（后续阶段的自动化验收基建就绪），且在文档中明确标注 unstable。
- [ ] secret 不落库断言通过。

## 为后续阶段预留 / 明确不做

- 预留：全部 31 事件变体在位；`session_branches`/`tool_calls` 等表随 migration 就绪但未消费；projection 只实现 resume 所需最小面。
- 不做：compaction（S5）、会话导入导出（S8）、lifecycle lease/integrity（S9 多客户端时激活）、分支/Fork UX（落点 S9，数据层本阶段已就绪）。

## 并行拆分建议

- 波 A（串行，契约 owner）：domain events 并入 + golden。
- 波 B（并行 ×2）：`pawork-sqlite`；`pawork-session`（依赖波 A 的事件类型，可先建表层后接类型）。
- 波 C（串行收口）：engine 事件化 + app/cli 接线 + 冒烟。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [../docs/design.md](../docs/design.md) §3.2 契约表（事件信封、会话存储行）
- [archive/M3-storage-session.md](archive/M3-storage-session.md)（session-store 迁移细则、DDL、触发器、migration 序列）
- [archive/M0-skeleton-foundation.md](archive/M0-skeleton-foundation.md)（pawork-sqlite 纯化细则）
