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
use std::time::{SystemTime, UNIX_EPOCH};

use agent_domain::{AgentId, ModelId, PrincipalId, ProviderId, RunId, SessionId, TenantId};
use serde::{Deserialize, Serialize};
use usage_ledger::{UsageLedger, UsageRecord};

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

/// Worker 预算控制器：按维度记录用量并对照 [`WorkerBudgetLimits`] 出报告。
#[derive(Clone, Debug)]
pub struct WorkerBudgetController {
    limits: WorkerBudgetLimits,
    usage: UsageAccumulator,
    soft_ratio: f64,
}

impl WorkerBudgetController {
    /// 以指定上限构造控制器（软阈值默认 `0.8`）。
    pub fn new(limits: WorkerBudgetLimits) -> Self {
        Self {
            limits,
            usage: UsageAccumulator::new(),
            soft_ratio: DEFAULT_SOFT_RATIO,
        }
    }

    /// 覆盖软阈值比例（`0.0..=1.0`）。
    pub fn with_soft_ratio(mut self, ratio: f64) -> Self {
        self.soft_ratio = ratio.clamp(0.0, 1.0);
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
            if used > limit {
                report.hard_exceeded.insert(dim.to_string());
            }
            if used as f64 >= limit as f64 * self.soft_ratio {
                report.soft_warnings.insert(dim.to_string());
            }
        }
        report
    }

    /// 把累计用量写入注入的 usage ledger（归属信息由 `ctx` 提供）。
    ///
    /// 无 token 用量时为空操作（账本拒绝零 token 记录）。不重置累加器，
    /// 是否重置由调用方决定。
    pub async fn flush_to_ledger(
        &self,
        ledger: &dyn UsageLedger,
        ctx: &LedgerContext,
    ) -> Result<(), usage_ledger::UsageLedgerError> {
        let input = self.usage.input_tokens();
        let output = self.usage.output_tokens();
        if input.saturating_add(output) == 0 {
            return Ok(());
        }
        let record = UsageRecord {
            record_id: String::new(),
            tenant_id: ctx.tenant_id.clone(),
            principal_id: ctx.principal_id.clone(),
            account_id: ctx.account_id.clone(),
            session_id: ctx.session_id.clone(),
            agent_id: ctx.agent_id.clone(),
            run_id: ctx.run_id.clone(),
            provider_id: ctx.provider_id.clone(),
            model_id: ctx.model_id.clone(),
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_micros: self.usage.cost_micros(),
            currency: "USD".to_string(),
            occurred_at_ms: now_ms(),
        };
        ledger.record(record).await
    }
}

/// flush 到账本所需的归属上下文。
#[derive(Clone, Debug)]
pub struct LedgerContext {
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
    use std::sync::Arc;
    use usage_ledger::{InMemoryUsageLedger, UsageQuery};

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

        let records = ledger.query(&UsageQuery::default()).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].agent_id, AgentId::new("agent-1"));
        assert_eq!(records[0].tenant_id, TenantId::new("tenant-a"));
        assert_eq!(records[0].input_tokens, 30);
        assert_eq!(records[0].output_tokens, 20);
        assert_eq!(records[0].cost_micros, 5_000);
        assert_eq!(records[0].model_id, ModelId::new("mock-model"));
    }

    #[tokio::test]
    async fn flush_without_tokens_is_noop() {
        let ledger = Arc::new(InMemoryUsageLedger::new());
        let controller = WorkerBudgetController::new(limits());
        controller.record_cost(10);
        controller
            .flush_to_ledger(ledger.as_ref(), &ctx())
            .await
            .unwrap();
        assert!(ledger.query(&UsageQuery::default()).await.is_empty());
    }
}
