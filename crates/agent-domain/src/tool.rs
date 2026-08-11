//! Canonical Tool v2（P15-1）：三类执行位点统一。
//!
//! 本模块是纯领域数据：不执行 IO，不依赖任何具体 Provider。执行位点语义：
//! - [`ToolKind::ClientFunction`]：由 Core 本地执行（如 read_file），唯一由
//!   Pawork 本地执行的位点；
//! - [`ToolKind::ProviderHosted`]：由 Provider 服务端执行（如 web_search），
//!   Pawork 只记录 / 归一 / 重放；
//! - [`ToolKind::ProviderExtension`]：由 Provider 中介的外部工具执行，拥有
//!   显式 approval / audit / execution ownership。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 工具执行位点（P15-1）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Core 本地执行（read_file 等）。旧数据缺省即此值。
    #[default]
    ClientFunction,
    /// Provider 服务端内置工具（web_search 等）。
    ProviderHosted,
    /// Provider 中介的外部工具（MCP / Connector / Remote extension）。
    ProviderExtension,
}

impl ToolKind {
    /// 「谁执行」与 [`ExecutionOwner`] 一一对应。
    pub const fn execution_owner(self) -> ExecutionOwner {
        match self {
            Self::ClientFunction => ExecutionOwner::Core,
            Self::ProviderHosted => ExecutionOwner::Provider,
            Self::ProviderExtension => ExecutionOwner::Extension,
        }
    }

    /// 按执行位点推导唯一合法的续接方式。
    ///
    /// `ToolResult` 只属于 `ClientFunction`；Provider-owned 工具只能经
    /// Provider transcript 续接，调用方不能在结果对象上覆写该语义。
    pub const fn continuation_mode(self) -> ContinuationMode {
        match self {
            Self::ClientFunction => ContinuationMode::CoreSuppliedResult,
            Self::ProviderHosted | Self::ProviderExtension => ContinuationMode::ProviderTranscript,
        }
    }
}

/// 工具执行所有权（与 [`ToolKind`] 一一对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOwner {
    /// Pawork Core 本地执行。
    Core,
    /// Provider 服务端执行。
    Provider,
    /// Provider 中介的外部扩展执行。
    Extension,
}

/// 工具结果的续接方式。
///
/// [`ContinuationMode::CoreSuppliedResult`] 是唯一由适配器翻译为 Provider
/// function-result 字段的路径；hosted / extension 的结果走
/// [`ContinuationMode::ProviderTranscript`]（P15-5 `ServerToolEvent` / Provider
/// transcript envelope），不得伪装成本地 `ToolResult`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationMode {
    /// Core 本地执行结果，由适配器翻译为 Provider 的 function-result 字段。
    #[default]
    CoreSuppliedResult,
    /// Provider 原生 output item / cursor / transcript reference。
    ProviderTranscript,
}

/// P15 能力标签（供 P15-8 协商；reasoning effort 与 citation/source 由同一
/// vocabulary 表达）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCapabilityTag {
    WebSearch,
    WebFetch,
    FileOrCollectionSearch,
    XSearch,
    CodeExecution,
    HostedShell,
    ProviderApplyPatch,
    ComputerUse,
    ImageGeneration,
    ServerSideMcp,
    ToolSearch,
    Memory,
    ProgrammaticToolCalling,
    ServerSideMultiAgent,
}

/// 执行位点细节（`ToolDescriptor.hosting`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolHosting {
    /// ClientFunction：Core 本地执行，无附加位点信息。
    #[default]
    Local,
    /// ProviderHosted：Provider 服务端内置工具。
    ProviderHosted {
        /// Provider 侧工具名（如 `web_search`）。
        hosted_name: String,
        /// 服务端工具类别（如 WebSearch）。
        kind: ToolCapabilityTag,
    },
    /// ProviderExtension：Provider 中介的外部工具引用。
    ProviderExtension {
        /// 外部工具引用（MCP server / connector / remote endpoint）。
        reference: String,
    },
}

