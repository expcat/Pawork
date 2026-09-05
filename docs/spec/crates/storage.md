# pawork-storage

> 存储层：串行 SQLite Actor、Session Event Store（append-only 事件账本 + 分支 + 投影 + 幂等账本 + 导入导出 + compaction）与内容寻址 Blob 三区（artifact / protected / checkpoint）。位于 domain 之上、engine/app 之下：只依赖 [pawork-domain](domain.md) 与基础设施 crate，不依赖任何 Provider、GUI、HTTP。

## 1. 职责与边界

- **sqlite/**（常开）：把一个 `rusqlite::Connection` 关进专用 OS 线程，经有界命令通道串行执行所有读写（Actor 模型）；附带通用的命名空间化 migration 框架（账本表 + 事务整批 + 迁移前备份）。
- **session/**（feature `session`，默认开）：Agent 会话事件的持久化事实源。`session_events` 是唯一账本，消息/运行/工具调用等物化表均为可重建投影；原生支持分支树（fork/lineage）、导出导入（export v3 / Pi JSONL / Claude·Codex·Grok·Cursor compat）、命令幂等账本（CommandLedger）与可选 compaction 引擎。
- **blob/**（feature `blob`，默认开）：BLAKE3 内容寻址 Blob Store（artifact 区），以及 opt-in 的 AEAD 加密 protected 区与基于 Blob 的写前快照/回滚 checkpoint 区。
- **不做的事**：不发起网络请求、不认识具体 Provider、不做 Policy 判断（只负责"给到我的数据安全落盘"，Secret 脱敏是最后一道防线而非唯一防线）；不暴露裸 `Connection` 给调用方；compaction 引擎只产出决策与快照，不追加事件、不删账本（事件删除只发生在投影层折叠）。

磁盘布局（三套互不相干的持久化根）：

| 存储 | 位置 | 内容 |
| --- | --- | --- |
| session | 调用方指定的单个 SQLite 文件 | 账本 + 投影 + 幂等/导入/registry 全部表；迁移备份 `<db>.pre-migration-v<N>.bak` 同目录 |
| artifact | `<root>/blobs/ab/cd/<blake3-hex>` + `<root>/artifacts.sqlite3` | 明文内容寻址 blob + 元数据；`CheckpointService` 的 `checkpoint-state-v1.json` 也落在同一 root |
| protected | `<root>/protected/<密文 digest>` + `<root>/protected.sqlite3` | PWB1 密文文件 + scoped 元数据 |

## 2. 模块与文件地图

共 29 个 `.rs` 源文件（约 1.92 万行）+ 2 个集成测试 + 8 个 fixture/golden 数据文件。单元测试内嵌于各源文件尾部 `#[cfg(test)] mod tests`（21 处），没有独立单元测试树。

| 路径 | 行数 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~10 | crate 根：声明 `sqlite` 常开、`session`/`blob` 按 feature；**根层无 re-export**，调用方必须写全路径（如 `pawork_storage::session::SessionStore`） |
| `src/sqlite/mod.rs` | ~400 | `DatabaseActor`（专用线程 + `sync_channel(128)` 命令队列）、`DatabaseOptions`、`DatabaseError`、`backup_to`/`restore_from`、只读打开 |
| `src/sqlite/migration.rs` | ~650 | 通用 migration 框架：`Migration`/`migrate`/`schema_version`/`MigrationReport`/`MigrationError`；账本表按命名空间隔离（不用 `PRAGMA user_version`），升级前 `.pre-migration-v<from>.bak` 备份，单事务整批应用 |
| `src/session/mod.rs` | ~230 | `SessionStore`（open/open_read_only/schema_version/shutdown）、`SessionStoreError` 全量错误枚举、子模块 re-export（`compaction` 随 feature） |
| `src/session/migration.rs` | ~1280 | `CURRENT_SCHEMA_VERSION = 14` 与 v1–v14 迁移清单；内嵌 v12–v14 迁移测试、升级 golden 断言与孤儿 fail-closed 回归 |
| `src/session/event_store.rs` | ~2250 | 事件读写核心：`create_session(_with_identity/_with_workspace)`/`create_branch`/`switch_branch`/`append_event`/`replay_events`/`tail_events`/`events_by_branch`；`set_session_workspace`（ADR-043 既有会话归属写穿）；写前 Secret 脱敏（`redact_sensitive_json`、`sanitize_reasoning_metadata`）与 legacy provider hint 键只读映射；`persist_event_in_transaction` 供导入复用 |
| `src/session/projection.rs` | ~1590 | 投影写入 `apply_projection`、读取 `ProjectionSnapshot`（messages/runs/tool_calls/server_tool_events/program_output/screenshots/transcript_envelopes）、compaction 水位折叠、`rebuild_projection` |
| `src/session/session_tree.rs` | ~580 | 分支树单点：`load_ancestor_lineage`/`visible_on_lineage`/`events_on_lineage`/`fork_from_event`/`session_tree`；fork 边界校验与幂等 |
| `src/session/catalog.rs` | ~290 | 会话目录：`list_sessions`/`get_session` 与 `SessionRecord`（v13 起含 `workspace_id: Option<String>` 归属弱引用）；`rename_session`/`archive_session`（ADR-054：更新 title/archived 与 `updated_at_ms`，缺失报 `SessionNotFound`）；`list_session_workspace_bindings` 返回含 archived 的全部非 NULL 绑定；v14 起含项目注册表 `WorkspaceRecord` 与 `register_workspace`/`list_workspaces` |
| `src/session/command_ledger.rs` | ~730 | `CommandLedger`：`check`/`record`/`release`/`reclaim_inflight`/`stats`，容量 4096 全局淘汰；`waiting_tool_call(s)` 审批恢复查询 |
| `src/session/client_adapter.rs` | ~530 | `SqliteClientSessionRegistryStore`：以 SQLite 实现 domain 的 `SessionRegistryStore`（load_all/insert/compare_and_swap/remove_if_owner，乐观并发） |
| `src/session/test_support.rs` | ~340 | `cfg(test)` 种子场景（fork_tree/interleaved/compaction），供迁移 golden 复现历史库形态 |
| `src/session/compaction/mod.rs` | ~60 | feature `compaction` 门面：`TokenEstimator` trait（依赖倒置，估算器由 engine 侧注入）、`CompactionError`、re-export |
| `src/session/compaction/engine.rs` | ~760 | `CompactionEngine::compact`：读 lineage → 建 recovery 分支 → 套 retention 策略 → 产出 `CompactionSnapshot`（不写事件流） |
| `src/session/compaction/retention.rs` | ~720 | `RetentionPolicy`/`RetentionInputs`/`RetentionDecision` 与纯函数 `apply`（last-N-turns、未决任务、用户约束、改动文件、pending/failed 工具调用等保留规则） |
| `src/session/compaction/snapshot.rs` | ~150 | `CompactionSnapshot` v1（serde 形状冻结）与 `SnapshotVersion` |
| `src/session/import/mod.rs` | ~15 | 导入门面：re-export formats 解析层，声明 persist_* 持久化层 |
| `src/session/import/formats/mod.rs` | ~15 | formats 门面 re-export |
| `src/session/import/formats/pi.rs` | ~270 | Pi JSONL 纯函数解析 `parse_pi_line`（Header/Message/ToolCall/ModelSwitch/Compaction/Branch/Custom/Unknown）与 `PiImportReport` |
| `src/session/import/formats/compat.rs` | ~1930 | 外部会话解析：`ExternalSource`（Claude/Codex/Grok/Cursor）各自解析器与 Claude/Codex 双形态嗅探、`find_secret` Secret 扫描、`derive_compat_session_id`/`content_fingerprint`/`effective_identity`、事件映射与 `validate_structure` |
| `src/session/import/formats/export.rs` | ~290 | `SessionExport` v3 JSON 形状（v1/v2 兼容读、身份回填）与 `validate` |
| `src/session/import/persist_pi.rs` | ~450 | `import_pi_jsonl(_lines)`：Secret 预扫 → header 必需 → 单 `Immediate` 事务落 main 分支（Branch marker 折叠为 Diagnostic） |
| `src/session/import/persist_compat.rs` | ~900 | `import_compat(_from_file/_dry_run)` 与 `compat_import_history`：指纹幂等、`compat_import_identity` 冲突检测、键集分页 |
| `src/session/import/persist_export.rs` | ~920 | `export_session`/`import_session`/`add_tags`/`get_session_identity`：v3 全量往返，导入侧身份匹配 + 顺序/parent 预检 |
| `src/blob/mod.rs` | ~30 | blob 门面：`atomic` 私有；artifact 常开 re-export；`protected`/`checkpoint` 随 feature |
| `src/blob/atomic.rs` | ~50 | crate 私有 `atomic_write_bytes`：同目录 `.tmp-{pid}-{counter}`、`create_new`、`write_all`+`sync_all`+`rename`，失败删临时文件；artifact / protected / checkpoint 共用 |
| `src/blob/artifact.rs` | ~1220 | `ArtifactStore`：`<root>/blobs/ab/cd/<hash>` 分片目录 + `artifacts.sqlite3` 元数据；put 去重/预算、get 重验哈希、`read_range`、refcount、gc、`integrity_check` |
| `src/blob/protected.rs` | ~1430 | `ProtectedBlobStore`：PWB1 AEAD 信封（XChaCha20-Poly1305）、`BlobScope` 隔离、pending/ready/deleting 三态、延迟回收 gc、崩溃 reconcile；`parse_pwb1_envelope`/`open_pwb1_envelope`/`pwb1_aad` 纯函数 |
| `src/blob/checkpoint.rs` | ~980 | `CheckpointService`：写前快照 `snapshot_before_write`、`rollback_tool_call`/`rollback_run`、`conflict_check`、`checkpoint-state-v1.json` 原子持久化（`spawn_blocking` 调 `atomic_write_bytes`）、roots 内路径解析防穿越 |
| `tests/pwb1_golden.rs` | ~160 | PWB1 已知向量 golden（required-features `protected`） |
| `tests/read_range.rs` | ~230 | `read_range` 边界/完整性集成测试（required-features `blob`） |
| `src/session/fixtures/*.jsonl` | 7 个 | v12 升级 golden 的 lineage 期望：`v12_fork_tree.{main,fork-a,fork-b}`、`v12_interleaved.{main,side}`、`v12_compaction.{main,side}`（见 §5/§7） |
| `tests/golden/pwb1_valid.hex` | 1 行 | PWB1 已知向量密文帧 |

## 3. 对外 API 面

### 3.1 sqlite：DatabaseActor 与 migration 框架

- `DatabaseActor::open(path)` / `open_with_options(path, DatabaseOptions)` / `open_read_only(path)`：写模式会建父目录、设 `journal_mode=WAL` + `synchronous=NORMAL`；只读模式以 `SQLITE_OPEN_READ_ONLY` 打开、不建目录、不改 journal。两种模式都固定 `foreign_keys=ON`、`busy_timeout=5s`。`DatabaseOptions.queue_capacity` 默认 128，传 0 得 `DatabaseError::InvalidQueueCapacity`。
- `actor.call(|conn| ...) -> Result<T, DatabaseError>`：唯一执行入口，闭包在 Actor 线程串行运行；命令经有界 `sync_channel` 排队（队列满时发送端阻塞形成背压）；闭包 panic 被 `catch_unwind` 捕获转 `OperationPanicked`（Actor 存活）。另有 `path()`、`is_read_only()`、`backup_to(path)`（拒绝目标 = 源，`BackupTargetsSource`）、`restore_from(path)`（只读模式报 `ReadOnly`）、`shutdown()`（幂等，等待线程退出）。
- `DatabaseError` 全集：`Sqlite`/`ActorClosed`（线程已退出）/`OperationPanicked`/`ResponseTypeMismatch`/`InvalidQueueCapacity`/`BackupTargetsSource`/`ReadOnly`/`Io`。
- migration 框架（`sqlite::migration`）：`Migration { version, name, sql }`（静态 SQL 批）数组 + `migrate(actor, table_name, migrations)` / `schema_version(actor, table_name)`。每套 schema 用独立账本表（如 `session_schema_migrations`，表名先经 `validate_table_name` 白名单校验），因此**信封版本、session schema、blob schema 互不相干**。行为：校验计划（版本从 1 连续、无重复）→ 读当前版本（账本表不存在视为 0）；库版本 > 计划最大版本时报降级拒绝 → 已存在且非空的库先物理备份为 `<db>.pre-migration-v<from>.bak` → 单事务应用全部待做迁移，任一步失败整批回滚 → 返回 `MigrationReport`（含 from/to 版本与 `backup_path`；全新建库无备份）。

### 3.2 session：SessionStore 生命周期

- `SessionStore::open(path) -> (SessionStore, MigrationReport)`：打开写模式 Actor（路径不存在则建父目录新建库）→ 跑 session migration 到 v14（旧库自动升级并留备份）→ **回收 CommandLedger 全部 `inflight` 残留**（崩溃恢复，见 §4.4）。`MigrationReport` 供调用方记录升级轨迹。
- `SessionStore::open_read_only(path)`：只读打开并要求库版本**严格等于** `CURRENT_SCHEMA_VERSION`——只读模式无法就地迁移，低版本库同样报 `UnsupportedSchema`；适合诊断/取证场景与并行只读副本。
- 其余：`schema_version()`、`database()`（借出 Actor 给同库子系统，如 client_adapter）、`path()`、`shutdown()`。错误统一 `SessionStoreError`：`Database`/`Sqlite`/`Ledger(LedgerError)`/`UnsupportedSchema`/`SessionNotFound`/`BranchNotFound`/`BranchAlreadyExists`/`BranchNotActive`/`ForkPointNotTurnBoundary`/`NonContiguousSequence`/`SequenceOverflow`/`ParentEventNotFound`/`ProjectionInvariant`/导入类（`CompatUnparseable`/`CompatSecretDetected`/`CompatValidationFailed`/`CompatImportConflict`/`InvalidHistoryCursor`）/导出身份与版本类（`ExportSchemaVersion`/`ExportIdentityMissing`/`ExportIdentityMismatch`/`EventSessionMismatch`），另有休眠变体 `LeaseHeld`/`LeaseNotHeld`/`SessionHasEvents`（见 §8）。

### 3.3 事件追加与读取

- `create_session(session_id, title, created_at)` / `create_session_with_identity(…, tenant_id, principal_id)`：前者为 legacy 便捷入口，固定身份 tenant `local/default`、principal `local/user`；同时建 `main` 分支（`DEFAULT_BRANCH_ID`）并置为 active。`create_session_with_workspace(…, workspace_id)` 在同一事务内创建 session、main 分支与初始归属，任一步失败整批回滚。
- `set_session_workspace(session_id, workspace_id)`（ADR-043，v13）：UPDATE `sessions.workspace_id` 归属列；弱引用不校验 workspace 是否登记，会话不存在报 `SessionNotFound`（fail-closed）；不进 import/export。
- `list_session_workspace_bindings()`：读取全部非 NULL 归属（含 archived），供 AppCore 启动或重复开库时原子替换内存缓存。
- `register_workspace(workspace_id, name, root_path)` / `list_workspaces()`（ADR-044，v14）：Host 本地项目注册表。按 canonical root 幂等登记——同 root 重登返回既有记录（stable id 不变）；同 id 不同 root 报 `WorkspaceRegistryInvariant`（fail-closed）；单事务写入，`list_workspaces` 按创建序返回全部条目。
- `append_event(branch_id, AgentEventEnvelope) -> AppendReceipt`：显式点名分支且必须是当前 active 分支（否则 `BranchNotActive`）。sequence 为 **session 级全局单调**：必须 = 全 session 现有最大 sequence + 1（首条 = 1），否则 `NonContiguousSequence { expected, actual }`；因此各分支的事件序是全局序的互不重叠子序列。写前做 Secret 脱敏与 legacy 键规范化（§4.1）。`AppendReceipt { event_id, sequence, branch_id }`。
- 读取三视图 + lineage 视图（`replay_events`/`tail_events` 对 `limit == 0` 直接返回空集）：
  - `replay_events(session, from_sequence, limit)`：全 session 账本视图（含所有分支），按全局 sequence 升序——历史兼容接口；
  - `tail_events(session, limit)`：尾部 N 条升序；
  - `events_by_branch(session, branch, from_sequence, limit)`：仅该分支本地事件，**不含祖先**（不能当恢复源）；
  - `events_on_lineage(session, branch, from_sequence, limit)`：**恢复重放唯一正确入口**，沿祖先链取可见事件（§4.2）。
- 全局 sequence 的推论：切回旧分支继续 `append_event` 是允许的（新事件接在**全局**尾部），因此单一分支内的 sequence 单调递增但**可能带洞**（洞位被其他分支占用）；分支内相对顺序与 lineage 可见性不受影响。

### 3.4 分支树与 lineage

- `create_branch(session, branch, parent, forked_from: Option<&EventId>)`：幂等（同参数重放返回成功，参数不同报 `BranchAlreadyExists`）。
- `fork_from_event(session, new_branch_id, from_event_id)`（源分支由事件反查）：仅允许从**闭合回合边界**分叉——`run_completed` / `run_cancelled` / `run_failed` / `compaction_completed` 四类事件；其余报 `ForkPointNotTurnBoundary`。幂等。
- `switch_branch(session, branch)`：切 active 分支（投影读与后续 append 跟随）。
- `session_tree(session) -> SessionTree { branches: Vec<BranchNode> }`：扁平节点列表，`BranchNode { branch_id, parent_branch_id, forked_from_event_id, head_sequence, active }`，树形结构由调用方按 `parent_branch_id` 自行拼装。
- 幂等细则：同 `(branch, parent, fork point)` 的 fork 重试静默成功；**同名分支换 fork 点**仍报 `BranchAlreadyExists`。

### 3.5 投影（ProjectionSnapshot）

- `projection_snapshot(session)`（active 分支）/ `projection_snapshot_on_branch(session, branch)`：返回 `ProjectionSnapshot { messages, runs, tool_calls, server_tool_events, program_outputs, screenshots, transcript_envelopes, compacted_through }`。messages 按 lineage 可见性 + compaction 水位从事件账本即时重建（event-ledger 语义，不读物化 messages 表的折叠盲区）；runs/tool_calls/server_tool_events/transcript_envelopes 为全 session 物化行。
- 物化表形状（随事件在同事务内由 `apply_projection` 维护）：`messages`（message_id、branch_id、sequence、role、message_json）、`runs`（run_id、state、started/completed_at_ms、run_json）、`tool_calls`（tool_call_id、run_id、name、state、arguments_json、result_json）、`server_tool_events`（v5 起：citations/sources/screenshots/outputs 等 JSON 列）、`transcript_envelopes`（PRIMARY KEY (session_id, sequence) 的信封 JSON）。
- `rebuild_projection(session)`：单事务清空五张投影表后按事件账本全量重放 `apply_projection`，返回重建后的快照——投影损坏时的自愈入口。

### 3.6 CommandLedger（命令幂等）

`store.command_ledger()` 返回轻量句柄 `CommandLedger`（共享同一 Actor）；条目以 `(tenant_id, client_scope, command_id)` 为键，可附 `idempotency_key`；错误类型 `LedgerError`，经 `SessionStoreError::Ledger` 透传：

- `check(tenant, scope, command_id, idempotency_key) -> LedgerCheck`：`New`（已占位 inflight）/ `Replay(response_json)`（曾完成，直接回放）/ `InFlight`（并发中）。同 key 绑定到不同 command_id 报 `LedgerError::KeyConflict`。
- `record(..., response_json)`：inflight → completed 并存响应；对已 completed 的重复 record 报 `LedgerError::DuplicateCommand`。写入后按 `DEFAULT_COMMAND_LEDGER_CAPACITY = 4096` **全局**（跨 tenant/scope）淘汰最老 completed 行。
- `release(...)`：放弃 inflight 占位（执行失败时调用，让重试可重新 check）。
- `reclaim_inflight()`：删除全部 inflight（`SessionStore::open` 自动调用，只读打开不执行）；`stats()` 返回 `LedgerStats { entries, inflight, completed }`。
- `SessionStore::waiting_tool_call(s)`：查询 `tool_calls` 中 `waiting_for_approval` 状态的行，返回 `WaitingToolCall { session_id, tool_call: ProjectedToolCall }`，供重启后恢复待审批队列。

### 3.7 目录、标签与身份

- `list_sessions()`（固定过滤 `archived=0`、按 `updated_at_ms` 降序，无参数）/ `get_session(&SessionId)`（缺失报 `SessionNotFound`）：`SessionRecord { session_id, title, created_at_ms, updated_at_ms, archived, active_branch }`。`rename_session(&SessionId, title, now_ms)` / `archive_session(&SessionId, archived, now_ms)`（ADR-054）：UPDATE 单行走 `updated_at_ms` 刷新，缺失报 `SessionNotFound`；归档不删事件与投影，`get_session` 仍可读。
- `add_tags(session, &[&str])`：幂等插入 `session_tags`。
- `get_session_identity(session) -> (tenant_id, principal_id)`。

### 3.8 导出 / 导入

- `export_session(session) -> SessionExport`：v3 形状 = `{ schema_version: 3, session_id, tenant_id, principal_id, title, created_at_ms, updated_at_ms, archived, active_branch, branches: [ExportedBranch{branch_id, parent_branch_id, forked_from_event_id, head_sequence}], events: [ExportedEvent{branch_id, event}]（按 sequence 升序）, tags }`。
- `import_session(&SessionExport, tenant_id, principal_id)`：`validate()`（版本 1..=3、v3 必须带身份）→ export 身份必须与调用方传入身份一致（`ExportIdentityMismatch`）→ 事件 session_id 一致预检 → 单 `Immediate` 事务重建 session/分支/事件/标签。v1/v2 输入在反序列化时回填 legacy 身份（tenant `local/default`、principal `local/user`）、v1 事件全部归 `main`。**不做 Secret 扫描**（事件在首次入库边界已脱敏）。
- `import_pi_jsonl(path)` / `import_pi_jsonl_lines(lines)`：Pi JSONL → 单分支导入（原文件只读不改）。逐行 `parse_pi_line` 得 `PiPayload::{Header, Message, ToolCall, ModelSwitch, Compaction, Branch, Raw}`：Message → `MessageCommitted`、ToolCall → `ToolCallStarted`、ModelSwitch → `pi.model_switched` Diagnostic、Compaction → 摘要 `MessageCommitted` + `CompactionCompleted`（`compacted_through` 为已落盘事件水位）、Branch marker → `pi.branch_collapsed` Diagnostic（R6 起收编单分支语义，不再建零事件分支行）；`PiEntryKind::{Custom, Unknown}` 行载荷为 Raw，原文进 `unknown_entries`。返回 `PiImportReport`（`header_found`、imported_messages/tool_calls/model_switches/compactions/branches 计数、`unknown_entries: BTreeMap<行号, 原文>`）；无 header fail-closed 拒绝导入。
- `import_compat(source, content)` / `import_compat_from_file(source, path)` / `import_compat_dry_run(source, content)`：外部导入（§4.5），来源由调用方指定（`ExternalSource::{Claude, Codex, Grok, Cursor}`）。返回 `CompatImportReport { source, session_id, original_id, imported_events/messages/tool_calls/tool_results/usages/reviews, raw_records, deduplicated, unknown_fields }`——`deduplicated = true` 表示命中既有导入（幂等去重，`imported_events == 0`）。
- `compat_import_history(limit, cursor) -> CompatImportHistoryPage { entries, cursor }`：键集分页（limit 夹到 1..=500，默认 50，cursor 为不透明令牌 `"{imported_at_ms}:{session_id}"`，坏令牌报 `InvalidHistoryCursor`）；每条 `(source, original_id)` identity 只留一条历史，重复导入不新增。
- 纯解析层（不落库，可独立调用）：`parse_pi_line`；`parse_external`（按来源分派 `parse_claude`/`parse_codex`/`parse_grok`/`parse_cursor`）；`find_secret`；`derive_compat_session_id`/`content_fingerprint`/`effective_identity`；`validate_structure`。

### 3.9 compaction（feature `compaction`）

- `CompactionEngine::new(&SessionStore, Arc<dyn TokenEstimator>)`（默认策略）/ `with_policy(store, RetentionPolicy, estimator)`；`compact(session, branch_id, reason: CompactionReason, summary_text, &RetentionInputs) -> CompactionResult { reason, snapshot, decision, total_events, .. }`。引擎读 active branch lineage → 在 head 事件处建 `compaction-recovery-<branch>-<head_seq>` recovery 分支（完整历史逃生门，同 head 重试幂等复用）→ 过滤掉 lineage 外的输入后 `retention::apply` 决策保留集 → 产出 `CompactionSnapshot` 与 token 估算。**引擎不追加 `CompactionStarted/Completed` 事件、不改写历史**——事件化由调用方（engine crate）走 `append_event`，投影层在收到 `CompactionCompleted` 时执行水位折叠（§4.3）。
- `RetentionPolicy` 字段：`keep_last_turns`、`keep_unresolved_tasks`、`keep_user_constraints`、`keep_modified_files`、`keep_pending_tool_calls`、`keep_failed_tool_calls`；`RetentionInputs` 由调用方提供各维度候选（消息回合、任务、约束、改动文件、带 `ToolCallRetentionState::{Pending, Failed, …}` 状态的工具调用），每项挂 `event_id`。
- `apply(policy, inputs)` 为纯函数：逐规则把命中项的 `EventId` 并入保留集，输出 `RetentionDecision { retained_event_ids, dropped_count, reasons }`——`reasons` 是人可读的保留理由清单，供快照与日志展示。

### 3.10 blob 三区

- **artifact（feature `blob`）**：`ArtifactStore::open(root)` / `open_with_options(root, ArtifactStoreOptions { disk_budget })`。`put(&[u8]) -> PutOutcome { id: BlobId, created, ref_count }`（BLAKE3 寻址，重复 put 命中去重仅引用 +1；超预算报 `DiskBudgetExceeded`；经 `atomic_write_bytes` tmp+fsync+rename 原子落盘）；`get`/`read_range(id, offset, limit)` 读时重验哈希，不符报 `BlobCorrupted`（另有 `EmptyRange`/`RangeOffsetOutOfBounds`/`UnknownBlob`/`BlobMissing`）；`release`（下溢报 `RefCountUnderflow`；归零不立即删文件）；`gc()`（删 0 引用 blob + 回收 >24h 的 `.tmp-` 与无元数据孤儿文件）；`integrity_check()`、`disk_usage()`、`metadata`、`byte_length`、`blob_path`、`database`、`shutdown`。
- **protected（feature `protected`）**：`ProtectedBlobStore::open(root, resolver: Arc<dyn ProtectedKeyResolver>)`。scope = `BlobScope::new(provider_id, session_id)`；内置 `InMemoryKeyResolver`（`insert`/`set_current`/`remove` 管理各 scope 的 `KeyVersion → AeadKey`）可作宿主实现。`put(scope, plaintext) -> PutOutcome { blob_ref, key_version, .. }`：取 scope 当前 `KeyVersion` → PWB1 AEAD 密封 → 三态写入（§4.7）。`get(scope, ref)` 解密返回 `ProtectedBlob`（`expose()` 取明文，Drop 时 `zeroize`）；跨 scope 访问、密钥缺失、文件缺失统一 fail-closed 为 `ProtectedBlobUnavailable`，格式/摘要/AEAD 失败为 `ProtectedBlobCorrupted`（`is_unavailable()`/`is_corrupted()` 判别）。`retain`/`release`（归零后进入 `retention_ms` 延迟回收窗，默认 7 天）、`gc()`、`metadata`、`shutdown`。纯函数 `parse_pwb1_envelope`/`open_pwb1_envelope`/`pwb1_aad` 供离线校验。
- **checkpoint（feature `checkpoint`）**：`CheckpointService::open(ArtifactStore)`（恢复 `checkpoint-state-v1.json`，版本不符 fail-closed）。`snapshot_run(run_id)` 幂等建条目；`snapshot_before_write(run_id, tool_call_id, roots, relative_path) -> FileSnapshot`：在 roots 内 canonicalize 解析（拒绝绝对路径与 `..` 穿越），读旧内容存 Blob（同 key 去重，并发多余引用自动 release）；`rollback_tool_call`/`rollback_run` 逆序恢复（同一 `atomic_write_bytes`、删新增文件、还原 unix mode）；缺 recorded path 报 `NotFound`，不跳过；`conflict_check(run_id, tool_call_id)` 重读文件比对 pre_hash 报告用户改动；`list_changes(run_id)` 同步快照。

### 3.11 client_adapter

`SqliteClientSessionRegistryStore::new(SessionStore)`：以 `client_adapter_sessions` 表实现 [pawork-domain](domain.md) 的 `SessionRegistryStore` trait——`load_all`、`insert`（`ON CONFLICT DO NOTHING`，冲突返回现值）、`compare_and_swap`（按 `ownership_epoch + revision` 乐观并发，失败返回 `Conflict(现值)`）、`remove_if_owner`。GUI Connection Protocol 断线重续的持久化底座（消费方见 [client.md](client.md)）。

## 4. 核心行为与数据流

### 4.1 一次事件 append 全流程

1. 校验 session 与目标分支存在，且目标分支是 active 分支（`SessionNotFound`/`BranchNotFound`/`BranchNotActive`）。
2. 读**全 session** `MAX(sequence)`，要求新事件 sequence 严格 = 其 +1（首条 =1、必须 >0），否则 `NonContiguousSequence { expected, actual }`——sequence 在 session 内全局唯一且连续，跨分支不复用编号。
3. `parent_event_id` 非空时校验父事件存在于本 session（`ParentEventNotFound`）。
4. 信封序列化为 JSON 后进入**写前净化**（`redact_event_for_persistence`）：a) `canonicalize_legacy_hint_keys` 只作用于 `opaque_metadata`/`continuation_metadata` 两个地图，把 V1 legacy 拼写（`responses.summary_entries`/`openai.responses.summary_entries`/`anthropic_block_kind`）改名为 `provider_hints.<provider>.<key>` 规范键（映射表来自 [pawork-domain](domain.md) 的 `provider_hints`；新旧并存时规范键优先）——读老库时 `decode_persisted_json` 做同样映射（不含旧拼写的载荷走免 Value 往返快路径），**旧拼写永不落盘**；b) `redact_sensitive_json` 递归**保形脱敏**（string 换 `"[REDACTED]"`、number 归 0、bool 归 false）：命中归一化键名片段 `authorization`/`apikey`/`accesskey`/`privatekey`/`secret`/`password`/`cookie`/`oauthcode`、token 凭证族（单数 `token` 默认敏感，`*tokens`/`token_usage`/`token_count` 等计数统计键豁免）、reasoning 凭证碎片（`encrypted_content`/`signature`/`reasoning_content`/`continuation_bytes`）与 `credential(s)`；`headers`/`request_headers`/`response_headers` 容器整体脱敏；c) `opaque_metadata`/`continuation_metadata` 整图改走 `sanitize_reasoning_metadata`：非 JSON 对象 fail-closed 整体脱敏；对象内仅 `provider_hints.<provider>.<key>` 语法合法、非敏感且序列化 ≤ `MAX_HINT_VALUE_BYTES` 的键可能保留——`.responses.summary_entries` 数组逐条只留 `{"type","text"}` 字符串字段、`.block_kind` 仅 string 放行、其余合法键递归通用扫描；违规值一律保形脱敏。
5. 单事务内：INSERT `session_events`（`event_id` 为全表主键，重复 id 直接被约束拒绝）→（同事务）`apply_projection` 更新物化表 → 更新分支 `head_sequence` 与 session `updated_at_ms`（取事件时间戳）→ COMMIT，返回 `AppendReceipt`。任一步失败整体回滚，账本与投影不会脱节。
6. 信封的 `event_id`/`run_id`/`timestamp` 由调用方生成并原样保留；storage 只校验、脱敏、编号落位，不改写业务字段。

### 4.2 会话恢复重放

1. `load_ancestor_lineage(session, branch)`：从目标分支沿 `parent_branch_id` 回溯到 `main`，每段祖先带上界 = fork 事件的 sequence（含），tip 分支无上界；环路防御 `ProjectionInvariant`。
2. `events_on_lineage` 按 lineage 段过滤 `session_events`（`visible_on_lineage`：事件属于链上分支且 sequence 不超过该段上界），升序返回 `from_sequence` 起至多 `limit` 条。
3. 上界语义的推论：fork 之后父分支继续追加的事件（sequence > fork 点）对子分支 lineage **不可见**——分叉即时间冻结，父子各自演化互不串扰。
4. 引擎恢复时把这些信封反序列化重放即可重建内存态；投影快照（§3.5）则额外叠加 compaction 水位：`compacted_through` = lineage 可见的最大 `CompactionCompleted.compacted_through`，messages 只取水位之后的事件。

### 4.3 compaction 水位折叠

1. 引擎（feature `compaction`）`compact`：取 active branch lineage 事件 → 直接 `create_branch` 在 lineage head 事件处建 `compaction-recovery-<branch>-<head_seq>` 分支（保留完整历史；raw 建分支不受四类 fork 边界限制，同 head 重试幂等复用）→ `retention::apply` 得保留集 → `TokenEstimator` 估算前后 token → 产出 `CompactionSnapshot`（v1，serde 冻结：`version`/`summary`/`retained_event_ids`/`replaced_range`/`token_usage_before`/`token_usage_after`/可选 `recovery_branch_id`）。
2. 调用方把 `CompactionStarted` / `CompactionCompleted{ compacted_through, snapshot }` 作为普通事件 `append_event` 到工作分支。
3. 投影层收到 `CompactionCompleted` 时执行**本分支物化折叠**：`DELETE FROM messages WHERE branch_id = 本分支 AND sequence <= compacted_through`（v12 冻结语义：只删本分支行，不动祖先/兄弟分支）。
4. 读侧水位由 projection 内部的 lineage 水位查询单点提供：沿祖先链取可见 `CompactionCompleted` 的最大 `compacted_through`，作为 `ProjectionSnapshot.compacted_through` 暴露给消费方（UI 折叠展示用）。
5. 读侧双保险：`ProjectionSnapshot.messages` 从事件账本按水位重建，因此即便物化表未折叠/被重建，读到的消息窗口一致；recovery 分支不受水位影响，可整段回看。

### 4.4 CommandLedger 幂等 check/record

1. 执行命令前 `check(tenant, scope, command_id, idempotency_key)`：先按 command_id 查——`completed` 行返回 `Replay(response_json)`；`inflight` 行返回 `InFlight`；再按 idempotency_key 查重（命中不同 command_id 报 `KeyConflict`）；都没有则 INSERT `inflight` 占位并返回 `New`（并发唯一键冲突时重查一次分类）。key 为 `NULL` 时不进 v11 的部分唯一索引，仅按 command_id 幂等。
2. 命令成功后 `record(...)` 把占位翻转为 `completed` 并存响应 JSON；随后全局容量淘汰（>4096 时按 `completed_at_ms` 升序删最老 completed 行）。失败路径调 `release` 删占位。
3. `stats()` 随时可读 `(entries, inflight, completed)` 计数，供诊断与容量观测。
4. 崩溃后 `SessionStore::open` 自动 `reclaim_inflight` 清掉全部占位——重启后同命令重新 `check` 得 `New`，可安全重试；只读打开不回收。

### 4.5 compat 导入嗅探与解析

1. `find_secret(content)` 全文扫描（`sk-`/`ghp_`/`AKIA`/`xoxb-`/`Bearer`/`AIza`/PEM 私钥块等模式），命中即 `CompatSecretDetected` fail-closed，**任何内容不落库**。
2. 来源由调用方指定（`ExternalSource`），`parse_external` 分派各来源解析器；**双形态在解析器内自动嗅探**：Claude = claude.ai 导出 JSON 数组 **或** Claude Code 本地 JSONL（按 `type:"user"/"assistant"` + `message` 包裹识别）；Codex = flat JSONL **或** rollout envelope JSONL（`{timestamp, type, payload}` 包裹）。无法解析报 `CompatUnparseable`。
3. 逐条解析为 `ExternalRecord`（消息/工具调用/噪音跳过计数入 `CompatImportReport`）；结构损坏（JSONL 行非对象、必需字段缺失等）fail-closed 报错而非静默跳过。
4. `derive_compat_session_id`（来源前缀 + 身份/内容指纹）与 `content_fingerprint`（BLAKE3）：同内容重复导入幂等返回原 session；同身份不同内容报 `CompatImportConflict`；`compat_import_identity` 表（`(source, original_id)` 主键 + fingerprint + session_id）在同一 `Immediate` 事务内写入，history 的 `imported_at` 取自 session 行时间。
5. 映射层归一化为 canonical `AgentEventEnvelope` 序列（合成 `RunStarted`/`RunCompleted` 边界、工具调用配对、原始记录附 `Diagnostic`），公开的 `validate_structure` 校验序列结构（空批、id 重复、引用悬空等报 `CompatValidationFailed`）后经 `persist_event_in_transaction` 落 `main` 分支。
6. 持久化整体包在单个 `Immediate` 事务里（先占写锁）：identity 查重、建 session、写事件、登记 `compat_import_identity` 原子完成，与并发导入互斥；`dry_run` 走完全部扫描/解析/校验但不开写事务。

### 4.6 export → import 全量往返

1. `export_session` 单次 Actor 调用内读齐 session 行（含身份/标题/归档/active_branch）、全部分支、全部事件（`decode_persisted_event` 解码并携带 `branch_id`，按 sequence 升序）与标签，组装 `SessionExport`（写死 `schema_version = 3`）。
2. `import_session(export, tenant, principal)` 先 `validate()`：版本必须在 1..=3；v3 身份非空；再比对 export 身份与调用方身份（`ExportIdentityMismatch` fail-closed）；预检每条事件的 `session_id` 与 export 一致（`EventSessionMismatch`）。
3. 单 `Immediate` 事务重放：建 session 与 `main` → 按事件全局 sequence 逐条插入，途中遇到某分支首事件时先建分支行（fork 点 `head_sequence` 由 fork 事件反查）→ 复用 `persist_event_in_transaction`（含顺序/parent 校验与投影写入）→ 尾部补建零事件分支（按 `head_sequence` 排序）→ 校验 `active_branch` 存在 → 恢复 archived 与 tags。任一步失败整体回滚，不产生半导入会话。
4. 往返不变量：export → import 到空库后，`session_tree`、`events_on_lineage`、投影快照与标签与原库一致（persist_export 内嵌测试覆盖全量往返）。

### 4.7 PWB1 protected 写入/读取

1. `put`：resolver 取 scope 当前 key_version 与密钥 → 随机 24B nonce → AAD = `"pawork.protected-blob.v1\0"` + 长度前缀的 provider/session/logical_ref + key_version(BE) → XChaCha20-Poly1305 密封 → 信封 = `PWB1` magic(4) + version(1) + algorithm(1) + key_version(4,BE) + nonce(24) + ciphertext。
2. 三态落盘：INSERT `pending` 元数据行（预算检查含 pending/deleting）→ 密文按 BLAKE3 digest 寻址原子写文件 → UPDATE 为 `ready`；中途崩溃由下次 `open` 的 reconcile 清理（pending/deleting 行 + 无主密文文件一律删除）。
3. `get`：按 `(scope, logical_ref)` 查 `ready` 行（scope 不匹配 = 不存在，防跨会话探测）→ 重算文件 digest 比对 → 解析信封并校验 key_version 与元数据一致 → resolver 解出该版本密钥 → AEAD open（AAD 绑定 scope/ref/版本，串扰即 `Corrupted`）。
4. `release` 归零后写 `retain_until_ms = now + retention`（默认 7 天）；`gc` 只回收 `deleting` 行与过期归零行，先标 `deleting` 再删文件删行，崩溃可续。

### 4.8 checkpoint 写前快照与回滚

1. `snapshot_before_write`：`relative_path` 逐个 root `join` + `canonicalize`，校验落在某 root 内（拒绝绝对路径与 `..` 穿越，`PathEscape`/`UnresolvedPath`）。
2. 同 `run_id + tool_call_id + relative_path` 已有快照直接复用；否则读当前内容（不存在则记 `existed = false`）→ `ArtifactStore::put` 存 pre 内容（去重）→ 记 `FileSnapshot { relative_path, existed, pre_blob, pre_hash, unix_mode }` 挂到该 tool_call 的 `ChangeRecord`；并发竞态下先 put 后发现重复会 `release` 多余引用。
3. 状态经 `checkpoint-state-v1.json`（`schema_version = 1`）由 `atomic_write_bytes`（`spawn_blocking`）原子持久化；锁内只做内存图操作，从不跨 `.await` 持锁。
4. `rollback_tool_call`/`rollback_run` 逆序恢复：从 Blob 取回 pre 内容原子写回（恢复 unix mode），`existed = false` 的文件直接删除；`conflict_check` 重读文件重算 BLAKE3 与 `pre_hash` 比对，产出 `ConflictReport { relative_path, user_modified }`，不阻止回滚、只供调用方决策。

## 5. 契约与不变量

- **版本体系相互独立**：`AgentEventEnvelope.schema_version = 1`（domain 信封，见 [domain.md](domain.md)）、session SQLite `CURRENT_SCHEMA_VERSION = 14`、export `schema_version = 3`、artifact/protected/checkpoint 各自 `SCHEMA_VERSION = 1`、PWB1 `version = 1`——升级互不牵连，migration 账本表按命名空间隔离。
- **常量冻结**：默认分支 `DEFAULT_BRANCH_ID = "main"`；`DEFAULT_COMMAND_LEDGER_CAPACITY = 4096`；脱敏占位符 `"[REDACTED]"`。
- **`session_events` 账本冻结**：建表 DDL（v1）带 `UNIQUE(session_id, sequence)`（sequence 为 session 级全局单调连续，跨分支不复用编号；v12 迁移注释明示不动此约束）与 `CHECK(sequence > 0)`；v2 加 append-only 双触发器 `session_events_no_update` / `session_events_no_delete`（`RAISE(ABORT)`）——账本行一经写入不可改删，compaction 也只折叠投影。
- **迁移史 append-only**：`MIGRATIONS` 数组 v1–v12 的 DDL 文本不可改写，演进只能追加新版本。全史：v1 `core_session_schema`（sessions / session_branches / session_events + 投影三表 messages / runs / tool_calls）；v2 `event_store_immutability`（append-only 双触发器）；v3 `active_branch` 列 + 分支序索引 + `session_leases`；v4 `session_tags`；v5 `server_tool_events` + `transcript_envelopes`；v6 `compat_import_identity`；v7 `client_adapter_sessions`；v8 sessions 补 `tenant_id`/`principal_id` 并回填 `local/default`、`local/user`；v9 `session_bindings`（归档预留，见 §8）；v10 messages 加 `branch_id DEFAULT 'main'` 并按事件回填；**v11 = R4 `command_ledger`**（含 `idempotency_key` 部分唯一索引）；**v12 = R6 branch lineage 原生化**——`messages` 整表重建去掉 `DEFAULT 'main'`（新表 `branch_id NOT NULL` 无默认值），回填按 `session_events` 反查真实分支，查不到归属的孤儿行经 TEMP 触发器 `RAISE(ABORT)` 整批回滚（fail-closed，升级前已留 `.pre-migration-v11.bak`）；**v13 = ADR-043 Session→Workspace 归属持久化**——`sessions` 纯追加可空 `workspace_id TEXT` 列，不回填（历史 NULL 按 Unassigned）、无 FK（弱引用）；**v14 = ADR-044 持久项目注册表**——新建 `workspaces` 表（`workspace_id` 主键、`root_path` UNIQUE、`created_at_ms`/`updated_at_ms`）与 `idx_workspaces_created` 索引，空表不回填、不据历史归属猜 root。
- **升级 golden**：`src/session/fixtures/v12_{fork_tree,interleaved,compaction}.*.jsonl`（7 份）锁定三个种子场景迁移后各分支 lineage 消息序列；重生成须显式 `PAWORK_WRITE_STORAGE_GOLDEN=1` 跑 ignored 测试。
- **PWB1 冻结**：magic `PWB1`、version=1、algorithm=1（XChaCha20-Poly1305）、header 34 字节、AAD 形状（见 §4.7）；已知向量 `tests/golden/pwb1_valid.hex`（key=0x11×32、nonce=0x00..0x17、key_version=1）。
- **blob 原子写单源**：`blob/atomic.rs` 的 `atomic_write_bytes` 是 artifact / protected / checkpoint 唯一落盘路径；临时文件 `.tmp-` 前缀，与 artifact `gc` 的 24h 孤儿回收约定对齐。
- **export v3 形状冻结**：字段见 §3.8；读兼容 v1/v2（身份回填 `local/default`、`local/user`，v1 事件归 `main`），写只出 v3。`command_ledger`、`compat_import_identity` 等运维表**不进** export。
- **CompactionSnapshot v1 serde 形状冻结**（`snapshot.rs` 注释明示），`SnapshotVersion` 单值 `V1`。
- **Secret 红线**：明文 Token 不落库不落日志——写前 `redact_sensitive_json` + `sanitize_reasoning_metadata`（provider_hints 白名单命名空间）+ 导入前 `find_secret` fail-closed；legacy hint 键只读映射、不回写。protected 区明文只存在于内存 `ProtectedBlob`（Drop zeroize），错误信息与 Debug 输出不含密钥/明文。
- **导入身份规则**：Pi 与 compat 导入固定落 `local/default` + `local/user`；export/import 要求身份显式匹配，不做静默改写或提权。
- **fork 边界**：只能从四类闭合事件分叉（§3.4）；`create_branch`/`fork_from_event`/`create_session`/`add_tags`/compat 导入均幂等可重试。
- **blob 完整性**：artifact 与 protected 均在**读路径**重算 BLAKE3 与元数据比对，篡改/位腐一律报 corrupted 而非返回坏数据；artifact 引用计数只增于 `put`、只减于 `release`，物理删除只发生在 `gc`。
- **checkpoint 状态冻结**：`checkpoint-state-v1.json` 的 `schema_version = 1` 与 `FileSnapshot.pre_blob` 的 64 字符 hex serde 形状固定（与 V1 磁盘 JSON 兼容）。
- **读写隔离**：`open_read_only` 的 Actor 拒绝 `restore_from`；所有多步写（append、导入、rebuild、迁移）都在单事务内，无部分可见状态。

## 6. 依赖关系

feature 依赖有传递关系：`compaction ⇒ session`，`checkpoint ⇒ blob`，`protected ⇒ blob`；`default = ["session", "blob"]`。上游（生产依赖，按 feature 激活）：

| feature | 额外拉入 |
| --- | --- |
| 基线（常开） | `rusqlite`、`thiserror`、`tokio`（`sync`） |
| `session`（默认） | `pawork-domain`、`async-trait`、`serde`、`serde_json`、`blake3`、`tokio` `fs`/`io-util`/`macros` |
| `blob`（默认） | `blake3`、`serde` |
| `compaction` | = `session`（无新第三方依赖） |
| `checkpoint` | `blob` + `serde_json`、`tracing`、`tokio` `rt` 等 |
| `protected` | `blob` + `chacha20poly1305`、`getrandom`、`zeroize`、`pawork-domain` |

工作区内唯一生产上游是 [pawork-domain](domain.md)（信封/ID/`provider_hints`/`SessionRegistryStore` 等类型）；不依赖 engine/protocol/provider。dev-dependencies：`pawork-protocol`（`adapter` feature，client_adapter 测试消费）、`tempfile`、`tokio`（rt-multi-thread）、`blake3`、`chacha20poly1305`、`proptest`（当前无用点）、`serde_json`。

下游消费方：

- [pawork-app](app.md)：全 feature（`compaction`、`checkpoint`、`protected`）生产依赖，宿主拼装三区与会话存储；
- [pawork-cli](cli.md)：`default-features = false, features = ["session"]`——CLI 面只用会话存储，**不拉 blob**；
- [pawork-client](client.md)：仅 dev-dependency（契约测试起真实 store）；
- [pawork-engine](engine.md)：**不依赖本包**——compaction 的 token 估算经 `TokenEstimator` trait 依赖倒置，由 engine 侧注入实现。

整体分层见 [../../architecture.md](../../architecture.md) 与 [../../design.md](../../design.md) §2；跨包事件持久化/重放动线见 [../flows.md](../flows.md)。

## 7. 测试与验证资产

所有源码文件均带内嵌 `#[cfg(test)] mod tests`（21 处），另有 2 个集成测试。覆盖要点：

| 资产 | 覆盖 |
| --- | --- |
| `sqlite/mod.rs` tests | PRAGMA 生效、串行执行、panic 隔离、backup/restore、只读拒写 |
| `sqlite/migration.rs` tests | 全新建库、既有库备份、失败整批回滚、降级拒绝、账本命名空间隔离、非法表名/计划校验 |
| `session/migration.rs` tests | v12 全链迁移、三个种子场景升级 golden（`fixtures/v12_*.jsonl`）、孤儿行 fail-closed、v13 归属列迁移与绑定写读回/重启存活、v12→v14 升级（历史归属保持 NULL、注册表为空）、golden writer（ignored） |
| `event_store.rs` tests | Secret 脱敏矩阵（provider metadata / server tool / envelope / reasoning hints 超限拒绝）、legacy 键映射与旧行读回、sequence/parent 校验、分支隔离分页 |
| `projection.rs` tests | 折叠后投影窗口、事件-投影一致性、append-only 触发器、`rebuild_projection` 与增量投影等价 |
| `session_tree.rs` tests | fork 四类边界与拒绝、幂等、lineage 排除 fork 后父分支追加 |
| `catalog.rs` / `client_adapter.rs` tests | 目录排序/归档过滤；Registry insert/CAS/remove 乐观并发（借 `pawork-protocol` adapter 消费）；workspace 注册表幂等重登、跨重开存活、同 id 异 root fail-closed |
| `test_support.rs`（cfg(test)） | fork_tree/interleaved/compaction 三个种子场景构造器，供迁移 golden 与 lineage 断言复现历史库形态 |
| `command_ledger.rs` tests | New/Replay/InFlight 分类、key 冲突、重启 reclaim、容量 4096 全局淘汰（跨 tenant/scope） |
| `compaction/*` tests | retention 各策略保留集、engine 产出 recovery 分支与快照（含同 head 重试复用）、snapshot v1 serde golden |
| `import/formats/*` tests | 各来源解析与 Claude/Codex 双形态嗅探、Secret 模式命中、损坏 fail-closed、export v1/v2/v3 兼容读 |
| `import/persist_*` tests | Pi 原文件只读不改、compat 幂等/冲突/历史分页、export↔import 全量往返 |
| `blob/artifact.rs` tests | put 去重与引用计数、预算拒绝、读时哈希校验、gc/孤儿回收、`integrity_check` |
| `blob/protected.rs` tests | 三态与崩溃 reconcile、scope 隔离 fail-closed、密钥缺失/损坏分类、延迟回收 gc |
| `blob/checkpoint.rs` tests | 快照去重、回滚（含新增文件删除）、`conflict_check`、路径穿越拒绝、状态持久化往返 |
| `tests/read_range.rs` | 范围读中点/跨界/越界/空段、分片拼回、`BlobId` hex serde 与 checkpoint 兼容（required-features `blob`，默认开） |
| `tests/pwb1_golden.rs` + `tests/golden/pwb1_valid.hex` | PWB1 布局/AAD/坏头 Corrupted/已知向量往返（required-features `protected`） |

默认验证命令（见 [../verification.md](../verification.md) 总策略）：

```bash
cargo test -p pawork-storage --offline --lib --tests
```

默认只覆盖 `session` + `blob`（`tests/pwb1_golden.rs` 因 required-features 在默认跑中自动跳过）。触碰 opt-in 面时追加 feature，仍是一次 Cargo 进程：

```bash
cargo test -p pawork-storage --offline --lib --tests --features compaction,checkpoint,protected
```

升级 golden 重生成：置 `PAWORK_WRITE_STORAGE_GOLDEN=1` 运行 `session/migration.rs` 中带 `#[ignore]` 的 writer 测试，随后人工 diff `src/session/fixtures/`；未置该环境变量时 writer 拒绝覆盖。

## 8. 注意事项与已知限制

- **休眠 DDL**：`session_leases`（v3）与 `session_bindings`（v9）建表后当前**无任何生产读写入口**；`SessionStoreError::LeaseHeld` / `LeaseNotHeld` / `SessionHasEvents` 错误变体同样无触发路径——属预留位，勿据此假设租约/绑定功能已可用（v9 注释明示 binding 状态机已随 R0/ADR-038 归档，留表只为 append-only）。
- **无归档/删除写口**：没有 `archive_session`、`delete_session`、`delete_branch` 之类的公开 API；`archived` 标记只能经 `import_session` 从导出恢复，目录查询固定隐藏 archived 行。
- `replay_events` 跨分支合并、`events_by_branch` 不含祖先：两者都**不是**恢复语义，恢复一律走 `events_on_lineage`。
- 物化 `messages` 表在 compaction 折叠后存在盲区，读消息请走 `ProjectionSnapshot`（事件账本重建）而非直查表；`runs`/`tool_calls` 等物化表为全 session 维度，不区分分支。
- CommandLedger 容量淘汰是**全局** 4096（非按 tenant/scope 配额）；淘汰后旧命令重放会得到 `New` 而非 `Replay`。
- compat 与 Pi 导入固定落 `local/default` / `local/user` 身份与 `main` 分支；Pi 的 Branch marker 折叠为 `pi.branch_collapsed` Diagnostic，不重建分支树。
- artifact 与 protected 各自持有独立 SQLite（`artifacts.sqlite3` / `protected.sqlite3`），schema 用 `CREATE TABLE IF NOT EXISTS` 直建（`SCHEMA_VERSION = 1`），未接入 migration 框架；每个 root 假定单宿主进程独占（protected 的 open-time reconcile 会清走并发宿主的 pending 文件）。
- protected 的 `logical_ref` 对外稳定、物理文件按**密文** digest 寻址：密钥轮换后重加密会换物理文件但 ref 不变；`retain` 会清除延迟回收标记（复活窗口内的 blob 可救回）。
- checkpoint 只靠 Blob 还原、绝不 `git reset --hard`；`checkpoint-state-v1.json` 版本不符直接 fail-closed，不做静默降级。
- `DatabaseActor` 单连接串行：长事务（大导入、rebuild、迁移）会队头阻塞同一 store 上的所有调用（含读）；诊断类只读负载可另开 `open_read_only` 副本绕开写队列。
- `switch_branch` 改的是 `sessions.active_branch` 行字段，属**全局写指针**：同一 store 的多个消费方共享 active 分支，切换是全局副作用。
- `import_compat_from_file` 的 JSONL 分支注释写"流式读取"，实现仍是 `read_to_string` 一次性入内存——与整篇 JSON 分支等价，超大文件没有真流式路径。
- **单宿主进程假设**：CommandLedger 的 `reclaim_inflight`（源码注释"单宿主进程模型"）与 protected 的 open-time reconcile（"one owning host per root"）都假定同一库/根只有一个宿主进程；不要跨进程共享同一 session 库或 blob root。
- sequence 经 SQLite `i64` 存储，越界防御为 `SequenceOverflow`；`archived`/`head_sequence` 等均带 `CHECK` 约束兜底。
- fixtures 是检入的 JSONL 文本，golden 断言在 `cfg(test)` 中 `include_str!` 引用；改动必须走 §7 的重生成流程并人工 diff，禁止手改。
- dev-dependencies 中 `proptest` 当前无用点（历史遗留声明）。
- 阶段状态与后续演进（R7 沙箱对 blob 路径的收紧等）以 [AGENTS.md](../../../AGENTS.md) 为准；产品可见能力口径见 [../README.md](../README.md)。
