# P14-10：Phase 14 评审修复（REVIEW remediation）

> Phase 14 · Usage / Quota · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P14-1 ~ P14-9

**最终目的**：按 [docs/review/p14-review.md](../docs/review/p14-review.md) §2/§3 的改进优先级收敛 Phase 14「库完整、产品链路未闭环」之外的复杂度与契约问题——删除零生产消费者的 capability matrix 与 OAuth 通用层、把复制四处的时间算法 / 双端点错误归并 / 三套脱敏规则收敛为单一事实源、压平 cache 读取形状并消灭伪造的失败归属（§3.6）、简化告警跨边界模型（删除可派生且最终被丢弃的 `Alert.suggestions`，补稳定 `AlertKind` + 脱敏 `source` 透传并重生成 schema）、CLI 改为 typed 解析与 typed 渲染、`QuotaOverview` 改为显式 provider 必填（不再静默选「第一个」）。结构性接线（持久化 Ledger、真实 tenant/account/credential 归属、生产 refresh/audit 生命周期、GUI 实际投影、§3.7 WebScrape 审计 Vec 合并）按评审结论显式延后至 Phase 18 / Phase 19 配套任务，不在此任务内扩大控制面。无新增 crate 与抽象，全部为「删重复 / 收口 / 压平 / 接线小修」。

**涉及范围**：`quota-service`（adapters/{mod,http_util,web_scrape}.rs、adapters/oauth.rs 删、providers/{mod,capability 删,openai,anthropic,xai,qwen}.rs、domain.rs、error.rs、ledger.rs、lib.rs、refresh.rs、service.rs、util.rs 新、Cargo.toml）、`app-service`（lib.rs、router.rs、supervisor.rs、tests/quota_overview.rs）、`core-api`（lib.rs）、`cli-command`（lib.rs）、`cli-host`（lib.rs）、`protocol-test-gui`（scenarios.rs：`subscribe_all_landed` 订阅落地屏障）、`gui-server`（session.rs：`spawn_forwarder` spawn 前同步建立 Hub receiver + 回归测试）、`schemas/core-api`、`schemas/gui-protocol`、`Cargo.lock`（自动同步）

## 处置策略（按评审 §6 矩阵）

- **现在修复（落地）**：§3.1 删 capability matrix（W1）、§3.2 删零消费者 OAuth 通用层 + auth-service 依赖（W1）、§3.3 时间算法单一事实源（W2）、§3.5 脱敏单一事实源 + Qwen provenance 基于实际 base（W2）、§3.4 双端点错误归并单一实现（W3）、§3.6 cache 压平 + 恒值字段/死函数/伪造归属清理（W4，含失败归属可空化）、§2.6 告警跨边界模型（删 suggestions、补 kind/source 透传、schema 重生成）（W5）、§2.4 QuotaOverview 显式 provider（W6）、§3.8 CLI typed 解析/渲染（W7）、§6 P0 状态语义（Phase 14 标注「库级收口、生产接线待 P18」，遵循既有 plan-README / testing.md L0/L1 规则，W8）。
- **显式延后（含后续任务）**：§2.1 持久化 Ledger 与真实 attribution → [P18-8 Usage / Cost Ledger](../plan/P18-8-usage-cost-ledger.md)（输入依赖 [P18-2 Tenant / Principal](../plan/P18-2-tenant-principal.md)、[P18-3 Provider Account](../plan/P18-3-provider-account.md)、[P18-4 Credential Lease](../plan/P18-4-credential-lease.md)）；§2.2 生产 refresh target 注册 / scheduler 生命周期 / audit sink → [P18-13 Canonical Audit / OTel](../plan/P18-13-audit-otel.md)、[P18-14 Provider Registry / Pool Reconciliation](../plan/P18-14-pool-reconciliation.md)；§2.3 / §2.5 GUI 实际投影与页面 → [P19-2 Client State Projection](../plan/P19-2-client-state-projection.md)、[P19-10 Provider Account Settings](../plan/P19-10-provider-account-settings.md)（Phase 19 落地时消费已交付的 Core 契约）；§3.7 WebScrape 内部审计 Vec 合并 → P18-13；§3.2 ApiKey 单消费者层内联决定 → P18 真实接线后评估。

