//! 可选的「WebScrape」低可信度回退适配器。
//!
//! 当 Provider 没有官方额度 API（如 Zhipu/BigModel）时，可 opt-in 启用控制台
//! 页面抓取作为最后的 fallback。它严格遵循低可信度约束：
//! - 版本化 profile/selector：profile 自带 version，写入 provenance 与审计；
//!   selector 失效时由上层升级版本，不静默 fallback。
//! - 全作用域 TTL 缓存：键为 (scope, window, unit, profile_version, 凭证指纹)，
//!   指纹是进程内随机密钥的 HMAC-SHA1 摘要（不落明文 / 可逆 secret），cookie
//!   轮换即换键、不复用旧缓存；命中前仍校验凭证，命中立即返回——不联网、不占
//!   限频时隙，沿用真实 fetched_at、不伪装新抓取，审计以 cached 标记供审计 /
//!   调度识别。
//! - 最小请求间隔：「下一保留时隙」算法，按脱敏端点限速，保证并发请求之间也有
//!   真实间隔；仅缓存未命中才占用时隙，限频键不含 query / cookie / 会话标识。
//! - 凭证前置校验：[`ScrapeProfile::auth_headers`] 返回 `Result`，缺失 / 无效凭证
//!   在任何网络调用之前以 `Unauthorized` 拒绝。
//! - 脱敏审计：成功与失败都记账，条目只含端点（去 query）、selector 版本、
//!   是否命中、字段计数与安全失败类别；绝不保留原始 HTML、cookie、URL query
//!   或错误原文；审计有界（最多 [`MAX_AUDIT_ENTRIES`] 条）。
//! - 取消：抓取与等待均与 CancellationToken 竞争；互斥锁只在无 await 的临界区
//!   持有，取消 / 丢弃 future 不会泄漏锁。
//!
//! 来源置信度恒为 Confidence::Scraped，仅作参考，不得用于硬性预算停摆。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_domain::{CancellationToken, Timestamp};
use async_trait::async_trait;
use hmac::{Hmac, KeyInit, Mac};
use provider_api::ResolvedCredential;
use provider_runtime::http::HttpClient;
use sha1::Sha1;
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    AdapterKind, Confidence, QuotaAdapter, QuotaError, QuotaMeasure, QuotaProvenance, QuotaRequest,
    QuotaReset, QuotaScope, QuotaSnapshot, QuotaUnit, QuotaValues, QuotaWindow,
};

use super::http_util::{api_get_text, now_millis, redact_endpoint, sleep_or_cancel};

/// 单条抓取审计记录。不含原始 HTML / cookie / URL query / 错误原文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrapeAuditEntry {
    pub fetched_at: Timestamp,
    pub endpoint: String,
    pub selector_version: String,
    pub matched: bool,
    pub extracted_fields: u32,
    /// 失败的安全类别；成功为 `None`。绝不携带错误消息原文。
    pub failure: Option<ScrapeFailureKind>,
    /// 是否 TTL 缓存命中（未发生网络抓取）；命中条目沿用真实 fetched_at，
    /// 供审计 / 调度识别缓存命中，不伪装新抓取。
    pub cached: bool,
}

/// 抓取失败的安全类别。仅可枚举的类别，不含任何错误消息原文。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrapeFailureKind {
    /// 凭证缺失或无效（网络前拒绝）。
    Unauthorized,
    /// 页面结构不符 / 响应解析失败。
    Parse,
    /// 远端禁止访问。
    Forbidden,
    /// 远端限流（429）。
    RateLimited,
    /// 网络 / 超时 / 5xx 等瞬时故障。
    Transient,
    /// 调用被取消。
    Cancelled,
    /// 其他未分类失败。
    Other,
}

impl ScrapeFailureKind {
    fn from_quota_error(error: &QuotaError) -> Self {
        match error {
            QuotaError::Cancelled => Self::Cancelled,
            QuotaError::Parse { .. } => Self::Parse,
            QuotaError::Unauthorized { .. } => Self::Unauthorized,
            QuotaError::Forbidden { .. } => Self::Forbidden,
            QuotaError::RateLimited { .. } => Self::RateLimited,
            QuotaError::Timeout { .. } | QuotaError::Transient { .. } => Self::Transient,
            _ => Self::Other,
        }
    }
}

