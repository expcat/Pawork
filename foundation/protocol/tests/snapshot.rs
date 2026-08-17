//! Snapshot 结构校验：data/artifact_id 互斥且 data 有界（L1 定向测试）。

use pawork_domain::{ArtifactId, CoreInstanceId, Timestamp};
use pawork_protocol::GlobalSequence;
use pawork_protocol::{
    ProtocolCodecError, Snapshot, SnapshotSection, SnapshotSectionKind,
    MAX_SNAPSHOT_SECTION_DATA_BYTES,
};
use serde_json::json;

fn section(data: Option<serde_json::Value>, artifact_id: Option<ArtifactId>) -> SnapshotSection {
    SnapshotSection {
        kind: SnapshotSectionKind::ActiveRuns,
        revision: 1,
        data,
        artifact_id,
    }
}

#[test]
fn section_with_both_data_and_artifact_is_rejected() {
    assert!(matches!(
        section(
            Some(json!({"run_ids": ["run-1"]})),
            Some(ArtifactId::from("artifact-1")),
        )
        .validate(),
        Err(ProtocolCodecError::AmbiguousSnapshotSection)
    ));
}

#[test]
fn section_with_neither_is_rejected() {
    assert!(matches!(
        section(None, None).validate(),
        Err(ProtocolCodecError::EmptySnapshotSection)
    ));
}

#[test]
fn section_data_within_bounds_is_accepted() {
    let data = json!({"payload": "x".repeat(1024)});
    assert!(section(Some(data), None).validate().is_ok());
}

#[test]
fn section_data_over_bounds_is_rejected() {
    let data = json!({"payload": "x".repeat(MAX_SNAPSHOT_SECTION_DATA_BYTES + 1)});
    assert!(matches!(
        section(Some(data), None).validate(),
        Err(ProtocolCodecError::SnapshotSectionDataTooLarge { actual, limit })
            if actual > MAX_SNAPSHOT_SECTION_DATA_BYTES && limit == MAX_SNAPSHOT_SECTION_DATA_BYTES
    ));
}

#[test]
fn artifact_backed_section_is_accepted() {
    assert!(section(None, Some(ArtifactId::from("artifact-1")))
        .validate()
        .is_ok());
}

#[test]
fn snapshot_validates_all_sections() {
    let invalid = Snapshot {
        instance_id: CoreInstanceId::from("instance-1"),
        snapshot_sequence: GlobalSequence(42),
        generated_at: Timestamp::from_unix_millis(1),
        sections: vec![
            section(None, Some(ArtifactId::from("artifact-1"))),
            section(Some(json!({"ok": true})), None),
            section(
                Some(json!({"bad": true})),
                Some(ArtifactId::from("artifact-2")),
            ),
        ],
    };
    assert!(matches!(
        invalid.validate(),
        Err(ProtocolCodecError::AmbiguousSnapshotSection)
    ));
}

#[test]
fn valid_snapshot_is_accepted() {
    let snapshot = Snapshot {
        instance_id: CoreInstanceId::from("instance-1"),
        snapshot_sequence: GlobalSequence(42),
        generated_at: Timestamp::from_unix_millis(1),
        sections: vec![
            section(None, Some(ArtifactId::from("artifact-1"))),
            section(Some(json!({"ok": true})), None),
        ],
    };
    assert!(snapshot.validate().is_ok());
}
