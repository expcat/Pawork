//! 凭证解析链（S6）：auth 文件 → env fallback → 无凭证。
//!
//! [`resolve_provider_credential`] 是 Provider 装配期的统一凭证入口：先查
//! SecretBackend 的 Provider 主条目（service 沿用 [`StoredCredential`] 约定
//! `pawork.<provider>`，固定 account `default`），未命中再读
//! `PAWORK_API_KEY_<ID 大写、`-`→`_`>`；仅 [`AuthError::NotFound`] 允许降级，
//! 后端损坏或访问失败原样上抛；两级都缺返回 [`CredentialSource::None`]，由调用方
//! fail-closed。
//!
//! env 值只进入 [`ResolvedCredential`]（`Debug` 已脱敏），不落任何日志或 Debug
//! 泄漏字段；env 名推导与 service 命名统一由 [`crate::locator`] 单一事实源提供。

use pawork_domain::{CredentialId, ProviderId};
use pawork_domain::{CredentialKind, ResolvedCredential};

use crate::backend::SecretBackend;
use crate::credential::StoredCredential;
use crate::error::AuthError;
use crate::locator::{read_api_key_from_env, secret_service_for};
use crate::masked::MaskedCredential;

/// Provider 主条目在 SecretBackend 中的固定 `account`。
pub const PROVIDER_DEFAULT_ACCOUNT: &str = "default";

/// 写入 Provider 主条目（account 固定 default）：auth set-key 的落点，
/// 与 resolve_provider_credential 的读取口径一致。
pub fn store_default_api_key(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    secret: &str,
) -> Result<StoredCredential, AuthError> {
    if secret.is_empty() {
        return Err(AuthError::InvalidSecret("secret is empty".into()));
    }
    let service = secret_service_for(provider);
    backend.store(&service, PROVIDER_DEFAULT_ACCOUNT, secret)?;
    Ok(StoredCredential::new(
        CredentialId::new(PROVIDER_DEFAULT_ACCOUNT),
        provider.clone(),
        format!("{} default", provider.as_str()),
        MaskedCredential::mask(secret),
        service,
        PROVIDER_DEFAULT_ACCOUNT,
        Vec::new(),
    ))
}

/// 删除 Provider 主条目（幂等：条目不存在视为成功）。env fallback 不受影响。
pub fn delete_default_api_key(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
) -> Result<(), AuthError> {
    match backend.delete(&secret_service_for(provider), PROVIDER_DEFAULT_ACCOUNT) {
        Ok(()) | Err(AuthError::NotFound) => Ok(()),
        Err(error) => Err(error),
    }
}

/// [`resolve_provider_credential`] 的解析结果（来源标记，不含明文 secret）。
///
/// - [`CredentialSource::AuthFile`]：主条目命中，返回可持久化元数据。明文仍在
///   SecretBackend（正式后端为 auth 文件）中，需要时
///   经 [`crate::ApiKeyCredential::resolve`] 解析。
/// - [`CredentialSource::EnvFallback`]：headless/CI fallback 命中的
///   [`ResolvedCredential`]（`Debug` 脱敏，仅供 adapter 构造认证请求）。
/// - [`CredentialSource::None`]：两级都未命中，调用方必须 fail-closed。
#[derive(Debug)]
pub enum CredentialSource {
    /// 持久化 auth 文件条目命中（元数据 + 定位信息，不含明文）。
    AuthFile(StoredCredential),
    /// auth 文件未命中、env fallback 命中。
    EnvFallback(ResolvedCredential),
    /// 两级均未命中。
    None,
}

