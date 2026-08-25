# ADR-037：S13 波 B 契约 / 红线决策

- **状态**：Accepted
- **日期**：2026-08-18
- **落实日期**：S13 波 B

## 背景

S12 全项目 Code Review 将五条契约 / 红线级 finding 登记为须先拍板再改代码（原 S13 任务书已随 V2 归档删除，整改原委见 [history.md](../history.md) S13 整改节，原 v2-summary §5）。用户于 2026-08-18 确认下列五支。信封 `schema_version = 1`、`UNIQUE(session_id, sequence)`、append-only 事实表不在本 ADR 范围内（F09 已钉死）。

## 决策

### F15 — `SessionRegistryStore` 归 `pawork-domain`

词典 §4.1 #13 承诺的 trait 倒置按「记录类型 + store trait 下沉 domain」落实：

- `ClientSessionId` / `ClientProtocol` / `ClientCapability` / `CapabilitySnapshot` / `ClientSessionState` / `ClientSessionRecord` / `RegistryWriteOutcome` / `SessionRegistryStore` 迁入 `pawork-domain`。
- trait 错误用 domain 侧类型，不引用 `AdapterError`。domain 只增加 `async-trait`，不引入 tokio / SQLite / protocol。
- `pawork-session` 实现 SQLite store，**去掉**对 `pawork-protocol` 的依赖。
- `pawork-protocol::adapter` 保留 `SessionRegistry` / `InMemorySessionRegistryStore` / 编解码，对 domain 类型做映射与 re-export。
- `host/cli` ACP 装配点改用 domain trait + session 实现。
- 不改 `client_adapter_sessions` DDL，不改 GUI 帧语义。

否决支：归 session（会把 protocol/channels 拉进 SQLite）；维持现状（词典说谎）。

### F19 — 维持 ADR-031 可观测回退

源码已按 [ADR-031](../../../Pawork_v1/docs/adr/ADR-031-sandbox-backend-architecture.md)（归档）在硬隔离不可用时回退 `NativeRestricted`，禁止静默降级。本波：

- **不**改选择器为拒跑（Windows 无硬隔离，且 `--sandbox off` 不存在）。
- 把该口径写回 [docs/design.md](../design.md) 与原 S4 任务书第 24 行（该任务书已随 V2 归档删除；回写当时已完成）。
- CLI / GUI 必须把 fallback / isolation 展示给用户，不能只留工具 metadata。

### F24 — 扩 `ToolResultContent.artifacts`

附加式字段：`#[serde(default, skip_serializing_if = "Vec::is_empty")]`。`ToolExecutionCompleted.result` 与 tool message 同路携带。不新增 `AgentEvent` 变体，不改 32 变体表。engine 映射 `ToolResult.artifacts`；`ArtifactAvailable` 流式通知仍可不单独落盘（终态事件已有引用）。golden 先行：空向量不改变现有 32 变体夹具字节；另补非空 artifacts 往返测试。

### F26 — `PlanEvent::Revised` 携带 `title` / `steps`

附加式：`#[serde(default)]`。保留修订链语义，不用 `Replaced` 顶替。`revise()` 接收新内容；重复 version ID 拒绝。host 仍无 revise 入口（本波不改 Draft→InReview 自动提交）。

### F28 — `ResultArchived` 加 `task_id`，幂等键 `(automation_id, task_id)`

附加式 optional `task_id`。新写入必填。重复 `record_result` 对同一键 no-op：不新增事件、不重复累计 failure streak。inbox 视图已按该键覆盖，保持不变。

## 后果

- 事件载荷 serde 仅附加式演进；旧 `session_events` 行可解码。
- F15 落实后 `rg pawork_protocol storage/session/src` 不得再有生产命中（测例 import 一并改 domain）。
- F19 与 S4「绝不静默裸跑」的字面冲突以本 ADR + ADR-031 为准：裸跑若发生必须对用户可见。
- 编号续接 V1；本仓自本文件起维护 `docs/adr/`。

## 相关

- S13 整改总结见 [history.md](../history.md)（原 v2-summary §5）；S12 审查报告 CR-01/CR-03/CR-06 已随文档重构删除，全文见 git 历史 `docs/reviews/s12/`，结论摘要见 [history.md](../history.md) S12 审查节
- [ADR-016](../../../Pawork_v1/docs/adr/ADR-016-core-event-persist-replay.md)（归档）· [ADR-018](../../../Pawork_v1/docs/adr/ADR-018-large-payload-artifact-id.md)（归档）· [ADR-031](../../../Pawork_v1/docs/adr/ADR-031-sandbox-backend-architecture.md)（归档）· [ADR-033](../../../Pawork_v1/docs/adr/ADR-033-control-plane-separation.md)（归档）
