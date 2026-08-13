//! 版本 pin 与解析（P17-3）。
//!
//! pin 两种：精确版本（[Pin::Exact]）与内容哈希（[Pin::Hash]，canonical payload
//! 的 blake3 hex）。pin 与安装状态一起持久化在可重放 state store（见 crate::store），
//! 在解析与拉取后校验两处强制执行：
//!
//! - Exact pin：只允许安装该版本；同时给出版本范围且不覆盖 pin 版本时
//!   VersionPinViolation（fail-closed）；
//! - Hash pin：索引带摘要的候选不匹配即拒绝；索引无摘要时延迟到拉取后重算
//!   （仍 fail-closed，见 crate::manager）。

use std::collections::BTreeMap;

use plugin_package::PackageId;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::error::MarketplaceError;
use crate::source::Candidate;

/// 版本 pin。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Pin {
    /// 锁定到具体版本。
    Exact { version: Version },
    /// 锁定到内容哈希（canonical payload 的 blake3 hex）。
    Hash { blake3_hex: String },
}

/// package id -> pin。
pub type PinMap = BTreeMap<String, Pin>;

impl Pin {
    pub fn exact(version: Version) -> Self {
        Self::Exact { version }
    }

    pub fn hash(blake3_hex: impl Into<String>) -> Self {
        Self::Hash {
            blake3_hex: blake3_hex.into(),
        }
    }
}

