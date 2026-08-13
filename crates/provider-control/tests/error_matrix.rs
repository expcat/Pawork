//! P18-5 错误矩阵 contract tests。
//!
//! 覆盖：401 refresh-once、402、provider-specific 400、429 有 / 无 Retry-After、
//! QuotaExceeded（hard/soft）、5xx、cancel、context-too-large、
//! protocol incompatible、stream interruption。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_domain::{AccountId, CredentialId, ModelId, ProviderId, Timestamp};
use provider_control::{
    BackoffPolicy, CircuitConfig, CircuitState, ClassifierRegistry, CooldownKey, ErrorClassifier,
    FailureClass, FailureClassification, FailureContext, FailureScope, HealthImpact, HealthRuntime,
    HealthState, HttpErrorClassifier, ProviderClassifier, ProviderErrorKind, ProviderErrorSignal,
    Retryability,
};

/// 可变时钟：推进时间验证冷却到期与断路器半开。
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

impl provider_control::account::Clock for MutableClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(self.0.load(Ordering::Relaxed))
    }
}

fn context() -> FailureContext {
    FailureContext::new(
        Some(AccountId::new("acct-a")),
        Some(CredentialId::new("cred-a")),
        Some(ModelId::new("model-a")),
        Some(ProviderId::new("prov-a")),
    )
}

fn classify(status: u16) -> FailureClassification {
    HttpErrorClassifier.classify_http(status, None)
}

fn runtime_with(clock: &MutableClock) -> HealthRuntime {
    HealthRuntime::with_config(
        Arc::new(clock.clone()),
        BackoffPolicy::default(),
        CircuitConfig {
            failure_threshold: 3,
            open_timeout_ms: 1_000,
            half_open_max_probes: 1,
            success_threshold: 2,
        },
    )
}

/// 401：refresh-once——第一次刷新、第二次冷却凭据；不切号、账号不降级。
#[test]
fn matrix_401_refresh_once_without_account_rotation() {
    let clock = MutableClock::new(1_000);
    let mut runtime = runtime_with(&clock);
    let ctx = context();

    let classification = classify(401);
    assert_eq!(classification.class, FailureClass::AuthInvalid);
    assert!(!classification.safe_to_failover, "认证失败不得自动错误切号");
    assert_eq!(classification.health_impact, HealthImpact::Evicted);

    runtime.record_failure(&ctx, classification, None);
    assert!(runtime.refresh_eligible(&CredentialId::new("cred-a")));
    assert_eq!(
        runtime.account_state(&AccountId::new("acct-a")),
        HealthState::Healthy
    );

    runtime.record_failure(&ctx, classification, Some(5_000));
    assert!(!runtime.refresh_eligible(&CredentialId::new("cred-a")));
    assert_eq!(
        runtime.cooldown_remaining_ms(&CooldownKey::credential("cred-a")),
        5_000
    );
    assert_eq!(
        runtime.account_state(&AccountId::new("acct-a")),
        HealthState::Healthy,
        "凭据级鉴权失败不得惩罚账号"
    );
}

/// 402：BillingBlocked——账号封禁、允许 failover、Retry-After 不适用。
#[test]
fn matrix_402_billing_blocked_failover_allowed() {
    let clock = MutableClock::new(1_000);
    let mut runtime = runtime_with(&clock);
    let ctx = context();

    let classification = classify(402);
    assert_eq!(classification.class, FailureClass::BillingBlocked);
    assert_eq!(classification.scope, FailureScope::Account);
    assert!(classification.safe_to_failover);
    assert_eq!(classification.retryability, Retryability::Never);

    runtime.record_failure(&ctx, classification, None);
    assert_eq!(
        runtime.account_state(&AccountId::new("acct-a")),
        HealthState::BillingBlocked
    );
    assert!(!runtime.is_admissible(&ctx));
}

/// Provider-specific 400（account blocked）：只经 adapter 扩展点覆盖，core 无名称分支。
#[test]
fn matrix_provider_specific_400_account_blocked_via_extension_point() {
    struct AccountBlockedClassifier;
    impl ProviderClassifier for AccountBlockedClassifier {
        fn classify(&self, signal: &ProviderErrorSignal) -> Option<FailureClassification> {
            if signal.status == 400 && signal.kind == ProviderErrorKind::AccountBlocked {
                Some(FailureClassification {
                    class: FailureClass::BillingBlocked,
                    scope: FailureScope::Account,
                    retryability: Retryability::Never,
                    health_impact: HealthImpact::Evicted,
                    safe_to_failover: true,
                })
            } else {
                None
            }
        }
    }

    let mut registry = ClassifierRegistry::new();
    registry.register(
        ProviderId::new("prov-a"),
        Arc::new(AccountBlockedClassifier),
    );

    let signal = ProviderErrorSignal::new(400, ProviderErrorKind::AccountBlocked)
        .with_redacted_message("account_blocked");
    let covered = registry.classify(&ProviderId::new("prov-a"), &signal);
    assert_eq!(covered.class, FailureClass::BillingBlocked);
    assert!(covered.safe_to_failover);

    // 未注册 provider 的 400 保持默认 InvalidRequest（不轮换、不惩罚）。
    let plain = registry.classify(
        &ProviderId::new("prov-unknown"),
        &ProviderErrorSignal::new(400, ProviderErrorKind::Other),
    );
    assert_eq!(plain.class, FailureClass::InvalidRequest);
    assert!(!plain.safe_to_failover);

    // 注册表是唯一入口：core 不存在按名称分支（此处仅证明查找语义）。
    let signal_debug = format!("{signal:?}");
    assert!(!signal_debug.contains("account_blocked"), "诊断必须脱敏");
}

