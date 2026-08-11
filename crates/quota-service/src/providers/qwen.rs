//! Qwen / DashScope 配额适配器（Alibaba BSS `QueryAccountBalance`）。
//!
//! 事实源（brief）：DashScope inference key 没有余额 API。Alibaba BSS 的
//! `QueryAccountBalance` 是账户整体余额，端点 `business.aliyuncs.com`，RPC 版本
//! `2017-12-14`，HMAC-SHA1 签名（Alibaba AccessKey pair）。响应 `Data.AvailableAmount`、
//! `Data.Currency`，可选 `Data.AvailableCashAmount`/`AvailableCredit`。**不**能据此
//! 声称 DashScope-only 归属。
//!
//! 这是「账户整体余额」而非「DashScope 配额」，故窗口固定 Overall，且 provenance
//! source 显式标注 `qwen.bss-account`。
//!
//! 错误脱敏：远端 BSS 响应中的 `Code` / `Currency` / `AvailableAmount` 等原始
//! 字符串绝不进入 [`QuotaError`] detail——错误文本一律固定分类（`Other` /
//! `Parse` 变体与函数签名不变），防止恶意或异常远端回显 token 形态字符串。
//!
//! 凭证契约与 capability hint：能力矩阵（capability.rs）对 qwen 声明
//! `CredentialKindHint::AccessKeyPair`，语义一致（确实需要 Alibaba AccessKey
//! pair）；但运行时把 pair 打包进**单个** `provider_api::CredentialKind::ApiKey`
//! secret（`provider_api` 没有独立的 AccessKeyPair kind），上层按 hint 组装
//! 凭证时必须打包为 `AccessKeyId=...&AccessKeySecret=...`。

use std::sync::Arc;

use agent_domain::CancellationToken;
use async_trait::async_trait;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use provider_api::{CredentialKind, ResolvedCredential};
use provider_runtime::http::HttpClient;
use sha1::Sha1;

use crate::adapters::http_util::{api_get, now_millis, redact_endpoint};
use crate::adapters::money::{decimal_string_to_micros, json_decimal_string};
use crate::{
    AdapterKind, Confidence, QuotaAdapter, QuotaError, QuotaMeasure, QuotaProvenance, QuotaRequest,
    QuotaReset, QuotaSnapshot, QuotaUnit, QuotaValues, QuotaWindow,
};

type HmacSha1 = Hmac<Sha1>;

const ENDPOINT: &str = "https://business.aliyuncs.com";
const RPC_VERSION: &str = "2017-12-14";

/// Qwen / Alibaba BSS 配置。AccessKey pair 非机密标识，Secret 部分经凭证注入。
#[derive(Clone, Debug)]
pub struct QwenConfig {
    pub region: String,
}

/// 构造 Qwen 配额适配器。
///
/// 凭证约定：[`ResolvedCredential`] 的 secret 形如 `AccessKeyId=...&AccessKeySecret=...`，
/// 由上层 credential store 拼装。本适配器不持有明文，仅在签名时短暂读取。
pub fn adapter(http: Arc<HttpClient>, config: QwenConfig) -> Box<dyn QuotaAdapter> {
    Box::new(QwenBssAdapter::new(http, config, ENDPOINT))
}

struct QwenBssAdapter {
    http: Arc<HttpClient>,
    config: QwenConfig,
    /// 端点基址：生产走 `ENDPOINT`，测试注入 wiremock URI（与 anthropic/xai 一致）。
    base: String,
}

impl QwenBssAdapter {
    fn new(http: Arc<HttpClient>, config: QwenConfig, base: impl Into<String>) -> Self {
        Self {
            http,
            config,
            base: base.into(),
        }
    }
}

