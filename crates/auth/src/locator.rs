//! 凭证定位规则单一事实源（R5 波 B）。
//!
//! env 名推导、auth 文件 service 命名与 MCP 域隔离规则集中在本模块；
//! 所有字符串值与既有 auth.json 落盘形状逐字节不变，仅消除多份同形实现。

use pawork_domain::ProviderId;

/// Secret 后端中按 Provider 分组的命名空间前缀。
pub const PROVIDER_SERVICE_PREFIX: &str = "pawork";

/// MCP secret 的独立命名空间前缀（域隔离：禁止解析 Provider / OAuth 凭证）。
pub const MCP_SERVICE_PREFIX: &str = "pawork.mcp.";

/// MCP 独立 secret 后端文件名（与 auth.json 同目录，绝不共用 Provider 后端）。
pub const MCP_AUTH_FILE_NAME: &str = "mcp-auth.json";

/// 计算 Provider 主条目在 Secret 后端中的 service 名（形如 pawork.openai）。
pub fn secret_service_for(provider: &ProviderId) -> String {
    format!("{PROVIDER_SERVICE_PREFIX}.{provider}")
}

/// 该 provider 的 OAuth service 名（形如 pawork.chatgpt.oauth）。
pub fn oauth_secret_service(provider: &ProviderId) -> String {
    format!("{PROVIDER_SERVICE_PREFIX}.{provider}.oauth")
}

/// 判断 service 是否落在 MCP 独立命名空间（pawork.mcp.*）。
pub fn is_mcp_secret_service(service: &str) -> bool {
    service.starts_with(MCP_SERVICE_PREFIX)
}

/// 由 provider id 推导环境变量名：PAWORK_API_KEY_ + 大写、- 转 _。
pub fn api_key_env_name(provider_id: &str) -> String {
    let suffix = provider_id.to_ascii_uppercase().replace('-', "_");
    format!("PAWORK_API_KEY_{suffix}")
}

/// 读取 PAWORK_API_KEY_<PROVIDER_ID>；未设置或空字符串视为缺失。
pub fn read_api_key_from_env(provider_id: &str) -> Option<String> {
    match std::env::var(api_key_env_name(provider_id)) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_names_keep_auth_file_shape() {
        let provider = ProviderId::new("chatgpt");
        assert_eq!(secret_service_for(&provider), "pawork.chatgpt");
        assert_eq!(oauth_secret_service(&provider), "pawork.chatgpt.oauth");
    }

    #[test]
    fn mcp_namespace_is_isolated_from_provider_services() {
        assert!(is_mcp_secret_service("pawork.mcp.filesystem"));
        assert!(!is_mcp_secret_service("pawork.mcp"));
        assert!(!is_mcp_secret_service("pawork.openai"));
        assert!(!is_mcp_secret_service("pawork.chatgpt.oauth"));
    }

    #[test]
    fn mcp_auth_file_name_is_pinned() {
        assert_eq!(MCP_AUTH_FILE_NAME, "mcp-auth.json");
    }
}
