//! 账号 / 凭据健康状态机、scope-aware Cooldown 与 Circuit Breaker（P18-5）。
//!
//! 把 transport retry、credential/account failover 与 protocol fallback 分开，
//! 使失败只影响正确 scope：`ClientCancelled`、`InvalidRequest`、
//! `ContextTooLarge`、`ProtocolIncompatible` 不触发账号轮换或健康惩罚；
//! `Cancelled` 不降低账号健康度。
//!
//! - [`HealthState`]：Healthy / Degraded / CoolingDown / BillingBlocked / Disabled；
//! - [`CooldownTracker`] + [`CooldownKey`]：scope-aware Retry-After（无
//!   Retry-After 时用有界指数退避）；
//! - [`CircuitBreaker`]：bounded retry——连续失败阈值跳闸、half-open 探针、
//!   连续成功复原；
//! - [`HealthRuntime`]：按失败上下文聚合上述机制，并实现 401 refresh-once。
//!
//! 本模块不接触 Secret：所有键使用 opaque id，不记录 / 不持久化错误原文。

use std::collections::HashMap;
use std::sync::Arc;

use agent_domain::{AccountId, CredentialId, ModelId, ProviderId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::account::Clock;
use crate::classifier::{
    FailureClass, FailureClassification, FailureScope, HealthImpact, Retryability,
};

/// 健康状态机（P18-5）。
///
/// 持久化字符串冻结（snake_case，与 `app-database` 控制面 schema 对齐）；
/// 未知值 fail-closed 返回 `None`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// 正常：参与路由与 lease。
    #[default]
    Healthy,
    /// 软降级：有连续失败但未达摘除阈值 / 正在按策略观察。
    Degraded,
    /// 冷却中：尊重 Retry-After 或退避，到期自动恢复。
    CoolingDown,
    /// 计费 / 账号封禁（402 / account blocked）：等待人工或周期恢复。
    BillingBlocked,
    /// 人工禁用（与 [`crate::account::AccountState::Disabled`] 对齐）。
    Disabled,
}

impl HealthState {
    /// 是否参与路由过滤（`BillingBlocked` / `Disabled` 一律排除；
    /// `CoolingDown` 由 cooldown 截止时间决定，不在此判定）。
    pub fn is_admissible(self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }

    /// 冻结的持久化字符串。
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::CoolingDown => "cooling_down",
            Self::BillingBlocked => "billing_blocked",
            Self::Disabled => "disabled",
        }
    }

    /// 由持久化字符串反解；未知值返回 `None`（fail-closed）。
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "healthy" => Some(Self::Healthy),
            "degraded" => Some(Self::Degraded),
            "cooling_down" => Some(Self::CoolingDown),
            "billing_blocked" => Some(Self::BillingBlocked),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

/// 冷却键：失败作用域 + 实体（opaque id，绝不含明文）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CooldownKey {
    /// 惩罚作用域（账号 / 凭据 / 模型 / Provider）。
    pub scope: FailureScope,
    /// 实体标识（account / credential / model / provider 的 opaque id）。
    pub entity: String,
}

impl CooldownKey {
    /// 账号作用域冷却键。
    pub fn account(id: impl AsRef<str>) -> Self {
        Self {
            scope: FailureScope::Account,
            entity: id.as_ref().to_string(),
        }
    }

    /// 凭据作用域冷却键。
    pub fn credential(id: impl AsRef<str>) -> Self {
        Self {
            scope: FailureScope::Credential,
            entity: id.as_ref().to_string(),
        }
    }

    /// 模型作用域冷却键。
    pub fn model(id: impl AsRef<str>) -> Self {
        Self {
            scope: FailureScope::Model,
            entity: id.as_ref().to_string(),
        }
    }

    /// Provider 作用域冷却键。
    pub fn provider(id: impl AsRef<str>) -> Self {
        Self {
            scope: FailureScope::Provider,
            entity: id.as_ref().to_string(),
        }
    }
}

/// 有界指数退避（cooldown 默认等待）：`base × 2^attempt`，封顶 `cap`，
/// 可选确定性抖动（attempt 派生，无 RNG，测试稳定）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackoffPolicy {
    /// 基础等待（毫秒）。
    pub base_ms: u64,
    /// 等待上限（毫秒）。
    pub cap_ms: u64,
    /// 最大退避档位（attempt 超出后按上限封顶）。
    pub max_attempts: u32,
    /// 抖动幅度（百分比 0..=100；0 = 无抖动）。
    pub jitter_ratio_pct: u32,
}

impl BackoffPolicy {
    /// 默认策略：200ms 起、30s 封顶、8 档、无抖动。
    pub const DEFAULT_BASE_MS: u64 = 200;
    pub const DEFAULT_CAP_MS: u64 = 30_000;
    pub const DEFAULT_MAX_ATTEMPTS: u32 = 8;

    /// 以给定参数构造。
    pub const fn new(base_ms: u64, cap_ms: u64, max_attempts: u32) -> Self {
        Self {
            base_ms,
            cap_ms,
            max_attempts,
            jitter_ratio_pct: 0,
        }
    }

    /// 设置抖动百分比（0..=100）。
    pub fn with_jitter(mut self, jitter_ratio_pct: u32) -> Self {
        self.jitter_ratio_pct = jitter_ratio_pct.min(100);
        self
    }

    /// 第 `attempt` 次失败的建议等待（毫秒），有界、确定性。
    pub fn delay_ms(&self, attempt: u32) -> u64 {
        let exponent = attempt.min(self.max_attempts.saturating_sub(1)).min(20);
        let raw = self
            .base_ms
            .saturating_mul(1u64 << exponent)
            .min(self.cap_ms);
        if self.jitter_ratio_pct == 0 || raw == 0 {
            return raw;
        }
        // 确定性伪抖动：attempt 派生，测试稳定；幅度 ±jitter_ratio_pct%。
        let hash = attempt.wrapping_mul(2_654_435_761) ^ 0x9E37_79B9;
        let magnitude = (raw as u128 * self.jitter_ratio_pct as u128 / 100) as u64;
        let magnitude = magnitude.min(raw / 2);
        if hash % 2 == 0 {
            raw.saturating_add(magnitude).min(self.cap_ms)
        } else {
            raw.saturating_sub(magnitude).max(1)
        }
    }
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self::new(
            Self::DEFAULT_BASE_MS,
            Self::DEFAULT_CAP_MS,
            Self::DEFAULT_MAX_ATTEMPTS,
        )
    }
}

/// scope-aware 冷却跟踪器：尊重 Retry-After，无 Retry-After 时用退避。
#[derive(Clone, Debug, Default)]
pub struct CooldownTracker {
    deadlines: HashMap<CooldownKey, u64>,
}

impl CooldownTracker {
    /// 记录冷却：等待 = Retry-After（若有）否则 `delay_ms`。
    pub fn cool(
        &mut self,
        key: CooldownKey,
        retry_after_ms: Option<u64>,
        now_ms: u64,
        delay_ms: u64,
    ) {
        let wait = retry_after_ms.unwrap_or(delay_ms);
        if wait == 0 {
            return;
        }
        let deadline = now_ms.saturating_add(wait);
        self.deadlines
            .entry(key)
            .and_modify(|current| *current = (*current).max(deadline))
            .or_insert(deadline);
    }

    /// 冷却是否仍在生效。
    pub fn is_cooling(&self, key: &CooldownKey, now_ms: u64) -> bool {
        self.remaining_ms(key, now_ms) > 0
    }

    /// 剩余冷却（毫秒）；无冷却记录返回 0。
    pub fn remaining_ms(&self, key: &CooldownKey, now_ms: u64) -> u64 {
        self.deadlines
            .get(key)
            .map_or(0, |deadline| deadline.saturating_sub(now_ms))
    }

    /// 清理已到期条目（惰性，避免无限增长）。
    pub fn expire(&mut self, now_ms: u64) {
        self.deadlines.retain(|_, deadline| *deadline > now_ms);
    }

