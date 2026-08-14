# S1：会话持久化与恢复

> 阶段 S1 · 会话持久化 · 状态：🟢已完成（2026-08-14 两通道真实冒烟） · 依赖：S0 · 规模：中

## 目标（本阶段结束时用户能做什么）

对话自动落盘为可重放的事件流：`pawork sessions list/show` 查看历史会话，`pawork chat --resume <session>` 续聊且模型记得上下文，进程被杀后恢复无损；新增 `pawork run "<prompt>"` 非交互单次模式与 `--json`（JSONL 事件流输出，**unstable**，S9 对齐正式 headless 协议），从本阶段起所有测试都可脚本化断言事件。

**本阶段把 V2 最重要的冻结契约立起来**：`AgentEventEnvelope`（`schema_version = 1`）与 `session_events` append-only 存储。此后一切功能（GUI 投影、编排、重放、导入导出）都建立在这条事件流上——这是「后期追加不重写」的地基。

## 涉及包与 V1 资产

| V2 包（目录） | 本阶段动作 | V1 来源与方式 |
| --- | --- | --- |
| `pawork-domain` | 增强：V1 `agent-events` 整包并入为 `events` 模块——`AgentEventEnvelope` 全字段 + `AgentEvent` **32 个变体一次迁入**（含 `Diagnostic` 与暂未消费的 Plan/Goal/Task/Compaction/Checkpoint 等，为 S5/S7/S11 预留位）；`ApprovalDecision`/`ToolOutputStream`/`EventSequence` 辅助类型同迁。serde golden **先于任何消费实现**落地并通过。V1 无独立 golden 夹具，本波按迁移词典 §6.1「缺失则补」新建 JSONL/JSON 字节锁 | 直接迁移 |
| `pawork-sqlite`（foundation/sqlite) | 激活：V1 `app-database` 纯化版——SQLite Actor 模式 + backup/restore + 通用 migration 框架；不持任何业务 schema | 直接迁移（[archive/M0](archive/M0-skeleton-foundation.md) pawork-sqlite 节） |
| `pawork-session`（storage/session） | 激活（核心）：`event_store`（append + `sequence` 严格连续校验 + parent 校验 + `AppendReceipt`）、**V1 migration 序列全量复用**（v1→v9，`CURRENT_SCHEMA_VERSION = 9`；含 `sessions/session_branches/session_events/messages/runs/tool_calls` 等全部表与 append-only 双触发器）、`projection` 最小子集（从事件重建对话消息，供 resume）。branch 概念随 DDL 存在，UX 只用默认分支 | 直接迁移（[archive/M3](archive/M3-storage-session.md) 细则；compaction/导入器/lifecycle 高级能力分别留 S5/S8） |
| `pawork-engine` | 增强：turn 全程事件化——`RunStarted → AssistantTextDelta/ThinkingDelta → UsageUpdated → MessageCommitted → RunCompleted/RunCancelled/RunFailed`，经 appender 分配 `sequence` 落库，同时推给渲染端（双写）；resume 时从 projection 重建 `initial_messages` | appender 参考 V1 `agent-engine::appender` |
| `pawork-app` | 增强：装配 sqlite + session store；会话生命周期（新建/打开/续聊） | 新写（薄） |
| `pawork-cli` | 增强：`sessions list/show`、`chat --resume <id>`、`run "<prompt>"`（非交互单轮任务模式）、`--json`（每行一个 envelope JSON；stdout 只承载 JSONL，文本与日志走 stderr——stdout 协议纪律自此生效） | 新写 |

## 关键任务

1. **golden 先行**：锁定 `AgentEventEnvelope` serde golden（字节级比对），先绿再动其他。V1 `agent-events` 只有 4 个 in-crate unit test、无检入 JSON 夹具；本波补建 `tests/fixtures/agent_event_envelope_*.json(l)`。
2. **events 并入 domain**：32 变体 + 信封（文档曾写 31，漏计生产路径上的 `Diagnostic`）；`schema_version = 1` 不变；注意信封版本（1）与 DB migration 版本（9）是两个独立版本号，不得混用。
3. **sqlite 迁移**：Actor + migration 框架独立编译、round-trip 测试。
4. **session 核心迁移**：migration 全序列建库；append-only 触发器（`session_events_no_update`/`no_delete`）回归；`append_event` 连续性（`UNIQUE(session_id, sequence)`、`is_immediately_after`）与 parent 悬空校验。
5. **engine 事件化**：单轮对话的完整事件序列定义 + 落库/渲染双写；崩溃恢复 = 重启后 projection 重建上下文续聊。
6. **CLI 扩展**：sessions 子命令、resume、run、`--json`（标注 unstable）。
7. **secret 红线**：credential 及其派生值不进事件 payload、不落库（断言测试）。

## 真实测试与评估（冒烟清单）