/// 429：有 Retry-After 尊重之；无 Retry-After 用有界退避；scope 隔离。
#[test]
fn matrix_429_retry_after_and_backoff_fallback() {
    let clock = MutableClock::new(1_000);
    let mut runtime = runtime_with(&clock);
    let ctx = context();

    let classification = classify(429);
    assert_eq!(classification.class, FailureClass::RateLimited);
    assert!(classification.safe_to_failover);
    assert_eq!(classification.retryability, Retryability::Delayed);

    // 有 Retry-After：精确遵守。
    runtime.record_failure(&ctx, classification, Some(4_000));
    assert_eq!(
        runtime.cooldown_remaining_ms(&CooldownKey::credential("cred-a")),
        4_000
    );
    assert!(!runtime.is_admissible(&ctx));
    clock.advance(4_000);
    assert!(runtime.is_admissible(&ctx));

    // 无 Retry-After：有界退避（≥ base，≤ cap）。
    runtime.record_failure(&ctx, classification, None);
    let remaining = runtime.cooldown_remaining_ms(&CooldownKey::credential("cred-a"));
    assert!(
        (BackoffPolicy::DEFAULT_BASE_MS..=BackoffPolicy::DEFAULT_CAP_MS).contains(&remaining),
        "退避必须落在有界区间: {remaining}"
    );
}

/// 429 模型 scope：只冷却模型，不波及其他 scope。
#[test]
fn matrix_429_scope_aware_cooldown() {
    let clock = MutableClock::new(1_000);
    let mut runtime = runtime_with(&clock);
    let ctx = context();

    runtime.record_failure(
        &ctx,
        FailureClassification {
            class: FailureClass::RateLimited,
            scope: FailureScope::Model,
            retryability: Retryability::Delayed,
            health_impact: HealthImpact::Degraded,
            safe_to_failover: true,
        },
        Some(3_000),
    );

    assert_eq!(
        runtime.cooldown_remaining_ms(&CooldownKey::model("model-a")),
        3_000
    );
    assert_eq!(
        runtime.cooldown_remaining_ms(&CooldownKey::credential("cred-a")),
        0,
        "模型 429 不得冷却凭据"
    );
    assert_eq!(
        runtime.cooldown_remaining_ms(&CooldownKey::account("acct-a")),
        0,
        "模型 429 不得冷却账号"
    );
}

/// QuotaExceeded：hard → 账号 failover；soft → 只降级、不冷却。
#[test]
fn matrix_quota_exceeded_hard_vs_soft() {
    let clock = MutableClock::new(1_000);
    let mut runtime = runtime_with(&clock);
    let ctx = context();

    let hard = HttpErrorClassifier.classify_signal(&ProviderErrorSignal::new(
        429,
        ProviderErrorKind::QuotaExceeded { hard: true },
    ));
    assert_eq!(hard.class, FailureClass::QuotaExceeded);
    assert!(hard.safe_to_failover);
    runtime.record_failure(&ctx, hard, None);
    assert!(runtime.cooldown_remaining_ms(&CooldownKey::account("acct-a")) > 0);
    assert_eq!(
        runtime.account_state(&AccountId::new("acct-a")),
        HealthState::CoolingDown
    );
    clock.advance(BackoffPolicy::DEFAULT_BASE_MS);
    assert!(runtime.is_admissible(&ctx));

    let soft = HttpErrorClassifier.classify_signal(&ProviderErrorSignal::new(
        429,
        ProviderErrorKind::QuotaExceeded { hard: false },
    ));
    assert_eq!(soft.class, FailureClass::QuotaSoftExceeded);
    assert!(!soft.safe_to_failover, "软配额不触发 failover");
    runtime.record_failure(&ctx, soft, None);
    assert_eq!(
        runtime.scope_state(&CooldownKey::model("model-a")),
        HealthState::Degraded,
        "软配额只降级"
    );
    assert_eq!(
        runtime.account_state(&AccountId::new("acct-a")),
        HealthState::Healthy
    );
    assert_eq!(
        runtime.cooldown_remaining_ms(&CooldownKey::account("acct-a")),
        0,
        "软配额不与 RateLimited 混淆"
    );
}

