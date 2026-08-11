//! Zhipu / BigModel 配额适配器（仅可选 WebScrape 回退）。
//!
//! 事实源（brief）：Zhipu/BigModel 无官方公开 usage/quota 端点。Exact API 标记为
//! Unsupported；唯一的可选读数是 **Coding Plan** 控制台的 Rolling5h / Weekly 用量计数
//! （非付费额度），通过 WebScrape 拿到，置信度 Scraped，且**必须有控制台 Cookie 登录会话
//! 凭据**（SessionToken；仅进程内消费，不持久化、不进日志）。
//! 付费额度（Cost）无任何读数来源 → Unsupported。
//!
//! profile 是版本化的：真实控制台 DOM 会随版本变化，故通过 selector_version 显式声明
//! （如 `zhipu-coding-plan@2026-08`），失效时由上层升级版本而非静默 fallback。

use std::sync::Arc;
use std::time::Duration;

use agent_domain::Timestamp;
use provider_api::{CredentialKind, ResolvedCredential};
use provider_runtime::http::HttpClient;

use crate::adapters::web_scrape::{ScrapeProfile, WebScrapeQuotaAdapter};
use crate::{
    QuotaAdapter, QuotaError, QuotaMeasure, QuotaRequest, QuotaReset, QuotaUnit, QuotaValues,
    QuotaWindow,
};

/// 默认 selector 版本。
pub const SELECTOR_VERSION: &str = "zhipu-coding-plan@2026-08";

/// Zhipu Coding Plan 控制台抓取配置。
#[derive(Clone, Debug)]
pub struct ZhipuScrapeConfig {
    /// Coding Plan 用量页 URL。
    pub url: String,
    /// 最小抓取间隔（默认 5s）。
    pub min_interval: Duration,
    /// 缓存 TTL（默认 60s）。
    pub ttl: Duration,
}

impl Default for ZhipuScrapeConfig {
    fn default() -> Self {
        Self {
            url: "https://open.bigmodel.cn/console/coding-plan/usage".to_string(),
            min_interval: Duration::from_secs(5),
            ttl: Duration::from_secs(60),
        }
    }
}

/// 构造 Zhipu Coding Plan WebScrape 适配器（Scraped，Count，Rolling5h / Weekly）。
pub fn adapter(http: Arc<HttpClient>, config: ZhipuScrapeConfig) -> Box<dyn QuotaAdapter> {
    Box::new(WebScrapeQuotaAdapter::new(
        http,
        Box::new(ZhipuCodingPlanProfile { config }),
    ))
}

struct ZhipuCodingPlanProfile {
    config: ZhipuScrapeConfig,
}

impl ScrapeProfile for ZhipuCodingPlanProfile {
    fn version(&self) -> &str {
        SELECTOR_VERSION
    }
    fn supports(&self, request: &QuotaRequest) -> bool {
        matches!(
            (request.window, &request.unit),
            (
                QuotaWindow::Rolling5h | QuotaWindow::Weekly,
                QuotaUnit::Count
            )
        )
    }
    fn url(&self, _request: &QuotaRequest) -> String {
        self.config.url.clone()
    }
    fn auth_headers(
        &self,
        credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<(String, String)>, QuotaError> {
        // Coding Plan 用量页需要登录态。缺失 / 错误凭证必须在任何网络调用
        // 之前以 Unauthorized 拒绝。Cookie 值由凭证提供，不持久化、不进审计。
        let credential = credential.ok_or_else(|| {
            QuotaError::unauthorized("zhipu: coding-plan scrape requires a session credential")
        })?;
        if credential.kind() != CredentialKind::SessionToken {
            return Err(QuotaError::unauthorized(
                "zhipu: coding-plan scrape requires a SessionToken session credential",
            ));
        }
        let secret = credential.expose_secret();
        if secret.trim().is_empty() {
            return Err(QuotaError::unauthorized(
                "zhipu: coding-plan scrape credential is empty",
            ));
        }
        Ok(vec![("Cookie".to_string(), secret.to_string())])
    }
    fn min_interval(&self) -> Duration {
        self.config.min_interval
    }
    fn ttl(&self) -> Duration {
        self.config.ttl
    }
    fn source(&self) -> &'static str {
        "zhipu.coding-plan"
    }
    fn parse(
        &self,
        request: &QuotaRequest,
        document: &scraper::Html,
        _now: Timestamp,
    ) -> Result<(QuotaValues, QuotaReset), QuotaError> {
        // Coding Plan DOM：
        //   <div data-coding-plan>
        //     <div data-window="rolling5h" data-used="1200" data-limit="5000"></div>
        //     <div data-window="weekly"   data-used="8000" data-limit="50000"></div>
        //   </div>
        // 按 request.window 选择对应 data-window 块。版本号 SELECTOR_VERSION 标识规则集。
        let want = match request.window {
            QuotaWindow::Rolling5h => "rolling5h",
            QuotaWindow::Weekly => "weekly",
            _ => {
                return Err(QuotaError::unsupported(
                    "zhipu: only Rolling5h/Weekly Coding Plan counts are scrapable",
                ))
            }
        };
        let selector = scraper::Selector::parse("[data-window]")
            .map_err(|_| QuotaError::parse("zhipu: internal selector parse failed"))?;
        let entry = document
            .select(&selector)
            .find(|el| el.value().attr("data-window") == Some(want))
            .ok_or_else(|| {
                QuotaError::parse(format!(
                    "zhipu: no [data-window={want}] block in coding-plan page"
                ))
            })?;
        let used = parse_count_attr(entry.value().attr("data-used"), "data-used")?;
        let limit = parse_count_attr(entry.value().attr("data-limit"), "data-limit")?;
        // used<=limit 时可精确给出 remaining；used>limit（超额）时 remaining
        // 必须 Unknown，绝不 saturating_sub 伪造为 0。
        let remaining = match used.exact_value().zip(limit.exact_value()) {
            Some((used, limit)) if used <= limit => QuotaMeasure::exact(limit - used),
            _ => QuotaMeasure::unknown(),
        };
        Ok((
            QuotaValues::new(used, limit, remaining),
            QuotaReset::Unknown,
        ))
    }
}

