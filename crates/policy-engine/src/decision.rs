//! 策略裁决结果与相关类型。

use serde::{Deserialize, Serialize};

/// 单条命令的静态风险等级（来自 [`crate::classify_command`]）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandRisk {
    #[default]
    Safe,
    Dangerous,
}

/// 提交给用户审批时的风险等级。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    #[default]
    Safe,
    Moderate,
    Dangerous,
}

/// 允许执行但附加的资源约束。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionConstraints {
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: Option<u64>,
}

/// 审批提示。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalPrompt {
    pub message: String,
    pub risk: RiskLevel,
}

/// 策略引擎对一次工具调用的裁决。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolicyDecision {
    /// 直接放行。
    Allow,
    /// 直接拒绝。
    Deny {
        /// 拒绝原因（可展示给用户/记入日志，禁止含 Secret）。
        reason: String,
    },
    /// 需要用户确认。
    AskUser { prompt: ApprovalPrompt },
    /// 放行但附加执行约束。
    AllowWithConstraints { constraints: ExecutionConstraints },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_roundtrips_with_kind_tag() {
        let dec = PolicyDecision::Deny {
            reason: "nope".into(),
        };
        let json = serde_json::to_string(&dec).expect("serialize");
        let back: PolicyDecision = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(dec, back);
        assert!(json.contains("\"kind\":\"deny\""));
    }

    #[test]
    fn ask_user_carries_risk() {
        let dec = PolicyDecision::AskUser {
            prompt: ApprovalPrompt {
                message: "ok?".into(),
                risk: RiskLevel::Moderate,
            },
        };
        let json = serde_json::to_string(&dec).expect("serialize");
        assert!(json.contains("\"kind\":\"ask_user\""));
        assert!(json.contains("\"moderate\""));
    }

    #[test]
    fn allow_with_constraints_tag() {
        let dec = PolicyDecision::AllowWithConstraints {
            constraints: ExecutionConstraints {
                timeout_ms: Some(1000),
                max_output_bytes: None,
            },
        };
        let json = serde_json::to_string(&dec).expect("serialize");
        assert!(json.contains("\"kind\":\"allow_with_constraints\""));
    }

    #[test]
    fn allow_has_no_payload() {
        let json = serde_json::to_string(&PolicyDecision::Allow).expect("serialize");
        assert_eq!(json, "{\"kind\":\"allow\"}");
    }

    #[test]
    fn risk_defaults_are_safe() {
        assert_eq!(RiskLevel::default(), RiskLevel::Safe);
        assert_eq!(CommandRisk::default(), CommandRisk::Safe);
    }
}