## 细分步骤（分组）

### A. §3.1 + §3.2 删零消费者 capability / OAuth（W1，quota-service）

1. 删 `providers/capability.rs`（454 行：`Capability`、`CredentialKindHint`、`capability_matrix`、`capability_for`、`now()`）；`providers/mod.rs` 模块声明与文档改为「能力事实源是各适配器 `QuotaAdapter::supports`」；qwen.rs 凭证契约注释同步移除 hint 引用。
2. 删 `adapters/oauth.rs`（1142 行：`OAuthQuotaAdapter` / endpoint / source 通用层）与 `adapters/mod.rs` 对应声明；`Cargo.toml` 移除 `auth-service` 依赖（该层首批六家 provider 均未消费）；`sleep_or_cancel` 注释同步。
3. 残留核验：`rg "capability_matrix|CredentialKindHint|OAuthQuotaAdapter|oauth::"` 在 crates/ apps/ 零生产命中。

### B. §3.3 + §3.5 时间 / 脱敏单一事实源（W2，quota-service）

4. 新 `util.rs`：`now_millis`、Hinnant 日历换算（`epoch_to_utc_from_days` / `civil_to_days` / `epoch_to_utc`）、自然月边界（`next_month_start_timestamp` / `month_start_unix_seconds`，显式接收 `now` 不读墙钟）、脱敏（`redact_endpoint` / `redact_secrets` / `redact_source`）；`domain::canonical_endpoint` 与 `adapters::http_util` 改为委托 / re-export，`service::SystemQuotaClock::now` 改调 `util::now_millis`。
5. 删 OpenAI / Anthropic / xAI 各自的 `next_month_start_timestamp` / `epoch_to_utc_from_days` / `civil_to_days` 复制、Qwen 私有 `epoch_to_utc`、`http_util` 私有 `redact_endpoint` / `redact_secrets` / `mask_token_like` / `truncate_chars`、`refresh::redact_source`（8 个 SECRET_KEY_VALUE marker）——全部收口到 `util`；util 自带固定时钟断言（月初/下月 1 号边界）与脱敏契约测试。
6. Qwen provenance `endpoint` 由固定 `ENDPOINT` 常量改为 `redact_endpoint(&self.base)`（请求实际使用的 base，测试注入 wiremock URI），测试断言 `ep == server.uri()` 且不含 `Signature` query。

### C. §3.4 双端点错误归并单一实现（W3，quota-service）

7. `error.rs` 新增 `merge_dual_failures` + `pick_winner` + `failure_rank` + `failure_label`：单一优先级表 `Cancelled > Unauthorized > ReauthorizationRequired > Forbidden > RateLimited > Timeout > Transient > Parse > Unsupported > Other`，`retry_after_ms` 取两端较大值，组合消息只含固定类别标签（不拼接子错误 detail）；错误分类与 `retry_after` 判定与参数顺序无关，完全平局时 `status` 保留首参数——测试名更新为 `merge_dual_failures_classification_and_retry_after_are_order_independent` 与 `merge_dual_failures_retry_after_takes_max_and_tie_keeps_first_param`。
8. 删 OpenAI `combine_endpoint_errors` / `FailureKind::rank` 整台状态机与 xAI `merge_dual_failures` / `failure_priority` 私有复制，两 adapter 改调 `crate::error::merge_dual_failures`（403 vs 429 统一取 Forbidden；Unauthorized 高于 ReauthorizationRequired，顺序确定）。

### D. §3.6 压平 cache / 失败归属（W4，quota-service + app-service）

