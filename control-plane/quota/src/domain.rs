//! Quota canonical domain. This module is pure data and never owns credentials.

use pawork_domain::{ModelId, ProviderId, TenantId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::AdapterKind;

/// Opaque provider-account identifier. It is not a secret.
///
/// Re-exported from [`pawork_domain`]; serde remains a JSON string.
pub use pawork_domain::AccountId;

/// Isolation key for every quota read and cache entry.
///
/// Tenant, account and provider are mandatory. Model is optional because many
/// official billing APIs only expose account-wide data. `credential_id` is an
/// opaque metadata identifier used to distinguish multiple credentials for one
/// account; it is never the credential value.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QuotaScope {
    pub tenant_id: TenantId,
    pub account_id: AccountId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    pub provider_id: ProviderId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<ModelId>,
}

impl QuotaScope {
    pub fn new(
        tenant_id: TenantId,
        account_id: AccountId,
        provider_id: ProviderId,
        model_id: Option<ModelId>,
    ) -> Self {
        Self {
            tenant_id,
            account_id,
            credential_id: None,
            provider_id,
            model_id,
        }
    }

    pub fn with_credential_id(mut self, credential_id: impl Into<String>) -> Self {
        self.credential_id = Some(credential_id.into());
        self
    }
}

/// Canonical quota window. Unsupported windows must remain explicit errors.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum QuotaWindow {
    #[default]
    Overall,
    Rolling5h,
    Weekly,
    Monthly,
}

/// Unit of `used`, `limit`, and `remaining`.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuotaUnit {
    #[default]
    Count,
    Token,
    /// Monetary values are integer micros in the named ISO-4217 currency.
    Cost {
        currency: String,
    },
}

/// A non-negative canonical amount.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum QuotaMeasure {
    Exact(u64),
    Infinite,
    #[default]
    Unknown,
}

impl QuotaMeasure {
    pub const fn exact(value: u64) -> Self {
        Self::Exact(value)
    }

    pub const fn infinite() -> Self {
        Self::Infinite
    }

    pub const fn unknown() -> Self {
        Self::Unknown
    }

