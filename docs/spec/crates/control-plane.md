# pawork-control-plane

> 控制面 core：tenant / identity、usage ledger、audit JSONL、quota 投影、credential lease/pool。合并自 V1 `tenant-service` + `usage-ledger` + `audit-log`；只依赖 `pawork-domain`，被 `pawork-app` 与 `pawork-orchestration` 消费。

## 1. 职责与边界

- 单机优先的控制面（ADR-038 D1）：默认宇宙是哨兵值 `local/default`（tenant）与 `local/user`（principal），产品形态是单机单用户，**不**按多租户 SaaS 扩张。
- 五块能力：用量记账（`usage`）、审计事件（`audit` + `decision`）、租户策略与 RBAC（`tenant` + `rbac` + `identity`）、配额读取与本地对账（`quota/`）、凭证租约与并发准入（`credential/`）。
- SQLite usage ledger **自开连接**（`rusqlite` optional，feature `sqlite` 默认开），不经 `pawork-storage` 的 DatabaseActor——两者不是同一连接。
- V2 的 account-control-v1 九模块、binding/schema、OTel exporter、identity_schema 已随 ADR-038 归档；本 crate **没有** `account-control-v1` feature，lease 模块在源码注释中明确「独立于该 feature」自洽工作。
- 远端 Provider 配额适配器与 RefreshScheduler 冻结候审，不在本 crate。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~45 | 8 个 `pub mod`；crate 根 re-export audit/decision/identity/rbac/tenant/usage 常用类型；`sqlite` 门控 `SqliteUsageLedger` + `SCHEMA_VERSION`。**credential / quota 类型走模块路径**，不在根 re-export |
| `src/identity.rs` | ~220 | `DEFAULT_TENANT = "local/default"`、`DEFAULT_PRINCIPAL = "local/user"`、`default_tenant()`/`default_principal()`、`IdentityContext`（tenant + principal）、`IdentityResolver` trait、`LocalIdentityResolver`、`IdentityError`（缺失身份 fail-closed） |
| `src/audit.rs` | ~470 | `AUDIT_SCHEMA_VERSION = 1`、`AuditEventV1`（`new` / `with_dimensions` / `validate`）、`AuditAction` / `AuditDecision` / `AuditTargetKind` / `AuditDimensions`、`AuditSink` / `AuditStore` trait、`InMemoryAuditStore`、`FileAuditStore`（JSONL 追加）、`AuditError`；含 JSONL golden 测试 |
| `src/decision.rs` | ~275 | `PolicyGate`（9 个 enforcement point：`RouteCandidate` / `LeaseAcquire` / `AgentSpawn` / `RequestAdmission` / `SessionQuery` / `UsageQuery` / `AuditQuery` / `AuditExport` / `Retention`）、`PolicyDecisionKind`（`Allow` / `Deny` / `Limit` / `Fallback`）、`PolicyDecisionEvent`（版本化决策事件）、`sanitize_reason`（脱敏 + 截断） |
| `src/rbac.rs` | ~290 | `PrincipalRole`（`Admin` / `User` / `Service` / `Viewer`，`rank` / `permissions` / `allows` / `merge_deny_first`）、`Permission`（8 项）、`PermissionProfile`（`effective_role`）、`AuditExportPolicy`；合并一律 deny-first |
| `src/tenant.rs` | ~930 | `TenantPolicy`（并发上限、预算、允许的 model/provider/account、保留期、审计导出）、`PolicyDecision` / `ConcurrencyKind` / `BudgetDimension` / `TenantPolicyError`、`TenantPolicyEngine` trait（check_* + `set_policy` / `policy_version`）、`InMemoryTenantPolicyEngine`、9 个纯函数 `decide_*`（决策逻辑与引擎分离，可独立单测） |
| `src/usage.rs` | ~2 830 | 多维 usage/cost 账本：`UsageRecord`（`RECORD_VERSION = 2`）、`UsageAttribution`、`UsageTotals`、`UsageQuery`、`UsageFilterField`、`CostConfidence`、`UsageLedgerError`、`UsageLedger` trait、`InMemoryUsageLedger`、`SqliteUsageLedger`（feature `sqlite`；`SCHEMA_VERSION = 3`）、`AUTO_RECORD_ID_PREFIX = "auto-rec-"` |
| `src/credential/mod.rs` | ~2 270 | `CONTROL_PLANE_SCHEMA_VERSION = 2`、`LeaseId`、`AcquireRequest`、`CredentialLease`（**无 secret 字段**）、`LeaseOutcome`、`PoolError`、`AccountHealth`、`ReleaseReceipt`、`CredentialPool` trait、`LeaseGuard`（RAII）、`DEFAULT_LEASE_TTL_MS = 3_600_000`、`CredentialPicker` / `LegacyCredentialPicker`、`PoolConfig`、`LeaseIdGenerator`、`InMemoryCredentialPool` |
| `src/credential/lease.rs` | ~840 | canonical 租约状态机（纯领域、无 I/O 无 await）：`LeaseState`、`LeaseRecord`（versioned、无 secret；`open` / `release` / `expire` / `reclaim` / `to_public_lease`）、`LeaseEvent`、`LeaseTransitionError`、`LeaseClock`（+ `SystemLeaseClock` / `FixedLeaseClock`）、`ReclaimReport`、`LeaseProjection`（对象安全持久化 sink，+ `Null` / `InMemory` 实现） |
| `src/quota/mod.rs` | ~35 | 模块文档与 re-export：`adapter::{AdapterKind, QuotaAdapter}`、`domain::*`、`error::QuotaError`、`ledger::LedgerQuotaAdapter`、`service::{CacheOverview, CacheRead, QuotaClock, QuotaService}`；`util` 为私有模块 |
| `src/quota/adapter.rs` | ~120 | `AdapterKind`（`ApiKeyApi` / `OAuthApi` / `WebScrape` / `LocalLedger`）与对象安全异步 `QuotaAdapter` trait（`fetch` 要求 cancel-safe） |
| `src/quota/domain.rs` | ~370 | canonical 配额领域：`QuotaScope`（tenant + account + provider + optional model，`with_credential_id`）、`QuotaWindow`（`Overall` / `Rolling5h` / `Weekly` / `Monthly`）、`QuotaUnit`（`Count` / `Token` / `Cost`）、`QuotaMeasure`（`Exact` / `Infinite` / `Unknown`）、`QuotaValues`、`Confidence`（`Exact` > `Derived` > `Scraped`）、`QuotaReset`、`QuotaProvenance`（endpoint 清洗）、`QuotaRequest`、`QuotaSnapshot` |
| `src/quota/error.rs` | ~540 | `QuotaError` 十变体（`Unsupported` / `Unauthorized` / `Forbidden` / `RateLimited` / `ReauthorizationRequired` / `Timeout` / `Transient` / `Parse` / `Cancelled` / `Other`）+ 构造器、`retryable()`、`retry_after_ms()`；`detail` 必须已脱敏 |
| `src/quota/ledger.rs` | ~1 320 | `LedgerQuotaAdapter`：直接消费 `UsageLedger` 派生本地 used/limit/remaining；`BudgetCap`（`none` / `with_limit`）、远端增量 `reconcile`、`ExhaustionPrediction` / `predict_exhaustion` |
| `src/quota/service.rs` | ~2 630 | `QuotaService`：适配器注册（`ScopeMatch` 路由）、per-(scope, window, unit) 缓存（默认 TTL 30 s）、singleflight（leader 中止可恢复）、多窗口并发聚合、stale 兜底；`QuotaClock`（+ `SystemQuotaClock` / `MutableQuotaClock`）、`QuotaRead` / `WindowRead` / `QuotaOverview` / `QuotaFailure` / `CacheRead` / `CacheOverview` |
| `src/quota/util.rs` | ~620 | **私有**工具：UTC 日历换算（`next_month_start_timestamp` 等）与脱敏（`redact_endpoint` / `redact_secrets` / `redact_source`） |
| `fixtures/audit/event-v1.jsonl` | 1 行 | audit JSONL 冻结 golden（单行、`\n` 结尾） |