9. `service.rs`：`CacheRead::{Hit,Stale,NoData}` 删除恒值 `from_cache` 字段并删除 `CacheWindowRead` 包装层（`CacheOverview.windows` 直接 `HashMap<QuotaWindow, CacheRead>`）；`validate_scope` 改返回普通 `QuotaError`，`publish_local_snapshot` / `read_cache_only` 同步去 `QuotaFailure` 包装；`QuotaFailure.adapter_kind` 改 `Option<AdapterKind>`（`new` 为 Some、新增 `domain()` 为 None——scope 校验 / 无候选 / 取消 / 内部耗尽不虚构归属）。
10. `ledger.rs`：删 `ExhaustionPrediction.uncertain` 恒值字段；`web_scrape.rs`：删 `_measure_used` 死函数（`allow(dead_code)` 消失）；`refresh.rs`：`AuditEntry.adapter_kind` 改 `Option`，`partial_failure` 去重与恢复状态机仅接受有真实 adapter 归属的失败，无归属查询级失败不入 partial_active 槽位；app-service `failure_view_from` 透传 `Option`（schema 可空）。

### E. §2.6 告警跨边界模型（W5，quota-service + app-service + core-api + schema + protocol-test-gui）

11. `refresh.rs`：删 `AlertSuggestion` 枚举与 `Alert.suggestions` 字段（`AlertKind::suggestions()` 一并删），5 个 emit 点清理；`Alert` 保留稳定 `AlertKind`。
12. `core-api`：新增冻结形态 `QuotaAlertKind`（snake_case，与 quota-service `AlertKind` 1:1）；`QuotaAlert` 增加 `kind: Option<QuotaAlertKind>` 与 `source: Option<String>`（serde default + skip_serializing_if，旧持久化 JSON 缺字段解码为 None 保重放兼容）；`QuotaFailureView.adapter_kind` 改 `Option<QuotaAdapterKind>`；`quota_alert_round_trip_is_safe` / `quota_alert_legacy_json_without_kind_source_decodes_to_none` / `quota_alert_kind_serde_is_stable_and_exhaustive` 测试。
13. `app-service::supervisor`：`quota_alert_from` 完整映射 kind（`quota_alert_kind_from` 穷尽匹配，不再丢弃），source 经 `redact_secrets` 二次脱敏后透传（最后防线），`quota_alert_kind_mapping_is_exhaustive_and_stable` + 二次脱敏断言。
14. `schema-typegen` 重生成 `schemas/{core-api,gui-protocol}/{QuotaAlert,QuotaFailureView,index}.d.ts` + 新 `QuotaAlertKind.d.ts`；`protocol-test-gui` 新增 `quota-alert-roundtrip` 场景（真实 ServerFrame 编解码 roundtrip、旧 JSON 缺字段兼容、adapter_kind None 不序列化）。

### F. §2.4 QuotaOverview 显式 provider（W6，app-service）

15. `router.rs`：删除「缺省取首个已注册 provider / 空默认 ID」逻辑，`provider_id` 缺失或为空返回 `InvalidRequest`（明确 validation error）；`cached_quota_signal` / `convert_cache_overview` 改经单一 `cache_window_read_snapshot` 压平匹配点（消灭两层匹配）；新增 `core_to_canonical_window/unit`、`cache_window_read_snapshot` 单测。
16. `tests/quota_overview.rs`：fixture 显式 `provider_id: mock`；新增 `missing_or_empty_provider_is_rejected_with_validation_error`（即使已注册 provider 也拒绝缺省）。

### G. §3.8 CLI typed 解析 / 渲染（W7，cli-command + cli-host）

17. `cli-command`：`UsageWindow`（clap `ValueEnum`，逗号分隔可重复）+ `UsageUnit`（`FromStr` 严格解析：token/count 大小写不敏感、`cost:<3位ASCII字母>`，其余在解析边界拒绝）；`usage_window_is_typed_and_rejects_invalid_values` / `usage_unit_is_strict_and_rejects_invalid_values`。
18. `cli-host`：`usage_query_from_args` 直接映射 typed 值（删除 `parse_window` / `parse_unit` 静默回退）；`render_usage_text` 先 `serde_json::from_value::<QuotaOverviewView>` 再按 typed 字段渲染（删除手工 `Value::get` 复制 core-api schema 的整段逻辑）。

### H. §6 P0 状态语义与验证规则同步（W8，文档）

