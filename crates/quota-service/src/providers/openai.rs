//! OpenAI 配额适配器（双端点合成）。
//!
//! 事实源（brief + 官方契约）：
//! - monthly **hard limit** 取 `GET /v1/organization/spend_limit`，响应
//!   `threshold_amount` 为整数「美分」（USD），`currency`、`interval=month`。
//!   该端点**只**给 limit，不含 used。
//! - monthly **used** 取 `GET /v1/organization/costs`（分页、按时间范围），每个
//!   bucket.result.amount.value 为十进制 USD（如 0.06）；跨当月所有 bucket / result
//!   求和得 used。`amount.value` 统一经定点换算到 micros，全程不经过 f64；
//!   负数、溢出、超过 6 位小数一律 `Parse`（不截断、不钳位、不饱和累加）。
//! - 两者均用 `https://api.openai.com` 与 organization **Admin** API key；普通
//!   inference key 无法查询，远端会以 403 拒绝，映射为 `Forbidden`。
//!
//! 合成语义（不伪造）：两个端点并发拉取，分别得到 (limit?, used?)。
//! - 全部成功 → 完整 Exact 快照（limit / used / remaining）。
//! - 仅 limit 成功 → used/remaining = Unknown，仍返回 Exact 的 limit。
//! - 仅 costs 成功 → limit/remaining = Unknown。
//! - 全部失败 → 返回合并分类错误：按优先级保留 `Cancelled` / 鉴权（401 与
//!   Reauthorization）/ `RateLimited`（含 `retry_after_ms`）/ `Forbidden` /
//!   retryable（`Timeout` / `Transient`）等分类，不统一降级为 `Other`；
//!   组合消息只含固定类别标签、不含远端 detail，交由服务层做部分失败处理。
//!
//! month 边界为 UTC 自然月起点（half-open：[month_start, now)）；used 的累加唯一源
//! 是 usage-ledger，本适配器只读远端 costs，不做本地累加。
//!
//! Reset 语义：spend_limit / costs 的窗口是「自然月」，次月 1 号 00:00 UTC 重置，
//! 故快照给出 `QuotaReset::Absolute { at: 下月1号UTC, uncertain: false }`。
//! 若 costs 翻到分页上限仍有 has_more=true，说明 used 未取全，**不得**继续标 Exact
//! 截断伪造——返回 Parse 错误，交由聚合层降级到 ledger 派生。
//!
//! 错误脱敏：远端返回的 `currency` / `interval` / `amount.value` /
//! `threshold_amount` 等原始值一律不拼入错误文本；错误 detail 只含本地固定
//! 描述与固定类别标签，非 2xx 响应正文永不进入错误。

use std::sync::Arc;

use agent_domain::CancellationToken;
use async_trait::async_trait;
use futures::future;
use provider_api::{CredentialKind, ResolvedCredential};
use provider_runtime::http::HttpClient;

use crate::adapters::http_util::{api_get, bearer_headers};
use crate::adapters::money::{cents_to_micros, decimal_string_to_micros, json_decimal_string};
use crate::util::{
    month_start_unix_seconds, next_month_start_timestamp, now_millis, redact_endpoint,
};
use crate::{
    AdapterKind, Confidence, QuotaAdapter, QuotaError, QuotaMeasure, QuotaProvenance, QuotaRequest,
    QuotaReset, QuotaSnapshot, QuotaUnit, QuotaValues, QuotaWindow,
};

const BASE: &str = "https://api.openai.com";
/// costs 分页上限，防止异常远端无限翻页。
const MAX_COST_PAGES: usize = 31;

/// OpenAI 组织 Admin API key 额度适配器（monthly USD）。
pub fn adapter(http: Arc<HttpClient>) -> Box<dyn QuotaAdapter> {
    Box::new(OpenAiAdapter::new(http, BASE))
}

struct OpenAiAdapter {
    http: Arc<HttpClient>,
    base: String,
}

impl OpenAiAdapter {
    fn new(http: Arc<HttpClient>, base: impl Into<String>) -> Self {
        Self {
            http,
            base: base.into(),
        }
    }
}