无 `tests/` 目录；回归全部在 `src/` 内联 `#[cfg(test)]` + audit JSONL golden。

## 3. 对外 API 面

**identity（根 re-export）**

- `IdentityContext { tenant_id, principal_id }`：一次请求 / 一条持久记录的身份上下文。
- `IdentityResolver` trait：解析失败必须 fail-closed（`IdentityError`），不得静默回退默认值。
- `LocalIdentityResolver`：单机实现，产出 `local/default` + `local/user`；`default_tenant()` / `default_principal()` 是同一哨兵值的便捷构造。

**usage（根 re-export；`SqliteUsageLedger` 由 feature `sqlite` 门控）**

- `UsageLedger` trait：`record(UsageRecord)`（幂等写入）、`query(&UsageQuery) -> Vec<UsageRecord>`、`aggregate(&UsageQuery) -> UsageTotals`。存储 / 行解码错误返回 `UsageLedgerError::Storage`，**绝不静默降级为空集**。
- `UsageRecord`（v2）分四组字段：
  - 身份归属：`tenant_id` / `principal_id` / `account_id` / `credential_id`（opaque 定位符，非 secret，允许持久化）；
  - run 归属：`session_id` / `agent_id` / `run_id`；
  - 计量：`input_tokens` / `output_tokens` / `cache_read_tokens` / `cache_write_tokens`、`cost_micros`、`currency`、`occurred_at_ms`；
  - v2 新增：trace（`request_id` / `event_id` / `upstream_attempt` / `trace_id`）与定价快照（`rate_card` / `rate_version` / `cost_confidence`（`Estimated` / `Actual` / `Unknown`）/ `cost_provenance`）。旧 v1 JSON 缺省字段可解码（`version` 缺省为 1）。
