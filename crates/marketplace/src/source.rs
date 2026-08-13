//! Source 模型与发现（P17-3）。
//!
//! source 三类：registry（索引 URL）、git（仓库 URL）、local（本地目录）。
//! 全部网络 / 文件系统 I/O 经注入的 [SourceIo] trait 委托，本 crate 只提供内存
//! mock 实现（[InMemorySourceIo]）；真实 registry / git 拉取由后续接线任务
//! （http-runtime 等）实现同一 trait，marketplace 逻辑本身不直接触达 I/O。
//!
//! 多 source 聚合 fail-closed：任一 source 索引拉取失败则整体发现报错，不静默
//! 降级为部分 source。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use plugin_package::PackageId;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::error::MarketplaceError;
use crate::signature::PackageSignature;
use crate::trust::TrustLevel;

/// Source 种类。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceKind {
    /// Registry：索引 + 包对象拉取 URL。
    Registry { url: String },
    /// Git 仓库。
    Git { url: String },
    /// 本地目录（离线 / 开发源）。
    Local { path: PathBuf },
}

impl SourceKind {
    /// 人类可读定位符（错误诊断用）。
    pub fn location(&self) -> String {
        match self {
            Self::Registry { url } | Self::Git { url } => url.clone(),
            Self::Local { path } => path.display().to_string(),
        }
    }
}

/// 已配置的 source：名称 + 种类 + 默认 trust。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpec {
    /// source 唯一名称（state、policy、trust 配置均以名索引）。
    pub name: String,
    pub kind: SourceKind,
    /// source 默认 trust，可被 [crate::trust::TrustConfig] 覆盖。缺省 untrusted（fail-closed）。
    #[serde(default)]
    pub trust: TrustLevel,
}

impl SourceSpec {
    pub fn registry(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: SourceKind::Registry { url: url.into() },
            trust: TrustLevel::default(),
        }
    }

    pub fn git(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: SourceKind::Git { url: url.into() },
            trust: TrustLevel::default(),
        }
    }

    pub fn local(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            kind: SourceKind::Local { path: path.into() },
            trust: TrustLevel::default(),
        }
    }

    pub fn with_trust(mut self, trust: TrustLevel) -> Self {
        self.trust = trust;
        self
    }
}

/// Source 索引中的单个包版本条目。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub id: PackageId,
    pub version: Version,
    /// 拉取定位符（registry 对象路径 / git ref / local 子目录），由对应 SourceIo 解释。
    pub location: String,
    /// 索引随附的内容摘要（canonical payload 的 blake3 hex）。无法预先知道内容的
    /// source 可缺省；拉取后一律重算并交叉校验（hash pin / bundle hash，fail-closed）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_hex: Option<String>,
    /// Ed25519 签名信封（可选；是否强制由 trust / team policy 决定）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<PackageSignature>,
}

/// Source 索引：该 source 可发现的全部包版本。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIndex {
    #[serde(default)]
    pub packages: Vec<IndexEntry>,
}

/// Source I/O 注入点。
///
/// 所有网络 / git / 文件系统访问由宿主实现本 trait；marketplace 逻辑只依赖该
/// 抽象，保证单测可完全离线用 mock 运行。
pub trait SourceIo {
    /// 拉取 source 索引；失败返回 SourceIo / InvalidIndex 错误（fail-closed）。
    fn fetch_index(&self, source: &SourceSpec) -> Result<SourceIndex, MarketplaceError>;

    /// 将索引条目对应的归档目录物化到 dest（须为 plugin_package::read_archive
    /// 可校验的归档形态）。
    fn fetch_archive(
        &self,
        source: &SourceSpec,
        entry: &IndexEntry,
        dest: &Path,
    ) -> Result<(), MarketplaceError>;
}

/// 安装候选：索引条目 + source 归属。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// source 在配置序列中的位置（tie-break 优先：小者先）。
    pub source_index: usize,
    pub source_name: String,
    pub entry: IndexEntry,
}

/// 多 source 聚合的发现结果。
#[derive(Clone, Debug, Default)]
pub struct Discovery {
    /// package id -> 候选列表（版本降序、source 序号升序）。
    pub packages: BTreeMap<String, Vec<Candidate>>,
}

