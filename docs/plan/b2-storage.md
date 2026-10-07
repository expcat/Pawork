# B2 · Storage 预留接口与恢复面裁决

> 状态：✅ 完成；裁决与实现完成，自动验证通过，独立 review PASS。完成：2026-10-08。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

核实存储预留接口的消费者，裁决保留或删除。

## 当前线索与读取范围

原 ROADMAP 登记参数化预留，以及 `retain` / `snapshot_run` / `conflict_check` / `replay_events` 保留面。裁决基线为 `main` / `644271a4`，开始时工作区干净。

[storage Spec](../spec/crates/storage.md)、[持久化与重放流程](../spec/flows.md)。

“参数化预留”追溯至 R-03 原始报告（2026-10-04，对话 `01a10597-bf22-7b11-91ea-a9c2da92c1f7`）的原句：`DatabaseOptions.queue_capacity/busy_timeout`、`ArtifactStoreOptions.disk_budget`、`ProtectedBlobStoreOptions.{disk_budget, retention}`；ROADMAP 的概括登记来自提交 `4dba7669`，后经 `215fc8c5` / `31729171` 迁入 B2。原报告的“无非默认定制”不能覆盖测试：当前 protected 测试明确用零保留期，artifact 预算测试也用非默认上限；本次以当前调用点重新分类。

## 实施范围

- 核对生产、恢复、迁移和测试消费面，区分预留与必需恢复能力。
- 裁决后只处理相关接口；不改动无关 schema、事件格式或迁移。

## 完成条件

- 保留或删除有事实依据，对应 Spec 与公开 API 一致。
- 涉及持久化或重放行为时，对应既有回归通过，恢复完整性有证据。

## 完成记录

### 2026-10-08 裁决与实现

先经 CodeGraph 定位，再核对当前源码与真实接收者。`replay_events` 定义在 `impl SessionStore`，当前没有 `SqliteEventStore` 类型；CodeGraph 的泛名 `retain` 调用统计混入集合与 Objective-C retain，不能作为 protected store 的消费者证据。

| 对象 | 当前事实依据 | 裁决与落地 |
| --- | --- | --- |
| `ProtectedBlobStore::retain` | 全仓 Rust 调用中的 `.retain` 接收者为集合；该 store 方法无生产、恢复、迁移或测试消费。`app/protected.rs` 的 protector 只调用 `put` / `get`，启动 reconcile 也不调用 retain | 删除方法；PWB1、scope 隔离、引用列、release/GC 与崩溃恢复保留。没有待删 retain 测试 |
| `CheckpointService::snapshot_run` | 仅库内 run 回滚测试与空条目幂等测试消费；生产 `app/checkpoint.rs::snapshot_write_tools` 调用 `snapshot_before_write`，其自身建立 run 条目 | 删除预建空条目入口；run 回滚测试改为依赖写前快照自动建条目；删除空条目专用测试 |
| `CheckpointService::conflict_check` / `ConflictReport` | 仅两项库内测试消费，生产回滚从未调用检测。算法对比的是写前哈希，工具自身写入也会被标为 user_modified，不能据此承诺回滚冲突保护 | 删除方法、结果类型、私有 `find_snapshot` 和两项专用测试；实际回滚、缺路径拒绝与路径越界回归保留 |
| `SessionStore::replay_events` | 生产 `services/session.rs::recover_usage_by_run` 为启动清扫恢复中断 run 用量；`services/usage.rs::reconcile_session_ledger` 恢复账单归属；另有 checkpoint run 查询、Plan/Goal 重建、技能录制与 CLI 模型切换记录。存储测试也消费此读口 | 保留实现；纠正 Spec 的“历史兼容接口”和“一律不用于恢复”表述。全账本读口与目标分支消息恢复明确区分 |
| `DatabaseOptions.queue_capacity/busy_timeout` | 当前所有开库均用 `open` / `open_read_only` 默认值；没有外部 Options 或非默认值消费，测试也无此参数定制 | Options / `open_with_options` 收为私有内部模式；删可调字段与不可达 `InvalidQueueCapacity`，固定容量 128、busy timeout 5 秒。读写建库与只读不建库保持原路径 |
| `ArtifactStoreOptions.disk_budget` | Host 只用 `ArtifactStore::open`；非默认预算由 `disk_budget_rejects_oversized_put_safely` 消费，证明拒绝写入且无残留的库边界 | Options / `open_with_options` 收为私有并移除 re-export；保留预算判拒实现及原回归，Host 仍无预算配置 |
| `ProtectedBlobStoreOptions.{disk_budget, retention}` | Host `open_protected` 只用默认入口；非默认 retention 用于现有 GC/崩溃回归，当前没有非默认 disk_budget 消费 | Options / `open_with_options` 收为私有并移除 re-export；保留现有预算判拒和 GC 实现、7 天默认保留期。没有把未消费的 protected 预算记为已验证产品能力 |