impl ToolHosting {
    /// hosting 结构自身声明的 canonical 执行位点。
    pub const fn tool_kind(&self) -> ToolKind {
        match self {
            Self::Local => ToolKind::ClientFunction,
            Self::ProviderHosted { .. } => ToolKind::ProviderHosted,
            Self::ProviderExtension { .. } => ToolKind::ProviderExtension,
        }
    }
}

/// 调度分类（P3-4 既有语义不变；ClientFunction 工具沿用）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCapability {
    ReadOnly,
    WorkspaceWrite,
    GitWrite,
    Process,
    Network,
    UserInteraction,
    ExternalPlugin,
}

impl ToolCapability {
    pub const fn permits_concurrent_execution(&self) -> bool {
        matches!(self, Self::ReadOnly)
    }
}

/// ToolDescriptor v2（P15-1）：三类执行位点在同一个 registry 共存。
///
/// ClientFunction 既有字段语义不变；新增字段均带 serde 默认值，旧序列化数据
/// 可无损反序列化（缺省即 ClientFunction / Local / 空能力 / 不强制审批）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// 调度分类（并发 / 串行 / 审批策略依据）。
    pub capability: ToolCapability,
    /// 执行位点。
    #[serde(default)]
    pub kind: ToolKind,
    /// 执行位点细节（hosted 工具名、extension 引用等）。
    #[serde(default)]
    pub hosting: ToolHosting,
    /// 能力标签（供 P15-8 协商）。
    #[serde(default)]
    pub capabilities: Vec<ToolCapabilityTag>,
    /// 是否要求显式审批（与 PolicyEngine 对齐；缺省按 capability 策略）。
    #[serde(default)]
    pub requires_approval: bool,
    pub read_only: bool,
    pub supports_concurrency: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_timeout_ms: Option<u64>,
    pub max_output_bytes: u64,
    #[serde(default)]
    pub allowed_in_untrusted_workspace: bool,
}

impl ToolDescriptor {
    /// 续接方式完全由 canonical kind 推导，不能由结果对象覆写。
    pub const fn continuation_mode(&self) -> ContinuationMode {
        self.kind.continuation_mode()
    }

