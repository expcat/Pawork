//! Quota 查询、视图与告警（canonical 镜像，脱敏）。

use pawork_domain::{ModelId, ProviderId, TenantId, Timestamp};
use serde::{Deserialize, Serialize};
#[cfg(feature = "typegen")]
use ts_rs::TS;

/// 默认 legacy Quota 身份作用域：tenant `local`、account `local/default`。
///
/// 未显式指定作用域的 CLI 查询与 run 归属都落在此默认作用域；非默认作用域
/// 的查询需要显式授权 grant。P14-8 引入 typed Quota 查询/视图/告警事件
/// （`AppQuery::QuotaOverview`、`AppEvent::QuotaChanged`、`AppEvent::QuotaAlert`），
/// 均为 TS 导出的 canonical 镜像，且只暴露脱敏的凭证提示。
/// Control Plane 的 legacy tenant 由 [`DEFAULT_CONTROL_PLANE_TENANT`] 独立冻结为
/// `local/default`，不复用此 Quota 常量。
pub const DEFAULT_QUOTA_TENANT: &str = "local";
pub const DEFAULT_QUOTA_ACCOUNT: &str = "local/default";

/// Canonical 身份 tenant（P18-2）：IdentityContext 归一后的本地用户租户为
/// `local/default`，与 legacy Quota 哨兵 [`DEFAULT_QUOTA_TENANT`]（`local`）
/// 显式映射为同一默认作用域。查询/授权判定同时接受两种写法，避免
/// `pawork usage --tenant local/default` 被误判为非默认作用域而拒绝。
pub const DEFAULT_QUOTA_TENANT_CANONICAL: &str = "local/default";

// =========================================================================
// Quota（P14-8）：canonical 镜像 + 查询/视图/告警，全部 TS 导出且脱敏。
// =========================================================================
//
// 这些类型是 quota-service canonical 领域类型的协议镜像：core-api 不依赖
// quota-service（避免把 reqwest/scraper 拖进协议 crate），但 serde 形态保持
// 一致，app-service 在边界做 1:1 转换。视图只暴露脱敏的 `credential_hint`，
// 永不包含 secret/token/cookie。

/// Canonical 配额窗口。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaWindow {
    #[default]
    Overall,
    Rolling5h,
    Weekly,
    Monthly,
}

/// Canonical 配额单位。`Cost` 携带 ISO-4217 币种。
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuotaUnit {
    #[default]
    Count,
    Token,
    Cost {
        currency: String,
    },
}

/// Canonical 非负度量值。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum QuotaMeasure {
    Exact(u64),
    Infinite,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct QuotaValues {
    pub used: QuotaMeasure,
    pub limit: QuotaMeasure,
    pub remaining: QuotaMeasure,
}

/// 可信度优先级：exact > derived > scraped；默认最低信任 scraped。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaConfidence {
    Exact,
    Derived,
    #[default]
    Scraped,
}

/// Canonical 适配器来源种类（脱敏枚举，不含任何凭证字段）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaAdapterKind {
    ApiKeyApi,
    OAuthApi,
    WebScrape,
    #[default]
    LocalLedger,
}

/// 安全的来源元数据。`endpoint` 已去除 query/fragment，永不泄漏 token。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct QuotaProvenanceView {
    pub adapter_kind: QuotaAdapterKind,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    pub fetched_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<Timestamp>,
    #[serde(default)]
    pub stale: bool,
}

/// 窗口重置语义：绝对 / 相对 / 未知。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
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

/// Quota 查询：tenant/account 必填，其余为可选过滤维度。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct QuotaOverviewQuery {
    pub tenant_id: TenantId,
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<ProviderId>,
    /// 凭证元数据 ID（opaque，绝非凭证值）；视图输出时脱敏。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<ModelId>,
    /// 空表 = 默认所有支持的窗口。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<QuotaWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<QuotaUnit>,
}

impl QuotaOverviewQuery {
    /// 默认 legacy 作用域（local / local/default），无任何过滤维度。
    pub fn default_local() -> Self {
        Self {
            tenant_id: TenantId::new(DEFAULT_QUOTA_TENANT),
            account_id: DEFAULT_QUOTA_ACCOUNT.to_string(),
            provider_id: None,
            credential_id: None,
            model_id: None,
            windows: Vec::new(),
            unit: None,
        }
    }

