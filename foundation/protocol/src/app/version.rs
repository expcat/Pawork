//! 协议版本、握手句柄与控制面作用域。

use pawork_domain::{CoreInstanceId, TenantId};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::quota::DEFAULT_QUOTA_ACCOUNT;

/// S7 基线协议版本（V1 core-api 形状）。
pub const V1_0: ApiVersion = ApiVersion { major: 1, minor: 0 };

pub const API_VERSION: ApiVersion = ApiVersion { major: 1, minor: 1 };


/// 宿主支持的完整 API 版本表（P13-10 schema 版本化）。
///
/// 同 major 内 minor 只增、已发布 minor 必须继续支持；删除或新增 major 走
/// [ADR-036](../../../../../Pawork_v1/docs/adr/ADR-036-gui-protocol-versioning.md) 定义的废弃与删除流程。
pub const SUPPORTED_API_VERSIONS: &[ApiVersion] = &[V1_0, API_VERSION];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
pub struct ApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl ApiVersion {
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// 返回下一个 minor 版本（minor 只增策略下的常规演进入口）。
    pub const fn bump_minor(self) -> Self {
        Self {
            major: self.major,
            minor: self.minor + 1,
        }
    }

    pub const fn is_compatible_with(self, other: Self) -> bool {
        self.major == other.major
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ApiHandle {
    pub instance_id: CoreInstanceId,
    pub api_version: ApiVersion,
}

/// 协议版本与本 crate semver 的对照表。握手 JSON 不得写入 crate semver；
/// 本表只供宿主/文档对照，不进入线上帧。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolCrateCompatibility {
    pub api: ApiVersion,
    pub crate_version: &'static str,
    pub note: &'static str,
}

pub const PROTOCOL_CRATE_COMPATIBILITY: &[ProtocolCrateCompatibility] = &[
    ProtocolCrateCompatibility {
        api: ApiVersion { major: 1, minor: 0 },
        crate_version: "0.1.0",
        note: "S7 基线（V1 core-api 形状）",
    },
    ProtocolCrateCompatibility {
        api: ApiVersion { major: 1, minor: 1 },
        crate_version: "0.1.0",
        note: "S7 Timeline / SessionGet 分页；当前 API_VERSION",
    },
];

// =========================================================================
// Control Plane 作用域（P18-1，ADR-033）：冻结 legacy 作用域与控制面 schema 版本。
// =========================================================================
//
// ADR-033 单独冻结控制面 tenant `local/default`；它与旧 Quota tenant
// `local` 不同，不得复用 `DEFAULT_QUOTA_TENANT`。account 仍与 legacy Quota
// account `local/default` 一致。所有控制面持久化实体与 canonical event 带
// schema_version（ADR-033）。

/// 控制面 schema 版本（与 `provider-control::CONTROL_PLANE_SCHEMA_VERSION` /
/// `app-database::CURRENT_CONTROL_PLANE_SCHEMA_VERSION` 对齐）。
pub const CONTROL_PLANE_SCHEMA_VERSION: u32 = 2;

/// Legacy 控制面租户（ADR-033：`local/default`）。
pub const DEFAULT_CONTROL_PLANE_TENANT: &str = "local/default";
/// Legacy 控制面账号（与 quota 默认账号一致）。
pub const DEFAULT_CONTROL_PLANE_ACCOUNT: &str = DEFAULT_QUOTA_ACCOUNT;
/// Legacy 控制面主体（ADR-033：principal `local/user`）。
pub const DEFAULT_CONTROL_PLANE_PRINCIPAL: &str = "local/user";

/// 控制面作用域：tenant / account / principal 三元组（脱敏，**无 secret 字段**）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ControlPlaneScope {
    pub tenant_id: TenantId,
    pub account_id: String,
    pub principal_id: String,
}

impl ControlPlaneScope {
    /// 默认 legacy 作用域（local/default / local/default / local/user）。
    pub fn legacy_default() -> Self {
        Self {
            tenant_id: TenantId::new(DEFAULT_CONTROL_PLANE_TENANT),
            account_id: DEFAULT_CONTROL_PLANE_ACCOUNT.to_string(),
            principal_id: DEFAULT_CONTROL_PLANE_PRINCIPAL.to_string(),
        }
    }

    /// 是否落在默认 legacy 作用域。
    pub fn is_legacy_default(&self) -> bool {
        self.tenant_id.as_str() == DEFAULT_CONTROL_PLANE_TENANT
            && self.account_id == DEFAULT_CONTROL_PLANE_ACCOUNT
            && self.principal_id == DEFAULT_CONTROL_PLANE_PRINCIPAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_major_version_controls_compatibility() {
        assert!(API_VERSION.is_compatible_with(ApiVersion { major: 1, minor: 9 }));
        assert!(!API_VERSION.is_compatible_with(ApiVersion { major: 2, minor: 0 }));
    }

    #[test]
    fn version_helpers_and_supported_table_are_consistent() {
        assert_eq!(ApiVersion::new(1, 1), API_VERSION);
        assert_eq!(ApiVersion::new(1, 1).bump_minor(), ApiVersion::new(1, 2));
        assert_eq!(
            ApiVersion::new(1, 1).bump_minor().bump_minor(),
            ApiVersion::new(1, 3)
        );
        assert!(SUPPORTED_API_VERSIONS.contains(&V1_0));
        assert!(SUPPORTED_API_VERSIONS.contains(&API_VERSION));
        assert_eq!(SUPPORTED_API_VERSIONS, &[V1_0, API_VERSION]);
        assert!(SUPPORTED_API_VERSIONS
            .iter()
            .all(|version| version.major == API_VERSION.major));
        for version in SUPPORTED_API_VERSIONS {
            assert!(
                PROTOCOL_CRATE_COMPATIBILITY
                    .iter()
                    .any(|row| row.api == *version),
                "compatibility table missing {version:?}"
            );
        }
    }

    #[test]
    fn handshake_types_do_not_embed_crate_semver() {
        let handle = ApiHandle {
            instance_id: CoreInstanceId::from("instance-1"),
            api_version: API_VERSION,
        };
        let json = serde_json::to_value(&handle).expect("serialize handle");
        assert!(json.get("crate_version").is_none());
        assert!(!json.to_string().contains("crate_version"));
        let version = serde_json::to_value(API_VERSION).expect("serialize version");
        assert_eq!(version, serde_json::json!({"major": 1, "minor": 1}));
    }

    #[test]
    fn control_plane_legacy_scope_is_default_and_round_trips() {
        let scope = ControlPlaneScope::legacy_default();
        assert!(scope.is_legacy_default());
        assert_eq!(scope.tenant_id.as_str(), "local/default");
        assert_eq!(scope.account_id, "local/default");
        assert_eq!(scope.principal_id, "local/user");
        assert_eq!(CONTROL_PLANE_SCHEMA_VERSION, 2);

        let json = serde_json::to_string(&scope).expect("serialize");
        let decoded: ControlPlaneScope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, scope);

        let other = ControlPlaneScope {
            tenant_id: TenantId::new("remote"),
            ..scope.clone()
        };
        assert!(!other.is_legacy_default());
    }

}