- [x] `pawork chat` 两轮 → 退出 → `pawork sessions list` 出现该会话（时间/摘要正确）→ `--resume` 第三轮提问引用第一轮内容，模型答对（两通道各测一次）。
- [x] 对话进行中杀进程（macOS `kill -9`，对应计划中的 `taskkill /F`）→ resume 后首轮 user 仍在、无重复 user 轮；半轮助手未 `MessageCommitted`（与 engine 约定一致）。`SIGKILL` 来不及写 `RunCancelled`，事件流停在最后一条已 persist 的 delta，下一轮 `sequence` 连续接上。
- [x] `pawork run "用一句话解释这个 workspace 是干什么的" --json > events.jsonl`：每行合法 JSON、`sequence` 从 1 严格递增、首行 `RunStarted` 末行 `RunCompleted`（GLM 本轮 183 行，stdout 无 `session` 横幅）。
- [x] 用 SQLite 工具打开库文件：该 session 的 `session_events` 行数与 JSONL 行数一致（183=183）；手工 `UPDATE`/`DELETE` 被触发器拒绝（`session_events is append-only`）。
- [x] 观察两模型在 resume 场景下的上下文利用质量（长历史是否遗忘），记录。

### 模型评估记录（2026-08-14 S1 冒烟）

隔离目录：`PAWORK_DATA_DIR` + workspace `.pawork/config.toml`（`fixtures/config/config.example.toml`）；凭证从 `Pawork_v2/.env` 注入，未写入仓库。

| 通道 | 模型 | 首轮 / 续聊体感 | resume 暗号 | `sessions list` | 流 / 落盘 | 备注 |
| --- | --- | --- | --- | --- | --- | --- |
| GLM Coding Plan | `glm-5.2` | 首轮 4.8s；数学续聊 1.5s；resume 1.8s | 第三轮只答「蓝松果」 | 时间 + 首轮摘要正确 | 稳定；thinking 走 stderr | 杀进程后续聊仍记得「在数到 80」；模型会把未完成的计数补完（半轮无助手投影，属约定而非丢轮） |
| OpenCode Go | `deepseek-v4-pro` | 首轮 2.7s；续聊 / resume 均 2.7s | 第三轮只答 `BLUE-PINECONE` | 两会话并列，摘要截断正常 | 稳定；同样有 thinking 前缀 | 首轮冒烟曾因跨 session 复用 `msg-1` 撞 `messages.message_id` 全局主键；`AppCore::chat_turn` 落库前重发全局唯一 user id 后通过 |

`--json`（unstable）：`RunStarted → MessageCommitted(user) → ContextPrepared → ProviderRequestStarted → deltas → MessageCommitted(assistant) → RunCompleted`。S1 无工具，workspace 解释题只验 JSONL 形状，不验仓库事实。

## 定向自动化测试

- `cargo test -p pawork-domain`：envelope serde golden（补建夹具字节锁）、32 变体 round-trip。
- `cargo test -p pawork-sqlite`：Actor、migration 框架 round-trip、backup/restore。
- `cargo test -p pawork-session`：append-only 触发器、sequence/parent 校验、projection 重建等价、合成 v6 库升级到 schema 9（仓库无检入 V1 `.db`）。
- `cargo test -p pawork-engine`：mock provider 单轮事件序列 golden；崩溃点注入（写一半）后 resume 一致性。
- secret 不落库断言：构造含 credential 的运行，扫描库文件与事件 payload 无 key 片段。

## 退出标准

- [x] envelope golden 与 append-only 契约测试全绿（此后任何阶段不得改动二者形状）。
- [x] 冒烟清单全项通过（两通道）。
- [x] V1 schema 升级到 `CURRENT_SCHEMA_VERSION = 9`：波 B 以合成 v6 库打开并升级（`legacy_sessions_backfill_to_local_default_identity`）；仓库未检入 V1 生产 `.db` 夹具。
- [x] `--json` 输出可脚本断言（后续阶段的自动化验收基建就绪），且在文档中明确标注 unstable（任务书目标、`design.md` §3.2/§4、clap `about`）。
- [x] secret 不落库断言通过。

## 为后续阶段预留 / 明确不做

- 预留：全部 32 事件变体在位；`session_branches`/`tool_calls` 等表随 migration 就绪但未消费；projection 只实现 resume 所需最小面。
- 不做：compaction（S5）、会话导入导出（S8）、lifecycle lease/integrity（S9 多客户端时激活）、分支/Fork UX（落点 S9，数据层本阶段已就绪）。

## 并行拆分建议

- 波 A（串行，契约 owner）：domain events 并入 + golden。
- 波 B（并行 ×2，已完成）：`pawork-sqlite`；`pawork-session`（依赖波 A 的事件类型，可先建表层后接类型）。
- 波 C（串行收口，已完成）：engine 事件化 + app/cli 接线 + 冒烟。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [../docs/design.md](../docs/design.md) §3.2 契约表（事件信封、会话存储行）
- [archive/M3-storage-session.md](archive/M3-storage-session.md)（session-store 迁移细则、DDL、触发器、migration 序列）
- [archive/M0-skeleton-foundation.md](archive/M0-skeleton-foundation.md)（pawork-sqlite 纯化细则）
