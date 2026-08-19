//! 版本化、脱敏的租户策略决策事件（P18-9）。
//!
//! 决策事件是审计的事实源：每条事件携带策略版本、闸口、决策种类与**脱敏后的**
//! 原因。脱敏在构造入口统一执行（[`sanitize_reason`]），调用方不得自行绕过；
//! reason 永不包含 Secret（明文 Token / API Key / Cookie）与控制字符，长度有界。

use serde::{Deserialize, Serialize};

use crate::identity::IdentityContext;

/// 策略闸口：一次决策发生在哪个强制入口。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyGate {
    /// route candidate 过滤（provider-control routing 注入的 tenant policy）。
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

impl PolicyGate {
    /// 冻结的持久化字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RouteCandidate => "route_candidate",
            Self::LeaseAcquire => "lease_acquire",
            Self::AgentSpawn => "agent_spawn",
            Self::RequestAdmission => "request_admission",
            Self::SessionQuery => "session_query",
            Self::UsageQuery => "usage_query",
            Self::AuditQuery => "audit_query",
            Self::AuditExport => "audit_export",
            Self::Retention => "retention",
        }
    }
}

/// 决策种类：allow / deny / limit / fallback。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecisionKind {
    /// 放行。
    Allow,
    /// 拒绝（deny-first，任何一层拒绝不可被覆盖）。
    Deny,
    /// 放行但受约束（如并发 / 预算上限边界）。
    Limit,
    /// 放行但需要回退动作（如换 provider / 换模型）。
    Fallback,
}

impl PolicyDecisionKind {
    /// 冻结的持久化字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Limit => "limit",
            Self::Fallback => "fallback",
        }
    }
}

/// 单条决策事件：versioned + 脱敏 reason，可持久化、可重放。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecisionEvent {
    /// 决策发生时生效的策略版本（未知租户为 0）。
    pub policy_version: u64,
    /// 决策主体（操作人），与执行 Agent 分离。
    pub principal_id: pawork_domain::PrincipalId,
    /// 决策所属租户。
    pub tenant_id: pawork_domain::TenantId,
    /// 强制入口。
    pub gate: PolicyGate,
    /// 决策种类。
    pub decision: PolicyDecisionKind,
    /// 脱敏后的原因（构造时已 sanitize，永不含 Secret / 控制字符）。
    pub reason: String,
    /// 决策时间（unix millis）。
    pub at_ms: u64,
}

impl PolicyDecisionEvent {
    /// 构造决策事件：`reason` 统一经 [`sanitize_reason`] 脱敏。
    pub fn new(
        policy_version: u64,
        identity: &IdentityContext,
        gate: PolicyGate,
        decision: PolicyDecisionKind,
        reason: impl Into<String>,
        at_ms: u64,
    ) -> Self {
        Self {
            policy_version,
            principal_id: identity.principal_id.clone(),
            tenant_id: identity.tenant_id.clone(),
            gate,
            decision,
            reason: sanitize_reason(&reason.into()),
            at_ms,
        }
    }

    /// 从 `PolicyDecision` 归一化为决策种类。
    pub fn kind_of(decision: &crate::PolicyDecision) -> PolicyDecisionKind {
        match decision {
            crate::PolicyDecision::Allow => PolicyDecisionKind::Allow,
            crate::PolicyDecision::Deny { .. } => PolicyDecisionKind::Deny,
            crate::PolicyDecision::Limit { .. } => PolicyDecisionKind::Limit,
            crate::PolicyDecision::Fallback { .. } => PolicyDecisionKind::Fallback,
        }
    }
}