    /// 是否落在默认 legacy 作用域：tenant 接受 legacy 哨兵 `local` 或
    /// canonical 身份租户 `local/default`（[`DEFAULT_QUOTA_TENANT_CANONICAL`]，
    /// 显式映射，不静默改写），account 必须为 `local/default`。
    pub fn is_default_scope(&self) -> bool {
        let tenant_is_default = self.tenant_id.as_str() == DEFAULT_QUOTA_TENANT
            || self.tenant_id.as_str() == DEFAULT_QUOTA_TENANT_CANONICAL;
        tenant_is_default && self.account_id == DEFAULT_QUOTA_ACCOUNT
    }
}

/// 作用域视图：只暴露脱敏的 `credential_hint`，永不暴露凭证原文。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct QuotaScopeView {
    pub tenant_id: TenantId,
    pub account_id: String,
    pub provider_id: ProviderId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<ModelId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_hint: Option<String>,
}

/// 单窗口快照（脱敏后的 canonical 镜像）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct QuotaSnapshotView {
    pub scope: QuotaScopeView,
    pub window: QuotaWindow,
    pub unit: QuotaUnit,
    pub values: QuotaValues,
    pub reset: QuotaReset,
    pub confidence: QuotaConfidence,
    pub provenance: QuotaProvenanceView,
    /// 该快照是否来自过期缓存兜底（fresh 抓取失败）。
    #[serde(default)]
    pub served_stale: bool,
}

/// typed 失败：适配器种类（可空）+ 错误码 + 脱敏详情。
///
/// `adapter_kind` 仅当失败确实来自某个 adapter 时为 `Some`；scope 校验、
/// 无候选、取消、内部耗尽等查询级失败为 `None`，不虚构归属。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct QuotaFailureView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_kind: Option<QuotaAdapterKind>,
    /// 错误短码（如 `forbidden`、`rate_limited`、`timeout`、`unsupported`）。
    pub error_code: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// 单个 (scope, window, unit) 读数结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WindowReadView {
    /// 至少一个适配器产出了可用快照（可能为过期缓存兜底）。
    Ok {
        snapshot: Box<QuotaSnapshotView>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        failures: Vec<QuotaFailureView>,
    },
    /// 所有候选适配器失败且无缓存兜底。
    Failed { failures: Vec<QuotaFailureView> },
    /// 该 (scope, window, unit) 当前无缓存数据（sync 查询只读缓存）。
    NoData,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct WindowReadEntry {
    pub window: QuotaWindow,
    pub read: WindowReadView,
}

/// Quota 总览视图：每个窗口一项，附生成时刻与是否命中缓存。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct QuotaOverviewView {
    pub scope: QuotaScopeView,
    pub windows: Vec<WindowReadEntry>,
    pub generated_at: Timestamp,
    /// 是否来自 quota-service 缓存（false = 当前无缓存，全是 NoData）。
    #[serde(default)]
    pub from_cache: bool,
}

/// 稳定告警种类：与 quota-service `refresh::AlertKind` 1:1 镜像，serde
/// 形态冻结（snake_case）。消费端按 kind 派生可执行动作与文案，不解析
/// 自由文本 `message`；`Threshold` 的 advisory 语义由
/// [`QuotaAlertSeverity`] 区分（Warning = advisory 估算，Critical = 真实触限）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaAlertKind {
    /// 剩余额度跌破配置阈值（advisory 时为抓取/估算数据，非硬停）。
    Threshold,
    /// 此前触发的 Threshold 已恢复。
    Recovered,
    /// 新鲜抓取失败，读取以过期缓存兜底。
    Stale,
    /// 凭证无效/被吊销，需要用户重新授权。
    ReauthorizationRequired,
    /// 部分适配器失败，但仍有其他适配器产出快照。
    PartialFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaAlertSeverity {
    Info,
    Warning,
    Critical,
}

/// 额度告警（安全 typed 视图，仅含脱敏字段）。
///
/// `source` 是已脱敏的来源标签（adapter kind + 短来源名），不携带端点
/// query/fragment 或 secret/token/cookie 原文；`kind` 是稳定种类，动作由
/// 消费端派生。二者均为 `Option`：`kind`/`source` 是后加的持久化字段，
/// 旧事件 JSON 缺省时可解码为 `None`（重放兼容），新事件总是 `Some`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct QuotaAlert {
    pub tenant_id: TenantId,
    pub account_id: String,
    pub provider_id: ProviderId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<ModelId>,
    pub window: QuotaWindow,
    pub unit: QuotaUnit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<QuotaAlertKind>,
    pub severity: QuotaAlertSeverity,
    /// 脱敏来源标签（adapter kind + 短来源名），永不包含 query/fragment
    /// 或 secret/token/cookie 原文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<QuotaSnapshotView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_hint: Option<String>,
}