    /// 清除指定键的冷却（成功恢复时调用）。
    pub fn clear(&mut self, key: &CooldownKey) {
        self.deadlines.remove(key);
    }

    /// 当前冷却条目数（测试 / 可观测性）。
    pub fn len(&self) -> usize {
        self.deadlines.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.deadlines.is_empty()
    }
}

/// 断路器状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    /// 关闭：放行全部请求。
    #[default]
    Closed,
    /// 打开：拒绝请求，等待 `open_timeout_ms` 后进入半开。
    Open,
    /// 半开：放行有限探针，连续成功复原或探针失败重新打开。
    HalfOpen,
}

/// 断路器配置（bounded retry）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CircuitConfig {
    /// 连续失败阈值：达到即跳闸（打开）。
    pub failure_threshold: u32,
    /// 打开后等待时长（毫秒），之后进入半开探针。
    pub open_timeout_ms: u64,
    /// 半开状态允许的探针数。
    pub half_open_max_probes: u32,
    /// 半开连续成功复原阈值。
    pub success_threshold: u32,
}

impl CircuitConfig {
    /// 默认：阈值 5、打开 30s、探针 1、复原 2 次连续成功。
    pub const DEFAULT_FAILURE_THRESHOLD: u32 = 5;
    pub const DEFAULT_OPEN_TIMEOUT_MS: u64 = 30_000;
    pub const DEFAULT_HALF_OPEN_MAX_PROBES: u32 = 1;
    pub const DEFAULT_SUCCESS_THRESHOLD: u32 = 2;
}

impl Default for CircuitConfig {
    fn default() -> Self {
        Self {
            failure_threshold: Self::DEFAULT_FAILURE_THRESHOLD,
            open_timeout_ms: Self::DEFAULT_OPEN_TIMEOUT_MS,
            half_open_max_probes: Self::DEFAULT_HALF_OPEN_MAX_PROBES,
            success_threshold: Self::DEFAULT_SUCCESS_THRESHOLD,
        }
    }
}

/// 断路器（per account / credential）：bounded retry + half-open probe。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CircuitBreaker {
    /// 配置。
    pub config: CircuitConfig,
    /// 当前状态。
    pub state: CircuitState,
    /// Closed 下累计的连续失败。
    pub consecutive_failures: u32,
    /// HalfOpen 下累计的连续成功。
    pub consecutive_successes: u32,
    /// 打开时刻（毫秒）；用于到期进入半开。
    pub opened_at_ms: Option<u64>,
    /// 半开已用探针数。
    pub probes_used: u32,
}

impl CircuitBreaker {
    /// 以配置构造（Closed）。
    pub fn new(config: CircuitConfig) -> Self {
        let config = CircuitConfig {
            failure_threshold: config.failure_threshold.max(1),
            open_timeout_ms: config.open_timeout_ms,
            half_open_max_probes: config.half_open_max_probes.max(1),
            success_threshold: config.success_threshold.max(1),
        };
        Self {
            config,
            state: CircuitState::Closed,
            consecutive_failures: 0,
            consecutive_successes: 0,
            opened_at_ms: None,
            probes_used: 0,
        }
    }

    /// 无副作用检查当前是否可放行；打开到期后惰性进入半开。
    pub fn can_allow(&mut self, now_ms: u64) -> bool {
        if self.state == CircuitState::Open {
            if let Some(opened_at) = self.opened_at_ms {
                if now_ms >= opened_at.saturating_add(self.config.open_timeout_ms) {
                    self.state = CircuitState::HalfOpen;
                    self.consecutive_successes = 0;
                    self.probes_used = 0;
                }
            }
        }
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => false,
            CircuitState::HalfOpen => self.probes_used < self.config.half_open_max_probes,
        }
    }

    /// 当前是否放行请求；HalfOpen 放行时预留一个并发探针槽位。
    pub fn allow(&mut self, now_ms: u64) -> bool {
        if !self.can_allow(now_ms) {
            return false;
        }
        if self.state == CircuitState::HalfOpen {
            self.probes_used += 1;
        }
        true
    }

    /// 记录成功：Closed 复原失败计数；HalfOpen 累计连续成功，达标后关闭。
    pub fn record_success(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.consecutive_failures = 0;
            }
            CircuitState::HalfOpen => {
                // `probes_used` tracks in-flight probes, not lifetime probes. A
                // successful probe releases its slot so `success_threshold >
                // half_open_max_probes` can make progress sequentially.
                self.probes_used = self.probes_used.saturating_sub(1);
                self.consecutive_successes += 1;
                if self.consecutive_successes >= self.config.success_threshold {
                    self.state = CircuitState::Closed;
                    self.consecutive_failures = 0;
                    self.consecutive_successes = 0;
                    self.opened_at_ms = None;
                    self.probes_used = 0;
                }
            }
            CircuitState::Open => {}
        }
    }

    /// 结算一个既未成功、也不应惩罚断路器的 HalfOpen 探针（取消、请求错误、
    /// rate limit 等）：只归还并发探针槽位，不累计成功或失败。
    pub fn release_probe(&mut self) {
        if self.state == CircuitState::HalfOpen {
            self.probes_used = self.probes_used.saturating_sub(1);
        }
    }

    /// 记录失败；返回是否因本次失败跳闸（进入 Open）。
    pub fn record_failure(&mut self, now_ms: u64) -> bool {
        match self.state {
            CircuitState::Closed => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= self.config.failure_threshold {
                    self.trip(now_ms);
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => {
                // 探针失败：立即重新打开（避免风暴）。
                self.trip(now_ms);
                true
            }
            CircuitState::Open => false,
        }
    }

    /// 打开（跳闸）：重置半开计数并记录打开时刻。
    fn trip(&mut self, now_ms: u64) {
        self.state = CircuitState::Open;
        self.opened_at_ms = Some(now_ms);
        self.consecutive_successes = 0;
        self.probes_used = 0;
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(CircuitConfig::default())
    }
}

/// 单账号健康记录（状态机载体）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthRecord {
    /// 当前状态。
    pub state: HealthState,
    /// 连续失败次数（仅服务端失败累加；取消不计）。
    pub consecutive_failures: u32,
    /// 累计取消次数（不惩罚健康）。
    pub cancelled_count: u64,
    /// 账号作用域冷却截止（毫秒）；`None` = 未冷却。
    pub cooldown_until_ms: Option<u64>,
}

impl HealthRecord {
    /// 健康记录（Healthy）。
    pub fn new() -> Self {
        Self {
            state: HealthState::Healthy,
            consecutive_failures: 0,
            cancelled_count: 0,
            cooldown_until_ms: None,
        }
    }

    /// 记录一次取消（不惩罚健康，仅计数）。
    pub fn record_cancelled(&mut self) {
        self.cancelled_count += 1;
    }