### 恢复与契约边界

- 全 session 账本读口支撑启动清扫和用量对账；活动分支消息走 `AppCore::resume_messages*` → `projection_snapshot_on_branch` → lineage 与 compaction 水位，直接信封查询用 `events_on_lineage`。这些生产读口、分页与 Secret 脱敏实现均保留。
- checkpoint `open` 继续读取既有 `checkpoint-state-v1.json`。`FileSnapshot.pre_hash`、`RunCheckpoint.head`、空 run 记录的反序列化与持久化映射形状保留，不因删除初始化/检测方法而删磁盘字段。
- session v1–v14 迁移、休眠 `session_leases` / `session_bindings` DDL、export v3、artifact/protected schema、PWB1 信封与 fixture/golden 均无改动；protected 旧表补 state 列、pending/deleting reconcile 保留。
- 删除的 3 个测试只覆盖已移除、从未接线的 API；不作为产品安全 gate。回滚完整性、缺记录路径拒绝、路径越界、AEAD/Secret、迁移和重放锚点保留，没有新增测试体系。

写入集：`crates/storage/src/{sqlite/mod.rs,blob/artifact.rs,blob/protected.rs,blob/checkpoint.rs,blob/mod.rs}`、[storage Spec](../spec/crates/storage.md)、本文与 [ROADMAP](../ROADMAP.md)。生产调用点、Cargo manifest/lock、schema、事件格式与迁移不改动。

### 自动验证与验收状态

- checkpoint 定向回归：`cargo test -p pawork-storage --offline --lib --features compaction,checkpoint,protected blob::checkpoint::tests`，13 项通过，覆盖写前快照自动建 run、去重、逆序回滚、重开恢复与路径拒绝。
- Storage 门禁：`bash scripts/test.sh storage`（包含 compaction/checkpoint/protected），160 项库测试、4 项 PWB1 golden 与 5 项 read_range 通过，共 169 项；1 项显式 golden writer 忽略，无失败。既有迁移、lineage/重放、Secret 脱敏、artifact 预算拒绝、protected GC/reconcile 回归通过。
- 宿主与依赖闭包：`cargo check -p pawork --offline`、`cargo tree -p pawork --offline --edges normal --prefix depth` 均退出 0；没有新增依赖或改变生产 feature。
- 契约差异检查：checkpoint 持久化字段/版本、artifact/protected schema 与 PWB1 常量不变；63 个受跟踪 manifest/lock/session/migration/test/golden 文件逐字节匹配基线 HEAD。
- 格式与文档检查：5 个修改源码文件定向 `rustfmt --edition 2021 --check`、`git diff --check` 通过；109 条相对链接目标存在，storage Spec 保持八节结构。
- 日志目录：`/tmp/pawork-b2-3xbFvw/`；对应 `checkpoint-tests.log`、`storage-gate.log`、`pawork-check.log`、`pawork-tree.log`、`contract-diff-check.log`、`rustfmt-check.log` 与 `docs-check.log`。
- 独立 review：`gpt-6.1-sol / max` 对话 `01a118c1-5cee-7301-bc5d-f96f754b55ed` 于 2026-10-08 给出 PASS；核对完整 8 文件差异、真实生产消费者与上述日志，确认删除/收窄依据充分，恢复与安全锚点未削弱。审查只读，没有重跑 Cargo。
- 实现对话：`01a118b0-95c0-7be2-ab87-a018c3a5e4b4`（`gpt-6.1-sol / max`）。指挥在独立审查 PASS 后执行本地提交；不推送、不发布、不归档。没有改 GUI 或执行真实 Provider / 真实 Host 产品验收，全 workspace 门禁未运行。
