//! Anthropic 配额适配器。
//!
//! 事实源（2026 Enterprise Spend Limits API 契约）：组织级 monthly spend limit
//! 取 `GET https://api.anthropic.com/v1/organizations/spend_limits/effective`，
//! 需 Admin key（scope `read:spend_limits`），通过 `x-api-key` 头携带，并附带
//! `anthropic-version` 头。
//!
//! 响应是 `data[]` 数组（分页），每条含 `scope={type:"user", user_id}`、
//! `amount`、`currency`、`period=monthly`、`period_to_date_spend`。
//! `amount` / `period_to_date_spend` 为可空 decimal string，单位是「美分」
//! （USD minor unit），支持分数美分（最多 4 位小数：1 cent = 10_000 micros）；
//! 超精度、负数、溢出、非法格式一律 `Parse`（不截断、不钳位，避免伪造读数）。
//! **仅显式 `null`** 表示无硬上限（`amount` → limit = Infinite；
//! `period_to_date_spend` → used = Unknown）；字段缺失（而非 `null`）一律
//! `Parse`。每个 user 作用域条目还必须携带 `currency=USD` 与 `period=monthly`，
//! 缺失/不匹配一律 `Parse`。仅 Enterprise usage credits 反映；Consumer
//! 5h/weekly Claude 限额无公开 API，故不提供。
//!
//! 分页：响应 `next_page`（cursor）经 URL 编码后作为 `page` 查询参数继续请求
//! （cursor 可能含 `&`/`=`/空格等保留字符），直到响应不再带 `next_page`；
//! 翻满 [`MAX_PAGES`] 页仍有 cursor 必须报 `Parse` 错误，不得截断当作完整数据。
//!
//! 多 user 场景：上层在 `QuotaScope.account_id` 指定目标 user，适配器在 `data[]`
//! 中按 `scope.type == "user"` 且 `scope.user_id == account_id` 选择对应条目；
//! 非 user 作用域条目不参与匹配；未命中则 `Unsupported`。全员聚合属本地累加，
//! 不在本适配器职责内（usage-ledger 是唯一累加源）。
//!
//! Reset 语义：window 为「自然月」，月初（1 号 00:00 UTC）重置，快照给出
//! `QuotaReset::Absolute { at: 下月1号UTC, uncertain: false }`。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use provider_api::{CredentialKind, ResolvedCredential};
use provider_runtime::http::HttpClient;

use crate::adapters::http_util::{api_get, now_millis, redact_endpoint};
use crate::adapters::money::json_decimal_string;
use crate::{
    AdapterKind, Confidence, QuotaAdapter, QuotaError, QuotaMeasure, QuotaProvenance, QuotaRequest,
    QuotaReset, QuotaSnapshot, QuotaUnit, QuotaValues, QuotaWindow,
};

const BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// data[] 分页上限，防止异常远端无限翻页。
const MAX_PAGES: usize = 16;
/// 1 cent = 10_000 micros（1 USD = 100_000_000 micros）。
const CENTS_TO_MICROS: u64 = 10_000;
/// 分数美分最多 4 位小数（1 cent = 10_000 micros，再多无法无损换算）。
const MAX_CENT_FRACTION_DIGITS: usize = 4;

/// Anthropic 组织 Admin API key 额度适配器（monthly USD）。
pub fn adapter(http: Arc<HttpClient>) -> Box<dyn QuotaAdapter> {
    Box::new(AnthropicAdapter::new(http, BASE))
}

struct AnthropicAdapter {
    http: Arc<HttpClient>,
    base: String,
}

impl AnthropicAdapter {
    fn new(http: Arc<HttpClient>, base: impl Into<String>) -> Self {
        Self {
            http,
            base: base.into(),
        }
    }
}