- `UsageAttribution`：由宿主在 run 生命周期注入的归属五元组（tenant / principal / account / credential / trace），账本不自行猜测默认账号。
- `UsageQuery`：`by_tenant` / `by_session` / `by_agent` / `by_credential` / `by_run` / `by_provider` / `by_model` / `by_currency` / `by_occurred_between`（半开区间）构造器，可叠加过滤；`UsageTotals` 聚合四类 token + `cost_micros`（饱和累加）。
- 错误：`InvalidRecord`（校验失败）、`Conflict`（幂等冲突）、`MixedCurrencies`（跨币种聚合拒绝）、`Storage`。
- `SqliteUsageLedger::open(path)`：自开 rusqlite 连接（`Mutex<Connection>`，整体 `Send + Sync`）；建表 + 迁移到 `SCHEMA_VERSION = 3`。

**audit / decision（根 re-export）**

- `AuditEventV1::new(...)` 构造 → `validate()` 校验 → `AuditSink::append` 落盘。事件只含结构化元数据：who（tenant / principal）、what（`AuditAction`）、result（`AuditDecision`）、target（`AuditTargetKind` + id）、`AuditDimensions`（provider / model / account 等维度），**不含** prompt、secret、tool 输出。
- `with_dimensions` builder 附加维度；`AUDIT_SCHEMA_VERSION = 1` 写进每条事件。
- `AuditSink::append(event)` 是唯一写入口；`AuditStore: AuditSink` 增加查询能力。
- `FileAuditStore::open(path)`：JSONL 追加式存储（一行一条、`\n` 结尾）；`InMemoryAuditStore` 供测试；`path()` 暴露落盘位置。
- `PolicyDecisionEvent::new(gate, kind, reason, ...)`：策略决策审计事件（版本化）；`kind_of(&PolicyDecision)` 把 tenant 决策映射为 `PolicyDecisionKind`；`reason` 一律过 `sanitize_reason`（脱敏 + 长度截断）。

**tenant / rbac（根 re-export）**

- `TenantPolicy` 字段族：Agent / Request 两类并发上限、按 `BudgetDimension` 的预算、允许的 model / provider / account 白名单（`None` = 不限制）、保留期天数、审计导出策略。
- `TenantPolicyEngine`：按 `TenantId` 查策略并执行 `check_*`（并发 / 预算 / model / provider / account / 权限 / 保留期 / 审计导出）；返回 `PolicyDecision`（allow / deny / limit 语义）。`set_policy` / `policy_version` 支持热更新与乐观版本。
- 纯函数 `decide_*` 系列（9 个：agent / request 并发、model、provider、account、permission、retention、audit_export、budget）：无状态决策逻辑，引擎实现与宿主可复用；`decide_permission` 基于 `PrincipalRole::permissions()` 静态表。
- `Permission` 八项：`AgentSpawn` / `RouteCandidate` / `LeaseAcquire` / `SessionRead` / `UsageRead` / `AuditRead` / `AuditExport` / `PolicyManage`。
- `PrincipalRole`：`rank()` 给出权限强弱序，`merge_deny_first` 合并取低权限侧；`PermissionProfile::effective_role(principal)` 查主体生效角色；`AuditExportPolicy` 控制审计导出授权。

**credential（模块路径 `credential::`）**

