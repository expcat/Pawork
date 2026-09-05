//! Settings 查询 / 命令的 `AppResponse::Data` 载荷（CLN-4）。
//!
//! 形状对齐 Host `gui_host/handlers/settings.rs` 今日写出的 JSON。
//! `DefaultModelPair` 自 ADR-055 起被 `AppCommand::SetDefaultRoleModel`
//! 引用，随信封进 typegen；其余 Data 类型仍是 serde-only 载荷——
//! `AppResponse::Data` 在 wire 上是 `Value`，形状由 Host 与 golden 钉死，
//! 不单独进 typegen `export_all`（避免 schema 空转）。
//!
//! `SetApprovalMode.mode` 本波仍是 `String`；[`ApprovalModeWire`] 是 GUI
//! 通道唯一的 snake_case 枚举（无 kebab、无 `on_failure` 别名）。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
#[cfg(feature = "typegen")]
use ts_rs::TS;

/// 取消 serde 对 `Option` 的隐式 default：键必须出现，`null` 才是 [`None`]。
fn deserialize_required_option<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// GUI wire 审批模式（ADR-048 D1/D2）：五个 snake_case 规范值。
///
/// 与 `pawork_policy::ApprovalMode` 变体同名，但 **不** 接受磁盘/CLI 别名
/// （`on_failure`、kebab-case）；未知值 serde 与 [`FromStr`] 一律失败。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalModeWire {
    AlwaysAsk,
    AskForWrites,
    AskForDangerous,
    NeverAsk,
    ReadOnly,
}

impl ApprovalModeWire {
    /// 规范 wire 串（与 Host `approval_mode_wire` 同口径）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlwaysAsk => "always_ask",
            Self::AskForWrites => "ask_for_writes",
            Self::AskForDangerous => "ask_for_dangerous",
            Self::NeverAsk => "never_ask",
            Self::ReadOnly => "read_only",
        }
    }
}

impl fmt::Display for ApprovalModeWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 未知审批模式（GUI wire fail-closed；不含 kebab / `on_failure`）。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error(
    "unknown approval mode `{0}`; expected always_ask|ask_for_writes|ask_for_dangerous|never_ask|read_only"
)]
pub struct UnknownApprovalModeError(pub String);

impl FromStr for ApprovalModeWire {
    type Err = UnknownApprovalModeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "always_ask" => Ok(Self::AlwaysAsk),
            "ask_for_writes" => Ok(Self::AskForWrites),
            "ask_for_dangerous" => Ok(Self::AskForDangerous),
            "never_ask" => Ok(Self::NeverAsk),
            "read_only" => Ok(Self::ReadOnly),
            other => Err(UnknownApprovalModeError(other.to_string())),
        }
    }
}

/// `general_settings` 查询与 `set_proxy_url` 回执：`proxy_url` JSON `null` = [`None`]。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneralSettingsData {
    #[serde(deserialize_with = "deserialize_required_option")]
    pub proxy_url: Option<String>,
}

/// `terminal_settings` 查询与 `set_terminal_settings` 回执。
///
/// `shell` JSON `null` = 跟随平台默认；`columns` / `rows` 为生效值。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSettingsData {
    #[serde(deserialize_with = "deserialize_required_option")]
    pub shell: Option<String>,
    pub columns: u16,
    pub rows: u16,
}

/// `permissions_settings` 查询载荷（ADR-048 D1，含实现期 `workspace_id`）。
///
/// `set_approval_mode` 回执只回 `{ approval_mode }`，复用 [`ApprovalModeWire`]
/// 而非第二套字符串 API；完整四元组仅此查询形状。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsSettingsData {
    pub approval_mode: ApprovalModeWire,
    pub workspace_trusted: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub trust_workspaces_global: Option<bool>,
    pub workspace_id: String,
}

/// 生效默认模型配对（Host 顶层 `default` 对象；缺 provider/model 时为 JSON `null`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct DefaultModelPair {
    pub provider_id: String,
    pub model_id: String,
}

/// 单通道认证态（Host `auth_state`；内部标签 `"type"`，无 `content`）。
///
/// env 回退的 `connected` 仍带 `masked_credential` 键，值为 JSON `null`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderAuthState {
    Connecting,
    Connected {
        method: String,
        masked_credential: Option<String>,
    },
    /// Host `"type": "none"`：无存储凭证且无 env 回退。
    None,
    Error {
        message: String,
    },
}

