//! xAI 配额适配器。
//!
//! 事实源（brief）：management key 使用 `https://management-api.x.ai`；team id
//! 是非机密 adapter 配置。
//!
//! - prepaid：`GET /v1/billing/teams/{team_id}/prepaid/balance`，整体余额，
//!   `total.val` 是有符号十进制整数（USD cents）→ 作为 Overall limit/remaining。
//! - postpaid：`GET /v1/billing/teams/{team_id}/postpaid/spending-limits`，
//!   `limits[].{period,limit.val}` 给月度**硬上限**（USD cents，必须显式
//!   `period=monthly`，不匹配绝不回退）；当期**消耗**（used）来自
//!   `GET /v1/billing/teams/{team_id}/invoice-preview` 的 `total.val`（USD cents）。
//!   两者组合得到 Monthly used / limit，避免伪造 used；窗口为自然月，reset 为
//!   次月 1 号 00:00 UTC。
//!
//! 金额严格契约：所有 cents 经 [`cents_to_micros`] 精确换算——负数、溢出一律
//! `Parse`（不钳位、不饱和）；`currency` 必须为 USD（缺失/不匹配 → `Parse`）。

use std::sync::Arc;

use async_trait::async_trait;
use futures::future;
use provider_api::{CredentialKind, ResolvedCredential};
use provider_runtime::http::HttpClient;

use crate::adapters::http_util::{api_get, bearer_headers, now_millis, redact_endpoint};
use crate::adapters::money::cents_to_micros;
use crate::{
    AdapterKind, Confidence, QuotaAdapter, QuotaError, QuotaMeasure, QuotaProvenance, QuotaRequest,
    QuotaReset, QuotaSnapshot, QuotaUnit, QuotaValues, QuotaWindow,
};

const BASE: &str = "https://management-api.x.ai";

/// xAI team 配置（team_id 非机密）。
#[derive(Clone, Debug)]
pub struct XaiConfig {
    pub team_id: String,
}

/// xAI management key 额度适配器。
///
/// prepaid 走 Overall；postpaid 组合 spending-limits(limit) + invoice-preview(used)
/// 走 Monthly。
pub fn adapter(http: Arc<HttpClient>, config: XaiConfig) -> Box<dyn QuotaAdapter> {
    Box::new(XaiAdapter::new(http, BASE, config))
}

struct XaiAdapter {
    http: Arc<HttpClient>,
    base: String,
    team_id: String,
}

impl XaiAdapter {
    fn new(http: Arc<HttpClient>, base: impl Into<String>, config: XaiConfig) -> Self {
        Self {
            http,
            base: base.into(),
            team_id: config.team_id,
        }
    }
}

#[async_trait]
impl QuotaAdapter for XaiAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::ApiKeyApi
    }

    fn supports(&self, request: &QuotaRequest) -> bool {
        match (request.window, &request.unit) {
            (QuotaWindow::Overall, QuotaUnit::Cost { currency })
                if currency.eq_ignore_ascii_case("USD") =>
            {
                true
            }
            (QuotaWindow::Monthly, QuotaUnit::Cost { currency })
                if currency.eq_ignore_ascii_case("USD") =>
            {
                true
            }
            _ => false,
        }
    }

    async fn fetch(
        &self,
        request: &QuotaRequest,
        credential: Option<&ResolvedCredential>,
        cancel: &agent_domain::CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError> {
        let credential = credential
            .ok_or_else(|| QuotaError::unauthorized("xai management api key required"))?;
        if credential.kind() != CredentialKind::ApiKey {
            return Err(QuotaError::unauthorized(
                "xai: management key is an API key",
            ));
        }
        let headers = bearer_headers(credential);

        match request.window {
            QuotaWindow::Overall => self.fetch_prepaid(request, &headers, cancel).await,
            QuotaWindow::Monthly => self.fetch_postpaid(request, &headers, cancel).await,
            _ => Err(QuotaError::unsupported("xai: unsupported window")),
        }
    }
}