/// 在有序候选列表（版本降序、source 序号升序，见 crate::source::discover）中
/// 解析唯一安装候选。
pub fn resolve_candidate<'a>(
    id: &PackageId,
    requirement: Option<&VersionReq>,
    pin: Option<&Pin>,
    candidates: &'a [Candidate],
) -> Result<&'a Candidate, MarketplaceError> {
    if candidates.is_empty() {
        return Err(MarketplaceError::PackageNotFound {
            id: id.as_str().to_string(),
        });
    }
    let mut pool: Vec<&Candidate> = candidates.iter().collect();

    match pin {
        Some(Pin::Exact { version }) => {
            if let Some(requirement) = requirement {
                if !requirement.matches(version) {
                    return Err(MarketplaceError::VersionPinViolation {
                        id: id.as_str().to_string(),
                        pinned: version.to_string(),
                    });
                }
            }
            pool.retain(|candidate| &candidate.entry.version == version);
            if pool.is_empty() {
                return Err(MarketplaceError::VersionPinViolation {
                    id: id.as_str().to_string(),
                    pinned: version.to_string(),
                });
            }
        }
        Some(Pin::Hash { blake3_hex }) => {
            let matching: Vec<&Candidate> = pool
                .iter()
                .filter(|candidate| {
                    candidate.entry.digest_hex.as_deref() == Some(blake3_hex.as_str())
                })
                .copied()
                .collect();
            if !matching.is_empty() {
                pool = matching;
            } else {
                let any_known = pool
                    .iter()
                    .any(|candidate| candidate.entry.digest_hex.is_some());
                if any_known {
                    let found = pool
                        .iter()
                        .find_map(|candidate| candidate.entry.digest_hex.clone())
                        .unwrap_or_default();
                    return Err(MarketplaceError::HashPinMismatch {
                        id: id.as_str().to_string(),
                        pinned: blake3_hex.clone(),
                        found,
                    });
                }
                // 候选索引摘要全部缺失：延迟到拉取后重算再 fail-closed。
            }
        }
        None => {}
    }

    let star = VersionReq::parse("*").expect("star requirement parses");
    let requirement = requirement.unwrap_or(&star);
    pool.iter()
        .find(|candidate| requirement.matches(&candidate.entry.version))
        .copied()
        .ok_or_else(|| MarketplaceError::NoMatchingVersion {
            id: id.as_str().to_string(),
            requirement: requirement.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::IndexEntry;

    fn candidate(
        source_index: usize,
        source: &str,
        version: &str,
        digest: Option<&str>,
    ) -> Candidate {
        Candidate {
            source_index,
            source_name: source.into(),
            entry: IndexEntry {
                id: PackageId::new("acme.pkg").unwrap(),
                version: Version::parse(version).unwrap(),
                location: "mem".into(),
                digest_hex: digest.map(str::to_string),
                signature: None,
            },
        }
    }

    fn sorted(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
        candidates.sort_by(|a, b| {
            b.entry
                .version
                .cmp(&a.entry.version)
                .then(a.source_index.cmp(&b.source_index))
        });
        candidates
    }

    #[test]
    fn resolves_highest_matching_version() {
        let candidates = sorted(vec![
            candidate(0, "a", "1.0.0", None),
            candidate(0, "a", "1.4.2", None),
            candidate(0, "a", "2.0.0", None),
        ]);
        let id = PackageId::new("acme.pkg").unwrap();
        let requirement = VersionReq::parse("^1").unwrap();
        let resolved = resolve_candidate(&id, Some(&requirement), None, &candidates).unwrap();
        assert_eq!(resolved.entry.version.to_string(), "1.4.2");
    }

    #[test]
    fn exact_pin_overrides_range_and_conflicts_fail_closed() {
        let candidates = sorted(vec![
            candidate(0, "a", "1.0.0", None),
            candidate(0, "a", "1.2.0", None),
            candidate(0, "a", "2.0.0", None),
        ]);
        let id = PackageId::new("acme.pkg").unwrap();
        let pin = Pin::exact(Version::new(1, 2, 0));
        let resolved = resolve_candidate(&id, None, Some(&pin), &candidates).unwrap();
        assert_eq!(resolved.entry.version.to_string(), "1.2.0");

        let requirement = VersionReq::parse("^2").unwrap();
        let error =
            resolve_candidate(&id, Some(&requirement), Some(&pin), &candidates).unwrap_err();
        assert!(matches!(
            error,
            MarketplaceError::VersionPinViolation { .. }
        ));

        // pin 版本不在候选中 → 同样 VersionPinViolation。
        let missing = Pin::exact(Version::new(9, 9, 9));
        let error = resolve_candidate(&id, None, Some(&missing), &candidates).unwrap_err();
        assert!(matches!(
            error,
            MarketplaceError::VersionPinViolation { .. }
        ));
    }

    #[test]
    fn hash_pin_filters_by_digest_and_fails_closed() {
        let candidates = sorted(vec![
            candidate(0, "a", "1.0.0", Some("aaa")),
            candidate(0, "a", "2.0.0", Some("bbb")),
        ]);
        let id = PackageId::new("acme.pkg").unwrap();
        let pin = Pin::hash("bbb");
        let resolved = resolve_candidate(&id, None, Some(&pin), &candidates).unwrap();
        assert_eq!(resolved.entry.version.to_string(), "2.0.0");

        let pin = Pin::hash("ccc");
        let error = resolve_candidate(&id, None, Some(&pin), &candidates).unwrap_err();
        assert!(matches!(error, MarketplaceError::HashPinMismatch { .. }));
    }

    #[test]
    fn hash_pin_with_unknown_digests_defers_to_fetch() {
        let candidates = sorted(vec![candidate(0, "a", "1.0.0", None)]);
        let id = PackageId::new("acme.pkg").unwrap();
        let pin = Pin::hash("ccc");
        let resolved = resolve_candidate(&id, None, Some(&pin), &candidates).unwrap();
        assert_eq!(resolved.entry.version.to_string(), "1.0.0");
    }

    #[test]
    fn no_matching_version_and_not_found() {
        let id = PackageId::new("acme.pkg").unwrap();
        let candidates = sorted(vec![candidate(0, "a", "1.0.0", None)]);
        let requirement = VersionReq::parse("^2").unwrap();
        assert!(matches!(
            resolve_candidate(&id, Some(&requirement), None, &candidates),
            Err(MarketplaceError::NoMatchingVersion { .. })
        ));
        assert!(matches!(
            resolve_candidate(&id, None, None, &[]),
            Err(MarketplaceError::PackageNotFound { .. })
        ));
    }
}