/// Provider 侧抓取胶水。
pub trait ScrapeProfile: Send + Sync {
    fn version(&self) -> &str;
    fn supports(&self, request: &QuotaRequest) -> bool;
    fn url(&self, request: &QuotaRequest) -> String;
    /// 由凭证构造认证头。缺失 / 无效凭证必须返回
    /// [`QuotaError::Unauthorized`]——适配器在任何网络调用与缓存命中之前短路
    /// 拒绝，不发送请求、也不占用限速时隙。
    fn auth_headers(
        &self,
        credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<(String, String)>, QuotaError>;
    fn min_interval(&self) -> Duration;
    fn ttl(&self) -> Duration;
    fn source(&self) -> &'static str;
    fn parse(
        &self,
        request: &QuotaRequest,
        document: &scraper::Html,
        now: Timestamp,
    ) -> Result<(QuotaValues, QuotaReset), QuotaError>;
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    scope: QuotaScope,
    window: QuotaWindow,
    unit: QuotaUnit,
    version: String,
    /// 凭证身份指纹：进程内随机密钥的 HMAC-SHA1 摘要（不可逆，绝不存明文 /
    /// 可逆 secret）。`None` 表示匿名（无凭证）请求；凭证轮换即换指纹，
    /// 不复用旧缓存条目。
    credential: Option<[u8; 20]>,
}

struct CachedEntry {
    snapshot: QuotaSnapshot,
    fetched_at: Instant,
}

/// 通用 WebScrape 配额适配器。
pub struct WebScrapeQuotaAdapter {
    http: Arc<HttpClient>,
    profile: Box<dyn ScrapeProfile>,
    cache: AsyncMutex<HashMap<CacheKey, CachedEntry>>,
    /// 每个限频键（脱敏端点）的下一个可用时隙。
    next_slot: AsyncMutex<HashMap<String, Instant>>,
    audit: AsyncMutex<Vec<ScrapeAuditEntry>>,
    /// 进程内随机 HMAC-SHA1 密钥：凭证指纹仅在本进程可计算；不持久化、不进
    /// 日志 / 审计，杜绝跨进程还原。
    credential_key: [u8; 32],
}

/// 审计记录上限。成功与失败共用同一有界队列，超过后丢弃最旧条目。
const MAX_AUDIT_ENTRIES: usize = 256;

impl WebScrapeQuotaAdapter {
    pub fn new(http: Arc<HttpClient>, profile: Box<dyn ScrapeProfile>) -> Self {
        Self {
            http,
            profile,
            cache: AsyncMutex::new(HashMap::new()),
            next_slot: AsyncMutex::new(HashMap::new()),
            audit: AsyncMutex::new(Vec::new()),
            credential_key: rand::random(),
        }
    }

    /// 凭证身份的进程内指纹：HMAC-SHA1（随机密钥）摘要 kind + secret，不可逆，
    /// 不存明文。无凭证为 `None`。仅用于缓存键区分身份，绝不写日志 / 审计。
    fn credential_fingerprint(&self, credential: Option<&ResolvedCredential>) -> Option<[u8; 20]> {
        credential.map(|cred| {
            let mut mac = <Hmac<Sha1>>::new_from_slice(&self.credential_key)
                .expect("HMAC accepts any key length");
            mac.update(&[cred.kind() as u8]);
            mac.update(b":");
            mac.update(cred.expose_secret().as_bytes());
            mac.finalize()
                .into_bytes()
                .as_slice()
                .try_into()
                .expect("HMAC-SHA1 tag is 20 bytes")
        })
    }

    /// 取最近 `limit` 条审计记录的快照（仅供测试 / 诊断）。
    pub async fn audit_entries(&self, limit: usize) -> Vec<ScrapeAuditEntry> {
        let audit = self.audit.lock().await;
        let start = audit.len().saturating_sub(limit);
        audit[start..].to_vec()
    }

    /// 当前缓存条目计数（仅供测试 / 诊断）。
    pub async fn cached_entry_count(&self) -> usize {
        self.cache.lock().await.len()
    }