    /// 记录一次失败：按类别转换状态；`cooldown_until_ms` 为 `Some` 时进入
    /// `CoolingDown`，否则软降级为 `Degraded`。客户端错误 / 取消 /
    /// 协议不兼容不在此调用（无惩罚）。
    pub fn record_failure(&mut self, class: FailureClass, cooldown_until_ms: Option<u64>) {
        match class {
            FailureClass::BillingBlocked => {
                self.state = HealthState::BillingBlocked;
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                self.cooldown_until_ms = None;
            }
            FailureClass::Cancelled
            | FailureClass::InvalidRequest
            | FailureClass::ContextTooLarge
            | FailureClass::ProtocolIncompatible
            | FailureClass::Unknown => {
                // 无惩罚：调用方不应走到这里（防御性保留）。
            }
            FailureClass::RateLimited
            | FailureClass::QuotaExceeded
            | FailureClass::QuotaSoftExceeded
            | FailureClass::AuthInvalid
            | FailureClass::UpstreamError
            | FailureClass::Network
            | FailureClass::StreamInterrupted => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                self.cooldown_until_ms = match (self.cooldown_until_ms, cooldown_until_ms) {
                    (Some(current), Some(next)) => Some(current.max(next)),
                    (current @ Some(_), None) => current,
                    (None, next) => next,
                };
                self.state = if self.cooldown_until_ms.is_some() {
                    HealthState::CoolingDown
                } else {
                    HealthState::Degraded
                };
            }
        }
    }

    /// 记录成功：`Degraded` 复原为 `Healthy`。`CoolingDown` 必须等待
    /// Retry-After / backoff 到期，避免并发中的旧成功提前清除冷却；
    /// `BillingBlocked` / `Disabled` 也只允许显式恢复。
    pub fn record_success(&mut self) {
        if self.state == HealthState::Degraded {
            self.consecutive_failures = 0;
            self.state = HealthState::Healthy;
        }
    }

    /// 惰性刷新：冷却到期后 `CoolingDown` → `Healthy` 并开始新的连续失败窗口。
    pub fn refresh(&mut self, now_ms: u64) {
        if let Some(deadline) = self.cooldown_until_ms {
            if deadline <= now_ms {
                self.cooldown_until_ms = None;
                if self.state == HealthState::CoolingDown {
                    self.consecutive_failures = 0;
                    self.state = HealthState::Healthy;
                }
            }
        }
    }
}

impl Default for HealthRecord {
    fn default() -> Self {
        Self::new()
    }
}

/// 失败上下文：一次失败涉及的实体（供健康惩罚定位 scope）。
///
/// 只携带 opaque id，**绝不含明文 / 错误原文**。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FailureContext {
    /// 失败请求绑定的账号。
    pub account_id: Option<AccountId>,
    /// 失败请求使用的凭据。
    pub credential_id: Option<CredentialId>,
    /// 失败请求使用的模型。
    pub model_id: Option<ModelId>,
    /// 失败请求使用的 Provider。
    pub provider_id: Option<ProviderId>,
}

impl FailureContext {
    /// 构造上下文。
    pub fn new(
        account_id: Option<AccountId>,
        credential_id: Option<CredentialId>,
        model_id: Option<ModelId>,
        provider_id: Option<ProviderId>,
    ) -> Self {
        Self {
            account_id,
            credential_id,
            model_id,
            provider_id,
        }
    }
}

/// 健康运行时：聚合 cooldown + circuit + 账号状态机 + 401 refresh-once。
///
/// 所有时间经注入的 [`Clock`] 获取（测试用 [`crate::account::FixedClock`] /
/// 可变时钟，生产用 [`crate::account::SystemClock`]），保证确定性。
pub struct HealthRuntime {
    clock: Arc<dyn Clock>,
    backoff: BackoffPolicy,
    circuit_config: CircuitConfig,
    cooldowns: CooldownTracker,
    records: HashMap<CooldownKey, HealthRecord>,
    circuits: HashMap<CooldownKey, CircuitBreaker>,
    /// 每个凭据的连续鉴权失败次数（401 refresh-once 判定）。
    auth_failures: HashMap<CredentialId, u32>,
}