/// 把凭证元数据 ID 脱敏为安全提示：保留首尾各 2 字符，中间以 `*` 替代；
/// 过短或空值返回 `None`。永不包含 secret/token/cookie 原文。
pub fn mask_credential_hint(id: &str) -> Option<String> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return None;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 4 {
        return Some("*".repeat(chars.len()));
    }
    let head: String = chars.iter().take(2).collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    Some(format!("{head}{}{tail}", "*".repeat(chars.len() - 4)))
}

#[cfg(test)]
mod tests {
    use pawork_domain::{ModelId, ProviderId, TenantId, Timestamp};

    use super::*;

    #[test]
    fn mask_credential_hint_never_leaks_full_id() {
        assert_eq!(
            mask_credential_hint("sk-secret-token-1234").as_deref(),
            Some("sk****************34")
        );
        assert_eq!(mask_credential_hint("abcd").as_deref(), Some("****"));
        assert_eq!(mask_credential_hint("ab").as_deref(), Some("**"));
        assert_eq!(mask_credential_hint(""), None);
        assert_eq!(mask_credential_hint("   "), None);
        let masked = mask_credential_hint("credential-abcde-xyz").unwrap();
        assert!(!masked.contains("abcde"));
        assert!(!masked.contains("xyz"));
    }

    #[test]
    fn quota_overview_query_default_local_matches_legacy_scope() {
        let query = QuotaOverviewQuery::default_local();
        assert!(query.is_default_scope());
        assert_eq!(query.tenant_id.as_str(), DEFAULT_QUOTA_TENANT);
        assert_eq!(query.account_id, DEFAULT_QUOTA_ACCOUNT);

        // P18-8 租户分歧：canonical 身份租户 `local/default` 必须与 legacy
        // 哨兵 `local` 映射为同一默认作用域（显式映射，不静默改写、不丢历史）。
        let canonical = QuotaOverviewQuery {
            tenant_id: TenantId::new(DEFAULT_QUOTA_TENANT_CANONICAL),
            ..query.clone()
        };
        assert!(canonical.is_default_scope());
        assert_eq!(
            canonical.tenant_id.as_str(),
            DEFAULT_QUOTA_TENANT_CANONICAL,
            "显式查询的 canonical tenant 原样保留，不做静默改写"
        );

        // canonical tenant + 非默认 account：仍不是默认作用域。
        let canonical_wrong_account = QuotaOverviewQuery {
            tenant_id: TenantId::new(DEFAULT_QUOTA_TENANT_CANONICAL),
            account_id: "other/account".into(),
            ..query.clone()
        };
        assert!(!canonical_wrong_account.is_default_scope());

        let other = QuotaOverviewQuery {
            tenant_id: TenantId::new("remote"),
            account_id: "remote/acc".into(),
            ..query.clone()
        };
        assert!(!other.is_default_scope());
    }

    #[test]
    fn quota_overview_view_round_trip_carries_no_secret() {
        let view = QuotaOverviewView {
            scope: QuotaScopeView {
                tenant_id: TenantId::new("local"),
                account_id: "local/default".into(),
                provider_id: ProviderId::from("anthropic"),
                model_id: Some(ModelId::from("claude")),
                credential_hint: mask_credential_hint("sk-secret-key-9999"),
            },
            windows: vec![WindowReadEntry {
                window: QuotaWindow::Monthly,
                read: WindowReadView::Ok {
                    snapshot: Box::new(QuotaSnapshotView {
                        scope: QuotaScopeView {
                            tenant_id: TenantId::new("local"),
                            account_id: "local/default".into(),
                            provider_id: ProviderId::from("anthropic"),
                            model_id: None,
                            credential_hint: None,
                        },
                        window: QuotaWindow::Monthly,
                        unit: QuotaUnit::Token,
                        values: QuotaValues {
                            used: QuotaMeasure::Exact(25),
                            limit: QuotaMeasure::Exact(100),
                            remaining: QuotaMeasure::Exact(75),
                        },
                        reset: QuotaReset::Unknown,
                        confidence: QuotaConfidence::Exact,
                        provenance: QuotaProvenanceView {
                            adapter_kind: QuotaAdapterKind::ApiKeyApi,
                            source: "anthropic.admin".into(),
                            endpoint: None,
                            fetched_at: Timestamp::from_unix_millis(1),
                            observed_at: None,
                            stale: false,
                        },
                        served_stale: false,
                    }),
                    failures: Vec::new(),
                },
            }],
            generated_at: Timestamp::from_unix_millis(1),
            from_cache: true,
        };
        let json = serde_json::to_string(&view).expect("serialize view");
        assert!(
            !json.contains("sk-secret-key-9999"),
            "leaked secret: {json}"
        );
        assert!(
            json.contains("sk**************99"),
            "masked hint missing: {json}"
        );
        let decoded: QuotaOverviewView = serde_json::from_str(&json).expect("deserialize view");
        assert_eq!(decoded, view);
    }

