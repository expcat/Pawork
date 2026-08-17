//! ErrorClassifier：把 Provider/HTTP 错误归一为统一失败分类（ADR-033）。
//!
//! 回退动作依据分类决定：retry same credential / failover credential /
//! fallback model / fallback provider / fallback protocol。规则边界：
//! `Cancelled`、`InvalidRequest`、`ContextTooLarge`、`ProtocolIncompatible`
//! 不得默认触发 credential rotation；`Cancelled` 不降低账号健康度。
//! HTTP status 只是输入，不是最终动作。
//!
//! 本模块为纯类型与默认分类逻辑，不执行网络 IO，不接触 Secret。

use crate::LeaseOutcome;
use pawork_domain::ProviderId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// 失败大类（归一化后的稳定枚举）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// 客户端主动取消（不惩罚健康）。
    Cancelled,
    /// 请求非法（4xx 语义错误，不可重试）。
    InvalidRequest,
    /// 上下文超长（需缩减或换模型，非账号问题）。
    ContextTooLarge,
    /// 客户端协议不兼容（非账号问题）。
    ProtocolIncompatible,
    /// 被限流（可 failover 到其它凭据）。
    RateLimited,
    /// 计费 / 账号被封禁（402 / account blocked；可 failover 到其它账号）。
    BillingBlocked,
    /// 硬配额耗尽（需 failover 账号或等待周期重置）。
    QuotaExceeded,
    /// 软配额告警（接近上限；策略降级 / 告警，不触发 failover）。
    QuotaSoftExceeded,
    /// 鉴权失败（evict 当前凭据，不轮换）。
    AuthInvalid,
    /// 上游 5xx（可 failover）。
    UpstreamError,
    /// 网络层失败（可 failover）。
    Network,
    /// 流式响应中断（可能已有部分输出；可重试但须明确标记）。
    StreamInterrupted,
    /// 未知失败（fail-closed：不据此轮换或惩罚资源）。
    Unknown,
}

/// 失败作用域：定位回退动作的边界。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureScope {
    /// 仅当前请求（retry same credential）。
    Request,
    /// 当前账号/凭据（failover credential）。
    Credential,
    /// 当前账号（failover account；计费封禁等账号级失败）。
    Account,
    /// 当前模型（fallback model）。
    Model,
    /// 当前 Provider（fallback provider）。
    Provider,
    /// 客户端协议（fallback protocol）。
    Protocol,
}

/// 可重试性。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retryability {
    /// 不可重试。
    Never,
    /// 可立即重试。
    Immediate,
    /// 需等待（如 rate limit cool-down）。
    Delayed,
}

/// 对账号健康的影响。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthImpact {
    /// 不影响（取消、客户端错误等）。
    None,
    /// 软降级（累加但不立即摘除）。
    Degraded,
    /// 摘除账号直至恢复。
    Evicted,
}

/// 归一化后的失败分类（ADR-033 的五元组 + `safe_to_failover`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct FailureClassification {
    pub class: FailureClass,
    pub scope: FailureScope,
    pub retryability: Retryability,
    pub health_impact: HealthImpact,
    /// 是否可安全 failover 到另一个凭据/账号。
    pub safe_to_failover: bool,
}

impl FailureClassification {
    /// 映射到 lease 释放结果：`Cancelled` 取消，可 failover 的服务端失败计 `Failed`，
    /// 客户端错误（非法请求/超长/协议不兼容）计 `Released`（不计失败、不惩罚健康）。
    pub fn to_lease_outcome(self) -> LeaseOutcome {
        match self.class {
            FailureClass::Cancelled => LeaseOutcome::Cancelled,
            FailureClass::InvalidRequest
            | FailureClass::ContextTooLarge
            | FailureClass::ProtocolIncompatible
            | FailureClass::Unknown => LeaseOutcome::Released,
            FailureClass::RateLimited
            | FailureClass::BillingBlocked
            | FailureClass::QuotaExceeded
            | FailureClass::QuotaSoftExceeded
            | FailureClass::AuthInvalid
            | FailureClass::UpstreamError
            | FailureClass::Network
            | FailureClass::StreamInterrupted => LeaseOutcome::Failed,
        }
    }
}