#[async_trait]
impl QuotaAdapter for OpenAiAdapter {
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
        cancel: &CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError> {
        let credential =
            credential.ok_or_else(|| QuotaError::unauthorized("openai admin api key required"))?;
        if credential.kind() != CredentialKind::ApiKey {
            return Err(QuotaError::unauthorized(
                "openai organization Admin API-key credential required",
            ));
        }
        let headers = bearer_headers(credential);

        // 并发拉取 limit 与 used；各自独立成败，不因一方失败丢弃另一方。
        let limit_fut = self.fetch_limit(&headers, cancel);
        let used_fut = self.fetch_used(&headers, cancel);
        let (limit, used) = future::join(limit_fut, used_fut).await;

        match (limit, used) {
            (Ok(limit_micros), Ok(used_micros)) => {
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
                )
            }
            (Ok(limit_micros), Err(_)) => {
                // costs 失败：limit 已知，used/remaining = Unknown（不伪造 used）。
                self.snapshot(
                    request,
                    QuotaValues::new(
                        QuotaMeasure::Unknown,
                        QuotaMeasure::exact(limit_micros),
                        QuotaMeasure::Unknown,
                    ),
                )
            }
            (Err(_), Ok(used_micros)) => {
                // limit 失败：used 已知，limit/remaining = Unknown（不伪造 limit）。
                self.snapshot(
                    request,
                    QuotaValues::new(
                        QuotaMeasure::exact(used_micros),
                        QuotaMeasure::Unknown,
                        QuotaMeasure::Unknown,
                    ),
                )
            }
            (Err(limit_err), Err(used_err)) => {
                // 全部失败：按优先级合并分类（保留 Cancelled / 鉴权 /
                // RateLimited / Forbidden / retryable），组合消息只含固定标签，
                // 交由服务层做部分失败处理。
                Err(crate::error::merge_dual_failures(
                    limit_err,
                    used_err,
                    "openai: both endpoints failed",
                ))
            }
        }
    }
}