19. Phase 14 行标注「库级收口、生产接线待 P18」与 P14-10 完成；TargetVerified 判定遵循既有 plan-README / docs/quality/testing.md 的 L0/L1 规则、affected-crate 判断与 Full Gate 升级条件（用户既有 plan-README、testing.md、CI 改动不归为本轮产出）。
20. P14-1 计划依赖描述与 workspace-layout 的依赖文字已按当前真实依赖图同步（本轮完成）；core-api ↔ quota-service 镜像类型继续以 exhaustive conversion + schema tests 防漂移（本任务已补 kind/unit/window 穷尽映射测试）。

## 主要产出物

- **删除**：capability.rs（454 行）、oauth.rs（1142 行）、auth-service 依赖；`AlertSuggestion` / `Alert.suggestions` / `CacheWindowRead` / `CacheRead.from_cache` / `ExhaustionPrediction.uncertain` / `_measure_used`；OpenAI / xAI 双套错误归并状态机；三套时间算法与三套脱敏规则的私有复制。
- **新增**：`util.rs`（时间 + 脱敏单一事实源，时钟纯函数显式接收 `now`）；`error.rs::merge_dual_failures`（单一优先级表）；`core_api::QuotaAlertKind`（冻结 snake_case）+ `QuotaAlert.kind/source` + `QuotaFailureView.adapter_kind` 可空；cli-command `UsageWindow` / `UsageUnit` typed 解析。
- **接线小修**：Qwen provenance 基于实际 base；supervisor 完整 kind 映射 + source 二次脱敏；router 显式 provider 必填；cli-host typed 渲染；schema 4+2 文件重生成；protocol-test-gui 新增 quota-alert-roundtrip 场景。
- **测试**：新增/强化断言（固定时钟边界、脱敏契约、错误归并确定性、kind 穷尽映射、旧 JSON 重放兼容、CLI 边界拒绝、provider 必填、roundtrip 场景），9 crate 联合 452 passed。

## 验收标准（保留 REVIEW 追踪章节）

- [x] **§3.1 / §3.2**：capability matrix 与 OAuth 通用层全删（零生产命中），auth-service 依赖移除；能力事实源仅为 adapter `supports()` + docs 表格
- [x] **§3.3 / §3.5**：时间算法与脱敏各只有 `util.rs` 一份实现；`canonical_endpoint` / `http_util` / `refresh` / `SystemQuotaClock` 全部委托；固定时钟可测（月初 / 下月 1 号边界断言）；Qwen provenance 基于实际 base 且测试断言
- [x] **§3.4**：双端点错误归并单一实现，优先级表确定性——错误分类与 `retry_after` 判定与参数顺序无关，完全平局时 `status` 保留首参数；403 vs 429 取 Forbidden，Unauthorized 高于 ReauthorizationRequired
- [x] **§3.6**：cache 结果压平（无 `CacheWindowRead`、`CacheRead` 每变体恒值 `from_cache` 删除）；`ExhaustionPrediction.uncertain` / `_measure_used` 删除；scope 校验与查询级失败不再伪造 `AdapterKind`（`QuotaFailure::domain` + `adapter_kind: Option`），refresh 去重/恢复只接受真实归属失败
- **§3.7**：WebScrape 内部审计 Vec 合并 → 显式延后 P18-13（见 Deferred items），非本轮修复。
- [x] **§2.6**：`Alert.suggestions` 删除（不再存储可派生且被丢弃的数据）；core-api 稳定 `QuotaAlertKind` + `source` 透传（supervisor 二次脱敏），旧事件 JSON 重放兼容（kind/source 解码 None）；schema 与 protocol roundtrip 场景通过
- [x] **§2.4**：未指定 provider 不再静默选「第一个」或空默认 ID，缺省即明确 validation error；多 provider 聚合语义按评审保留给 P18 binding enumeration
- [x] **§3.8**：无效 window/unit 在 clap 边界拒绝；CLI 渲染不再经 `serde_json::Value` 手工读取字段（typed `QuotaOverviewView` 直接渲染）
- [x] **§6 P0**：Phase 14 标注「库级收口、生产接线待 P18」；遵循既有 plan-README / testing.md 的 L0/L1 规则与 Full Gate 升级条件（用户既有 plan-README、testing.md、CI 改动不归为本轮产出）
- [x] **定向验证**：9 crate 联合 `cargo test`（452 passed / 0 failed）/ `protocol-test-gui --self-test`（自测修复后连续 5 次 9/9 + 最终再 1 次仍 9/9）/ 9 成员 `cargo clippy --all-targets -- -D warnings`（0 warning）/ `cargo fmt --all -- --check`（PASS）/ `cargo run -p schema-typegen -- --check`（PASS）/ diff 与残留写集合 `rg` 核验（见验证记录）