    #[test]
    fn quota_alert_round_trip_is_safe() {
        let alert = QuotaAlert {
            tenant_id: TenantId::new("local"),
            account_id: "local/default".into(),
            provider_id: ProviderId::from("openai"),
            model_id: None,
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Token,
            kind: Some(QuotaAlertKind::ReauthorizationRequired),
            severity: QuotaAlertSeverity::Warning,
            source: Some("ApiKeyApi:api.openai.com/v1/organization/usage".into()),
            message: "low balance".into(),
            snapshot: None,
            credential_hint: mask_credential_hint("sk-leak"),
        };
        let json = serde_json::to_string(&alert).expect("serialize alert");
        assert!(!json.contains("sk-leak"));
        assert!(
            json.contains("\"kind\":\"reauthorization_required\""),
            "kind 必须按冻结的 snake_case 形态序列化: {json}"
        );
        assert!(
            json.contains("\"source\":\"ApiKeyApi:api.openai.com/v1/organization/usage\""),
            "source 原样往返: {json}"
        );
        let decoded: QuotaAlert = serde_json::from_str(&json).expect("deserialize alert");
        assert_eq!(decoded, alert);
    }

    #[test]
    fn quota_alert_legacy_json_without_kind_source_decodes_to_none() {
        // kind/source 是后加的持久化字段：旧事件 JSON 缺少二者时必须可解码
        // （重放兼容），得到 None；其余字段原样保留。
        let alert = QuotaAlert {
            tenant_id: TenantId::new("local"),
            account_id: "local/default".into(),
            provider_id: ProviderId::from("openai"),
            model_id: None,
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Token,
            kind: Some(QuotaAlertKind::Threshold),
            severity: QuotaAlertSeverity::Warning,
            source: Some("ApiKeyApi:api.openai.com/v1/usage".into()),
            message: "low balance".into(),
            snapshot: None,
            credential_hint: None,
        };
        let mut json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&alert).expect("serialize")).expect("json");
        for key in ["kind", "source"] {
            assert!(
                json.get(key).is_some(),
                "precondition: new events serialize {key}"
            );
            json.as_object_mut().expect("object").remove(key);
        }
        let decoded: QuotaAlert =
            serde_json::from_value(json).expect("legacy JSON without kind/source must decode");
        assert_eq!(decoded.kind, None);
        assert_eq!(decoded.source, None);
        assert_eq!(decoded.severity, QuotaAlertSeverity::Warning);
        assert_eq!(decoded.message, "low balance");
        assert_eq!(decoded.window, QuotaWindow::Monthly);
    }

    #[test]
    fn quota_alert_kind_serde_is_stable_and_exhaustive() {
        // 冻结的线上形态：kind 必须与 quota-service refresh::AlertKind 的
        // snake_case 序列化一致，消费端依赖该字符串做映射，不可漂移。
        let wire = [
            (QuotaAlertKind::Threshold, "threshold"),
            (QuotaAlertKind::Recovered, "recovered"),
            (QuotaAlertKind::Stale, "stale"),
            (
                QuotaAlertKind::ReauthorizationRequired,
                "reauthorization_required",
            ),
            (QuotaAlertKind::PartialFailure, "partial_failure"),
        ];
        for (kind, expected) in wire {
            let json = serde_json::to_string(&kind).expect("serialize kind");
            assert_eq!(json, format!("\"{expected}\""));
            let decoded: QuotaAlertKind = serde_json::from_str(&json).expect("deserialize kind");
            assert_eq!(decoded, kind);
        }
    }

}