#[async_trait]
impl QuotaAdapter for AnthropicAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::ApiKeyApi
    }

    fn supports(&self, request: &QuotaRequest) -> bool {
        matches!(
            (request.window, &request.unit),
            (QuotaWindow::Monthly, QuotaUnit::Cost { currency })
                if currency.eq_ignore_ascii_case("USD")
        )
    }

    async fn fetch(
        &self,
        request: &QuotaRequest,
        credential: Option<&ResolvedCredential>,
        cancel: &agent_domain::CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError> {
        let credential = credential.ok_or_else(|| {
            QuotaError::unauthorized("anthropic admin api key (x-api-key) required")
        })?;
        if credential.kind() != CredentialKind::ApiKey {
            return Err(QuotaError::unauthorized(
                "anthropic: x-api-key requires an Admin API key",
            ));
        }
        let headers = x_api_key_headers(credential);
        // 目标 user = account_id（非机密标识）。
        let target_user = request.scope.account_id.as_str().to_string();

        // 翻页：next_page 原样作为 page=<cursor> 参数，直到响应无 next_page。
        let mut entries = Vec::new();
        let mut page: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let url = match &page {
                Some(cursor) => format!(
                    "{}/v1/organizations/spend_limits/effective?page={}",
                    self.base,
                    percent_encode_query_param(cursor)
                ),
                None => format!("{}/v1/organizations/spend_limits/effective", self.base),
            };
            let body = api_get(self.http.as_ref(), &url, &headers, cancel).await?;
            let (items, next_page) = parse_page(&body)?;
            entries.extend(items);
            match next_page {
                Some(cursor) => page = Some(cursor),
                None => {
                    page = None;
                    break;
                }
            }
        }
        if page.is_some() {
            // 翻满页上限仍有 cursor：数据未取全，不得截断当作完整结果。
            return Err(QuotaError::parse(format!(
                "anthropic: spend_limits pagination exceeded {MAX_PAGES} pages without exhaustion"
            )));
        }

        // 在 data[] 中按 scope.type=="user" 且 scope.user_id == target_user 选择。
        let member = entries
            .iter()
            .find(|e| e.user_id == target_user)
            .ok_or_else(|| {
                QuotaError::unsupported(format!(
                    "anthropic: user '{target_user}' not found in spend_limits data[]"
                ))
            })?;

        // amount 为 null → 无硬上限 → limit=Infinite。
        let limit = match member.amount_micros {
            Some(m) => QuotaMeasure::exact(m),
            None => QuotaMeasure::Infinite,
        };
        let used = match member.period_to_date_spend_micros {
            Some(m) => QuotaMeasure::exact(m),
            None => QuotaMeasure::Unknown,
        };
        let remaining = match (limit, used) {
            (QuotaMeasure::Infinite, QuotaMeasure::Exact(_)) => QuotaMeasure::Infinite,
            (QuotaMeasure::Infinite, _) => QuotaMeasure::Infinite,
            (QuotaMeasure::Exact(l), QuotaMeasure::Exact(u)) => match l.checked_sub(u) {
                Some(v) => QuotaMeasure::exact(v),
                // used > limit：负数无法表示，诚实 Unknown，不伪造 0。
                None => QuotaMeasure::Unknown,
            },
            _ => QuotaMeasure::Unknown,
        };

        let now = now_millis();
        Ok(QuotaSnapshot {
            scope: request.scope.clone(),
            window: request.window,
            unit: request.unit.clone(),
            values: QuotaValues::new(used, limit, remaining),
            // 自然月：月初（下月 1 号）00:00 UTC 重置。
            reset: QuotaReset::Absolute {
                at: next_month_start_timestamp(),
                uncertain: false,
            },
            confidence: Confidence::Exact,
            provenance: QuotaProvenance {
                adapter_kind: AdapterKind::ApiKeyApi,
                source: "anthropic.admin".to_string(),
                endpoint: Some(redact_endpoint(&format!(
                    "{}/v1/organizations/spend_limits/effective",
                    self.base
                ))),
                fetched_at: now,
                observed_at: Some(now),
                selector_version: Some(ANTHROPIC_VERSION.to_string()),
                stale: false,
            },
        })
    }
}

