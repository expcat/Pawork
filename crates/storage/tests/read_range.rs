//! P13-8：范围/流式读取集成测试。
//!
//! 覆盖 `read_range` 的中点/跨边界/越界/空范围/完整性校验，以及分片循环
//! 拼回全量与 `put` 结果一致；另验证 `BlobId` 的 serde 字符串格式兼容
//! checkpoint 的 hex 字符串形状。

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use pawork_storage::blob::{ArtifactStore, ArtifactStoreError, BlobId};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

fn temp_root(name: &str) -> PathBuf {
    let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "pawork-blob-store-{}-{unique}-{name}",
        std::process::id()
    ))
}

fn cleanup(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
}

/// 确定性内容：模拟多行 diff 文本，长度 `total`（不要求是 chunk 的整数倍）。
fn diff_like_content(total: usize) -> Vec<u8> {
    let mut content = Vec::with_capacity(total);
    let mut line = 0u64;
    while content.len() < total {
        let text = format!("diff line {line}: payload {}\n", "x".repeat(57));
        let remaining = total - content.len();
        content.extend_from_slice(&text.as_bytes()[..text.len().min(remaining)]);
        line += 1;
    }
    content
}

async fn open_store(root: &Path) -> ArtifactStore {
    ArtifactStore::open(root).await.expect("open store")
}

#[tokio::test]
async fn read_range_mid_and_boundaries() {
    let root = temp_root("range-boundaries");
    let store = open_store(&root).await;
    let content = diff_like_content(100_000);
    let id = store.put(&content).await.expect("put").id;
    let size = content.len() as u64;

    // 起点：offset 0。
    assert_eq!(
        store.read_range(&id, 0, 32).await.expect("head"),
        &content[..32]
    );
    // 中点：offset 4096，limit 64。
    assert_eq!(
        store.read_range(&id, 4096, 64).await.expect("mid"),
        &content[4096..4160]
    );
    // 跨 chunk 边界：offset 65533，limit 8（跨越 65536 边界）。
    assert_eq!(
        store.read_range(&id, 65_533, 8).await.expect("cross chunk"),
        &content[65_533..65_541]
    );
    // limit 超过剩余：截断到尾部。
    assert_eq!(
        store.read_range(&id, size - 5, 1024).await.expect("tail"),
        &content[content.len() - 5..]
    );
    // 全量范围等于 get 的结果。
    assert_eq!(
        store.read_range(&id, 0, size).await.expect("full"),
        store.get(&id).await.expect("get")
    );
    // offset == size 且 limit > 0：空切片（分片循环的自然终止）。
    assert_eq!(
        store.read_range(&id, size, 64).await.expect("exact tail"),
        Vec::<u8>::new()
    );
    // byte_length 与 put 的 size 一致。
    assert_eq!(store.byte_length(&id).await.expect("byte length"), size);

    store.shutdown().await.expect("shutdown");
    cleanup(&root);
}

#[tokio::test]
async fn read_range_structured_out_of_bounds_errors() {
    let root = temp_root("range-errors");
    let store = open_store(&root).await;
    let content = diff_like_content(1_000);
    let id = store.put(&content).await.expect("put").id;
    let size = content.len() as u64;

    // 空范围：limit == 0，与 offset 无关。
    let error = store.read_range(&id, 0, 0).await.expect_err("empty range");
    let empty = match error {
        ArtifactStoreError::EmptyRange {
            id: error_id,
            offset,
            limit,
        } => (error_id, offset, limit),
        other => panic!("unexpected error: {other:?}"),
    };
    assert_eq!(empty, (id.clone(), 0, 0));
    let error = store
        .read_range(&id, size, 0)
        .await
        .expect_err("empty range at tail");
    assert!(matches!(error, ArtifactStoreError::EmptyRange { .. }));

    // offset 超尾：offset > size。
    let error = store
        .read_range(&id, size + 1, 64)
        .await
        .expect_err("offset beyond tail");
    let out_of_bounds = match error {
        ArtifactStoreError::RangeOffsetOutOfBounds {
            id: error_id,
            offset,
            size: error_size,
        } => (error_id, offset, error_size),
        other => panic!("unexpected error: {other:?}"),
    };
    assert_eq!(out_of_bounds, (id.clone(), size + 1, size));

    // 不存在：即使 limit == 0 也优先报 UnknownBlob。
    let unknown = BlobId::from_hash(blake3::hash(b"never stored"));
    let error = store
        .read_range(&unknown, 0, 64)
        .await
        .expect_err("unknown blob");
    assert!(matches!(error, ArtifactStoreError::UnknownBlob { .. }));
    let error = store
        .read_range(&unknown, 0, 0)
        .await
        .expect_err("unknown blob, empty range");
    assert!(matches!(error, ArtifactStoreError::UnknownBlob { .. }));
    let error = store
        .byte_length(&unknown)
        .await
        .expect_err("byte_length unknown");
    assert!(matches!(error, ArtifactStoreError::UnknownBlob { .. }));

    store.shutdown().await.expect("shutdown");
    cleanup(&root);
}