impl HealthRuntime {
    /// 默认配置构造。
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::with_config(clock, BackoffPolicy::default(), CircuitConfig::default())
    }

    /// 自定义退避与断路器配置构造。
    pub fn with_config(
        clock: Arc<dyn Clock>,
        backoff: BackoffPolicy,
        circuit_config: CircuitConfig,
    ) -> Self {
        Self {
            clock,
            backoff,
            circuit_config,
            cooldowns: CooldownTracker::default(),
            records: HashMap::new(),
            circuits: HashMap::new(),
            auth_failures: HashMap::new(),
        }
    }

    /// 记录一次失败（已由 [`crate::classifier::ErrorClassifier`] 归一化）。
    ///
    /// 惩罚边界（ADR-033）：
    /// - `Cancelled` / `InvalidRequest` / `ContextTooLarge` /
    ///   `ProtocolIncompatible`：无惩罚、无轮换；
    /// - `AuthInvalid`：401 refresh-once，二次失败才冷却凭据，**不切号**；
    /// - `BillingBlocked` / `QuotaExceeded`：账号级冷却；
    /// - `RateLimited`：按分类 scope 冷却，尊重 Retry-After；
    /// - `UpstreamError` / `Network` / `StreamInterrupted`：账号断路器 + scope 冷却。
    pub fn record_failure(
        &mut self,
        ctx: &FailureContext,
        classification: FailureClassification,
        retry_after_ms: Option<u64>,
    ) {
        let now_ms = self.clock.now().as_unix_millis();

        // 客户端错误 / 取消 / 协议不兼容：不惩罚健康、不触发轮换。
        match classification.class {
            FailureClass::Cancelled
            | FailureClass::InvalidRequest
            | FailureClass::ContextTooLarge
            | FailureClass::ProtocolIncompatible
            | FailureClass::Unknown => {
                self.release_probe_slots(ctx, None);
                return;
            }
            _ => {}
        }

        // 惩罚 scope 定位：按分类 scope 选择实体键。退避计数同样按 scope
        // 隔离，模型限流不得抬高账号或 Provider 的退避档位。
        let penalty_key = self.penalty_key(ctx, classification.scope);
        let attempt = penalty_key
            .as_ref()
            .and_then(|key| self.records.get(key))
            .map_or(0, |record| record.consecutive_failures);
        let delay_ms = self.backoff.delay_ms(attempt);
        let mut update_health_record = true;

        match classification.class {
            FailureClass::AuthInvalid => {
                // 401 refresh-once：第一次失败允许刷新；第二次起冷却凭据，
                // 绝不自动错误切号（safe_to_failover=false 由调用方遵守）。
                let Some(credential) = &ctx.credential_id else {
                    return;
                };
                let count = self.auth_failures.entry(credential.clone()).or_default();
                *count += 1;
                if *count >= 2 {
                    self.cooldowns.cool(
                        CooldownKey::credential(credential),
                        retry_after_ms,
                        now_ms,
                        delay_ms,
                    );
                } else {
                    // 第一次 401 只触发 refresh-once，不先摘除同一凭据。
                    update_health_record = false;
                }
            }
            FailureClass::BillingBlocked | FailureClass::QuotaExceeded => {
                if classification.class == FailureClass::QuotaExceeded {
                    if let Some(account) = &ctx.account_id {
                        self.cooldowns.cool(
                            CooldownKey::account(account),
                            retry_after_ms,
                            now_ms,
                            delay_ms,
                        );
                    }
                }
            }
            FailureClass::RateLimited => {
                if let Some(key) = penalty_key.as_ref() {
                    self.cooldowns
                        .cool(key.clone(), retry_after_ms, now_ms, delay_ms);
                }
            }
            FailureClass::QuotaSoftExceeded => {
                // 软配额：策略降级 / 告警，不冷却、不与 RateLimited 混淆。
            }
            FailureClass::UpstreamError
            | FailureClass::Network
            | FailureClass::StreamInterrupted => {
                if let Some(key) = penalty_key.as_ref() {
                    let circuit = self
                        .circuits
                        .entry(key.clone())
                        .or_insert_with(|| CircuitBreaker::new(self.circuit_config));
                    circuit.record_failure(now_ms);
                }
                if let Some(key) = penalty_key.as_ref() {
                    self.cooldowns
                        .cool(key.clone(), retry_after_ms, now_ms, delay_ms);
                }
            }
            FailureClass::Cancelled
            | FailureClass::InvalidRequest
            | FailureClass::ContextTooLarge
            | FailureClass::ProtocolIncompatible
            | FailureClass::Unknown => unreachable!("已在上方提前返回"),
        }

        // `is_admissible` 可能为多个 scope 预留 HalfOpen 探针槽位。只有
        // canonical upstream/network/stream failure 对分类命中的 scope 记失败；
        // 其它 scope（以及 401/402/429/quota）只归还槽位，避免永久堵塞。
        let failed_circuit_key = matches!(
            classification.class,
            FailureClass::UpstreamError | FailureClass::Network | FailureClass::StreamInterrupted
        )
        .then_some(penalty_key.as_ref())
        .flatten();
        self.release_probe_slots(ctx, failed_circuit_key);

        // 健康状态与 cooldown/circuit 使用同一 scope key，避免模型或 Provider
        // 故障污染账号状态。第一次 401 是 refresh-once 的唯一例外。
        if update_health_record {
            if let Some(key) = penalty_key {
                let remaining = self.cooldowns.remaining_ms(&key, now_ms);
                self.records.entry(key).or_default().record_failure(
                    classification.class,
                    (remaining > 0).then_some(now_ms.saturating_add(remaining)),
                );
            }
        }
    }

    /// 记录一次成功：复原对应 scope 的软降级、推进 half-open probe，并清
    /// auth 连续失败计数。活跃 Retry-After/cooldown 不会被并发中的旧成功提前清除。
    pub fn record_success(&mut self, ctx: &FailureContext) {
        let now_ms = self.clock.now().as_unix_millis();
        self.cooldowns.expire(now_ms);
        for key in Self::context_keys(ctx) {
            if let Some(record) = self.records.get_mut(&key) {
                record.refresh(now_ms);
                record.record_success();
            }
            if let Some(circuit) = self.circuits.get_mut(&key) {
                circuit.record_success();
            }
        }
        if let Some(credential) = &ctx.credential_id {
            self.auth_failures.remove(credential);
        }
    }

    /// 记录一次取消（不惩罚健康，仅计数），并结算本次准入预留的所有
    /// HalfOpen 探针槽位。
    pub fn record_cancelled(&mut self, ctx: &FailureContext) {
        if let Some(account) = &ctx.account_id {
            self.records
                .entry(CooldownKey::account(account))
                .or_default()
                .record_cancelled();
        }
        self.release_probe_slots(ctx, None);
    }

    /// 账号健康状态（惰性刷新冷却到期）。
    pub fn account_state(&mut self, account: &AccountId) -> HealthState {
        self.scope_state(&CooldownKey::account(account))
    }

    /// 指定账号/凭据/模型/Provider scope 的健康状态。
    pub fn scope_state(&mut self, key: &CooldownKey) -> HealthState {
        let now_ms = self.clock.now().as_unix_millis();
        let record = self.records.entry(key.clone()).or_default();
        record.refresh(now_ms);
        record.state
    }

    /// 只读路由过滤：账号 / 凭据 / 模型 / Provider 全部放行才算可准入。
    ///
    /// 此检查会惰性推进到期状态，但**不会预留** HalfOpen 探针槽位；候选排序、
    /// 解释或预览必须使用该入口，避免未被选中的候选耗尽探针并发。
    pub fn can_admit(&mut self, ctx: &FailureContext) -> bool {
        let now_ms = self.clock.now().as_unix_millis();
        self.cooldowns.expire(now_ms);
        let keys = Self::context_keys(ctx);

        for key in &keys {
            let record = self.records.entry(key.clone()).or_default();
            record.refresh(now_ms);
            if !record.state.is_admissible() {
                return false;
            }
            if self.cooldowns.is_cooling(key, now_ms) {
                return false;
            }
        }

        for key in &keys {
            if let Some(circuit) = self.circuits.get_mut(key) {
                if !circuit.can_allow(now_ms) {
                    return false;
                }
            }
        }
        true
    }

    /// 执行准入并为所有 HalfOpen scope 原子预留探针槽位。
    ///
    /// 只有 route winner 在即将 Acquire Lease / 发起 Provider 调用时才能使用该
    /// 入口；只读候选过滤应使用 [`Self::can_admit`]。
    pub fn is_admissible(&mut self, ctx: &FailureContext) -> bool {
        if !self.can_admit(ctx) {
            return false;
        }

        let now_ms = self.clock.now().as_unix_millis();
        let keys = Self::context_keys(ctx);
        for key in keys {
            if let Some(circuit) = self.circuits.get_mut(&key) {
                debug_assert!(circuit.allow(now_ms));
            }
        }
        true
    }

    /// 指定冷却键的剩余冷却（毫秒）。
    pub fn cooldown_remaining_ms(&self, key: &CooldownKey) -> u64 {
        let now_ms = self.clock.now().as_unix_millis();
        self.cooldowns.remaining_ms(key, now_ms)
    }

    /// 401 refresh-once：连续鉴权失败 < 2 时允许刷新一次。
    pub fn refresh_eligible(&self, credential: &CredentialId) -> bool {
        self.auth_failures.get(credential).copied().unwrap_or(0) < 2
    }

    /// 账号断路器状态（可观测性）。
    pub fn circuit_state(&self, account: &AccountId) -> CircuitState {
        self.circuit_state_for(&CooldownKey::account(account))
    }

    /// 指定 scope 的断路器状态（用于路由解释与可观测性）。
    pub fn circuit_state_for(&self, key: &CooldownKey) -> CircuitState {
        self.circuits
            .get(key)
            .map_or(CircuitState::Closed, |circuit| circuit.state)
    }

    /// 显式恢复账号（清计费封禁 / 禁用）。
    pub fn recover_account(&mut self, account: &AccountId) {
        let key = CooldownKey::account(account);
        let record = self.records.entry(key.clone()).or_default();
        record.state = HealthState::Healthy;
        record.consecutive_failures = 0;
        record.cooldown_until_ms = None;
        self.cooldowns.clear(&key);
        self.circuits.remove(&key);
    }

    /// 显式禁用 / 启用账号（与 [`crate::account::AccountState`] 对齐）。
    pub fn set_account_disabled(&mut self, account: &AccountId, disabled: bool) {
        let record = self
            .records
            .entry(CooldownKey::account(account))
            .or_default();
        if disabled {
            record.state = HealthState::Disabled;
        } else if record.state == HealthState::Disabled {
            record.state = HealthState::Healthy;
            record.consecutive_failures = 0;
            record.cooldown_until_ms = None;
        }
    }

    /// 冷却条目数（测试 / 可观测性）。
    pub fn cooldown_len(&self) -> usize {
        self.cooldowns.len()
    }

    /// 扫描并刷新过期 cooldown / CoolingDown 记录（reconciler stale-health 步）。
    ///
    /// 不改变 `BillingBlocked` / `Disabled`；到期冷却惰性复原为 `Healthy`。
    pub fn refresh_stale(&mut self) -> usize {
        let now_ms = self.clock.now().as_unix_millis();
        self.cooldowns.expire(now_ms);
        let mut refreshed = 0usize;
        for record in self.records.values_mut() {
            let before = record.state;
            record.refresh(now_ms);
            if before != record.state {
                refreshed += 1;
            }
        }
        refreshed
    }

    /// 由分类 scope 与失败上下文定位惩罚键。
    fn penalty_key(&self, ctx: &FailureContext, scope: FailureScope) -> Option<CooldownKey> {
        match scope {
            FailureScope::Credential => ctx.credential_id.as_ref().map(CooldownKey::credential),
            FailureScope::Account => ctx.account_id.as_ref().map(CooldownKey::account),
            FailureScope::Model => ctx.model_id.as_ref().map(CooldownKey::model),
            FailureScope::Provider => ctx.provider_id.as_ref().map(CooldownKey::provider),
            FailureScope::Request | FailureScope::Protocol => None,
        }
    }

    fn context_keys(ctx: &FailureContext) -> Vec<CooldownKey> {
        let mut keys = Vec::with_capacity(4);
        if let Some(account) = &ctx.account_id {
            keys.push(CooldownKey::account(account));
        }
        if let Some(credential) = &ctx.credential_id {
            keys.push(CooldownKey::credential(credential));
        }
        if let Some(model) = &ctx.model_id {
            keys.push(CooldownKey::model(model));
        }
        if let Some(provider) = &ctx.provider_id {
            keys.push(CooldownKey::provider(provider));
        }
        keys
    }

    fn release_probe_slots(&mut self, ctx: &FailureContext, failed: Option<&CooldownKey>) {
        for key in Self::context_keys(ctx) {
            if failed == Some(&key) {
                continue;
            }
            if let Some(circuit) = self.circuits.get_mut(&key) {
                circuit.release_probe();
            }
        }
    }
}