/// 解析 data[] 一页：返回 (条目列表, next_page cursor)。
fn parse_page(body: &serde_json::Value) -> Result<(Vec<MemberEntry>, Option<String>), QuotaError> {
    let data = body
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| QuotaError::parse("anthropic: response missing data[] array"))?;
    let mut entries = Vec::with_capacity(data.len());
    for item in data {
        // scope={type:"user", user_id}；非 user 作用域条目不参与账户匹配。
        let scope = item
            .get("scope")
            .and_then(|v| v.as_object())
            .ok_or_else(|| QuotaError::parse("anthropic: data[] item missing scope object"))?;
        let scope_type = scope
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| QuotaError::parse("anthropic: scope.type missing"))?;
        if scope_type != "user" {
            continue;
        }
        // 仅 user 作用域条目参与匹配，且必须携带完整契约字段。
        let user_id = scope
            .get("user_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| QuotaError::parse("anthropic: scope.user_id missing for user scope"))?;
        if user_id.is_empty() {
            return Err(QuotaError::parse("anthropic: scope.user_id empty"));
        }
        let currency = item
            .get("currency")
            .and_then(|v| v.as_str())
            .ok_or_else(|| QuotaError::parse("anthropic: entry missing currency"))?;
        if !currency.eq_ignore_ascii_case("USD") {
            // 远端原始值不回显进 detail（可能含密钥/token），只留固定安全描述。
            return Err(QuotaError::parse("anthropic: unexpected currency"));
        }
        let period = item
            .get("period")
            .and_then(|v| v.as_str())
            .ok_or_else(|| QuotaError::parse("anthropic: entry missing period"))?;
        if !period.eq_ignore_ascii_case("monthly") {
            // 同上：远端原始值不回显。
            return Err(QuotaError::parse("anthropic: unexpected period"));
        }
        let amount_micros = nullable_cents_field(item, "amount")?;
        let period_to_date_spend_micros = nullable_cents_field(item, "period_to_date_spend")?;
        entries.push(MemberEntry {
            user_id: user_id.to_string(),
            amount_micros,
            period_to_date_spend_micros,
        });
    }
    let next_page = body
        .get("next_page")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Ok((entries, next_page))
}

struct MemberEntry {
    user_id: String,
    amount_micros: Option<u64>,
    period_to_date_spend_micros: Option<u64>,
}

/// 取 data[] 条目中可空的 decimal string 字段（单位=美分），精确换算为 micros。
///
/// **仅显式 `null`** → `None`；字段缺失（而非 `null`）→ `Parse`；
/// 非法、负数、超 4 位小数、溢出 → `Parse`。所有错误 detail 为固定安全描述，
/// 不回显远端原始值（可能携带密钥/token）。
fn nullable_cents_field(item: &serde_json::Value, field: &str) -> Result<Option<u64>, QuotaError> {
    match item.get(field) {
        None => Err(QuotaError::parse(format!(
            "anthropic: missing {field} field"
        ))),
        Some(serde_json::Value::Null) => Ok(None),
        Some(value) => Ok(Some(cents_decimal_to_micros(value, field)?)),
    }
}

