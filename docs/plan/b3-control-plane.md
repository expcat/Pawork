# B3 · Control-plane 读面与未接线能力裁决

> 状态：✅ 完成；已裁决并实现，自动验证通过，独立 review PASS。完成：2026-10-08。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决额度读面、租约恢复和未消费的凭证、预算及保留策略类型。

## 事实源与读取范围

基线为 `main` / `ce4f554a3638862db9f86e3489fccbd2f1db8d84`，开工时工作区干净；B1 / B2 已提交，不重做。先通过 CodeGraph 定位，再读当前源码与调用点；不以旧 Spec 的消费描述代替源码。

按写入集读取 [control-plane Spec](../spec/crates/control-plane.md)、[app Spec](../spec/crates/app.md)，核查真实下游时读取 [orchestration Spec](../spec/crates/orchestration.md)。Cargo manifest 表明本包生产下游只有 App 与 Orchestration。

## 逐项裁决

| 项 | 当前事实与证据 | 裁决与实际结果 |
| --- | --- | --- |
| `QuotaService::overview` / `overview_with_credential` | App `UsageService::usage_overview` → `control::quota_windows` → `overview` → `overview_with_credential` → 私有 `read_impl`，是真实额度读链 | 保留读链、TTL / singleflight / stale / 部分失败 / confidence 与 Ledger 对账；不纳入零消费者删除 |
| `read` / `read_with_credential` | 全仓外部消费者为零；`read` 有大量库内回归，带凭证入口仅自身测试消费 | `read` 收为 `cfg(test)` 私有助手；删除公开 `read_with_credential`，Secret 注入测试改调共享 `read_impl`，序列化不含 Secret 的断言保留 |
| `read_cache_only` / `overview_cache_only` | 仅 5 个专用测试消费，没有生产调用 | 删除两个入口、`CacheRead` / `CacheOverview` 及这 5 个专用测试；现有缓存、取消与 stale 回归保留 |
| `invalidate` / `invalidate_local_scope` | 全缓存 invalidate 连测试消费者都没有；`record_attributed_usage` 成功入账后真实调用 local_scope 失效（R-13） | 删除全缓存入口与私有 `QuotaCache::clear`；保留本地 scope 失效、快照发布 / 枚举及 R-13 回归 |
| `LeaseProjection` / `restore` / `reclaim_expired` | 库边界有恢复、TTL、durable-first、投影失败和并发回归；App `persistent` 仍装配 `InMemoryCredentialPool::new(4)` + `NullLeaseProjection`，没有真实持久投影或恢复 / 回收调用 | 保留库能力与全部回归，明确只有本进程准入保障；登记 [RV-2026-10-C](../spec/backlog.md#8-条件性工程项) 的持久消费者 / 所有权 / 存活判定 / 恢复接线条件，不提前装配持久池 |
| `CredentialPicker` / `LegacyCredentialPicker` | 只有 Legacy 一个实现，恒返回 opaque `CredentialId::new("default")`；没有真实选择器注入，账号选择与 Secret 解析由 App / Auth 承担 | 删除 trait、Legacy 实现、picker 字段及两个注入构造；acquire 保持 `default` 标识，既有测试改用保留的 `with_projection` |
| `BudgetCap` / `with_budget` | App 生产仅 `LedgerQuotaAdapter::new`，cap 为 None；带 cap 只有算术与 Percent 拒绝两项库测试 | 收为私有内部配置，保留两项回归；生产默认 limit / remaining 为 `Infinite`，没有本地产品预算 gate，纠正旧 Spec 的 Unknown / 预算预检描述 |
| `PolicyGate::Retention` | 无生产执行入口，但 `as_str` 标注冻结持久化字符串，serde 接受 `"retention"` | 保留兼容标签，在原稳定性测试中增加 `as_str` / serde 序列化 / 反序列化断言；不新增保留期执行器 |

租约恢复的实际语义是：未过 TTL 的 `Acquired` 恢复 active，过 TTL / 已 settled 的记录推进回收；不声称重启立即回收所有孤儿。App `projected_run_usage` 读取持久 run 投影中的已报告用量，供 Run 错误收尾补账；Go 账号额度选择仍走 `select_account_for_run` / `provider_quota`，两者都不是本地 BudgetCap 通用预算预检。

源码证据：[额度读链与 runtime 装配](../../crates/app/src/control.rs)、[入账失效与持久用量投影](../../crates/app/src/services/usage.rs)、[Run 前账号选择与错误收尾](../../crates/app/src/services/run.rs)、[租约库](../../crates/control-plane/src/credential/mod.rs)、[QuotaService](../../crates/control-plane/src/quota/service.rs)、[Ledger 派生](../../crates/control-plane/src/quota/ledger.rs)、[持久化标签](../../crates/control-plane/src/decision.rs)。

## 写入集与契约边界

生产源码只改 Control-plane 五个文件：`credential/mod.rs`、`decision.rs`、`quota/{service,ledger,mod}.rs`。同步 Control-plane / App Spec、本计划、ROADMAP 与 backlog 激活条件；App / Orchestration 生产逻辑未改。

`credential/lease.rs` 的状态 / 序列化、audit JSONL golden、usage schema / wire / 去重索引保持基线内容。无新增依赖、feature、框架、数据库迁移或 Host 恢复接线；由指挥在独立 review PASS 后本地提交，不推送、发布或归档。

## 验证与证据

日志目录：`/tmp/pawork-b3.ElBVi0`，`commands.md` 保存定位、实际命令、退出状态及一次编译失败修复记录。Cargo 串行执行，沿用仓库 kache wrapper。

| 实际命令 | 结果与覆盖 | 日志 |
| --- | --- | --- |
| `bash scripts/test.sh --log /tmp/pawork-b3.ElBVi0/control-plane-gate.log control-plane`（实际 `cargo test --offline --tests -p pawork-control-plane`，默认 sqlite） | 191 passed；恢复 / TTL / 投影失败 / 并发释放、singleflight / stale / Secret、账本幂等 / 迁移 / audit golden、Retention 标签 | `control-plane-gate.log` |
| `cargo test -p pawork-app --offline --lib --features ui-fixture services::usage::tests` | 6 passed；R-13 入账失效、R-22 启动补账恰好一次、持久用量报表 | `app-usage-regressions.log` |
| `cargo test -p pawork-app --offline --lib --features ui-fixture gateway_tests::` | 4 passed；真实 App 装配 + loopback 模拟上游的 Chat / 视频租约收尾、取消 / 部分用量、Secret 脱敏 | `app-gateway-regressions.log` |
| `cargo test -p pawork-orchestration --offline --no-default-features --lib lease` | 11 passed；Supervisor 申请 / 完成 / 失败 / 取消、作用域拒绝、资源收尾；含 5 项名称匹配 release 的纯 WorktreeGuard 回归，未启用 git | `orchestration-lease-regressions.log` |
| `cargo check -p pawork --offline` | exit 0；正式宿主与 App / Orchestration 消费闭包类型检查通过 | `pawork-host-check.log` |

首轮 Control-plane 编译发现 4 个测试残留旧 build / picker 构造与一个未使用变量，修正后上表重跑通过；失败输出保留在 `control-plane-initial-failure.log`。未增加独立测试，只扩充现有 Retention 标签测试。

`git diff --check` 与五个改动 Rust 文件的定向 rustfmt 检查通过。五篇文档的 178 个相对链接目标存在，两个包 Spec 保持八节；已删除源码名称无残留，10 个冻结 / manifest 文件与 HEAD 逐字节一致，protocol / storage diff 为空（`document-contract-checks.log`）。未执行真实 Provider / GUI 验收；自动回归不据此宣称产品人工验收。

```text
Validated: scripts/test.sh control-plane（191 passed）；App usage（6）/ Gateway（4）；Orchestration lease（11）；cargo check -p pawork --offline；rustfmt / diff / 文档链接与结构 / 冻结内容检查通过
Targeted regressions: R-13 / R-22、quota singleflight / stale / Secret、lease durable-first / TTL / 恢复 / 投影失败 / 并发释放、usage 幂等与 audit golden、Retention 标签兼容
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

## 完成记录

2026-10-08 已完成逐项裁决与实现，Control-plane / App / Supervisor 定向回归、正式宿主类型检查及最终静态检查通过。实现对话 `01a118c6-1866-7f50-8c85-91abb653dbc4` 交付后，独立 `gpt-6.1-sol` / `max` 审查对话 `01a118d8-471a-7c61-8ab9-0f0df72483ef` 只读核对源码、diff、文档与上述日志，结论为 PASS，未发现可执行问题；审查没有运行 Cargo 或修改文件。指挥同步审查状态并本地提交，ROADMAP 标 ✅。

B3 自身裁决与验证条件已满足；App 持久租约恢复仍按 RV-2026-10-C 等待真实消费者和所有权设计，本任务没有宣称已接线。真实 Provider、GUI、人工验收与发布未执行。
