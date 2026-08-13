//! 最小 RBAC：主体角色、权限与 deny-first 合并规则（P18-9）。
//!
//! 角色与权限是 PolicySet 的一部分，判定规则全部 deny-first：
//! - 未显式授权的角色一律拒绝，不存在「默认放行」路径；
//! - 多个来源的合并（default_role × principal_roles）取**更受限**的角色
//!   （[`PrincipalRole::merge_deny_first`]）；
//! - 操作人与执行 Agent 分离：任何执行入口只认解析出的
//!   [`crate::identity::IdentityContext`]，不得从请求 payload 里自行推断角色。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use agent_domain::PrincipalId;

/// 主体最小角色集。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalRole {
    /// 租户管理员：全部权限，含 audit 导出与策略管理。
    Admin,
    /// 普通用户：操作与读自己的 session / usage / audit。
    User,
    /// 服务账号：执行（spawn / route / lease）与 usage 对账，不读内容与 audit。
    Service,
    /// 只读观察者：只读自己的 session / usage。
    Viewer,
}

impl Default for PrincipalRole {
    /// deny-first 默认：未配置角色一律最受限的 Viewer。
    fn default() -> Self {
        PrincipalRole::Viewer
    }
}

/// 权限：执行入口的最小能力单位。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// 创建 Agent。
    AgentSpawn,
    /// 参与 route candidate 过滤。
    RouteCandidate,
    /// 申请 credential lease。
    LeaseAcquire,
    /// 查询 Session。
    SessionRead,
    /// 查询 Usage / Quota。
    UsageRead,
    /// 查询 Audit（决策事件）。
    AuditRead,
    /// 导出 Audit。
    AuditExport,
    /// 管理租户策略。
    PolicyManage,
}

impl Permission {
    /// 冻结的持久化字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentSpawn => "agent_spawn",
            Self::RouteCandidate => "route_candidate",
            Self::LeaseAcquire => "lease_acquire",
            Self::SessionRead => "session_read",
            Self::UsageRead => "usage_read",
            Self::AuditRead => "audit_read",
            Self::AuditExport => "audit_export",
            Self::PolicyManage => "policy_manage",
        }
    }
}

impl PrincipalRole {
    /// 角色特权等级（数值越大越宽松），供 deny-first 合并比较。
    pub fn rank(self) -> u8 {
        match self {
            PrincipalRole::Admin => 4,
            PrincipalRole::User => 3,
            PrincipalRole::Service => 2,
            PrincipalRole::Viewer => 1,
        }
    }

    /// 冻结的持久化字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            PrincipalRole::Admin => "admin",
            PrincipalRole::User => "user",
            PrincipalRole::Service => "service",
            PrincipalRole::Viewer => "viewer",
        }
    }

    /// 角色可用的权限集合（最小 RBAC 首轮）。
    pub fn permissions(self) -> &'static [Permission] {
        match self {
            PrincipalRole::Admin => &[
                Permission::AgentSpawn,
                Permission::RouteCandidate,
                Permission::LeaseAcquire,
                Permission::SessionRead,
                Permission::UsageRead,
                Permission::AuditRead,
                Permission::AuditExport,
                Permission::PolicyManage,
            ],
            PrincipalRole::User => &[
                Permission::AgentSpawn,
                Permission::RouteCandidate,
                Permission::LeaseAcquire,
                Permission::SessionRead,
                Permission::UsageRead,
                Permission::AuditRead,
            ],
            PrincipalRole::Service => &[
                Permission::AgentSpawn,
                Permission::RouteCandidate,
                Permission::LeaseAcquire,
                Permission::UsageRead,
            ],
            PrincipalRole::Viewer => &[Permission::SessionRead, Permission::UsageRead],
        }
    }

    /// 角色是否被授权该权限（deny-first：不在集合内一律 false）。
    pub fn allows(self, permission: Permission) -> bool {
        self.permissions().contains(&permission)
    }

    /// deny-first 合并：返回两个角色中更受限的一个（rank 更低）。
    pub fn merge_deny_first(self, other: Self) -> Self {
        if self.rank() <= other.rank() {
            self
        } else {
            other
        }
    }
}

/// 权限配置：默认角色 + 按 principal 覆盖。
///
/// deny-first 合并：principal 显式角色与默认角色同时存在时取更受限者，
/// 避免宽松默认「冲掉」显式限制。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionProfile {
    /// 未在 [`Self::principal_roles`] 命中的主体使用该默认角色；
    /// `None` 时未命中主体回落到 [`PrincipalRole::default`]（Viewer）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_role: Option<PrincipalRole>,
    /// 按 principal 的显式角色覆盖。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub principal_roles: BTreeMap<PrincipalId, PrincipalRole>,
}