/// Synthetic health probe cost class. Expensive probes default **off**.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    /// Cheap / local liveness (default-eligible).
    #[default]
    Cheap,
    /// Expensive (network / billed). Default off; factories must opt in.
    Expensive,
}

/// Independent probe concurrency / frequency / budget. Failure must not avalanche.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeBudget {
    /// Whether this class of probe is enabled. Expensive defaults to `false`.
    pub enabled: bool,
    /// Max concurrent in-flight probes of this class in one tick.
    pub max_in_flight: u32,
    /// Max probes launched per tick (frequency cap).
    pub max_per_tick: u32,
    /// Stop launching further probes this tick after this many failures.
    pub max_failures_per_tick: u32,
    /// Minimum interval between probes of the same target (milliseconds).
    pub min_interval_ms: u64,
}

impl ProbeBudget {
    /// Cheap-probe default: enabled, modest concurrency, 30s interval.
    pub const fn cheap_default() -> Self {
        Self {
            enabled: true,
            max_in_flight: 4,
            max_per_tick: 8,
            max_failures_per_tick: 2,
            min_interval_ms: 30_000,
        }
    }

    /// Expensive-probe default: **off**, single-flight, 5min interval.
    pub const fn expensive_default() -> Self {
        Self {
            enabled: false,
            max_in_flight: 1,
            max_per_tick: 1,
            max_failures_per_tick: 1,
            min_interval_ms: 300_000,
        }
    }
}

impl Default for ProbeBudget {
    fn default() -> Self {
        Self::cheap_default()
    }
}

/// Opaque probe target (provider + account). Never contains a secret.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProbeTargetKey {
    pub provider_id: ProviderId,
    pub account_id: AccountId,
}

impl ProbeTargetKey {
    /// Construct from opaque ids.
    pub fn new(provider_id: ProviderId, account_id: AccountId) -> Self {
        Self {
            provider_id,
            account_id,
        }
    }
}

/// Probe failure (no secret / no provider-name payload).
#[derive(Clone, Debug, Error)]
#[error("health probe failed: {class:?}")]
pub struct ProbeFailure {
    /// Normalized failure class (typically `Network` / `UpstreamError`).
    pub class: FailureClass,
    /// Optional Retry-After (milliseconds).
    pub retry_after_ms: Option<u64>,
}

impl ProbeFailure {
    /// Construct a classified probe failure.
    pub fn new(class: FailureClass) -> Self {
        Self {
            class,
            retry_after_ms: None,
        }
    }

    /// Attach Retry-After.
    pub fn with_retry_after(mut self, retry_after_ms: u64) -> Self {
        self.retry_after_ms = Some(retry_after_ms);
        self
    }
}

/// One tick of the probe runtime.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProbeReport {
    /// Probes launched this tick.
    pub launched: usize,
    /// Successful probes.
    pub succeeded: usize,
    /// Failed probes (counted against the failure budget).
    pub failed: usize,
    /// Skipped: disabled class, interval, circuit open, or avalanche budget.
    pub skipped: usize,
}

/// Factory / capability extension point for synthetic probes.
///
/// Core never branches on Provider name; a [`crate::factory::ProviderFactory`]
/// opts in via [`crate::factory::ProviderFactory::health_probe`].
#[async_trait]
pub trait HealthProbe: Send + Sync {
    /// Cheap vs expensive (budget class).
    fn kind(&self) -> ProbeKind;

    /// Failure / success context (opaque ids only).
    fn context(&self) -> FailureContext;

    /// Target key used for interval tracking.
    fn target_key(&self) -> ProbeTargetKey {
        let ctx = self.context();
        ProbeTargetKey::new(
            ctx.provider_id
                .unwrap_or_else(|| ProviderId::new("unknown")),
            ctx.account_id.unwrap_or_else(|| AccountId::new("unknown")),
        )
    }

    /// Run the synthetic probe. Must not hold secrets after return.
    async fn probe(&self) -> Result<(), ProbeFailure>;
}

/// Configurable probe scheduler: independent concurrency / frequency / budget.
///
/// Probe failures feed [`HealthRuntime`] (cooldown + circuit) but **do not**
/// set `safe_to_failover`, so they cannot avalanche into account rotation.
pub struct ProbeRuntime {
    cheap: ProbeBudget,
    expensive: ProbeBudget,
    last_probe_ms: HashMap<ProbeTargetKey, u64>,
}

impl ProbeRuntime {
    /// Default budgets (cheap on, expensive off).
    pub fn new() -> Self {
        Self {
            cheap: ProbeBudget::cheap_default(),
            expensive: ProbeBudget::expensive_default(),
            last_probe_ms: HashMap::new(),
        }
    }

    /// Override budgets.
    pub fn with_budgets(cheap: ProbeBudget, expensive: ProbeBudget) -> Self {
        Self {
            cheap,
            expensive,
            last_probe_ms: HashMap::new(),
        }
    }

    /// Cheap-probe budget (mutable for tests / config reload).
    pub fn cheap_budget(&self) -> ProbeBudget {
        self.cheap
    }

    /// Expensive-probe budget.
    pub fn expensive_budget(&self) -> ProbeBudget {
        self.expensive
    }

    /// Replace the cheap-probe budget.
    pub fn set_cheap_budget(&mut self, budget: ProbeBudget) {
        self.cheap = budget;
    }

    /// Replace the expensive-probe budget.
    pub fn set_expensive_budget(&mut self, budget: ProbeBudget) {
        self.expensive = budget;
    }

    fn budget(&self, kind: ProbeKind) -> ProbeBudget {
        match kind {
            ProbeKind::Cheap => self.cheap,
            ProbeKind::Expensive => self.expensive,
        }
    }

    /// Run eligible probes under budget. Uses [`HealthRuntime`] for circuit /
    /// cooldown; does not invent a second health machine.
    pub async fn tick(
        &mut self,
        health: &mut HealthRuntime,
        probes: &[Arc<dyn HealthProbe>],
        now_ms: u64,
    ) -> ProbeReport {
        let mut report = ProbeReport::default();
        // Partition by kind so each class has an independent budget.
        for kind in [ProbeKind::Cheap, ProbeKind::Expensive] {
            let class_report = self.tick_class(health, probes, kind, now_ms).await;
            report.launched += class_report.launched;
            report.succeeded += class_report.succeeded;
            report.failed += class_report.failed;
            report.skipped += class_report.skipped;
        }
        report
    }

