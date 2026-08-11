//! 六家 Provider 的配额能力矩阵。
//!
//! [`capability_matrix`] 是静态声明：对每个 Provider 的每个 (window, unit) 组合，
//! 说明它的读数来源是 Exact / Derived / Scraped / Unsupported，以及所需凭证类型。
//! 它不发起任何网络请求，供聚合层在调用真实适配器前做能力发现与降级决策。
//!
//! 能力声明以事实源为准（见 brief）：只有官方 billing/quota 接口能给出 Exact。

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::{AdapterKind, Confidence, QuotaUnit, QuotaWindow};
use agent_domain::{ProviderId, Timestamp};

/// 单条能力声明。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capability {
    pub provider: ProviderId,
    pub window: QuotaWindow,
    pub unit: QuotaUnit,
    /// 实际能拿到的最高可信度；Unsupported 表示完全无读数。
    pub best_confidence: ConfidenceOrUnsupported,
    /// 提供该读数的适配器获取方式。
    pub adapter_kind: AdapterKind,
    /// 所需凭证类型（用于上层解析与降级提示）。
    pub credential_kind: CredentialKindHint,
    /// 远端名义窗口（用于 provenance/审计说明，非隔离键）。
    pub note: &'static str,
}

/// 能力上限：Exact / Derived / Scraped，或完全 Unsupported。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConfidenceOrUnsupported {
    Exact,
    Derived,
    Scraped,
    Unsupported,
}

impl ConfidenceOrUnsupported {
    pub fn to_confidence(self) -> Option<Confidence> {
        match self {
            Self::Exact => Some(Confidence::Exact),
            Self::Derived => Some(Confidence::Derived),
            Self::Scraped => Some(Confidence::Scraped),
            Self::Unsupported => None,
        }
    }
}

/// 所需凭证类型提示（与 provider_api::CredentialKind 对应，但保持解耦）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CredentialKindHint {
    /// 普通 inference API key（无法读取远端额度）。
    InferenceApiKey,
    /// 组织/账户管理 API key（可读取远端 billing）。
    AdminApiKey,
    /// OAuth bearer token（需 refresh）。
    OAuthBearer,
    /// 控制台 Cookie 登录会话（WebScrape 回退；仅进程内消费，不持久化、不进日志）。
    CookieSession,
    /// 云账号 AccessKey pair（HMAC 签名）。
    AccessKeyPair,
    /// 无凭证（公开页面抓取，仅作 Scraped 回退）。
    None,
}

/// 由 Provider 名（canonical lowercase）索引的能力表。
pub fn capability_matrix() -> &'static HashMap<String, Vec<Capability>> {
    static MATRIX: OnceLock<HashMap<String, Vec<Capability>>> = OnceLock::new();
    MATRIX.get_or_init(|| {
        let all = all_capabilities();
        let mut map = HashMap::new();
        for cap in all {
            map.entry(cap.provider.to_string())
                .or_insert_with(Vec::new)
                .push(cap);
        }
        map
    })
}

/// 查询某 Provider 在指定 (window, unit) 下的最佳能力。
pub fn capability_for(
    provider: &ProviderId,
    window: QuotaWindow,
    unit: &QuotaUnit,
) -> Option<&'static Capability> {
    let matrix = capability_matrix();
    matrix
        .get(&provider.to_string())?
        .iter()
        .find(|c| c.window == window && &c.unit == unit)
}

/// 用作账本派生时的 observed_at 戳。
pub fn now() -> Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    Timestamp::from_unix_millis(ms)
}

fn cap(
    provider: &str,
    window: QuotaWindow,
    unit: QuotaUnit,
    best: ConfidenceOrUnsupported,
    kind: AdapterKind,
    cred: CredentialKindHint,
    note: &'static str,
) -> Capability {
    Capability {
        provider: ProviderId::new(provider),
        window,
        unit,
        best_confidence: best,
        adapter_kind: kind,
        credential_kind: cred,
        note,
    }
}

