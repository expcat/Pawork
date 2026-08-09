# Session 系统

## 职责

以事件为事实来源管理会话，支持分支、恢复、压缩、导入导出；大型内容走 Blob Store。

## 存储决策

新核心不以 JSONL 为主存储，采用：

> **SQLite Event Store + Materialized Projections + Content-addressed Blob Store**

Pi 的 Session 文件仅作导入来源，不继续双写。

## 核心数据表

```text
workspaces / sessions / session_branches / session_events
messages / runs / provider_calls / tool_calls / tool_approvals
compactions / attachments / artifacts / checkpoints
model_profiles / plugin_state / mcp_servers / settings / audit_events
```

## Event Store

`session_events` 是事实来源。每个事件包含：`event_id`、`session_id`、`branch_id`、`parent_event_id`、`sequence`、`event_type`、`schema_version`、`timestamp`、`payload`。其他表是可重建 Projection。

## Session 功能

创建；打开；关闭；重命名；归档；删除；恢复；Fork；Branch；切换 Branch；从任意事件创建新 Branch；查看 Session Tree；导出；导入；标签；搜索；Interrupted Run 恢复；Session Lease；并发写保护；Schema Migration；损坏检测；Projection 重建。

## Pi 导入器

扫描 Pi JSONL；解析 Header；导入消息；导入 Tool Calls；导入模型切换；导入 Compaction；导入 Branch；导入自定义 Entry；保存未知字段；产生迁移报告；**不修改原始 Pi 文件**。导入后使用新数据库。

## Blob Store

大型内容（Tool Output、图片、文件快照、Provider 原始响应、Diff、日志、导出文件）以 BLAKE3 内容寻址存储，支持去重、引用计数、完整性校验、可配置保留期限、GC、最大磁盘预算。

```text
blobs/
└── ab/
    └── cd/
        └── <blake3-hash>
```

## Protected Blob 与现代工作流

存储分为三条边界：Event Store 保存可重放业务事实；普通 Blob Store 保存大型非敏感内容；[Protected Blob Store](../adr/ADR-032-protected-blob-store.md) 保存 reasoning signature / encrypted continuation 等敏感制品。`session_events` 只保存 `ReasoningItem.protected_blob_ref` 与脱敏摘要，不保存明文、密钥或解密句柄；GUI、导出文件、日志、诊断包和 OS Keychain 均不得收到 blob 内容。

Protected Blob 按 Provider + Session 作用域校验，采用独立引用计数、retention、完整性验证与 GC。Fork / Branch / retry / compaction / crash recovery 必须原子调整引用；活动 continuation 仍引用时不可回收，密钥缺失或密文损坏时产生显式脱敏错误，禁止回退普通 Blob/Event 明文。

Plan、Goal、BackgroundTask、Automation、Monitor、Memory、Review 与 Hook 状态都以 canonical event 为事实源；Projection 可删除重建，后台与无人值守流程在宿主重启后按事件恢复，不依赖 GUI 在线。

## 数据库设置

WAL；Foreign Keys；Busy Timeout；Migration；定期 Checkpoint；Integrity Check；Vacuum 策略；备份；只读恢复模式。推荐专用数据库 Actor，而非在任意 Tokio Task 直接并发操作。

## Phase 1 存储基线

`app-database` 已实现单连接、专用线程和有界异步命令通道，并启用 WAL、Foreign Keys 与 Busy Timeout；支持一致性备份、恢复和只读恢复模式。`session-store` 已实现只向前迁移、升级前备份、append-only Event Store、严格连续 sequence、尾部读取和可删除重建的 Projection。`artifact-store` 已实现 BLAKE3 内容寻址、持久化引用计数、完整性检查、磁盘预算与仅回收零引用 Blob 的 GC。

## Migration 原则

Migration 只向前；每次升级前备份；Migration 可恢复；Projection 可删除重建；Event Store 不可破坏；插件状态独立版本；导入器版本单独记录。

## Phase 5 Session 基线

`session-store`（schema v4）已实现 Session Tree / Fork（从任意事件分叉、按 branch 分页读取事件）、Branch 切换（active_branch + 并发写保护）、完整生命周期（rename / archive / unarchive / delete / resume）、Session Lease（过期可抢占）、损坏检测（只读 parent 缺失 / sequence 间隙检测）以及搜索与标签。内容搜索只反序列化并匹配 `Text` part，snippet 不暴露原始 JSON；`replay_events` / `tail_events` 明确定义为整 session 语义，分支消费者使用 `events_by_branch`。

Export / Import schema v2 为每条事件显式保存 `branch_id`，多分支往返不改变归属；读取器仍兼容 v1，并把历史上无法判定归属的事件迁移到 main。`compaction-engine` 按目标 branch 折叠事件并保留 recovery fork 语义。Pi JSONL Importer（`import_pi_jsonl`）使用 Tokio 逐行读取，按真实行号保存多条未知记录，将 ModelSwitch 持久化为诊断事件，生成迁移报告且不修改原文件（ADR-005）。

## 验收标准

- 事件可重放、Projection 可重建
- 大型 Session 不需全量读取即可打开尾部
- 崩溃后 Session 可恢复
- Pi 导入测试通过
- Protected Blob 明文不进入 Event/Projection/导出/日志/Keychain，引用在 Fork/compaction/GC/crash 后保持一致

## 相关文档

- [领域模型](../architecture/domain-model.md) · [artifacts](artifacts.md) · [checkpoint](checkpoint.md)
- [ADR-003 Event Store](../adr/ADR-003-sqlite-event-store.md) · [ADR-004 Blob Store](../adr/ADR-004-blob-store.md) · [ADR-005 Pi JSONL 导入](../adr/ADR-005-pi-jsonl-import-only.md) · [ADR-016 事件可重放](../adr/ADR-016-core-event-persist-replay.md)
- [ROADMAP Phase 1 / Phase 5](../../ROADMAP.md)