/// Provider 特有错误类别（结构化 hint，由 adapter 填写；**禁止携带明文 secret**）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderErrorKind {
    /// 400 中的 account blocked（账号被封禁 / 不可用）。
    AccountBlocked,
    /// 402 Payment Required（计费问题）。
    PaymentRequired,
    /// 配额耗尽；`hard` 区分硬 / 软配额（hard → failover；soft → 策略降级）。
    QuotaExceeded {
        /// `true` = 硬配额（已耗尽）；`false` = 软配额（接近上限）。
        hard: bool,
    },
    /// 429 限流（可携带 scope 语义，如 model / provider）。
    RateLimited,
    /// 协议错误（版本 / 握手 / 解码失败）。
    ProtocolError,
    /// 流式响应中断（部分输出）。
    StreamInterrupted,
    /// 上游 / 网络失败。
    Upstream,
    /// 其它（回退到 HTTP 状态码分类）。
    Other,
}

/// Provider 特有错误信号（脱敏）：错误原文绝不进入本结构。
///
/// `redacted_message` 只能存放 adapter 已脱敏的简短诊断（如 `"account_blocked"`）；
/// `Debug` 输出一律显示 `[redacted]`，防止诊断日志泄漏原文。
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderErrorSignal {
    /// HTTP 状态码（0 表示无状态码，如流中断）。
    pub status: u16,
    /// 结构化错误类别。
    pub kind: ProviderErrorKind,
    /// 服务端 Retry-After（毫秒），供 cooldown 尊重。
    pub retry_after_ms: Option<u64>,
    /// 已脱敏的简短诊断；未脱敏内容不得传入。
    pub redacted_message: Option<String>,
}

impl ProviderErrorSignal {
    /// 以状态码 + 类别构造信号。
    pub fn new(status: u16, kind: ProviderErrorKind) -> Self {
        Self {
            status,
            kind,
            retry_after_ms: None,
            redacted_message: None,
        }
    }

    /// 附加 Retry-After（毫秒）。
    pub fn with_retry_after(mut self, retry_after_ms: u64) -> Self {
        self.retry_after_ms = Some(retry_after_ms);
        self
    }

    /// 附加已脱敏的简短诊断。
    pub fn with_redacted_message(mut self, message: impl Into<String>) -> Self {
        self.redacted_message = Some(message.into());
        self
    }
}

impl fmt::Debug for ProviderErrorSignal {
    /// 脱敏 Debug：不打印 `redacted_message` 内容。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderErrorSignal")
            .field("status", &self.status)
            .field("kind", &self.kind)
            .field("retry_after_ms", &self.retry_after_ms)
            .field("redacted_message", &"[redacted]")
            .finish()
    }
}

/// 错误归一化契约：HTTP status（及 Provider 特有 hint）只是输入，不是最终动作。
pub trait ErrorClassifier: Send + Sync {
    /// 归一化 HTTP 状态码；`provider_hint` 可携带 Provider 特有信号，供实现细分。
    fn classify_http(&self, status: u16, provider_hint: Option<&str>) -> FailureClassification;

    /// 归一化显式取消（永不 failover、永不惩罚健康）。
    fn classify_cancelled(&self) -> FailureClassification {
        FailureClassification {
            class: FailureClass::Cancelled,
            scope: FailureScope::Request,
            retryability: Retryability::Never,
            health_impact: HealthImpact::None,
            safe_to_failover: false,
        }
    }

    /// 归一化协议不兼容（版本 / 握手失败）。
    ///
    /// ADR-033：不得默认触发 credential rotation，也不得盲目轮询所有 credential；
    /// 允许调用方显式降级协议。
    fn classify_protocol_incompatible(&self) -> FailureClassification {
        FailureClassification {
            class: FailureClass::ProtocolIncompatible,
            scope: FailureScope::Protocol,
            retryability: Retryability::Never,
            health_impact: HealthImpact::None,
            safe_to_failover: false,
        }
    }