/// 六家 Provider 的完整能力矩阵。每条对应一个 (provider, window, unit) 组合。
///
/// 构建原则：
/// - 只把有官方 billing/quota 接口的 (window, unit) 标为 Exact；
/// - 没有官方读数的标 Unsupported（不编造额度），仅 Zhipu 提供可选 Scraped 回退；
/// - DashScope-only 用量无远端读数，统一 Unsupported。
#[allow(clippy::vec_init_then_push)]
pub fn all_capabilities() -> Vec<Capability> {
    let usd = || QuotaUnit::Cost {
        currency: "USD".to_string(),
    };
    let cny = || QuotaUnit::Cost {
        currency: "CNY".to_string(),
    };
    let count = || QuotaUnit::Count;
    let token = || QuotaUnit::Token;

    let mut v: Vec<Capability> = Vec::new();

    // OpenAI：Admin key，组织级 monthly spend limit + costs，USD。
    v.push(cap(
        "openai",
        QuotaWindow::Monthly,
        usd(),
        ConfidenceOrUnsupported::Exact,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AdminApiKey,
        "organization spend_limit (cents) + costs (USD); resets at next month 00:00 UTC",
    ));
    v.push(cap(
        "openai",
        QuotaWindow::Monthly,
        token(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AdminApiKey,
        "no official token-quota API",
    ));
    v.push(cap(
        "openai",
        QuotaWindow::Overall,
        usd(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AdminApiKey,
        "no overall balance API",
    ));
    v.push(cap(
        "openai",
        QuotaWindow::Rolling5h,
        count(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AdminApiKey,
        "no rate-limit quota API",
    ));

    // Anthropic：Admin key read:spend_limits，组织级 monthly，USD。
    v.push(cap(
        "anthropic",
        QuotaWindow::Monthly,
        usd(),
        ConfidenceOrUnsupported::Exact,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AdminApiKey,
        "organizations spend_limits/effective: data[] scope={type:user,user_id}; amount nullable decimal USD cents (explicit null only = no hard limit, missing field = parse error); period monthly, resets 1st 00:00 UTC; Enterprise usage credits only",
    ));
    v.push(cap(
        "anthropic",
        QuotaWindow::Rolling5h,
        count(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AdminApiKey,
        "consumer 5h limit has no public API",
    ));
    v.push(cap(
        "anthropic",
        QuotaWindow::Weekly,
        count(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AdminApiKey,
        "consumer weekly limit has no public API",
    ));
    v.push(cap(
        "anthropic",
        QuotaWindow::Overall,
        usd(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AdminApiKey,
        "no overall balance API",
    ));

    // xAI：management key，team 级 prepaid overall（USD）+ postpaid monthly（USD）。
    v.push(cap(
        "xai",
        QuotaWindow::Overall,
        usd(),
        ConfidenceOrUnsupported::Exact,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AdminApiKey,
        "teams prepaid balance total.val (cents)",
    ));
    v.push(cap(
        "xai",
        QuotaWindow::Monthly,
        usd(),
        ConfidenceOrUnsupported::Exact,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AdminApiKey,
        "teams postpaid spending-limits (limit cents) + invoice-preview total.val (used cents)",
    ));
    v.push(cap(
        "xai",
        QuotaWindow::Rolling5h,
        count(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AdminApiKey,
        "no rate-limit quota API",
    ));

    // Zhipu/BigModel：无官方公开 usage/quota 端点；仅 Coding Plan 控制台 Rolling5h/Weekly
    // 用量计数（非付费额度）可经 WebScrape 拿到，需控制台 Cookie 登录会话，置信度 Scraped。
    v.push(cap(
        "zhipu",
        QuotaWindow::Rolling5h,
        count(),
        ConfidenceOrUnsupported::Scraped,
        AdapterKind::WebScrape,
        CredentialKindHint::CookieSession,
        "coding-plan console scrape (counts, cookie session required)",
    ));
    v.push(cap(
        "zhipu",
        QuotaWindow::Weekly,
        count(),
        ConfidenceOrUnsupported::Scraped,
        AdapterKind::WebScrape,
        CredentialKindHint::CookieSession,
        "coding-plan console scrape (counts, cookie session required)",
    ));
    // Zhipu 付费额度（Cost，任意窗口）无任何读数来源 → Unsupported。
    v.push(cap(
        "zhipu",
        QuotaWindow::Overall,
        cny(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::WebScrape,
        CredentialKindHint::None,
        "no official API; cost balance not exposed",
    ));
    v.push(cap(
        "zhipu",
        QuotaWindow::Monthly,
        cny(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::WebScrape,
        CredentialKindHint::None,
        "no official API",
    ));

    // Qwen/DashScope：inference key 无余额 API；Alibaba BSS 给的是账户整体余额。
    v.push(cap(
        "qwen",
        QuotaWindow::Overall,
        cny(),
        ConfidenceOrUnsupported::Exact,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AccessKeyPair,
        "Alibaba BSS QueryAccountBalance (account-wide, not DashScope-only)",
    ));
    v.push(cap(
        "qwen",
        QuotaWindow::Monthly,
        cny(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AccessKeyPair,
        "no DashScope-scoped monthly API",
    ));
    v.push(cap(
        "qwen",
        QuotaWindow::Overall,
        token(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::AccessKeyPair,
        "no token-quota API",
    ));

    // Moonshot/Kimi：bearer key，账户整体余额（CNY）。
    v.push(cap(
        "moonshot",
        QuotaWindow::Overall,
        cny(),
        ConfidenceOrUnsupported::Exact,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::InferenceApiKey,
        "users/me/balance",
    ));
    v.push(cap(
        "moonshot",
        QuotaWindow::Monthly,
        cny(),
        ConfidenceOrUnsupported::Unsupported,
        AdapterKind::ApiKeyApi,
        CredentialKindHint::InferenceApiKey,
        "no monthly scoped API",
    ));

    let _ = (count, token);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_covers_six_providers() {
        let m = capability_matrix();
        for p in ["openai", "anthropic", "xai", "zhipu", "qwen", "moonshot"] {
            assert!(m.contains_key(p), "missing provider {p}");
            assert!(!m[p].is_empty(), "empty caps for {p}");
        }
    }

    #[test]
    fn openai_monthly_cost_is_exact_admin() {
        let c = capability_for(
            &ProviderId::new("openai"),
            QuotaWindow::Monthly,
            &QuotaUnit::Cost {
                currency: "USD".into(),
            },
        )
        .expect("openai monthly cost");
        assert_eq!(c.best_confidence, ConfidenceOrUnsupported::Exact);
        assert_eq!(c.adapter_kind, AdapterKind::ApiKeyApi);
        assert_eq!(c.credential_kind, CredentialKindHint::AdminApiKey);
    }

    #[test]
    fn zhipu_only_supports_scraped_overall() {
        let c = capability_for(
            &ProviderId::new("zhipu"),
            QuotaWindow::Overall,
            &QuotaUnit::Cost {
                currency: "CNY".into(),
            },
        )
        .expect("zhipu overall");
        // 付费额度（Cost）无读数来源 → Unsupported（Coding Plan 才是 Scraped）。
        assert_eq!(c.best_confidence, ConfidenceOrUnsupported::Unsupported);
        assert_eq!(c.adapter_kind, AdapterKind::WebScrape);
        assert_eq!(c.credential_kind, CredentialKindHint::None);
    }

    #[test]
    fn unsupported_windows_return_none() {
        let c = capability_for(
            &ProviderId::new("anthropic"),
            QuotaWindow::Rolling5h,
            &QuotaUnit::Count,
        );
        assert!(c.is_some(), "entry exists to mark Unsupported");
        assert_eq!(
            c.unwrap().best_confidence,
            ConfidenceOrUnsupported::Unsupported
        );
    }

    #[test]
    fn zhipu_coding_plan_windows_are_scraped_counts() {
        for window in [QuotaWindow::Rolling5h, QuotaWindow::Weekly] {
            let c = capability_for(&ProviderId::new("zhipu"), window, &QuotaUnit::Count)
                .expect("zhipu coding-plan entry");
            assert_eq!(c.best_confidence, ConfidenceOrUnsupported::Scraped);
            assert_eq!(c.adapter_kind, AdapterKind::WebScrape);
            assert_eq!(c.credential_kind, CredentialKindHint::CookieSession);
        }
    }

    #[test]
    fn xai_monthly_combines_limit_and_invoice_preview() {
        let c = capability_for(
            &ProviderId::new("xai"),
            QuotaWindow::Monthly,
            &QuotaUnit::Cost {
                currency: "USD".into(),
            },
        )
        .expect("xai monthly");
        assert_eq!(c.best_confidence, ConfidenceOrUnsupported::Exact);
        assert!(c.note.contains("invoice-preview"));
    }

    #[test]
    fn openai_monthly_note_mentions_reset() {
        let c = capability_for(
            &ProviderId::new("openai"),
            QuotaWindow::Monthly,
            &QuotaUnit::Cost {
                currency: "USD".into(),
            },
        )
        .expect("openai monthly");
        assert!(c.note.contains("reset"));
    }

    #[test]
    fn anthropic_monthly_note_mentions_user_scope_null_and_credits() {
        let c = capability_for(
            &ProviderId::new("anthropic"),
            QuotaWindow::Monthly,
            &QuotaUnit::Cost {
                currency: "USD".into(),
            },
        )
        .expect("anthropic monthly");
        assert!(c.note.contains("scope={type:user,user_id}"));
        assert!(c.note.contains("explicit null"));
        assert!(c.note.contains("missing field = parse error"));
        assert!(c.note.contains("Enterprise"));
    }
}
