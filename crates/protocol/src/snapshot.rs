//! Snapshot 结构校验：`SnapshotSection.data` / `artifact_id` 互斥且 data 有界。

use crate::{ProtocolCodecError, Snapshot, SnapshotSection, MAX_SNAPSHOT_SECTION_DATA_BYTES};

impl Snapshot {
    /// 校验全部 section；任一 section 非法则整个 Snapshot 拒绝。
    pub fn validate(&self) -> Result<(), ProtocolCodecError> {
        for section in &self.sections {
            section.validate()?;
        }
        Ok(())
    }
}

impl SnapshotSection {
    /// 校验 section：`data` 与 `artifact_id` 必须恰好设置其一，且内联 `data`
    /// 编码后不超过 [`MAX_SNAPSHOT_SECTION_DATA_BYTES`]。
    pub fn validate(&self) -> Result<(), ProtocolCodecError> {
        match (&self.data, &self.artifact_id) {
            (Some(_), Some(_)) => Err(ProtocolCodecError::AmbiguousSnapshotSection),
            (None, None) => Err(ProtocolCodecError::EmptySnapshotSection),
            (Some(data), None) => {
                let encoded = serde_json::to_vec(data).map_err(ProtocolCodecError::InvalidJson)?;
                if encoded.len() > MAX_SNAPSHOT_SECTION_DATA_BYTES {
                    return Err(ProtocolCodecError::SnapshotSectionDataTooLarge {
                        actual: encoded.len(),
                        limit: MAX_SNAPSHOT_SECTION_DATA_BYTES,
                    });
                }
                Ok(())
            }
            (None, Some(_)) => Ok(()),
        }
    }
}