    /// 归一化流式响应中断（可能已有部分输出）。
    ///
    /// 不是客户端取消：可重试 / 可 failover，但类别明确，调用方自行决定
    /// 是否丢弃部分输出重试。
    fn classify_stream_interrupted(&self) -> FailureClassification {
        FailureClassification {
            class: FailureClass::StreamInterrupted,
            scope: FailureScope::Credential,
            retryability: Retryability::Delayed,
            health_impact: HealthImpact::Degraded,
            safe_to_failover: true,
        }
    }

    /// 归一化结构化 Provider 信号（脱敏）：canonical kind → 分类；
    /// 未知 kind 回退到 HTTP 状态码分类。
    fn classify_signal(&self, signal: &ProviderErrorSignal) -> FailureClassification {
        match signal.kind {
            ProviderErrorKind::AccountBlocked => FailureClassification {
                class: FailureClass::BillingBlocked,
                scope: FailureScope::Account,
                retryability: Retryability::Never,
                health_impact: HealthImpact::Evicted,
                safe_to_failover: true,
            },
            ProviderErrorKind::PaymentRequired => FailureClassification {
                class: FailureClass::BillingBlocked,
                scope: FailureScope::Account,
                retryability: Retryability::Never,
                health_impact: HealthImpact::Evicted,
                safe_to_failover: true,
            },
            ProviderErrorKind::QuotaExceeded { hard: true } => FailureClassification {
                class: FailureClass::QuotaExceeded,
                scope: FailureScope::Account,
                retryability: Retryability::Delayed,
                health_impact: HealthImpact::Degraded,
                safe_to_failover: true,
            },
            ProviderErrorKind::QuotaExceeded { hard: false } => FailureClassification {
                class: FailureClass::QuotaSoftExceeded,
                scope: FailureScope::Model,
                retryability: Retryability::Never,
                health_impact: HealthImpact::Degraded,
                safe_to_failover: false,
            },
            ProviderErrorKind::RateLimited => self.classify_http(429, None),
            ProviderErrorKind::ProtocolError => self.classify_protocol_incompatible(),
            ProviderErrorKind::StreamInterrupted => self.classify_stream_interrupted(),
            ProviderErrorKind::Upstream | ProviderErrorKind::Other => {
                self.classify_http(signal.status, signal.redacted_message.as_deref())
            }
        }
    }
}

/// Provider 特有分类器扩展点（adapter / factory 扩展点）。
///
/// Provider 特例（400 中的 account blocked、402、429 scope、协议错误等）只允许
/// 在这里实现并注册；core **禁止**按 Provider 名分支。返回 `Some` 覆盖默认分类，
/// `None` 回退到默认 HTTP 分类。
pub trait ProviderClassifier: Send + Sync {
    /// 分类一个 Provider 特有错误信号；`None` 表示不覆盖（走默认分类）。
    fn classify(&self, signal: &ProviderErrorSignal) -> Option<FailureClassification>;
}

/// 按 [`ProviderId`] 注册的分类器表；未命中时回退 [`HttpErrorClassifier`]。
///
/// 注册表查找是 sanctioned 扩展点（与 P18-3 factory 的 provider-id registry 一致），
/// 不是 core 内按名称分支。
#[derive(Default)]
pub struct ClassifierRegistry {
    classifiers: HashMap<ProviderId, Arc<dyn ProviderClassifier>>,
}

impl ClassifierRegistry {
    /// 空注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册 Provider 特有分类器。
    pub fn register(&mut self, provider: ProviderId, classifier: Arc<dyn ProviderClassifier>) {
        self.classifiers.insert(provider, classifier);
    }

    /// 分类：优先 Provider 特有分类器，未命中 / 不覆盖时回退默认分类。
    pub fn classify(
        &self,
        provider: &ProviderId,
        signal: &ProviderErrorSignal,
    ) -> FailureClassification {
        self.classifiers
            .get(provider)
            .and_then(|classifier| classifier.classify(signal))
            .unwrap_or_else(|| HttpErrorClassifier.classify_signal(signal))
    }
}

