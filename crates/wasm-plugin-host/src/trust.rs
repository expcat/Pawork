//! Ed25519 trust store 与插件签名验证（P10-1）。
//!
//! 信任模型：宿主持有一组 opaque `key_id -> Ed25519 VerifyingKey` 的 trust store。
//! 插件 manifest 的 `PluginSignature::key_id` 必须命中 trust store；签名覆盖
//! `PluginManifest::canonical_signing_payload(component_bytes)`，即 manifest 的
//! 规范化 JSON + 组件字节的 blake3 摘要。篡改 manifest 字段或替换组件字节都会
//! 导致验签失败。
//!
//! 本模块只做密钥管理与验签；manifest 自身校验（`validate`）与 API 版本兼容
//! 由 `host::WasmPluginHost::load` 调用，确保 trust store 保持单一职责。

use std::collections::BTreeMap;

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use plugin_api::{
    PluginError, PluginErrorKind, PluginManifest, PluginSignature, PluginSignatureAlgorithm,
};

/// trust store / 验签相关错误。这些错误在内部使用，对外统一映射为
/// [`PluginErrorKind::SignatureRejected`]（或 InvalidManifest）。
#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    #[error("unknown signing key id: {0}")]
    UnknownKey(String),
    #[error("unsupported signature algorithm")]
    UnsupportedAlgorithm,
    #[error("invalid signature encoding: {0}")]
    InvalidEncoding(String),
    #[error("invalid signed manifest: {0}")]
    InvalidManifest(String),
    #[error("signature rejected by verifying key")]
    InvalidSignature,
}

/// Ed25519 公钥 trust store：`key_id -> VerifyingKey`。
///
/// `key_id` 是宿主自管理的 opaque 标识，不暴露公钥本身，避免与 manifest 中
/// 嵌入的公钥混淆（防「自带公钥」绕过）。
#[derive(Clone, Debug, Default)]
pub struct TrustStore {
    keys: BTreeMap<String, VerifyingKey>,
}

impl TrustStore {
    pub fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
        }
    }

    /// 直接安装一个已构造的 `VerifyingKey`（测试或外部密钥管理场景）。
    pub fn install_verifying_key(&mut self, key_id: impl Into<String>, verifying: VerifyingKey) {
        self.keys.insert(key_id.into(), verifying);
    }

    /// 校验签名。`canonical_signing_payload` 由 `PluginManifest` 计算，已把
    /// manifest 规范化 JSON 与组件字节摘要绑定；本方法只负责 Ed25519 验签。
    pub fn verify_signature(
        &self,
        signature: &PluginSignature,
        manifest: &PluginManifest,
        component_bytes: &[u8],
    ) -> Result<(), SignatureError> {
        if !matches!(signature.algorithm, PluginSignatureAlgorithm::Ed25519) {
            return Err(SignatureError::UnsupportedAlgorithm);
        }
        let verifying = self
            .keys
            .get(&signature.key_id)
            .ok_or_else(|| SignatureError::UnknownKey(signature.key_id.clone()))?;

        let payload = manifest
            .canonical_signing_payload(component_bytes)
            .map_err(|error| SignatureError::InvalidManifest(error.to_string()))?;

        let raw = base64::engine::general_purpose::STANDARD
            .decode(&signature.signature)
            .map_err(|error| SignatureError::InvalidEncoding(error.to_string()))?;
        let bytes: [u8; 64] = raw.as_slice().try_into().map_err(|_| {
            SignatureError::InvalidEncoding("ed25519 signature must be 64 bytes".into())
        })?;
        let sig = Signature::from_bytes(&bytes);

        verifying
            .verify_strict(&payload, &sig)
            .map_err(|_| SignatureError::InvalidSignature)
    }
}

/// 将 [`SignatureError`] 映射为面向调用方的 [`PluginError`]，全部归类为
/// 签名拒绝；只有 canonical manifest 构造失败归为 InvalidManifest。
pub(crate) fn signature_error_to_plugin(error: SignatureError) -> PluginError {
    match error {
        SignatureError::InvalidManifest(message) => {
            PluginError::new(PluginErrorKind::InvalidManifest, message)
        }
        other => PluginError::new(PluginErrorKind::SignatureRejected, other.to_string()),
    }
}
