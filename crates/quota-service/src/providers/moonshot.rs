//! Moonshot / Kimi 配额适配器。
//!
//! 事实源（brief）：账户整体余额 `GET https://api.moonshot.cn/v1/users/me/balance`，
//! bearer key。响应 `data.available_balance`、可选 `voucher_balance`/`cash_balance`，
//! `currency` 必须为 CNY（缺失/不匹配一律 `Parse`）。十进制货币精确换算为
//! micros，全程不经过 f64；负数、溢出、超精度一律 `Parse`（不钳位、不截断）。
//!
//! 这是「账户整体余额」而非月度配额：limit/remaining = 余额，used = 0
//! （Moonshot 不报告消耗；如需消耗走 usage-ledger 派生）。

use std::sync::Arc;

use provider_api::ResolvedCredential;
use provider_runtime::http::HttpClient;

use crate::adapters::api_key::{ApiKeyQuotaAdapter, ApiKeyQuotaEndpoint};
use crate::adapters::http_util::bearer_headers;
use crate::adapters::money::{decimal_string_to_micros, json_decimal_string};
use crate::{
    QuotaAdapter, QuotaError, QuotaMeasure, QuotaRequest, QuotaReset, QuotaUnit, QuotaValues,
    QuotaWindow,
};

const BASE: &str = "https://api.moonshot.cn";

/// Moonshot bearer key 额度适配器（overall CNY）。
pub fn adapter(http: Arc<HttpClient>) -> Box<dyn QuotaAdapter> {
    Box::new(ApiKeyQuotaAdapter::new(http, Box::new(MoonshotEndpoint)))
}

struct MoonshotEndpoint;

impl ApiKeyQuotaEndpoint for MoonshotEndpoint {
    fn supports(&self, request: &QuotaRequest) -> bool {
        matches!(
            (request.window, &request.unit),
            (QuotaWindow::Overall, QuotaUnit::Cost { currency })
                if currency.eq_ignore_ascii_case("CNY")
        )
    }

    fn endpoint(&self, _request: &QuotaRequest) -> String {
        format!("{BASE}/v1/users/me/balance")
    }

    fn auth_headers(
        &self,
        credential: &ResolvedCredential,
    ) -> Result<Vec<(String, String)>, QuotaError> {
        Ok(bearer_headers(credential))
    }