/// 决策原因脱敏：控制字符替换为空格、连续空白折叠、疑似凭证串掩码、
/// 截断到有界长度。确定性：同一输入必得同一输出。
pub fn sanitize_reason(reason: &str) -> String {
    const MAX_LEN: usize = 512;
    const SECRET_LIKE_RUN: usize = 20;

    let mut output = String::with_capacity(reason.len().min(MAX_LEN));
    for ch in reason.chars() {
        output.push(if ch.is_control() { ' ' } else { ch });
    }

    // 掩码疑似凭证串：连续的 [A-Za-z0-9_-] 长度 >= 20。
    let mut masked = String::with_capacity(output.len());
    let bytes = output.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_' || bytes[index] == b'-' {
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric()
                    || bytes[index] == b'_'
                    || bytes[index] == b'-')
            {
                index += 1;
            }
            let run = &output[start..index];
            if run.chars().count() >= SECRET_LIKE_RUN {
                masked.push_str("***");
            } else {
                masked.push_str(run);
            }
        } else {
            let ch = output[index..].chars().next().expect("non-empty byte");
            masked.push(ch);
            index += ch.len_utf8();
        }
    }

    // 折叠连续空白。
    let collapsed = masked.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_LEN {
        collapsed
    } else {
        let mut truncated: String = collapsed.chars().take(MAX_LEN).collect();
        truncated.push('…');
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IdentityContext, PolicyDecision, PolicyGate};

    #[test]
    fn gate_and_kind_strings_are_stable() {
        assert_eq!(PolicyGate::AgentSpawn.as_str(), "agent_spawn");
        assert_eq!(PolicyGate::AuditExport.as_str(), "audit_export");
        assert_eq!(PolicyDecisionKind::Deny.as_str(), "deny");
        assert_eq!(PolicyDecisionKind::Fallback.as_str(), "fallback");
        assert_eq!(
            PolicyDecisionEvent::kind_of(&PolicyDecision::Allow),
            PolicyDecisionKind::Allow
        );
        assert_eq!(
            PolicyDecisionEvent::kind_of(&PolicyDecision::Limit { reason: "x".into() }),
            PolicyDecisionKind::Limit
        );
        assert_eq!(
            PolicyDecisionEvent::kind_of(&PolicyDecision::Deny { reason: "x".into() }),
            PolicyDecisionKind::Deny
        );
        assert_eq!(
            PolicyDecisionEvent::kind_of(&PolicyDecision::Fallback { reason: "x".into() }),
            PolicyDecisionKind::Fallback
        );
    }

    #[test]
    fn sanitize_reason_strips_control_chars_and_collapses_whitespace() {
        assert_eq!(
            sanitize_reason("model\0gpt-4o\n\t不在\n允许列表内"),
            "model gpt-4o 不在 允许列表内"
        );
    }

    #[test]
    fn sanitize_reason_masks_secret_like_runs() {
        let sanitized =
            sanitize_reason("credential sk-abcdefghijklmnopqrstuvwxyz-1234567890 leaked");
        assert!(
            !sanitized.contains("abcdefghijklmnopqrstuvwxyz"),
            "secret-like run must be masked: {sanitized}"
        );
        assert!(sanitized.contains("***"));
        // 短 run（普通模型名）不被误伤。
        assert_eq!(
            sanitize_reason("模型 claude-3-5-sonnet 不在允许列表内"),
            "模型 claude-3-5-sonnet 不在允许列表内"
        );
    }

    #[test]
    fn sanitize_reason_truncates_bounded() {
        // 用短词 + 空格构造超长输入：不会被当作疑似凭证串整体掩码，
        // 才能走到长度截断分支。
        let long = "word ".repeat(2048);
        let sanitized = sanitize_reason(&long);
        assert!(sanitized.chars().count() <= 513, "512 + 省略号");
        assert!(sanitized.ends_with('…'));
    }

    #[test]
    fn decision_event_sanitizes_reason_at_construction() {
        let identity = IdentityContext::local();
        let event = PolicyDecisionEvent::new(
            3,
            &identity,
            PolicyGate::LeaseAcquire,
            PolicyDecisionKind::Deny,
            "provider sk-abcdefghijklmnopqrstuvwxyz not allowed\n",
            42,
        );
        assert_eq!(event.policy_version, 3);
        assert_eq!(event.tenant_id, identity.tenant_id);
        assert_eq!(event.principal_id, identity.principal_id);
        assert_eq!(event.gate, PolicyGate::LeaseAcquire);
        assert_eq!(event.decision, PolicyDecisionKind::Deny);
        assert_eq!(event.at_ms, 42);
        assert!(!event.reason.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!event.reason.contains('\n'));
    }

    #[test]
    fn decision_event_round_trips_json() {
        let event = PolicyDecisionEvent::new(
            1,
            &IdentityContext::local(),
            PolicyGate::UsageQuery,
            PolicyDecisionKind::Limit,
            "预算超限",
            7,
        );
        let json = serde_json::to_string(&event).expect("serialize");
        let decoded: PolicyDecisionEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, event);
    }
}