/// 默认 HTTP 分类器：基于状态码的保守归一化。
///
/// 遵守 ADR-033 边界：4xx 客户端错误（400/413）与取消（499）不触发 failover、
/// 不降健康；鉴权失败（401/403）evict 当前凭据但不轮换；429/408/5xx 允许
/// failover 并软降级。
#[derive(Clone, Copy, Debug, Default)]
pub struct HttpErrorClassifier;

impl ErrorClassifier for HttpErrorClassifier {
    fn classify_http(&self, status: u16, _provider_hint: Option<&str>) -> FailureClassification {
        match status {
            // 客户端取消（NGINX 惯例）：不 failover、不降健康。
            499 => FailureClassification {
                class: FailureClass::Cancelled,
                scope: FailureScope::Request,
                retryability: Retryability::Never,
                health_impact: HealthImpact::None,
                safe_to_failover: false,
            },
            // 计费 / 账号被封禁：可 failover 到其它账号，账号健康置 BillingBlocked。
            402 => FailureClassification {
                class: FailureClass::BillingBlocked,
                scope: FailureScope::Account,
                retryability: Retryability::Never,
                health_impact: HealthImpact::Evicted,
                safe_to_failover: true,
            },
            // 鉴权失败：evict 凭据，不轮换（轮换无意义）。
            401 | 403 => FailureClassification {
                class: FailureClass::AuthInvalid,
                scope: FailureScope::Credential,
                retryability: Retryability::Never,
                health_impact: HealthImpact::Evicted,
                safe_to_failover: false,
            },
            // 超时：可 failover，软降级。
            408 => FailureClassification {
                class: FailureClass::Network,
                scope: FailureScope::Credential,
                retryability: Retryability::Immediate,
                health_impact: HealthImpact::Degraded,
                safe_to_failover: true,
            },
            // 上下文超长：换模型/缩减上下文，非账号问题。
            413 => FailureClassification {
                class: FailureClass::ContextTooLarge,
                scope: FailureScope::Request,
                retryability: Retryability::Never,
                health_impact: HealthImpact::None,
                safe_to_failover: false,
            },
            // 限流：可 failover 到其它凭据，软降级，需等待。
            429 => FailureClassification {
                class: FailureClass::RateLimited,
                scope: FailureScope::Credential,
                retryability: Retryability::Delayed,
                health_impact: HealthImpact::Degraded,
                safe_to_failover: true,
            },
            // 其余 4xx 是请求/权限语义错误：默认不得切换 credential。
            // Provider adapter 可通过结构化扩展点把其中的 account-blocked
            // 等语义提升为账号级分类。
            400..=499 => FailureClassification {
                class: FailureClass::InvalidRequest,
                scope: FailureScope::Request,
                retryability: Retryability::Never,
                health_impact: HealthImpact::None,
                safe_to_failover: false,
            },
            // 上游 5xx：默认影响 Provider scope；adapter 可进一步收窄。
            500..=599 => FailureClassification {
                class: FailureClass::UpstreamError,
                scope: FailureScope::Provider,
                retryability: Retryability::Immediate,
                health_impact: HealthImpact::Degraded,
                safe_to_failover: true,
            },
            // 非错误状态或未知信号 fail-closed：不据此轮换或惩罚资源。
            _ => FailureClassification {
                class: FailureClass::Unknown,
                scope: FailureScope::Request,
                retryability: Retryability::Never,
                health_impact: HealthImpact::None,
                safe_to_failover: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::ProviderId;

    fn classifier() -> HttpErrorClassifier {
        HttpErrorClassifier
    }

    #[test]
    fn cancelled_and_client_errors_never_failover_or_penalize() {
        let c = classifier();
        // ADR-033：Cancelled/InvalidRequest/ContextTooLarge/ProtocolIncompatible 不得触发 rotation。
        let cancelled = c.classify_cancelled();
        assert!(!cancelled.safe_to_failover);
        assert_eq!(cancelled.health_impact, HealthImpact::None);
        assert_eq!(cancelled.to_lease_outcome(), LeaseOutcome::Cancelled);

        let bad_request = c.classify_http(400, None);
        assert!(!bad_request.safe_to_failover);
        assert_eq!(bad_request.health_impact, HealthImpact::None);
        assert_eq!(bad_request.to_lease_outcome(), LeaseOutcome::Released);

        let too_large = c.classify_http(413, None);
        assert!(!too_large.safe_to_failover);
        assert_eq!(too_large.to_lease_outcome(), LeaseOutcome::Released);

        let nginx_cancel = c.classify_http(499, None);
        assert!(!nginx_cancel.safe_to_failover);
        assert_eq!(nginx_cancel.to_lease_outcome(), LeaseOutcome::Cancelled);
    }

    #[test]
    fn auth_failure_evicts_but_does_not_rotate() {
        let c = classifier();
        for status in [401, 403] {
            let auth = c.classify_http(status, None);
            assert_eq!(auth.class, FailureClass::AuthInvalid);
            assert!(!auth.safe_to_failover, "auth 必须不轮换");
            assert_eq!(auth.health_impact, HealthImpact::Evicted);
            assert_eq!(auth.to_lease_outcome(), LeaseOutcome::Failed);
        }
    }

    #[test]
    fn server_and_rate_limit_failures_can_failover() {
        let c = classifier();
        let rate_limited = c.classify_http(429, None);
        assert!(rate_limited.safe_to_failover);
        assert_eq!(rate_limited.retryability, Retryability::Delayed);
        assert_eq!(rate_limited.health_impact, HealthImpact::Degraded);

        let timeout = c.classify_http(408, None);
        assert!(timeout.safe_to_failover);

        let upstream = c.classify_http(503, None);
        assert!(upstream.safe_to_failover);
        assert_eq!(upstream.class, FailureClass::UpstreamError);
        assert_eq!(upstream.scope, FailureScope::Provider);

        let unknown = c.classify_http(599, None);
        assert!(unknown.safe_to_failover);
        assert_eq!(unknown.class, FailureClass::UpstreamError);

        let other = c.classify_http(200, None);
        assert_eq!(other.class, FailureClass::Unknown);
        assert!(!other.safe_to_failover);

        for status in [404, 409, 422] {
            let client = c.classify_http(status, None);
            assert_eq!(client.class, FailureClass::InvalidRequest);
            assert!(!client.safe_to_failover);
            assert_eq!(client.health_impact, HealthImpact::None);
        }
    }

    #[test]
    fn payment_required_is_billing_blocked_and_may_failover() {
        let c = classifier();
        let billing = c.classify_http(402, None);
        assert_eq!(billing.class, FailureClass::BillingBlocked);
        assert_eq!(billing.scope, FailureScope::Account);
        assert_eq!(billing.health_impact, HealthImpact::Evicted);
        assert_eq!(billing.retryability, Retryability::Never);
        assert!(billing.safe_to_failover, "计费封禁可 failover 到其它账号");
        assert_eq!(billing.to_lease_outcome(), LeaseOutcome::Failed);
    }

    #[test]
    fn protocol_incompatible_and_stream_interruption_are_distinct() {
        let c = classifier();
        let protocol = c.classify_protocol_incompatible();
        assert_eq!(protocol.class, FailureClass::ProtocolIncompatible);
        assert_eq!(protocol.scope, FailureScope::Protocol);
        assert!(!protocol.safe_to_failover, "协议不兼容不得轮换 credential");
        assert_eq!(protocol.health_impact, HealthImpact::None);
        assert_eq!(protocol.to_lease_outcome(), LeaseOutcome::Released);

        let stream = c.classify_stream_interrupted();
        assert_eq!(stream.class, FailureClass::StreamInterrupted);
        assert!(stream.safe_to_failover);
        assert_eq!(stream.health_impact, HealthImpact::Degraded);
        assert_eq!(stream.to_lease_outcome(), LeaseOutcome::Failed);
    }

    #[test]
    fn signal_maps_canonical_kinds_with_hard_soft_quota_split() {
        let c = classifier();

        let blocked = c.classify_signal(
            &ProviderErrorSignal::new(400, ProviderErrorKind::AccountBlocked)
                .with_redacted_message("account_blocked"),
        );
        assert_eq!(blocked.class, FailureClass::BillingBlocked);
        assert_eq!(blocked.scope, FailureScope::Account);
        assert!(blocked.safe_to_failover);

        let payment = c.classify_signal(&ProviderErrorSignal::new(
            402,
            ProviderErrorKind::PaymentRequired,
        ));
        assert_eq!(payment.class, FailureClass::BillingBlocked);

        let hard = c.classify_signal(&ProviderErrorSignal::new(
            429,
            ProviderErrorKind::QuotaExceeded { hard: true },
        ));
        assert_eq!(hard.class, FailureClass::QuotaExceeded);
        assert_eq!(hard.scope, FailureScope::Account);
        assert!(hard.safe_to_failover);

        let soft = c.classify_signal(&ProviderErrorSignal::new(
            429,
            ProviderErrorKind::QuotaExceeded { hard: false },
        ));
        assert_eq!(soft.class, FailureClass::QuotaSoftExceeded);
        assert_eq!(soft.scope, FailureScope::Model);
        assert!(!soft.safe_to_failover, "软配额不触发 failover");

        let protocol = c.classify_signal(&ProviderErrorSignal::new(
            0,
            ProviderErrorKind::ProtocolError,
        ));
        assert_eq!(protocol.class, FailureClass::ProtocolIncompatible);
        assert!(!protocol.safe_to_failover);

        let stream = c.classify_signal(&ProviderErrorSignal::new(
            0,
            ProviderErrorKind::StreamInterrupted,
        ));
        assert_eq!(stream.class, FailureClass::StreamInterrupted);

        let upstream =
            c.classify_signal(&ProviderErrorSignal::new(503, ProviderErrorKind::Upstream));
        assert_eq!(upstream.class, FailureClass::UpstreamError);
    }

    #[test]
    fn signal_debug_redacts_message() {
        let signal = ProviderErrorSignal::new(400, ProviderErrorKind::AccountBlocked)
            .with_redacted_message("sensitive provider detail");
        let debug = format!("{signal:?}");
        assert!(!debug.contains("sensitive provider detail"));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn registry_dispatches_to_provider_classifier_and_falls_back() {
        struct Blocked400Classifier;
        impl ProviderClassifier for Blocked400Classifier {
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
            ProviderId::new("provider-a"),
            Arc::new(Blocked400Classifier),
        );

        // 已注册 provider：400 + AccountBlocked → 覆盖为 BillingBlocked。
        let covered = registry.classify(
            &ProviderId::new("provider-a"),
            &ProviderErrorSignal::new(400, ProviderErrorKind::AccountBlocked),
        );
        assert_eq!(covered.class, FailureClass::BillingBlocked);

        // 已注册 provider 但 classifier 不覆盖：回退默认分类。
        let fallback = registry.classify(
            &ProviderId::new("provider-a"),
            &ProviderErrorSignal::new(400, ProviderErrorKind::Other),
        );
        assert_eq!(fallback.class, FailureClass::InvalidRequest);

        // 未注册 provider：直接回退默认分类（core 无名称分支）。
        let unknown = registry.classify(
            &ProviderId::new("provider-b"),
            &ProviderErrorSignal::new(402, ProviderErrorKind::PaymentRequired),
        );
        assert_eq!(unknown.class, FailureClass::BillingBlocked);
    }

    #[test]
    fn classification_round_trips_with_snake_case_and_rejects_unknowns() {
        let classification = classifier().classify_http(429, None);
        let json = serde_json::to_value(classification).expect("serialize classification");
        assert_eq!(json["class"], "rate_limited");
        assert_eq!(json["scope"], "credential");
        assert_eq!(json["retryability"], "delayed");
        assert_eq!(json["health_impact"], "degraded");
        assert_eq!(
            serde_json::from_value::<FailureClassification>(json.clone())
                .expect("decode classification"),
            classification
        );

        let mut unknown_variant = json.clone();
        unknown_variant["class"] = serde_json::json!("future_failure");
        assert!(serde_json::from_value::<FailureClassification>(unknown_variant).is_err());

        let mut unknown_field = json;
        unknown_field["future_field"] = serde_json::json!(true);
        assert!(serde_json::from_value::<FailureClassification>(unknown_field).is_err());
    }
}