- `CredentialPool` trait：`acquire(AcquireRequest) -> CredentialLease`、`acquire_guard -> LeaseGuard`、`release(lease_id, LeaseOutcome) -> ReleaseReceipt`（幂等，未知 / 已释放返回 `already_released = true`）、`active_count` / `account_health`（legacy 聚合）与 `active_count_for` / `account_health_for`（tenant-scoped canonical 视图，跨租户同名账号互不影响）、`reclaim_expired`（TTL 扫描回收）、`lease_state`（可观测性）、`restore`（崩溃恢复；默认空实现）。
- `AcquireRequest { tenant_id, principal_id, session_id, agent_id, provider_id, account_id, trace_id }`：`provider_id` / `account_id` 为 `None` 时池取默认值（账号默认 `local/default`）；`trace_id` 贯通到 `UsageRecord.trace_id`。
- `CredentialLease`：公开视图，携带 lease / credential / account / provider / agent / session / principal / tenant 定位信息与 `acquired_at_ms` / `expires_at_ms` / `version`——**无任何 secret 字段**；resolve 明文 secret 是宿主的事。
- `LeaseOutcome`：`Completed` / `Cancelled` / `Failed` / `Released`；只有 `Failed` 累加 `AccountHealth.consecutive_failures`，`Cancelled` 只累加取消计数（取消不惩罚账号健康）。
- 错误 `PoolError`：`NoCandidate`、`ConcurrencyExhausted`（账号并发满，携带 active / max）、`TenantConcurrencyExhausted`（租户 cap）、`TenantDenied`、`Projection`（持久化投影失败）。
- `LeaseGuard`：RAII；`Drop` 以当前 outcome 释放（先尝试同步完成，`Pending` 则交 detached task 驱动到完成，杜绝额度泄漏）；`outcome_mut()` 在释放前改写分类；`into_lease()` 取走后 `Drop` 无副作用。
- `CredentialPicker` trait / `LegacyCredentialPicker`：账号 → 凭据的候选选择策略（默认按账号同名派生）。
- `InMemoryCredentialPool`：默认实现；`PoolConfig::new(max).with_tenant_cap(..).with_ttl_ms(..).with_account_override(tenant, account, max)`（`max_for` 分层取值：账号覆盖 > 默认）；`LeaseIdGenerator::system()`（进程唯一前缀 + 计数，崩溃重启不重复）；`with_projection` 注入 `LeaseProjection`；`recover_records` 从快照重建。`DEFAULT_LEASE_TTL_MS = 3_600_000`（1 小时）。

**quota 领域类型（`quota::domain`，经 `quota::*` re-export）**

- `QuotaScope::new(tenant, account, provider, model)`：隔离作用域四元组（model 可选）；`with_credential_id` 附加凭据维度。
- `QuotaWindow`：`Overall`（不随时间重置）/ `Rolling5h` / `Weekly` / `Monthly`；`QuotaUnit`：`Count` / `Token` / `Cost`。
- `QuotaMeasure`：`Exact(u64)` / `Infinite` / `Unknown`（`exact_value()` 仅对 Exact 返回 Some）——三态语义贯穿 used / limit / remaining（`QuotaValues`）。
- `Confidence::priority()`：`Exact > Derived > Scraped`，聚合择优依据。
- `QuotaReset`：`Absolute { at, uncertain }` / `Relative { after_secs, observed_at, uncertain }` / `Unknown`（缺省）；`QuotaProvenance`（adapter_kind / source / 可选 endpoint / fetched_at / stale 等）的 endpoint 必须经 `canonical_endpoint(raw)` 清洗 query / fragment 后才允许携带。

**quota 服务（模块路径 `quota::`；`QuotaRead` / `QuotaOverview` / `WindowRead` / `ScopeMatch` / `QuotaFailure` 在 `quota::service::`）**