/// 聚合多个 source 的索引；任一 source 失败则整体失败（fail-closed）。
pub fn discover(sources: &[SourceSpec], io: &dyn SourceIo) -> Result<Discovery, MarketplaceError> {
    let mut discovery = Discovery::default();
    for (index, source) in sources.iter().enumerate() {
        let source_index = io.fetch_index(source)?;
        for entry in source_index.packages {
            let id = entry.id.as_str().to_string();
            discovery.packages.entry(id).or_default().push(Candidate {
                source_index: index,
                source_name: source.name.clone(),
                entry,
            });
        }
    }
    for candidates in discovery.packages.values_mut() {
        candidates.sort_by(|a, b| {
            b.entry
                .version
                .cmp(&a.entry.version)
                .then(a.source_index.cmp(&b.source_index))
        });
    }
    Ok(discovery)
}

type ArchiveFactory = Arc<dyn Fn(&Path) -> Result<(), MarketplaceError> + Send + Sync>;

fn archive_key(source_name: &str, entry: &IndexEntry) -> (String, String, Version) {
    (
        source_name.to_string(),
        entry.id.as_str().to_string(),
        entry.version.clone(),
    )
}

/// 内存 mock source I/O：定向测试与离线开发用。
///
/// 支持注册索引、发布归档目录（fetch 时递归拷贝）、注入索引 / 归档拉取失败。
#[derive(Default)]
pub struct InMemorySourceIo {
    indices: BTreeMap<String, SourceIndex>,
    archives: BTreeMap<(String, String, Version), ArchiveFactory>,
    index_failures: BTreeMap<String, String>,
    archive_failures: BTreeMap<(String, String, Version), String>,
}

impl InMemorySourceIo {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册某 source 的索引。
    ///
    /// 整体替换该 source 的索引（含 publish_dir / publish_with 自动登记的条目）。
    pub fn set_index(&mut self, source_name: impl Into<String>, index: SourceIndex) {
        self.indices.insert(source_name.into(), index);
    }

    /// 登记 / 替换某条目的索引项（mock 语义：publish 即可被发现）。
    fn upsert_index(&mut self, source_name: &str, entry: IndexEntry) {
        let packages = &mut self
            .indices
            .entry(source_name.to_string())
            .or_default()
            .packages;
        if let Some(existing) = packages
            .iter_mut()
            .find(|item| item.id == entry.id && item.version == entry.version)
        {
            *existing = entry;
        } else {
            packages.push(entry);
        }
    }

    /// 发布归档目录：fetch 时把 src_dir 递归拷贝到 dest。
    ///
    /// 同时把条目登记进该 source 的索引（publish 即 discoverable）。
    pub fn publish_dir(&mut self, source_name: &str, entry: &IndexEntry, src_dir: PathBuf) {
        self.upsert_index(source_name, entry.clone());
        let factory: ArchiveFactory = Arc::new(move |dest| copy_dir_recursive(&src_dir, dest));
        self.archives
            .insert(archive_key(source_name, entry), factory);
    }

    /// 发布自定义归档工厂（把完整归档写入 dest）。
    pub fn publish_with(
        &mut self,
        source_name: &str,
        entry: &IndexEntry,
        factory: impl Fn(&Path) -> Result<(), MarketplaceError> + Send + Sync + 'static,
    ) {
        self.upsert_index(source_name, entry.clone());
        self.archives
            .insert(archive_key(source_name, entry), Arc::new(factory));
    }

    /// 注入某 source 的索引拉取失败。
    pub fn fail_index(&mut self, source_name: impl Into<String>, message: impl Into<String>) {
        self.index_failures
            .insert(source_name.into(), message.into());
    }

    /// 注入某条目的归档拉取失败。
    pub fn fail_archive(
        &mut self,
        source_name: &str,
        entry: &IndexEntry,
        message: impl Into<String>,
    ) {
        self.archive_failures
            .insert(archive_key(source_name, entry), message.into());
    }
}

