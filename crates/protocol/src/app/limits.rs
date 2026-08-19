//! Tenant Policy / RBAC 协议镜像。

use pawork_domain::{AccountId, ModelId, ProviderId, TenantId};
use serde::{Deserialize, Serialize};
#[cfg(feature = "typegen")]
use ts_rs::TS;

// =========================================================================
// Tenant Policy / RBAC（P18-9）：protocol 镜像 + 脱敏决策事件视图。
// =========================================================================
//
// 这些类型是 tenant-service PolicySet / PrincipalRole / PolicyDecisionEvent
// 的协议镜像：core-api 不依赖 tenant-service，但 serde 形态保持一致，
// app-service 在边界做 1:1 转换。视图永不包含 Secret；决策 reason 在
// tenant-service 构造时已完成脱敏，此处只透传。

/// 主体最小角色（与 tenant-service `PrincipalRole` 对齐）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum PrincipalRole {
    /// 租户管理员：全部权限，含 audit 导出与策略管理。
    Admin,
    /// 普通用户：操作与读自己的 session / usage / audit。
    #[default]
    User,
    /// 服务账号：执行与 usage 对账，不读内容与 audit。
    Service,
    /// 只读观察者：只读自己的 session / usage。
    Viewer,
}

/// 策略闸口（与 tenant-service `PolicyGate` 对齐）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum PolicyGate {
    /// route candidate 过滤。
    RouteCandidate,
    /// credential lease 申请。
    LeaseAcquire,
    /// Agent spawn 准入。
    AgentSpawn,
    /// 请求并发准入。
    RequestAdmission,
    /// Session 查询。
    SessionQuery,
    /// Usage 查询。
    UsageQuery,
    /// Audit 查询。
    AuditQuery,
    /// Audit 导出。
    AuditExport,
    /// Retention（保留期）判定。
    Retention,
}

/// 决策种类：allow / deny / limit / fallback。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecisionKind {
    #[default]
    Allow,
    Deny,
    Limit,
    Fallback,
}

/// Audit 导出策略视图（deny-first：未启用一律拒绝）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct AuditExportPolicyView {
    /// 是否启用导出（默认关闭）。
    #[serde(default)]
    pub enabled: bool,
    /// 允许的导出目标（空列表 = 无任何目标可导出）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_destinations: Vec<String>,
}

/// 单条 principal → role 绑定（TS 友好的 Vec 形态，替代 map）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct PrincipalRoleBinding {
    pub principal_id: String,
    pub role: PrincipalRole,
}

/// 权限配置视图：默认角色 + 按 principal 覆盖。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct PermissionProfileView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_role: Option<PrincipalRole>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub principal_roles: Vec<PrincipalRoleBinding>,
}

/// 租户策略视图（deny-first PolicySet 镜像）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct TenantPolicyView {
    pub tenant_id: TenantId,
    /// 策略版本（每次更新递增；未知租户为 0）。
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_agents: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_input_token_budget: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_output_token_budget: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_cost_micros_budget: Option<u64>,
    /// Provider 白名单；`None` 表示不限制，`Some([])` 表示拒绝全部。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_providers: Option<Vec<ProviderId>>,
    /// 模型白名单；`None` 表示不限制，`Some([])` 表示拒绝全部。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_models: Option<Vec<ModelId>>,
    /// 账号白名单；`None` 表示不限制，`Some([])` 表示拒绝全部。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_accounts: Option<Vec<AccountId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_profile: Option<PermissionProfileView>,
    /// 保留天数；`None` 永久保留。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_export: Option<AuditExportPolicyView>,
}

/// 版本化、脱敏的决策事件视图（审计读取输出）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct PolicyDecisionEventView {
    /// 决策发生时生效的策略版本。
    pub policy_version: u64,
    pub tenant_id: TenantId,
    pub principal_id: String,
    pub gate: PolicyGate,
    pub decision: PolicyDecisionKind,
    /// 已脱敏的原因（永不含 Secret / 控制字符）。
    pub reason: String,
    pub at_ms: u64,
}
