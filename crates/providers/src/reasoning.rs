//! Provider-neutral 的 reasoning continuation 保护 trait（S5 波 A）。
//!
//! 迁自 V1 `provider-runtime::reasoning`。本包提供 trait 与内存实现；R5 波 C
//! 已由宿主 `pawork-app::protected` 接到 `pawork-storage::blob`。本 crate 不
//! 依赖 storage，只使用 `pawork-domain` 的 `ProtectedBlobRef` 逻辑引用。

use pawork_domain::ProtectedBlobRef;
use thiserror::Error;

/// 统一的 reasoning 保护错误，屏蔽底层存储的失败形状。
#[derive(Debug, Error)]
pub enum ReasoningProtectError {
    /// 引用不存在、跨 scope 访问、密钥不可用等一律失败关闭。
    #[error("reasoning continuation unavailable")]
    Unavailable,
    /// 密文摘要、信封或 AEAD 认证失败。
    #[error("reasoning continuation corrupted")]
    Corrupted,
}

impl ReasoningProtectError {
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable)
    }

    pub fn is_corrupted(&self) -> bool {
        matches!(self, Self::Corrupted)
    }
}

/// 受保护 reasoning continuation 的统一存取边界。
///
/// Provider crates 只负责解析与重组各自的 wire 格式；加密 opaque
/// continuation payload 与稳定逻辑引用的互转统一走本 trait，不按 Provider
/// 名分支、不解释明文。payload 的加密与持久化由实现负责（宿主组装层）。
#[async_trait::async_trait]
pub trait ReasoningProtector: Send + Sync {
    /// 保护 opaque payload，返回稳定逻辑引用。
    async fn protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ReasoningProtectError>;

    /// 解析稳定逻辑引用指向的 payload（实现负责解密），不解释其内容。
    async fn resolve(
        &self,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<Vec<u8>, ReasoningProtectError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::RwLock;

    /// 测试内最小实现：只验证 trait 可用性（dyn 兼容 + 往返 + fail-closed），
    /// 不作为产品 API 提供。
    struct MapProtector(RwLock<HashMap<ProtectedBlobRef, Vec<u8>>>);

    #[async_trait::async_trait]
    impl ReasoningProtector for MapProtector {
        async fn protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ReasoningProtectError> {
            let blob_ref = ProtectedBlobRef::new("test-ref-1");
            self.0
                .write()
                .expect("test protector lock")
                .insert(blob_ref.clone(), payload.to_vec());
            Ok(blob_ref)
        }

        async fn resolve(
            &self,
            blob_ref: &ProtectedBlobRef,
        ) -> Result<Vec<u8>, ReasoningProtectError> {
            self.0
                .read()
                .expect("test protector lock")
                .get(blob_ref)
                .cloned()
                .ok_or(ReasoningProtectError::Unavailable)
        }
    }

    #[tokio::test]
    async fn trait_round_trips_and_misses_fail_closed() {
        let protector = MapProtector(RwLock::new(HashMap::new()));
        let payload = b"opaque-continuation-bytes".to_vec();

        let blob_ref = protector.protect(&payload).await.expect("protect");
        assert_eq!(
            protector.resolve(&blob_ref).await.expect("resolve"),
            payload
        );

        let missing = ProtectedBlobRef::new("missing");
        let error = protector
            .resolve(&missing)
            .await
            .expect_err("unknown ref fails closed");
        assert!(error.is_unavailable());
        assert!(!error.is_corrupted());
    }
}
