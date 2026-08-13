//! Package 标识、归档内相对路径、作用域、依赖声明与来源。
//!
//! 复用 `resource-loader` 的作用域概念：package 作用域与 `ConfigTier` 一致——
//! `Global` 对所有 workspace 生效，`Workspace` 仅对指定 workspace 生效。

use std::path::{Component, Path, PathBuf};

use agent_domain::WorkspaceId;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::error::PackageError;

/// Package 的稳定标识。与 `agent-domain::PluginId` 同构（允许字母数字、`-`、`_`、
/// `.`，但不得含 `..`、不得以分隔符开头/结尾），由 [`PackageId::new`] 强制校验。
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PackageId(String);

impl PackageId {
    /// 校验并构造 Package 标识。
    pub fn new(value: impl Into<String>) -> Result<Self, PackageError> {
        let value = value.into();
        validate_package_id(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for PackageId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PackageId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for PackageId {
    type Error = PackageError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PackageId> for String {
    fn from(value: PackageId) -> Self {
        value.0
    }
}

fn validate_package_id(value: &str) -> Result<(), PackageError> {
    if value.is_empty() {
        return Err(PackageError::field("id", "package id must not be empty"));
    }
    if value.len() > 128 {
        return Err(PackageError::field(
            "id",
            "package id must not exceed 128 characters",
        ));
    }
    if value.contains("..") {
        return Err(PackageError::field(
            "id",
            "package id must not contain '..'",
        ));
    }
    let valid = value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        && value
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && value
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_alphanumeric());
    if !valid {
        return Err(PackageError::field(
            "id",
            "package id may only contain ASCII alphanumerics, '-', '_', '.' and must start/end alphanumeric",
        ));
    }
    Ok(())
}

/// 归档内相对路径的分量数上限（防深路径遍历与分配放大）。
pub const MAX_PATH_DEPTH: usize = 32;

/// 归档内相对路径的编码字节长度上限（每分量计入一个分隔符）。
pub const MAX_PATH_BYTES: usize = 1024;

/// 归档内受校验的相对路径：拒绝绝对路径、前缀与 `..`，防止解包时越界写出归档根。
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "PathBuf")]
pub struct PackageRelativePath(PathBuf);

impl PackageRelativePath {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, PackageError> {
        let path = path.into();
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(PackageError::PathEscape(path));
        }
        // 归一化（分配）前先检查深度 / 长度上限，超长 / 深路径 fail-closed。
        let mut depth = 0usize;
        let mut encoded_len = 0usize;
        for component in path.components() {
            if let Component::Normal(part) = component {
                depth += 1;
                if depth > MAX_PATH_DEPTH {
                    return Err(PackageError::ResourceLimit {
                        resource: "path depth",
                        limit: MAX_PATH_DEPTH as u64,
                        found: depth as u64,
                    });
                }
                encoded_len = encoded_len.saturating_add(part.len().saturating_add(1));
                if encoded_len > MAX_PATH_BYTES {
                    return Err(PackageError::ResourceLimit {
                        resource: "path length",
                        limit: MAX_PATH_BYTES as u64,
                        found: encoded_len as u64,
                    });
                }
            }
        }
        let normalized: PathBuf = path
            .components()
            .filter_map(|component| match component {
                Component::Normal(part) => Some(part),
                Component::CurDir => None,
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => None,
            })
            .collect();
        if normalized.as_os_str().is_empty() {
            return Err(PackageError::field(
                "path",
                "package resource path must not be empty",
            ));
        }
        Ok(Self(normalized))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// 归档内的 POSIX 风格字符串（`/` 分隔），用于内容清单与冲突键。
    pub fn to_posix_string(&self) -> String {
        self.0
            .components()
            .filter_map(|component| match component {
                Component::Normal(part) => part.to_str(),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/")
    }
}

impl TryFrom<PathBuf> for PackageRelativePath {
    type Error = PackageError;
    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Package 作用域，与 `resource-loader` 的层级语义一致。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PackageScope {
    #[default]
    /// 对所有 workspace 生效（对应 `ConfigTier::Global`）。
    Global,
    /// 仅对指定 workspace 生效（对应 `ConfigTier::Workspace`）。
    Workspace { workspace_id: WorkspaceId },
}

/// Package 依赖声明：其他 package / provider / runtime 约束。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PackageDependency {
    /// 依赖另一个 package（按 id + semver 需求）。
    Package {
        id: PackageId,
        #[serde(default = "any_version")]
        version: String,
    },
    /// 依赖某个 provider 存在（可选版本约束）。
    Provider {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// 依赖某个 runtime 能力存在（wasm / mcp / lsp / sandbox）。
    Runtime {
        capability: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
}

impl PackageDependency {
    /// 校验依赖声明的内部一致性（semver 需求可解析、capability 非空）。
    pub fn validate(&self) -> Result<(), PackageError> {
        match self {
            Self::Package { version, .. } => {
                validate_version_req(version)?;
            }
            Self::Provider { name, version }
            | Self::Runtime {
                capability: name,
                version,
            } => {
                if name.trim().is_empty() {
                    return Err(PackageError::field(
                        "dependency",
                        "dependency name/capability must not be empty",
                    ));
                }
                if let Some(version) = version {
                    validate_version_req(version)?;
                }
            }
        }
        Ok(())
    }
}

fn any_version() -> String {
    "*".to_string()
}

fn validate_version_req(value: &str) -> Result<(), PackageError> {
    if value.trim() == "*" {
        return Ok(());
    }
    VersionReq::parse(value)
        .map_err(|error| PackageError::DependencyRequirement(error.to_string()))?;
    Ok(())
}

/// 已解析的 package 版本（用于反序列化后强类型化）。
pub(crate) fn parse_version(value: &str) -> Result<Version, PackageError> {
    Version::parse(value)
        .map_err(|error| PackageError::field("version", format!("invalid semver version: {error}")))
}

/// 分发与诊断使用的 package 来源记录（不含宿主绝对路径）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageProvenance {
    pub package_id: PackageId,
    pub version: Version,
    pub scope: PackageScope,
    /// 稳定来源键（如 `package:<id>@<version>`），用于诊断排序。
    pub source_key: String,
}

impl PackageProvenance {
    pub fn new(package_id: PackageId, version: Version, scope: PackageScope) -> Self {
        let source_key = format!("package:{}@{}", package_id.as_str(), version);
        Self {
            package_id,
            version,
            scope,
            source_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_id_validates() {
        assert!(PackageId::new("acme.search").is_ok());
        assert!(PackageId::new("acme_search-1").is_ok());
        assert!(PackageId::new("").is_err());
        assert!(PackageId::new(".leading").is_err());
        assert!(PackageId::new("trailing.").is_err());
        assert!(PackageId::new("a..b").is_err());
        assert!(PackageId::new("bad space").is_err());
    }

    #[test]
    fn relative_path_rejects_escape_and_absolute() {
        assert!(PackageRelativePath::new("skills/x/manifest.toml").is_ok());
        assert!(PackageRelativePath::new("./skills/./x").is_ok());
        assert!(PackageRelativePath::new("../escape").is_err());
        assert!(PackageRelativePath::new("/etc/passwd").is_err());
        assert!(PackageRelativePath::new("").is_err());
    }

    #[test]
    fn relative_path_depth_and_length_are_bounded() {
        // 32 个分量：恰好在上限内，通过。
        let at_limit = (0..MAX_PATH_DEPTH)
            .map(|index| format!("d{index}"))
            .collect::<Vec<_>>()
            .join("/");
        assert!(PackageRelativePath::new(&at_limit).is_ok());

        // 33 个分量：超出深度上限。
        let error = PackageRelativePath::new(format!("{at_limit}/extra")).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "path depth",
                ..
            }
        ));

        // 单分量 1025 字节：计入分隔符后超出长度上限。
        let error = PackageRelativePath::new("x".repeat(MAX_PATH_BYTES + 1)).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "path length",
                ..
            }
        ));
    }

    #[test]
    fn dependency_validates_version_req() {
        assert!(PackageDependency::Package {
            id: PackageId::new("dep").unwrap(),
            version: ">=1, <2".into(),
        }
        .validate()
        .is_ok());
        assert!(PackageDependency::Package {
            id: PackageId::new("dep").unwrap(),
            version: "not semver".into(),
        }
        .validate()
        .is_err());
        assert!(PackageDependency::Runtime {
            capability: "  ".into(),
            version: None,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn package_id_round_trips_through_serde() {
        let id = PackageId::new("acme.pkg").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        let back: PackageId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
        assert!(serde_json::from_str::<PackageId>(r#""bad..id""#).is_err());
    }
}