    async fn tick_class(
        &mut self,
        health: &mut HealthRuntime,
        probes: &[Arc<dyn HealthProbe>],
        kind: ProbeKind,
        now_ms: u64,
    ) -> ProbeReport {
        let budget = self.budget(kind);
        let mut report = ProbeReport::default();
        if !budget.enabled {
            report.skipped = probes.iter().filter(|probe| probe.kind() == kind).count();
            return report;
        }

        let mut eligible: Vec<Arc<dyn HealthProbe>> = Vec::new();
        for probe in probes.iter().filter(|probe| probe.kind() == kind) {
            let key = probe.target_key();
            if let Some(last) = self.last_probe_ms.get(&key) {
                if now_ms.saturating_sub(*last) < budget.min_interval_ms {
                    report.skipped += 1;
                    continue;
                }
            }
            let ctx = probe.context();
            if !health.can_admit(&ctx) {
                report.skipped += 1;
                continue;
            }
            eligible.push(Arc::clone(probe));
        }

        let cap = budget.max_per_tick as usize;
        if eligible.len() > cap {
            report.skipped += eligible.len() - cap;
            eligible.truncate(cap);
        }

        let mut joinset = tokio::task::JoinSet::new();
        let mut launched = 0usize;
        let max_in_flight = budget.max_in_flight.max(1) as usize;
        let max_failures = budget.max_failures_per_tick as usize;

        for probe in eligible {
            if report.failed >= max_failures {
                report.skipped += 1;
                continue;
            }
            while joinset.len() >= max_in_flight {
                self.collect_one(&mut joinset, health, &mut report).await;
                if report.failed >= max_failures {
                    break;
                }
            }
            if report.failed >= max_failures {
                report.skipped += 1;
                continue;
            }
            let key = probe.target_key();
            self.last_probe_ms.insert(key, now_ms);
            joinset.spawn(async move {
                let ctx = probe.context();
                let result = probe.probe().await;
                (ctx, result)
            });
            launched += 1;
        }
        report.launched += launched;
        while !joinset.is_empty() {
            self.collect_one(&mut joinset, health, &mut report).await;
        }
        report
    }

    async fn collect_one(
        &self,
        joinset: &mut tokio::task::JoinSet<(FailureContext, Result<(), ProbeFailure>)>,
        health: &mut HealthRuntime,
        report: &mut ProbeReport,
    ) {
        let Some(joined) = joinset.join_next().await else {
            return;
        };
        match joined {
            Ok((ctx, Ok(()))) => {
                health.record_success(&ctx);
                report.succeeded += 1;
            }
            Ok((ctx, Err(failure))) => {
                health.record_failure(
                    &ctx,
                    probe_classification(failure.class),
                    failure.retry_after_ms,
                );
                report.failed += 1;
            }
            Err(_) => {
                report.failed += 1;
            }
        }
    }
}

impl Default for ProbeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn probe_classification(class: FailureClass) -> FailureClassification {
    FailureClassification {
        class,
        scope: FailureScope::Account,
        retryability: Retryability::Delayed,
        health_impact: HealthImpact::Degraded,
        // Probe failure must not avalanche into credential/account rotation.
        safe_to_failover: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    use agent_domain::{AccountId, CredentialId, ModelId, ProviderId, Timestamp};

    use crate::classifier::{ErrorClassifier, HttpErrorClassifier};

    /// 可变时钟：测试推进时间用。
    #[derive(Clone)]
    struct MutableClock(Arc<AtomicU64>);

    impl MutableClock {
        fn new(now_ms: u64) -> Self {
            Self(Arc::new(AtomicU64::new(now_ms)))
        }

        fn advance(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::Relaxed);
        }
    }

