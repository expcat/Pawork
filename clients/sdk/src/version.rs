//! 版本与兼容策略。

use pawork_protocol::{ApiVersion, SUPPORTED_API_VERSIONS};

/// SDK 自身语义化版本（crate 版本；按 semver 演进）。
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// SDK 期望的协议版本（V2 `API_VERSION` = 1.2；与 Host 握手时协商）。
///
/// 策略：SDK 的 minor 版本固定于它编译所对的协议 minor；Host 取 major 相同
/// 的最高共同 minor。SDK 遇到不兼容 major 时以
/// [`crate::SdkErrorKind::IncompatibleApiVersion`] 显式失败，不做猜测降级。
pub const SDK_API_VERSION: ApiVersion = pawork_protocol::API_VERSION;

/// SDK 声明的候选协议版本表（首元素为偏好版本）。
pub const SDK_SUPPORTED_API_VERSIONS: &[ApiVersion] = SUPPORTED_API_VERSIONS;

/// 人类可读的版本标识。
pub fn sdk_version_string() -> String {
    format!("pawork-sdk {SDK_VERSION} (protocol {SDK_API_VERSION:?})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_api_version_is_current() {
        assert_eq!(SDK_API_VERSION, pawork_protocol::API_VERSION);
        assert!(SDK_API_VERSION.is_compatible_with(pawork_protocol::API_VERSION));
    }

    #[test]
    fn version_string_contains_both_identities() {
        let text = sdk_version_string();
        assert!(text.contains("pawork-sdk"));
        assert!(text.contains("protocol"));
    }
}
