//! P13-8 AppService::artifact_read 流式读取测试。
//!
//! 覆盖：put blob + aggregate 登记 → 真实 payload 分片读（offset 连续、末片 eof）；
//! 无 aggregate 记录 → NotFound；未配置 store → Unavailable；offset 超尾 → 空 data
//! + eof；`limit == 0` 读到文件尾；非 64-hex ID → NotFound；Blob 损坏 → Internal。

use std::sync::Arc;

use agent_domain::{ArtifactId, ErrorCategory};
use app_service::{AppService, AppServiceError};
use artifact_store::ArtifactStore;
use tempfile::TempDir;

struct Fixture {
    service: Arc<AppService>,
    store: Arc<ArtifactStore>,
    _temp: TempDir,
}

async fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        ArtifactStore::open(temp.path().join("store"))
            .await
            .expect("open store"),
    );
    let service = Arc::new(AppService::with_artifact_store(
        "artifact-read-test",
        Arc::clone(&store),
    ));
    Fixture {
        service,
        store,
        _temp: temp,
    }
}

fn register(service: &AppService, artifact_id: &ArtifactId, byte_length: u64) {
    service
        .router()
        .aggregate()
        .put_artifact(artifact_id.clone(), byte_length, "text/plain".into())
        .expect("register artifact");
}

fn pattern_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

#[tokio::test]
async fn chunked_reads_reassemble_payload_with_continuous_offsets() {
    let fixture = fixture().await;
    let content = pattern_bytes(300 * 1024);
    let outcome = fixture.store.put(&content).await.expect("put blob");
    let artifact_id = ArtifactId::from(outcome.id.as_str());
    register(&fixture.service, &artifact_id, content.len() as u64);

    let mut assembled = Vec::new();
    let mut offset = 0u64;
    let mut chunks = 0u64;
    loop {
        let result = fixture
            .service
            .artifact_read(&artifact_id, offset, 64 * 1024)
            .await
            .expect("read chunk");
        assert_eq!(result.byte_length, content.len() as u64);
        assert_eq!(offset, assembled.len() as u64, "offset 必须与已读字节连续");
        assert!(result.data.len() <= 64 * 1024, "单片不得超 64KiB");
        if !result.eof {
            assert!(!result.data.is_empty(), "非末片必须返回数据");
        }
        assembled.extend_from_slice(&result.data);
        chunks += 1;
        offset += result.data.len() as u64;
        if result.eof {
            break;
        }
    }
    assert!(chunks >= 5, "300KiB 应按 64KiB 切成多片，实际 {chunks}");
    assert_eq!(assembled, content, "分片重组必须等于原始 payload");
}

#[tokio::test]
async fn limit_zero_reads_to_end_of_file() {
    let fixture = fixture().await;
    let content = pattern_bytes(150 * 1024 + 17);
    let outcome = fixture.store.put(&content).await.expect("put blob");
    let artifact_id = ArtifactId::from(outcome.id.as_str());
    register(&fixture.service, &artifact_id, content.len() as u64);

    let result = fixture
        .service
        .artifact_read(&artifact_id, 0, 0)
        .await
        .expect("limit=0 读到文件尾");
    assert_eq!(result.data, content);
    assert!(result.eof);
    assert_eq!(result.byte_length, content.len() as u64);
}

#[tokio::test]
async fn partial_limit_respects_range_and_flags_eof_at_tail() {
    let fixture = fixture().await;
    let content = pattern_bytes(100 * 1024 + 5);
    let outcome = fixture.store.put(&content).await.expect("put blob");
    let artifact_id = ArtifactId::from(outcome.id.as_str());
    register(&fixture.service, &artifact_id, content.len() as u64);

    // 中段读取：返回完整 limit 且 eof=false。
    let mid = fixture
        .service
        .artifact_read(&artifact_id, 1024, 4096)
        .await
        .expect("read middle");
    assert_eq!(mid.data, content[1024..1024 + 4096]);
    assert!(!mid.eof);

    // 尾部读取：读到文件尾截断，eof=true。
    let tail = fixture
        .service
        .artifact_read(&artifact_id, content.len() as u64 - 100, 4096)
        .await
        .expect("read tail");
    assert_eq!(tail.data.len(), 100);
    assert!(tail.eof);
}

#[tokio::test]
async fn missing_aggregate_record_is_not_found() {
    let fixture = fixture().await;
    let content = b"orphan".to_vec();
    let outcome = fixture.store.put(&content).await.expect("put blob");
    let artifact_id = ArtifactId::from(outcome.id.as_str());
    // blob 存在但未登记到 aggregate。
    let error = fixture
        .service
        .artifact_read(&artifact_id, 0, 0)
        .await
        .expect_err("missing record must fail");
    assert!(matches!(error, AppServiceError::NotFound(_)));
    assert_eq!(error.error_context().category, ErrorCategory::NotFound);
}

#[tokio::test]
async fn no_store_is_unavailable() {
    let fixture = fixture().await;
    let content = b"no store".to_vec();
    let outcome = fixture.store.put(&content).await.expect("put blob");
    let artifact_id = ArtifactId::from(outcome.id.as_str());
    let service = AppService::new("no-store");
    service
        .router()
        .aggregate()
        .put_artifact(
            artifact_id.clone(),
            content.len() as u64,
            "text/plain".into(),
        )
        .expect("register");
    let error = service
        .artifact_read(&artifact_id, 0, 0)
        .await
        .expect_err("no store must fail");
    assert!(matches!(error, AppServiceError::Unavailable(_)));
    assert_eq!(error.error_context().category, ErrorCategory::Unavailable);
}

#[tokio::test]
async fn offset_beyond_tail_returns_empty_data_with_eof() {
    let fixture = fixture().await;
    let content = b"short".to_vec();
    let outcome = fixture.store.put(&content).await.expect("put blob");
    let artifact_id = ArtifactId::from(outcome.id.as_str());
    register(&fixture.service, &artifact_id, content.len() as u64);

    // offset == length 与 offset >> length 都返回空 data + eof=true。
    for offset in [content.len() as u64, content.len() as u64 + 1000] {
        let result = fixture
            .service
            .artifact_read(&artifact_id, offset, 0)
            .await
            .expect("read beyond tail");
        assert_eq!(result.byte_length, content.len() as u64);
        assert!(result.data.is_empty(), "超尾必须返回空 data");
        assert!(result.eof);
    }
}

#[tokio::test]
async fn invalid_hex_artifact_id_is_not_found() {
    let fixture = fixture().await;
    let artifact_id = ArtifactId::from("not-a-64-hex-id");
    register(&fixture.service, &artifact_id, 42);
    let error = fixture
        .service
        .artifact_read(&artifact_id, 0, 0)
        .await
        .expect_err("invalid hex must fail");
    assert!(matches!(error, AppServiceError::NotFound(_)));
    assert_eq!(error.error_context().category, ErrorCategory::NotFound);
}

#[tokio::test]
async fn corrupted_blob_maps_to_internal_error() {
    let fixture = fixture().await;
    let content = b"pristine".to_vec();
    let outcome = fixture.store.put(&content).await.expect("put blob");
    let artifact_id = ArtifactId::from(outcome.id.as_str());
    register(&fixture.service, &artifact_id, content.len() as u64);
    std::fs::write(fixture.store.blob_path(&outcome.id), b"tampered").expect("tamper blob");

    let error = fixture
        .service
        .artifact_read(&artifact_id, 0, 0)
        .await
        .expect_err("corrupted blob must fail");
    assert!(matches!(error, AppServiceError::ArtifactStore(_)));
    assert_eq!(error.error_context().category, ErrorCategory::Internal);
}