### Deferred items（建议/跟踪，本任务不做）

- **§2.1 持久化 Ledger 与真实 attribution**：`InMemoryUsageLedger` 仍为每次 CLI 进程新建，`record_run_usage` 固定 `tenant=local` / `account=local/default` / `credential=None`。由 [P18-8](../plan/P18-8-usage-cost-ledger.md) 提供持久化 Ledger 并在启动时 replay；归属输入由 [P18-2](../plan/P18-2-tenant-principal.md) / [P18-3](../plan/P18-3-provider-account.md) / [P18-4](../plan/P18-4-credential-lease.md) 提供；quota-service 只消费已确定的 scope，不新建第二套账本或账号选择逻辑。
- **§2.2 生产 refresh / audit 生命周期**：六家 adapter factory 生产零调用，`RefreshScheduler` 无 target 注册 / 后台任务 / 持久 audit sink。复用现有 `QuotaRuntime` 作为 composition/lifecycle owner，在 P18 binding 可用后装配 adapter / resolver / target / scheduler / shutdown；审计所有权并入 [P18-13](../plan/P18-13-audit-otel.md)，provider registry 归属见 [P18-14](../plan/P18-14-pool-reconciliation.md)。
- **§2.3 / §2.5 GUI 实际投影**：GUI Protocol 已携带 `QuotaOverview` / `QuotaChanged` / `QuotaAlert`（协议先行），仓库尚无 quota projection / controller / page。随 [P19-2](../plan/P19-2-client-state-projection.md) 状态投影与 [P19-10](../plan/P19-10-provider-account-settings.md) 账号设置落地，只消费 Core projection，不用前端本地状态伪造未实现能力。
- **§3.7 WebScrape 内部审计 Vec 合并**：adapter 自带的有界内存 `ScrapeAuditEntry` Vec 仍保留（当前仅测试断言使用）；生产路径只保留 scheduler/控制面外部 audit sink 的接线并入 P18-13，避免第二份运行时审计记录。
- **§3.2 ApiKey 通用层内联决定**：`ApiKeyQuotaAdapter` / `ApiKeyQuotaEndpoint` 目前仍只有 Moonshot 一个生产消费者；P18 真实接线后若仍单消费者，则内联到 Moonshot 并删除第二层 endpoint trait。

### Reviewer 提出但判定为可接受的低优先项（不另立任务）

- **§2.2 建议「不要再新增 QuotaManager / 独立 daemon / 第二条 refresh control plane」**：本任务未新增任何 manager / daemon，composition 仍由 `QuotaRuntime` 负责，与建议一致。
- **§3.3 测试只能断言「未来且少于 32 天」**：`util.rs` 引入显式 `now` 参数后，新增固定边界断言（`next_month_start_lands_on_first_of_next_month_utc`、`month_start_is_first_day_of_current_month`），不再依赖墙钟窗口。
- **§2.5 GUI「协议先行」**：保持 `protocol-test-gui` 无 quota 页面场景前的协议层验证（本次补 roundtrip 场景），实际投影属 P19-2 / P19-10 范围。

## 验证记录（2026-08-11）