/// 单通道目录三态（Host `catalog_state`；内部标签 `"type"`）。
///
/// `fixed_fallback` / `unavailable` 的 Host JSON **始终**带 `"fetched_at": null`，
/// 字段必须留下才能往返；`remote` 的 `fetched_at` 为 ISO-8601 字符串。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderCatalogState {
    Remote {
        fetched_at: String,
    },
    FixedFallback {
        snapshot_label: String,
        fetched_at: Option<String>,
    },
    Unavailable {
        error: String,
        fetched_at: Option<String>,
    },
}

/// `provider_auth_status` 数组中的一项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAuthStatusEntry {
    pub provider_id: String,
    pub display_name: String,
    pub endpoint_label: String,
    pub auth_methods: Vec<String>,
    pub auth: ProviderAuthState,
    pub catalog: ProviderCatalogState,
    /// 该 provider 是否跟随 Global `proxy_url`（ADR-052 SET-6h）。
    /// Host 输出 `config.providers[].use_proxy != Some(false)` 的生效值。
    pub use_proxy: bool,
}

/// `set_provider_use_proxy` 回执 Data（ADR-052 SET-6h；回执即写后状态）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUseProxyData {
    pub provider_id: String,
    pub use_proxy: bool,
}

/// `set_model_enabled` 回执 Data（ADR-055 OPT-3a）：写后状态 +
/// 本次禁用清除的角色默认对 wire 名列表（D3；启用恒为空）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetModelEnabledData {
    pub provider_id: String,
    pub model_id: String,
    pub enabled: bool,
    pub cleared_roles: Vec<String>,
}

/// `set_provider_models_enabled` 回执 Data（ADR-055 OPT-3a）：写后状态 +
/// 全关展开清除的角色默认对 wire 名列表。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetProviderModelsEnabledData {
    pub provider_id: String,
    pub enabled: bool,
    pub cleared_roles: Vec<String>,
}

/// `set_default_role_model` 回执 Data（ADR-055 OPT-3b）：写后状态；
/// `value` 必填可空，清除时为 JSON `null`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetDefaultRoleModelData {
    pub role: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub value: Option<DefaultModelPair>,
}

/// `provider_auth_status.role_defaults`（ADR-055 D5）：naming / vision /
/// search 三键 required-nullable，半配对输出 `null`（同顶层 `default`
/// 口径）；conversation 仍由既有顶层 `default` 透出，不在此重复。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDefaultsData {
    #[serde(deserialize_with = "deserialize_required_option")]
    pub naming: Option<DefaultModelPair>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub vision: Option<DefaultModelPair>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub search: Option<DefaultModelPair>,
}

/// `provider_auth_status` 查询 Data。Rust 字段名 `default` 对应 JSON 键 `default`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAuthStatusData {
    pub providers: Vec<ProviderAuthStatusEntry>,
    #[serde(rename = "default", deserialize_with = "deserialize_required_option")]
    pub default: Option<DefaultModelPair>,
    /// 三辅助角色默认对（ADR-055 D5，since 1.12 必填）。
    pub role_defaults: RoleDefaultsData,
}

