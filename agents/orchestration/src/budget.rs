//! Worker 预算度量 / 账本 flush（P12-4）。
//!
//! 双层并发上限的实现位置：
//!
//! - **Agent 并发**：由 [`crate::AgentSupervisor`] 的活动 worker 计数 +
//!   `TenantPolicyEngine::check_agent_concurrency` 实现（见 supervisor.rs）；
//! - **请求 / Lease 并发**：由 `provider-control` 的 `CredentialPool` 实现。
//!
//! 两层互不读写对方状态（P12-4 验收标准："Agent 并发与 account request
//! concurrency 使用独立计数器/状态机"）。本模块只负责 token / cost 度量
//! 与 ledger flush。
//!
//! [`WorkerBudgetController`] 记录 token / cost 用量并输出软告警与硬超限报告；
//! 达标行为（pause/cancel/reassign/fallback）由调用方依据报告产生显式事件
//! （如 [`crate::OrchestrationEvent::BudgetExceeded`]），本模块只负责度量。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use pawork_domain::{AgentId, ModelId, PrincipalId, ProviderId, RunId, SessionId, TenantId};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;
use pawork_control_plane::{UsageLedger, UsageRecord, AUTO_RECORD_ID_PREFIX};

/// 为每个逻辑控制器分配进程内唯一 ID，隔离不同控制器的 flush 幂等键。
static NEXT_BUDGET_CONTROLLER_ID: AtomicU64 = AtomicU64::new(0);

/// Worker 预算上限；`None` 表示不限制。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerBudgetLimits {
    /// 输入 token 上限。
    #[serde(default)]
    pub max_input_tokens: Option<u64>,
    /// 输出 token 上限。
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    /// 成本上限（micros，1e-6 美元）。
    #[serde(default)]
    pub max_cost_micros: Option<u64>,
    /// 该 worker 允许的并发请求上限（由 lease 层执行；此处仅记录）。
    #[serde(default)]
    pub max_concurrency: Option<u64>,
}

/// 多维用量累加器（原子计数，无锁读）。
#[derive(Debug, Default)]
pub struct UsageAccumulator {
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
    cost_micros: AtomicU64,
}

impl Clone for UsageAccumulator {
    fn clone(&self) -> Self {
        Self {
            input_tokens: AtomicU64::new(self.input_tokens()),
            output_tokens: AtomicU64::new(self.output_tokens()),
            cost_micros: AtomicU64::new(self.cost_micros()),
        }
    }
}

impl UsageAccumulator {
    /// 新建空累加器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 累加 token 用量。
    pub fn add_tokens(&self, input: u64, output: u64) {
        self.input_tokens.fetch_add(input, Ordering::Relaxed);
        self.output_tokens.fetch_add(output, Ordering::Relaxed);
    }

    /// 累加成本（micros）。
    pub fn add_cost(&self, micros: u64) {
        self.cost_micros.fetch_add(micros, Ordering::Relaxed);
    }

    /// 已用输入 token。
    pub fn input_tokens(&self) -> u64 {
        self.input_tokens.load(Ordering::Relaxed)
    }

    /// 已用输出 token。
    pub fn output_tokens(&self) -> u64 {
        self.output_tokens.load(Ordering::Relaxed)
    }

    /// 已用成本（micros）。
    pub fn cost_micros(&self) -> u64 {
        self.cost_micros.load(Ordering::Relaxed)
    }
}

/// 预算检查报告。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetReport {
    /// 达到软阈值（≥ `soft_ratio × limit`）的维度。
    pub soft_warnings: BTreeSet<String>,
    /// 超过硬上限（> limit）的维度。
    pub hard_exceeded: BTreeSet<String>,
}

/// 维度名常量（与 [`crate::OrchestrationEvent::BudgetExceeded`] 的 `dimension` 一致）。
pub const DIM_INPUT_TOKENS: &str = "input_tokens";
/// 维度名常量。
pub const DIM_OUTPUT_TOKENS: &str = "output_tokens";
/// 维度名常量。
pub const DIM_COST_MICROS: &str = "cost_micros";

