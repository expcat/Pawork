//! Agent Profile v2（P17-5）：可复用 Agent 的完整配置领域类型。
//!
//! 纯领域数据：不执行 IO、不依赖 infra / 具体 Provider。覆盖 prompt / model /
//! canonical effort / tools（含显式 denied，deny 优先）/ skills / mcp /
//! permissions / hooks / memory / max turns / background / isolation 全维度。
//! 引用（skills / mcp / permissions / hooks）只表达 id + version pin，由加载
//! 方解析；profile 本身不携带明文 secret（v2 文件格式不存在 secret 字段）。

use serde::{Deserialize, Serialize};

use crate::{MemoryPrivacy, ReasoningEffort};

/// Profile 对单个工具名的策略裁决（deny 优先、fail-closed）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicyDecision {
    /// 命中显式 denied 清单，任何方式都不可执行。
    Denied,
    /// 命中显式 allowed 清单且未被 denied。
    Allowed,
    /// 未声明约束（profile 不限制，交由运行时 Policy 判定）。
    Unrestricted,
}

/// Profile 声明的工具规则：显式 allowed 与显式 denied 清单。
///
/// deny 优先：同一工具名同时出现在两清单时按 denied 处理，allowed 无法绕过
/// denied。allowed 为空表示「profile 不施加白名单约束」；denied 为空表示
/// 「无显式禁用」。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileToolRules {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied: Vec<String>,
}

impl ProfileToolRules {
    pub fn is_denied(&self, name: &str) -> bool {
        self.denied.iter().any(|item| item == name)
    }

    /// 是否被 profile 显式允许。与 [`Self::policy`] 一样始终 deny-first：
    /// 同名工具同时出现在 allowed / denied 时返回 `false`，调用方不能通过
    /// 这个便捷 API 绕过 denied。
    pub fn is_allowed(&self, name: &str) -> bool {
        !self.is_denied(name) && self.allowed.iter().any(|item| item == name)
    }

    /// deny 优先：先查 denied，再查 allowed，均未命中则不受 profile 约束。
    pub fn policy(&self, name: &str) -> ToolPolicyDecision {
        if self.is_denied(name) {
            return ToolPolicyDecision::Denied;
        }
        if self.is_allowed(name) {
            return ToolPolicyDecision::Allowed;
        }
        ToolPolicyDecision::Unrestricted
    }
}

/// Profile 对子系统资源的引用：id + 可选 version pin（semver 需求或 `*`）。
///
/// 解析失败或越权由加载 / 消费方按 fail-closed 降级或报错；本类型只描述引用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl ProfileRef {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version: None,
        }
    }

    /// version pin 是否约束具体版本：`None` / `*` / `latest` 表示任意版本。
    pub fn pins_version(&self) -> bool {
        matches!(self.version.as_deref(), Some(v) if v != "*" && v != "latest")
    }
}

/// Prompt 维度：system 必填，instructions 可选。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilePrompt {
    pub system: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Model 维度：provider + model name 引用，均由调用方解析为具体模型。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileModel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// 长期记忆可用性（P16-7 / P16-10）。
///
/// `enabled` 默认关（default-off）。`unavailable` 为显式不可用标注：存在时
/// 无论 `enabled` 如何都按不可用处理（fail-closed），绝不虚假可用。生产记忆
/// 接线（真实 EmbeddingProvider + SQLite 持久化 + context-engine 消费）完成前，
/// 加载方会把 `enabled=true` 显式解析为 `Unavailable` 并给出原因。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileMemory {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub privacy: MemoryPrivacy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

impl ProfileMemory {
    pub fn availability(&self) -> ProfileMemoryAvailability {
        if self.unavailable.is_some() {
            return ProfileMemoryAvailability::Unavailable;
        }
        if self.enabled {
            ProfileMemoryAvailability::Enabled
        } else {
            ProfileMemoryAvailability::Disabled
        }
    }
}

/// 记忆显式状态：`Unavailable` 携带原因，绝不与「可用」混淆。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileMemoryAvailability {
    Enabled,
    Disabled,
    Unavailable,
}

