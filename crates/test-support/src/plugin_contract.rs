//! P10-6 Plugin API 版本兼容矩阵与断言辅助。
//!
//! API 兼容由 manifest 的 semver `VersionReq` 与 host API version 共同判定：
//! 范围包含当前版本才允许加载；minor 兼容由声明范围显式表达，跨 major 默认拒绝。
//! 本模块作为 `test-support` contract 由 workspace test 三平台执行。

use plugin_api::{ManifestValidationError, PluginManifest};
use semver::{Version, VersionReq};

/// 兼容矩阵中的预期结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatibilityExpectation {
    Compatible,
    Incompatible,
}

/// 一条 host API 兼容用例。
#[derive(Clone, Copy, Debug)]
pub struct ApiCompatibilityCase {
    pub name: &'static str,
    pub plugin_requirement: &'static str,
    pub host_version: &'static str,
    pub expectation: CompatibilityExpectation,
}

/// P10-6 版本兼容矩阵：同 major/minor、跨 major、不满足声明范围三类组合。
pub const HOST_API_COMPATIBILITY_MATRIX: &[ApiCompatibilityCase] = &[
    // 同 major / minor 内兼容。
    ApiCompatibilityCase {
        name: "exact current api version",
        plugin_requirement: "=1.0.0",
        host_version: "1.0.0",
        expectation: CompatibilityExpectation::Compatible,
    },
    ApiCompatibilityCase {
        name: "bare major implies caret",
        plugin_requirement: "1",
        host_version: "1.3.0",
        expectation: CompatibilityExpectation::Compatible,
    },
    ApiCompatibilityCase {
        name: "caret within same major",
        plugin_requirement: "^1.0",
        host_version: "1.4.2",
        expectation: CompatibilityExpectation::Compatible,
    },
    ApiCompatibilityCase {
        name: "tilde within same major/minor",
        plugin_requirement: "~1.2.0",
        host_version: "1.2.9",
        expectation: CompatibilityExpectation::Compatible,
    },
    ApiCompatibilityCase {
        name: "explicit range within same major",
        plugin_requirement: ">=1.0, <2.0",
        host_version: "1.9.0",
        expectation: CompatibilityExpectation::Compatible,
    },
    // 跨 major 默认拒绝。
    ApiCompatibilityCase {
        name: "caret across major",
        plugin_requirement: "^1.0",
        host_version: "2.0.0",
        expectation: CompatibilityExpectation::Incompatible,
    },
    ApiCompatibilityCase {
        name: "range across major",
        plugin_requirement: ">=1.0, <2.0",
        host_version: "2.1.0",
        expectation: CompatibilityExpectation::Incompatible,
    },
    // 同 major 内不满足声明范围。
    ApiCompatibilityCase {
        name: "host below minimum minor",
        plugin_requirement: "^1.2",
        host_version: "1.1.9",
        expectation: CompatibilityExpectation::Incompatible,
    },
    ApiCompatibilityCase {
        name: "host below minimum patch",
        plugin_requirement: "~1.2.3",
        host_version: "1.2.2",
        expectation: CompatibilityExpectation::Incompatible,
    },
    ApiCompatibilityCase {
        name: "exact version mismatch",
        plugin_requirement: "=1.2.3",
        host_version: "1.2.4",
        expectation: CompatibilityExpectation::Incompatible,
    },
    ApiCompatibilityCase {
        name: "prerelease host outside range",
        plugin_requirement: "^1.0",
        host_version: "1.0.0-rc.1",
        expectation: CompatibilityExpectation::Incompatible,
    },
];

/// 判断插件 API 要求是否包含宿主版本。
pub fn matches_api(plugin_requirement: &str, host_version: &str) -> bool {
    let requirement = VersionReq::parse(plugin_requirement)
        .expect("plugin API requirement must be a valid semver requirement");
    let host =
        Version::parse(host_version).expect("host API version must be a valid semver version");
    requirement.matches(&host)
}

/// 断言插件 API 要求包含宿主版本（同 major/minor 兼容）。
pub fn assert_api_compatible(plugin_requirement: &str, host_version: &str) {
    assert!(
        matches_api(plugin_requirement, host_version),
        "plugin API requirement {plugin_requirement} must include host API {host_version}"
    );
}

/// 断言插件 API 要求不包含宿主版本（跨 major 或不满足范围）。
pub fn assert_api_incompatible(plugin_requirement: &str, host_version: &str) {
    assert!(
        !matches_api(plugin_requirement, host_version),
        "plugin API requirement {plugin_requirement} must not include host API {host_version}"
    );
}

/// 逐条执行 [`HOST_API_COMPATIBILITY_MATRIX`]，作为 P10-6 的门禁入口。
pub fn assert_compatibility_matrix() {
    for case in HOST_API_COMPATIBILITY_MATRIX {
        let actual = matches_api(case.plugin_requirement, case.host_version);
        let expected = case.expectation == CompatibilityExpectation::Compatible;
        assert_eq!(
            actual, expected,
            "compatibility case '{}' diverges: plugin requirement {} vs host {}",
            case.name, case.plugin_requirement, case.host_version
        );
    }
}

/// 断言 manifest 的 API 要求包含指定宿主版本。
pub fn assert_manifest_api_compatible(manifest: &PluginManifest, host_version: &Version) {
    manifest
        .ensure_api_compatible(host_version)
        .unwrap_or_else(|error| {
            panic!(
                "manifest API requirement {} must include host {host_version}: {error}",
                manifest.api_version
            )
        });
}

/// 断言 manifest 的 API 要求被指定宿主版本拒绝（`IncompatibleApi`）。
pub fn assert_manifest_api_incompatible(manifest: &PluginManifest, host_version: &Version) {
    match manifest.ensure_api_compatible(host_version) {
        Err(ManifestValidationError::IncompatibleApi { .. }) => {}
        other => panic!(
            "manifest API requirement {} must be rejected against host {host_version}, got {other:?}",
            manifest.api_version
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_plugin_with_api;
    use plugin_api::plugin_api_version;

    #[test]
    fn matrix_matches_expected_outcomes() {
        assert_compatibility_matrix();
    }

    #[test]
    fn same_major_and_minor_are_compatible() {
        assert_api_compatible("^1.0", "1.2.3");
        assert_api_compatible("1", "1.9.0");
        assert_api_compatible("~1.2.0", "1.2.5");
    }

    #[test]
    fn cross_major_is_rejected() {
        assert_api_incompatible("^1.0", "2.0.0");
        assert_api_incompatible(">=1, <2", "2.1.0");
    }

    #[test]
    fn unsatisfied_range_is_rejected() {
        assert_api_incompatible("^1.2", "1.1.9");
        assert_api_incompatible("~1.2.3", "1.2.2");
        assert_api_incompatible("=1.2.3", "1.2.4");
    }

    #[test]
    fn manifest_helpers_apply_host_version() {
        let compatible = hook_plugin_with_api("p", "^1.2", []);
        assert_manifest_api_compatible(compatible.manifest(), &Version::new(1, 2, 0));
        assert_manifest_api_incompatible(compatible.manifest(), &Version::new(1, 1, 0));

        let cross_major = hook_plugin_with_api("q", "^2.0", []);
        assert_manifest_api_incompatible(cross_major.manifest(), &plugin_api_version());
    }
}
