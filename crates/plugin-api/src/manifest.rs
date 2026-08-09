use std::collections::BTreeSet;

use agent_domain::PluginId;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use tool_api::ToolCapability;

use crate::PluginLifecycleEventKind;

pub const PLUGIN_API_VERSION: &str = "1.0.0";
pub const PLUGIN_INVOKE_EXPORT: &str = "invoke";
pub const MAX_PLUGIN_MANIFEST_BYTES: usize = 1024 * 1024;
pub const DEFAULT_PLUGIN_OUTPUT_BYTES: u64 = 1024 * 1024;

pub fn plugin_api_version() -> Version {
    Version::parse(PLUGIN_API_VERSION).expect("PLUGIN_API_VERSION must be valid semver")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: PluginId,
    pub name: String,
    pub version: Version,
    pub api_version: VersionReq,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub permissions: PluginPermissions,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<PluginCapability>,
    /// 插件希望通过受控宿主 API 使用的 canonical tool capability。
    /// 调度器仍会把插件提供的工具标记为 `ExternalPlugin`，本字段不能降低审批级别。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_capabilities: Vec<ToolCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<PluginToolRegistration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<PluginCommandRegistration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lifecycle_hooks: Vec<PluginLifecycleEventKind>,
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), ManifestValidationError> {
        validate_identifier("plugin id", self.id.as_str(), true)?;
        if self.name.trim().is_empty() {
            return Err(ManifestValidationError::EmptyName);
        }
        reject_duplicates("capability", &self.capabilities)?;
        reject_duplicates("tool capability", &self.tool_capabilities)?;
        reject_duplicates("lifecycle hook", &self.lifecycle_hooks)?;

        validate_paths("filesystem_read", &self.permissions.filesystem_read)?;
        validate_paths("filesystem_write", &self.permissions.filesystem_write)?;
        validate_network_hosts(&self.permissions.network)?;
        validate_refs(&self.permissions.secret_refs)?;

        validate_registrations("tool", self.tools.iter().map(|item| item.name.as_str()))?;
        validate_registrations(
            "command",
            self.commands.iter().map(|item| item.name.as_str()),
        )?;
        for tool in &self.tools {
            validate_schema("tool", &tool.name, &tool.input_schema)?;
            if tool.max_output_bytes == 0 {
                return Err(ManifestValidationError::InvalidLimit {
                    field: format!("tools.{}.max_output_bytes", tool.name),
                });
            }
        }
        for command in &self.commands {
            validate_schema("command", &command.name, &command.input_schema)?;
        }

        self.require_capability_for(
            !self.tools.is_empty(),
            PluginCapability::RegisterTool,
            "tools",
        )?;
        self.require_capability_for(
            !self.commands.is_empty(),
            PluginCapability::RegisterCommand,
            "commands",
        )?;
        self.require_capability_for(
            !self.lifecycle_hooks.is_empty(),
            PluginCapability::LifecycleHook,
            "lifecycle_hooks",
        )?;
        if self
            .tool_capabilities
            .contains(&ToolCapability::ExternalPlugin)
        {
            return Err(ManifestValidationError::ReservedToolCapability);
        }
        Ok(())
    }

    pub fn ensure_api_compatible(
        &self,
        host_api_version: &Version,
    ) -> Result<(), ManifestValidationError> {
        if self.api_version.matches(host_api_version) {
            Ok(())
        } else {
            Err(ManifestValidationError::IncompatibleApi {
                plugin_requirement: self.api_version.to_string(),
                host_version: host_api_version.to_string(),
            })
        }
    }

    /// 构造 Ed25519 的稳定签名消息，并把组件内容摘要绑定到 manifest。
    pub fn canonical_signing_payload(
        &self,
        component_bytes: &[u8],
    ) -> Result<Vec<u8>, ManifestValidationError> {
        self.validate()?;
        let value = serde_json::to_value(self)
            .map_err(|error| ManifestValidationError::Serialization(error.to_string()))?;
        let manifest_bytes = serde_json::to_vec(&canonicalize_json(value))
            .map_err(|error| ManifestValidationError::Serialization(error.to_string()))?;
        if manifest_bytes.len() > MAX_PLUGIN_MANIFEST_BYTES {
            return Err(ManifestValidationError::ManifestTooLarge);
        }
        let manifest_len = u64::try_from(manifest_bytes.len())
            .map_err(|_| ManifestValidationError::ManifestTooLarge)?;

        let mut payload = Vec::with_capacity(48 + manifest_bytes.len());
        payload.extend_from_slice(b"pawork.plugin.manifest.signature.v1\0");
        payload.extend_from_slice(&manifest_len.to_be_bytes());
        payload.extend_from_slice(&manifest_bytes);
        payload.extend_from_slice(blake3::hash(component_bytes).as_bytes());
        Ok(payload)
    }

    fn require_capability_for(
        &self,
        condition: bool,
        capability: PluginCapability,
        field: &'static str,
    ) -> Result<(), ManifestValidationError> {
        if condition && !self.capabilities.contains(&capability) {
            Err(ManifestValidationError::MissingCapability { field, capability })
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginPermissions {
    /// Workspace 相对路径或命名 scope；空列表表示无权限。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystem_read: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystem_write: Vec<String>,
    /// 允许访问的主机名；空列表表示无网络权限。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<String>,
    #[serde(default)]
    pub process: bool,
    /// Secret 引用名，不是明文 Secret。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_refs: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    RegisterTool,
    RegisterCommand,
    LifecycleHook,
    ModifyContext,
    CompactionStrategy,
    RegisterProvider,
    PersistentState,
    UserInteraction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginToolRegistration {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_timeout_ms: Option<u64>,
    #[serde(default = "default_plugin_output_bytes")]
    pub max_output_bytes: u64,
}

const fn default_plugin_output_bytes() -> u64 {
    DEFAULT_PLUGIN_OUTPUT_BYTES
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCommandRegistration {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSignatureAlgorithm {
    Ed25519,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSignature {
    pub algorithm: PluginSignatureAlgorithm,
    /// 宿主 trust store 中的 opaque key id，不是公钥本身。
    pub key_id: String,
    /// RFC 4648 Base64（无换行）的 64-byte Ed25519 signature。
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedPluginManifest {
    pub manifest: PluginManifest,
    pub signature: PluginSignature,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ManifestValidationError {
    #[error("plugin id must not be empty")]
    EmptyId,
    #[error("plugin id contains unsupported characters: {0}")]
    InvalidId(String),
    #[error("plugin name must not be empty")]
    EmptyName,
    #[error("duplicate {kind}: {value}")]
    Duplicate { kind: &'static str, value: String },
    #[error("invalid {kind} registration name: {name}")]
    InvalidRegistrationName { kind: &'static str, name: String },
    #[error("{kind} {name} input schema must be a JSON object")]
    InvalidInputSchema { kind: &'static str, name: String },
    #[error("invalid plugin permission {field}: {value}")]
    InvalidPermission { field: &'static str, value: String },
    #[error("{field} requires plugin capability {capability:?}")]
    MissingCapability {
        field: &'static str,
        capability: PluginCapability,
    },
    #[error("ExternalPlugin is reserved for the host scheduler")]
    ReservedToolCapability,
    #[error("plugin limit must be greater than zero: {field}")]
    InvalidLimit { field: String },
    #[error("plugin API requirement {plugin_requirement} does not include host {host_version}")]
    IncompatibleApi {
        plugin_requirement: String,
        host_version: String,
    },
    #[error("manifest is too large to sign")]
    ManifestTooLarge,
    #[error("manifest serialization failed: {0}")]
    Serialization(String),
}

fn reject_duplicates<T>(kind: &'static str, values: &[T]) -> Result<(), ManifestValidationError>
where
    T: std::fmt::Debug + Ord,
{
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(ManifestValidationError::Duplicate {
                kind,
                value: format!("{value:?}"),
            });
        }
    }
    Ok(())
}

fn validate_identifier(
    kind: &'static str,
    value: &str,
    allow_dots: bool,
) -> Result<(), ManifestValidationError> {
    if value.is_empty() {
        return if kind == "plugin id" {
            Err(ManifestValidationError::EmptyId)
        } else {
            Err(ManifestValidationError::InvalidRegistrationName {
                kind,
                name: value.to_string(),
            })
        };
    }
    let valid = value.len() <= 128
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || (allow_dots && ch == '.')
        })
        && value
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && value
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && (!allow_dots || !value.contains(".."));
    if valid {
        Ok(())
    } else if kind == "plugin id" {
        Err(ManifestValidationError::InvalidId(value.to_string()))
    } else {
        Err(ManifestValidationError::InvalidRegistrationName {
            kind,
            name: value.to_string(),
        })
    }
}

fn validate_registrations<'a>(
    kind: &'static str,
    names: impl Iterator<Item = &'a str>,
) -> Result<(), ManifestValidationError> {
    let mut seen = BTreeSet::new();
    for name in names {
        validate_identifier(kind, name, false)?;
        if !seen.insert(name) {
            return Err(ManifestValidationError::Duplicate {
                kind,
                value: name.to_string(),
            });
        }
    }
    Ok(())
}

fn validate_schema(
    kind: &'static str,
    name: &str,
    schema: &Value,
) -> Result<(), ManifestValidationError> {
    if schema.is_object() {
        Ok(())
    } else {
        Err(ManifestValidationError::InvalidInputSchema {
            kind,
            name: name.to_string(),
        })
    }
}

fn validate_paths(field: &'static str, values: &[String]) -> Result<(), ManifestValidationError> {
    let mut seen = BTreeSet::new();
    for value in values {
        let invalid = value.is_empty()
            || value.len() > 1024
            || value.starts_with('/')
            || value.starts_with('\\')
            || value.starts_with('~')
            || value.contains(':')
            || value
                .split(['/', '\\'])
                .any(|part| part.is_empty() || part == "." || part == "..");
        if invalid || !seen.insert(value) {
            return Err(ManifestValidationError::InvalidPermission {
                field,
                value: value.clone(),
            });
        }
    }
    Ok(())
}

fn validate_network_hosts(values: &[String]) -> Result<(), ManifestValidationError> {
    let mut seen = BTreeSet::new();
    for value in values {
        let invalid = value.is_empty()
            || value.len() > 253
            || value.contains("//")
            || value.contains('/')
            || value.contains('\\')
            || value.chars().any(char::is_whitespace)
            || !value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '*' | ':'));
        if invalid || !seen.insert(value) {
            return Err(ManifestValidationError::InvalidPermission {
                field: "network",
                value: value.clone(),
            });
        }
    }
    Ok(())
}

fn validate_refs(values: &[String]) -> Result<(), ManifestValidationError> {
    let mut seen = BTreeSet::new();
    for value in values {
        let invalid = value.is_empty()
            || value.len() > 128
            || !value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'));
        if invalid || !seen.insert(value) {
            return Err(ManifestValidationError::InvalidPermission {
                field: "secret_refs",
                value: value.clone(),
            });
        }
    }
    Ok(())
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let mut sorted = Map::new();
            for (key, value) in entries {
                sorted.insert(key, canonicalize_json(value));
            }
            Value::Object(sorted)
        }
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::from("example.plugin"),
            name: "Example".into(),
            version: Version::new(1, 2, 0),
            api_version: VersionReq::parse(">=1, <2").expect("valid requirement"),
            description: None,
            permissions: PluginPermissions {
                filesystem_read: vec!["workspace".into()],
                network: vec!["api.example.com".into()],
                ..PluginPermissions::default()
            },
            capabilities: vec![PluginCapability::RegisterTool],
            tool_capabilities: vec![ToolCapability::ReadOnly],
            tools: vec![PluginToolRegistration {
                name: "echo".into(),
                description: "Echo input".into(),
                input_schema: serde_json::json!({"properties": {"value": {"type": "string"}}, "type": "object"}),
                default_timeout_ms: Some(1_000),
                max_output_bytes: 1024,
            }],
            commands: Vec::new(),
            lifecycle_hooks: Vec::new(),
        }
    }

    #[test]
    fn manifest_round_trip_preserves_registrations() {
        let manifest = manifest();
        let encoded = serde_json::to_string(&manifest).expect("serialize manifest");
        let decoded: PluginManifest = serde_json::from_str(&encoded).expect("deserialize manifest");
        assert_eq!(decoded, manifest);
        decoded.validate().expect("valid manifest");
    }

    #[test]
    fn canonical_payload_is_independent_of_json_object_insertion_order() {
        let mut first = manifest();
        first.tools[0].input_schema = serde_json::from_str(
            r#"{"type":"object","properties":{"b":{"type":"number"},"a":{"type":"string"}}}"#,
        )
        .expect("schema");
        let mut second = manifest();
        second.tools[0].input_schema = serde_json::from_str(
            r#"{"properties":{"a":{"type":"string"},"b":{"type":"number"}},"type":"object"}"#,
        )
        .expect("schema");

        assert_eq!(
            first.canonical_signing_payload(b"component").unwrap(),
            second.canonical_signing_payload(b"component").unwrap()
        );
    }

    #[test]
    fn canonical_payload_binds_component_bytes() {
        let manifest = manifest();
        assert_ne!(
            manifest.canonical_signing_payload(b"component-a").unwrap(),
            manifest.canonical_signing_payload(b"component-b").unwrap()
        );
    }

    #[test]
    fn canonical_payload_rejects_oversized_manifest() {
        let mut manifest = manifest();
        manifest.description = Some("x".repeat(MAX_PLUGIN_MANIFEST_BYTES));
        assert!(matches!(
            manifest.canonical_signing_payload(b"component"),
            Err(ManifestValidationError::ManifestTooLarge)
        ));
    }

    #[test]
    fn registrations_require_explicit_capability() {
        let mut manifest = manifest();
        manifest.capabilities.clear();
        assert!(matches!(
            manifest.validate(),
            Err(ManifestValidationError::MissingCapability {
                field: "tools",
                capability: PluginCapability::RegisterTool,
            })
        ));
    }

    #[test]
    fn compatibility_uses_semver_requirement() {
        let manifest = manifest();
        manifest
            .ensure_api_compatible(&Version::new(1, 9, 0))
            .expect("1.x is supported");
        assert!(matches!(
            manifest.ensure_api_compatible(&Version::new(2, 0, 0)),
            Err(ManifestValidationError::IncompatibleApi { .. })
        ));
    }

    #[test]
    fn permissions_reject_parent_paths_and_urls() {
        let mut parent = manifest();
        parent.permissions.filesystem_read = vec!["../secret".into()];
        assert!(matches!(
            parent.validate(),
            Err(ManifestValidationError::InvalidPermission { .. })
        ));

        let mut url = manifest();
        url.permissions.network = vec!["https://api.example.com".into()];
        assert!(matches!(
            url.validate(),
            Err(ManifestValidationError::InvalidPermission { .. })
        ));
    }
}