/// 默认软阈值比例。
pub const DEFAULT_SOFT_RATIO: f64 = 0.8;

/// 软阈值比例整数刻度（百万分之一）；决策比较使用，避免 f64 精度误差。
const SOFT_RATIO_SCALE: u64 = 1_000_000;
/// 默认软阈值比例（ppm，对应 `0.8`）。
const DEFAULT_SOFT_RATIO_PPM: u64 = 800_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct UsageSnapshot {
    input_tokens: u64,
    output_tokens: u64,
    cost_micros: u64,
}

impl UsageSnapshot {
    fn delta_since(self, previous: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_sub(previous.input_tokens),
            output_tokens: self.output_tokens.saturating_sub(previous.output_tokens),
            cost_micros: self.cost_micros.saturating_sub(previous.cost_micros),
        }
    }

    fn has_tokens(self) -> bool {
        self.input_tokens.saturating_add(self.output_tokens) > 0
    }

    /// 是否存在任何可记账的用量（token 或成本）。
    fn has_usage(self) -> bool {
        self.has_tokens() || self.cost_micros > 0
    }
}

#[derive(Clone, Debug)]
struct PendingFlush {
    target: UsageSnapshot,
    record: UsageRecord,
}

#[derive(Debug, Default)]
struct FlushState {
    last_committed: UsageSnapshot,
    pending: Option<PendingFlush>,
}

/// Worker 预算控制器：按维度记录用量并对照 [`WorkerBudgetLimits`] 出报告。
///
/// `Clone` 产生同一逻辑控制器的共享句柄：用量累加器与 flush 提交状态均共享，
/// 因此从任意 clone 发起的并发 flush 都经过同一序列化提交游标。
#[derive(Debug)]
pub struct WorkerBudgetController {
    limits: WorkerBudgetLimits,
    usage: Arc<UsageAccumulator>,
    soft_ratio_ppm: u64,
    controller_id: u64,
    flush_state: Arc<AsyncMutex<FlushState>>,
    /// 已发出 `BudgetExceeded` 的硬超限维度：持续超限去重，恢复后再告警。
    /// `Clone` 共享同一集合，保证多句柄去重一致。
    signaled_hard: Arc<Mutex<BTreeSet<String>>>,
}

impl Clone for WorkerBudgetController {
    fn clone(&self) -> Self {
        Self {
            limits: self.limits.clone(),
            usage: Arc::clone(&self.usage),
            soft_ratio_ppm: self.soft_ratio_ppm,
            controller_id: self.controller_id,
            flush_state: Arc::clone(&self.flush_state),
            signaled_hard: Arc::clone(&self.signaled_hard),
        }
    }
}