    /// `kind` 与 `hosting` 必须描述同一执行位点。
    pub fn has_consistent_hosting(&self) -> bool {
        self.kind == self.hosting.tool_kind()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_kind_owner_mapping_is_one_to_one() {
        assert_eq!(
            ToolKind::ClientFunction.execution_owner(),
            ExecutionOwner::Core
        );
        assert_eq!(
            ToolKind::ProviderHosted.execution_owner(),
            ExecutionOwner::Provider
        );
        assert_eq!(
            ToolKind::ProviderExtension.execution_owner(),
            ExecutionOwner::Extension
        );
    }

    #[test]
    fn tool_kind_serde_round_trip_uses_snake_case() {
        for (kind, wire) in [
            (ToolKind::ClientFunction, "client_function"),
            (ToolKind::ProviderHosted, "provider_hosted"),
            (ToolKind::ProviderExtension, "provider_extension"),
        ] {
            let value = serde_json::to_value(kind).expect("serialize kind");
            assert_eq!(value, serde_json::json!(wire));
            let decoded: ToolKind = serde_json::from_value(value).expect("deserialize kind");
            assert_eq!(decoded, kind);
        }
    }

    #[test]
    fn execution_owner_and_continuation_mode_serde_round_trip() {
        for owner in [
            ExecutionOwner::Core,
            ExecutionOwner::Provider,
            ExecutionOwner::Extension,
        ] {
            let value = serde_json::to_value(owner).expect("serialize owner");
            let decoded: ExecutionOwner = serde_json::from_value(value).expect("deserialize owner");
            assert_eq!(decoded, owner);
        }
        for mode in [
            ContinuationMode::CoreSuppliedResult,
            ContinuationMode::ProviderTranscript,
        ] {
            let value = serde_json::to_value(mode).expect("serialize mode");
            let decoded: ContinuationMode =
                serde_json::from_value(value).expect("deserialize mode");
            assert_eq!(decoded, mode);
        }

        assert_eq!(
            ToolKind::ClientFunction.continuation_mode(),
            ContinuationMode::CoreSuppliedResult
        );
        assert_eq!(
            ToolKind::ProviderHosted.continuation_mode(),
            ContinuationMode::ProviderTranscript
        );
        assert_eq!(
            ToolKind::ProviderExtension.continuation_mode(),
            ContinuationMode::ProviderTranscript
        );
    }

    #[test]
    fn capability_tag_vocabulary_covers_p15_surface() {
        let tags = vec![
            ToolCapabilityTag::WebSearch,
            ToolCapabilityTag::WebFetch,
            ToolCapabilityTag::FileOrCollectionSearch,
            ToolCapabilityTag::XSearch,
            ToolCapabilityTag::CodeExecution,
            ToolCapabilityTag::HostedShell,
            ToolCapabilityTag::ProviderApplyPatch,
            ToolCapabilityTag::ComputerUse,
            ToolCapabilityTag::ImageGeneration,
            ToolCapabilityTag::ServerSideMcp,
            ToolCapabilityTag::ToolSearch,
            ToolCapabilityTag::Memory,
            ToolCapabilityTag::ProgrammaticToolCalling,
            ToolCapabilityTag::ServerSideMultiAgent,
        ];
        let json = serde_json::to_value(tags.clone()).expect("serialize tags");
        assert_eq!(json.as_array().map(Vec::len), Some(14));
        let decoded: Vec<ToolCapabilityTag> =
            serde_json::from_value(json).expect("deserialize tags");
        assert_eq!(decoded, tags);
    }

    #[test]
    fn legacy_descriptor_json_without_v2_fields_defaults_to_client_function() {
        let legacy = serde_json::json!({
            "name": "read_file",
            "description": "read",
            "input_schema": {"type": "object"},
            "capability": "read_only",
            "read_only": true,
            "supports_concurrency": true,
            "max_output_bytes": 1024,
            "allowed_in_untrusted_workspace": true
        });
        let descriptor: ToolDescriptor =
            serde_json::from_value(legacy).expect("legacy descriptor must deserialize");
        assert_eq!(descriptor.kind, ToolKind::ClientFunction);
        assert_eq!(descriptor.hosting, ToolHosting::Local);
        assert!(descriptor.capabilities.is_empty());
        assert!(!descriptor.requires_approval);
    }

    #[test]
    fn descriptor_v2_round_trip_keeps_kind_hosting_and_capabilities() {
        let descriptor = ToolDescriptor {
            name: "web_search".into(),
            description: "provider-hosted search".into(),
            input_schema: serde_json::json!({"type": "object"}),
            capability: ToolCapability::Network,
            kind: ToolKind::ProviderHosted,
            hosting: ToolHosting::ProviderHosted {
                hosted_name: "web_search".into(),
                kind: ToolCapabilityTag::WebSearch,
            },
            capabilities: vec![ToolCapabilityTag::WebSearch],
            requires_approval: false,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: None,
            max_output_bytes: 64 * 1024,
            allowed_in_untrusted_workspace: true,
        };
        let value = serde_json::to_value(&descriptor).expect("serialize descriptor");
        assert_eq!(value["kind"], "provider_hosted");
        assert_eq!(value["hosting"]["type"], "provider_hosted");
        assert_eq!(value["hosting"]["hosted_name"], "web_search");
        assert_eq!(value["capabilities"], serde_json::json!(["web_search"]));
        assert!(descriptor.has_consistent_hosting());
        assert_eq!(
            descriptor.continuation_mode(),
            ContinuationMode::ProviderTranscript
        );
        let decoded: ToolDescriptor =
            serde_json::from_value(value).expect("deserialize descriptor");
        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn descriptor_rejects_kind_hosting_mismatch() {
        let descriptor = ToolDescriptor {
            name: "bad".into(),
            description: String::new(),
            input_schema: Value::Null,
            capability: ToolCapability::Network,
            kind: ToolKind::ProviderHosted,
            hosting: ToolHosting::Local,
            capabilities: Vec::new(),
            requires_approval: false,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: None,
            max_output_bytes: 1024,
            allowed_in_untrusted_workspace: false,
        };
        assert!(!descriptor.has_consistent_hosting());
    }
}