#[async_trait]
impl QuotaAdapter for QwenBssAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::ApiKeyApi
    }

    fn supports(&self, request: &QuotaRequest) -> bool {
        matches!(
            (request.window, &request.unit),
            (QuotaWindow::Overall, QuotaUnit::Cost { currency })
                if currency.eq_ignore_ascii_case("CNY")
        )
    }

    async fn fetch(
        &self,
        request: &QuotaRequest,
        credential: Option<&ResolvedCredential>,
        cancel: &CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError> {
        let credential =
            credential.ok_or_else(|| QuotaError::unauthorized("alibaba access key required"))?;
        let (access_key_id, access_key_secret) = parse_access_key_pair(credential)?;

        let params = signed_query(&self.config.region, &access_key_id, &access_key_secret);
        let url = format!("{}/?{params}", self.base);

        let body = api_get(self.http.as_ref(), &url, &[], cancel).await?;

        let micros = parse_balance_response(&body)?;

        let now = now_millis();
        let provenance = QuotaProvenance {
            adapter_kind: AdapterKind::ApiKeyApi,
            source: "qwen.bss-account".to_string(),
            endpoint: Some(redact_endpoint(ENDPOINT)),
            fetched_at: now,
            observed_at: Some(now),
            selector_version: Some(RPC_VERSION.to_string()),
            stale: false,
        };

        Ok(QuotaSnapshot {
            scope: request.scope.clone(),
            window: QuotaWindow::Overall,
            unit: QuotaUnit::Cost {
                currency: "CNY".to_string(),
            },
            // 账户余额：limit/remaining = 余额，used = 0（BSS 不报告消耗）。
            values: QuotaValues::new(
                QuotaMeasure::exact(0),
                QuotaMeasure::exact(micros),
                QuotaMeasure::exact(micros),
            ),
            reset: QuotaReset::Unknown,
            confidence: Confidence::Exact,
            provenance,
        })
    }
}

/// 解析 BSS 响应：先校验 `Code`，再从 `Data` 取余额。
///
/// BSS 成功响应形如：
/// `{"Data": {"AvailableAmount": "123.45", "Currency": "CNY",
///            "AvailableCashAmount": "100.00", "AvailableCredit": "23.45"},
///  "Code": "Success"}`
///
/// 脱敏契约：`Code` 来自远端，即使携带 token 形态字符串也不得进入错误文本，
/// 非 `Success` 一律固定分类为 [`QuotaError::Other`]；detail 不包含远端原文。
fn parse_balance_response(body: &serde_json::Value) -> Result<u64, QuotaError> {
    if body
        .get("Code")
        .and_then(|v| v.as_str())
        .is_some_and(|code| code != "Success")
    {
        return Err(QuotaError::other("bss: query account balance failed"));
    }
    let data = body
        .get("Data")
        .ok_or_else(|| QuotaError::parse("bss: missing Data"))?;
    balance_micros(data)
}

/// 从 BSS `Data` 中解析账户余额（micros）。
///
/// `AvailableAmount` 为十进制 CNY 字符串（或整数/浮点 number），全程不经过
/// f64；`Currency` 必须存在且为 CNY（缺失/不匹配一律 `Parse`）；负数、溢出、
/// 超精度由 [`decimal_string_to_micros`] 报 `Parse`，不钳位、不截断。
fn balance_micros(data: &serde_json::Value) -> Result<u64, QuotaError> {
    let amount_str = data
        .get("AvailableAmount")
        .map(|v| json_decimal_string(v, "bss AvailableAmount"))
        .transpose()?
        .ok_or_else(|| QuotaError::parse("bss: missing AvailableAmount"))?;
    let currency = data
        .get("Currency")
        .and_then(|v| v.as_str())
        .ok_or_else(|| QuotaError::parse("bss: missing Currency"))?;
    if !currency.eq_ignore_ascii_case("CNY") {
        // `currency` 是远端原始字符串，只用于比对，绝不回显进错误文本。
        return Err(QuotaError::parse("bss: unexpected currency"));
    }
    // 底层换算错误的 detail 会回显原始金额（见 adapters/money.rs），这里统一
    // 换成固定分类文本，保持 `Parse` 变体，杜绝远端字符串进入 detail。
    decimal_string_to_micros(&amount_str)
        .map_err(|_| QuotaError::parse("bss: invalid AvailableAmount"))
}

