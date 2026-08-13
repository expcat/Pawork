//! signed thinking continuity（P18-12 §4 · ADR-032 · P15-7）。
//!
//! Claude 的 extended-thinking 续传凭证以两类块出现：
//!
//! - `{"type":"thinking","signature":...}` —— `signature` 是续传凭证；
//! - `{"type":"redacted_thinking","data":...}` —— 服务端遮蔽后的续传凭证。
//!
//! 本模块只做三件无副作用的事：
//!
//! 1. 抽取受保护材料（[`SignedThinkingMaterial`]；`Debug` 脱敏）；
//! 2. 经注入的 [`SignedThinkingProtector`] 得到 Protected Blob 引用；
//! 3. 产出只含安全引用的 canonical [`ReasoningItem`]。
//!
//! 红线（ADR-032）：明文 `signature` / `data` 不进 canonical 事件、不进
//! `Debug` / 日志、不进普通存储；缺失 / 形状不符显式失败，绝不猜值。

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use agent_domain::{ProtectedBlobRef, ReasoningItem, ReasoningItemId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ClaudeGatewayError;
use crate::wire::{SignedThinkingBlock, ThinkingBlockKind};

/// [`ReasoningItem::continuation_metadata`] 中记录 Anthropic block kind 的键。
///
/// 与 provider-anthropic 的线协议约定一致（P15-7 对齐表）；值仅为结构性翻译
/// 提示（`"thinking"` / `"redacted_thinking"`），不含任何凭证材料。
pub const ANTHROPIC_BLOCK_KIND_KEY: &str = "anthropic_block_kind";

/// 从 Anthropic thinking 块抽取的受保护载荷。
///
/// 这是写入 Protected Blob Store、加密前的明文结构。`Serialize` 仅用于
/// 受控载荷序列化（`to_protected_bytes`）；`Debug` 脱敏，不实现 `Display`。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "anthropic_block_kind", content = "protected")]
pub enum SignedThinkingMaterial {
    /// `thinking` 块的受保护部分：仅 `signature`。
    /// 明文推理文本经 `ThinkingDelta` 独立流转，不属于受保护载荷。
    #[serde(rename = "thinking")]
    Thinking { signature: String },
    /// `redacted_thinking` 块的受保护部分：服务端遮蔽后的 `data`。
    #[serde(rename = "redacted_thinking")]
    Redacted { data: String },
}

impl SignedThinkingMaterial {
    /// 从线协议块抽取受保护材料；缺字段 / 形状不符显式失败。
    pub fn from_block(block: &SignedThinkingBlock) -> Result<Self, ClaudeGatewayError> {
        match block.kind {
            ThinkingBlockKind::Thinking => Ok(SignedThinkingMaterial::Thinking {
                signature: block.material().to_string(),
            }),
            ThinkingBlockKind::RedactedThinking => Ok(SignedThinkingMaterial::Redacted {
                data: block.material().to_string(),
            }),
        }
    }

    /// 对应的 Anthropic block `type` 字符串。
    pub fn kind(&self) -> &'static str {
        match self {
            SignedThinkingMaterial::Thinking { .. } => "thinking",
            SignedThinkingMaterial::Redacted { .. } => "redacted_thinking",
        }
    }

    /// 序列化为待加密载荷（仅供 protector 消费）。
    pub fn to_protected_bytes(&self) -> Result<Vec<u8>, ClaudeGatewayError> {
        serde_json::to_vec(self).map_err(|_| {
            ClaudeGatewayError::MalformedSignedThinking("protected payload serialization failed")
        })
    }
}

impl fmt::Debug for SignedThinkingMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, field) = match self {
            SignedThinkingMaterial::Thinking { .. } => ("thinking", "signature"),
            SignedThinkingMaterial::Redacted { .. } => ("redacted_thinking", "data"),
        };
        formatter
            .debug_struct("SignedThinkingMaterial")
            .field("kind", &kind)
            .field(field, &"[REDACTED]")
            .finish()
    }
}

/// signed thinking 材料保护边界（adapter 本地 seam）。
///
/// 宿主以 `provider-runtime::reasoning::ReasoningProtector`（P15-10 统一抽象）
/// 桥接实现本 trait；adapter 不依赖 Provider runtime / 存储实现。生产必须
/// 落到 `ProtectedBlobStoreProtector`（ADR-032 encrypted-at-rest），进程内
/// 实现仅限测试与组合层开发。
#[async_trait]
pub trait SignedThinkingProtector: Send + Sync {
    /// 加密保护 opaque payload，返回稳定逻辑引用。
    async fn protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ClaudeGatewayError>;

    /// 解析稳定逻辑引用指向的明文（不解释内容）。
    async fn resolve(&self, blob_ref: &ProtectedBlobRef) -> Result<Vec<u8>, ClaudeGatewayError>;
}

/// 内存保护器：进程内「引用 → 密文前载荷」映射（测试与组合层开发用；
/// 重启即丢，生产必须桥接 `ProtectedBlobStoreProtector`，见 ADR-032）。
#[derive(Default)]
pub struct InMemorySignedThinkingProtector {
    blobs: Mutex<std::collections::HashMap<ProtectedBlobRef, Vec<u8>>>,
    next_ref: AtomicU64,
}

