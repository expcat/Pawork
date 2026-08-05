//! Secret 脱敏状态：`MaskedCredential` 只保留可安全记录的尾号/前缀信息。

use std::fmt;

use serde::{Deserialize, Serialize};

/// 仅含脱敏信息的凭证展示状态。
///
/// `Display` / `Debug` / `Serialize` 的输出**永远不会**包含明文 secret，
/// 因此可安全地写入数据库、日志或事件。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaskedCredential {
    /// 预计算好的脱敏字符串，例如 `sk-…abcd`。明文本身不会被保存到这里。
    masked: String,
}

impl MaskedCredential {
    /// 根据明文 secret 计算脱敏表示。明文不会以任何形式保留在本结构中。
    pub fn mask(secret: &str) -> Self {
        Self {
            masked: mask_secret(secret),
        }
    }

    /// 直接以已知脱敏字符串构造；调用方负责确保传入的不是明文。
    pub fn from_masked(masked: impl Into<String>) -> Self {
        Self {
            masked: masked.into(),
        }
    }

    /// 返回脱敏后的展示字符串（不含明文）。
    pub fn as_str(&self) -> &str {
        &self.masked
    }
}

/// 计算脱敏字符串的纯函数。
///
/// 规则（以 Unicode 标量值为单位，避免截断多字节字符）：
/// - 长度 `<= 4`：完全遮蔽为 `••••`，避免泄露过短 secret 的全部字符。
/// - 长度 `5..=8`：仅展示尾部 2 字符，形如 `…xy`。
/// - 长度 `> 8`：保留前 3 + 后 4 字符，形如 `pre…wxyz`。
fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    let len = chars.len();
    match len {
        0..=4 => "••••".to_string(),
        5..=8 => {
            let tail: String = chars[len - 2..].iter().collect();
            format!("…{tail}")
        }
        _ => {
            let head: String = chars[..3].iter().collect();
            let tail: String = chars[len - 4..].iter().collect();
            format!("{head}…{tail}")
        }
    }
}

impl fmt::Display for MaskedCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.masked)
    }
}

impl fmt::Debug for MaskedCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MaskedCredential")
            .field("masked", &self.masked)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_secret_keeps_head_and_tail() {
        let masked = MaskedCredential::mask("sk-abcdEFGHwxyz");
        assert_eq!(masked.as_str(), "sk-…wxyz");
    }

    #[test]
    fn short_secret_is_fully_redacted() {
        assert_eq!(MaskedCredential::mask("").as_str(), "••••");
        assert_eq!(MaskedCredential::mask("abc").as_str(), "••••");
        assert_eq!(MaskedCredential::mask("abcdef").as_str(), "…ef");
    }

    #[test]
    fn debug_display_serialize_never_leak_plaintext() {
        let secret = "sk-supersecret-token-1234567890";
        let masked = MaskedCredential::mask(secret);

        assert!(!format!("{masked:?}").contains(secret));
        assert!(!format!("{masked}").contains(secret));

        let json = serde_json::to_string(&masked).expect("serialize");
        assert!(!json.contains(secret));
        // 脱敏字符串确实出现在 JSON 里，但它只是 `sk-…7890` 这样的片段。
        assert!(json.contains("sk-"));
    }
}