impl SourceIo for InMemorySourceIo {
    fn fetch_index(&self, source: &SourceSpec) -> Result<SourceIndex, MarketplaceError> {
        if let Some(message) = self.index_failures.get(&source.name) {
            return Err(MarketplaceError::SourceIo {
                name: source.name.clone(),
                message: message.clone(),
            });
        }
        self.indices
            .get(&source.name)
            .cloned()
            .ok_or_else(|| MarketplaceError::SourceIo {
                name: source.name.clone(),
                message: format!("source not found: {}", source.kind.location()),
            })
    }

    fn fetch_archive(
        &self,
        source: &SourceSpec,
        entry: &IndexEntry,
        dest: &Path,
    ) -> Result<(), MarketplaceError> {
        let key = archive_key(&source.name, entry);
        if let Some(message) = self.archive_failures.get(&key) {
            return Err(MarketplaceError::SourceIo {
                name: source.name.clone(),
                message: message.clone(),
            });
        }
        let factory = self
            .archives
            .get(&key)
            .ok_or_else(|| MarketplaceError::SourceIo {
                name: source.name.clone(),
                message: format!(
                    "no archive published for {}@{}",
                    entry.id.as_str(),
                    entry.version
                ),
            })?;
        factory(dest)
    }
}

/// 递归拷贝目录（mock fetch I/O；错误归属 staging）。
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), MarketplaceError> {
    let meta = std::fs::metadata(src).map_err(|error| {
        MarketplaceError::Staging(format!("mock source dir {}: {error}", src.display()))
    })?;
    if !meta.is_dir() {
        return Err(MarketplaceError::Staging(format!(
            "mock source {} is not a directory",
            src.display()
        )));
    }
    std::fs::create_dir_all(dest).map_err(|error| {
        MarketplaceError::Staging(format!("create {}: {error}", dest.display()))
    })?;
    for entry in std::fs::read_dir(src)
        .map_err(|error| MarketplaceError::Staging(format!("read {}: {error}", src.display())))?
    {
        let entry = entry.map_err(|error| MarketplaceError::Staging(error.to_string()))?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|error| {
                MarketplaceError::Staging(format!(
                    "copy {} -> {}: {error}",
                    from.display(),
                    to.display()
                ))
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, version: &str) -> IndexEntry {
        IndexEntry {
            id: PackageId::new(id).unwrap(),
            version: Version::parse(version).unwrap(),
            location: "mem".into(),
            digest_hex: None,
            signature: None,
        }
    }

    #[test]
    fn discovery_aggregates_and_sorts_candidates() {
        let mut io = InMemorySourceIo::new();
        io.set_index(
            "a",
            SourceIndex {
                packages: vec![entry("acme.one", "1.0.0"), entry("acme.one", "2.0.0")],
            },
        );
        io.set_index(
            "b",
            SourceIndex {
                packages: vec![entry("acme.one", "2.0.0"), entry("acme.two", "0.1.0")],
            },
        );
        let sources = vec![
            SourceSpec::registry("a", "https://a.example/index.json"),
            SourceSpec::registry("b", "https://b.example/index.json"),
        ];
        let discovery = discover(&sources, &io).unwrap();
        let one = &discovery.packages["acme.one"];
        assert_eq!(one.len(), 3);
        // 版本降序，同版本优先 source a（序号小）。
        assert_eq!(one[0].entry.version.to_string(), "2.0.0");
        assert_eq!(one[0].source_name, "a");
        assert_eq!(one[1].source_name, "b");
        assert_eq!(one[2].entry.version.to_string(), "1.0.0");
        assert!(discovery.packages.contains_key("acme.two"));
    }

    #[test]
    fn discovery_fails_closed_when_any_source_fails() {
        let mut io = InMemorySourceIo::new();
        io.set_index("a", SourceIndex::default());
        io.fail_index("b", "network down");
        let sources = vec![
            SourceSpec::registry("a", "https://a.example"),
            SourceSpec::registry("b", "https://b.example"),
        ];
        let error = discover(&sources, &io).unwrap_err();
        assert!(matches!(error, MarketplaceError::SourceIo { .. }));
    }
}