- `QuotaService`：`register(ScopeMatch, Arc<dyn QuotaAdapter>)` 注册适配器（`ScopeMatch::any()` / `for_provider(id)` 路由）；`read` / `read_with_credential`（单 (scope, window, unit)）与 `overview` / `overview_with_credential`（多窗口并发）；`read_cache_only` / `overview_cache_only`（纯缓存判读，不触发 fetch，返回 `CacheRead` / `CacheOverview`）；`publish_local_snapshot` / `cached_snapshots_for_scope`（本地投影直写缓存）；`set_ledger_reconciler` 挂接 Ledger 对账；`invalidate` / `cache_size`；`new`（TTL 30 s）/ `with_ttl`（0 TTL = 永不新鲜，逢读必 fetch 但仍 singleflight，最近缓存留作 stale 兜底）。
- `QuotaAdapter::fetch` 契约：对象安全、cancel-safe（调用方可随时 drop future）；secret 仅以 `pawork_domain::ResolvedCredential` 在适配器调用边界注入——该类型 `Debug` 脱敏且未实现 `Serialize`，本 crate 结构上无法持有或泄漏明文。
- `QuotaSnapshot`：scope + window + unit + `QuotaValues`（used / limit / remaining，均为 `QuotaMeasure`）+ `Confidence` + `QuotaReset`（绝对 / 相对 + 不确定性）+ `QuotaProvenance`（adapter kind、脱敏 endpoint、取数时刻）。
- `QuotaError::retryable()`：`RateLimited` / `Timeout` / `Transient` 可重试，`retry_after_ms()` 透传服务器建议；`Unauthorized` / `Forbidden` / `ReauthorizationRequired` 需要上层处理凭证后再试。
- `LedgerQuotaAdapter::new(ledger, clock)` / `with_budget(BudgetCap)`：从唯一 UsageLedger 派生本地 used（`BudgetCap` 提供 limit），`reconcile` 做远端增量对账，`predict_exhaustion` 给出耗尽预测（`ExhaustionPrediction`）。
- 时钟注入：`QuotaClock`（`SystemQuotaClock` 生产 / `MutableQuotaClock` 测试 `set` / `advance`）；credential 侧对应 `LeaseClock`（`SystemLeaseClock` / `FixedLeaseClock`），两侧独立定义，宿主可 cross-wiring。

**公开常量速查**

| 常量 | 值 | 含义 |
| --- | --- | --- |
| `DEFAULT_TENANT` / `DEFAULT_PRINCIPAL` | `"local/default"` / `"local/user"` | 单机哨兵身份（ADR-038 D1） |
| `RECORD_VERSION` | 2 | `UsageRecord` 当前版本（v1 JSON 兼容解码） |
| `SCHEMA_VERSION`（feature `sqlite`） | 3 | usage SQLite 库 schema 版本 |
| `AUTO_RECORD_ID_PREFIX` | `"auto-rec-"` | 自动记录 ID 保留前缀 |
| `AUDIT_SCHEMA_VERSION` | 1 | `AuditEventV1` schema 版本 |
| `CONTROL_PLANE_SCHEMA_VERSION` = `LEASE_SCHEMA_VERSION` | 2 | 凭证租约实体 schema 版本（与 app-database 迁移对齐） |
| `DEFAULT_LEASE_TTL_MS` | 3 600 000 | lease 默认 TTL（1 小时） |

## 4. 核心行为与数据流

**usage 记账与去重**

1. 调用方构造 `UsageRecord`（宿主从 `CredentialLease` / `IdentityContext` 派生 `UsageAttribution`）→ `UsageLedger::record`。
2. 校验（tenant 非空、token 计量、时间戳）失败 → `InvalidRecord`；`record_id` 为空时账本以原子计数器补写 `auto-rec-N`（仅内存实现的 legacy 行为，生产要求调用方给跨进程稳定 ID）。
3. 幂等判定：同 `(tenant, account)` 内相同 `record_id` + 相同内容 → 重放成功不重复记账；相同 ID 不同内容 → `Conflict`。
4. SQLite 存储层第二道防线：`UNIQUE(tenant_id, account_id, record_id)` 主键 + 部分唯一索引 `idx_usage_dedup ON (tenant_id, account_id, request_id, COALESCE(upstream_attempt,'0')) WHERE request_id IS NOT NULL`——带 request 的记录按 (request, attempt) 去重，不同 `record_id` 的重复观测也会被拒（不只信自造 record_id）；`request_id` 为 `None` 的记录不参与该去重（`upstream_attempt` 缺省按 `'0'` 折叠，防止 NULL/NULL 绕过唯一性）。
5. 并发竞态兜底：约束冲突后事务内重读——同 `record_id` 同内容判重放成功、同 dedup 键不同 `record_id` 判 `Conflict`、两者都不匹配报 `Storage`（并发插入丢失），跨进程并发重放安全。
6. 账本只追加（immutable append），不更新不删除；`aggregate` 饱和累加，命中多币种且未按币种过滤 → `MixedCurrencies`。

**credential lease 生命周期**