impl OpenAiAdapter {
    async fn fetch_limit(
        &self,
        headers: &[(String, String)],
        cancel: &CancellationToken,
    ) -> Result<u64, QuotaError> {
        let url = format!("{}/v1/organization/spend_limit", self.base);
        let body = api_get(self.http.as_ref(), &url, headers, cancel).await?;
        // threshold_amount 为整数 cents（USD）。
        let cents = body
            .get("threshold_amount")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| QuotaError::parse("openai spend_limit: missing threshold_amount"))?;
        let currency = body
            .get("currency")
            .and_then(|v| v.as_str())
            .ok_or_else(|| QuotaError::parse("openai spend_limit: missing currency"))?;
        if !currency.eq_ignore_ascii_case("USD") {
            // 远端值不拼入错误文本（可能被注入敏感内容）。
            return Err(QuotaError::parse("openai spend_limit: unexpected currency"));
        }
        let interval = body
            .get("interval")
            .and_then(|v| v.as_str())
            .ok_or_else(|| QuotaError::parse("openai spend_limit: missing interval"))?;
        if !interval.eq_ignore_ascii_case("month") {
            // 远端值不拼入错误文本（可能被注入敏感内容）。
            return Err(QuotaError::parse("openai spend_limit: unexpected interval"));
        }
        // 换算失败的 detail 会回显远端 threshold_amount 原始值，收敛为固定描述。
        cents_to_micros(cents)
            .map_err(|_| QuotaError::parse("openai spend_limit: invalid threshold_amount"))
    }

    async fn fetch_used(
        &self,
        headers: &[(String, String)],
        cancel: &CancellationToken,
    ) -> Result<u64, QuotaError> {
        let month_start = month_start_unix_seconds(now_millis());
        let mut total_micros: u64 = 0;
        let mut after: Option<String> = None;
        for _ in 0..MAX_COST_PAGES {
            let url = match &after {
                Some(cursor) => format!(
                    "{}/v1/organization/costs?start_time={month_start}&after={}",
                    self.base,
                    percent_encode_query_param(cursor)
                ),
                None => format!(
                    "{}/v1/organization/costs?start_time={month_start}",
                    self.base
                ),
            };
            let body = api_get(self.http.as_ref(), &url, headers, cancel).await?;
            // data: [ { results: [ { amount: { value: 0.06, currency: "usd" } }, ... ] }, ... ]
            let buckets = body
                .get("data")
                .and_then(|v| v.as_array())
                .ok_or_else(|| QuotaError::parse("openai costs: response missing data[]"))?;
            for bucket in buckets {
                let results = bucket
                    .get("results")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| QuotaError::parse("openai costs: bucket missing results[]"))?;
                for result in results {
                    let amount = result
                        .get("amount")
                        .ok_or_else(|| QuotaError::parse("openai costs: result missing amount"))?;
                    let currency =
                        amount
                            .get("currency")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                QuotaError::parse("openai costs: amount missing currency")
                            })?;
                    if !currency.eq_ignore_ascii_case("USD") {
                        // 远端值不拼入错误文本（可能被注入敏感内容）。
                        return Err(QuotaError::parse(
                            "openai costs: unexpected amount currency",
                        ));
                    }
                    let value = amount
                        .get("value")
                        .ok_or_else(|| QuotaError::parse("openai costs: amount missing value"))?;
                    let micros = usd_value_to_micros(value)?;
                    total_micros = total_micros
                        .checked_add(micros)
                        .ok_or_else(|| QuotaError::parse("openai costs: used total overflow"))?;
                }
            }
            let has_more = body
                .get("has_more")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !has_more {
                return Ok(total_micros);
            }
            after = body
                .get("next_page")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            if after.is_none() {
                // has_more=true 但没有 next_page：远端契约异常，无法继续翻页。
                return Err(QuotaError::parse(
                    "openai costs: has_more=true without next_page cursor",
                ));
            }
        }
        // 翻满 MAX_COST_PAGES 仍有 has_more=true：used 未取全，不得截断标 Exact 伪造。
        Err(QuotaError::parse(format!(
            "openai costs: pagination exceeded {MAX_COST_PAGES} pages without exhaustion"
        )))
    }

    fn snapshot(
        &self,
        request: &QuotaRequest,
        values: QuotaValues,
    ) -> Result<QuotaSnapshot, QuotaError> {
        let now = now_millis();
        Ok(QuotaSnapshot {
            scope: request.scope.clone(),
            window: request.window,
            unit: request.unit.clone(),
            values,
            // 自然月：次月 1 号 00:00 UTC 重置。
            reset: QuotaReset::Absolute {
                at: next_month_start_timestamp(now),
                uncertain: false,
            },
            confidence: Confidence::Exact,
            provenance: QuotaProvenance {
                adapter_kind: AdapterKind::ApiKeyApi,
                source: "openai.admin".to_string(),
                endpoint: Some(redact_endpoint(&self.base)),
                fetched_at: now,
                observed_at: Some(now),
                selector_version: None,
                stale: false,
            },
        })
    }
}