impl InMemorySignedThinkingProtector {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SignedThinkingProtector for InMemorySignedThinkingProtector {
    async fn protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ClaudeGatewayError> {
        let blob_ref = ProtectedBlobRef::from(format!(
            "claude-signed-{}",
            self.next_ref.fetch_add(1, Ordering::Relaxed)
        ));
        self.blobs
            .lock()
            .expect("in-memory signed thinking protector poisoned")
            .insert(blob_ref.clone(), payload.to_vec());
        Ok(blob_ref)
    }

    async fn resolve(&self, blob_ref: &ProtectedBlobRef) -> Result<Vec<u8>, ClaudeGatewayError> {
        self.blobs
            .lock()
            .expect("in-memory signed thinking protector poisoned")
            .get(blob_ref)
            .cloned()
            .ok_or_else(|| {
                ClaudeGatewayError::SignedThinkingProtectorUnavailable("blob ref unknown")
            })
    }
}

/// 给定调用方合成的 [`ReasoningItemId`] 与已存 [`ProtectedBlobRef`]，产出只含
/// 安全引用 + 非敏感 kind 提示的 [`ReasoningItem`]。不读取、不记录明文。
pub fn build_reasoning_item(
    id: ReasoningItemId,
    blob_ref: ProtectedBlobRef,
    material: &SignedThinkingMaterial,
) -> ReasoningItem {
    let mut continuation_metadata = BTreeMap::new();
    continuation_metadata.insert(
        ANTHROPIC_BLOCK_KIND_KEY.to_string(),
        Value::String(material.kind().to_string()),
    );
    ReasoningItem {
        id,
        summary: None,
        protected_blob_ref: blob_ref,
        opaque_metadata: BTreeMap::new(),
        continuation_metadata,
    }
}

/// 保护一个 signed thinking 块：抽取材料 → protector 加密 → 安全引用。
///
/// 失败（保护器不可用 / 序列化失败）显式返回错误；明文不落普通存储与日志。
pub async fn protect_signed_thinking(
    block: &SignedThinkingBlock,
    protector: &dyn SignedThinkingProtector,
    id: ReasoningItemId,
) -> Result<ReasoningItem, ClaudeGatewayError> {
    let material = SignedThinkingMaterial::from_block(block)?;
    let payload = material.to_protected_bytes()?;
    let blob_ref = protector.protect(&payload).await?;
    Ok(build_reasoning_item(id, blob_ref, &material))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_secret_absent(haystack: &str) {
        for forbidden in ["SIG-SECRET-123", "DATA-SECRET-456"] {
            assert!(
                !haystack.contains(forbidden),
                "protected material leaked into: {haystack}"
            );
        }
    }

    #[tokio::test]
    async fn protect_and_resolve_round_trip_through_reference_only() {
        let protector = InMemorySignedThinkingProtector::new();
        let block = SignedThinkingBlock::thinking("SIG-SECRET-123".into());
        let item = protect_signed_thinking(&block, &protector, ReasoningItemId::from("r-1"))
            .await
            .expect("protect");

        // canonical 项只含安全引用 + kind 提示。
        assert_eq!(item.id.as_str(), "r-1");
        assert_eq!(
            item.continuation_metadata[ANTHROPIC_BLOCK_KIND_KEY],
            Value::String("thinking".into())
        );
        let encoded = serde_json::to_string(&item).expect("serialize item");
        assert!(encoded.contains("claude-signed-0"));
        assert_secret_absent(&encoded);
        assert_secret_absent(&format!("{item:?}"));

        // 引用可解析回载荷（载荷本身是受控序列化形式，宿主按需重建线协议块）。
        let payload = protector
            .resolve(&item.protected_blob_ref)
            .await
            .expect("resolve");
        let decoded: SignedThinkingMaterial =
            serde_json::from_slice(&payload).expect("decode payload");
        assert_eq!(decoded.kind(), "thinking");
        match decoded {
            SignedThinkingMaterial::Thinking { signature } => {
                assert_eq!(signature, "SIG-SECRET-123")
            }
            SignedThinkingMaterial::Redacted { .. } => panic!("expected Thinking"),
        }
    }

    #[tokio::test]
    async fn redacted_block_protects_data_without_text() {
        let protector = InMemorySignedThinkingProtector::new();
        let block = SignedThinkingBlock::redacted("DATA-SECRET-456".into());
        let item = protect_signed_thinking(&block, &protector, ReasoningItemId::from("r-2"))
            .await
            .expect("protect");
        assert_eq!(
            item.continuation_metadata[ANTHROPIC_BLOCK_KIND_KEY],
            Value::String("redacted_thinking".into())
        );
        assert_secret_absent(&serde_json::to_string(&item).expect("serialize"));
        assert_secret_absent(&format!("{item:?}"));
    }

    #[test]
    fn material_debug_redacts_plaintext() {
        let thinking = SignedThinkingMaterial::Thinking {
            signature: "SIG-SECRET-123".into(),
        };
        let redacted = SignedThinkingMaterial::Redacted {
            data: "DATA-SECRET-456".into(),
        };
        for debug in [format!("{thinking:?}"), format!("{redacted:?}")] {
            assert!(debug.contains("[REDACTED]"));
            assert_secret_absent(&debug);
        }
    }

    #[tokio::test]
    async fn unknown_reference_resolve_fails_closed() {
        let protector = InMemorySignedThinkingProtector::new();
        let error = protector
            .resolve(&ProtectedBlobRef::from("no-such-ref"))
            .await
            .expect_err("unknown ref must fail");
        assert!(matches!(
            error,
            ClaudeGatewayError::SignedThinkingProtectorUnavailable(_)
        ));
    }
}