/// `auth_start` 回执：三键稳定；PKCE 时 `user_code` / `expires_at` 为 JSON `null`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthStartData {
    pub verification_url: String,
    pub user_code: Option<String>,
    pub expires_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roundtrip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let encoded = serde_json::to_value(value).expect("serialize");
        serde_json::from_value(encoded).expect("deserialize")
    }

    #[test]
    fn general_permissions_terminal_roundtrip() {
        let general = GeneralSettingsData {
            proxy_url: Some("http://127.0.0.1:7890".into()),
        };
        assert_eq!(roundtrip(&general), general);
        assert_eq!(
            serde_json::to_value(&general).expect("serialize"),
            json!({ "proxy_url": "http://127.0.0.1:7890" })
        );

        let general_clear = GeneralSettingsData { proxy_url: None };
        assert_eq!(
            serde_json::to_value(&general_clear).expect("serialize"),
            json!({ "proxy_url": null })
        );
        assert_eq!(roundtrip(&general_clear), general_clear);

        let permissions_null_global = PermissionsSettingsData {
            approval_mode: ApprovalModeWire::ReadOnly,
            workspace_trusted: false,
            trust_workspaces_global: None,
            workspace_id: "workspace-1".into(),
        };
        assert_eq!(
            serde_json::to_value(&permissions_null_global).expect("serialize"),
            json!({
                "approval_mode": "read_only",
                "workspace_trusted": false,
                "trust_workspaces_global": null,
                "workspace_id": "workspace-1",
            })
        );
        assert_eq!(roundtrip(&permissions_null_global), permissions_null_global);

        let permissions_trusted = PermissionsSettingsData {
            approval_mode: ApprovalModeWire::AskForWrites,
            workspace_trusted: true,
            trust_workspaces_global: Some(true),
            workspace_id: "workspace-1".into(),
        };
        assert_eq!(roundtrip(&permissions_trusted), permissions_trusted);

        let terminal = TerminalSettingsData {
            shell: None,
            columns: 80,
            rows: 24,
        };
        assert_eq!(
            serde_json::to_value(&terminal).expect("serialize"),
            json!({ "shell": null, "columns": 80, "rows": 24 })
        );
        assert_eq!(roundtrip(&terminal), terminal);

        let terminal_set = TerminalSettingsData {
            shell: Some("/bin/zsh".into()),
            columns: 120,
            rows: 40,
        };
        assert_eq!(roundtrip(&terminal_set), terminal_set);
    }

    #[test]
    fn settings_data_missing_nullable_keys_fail_closed() {
        assert!(serde_json::from_value::<GeneralSettingsData>(json!({})).is_err());
        assert!(
            serde_json::from_value::<ProviderAuthStatusData>(json!({ "providers": [] })).is_err()
        );
        assert!(serde_json::from_value::<TerminalSettingsData>(
            json!({ "columns": 80, "rows": 24 })
        )
        .is_err());
        assert!(serde_json::from_value::<PermissionsSettingsData>(json!({
            "approval_mode": "always_ask",
            "workspace_trusted": false,
            "workspace_id": "ws-1",
        }))
        .is_err());
    }

    #[test]
    fn unknown_approval_mode_fails_closed() {
        for mode in [
            ApprovalModeWire::AlwaysAsk,
            ApprovalModeWire::AskForWrites,
            ApprovalModeWire::AskForDangerous,
            ApprovalModeWire::NeverAsk,
            ApprovalModeWire::ReadOnly,
        ] {
            let wire = format!("\"{}\"", mode.as_str());
            let decoded: ApprovalModeWire = serde_json::from_str(&wire).expect("known mode");
            assert_eq!(decoded, mode);
            assert_eq!(mode.to_string(), mode.as_str());
            assert_eq!(
                mode.as_str().parse::<ApprovalModeWire>().expect("FromStr"),
                mode
            );
        }

        assert!("always-ask".parse::<ApprovalModeWire>().is_err());
        assert!("on_failure".parse::<ApprovalModeWire>().is_err());
        assert!("on-failure".parse::<ApprovalModeWire>().is_err());
        assert!("unknown".parse::<ApprovalModeWire>().is_err());
        assert!(serde_json::from_str::<ApprovalModeWire>("\"always-ask\"").is_err());
        assert!(serde_json::from_str::<ApprovalModeWire>("\"on_failure\"").is_err());
        assert!(serde_json::from_str::<ApprovalModeWire>("\"unknown\"").is_err());
    }

    #[test]
    fn provider_auth_connected_env_masked_null() {
        let json = json!({
            "providers": [{
                "provider_id": "glm-coding",
                "display_name": "GLM Coding",
                "endpoint_label": "https://api.z.ai/api/coding/paas/v4",
                "auth_methods": ["api_key"],
                "auth": {
                    "type": "connected",
                    "method": "api_key",
                    "masked_credential": null
                },
                "catalog": {
                    "type": "remote",
                    "fetched_at": "2026-09-04T00:00:00Z"
                },
                "use_proxy": true
            }],
            "default": null,
            "role_defaults": {
                "naming": null,
                "vision": null,
                "search": null
            }
        });
        let status: ProviderAuthStatusData =
            serde_json::from_value(json.clone()).expect("env connected status");
        assert_eq!(
            status.providers[0].auth,
            ProviderAuthState::Connected {
                method: "api_key".into(),
                masked_credential: None,
            }
        );
        assert!(status.default.is_none());
        assert_eq!(serde_json::to_value(&status).expect("serialize"), json);

        let connected = ProviderAuthState::Connected {
            method: "api_key".into(),
            masked_credential: None,
        };
        assert_eq!(
            serde_json::to_value(&connected).expect("serialize"),
            json!({
                "type": "connected",
                "method": "api_key",
                "masked_credential": null
            })
        );
        assert_eq!(roundtrip(&connected), connected);
    }

    #[test]
    fn catalog_fixed_fallback_deserializes_fetched_at_null() {
        let json = json!({
            "type": "fixed_fallback",
            "snapshot_label": "pawork-providers/0.0.0",
            "fetched_at": null
        });
        let catalog: ProviderCatalogState =
            serde_json::from_value(json.clone()).expect("fixed_fallback");
        assert_eq!(
            catalog,
            ProviderCatalogState::FixedFallback {
                snapshot_label: "pawork-providers/0.0.0".into(),
                fetched_at: None,
            }
        );
        assert_eq!(serde_json::to_value(&catalog).expect("serialize"), json);
        assert_eq!(roundtrip(&catalog), catalog);

        let unavailable = json!({
            "type": "unavailable",
            "error": "runtime model probe timed out",
            "fetched_at": null
        });
        let decoded: ProviderCatalogState =
            serde_json::from_value(unavailable.clone()).expect("unavailable");
        assert_eq!(
            decoded,
            ProviderCatalogState::Unavailable {
                error: "runtime model probe timed out".into(),
                fetched_at: None,
            }
        );
        assert_eq!(
            serde_json::to_value(&decoded).expect("serialize"),
            unavailable
        );
    }

    #[test]
    fn auth_start_data_keeps_null_device_fields() {
        let pkce = AuthStartData {
            verification_url: "https://example/verify".into(),
            user_code: None,
            expires_at: None,
        };
        assert_eq!(
            serde_json::to_value(&pkce).expect("serialize"),
            json!({
                "verification_url": "https://example/verify",
                "user_code": null,
                "expires_at": null
            })
        );
        assert_eq!(roundtrip(&pkce), pkce);
    }

    #[test]
    fn opt3_model_enablement_and_role_data_roundtrip() {
        let model_enabled = SetModelEnabledData {
            provider_id: "glm-coding".into(),
            model_id: "glm-4.7".into(),
            enabled: false,
            cleared_roles: vec!["naming".into(), "conversation".into()],
        };
        assert_eq!(
            serde_json::to_value(&model_enabled).expect("serialize"),
            json!({
                "provider_id": "glm-coding",
                "model_id": "glm-4.7",
                "enabled": false,
                "cleared_roles": ["naming", "conversation"]
            })
        );
        assert_eq!(roundtrip(&model_enabled), model_enabled);

        let provider_enabled = SetProviderModelsEnabledData {
            provider_id: "glm-coding".into(),
            enabled: true,
            cleared_roles: vec![],
        };
        assert_eq!(roundtrip(&provider_enabled), provider_enabled);

        let role_set = SetDefaultRoleModelData {
            role: "naming".into(),
            value: Some(DefaultModelPair {
                provider_id: "glm-coding".into(),
                model_id: "glm-4.7".into(),
            }),
        };
        assert_eq!(
            serde_json::to_value(&role_set).expect("serialize"),
            json!({
                "role": "naming",
                "value": {"provider_id": "glm-coding", "model_id": "glm-4.7"}
            })
        );
        assert_eq!(roundtrip(&role_set), role_set);

        let role_clear = SetDefaultRoleModelData {
            role: "vision".into(),
            value: None,
        };
        assert_eq!(
            serde_json::to_value(&role_clear).expect("serialize"),
            json!({"role": "vision", "value": null})
        );
        assert_eq!(roundtrip(&role_clear), role_clear);

        let role_defaults = RoleDefaultsData {
            naming: Some(DefaultModelPair {
                provider_id: "glm-coding".into(),
                model_id: "glm-4.7".into(),
            }),
            vision: None,
            search: None,
        };
        assert_eq!(
            serde_json::to_value(&role_defaults).expect("serialize"),
            json!({
                "naming": {"provider_id": "glm-coding", "model_id": "glm-4.7"},
                "vision": null,
                "search": null
            })
        );
        assert_eq!(roundtrip(&role_defaults), role_defaults);
    }

    #[test]
    fn opt3_required_nullable_role_fields_fail_closed() {
        assert!(serde_json::from_value::<SetDefaultRoleModelData>(json!({
            "role": "naming"
        }))
        .is_err());
        assert!(serde_json::from_value::<RoleDefaultsData>(json!({
            "naming": null,
            "vision": null
        }))
        .is_err());
        assert!(serde_json::from_value::<ProviderAuthStatusData>(json!({
            "providers": [],
            "default": null
        }))
        .is_err());
    }
}
