//! 大 payload 归一为 artifact 引用（ADR-018，P17-10）。
//!
//! 截图字节 / 大 DOM 文本不应进入上下文。本模块把它们写入 `artifact-store`，
//! 在 snapshot 中保留 `ArtifactReference` 与短摘要。
use agent_domain::{ArtifactId, ArtifactReference};
use artifact_store::{ArtifactStore, BlobId};

use crate::action::BrowserComputerSnapshot;
use crate::error::BrowserComputerError;

/// 默认阈值：超过该字节数的内联载荷折叠为 artifact 引用。
pub const DEFAULT_LARGE_PAYLOAD_BYTES: u64 = 16 * 1024;

/// 截图 artifact 的 media type。
pub const SCREENSHOT_MEDIA_TYPE: &str = "image/png";
/// DOM 文本 artifact 的 media type。
pub const DOM_MEDIA_TYPE: &str = "text/html";

/// 由 BlobId 构造 `ArtifactReference`。
pub fn artifact_reference(
    blob: &BlobId,
    media_type: &str,
    byte_len: u64,
    label: Option<String>,
) -> ArtifactReference {
    ArtifactReference {
        id: ArtifactId::new(blob.as_str()),
        media_type: media_type.to_string(),
        byte_length: byte_len,
        content_hash: Some(blob.as_str().to_string()),
        label,
    }
}

/// 把一段 payload 写入 artifact-store 并返回引用。
pub async fn store_payload(
    store: &ArtifactStore,
    payload: &[u8],
    media_type: &str,
    label: Option<String>,
) -> Result<ArtifactReference, BrowserComputerError> {
    let byte_len = payload.len() as u64;
    let outcome = store
        .put(payload)
        .await
        .map_err(|err| BrowserComputerError::Artifact(err.to_string()))?;
    Ok(artifact_reference(&outcome.id, media_type, byte_len, label))
}

/// 把 snapshot 中超阈值的 DOM 文本折叠为 artifact 引用。
///
/// - `Some(store)`：写入 artifact-store，`dom` 置空，`artifacts` 追加引用；写入失败
///   时安全移除全量 DOM 并标记 `artifact_error` / `truncated`，绝不向 facade 调用方
///   返回大 DOM。
/// - `None`：安全移除全量 DOM，并在 `metadata` 标记 `truncated`。
pub async fn normalize_snapshot(
    mut snapshot: BrowserComputerSnapshot,
    store: Option<&ArtifactStore>,
    threshold: u64,
) -> BrowserComputerSnapshot {
    let Some(dom) = snapshot.dom.clone() else {
        return snapshot;
    };
    if (dom.len() as u64) <= threshold {
        return snapshot;
    }
    let folded_note = format!("dom folded to artifact ({} bytes)", dom.len());
    match store {
        Some(store) => {
            match store_payload(store, dom.as_bytes(), DOM_MEDIA_TYPE, Some("dom".into())).await {
                Ok(reference) => {
                    snapshot.artifacts.push(reference);
                    snapshot.dom = None;
                    if snapshot.summary.is_empty() {
                        snapshot.summary = folded_note;
                    }
                }
                Err(err) => {
                    // artifact-store 已配置但写入失败：安全截断，不能把全量 DOM
                    // 交还给 facade 直调方。
                    snapshot.dom = None;
                    mark_metadata(&mut snapshot.metadata, "artifact_error", err.to_string());
                    mark_metadata(&mut snapshot.metadata, "truncated", true.to_string());
                    mark_metadata(
                        &mut snapshot.metadata,
                        "original_dom_bytes",
                        (dom.len() as u64).to_string(),
                    );
                    if snapshot.summary.is_empty() {
                        snapshot.summary = format!(
                            "dom omitted after artifact storage failure ({} bytes)",
                            dom.len()
                        );
                    }
                }
            }
        }
        None => {
            // 没有 artifact-store 时也不能泄漏超过阈值的全量 DOM。
            snapshot.dom = None;
            mark_metadata(&mut snapshot.metadata, "truncated", true.to_string());
            mark_metadata(
                &mut snapshot.metadata,
                "original_dom_bytes",
                (dom.len() as u64).to_string(),
            );
            if snapshot.summary.is_empty() {
                snapshot.summary = format!("dom omitted ({} bytes)", dom.len());
            }
        }
    }
    snapshot
}

fn mark_metadata(metadata: &mut serde_json::Value, key: &str, value: String) {
    if !metadata.is_object() {
        *metadata = serde_json::json!({});
    }
    if let Some(obj) = metadata.as_object_mut() {
        obj.insert(key.to_string(), serde_json::Value::String(value));
    }
}