    pub const fn exact_value(self) -> Option<u64> {
        match self {
            Self::Exact(value) => Some(value),
            Self::Infinite | Self::Unknown => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaValues {
    pub used: QuotaMeasure,
    pub limit: QuotaMeasure,
    pub remaining: QuotaMeasure,
}

impl QuotaValues {
    pub const fn new(used: QuotaMeasure, limit: QuotaMeasure, remaining: QuotaMeasure) -> Self {
        Self {
            used,
            limit,
            remaining,
        }
    }
}

/// Confidence and source priority: exact > derived > scraped.
///
/// Default is the lowest-trust [`Scraped`](Self::Scraped): any path that does
/// not explicitly pick a confidence must never claim `Exact` by accident.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Exact,
    Derived,
    #[default]
    Scraped,
}

impl Confidence {
    pub const fn priority(self) -> u8 {
        match self {
            Self::Exact => 3,
            Self::Derived => 2,
            Self::Scraped => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuotaReset {
    Absolute {
        at: Timestamp,
        uncertain: bool,
    },
    Relative {
        after_secs: u64,
        observed_at: Timestamp,
        uncertain: bool,
    },
    #[default]
    Unknown,
}

/// Safe, user-visible provenance. Endpoint values must omit query strings.
///
/// Use [`QuotaProvenance::with_endpoint`] to record endpoints: it canonicalizes
/// the value (strips query/fragment, rejects empty input) so credentials or
/// tokens embedded in a URL never leak into provenance or serialized events.
/// Fields stay public; the canonical helper is the supported construction path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaProvenance {
    pub adapter_kind: AdapterKind,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    pub fetched_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_version: Option<String>,
    #[serde(default)]
    pub stale: bool,
}

impl QuotaProvenance {
    pub fn new(
        adapter_kind: AdapterKind,
        source: impl Into<String>,
        fetched_at: Timestamp,
    ) -> Self {
        Self {
            adapter_kind,
            source: source.into(),
            endpoint: None,
            fetched_at,
            observed_at: None,
            selector_version: None,
            stale: false,
        }
    }

    /// 记录 canonical endpoint：去除 query 与 fragment，异常输入不泄漏。
    ///
    /// 清洗后为空（如纯 query/fragment、空白输入）则保持 `None`。
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Self::canonical_endpoint(&endpoint.into());
        self
    }

    /// 将原始 endpoint 清洗为 canonical 形式（实现见 [`crate::util::canonical_endpoint`]）：
    ///
    /// - 去除首尾空白；
    /// - 截断首个 `?`（query）与 `#`（fragment）之前的部分；
    /// - 结果为空白则返回 `None`，异常输入不会把 query/fragment 带入结果。
    pub fn canonical_endpoint(raw: &str) -> Option<String> {
        crate::util::canonical_endpoint(raw)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaRequest {
    pub scope: QuotaScope,
    pub window: QuotaWindow,
    pub unit: QuotaUnit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub scope: QuotaScope,
    pub window: QuotaWindow,
    pub unit: QuotaUnit,
    pub values: QuotaValues,
    pub reset: QuotaReset,
    pub confidence: Confidence,
    pub provenance: QuotaProvenance,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(tenant: &str, account: &str) -> QuotaScope {
        QuotaScope::new(
            TenantId::new(tenant),
            AccountId::new(account),
            ProviderId::new("anthropic"),
            Some(ModelId::new("claude-opus")),
        )
    }

    #[test]
    fn scope_key_enforces_tenant_and_account_isolation() {
        assert_ne!(
            scope("tenant-a", "account-1"),
            scope("tenant-b", "account-1")
        );
        assert_ne!(
            scope("tenant-a", "account-1"),
            scope("tenant-a", "account-2")
        );
    }

    #[test]
    fn confidence_priority_is_explicit() {
        assert!(Confidence::Exact.priority() > Confidence::Derived.priority());
        assert!(Confidence::Derived.priority() > Confidence::Scraped.priority());
    }

    #[test]
    fn confidence_default_is_lowest_trust_scraped() {
        assert_eq!(Confidence::default(), Confidence::Scraped);
    }

    #[test]
    fn canonical_endpoint_strips_query_and_fragment() {
        assert_eq!(
            QuotaProvenance::canonical_endpoint(
                "https://api.example.com/v1/usage?api_key=sk-secret&page=2#frag"
            )
            .as_deref(),
            Some("https://api.example.com/v1/usage")
        );
        assert_eq!(
            QuotaProvenance::canonical_endpoint("https://console.example.com/quota#overview")
                .as_deref(),
            Some("https://console.example.com/quota")
        );
    }

    #[test]
    fn canonical_endpoint_truncates_at_first_marker() {
        assert_eq!(
            QuotaProvenance::canonical_endpoint("https://x/y?token=abc?more").as_deref(),
            Some("https://x/y")
        );
        assert_eq!(
            QuotaProvenance::canonical_endpoint("https://x/y#frag?token=secret").as_deref(),
            Some("https://x/y")
        );
        assert_eq!(
            QuotaProvenance::canonical_endpoint("https://x/?a=b").as_deref(),
            Some("https://x/")
        );
    }

    #[test]
    fn canonical_endpoint_rejects_abnormal_input_without_leak() {
        assert_eq!(QuotaProvenance::canonical_endpoint(""), None);
        assert_eq!(QuotaProvenance::canonical_endpoint("   "), None);
        assert_eq!(QuotaProvenance::canonical_endpoint("#fragment-only"), None);
        assert_eq!(QuotaProvenance::canonical_endpoint("?token=secret"), None);
        assert_eq!(
            QuotaProvenance::canonical_endpoint("  https://x/y?token=secret  ").as_deref(),
            Some("https://x/y")
        );
    }

    #[test]
    fn provenance_with_endpoint_never_serializes_query_or_fragment() {
        let ts = Timestamp::from_unix_millis(1_700_000_000_000);
        let provenance = QuotaProvenance::new(AdapterKind::ApiKeyApi, "anthropic.admin", ts)
            .with_endpoint("https://api.example.com/v1/usage?api_key=sk-secret#top");
        assert_eq!(
            provenance.endpoint.as_deref(),
            Some("https://api.example.com/v1/usage")
        );
        let json = serde_json::to_string(&provenance).expect("serialize provenance");
        assert!(!json.contains("sk-secret"));
        assert!(!json.contains("?"));
        assert!(!json.contains("#"));

        let empty = QuotaProvenance::new(AdapterKind::ApiKeyApi, "anthropic.admin", ts)
            .with_endpoint("?token=sk-secret");
        assert_eq!(empty.endpoint, None);
        assert!(!serde_json::to_string(&empty).unwrap().contains("sk-secret"));
    }

    #[test]
    fn snapshot_round_trip_has_currency_and_no_secret() {
        let ts = Timestamp::from_unix_millis(1_700_000_000_000);
        let snapshot = QuotaSnapshot {
            scope: scope("tenant-a", "account-1").with_credential_id("cred-1"),
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Cost {
                currency: "USD".into(),
            },
            values: QuotaValues::new(
                QuotaMeasure::exact(3_000_000),
                QuotaMeasure::exact(10_000_000),
                QuotaMeasure::exact(7_000_000),
            ),
            reset: QuotaReset::Absolute {
                at: ts,
                uncertain: false,
            },
            confidence: Confidence::Exact,
            provenance: QuotaProvenance::new(AdapterKind::ApiKeyApi, "anthropic.admin", ts),
        };
        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        assert!(json.contains("USD"));
        assert!(!json.contains("sk-"));
        assert!(!json.contains("secret"));
        assert_eq!(
            serde_json::from_str::<QuotaSnapshot>(&json).expect("deserialize snapshot"),
            snapshot
        );
    }
}
