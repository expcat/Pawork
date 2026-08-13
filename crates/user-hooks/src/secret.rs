//! Secret 引用与 redaction（P17-1 步骤 4）。
//!
//! 设计原则（与 Pawork 安全红线一致）：
//! - 配置、Event 与日志只保存 secret **引用**（[`SecretRef`]）；
//! - 运行前即时解析为明文 [`SecretValue`]，仅注入获批的环境变量 / allowlisted header；
//! - 明文 `SecretValue` 不实现 `Debug`/`Serialize`，作用域结束即 drop；
//! - 审计 / 日志全程经 [`redact`] 替换为占位符。

use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroizing;
/// Secret 引用：只携带逻辑名，永不包含明文。可安全入库 / 入日志。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(pub String);

impl SecretRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 引用名本身不是明文，可展示。
        write!(f, "secret:{}", self.0)
    }
}

/// Secret 明文值。内部用 `Zeroizing<String>`：所有副本（含 clone）在 Drop 时
/// 自动清零，避免明文在内存驻留。不实现 `Serialize`；不实现 `Debug` 以防误打印。
pub struct SecretValue {
    inner: Zeroizing<String>,
}

impl SecretValue {
    /// 由可信的 [`crate::SecretResolver`] 实现构造明文值。
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            inner: Zeroizing::new(value.into()),
        }
    }
    /// 以明文访问（仅在注入执行器前临时使用）。
    pub fn as_str(&self) -> &str {
        &self.inner
    }
    /// 转为可放进请求字段的 [`SecretString`]（同样 Drop 清零）。
    pub fn to_secret_string(&self) -> SecretString {
        SecretString::new(self.inner.as_str())
    }
}

/// 可放进请求字段（如 `CommandRequest::env`、`WebhookRequest::headers`）的
/// secret 明文 wrapper。`Clone` 产生的每个副本都会在 Drop 时清零，确保所有
/// 明文副本（包括注入到请求里的那份）都最终清零。不实现 `Serialize`；
/// `Debug` 永远显示占位符，永不泄露明文。
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Clone for SecretString {
    fn clone(&self) -> Self {
        Self(Zeroizing::new(self.0.as_str().to_string()))
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(***REDACTED***)")
    }
}

/// redaction 占位符。
pub const REDACTED: &str = "***REDACTED***";

/// 把文本中所有出现的 secret 明文替换为 [`REDACTED`]。
///
/// 用于审计记录、日志、provider prompt 模板渲染前。输入 `secrets` 为本次
/// 解析出的明文集合（短生命周期）。
pub fn redact(text: &str, secrets: &[&SecretValue]) -> String {
    let mut out = text.to_string();
    for s in secrets {
        let plain = s.as_str();
        if !plain.is_empty() && out.contains(plain) {
            out = out.replace(plain, REDACTED);
        }
    }
    out
}

/// 递归 redaction 一个 JSON value 中所有字符串字段。
pub fn redact_value(value: &serde_json::Value, secrets: &[&SecretValue]) -> serde_json::Value {
    use serde_json::{Map, Value};
    match value {
        Value::String(s) => Value::String(redact(s, secrets)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| redact_value(v, secrets)).collect())
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                // key 也可能泄露 secret（如 header 名为 secret 值），统一 redact
                out.insert(redact(k, secrets), redact_value(v, secrets));
            }
            Value::Object(out)
        }
        // 数字 / bool / null 不会包含 secret 明文，原样返回。
        other => other.clone(),
    }
}

/// 对 URL 做审计 / 日志 redaction：query 与 fragment 可能携带 secret（token、
/// signature 等），统一替换为占位符；保留 scheme://host/path 供审计定位。
///
/// 不解析 URL（不引入第三方依赖）：按第一个 `?` / `#` 截断，后续内容整体
/// 遮蔽；无 query / fragment 时原样返回。
pub fn redact_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    match trimmed.find(['?', '#']) {
        Some(index) => {
            let mut out = trimmed[..index].to_string();
            if trimmed.as_bytes().get(index) == Some(&b'?') {
                out.push_str("?***REDACTED***");
            } else {
                out.push_str("#***REDACTED***");
            }
            out
        }
        None => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_url_strips_query_and_fragment() {
        assert_eq!(
            redact_url("https://hooks.example.com/endpoint?token=secret&a=b"),
            "https://hooks.example.com/endpoint?***REDACTED***"
        );
        assert_eq!(
            redact_url("https://hooks.example.com/endpoint#frag"),
            "https://hooks.example.com/endpoint#***REDACTED***"
        );
        assert_eq!(
            redact_url("https://hooks.example.com/plain"),
            "https://hooks.example.com/plain"
        );
        assert_eq!(redact_url(""), "");
    }
}