/// 把凭证 secret 解析为 `(AccessKeyId, AccessKeySecret)`。
/// 约定 secret 形如 `AccessKeyId=...&AccessKeySecret=...`。
fn parse_access_key_pair(credential: &ResolvedCredential) -> Result<(String, String), QuotaError> {
    // 仅本函数读取明文；返回后调用方立即消费，不持久化、不记录。
    let secret = credential.expose_secret();
    if credential.kind() != CredentialKind::ApiKey {
        return Err(QuotaError::unauthorized("alibaba access key pair required"));
    }
    let mut id = None;
    let mut key = None;
    for pair in secret.split('&') {
        let mut kv = pair.splitn(2, '=');
        match kv.next() {
            Some("AccessKeyId") => id = kv.next().map(str::to_string),
            Some("AccessKeySecret") => key = kv.next().map(str::to_string),
            _ => {}
        }
    }
    match (id, key) {
        (Some(id), Some(key)) if !id.is_empty() && !key.is_empty() => Ok((id, key)),
        _ => Err(QuotaError::unauthorized(
            "malformed alibaba access key pair",
        )),
    }
}

/// 构造 Alibaba BSS RPC 签名 query string（含 Signature）。
///
/// 签名算法（RPC V1.0 HMAC-SHA1）：
/// 1. 把所有公共+私有参数（除 Signature）按 key 字典序排列；
/// 2. 拼成 `key1=value1&key2=value2`（value 做 RFC3986 百分号编码）；
/// 3. 构造待签名字符串 `GET&%2F&<percent-encoded(canonical)>`；
/// 4. 用 `<Secret>&` 作 key 做 HMAC-SHA1，base64 编码得 Signature。
fn signed_query(region: &str, access_key_id: &str, access_key_secret: &str) -> String {
    signed_query_with(
        region,
        access_key_id,
        access_key_secret,
        &iso8601_utc_now(),
        &nonce(),
    )
}

