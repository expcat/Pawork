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

Protected Blob 按 Provider + Session 作用域校验。当前实现对每次写入生成随机逻辑 ref 与 XChaCha20-Poly1305 nonce，以密文 BLAKE3 摘要作物理地址；AEAD AAD 绑定 Provider、Session、逻辑 ref 与 key version。版本化 `ProtectedKeyResolver` 由组合层注入，轮换只替换密文地址，不改变逻辑 ref 或引用计数。解密结果与数据密钥在 drop 时清零，`Debug` 只显示脱敏占位；缺引用、跨 scope、缺文件或缺密钥返回 `ProtectedBlobUnavailable`，摘要、envelope 或认证标签异常返回 `ProtectedBlobCorrupted`，均禁止回退普通 Blob/Event 明文。Reasoning metadata 在 Event Store 边界走精确 allowlist：只保留已验证形状的 summary entries / block kind hint，未知键与嵌套敏感载荷按原形状脱敏。

写入遵循 create-before-reference：先提交 `pending` 元数据，原子发布并同步加密文件，再标记 `ready`，只有 `ready` 条目可解析；启动恢复会回滚残留 `pending`、完成 `deleting` 并清理无元数据密文。GC 先把到期零引用条目标记为 `deleting`，再删文件和元数据，任一 crash 窗口都可在下次打开或 GC 时继续。首次 `put` 的 `ref_count = 1` 就是首个持久化事件的所有权，成功 append 不重复 `retain`；正常 append 失败由上层调用 `release` 回滚未提交首引用，新增真实所有者才 `retain`，事件物理删除时 `release`。进程在 blob 已 `ready`、事件尚未提交之间异常退出，仍可能留下带元数据且保守持有初始引用的未挂接加密条目；它不会产生悬空事件引用，取舍是暂占空间而不是丢失 continuation，需由后续宿主维护核对后释放。引用降到零后进入 retention（默认 7 天），到期才允许 GC。当前 Fork / Branch 共享不可变历史事件，Compaction 也只保留或隐藏事件而不物理删除，因此不复制、不递减 reasoning blob 引用；未来若物理删除事件，删除事务必须显式释放对应引用。

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

## Phase 15 Reasoning 存储基线

`protected-blob-store` 已实现独立 SQLite 元数据、加密文件命名空间、作用域隔离、随机化密文寻址、可恢复两阶段写入/删除、版本化密钥（key version，公开在线轮换 API 按 [P15-10](../../plan/P15-10-review-remediation.md) 删除）、引用计数、retention GC、磁盘预算内部约束与 crash reconcile（公开 `integrity_check` / `disk_usage` 按 P15-10 删除，等真实需求出现再加）。`provider-runtime::reasoning::ReasoningProtector` 统一三家保护抽象：`InMemoryReasoningProtector` 为默认实现（openai / xai / anthropic 共享同一实现）、`ProtectedBlobStoreProtector` 为标准持久实现（构造时经 `new(store, scope)` 捕获 store 与 `BlobScope`；API 为 `protect(payload) -> ProtectedBlobRef` + `resolve(&blob_ref) -> ProtectedBlob`，不逐次传 scope）；`retain` / `release` / `gc` / `shutdown` 生命周期由 protected-blob-store 自身提供，上层以 `release` 回滚 append 失败的未提交首引用，`ReasoningProtector` 不解析 Provider wire 格式；事件持久化 / Projection / crash replay 只处理 `ReasoningItem` 的安全引用。三家 wire 翻译分别留在 `provider-openai`、`provider-anthropic` 与 `provider-xai`，Core 不按 Provider 名称分支。

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
