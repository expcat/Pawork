//! 大体积结果的 artifact 引用归一化（ADR-018）。
//!
//! 大体积 diagnostics / 符号表 / 大范围 WorkspaceEdit 经注入的 [`ArtifactSink`]
//! 落到 artifact-store，结果归一为 [`crate::protocol::ArtifactRef`]；lsp-runtime
//! 自身不直接依赖 artifact-store crate（避免引入 SQLite / 文件 IO），由调用方注入。

use async_trait::async_trait;

use crate::error::LspError;
use crate::protocol::ArtifactRef;

/// artifact 写入器契约。
#[async_trait]
pub trait ArtifactSink: Send + Sync {
    async fn store(&self, kind: &str, bytes: Vec<u8>) -> Result<ArtifactRef, LspError>;
}

/// 默认阈值：单次结果序列化后超过该字节数即转 artifact 引用。
pub const ARTIFACT_INLINE_THRESHOLD: usize = 64 * 1024;

/// 序列化结果，按阈值决定内联还是 artifact 引用。
pub async fn maybe_offload<T>(
    sink: Option<&(dyn ArtifactSink + Send + Sync)>,
    kind: &str,
    value: &T,
) -> Result<crate::protocol::ResultPayload<Vec<u8>>, LspError>
where
    T: serde::Serialize,
{
    let bytes = serde_json::to_vec(value).map_err(LspError::Json)?;
    if bytes.len() <= ARTIFACT_INLINE_THRESHOLD {
        return Ok(crate::protocol::ResultPayload::Inline(bytes));
    }
    let sink = sink.ok_or_else(|| {
        LspError::Transport(
            "large LSP result requires an artifact sink; refusing oversized inline payload".into(),
        )
    })?;
    let reference = sink.store(kind, bytes).await?;
    Ok(crate::protocol::ResultPayload::Artifact(reference))
}

/// 内存 artifact sink：仅用于测试。id = blake3-free 的简单计数器串。
#[derive(Debug, Default)]
pub struct InMemorySink {
    counter: std::sync::atomic::AtomicU64,
}

#[async_trait]
impl ArtifactSink for InMemorySink {
    async fn store(&self, kind: &str, bytes: Vec<u8>) -> Result<ArtifactRef, LspError> {
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(ArtifactRef {
            store: "memory",
            kind: kind.to_string(),
            id: format!("{kind}-{n}"),
            size: bytes.len() as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn small_result_stays_inline() {
        let value = serde_json::json!({ "small": true });
        let payload = maybe_offload::<serde_json::Value>(None, "test", &value)
            .await
            .unwrap();
        assert!(matches!(payload, crate::protocol::ResultPayload::Inline(_)));
    }

    #[tokio::test]
    async fn large_result_without_sink_fails_closed() {
        let big = "x".repeat(ARTIFACT_INLINE_THRESHOLD + 1);
        let error = maybe_offload::<String>(None, "test", &big)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("requires an artifact sink"));
    }

    #[tokio::test]
    async fn large_result_with_sink_offloads_to_artifact() {
        let sink = InMemorySink::default();
        let big = "y".repeat(ARTIFACT_INLINE_THRESHOLD + 1);
        let payload = maybe_offload(Some(&sink), "workspace/symbol", &big)
            .await
            .unwrap();
        match payload {
            crate::protocol::ResultPayload::Artifact(reference) => {
                assert_eq!(reference.kind, "workspace/symbol");
                assert_eq!(
                    reference.size as usize,
                    serde_json::to_vec(&big).unwrap().len()
                );
            }
            crate::protocol::ResultPayload::Inline(_) => panic!("expected artifact offload"),
        }
    }
}
