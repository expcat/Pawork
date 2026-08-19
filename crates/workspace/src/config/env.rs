//! 环境变量凭证读取（S0–S5 过渡机制）。
//!
//! 不构造 `ResolvedCredential`，不读配置文件、`.env` 或 auth 文件。

/// 由 provider id 推导环境变量名：`PAWORK_API_KEY_` + 大写，且 `-` → `_`。
pub fn api_key_env_name(provider_id: &str) -> String {
    let suffix = provider_id.to_ascii_uppercase().replace('-', "_");
    format!("PAWORK_API_KEY_{suffix}")
}

/// 读取 `PAWORK_API_KEY_<PROVIDER_ID>`；未设置或空字符串返回 `None`。
pub fn read_api_key_from_env(provider_id: &str) -> Option<String> {
    match std::env::var(api_key_env_name(provider_id)) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_env(key: &str, value: &str) {
        // Rust 1.87+ 将 set_var 标为 unsafe；各测试使用独立 key。
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(key, value);
        }
    }

    fn remove_env(key: &str) {
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn glm_coding_env_name() {
        assert_eq!(
            api_key_env_name("glm-coding"),
            "PAWORK_API_KEY_GLM_CODING"
        );
    }

    #[test]
    fn opencode_go_env_name() {
        assert_eq!(
            api_key_env_name("opencode-go"),
            "PAWORK_API_KEY_OPENCODE_GO"
        );
    }

    #[test]
    fn read_api_key_from_env_returns_none_when_unset() {
        let provider = "pawork-config-test-unset";
        let key = api_key_env_name(provider);
        remove_env(&key);
        assert_eq!(read_api_key_from_env(provider), None);
    }

    #[test]
    fn read_api_key_from_env_returns_none_when_empty() {
        let provider = "pawork-config-test-empty";
        let key = api_key_env_name(provider);
        set_env(&key, "");
        let got = read_api_key_from_env(provider);
        remove_env(&key);
        assert_eq!(got, None);
    }

    #[test]
    fn read_api_key_from_env_returns_non_empty() {
        let provider = "pawork-config-test-read";
        let key = api_key_env_name(provider);
        let fake = "test-fake-key-not-a-secret";
        set_env(&key, fake);
        let got = read_api_key_from_env(provider);
        remove_env(&key);
        assert_eq!(got.as_deref(), Some(fake));
    }
}
