//! 远程配额适配器通用骨架与跨 Provider 复用的工具。
//!
//! 三类可复用适配器骨架：
//! - [`api_key::ApiKeyQuotaAdapter`]：官方 API Key 额度接口（Exact）。
//! - [`oauth::OAuthQuotaAdapter`]：官方 OAuth 额度接口，含 refresh / 401 一次性
//!   重试 / 重新授权映射（Exact）。
//! - [`web_scrape::WebScrapeQuotaAdapter`]：控制台网页抓取回退（Scraped）。
//!
//! 具体六家 Provider 的实现位于 `crate::providers`。

pub mod api_key;
pub mod http_util;
pub mod money;
pub mod oauth;
pub mod web_scrape;
