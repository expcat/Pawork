//! 远程配额适配器通用骨架与跨 Provider 复用的工具。
//!
//! 两类可复用适配器骨架：
//! - [`api_key::ApiKeyQuotaAdapter`]：官方 API Key 额度接口（Exact）。
//! - [`web_scrape::WebScrapeQuotaAdapter`]：控制台网页抓取回退（Scraped）。
//!
//! 具体六家 Provider 的实现位于 `crate::providers`。通用 OAuth 层尚无生产
//! 消费者，已在首个真实 provider 接入前移除（P14 review §3.2）。

pub mod api_key;
pub mod http_util;
pub mod money;
pub mod web_scrape;