1. `acquire`：解析默认值（provider / account 缺省取池默认，账号 `local/default`）→ 租户 / 账号并发额度检查（`PoolConfig::max_for` 分层：按 (tenant, account) 覆盖 > 默认上限；租户 cap 独立判定）。
2. 通过后 `LeaseIdGenerator::next()` 产生进程内唯一 `LeaseId` → `LeaseRecord::open`（状态 `Requested → Acquired`，记 `acquired_at_ms` / `expires_at_ms = acquired + ttl_ms`，产生 `LeaseEvent`）。
3. 若配置投影：事务化持久化 snapshot + 累计事件（`LeaseProjection::apply`）；失败回滚 active 计数并返回 `PoolError::Projection`——**先持久化后放行**（durable-first）。成功则返回公开视图 `CredentialLease`（`to_public_lease`）。
4. 使用期间 `AccountHealth` 跟踪 `active_leases`；lease 的 `credential_id` 只是 opaque 定位符，resolve 明文 secret 是宿主的事。
5. `release(lease_id, outcome)`：`Acquired → Released` 归还额度；只有 `Failed` 累加 `consecutive_failures`，`Cancelled` 只累加取消计数（取消不惩罚账号健康）；重复释放幂等（`already_released = true`）。`LeaseGuard` 被 Drop 时自动走该路径。
6. TTL 过期：`reclaim_expired` 扫描过期的 `Acquired → Expired`（归还额度），再把所有 `Released` / `Expired` 收敛到终态 `Reclaimed` 并事务化持久化每个 lease 的终态 snapshot 与事件；宿主在启动 / 周期心跳时调用；返回 `ReclaimReport`。
7. 崩溃恢复：`restore` / `recover_records` 从投影读非终态快照，重建 active 计数并回收孤儿 lease——重启不泄漏额度。

**quota 读取与窗口聚合**

1. `read(request)`：`QuotaRequest`（scope + window + unit）先查 per-key 缓存，TTL 内的 fresh 条目直接返回（`CacheRead::is_hit`）。
2. 缓存未命中进 singleflight：同 key 并发请求只有 leader 真正调用 `QuotaAdapter::fetch`，followers 等待共享结果；leader future 被 drop（调用方取消 / panic）时 `LeaderGuard` 标记 leaderless 并唤醒 followers，下一个 follower 晋升重跑（abort-safe，有界重试）。
3. `overview`：对 scope 匹配（`ScopeMatch`）的所有适配器按多窗口并发 fetch，按 `Confidence` 优先级（`Exact > Derived > Scraped`）取最优快照；其余失败以 `QuotaFailure`（typed，含 retryable / retry_after_ms）附带返回——部分失败不掩盖成功窗口（`QuotaOverview::ok_count` / `all_failures`）。
4. fetch 失败且存在旧缓存时才返回 stale 数据，并置 `QuotaRead::served_stale` 让来源 / 新鲜度链路可见；成功 fetch 总是刷新缓存。
5. `publish_local_snapshot` 允许本地投影（如 Ledger 派生、宿主推送）直接写入缓存；`set_ledger_reconciler` 注册后，读路径可用 Ledger 事实对远端读数做增量对账。
6. `LedgerQuotaAdapter` 把 usage 事实换算为窗口读数：窗口起点用 UTC 日历函数（如 `month_start_unix_seconds` / `next_month_start_timestamp`），`BudgetCap` 给出 limit，remaining = limit − used（饱和）；无 budget 时 limit / remaining 为 `Unknown`。

**audit 事件**

1. 构造 `AuditEventV1`（schema_version = 1，action / decision / target / dimensions）→ `validate`（必填字段、禁止敏感内容）→ `AuditSink::append`。
2. `FileAuditStore` 逐行追加 JSONL（一行一条、`\n` 结尾）；策略决策经 `PolicyDecisionEvent` + `sanitize_reason` 汇入同一审计通道。

**tenant 策略决策**

1. 宿主在各 enforcement point（`PolicyGate` 九点位）调用 `TenantPolicyEngine::check_*`，引擎按 `TenantId` 取策略（无则用构造时的默认策略）。
2. `check_*` 内部委托无状态 `decide_*` 纯函数：白名单为 `None` 视为不限制、命中拒绝返回 deny（含原因）、并发 / 预算比较返回 allow 或 limit。
3. 决策结果可选地包装为 `PolicyDecisionEvent`（`kind_of` 映射 kind、`sanitize_reason` 清洗原因）写入审计通道；`policy_version` 随 `set_policy` 递增，供乐观并发检查。

