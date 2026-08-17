//! 配额适配器抽象与获取方式分类。

use async_trait::async_trait;
use pawork_api::ResolvedCredential;
use pawork_domain::CancellationToken;
use serde::{Deserialize, Serialize};

use crate::{QuotaError, QuotaRequest, QuotaSnapshot};

/// 适配器获取配额读数的方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    /// 调用需要 API Key 的官方额度接口。
    #[default]
    ApiKeyApi,
    /// 调用需要 OAuth 的官方额度接口。
    #[serde(rename = "oauth_api")]
    OAuthApi,
    /// 抓取控制台网页（DOM/截图解析）。
    WebScrape,
    /// 由本地 Usage Ledger 派生。
    LocalLedger,
}

/// 对象安全的异步配额适配器。
///
/// 凭证仅以 [`ResolvedCredential`] 引用传入：其 `Debug` 已脱敏、未实现
/// `Serialize`，因此实现方在结构上无法把明文 secret 写入日志、事件或返回的
/// [`QuotaSnapshot`]。实现方应在内部尽快消费凭证、不得持有其拷贝。
///
/// ## Cancel-safe 契约
///
/// - `cancel` 触发后，实现必须尽快返回（优先返回 [`QuotaError::Cancelled`]），
///   不得继续外部调用、重试或写入副作用；未触发时不得提前返回。
/// - 被取消的 future 可能随时被直接 drop（无 `Cancelled` 返回路径），因此实现
///   不得在取消后留下部分状态或悬空资源，也不得依赖 drop 之外的后置清理。
/// - 对外部服务的请求必须与 `cancel` 绑定（如 `tokio::select!` 竞争），
///   取消不得泄漏正在进行的请求或触发重复计费。
#[async_trait]
pub trait QuotaAdapter: Send + Sync {
    /// 适配器获取读数的方式。
    fn kind(&self) -> AdapterKind;

    /// Capability discovery. Returning false is equivalent to `Unsupported`,
    /// but lets the aggregator avoid unnecessary authentication and network IO.
    fn supports(&self, request: &QuotaRequest) -> bool;

    /// 读取配额快照。
    async fn fetch(
        &self,
        request: &QuotaRequest,
        credential: Option<&ResolvedCredential>,
        cancel: &CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_kind_serde_is_snake_case() {
        let cases = [
            (AdapterKind::ApiKeyApi, "api_key_api"),
            (AdapterKind::OAuthApi, "oauth_api"),
            (AdapterKind::WebScrape, "web_scrape"),
            (AdapterKind::LocalLedger, "local_ledger"),
        ];
        for (kind, expected) in cases {
            let json = serde_json::to_string(&kind).expect("serialize adapter kind");
            assert_eq!(json, format!("\"{expected}\""));
            let back: AdapterKind = serde_json::from_str(&json).expect("deserialize adapter kind");
            assert_eq!(back, kind);
        }
    }

    /// 最小桩适配器：证明 `dyn QuotaAdapter` 可构造、可调用，并短路为 Unsupported。
    struct UnsupportedAdapter(AdapterKind);

    #[async_trait]
    impl QuotaAdapter for UnsupportedAdapter {
        fn kind(&self) -> AdapterKind {
            self.0
        }

        fn supports(&self, _request: &QuotaRequest) -> bool {
            false
        }

        async fn fetch(
            &self,
            _request: &QuotaRequest,
            _credential: Option<&ResolvedCredential>,
            _cancel: &CancellationToken,
        ) -> Result<QuotaSnapshot, QuotaError> {
            Err(QuotaError::unsupported("stub adapter reports nothing"))
        }
    }

    #[tokio::test]
    async fn dyn_adapter_is_object_safe_and_reports_unsupported() {
        let adapter: Box<dyn QuotaAdapter> = Box::new(UnsupportedAdapter(AdapterKind::ApiKeyApi));
        assert_eq!(adapter.kind(), AdapterKind::ApiKeyApi);

        let request = QuotaRequest {
            scope: crate::QuotaScope::new(
                pawork_domain::TenantId::new("tenant-a"),
                crate::AccountId::new("account-1"),
                pawork_domain::ProviderId::new("mock"),
                None,
            ),
            ..Default::default()
        };
        assert!(!adapter.supports(&request));
        let cancel = CancellationToken::new();
        let outcome = adapter.fetch(&request, None, &cancel).await;

        assert!(matches!(outcome, Err(QuotaError::Unsupported { .. })));
    }
}