    /// 追加一条审计记录，丢弃最旧条目保持有界。
    async fn record_audit(&self, entry: ScrapeAuditEntry) {
        let mut audit = self.audit.lock().await;
        audit.push(entry);
        if audit.len() > MAX_AUDIT_ENTRIES {
            let drop_n = audit.len() - MAX_AUDIT_ENTRIES;
            audit.drain(0..drop_n);
        }
    }

    /// 「下一保留时隙」限速。
    ///
    /// 原子地取出并推进该限频键的下一个可用时隙，返回需等待的时长；锁只在
    /// 计算期间持有，等待发生在锁外——取消或 future 被 drop 都不会泄漏锁。
    /// 被取消的等待会保留其时隙（保守节流，不影响正确性）。
    ///
    /// 限频键是脱敏端点（去 query/fragment），cookie / 会话标识不会成为键的
    /// 一部分；原始 URL 永不作键。
    async fn reserve_and_wait(
        &self,
        url: &str,
        cancel: &CancellationToken,
    ) -> Result<(), QuotaError> {
        let key = redact_endpoint(url);
        let wait = {
            let mut next_slot = self.next_slot.lock().await;
            let now = Instant::now();
            let wait = next_slot
                .get(&key)
                .copied()
                .unwrap_or(now)
                .saturating_duration_since(now);
            next_slot.insert(key, now + wait + self.profile.min_interval());
            wait
        };
        sleep_or_cancel(wait, cancel).await
    }

    /// 网络抓取 + 解析。调用方负责时隙与审计；原始 HTML 不离开本函数。
    async fn fetch_and_parse(
        &self,
        request: &QuotaRequest,
        url: &str,
        headers: &[(String, String)],
        cancel: &CancellationToken,
    ) -> Result<(QuotaSnapshot, u32), QuotaError> {
        let text = api_get_text(self.http.as_ref(), url, headers, cancel).await?;
        // 原始 HTML 只在本作用域短暂存在；解析后立即丢弃。
        // scraper::Html 非 Send（tendril 内部用 Cell），须在跨 await 前 drop。
        let now_ts = now_millis();
        let (values, reset) = {
            let document = scraper::Html::parse_document(&text);
            drop(text);
            self.profile.parse(request, &document, now_ts)?
        };

        let extracted = extracted_field_count(&values);
        let snapshot = QuotaSnapshot {
            scope: request.scope.clone(),
            window: request.window,
            unit: request.unit.clone(),
            values,
            reset,
            confidence: Confidence::Scraped,
            provenance: QuotaProvenance {
                adapter_kind: AdapterKind::WebScrape,
                source: self.profile.source().to_string(),
                endpoint: Some(redact_endpoint(url)),
                fetched_at: now_ts,
                observed_at: Some(now_ts),
                selector_version: Some(self.profile.version().to_string()),
                stale: false,
            },
        };
        Ok((snapshot, extracted))
    }
}

#[async_trait]
impl QuotaAdapter for WebScrapeQuotaAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::WebScrape
    }

    fn supports(&self, request: &QuotaRequest) -> bool {
        self.profile.supports(request)
    }

    async fn fetch(
        &self,
        request: &QuotaRequest,
        credential: Option<&ResolvedCredential>,
        cancel: &CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError> {
        let key = CacheKey {
            scope: request.scope.clone(),
            window: request.window,
            unit: request.unit.clone(),
            version: self.profile.version().to_string(),
            credential: self.credential_fingerprint(credential),
        };
        let url = self.profile.url(request);
        let endpoint = redact_endpoint(&url);
        let selector_version = self.profile.version().to_string();

        // 凭证校验（网络与缓存命中之前）：缺失 / 无效凭证直接 Unauthorized 拒绝，
        // 不联网、不命中缓存、也不占用限速时隙；拒绝同样入审计（安全类别，无
        // 错误原文）。
        let headers = match self.profile.auth_headers(credential) {
            Ok(headers) => headers,
            Err(error) => {
                self.record_audit(ScrapeAuditEntry {
                    fetched_at: now_millis(),
                    endpoint,
                    selector_version,
                    matched: false,
                    extracted_fields: 0,
                    failure: Some(ScrapeFailureKind::from_quota_error(&error)),
                    cached: false,
                })
                .await;
                return Err(error);
            }
        };

        // TTL 缓存命中（命中前凭证已校验）：命中立即返回，不联网、也不占限频
        // 时隙；快照沿用真实 fetched_at，不伪装新抓取；命中条目以 cached=true
        // 入审计，供审计 / 调度识别。
        let ttl = self.profile.ttl();
        {
            let cache = self.cache.lock().await;
            if let Some(entry) = cache.get(&key) {
                if entry.fetched_at.elapsed() < ttl {
                    let snapshot = entry.snapshot.clone();
                    let extracted = extracted_field_count(&snapshot.values);
                    drop(cache);
                    self.record_audit(ScrapeAuditEntry {
                        fetched_at: snapshot.provenance.fetched_at,
                        endpoint,
                        selector_version,
                        matched: extracted > 0,
                        extracted_fields: extracted,
                        failure: None,
                        cached: true,
                    })
                    .await;
                    return Ok(snapshot);
                }
            }
        }

        // 仅缓存未命中：下一保留时隙，并发请求之间也有真实的最小间隔。
        if let Err(error) = self.reserve_and_wait(&url, cancel).await {
            self.record_audit(ScrapeAuditEntry {
                fetched_at: now_millis(),
                endpoint,
                selector_version,
                matched: false,
                extracted_fields: 0,
                failure: Some(ScrapeFailureKind::from_quota_error(&error)),
                cached: false,
            })
            .await;
            return Err(error);
        }

        let fetched_at = now_millis();
        let outcome = self.fetch_and_parse(request, &url, &headers, cancel).await;
        let (result, failure, extracted) = match outcome {
            Ok((snapshot, extracted)) => {
                {
                    let mut cache = self.cache.lock().await;
                    cache.insert(
                        key,
                        CachedEntry {
                            snapshot: snapshot.clone(),
                            fetched_at: Instant::now(),
                        },
                    );
                }
                (Ok(snapshot), None, extracted)
            }
            Err(error) => {
                let failure = ScrapeFailureKind::from_quota_error(&error);
                (Err(error), Some(failure), 0)
            }
        };

        // 成功与失败都写脱敏审计：仅类别 / selector 版本 / 字段数 / 脱敏端点，
        // 不含原始 HTML、cookie、URL query 或错误原文；队列有界。
        self.record_audit(ScrapeAuditEntry {
            fetched_at,
            endpoint,
            selector_version,
            matched: extracted > 0,
            extracted_fields: extracted,
            failure,
            cached: false,
        })
        .await;
        result
    }
}