impl WorkerBudgetController {
    /// 以指定上限构造控制器（软阈值默认 `0.8`）。
    pub fn new(limits: WorkerBudgetLimits) -> Self {
        Self {
            limits,
            usage: Arc::new(UsageAccumulator::new()),
            soft_ratio_ppm: DEFAULT_SOFT_RATIO_PPM,
            controller_id: NEXT_BUDGET_CONTROLLER_ID.fetch_add(1, Ordering::Relaxed),
            flush_state: Arc::new(AsyncMutex::new(FlushState::default())),
            signaled_hard: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    /// 覆盖软阈值比例（`0.0..=1.0`）。
    pub fn with_soft_ratio(mut self, ratio: f64) -> Self {
        let ratio = ratio.clamp(0.0, 1.0);
        self.soft_ratio_ppm = (ratio * SOFT_RATIO_SCALE as f64).round() as u64;
        self
    }

    /// 记录 token 用量。
    pub fn record_tokens(&self, input: u64, output: u64) {
        self.usage.add_tokens(input, output);
    }

    /// 记录成本（micros）。
    pub fn record_cost(&self, micros: u64) {
        self.usage.add_cost(micros);
    }

    /// 当前用量快照。
    pub fn usage(&self) -> (u64, u64, u64) {
        (
            self.usage.input_tokens(),
            self.usage.output_tokens(),
            self.usage.cost_micros(),
        )
    }

    /// 当前上限配置。
    pub fn limits(&self) -> &WorkerBudgetLimits {
        &self.limits
    }

    /// 对照上限出报告：软告警（`used >= ratio × limit`）与硬超限（`used > limit`）。
    pub fn check(&self) -> BudgetReport {
        let mut report = BudgetReport::default();
        let input = self.usage.input_tokens();
        let output = self.usage.output_tokens();
        let cost = self.usage.cost_micros();
        for (used, limit, dim) in [
            (input, self.limits.max_input_tokens, DIM_INPUT_TOKENS),
            (output, self.limits.max_output_tokens, DIM_OUTPUT_TOKENS),
            (cost, self.limits.max_cost_micros, DIM_COST_MICROS),
        ] {
            let Some(limit) = limit else { continue };
            if used >= limit {
                report.hard_exceeded.insert(dim.to_string());
            }
            // 整数比较 `used/limit >= soft_ratio`：等价于
            // `used * SCALE >= limit * soft_ratio_ppm`，用 u128 中间值避免 u64 溢出，
            // 全程无 f64 精度误差。
            let used_scaled = used as u128 * SOFT_RATIO_SCALE as u128;
            let limit_scaled = limit as u128 * self.soft_ratio_ppm as u128;
            if used_scaled >= limit_scaled {
                report.soft_warnings.insert(dim.to_string());
            }
        }
        report
    }

    /// 返回本次报告中「新进入硬超限且尚未发出过 `BudgetExceeded`」的维度，
    /// 并把已恢复（不再硬超限）的维度从内部记忆中移除。
    ///
    /// 语义：
    /// - 同一维度持续超限只返回一次（调用方据此去重发事件）；
    /// - 用量回落到上限以下后该维度被「忘记」，下次再次超限会重新返回
    ///   （恢复后可再告警）。
    ///
    /// `Clone` 句柄共享同一记忆集合，去重跨 clone 一致。本方法只读取
    /// `report`，不重新计算；调用方负责先 `check()` 再传入。
    pub fn diff_hard_exceeded(&self, report: &BudgetReport) -> BTreeSet<String> {
        let mut signaled = self
            .signaled_hard
            .lock()
            .expect("signaled_hard mutex poisoned");
        // 恢复：此前发过但现在不再硬超限的维度 → 移出记忆。
        let recovered: Vec<String> = signaled
            .iter()
            .filter(|dim| !report.hard_exceeded.contains(*dim))
            .cloned()
            .collect();
        for dim in &recovered {
            signaled.remove(dim);
        }
        // 新增：当前硬超限但未发过的维度 → 记录并返回。
        let newly: BTreeSet<String> = report
            .hard_exceeded
            .iter()
            .filter(|dim| !signaled.contains(*dim))
            .cloned()
            .collect();
        for dim in &newly {
            signaled.insert(dim.clone());
        }
        newly
    }

    /// 把尚未成功提交的增量用量写入注入的 usage ledger（归属信息由 `ctx` 提供）。
    ///
    /// flush 由 async mutex 序列化（不持有 `std::sync::Mutex` 跨 await）。每条
    /// record 的内容为 `target - last_committed`，ID 包含逻辑控制器 ID 与目标
    /// totals。record 在 await 前保存为 pending；ledger 返回 `Ok`（包括幂等
    /// 重放成功）后才推进 `last_committed`，错误或取消会保留完全相同的 ID、
    /// delta 与 `occurred_at_ms` 供下次重试。无新增 token 且无新增成本时
    /// 为空操作；cost-only 增量（token 为 0、成本大于 0）单独成条提交。
    pub async fn flush_to_ledger(
        &self,
        ledger: &dyn UsageLedger,
        ctx: &LedgerContext,
    ) -> Result<(), pawork_control_plane::UsageLedgerError> {
        let mut state = self.flush_state.lock().await;
        let goal = self.usage_snapshot();

        loop {
            if state.pending.is_none() {
                let delta = goal.delta_since(state.last_committed);
                if !delta.has_usage() {
                    return Ok(());
                }
                state.pending = Some(PendingFlush {
                    target: goal,
                    record: self.make_usage_record(ctx, goal, delta),
                });
            }

            let pending = state
                .pending
                .as_ref()
                .expect("pending flush was initialized")
                .clone();
            ledger.record(pending.record).await?;

            state.last_committed = pending.target;
            state.pending = None;

            // 若本次调用开始时已有失败/取消留下的旧 pending，先重放该 record，
            // 再继续提交调用开始时观察到的较新目标快照。
            if state.last_committed == goal {
                return Ok(());
            }
        }
    }

    fn usage_snapshot(&self) -> UsageSnapshot {
        UsageSnapshot {
            input_tokens: self.usage.input_tokens(),
            output_tokens: self.usage.output_tokens(),
            cost_micros: self.usage.cost_micros(),
        }
    }

    fn make_usage_record(
        &self,
        ctx: &LedgerContext,
        target: UsageSnapshot,
        delta: UsageSnapshot,
    ) -> UsageRecord {
        UsageRecord {
            record_id: format!(
                "{AUTO_RECORD_ID_PREFIX}budget-{}-{}-{}-{}",
                self.controller_id, target.input_tokens, target.output_tokens, target.cost_micros,
            ),
            tenant_id: ctx.tenant_id.clone(),
            principal_id: ctx.principal_id.clone(),
            account_id: ctx.account_id.clone(),
            credential_id: ctx.credential_id.clone(),
            session_id: ctx.session_id.clone(),
            agent_id: ctx.agent_id.clone(),
            run_id: ctx.run_id.clone(),
            provider_id: ctx.provider_id.clone(),
            model_id: ctx.model_id.clone(),
            input_tokens: delta.input_tokens,
            output_tokens: delta.output_tokens,
            // P14-7 审查（cache 通路）：本地预算度量当前只跟踪 input/output/cost，
            // 不贯通 cache_read/cache_write token。贯通需要 usage-ledger 的
            // UsageRecord（已含字段）+ 本快照 + 累加器 + record_usage 签名四处协同
            // 扩展，且 provider-control 侧需提供 cache 维度来源；此处明确停止，
            // 不在未贯通的情况下写入 0 之外的值，避免误导对账。完整贯通单独排期。
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_micros: delta.cost_micros,
            currency: "USD".to_string(),
            occurred_at_ms: now_ms(),
            // P18-8 v2：其余字段（version=2、trace/pricing 快照）保持默认。
            ..UsageRecord::default()
        }
    }
}

/// flush 到账本所需的归属上下文。
#[derive(Clone, Debug)]
pub struct LedgerContext {
    /// 关联的 opaque credential 标识（可选；不写入日志）。
    pub credential_id: Option<String>,
    /// 租户。
    pub tenant_id: TenantId,
    /// 主体。
    pub principal_id: PrincipalId,
    /// 账号。
    pub account_id: String,
    /// 会话。
    pub session_id: SessionId,
    /// agent。
    pub agent_id: AgentId,
    /// run（可选）。
    pub run_id: Option<RunId>,
    /// provider。
    pub provider_id: ProviderId,
    /// 模型。
    pub model_id: ModelId,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};
    use pawork_control_plane::{InMemoryUsageLedger, UsageLedgerError, UsageQuery, UsageTotals};

    fn limits() -> WorkerBudgetLimits {
        WorkerBudgetLimits {
            max_input_tokens: Some(100),
            max_output_tokens: Some(100),
            max_cost_micros: Some(100),
            max_concurrency: None,
        }
    }

    fn ctx() -> LedgerContext {
        LedgerContext {
            credential_id: Some("credential-1".to_string()),
            tenant_id: TenantId::new("tenant-a"),
            principal_id: PrincipalId::new("principal-1"),
            account_id: "account-1".to_string(),
            session_id: SessionId::new("session-1"),
            agent_id: AgentId::new("agent-1"),
            run_id: Some(RunId::new("run-1")),
            provider_id: ProviderId::new("local"),
            model_id: ModelId::new("mock-model"),
        }
    }

    /// 可模拟“账本已写入但调用方收到错误”的测试账本，并记录实际 record 调用。
    #[derive(Debug, Default)]
    struct TestLedger {
        inner: InMemoryUsageLedger,
        attempts: AtomicUsize,
        attempted_records: Mutex<Vec<UsageRecord>>,
        fail_after_first_record: bool,
        yield_before_record: bool,
    }

    impl TestLedger {
        fn fail_after_first_record() -> Self {
            Self {
                fail_after_first_record: true,
                ..Self::default()
            }
        }

        fn yielding() -> Self {
            Self {
                yield_before_record: true,
                ..Self::default()
            }
        }

        fn attempts(&self) -> usize {
            self.attempts.load(Ordering::SeqCst)
        }

        fn attempted_records(&self) -> Vec<UsageRecord> {
            self.attempted_records
                .lock()
                .expect("attempted records mutex poisoned")
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl UsageLedger for TestLedger {
        async fn record(&self, record: UsageRecord) -> Result<(), UsageLedgerError> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            {
                let mut attempted_records = self
                    .attempted_records
                    .lock()
                    .expect("attempted records mutex poisoned");
                attempted_records.push(record.clone());
            }
            if self.yield_before_record {
                tokio::task::yield_now().await;
            }
            self.inner.record(record).await?;
            if self.fail_after_first_record && attempt == 0 {
                return Err(UsageLedgerError::InvalidRecord {
                    reason: "simulated lost acknowledgement".to_string(),
                });
            }
            Ok(())
        }

        async fn query(&self, query: &UsageQuery) -> Result<Vec<UsageRecord>, UsageLedgerError> {
            Ok(self.inner.query(query).await?)
        }

        async fn aggregate(&self, query: &UsageQuery) -> Result<UsageTotals, UsageLedgerError> {
            self.inner.aggregate(query).await
        }
    }

    /// 模拟“账本已持久化但确认丢失且调用被挂起”：record 先写入 inner，
    /// 再阻塞等待放行，用于 mid-await abort/drop 场景。
    #[derive(Debug)]
    struct BlockingLedger {
        inner: InMemoryUsageLedger,
        attempts: AtomicUsize,
        attempted_records: Mutex<Vec<UsageRecord>>,
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    impl BlockingLedger {
        fn new() -> Self {
            Self {
                inner: InMemoryUsageLedger::new(),
                attempts: AtomicUsize::new(0),
                attempted_records: Mutex::new(Vec::new()),
                started: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            }
        }

        fn attempts(&self) -> usize {
            self.attempts.load(Ordering::SeqCst)
        }

        fn attempted_records(&self) -> Vec<UsageRecord> {
            self.attempted_records
                .lock()
                .expect("attempted records mutex poisoned")
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl UsageLedger for BlockingLedger {
        async fn record(&self, record: UsageRecord) -> Result<(), UsageLedgerError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            self.attempted_records
                .lock()
                .expect("attempted records mutex poisoned")
                .push(record.clone());
            self.inner.record(record).await?;
            self.started.notify_one();
            self.release.notified().await;
            Ok(())
        }

        async fn query(&self, query: &UsageQuery) -> Result<Vec<UsageRecord>, UsageLedgerError> {
            Ok(self.inner.query(query).await?)
        }

        async fn aggregate(&self, query: &UsageQuery) -> Result<UsageTotals, UsageLedgerError> {
            self.inner.aggregate(query).await
        }
    }

    #[test]
    fn budget_exceeded_records_event_dimensions() {
        let controller = WorkerBudgetController::new(limits());

        // 低于软阈值：无告警。
        controller.record_tokens(10, 10);
        controller.record_cost(10);
        let report = controller.check();
        assert!(report.soft_warnings.is_empty());
        assert!(report.hard_exceeded.is_empty());

        // 越过软阈值（100 × 0.8 = 80）：三个维度都进软告警。
        controller.record_tokens(80, 70);
        controller.record_cost(70);
        let report = controller.check();
        assert_eq!(
            report.soft_warnings,
            BTreeSet::from([
                DIM_INPUT_TOKENS.to_string(),
                DIM_OUTPUT_TOKENS.to_string(),
                DIM_COST_MICROS.to_string(),
            ])
        );
        assert!(report.hard_exceeded.is_empty());

        // 越过硬上限：input 110 > 100、cost 120 > 100；output 90 仅软告警。
        controller.record_tokens(20, 10);
        controller.record_cost(40);
        let report = controller.check();
        assert_eq!(
            report.hard_exceeded,
            BTreeSet::from([DIM_INPUT_TOKENS.to_string(), DIM_COST_MICROS.to_string()])
        );
        assert!(report.soft_warnings.contains(DIM_INPUT_TOKENS));
        assert!(report.soft_warnings.contains(DIM_OUTPUT_TOKENS));
        assert!(report.soft_warnings.contains(DIM_COST_MICROS));

        // 维度名与 OrchestrationEvent::BudgetExceeded 的 dimension 字段一致。
        let event = crate::OrchestrationEvent::BudgetExceeded {
            agent_id: AgentId::new("agent-1"),
            dimension: DIM_INPUT_TOKENS.to_string(),
            used: 110,
            limit: 100,
        };
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["data"]["dimension"], "input_tokens");
    }

    #[tokio::test]
    async fn flush_to_ledger_writes_attributed_record() {
        let ledger = Arc::new(InMemoryUsageLedger::new());
        let controller = WorkerBudgetController::new(limits());
        controller.record_tokens(30, 20);
        controller.record_cost(5_000);
        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();

        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].agent_id, AgentId::new("agent-1"));
        assert_eq!(records[0].tenant_id, TenantId::new("tenant-a"));
        assert_eq!(records[0].input_tokens, 30);
        assert_eq!(records[0].output_tokens, 20);
        assert_eq!(records[0].cost_micros, 5_000);
        assert_eq!(records[0].model_id, ModelId::new("mock-model"));
        assert!(
            records[0].record_id.starts_with(AUTO_RECORD_ID_PREFIX),
            "flush 幂等键必须使用账本保留前缀"
        );
        assert!(
            records[0].occurred_at_ms > 0,
            "occurred_at 必须为有效时间戳"
        );
        assert_eq!(
            records[0].credential_id.as_deref(),
            Some("credential-1"),
            "credential_id 必须从 LedgerContext 写入记录"
        );
    }

    #[tokio::test]
    async fn flush_cost_only_writes_cost_record() {
        let ledger = Arc::new(InMemoryUsageLedger::new());
        let controller = WorkerBudgetController::new(limits());
        controller.record_cost(10);
        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();

        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1, "cost-only 增量必须单独成条提交");
        assert_eq!(records[0].input_tokens, 0);
        assert_eq!(records[0].output_tokens, 0);
        assert_eq!(records[0].cost_micros, 10);
        let totals = ledger.aggregate(&UsageQuery::default()).await.unwrap();
        assert_eq!(totals.cost_micros, 10);

        // 无新增用量时 flush 仍是空操作。
        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();
        assert_eq!(ledger.query(&UsageQuery::default()).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn flush_replay_same_snapshot_is_idempotent() {
        let ledger = Arc::new(InMemoryUsageLedger::new());
        let controller = WorkerBudgetController::new(limits());
        controller.record_tokens(30, 20);
        controller.record_cost(5_000);

        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();
        let first = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(first.len(), 1);

        // 同一控制器、同一快照重放：稳定 record_id/occurred_at，不重复记账。
        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1, "同一快照重放不得重复写入");
        assert_eq!(
            records[0].record_id, first[0].record_id,
            "重放必须复用同一幂等键"
        );
        assert_eq!(
            records[0].occurred_at_ms, first[0].occurred_at_ms,
            "重放必须复用同一 occurred_at"
        );
        assert_eq!(records[0].input_tokens, 30);
        assert_eq!(records[0].cost_micros, 5_000);
    }

    #[tokio::test]
    async fn flush_cumulative_ten_to_twenty_aggregates_to_twenty() {
        let ledger = Arc::new(InMemoryUsageLedger::new());
        let controller = WorkerBudgetController::new(limits());
        controller.record_tokens(10, 0);
        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();

        // 累计目标从 10 增长到 20，第二条 record 只提交增量 10。
        controller.record_tokens(10, 0);
        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();

        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 2, "快照变化应落新记录");
        assert_ne!(
            records[0].record_id, records[1].record_id,
            "快照变化必须生成新幂等键"
        );
        assert_eq!(records[0].input_tokens, 10);
        assert_eq!(records[1].input_tokens, 10, "第二条记录必须仅包含增量");
        let totals = ledger.aggregate(&UsageQuery::default()).await.unwrap();
        assert_eq!(totals.input_tokens, 20, "累计快照不得重复计入");

        // 新快照重放仍不重复。
        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();
        assert_eq!(ledger.query(&UsageQuery::default()).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn flush_error_retries_same_record_before_advancing_state() {
        let ledger = Arc::new(TestLedger::fail_after_first_record());
        let controller = WorkerBudgetController::new(limits());
        controller.record_tokens(10, 0);

        // 模拟账本已持久化，但确认丢失：控制器收到 Err，不得推进提交游标。
        let first = controller.flush_to_ledger(ledger.as_ref(), &ctx()).await;
        assert!(first.is_err());
        assert_eq!(ledger.query(&UsageQuery::default()).await.unwrap().len(), 1);

        // 重试必须重放完全相同的 ID、delta 和 occurred_at；账本幂等返回成功后推进。
        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();
        let attempted = ledger.attempted_records();
        assert_eq!(attempted.len(), 2);
        assert_eq!(attempted[0], attempted[1], "错误重试必须重放同一 record");
        assert_eq!(ledger.query(&UsageQuery::default()).await.unwrap().len(), 1);

        // 已成功推进到目标 10，再次 flush 是本地 no-op，不再调用账本。
        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();
        assert_eq!(ledger.attempts(), 2);
        let totals = ledger.aggregate(&UsageQuery::default()).await.unwrap();
        assert_eq!(totals.input_tokens, 10);
    }

    #[tokio::test]
    async fn flush_after_failed_pending_with_growth_aggregates_both_deltas() {
        let ledger = Arc::new(TestLedger::fail_after_first_record());
        let controller = WorkerBudgetController::new(limits());
        controller.record_tokens(10, 0);
        controller.record_cost(100);

        // 第一次 flush 模拟确认丢失：账本已写入第一条 delta，控制器保留 pending。
        let first = controller.flush_to_ledger(ledger.as_ref(), &ctx()).await;
        assert!(first.is_err());
        assert_eq!(ledger.query(&UsageQuery::default()).await.unwrap().len(), 1);

        // pending 未提交期间用量继续增长。
        controller.record_tokens(10, 0);
        controller.record_cost(50);

        // 重试：先重放原 pending（同一 record），再提交增长增量。
        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();
        let attempted = ledger.attempted_records();
        assert_eq!(attempted.len(), 3, "失败重试 + 增长增量共三次 ledger 调用");
        assert_eq!(
            attempted[0], attempted[1],
            "重试必须重放同一 pending record"
        );
        assert_eq!(attempted[0].input_tokens, 10);
        assert_eq!(attempted[0].cost_micros, 100);
        assert_eq!(attempted[2].input_tokens, 10, "增长增量只含新 delta");
        assert_eq!(attempted[2].cost_micros, 50);
        assert_ne!(
            attempted[0].record_id, attempted[2].record_id,
            "快照变化必须生成新幂等键"
        );

        // 两条 delta 聚合到新总量，且不重复记账。
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 2, "幂等重放不得重复写入");
        let totals = ledger.aggregate(&UsageQuery::default()).await.unwrap();
        assert_eq!(totals.input_tokens, 20);
        assert_eq!(totals.cost_micros, 150);
    }

    #[tokio::test]
    async fn flush_aborted_mid_await_replays_same_record_without_duplication() {
        let ledger = Arc::new(BlockingLedger::new());
        let controller = WorkerBudgetController::new(limits());
        controller.record_tokens(10, 0);
        let context = ctx();

        // 发起 flush：record 已写入账本但尚未返回（持久化后确认丢失）。
        let handle = tokio::spawn({
            let ledger = Arc::clone(&ledger);
            let controller = controller.clone();
            let context = context.clone();
            async move { controller.flush_to_ledger(ledger.as_ref(), &context).await }
        });
        ledger.started.notified().await;
        assert_eq!(ledger.attempts(), 1);
        assert_eq!(ledger.query(&UsageQuery::default()).await.unwrap().len(), 1);

        // mid-await 中止：pending 保留原 record_id/occurred_at，游标不推进。
        handle.abort();
        let _ = handle.await;
        ledger.release.notify_one();

        // 重试：重放完全相同的 pending record，幂等不重复写入。
        controller
            .flush_to_ledger(ledger.as_ref(), &context)
            .await
            .unwrap();
        let attempted = ledger.attempted_records();
        assert_eq!(attempted.len(), 2, "abort 后重试应再次调用 ledger");
        assert_eq!(
            attempted[0], attempted[1],
            "abort 后重试必须重放同一 record"
        );
        assert_eq!(
            attempted[0].record_id, attempted[1].record_id,
            "record_id 必须与 abort 前一致"
        );
        assert_eq!(
            attempted[0].occurred_at_ms, attempted[1].occurred_at_ms,
            "occurred_at 必须与 abort 前一致"
        );
        assert_eq!(
            ledger.query(&UsageQuery::default()).await.unwrap().len(),
            1,
            "abort 后重放不得重复记账"
        );
        let totals = ledger.aggregate(&UsageQuery::default()).await.unwrap();
        assert_eq!(totals.input_tokens, 10);
    }

    #[tokio::test]
    async fn concurrent_flushes_share_serialized_state_and_do_not_duplicate() {
        let ledger = Arc::new(TestLedger::yielding());
        let controller = WorkerBudgetController::new(limits());
        let cloned_handle = controller.clone();
        let context = ctx();
        controller.record_tokens(10, 0);

        // clone 是同一逻辑控制器的共享句柄；yield 强制另一 flush 有机会并发轮询。
        let (first, second) = tokio::join!(
            controller.flush_to_ledger(ledger.as_ref(), &context),
            cloned_handle.flush_to_ledger(ledger.as_ref(), &context),
        );
        first.unwrap();
        second.unwrap();

        assert_eq!(ledger.attempts(), 1, "并发 flush 只能发起一次 ledger 写入");
        assert_eq!(ledger.query(&UsageQuery::default()).await.unwrap().len(), 1);
        let totals = ledger.aggregate(&UsageQuery::default()).await.unwrap();
        assert_eq!(totals.input_tokens, 10);
    }
}