/// 解析 data-used / data-limit（非负整数 token 计数）。
fn parse_count_attr(value: Option<&str>, name: &str) -> Result<QuotaMeasure, QuotaError> {
    let raw = value
        .ok_or_else(|| QuotaError::parse(format!("zhipu: missing {name} attribute")))?
        .trim();
    if raw.is_empty() {
        return Err(QuotaError::parse(format!("zhipu: empty {name} attribute")));
    }
    // 远端属性值（含 token/金额/注入载荷）不得回显进错误字符串；
    // 只输出稳定分类与本地常量字段名。
    let n: u64 = raw
        .parse()
        .map_err(|_| QuotaError::parse(format!("zhipu: {name} attribute is not a valid count")))?;
    Ok(QuotaMeasure::exact(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合成契约 fixture（见 fixtures/quota/zhipu_console_scrape.html）：
    /// rolling5h 1200/5000、weekly 8000/50000，selector 版本 zhipu-coding-plan@2026-08。
    const CONSOLE_FIXTURE: &str =
        include_str!("../../../../fixtures/quota/zhipu_console_scrape.html");

    fn req(window: QuotaWindow) -> QuotaRequest {
        QuotaRequest {
            scope: crate::QuotaScope::new(
                agent_domain::TenantId::new("t"),
                crate::AccountId::new("a"),
                agent_domain::ProviderId::new("zhipu"),
                None,
            ),
            window,
            unit: QuotaUnit::Count,
        }
    }

    fn fixture_doc() -> scraper::Html {
        scraper::Html::parse_document(CONSOLE_FIXTURE)
    }

    #[test]
    fn parse_rolling5h_counts() {
        let profile = ZhipuCodingPlanProfile {
            config: ZhipuScrapeConfig::default(),
        };
        let (values, _) = profile
            .parse(
                &req(QuotaWindow::Rolling5h),
                &fixture_doc(),
                Timestamp::from_unix_millis(0),
            )
            .unwrap();
        assert_eq!(values.used, QuotaMeasure::exact(1200));
        assert_eq!(values.limit, QuotaMeasure::exact(5000));
        assert_eq!(values.remaining, QuotaMeasure::exact(3800));
    }

    #[test]
    fn parse_weekly_counts() {
        let profile = ZhipuCodingPlanProfile {
            config: ZhipuScrapeConfig::default(),
        };
        let (values, _) = profile
            .parse(
                &req(QuotaWindow::Weekly),
                &fixture_doc(),
                Timestamp::from_unix_millis(0),
            )
            .unwrap();
        assert_eq!(values.used, QuotaMeasure::exact(8000));
        assert_eq!(values.limit, QuotaMeasure::exact(50000));
        assert_eq!(values.remaining, QuotaMeasure::exact(42000));
    }

    #[tokio::test]
    async fn fixture_fetch_is_scraped_confidence_with_selector_version() {
        use crate::adapters::web_scrape::WebScrapeQuotaAdapter;
        use crate::Confidence;
        use agent_domain::CancellationToken;
        use provider_runtime::http::{HttpClient, HttpClientConfig};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string(CONSOLE_FIXTURE))
            .mount(&server)
            .await;
        let http = Arc::new(
            HttpClient::new(HttpClientConfig::builder().disable_system_proxy().build())
                .expect("client"),
        );
        let adapter = WebScrapeQuotaAdapter::new(
            http,
            Box::new(ZhipuCodingPlanProfile {
                config: ZhipuScrapeConfig {
                    url: format!("{}/console", server.uri()),
                    // 连续两次抓取（不同 window 均缓存未命中）不等待最小间隔。
                    min_interval: Duration::ZERO,
                    ..Default::default()
                },
            }),
        );
        let credential = ResolvedCredential::new(CredentialKind::SessionToken, "session=FAKE");
        let cancel = CancellationToken::new();

        // canonical used/limit/remaining 与 fixture 一致，且经真实抓取路径
        // 获得 Scraped 置信度与显式 selector 版本。
        let cases = [
            (QuotaWindow::Rolling5h, 1200u64, 5000u64, 3800u64),
            (QuotaWindow::Weekly, 8000u64, 50000u64, 42000u64),
        ];
        for (window, used, limit, remaining) in cases {
            let snap = adapter
                .fetch(&req(window), Some(&credential), &cancel)
                .await
                .expect("fixture fetch");
            assert_eq!(snap.confidence, Confidence::Scraped);
            assert_eq!(snap.values.used, QuotaMeasure::exact(used));
            assert_eq!(snap.values.limit, QuotaMeasure::exact(limit));
            assert_eq!(snap.values.remaining, QuotaMeasure::exact(remaining));
            assert_eq!(
                snap.provenance.selector_version.as_deref(),
                Some(SELECTOR_VERSION)
            );
            assert_eq!(snap.provenance.source, "zhipu.coding-plan");
        }
    }

    #[test]
    fn parse_remaining_exact_when_within_limit_and_unknown_when_over() {
        let profile = ZhipuCodingPlanProfile {
            config: ZhipuScrapeConfig::default(),
        };
        let doc = scraper::Html::parse_document(
            r#"<html><body><div data-coding-plan>
                 <div data-window="rolling5h" data-used="3800" data-limit="5000"></div>
                 <div data-window="weekly" data-used="55000" data-limit="50000"></div>
               </div></body></html>"#,
        );
        // 正常值：remaining = Exact(limit - used)。
        let (within, _) = profile
            .parse(
                &req(QuotaWindow::Rolling5h),
                &doc,
                Timestamp::from_unix_millis(0),
            )
            .unwrap();
        assert_eq!(within.remaining, QuotaMeasure::exact(1200));
        // 超额：used > limit 时 remaining 必须 Unknown，不得伪造为 0。
        let (over, _) = profile
            .parse(
                &req(QuotaWindow::Weekly),
                &doc,
                Timestamp::from_unix_millis(0),
            )
            .unwrap();
        assert_eq!(over.remaining, QuotaMeasure::unknown());
    }

    #[test]
    fn monthly_cost_is_unsupported_via_supports() {
        // ScrapeProfile.supports 仅接受 Rolling5h/Weekly Count；Cost 视图不被声明。
        let profile = ZhipuCodingPlanProfile {
            config: ZhipuScrapeConfig::default(),
        };
        let cost_req = QuotaRequest {
            unit: QuotaUnit::Cost {
                currency: "CNY".into(),
            },
            ..req(QuotaWindow::Monthly)
        };
        assert!(!profile.supports(&cost_req));
        assert!(profile.supports(&req(QuotaWindow::Rolling5h)));
        assert!(profile.supports(&req(QuotaWindow::Weekly)));
    }

    #[test]
    fn missing_window_block_is_parse_error() {
        let profile = ZhipuCodingPlanProfile {
            config: ZhipuScrapeConfig::default(),
        };
        let doc = scraper::Html::parse_document(
            "<html><body><div data-window='daily'></div></body></html>",
        );
        let err = profile
            .parse(
                &req(QuotaWindow::Rolling5h),
                &doc,
                Timestamp::from_unix_millis(0),
            )
            .expect_err("no block");
        assert!(matches!(err, QuotaError::Parse { .. }));
    }

    #[test]
    fn malicious_attribute_values_never_leak_into_errors() {
        let profile = ZhipuCodingPlanProfile {
            config: ZhipuScrapeConfig::default(),
        };
        // 注入/敏感样式的远端属性值（脚本、伪 token、金额串、SQL 载荷）
        // 不得出现在任何错误字符串中；错误只含稳定分类与本地字段名。
        let payloads = [
            "alert('XSS')",
            "<script>window.token='S3CR3T-TOKEN-abcdef123456'</script>",
            "1234567890-SESSION-LEAK",
            "' OR 1=1 --",
        ];
        for payload in payloads {
            // data-used / data-limit：非法值必须走 Parse 且不回显原始值，
            // 错误只保留稳定分类与本地常量字段名。
            for (attr, other) in [("data-used", "data-limit"), ("data-limit", "data-used")] {
                let doc = scraper::Html::parse_document(&format!(
                    r#"<html><body><div data-coding-plan>
                         <div data-window="rolling5h" {attr}="{payload}" {other}="1"></div>
                       </div></body></html>"#
                ));
                let err = profile
                    .parse(
                        &req(QuotaWindow::Rolling5h),
                        &doc,
                        Timestamp::from_unix_millis(0),
                    )
                    .expect_err("malicious count value must fail parse");
                let text = err.to_string();
                assert!(matches!(err, QuotaError::Parse { .. }));
                assert!(
                    !text.contains(payload),
                    "raw attribute value leaked into error: {text}"
                );
                assert!(
                    text.contains(attr),
                    "stable local field name should be kept: {text}"
                );
            }
            // data-window：不匹配时错误只提及本地 want，不得回显远端窗口值。
            let doc = scraper::Html::parse_document(&format!(
                r#"<html><body><div data-coding-plan>
                     <div data-window="{payload}"></div>
                   </div></body></html>"#
            ));
            let err = profile
                .parse(
                    &req(QuotaWindow::Rolling5h),
                    &doc,
                    Timestamp::from_unix_millis(0),
                )
                .expect_err("malicious window value must not match");
            let text = err.to_string();
            assert!(matches!(err, QuotaError::Parse { .. }));
            assert!(
                !text.contains(payload),
                "remote window value leaked into error: {text}"
            );
        }
    }

    #[test]
    fn version_is_explicit() {
        let profile = ZhipuCodingPlanProfile {
            config: ZhipuScrapeConfig::default(),
        };
        assert_eq!(profile.version(), SELECTOR_VERSION);
        assert!(SELECTOR_VERSION.starts_with("zhipu-coding-plan@"));
    }

    #[test]
    fn requires_session_token_credential_for_cookie() {
        let profile = ZhipuCodingPlanProfile {
            config: ZhipuScrapeConfig::default(),
        };
        // 无凭据 / 非 SessionToken / 空 secret → 网络前 Unauthorized。
        assert!(matches!(
            profile.auth_headers(None),
            Err(QuotaError::Unauthorized { .. })
        ));
        let bearer = ResolvedCredential::new(CredentialKind::OAuthBearer, "session=FAKE");
        assert!(matches!(
            profile.auth_headers(Some(&bearer)),
            Err(QuotaError::Unauthorized { .. })
        ));
        let api_key = ResolvedCredential::new(CredentialKind::ApiKey, "session=FAKE");
        assert!(matches!(
            profile.auth_headers(Some(&api_key)),
            Err(QuotaError::Unauthorized { .. })
        ));
        let empty = ResolvedCredential::new(CredentialKind::SessionToken, "");
        assert!(matches!(
            profile.auth_headers(Some(&empty)),
            Err(QuotaError::Unauthorized { .. })
        ));
        // SessionToken 凭据 → Cookie 头。
        let cred = ResolvedCredential::new(CredentialKind::SessionToken, "session=FAKE");
        let headers = profile.auth_headers(Some(&cred)).expect("ok");
        assert_eq!(
            headers,
            vec![("Cookie".to_string(), "session=FAKE".to_string())]
        );
    }

    #[tokio::test]
    async fn missing_credential_is_unauthorized_with_zero_requests() {
        use crate::adapters::web_scrape::{ScrapeFailureKind, WebScrapeQuotaAdapter};
        use agent_domain::CancellationToken;
        use provider_runtime::http::{HttpClient, HttpClientConfig};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
            .mount(&server)
            .await;
        let http = Arc::new(
            HttpClient::new(HttpClientConfig::builder().disable_system_proxy().build())
                .expect("client"),
        );
        let adapter = WebScrapeQuotaAdapter::new(
            http,
            Box::new(ZhipuCodingPlanProfile {
                config: ZhipuScrapeConfig {
                    url: format!("{}/console?token=QUERY-SECRET-abcdef123456", server.uri()),
                    ..Default::default()
                },
            }),
        );

        let err = adapter
            .fetch(
                &req(QuotaWindow::Rolling5h),
                None,
                &CancellationToken::new(),
            )
            .await
            .expect_err("unauthorized");
        assert!(matches!(err, QuotaError::Unauthorized { .. }));

        // 零网络请求，且 secret 不出审计。
        let hits = server.received_requests().await.expect("recorded");
        assert!(hits.is_empty(), "no network before credential check");
        let entries = adapter.audit_entries(10).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].failure, Some(ScrapeFailureKind::Unauthorized));
        let dump = format!("{entries:?}");
        assert!(!dump.contains("QUERY-SECRET"));
        assert!(!dump.contains("session=FAKE"));
    }
}
