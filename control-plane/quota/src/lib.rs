//! Pawork 本地额度投影与 LocalLedger。
//!
//! 提供配额监控的 canonical 领域、适配器抽象、聚合缓存与 Ledger 对账：
//! - 隔离作用域：tenant + account + provider + optional model（[`QuotaScope`]）。
//! - 窗口：Overall / Rolling5h / Weekly / Monthly（[`QuotaWindow`]）。
//! - 单位：Count / Token / Cost（[`QuotaUnit`]）。
//! - 度量：used / limit / remaining，含 Infinite / Unknown（[`QuotaMeasure`]、[`QuotaValues`]）。
//! - 重置：绝对 / 相对 + 不确定性（[`QuotaReset`]）。
//! - 适配器获取方式：ApiKeyApi / OAuthApi / WebScrape / LocalLedger（[`AdapterKind`]）。
//! - 可信度：Exact / Derived / Scraped，默认最低可信 Scraped（[`Confidence`]）。
//! - 可见来源与新鲜度；endpoint 经清洗去除 query/fragment（[`QuotaProvenance`]）。
//! - 对象安全的异步适配器（cancel-safe，[`QuotaAdapter`]）与错误
//!   （[`QuotaError`]，含 retryable / retry_after_ms 分类）。
//! - 多来源并发聚合、singleflight、TTL、stale/部分失败（[`service`]）。
//! - 直接消费唯一 Usage Ledger 的本地派生与远端增量对账（[`ledger`]）。
//!
//! 远端 Provider 适配器与 RefreshScheduler 冻结候审，不在本 crate。
//!
//! 凭证安全：secret 仅以 [`pawork_domain::ResolvedCredential`] 在适配器调用边界注入；
//! 该类型 `Debug` 已脱敏且未实现 `Serialize`，故本 crate 的任何类型都无法在结构上
//! 持有或泄漏明文 secret。

mod adapter;
mod domain;
mod error;
pub mod ledger;
pub mod service;
mod util;

pub use adapter::{AdapterKind, QuotaAdapter};
pub use domain::*;
pub use error::QuotaError;
pub use ledger::LedgerQuotaAdapter;
pub use service::{CacheOverview, CacheRead, QuotaClock, QuotaService};