/// 解析 Provider 凭证：auth 文件主条目 → env fallback → 无凭证。
///
/// auth 文件侧仅「条目不存在」视为未命中并继续 env；后端损坏或访问异常必须
/// fail-closed。两级都缺时返回 [`CredentialSource::None`]，绝不构造伪凭证。
pub fn resolve_provider_credential(
    backend: &dyn SecretBackend,
    provider_id: &str,
) -> Result<CredentialSource, AuthError> {
    let provider = ProviderId::new(provider_id);
    let service = secret_service_for(&provider);
    match backend.get(&service, PROVIDER_DEFAULT_ACCOUNT) {
        Ok(secret) => {
            let stored = StoredCredential::new(
                CredentialId::new(PROVIDER_DEFAULT_ACCOUNT),
                provider,
                format!("{provider_id} default"),
                MaskedCredential::mask(&secret),
                service,
                PROVIDER_DEFAULT_ACCOUNT,
                Vec::new(),
            );
            Ok(CredentialSource::AuthFile(stored))
        }
        Err(AuthError::NotFound) => Ok(match read_api_key_from_env(provider_id) {
            Some(value) => CredentialSource::EnvFallback(ResolvedCredential::new(
                CredentialKind::ApiKey,
                value,
            )),
            None => CredentialSource::None,
        }),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MemoryBackend;
    use crate::credential::ApiKeyCredential;
    use crate::locator::api_key_env_name;

    /// 各测试使用独立 provider id，避免并行测试共享同一环境变量。
    fn set_env(key: &str, value: &str) {
        // Rust 1.87+ 将 set_var 标为 unsafe；此处为测试专用 key。
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
    fn env_name_uppercases_and_replaces_hyphens() {
        assert_eq!(api_key_env_name("glm-coding"), "PAWORK_API_KEY_GLM_CODING");
        assert_eq!(
            api_key_env_name("opencode-go"),
            "PAWORK_API_KEY_OPENCODE_GO"
        );
    }

    #[test]
    fn file_hit_beats_env_fallback() {
        let backend = MemoryBackend::new();
        let provider = "resolve-file-hit";
        let env_name = api_key_env_name(provider);
        set_env(&env_name, "sk-env-should-not-be-used");
        backend
            .store(
                "pawork.resolve-file-hit",
                PROVIDER_DEFAULT_ACCOUNT,
                "sk-file-primary-000000",
            )
            .expect("store");

        let source = resolve_provider_credential(&backend, provider).expect("resolve");
        remove_env(&env_name);

        let CredentialSource::AuthFile(stored) = source else {
            panic!("expected file hit");
        };
        assert_eq!(stored.provider.as_str(), provider);
        assert_eq!(stored.id.as_str(), PROVIDER_DEFAULT_ACCOUNT);
        assert_eq!(stored.secret_service, "pawork.resolve-file-hit");
        assert_eq!(stored.secret_account, PROVIDER_DEFAULT_ACCOUNT);
        // 元数据与 Debug 输出不含明文。
        assert!(!format!("{stored:?}").contains("sk-file-primary"));
        // 沿用 StoredCredential 惯例可解析回明文。
        let resolved = ApiKeyCredential::from_stored(stored)
            .expect("from_stored")
            .resolve(&backend)
            .expect("resolve");
        assert_eq!(resolved.expose_secret(), "sk-file-primary-000000");
    }

    #[test]
    fn file_miss_falls_back_to_env() {
        let backend = MemoryBackend::new();
        let provider = "resolve-env-fallback";
        let env_name = api_key_env_name(provider);
        set_env(&env_name, "sk-env-fallback-abcdef");

        let source = resolve_provider_credential(&backend, provider).expect("resolve");
        let source_debug = format!("{source:?}");
        remove_env(&env_name);

        let CredentialSource::EnvFallback(resolved) = source else {
            panic!("expected env fallback");
        };
        assert_eq!(resolved.kind(), CredentialKind::ApiKey);
        assert_eq!(resolved.expose_secret(), "sk-env-fallback-abcdef");
        // env 值不落任何 Debug 泄漏字段。
        assert!(!format!("{resolved:?}").contains("sk-env-fallback-abcdef"));
        assert!(!source_debug.contains("sk-env-fallback-abcdef"));
    }

    #[test]
    fn both_missing_returns_none() {
        let backend = MemoryBackend::new();
        let provider = "resolve-both-missing";
        let env_name = api_key_env_name(provider);
        remove_env(&env_name);
        assert!(matches!(
            resolve_provider_credential(&backend, provider).expect("resolve"),
            CredentialSource::None
        ));
    }

    #[test]
    fn empty_env_value_counts_as_missing() {
        let backend = MemoryBackend::new();
        let provider = "resolve-env-empty";
        let env_name = api_key_env_name(provider);
        set_env(&env_name, "");
        assert!(matches!(
            resolve_provider_credential(&backend, provider).expect("resolve"),
            CredentialSource::None
        ));
        remove_env(&env_name);
    }

    #[test]
    fn corrupt_auth_file_never_falls_back_to_env() {
        let provider = format!("resolve-corrupt-{}", std::process::id());
        let env_name = api_key_env_name(&provider);
        set_env(&env_name, "sk-env-must-not-mask-corruption");
        let path = std::env::temp_dir().join(format!(
            "pawork-auth-corrupt-{}-{}.json",
            std::process::id(),
            crate::credential::generate_credential_id().as_str()
        ));
        std::fs::write(&path, b"{not-valid-json").expect("write corrupt auth file");
        let backend = crate::FileBackend::with_path(&path);

        let error = resolve_provider_credential(&backend, &provider)
            .expect_err("corrupt auth file must fail closed before env fallback");
        remove_env(&env_name);
        std::fs::remove_file(&path).expect("remove corrupt auth fixture");

        assert!(matches!(error, AuthError::Storage(_)));
        assert!(!error
            .to_string()
            .contains("sk-env-must-not-mask-corruption"));
    }
}
