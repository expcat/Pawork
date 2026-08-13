//! 可重放 state / pin store（P17-3）。
//!
//! 安装集、版本 pin 与快照 revision 一起持久化为原子快照：每次保存写入完整
//! 状态（temp 文件 + rename 原子替换），确定性 JSON（BTreeMap 有序）可重放、
//! 可审计。Marketplace 安装 / 更新 / 卸载后的状态变更均以该快照为唯一事实源。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use plugin_package::PackageDependency;
use plugin_package::{DispatchPlan, PackageId};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::error::MarketplaceError;
use crate::pin::Pin;

/// 已安装包记录（含完整分发计划，支撑精确卸载与回滚恢复）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPackage {
    pub id: PackageId,
    pub version: Version,
    /// 安装来源 source 名。
    pub source: String,
    /// 内容摘要（canonical payload 的 blake3 hex）。
    pub digest_hex: String,
    /// 安装时解析并校验的 Package 依赖。更新/卸载前用它验证变更后的反向依赖
    /// 闭包，避免留下仍标记为 installed 的悬空依赖方。
    #[serde(default)]
    pub dependencies: Vec<PackageDependency>,
    /// 安装时全部子资源的分发计划（卸载 / 回滚重放依赖它）。
    pub plan: DispatchPlan,
    /// 安装时从已验证归档抽出的身份键（skill id / agent name / hook trigger /
    /// mcp server / lsp id / monitor id）。路径键仍在 `plan` 中；身份键挡住
    /// 「同身份、不同路径」的跨包冲突。旧快照缺省为空。
    #[serde(default)]
    pub identity_keys: Vec<(String, String)>,
}

/// Marketplace 状态：安装集 + pin + 单调 revision。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceState {
    /// 快照 revision（每次保存 +1，重放 / 审计用）。
    #[serde(default)]
    pub revision: u64,
    /// package id -> 安装记录。
    #[serde(default)]
    pub installed: BTreeMap<String, InstalledPackage>,
    /// package id -> pin。
    #[serde(default)]
    pub pins: BTreeMap<String, Pin>,
}

impl MarketplaceState {
    pub fn installed(&self, id: &PackageId) -> Option<&InstalledPackage> {
        self.installed.get(id.as_str())
    }

    pub fn pin(&self, id: &PackageId) -> Option<&Pin> {
        self.pins.get(id.as_str())
    }
}

/// 状态存储抽象（可重放）。
pub trait StateStore {
    fn load(&self) -> Result<MarketplaceState, MarketplaceError>;
    fn save(&mut self, state: &MarketplaceState) -> Result<(), MarketplaceError>;
}

/// 内存状态存储（测试 / 临时宿主）。
#[derive(Clone, Debug, Default)]
pub struct MemoryStore {
    state: MarketplaceState,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StateStore for MemoryStore {
    fn load(&self) -> Result<MarketplaceState, MarketplaceError> {
        Ok(self.state.clone())
    }

    fn save(&mut self, state: &MarketplaceState) -> Result<(), MarketplaceError> {
        self.state = state.clone();
        Ok(())
    }
}

/// 原子文件状态存储：确定性 JSON + temp 文件 + rename（写入对外原子可见）。
#[derive(Clone, Debug)]
pub struct AtomicFileStore {
    path: PathBuf,
}

impl AtomicFileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl StateStore for AtomicFileStore {
    fn load(&self) -> Result<MarketplaceState, MarketplaceError> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| {
                MarketplaceError::State(format!(
                    "state file {} is unreadable (fail-closed): {error}",
                    self.path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(MarketplaceState::default())
            }
            Err(error) => Err(MarketplaceError::State(format!(
                "cannot read {}: {error}",
                self.path.display()
            ))),
        }
    }

    fn save(&mut self, state: &MarketplaceState) -> Result<(), MarketplaceError> {
        let io_error = |error: std::io::Error| {
            MarketplaceError::State(format!("{}: {error}", self.path.display()))
        };
        let text = serde_json::to_string_pretty(state).map_err(|error| {
            MarketplaceError::State(format!("state serialization failed: {error}"))
        })?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("marketplace-state");
        let tmp = self.path.with_file_name(format!("{file_name}.tmp"));
        let mut file = fs::File::create(&tmp).map_err(io_error)?;
        file.write_all(text.as_bytes()).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        fs::rename(&tmp, &self.path).map_err(io_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> MarketplaceState {
        let mut state = MarketplaceState {
            revision: 3,
            ..MarketplaceState::default()
        };
        state
            .pins
            .insert("acme.pkg".into(), Pin::exact(Version::new(1, 2, 0)));
        state
            .pins
            .insert("acme.other".into(), Pin::hash("deadbeef"));
        state
    }

    #[test]
    fn memory_store_round_trip() {
        let mut store = MemoryStore::new();
        assert_eq!(store.load().unwrap(), MarketplaceState::default());
        let state = sample_state();
        store.save(&state).unwrap();
        assert_eq!(store.load().unwrap(), state);
    }

    #[test]
    fn atomic_file_store_round_trip_is_replayable_json() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = AtomicFileStore::new(dir.path().join("state.json"));
        // 缺失文件 → 缺省状态。
        assert_eq!(store.load().unwrap(), MarketplaceState::default());

        let state = sample_state();
        store.save(&state).unwrap();
        assert_eq!(store.load().unwrap(), state);

        // 文件是确定性 JSON（pins 按 id 有序），可直接审计 / 重放。
        let text = fs::read_to_string(dir.path().join("state.json")).unwrap();
        let first = text.find("acme.other").unwrap();
        let second = text.find("acme.pkg").unwrap();
        assert!(first < second, "BTreeMap ordering must be preserved");
        // 无残留 temp 文件。
        assert!(fs::read_dir(dir.path())
            .unwrap()
            .all(|entry| entry.unwrap().file_name() == "state.json"));
    }

    #[test]
    fn atomic_file_store_fails_closed_on_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let store = AtomicFileStore::new(&path);
        fs::write(&path, "not json").unwrap();
        let error = store.load().unwrap_err();
        assert!(matches!(error, MarketplaceError::State(_)));
        assert!(error.to_string().contains("fail-closed"));
    }
}