impl XaiAdapter {
    async fn fetch_prepaid(
        &self,
        request: &QuotaRequest,
        headers: &[(String, String)],
        cancel: &agent_domain::CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError> {
        let url = format!(
            "{}/v1/billing/teams/{}/prepaid/balance",
            self.base, self.team_id
        );
        let body = api_get(self.http.as_ref(), &url, headers, cancel).await?;
        expect_usd_currency(&body, &["total"], "xai prepaid")?;
        // total.val 为有符号整数 USD cents；prepaid 余额即可用上限。
        let cents = field_cents(&body, &["total"], &["val"])
            .ok_or_else(|| QuotaError::parse("xai prepaid: missing total.val"))?;
        let amount = cents_to_micros(cents)?;
        self.snapshot(
            request,
            QuotaValues::new(
                QuotaMeasure::exact(0),
                QuotaMeasure::exact(amount),
                QuotaMeasure::exact(amount),
            ),
            "xai.prepaid",
            &format!(
                "{}/v1/billing/teams/{}/prepaid/balance",
                self.base, self.team_id
            ),
        )
    }

    async fn fetch_postpaid(
        &self,
        request: &QuotaRequest,
        headers: &[(String, String)],
        cancel: &agent_domain::CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError> {
        // 并发拉取 limit（spending-limits）与 used（invoice-preview），各自独立成败。
        let limit_url = format!(
            "{}/v1/billing/teams/{}/postpaid/spending-limits",
            self.base, self.team_id
        );
        let used_url = format!(
            "{}/v1/billing/teams/{}/invoice-preview",
            self.base, self.team_id
        );
        let limit_fut = async {
            let body = api_get(self.http.as_ref(), &limit_url, headers, cancel).await?;
            monthly_limit_cents(&body)
        };
        let used_fut = async {
            let body = api_get(self.http.as_ref(), &used_url, headers, cancel).await?;
            expect_usd_currency(&body, &["total"], "xai invoice-preview")?;
            field_cents(&body, &["total"], &["val"])
                .ok_or_else(|| QuotaError::parse("xai invoice-preview: missing total.val"))
        };
        let (limit_res, used_res) = future::join(limit_fut, used_fut).await;

        match (limit_res, used_res) {
            (Ok(limit_cents), Ok(used_cents)) => {
                let limit_micros = cents_to_micros(limit_cents)?;
                let used_micros = cents_to_micros(used_cents)?;
                let remaining = match limit_micros.checked_sub(used_micros) {
                    Some(v) => QuotaMeasure::exact(v),
                    // used > limit：负数无法表示，诚实 Unknown，不伪造 0。
                    None => QuotaMeasure::Unknown,
                };
                self.snapshot(
                    request,
                    QuotaValues::new(
                        QuotaMeasure::exact(used_micros),
                        QuotaMeasure::exact(limit_micros),
                        remaining,
                    ),
                    "xai.postpaid",
                    &limit_url,
                )
            }
            (Ok(limit_cents), Err(_)) => {
                // used 拉取失败：limit 已知，used/remaining = Unknown（不伪造）。
                self.snapshot(
                    request,
                    QuotaValues::new(
                        QuotaMeasure::Unknown,
                        QuotaMeasure::exact(cents_to_micros(limit_cents)?),
                        QuotaMeasure::Unknown,
                    ),
                    "xai.postpaid",
                    &limit_url,
                )
            }
            (Err(_), Ok(used_cents)) => {
                // limit 拉取失败：used 已知，limit/remaining = Unknown（不伪造）。
                self.snapshot(
                    request,
                    QuotaValues::new(
                        QuotaMeasure::exact(cents_to_micros(used_cents)?),
                        QuotaMeasure::Unknown,
                        QuotaMeasure::Unknown,
                    ),
                    "xai.postpaid",
                    &used_url,
                )
            }
            (Err(limit_err), Err(used_err)) => Err(merge_dual_failures(limit_err, used_err)),
        }
    }

    fn snapshot(
        &self,
        request: &QuotaRequest,
        values: QuotaValues,
        source: &'static str,
        endpoint_url: &str,
    ) -> Result<QuotaSnapshot, QuotaError> {
        let now = now_millis();
        // Monthly 窗口（postpaid 组合）是自然月：次月 1 号 00:00 UTC 重置；
        // Overall（prepaid 余额）无重置概念。
        let reset = match request.window {
            QuotaWindow::Monthly => QuotaReset::Absolute {
                at: next_month_start_timestamp(),
                uncertain: false,
            },
            _ => QuotaReset::Unknown,
        };
        Ok(QuotaSnapshot {
            scope: request.scope.clone(),
            window: request.window,
            unit: request.unit.clone(),
            values,
            reset,
            confidence: Confidence::Exact,
            provenance: QuotaProvenance {
                adapter_kind: AdapterKind::ApiKeyApi,
                source: source.to_string(),
                endpoint: Some(redact_endpoint(endpoint_url)),
                fetched_at: now,
                observed_at: Some(now),
                selector_version: None,
                stale: false,
            },
        })
    }
}

/// 两个 postpaid 端点同时失败时的合并分类。
///
/// 优先级（高 → 低）：`Cancelled`、`Unauthorized` / `ReauthorizationRequired`、
/// `RateLimited`（保留两端最大 `retry_after_ms`）、`Forbidden`、retryable
/// （`Timeout` / `Transient`）、其余（`Unsupported` / `Parse` / `Other`）。
/// 任一高优分类存在即整体保留该分类，绝不因两端同时失败统一塌缩为 `Other`。
/// 组合消息只含固定文本与 typed 分类名，不携带任一子错误的 `detail`——远端
/// detail 与潜在 secret 永不进入错误文本。
fn merge_dual_failures(limit_err: QuotaError, used_err: QuotaError) -> QuotaError {
    let combined = format!(
        "xai: both postpaid endpoints failed (limit: {}; used: {})",
        failure_kind_label(&limit_err),
        failure_kind_label(&used_err),
    );
    let (dominant, other) = if failure_priority(&limit_err) >= failure_priority(&used_err) {
        (&limit_err, &used_err)
    } else {
        (&used_err, &limit_err)
    };
    match dominant {
        QuotaError::Cancelled => QuotaError::Cancelled,
        QuotaError::Unauthorized { .. } => QuotaError::unauthorized(combined),
        QuotaError::ReauthorizationRequired { .. } => {
            QuotaError::reauthorization_required(combined)
        }
        QuotaError::RateLimited { .. } => QuotaError::rate_limited(
            combined,
            max_retry_after(dominant.retry_after_ms(), other.retry_after_ms()),
        ),
        QuotaError::Forbidden { .. } => QuotaError::forbidden(combined),
        QuotaError::Timeout {
            status,
            retry_after_ms,
            ..
        } => QuotaError::Timeout {
            detail: combined,
            status: *status,
            retry_after_ms: *retry_after_ms,
        },
        QuotaError::Transient {
            status,
            retry_after_ms,
            ..
        } => QuotaError::Transient {
            detail: combined,
            status: *status,
            retry_after_ms: *retry_after_ms,
        },
        // Unsupported / Parse / Other：无更高优分类时保持 Other 组合消息。
        _ => QuotaError::other(combined),
    }
}

/// 错误分类的合并优先级（数值越大越优先）。
fn failure_priority(err: &QuotaError) -> u8 {
    match err {
        QuotaError::Cancelled => 10,
        QuotaError::Unauthorized { .. } => 9,
        QuotaError::ReauthorizationRequired { .. } => 8,
        QuotaError::RateLimited { .. } => 7,
        QuotaError::Forbidden { .. } => 6,
        QuotaError::Timeout { .. } => 5,
        QuotaError::Transient { .. } => 4,
        QuotaError::Unsupported { .. } => 3,
        QuotaError::Parse { .. } => 2,
        QuotaError::Other { .. } => 1,
    }
}

/// 组合消息中使用的固定分类名（不含任何远端 detail）。
fn failure_kind_label(err: &QuotaError) -> &'static str {
    match err {
        QuotaError::Cancelled => "cancelled",
        QuotaError::Unauthorized { .. } => "unauthorized",
        QuotaError::ReauthorizationRequired { .. } => "reauthorization required",
        QuotaError::RateLimited { .. } => "rate limited",
        QuotaError::Forbidden { .. } => "forbidden",
        QuotaError::Timeout { .. } => "timed out",
        QuotaError::Transient { .. } => "transient failure",
        QuotaError::Unsupported { .. } => "unsupported",
        QuotaError::Parse { .. } => "parse failure",
        QuotaError::Other { .. } => "failed",
    }
}

/// 两端 `retry_after` 取较大者（保守等待更长），任一缺失取另一侧。
fn max_retry_after(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// 从 spending-limits 响应中取 monthly 项的 limit（USD cents）。
fn monthly_limit_cents(body: &serde_json::Value) -> Result<i64, QuotaError> {
    let arr = body
        .get("limits")
        .or_else(|| body.get("data"))
        .and_then(|v| v.as_array())
        .or_else(|| body.as_array())
        .ok_or_else(|| QuotaError::parse("xai postpaid: expected limits[] array"))?;
    // 必须显式命中 period=monthly 条目；无 monthly 条目是契约异常，绝不回退
    // 到数组首个条目（避免把 weekly 等其他周期误当月度上限）。
    let monthly = arr
        .iter()
        .find(|item| {
            item.get("period")
                .and_then(|p| p.as_str())
                .is_some_and(|p| p.eq_ignore_ascii_case("monthly"))
        })
        .ok_or_else(|| QuotaError::parse("xai postpaid: no monthly entry"))?;
    if let Some(currency) = monthly.get("currency").and_then(|v| v.as_str()) {
        if !currency.eq_ignore_ascii_case("USD") {
            return Err(QuotaError::parse("xai postpaid: unexpected limit currency"));
        }
    }
    field_cents(monthly, &["limit", "amount"], &["val"])
        .ok_or_else(|| QuotaError::parse("xai postpaid: monthly limit missing limit.val"))
}

/// 校验嵌套对象（按 `outer` 路径）中的 `currency` 必须为 USD（大小写不敏感）。
///
/// 路径上任意一环缺失、`currency` 缺失或不匹配一律 `Parse`——金额单位不明
/// 时绝不默认假设。
fn expect_usd_currency(
    value: &serde_json::Value,
    outer: &[&str],
    what: &str,
) -> Result<(), QuotaError> {
    let mut node = value;
    for key in outer {
        node = node
            .get(*key)
            .ok_or_else(|| QuotaError::parse(format!("{what}: missing {key}")))?;
    }
    let currency = node
        .get("currency")
        .and_then(|v| v.as_str())
        .ok_or_else(|| QuotaError::parse(format!("{what}: missing currency")))?;
    if !currency.eq_ignore_ascii_case("USD") {
        return Err(QuotaError::parse(format!("{what}: unexpected currency")));
    }
    Ok(())
}

/// 下月 1 号 00:00 UTC 的 Timestamp（自然月 reset 时刻）。
fn next_month_start_timestamp() -> agent_domain::Timestamp {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (now / 86_400) as i64;
    let (y, mo, _, _, _, _) = epoch_to_utc_from_days(days);
    // 下月：mo in 1..=12；mo==12 → 次年 1 月。
    let (ny, nmo) = if mo == 12 { (y + 1, 1) } else { (y, mo + 1) };
    let secs = civil_to_days(ny, nmo, 1) * 86_400;
    agent_domain::Timestamp::from_unix_millis(secs as u64 * 1_000)
}

/// Unix 天数 → UTC 民用日期（Howard Hinnant 算法）。
fn epoch_to_utc_from_days(days: i64) -> (i32, u32, u32, u32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = ((doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365) as i64;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe as u64 + yoe as u64 / 4 - yoe as u64 / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (if mo <= 2 { y + 1 } else { y }) as i32;
    (y, mo, d, 0, 0, 0)
}

/// 民用日期 → Unix 天数（UTC）。
fn civil_to_days(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { (y - 1) as i64 } else { y as i64 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m_adj = if m > 2 { m as i64 - 3 } else { m as i64 + 9 };
    let doy = (153 * m_adj as u64 + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// 在 `value.<outer>` 下按 `inner` 键找整数 cents。
///
/// 支持两种结构：
/// - `{"<outer>": {"<inner>": "3000"}}`（xAI 真实形态 `{val: "..."}`）
/// - `value` 本身就是 `{"<outer>": "3000"}` 或裸 `"<inner>": "3000"`。
fn field_cents(value: &serde_json::Value, outer: &[&str], inner: &[&str]) -> Option<i64> {
    // 先按 outer 嵌套结构尝试。
    for o in outer {
        if let Some(node) = value.get(o) {
            if node.is_null() {
                continue;
            }
            if let Some(cents) = cents_from_node(node, inner) {
                return Some(cents);
            }
        }
    }
    // 回退：value 本身就是包含 inner 的对象。
    if let Some(cents) = cents_from_node(value, inner) {
        return Some(cents);
    }
    None
}

fn cents_from_node(node: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        if let Some(v) = node.get(key) {
            if v.is_null() {
                continue;
            }
            let s = v
                .get("val")
                .and_then(|x| {
                    x.as_str()
                        .map(str::to_owned)
                        .or_else(|| x.as_i64().map(|n| n.to_string()))
                })
                .or_else(|| v.as_str().map(str::to_owned))
                .or_else(|| v.as_i64().map(|n| n.to_string()))?;
            return s.trim().parse::<i64>().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountId, QuotaScope};
    use agent_domain::{ProviderId, TenantId};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn http() -> Arc<HttpClient> {
        Arc::new(
            HttpClient::new(
                provider_runtime::http::HttpClientConfig::builder()
                    .disable_system_proxy()
                    .build(),
            )
            .expect("client"),
        )
    }

    fn req(window: QuotaWindow) -> QuotaRequest {
        QuotaRequest {
            scope: QuotaScope::new(
                TenantId::new("t"),
                AccountId::new("a"),
                ProviderId::new("xai"),
                None,
            ),
            window,
            unit: QuotaUnit::Cost {
                currency: "USD".into(),
            },
        }
    }

    fn cred() -> ResolvedCredential {
        ResolvedCredential::new(CredentialKind::ApiKey, "xai-mgmt-FAKE")
    }

    /// 从仓库 fixtures/quota/ 加载 contract fixture（只读，不参与生产代码）。
    fn fixture(name: &str) -> serde_json::Value {
        let raw = match name {
            "xai_invoice_preview.json" => {
                include_str!("../../../../fixtures/quota/xai_invoice_preview.json")
            }
            "xai_postpaid_spending_limits.json" => {
                include_str!("../../../../fixtures/quota/xai_postpaid_spending_limits.json")
            }
            "xai_prepaid_balance.json" => {
                include_str!("../../../../fixtures/quota/xai_prepaid_balance.json")
            }
            other => panic!("unknown fixture: {other}"),
        };
        serde_json::from_str(raw).expect("fixture must be valid JSON")
    }

    #[tokio::test]
    async fn prepaid_overall_balance() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/prepaid/balance"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(fixture("xai_prepaid_balance.json")),
            )
            .mount(&server)
            .await;
        let a = XaiAdapter::new(
            http(),
            server.uri(),
            XaiConfig {
                team_id: "team-1".into(),
            },
        );
        let snap = a
            .fetch(
                &req(QuotaWindow::Overall),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("ok");
        assert_eq!(snap.values.limit, QuotaMeasure::exact(50_000_000));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(50_000_000));
        assert_eq!(snap.values.used, QuotaMeasure::exact(0));
        assert_eq!(snap.provenance.source, "xai.prepaid");
        // Overall（prepaid 余额）无重置概念。
        assert_eq!(snap.reset, QuotaReset::Unknown);
    }

    #[tokio::test]
    async fn postpaid_combines_spending_limit_and_invoice_preview() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/postpaid/spending-limits"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(fixture("xai_postpaid_spending_limits.json")),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/invoice-preview"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(fixture("xai_invoice_preview.json")),
            )
            .mount(&server)
            .await;
        let a = XaiAdapter::new(
            http(),
            server.uri(),
            XaiConfig {
                team_id: "team-1".into(),
            },
        );
        let snap = a
            .fetch(
                &req(QuotaWindow::Monthly),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("ok");
        assert_eq!(snap.values.limit, QuotaMeasure::exact(100_000_000));
        assert_eq!(snap.values.used, QuotaMeasure::exact(30_000_000));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(70_000_000));
        assert_eq!(snap.provenance.source, "xai.postpaid");
        // Monthly 窗口是自然月：reset 必须为次月 1 号 00:00 UTC。
        match snap.reset {
            QuotaReset::Absolute { at, uncertain } => {
                assert!(!uncertain);
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let reset_ms = at.as_unix_millis();
                assert!(reset_ms > now_ms, "monthly reset must be in the future");
                assert!(reset_ms <= now_ms + 32 * 86_400 * 1_000);
            }
            other => panic!("expected Absolute reset, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn postpaid_invoice_failure_keeps_limit_unknown_used() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/postpaid/spending-limits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "limits": [{"period": "monthly", "limit": {"val": "10000"}}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/invoice-preview"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let a = XaiAdapter::new(
            http(),
            server.uri(),
            XaiConfig {
                team_id: "team-1".into(),
            },
        );
        let snap = a
            .fetch(
                &req(QuotaWindow::Monthly),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("partial");
        assert_eq!(snap.values.limit, QuotaMeasure::exact(100_000_000));
        assert_eq!(snap.values.used, QuotaMeasure::Unknown);
        assert_eq!(snap.values.remaining, QuotaMeasure::Unknown);
    }

    #[tokio::test]
    async fn postpaid_limit_failure_keeps_used_unknown_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/postpaid/spending-limits"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/invoice-preview"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total": {"val": "1500", "currency": "USD"}
            })))
            .mount(&server)
            .await;
        let a = XaiAdapter::new(
            http(),
            server.uri(),
            XaiConfig {
                team_id: "team-1".into(),
            },
        );
        let snap = a
            .fetch(
                &req(QuotaWindow::Monthly),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("used-only");
        assert_eq!(snap.values.used, QuotaMeasure::exact(15_000_000));
        assert_eq!(snap.values.limit, QuotaMeasure::Unknown);
    }

    #[test]
    fn monthly_limit_cents_requires_explicit_monthly_period() {
        // 只有 weekly 条目：绝不回退到数组首个条目。
        let weekly_only =
            serde_json::json!({"limits": [{"period": "weekly", "limit": {"val": "2000"}}]});
        assert!(matches!(
            monthly_limit_cents(&weekly_only),
            Err(QuotaError::Parse { .. })
        ));
        // 无 period 字段：不匹配 → Parse。
        let no_period = serde_json::json!({"limits": [{"limit": {"val": "2000"}}]});
        assert!(matches!(
            monthly_limit_cents(&no_period),
            Err(QuotaError::Parse { .. })
        ));
        let monthly =
            serde_json::json!({"limits": [{"period": "monthly", "limit": {"val": "10000"}}]});
        assert_eq!(monthly_limit_cents(&monthly).unwrap(), 10000);
        // 显式非 USD 币种 → Parse。
        let bad_currency = serde_json::json!({"limits": [{"period": "monthly", "currency": "CNY", "limit": {"val": "10000"}}]});
        let err = monthly_limit_cents(&bad_currency).expect_err("bad currency");
        match err {
            QuotaError::Parse { detail } => {
                // 远端 currency 值不得拼入错误消息。
                assert!(!detail.contains("CNY"));
            }
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn prepaid_rejects_wrong_currency() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/prepaid/balance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total": {"val": "5000", "currency": "CNY"}
            })))
            .mount(&server)
            .await;
        let a = XaiAdapter::new(
            http(),
            server.uri(),
            XaiConfig {
                team_id: "team-1".into(),
            },
        );
        let err = a
            .fetch(
                &req(QuotaWindow::Overall),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect_err("currency");
        match err {
            QuotaError::Parse { detail } => {
                // 远端 currency 值不得拼入错误消息。
                assert!(!detail.contains("CNY"));
            }
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn prepaid_rejects_negative_and_overflow_cents() {
        for val in ["-5000", "99999999999999999999"] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1/billing/teams/team-1/prepaid/balance"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "total": {"val": val, "currency": "USD"}
                })))
                .mount(&server)
                .await;
            let a = XaiAdapter::new(
                http(),
                server.uri(),
                XaiConfig {
                    team_id: "team-1".into(),
                },
            );
            let err = a
                .fetch(
                    &req(QuotaWindow::Overall),
                    Some(&cred()),
                    &agent_domain::CancellationToken::new(),
                )
                .await
                .expect_err("bad cents");
            assert!(matches!(err, QuotaError::Parse { .. }));
        }
    }

    #[tokio::test]
    async fn invoice_wrong_currency_keeps_limit_and_unknown_used() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/postpaid/spending-limits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "limits": [{"period": "monthly", "limit": {"val": "10000"}}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/invoice-preview"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total": {"val": "3000", "currency": "CNY"}
            })))
            .mount(&server)
            .await;
        let a = XaiAdapter::new(
            http(),
            server.uri(),
            XaiConfig {
                team_id: "team-1".into(),
            },
        );
        let snap = a
            .fetch(
                &req(QuotaWindow::Monthly),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("partial: limit only");
        assert_eq!(snap.values.limit, QuotaMeasure::exact(100_000_000));
        assert_eq!(snap.values.used, QuotaMeasure::Unknown);
        assert_eq!(snap.values.remaining, QuotaMeasure::Unknown);
    }

    #[tokio::test]
    async fn postpaid_both_403_keeps_forbidden_with_combined_message() {
        // 两端同时失败不统一塌缩为 Other：403 → Forbidden 保留。
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/postpaid/spending-limits"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/invoice-preview"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let a = XaiAdapter::new(
            http(),
            server.uri(),
            XaiConfig {
                team_id: "team-1".into(),
            },
        );
        let err = a
            .fetch(
                &req(QuotaWindow::Monthly),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect_err("both fail");
        match err {
            QuotaError::Forbidden { detail } => {
                assert_eq!(
                    detail,
                    "xai: both postpaid endpoints failed (limit: forbidden; used: forbidden)"
                );
            }
            other => panic!("expected Forbidden, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn postpaid_both_401_returns_unauthorized() {
        // 401 是凭证级错误：两端 401 → Unauthorized，不塌缩为 Other。
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/postpaid/spending-limits"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/invoice-preview"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let a = XaiAdapter::new(
            http(),
            server.uri(),
            XaiConfig {
                team_id: "team-1".into(),
            },
        );
        let err = a
            .fetch(
                &req(QuotaWindow::Monthly),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect_err("both 401");
        match err {
            QuotaError::Unauthorized { detail } => {
                assert!(detail.contains("both postpaid endpoints failed"));
            }
            other => panic!("expected Unauthorized, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn postpaid_both_429_keeps_rate_limit_and_max_retry_after() {
        // 两端 429：保留 RateLimited，retry_after 取两端较大者（保守等待更长）。
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/postpaid/spending-limits"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "2"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/invoice-preview"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "5"))
            .mount(&server)
            .await;
        let a = XaiAdapter::new(
            http(),
            server.uri(),
            XaiConfig {
                team_id: "team-1".into(),
            },
        );
        let err = a
            .fetch(
                &req(QuotaWindow::Monthly),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect_err("both 429");
        match err {
            QuotaError::RateLimited {
                detail,
                retry_after_ms,
            } => {
                assert_eq!(retry_after_ms, Some(5_000));
                assert!(detail.contains("both postpaid endpoints failed"));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn postpaid_errors_do_not_leak_remote_detail_or_secrets() {
        // 非 2xx 正文（可能含 token / 远端 detail）绝不进入错误文本；组合消息
        // 必须是固定描述，不含远端 detail。
        let secret = "sk-xai-SECRET-9f3c";
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/postpaid/spending-limits"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error": {"message": format!("token leaked {secret}")}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/billing/teams/team-1/invoice-preview"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "detail": secret
            })))
            .mount(&server)
            .await;
        let a = XaiAdapter::new(
            http(),
            server.uri(),
            XaiConfig {
                team_id: "team-1".into(),
            },
        );
        let err = a
            .fetch(
                &req(QuotaWindow::Monthly),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect_err("both 403 with secret bodies");
        match err {
            QuotaError::Forbidden { detail } => {
                assert!(
                    !detail.contains(secret),
                    "secret leaked into error: {detail}"
                );
                assert_eq!(
                    detail,
                    "xai: both postpaid endpoints failed (limit: forbidden; used: forbidden)"
                );
            }
            other => panic!("expected Forbidden, got {other:?}"),
        }
    }

    #[test]
    fn merge_dual_failures_preserves_classifications() {
        // Cancelled 最高优先。
        let err = merge_dual_failures(QuotaError::Cancelled, QuotaError::forbidden("x"));
        assert!(matches!(err, QuotaError::Cancelled));
        // 鉴权（401）优先于 Forbidden，且组合消息不含子错误 detail（潜在 secret）。
        let err = merge_dual_failures(
            QuotaError::unauthorized("sk-secret"),
            QuotaError::forbidden("sk-secret"),
        );
        match err {
            QuotaError::Unauthorized { detail } => {
                assert!(!detail.contains("sk-secret"));
                assert!(detail.contains("limit: unauthorized"));
            }
            other => panic!("expected Unauthorized, got {other:?}"),
        }
        // RateLimited 保留两端最大 retry_after。
        let err = merge_dual_failures(
            QuotaError::rate_limited("x", Some(3_000)),
            QuotaError::rate_limited("x", Some(5_000)),
        );
        match err {
            QuotaError::RateLimited { retry_after_ms, .. } => {
                assert_eq!(retry_after_ms, Some(5_000))
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
        // Forbidden 优先于 retryable。
        let err = merge_dual_failures(
            QuotaError::transient("x", Some(503), None),
            QuotaError::forbidden("x"),
        );
        assert!(matches!(err, QuotaError::Forbidden { .. }));
        // retryable 保留（Timeout）。
        let err = merge_dual_failures(QuotaError::timeout("x"), QuotaError::parse("x"));
        assert!(err.retryable());
        // 两端均无高优分类才走 Other 组合消息。
        let err = merge_dual_failures(QuotaError::parse("a"), QuotaError::other("b"));
        match err {
            QuotaError::Other { detail } => {
                assert!(detail.contains("both postpaid endpoints failed"));
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn supports_both_overall_and_monthly_usd() {
        let a = XaiAdapter::new(
            http(),
            "https://example.test",
            XaiConfig {
                team_id: "t".into(),
            },
        );
        assert!(a.supports(&req(QuotaWindow::Overall)));
        assert!(a.supports(&req(QuotaWindow::Monthly)));
        assert!(!a.supports(&QuotaRequest {
            window: QuotaWindow::Rolling5h,
            ..req(QuotaWindow::Monthly)
        }));
        assert!(!a.supports(&QuotaRequest {
            unit: QuotaUnit::Cost {
                currency: "CNY".into(),
            },
            ..req(QuotaWindow::Monthly)
        }));
    }
}