/// 运行隔离等级（P11 sandbox）：none / restricted / container。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileIsolation {
    /// 不额外隔离，由运行时默认策略决定。
    #[default]
    None,
    /// 受限模式：本机进程级沙箱（文件系统 / 网络 / 进程约束）。
    Restricted,
    /// 容器级隔离。
    Container,
}

/// Agent Profile v2：可复用 Agent 的完整配置档案。
///
/// `effort` 为 canonical 一等字段（[`ReasoningEffort`]），经 P15-8 协商翻译，
/// 不走 `provider_options`；本类型不含任何 Provider-specific reasoning 字段。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfileV2 {
    pub name: String,
    pub prompt: ProfilePrompt,
    #[serde(default)]
    pub model: ProfileModel,
    #[serde(default)]
    pub effort: ReasoningEffort,
    #[serde(default)]
    pub tools: ProfileToolRules,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<ProfileRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp: Vec<ProfileRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<ProfileRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<ProfileRef>,
    #[serde(default)]
    pub memory: ProfileMemory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u64>,
    #[serde(default)]
    pub background: bool,
    #[serde(default)]
    pub isolation: ProfileIsolation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_rules_are_deny_first_and_fail_closed() {
        let rules = ProfileToolRules {
            allowed: vec!["read_file".into(), "shell".into()],
            denied: vec!["shell".into()],
        };
        assert_eq!(rules.policy("shell"), ToolPolicyDecision::Denied);
        assert!(rules.is_denied("shell"));
        assert!(!rules.is_allowed("shell"));
        assert_eq!(rules.policy("read_file"), ToolPolicyDecision::Allowed);
        assert_eq!(rules.policy("untracked"), ToolPolicyDecision::Unrestricted);
        assert_eq!(
            ProfileToolRules::default().policy("anything"),
            ToolPolicyDecision::Unrestricted
        );
    }

    #[test]
    fn memory_is_default_off_and_explicitly_unavailable_when_marked() {
        let memory = ProfileMemory::default();
        assert_eq!(memory.availability(), ProfileMemoryAvailability::Disabled);

        let enabled = ProfileMemory {
            enabled: true,
            ..ProfileMemory::default()
        };
        assert_eq!(enabled.availability(), ProfileMemoryAvailability::Enabled);

        let unavailable = ProfileMemory {
            enabled: true,
            unavailable: Some("production memory not wired".into()),
            ..ProfileMemory::default()
        };
        assert_eq!(
            unavailable.availability(),
            ProfileMemoryAvailability::Unavailable
        );
    }

    #[test]
    fn profile_v2_round_trip_covers_all_dimensions() {
        let profile = AgentProfileV2 {
            name: "reviewer".into(),
            prompt: ProfilePrompt {
                system: "You are a careful reviewer.".into(),
                instructions: Some("Prefer minimal diffs.".into()),
            },
            model: ProfileModel {
                provider: Some("default".into()),
                name: Some("review-model".into()),
            },
            effort: ReasoningEffort::High,
            tools: ProfileToolRules {
                allowed: vec!["read_file".into()],
                denied: vec!["shell".into()],
            },
            skills: vec![ProfileRef {
                id: "rust".into(),
                version: Some("^1.2.0".into()),
            }],
            mcp: vec![ProfileRef::new("filesystem")],
            permissions: vec![ProfileRef::new("read-only")],
            hooks: vec![ProfileRef::new("on-completion")],
            memory: ProfileMemory {
                enabled: false,
                ..ProfileMemory::default()
            },
            max_turns: Some(120),
            background: true,
            isolation: ProfileIsolation::Restricted,
        };

        let encoded = serde_json::to_string(&profile).expect("serialize");
        let decoded: AgentProfileV2 = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, profile);
        // Canonical effort 序列化为稳定词汇，且无 provider 名分支字段。
        assert!(encoded.contains(r#""effort":"high""#));
        assert!(!encoded.contains("reasoning_effort"));
    }

    #[test]
    fn version_pin_semantics() {
        assert!(!ProfileRef::new("x").pins_version());
        assert!(!ProfileRef {
            id: "x".into(),
            version: Some("*".into()),
        }
        .pins_version());
        assert!(!ProfileRef {
            id: "x".into(),
            version: Some("latest".into()),
        }
        .pins_version());
        assert!(ProfileRef {
            id: "x".into(),
            version: Some("1.2.3".into()),
        }
        .pins_version());
    }
}