/// 把 costs 的 `amount.value`（USD，JSON 数字或字符串）换算为 micros。
///
/// 统一经定点路径：由 [`json_decimal_string`] 取出字符串（不经过 f64），再经
/// [`decimal_string_to_micros`] 精确换算——负数、溢出、超过 6 位小数一律
/// `Parse`，绝不截断或钳位。
fn usd_value_to_micros(value: &serde_json::Value) -> Result<u64, QuotaError> {
    let s = json_decimal_string(value, "openai costs amount.value")?;
    // 底层换算错误的 detail 会回显远端原始值（负数 / 超精度 / 溢出），此处
    // 收敛为固定描述，远端值不进入错误文本；分类仍是 Parse，不截断不钳位。
    decimal_string_to_micros(&s)
        .map_err(|_| QuotaError::parse("openai costs amount.value: invalid non-negative decimal"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use provider_api::CredentialKind;
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

    fn req() -> QuotaRequest {
        QuotaRequest {
            scope: crate::QuotaScope::new(
                agent_domain::TenantId::new("t"),
                crate::AccountId::new("a"),
                agent_domain::ProviderId::new("openai"),
                None,
            ),
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Cost {
                currency: "USD".into(),
            },
        }
    }

    fn cred() -> ResolvedCredential {
        ResolvedCredential::new(CredentialKind::ApiKey, "sk-admin-FAKE")
    }

    /// 从仓库 fixtures/quota/ 加载 contract fixture（只读，不参与生产代码）。
    fn fixture(name: &str) -> serde_json::Value {
        let raw = match name {
            "openai_costs.json" => {
                include_str!("../../../../fixtures/quota/openai_costs.json")
            }
            "openai_spend_limit.json" => {
                include_str!("../../../../fixtures/quota/openai_spend_limit.json")
            }
            other => panic!("unknown fixture: {other}"),
        };
        serde_json::from_str(raw).expect("fixture must be valid JSON")
    }

    #[tokio::test]
    async fn rejects_non_api_key_credential_before_network() {
        let server = MockServer::start().await;
        let adapter = OpenAiAdapter::new(http(), server.uri());
        let credential = ResolvedCredential::new(CredentialKind::OAuthBearer, "not-an-admin-key");
        let error = adapter
            .fetch(&req(), Some(&credential), &CancellationToken::new())
            .await
            .expect_err("wrong credential kind");

        assert!(matches!(error, QuotaError::Unauthorized { .. }));
        assert!(server
            .received_requests()
            .await
            .expect("requests")
            .is_empty());
    }

    #[tokio::test]
    async fn synthesizes_full_snapshot_from_both_endpoints() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/spend_limit"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(fixture("openai_spend_limit.json")),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture("openai_costs.json")))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let snap = a
            .fetch(&req(), Some(&cred()), &CancellationToken::new())
            .await
            .expect("ok");
        // 10000 cents -> 100M micros; 0.10 USD -> 100_000 micros.
        assert_eq!(snap.values.limit, QuotaMeasure::exact(100_000_000));
        assert_eq!(snap.values.used, QuotaMeasure::exact(100_000));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(99_900_000));
        assert_eq!(snap.confidence, Confidence::Exact);
    }

    #[tokio::test]
    async fn costs_failure_keeps_limit_unknown_used_no_fabrication() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/spend_limit"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "threshold_amount": 5000, "currency": "USD", "interval": "month"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let snap = a
            .fetch(&req(), Some(&cred()), &CancellationToken::new())
            .await
            .expect("partial ok");
        // limit 已知，used/remaining 必须是 Unknown（不得伪造 used）。
        assert_eq!(snap.values.limit, QuotaMeasure::exact(50_000_000));
        assert_eq!(snap.values.used, QuotaMeasure::Unknown);
        assert_eq!(snap.values.remaining, QuotaMeasure::Unknown);
    }

    #[tokio::test]
    async fn limit_failure_keeps_used_unknown_limit_no_fabrication() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/spend_limit"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"results": [{"amount": {"value": 1.0, "currency": "usd"}}]}], "has_more": false
            })))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let snap = a
            .fetch(&req(), Some(&cred()), &CancellationToken::new())
            .await
            .expect("used-only");
        // limit 403 但 costs 成功：used 已知，limit/remaining = Unknown（诚实，不伪造 limit）。
        assert_eq!(snap.values.used, QuotaMeasure::exact(1_000_000));
        assert_eq!(snap.values.limit, QuotaMeasure::Unknown);
        assert_eq!(snap.values.remaining, QuotaMeasure::Unknown);
    }

    #[tokio::test]
    async fn both_endpoints_403_stays_forbidden_without_remote_detail() {
        // 两端 403 → Forbidden（不统一 Other）；响应正文中的 secret 不进入
        // 组合消息（api_get 从不读取非 2xx 正文，组合消息只含固定类别标签）。
        let secret = "sk-remote-body-secret-7f3a";
        let server = MockServer::start().await;
        for endpoint in ["/v1/organization/spend_limit", "/v1/organization/costs"] {
            Mock::given(method("GET"))
                .and(path(endpoint))
                .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                    "error": { "message": secret }
                })))
                .mount(&server)
                .await;
        }
        let a = OpenAiAdapter::new(http(), server.uri());
        let err = a
            .fetch(&req(), Some(&cred()), &CancellationToken::new())
            .await
            .expect_err("both fail");
        let detail = match &err {
            QuotaError::Forbidden { detail } => detail,
            other => panic!("expected Forbidden, got {other:?}"),
        };
        assert!(detail.contains("both endpoints failed"));
        assert!(detail.contains("limit: forbidden"));
        assert!(detail.contains("used: forbidden"));
        assert!(
            !detail.contains(secret),
            "remote body must not leak: {detail}"
        );
    }

    #[tokio::test]
    async fn both_endpoints_401_stays_unauthorized_and_redacts_secret() {
        // 两端 401 → Unauthorized（保留 reauth 分类，不统一 Other）；正文中的
        // secret 不进错误文本。
        let secret = "sk-remote-401-secret-9c2b";
        let server = MockServer::start().await;
        for endpoint in ["/v1/organization/spend_limit", "/v1/organization/costs"] {
            Mock::given(method("GET"))
                .and(path(endpoint))
                .respond_with(
                    ResponseTemplate::new(401)
                        .set_body_string(format!(r#"{{"error":{{"message":"{secret}"}}}}"#)),
                )
                .mount(&server)
                .await;
        }
        let a = OpenAiAdapter::new(http(), server.uri());
        let err = a
            .fetch(&req(), Some(&cred()), &CancellationToken::new())
            .await
            .expect_err("both 401");
        assert!(matches!(&err, QuotaError::Unauthorized { .. }));
        let rendered = format!("{err:?}") + &err.to_string();
        assert!(rendered.contains("both endpoints failed"));
        assert!(
            !rendered.contains(secret),
            "remote body secret leaked: {rendered}"
        );
    }

    #[tokio::test]
    async fn both_endpoints_429_keeps_rate_limited_with_max_retry_after() {
        // 两端 429 → RateLimited（保留 retry_after，取两端较大值）；组合消息
        // 为纯固定标签，不含任何远端字段。
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/spend_limit"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "2"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "5"))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let err = a
            .fetch(&req(), Some(&cred()), &CancellationToken::new())
            .await
            .expect_err("both 429");
        assert!(matches!(
            &err,
            QuotaError::RateLimited {
                retry_after_ms: Some(5_000),
                ..
            }
        ));
        let detail = match &err {
            QuotaError::RateLimited { detail, .. } => detail,
            other => panic!("expected RateLimited, got {other:?}"),
        };
        assert_eq!(
            detail,
            "openai: both endpoints failed (limit: rate-limited, used: rate-limited)"
        );
    }

    #[tokio::test]
    async fn both_endpoints_503_stays_transient_retryable() {
        // 两端 503 → Transient（retryable 分类保留），不统一 Other。
        let server = MockServer::start().await;
        for endpoint in ["/v1/organization/spend_limit", "/v1/organization/costs"] {
            Mock::given(method("GET"))
                .and(path(endpoint))
                .respond_with(ResponseTemplate::new(503))
                .mount(&server)
                .await;
        }
        let a = OpenAiAdapter::new(http(), server.uri());
        let err = a
            .fetch(&req(), Some(&cred()), &CancellationToken::new())
            .await
            .expect_err("both 503");
        assert!(matches!(
            &err,
            QuotaError::Transient {
                status: Some(503),
                ..
            }
        ));
        assert!(err.retryable());
    }

    #[test]
    fn dual_failure_uses_unified_merge_with_provider_context() {
        // 优先级表与分类语义由 crate::error::merge_dual_failures 统一维护
        // （P14 review §3.4）；此处只验证 provider 上下文消息与不泄漏。
        let combined = crate::error::merge_dual_failures(
            QuotaError::forbidden("remote detail from body"),
            QuotaError::parse("remote value: EUR"),
            "openai: both endpoints failed",
        );
        let detail = match combined {
            QuotaError::Forbidden { detail } => detail,
            other => panic!("expected Forbidden, got {other:?}"),
        };
        assert_eq!(
            detail,
            "openai: both endpoints failed (limit: forbidden, used: parse)"
        );
        assert!(!detail.contains("remote"));
    }

    #[tokio::test]
    async fn costs_pagination_accumulates_across_pages() {
        // fetch_used 直接测试：第一页 has_more=true 给 cursor，第二页 has_more=false。
        let server = MockServer::start().await;
        // 第一页响应（仅在无 after 参数时命中，用 up_to_n_times 限制为 1 次）。
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"results": [{"amount": {"value": 0.50, "currency": "usd"}}]}],
                "has_more": true,
                "next_page": "cursor-2"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // 第二页及以后：累加兜底。
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"results": [{"amount": {"value": 0.25, "currency": "usd"}}]}],
                "has_more": false
            })))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let used = a
            .fetch_used(&bearer_headers(&cred()), &CancellationToken::new())
            .await
            .expect("ok");
        // 0.50 + 0.25 = 0.75 USD -> 750_000 micros
        assert_eq!(used, 750_000);
    }

    #[test]
    fn usd_value_to_micros_is_exact_and_rejects_over_precision() {
        assert_eq!(
            usd_value_to_micros(&serde_json::json!(0.06)).unwrap(),
            60_000
        );
        assert_eq!(
            usd_value_to_micros(&serde_json::json!("1.25")).unwrap(),
            1_250_000
        );
        // 超过 6 位小数 → Parse（不截断）；负数 → Parse（不钳位）。
        for bad in ["0.1234567", "-0.5"] {
            let err = usd_value_to_micros(&serde_json::json!(bad)).expect_err("bad value");
            assert!(matches!(err, QuotaError::Parse { .. }));
            // 远端值不拼入错误文本（脱敏回归）。
            let rendered = err.to_string();
            assert!(!rendered.contains(bad), "remote value leaked: {rendered}");
        }
    }

    #[test]
    fn month_start_is_first_day_of_current_month() {
        let mar15 = crate::util::civil_to_days(2026, 3, 15);
        let mar1 = crate::util::civil_to_days(2026, 3, 1);
        assert_eq!(
            crate::util::epoch_to_utc_from_days(mar15),
            (2026, 3, 15, 0, 0, 0)
        );
        assert_eq!(
            crate::util::epoch_to_utc_from_days(mar1),
            (2026, 3, 1, 0, 0, 0)
        );
        assert!(mar15 > mar1);
    }

    #[tokio::test]
    async fn costs_pagination_overflow_does_not_truncate_as_exact() {
        // 翻到分页上限仍有 has_more=true：必须报错，不得截断伪造 used 标 Exact。
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"results": [{"amount": {"value": 0.50, "currency": "usd"}}]}],
                "has_more": true,
                "next_page": "cursor-loop"
            })))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let err = a
            .fetch_used(&bearer_headers(&cred()), &CancellationToken::new())
            .await
            .expect_err("must not truncate");
        assert!(matches!(err, QuotaError::Parse { .. }));
    }

    #[tokio::test]
    async fn costs_has_more_without_cursor_is_parse_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"results": [{"amount": {"value": 0.50, "currency": "usd"}}]}],
                "has_more": true
            })))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let err = a
            .fetch_used(&bearer_headers(&cred()), &CancellationToken::new())
            .await
            .expect_err("no cursor");
        assert!(matches!(err, QuotaError::Parse { detail } if detail.contains("next_page")));
    }

    #[tokio::test]
    async fn costs_cursor_is_percent_encoded_in_url() {
        let server = MockServer::start().await;
        // 第一页给出带保留字符的 cursor（&、空格、=）。
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"results": [{"amount": {"value": 0.50, "currency": "usd"}}]}],
                "has_more": true,
                "next_page": "cur &next=x=y"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"results": [{"amount": {"value": 0.25, "currency": "usd"}}]}],
                "has_more": false
            })))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let used = a
            .fetch_used(&bearer_headers(&cred()), &CancellationToken::new())
            .await
            .expect("ok");
        assert_eq!(used, 750_000);
        // 第二页请求必须携带编码后的 cursor，而不是裸保留字符。
        let requests = server.received_requests().await.unwrap();
        assert!(requests.iter().any(|r| r
            .url
            .query()
            .is_some_and(|q| q.contains("after=cur%20%26next%3Dx%3Dy"))));
        assert!(!requests.iter().any(|r| r
            .url
            .query()
            .is_some_and(|q| q.contains("after=cur &next=x=y"))));
    }

    #[tokio::test]
    async fn costs_rejects_non_usd_amount_currency() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"results": [{"amount": {"value": 0.50, "currency": "EUR-SECRET"}}]}],
                "has_more": false
            })))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let err = a
            .fetch_used(&bearer_headers(&cred()), &CancellationToken::new())
            .await
            .expect_err("currency");
        let detail = match err {
            QuotaError::Parse { detail } => detail,
            other => panic!("expected Parse, got {other:?}"),
        };
        assert!(
            !detail.contains("EUR-SECRET"),
            "remote currency leaked: {detail}"
        );
    }

    #[tokio::test]
    async fn costs_rejects_missing_amount_currency_or_value() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"results": [{"amount": {"value": 0.50}}]}],
                "has_more": false
            })))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let err = a
            .fetch_used(&bearer_headers(&cred()), &CancellationToken::new())
            .await
            .expect_err("missing currency");
        assert!(matches!(err, QuotaError::Parse { detail } if detail.contains("currency")));
    }

    #[tokio::test]
    async fn costs_rejects_negative_and_over_precision_values() {
        for bad in ["-0.5", "0.1234567"] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1/organization/costs"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": [{"results": [{"amount": {"value": bad, "currency": "usd"}}]}],
                    "has_more": false
                })))
                .mount(&server)
                .await;
            let a = OpenAiAdapter::new(http(), server.uri());
            let err = a
                .fetch_used(&bearer_headers(&cred()), &CancellationToken::new())
                .await
                .expect_err("bad value");
            assert!(matches!(err, QuotaError::Parse { .. }));
            // 远端值不拼入错误文本（脱敏回归）。
            let rendered = err.to_string();
            assert!(!rendered.contains(bad), "remote value leaked: {rendered}");
        }
    }

    #[tokio::test]
    async fn costs_used_total_overflow_is_parse_error() {
        // 两个 u64::MAX micros 求和溢出：报错，不饱和累加。
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"results": [
                    {"amount": {"value": "18446744073709.551615", "currency": "usd"}},
                    {"amount": {"value": "18446744073709.551615", "currency": "usd"}}
                ]}],
                "has_more": false
            })))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let err = a
            .fetch_used(&bearer_headers(&cred()), &CancellationToken::new())
            .await
            .expect_err("overflow");
        assert!(matches!(err, QuotaError::Parse { detail } if detail.contains("overflow")));
    }

    #[tokio::test]
    async fn spend_limit_rejects_non_usd_currency_and_wrong_interval() {
        for body in [
            serde_json::json!({"threshold_amount": 100, "currency": "CNY", "interval": "month"}),
            serde_json::json!({"threshold_amount": 100, "currency": "USD", "interval": "weekly"}),
            serde_json::json!({"threshold_amount": 100, "interval": "month"}),
            serde_json::json!({"threshold_amount": 100, "currency": "USD"}),
            serde_json::json!({"threshold_amount": -5, "currency": "USD", "interval": "month"}),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1/organization/spend_limit"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
            let a = OpenAiAdapter::new(http(), server.uri());
            let err = a
                .fetch_limit(&bearer_headers(&cred()), &CancellationToken::new())
                .await
                .expect_err("contract violation");
            assert!(matches!(err, QuotaError::Parse { .. }));
            // 远端值（currency / interval / threshold_amount）不拼入错误文本。
            let rendered = err.to_string();
            for needle in ["CNY", "weekly", "-5"] {
                assert!(
                    !rendered.contains(needle),
                    "remote value {needle:?} leaked: {rendered}"
                );
            }
        }
    }

    #[tokio::test]
    async fn remaining_is_unknown_when_used_exceeds_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/spend_limit"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "threshold_amount": 100, "currency": "USD", "interval": "month"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/organization/costs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"results": [{"amount": {"value": 2.0, "currency": "usd"}}]}],
                "has_more": false
            })))
            .mount(&server)
            .await;
        let a = OpenAiAdapter::new(http(), server.uri());
        let snap = a
            .fetch(&req(), Some(&cred()), &CancellationToken::new())
            .await
            .expect("ok");
        // limit 100 cents = 1_000_000 micros < used 2 USD = 2_000_000 micros。
        assert_eq!(snap.values.remaining, QuotaMeasure::Unknown);
    }

    #[test]
    fn reset_is_first_day_of_next_month_utc() {
        // 固定边界：显式 now 而非墙钟（P14 review §3.3）。
        let now = agent_domain::Timestamp::from_unix_millis(
            (crate::util::civil_to_days(2026, 3, 15) * 86_400 + 12 * 3_600) as u64 * 1_000,
        );
        let reset_at = crate::util::next_month_start_timestamp(now);
        assert_eq!(
            reset_at.as_unix_millis(),
            (crate::util::civil_to_days(2026, 4, 1) * 86_400) as u64 * 1_000
        );
    }
}