- `cargo test -p quota-service -p app-service -p core-api -p cli-command -p cli-host -p cli-renderer -p gui-protocol -p protocol-test-gui -p gui-server`：**452 passed / 0 failed**（9 crate 联合；覆盖删重复、压平、kind 穷尽映射、重放兼容、CLI 边界、provider 必填、错误归并顺序无关与平局首参数、source 保守脱敏、gui-server spawn 前 Hub receiver 回归、protocol-test-gui `subscribe_all_landed` 订阅落地屏障、quota-alert roundtrip 等全部新增断言）。
- `cargo run -p protocol-test-gui -- --self-test`：**自测修复后连续 5 次复跑 9/9 PASS，最终再复跑 1 次仍 9/9**（共 6 次，含新增 quota-alert-roundtrip 场景）。
- `cargo clippy`（9 成员：quota-service / core-api / app-service / cli-command / cli-host / gui-protocol / cli-renderer / protocol-test-gui / gui-server，`--all-targets -- -D warnings`）：**0 warning**。
- `cargo fmt --all -- --check`：PASS；`cargo run -p schema-typegen -- --check`：PASS（TypeScript declarations up to date，新增 `QuotaAlertKind` + `QuotaAlert.kind/source` + `QuotaFailureView.adapter_kind` 可空已重生成）；diff 核验：PASS（写集合收敛，未触碰无关源码）。
- 残留核验（rg）：已删符号 `capability_matrix` / `CredentialKindHint` / `OAuthQuotaAdapter` / `AlertSuggestion` / `CacheWindowRead` / `_measure_used` 在 crates/ apps/ 零生产命中；`from_cache` / `uncertain` 不作全局零命中声明——剩余命中仅为合法保留的 `QuotaOverviewView.from_cache` / `QuotaReset.uncertain`，删除的是 `CacheRead` 每变体恒值字段与 `ExhaustionPrediction.uncertain`；`redact_source` / `next_month_start_timestamp` / `merge_dual_failures` 各仅 `util.rs` / `error.rs` 一份实现；`quota_alert_kind_from` / `QuotaAlertKind` 现有 supervisor / core-api / schema / protocol-test-gui 消费点。
- 写集合核验（rg + diff）：已删符号零生产命中（见上），`quota-service` 净删约 1000 行（oauth 1142 + capability 454 删、util 收口净增），新增源码文件仅 `quota-service/src/util.rs`；gui-server 改动仅 `session.rs`（`spawn_forwarder` spawn 前同步建立 Hub receiver + 回归测试）；测试侧订阅落地屏障 `subscribe_all_landed` 在 `apps/protocol-test-gui/src/scenarios.rs`；未触碰其它无关源码。
- 独立 reviewer（deepseek reviewer，模型复核）最终结论：**VERDICT PASS**——4 类 findings 已全部处理，每类均附确定性门禁 / rg / diff 证据，无遗留；补记关闭，本节与 [docs/review/p14-review.md](../docs/review/p14-review.md) §7.5 均为最终结论。
- 门禁节奏：修复面跨 9 crate + schema，按实际 diff 使用多 `-p` 定向验证；未命中 Workspace Full Gate 升级条件（非功能簇收尾、非大规模跨 crate 重构、无 workspace/resolver/toolchain/关键依赖重大变化、非 canonical protocol 大范围变更——core-api 枚举为后加可选字段且冻结形态，关键消费者已覆盖），Full gate NOT RUN；三平台与发布门禁留待 Core 主干 L2/L3。

```text
Validation Level: L1
Affected crates: quota-service、app-service、core-api、cli-command、cli-host、cli-renderer、gui-protocol、protocol-test-gui、gui-server（changed + 关键直接消费者）
Validated: cargo test（9 crate，452 passed）/ protocol-test-gui --self-test（自测修复后连续 5 次 9/9 + 最终再 1 次）/ cargo clippy（9 成员，0 warning）/ cargo fmt --all -- --check / cargo run -p schema-typegen -- --check / diff 与 rg 残留写集合核验
Targeted regressions: 固定时钟边界、脱敏契约与 source 保守脱敏、错误归并顺序无关与平局首参数、kind 穷尽映射、旧 JSON 重放兼容、CLI 边界拒绝、provider 必填、cache 压平、失败归属可空、gui-server spawn 前 Hub receiver 回归、protocol-test-gui `subscribe_all_landed` 订阅落地屏障、quota-alert roundtrip 场景
Full workspace gate: NOT RUN（未命中升级条件）
```

**相关文档**：[REVIEW.md](../REVIEW.md) §P14 · [docs/review/p14-review.md](../docs/review/p14-review.md) · [usage-quota](../docs/features/usage-quota.md) · [ADR-033 控制面分离](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP Phase 14](../ROADMAP.md) · [plan/README](../plan/README.md) · [测试体系](../docs/quality/testing.md)
