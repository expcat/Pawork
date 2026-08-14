use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 跨 crate 稳定的错误大类；具体模块错误通过 `From` 转换到此分类。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Provider,
    Tool,
    Internal,
    Cancelled,
    RateLimit,
    Timeout,
    Authentication,
    Authorization,
    InvalidRequest,
    NotFound,
    Conflict,
    ResourceExhausted,
    Unavailable,
    MalformedData,
}

/// 可安全跨边界传递的错误上下文；不得包含 Secret 或未经脱敏的响应正文。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
pub struct ErrorContext {
    pub category: ErrorCategory,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub diagnostics: BTreeMap<String, String>,
}