#[tokio::test]
async fn read_range_detects_corruption_like_get() {
    let root = temp_root("range-corruption");
    let store = open_store(&root).await;
    let id = store.put(b"pristine range content").await.expect("put").id;
    std::fs::write(store.blob_path(&id), b"tampered").expect("tamper blob");

    let error = store
        .read_range(&id, 1, 4)
        .await
        .expect_err("corrupted range read must fail");
    assert!(matches!(error, ArtifactStoreError::BlobCorrupted { .. }));
    store.shutdown().await.expect("shutdown");
    cleanup(&root);
}

#[tokio::test]
async fn sharded_reads_reassemble_full_content() {
    let root = temp_root("range-shards");
    let store = open_store(&root).await;
    let content = diff_like_content(300_000);
    let id = store.put(&content).await.expect("put").id;

    // 64KiB chunk 分片循环：直到返回空切片终止。
    const CHUNK: u64 = 64 * 1024;
    let mut assembled = Vec::new();
    let mut offset = 0u64;
    loop {
        let chunk = store
            .read_range(&id, offset, CHUNK)
            .await
            .expect("chunk read");
        if chunk.is_empty() {
            break;
        }
        assembled.extend_from_slice(&chunk);
        offset += chunk.len() as u64;
    }
    assert_eq!(assembled, content);
    assert_eq!(offset, content.len() as u64);
    // 分片结果与 get 全量一致。
    assert_eq!(assembled, store.get(&id).await.expect("get"));

    // 非对齐 chunk 同样完整拼回：65535 跨 64KiB 边界，3 字节覆盖极小切片。
    for (total, chunk_size) in [(100_000usize, 65_535u64), (12_345usize, 3u64)] {
        let content = diff_like_content(total);
        let id = store.put(&content).await.expect("put").id;
        let mut assembled = Vec::new();
        let mut offset = 0u64;
        loop {
            let chunk = store
                .read_range(&id, offset, chunk_size)
                .await
                .expect("chunk read");
            if chunk.is_empty() {
                break;
            }
            assert!(chunk.len() as u64 <= chunk_size);
            assembled.extend_from_slice(&chunk);
            offset += chunk.len() as u64;
        }
        assert_eq!(assembled, content);
    }
    store.shutdown().await.expect("shutdown");
    cleanup(&root);
}

#[tokio::test]
async fn blob_id_serde_preserves_string_format() {
    use std::str::FromStr;

    let id = BlobId::from_str(&"f".repeat(64)).expect("valid hex");

    // 序列化为裸 JSON 字符串，值就是 hex（与 checkpoint FileSnapshot 形状一致）。
    let json = serde_json::to_string(&id).expect("serialize");
    assert_eq!(json, format!("\"{}\"", id.as_str()));
    // 反序列化还原。
    let decoded: BlobId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, id);
    // 非法 hex 拒绝。
    let error = serde_json::from_str::<BlobId>("\"not-a-hex-blob-id\"").expect_err("invalid hex");
    assert!(error.to_string().contains("invalid blob id"));
}