/// 把 decimal string（单位=美分，支持分数美分）精确换算为 micros。
///
/// 1 cent = 10_000 micros，因此最多支持 4 位小数；超精度、负数、溢出、非法格式
/// 一律 `Parse`（不截断、不钳位，避免伪造读数）。接受 JSON 字符串（契约形态）
/// 与数字（兼容远端以 number 下发）两种形态。错误 detail 一律固定安全描述，
/// 不回显原始字符串（可能被注入密钥/token）。
fn cents_decimal_to_micros(value: &serde_json::Value, field: &str) -> Result<u64, QuotaError> {
    let s = json_decimal_string(value, &format!("anthropic {field}"))?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(QuotaError::parse(format!("anthropic: {field} empty")));
    }
    let (negative, rest) = match trimmed.as_bytes() {
        [b'-', rest @ ..] => (true, std::str::from_utf8(rest).unwrap_or("")),
        [b'+', rest @ ..] => (false, std::str::from_utf8(rest).unwrap_or("")),
        _ => (false, trimmed),
    };
    if negative {
        return Err(QuotaError::parse(format!(
            "anthropic: {field} must not be negative"
        )));
    }
    if rest.is_empty() {
        return Err(QuotaError::parse(format!(
            "anthropic: {field} invalid decimal"
        )));
    }

    let mut split = rest.splitn(2, '.');
    let int_part = split.next().unwrap_or("");
    let frac_part = split.next().unwrap_or("");
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(QuotaError::parse(format!(
            "anthropic: {field} invalid decimal"
        )));
    }
    if !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(QuotaError::parse(format!(
            "anthropic: {field} invalid decimal"
        )));
    }
    if frac_part.len() > MAX_CENT_FRACTION_DIGITS {
        // 1 cent = 10_000 micros：超过 4 位小数的美分无法无损换算，必须报错。
        return Err(QuotaError::parse(format!(
            "anthropic: {field} exceeds {MAX_CENT_FRACTION_DIGITS} fractional digits"
        )));
    }

    let int_cents: u64 = if int_part.is_empty() {
        0
    } else {
        int_part
            .parse()
            .map_err(|_| QuotaError::parse(format!("anthropic: {field} overflow")))?
    };
    let mut frac4 = [b'0'; MAX_CENT_FRACTION_DIGITS];
    frac4[..frac_part.len()].copy_from_slice(frac_part.as_bytes());
    let frac_micros: u64 = std::str::from_utf8(&frac4)
        .unwrap_or("0")
        .parse()
        .map_err(|_| QuotaError::parse(format!("anthropic: {field} overflow")))?;

    int_cents
        .checked_mul(CENTS_TO_MICROS)
        .and_then(|v| v.checked_add(frac_micros))
        .ok_or_else(|| QuotaError::parse(format!("anthropic: {field} overflow")))
}