fn extracted_field_count(values: &QuotaValues) -> u32 {
    let mut count = 0u32;
    if values.used.exact_value().is_some() {
        count += 1;
    }
    if values.limit.exact_value().is_some() {
        count += 1;
    }
    if values.remaining.exact_value().is_some() {
        count += 1;
    }
    count
}

#[allow(dead_code)]
fn _measure_used(_: QuotaMeasure) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AccountId;
    use agent_domain::{ProviderId, TenantId};
    use provider_api::CredentialKind;
    use provider_runtime::http::{HttpClient, HttpClientConfig};
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct ConsoleProfile {
        url: String,
        version: &'static str,
        min_interval: Duration,
        ttl: Duration,
        /// 为 true 时要求凭证（缺失 → Unauthorized），用于验证网络前拒绝。
        require_credential: bool,
        /// 记录凭证校验（auth_headers）调用时刻；并发间隔断言见 `network_times`。
        starts: std::sync::Arc<std::sync::Mutex<Vec<Instant>>>,
        /// 记录网络响应到达时刻（parse 入口），用于断言并发抓取被保留时隙拉开。
        network_times: std::sync::Arc<std::sync::Mutex<Vec<Instant>>>,
        /// 为 true 时 parse 返回 Parse 错误，用于验证解析失败审计。
        fail_parse: bool,
    }

    impl ConsoleProfile {
        fn new(url: impl Into<String>, version: &'static str) -> Self {
            Self {
                url: url.into(),
                version,
                min_interval: Duration::ZERO,
                ttl: Duration::from_secs(60),
                require_credential: false,
                starts: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                network_times: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                fail_parse: false,
            }
        }
    }

    impl ScrapeProfile for ConsoleProfile {
        fn version(&self) -> &str {
            self.version
        }
        fn supports(&self, _r: &QuotaRequest) -> bool {
            true
        }
        fn url(&self, _r: &QuotaRequest) -> String {
            self.url.clone()
        }
        fn auth_headers(
            &self,
            credential: Option<&ResolvedCredential>,
        ) -> Result<Vec<(String, String)>, QuotaError> {
            self.starts
                .lock()
                .expect("starts poisoned")
                .push(Instant::now());
            if self.require_credential && credential.is_none() {
                return Err(QuotaError::unauthorized("credential required"));
            }
            Ok(Vec::new())
        }
        fn min_interval(&self) -> Duration {
            self.min_interval
        }
        fn ttl(&self) -> Duration {
            self.ttl
        }
        fn source(&self) -> &'static str {
            "zhipu.console"
        }
        fn parse(
            &self,
            _r: &QuotaRequest,
            document: &scraper::Html,
            _now: Timestamp,
        ) -> Result<(QuotaValues, QuotaReset), QuotaError> {
            self.network_times
                .lock()
                .expect("network_times poisoned")
                .push(Instant::now());
            if self.fail_parse {
                return Err(QuotaError::parse(
                    "zhipu: console marker=PARSE-FAIL-TOP-SECRET-abcdef123456",
                ));
            }
            let selector = scraper::Selector::parse("[data-quota-remaining]")
                .map_err(|e| QuotaError::parse(format!("selector parse failed: {e}")))?;
            let remaining = document
                .select(&selector)
                .next()
                .and_then(|el| el.value().attr("data-quota-remaining"))
                .and_then(|v| v.parse::<u64>().ok())
                .map(QuotaMeasure::exact)
                .unwrap_or(QuotaMeasure::Unknown);
            Ok((
                QuotaValues::new(QuotaMeasure::Unknown, QuotaMeasure::Unknown, remaining),
                QuotaReset::Unknown,
            ))
        }
    }

    fn request() -> QuotaRequest {
        QuotaRequest {
            scope: QuotaScope::new(
                TenantId::new("t"),
                AccountId::new("a"),
                ProviderId::new("zhipu"),
                None,
            ),
            window: QuotaWindow::Overall,
            unit: QuotaUnit::Cost {
                currency: "CNY".into(),
            },
        }
    }

    fn http() -> Arc<HttpClient> {
        Arc::new(
            HttpClient::new(HttpClientConfig::builder().disable_system_proxy().build())
                .expect("client"),
        )
    }

    const QUOTA_HTML: &str =
        r#"<html><body><div data-quota-remaining="7500000">balance</div></body></html>"#;

    #[tokio::test]
    async fn scrape_returns_scraped_confidence_with_selector_version() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string(QUOTA_HTML))
            .mount(&server)
            .await;
        let adapter = WebScrapeQuotaAdapter::new(
            http(),
            Box::new(ConsoleProfile::new(
                format!("{}/console", server.uri()),
                "zhipu-console@2026-08",
            )),
        );
        let snap = adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .expect("ok");
        assert_eq!(snap.confidence, Confidence::Scraped);
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(7_500_000));
        assert_eq!(
            snap.provenance.selector_version.as_deref(),
            Some("zhipu-console@2026-08")
        );
    }

    #[tokio::test]
    async fn ttl_cache_avoids_second_network_call() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string(QUOTA_HTML))
            .mount(&server)
            .await;
        let adapter = WebScrapeQuotaAdapter::new(
            http(),
            Box::new(ConsoleProfile::new(
                format!("{}/console", server.uri()),
                "v1",
            )),
        );
        let req = request();
        adapter
            .fetch(&req, None, &CancellationToken::new())
            .await
            .unwrap();
        adapter
            .fetch(&req, None, &CancellationToken::new())
            .await
            .unwrap();
        let hits = server.received_requests().await.expect("recorded").len();
        assert_eq!(hits, 1, "second fetch served from cache");
    }

    #[tokio::test]
    async fn cache_hit_skips_min_interval_wait() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string(QUOTA_HTML))
            .mount(&server)
            .await;
        let mut profile = ConsoleProfile::new(format!("{}/console", server.uri()), "v1");
        profile.require_credential = true;
        profile.min_interval = Duration::from_millis(200);
        let adapter = WebScrapeQuotaAdapter::new(http(), Box::new(profile));
        let req = request();
        let cred = ResolvedCredential::new(CredentialKind::SessionToken, "session=FAKE");
        let cancel = CancellationToken::new();

        adapter
            .fetch(&req, Some(&cred), &cancel)
            .await
            .expect("first fetch");
        // 非零 min_interval：同凭证 TTL 命中立即返回——不联网、也不等保留时隙。
        let t0 = Instant::now();
        adapter
            .fetch(&req, Some(&cred), &cancel)
            .await
            .expect("cache hit");
        let hit_elapsed = t0.elapsed();
        assert!(
            hit_elapsed < Duration::from_millis(100),
            "cache hit must not wait for the rate-limit slot, elapsed={hit_elapsed:?}"
        );
        assert_eq!(
            server.received_requests().await.expect("recorded").len(),
            1,
            "cache hit makes no network call"
        );

        // 命中不占时隙：等首抓的保留时隙（t0+200ms）过后，用新凭证的真实抓取
        // 应立即可发；若命中曾占用时隙，它会被推迟到更晚的时隙。
        tokio::time::sleep(Duration::from_millis(250)).await;
        let rotated = ResolvedCredential::new(CredentialKind::SessionToken, "session=ROTATED");
        let t1 = Instant::now();
        adapter
            .fetch(&req, Some(&rotated), &cancel)
            .await
            .expect("real fetch after the first slot passed");
        let miss_elapsed = t1.elapsed();
        assert!(
            miss_elapsed < Duration::from_millis(100),
            "cache hit must not occupy the rate-limit slot, elapsed={miss_elapsed:?}"
        );
        assert_eq!(server.received_requests().await.expect("recorded").len(), 2);
    }

    #[tokio::test]
    async fn min_interval_throttles_repeated_fetches() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string(QUOTA_HTML))
            .mount(&server)
            .await;
        let mut profile = ConsoleProfile::new(format!("{}/console", server.uri()), "v1");
        profile.min_interval = Duration::from_millis(120);
        profile.ttl = Duration::ZERO;
        let adapter = WebScrapeQuotaAdapter::new(http(), Box::new(profile));
        let req = request();
        let t0 = Instant::now();
        adapter
            .fetch(&req, None, &CancellationToken::new())
            .await
            .unwrap();
        adapter
            .fetch(&req, None, &CancellationToken::new())
            .await
            .unwrap();
        let elapsed = t0.elapsed();
        assert!(
            elapsed >= Duration::from_millis(90),
            "min interval enforced, elapsed={elapsed:?}"
        );
        let hits = server.received_requests().await.expect("recorded").len();
        assert_eq!(hits, 2);
    }

    #[tokio::test]
    async fn concurrent_fetches_are_spaced_by_min_interval() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string(QUOTA_HTML))
            .mount(&server)
            .await;
        let mut profile = ConsoleProfile::new(format!("{}/console", server.uri()), "v1");
        profile.min_interval = Duration::from_millis(120);
        profile.ttl = Duration::ZERO;
        let starts = profile.starts.clone();
        let network_times = profile.network_times.clone();
        let adapter = WebScrapeQuotaAdapter::new(http(), Box::new(profile));
        let req = request();
        let cancel = CancellationToken::new();

        // 三个并发抓取：凭证校验先于限流（三次近乎同时），真实网络抓取必须被
        // 保留时隙拉开间隔（缓存未命中才占时隙），而不是并发同时发请求。
        let outcomes =
            futures::future::join_all((0..3).map(|_| adapter.fetch(&req, None, &cancel))).await;
        for outcome in outcomes {
            outcome.expect("all fetches succeed");
        }

        assert_eq!(
            starts.lock().expect("poisoned").len(),
            3,
            "three credential validations"
        );
        let mut times = network_times.lock().expect("poisoned").clone();
        times.sort();
        assert_eq!(times.len(), 3, "three network attempts");
        for pair in times.windows(2) {
            let gap = pair[1].duration_since(pair[0]);
            assert!(
                gap >= Duration::from_millis(90),
                "concurrent requests spaced, gap={gap:?}"
            );
        }
        assert!(
            times[2].duration_since(times[0]) >= Duration::from_millis(230),
            "three requests span at least two intervals"
        );
        let hits = server.received_requests().await.expect("recorded").len();
        assert_eq!(hits, 3);
    }

    #[tokio::test]
    async fn cache_hit_still_requires_credential() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string(QUOTA_HTML))
            .mount(&server)
            .await;
        let mut profile = ConsoleProfile::new(format!("{}/console", server.uri()), "v1");
        profile.require_credential = true;
        let adapter = WebScrapeQuotaAdapter::new(http(), Box::new(profile));
        let req = request();
        let cred = ResolvedCredential::new(CredentialKind::ApiKey, "session=FAKE");
        let cancel = CancellationToken::new();

        adapter
            .fetch(&req, Some(&cred), &cancel)
            .await
            .expect("first fetch");
        // 第二次不带凭证：缓存命中前仍校验凭证 → Unauthorized，不命中缓存、不联网。
        let err = adapter
            .fetch(&req, None, &cancel)
            .await
            .expect_err("missing credential must not hit cache");
        assert!(matches!(err, QuotaError::Unauthorized { .. }));

        let hits = server.received_requests().await.expect("recorded").len();
        assert_eq!(hits, 1, "missing credential never reaches the network");
        let entries = adapter.audit_entries(10).await;
        assert_eq!(entries.len(), 2);
        assert!(!entries[0].cached);
        assert_eq!(entries[1].failure, Some(ScrapeFailureKind::Unauthorized));
        assert!(!entries[1].cached, "rejection is not a cache hit");
    }

    #[tokio::test]
    async fn rotated_cookie_does_not_reuse_cached_snapshot() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string(QUOTA_HTML))
            .mount(&server)
            .await;
        let mut profile = ConsoleProfile::new(format!("{}/console", server.uri()), "v1");
        profile.require_credential = true;
        let adapter = WebScrapeQuotaAdapter::new(http(), Box::new(profile));
        let req = request();
        let cancel = CancellationToken::new();
        let old_cookie =
            ResolvedCredential::new(CredentialKind::SessionToken, "session=OLD-cookie");
        let new_cookie =
            ResolvedCredential::new(CredentialKind::SessionToken, "session=NEW-cookie");

        let first = adapter
            .fetch(&req, Some(&old_cookie), &cancel)
            .await
            .expect("old cookie fetch");
        // 同一凭证在 TTL 内命中缓存（不联网）。
        let hit = adapter
            .fetch(&req, Some(&old_cookie), &cancel)
            .await
            .expect("same cookie served from cache");
        assert_eq!(
            hit.provenance.fetched_at, first.provenance.fetched_at,
            "cache hit serves the snapshot with its real fetched_at"
        );
        assert_eq!(
            server.received_requests().await.expect("recorded").len(),
            1,
            "same credential hits TTL cache"
        );
        // cookie 轮换：指纹换键，不复用旧缓存 → 必须重新抓取。
        adapter
            .fetch(&req, Some(&new_cookie), &cancel)
            .await
            .expect("rotated cookie refetches");
        assert_eq!(
            server.received_requests().await.expect("recorded").len(),
            2,
            "rotated cookie must not reuse the old cached snapshot"
        );
        assert_eq!(adapter.cached_entry_count().await, 2, "two identities");

        // 审计可识别缓存命中：第二条是 cached=true 命中条目，且沿用真实
        // fetched_at（等于首抓时刻），不伪装新抓取。
        let entries = adapter.audit_entries(10).await;
        assert_eq!(entries.len(), 3);
        assert!(!entries[0].cached);
        assert!(entries[1].cached, "same credential hit is marked cached");
        assert!(!entries[2].cached, "rotation is a real fetch");
        assert_eq!(
            entries[1].fetched_at, first.provenance.fetched_at,
            "cache hit audit keeps the snapshot's real fetched_at"
        );
        assert!(entries[1].matched);
        assert_eq!(entries[1].extracted_fields, 1);
    }

    #[tokio::test]
    async fn missing_credential_is_unauthorized_before_network() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string(QUOTA_HTML))
            .mount(&server)
            .await;
        let mut profile = ConsoleProfile::new(format!("{}/console", server.uri()), "v1");
        profile.require_credential = true;
        let adapter = WebScrapeQuotaAdapter::new(http(), Box::new(profile));

        let err = adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .expect_err("unauthorized");
        assert!(matches!(err, QuotaError::Unauthorized { .. }));
        let hits = server.received_requests().await.expect("recorded");
        assert!(hits.is_empty(), "zero requests without credential");
        let entries = adapter.audit_entries(10).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].failure, Some(ScrapeFailureKind::Unauthorized));
    }

    #[tokio::test]
    async fn parse_failure_still_writes_sanitized_audit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    "<html><body>cookie=TOP-SECRET-COOKIE-abcdef123456</body></html>",
                ),
            )
            .mount(&server)
            .await;
        let mut profile = ConsoleProfile::new(
            format!("{}/console?session=QUERY-SECRET-abcdef123456", server.uri()),
            "v1",
        );
        profile.fail_parse = true;
        let adapter = WebScrapeQuotaAdapter::new(http(), Box::new(profile));

        let err = adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .expect_err("parse failure");
        assert!(matches!(err, QuotaError::Parse { .. }));

        let entries = adapter.audit_entries(10).await;
        assert_eq!(entries.len(), 1, "failure is audited too");
        let entry = &entries[0];
        assert_eq!(entry.failure, Some(ScrapeFailureKind::Parse));
        assert!(!entry.matched);
        assert_eq!(entry.extracted_fields, 0);
        assert!(!entry.endpoint.contains("session="), "query stripped");
        assert!(!entry.endpoint.contains("QUERY-SECRET"));

        // 审计 dump 不得含原始 HTML、cookie、URL query 或错误原文。
        let dump = format!("{entries:?}");
        assert!(!dump.contains("TOP-SECRET-COOKIE"));
        assert!(!dump.contains("QUERY-SECRET"));
        assert!(!dump.contains("PARSE-FAIL"));
        assert!(!dump.contains("selector mismatch"));
    }

    #[tokio::test]
    async fn audit_stays_bounded_across_many_fetches() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string(QUOTA_HTML))
            .mount(&server)
            .await;
        let mut profile = ConsoleProfile::new(format!("{}/console", server.uri()), "v1");
        profile.ttl = Duration::ZERO;
        let adapter = WebScrapeQuotaAdapter::new(http(), Box::new(profile));
        let req = request();
        let cancel = CancellationToken::new();

        for _ in 0..300 {
            adapter.fetch(&req, None, &cancel).await.expect("ok");
        }
        let entries = adapter.audit_entries(1000).await;
        assert_eq!(entries.len(), 256, "audit queue is bounded");
        let hits = server.received_requests().await.expect("recorded").len();
        assert_eq!(hits, 300, "every fetch really hit the network");
    }

    #[tokio::test]
    async fn audit_log_excludes_raw_html_and_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_body_string(QUOTA_HTML))
            .mount(&server)
            .await;
        let adapter = WebScrapeQuotaAdapter::new(
            http(),
            Box::new(ConsoleProfile::new(
                format!("{}/console?session=SECRET-cookie-value", server.uri()),
                "v1",
            )),
        );
        adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .unwrap();
        let entries = adapter.audit_entries(10).await;
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert!(entry.matched);
        assert_eq!(entry.extracted_fields, 1);
        assert_eq!(entry.failure, None, "success entry has no failure kind");
        assert!(
            !entry.endpoint.contains("session="),
            "query stripped: {}",
            entry.endpoint
        );
        assert!(!entry.endpoint.contains("SECRET"));
        assert_eq!(entry.endpoint, format!("{}/console", server.uri()));
        let dump = format!("{entry:?}");
        assert!(!dump.contains("data-quota-remaining"));
    }

    #[tokio::test]
    async fn cancellation_aborts_text_fetch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;
        let adapter = WebScrapeQuotaAdapter::new(
            http(),
            Box::new(ConsoleProfile::new(
                format!("{}/console", server.uri()),
                "v1",
            )),
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = adapter
            .fetch(&request(), None, &cancel)
            .await
            .expect_err("cancel");
        assert!(matches!(err, QuotaError::Cancelled));
    }
}