## 5. 契约与不变量

- **usage 去重契约（冻结）**：`record_id` 在 `(tenant, account)` 作用域内是幂等键；SQLite 侧 `idx_usage_dedup` 部分唯一索引按 `(tenant, account, request_id, COALESCE(upstream_attempt,'0'))` 强制 event/request attempt 去重。账本 append-only。
- **audit JSONL golden（冻结）**：`AuditEventV1` 序列化必须与 `fixtures/audit/event-v1.jsonl` **逐字节**一致（内联测试 `audit_event_v1_jsonl_matches_frozen_fixture`）；fixture 中不得出现 `prompt` / `secret` / `tool_output` 字段。`AUDIT_SCHEMA_VERSION = 1`。
- **版本常量**：`RECORD_VERSION = 2`（v1 JSON 缺省解码兼容）；SQLite `SCHEMA_VERSION = 3`（v2 → v3 迁移保留历史、补 `trace_id` 列与去重索引）；`CONTROL_PLANE_SCHEMA_VERSION = 2` = `LEASE_SCHEMA_VERSION`（与 app-database `credential_leases` 迁移对齐）。
- **lease 状态机（冻结）**：`Requested → Acquired → Released | Expired → Reclaimed`；`holds_slot`（占用并发额度）只在 `Acquired`；`is_settled` 表示 `Released` / `Expired`，终态（`is_terminal`）只有 `Reclaimed`；非法迁移返回 `LeaseTransitionError`；`as_db_str` / `from_db_str` 的字符串形态与持久化层对齐。
- **LeaseRecord 版本单调**：每次合法状态迁移 `version` 自增并产出对应 `LeaseEvent`，投影按事件序回放可重建同一快照。
- **无 secret 不变量**：`CredentialLease` / `LeaseRecord` / audit / quota 各视图均无明文 secret 字段；`ResolvedCredential` 不可序列化；`QuotaProvenance::canonical_endpoint` 清洗 query / fragment；`QuotaError.detail` 必须已脱敏。
- **单机哨兵语义（ADR-038 D1）**：`DEFAULT_TENANT = "local/default"`、`DEFAULT_PRINCIPAL = "local/user"`；池的默认账号同为 `local/default`。
- **record_id 命名空间**：`auto-rec-*` 前缀保留给账本自动补写与系统组件（如预算控制器 flush）；显式 ID（惯例 `rec-*`）不得使用该前缀，否则可能与自动 ID 冲突。
- **quota 单飞与缓存语义**：stale 只在 fresh fetch 失败时返回且必须带 `served_stale`；singleflight leader 中止可恢复（不死锁、不重复计费上游）。
- **fail-closed**：identity 解析失败、账本存储错误、跨币种聚合、投影持久化失败——全部显式报错，不静默降级。

## 6. 依赖关系

- **上游**：`pawork-domain`（[domain.md](domain.md)，唯一 `pawork-*` 依赖）——提供全部 opaque ID 类型（`TenantId` / `PrincipalId` / `AccountId` / `CredentialId` / `SessionId` / `AgentId` / `RunId` / `RequestId` / `EventId` 等）、`Timestamp` 与 `ResolvedCredential`（脱敏凭证载体）。
- **外部依赖**：`async-trait`（trait 异步化）、`futures`、`serde` / `serde_json`（记录与事件序列化）、`thiserror`、`tracing`、`tokio`（sync / rt / macros / time）、`url`（endpoint 清洗）、`rusqlite`（optional，SQLite 账本）。
- **features**：`default = ["sqlite"]`；`sqlite = ["dep:rusqlite"]`。无其他 feature。
- **下游**：`pawork-app`（[app.md](app.md)，默认 sqlite，注入 UsageLedger / TenantPolicyEngine / AuditStore / CredentialPool / QuotaService）；`pawork-orchestration`（[orchestration.md](orchestration.md)，`default-features = false`，只用类型与 trait，不拉 rusqlite）。

## 7. 测试与验证资产

无独立 `tests/` 目录，约 245 个内联测试分布如下（`#[test]` + `#[tokio::test]` 计数）：

