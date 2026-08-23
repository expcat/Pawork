# pawork-storage

SQLite Actor、Session Event Store、内容寻址 Blob（PWB1）。依赖 `pawork-domain`（session / protected feature）。

## 职责

串行化所有 SQLite 访问；以 append-only 事件流持久化 `AgentEventEnvelope`；提供会话树、投影、CommandLedger（幂等）、可选 compaction / checkpoint；blob 侧做内容寻址工件与受保护 AEAD 信封。调用方从不共享 `rusqlite::Connection`。

## 模块树

```
src/
  lib.rs                         # 根级不做 re-export
  sqlite/{mod.rs, migration.rs}  # 始终编译
  session/                       # feature session
    event_store.rs  catalog.rs  command_ledger.rs
    projection.rs  session_tree.rs  client_adapter.rs  migration.rs
    test_support.rs  fixtures/       # cfg(test) v12 升级 golden
    compaction/                  # feature compaction
    import/                      # Pi / compat / export；compat 含 Claude Code 本地 JSONL 与 Codex rollout 信封(R6 波 C)
  blob/                          # feature blob
    artifact.rs
    protected.rs                 # feature protected
    checkpoint.rs                # feature checkpoint
tests/
  pwb1_golden.rs  read_range.rs  golden/pwb1_valid.hex
```

## 对外入口/API 面

路径为 `pawork_storage::{sqlite,session,blob}`。

- **sqlite**：`DatabaseActor`、`migrate` / `schema_version`、`Migration*`。
- **session**：`CURRENT_SCHEMA_VERSION = 12`（**SQLite 迁移号**，与信封 v1 独立）；`SessionStore`、`AppendReceipt`、`DEFAULT_BRANCH_ID = "main"`、`SessionTree`；`fork_from_event` 仅接受三类 run 终态与 `CompactionCompleted`，相同 `(parent, fork point)` 重试幂等；`CommandLedger`（`LedgerCheck::{New, Replay, InFlight}`，容量默认 4096）；投影 `ProjectionSnapshot`；import/export（`EXPORT_SCHEMA_VERSION = 3`）。`SessionStore::open` 会 `reclaim_inflight`。
- **blob**：`ArtifactStore`、`BlobId`；protected：`PWB1_MAGIC` / `PWB1_VERSION = 1` / XChaCha20-Poly1305、`ProtectedBlobStore`、`ProtectedKeyResolver`；checkpoint：`CheckpointService` / `RunCheckpoint`。
- **compaction**（opt-in）：`CompactionEngine`、`RetentionPolicy`。

`command_ledger` 表为 v11 纯新增，不进 export。
v12（R6 波 A）起 `messages` 整表重建去 `DEFAULT 'main'`、按事件所属 branch 原生物化（回填即校验，无事件背书的孤儿行整批迁移失败）；升级 golden 检入 `src/session/fixtures/`（`PAWORK_WRITE_STORAGE_GOLDEN=1` 门控再生）。
R6 波 B 起 branch snapshot 的消息以 append-only `session_events` 重建，再按目标 lineage 可见的最大 compaction 水位折叠；v12 `messages` 物化表的 branch-local fold 保持冻结。Pi Branch marker 只在 main 上落 `pi.branch_collapsed` Diagnostic，不创建 branch 行。

## 依赖与被依赖

- **依赖**：`rusqlite` / `tokio`（sqlite 无 feature 门）。`session`/`protected` 拉 `pawork-domain`。`chacha20poly1305` 仅 `protected`。
- **features**：`default = ["session", "blob"]`；另 `compaction` / `checkpoint` / `protected`。
- **被依赖**：`pawork-app`（开 compaction/checkpoint/protected）；`pawork-cli`（`session` only）；`pawork-client` 仅 dev-dep。
- **不依赖本包**：`pawork-engine` 生产面；`apps/desktop`（deny-list）。

## 红线与注意事项

- Secret 不落库：事件 `opaque_metadata` 经 Secret 键扫描与保形脱敏；旧 `provider_hints` 拼写只读不写。
- Compat 导入检测到 Secret → `CompatSecretDetected`，拒绝导入。R6 波 C 起 compat 解析双形态：Claude = claude.ai 导出 JSON 或 Claude Code 本地 JSONL（自动判定；sidechain/thinking/噪声行跳过，标题取 `aiTitle`/`customTitle`）；Codex = 平铺 typed entry 或 rollout 信封 `{timestamp,type,payload}`（自动判定；developer/reasoning/event_msg 镜像跳过，未知落 Raw）。
- 分支读取统一经 storage lineage；父支 fork 后追加/压缩不得污染旧 fork，兄弟支 compaction 互不可见。
- PWB1：明文只在 AEAD 信封内；事件只带 `ProtectedBlobRef`；`ProtectedBlob` Debug 为 redacted。
- 改 DDL 必须迁移 + golden；v1–v11 不改写，只追加。R6 波 A 已落 v12（`messages` 整表重建去 `DEFAULT 'main'`，无事件背书的孤儿行 fail-closed），信封 v1 不变。

## 相关文档

- [docs/design.md](../../docs/design.md) §3.2 会话存储 / blob
- [plan/R4-host-decomposition.md](../../plan/R4-host-decomposition.md)（CommandLedger）
- [plan/R6-session-branching.md](../../plan/R6-session-branching.md)
- [代码地图总索引](../../docs/code-map/README.md)