/// 5xx：bounded retry + circuit breaker——跳闸、半开探针、成功复原。
#[test]
fn matrix_5xx_bounded_retry_with_circuit_breaker() {
    let clock = MutableClock::new(1_000);
    let mut runtime = runtime_with(&clock);
    let ctx = context();

    let classification = classify(503);
    assert_eq!(classification.class, FailureClass::UpstreamError);
    assert!(classification.safe_to_failover);

    for _ in 0..3 {
        runtime.record_failure(&ctx, classification, None);
    }
    assert_eq!(
        runtime.circuit_state_for(&CooldownKey::provider("prov-a")),
        CircuitState::Open
    );
    assert!(
        !runtime.is_admissible(&ctx),
        "Open 拒绝请求（bounded retry）"
    );

    clock.advance(1_000);
    assert!(runtime.is_admissible(&ctx), "半开探针放行");
    assert!(!runtime.is_admissible(&ctx), "探针数受限");

    runtime.record_success(&ctx);
    assert!(runtime.is_admissible(&ctx), "成功探针释放并发槽位");
    runtime.record_success(&ctx);
    assert_eq!(
        runtime.circuit_state_for(&CooldownKey::provider("prov-a")),
        CircuitState::Closed,
        "连续成功复原"
    );
    assert!(runtime.is_admissible(&ctx));
}

/// 取消 / 客户端错误 / context-too-large：不触发账号轮换、不惩罚健康。
#[test]
fn matrix_cancel_and_client_errors_never_penalize() {
    let clock = MutableClock::new(1_000);
    let mut runtime = runtime_with(&clock);
    let ctx = context();

    runtime.record_failure(&ctx, HttpErrorClassifier.classify_cancelled(), None);
    runtime.record_failure(&ctx, classify(499), None);
    runtime.record_failure(&ctx, classify(400), None);
    runtime.record_failure(&ctx, classify(413), None);
    runtime.record_cancelled(&ctx);

    assert_eq!(
        runtime.account_state(&AccountId::new("acct-a")),
        HealthState::Healthy
    );
    assert!(runtime.is_admissible(&ctx));
    assert_eq!(runtime.cooldown_len(), 0, "无任何冷却条目");
    assert_eq!(
        runtime.circuit_state(&AccountId::new("acct-a")),
        CircuitState::Closed
    );
    assert!(runtime.refresh_eligible(&CredentialId::new("cred-a")));
}

/// ProtocolIncompatible：显式失败 / 协议降级，不盲目轮询 credential。
#[test]
fn matrix_protocol_incompatible_no_credential_polling() {
    let clock = MutableClock::new(1_000);
    let mut runtime = runtime_with(&clock);
    let ctx = context();

    let classification = HttpErrorClassifier.classify_protocol_incompatible();
    assert_eq!(classification.class, FailureClass::ProtocolIncompatible);
    assert_eq!(classification.scope, FailureScope::Protocol);
    assert!(
        !classification.safe_to_failover,
        "不得轮换 / 轮询 credential"
    );

    runtime.record_failure(&ctx, classification, None);
    assert_eq!(
        runtime.account_state(&AccountId::new("acct-a")),
        HealthState::Healthy
    );
    assert_eq!(runtime.cooldown_len(), 0);
    assert!(runtime.is_admissible(&ctx));
}

/// Stream interruption：明确类别（非取消）、可重试 / failover、部分输出由调用方处置。
#[test]
fn matrix_stream_interruption_is_not_cancel_and_may_failover() {
    let clock = MutableClock::new(1_000);
    let mut runtime = runtime_with(&clock);
    let ctx = context();

    let classification = HttpErrorClassifier.classify_stream_interrupted();
    assert_eq!(classification.class, FailureClass::StreamInterrupted);
    assert_ne!(classification.class, FailureClass::Cancelled);
    assert!(classification.safe_to_failover);
    assert_eq!(classification.health_impact, HealthImpact::Degraded);

    runtime.record_failure(&ctx, classification, None);
    assert_eq!(
        runtime.account_state(&AccountId::new("acct-a")),
        HealthState::Healthy,
        "凭据级流中断不得污染账号健康"
    );
    assert_eq!(
        runtime.scope_state(&CooldownKey::credential("cred-a")),
        HealthState::CoolingDown
    );
    assert_eq!(
        runtime.circuit_state_for(&CooldownKey::credential("cred-a")),
        CircuitState::Closed,
        "单次中断未跳闸"
    );
    assert!(
        !runtime.is_admissible(&ctx),
        "凭据 scope 冷却期间不可准入（有界退避）"
    );
    clock.advance(BackoffPolicy::DEFAULT_BASE_MS);
    assert!(
        runtime.is_admissible(&ctx),
        "冷却到期后降级但仍可准入（调用方决定是否重试）"
    );

    // 信号路径同样归一化。
    let via_signal = HttpErrorClassifier.classify_signal(&ProviderErrorSignal::new(
        0,
        ProviderErrorKind::StreamInterrupted,
    ));
    assert_eq!(via_signal.class, FailureClass::StreamInterrupted);
}