- `usage.rs`（36）：幂等重放 / 冲突、存储层去重（`sqlite_dedup_unique_index_is_registered` 断言 `idx_usage_dedup` 已登记、`sqlite_dedup_by_request_and_attempt_conflicts` 断言不同 record_id 的同 (request, attempt) 冲突、`in_memory_dedup_matches_sqlite_semantics` 保证双实现语义一致）、`sqlite_v2_to_v3_migration_preserves_history`（迁移保历史）、跨币种聚合拒绝、查询过滤与半开区间、`Send + Sync` 断言。
- `credential/mod.rs`（27）+ `credential/lease.rs`（9）：并发额度（账号 / 租户 cap / 按 (tenant, account) 覆盖）、幂等释放、`LeaseGuard` Drop 释放（含 detached 驱动）、TTL 过期与 `reclaim_expired`、投影事务失败回滚计数、`recover_records` 崩溃恢复、状态机合法 / 非法迁移、property 测试（proptest）。
- `quota/service.rs`（34）：缓存 TTL / invalidate、singleflight 并发去重（`singleflight_dedups_concurrent_reads`）与 leader 中止后 follower 晋升、stale 兜底 + `served_stale` 标记、部分失败聚合（`QuotaFailure` 附带）、confidence 择优。
- `quota/ledger.rs`（28）：Ledger 派生窗口读数（月窗 / 滚动窗）、`BudgetCap` limit 语义、`reconcile` 对账、耗尽预测。
- `quota/domain.rs`（8）/ `quota/error.rs`（11）/ `quota/adapter.rs`（2）/ `quota/util.rs`（17）：领域类型不变量（measure / confidence 优先级 / endpoint 清洗）、错误可重试分类与 retry_after 透传、secret 脱敏、UTC 日历换算（闰月 / 月界）。
- `audit.rs`（5）：**`audit_event_v1_jsonl_matches_frozen_fixture`——与 `fixtures/audit/event-v1.jsonl` 逐字节比对**，并断言 fixture 单行、`\n` 结尾、不含 `prompt` / `secret` / `tool_output`；validate 拒绝路径。
- `decision.rs`（6）：`sanitize_reason` 脱敏与截断、gate / kind 标签稳定。`rbac.rs`（6）：deny-first 合并、角色权限表。`tenant.rs`（11）：各 `decide_*` 决策分支。`identity.rs`（5）：默认哨兵身份与 fail-closed。

默认验证命令：`cargo test -p pawork-control-plane --offline --lib --tests`。

## 8. 注意事项与已知限制

- 归档资产不存在于本包：account-control-v1 九模块、binding/schema、OTel exporter、identity_schema（复活条件见 [../../../ROADMAP.md](../../../ROADMAP.md) §5）。RBAC 三类型（role / permission / profile）保留。
- `mask_credential_hint` **不在本包**（在 `pawork-protocol`，见 [protocol.md](protocol.md)）；本包 quota 的脱敏工具（`redact_endpoint` / `redact_secrets` / `redact_source`）在私有模块 `quota/util.rs`，对外不可见。
- `quota::service` 的 `QuotaRead` / `QuotaOverview` / `WindowRead` / `ScopeMatch` / `QuotaFailure` 需走 `quota::service::` 路径（`quota::` 只 re-export `QuotaService` / `QuotaClock` / `CacheRead` / `CacheOverview`）。
- `SqliteUsageLedger` 与 `pawork-storage` DatabaseActor 是**两条独立连接**；同库并发访问依赖 SQLite 自身锁。
- `InMemoryUsageLedger` 的空 `record_id` 自动补写是 legacy 测试便利，生产幂等性必须由调用方提供稳定 ID。
- `SqliteUsageLedger` 的连接是 `Mutex<Connection>` 同步互斥：`record` / `query` / `aggregate` 在 async trait 内短临界区持锁，长事务会阻塞同进程其他账本调用。
- quota 的时间换算全部按 UTC 日历（无时区配置）；`Rolling5h` 等相对窗口以 `QuotaClock` 注入时钟推算，测试用 `MutableQuotaClock` 拨表。
- credential 池的公平性：无等待队列，额度耗尽即时返回 `ConcurrencyExhausted`，重试节奏由调用方决定。
- `LeaseGuard::Drop` 的 detached 释放依赖 tokio runtime 存活；在 runtime 关闭后 drop guard 无法保证释放落投影（进程退出场景由 `restore` 兜底）。
- 架构位置与冻结契约总表见 [../../design.md](../../design.md) §2 / §3.2 与 [../contracts.md](../contracts.md)；跨包数据流见 [../flows.md](../flows.md)；产品 Spec 汇总见 [../README.md](../README.md)。