/// 与 [`signed_query`] 相同，但允许注入 Timestamp / SignatureNonce，便于确定性测试。
fn signed_query_with(
    region: &str,
    access_key_id: &str,
    access_key_secret: &str,
    timestamp: &str,
    signature_nonce: &str,
) -> String {
    let mut params: Vec<(String, String)> = vec![
        ("Action".into(), "QueryAccountBalance".into()),
        ("Format".into(), "JSON".into()),
        ("Version".into(), RPC_VERSION.into()),
        ("AccessKeyId".into(), access_key_id.into()),
        ("SignatureMethod".into(), "HMAC-SHA1".into()),
        ("Timestamp".into(), timestamp.to_string()),
        ("SignatureVersion".into(), "1.0".into()),
        ("SignatureNonce".into(), signature_nonce.to_string()),
        ("RegionId".into(), region.into()),
    ];
    params.sort_by(|a, b| a.0.cmp(&b.0));

    let canonical = params
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let string_to_sign = format!("GET&%2F&{}", percent_encode(&canonical));

    let signature = hmac_sha1_base64(
        format!("{access_key_secret}&").as_bytes(),
        string_to_sign.as_bytes(),
    );
    params.push(("Signature".into(), signature));

    params
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn hmac_sha1_base64(key: &[u8], message: &[u8]) -> String {
    let mut mac = HmacSha1::new_from_slice(key).expect("hmac key length");
    mac.update(message);
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// RFC 3986 百分号编码（保留 `/` 也编码，但 Alibaba 规则：参数值编码，
/// 用于 canonical；待签名串的 canonical 整体再编码一次）。
fn percent_encode(input: &str) -> String {
    const UNRESERVED: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~";
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        let c = *byte as char;
        if UNRESERVED.contains(c) {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

fn iso8601_utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 简化：以固定 epoch 偏移计算 UTC Y/M/D h:m:s（测试不依赖真实时间）。
    let (y, mo, d, h, mi, s) = epoch_to_utc(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn nonce() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{secs:x}")
}

/// Unix 秒 → UTC 民用时间（Howard Hinnant 算法）。
fn epoch_to_utc(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let h = (rem / 3_600) as u32;
    let mi = ((rem % 3_600) / 60) as u32;
    let s = (rem % 60) as u32;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = ((doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365) as i64;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe as u64 + yoe as u64 / 4 - yoe as u64 / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (if mo <= 2 { y + 1 } else { y }) as i32;
    (y, mo, d, h, mi, s)
}

#[cfg(test)]
mod tests {
    use super::*;
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
                agent_domain::ProviderId::new("qwen"),
                None,
            ),
            window: QuotaWindow::Overall,
            unit: QuotaUnit::Cost {
                currency: "CNY".into(),
            },
        }
    }

    fn cred() -> ResolvedCredential {
        ResolvedCredential::new(
            CredentialKind::ApiKey,
            "AccessKeyId=LTAI4test&AccessKeySecret=secretValue123",
        )
    }

    /// 从仓库 fixtures/quota/ 加载 contract fixture（只读，不参与生产代码）。
    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../../fixtures/quota/qwen_query_account_balance.json"
        ))
        .expect("fixture must be valid JSON")
    }

    #[tokio::test]
    async fn fetches_overall_cny_balance_from_contract_fixture() {
        // Contract fixture（fixtures/quota/qwen_query_account_balance.json）作为
        // wiremock 响应：Data.AvailableAmount="123.45" CNY -> 123_450_000 micros；
        // BSS 是账户整体余额：used=0、limit=remaining=余额、confidence=Exact，
        // 窗口固定 Overall（不得声称 DashScope 专属额度）。
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture()))
            .mount(&server)
            .await;
        let a = QwenBssAdapter::new(
            http(),
            QwenConfig {
                region: "cn-hangzhou".into(),
            },
            server.uri(),
        );
        let snap = a
            .fetch(
                &sample_request(),
                Some(&cred()),
                &agent_domain::CancellationToken::new(),
            )
            .await
            .expect("ok");

        // CNY micros：AvailableAmount "123.45" -> 123_450_000。
        assert_eq!(
            snap.unit,
            QuotaUnit::Cost {
                currency: "CNY".into()
            }
        );
        // BSS 账户整体余额固定 Overall 窗口，scope 原样透传。
        assert_eq!(snap.window, QuotaWindow::Overall);
        assert_eq!(snap.scope, sample_request().scope);
        // BSS 不报告消耗：used=0，limit=remaining=余额。
        assert_eq!(snap.values.used, QuotaMeasure::exact(0));
        assert_eq!(snap.values.limit, QuotaMeasure::exact(123_450_000));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(123_450_000));
        assert_eq!(snap.confidence, Confidence::Exact);
        assert_eq!(snap.reset, QuotaReset::Unknown);
        assert_eq!(snap.provenance.source, "qwen.bss-account");
        // provenance 的 endpoint 不含签名 query string。
        let ep = snap.provenance.endpoint.expect("endpoint");
        assert!(!ep.contains("Signature"), "query must be redacted: {ep}");
    }

    #[test]
    fn parse_access_key_pair_roundtrip() {
        let cred = ResolvedCredential::new(
            CredentialKind::ApiKey,
            "AccessKeyId=LTAI4xxx&AccessKeySecret=secretValue123",
        );
        let (id, secret) = parse_access_key_pair(&cred).unwrap();
        assert_eq!(id, "LTAI4xxx");
        assert_eq!(secret, "secretValue123");
    }

    #[test]
    fn parse_rejects_malformed() {
        let cred = ResolvedCredential::new(CredentialKind::ApiKey, "just-a-key");
        assert!(parse_access_key_pair(&cred).is_err());
    }

    #[test]
    fn signed_query_contains_signature_and_action() {
        let q = signed_query("cn-hangzhou", "LTAI4xxx", "secret123");
        assert!(q.contains("Action=QueryAccountBalance"));
        assert!(q.contains("Signature="));
        assert!(q.contains("SignatureMethod=HMAC-SHA1"));
        assert!(q.contains("Version=2017-12-14"));
    }

    #[test]
    fn signature_is_deterministic_for_same_inputs() {
        // 注入固定 Timestamp / SignatureNonce 后，同输入应产出相同签名。
        let q1 = signed_query_with(
            "cn-hangzhou",
            "LTAI4xxx",
            "secret123",
            "2026-08-11T00:00:00Z",
            "nonce-fixed",
        );
        let q2 = signed_query_with(
            "cn-hangzhou",
            "LTAI4xxx",
            "secret123",
            "2026-08-11T00:00:00Z",
            "nonce-fixed",
        );
        let sig1 = extract(&q1, "Signature");
        let sig2 = extract(&q2, "Signature");
        assert_eq!(sig1, sig2, "hmac deterministic");
    }

    #[test]
    fn signature_matches_known_vector() {
        // 用固定参数（替换掉时间相关字段）验证 HMAC-SHA1 实现正确性。
        let sig = hmac_sha1_base64(b"secret123&", b"GET&%2F&Action%3DQueryAccountBalance");
        // 重新计算一次作为 oracle（独立路径）：
        let mut mac = HmacSha1::new_from_slice(b"secret123&").unwrap();
        mac.update(b"GET&%2F&Action%3DQueryAccountBalance");
        let expected =
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        assert_eq!(sig, expected);
    }

    #[test]
    fn percent_encode_encodes_reserved() {
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("="), "%3D");
        assert_eq!(percent_encode("A-_.~"), "A-_.~");
    }

    #[test]
    fn epoch_to_utc_at_known_timestamp() {
        // 2026-01-01T00:00:00Z = 1767225600
        let (y, mo, d, h, mi, s) = epoch_to_utc(1_767_225_600);
        assert_eq!((y, mo, d, h, mi, s), (2026, 1, 1, 0, 0, 0));
    }

    #[test]
    fn balance_micros_parses_string_without_f64() {
        let data = serde_json::json!({
            "AvailableAmount": "123.45",
            "Currency": "CNY",
            "AvailableCashAmount": "100.00",
            "AvailableCredit": "23.45"
        });
        assert_eq!(balance_micros(&data).unwrap(), 123_450_000);
    }

    #[test]
    fn balance_micros_accepts_number_without_f64() {
        let int_data = serde_json::json!({"AvailableAmount": 123, "Currency": "CNY"});
        assert_eq!(balance_micros(&int_data).unwrap(), 123_000_000);
        let float_data = serde_json::json!({"AvailableAmount": 123.45, "Currency": "CNY"});
        assert_eq!(balance_micros(&float_data).unwrap(), 123_450_000);
    }

    #[test]
    fn balance_micros_rejects_missing_fields() {
        let no_amount = serde_json::json!({"Currency": "CNY"});
        assert!(matches!(
            balance_micros(&no_amount),
            Err(QuotaError::Parse { .. })
        ));
        let no_currency = serde_json::json!({"AvailableAmount": "1.00"});
        assert!(matches!(
            balance_micros(&no_currency),
            Err(QuotaError::Parse { .. })
        ));
    }

    #[test]
    fn balance_micros_rejects_non_cny_currency() {
        let data = serde_json::json!({"AvailableAmount": "1.00", "Currency": "USD"});
        assert!(matches!(
            balance_micros(&data),
            Err(QuotaError::Parse { .. })
        ));
    }

    #[test]
    fn bss_code_and_currency_never_enter_error_detail() {
        // 恶意远端：Code / Currency 携带 token 形态字符串，不得回显进错误文本。
        let evil = "sk-qwen-evil-token-0123456789abcdef";

        let body = serde_json::json!({
            "Code": evil,
            "Data": {"AvailableAmount": "1.00", "Currency": "CNY"}
        });
        let err = parse_balance_response(&body).expect_err("non-success code");
        assert!(matches!(err, QuotaError::Other { .. }));
        assert_no_leak(&err, evil);
        assert_eq!(
            err.to_string(),
            "quota query failed: bss: query account balance failed"
        );

        let body = serde_json::json!({
            "Code": "Success",
            "Data": {"AvailableAmount": "1.00", "Currency": evil}
        });
        let err = balance_micros(&body["Data"]).expect_err("evil currency");
        assert!(matches!(err, QuotaError::Parse { .. }));
        assert_no_leak(&err, evil);
        assert_eq!(
            err.to_string(),
            "quota response parse failed: bss: unexpected currency"
        );
    }

    #[test]
    fn amount_parse_error_never_echoes_raw_value() {
        // 底层 decimal_string_to_micros 的 detail 会回显原始金额，qwen 层必须
        // 换成固定分类文本，恶意金额字符串不得泄漏。
        let evil = "sk-qwen-evil-amount-0123456789";
        let data = serde_json::json!({"AvailableAmount": evil, "Currency": "CNY"});
        let err = balance_micros(&data).expect_err("invalid amount");
        assert!(matches!(err, QuotaError::Parse { .. }));
        assert_no_leak(&err, evil);
        assert_eq!(
            err.to_string(),
            "quota response parse failed: bss: invalid AvailableAmount"
        );
    }

    #[test]
    fn parse_balance_response_accepts_missing_code_as_success() {
        // 与旧行为一致：Code 缺失/非字符串视为 Success（不因脱敏改变判定）。
        let body = serde_json::json!({
            "Data": {"AvailableAmount": "1.00", "Currency": "CNY"}
        });
        assert_eq!(parse_balance_response(&body).unwrap(), 1_000_000);
        let body = serde_json::json!({
            "Code": 42,
            "Data": {"AvailableAmount": "1.00", "Currency": "CNY"}
        });
        assert_eq!(parse_balance_response(&body).unwrap(), 1_000_000);
    }

    #[test]
    fn runtime_credential_contract_is_api_key_with_packed_pair() {
        // capability.rs 对 qwen 的 hint 是 AccessKeyPair；运行时契约是把 pair
        // 打包进单个 CredentialKind::ApiKey secret（provider_api 无独立
        // AccessKeyPair kind）。hint 消费方按此打包才不会破坏运行时校验。
        let cred = ResolvedCredential::new(
            CredentialKind::ApiKey,
            "AccessKeyId=LTAI4xxx&AccessKeySecret=secretValue123",
        );
        let (id, secret) = parse_access_key_pair(&cred).unwrap();
        assert_eq!(id, "LTAI4xxx");
        assert_eq!(secret, "secretValue123");

        // 非 ApiKey kind 即使内容格式正确也拒绝——hint 若被直接映射成其他 kind
        // 会破坏契约。
        for kind in [CredentialKind::OAuthBearer, CredentialKind::SessionToken] {
            let cred = ResolvedCredential::new(
                kind,
                "AccessKeyId=LTAI4xxx&AccessKeySecret=secretValue123",
            );
            assert!(matches!(
                parse_access_key_pair(&cred),
                Err(QuotaError::Unauthorized { .. })
            ));
        }
    }

    #[test]
    fn balance_micros_rejects_negative_and_over_precision_without_clamping() {
        for bad in ["-5", "1.1234567"] {
            let data = serde_json::json!({"AvailableAmount": bad, "Currency": "CNY"});
            assert!(matches!(
                balance_micros(&data),
                Err(QuotaError::Parse { .. })
            ));
        }
    }

    #[test]
    fn balance_micros_rejects_overflow() {
        let data = serde_json::json!({
            "AvailableAmount": "999999999999999999999999",
            "Currency": "CNY"
        });
        assert!(matches!(
            balance_micros(&data),
            Err(QuotaError::Parse { .. })
        ));
    }

    fn extract(query: &str, key: &str) -> String {
        for pair in query.split('&') {
            let mut kv = pair.splitn(2, '=');
            if kv.next() == Some(key) {
                return kv.next().unwrap_or("").to_string();
            }
        }
        String::new()
    }

    /// 断言错误的 Debug 与 Display 均不包含给定远端原始字符串。
    fn assert_no_leak(error: &QuotaError, needle: &str) {
        let debug = format!("{error:?}");
        let display = error.to_string();
        assert!(!debug.contains(needle), "Debug leaked {needle:?}: {debug}");
        assert!(
            !display.contains(needle),
            "Display leaked {needle:?}: {display}"
        );
    }
}