/// 对查询参数值做 RFC 3986 百分号编码。
///
/// cursor 可能含 `&`、`=`、`?`、空格等保留字符，必须编码后才能拼进 URL，
/// 否则会被解析成额外参数。
fn percent_encode_query_param(value: &str) -> String {
    const UNRESERVED: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~";
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        if UNRESERVED.as_bytes().contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 构造 Anthropic Admin 头：`x-api-key` + `anthropic-version`。不写 `Authorization: Bearer`。
fn x_api_key_headers(credential: &ResolvedCredential) -> Vec<(String, String)> {
    vec![
        (
            "x-api-key".to_string(),
            credential.expose_secret().to_string(),
        ),
        (
            "anthropic-version".to_string(),
            ANTHROPIC_VERSION.to_string(),
        ),
    ]
}

/// 下月 1 号 00:00 UTC 的 Timestamp（自然月 reset 时刻）。
fn next_month_start_timestamp() -> agent_domain::Timestamp {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QuotaScope;
    use agent_domain::{ProviderId, TenantId};
    use wiremock::matchers::{header, method, path, query_param};
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

    fn req(user: &str) -> QuotaRequest {
        QuotaRequest {
            scope: QuotaScope::new(
                TenantId::new("t"),
                crate::AccountId::new(user),
                ProviderId::new("anthropic"),
                None,
            ),
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Cost {
                currency: "USD".into(),
            },
        }
    }

    fn cred() -> ResolvedCredential {
        ResolvedCredential::new(CredentialKind::ApiKey, "sk-ant-admin-FAKE")
    }

    fn assert_month_reset(snap: &QuotaSnapshot) {
        // 月初 00:00 UTC 重置：Absolute + uncertain=false，at 在 (now, now+32d]。
        let reset = match &snap.reset {
            QuotaReset::Absolute { at, uncertain } => {
                assert!(!uncertain);
                at.as_unix_millis()
            }
            other => panic!("expected Absolute reset, got {other:?}"),
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        assert!(reset > now_ms);
        assert!(reset <= now_ms + 32 * 86_400 * 1_000);
    }

    #[tokio::test]
    async fn selects_user_scoped_entry_with_decimal_cents() {
        // Contract fixture（fixtures/quota/anthropic_spend_limits.json）作为 wiremock 响应：
        // data[] 含 user-alice（amount=10000.50、spend=3000.25，带小数）与 user-bob（amount=null）。
        let body: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/quota/anthropic_spend_limits.json"
        ))
        .expect("fixture json");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .and(header("x-api-key", "sk-ant-admin-FAKE"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let a = AnthropicAdapter::new(http(), server.uri());
        let snap = a
            .fetch(
                &req("user-alice"),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("ok");
        // canonical micros：amount 10000.50 cents -> 100_005_000 micros；
        // period_to_date_spend 3000.25 cents -> 30_002_500 micros；remaining = limit - used = 70_002_500。
        assert_eq!(snap.values.limit, QuotaMeasure::exact(100_005_000));
        assert_eq!(snap.values.used, QuotaMeasure::exact(30_002_500));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(70_002_500));
        assert_eq!(snap.confidence, Confidence::Exact);
        assert_month_reset(&snap);
        // bob 的 amount=null 只影响 bob 自身：按 user_id 精确选择 alice，limit 仍为有限 Exact。
        assert_ne!(snap.values.limit, QuotaMeasure::Infinite);
        // 不使用 Bearer。
        assert!(snap.provenance.source == "anthropic.admin");
    }

    #[tokio::test]
    async fn null_amount_means_infinite_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"scope": {"type": "user", "user_id": "user-no-limit"}, "amount": null, "currency": "USD", "period": "monthly", "period_to_date_spend": "1234"}
                ],
                "next_page": null
            })))
            .mount(&server)
            .await;
        let a = AnthropicAdapter::new(http(), server.uri());
        let snap = a
            .fetch(
                &req("user-no-limit"),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("ok");
        assert_eq!(snap.values.limit, QuotaMeasure::Infinite);
        assert_eq!(snap.values.used, QuotaMeasure::exact(12_340_000));
        assert_eq!(snap.values.remaining, QuotaMeasure::Infinite);
    }

    #[tokio::test]
    async fn null_period_to_date_spend_means_unknown_used() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"scope": {"type": "user", "user_id": "user-x"}, "amount": "5000", "currency": "USD", "period": "monthly", "period_to_date_spend": null}
                ],
                "next_page": null
            })))
            .mount(&server)
            .await;
        let a = AnthropicAdapter::new(http(), server.uri());
        let snap = a
            .fetch(
                &req("user-x"),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("ok");
        assert_eq!(snap.values.limit, QuotaMeasure::exact(50_000_000));
        assert_eq!(snap.values.used, QuotaMeasure::Unknown);
        assert_eq!(snap.values.remaining, QuotaMeasure::Unknown);
    }

    #[tokio::test]
    async fn missing_user_returns_unsupported_and_non_user_scopes_are_skipped() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"scope": {"type": "organization"}, "amount": "1", "currency": "USD", "period": "monthly", "period_to_date_spend": "1"},
                    {"scope": {"type": "user", "user_id": "someone-else"}, "amount": "100", "currency": "USD", "period": "monthly", "period_to_date_spend": "1"}
                ],
                "next_page": null
            })))
            .mount(&server)
            .await;
        let a = AnthropicAdapter::new(http(), server.uri());
        let err = a
            .fetch(
                &req("ghost"),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect_err("unsupported");
        assert!(matches!(err, QuotaError::Unsupported { .. }));
    }

    #[tokio::test]
    async fn missing_amount_field_is_parse_error_not_infinite() {
        // 缺 amount 字段（而非显式 null）→ Parse；绝不当作无硬上限。
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"scope": {"type": "user", "user_id": "user-x"}, "currency": "USD", "period": "monthly", "period_to_date_spend": "1"}
                ],
                "next_page": null
            })))
            .mount(&server)
            .await;
        let a = AnthropicAdapter::new(http(), server.uri());
        let err = a
            .fetch(
                &req("user-x"),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect_err("missing amount");
        assert!(matches!(err, QuotaError::Parse { detail } if detail.contains("amount")));
    }

    #[tokio::test]
    async fn missing_period_to_date_spend_field_is_parse_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"scope": {"type": "user", "user_id": "user-x"}, "amount": "5000", "currency": "USD", "period": "monthly"}
                ],
                "next_page": null
            })))
            .mount(&server)
            .await;
        let a = AnthropicAdapter::new(http(), server.uri());
        let err = a
            .fetch(
                &req("user-x"),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect_err("missing period_to_date_spend");
        assert!(
            matches!(err, QuotaError::Parse { detail } if detail.contains("period_to_date_spend"))
        );
    }

    #[tokio::test]
    async fn missing_or_wrong_currency_and_period_are_parse_errors() {
        for entry in [
            serde_json::json!({"scope": {"type": "user", "user_id": "user-x"}, "amount": "5000", "period": "monthly", "period_to_date_spend": "1"}),
            serde_json::json!({"scope": {"type": "user", "user_id": "user-x"}, "amount": "5000", "currency": "CNY", "period": "monthly", "period_to_date_spend": "1"}),
            serde_json::json!({"scope": {"type": "user", "user_id": "user-x"}, "amount": "5000", "currency": "USD", "period_to_date_spend": "1"}),
            serde_json::json!({"scope": {"type": "user", "user_id": "user-x"}, "amount": "5000", "currency": "USD", "period": "weekly", "period_to_date_spend": "1"}),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1/organizations/spend_limits/effective"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": [entry],
                    "next_page": null
                })))
                .mount(&server)
                .await;
            let a = AnthropicAdapter::new(http(), server.uri());
            let err = a
                .fetch(
                    &req("user-x"),
                    Some(&cred()),
                    &agent_domain::CancellationToken::new(),
                )
                .await
                .expect_err("contract violation");
            assert!(matches!(err, QuotaError::Parse { .. }));
        }
    }

    #[tokio::test]
    async fn remote_raw_values_never_leak_into_error_detail() {
        // 恶意远端把 token 注入 currency / period / decimal 字段；错误 detail
        // 必须是固定安全描述，绝不回显原始字符串（含密钥）。
        let token = "sk-ant-injected-SECRET-TOKEN";
        let overflow_raw = "184467440737095516160000";
        let frac_raw = "1.00001";
        let cases: [(&str, serde_json::Value); 6] = [
            // currency 不匹配（含 token）→ 固定 "unexpected currency"。
            (
                "unexpected currency",
                serde_json::json!({"scope": {"type": "user", "user_id": "user-x"}, "amount": "5000", "currency": format!("USD-{token}"), "period": "monthly", "period_to_date_spend": "1"}),
            ),
            // period 不匹配（含 token）→ 固定 "unexpected period"。
            (
                "unexpected period",
                serde_json::json!({"scope": {"type": "user", "user_id": "user-x"}, "amount": "5000", "currency": "USD", "period": format!("monthly-{token}"), "period_to_date_spend": "1"}),
            ),
            // amount 非法格式（含 token）→ 固定 "invalid decimal"。
            (
                "invalid decimal",
                serde_json::json!({"scope": {"type": "user", "user_id": "user-x"}, "amount": format!("100{token}"), "currency": "USD", "period": "monthly", "period_to_date_spend": "1"}),
            ),
            // amount 超精度且小数含 token → 固定 "invalid decimal"（非数字检查先于精度检查）。
            (
                "invalid decimal",
                serde_json::json!({"scope": {"type": "user", "user_id": "user-x"}, "amount": format!("1.0000{token}"), "currency": "USD", "period": "monthly", "period_to_date_spend": "1"}),
            ),
            // amount 溢出：纯数字溢出 u64，原始串同样不得回显。
            (
                "overflow",
                serde_json::json!({"scope": {"type": "user", "user_id": "user-x"}, "amount": overflow_raw, "currency": "USD", "period": "monthly", "period_to_date_spend": "1"}),
            ),
            // period_to_date_spend 非法（含 token）→ 固定 "invalid decimal"。
            (
                "invalid decimal",
                serde_json::json!({"scope": {"type": "user", "user_id": "user-x"}, "amount": "5000", "currency": "USD", "period": "monthly", "period_to_date_spend": format!("1{token}")}),
            ),
        ];
        for (expected_fragment, entry) in cases {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1/organizations/spend_limits/effective"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": [entry],
                    "next_page": null
                })))
                .mount(&server)
                .await;
            let a = AnthropicAdapter::new(http(), server.uri());
            let err = a
                .fetch(
                    &req("user-x"),
                    Some(&cred()),
                    &agent_domain::CancellationToken::new(),
                )
                .await
                .expect_err("malicious payload must be rejected");
            assert!(
                matches!(&err, QuotaError::Parse { detail } if detail.contains(expected_fragment)),
                "expected fixed fragment '{expected_fragment}', got {err}"
            );
            let detail = match &err {
                QuotaError::Parse { detail } => detail,
                other => panic!("expected Parse, got {other:?}"),
            };
            for leaked in [token, overflow_raw, frac_raw] {
                assert!(
                    !detail.contains(leaked),
                    "detail leaked remote raw value '{leaked}': {detail}"
                );
            }
        }
    }

    #[test]
    fn decimal_errors_never_echo_raw_value() {
        // 纯数字超精度/溢出/非法输入：detail 为固定描述，不包含原始值。
        let overflow_raw = "184467440737095516160000";
        let frac_raw = "1.00001";
        for (input, fragment, raw) in [
            (serde_json::json!(frac_raw), "fractional digits", frac_raw),
            (serde_json::json!(overflow_raw), "overflow", overflow_raw),
            (serde_json::json!("1e9"), "invalid decimal", "1e9"),
        ] {
            let err = cents_decimal_to_micros(&input, "amount").expect_err("must reject");
            assert!(
                matches!(&err, QuotaError::Parse { detail } if detail.contains(fragment)),
                "expected fragment '{fragment}', got {err}"
            );
            assert!(
                !err.to_string().contains(raw),
                "error echoed raw value '{raw}': {err}"
            );
        }
    }

    #[tokio::test]
    async fn malformed_other_user_entry_is_parse_error() {
        // 任一个 user 作用域条目缺字段都必须报错，不能静默跳过（避免漏数据）。
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"scope": {"type": "user", "user_id": "user-other"}, "amount": "1", "currency": "USD", "period": "monthly"},
                    {"scope": {"type": "user", "user_id": "user-target"}, "amount": "100", "currency": "USD", "period": "monthly", "period_to_date_spend": "1"}
                ],
                "next_page": null
            })))
            .mount(&server)
            .await;
        let a = AnthropicAdapter::new(http(), server.uri());
        let err = a
            .fetch(
                &req("user-target"),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect_err("malformed other entry");
        assert!(matches!(err, QuotaError::Parse { .. }));
    }

    #[tokio::test]
    async fn cursor_with_reserved_chars_is_percent_encoded() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"scope": {"type": "user", "user_id": "user-a"}, "amount": "100", "currency": "USD", "period": "monthly", "period_to_date_spend": "1"}],
                "next_page": "cur &next=x=y"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"scope": {"type": "user", "user_id": "user-b"}, "amount": "7000.25", "currency": "USD", "period": "monthly", "period_to_date_spend": "2000.5"}],
                "next_page": null
            })))
            .mount(&server)
            .await;
        let a = AnthropicAdapter::new(http(), server.uri());
        let snap = a
            .fetch(
                &req("user-b"),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("ok");
        assert_eq!(snap.values.limit, QuotaMeasure::exact(70_002_500));
        // 第二页请求必须携带编码后的 cursor，而不是裸保留字符。
        let requests = server.received_requests().await.unwrap();
        assert!(requests.iter().any(|r| r
            .url
            .query()
            .is_some_and(|q| q.contains("page=cur%20%26next%3Dx%3Dy"))));
        assert!(!requests.iter().any(|r| r
            .url
            .query()
            .is_some_and(|q| q.contains("page=cur &next=x=y"))));
    }

    #[tokio::test]
    async fn paginates_by_passing_next_page_verbatim_as_page_param() {
        let server = MockServer::start().await;
        // 第一页（无 page 参数）：目标 user 在第二页。
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"scope": {"type": "user", "user_id": "user-a"}, "amount": "100", "currency": "USD", "period": "monthly", "period_to_date_spend": "1"}],
                "next_page": "cur-next-page"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // 第二页：必须携带 page=cur-next-page（原样透传）。
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .and(query_param("page", "cur-next-page"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"scope": {"type": "user", "user_id": "user-b"}, "amount": "7000.25", "currency": "USD", "period": "monthly", "period_to_date_spend": "2000.5"}],
                "next_page": null
            })))
            .mount(&server)
            .await;
        let a = AnthropicAdapter::new(http(), server.uri());
        let snap = a
            .fetch(
                &req("user-b"),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("ok");
        // 7000.25 cents -> 70_002_500 micros; 2000.5 cents -> 20_005_000 micros.
        assert_eq!(snap.values.limit, QuotaMeasure::exact(70_002_500));
        assert_eq!(snap.values.used, QuotaMeasure::exact(20_005_000));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(49_997_500));
    }

    #[tokio::test]
    async fn page_cap_with_ongoing_cursor_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"scope": {"type": "user", "user_id": "user-x"}, "amount": "100", "currency": "USD", "period": "monthly", "period_to_date_spend": "1"}],
                "next_page": "cur-loop"
            })))
            .mount(&server)
            .await;
        let a = AnthropicAdapter::new(http(), server.uri());
        let err = a
            .fetch(
                &req("user-x"),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect_err("page cap");
        assert!(
            matches!(err, QuotaError::Parse { detail } if detail.contains("pagination exceeded"))
        );
        // 翻满 MAX_PAGES 页后仍带 cursor，必须报错，不静默截断。
        let hits = server.received_requests().await.unwrap();
        assert_eq!(hits.len(), MAX_PAGES);
    }

    #[tokio::test]
    async fn forbidden_is_distinct() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organizations/spend_limits/effective"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let a = AnthropicAdapter::new(http(), server.uri());
        let err = a
            .fetch(
                &req("x"),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect_err("forbidden");
        assert!(matches!(err, QuotaError::Forbidden { .. }));
    }

    #[test]
    fn cents_decimal_conversion_is_exact_and_rejects_bad_input() {
        // 分数美分：1 cent = 10_000 micros，最多 4 位小数。
        assert_eq!(
            cents_decimal_to_micros(&serde_json::json!("12.3456"), "amount").unwrap(),
            123_456
        );
        assert_eq!(
            cents_decimal_to_micros(&serde_json::json!("0.0001"), "amount").unwrap(),
            1
        );
        assert_eq!(
            cents_decimal_to_micros(&serde_json::json!("123.45"), "amount").unwrap(),
            1_234_500
        );
        assert_eq!(
            cents_decimal_to_micros(&serde_json::json!("100"), "amount").unwrap(),
            1_000_000
        );
        assert_eq!(
            cents_decimal_to_micros(&serde_json::json!(".5"), "amount").unwrap(),
            5_000
        );
        // JSON number 形态兼容。
        assert_eq!(
            cents_decimal_to_micros(&serde_json::json!(10000), "amount").unwrap(),
            100_000_000
        );
        // 超精度（>4 位小数）→ Parse。
        assert!(matches!(
            cents_decimal_to_micros(&serde_json::json!("1.00001"), "amount"),
            Err(QuotaError::Parse { .. })
        ));
        // 负数 → Parse。
        assert!(matches!(
            cents_decimal_to_micros(&serde_json::json!("-1.5"), "amount"),
            Err(QuotaError::Parse { .. })
        ));
        // 溢出 → Parse。
        assert!(matches!(
            cents_decimal_to_micros(&serde_json::json!("999999999999999999999999"), "amount"),
            Err(QuotaError::Parse { .. })
        ));
        // 非法格式 → Parse。
        for bad in ["", "abc", "1.2.3", "1e3", "-"] {
            assert!(matches!(
                cents_decimal_to_micros(&serde_json::json!(bad), "amount"),
                Err(QuotaError::Parse { .. })
            ));
        }
    }

    #[test]
    fn supports_only_monthly_usd() {
        let e = AnthropicAdapter::new(http(), "https://example.test");
        assert!(e.supports(&req("m")));
        assert!(!e.supports(&QuotaRequest {
            window: QuotaWindow::Rolling5h,
            ..req("m")
        }));
        assert!(!e.supports(&QuotaRequest {
            unit: QuotaUnit::Cost {
                currency: "CNY".into(),
            },
            ..req("m")
        }));
    }
}