    impl Clock for MutableClock {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_millis(self.0.load(Ordering::Relaxed))
        }
    }

    fn ctx(
        account: &AccountId,
        credential: &CredentialId,
        model: &ModelId,
        provider: &ProviderId,
    ) -> FailureContext {
        FailureContext::new(
            Some(account.clone()),
            Some(credential.clone()),
            Some(model.clone()),
            Some(provider.clone()),
        )
    }

    fn classify(status: u16) -> FailureClassification {
        HttpErrorClassifier.classify_http(status, None)
    }

    #[test]
    fn health_state_db_strings_round_trip_and_unknown_fails_closed() {
        for state in [
            HealthState::Healthy,
            HealthState::Degraded,
            HealthState::CoolingDown,
            HealthState::BillingBlocked,
            HealthState::Disabled,
        ] {
            assert_eq!(HealthState::from_db_str(state.as_db_str()), Some(state));
        }
        assert_eq!(HealthState::from_db_str("unknown"), None);
        assert_eq!(HealthState::default(), HealthState::Healthy);
        assert!(HealthState::Healthy.is_admissible());
        assert!(HealthState::Degraded.is_admissible());
        assert!(!HealthState::BillingBlocked.is_admissible());
        assert!(!HealthState::Disabled.is_admissible());
    }

    #[test]
    fn backoff_is_bounded_and_deterministic() {
        let policy = BackoffPolicy::new(200, 30_000, 8);
        let mut previous = 0u64;
        for attempt in 0..12 {
            let delay = policy.delay_ms(attempt);
            assert!(delay >= 200, "delay {delay} below base");
            assert!(delay <= 30_000, "delay {delay} above cap");
            assert!(delay >= previous, "backoff must not shrink");
            previous = delay;
        }
        // 确定性：相同 attempt 相同 delay。
        assert_eq!(policy.delay_ms(3), policy.delay_ms(3));

        let jittered = BackoffPolicy::new(200, 30_000, 8).with_jitter(20);
        for attempt in 0..12 {
            let delay = jittered.delay_ms(attempt);
            assert!((1..=30_000).contains(&delay));
            assert_eq!(jittered.delay_ms(attempt), jittered.delay_ms(attempt));
        }
    }

    #[test]
    fn cooldown_respects_retry_after_and_falls_back_to_backoff() {
        let mut tracker = CooldownTracker::default();
        let key = CooldownKey::account("acct-1");

        tracker.cool(key.clone(), Some(5_000), 1_000, 999);
        assert!(tracker.is_cooling(&key, 5_999));
        assert_eq!(tracker.remaining_ms(&key, 5_000), 1_000);
        assert!(!tracker.is_cooling(&key, 6_000));

        // 无 Retry-After → 使用退避延迟。
        let key2 = CooldownKey::credential("cred-1");
        tracker.cool(key2.clone(), None, 1_000, 800);
        assert_eq!(tracker.remaining_ms(&key2, 1_000), 800);
        tracker.expire(2_000);
        assert_eq!(tracker.len(), 1); // key 已到期被清理，key2 未到期保留
    }

    #[test]
    fn circuit_breaker_trips_opens_half_opens_and_recovers() {
        let config = CircuitConfig {
            failure_threshold: 3,
            open_timeout_ms: 1_000,
            half_open_max_probes: 1,
            success_threshold: 2,
        };
        let mut breaker = CircuitBreaker::new(config);
        assert!(breaker.allow(0));

        // 连续失败 3 次 → 跳闸。
        assert!(!breaker.record_failure(100));
        assert!(!breaker.record_failure(110));
        assert!(breaker.record_failure(120), "阈值达到必须跳闸");
        assert_eq!(breaker.state, CircuitState::Open);
        assert!(!breaker.allow(150), "Open 拒绝请求");

        // 打开到期 → 半开，探针放行一次。
        assert!(breaker.allow(1_120), "到期进入 HalfOpen 且探针放行");
        assert!(!breaker.allow(1_121), "探针数受限");

        // 半开连续成功 2 次 → 关闭复原。
        breaker.record_success();
        assert_eq!(breaker.state, CircuitState::HalfOpen);
        assert!(breaker.allow(1_122), "成功探针释放并发槽位");
        breaker.record_success();
        assert_eq!(breaker.state, CircuitState::Closed);
        assert!(breaker.allow(1_200));
        breaker.record_success();
        assert_eq!(breaker.consecutive_failures, 0);
    }

    #[test]
    fn circuit_breaker_half_open_probe_failure_retrips() {
        let config = CircuitConfig {
            failure_threshold: 2,
            open_timeout_ms: 1_000,
            half_open_max_probes: 1,
            success_threshold: 2,
        };
        let mut breaker = CircuitBreaker::new(config);
        breaker.record_failure(0);
        assert!(breaker.record_failure(10));
        assert!(!breaker.allow(100));
        assert!(breaker.allow(1_010), "半开探针");
        assert!(breaker.record_failure(1_010), "探针失败重新打开");
        assert_eq!(breaker.state, CircuitState::Open);
        assert!(!breaker.allow(1_020));
    }

    #[test]
    fn runtime_half_open_cancel_and_client_error_release_probe_slot() {
        let clock = MutableClock::new(1_000);
        let mut runtime = HealthRuntime::with_config(
            Arc::new(clock.clone()),
            BackoffPolicy::default(),
            CircuitConfig {
                failure_threshold: 1,
                open_timeout_ms: 1_000,
                half_open_max_probes: 1,
                success_threshold: 1,
            },
        );
        let account = AccountId::new("acct-a");
        let credential = CredentialId::new("cred-a");
        let model = ModelId::new("model-a");
        let provider = ProviderId::new("prov-a");
        let context = ctx(&account, &credential, &model, &provider);
        let provider_failure = FailureClassification {
            class: FailureClass::UpstreamError,
            scope: FailureScope::Provider,
            retryability: crate::classifier::Retryability::Immediate,
            health_impact: crate::classifier::HealthImpact::Degraded,
            safe_to_failover: true,
        };

        runtime.record_failure(&context, provider_failure, None);
        clock.advance(1_000);
        assert!(runtime.is_admissible(&context));
        runtime.record_cancelled(&context);
        assert!(runtime.is_admissible(&context), "取消必须归还 probe 槽位");
        runtime.record_failure(&context, classify(400), None);
        assert!(
            runtime.is_admissible(&context),
            "客户端错误必须归还 probe 槽位"
        );
    }

    #[test]
    fn health_record_transitions_follow_matrix() {
        let mut record = HealthRecord::new();
        record.record_failure(FailureClass::RateLimited, Some(5_000));
        assert_eq!(record.state, HealthState::CoolingDown);
        assert_eq!(record.consecutive_failures, 1);

        record.record_failure(FailureClass::UpstreamError, None);
        assert_eq!(record.state, HealthState::CoolingDown, "冷却截止保留");
        assert_eq!(record.consecutive_failures, 2);

        record.record_success();
        assert_eq!(record.state, HealthState::CoolingDown);
        assert_eq!(record.consecutive_failures, 2);
        record.refresh(5_000);
        assert_eq!(record.state, HealthState::Healthy);
        assert_eq!(record.consecutive_failures, 0);

        record.record_failure(FailureClass::BillingBlocked, None);
        assert_eq!(record.state, HealthState::BillingBlocked);
        record.record_success();
        assert_eq!(
            record.state,
            HealthState::BillingBlocked,
            "计费封禁不受成功影响"
        );

        let mut record = HealthRecord::new();
        record.record_cancelled();
        assert_eq!(record.state, HealthState::Healthy);
        assert_eq!(record.cancelled_count, 1);
    }

    #[test]
    fn health_record_cooldown_expires_lazily() {
        let mut record = HealthRecord::new();
        record.record_failure(FailureClass::RateLimited, Some(1_000));
        assert_eq!(record.state, HealthState::CoolingDown);
        record.refresh(999);
        assert_eq!(record.state, HealthState::CoolingDown);
        record.refresh(1_000);
        assert_eq!(record.state, HealthState::Healthy);
    }

    #[test]
    fn runtime_client_errors_never_penalize() {
        let clock = MutableClock::new(1_000);
        let mut runtime = HealthRuntime::new(Arc::new(clock.clone()));
        let account = AccountId::new("acct-a");
        let credential = CredentialId::new("cred-a");
        let model = ModelId::new("model-a");
        let provider = ProviderId::new("prov-a");
        let context = ctx(&account, &credential, &model, &provider);

        for status in [400, 413, 499] {
            runtime.record_failure(&context, classify(status), None);
        }
        runtime.record_failure(
            &context,
            HttpErrorClassifier.classify_protocol_incompatible(),
            None,
        );
        runtime.record_failure(&context, HttpErrorClassifier.classify_cancelled(), None);

        assert_eq!(runtime.account_state(&account), HealthState::Healthy);
        assert!(runtime.is_admissible(&context));
        assert_eq!(runtime.cooldown_len(), 0);
        assert_eq!(runtime.circuit_state(&account), CircuitState::Closed);
    }

    #[test]
    fn runtime_auth_refresh_once_then_cooldown_without_rotation() {
        let clock = MutableClock::new(1_000);
        let mut runtime = HealthRuntime::new(Arc::new(clock.clone()));
        let account = AccountId::new("acct-a");
        let credential = CredentialId::new("cred-a");
        let context = ctx(
            &account,
            &credential,
            &ModelId::new("m"),
            &ProviderId::new("p"),
        );

        assert!(runtime.refresh_eligible(&credential));
        runtime.record_failure(&context, classify(401), None);
        assert!(
            runtime.refresh_eligible(&credential),
            "第一次 401 允许 refresh-once"
        );
        assert_eq!(
            runtime.account_state(&account),
            HealthState::Healthy,
            "不切号"
        );
        assert!(runtime.is_admissible(&context), "第一次 401 不冷却凭据");

        runtime.record_failure(&context, classify(401), Some(5_000));
        assert!(
            !runtime.refresh_eligible(&credential),
            "第二次 401 不再自动刷新"
        );
        assert!(runtime
            .cooldowns
            .is_cooling(&CooldownKey::credential(&credential), 1_000));
        assert_eq!(
            runtime.account_state(&account),
            HealthState::Healthy,
            "凭据级失败不降级账号"
        );
        assert_eq!(
            runtime.scope_state(&CooldownKey::credential(&credential)),
            HealthState::CoolingDown
        );

        runtime.record_success(&context);
        assert!(runtime.refresh_eligible(&credential));
        assert_eq!(runtime.cooldown_len(), 1);
        clock.advance(5_000);
        assert!(runtime.is_admissible(&context));
        assert_eq!(runtime.cooldown_len(), 0);
    }

    #[test]
    fn runtime_billing_blocked_marks_account_and_blocks_admission() {
        let clock = MutableClock::new(1_000);
        let mut runtime = HealthRuntime::new(Arc::new(clock.clone()));
        let account = AccountId::new("acct-a");
        let context = ctx(
            &account,
            &CredentialId::new("cred-a"),
            &ModelId::new("m"),
            &ProviderId::new("p"),
        );

        runtime.record_failure(&context, classify(402), None);
        assert_eq!(runtime.account_state(&account), HealthState::BillingBlocked);
        assert!(!runtime.is_admissible(&context));

        runtime.recover_account(&account);
        assert_eq!(runtime.account_state(&account), HealthState::Healthy);
        assert!(runtime.is_admissible(&context));
    }

    #[test]
    fn runtime_rate_limit_respects_retry_after_with_scope_isolation() {
        let clock = MutableClock::new(1_000);
        let mut runtime = HealthRuntime::new(Arc::new(clock.clone()));
        let account = AccountId::new("acct-a");
        let credential = CredentialId::new("cred-a");
        let model = ModelId::new("model-a");
        let provider = ProviderId::new("prov-a");
        let context = ctx(&account, &credential, &model, &provider);

        // 429 有 Retry-After：5s 冷却。
        runtime.record_failure(&context, classify(429), Some(5_000));
        assert_eq!(
            runtime.cooldown_remaining_ms(&CooldownKey::credential(&credential)),
            5_000
        );
        assert!(!runtime.is_admissible(&context));
        clock.advance(5_000);
        assert!(runtime.is_admissible(&context), "Retry-After 到期恢复");

        // 模型 scope 的 429：只冷却模型，不影响账号 / 凭据。
        let model_scoped = crate::classifier::FailureClassification {
            class: FailureClass::RateLimited,
            scope: FailureScope::Model,
            retryability: crate::classifier::Retryability::Delayed,
            health_impact: crate::classifier::HealthImpact::Degraded,
            safe_to_failover: true,
        };
        runtime.record_failure(&context, model_scoped, Some(3_000));
        assert_eq!(
            runtime.cooldown_remaining_ms(&CooldownKey::model(&model)),
            3_000
        );
        assert_eq!(
            runtime.cooldown_remaining_ms(&CooldownKey::credential(&credential)),
            0,
            "模型 scope 失败不得冷却凭据"
        );
        assert_eq!(
            runtime.cooldown_remaining_ms(&CooldownKey::account(&account)),
            0,
            "模型 scope 失败不得冷却账号"
        );
        assert_eq!(runtime.account_state(&account), HealthState::Healthy);
        assert_eq!(
            runtime.scope_state(&CooldownKey::model(&model)),
            HealthState::CoolingDown
        );
    }

    #[test]
    fn runtime_hard_and_soft_quota_differ() {
        let clock = MutableClock::new(1_000);
        let mut runtime = HealthRuntime::new(Arc::new(clock.clone()));
        let account = AccountId::new("acct-a");
        let context = ctx(
            &account,
            &CredentialId::new("cred-a"),
            &ModelId::new("model-a"),
            &ProviderId::new("prov-a"),
        );

        // 硬配额：账号冷却 + Degraded/CoolingDown，允许 failover。
        runtime.record_failure(
            &context,
            HttpErrorClassifier.classify_signal(&crate::classifier::ProviderErrorSignal::new(
                429,
                crate::classifier::ProviderErrorKind::QuotaExceeded { hard: true },
            )),
            None,
        );
        assert!(runtime.cooldown_remaining_ms(&CooldownKey::account(&account)) > 0);
        assert_eq!(runtime.account_state(&account), HealthState::CoolingDown);
        clock.advance(BackoffPolicy::DEFAULT_BASE_MS);
        assert!(runtime.is_admissible(&context));

        // 软配额：只降级，不冷却账号。
        runtime.record_failure(
            &context,
            HttpErrorClassifier.classify_signal(&crate::classifier::ProviderErrorSignal::new(
                429,
                crate::classifier::ProviderErrorKind::QuotaExceeded { hard: false },
            )),
            None,
        );
        assert_eq!(
            runtime.scope_state(&CooldownKey::model("model-a")),
            HealthState::Degraded,
            "软配额只降级"
        );
        assert_eq!(runtime.account_state(&account), HealthState::Healthy);
        assert_eq!(
            runtime.cooldown_remaining_ms(&CooldownKey::account(&account)),
            0,
            "软配额不冷却"
        );
    }

    #[test]
    fn runtime_upstream_failures_trip_circuit_and_probe_recovers() {
        let clock = MutableClock::new(1_000);
        let runtime_clock = Arc::new(clock.clone());
        let mut runtime = HealthRuntime::with_config(
            runtime_clock,
            BackoffPolicy::default(),
            CircuitConfig {
                failure_threshold: 3,
                open_timeout_ms: 1_000,
                half_open_max_probes: 1,
                success_threshold: 2,
            },
        );
        let account = AccountId::new("acct-a");
        let credential = CredentialId::new("cred-a");
        let context = ctx(
            &account,
            &credential,
            &ModelId::new("model-a"),
            &ProviderId::new("prov-a"),
        );

        for _ in 0..3 {
            runtime.record_failure(&context, classify(503), None);
        }
        let provider_key = CooldownKey::provider("prov-a");
        assert_eq!(runtime.circuit_state_for(&provider_key), CircuitState::Open);
        assert!(!runtime.is_admissible(&context));

        clock.advance(1_000);
        assert!(runtime.is_admissible(&context), "到期进入半开，探针放行");
        runtime.record_success(&context);
        assert!(runtime.is_admissible(&context), "成功探针释放并发槽位");
        runtime.record_success(&context);
        assert_eq!(
            runtime.circuit_state_for(&provider_key),
            CircuitState::Closed
        );
        assert!(runtime.is_admissible(&context));
    }

    #[test]
    fn runtime_disable_and_recover_account() {
        let clock = MutableClock::new(1_000);
        let mut runtime = HealthRuntime::new(Arc::new(clock.clone()));
        let account = AccountId::new("acct-a");
        let context = ctx(
            &account,
            &CredentialId::new("cred-a"),
            &ModelId::new("m"),
            &ProviderId::new("p"),
        );
        runtime.set_account_disabled(&account, true);
        assert_eq!(runtime.account_state(&account), HealthState::Disabled);
        assert!(!runtime.is_admissible(&context));
        runtime.set_account_disabled(&account, false);
        assert_eq!(runtime.account_state(&account), HealthState::Healthy);
        assert!(runtime.is_admissible(&context));
    }

    struct CountingProbe {
        kind: ProbeKind,
        ctx: FailureContext,
        calls: Arc<AtomicU64>,
        fail: bool,
    }

    #[async_trait]
    impl HealthProbe for CountingProbe {
        fn kind(&self) -> ProbeKind {
            self.kind
        }

        fn context(&self) -> FailureContext {
            self.ctx.clone()
        }

        async fn probe(&self) -> Result<(), ProbeFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(ProbeFailure::new(FailureClass::Network))
            } else {
                Ok(())
            }
        }
    }

    fn probe(
        kind: ProbeKind,
        account: &str,
        fail: bool,
        calls: &Arc<AtomicU64>,
    ) -> Arc<dyn HealthProbe> {
        Arc::new(CountingProbe {
            kind,
            ctx: FailureContext::new(
                Some(AccountId::new(account)),
                None,
                None,
                Some(ProviderId::new("stub")),
            ),
            calls: Arc::clone(calls),
            fail,
        })
    }

    #[tokio::test]
    async fn expensive_probes_default_off() {
        let clock = Arc::new(MutableClock::new(1_000));
        let mut health = HealthRuntime::new(clock);
        let mut runtime = ProbeRuntime::new();
        let calls = Arc::new(AtomicU64::new(0));
        let probes = vec![probe(ProbeKind::Expensive, "a1", false, &calls)];
        let report = runtime.tick(&mut health, &probes, 1_000).await;
        assert_eq!(report.launched, 0);
        assert_eq!(report.skipped, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn probe_storm_respects_per_tick_and_failure_budget() {
        let clock = Arc::new(MutableClock::new(1_000));
        let mut health = HealthRuntime::new(clock);
        let mut runtime = ProbeRuntime::with_budgets(
            ProbeBudget {
                enabled: true,
                max_in_flight: 2,
                max_per_tick: 3,
                max_failures_per_tick: 1,
                min_interval_ms: 0,
            },
            ProbeBudget::expensive_default(),
        );
        let calls = Arc::new(AtomicU64::new(0));
        let probes: Vec<Arc<dyn HealthProbe>> = (0..10)
            .map(|i| probe(ProbeKind::Cheap, &format!("acct-{i}"), true, &calls))
            .collect();
        let report = runtime.tick(&mut health, &probes, 1_000).await;
        assert!(
            report.launched <= 3,
            "max_per_tick=3, launched={}",
            report.launched
        );
        assert!(
            report.failed <= 3,
            "failure budget plus in-flight overshoot, failed={}",
            report.failed
        );
        assert!(calls.load(Ordering::SeqCst) <= 3);
        assert!(report.skipped >= 7);
    }

    #[tokio::test]
    async fn probe_interval_skips_until_min_interval() {
        let clock = Arc::new(MutableClock::new(1_000));
        let mut health = HealthRuntime::new(clock);
        let mut runtime = ProbeRuntime::with_budgets(
            ProbeBudget {
                enabled: true,
                max_in_flight: 1,
                max_per_tick: 8,
                max_failures_per_tick: 8,
                min_interval_ms: 5_000,
            },
            ProbeBudget::expensive_default(),
        );
        let calls = Arc::new(AtomicU64::new(0));
        let probes = vec![probe(ProbeKind::Cheap, "a1", false, &calls)];
        let first = runtime.tick(&mut health, &probes, 1_000).await;
        assert_eq!(first.launched, 1);
        let second = runtime.tick(&mut health, &probes, 2_000).await;
        assert_eq!(second.launched, 0);
        assert_eq!(second.skipped, 1);
        let third = runtime.tick(&mut health, &probes, 6_000).await;
        assert_eq!(third.launched, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