    fn source(&self) -> &'static str {
        "moonshot.balance"
    }

    fn parse(
        &self,
        _request: &QuotaRequest,
        body: serde_json::Value,
    ) -> Result<(QuotaValues, QuotaReset), QuotaError> {
        // 形如 {"code":0,"data":{"available_balance":"123.45","voucher_balance":"23.45",
        //          "cash_balance":"100.00","currency":"CNY"}}
        let data = body
            .get("data")
            .ok_or_else(|| QuotaError::parse("moonshot: missing data"))?;
        let currency = data
            .get("currency")
            .and_then(|v| v.as_str())
            .ok_or_else(|| QuotaError::parse("moonshot: missing currency"))?;
        if !currency.eq_ignore_ascii_case("CNY") {
            // 远端 currency 原始串不得进入 detail（可能含 token / 超长内容）。
            return Err(QuotaError::parse("moonshot: currency must be CNY"));
        }
        let balance_str = data
            .get("available_balance")
            .map(|v| json_decimal_string(v, "moonshot available_balance"))
            .transpose()?
            .ok_or_else(|| QuotaError::parse("moonshot: missing available_balance"))?;
        // 远端金额原始串不得进入 detail（可能含 token / 超长数字）；负数、溢出、
        // 超精度仍一律 Parse，不钳位、不截断。
        let micros = decimal_string_to_micros(&balance_str)
            .map_err(|_| QuotaError::parse("moonshot: invalid available_balance"))?;
        // 账户余额：limit/remaining = 余额，used = 0。
        Ok((
            QuotaValues::new(
                QuotaMeasure::exact(0),
                QuotaMeasure::exact(micros),
                QuotaMeasure::exact(micros),
            ),
            QuotaReset::Unknown,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::http_util::redact_endpoint;
    use provider_api::CredentialKind;
    use provider_runtime::http::{HttpClient, HttpClientConfig};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn http() -> Arc<HttpClient> {
        Arc::new(
            HttpClient::new(HttpClientConfig::builder().disable_system_proxy().build())
                .expect("client"),
        )
    }

    fn sample_request() -> QuotaRequest {
        QuotaRequest {
            scope: crate::QuotaScope::new(
                agent_domain::TenantId::new("t"),
                crate::AccountId::new("a"),
                agent_domain::ProviderId::new("moonshot"),
                None,
            ),
            window: QuotaWindow::Overall,
            unit: QuotaUnit::Cost {
                currency: "CNY".into(),
            },
        }
    }

    fn cred() -> ResolvedCredential {
        ResolvedCredential::new(CredentialKind::ApiKey, "sk-moonshot-FAKE")
    }

    /// 从仓库 fixtures/quota/ 加载 contract fixture（只读，不参与生产代码）。
    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../../fixtures/quota/moonshot_balance.json"
        ))
        .expect("fixture must be valid JSON")
    }

    /// 测试专用端点：仅把 BASE 指向 wiremock，其余全部委托生产 MoonshotEndpoint
    /// （生产端点不注入 base URL，测试不得改生产逻辑）。
    struct TestEndpoint {
        base: String,
    }

    impl ApiKeyQuotaEndpoint for TestEndpoint {
        fn supports(&self, request: &QuotaRequest) -> bool {
            MoonshotEndpoint.supports(request)
        }

        fn endpoint(&self, _request: &QuotaRequest) -> String {
            format!("{}/v1/users/me/balance", self.base)
        }

        fn auth_headers(
            &self,
            credential: &ResolvedCredential,
        ) -> Result<Vec<(String, String)>, QuotaError> {
            MoonshotEndpoint.auth_headers(credential)
        }

        fn source(&self) -> &'static str {
            MoonshotEndpoint.source()
        }

        fn parse(
            &self,
            request: &QuotaRequest,
            body: serde_json::Value,
        ) -> Result<(QuotaValues, QuotaReset), QuotaError> {
            MoonshotEndpoint.parse(request, body)
        }
    }

    #[tokio::test]
    async fn fetches_overall_cny_balance_from_contract_fixture() {
        // Contract fixture（fixtures/quota/moonshot_balance.json）作为 wiremock 响应：
        // data.available_balance="123.45" CNY -> 123_450_000 micros；账户整体余额：
        // used=0、limit=remaining=余额，confidence 为 Exact。
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/users/me/balance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture()))
            .mount(&server)
            .await;
        let a = ApiKeyQuotaAdapter::new(http(), Box::new(TestEndpoint { base: server.uri() }));
        let snap = a
            .fetch(
                &sample_request(),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("ok");
        assert_eq!(
            snap.unit,
            QuotaUnit::Cost {
                currency: "CNY".into()
            }
        );
        assert_eq!(snap.values.used, QuotaMeasure::exact(0));
        assert_eq!(snap.values.limit, QuotaMeasure::exact(123_450_000));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(123_450_000));
        assert_eq!(snap.confidence, crate::Confidence::Exact);
        assert_eq!(snap.reset, QuotaReset::Unknown);
        assert_eq!(snap.provenance.source, "moonshot.balance");
    }

    #[test]
    fn supports_only_overall_cny() {
        let e = MoonshotEndpoint;
        assert!(e.supports(&sample_request()));
        assert!(!e.supports(&QuotaRequest {
            window: QuotaWindow::Monthly,
            ..sample_request()
        }));
    }

    #[test]
    fn endpoint_has_no_secret() {
        let url = MoonshotEndpoint.endpoint(&sample_request());
        assert_eq!(
            redact_endpoint(&url),
            "https://api.moonshot.cn/v1/users/me/balance"
        );
    }

    #[test]
    fn parse_rejects_non_cny_currency() {
        let body = serde_json::json!({
            "code": 0,
            "data": {"available_balance": "123.45", "currency": "USD"}
        });
        assert!(matches!(
            MoonshotEndpoint.parse(&sample_request(), body),
            Err(QuotaError::Parse { .. })
        ));
    }

    #[test]
    fn parse_error_detail_never_echoes_remote_currency() {
        // 恶意 currency：含 token 形状的内容不得泄漏进 detail。
        let token = "sk-mock-token-0123456789abcdef";
        for currency in [format!("CNY {token}"), token.to_string(), "9".repeat(4096)] {
            let body = serde_json::json!({
                "code": 0,
                "data": {"available_balance": "123.45", "currency": currency}
            });
            let err = MoonshotEndpoint
                .parse(&sample_request(), body)
                .expect_err("non-CNY currency must be rejected");
            assert_eq!(
                err,
                QuotaError::parse("moonshot: currency must be CNY"),
                "detail must be a fixed safe description"
            );
            let detail = match err {
                QuotaError::Parse { detail } => detail,
                _ => panic!("expected Parse, got {err:?}"),
            };
            assert!(!detail.contains(token));
            assert!(detail.len() < 256);
        }
    }

    #[test]
    fn parse_error_detail_never_echoes_malicious_balance() {
        // 恶意金额：token 形状与超长数字都不得泄漏进 detail，且仍为 Parse。
        let token = "sk-mock-token-0123456789abcdef";
        let oversized = "9".repeat(4096);
        for bad_balance in [
            token.to_string(),
            format!("123.45 {token}"),
            oversized.clone(),
            "1.1234567".to_string(),
        ] {
            let body = serde_json::json!({
                "code": 0,
                "data": {"available_balance": bad_balance, "currency": "CNY"}
            });
            let err = MoonshotEndpoint
                .parse(&sample_request(), body)
                .expect_err("malformed balance must be rejected");
            assert_eq!(
                err,
                QuotaError::parse("moonshot: invalid available_balance"),
                "detail must be a fixed safe description"
            );
            let detail = match err {
                QuotaError::Parse { detail } => detail,
                _ => panic!("expected Parse, got {err:?}"),
            };
            assert!(!detail.contains(token));
            assert!(!detail.contains(&oversized));
            assert!(detail.len() < 256);
        }
    }

    #[test]
    fn parse_rejects_missing_currency_and_balance() {
        let no_currency = serde_json::json!({
            "code": 0,
            "data": {"available_balance": "123.45"}
        });
        assert!(matches!(
            MoonshotEndpoint.parse(&sample_request(), no_currency),
            Err(QuotaError::Parse { .. })
        ));
        let no_balance = serde_json::json!({
            "code": 0,
            "data": {"currency": "CNY"}
        });
        assert!(matches!(
            MoonshotEndpoint.parse(&sample_request(), no_balance),
            Err(QuotaError::Parse { .. })
        ));
    }

    #[test]
    fn parse_rejects_negative_and_over_precision_without_clamping() {
        for bad in ["-5", "1.1234567"] {
            let body = serde_json::json!({
                "code": 0,
                "data": {"available_balance": bad, "currency": "CNY"}
            });
            assert!(matches!(
                MoonshotEndpoint.parse(&sample_request(), body),
                Err(QuotaError::Parse { .. })
            ));
        }
    }

    #[test]
    fn parse_accepts_numbers_without_f64() {
        let int_body = serde_json::json!({
            "code": 0,
            "data": {"available_balance": 123, "currency": "CNY"}
        });
        let (values, _) = MoonshotEndpoint.parse(&sample_request(), int_body).unwrap();
        assert_eq!(values.limit, QuotaMeasure::exact(123_000_000));

        let float_body = serde_json::json!({
            "code": 0,
            "data": {"available_balance": 123.45, "currency": "CNY"}
        });
        let (values, _) = MoonshotEndpoint
            .parse(&sample_request(), float_body)
            .unwrap();
        assert_eq!(values.limit, QuotaMeasure::exact(123_450_000));
    }
}