impl PermissionProfile {
    /// 计算主体有效角色（deny-first 合并，见 [`PrincipalRole::merge_deny_first`]）；
    /// 未命中且无默认角色返回 `None`（调用方按最受限角色处理）。
    pub fn effective_role(&self, principal: &PrincipalId) -> Option<PrincipalRole> {
        match (
            self.principal_roles.get(principal).copied(),
            self.default_role,
        ) {
            (Some(role), Some(default)) => Some(role.merge_deny_first(default)),
            (Some(role), None) => Some(role),
            (None, Some(default)) => Some(default),
            (None, None) => None,
        }
    }
}

/// Audit 导出策略（deny-first）：`TenantPolicy.audit_export = None` 或
/// `enabled = false` 一律拒绝导出；目标必须在 `allowed_destinations` 内。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditExportPolicy {
    /// 是否启用导出（默认关闭）。
    #[serde(default)]
    pub enabled: bool,
    /// 允许的导出目标（空列表 = 无任何目标可导出）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_destinations: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_permission_matrix_is_deny_first() {
        assert!(PrincipalRole::Admin.allows(Permission::AuditExport));
        assert!(PrincipalRole::Admin.allows(Permission::PolicyManage));
        assert!(PrincipalRole::User.allows(Permission::AgentSpawn));
        assert!(PrincipalRole::User.allows(Permission::AuditRead));
        assert!(!PrincipalRole::User.allows(Permission::AuditExport));
        assert!(!PrincipalRole::User.allows(Permission::PolicyManage));
        assert!(PrincipalRole::Service.allows(Permission::UsageRead));
        assert!(PrincipalRole::Service.allows(Permission::AgentSpawn));
        assert!(!PrincipalRole::Service.allows(Permission::SessionRead));
        assert!(!PrincipalRole::Service.allows(Permission::AuditRead));
        assert!(PrincipalRole::Viewer.allows(Permission::SessionRead));
        assert!(!PrincipalRole::Viewer.allows(Permission::AgentSpawn));
        assert!(!PrincipalRole::Viewer.allows(Permission::AuditExport));
    }

    #[test]
    fn default_role_is_most_restrictive() {
        assert_eq!(PrincipalRole::default(), PrincipalRole::Viewer);
        assert_eq!(PrincipalRole::Viewer.rank(), 1);
        assert_eq!(PrincipalRole::Admin.rank(), 4);
    }

    #[test]
    fn deny_first_merge_picks_more_restrictive_role() {
        assert_eq!(
            PrincipalRole::Admin.merge_deny_first(PrincipalRole::Viewer),
            PrincipalRole::Viewer
        );
        assert_eq!(
            PrincipalRole::Viewer.merge_deny_first(PrincipalRole::Admin),
            PrincipalRole::Viewer
        );
        assert_eq!(
            PrincipalRole::User.merge_deny_first(PrincipalRole::Service),
            PrincipalRole::Service
        );
        assert_eq!(
            PrincipalRole::User.merge_deny_first(PrincipalRole::User),
            PrincipalRole::User
        );
    }

    #[test]
    fn effective_role_merges_deny_first() {
        let mut profile = PermissionProfile {
            default_role: Some(PrincipalRole::Admin),
            principal_roles: BTreeMap::new(),
        };
        assert_eq!(
            profile.effective_role(&PrincipalId::new("p-1")),
            Some(PrincipalRole::Admin)
        );
        // 显式更宽松的 User 与更严格的默认 Service 合并 → Service。
        profile
            .principal_roles
            .insert(PrincipalId::new("p-1"), PrincipalRole::User);
        profile.default_role = Some(PrincipalRole::Service);
        assert_eq!(
            profile.effective_role(&PrincipalId::new("p-1")),
            Some(PrincipalRole::Service)
        );
        // 无默认、未命中 → None（调用方按 Viewer 处理）。
        let empty = PermissionProfile::default();
        assert_eq!(empty.effective_role(&PrincipalId::new("p-1")), None);
    }

    #[test]
    fn role_and_permission_strings_are_stable() {
        for (role, wire) in [
            (PrincipalRole::Admin, "admin"),
            (PrincipalRole::User, "user"),
            (PrincipalRole::Service, "service"),
            (PrincipalRole::Viewer, "viewer"),
        ] {
            assert_eq!(role.as_str(), wire);
            let json = serde_json::to_string(&role).expect("serialize role");
            assert_eq!(json, format!("\"{wire}\""));
            assert_eq!(
                serde_json::from_str::<PrincipalRole>(&json).expect("deserialize role"),
                role
            );
        }
        assert_eq!(Permission::SessionRead.as_str(), "session_read");
        assert_eq!(Permission::PolicyManage.as_str(), "policy_manage");
    }

    #[test]
    fn profile_round_trips_json_with_principals() {
        let profile = PermissionProfile {
            default_role: Some(PrincipalRole::Service),
            principal_roles: [
                (PrincipalId::new("ops:ci"), PrincipalRole::Service),
                (PrincipalId::new("human:lead"), PrincipalRole::Admin),
            ]
            .into_iter()
            .collect(),
        };
        let json = serde_json::to_string(&profile).expect("serialize");
        let decoded: PermissionProfile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, profile);
    }
}
