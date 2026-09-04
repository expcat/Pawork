//! DesktopProjection 定向测试。

use super::*;
use pawork_client::{AppEventEnvelope, ResumeDisposition, ResumeOutcome, Snapshot, TimelinePage};
use serde_json::{json, Value};

fn snapshot_with_sessions(entries: Vec<Value>) -> Snapshot {
    serde_json::from_value(json!({
        "instance_id": "instance-1",
        "snapshot_sequence": 0,
        "generated_at": 1,
        "sections": [
            {
                "kind": "workspaces",
                "revision": 1,
                "data": [{ "id": "ws-default", "name": "default", "trusted": true }]
            },
            { "kind": "session_tree", "revision": 2, "data": entries }
        ]
    }))
    .expect("decode Snapshot")
}

#[test]
fn provider_status_entries_map_host_wire_to_readonly_labels() {
    let data = json!({
        "providers": [
            {
                "provider_id": "glm-coding",
                "display_name": "Z.AI GLM Coding Plan",
                "endpoint_label": "https://api.z.ai",
                "auth_methods": ["api_key"],
                "auth": {
                    "type": "connected",
                    "method": "api_key",
                    "masked_credential": "sk-…ab12"
                },
                "catalog": { "type": "remote", "fetched_at": "2026-09-02T08:00:00Z" }
            },
            {
                "provider_id": "kimi",
                "display_name": "Kimi",
                "endpoint_label": "https://api.moonshot.cn",
                "auth_methods": ["api_key", "oauth"],
                "auth": { "type": "none" },
                "catalog": {
                    "type": "fixed_fallback",
                    "snapshot_label": "models.dev@v1",
                    "fetched_at": null
                }
            }
        ],
        "default": null
    });
    let loaded = serde_json::from_value::<ProviderAuthStatusData>(data.clone())
        .expect("parse provider status");
    let entries = &loaded.providers;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].auth_methods_label(), "API key");
    assert_eq!(entries[0].auth_label(), "Connected");
    assert_eq!(
        entries[0].catalog_label(),
        "Remote catalog · fetched 2026-09-02T08:00:00Z"
    );
    assert_eq!(entries[1].auth_methods_label(), "API key / OAuth");
    assert_eq!(entries[1].auth_label(), "Not connected");
    assert_eq!(
        entries[1].catalog_label(),
        "Built-in catalog fallback · models.dev@v1"
    );
    assert_eq!(loaded.default, None);
}

#[test]
fn provider_status_entries_fail_closed_on_malformed_payload() {
    // default 合法（null），钉住错误只来自 providers 侧。
    let payload = |providers: Value| json!({ "providers": providers, "default": null });
    // 缺 providers 数组：整体 fail-closed。
    assert!(serde_json::from_value::<ProviderAuthStatusData>(payload(json!("nope"))).is_err());
    // 单条缺 auth / 未知 auth 状态：不静默丢条目。
    assert!(
        serde_json::from_value::<ProviderAuthStatusData>(payload(json!([
            { "provider_id": "glm-coding", "display_name": "Z.AI", "endpoint_label": "e" }
        ])))
        .is_err()
    );
    // auth_methods 缺失 / 非数组 / 含非字符串项：fail-closed，不默认空表。
    assert!(
        serde_json::from_value::<ProviderAuthStatusData>(payload(json!([
            {
                "provider_id": "glm-coding",
                "display_name": "Z.AI",
                "endpoint_label": "e",
                "auth": { "type": "none" },
                "catalog": { "type": "remote", "fetched_at": "t" }
            }
        ])))
        .is_err()
    );
    assert!(
        serde_json::from_value::<ProviderAuthStatusData>(payload(json!([
            {
                "provider_id": "glm-coding",
                "display_name": "Z.AI",
                "endpoint_label": "e",
                "auth_methods": "api_key",
                "auth": { "type": "none" },
                "catalog": { "type": "remote", "fetched_at": "t" }
            }
        ])))
        .is_err()
    );
    assert!(
        serde_json::from_value::<ProviderAuthStatusData>(payload(json!([
            {
                "provider_id": "glm-coding",
                "display_name": "Z.AI",
                "endpoint_label": "e",
                "auth": { "type": "mystery" },
                "catalog": { "type": "remote", "fetched_at": "t" }
            }
        ])))
        .is_err()
    );
}

#[test]
fn provider_status_default_parses_host_default() {
    // 主路径：default 对象 → Some(pair)；null → None（Host 权威语义）。
    let mut data = json!({
        "default": { "provider_id": "kimi", "model_id": "kimi-k2-0905-preview" }
    });
    data["providers"] = json!([]);
    let loaded =
        serde_json::from_value::<ProviderAuthStatusData>(data.clone()).expect("parse default");
    assert_eq!(
        loaded.default,
        Some(DefaultModelPair {
            provider_id: "kimi".to_string(),
            model_id: "kimi-k2-0905-preview".to_string(),
        })
    );
    let mut none = json!({ "default": null });
    none["providers"] = json!([]);
    assert_eq!(
        serde_json::from_value::<ProviderAuthStatusData>(none.clone())
            .expect("parse null default")
            .default,
        None
    );
}

#[test]
fn provider_status_default_fails_closed_on_malformed_payload() {
    let payload = |default: Value| json!({ "providers": [], "default": default });
    // 缺顶层 default：整体 fail-closed，不静默当 null。
    assert!(serde_json::from_value::<ProviderAuthStatusData>(json!({ "providers": [] })).is_err());
    // 非对象非 null / 缺 model_id / 字段非字符串：同样 fail-closed。
    assert!(serde_json::from_value::<ProviderAuthStatusData>(payload(json!("kimi"))).is_err());
    assert!(serde_json::from_value::<ProviderAuthStatusData>(payload(
        json!({ "provider_id": "kimi" })
    ))
    .is_err());
    assert!(
        serde_json::from_value::<ProviderAuthStatusData>(payload(json!({
            "provider_id": "kimi",
            "model_id": 7
        })))
        .is_err()
    );
}

#[test]
fn set_default_confirmation_syncs_composer_projection() {
    let mut projection = DesktopProjection::default();
    // 确认后重查 provider_auth_status：权威 default 先落地 Settings 状态。
    projection
        .settings_providers
        .apply_loaded(ProviderAuthStatusData {
            providers: Vec::new(),
            default: Some(DefaultModelPair {
                provider_id: "kimi".to_string(),
                model_id: "kimi-k2-0905-preview".to_string(),
            }),
        });
    projection.set_pending_model("glm-coding".into(), "glm-4.7".into());
    // Host Data 确认到达：selected_model 同步为已确认默认，pending 清空
    //（Composer 同步；不改会话 / 草稿 / Run）。
    projection.confirm_default_model("kimi".into(), "kimi-k2-0905-preview".into());
    assert_eq!(
        projection.selected_model,
        Some(("kimi".to_string(), "kimi-k2-0905-preview".to_string()))
    );
    assert_eq!(projection.pending_model, None);
    assert_eq!(
        projection.effective_model(),
        Some(&("kimi".to_string(), "kimi-k2-0905-preview".to_string()))
    );
    assert_eq!(
        projection.settings_providers.default_model,
        projection.selected_model
    );
}

#[test]
fn default_model_unavailable_flag_tracks_connection_and_catalog() {
    let mut projection = DesktopProjection::default();
    let entry = |auth: ProviderAuthState| ProviderAuthStatusEntry {
        provider_id: "kimi".into(),
        display_name: "Kimi".into(),
        endpoint_label: "https://api.kimi.com".into(),
        auth_methods: vec!["oauth".into()],
        auth,
        catalog: ProviderCatalogState::Unavailable {
            error: "offline".into(),
            fetched_at: None,
        },
    };
    // 目录为空（尚未加载 / model_list 失败）：即使已连接且有默认，
    // 也区分「无目录数据」与「目录明确不含」，不误报失效。
    projection.settings_providers.providers = vec![entry(ProviderAuthState::Connected {
        method: "oauth".into(),
        masked_credential: None,
    })];
    projection.settings_providers.default_model =
        Some(("kimi".into(), "kimi-k2-0905-preview".into()));
    assert!(!projection.default_model_unavailable());
    projection.set_models(vec![ModelEntry {
        provider_id: "kimi".into(),
        id: "kimi-k2-0905-preview".into(),
        display_name: "Kimi K2".into(),
        context_window_tokens: None,
    }]);
    // 无默认：不误报失效。
    projection.settings_providers.default_model = None;
    assert!(!projection.default_model_unavailable());
    // 默认 provider 未连接：显式失效。
    projection
        .settings_providers
        .apply_loaded(ProviderAuthStatusData {
            providers: vec![entry(ProviderAuthState::None)],
            default: Some(DefaultModelPair {
                provider_id: "kimi".into(),
                model_id: "kimi-k2-0905-preview".into(),
            }),
        });
    assert!(projection.default_model_unavailable());
    // 已连接但默认 model 不在该 provider 当前目录：显式失效。
    projection.settings_providers.providers[0].auth = ProviderAuthState::Connected {
        method: "oauth".into(),
        masked_credential: None,
    };
    projection.settings_providers.default_model = Some(("kimi".into(), "kimi-latest".into()));
    assert!(projection.default_model_unavailable());
    // 已连接且在当前目录：可用。
    projection.settings_providers.default_model =
        Some(("kimi".into(), "kimi-k2-0905-preview".into()));
    assert!(!projection.default_model_unavailable());
}

#[test]
fn provider_status_refresh_failure_keeps_last_list_and_default() {
    // 页级刷新失败（OperationFailed → apply_failed）：保留旧列表与
    // 默认项，只记录错误，不伪造空态。
    let mut state = SettingsProvidersState::default();
    state.apply_loaded(ProviderAuthStatusData {
        providers: vec![ProviderAuthStatusEntry {
            provider_id: "kimi".into(),
            display_name: "Kimi".into(),
            endpoint_label: "https://api.kimi.com".into(),
            auth_methods: vec!["oauth".into()],
            auth: ProviderAuthState::None,
            catalog: ProviderCatalogState::Unavailable {
                error: "offline".into(),
                fetched_at: None,
            },
        }],
        default: Some(DefaultModelPair {
            provider_id: "kimi".to_string(),
            model_id: "kimi-k2-0905-preview".to_string(),
        }),
    });
    state.apply_failed("query failed");
    assert_eq!(state.providers.len(), 1);
    assert_eq!(
        state.default_model,
        Some(("kimi".to_string(), "kimi-k2-0905-preview".to_string()))
    );
    assert_eq!(state.query.error.as_deref(), Some("query failed"));
    assert!(!state.query.loading);
}

#[test]
fn general_settings_parses_host_proxy_url() {
    assert_eq!(
        serde_json::from_value::<GeneralSettingsData>(
            json!({ "proxy_url": "http://127.0.0.1:7890" })
        )
        .expect("parse proxy_url string")
        .proxy_url,
        Some("http://127.0.0.1:7890".into())
    );
    assert_eq!(
        serde_json::from_value::<GeneralSettingsData>(json!({ "proxy_url": null }))
            .expect("parse null proxy_url")
            .proxy_url,
        None
    );
    let mut state = SettingsGeneralState::default();
    state.apply_loaded(GeneralSettingsData {
        proxy_url: Some("http://127.0.0.1:7890".into()),
    });
    assert!(state.query.available);
    assert_eq!(state.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));
    state.apply_loaded(GeneralSettingsData { proxy_url: None });
    assert_eq!(state.proxy_url, None);
    assert!(state.query.available);
}

#[test]
fn general_settings_fails_closed_on_malformed_payload() {
    assert!(serde_json::from_value::<GeneralSettingsData>(json!({})).is_err());
    assert!(serde_json::from_value::<GeneralSettingsData>(json!({ "proxy_url": 7 })).is_err());
    assert!(
        serde_json::from_value::<GeneralSettingsData>(json!({ "proxy_url": { "url": "x" } }))
            .is_err()
    );
    let mut state = SettingsGeneralState::default();
    state.apply_failed("malformed payload");
    assert!(!state.query.available);
    assert_eq!(state.proxy_url, None);
    assert_eq!(state.query.error.as_deref(), Some("malformed payload"));
}

#[test]
fn general_settings_stale_keeps_last_value_and_disables_writes() {
    let mut state = SettingsGeneralState::default();
    state.apply_loaded(GeneralSettingsData {
        proxy_url: Some("http://127.0.0.1:7890".into()),
    });
    assert!(state.writes_enabled(true));
    state.mark_stale("socket closed");
    assert_eq!(state.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));
    assert!(state.query.available);
    assert_eq!(state.query.stale_reason.as_deref(), Some("socket closed"));
    assert!(!state.writes_enabled(true));
    assert!(!state.writes_enabled(false));
    state.apply_failed("query failed");
    assert_eq!(state.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));
    assert!(state.query.available);
}

#[test]
fn permissions_settings_parses_host_triple() {
    // 主路径：四元组解析（null global = 未设置）+ 全五档 wire 串往返。
    let data = serde_json::from_value::<PermissionsSettingsData>(json!({
        "approval_mode": "ask_for_writes",
        "workspace_trusted": true,
        "trust_workspaces_global": null,
        "workspace_id": "workspace-1"
    }))
    .expect("parse permissions settings");
    assert_eq!(data.approval_mode, ApprovalModeWire::AskForWrites);
    assert!(data.workspace_trusted);
    assert_eq!(data.trust_workspaces_global, None);
    assert_eq!(data.workspace_id, "workspace-1");
    for mode in [
        ApprovalModeWire::AlwaysAsk,
        ApprovalModeWire::AskForWrites,
        ApprovalModeWire::AskForDangerous,
        ApprovalModeWire::NeverAsk,
        ApprovalModeWire::ReadOnly,
    ] {
        let parsed = serde_json::from_value::<PermissionsSettingsData>(json!({
            "approval_mode": mode.as_str(),
            "workspace_trusted": false,
            "trust_workspaces_global": true,
            "workspace_id": "workspace-1"
        }))
        .expect("known mode parses");
        assert_eq!(parsed.approval_mode, mode);
    }
    let mut state = SettingsPermissionsState::default();
    state.apply_loaded(data);
    assert!(state.query.available);
    assert_eq!(state.approval_mode, Some(ApprovalModeWire::AskForWrites));
    assert!(state.writes_enabled(true));
    // 写回执按字段确认（回执即写后状态）。
    state.confirm_approval_mode(ApprovalModeWire::NeverAsk);
    assert_eq!(state.approval_mode, Some(ApprovalModeWire::NeverAsk));
    state.confirm_workspace_trusted(false);
    assert!(!state.workspace_trusted);
}

#[test]
fn permissions_settings_fails_closed_on_malformed_payload() {
    assert!(serde_json::from_value::<PermissionsSettingsData>(json!({})).is_err());
    assert!(serde_json::from_value::<PermissionsSettingsData>(
        json!({ "approval_mode": "always_ask" })
    )
    .is_err());
    assert!(serde_json::from_value::<PermissionsSettingsData>(json!({
        "approval_mode": "yolo",
        "workspace_trusted": false,
        "trust_workspaces_global": null,
        "workspace_id": "workspace-1"
    }))
    .is_err());
    assert!(serde_json::from_value::<PermissionsSettingsData>(json!({
        "approval_mode": 7,
        "workspace_trusted": false,
        "trust_workspaces_global": null,
        "workspace_id": "workspace-1"
    }))
    .is_err());
    assert!(serde_json::from_value::<PermissionsSettingsData>(json!({
        "approval_mode": "read_only",
        "workspace_trusted": "yes",
        "trust_workspaces_global": null,
        "workspace_id": "workspace-1"
    }))
    .is_err());
    assert!(serde_json::from_value::<PermissionsSettingsData>(json!({
        "approval_mode": "read_only",
        "workspace_trusted": false,
        "trust_workspaces_global": "true",
        "workspace_id": "workspace-1"
    }))
    .is_err());
    // 缺 workspace_id 同样 fail-closed（ADR-048 D1 实现期修订字段）。
    assert!(serde_json::from_value::<PermissionsSettingsData>(json!({
        "approval_mode": "read_only",
        "workspace_trusted": false,
        "trust_workspaces_global": null
    }))
    .is_err());
    let mut state = SettingsPermissionsState::default();
    state.apply_failed("malformed payload");
    assert!(!state.query.available);
    assert_eq!(state.approval_mode, None);
    assert!(!state.workspace_trusted);
    assert_eq!(state.query.error.as_deref(), Some("malformed payload"));
}

#[test]
fn permissions_settings_stale_keeps_last_values_and_disables_writes() {
    let mut state = SettingsPermissionsState::default();
    state.apply_loaded(
        serde_json::from_value::<PermissionsSettingsData>(json!({
            "approval_mode": "ask_for_dangerous",
            "workspace_trusted": true,
            "trust_workspaces_global": null,
            "workspace_id": "workspace-1"
        }))
        .expect("parse"),
    );
    assert!(state.writes_enabled(true));
    state.mark_stale("socket closed");
    assert_eq!(state.approval_mode, Some(ApprovalModeWire::AskForDangerous));
    assert!(state.workspace_trusted);
    assert_eq!(state.trust_workspaces_global, None);
    assert!(state.query.available);
    assert_eq!(state.query.stale_reason.as_deref(), Some("socket closed"));
    assert!(!state.writes_enabled(true));
    assert!(!state.writes_enabled(false));
    // 写失败保旧（fail-closed）：值不动，只记录错误。
    state.apply_failed("set approval mode failed");
    assert_eq!(state.approval_mode, Some(ApprovalModeWire::AskForDangerous));
    assert!(state.workspace_trusted);
    assert!(state.query.available);
}

#[test]
fn terminal_settings_main_path_confirms_full_state_and_sizes_new_terminal() {
    // 主路径：解析应用 + 全态写串联 + 初始尺寸取生效值（ADR-050 D2-D4）。
    let mut state = SettingsTerminalState::default();
    assert_eq!(state.effective_size(), (80, 24), "unqueried falls back");
    state.apply_loaded(
        serde_json::from_value::<TerminalSettingsData>(json!({
            "shell": "/bin/zsh", "columns": 120, "rows": 40
        }))
        .expect("parse terminal settings"),
    );
    assert!(state.query.available);
    assert!(state.writes_enabled(true));
    assert_eq!(state.shell.as_deref(), Some("/bin/zsh"));
    assert_eq!(state.effective_size(), (120, 40));
    // 全态写回执（shell=null 清除 + 新尺寸）即写后状态。
    state.apply_confirmed(
        serde_json::from_value::<TerminalSettingsData>(json!({
            "shell": null, "columns": 100, "rows": 30
        }))
        .expect("parse clear receipt"),
    );
    assert_eq!(state.shell, None);
    assert_eq!(state.effective_size(), (100, 30));
    // 新建终端投影初始尺寸取生效值（不置 resize_confirmed，回执才确认）。
    let mut projection = DesktopProjection::default();
    projection.workspace_id = Some("ws-1".into());
    projection.settings_terminal = state;
    projection.apply_terminal_created("ws-1".into(), "term-1".into());
    let (columns, rows) = projection.settings_terminal.effective_size();
    assert!(projection.apply_terminal_initial_size("term-1", columns, rows));
    assert_eq!(
        (projection.terminal.columns, projection.terminal.rows),
        (100, 30)
    );
    assert!(!projection.terminal.resize_confirmed);
}

#[test]
fn terminal_settings_fails_closed_on_malformed_payload() {
    assert!(serde_json::from_value::<TerminalSettingsData>(json!({})).is_err());
    assert!(
        serde_json::from_value::<TerminalSettingsData>(json!({ "columns": 80, "rows": 24 }))
            .is_err()
    );
    assert!(serde_json::from_value::<TerminalSettingsData>(json!({
        "shell": null, "rows": 24
    }))
    .is_err());
    assert!(serde_json::from_value::<TerminalSettingsData>(json!({
        "shell": 7, "columns": 80, "rows": 24
    }))
    .is_err());
    assert!(serde_json::from_value::<TerminalSettingsData>(json!({
        "shell": null, "columns": "80", "rows": 24
    }))
    .is_err());
    assert!(serde_json::from_value::<TerminalSettingsData>(json!({
        "shell": null, "columns": 80, "rows": 70000
    }))
    .is_err());
    let mut state = SettingsTerminalState::default();
    state.apply_failed("malformed payload");
    assert!(!state.query.available);
    assert_eq!(state.shell, None);
    assert_eq!((state.columns, state.rows), (0, 0));
    assert_eq!(state.query.error.as_deref(), Some("malformed payload"));
}

fn settings_state_with_provider(auth_methods: &[&str]) -> SettingsProvidersState {
    let mut state = SettingsProvidersState::default();
    state.apply_loaded(ProviderAuthStatusData {
        providers: vec![ProviderAuthStatusEntry {
            provider_id: "kimi".into(),
            display_name: "Kimi".into(),
            endpoint_label: "https://api.moonshot.cn".into(),
            auth_methods: auth_methods
                .iter()
                .map(|method| method.to_string())
                .collect(),
            auth: ProviderAuthState::None,
            catalog: ProviderCatalogState::Unavailable {
                error: "offline".into(),
                fetched_at: None,
            },
        }],
        default: None,
    });
    state
}

fn provider_auth(state: &SettingsProvidersState) -> &ProviderAuthState {
    &state.providers[0].auth
}

#[test]
fn auth_changed_states_parse_and_apply_to_provider_auth() {
    // wire 形态（tag=type / content=data）六态解析。
    assert_eq!(
        parse_auth_change(&json!({ "type": "pending" })),
        Ok(AuthChange::Pending)
    );
    assert_eq!(
        parse_auth_change(&json!({
            "type": "succeeded",
            "data": { "method": "api_key", "masked_credential": "sk-…ab12" }
        })),
        Ok(AuthChange::Succeeded {
            method: "api_key".into(),
            masked_credential: "sk-…ab12".into()
        })
    );
    assert_eq!(
        parse_auth_change(&json!({
            "type": "failed",
            "data": { "error": "invalid key" }
        })),
        Ok(AuthChange::Failed {
            error: "invalid key".into()
        })
    );
    assert_eq!(
        parse_auth_change(&json!({ "type": "cancelled" })),
        Ok(AuthChange::Cancelled)
    );
    assert_eq!(
        parse_auth_change(&json!({ "type": "expired" })),
        Ok(AuthChange::Expired)
    );
    assert_eq!(
        parse_auth_change(&json!({ "type": "removed" })),
        Ok(AuthChange::Removed)
    );

    // Pending → Connecting。
    let mut state = settings_state_with_provider(&["oauth"]);
    state.apply_auth_changed_value("kimi", &json!({ "type": "pending" }));
    assert_eq!(*provider_auth(&state), ProviderAuthState::Connecting);

    // auth_start 回执登记等待详情；Succeeded 清等待、置 Connected，
    // 并置再查询提示（认证成功≠目录成功）。
    state.apply_auth_started(
        "kimi",
        AuthStartData {
            verification_url: "https://example/verify".into(),
            user_code: Some("ABCD".into()),
            expires_at: Some("2026-09-02T09:00:00Z".into()),
        },
    );
    assert_eq!(state.oauth_waits["kimi"].user_code.as_deref(), Some("ABCD"));
    state.apply_auth_changed_value(
        "kimi",
        &json!({
            "type": "succeeded",
            "data": { "method": "oauth", "masked_credential": "mo…cd" }
        }),
    );
    assert_eq!(
        *provider_auth(&state),
        ProviderAuthState::Connected {
            method: "oauth".into(),
            masked_credential: Some("mo…cd".into())
        }
    );
    assert!(!state.oauth_waits.contains_key("kimi"));
    assert!(state.take_pending_status_refresh());
    assert!(!state.pending_status_refresh);

    // Failed → Error（只承载 Host 已脱敏 message）。
    state.apply_auth_changed_value(
        "kimi",
        &json!({ "type": "failed", "data": { "error": "denied" } }),
    );
    assert_eq!(
        *provider_auth(&state),
        ProviderAuthState::Error {
            message: "denied".into()
        }
    );

    // Cancelled / Expired / Removed → None + 瞬态 note + 清等待。
    for (kind, note) in [
        ("cancelled", "Authorization cancelled"),
        ("expired", "Authorization expired"),
        ("removed", "Connection removed"),
    ] {
        state.apply_auth_started(
            "kimi",
            AuthStartData {
                verification_url: "u".into(),
                user_code: None,
                expires_at: None,
            },
        );
        state.apply_auth_changed_value("kimi", &json!({ "type": kind }));
        assert_eq!(*provider_auth(&state), ProviderAuthState::None, "{kind}");
        assert_eq!(state.auth_notes.get("kimi").map(String::as_str), Some(note));
        assert!(!state.oauth_waits.contains_key("kimi"), "{kind}");
        if kind == "removed" {
            assert!(state.take_pending_status_refresh(), "{kind}");
        } else {
            assert!(!state.pending_status_refresh, "{kind}");
        }
    }
    // 下一次权威状态到达即清空瞬态反馈。
    let providers = state.providers.clone();
    state.apply_loaded(ProviderAuthStatusData {
        providers,
        default: None,
    });
    assert!(state.auth_notes.is_empty());
}

#[test]
fn malformed_auth_change_fails_closed_without_state_landing() {
    let mut state = settings_state_with_provider(&["api_key"]);
    for payload in [
        json!({ "type": "mystery" }),
        json!({ "type": "succeeded", "data": { "method": "api_key" } }),
        json!({ "data": { "error": "x" } }),
    ] {
        assert!(!state.apply_auth_changed_value("kimi", &payload));
        assert_eq!(*provider_auth(&state), ProviderAuthState::None);
    }
    assert!(state
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("malformed auth change"));
    assert!(state.oauth_waits.is_empty());
    assert!(state.auth_notes.is_empty());
    assert!(!state.pending_status_refresh);
}

#[test]
fn replace_flow_terminal_keeps_old_credential_and_triggers_requery() {
    // Connected 起点的 Replace 流程：Cancelled / Expired / Failed 不清
    // 旧凭证（Host 未删除），保留现状态并触发权威重查。
    let mut state = settings_state_with_provider(&["oauth"]);
    state.providers[0].auth = ProviderAuthState::Connected {
        method: "oauth".into(),
        masked_credential: Some("mo…cd".into()),
    };
    state.begin_auth_flow("kimi");
    // 乐观 / Pending 先置 Connecting（终态到达前的 UI 状态）。
    state.apply_auth_changed_value("kimi", &json!({ "type": "pending" }));
    assert_eq!(*provider_auth(&state), ProviderAuthState::Connecting);

    for kind in ["cancelled", "expired"] {
        state.apply_auth_changed_value("kimi", &json!({ "type": kind }));
        assert_eq!(
            *provider_auth(&state),
            ProviderAuthState::Connecting,
            "{kind}: replace keeps the old credential pending requery"
        );
        assert!(state.pending_status_refresh, "{kind}");
        assert!(state.take_pending_status_refresh());
    }

    // Failed：不降级 Error，失败原因走瞬态 note，重查置位。
    state.apply_auth_changed_value(
        "kimi",
        &json!({ "type": "failed", "data": { "error": "invalid key" } }),
    );
    assert_eq!(*provider_auth(&state), ProviderAuthState::Connecting);
    assert!(state.pending_status_refresh);
    assert_eq!(
        state.auth_notes.get("kimi").map(String::as_str),
        Some("Replacement failed · invalid key")
    );
    assert!(state.take_pending_status_refresh());

    // Removed：凭证确实删除，仍复位 ProviderAuthState::None。
    state.providers[0].auth = ProviderAuthState::Connected {
        method: "oauth".into(),
        masked_credential: Some("mo…cd".into()),
    };
    state.begin_auth_flow("kimi");
    state.apply_auth_changed_value("kimi", &json!({ "type": "removed" }));
    assert_eq!(*provider_auth(&state), ProviderAuthState::None);
    assert!(state.take_pending_status_refresh());

    // 权威数据到达即清基线（后续事件回到首连语义）。
    state.providers[0].auth = ProviderAuthState::Connected {
        method: "oauth".into(),
        masked_credential: Some("mo…cd".into()),
    };
    state.begin_auth_flow("kimi");
    let providers = state.providers.clone();
    let default_model = state.default_model.clone();
    state.apply_loaded(ProviderAuthStatusData {
        providers,
        default: default_model.map(|(provider_id, model_id)| DefaultModelPair {
            provider_id,
            model_id,
        }),
    });
    assert!(state.auth_replacing_connected.is_empty());
}

fn session_entry(id: &str, title: &str, updated: u64) -> Value {
    session_entry_in(id, title, updated, None)
}

fn session_entry_in(id: &str, title: &str, updated: u64, workspace_id: Option<&str>) -> Value {
    let mut entry = json!({
        "session_id": id,
        "title": title,
        "created_at_ms": 1,
        "updated_at_ms": updated,
        "active_branch": "main",
        "archived": false
    });
    if let Some(workspace_id) = workspace_id {
        entry["workspace_id"] = json!(workspace_id);
    }
    entry
}

fn event(sequence: u64, payload: Value) -> AppEventEnvelope {
    serde_json::from_value(json!({
        "api_version": { "major": 1, "minor": 1 },
        "instance_id": "instance-1",
        "event_id": format!("app-{sequence}"),
        "global_sequence": sequence,
        "stream": { "type": "session", "id": "s-1" },
        "stream_sequence": sequence,
        "timestamp": 1_000 + sequence,
        "source": { "type": "core" },
        "payload": payload
    }))
    .expect("decode AppEventEnvelope")
}

fn run_changed(sequence: u64, state: &str) -> AppEventEnvelope {
    event(
        sequence,
        json!({ "type": "run_changed", "data": { "run_id": "r-1", "state": state } }),
    )
}

fn assistant_delta(sequence: u64, message_id: &str, delta: &str) -> AppEventEnvelope {
    event(
        sequence,
        json!({
            "type": "assistant_delta",
            "data": { "run_id": "r-1", "message_id": message_id, "delta": delta }
        }),
    )
}

fn page(items: Vec<Value>, complete: bool) -> TimelinePage {
    serde_json::from_value(json!({
        "items": items,
        "head_sequence": items.len() as u64,
        "complete": complete
    }))
    .expect("decode TimelinePage")
}

fn history_item(sequence: u64, kind: &str, extra: Value) -> Value {
    let mut item = json!({
        "sequence": sequence,
        "event_id": format!("hist-{sequence}"),
        "kind": kind,
        "run_id": "r-1",
        "timestamp": "2000"
    });
    if let Some(fields) = extra.as_object() {
        for (key, value) in fields {
            item[key] = value.clone();
        }
    }
    item
}

fn raw_entry(sequence: u64, kind: TimelineEntryKind, run_id: Option<&str>) -> TimelineEntry {
    TimelineEntry {
        sequence,
        event_id: format!("raw-{sequence}"),
        kind,
        fork_boundary: None,
        timestamp: "2000".into(),
        run_id: run_id.map(str::to_string),
    }
}

fn tool_entry(sequence: u64, run_id: &str, name: &str, status: &str) -> TimelineEntry {
    raw_entry(
        sequence,
        TimelineEntryKind::ToolCall {
            name: name.into(),
            status: status.into(),
            detail: None,
        },
        Some(run_id),
    )
}

fn terminal_entry(sequence: u64, boundary: ForkBoundary) -> TimelineEntry {
    let mut entry = raw_entry(
        sequence,
        TimelineEntryKind::RunState("run terminal".into()),
        Some("r-1"),
    );
    entry.fork_boundary = Some(boundary);
    entry
}

#[test]
fn snapshot_rebuilds_sessions_and_events_rebuild_timeline() {
    let snapshot = snapshot_with_sessions(vec![
        session_entry("s-old", "Old", 10),
        session_entry("s-new", "New", 20),
    ]);
    let mut projection = DesktopProjection::from_snapshot(&snapshot);
    assert_eq!(projection.workspace_id.as_deref(), Some("ws-default"));
    // 按 updated_at_ms 倒序，最新 session 在最前。
    assert_eq!(projection.sessions[0].session_id, "s-new");
    assert_eq!(projection.sessions.len(), 2);

    projection.set_connection(ConnectionState::Connected {
        instance_id: "instance-1".into(),
    });
    projection.select_session("s-1");

    assert!(projection.apply_event(&run_changed(1, "created")));
    assert!(projection.apply_event(&assistant_delta(2, "m-1", "Hello ")));
    assert!(projection.apply_event(&assistant_delta(3, "m-1", "world")));
    assert!(projection.apply_event(&run_changed(4, "completed")));
    // 终态清空 active_run_id，Composer 恢复可用。
    assert_eq!(projection.active_run_id, None);

    let texts: Vec<String> = projection
        .timeline
        .iter()
        .map(|entry| match &entry.kind {
            TimelineEntryKind::AssistantMessage { text } => format!("assistant:{text}"),
            TimelineEntryKind::RunState(state) => format!("run:{state}"),
            other => format!("other:{other:?}"),
        })
        .collect();
    assert_eq!(
        texts,
        vec![
            "run:run started".to_string(),
            "assistant:Hello world".to_string(),
            "run:run completed".to_string()
        ]
    );
}

#[test]
fn approval_card_clears_on_terminal_run() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    assert!(projection.apply_event(&event(
        1,
        json!({
            "type": "tool_approval_required",
            "data": {
                "run_id": "r-1",
                "tool_call_id": "call-1",
                "reason": "write_file · notes.txt · Approve workspace file write"
            }
        }),
    )));
    assert_eq!(
        projection
            .pending_approval
            .as_ref()
            .map(|item| item.tool_name.as_str()),
        Some("write_file")
    );
    assert!(projection.apply_event(&run_changed(2, "cancelled")));
    assert_eq!(projection.pending_approval, None);
}

#[test]
fn pending_model_is_overwritten_by_diagnostic() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    projection.set_pending_model("mock".into(), "model-2".into());
    assert!(projection.apply_event(&event(
        1,
        json!({
            "type": "diagnostic",
            "data": {
                "level": "info",
                "code": "model.switched",
                "message": "{\"to\":{\"provider\":\"mock\",\"model\":\"model-2\"}}"
            }
        }),
    )));
    assert_eq!(
        projection
            .selected_model
            .as_ref()
            .map(|(provider, model)| (provider.as_str(), model.as_str())),
        Some(("mock", "model-2"))
    );
    assert_eq!(projection.pending_model, None);
}

#[test]
fn sandbox_fallback_diagnostic_appears_on_timeline() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    assert!(projection.apply_event(&event(
        1,
        json!({
            "type": "diagnostic",
            "data": {
                "level": "info",
                "code": "sandbox.fallback",
                "message": "{\"message\":\"沙箱回退：isolation=soft backend=native_restricted\"}"
            }
        }),
    )));
    assert!(matches!(
        &projection.timeline[0].kind,
        TimelineEntryKind::RunState(text) if text.contains("沙箱回退")
    ));
}

fn snapshot_with_runs_and_approvals(runs: Vec<Value>, approvals: Vec<Value>) -> Snapshot {
    serde_json::from_value(json!({
        "instance_id": "instance-1",
        "snapshot_sequence": 0,
        "generated_at": 1,
        "sections": [
            {
                "kind": "session_tree",
                "revision": 1,
                "data": [session_entry("s-1", "One", 20)]
            },
            { "kind": "active_runs", "revision": 2, "data": runs },
            { "kind": "pending_tool_approvals", "revision": 3, "data": approvals }
        ]
    }))
    .expect("decode Snapshot")
}

#[test]
fn snapshot_active_runs_restore_cancel_target_on_select() {
    let snapshot = snapshot_with_runs_and_approvals(
        vec![json!({
            "run_id": "r-live",
            "session_id": "s-1",
            "started_at_ms": 1_700_000_000_000_u64
        })],
        vec![json!({
            "run_id": "r-live",
            "session_id": "s-1",
            "tool_call_id": "call-9",
            "tool_name": "write_file",
            "message": "Approve workspace file write",
            "relative_path": "notes.txt"
        })],
    );
    let mut projection = DesktopProjection::from_snapshot(&snapshot);
    assert_eq!(projection.active_run_id, None);
    projection.select_session("s-1");
    assert_eq!(projection.active_run_id.as_deref(), Some("r-live"));
    assert_eq!(projection.active_run_started_at_ms, Some(1_700_000_000_000));
    assert_eq!(
        projection
            .pending_approval
            .as_ref()
            .map(|item| item.tool_call_id.as_str()),
        Some("call-9")
    );
    assert_eq!(
        projection.run_status_label(1_700_000_045_000),
        "Task — tokens | Quota unavailable | — tok/s | Run 00:45"
    );
}

#[test]
fn session_live_status_running_needs_input_priority_and_plain() {
    let snapshot = snapshot_with_runs_and_approvals(
        vec![
            json!({
                "run_id": "r-run",
                "session_id": "s-run",
                "started_at_ms": 10_u64
            }),
            json!({
                "run_id": "r-both",
                "session_id": "s-both",
                "started_at_ms": 11_u64
            }),
        ],
        vec![
            json!({
                "run_id": "r-both",
                "session_id": "s-both",
                "tool_call_id": "c-both",
                "tool_name": "write_file",
                "message": "Approve workspace file write"
            }),
            json!({
                "run_id": "r-wait",
                "session_id": "s-wait",
                "tool_call_id": "c-wait",
                "tool_name": "bash",
                "message": "Approve command"
            }),
        ],
    );
    let mut projection = DesktopProjection::from_snapshot(&snapshot);
    assert_eq!(
        projection.session_live_status("s-run"),
        Some(SessionLiveStatus::Running)
    );
    // 与 Running 并存时 Needs input 优先。
    assert_eq!(
        projection.session_live_status("s-both"),
        Some(SessionLiveStatus::NeedsInput)
    );
    assert_eq!(
        projection.session_live_status("s-wait"),
        Some(SessionLiveStatus::NeedsInput)
    );
    // 无 live 状态：不声明语义（空心灰圆）。
    assert_eq!(projection.session_live_status("s-idle"), None);

    // live ToolApprovalRequired 归属当时的 active session。
    projection.select_session("s-1");
    assert!(projection.apply_event(&event(
        1,
        json!({
            "type": "tool_approval_required",
            "data": {
                "run_id": "r-live",
                "tool_call_id": "c-live",
                "reason": "bash · run.sh · Approve command"
            }
        }),
    )));
    assert_eq!(
        projection.session_live_status("s-1"),
        Some(SessionLiveStatus::NeedsInput)
    );

    // 无 session 归属字段的 snapshot pending 归 active session
    // （与 pending_for_active_session 同规）。
    let orphan: Snapshot = serde_json::from_value(json!({
        "instance_id": "instance-1",
        "snapshot_sequence": 0,
        "generated_at": 1,
        "sections": [
            {
                "kind": "pending_tool_approvals",
                "revision": 1,
                "data": [
                    {
                        "run_id": "r-x",
                        "tool_call_id": "c-x",
                        "tool_name": "bash",
                        "message": "Approve command"
                    }
                ]
            }
        ]
    }))
    .expect("decode Snapshot");
    let mut orphan_projection = DesktopProjection::from_snapshot(&orphan);
    assert_eq!(orphan_projection.session_live_status("s-any"), None);
    orphan_projection.select_session("s-1");
    assert_eq!(
        orphan_projection.session_live_status("s-1"),
        Some(SessionLiveStatus::NeedsInput)
    );
}

#[test]
fn run_status_label_uses_final_order_and_vertical_separators() {
    let mut projection = DesktopProjection::default();
    assert_eq!(
        projection.run_status_label(0),
        "Task — tokens | Quota unavailable | — tok/s | Run idle"
    );
    // active run 缺权威起始时间：时长诚实显示 —，不编造 mm:ss。
    projection.active_run_id = Some("r-unknown-start".into());
    assert_eq!(
        projection.run_status_label(0),
        "Task — tokens | Quota unavailable | — tok/s | Run —"
    );
}

/// R3 Wave A 审查修复（P1）：live RunChanged 非终态登记 run 成员（含
/// 非 active 的后台会话），终态按 run_id 移除并清 pendings——rail
/// 状态点不假阴性也不陈旧残留。
#[test]
fn session_live_status_tracks_live_run_changed_membership() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    // live 非终态：active 会话登记 Running。
    assert!(projection.apply_event(&run_changed(1, "created")));
    assert_eq!(
        projection.session_live_status("s-1"),
        Some(SessionLiveStatus::Running)
    );
    // 后台会话的 RunChanged 同样登记（不过 active 闸门）。
    let background = serde_json::from_value(json!({
        "api_version": { "major": 1, "minor": 1 },
        "instance_id": "instance-1",
        "event_id": "app-2",
        "global_sequence": 2,
        "stream": { "type": "session", "id": "s-2" },
        "stream_sequence": 2,
        "timestamp": 1_002,
        "source": { "type": "core" },
        "payload": { "type": "run_changed", "data": { "run_id": "r-2", "state": "created" } }
    }))
    .expect("decode AppEventEnvelope");
    assert!(projection.apply_event(&background));
    assert_eq!(
        projection.session_live_status("s-2"),
        Some(SessionLiveStatus::Running)
    );
    // 终态移除：蓝点不残留；同 run 的 pending 一并清除。
    assert!(projection.apply_event(&event(
        3,
        json!({
            "type": "tool_approval_required",
            "data": {
                "run_id": "r-1",
                "tool_call_id": "c-1",
                "reason": "bash · run.sh · Approve command"
            }
        }),
    )));
    assert_eq!(
        projection.session_live_status("s-1"),
        Some(SessionLiveStatus::NeedsInput)
    );
    assert!(projection.apply_event(&run_changed(4, "completed")));
    assert_eq!(projection.session_live_status("s-1"), None);
    assert!(projection.snapshot_pendings.is_empty());
    // 后台会话终态同样清除（用 completed：failed / interrupted 会按
    // R3 Wave B 语义派生 Blocked，另行专项测试）。
    let background_done = serde_json::from_value(json!({
        "api_version": { "major": 1, "minor": 1 },
        "instance_id": "instance-1",
        "event_id": "app-5",
        "global_sequence": 5,
        "stream": { "type": "session", "id": "s-2" },
        "stream_sequence": 5,
        "timestamp": 1_005,
        "source": { "type": "core" },
        "payload": { "type": "run_changed", "data": { "run_id": "r-2", "state": "completed" } }
    }))
    .expect("decode AppEventEnvelope");
    assert!(projection.apply_event(&background_done));
    assert_eq!(projection.session_live_status("s-2"), None);
    assert!(projection.active_runs.is_empty());
}

#[test]
fn note_session_run_marks_running_before_live_run_changed() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    projection.note_session_run("s-1", "r-1", 1_000);
    assert_eq!(
        projection.session_live_status("s-1"),
        Some(SessionLiveStatus::Running)
    );
    assert_eq!(projection.active_run_id.as_deref(), Some("r-1"));
    // 随后的 live RunChanged 不得重复登记。
    assert!(projection.apply_event(&run_changed(1, "created")));
    assert_eq!(
        projection
            .active_runs
            .iter()
            .filter(|run| run.run_id == "r-1")
            .count(),
        1
    );
    assert!(projection.apply_event(&run_changed(2, "completed")));
    assert_eq!(projection.session_live_status("s-1"), None);
}

/// R4 Wave B WS-4a：用户消息乐观回显——active session 回执即上屏，
/// 后续 wire 事件严格落在 echo 之后；非 active 不产生行。
#[test]
fn note_user_echo_appends_active_then_wire_events_land_after() {
    // entries 为空的理论分支：sequence 兜底 0。
    let mut fresh = DesktopProjection::default();
    fresh.select_session("s-1");
    assert!(fresh.note_user_echo("s-1", "r-0", "first", 1_000));
    assert_eq!(fresh.timeline.len(), 1);
    assert_eq!(fresh.timeline[0].sequence, 0);

    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    assert!(projection.apply_event(&assistant_delta(4, "m-1", "before")));
    assert!(projection.note_user_echo("s-1", "r-2", "hello", 5_000));
    let echo = projection.timeline.last().expect("echo appended");
    // 借用最大 wire sequence，不占号段、不进 seen。
    assert_eq!(echo.sequence, 4);
    assert_eq!(echo.event_id, "local-echo-r-2");
    assert_eq!(echo.run_id.as_deref(), Some("r-2"));
    assert_eq!(echo.timestamp, "5000");
    assert!(matches!(
        &echo.kind,
        TimelineEntryKind::UserMessage { text } if text == "hello"
    ));
    // 后续 wire 事件（sequence 严格更大）有序插到 echo 之后。
    assert!(projection.apply_event(&run_changed(5, "created")));
    assert_eq!(projection.timeline.len(), 3);
    assert_eq!(
        projection
            .timeline
            .last()
            .expect("wire after echo")
            .event_id,
        "app-5"
    );
    // 非 active session（发送后已切走）不 echo：重放会补。
    assert!(!projection.note_user_echo("s-2", "r-3", "away", 6_000));
    assert_eq!(projection.timeline.len(), 3);
}

/// R4 Wave B 评审 P2 修复：早死路径（engine 未报终态）的合成
/// RunChanged{Failed} 由宿主 publish_raw 分配 2^60 起的合成序号
/// （crates/app gui_host SYNTHETIC_SEQUENCE_BASE，不占真实持久化号段），
/// 有序插入落在用户消息乐观回显之后；seq-0 旧行为会插到时间线顶端。
#[test]
fn synthetic_terminal_after_user_echo_lands_at_bottom() {
    const SYNTHETIC_BASE: u64 = 1 << 60;
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    assert!(projection.apply_event(&assistant_delta(4, "m-1", "before")));
    assert!(projection.note_user_echo("s-1", "r-1", "blocked message", 5_000));
    assert!(projection.apply_event(&run_changed(SYNTHETIC_BASE, "failed")));
    assert_eq!(projection.timeline.len(), 3);
    assert_eq!(projection.timeline[0].event_id, "app-4");
    assert_eq!(projection.timeline[1].event_id, "local-echo-r-1");
    assert_eq!(
        projection.timeline[2].event_id,
        format!("app-{SYNTHETIC_BASE}")
    );
    assert!(matches!(
        &projection.timeline[2].kind,
        TimelineEntryKind::RunState(label) if label == "run failed"
    ));
    // 条目序列保持升序不变量（insert_entry 的 partition_point 前提）。
    assert!(
        projection.timeline[1].sequence <= projection.timeline[2].sequence,
        "entries must stay ascending by sequence: {:?}",
        projection
            .timeline
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>()
    );
}

#[test]
fn background_tool_approval_marks_needs_input_without_active_session() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    let background = serde_json::from_value(json!({
        "api_version": { "major": 1, "minor": 1 },
        "instance_id": "instance-1",
        "event_id": "app-2",
        "global_sequence": 2,
        "stream": { "type": "session", "id": "s-2" },
        "stream_sequence": 2,
        "timestamp": 1_002,
        "source": { "type": "core" },
        "payload": {
            "type": "tool_approval_required",
            "data": {
                "run_id": "r-2",
                "tool_call_id": "c-2",
                "reason": "bash · run.sh · Approve command"
            }
        }
    }))
    .expect("decode AppEventEnvelope");
    assert!(projection.apply_event(&background));
    assert_eq!(
        projection.session_live_status("s-2"),
        Some(SessionLiveStatus::NeedsInput)
    );
    assert_eq!(projection.session_live_status("s-1"), None);
    assert!(projection.pending_approval.is_none());
    let background_done = serde_json::from_value(json!({
        "api_version": { "major": 1, "minor": 1 },
        "instance_id": "instance-1",
        "event_id": "app-3",
        "global_sequence": 3,
        "stream": { "type": "session", "id": "s-2" },
        "stream_sequence": 3,
        "timestamp": 1_003,
        "source": { "type": "core" },
        "payload": {
            "type": "tool_completed",
            "data": {
                "run_id": "r-2",
                "tool_call_id": "c-2",
                "success": true
            }
        }
    }))
    .expect("decode AppEventEnvelope");
    assert!(projection.apply_event(&background_done));
    assert_eq!(projection.session_live_status("s-2"), None);
}

/// R3 Wave B：Blocked live 派生——最近一条 RunChanged 为终态且
/// failed / interrupted 记 Blocked；非终态与 completed / cancelled
/// 清除；优先级 NeedsInput > Running > Blocked；快照重建清空、
/// Replay 重放终态事件重新派生。
#[test]
fn session_live_status_blocked_derivation_and_clearing() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    assert_eq!(SessionLiveStatus::Blocked.label(), "Blocked");

    // 后台会话 failed / interrupted 终态 → Blocked。
    assert!(projection.apply_event(&session_event(
        1,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "failed" } }),
    )));
    assert_eq!(
        projection.session_live_status("s-2"),
        Some(SessionLiveStatus::Blocked)
    );
    // 已 Blocked 的重复终态无成员增量，返回 false 是正确语义。
    projection.apply_event(&session_event(
        2,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "interrupted" } }),
    ));
    assert_eq!(
        projection.session_live_status("s-2"),
        Some(SessionLiveStatus::Blocked)
    );

    // completed / cancelled 终态不算 Blocked（「最近一条」语义清除）。
    assert!(projection.apply_event(&session_event(
        3,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "completed" } }),
    )));
    assert_eq!(projection.session_live_status("s-2"), None);
    // 已清除后的再次非 Blocked 终态无增量，返回 false 是正确语义。
    projection.apply_event(&session_event(
        4,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-3", "state": "cancelled" } }),
    ));
    assert_eq!(projection.session_live_status("s-2"), None);

    // failed 后同 session 非终态 RunChanged 清除（新一轮 run 开始）。
    assert!(projection.apply_event(&session_event(
        5,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-4", "state": "failed" } }),
    )));
    assert_eq!(
        projection.session_live_status("s-2"),
        Some(SessionLiveStatus::Blocked)
    );
    assert!(projection.apply_event(&session_event(
        6,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-5", "state": "created" } }),
    )));
    // 新 run 登记成员：Running（优先级高于 Blocked，且 blocked 已清）。
    assert_eq!(
        projection.session_live_status("s-2"),
        Some(SessionLiveStatus::Running)
    );
    assert!(projection.apply_event(&session_event(
        6,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-5", "state": "completed" } }),
    )));
    assert_eq!(projection.session_live_status("s-2"), None);

    // 快照重建清空 blocked（wire 无终态来源，诚实）；Replay 重放终态
    // 事件可重新派生。
    assert!(projection.apply_event(&session_event(
        7,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-6", "state": "interrupted" } }),
    )));
    assert_eq!(
        projection.session_live_status("s-2"),
        Some(SessionLiveStatus::Blocked)
    );
    let snapshot = snapshot_with_sessions(vec![session_entry("s-2", "Two", 20)]);
    projection.apply_snapshot_required(&snapshot);
    assert_eq!(projection.session_live_status("s-2"), None);
    assert!(projection.apply_replay(&[session_event(
        8,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-7", "state": "failed" } }),
    )]));
    assert_eq!(
        projection.session_live_status("s-2"),
        Some(SessionLiveStatus::Blocked)
    );

    // 优先级：snapshot active run（Running）与 pending（NeedsInput）
    // 均压过 live 派生的 Blocked。
    let snapshot = snapshot_with_runs_and_approvals(
        vec![json!({
            "run_id": "r-run",
            "session_id": "s-run",
            "started_at_ms": 10_u64
        })],
        vec![json!({
            "run_id": "r-wait",
            "session_id": "s-wait",
            "tool_call_id": "c-wait",
            "tool_name": "bash",
            "message": "Approve command"
        })],
    );
    let mut priority = DesktopProjection::from_snapshot(&snapshot);
    assert!(priority.apply_event(&session_event(
        9,
        "s-run",
        json!({ "type": "run_changed", "data": { "run_id": "r-x", "state": "failed" } }),
    )));
    assert_eq!(
        priority.session_live_status("s-run"),
        Some(SessionLiveStatus::Running)
    );
    assert!(priority.apply_event(&session_event(
        10,
        "s-wait",
        json!({ "type": "run_changed", "data": { "run_id": "r-y", "state": "failed" } }),
    )));
    assert_eq!(
        priority.session_live_status("s-wait"),
        Some(SessionLiveStatus::NeedsInput)
    );
}

/// R3 Wave B：unread 通道——非 active session 的 Session-stream 活动
/// 事件记 unread；active 自身活动不记；select_session 清除；首连 /
/// 快照重建不产生（仍存标记保留、消失清除、新 session 无）；
/// Replay 重放后台活动同样记 unread。
#[test]
fn session_unread_marks_background_activity_and_clears_on_select() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    assert!(!projection.session_unread("s-2"));

    // 拍板集合逐类事件：RunChanged / AssistantDelta / ToolStarted /
    // ToolOutput / ToolCompleted / Diagnostic。
    let activities = [
        json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "created" } }),
        json!({ "type": "assistant_delta", "data": { "run_id": "r-2", "message_id": "m-1", "delta": "hi" } }),
        json!({ "type": "tool_started", "data": { "run_id": "r-2", "tool_call_id": "c-1", "name": "fs_read" } }),
        json!({ "type": "tool_output", "data": { "run_id": "r-2", "tool_call_id": "c-1", "delta": "chunk", "truncated": false } }),
        json!({ "type": "tool_completed", "data": { "run_id": "r-2", "tool_call_id": "c-1", "success": true } }),
        json!({ "type": "diagnostic", "data": { "level": "info", "code": "sandbox.fallback", "message": "{}" } }),
    ];
    for (index, payload) in activities.into_iter().enumerate() {
        projection.apply_event(&session_event(index as u64 + 1, "s-2", payload));
        assert!(
            projection.session_unread("s-2"),
            "activity #{index} should keep unread"
        );
    }
    // active session 自身的活动不记 unread。
    assert!(projection.apply_event(&assistant_delta(20, "m-9", "active")));
    assert!(!projection.session_unread("s-1"));

    // select_session（打开 / 切换）清除；切走后新活动重新记 unread。
    projection.select_session("s-2");
    assert!(!projection.session_unread("s-2"));
    projection.select_session("s-1");
    assert!(projection.apply_event(&session_event(
        21,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-9", "state": "created" } }),
    )));
    assert!(projection.session_unread("s-2"));

    // 快照重建：仍存 session 的 unread 保留；新增 session（本地新建
    // 同走快照）不产生 unread；全新投影（首连）无 unread。
    let snapshot = snapshot_with_sessions(vec![
        session_entry("s-1", "One", 20),
        session_entry("s-2", "Two", 10),
    ]);
    projection.apply_snapshot_required(&snapshot);
    assert!(projection.session_unread("s-2"));
    assert!(!projection.session_unread("s-new"));
    let fresh = DesktopProjection::from_snapshot(&snapshot);
    assert!(!fresh.session_unread("s-2"));

    // Replay 重放后台活动同样记 unread（断线期间发生的事用户未看过）。
    let mut replayed = DesktopProjection::default();
    replayed.select_session("s-1");
    assert!(replayed.apply_replay(&[session_event(
        1,
        "s-2",
        json!({ "type": "assistant_delta", "data": { "run_id": "r-2", "message_id": "m-1", "delta": "while away" } }),
    )]));
    assert!(replayed.session_unread("s-2"));
}

/// R3 Wave B 导航回归：断线（Disconnected）不清 active_session_id /
/// unread / blocked——连接态与导航态解耦，Reconnect 后可续。
#[test]
fn disconnect_preserves_active_unread_and_blocked() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    assert!(projection.apply_event(&session_event(
        1,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "failed" } }),
    )));
    assert!(projection.apply_event(&session_event(
        2,
        "s-3",
        json!({ "type": "assistant_delta", "data": { "run_id": "r-3", "message_id": "m-1", "delta": "bg" } }),
    )));
    projection.set_connection(ConnectionState::Disconnected {
        reason: "heartbeat timeout".into(),
    });
    assert_eq!(projection.active_session_id.as_deref(), Some("s-1"));
    assert_eq!(
        projection.session_live_status("s-2"),
        Some(SessionLiveStatus::Blocked)
    );
    assert!(projection.session_unread("s-3"));
    assert!(projection.show_reconnect());
}

/// R3 Wave B 导航回归：apply_snapshot_required 换基线——active 仍存
/// 则保留并清其 unread、消失则置 None；消失 session 的 unread 清除、
/// 仍存保留；blocked 清空（wire 无终态来源）。
#[test]
fn snapshot_required_keeps_active_clears_unread_and_prunes_vanished() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    assert!(projection.apply_event(&session_event(
        1,
        "s-2",
        json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "failed" } }),
    )));
    // unread 已记的后续活动事件无增量，返回 false 是正确语义。
    projection.apply_event(&session_event(
        2,
        "s-2",
        json!({ "type": "assistant_delta", "data": { "run_id": "r-2", "message_id": "m-1", "delta": "bg" } }),
    ));
    // 公开路径下 active 不产生 unread（select 即清）；直接置位以钉住
    // 「保留仍存 active 并清其 unread」这条拍板规则。
    projection.unread_sessions.insert("s-1".into());

    let keeps = snapshot_with_sessions(vec![
        session_entry("s-1", "One", 20),
        session_entry("s-new", "New", 10),
    ]);
    projection.apply_snapshot_required(&keeps);
    assert_eq!(projection.active_session_id.as_deref(), Some("s-1"));
    assert!(!projection.session_unread("s-1"));
    assert!(!projection.session_unread("s-2"));
    assert!(!projection.session_unread("s-new"));
    assert_eq!(projection.session_live_status("s-2"), None);

    // active 消失：置 None（UI 侧焦点回退 scope 触发器）。
    assert!(projection.apply_event(&session_event(
        3,
        "s-3",
        json!({ "type": "run_changed", "data": { "run_id": "r-3", "state": "created" } }),
    )));
    assert!(projection.session_unread("s-3"));
    let drops = snapshot_with_sessions(vec![session_entry("s-new", "New", 10)]);
    projection.apply_snapshot_required(&drops);
    assert_eq!(projection.active_session_id, None);
    assert!(!projection.session_unread("s-3"));
}

#[test]
fn reconnect_shows_only_for_disconnected_or_failed() {
    let mut projection = DesktopProjection::default();
    projection.connection = ConnectionState::Connecting;
    assert!(!projection.show_reconnect());
    projection.connection = ConnectionState::Connected {
        instance_id: "i-1".into(),
    };
    assert!(!projection.show_reconnect());
    projection.connection = ConnectionState::Disconnected {
        reason: "heartbeat timeout".into(),
    };
    assert!(projection.show_reconnect());
    projection.connection = ConnectionState::Failed {
        reason: "no token".into(),
    };
    assert!(projection.show_reconnect());
}

#[test]
fn workspace_empty_hint_requires_no_session_and_no_entries() {
    let mut projection = DesktopProjection::default();
    assert!(projection.workspace_empty_hint_visible());
    // 有 active session（即使条目尚未加载）不显示引导。
    projection.active_session_id = Some("s-1".into());
    assert!(!projection.workspace_empty_hint_visible());
    // Disconnected 保留旧条目时不显示引导。
    projection.active_session_id = None;
    projection.connection = ConnectionState::Disconnected {
        reason: "connection lost".into(),
    };
    projection.timeline.entries.push(TimelineEntry {
        sequence: 1,
        event_id: "e-1".into(),
        kind: TimelineEntryKind::UserMessage {
            text: "kept entries".into(),
        },
        fork_boundary: None,
        timestamp: "2026-08-27T00:00:00Z".into(),
        run_id: None,
    });
    assert!(!projection.workspace_empty_hint_visible());
}

#[test]
fn context_meter_uses_catalog_window_and_stays_honest() {
    let mut projection = DesktopProjection::default();
    assert_eq!(projection.context_meter_label(), "Context · unavailable");
    projection.set_models(vec![ModelEntry {
        provider_id: "glm-coding".into(),
        id: "glm-4.7".into(),
        display_name: "GLM 4.7".into(),
        context_window_tokens: Some(200_000),
    }]);
    projection.set_pending_model("glm-coding".into(), "glm-4.7".into());
    assert_eq!(projection.context_meter_label(), "Context · — / 200000");
}

fn day_ms(days: u64) -> u64 {
    days * 86_400_000
}

fn snapshot_with_named_workspaces(workspaces: Vec<Value>, sessions: Vec<Value>) -> Snapshot {
    serde_json::from_value(json!({
        "instance_id": "instance-1",
        "snapshot_sequence": 0,
        "generated_at": 1,
        "sections": [
            { "kind": "workspaces", "revision": 1, "data": workspaces },
            { "kind": "session_tree", "revision": 2, "data": sessions }
        ]
    }))
    .expect("decode Snapshot")
}

#[test]
fn task_rail_groups_date_then_project_and_keeps_unassigned() {
    let now = day_ms(20);
    let snapshot = snapshot_with_named_workspaces(
        vec![
            json!({ "id": "ws-alpha", "name": "Alpha" }),
            json!({ "id": "ws-beta", "name": "Beta" }),
        ],
        vec![
            session_entry_in("s-today-beta", "Beta today", now + 2, Some("ws-beta")),
            session_entry_in("s-today-alpha-new", "Alpha new", now + 1, Some("ws-alpha")),
            session_entry_in("s-today-alpha-old", "Alpha old", now, Some("ws-alpha")),
            session_entry_in("s-yesterday", "Y", now - day_ms(1), Some("ws-alpha")),
            session_entry_in("s-week", "W", now - day_ms(3), Some("ws-beta")),
            session_entry_in("s-old", "Old", now - day_ms(20), Some("ws-alpha")),
            session_entry("s-orphan", "Orphan", now - 10),
        ],
    );
    let projection = DesktopProjection::from_snapshot(&snapshot);
    assert_eq!(projection.workspace_name(None), UNASSIGNED_PROJECT);
    assert_eq!(projection.workspace_name(Some("ws-alpha")), "Alpha");

    let timeline = projection.timeline_groups(None, now + 3);
    assert_eq!(
        timeline
            .iter()
            .map(|group| group.bucket)
            .collect::<Vec<_>>(),
        vec![
            DateBucket::Today,
            DateBucket::Yesterday,
            DateBucket::Previous7Days,
            DateBucket::Earlier
        ]
    );
    let today = &timeline[0];
    assert_eq!(
        today
            .projects
            .iter()
            .map(|project| project.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Beta", "Alpha"]
    );
    assert_eq!(
        today.projects[1]
            .tasks
            .iter()
            .map(|task| task.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["s-today-alpha-new", "s-today-alpha-old"]
    );
    assert!(today.projects.iter().all(|project| {
        project
            .tasks
            .iter()
            .all(|task| task.workspace_id.as_deref() != Some("title-guess"))
    }));

    let earlier = timeline.last().expect("earlier");
    assert_eq!(earlier.projects[0].name, "Alpha");
    assert_eq!(earlier.projects[0].tasks[0].session_id, "s-old");

    let projects = projection.project_groups(None);
    assert_eq!(
        projects
            .iter()
            .map(|project| (project.name.as_str(), project.task_count()))
            .collect::<Vec<_>>(),
        vec![("Beta", 2), ("Alpha", 4), (UNASSIGNED_PROJECT, 1)]
    );
    assert!(projects.last().expect("unassigned").is_unassigned());
    assert_eq!(
        projects.last().expect("unassigned").tasks[0].session_id,
        "s-orphan"
    );

    let scoped = projection.project_groups(Some("ws-beta"));
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].name, "Beta");
    assert_eq!(scoped[0].task_count(), 2);
}

#[test]
fn task_rail_empty_state_and_scope_options() {
    let empty = DesktopProjection::default();
    assert!(empty.timeline_groups(None, 1).is_empty());
    assert!(empty.project_groups(None).is_empty());
    assert_eq!(
        empty.project_scope_options(),
        vec![(None, "All projects".into())]
    );

    let snapshot = snapshot_with_named_workspaces(
        vec![json!({ "id": "ws-default", "name": "default" })],
        vec![session_entry_in("s-1", "One", 10, Some("ws-default"))],
    );
    let projection = DesktopProjection::from_snapshot(&snapshot);
    assert_eq!(
        projection.project_scope_options(),
        vec![
            (None, "All projects".into()),
            (Some("ws-default".into()), "default".into())
        ]
    );
}

#[test]
fn grouping_switch_does_not_change_active_session() {
    let snapshot = snapshot_with_named_workspaces(
        vec![json!({ "id": "ws-default", "name": "default" })],
        vec![
            session_entry_in("s-1", "One", 20, Some("ws-default")),
            session_entry_in("s-2", "Two", 10, Some("ws-default")),
        ],
    );
    let mut projection = DesktopProjection::from_snapshot(&snapshot);
    projection.select_session("s-2");
    let before = projection.active_session_id.clone();
    let _timeline = projection.timeline_groups(None, 20);
    let _projects = projection.project_groups(None);
    assert_eq!(projection.active_session_id, before);
    assert!(projection
        .project_groups(None)
        .iter()
        .flat_map(|project| &project.tasks)
        .any(|task| task.session_id == "s-2"));
}

fn resume_outcome(
    disposition: ResumeDisposition,
    replayed: Vec<AppEventEnvelope>,
    snapshot: Option<Snapshot>,
) -> ResumeOutcome {
    ResumeOutcome {
        disposition,
        replayed,
        snapshot,
    }
}

#[test]
fn session_tree_accepts_flat_sessions_and_branch_nodes() {
    let flat = snapshot_with_sessions(vec![session_entry("s-1", "One", 20)]);
    let projection = DesktopProjection::from_snapshot(&flat);
    assert_eq!(projection.sessions[0].session_id, "s-1");
    assert!(projection.sessions[0].active);
    assert_eq!(projection.sessions[0].parent_branch_id, None);

    let branched = snapshot_with_sessions(vec![json!({
        "branch_id": "br-2",
        "parent_branch_id": "br-1",
        "forked_from_event_id": "evt-9",
        "active": true,
        "title": "Forked",
        "updated_at_ms": 40,
        "workspace_id": "ws-default"
    })]);
    let projection = DesktopProjection::from_snapshot(&branched);
    assert_eq!(projection.sessions[0].session_id, "br-2");
    assert_eq!(
        projection.sessions[0].parent_branch_id.as_deref(),
        Some("br-1")
    );
    assert_eq!(
        projection.sessions[0].forked_from_event_id.as_deref(),
        Some("evt-9")
    );

    let wrapped = snapshot_with_named_workspaces(
        vec![json!({ "id": "ws-default", "name": "default" })],
        vec![],
    );
    let mut wrapped_json = serde_json::to_value(&wrapped).expect("snapshot json");
    wrapped_json["sections"][1]["data"] = json!({
        "nodes": [{
            "branch_id": "br-wrap",
            "parent_branch_id": null,
            "forked_from_event_id": null,
            "active": false,
            "name": "Wrapped",
            "updated_at_ms": 5
        }]
    });
    let wrapped: Snapshot = serde_json::from_value(wrapped_json).expect("decode wrapped");
    let projection = DesktopProjection::from_snapshot(&wrapped);
    assert_eq!(projection.sessions[0].session_id, "br-wrap");
    assert!(!projection.sessions[0].active);
    assert_eq!(projection.sessions[0].title, "Wrapped");
}

/// 与 `event` 相同，但 stream 指向给定 session/branch（wire 无 branch
/// 字段，分支事件以分支自身的 stream id 表达）。
fn session_event(sequence: u64, session: &str, payload: Value) -> AppEventEnvelope {
    serde_json::from_value(json!({
        "api_version": { "major": 1, "minor": 1 },
        "instance_id": "instance-1",
        "event_id": format!("app-{sequence}"),
        "global_sequence": sequence,
        "stream": { "type": "session", "id": session },
        "stream_sequence": sequence,
        "timestamp": 1_000 + sequence,
        "source": { "type": "core" },
        "payload": payload
    }))
    .expect("decode AppEventEnvelope")
}

#[test]
fn switching_branch_within_session_resets_timeline_baseline() {
    // R6：切支沿用 select_session -> reset_baseline -> reload，不加 wire
    // 字段；同一 session 换 branch 也无条件清 entries/seen/anchors。
    let snapshot = snapshot_with_sessions(vec![json!({
        "session_id": "s-1",
        "title": "Branching session",
        "updated_at_ms": 20,
        "active_branch": "main",
        "workspace_id": "ws-default"
    })]);
    let mut projection = DesktopProjection::from_snapshot(&snapshot);
    projection.select_session("s-1");

    let delta = |sequence: u64, session: &str, message_id: &str, text: &str| {
        session_event(
            sequence,
            session,
            json!({
                "type": "assistant_delta",
                "data": { "run_id": "r-1", "message_id": message_id, "delta": text }
            }),
        )
    };
    let tool_started = |sequence: u64, session: &str| {
        session_event(
            sequence,
            session,
            json!({
                "type": "tool_started",
                "data": { "run_id": "r-1", "tool_call_id": "call-1", "name": "fs_read" }
            }),
        )
    };
    let tool_output = |sequence: u64, session: &str| {
        session_event(
            sequence,
            session,
            json!({
                "type": "tool_output",
                "data": {
                    "run_id": "r-1",
                    "tool_call_id": "call-1",
                    "delta": "chunk",
                    "truncated": false
                }
            }),
        )
    };
    let run_completed = |sequence: u64, session: &str| {
        session_event(
            sequence,
            session,
            json!({ "type": "run_changed", "data": { "run_id": "r-1", "state": "completed" } }),
        )
    };

    // 基线：assistant committed tombstone、tool 锚点、run 终态边界。
    assert!(projection.apply_event(&delta(2, "s-1", "m-1", "Hello")));
    projection.apply_timeline_page(&page(
        vec![history_item(
            4,
            "assistant_message",
            json!({ "text": "Hello world" }),
        )],
        true,
    ));
    assert!(!projection.apply_event(&delta(3, "s-1", "m-1", " late")));
    assert!(projection.apply_event(&tool_started(10, "s-1")));
    assert!(projection.apply_event(&tool_output(11, "s-1")));
    assert!(projection.apply_event(&run_completed(12, "s-1")));
    assert_eq!(projection.timeline.len(), 3);
    assert!(projection.timeline[2].is_fork_boundary());

    // 同 session 换 branch：entries / seen / assistant / tool anchors 全清。
    // SessionForked 后 controller 以同一个 session_id 重新 open；active branch
    // 只存在 host/storage，不进 wire，因此这里必须用同 id 再次选中。
    projection.select_session("s-1");
    assert!(projection.timeline.is_empty());
    assert_eq!(projection.active_session_id.as_deref(), Some("s-1"));

    // seen 已清：同 sequence 重放不判重；tombstone 已清：同 message delta
    // 不再被吞；tool 锚点已清：重放重建并回填。
    assert!(projection.apply_event(&delta(2, "s-1", "m-1", "Hello")));
    assert!(projection.apply_event(&delta(3, "s-1", "m-1", " again")));
    assert!(projection.apply_event(&tool_started(10, "s-1")));
    assert!(projection.apply_event(&tool_output(11, "s-1")));
    assert!(projection.apply_event(&run_completed(12, "s-1")));
    assert_eq!(projection.timeline.len(), 3);
    let texts: Vec<String> = projection
        .timeline
        .iter()
        .map(|entry| match &entry.kind {
            TimelineEntryKind::AssistantMessage { text } => format!("assistant:{text}"),
            TimelineEntryKind::ToolCall { detail, .. } => format!("tool:{detail:?}"),
            TimelineEntryKind::RunState(state) => format!("run:{state}"),
            other => format!("other:{other:?}"),
        })
        .collect();
    assert_eq!(
        texts,
        vec![
            "assistant:Hello again".to_string(),
            "tool:Some(\"chunk\")".to_string(),
            "run:run completed".to_string(),
        ]
    );
}

fn terminal_output(sequence: u64, terminal: &str, delta: &str) -> AppEventEnvelope {
    serde_json::from_value(json!({
        "api_version": { "major": 1, "minor": 1 },
        "instance_id": "instance-1",
        "event_id": format!("term-{sequence}"),
        "global_sequence": sequence,
        "stream": { "type": "terminal", "id": terminal },
        "stream_sequence": sequence,
        "timestamp": 1_000 + sequence,
        "source": { "type": "core" },
        "payload": {
            "type": "terminal_output",
            "data": { "terminal_session_id": terminal, "delta": delta }
        }
    }))
    .expect("decode TerminalOutput")
}

fn terminal_exited(sequence: u64, terminal: &str, reason: &str) -> AppEventEnvelope {
    serde_json::from_value(json!({
        "api_version": { "major": 1, "minor": 3 },
        "instance_id": "instance-1",
        "event_id": format!("term-exit-{sequence}"),
        "global_sequence": sequence,
        "stream": { "type": "terminal", "id": terminal },
        "stream_sequence": sequence,
        "timestamp": 1_000 + sequence,
        "source": { "type": "core" },
        "payload": {
            "type": "terminal_exited",
            "data": {
                "terminal_session_id": terminal,
                "exit_code": 0,
                "reason": reason
            }
        }
    }))
    .expect("decode TerminalExited")
}

/// ADR-045：live 终态事件即时刷新（不等断连重连快照），且与快照终态
/// 同口径——旧输出不得复活终态终端。
#[test]
fn terminal_exited_event_marks_terminal_stale_and_blocks_resurrection() {
    let mut projection = DesktopProjection::default();
    projection.workspace_id = Some("ws-a".into());
    projection.apply_terminal_created("ws-a".into(), "term-a".into());
    assert_eq!(
        projection.terminal.runtime_state.as_deref(),
        Some("running")
    );

    assert!(projection.apply_event(&terminal_exited(1, "term-a", "killed")));
    assert_eq!(projection.terminal.runtime_state.as_deref(), Some("killed"));
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Stale { .. }
    ));
    // 迟到输出仍追加（保留现场），但不得复活 running/Ready。
    assert!(projection.apply_event(&terminal_output(2, "term-a", "late")));
    assert_eq!(projection.terminal.runtime_state.as_deref(), Some("killed"));
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Stale { .. }
    ));
}

/// ADR-045：Close 清理回执后本地移除条目；当前终端回到 not started。
#[test]
fn remove_terminal_clears_current_terminal_after_close() {
    let mut projection = DesktopProjection::default();
    projection.workspace_id = Some("ws-a".into());
    projection.apply_terminal_created("ws-a".into(), "term-a".into());
    projection.apply_event(&terminal_exited(1, "term-a", "exited"));

    assert!(!projection.remove_terminal("term-unknown"));
    assert!(projection.remove_terminal("term-a"));
    assert!(projection.terminals.is_empty());
    assert_eq!(projection.terminal.session_id, None);
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Stale { .. }
    ));
}

#[test]
fn terminal_output_appends_without_vt100() {
    let mut projection = DesktopProjection::default();
    assert!(!projection.apply_event(&terminal_output(1, "term-1", "hello")));
    assert!(!projection.apply_event(&terminal_output(2, "term-1", "\nworld")));
    assert_eq!(projection.terminal.session_id, None);
    assert_eq!(projection.terminals[0].output, "hello\nworld");
    assert!(!projection.apply_event(&terminal_output(3, "term-other", "nope")));
    assert!(projection.terminal.output.is_empty());
}

#[test]
fn terminal_created_preserves_output_that_arrived_before_receipt() {
    let mut projection = DesktopProjection::default();
    projection.workspace_id = Some("ws-a".into());
    assert!(!projection.apply_event(&terminal_output(1, "term-a", "shell$ ")));
    assert!(projection.terminal.output.is_empty());
    projection.apply_terminal_created("ws-a".into(), "term-a".into());
    assert_eq!(projection.terminal.output, "shell$ ");
    assert_eq!(projection.terminals[0].output, "shell$ ");
    assert_eq!(projection.terminal.workspace_id.as_deref(), Some("ws-a"));
    assert_eq!(
        projection.terminal.availability,
        TerminalAvailability::Ready
    );
}

#[test]
fn terminal_output_waits_for_workspace_receipt_before_becoming_visible() {
    let mut projection = DesktopProjection::default();
    projection.workspace_id = Some("ws-b".into());
    assert!(!projection.apply_event(&terminal_output(1, "term-a", "shell$ ")));
    assert_eq!(projection.terminal.workspace_id.as_deref(), None);

    projection.apply_terminal_created("ws-a".into(), "term-a".into());
    assert_eq!(projection.terminal.session_id, None);
    projection.select_terminal_for_workspace(Some("ws-a"));
    assert_eq!(projection.terminal.session_id.as_deref(), Some("term-a"));
    assert_eq!(projection.terminal.output, "shell$ ");
}

#[test]
fn terminal_selection_prefers_current_then_uses_deterministic_fallback() {
    let mut projection = DesktopProjection::default();
    let terminal = |id: &str| TerminalState {
        session_id: Some(id.into()),
        workspace_id: Some("ws-a".into()),
        runtime_state: Some("running".into()),
        availability: TerminalAvailability::Ready,
        ..TerminalState::default()
    };
    projection.terminals = vec![terminal("term-b"), terminal("term-a")];
    projection.terminal = terminal("term-b");
    assert!(!projection.select_terminal_for_workspace(Some("ws-a")));
    assert_eq!(projection.terminal.session_id.as_deref(), Some("term-b"));

    projection.terminal = terminal("term-other");
    projection.terminal.workspace_id = Some("ws-b".into());
    assert!(projection.select_terminal_for_workspace(Some("ws-a")));
    assert_eq!(projection.terminal.session_id.as_deref(), Some("term-a"));
}

#[test]
fn terminal_snapshot_parses_all_fields_and_selects_active_workspace() {
    let snapshot: Snapshot = serde_json::from_value(json!({
        "instance_id": "instance-1", "snapshot_sequence": 0, "generated_at": 1,
        "sections": [
            { "kind": "workspaces", "revision": 1, "data": [
                { "id": "ws-a", "name": "A" }, { "id": "ws-b", "name": "B" }
            ]},
            { "kind": "session_tree", "revision": 1, "data": [
                { "session_id": "s-b", "title": "B task", "updated_at_ms": 1,
                  "workspace_id": "ws-b" }
            ]},
            { "kind": "terminal_sessions", "revision": 2, "data": [
                { "terminal_session_id": "term-a", "owner_session": "ws-a",
                  "state": "running", "columns": 120, "rows": 40, "dropped_events": 3 },
                { "terminal_session_id": "term-b", "owner_session": "ws-b",
                  "state": "exited", "columns": 90, "rows": 30, "dropped_events": 0 }
            ]}
        ]
    }))
    .expect("terminal snapshot");
    let mut projection = DesktopProjection::from_snapshot(&snapshot);
    assert_eq!(projection.terminals.len(), 2);
    assert_eq!(projection.terminal.session_id.as_deref(), Some("term-a"));
    assert_eq!(
        (projection.terminal.columns, projection.terminal.rows),
        (120, 40)
    );
    assert_eq!(projection.terminal.dropped_events, 3);
    assert_eq!(
        projection.terminal.availability,
        TerminalAvailability::Ready
    );
    projection.select_session("s-b");
    assert_eq!(projection.active_workspace_id(), Some("ws-b"));
    assert_eq!(projection.terminal.session_id.as_deref(), Some("term-b"));
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Stale { .. }
    ));
}

/// G3：快照恢复解析 Host 回报的 workspace 相对 cwd；缺键（旧 Host /
/// 记账缺失）时诚实显示 unknown，不臆造工作区根 "."。
#[test]
fn terminal_snapshot_restores_cwd_or_shows_unknown() {
    let with_cwd = TerminalState::from_snapshot(&json!({
        "terminal_session_id": "term-a",
        "owner_session": "ws-a",
        "state": "running",
        "columns": 80,
        "rows": 24,
        "cwd": "src/app"
    }))
    .expect("terminal with cwd");
    assert_eq!(with_cwd.cwd, "src/app");

    let without_cwd = TerminalState::from_snapshot(&json!({
        "terminal_session_id": "term-b",
        "owner_session": "ws-a",
        "state": "running"
    }))
    .expect("terminal without cwd");
    assert_eq!(without_cwd.cwd, TERMINAL_CWD_UNKNOWN);

    let empty_cwd = TerminalState::from_snapshot(&json!({
        "terminal_session_id": "term-c",
        "owner_session": "ws-a",
        "state": "running",
        "cwd": ""
    }))
    .expect("terminal with empty cwd");
    assert_eq!(empty_cwd.cwd, TERMINAL_CWD_UNKNOWN);
}

/// G2：write/resize 瞬态失败不把 running 终端锁死（可用性保持
/// Ready，报错走 status_hint）；非 running 终端保留 Failed 归因。
#[test]
fn terminal_io_failure_keeps_running_terminal_operable() {
    let mut projection = DesktopProjection::default();
    projection.workspace_id = Some("ws-a".into());
    projection.apply_terminal_created("ws-a".into(), "term-a".into());

    assert!(!projection.note_terminal_io_failed("term-a", "transient write error"));
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Ready
    ));

    projection.terminals[0].runtime_state = Some("exited".into());
    projection.terminal.runtime_state = Some("exited".into());
    assert!(projection.note_terminal_io_failed("term-a", "io error after exit"));
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Failed { .. }
    ));
}

#[test]
fn up_to_date_snapshot_keeps_timeline_and_terminal_exit_beats_replayed_output() {
    let initial: Snapshot = serde_json::from_value(json!({
        "instance_id": "instance-1", "snapshot_sequence": 1, "generated_at": 1,
        "sections": [
            { "kind": "workspaces", "revision": 1, "data": [
                { "id": "ws-a", "name": "A" }
            ]},
            { "kind": "session_tree", "revision": 1, "data": [
                { "session_id": "s-1", "title": "A task", "updated_at_ms": 1,
                  "workspace_id": "ws-a" }
            ]},
            { "kind": "terminal_sessions", "revision": 1, "data": [
                { "terminal_session_id": "term-a", "owner_session": "ws-a",
                  "state": "running", "columns": 80, "rows": 24 }
            ]}
        ]
    }))
    .expect("initial terminal snapshot");
    let mut projection = DesktopProjection::from_snapshot(&initial);
    projection.select_session("s-1");
    assert!(projection.apply_event(&run_changed(1, "created")));
    let timeline_len = projection.timeline.len();
    projection.set_connection(ConnectionState::Disconnected {
        reason: "socket closed".into(),
    });

    let exited: Snapshot = serde_json::from_value(json!({
        "instance_id": "instance-1", "snapshot_sequence": 1, "generated_at": 2,
        "sections": [
            { "kind": "workspaces", "revision": 2, "data": [
                { "id": "ws-a", "name": "A" }
            ]},
            { "kind": "session_tree", "revision": 2, "data": [
                { "session_id": "s-1", "title": "A task", "updated_at_ms": 1,
                  "workspace_id": "ws-a" }
            ]},
            { "kind": "terminal_sessions", "revision": 2, "data": [
                { "terminal_session_id": "term-a", "owner_session": "ws-a",
                  "state": "exited", "columns": 80, "rows": 24 }
            ]}
        ]
    }))
    .expect("exited terminal snapshot");
    let apply = projection.apply_resume_outcome(
        &resume_outcome(
            ResumeDisposition::UpToDate {
                current_sequence: pawork_client::GlobalSequence(1),
            },
            Vec::new(),
            None,
        ),
        &exited,
    );
    assert_eq!(apply, ResumeApply::Unchanged);
    assert_eq!(projection.timeline.len(), timeline_len);
    assert_eq!(projection.terminal.runtime_state.as_deref(), Some("exited"));
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Stale { .. }
    ));

    assert!(projection.apply_event(&terminal_output(2, "term-a", "late output")));
    assert_eq!(projection.terminal.output, "late output");
    assert_eq!(projection.terminal.runtime_state.as_deref(), Some("exited"));
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Stale { .. }
    ));
}

#[test]
fn terminal_disconnect_and_failure_are_honest_states() {
    let mut projection = DesktopProjection::default();
    projection.workspace_id = Some("ws-a".into());
    projection.apply_terminal_created("ws-a".into(), "term-a".into());
    assert!(!projection.terminal.resize_confirmed);
    assert!(projection.apply_terminal_resize("term-a", 100, 30));
    assert!(projection.terminal.resize_confirmed);
    assert_eq!(
        (projection.terminal.columns, projection.terminal.rows),
        (100, 30)
    );
    assert!(projection.mark_terminal_failed("term-a", "write denied"));
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Failed { .. }
    ));
    assert!(matches!(
        projection.terminals[0].availability,
        TerminalAvailability::Failed { .. }
    ));
    projection.set_connection(ConnectionState::Disconnected {
        reason: "socket closed".into(),
    });
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Stale { .. }
    ));
    projection.set_connection(ConnectionState::Connected {
        instance_id: "instance-1".into(),
    });
    assert_eq!(
        projection.terminal.availability,
        TerminalAvailability::Ready
    );

    projection.mark_terminal_create_failed("ws-b", "policy denied");
    assert_eq!(projection.terminal.workspace_id.as_deref(), Some("ws-a"));
    projection.select_terminal_for_workspace(Some("ws-b"));
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Failed { .. }
    ));
    projection.apply_terminal_created("ws-b".into(), "term-b".into());
    assert_eq!(projection.terminal.session_id.as_deref(), Some("term-b"));
    assert!(matches!(
        projection.terminal.availability,
        TerminalAvailability::Ready
    ));
    assert_eq!(
        projection
            .terminals
            .iter()
            .filter(|terminal| terminal.workspace_id.as_deref() == Some("ws-b"))
            .count(),
        1
    );
}

fn tool_started(sequence: u64, tool_call_id: &str, name: &str) -> AppEventEnvelope {
    event(
        sequence,
        json!({
            "type": "tool_started",
            "data": {
                "run_id": "r-1",
                "tool_call_id": tool_call_id,
                "name": name
            }
        }),
    )
}

fn tool_output(sequence: u64, tool_call_id: &str, delta: &str) -> AppEventEnvelope {
    event(
        sequence,
        json!({
            "type": "tool_output",
            "data": {
                "run_id": "r-1",
                "tool_call_id": tool_call_id,
                "delta": delta,
                "truncated": false
            }
        }),
    )
}

fn tool_completed(sequence: u64, tool_call_id: &str, success: bool) -> AppEventEnvelope {
    event(
        sequence,
        json!({
            "type": "tool_completed",
            "data": {
                "run_id": "r-1",
                "tool_call_id": tool_call_id,
                "success": success
            }
        }),
    )
}

#[test]
fn live_tool_output_fills_running_entry() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    assert!(projection.apply_event(&tool_started(10, "call-1", "fs_read")));
    assert!(projection.apply_event(&tool_output(11, "call-1", "chunk-a")));
    assert!(matches!(
        &projection.timeline[0].kind,
        TimelineEntryKind::ToolCall { name, status, detail }
            if name == "fs_read" && status == "running" && detail.as_deref() == Some("chunk-a")
    ));
    projection.apply_timeline_page(&page(
        vec![history_item(
            11,
            "tool_output",
            json!({ "tool_name": "fs_read", "text": "chunk-a" }),
        )],
        false,
    ));
    let tools: Vec<_> = projection
        .timeline
        .iter()
        .filter_map(|entry| match &entry.kind {
            TimelineEntryKind::ToolCall {
                name,
                status,
                detail,
            } => Some((name.as_str(), status.as_str(), detail.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(tools, vec![("fs_read", "running", Some("chunk-a"))]);
}

#[test]
fn history_approval_events_leave_traces() {
    let mut projection = DesktopProjection::default();
    projection.select_session("s-1");
    projection.apply_timeline_page(&page(
        vec![
            history_item(
                1,
                "approval_requested",
                json!({ "tool_name": "write_file", "text": "edit src/lib.rs" }),
            ),
            history_item(2, "approval_responded", json!({ "status": "approve_once" })),
        ],
        true,
    ));
    let labels: Vec<&str> = projection
        .timeline
        .iter()
        .filter_map(|entry| match &entry.kind {
            TimelineEntryKind::RunState(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        labels
            .iter()
            .any(|text| text.contains("approval requested") && text.contains("write_file")),
        "history approval_requested should remain, got {labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|text| text.contains("approval approve_once")),
        "history approval_responded should remain, got {labels:?}"
    );
    assert!(projection.pending_approval.is_none());
}

#[test]
fn timeline_repagination_keeps_outstanding_pending_approval() {
    // D3：重开会话重放历史时，其它 run 的 approval_responded /
    // tool_completed 不能改写 snapshot 权威的未决议审批（含
    // tool_call_id，供冻结 tool_approve 使用）。
    let snapshot = snapshot_with_runs_and_approvals(
        vec![json!({
            "run_id": "r-2",
            "session_id": "s-1",
            "started_at_ms": 20_u64
        })],
        vec![json!({
            "run_id": "r-2",
            "session_id": "s-1",
            "tool_call_id": "call-2",
            "tool_name": "run_command",
            "message": "Approve command"
        })],
    );
    let mut projection = DesktopProjection::from_snapshot(&snapshot);
    projection.select_session("s-1");
    assert!(projection.pending_approval.is_some());
    projection.apply_timeline_page(&page(
        vec![
            history_item(
                1,
                "approval_requested",
                json!({ "tool_name": "run_command" }),
            ),
            history_item(2, "approval_responded", json!({ "status": "approve_once" })),
            history_item(
                3,
                "tool_completed",
                json!({ "tool_name": "run_command", "status": "succeeded" }),
            ),
            history_item(
                4,
                "approval_requested",
                json!({ "tool_name": "run_command", "run_id": "r-2" }),
            ),
        ],
        true,
    ));
    let pending = projection
        .pending_approval
        .as_ref()
        .expect("outstanding approval must survive timeline repagination");
    assert_eq!(pending.run_id, "r-2");
    assert_eq!(pending.tool_call_id, "call-2");
}

#[test]
fn timeline_earlier_items_in_same_run_keep_later_pending_approval() {
    // 同一 run 可串行执行多个工具：更早工具的 responded/completed
    // 历史条目不能清除 snapshot 中更晚工具的当前审批。历史 wire 的
    // approval_responded 不含 tool_call_id，无法安全做工具级清除。
    let snapshot = snapshot_with_runs_and_approvals(
        vec![],
        vec![json!({
            "run_id": "r-1",
            "session_id": "s-1",
            "tool_call_id": "call-2",
            "tool_name": "run_command",
            "message": "Approve command"
        })],
    );
    let mut projection = DesktopProjection::from_snapshot(&snapshot);
    projection.select_session("s-1");
    assert!(projection.pending_approval.is_some());
    projection.apply_timeline_page(&page(
        vec![
            history_item(1, "approval_responded", json!({ "status": "approve_once" })),
            history_item(
                2,
                "tool_completed",
                json!({ "tool_name": "read_file", "status": "succeeded" }),
            ),
            history_item(
                3,
                "approval_requested",
                json!({ "tool_name": "run_command" }),
            ),
        ],
        true,
    ));
    let pending = projection
        .pending_approval
        .as_ref()
        .expect("later pending approval in the same run must survive history replay");
    assert_eq!(pending.run_id, "r-1");
    assert_eq!(pending.tool_call_id, "call-2");
}

/// R1 Wave B Phase C：读取 `fixtures/ui/expected/snapshot.json`（由
/// `ui_fixture snapshot-dump` 生成的归一化 golden，再生步骤见
/// `fixtures/ui/README.md`），断言 DesktopProjection 分组与状态。
#[test]
fn ui_fixture_expected_snapshot_rebuilds_groups_and_status() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/ui/expected/snapshot.json"
    );
    let raw = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("读取 {path} 失败（{error}）：golden 由 ui_fixture snapshot-dump 生成，再生步骤见 fixtures/ui/README.md")
    });
    let snapshot: Snapshot = serde_json::from_str(&raw).expect("decode expected snapshot");
    // FIXTURE_NOW_MS 锚点恰为 UTC 午夜；取锚点前 1ms 作参照 now，
    // 使 seed 中 -2h/-2.5h 同日偏移落 Today、四桶齐全（与 app 侧
    // tests/ui_fixture_projection.rs 同一分桶口径）。
    let now_ms = 1_767_225_599_999_u64;

    let mut projection = DesktopProjection::from_snapshot(&snapshot);

    // 会话清单：7 个种子会话全量恢复，最新在前，绑定各自 workspace。
    assert_eq!(projection.sessions.len(), 7);
    assert!(projection.sessions.iter().all(|session| session.active));
    let ids: BTreeSet<&str> = projection
        .sessions
        .iter()
        .map(|session| session.session_id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "fx-ses-alpha-today",
            "fx-ses-alpha-yesterday",
            "fx-ses-beta-pending",
            "fx-ses-beta-toolfailed",
            "fx-ses-beta-cancelled",
            "fx-ses-alpha-longtitle",
            "fx-ses-beta-long",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
    );
    assert_eq!(projection.sessions[0].session_id, "fx-ses-alpha-today");
    assert_eq!(projection.sessions[0].title, "Refactor launcher tabs");
    let long_title = projection
        .sessions
        .iter()
        .find(|session| session.session_id == "fx-ses-alpha-longtitle")
        .expect("long title session");
    assert!(long_title.title.chars().count() >= 200);
    // 嵌套 fn 而非闭包：返回引用派生自引用参数时，闭包的 Fn 签名
    // 只能固定单一生命周期（error: lifetime may not live long enough），
    // fn 的省略生命周期天然 higher-ranked。
    fn session_workspace<'a>(projection: &'a DesktopProjection, id: &str) -> Option<&'a str> {
        projection
            .sessions
            .iter()
            .find(|session| session.session_id == id)
            .and_then(|session| session.workspace_id.as_deref())
    }
    assert_eq!(
        session_workspace(&projection, "fx-ses-alpha-today"),
        Some("fx-alpha-app")
    );
    assert_eq!(
        session_workspace(&projection, "fx-ses-beta-pending"),
        Some("fx-beta-lib")
    );

    // TaskRail 分组：日期四桶齐全，桶内会话集合与 seed offsets 一致。
    let timeline = projection.timeline_groups(None, now_ms);
    assert_eq!(
        timeline
            .iter()
            .map(|group| group.bucket)
            .collect::<Vec<_>>(),
        vec![
            DateBucket::Today,
            DateBucket::Yesterday,
            DateBucket::Previous7Days,
            DateBucket::Earlier,
        ]
    );
    fn ids_of(group: &TaskRailDateGroup) -> Vec<&str> {
        let mut ids: Vec<&str> = group
            .projects
            .iter()
            .flat_map(|project| project.tasks.iter().map(|task| task.session_id.as_str()))
            .collect();
        ids.sort_unstable();
        ids
    }
    assert_eq!(
        ids_of(&timeline[0]),
        vec!["fx-ses-alpha-today", "fx-ses-beta-pending"]
    );
    assert_eq!(ids_of(&timeline[1]), vec!["fx-ses-alpha-yesterday"]);
    assert_eq!(
        ids_of(&timeline[2]),
        vec!["fx-ses-beta-long", "fx-ses-beta-toolfailed"]
    );
    assert_eq!(
        ids_of(&timeline[3]),
        vec!["fx-ses-alpha-longtitle", "fx-ses-beta-cancelled"]
    );

    // Today 桶内项目分组：按最新活动排序；wire workspaces 段当前只携带
    // 主 workspace，beta 组名回退 id（诚实回退，不臆造名字）。
    let today = &timeline[0];
    assert_eq!(today.projects.len(), 2);
    assert_eq!(
        today.projects[0].workspace_id.as_deref(),
        Some("fx-alpha-app")
    );
    assert_eq!(today.projects[0].name, "alpha-app");
    assert_eq!(
        today.projects[0]
            .tasks
            .iter()
            .map(|task| task.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["fx-ses-alpha-today"]
    );
    assert_eq!(
        today.projects[1].workspace_id.as_deref(),
        Some("fx-beta-lib")
    );
    assert_eq!(today.projects[1].name, "fx-beta-lib");
    assert_eq!(
        today.projects[1]
            .tasks
            .iter()
            .map(|task| task.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["fx-ses-beta-pending"]
    );

    // Projects 分组：alpha 3 个任务、beta 4 个；gamma 无会话（空项目态），
    // 无 Unassigned。
    let projects = projection.project_groups(None);
    assert_eq!(
        projects
            .iter()
            .map(|project| (project.workspace_id.as_deref(), project.task_count()))
            .collect::<Vec<_>>(),
        vec![(Some("fx-alpha-app"), 3), (Some("fx-beta-lib"), 4)]
    );
    assert!(!projects.iter().any(|project| project.is_unassigned()));

    // 状态：provider 快照恢复；pending 审批卡随会话选择出现/消失；
    // 纯 seed 数据无 live run。
    assert_eq!(
        projection
            .selected_model
            .as_ref()
            .map(|(provider, model)| (provider.as_str(), model.as_str())),
        Some(("mock", "fixture-model"))
    );
    assert!(projection.active_runs.is_empty());
    assert_eq!(projection.active_run_id, None);
    assert_eq!(projection.pending_approval, None);
    projection.select_session("fx-ses-beta-pending");
    let pending = projection
        .pending_approval
        .as_ref()
        .expect("pending approval restored from snapshot");
    assert_eq!(pending.tool_call_id, "call-fx-ses-beta-pending-0-0");
    assert_eq!(pending.tool_name, "write_file");
    assert!(pending.reason.contains("src/lib.ts"));
    projection.select_session("fx-ses-alpha-today");
    assert_eq!(projection.pending_approval, None);
}

#[test]
fn timeline_rows_group_adjacent_tools_and_absorb_into_summary() {
    let mut projection = DesktopProjection::default();
    projection.timeline.entries = vec![
        raw_entry(
            1,
            TimelineEntryKind::UserMessage { text: "go".into() },
            Some("r-1"),
        ),
        tool_entry(2, "r-1", "read_file", "succeeded"),
        tool_entry(3, "r-1", "edit_file", "succeeded"),
        // 同 run 终态紧邻 → 吸收该组为摘要区域。
        terminal_entry(4, ForkBoundary::Completed),
        // 不同 run 的 tool 不被跨 run 终态吞并（审查 P2 防护）。
        tool_entry(5, "r-2", "bash", "succeeded"),
        terminal_entry(6, ForkBoundary::Completed),
    ];
    let rows = projection.timeline_rows();
    assert_eq!(
        rows,
        vec![
            TimelineRow::Message { entry_index: 0 },
            TimelineRow::RunSummary {
                group: Some(vec![1, 2]),
                terminal: 3,
            },
            TimelineRow::ToolGroup {
                entry_indices: vec![4]
            },
            TimelineRow::RunSummary {
                group: None,
                terminal: 5,
            },
        ]
    );

    // 不同 run 的相邻 tool 不并组。
    let mut projection = DesktopProjection::default();
    projection.timeline.entries = vec![
        tool_entry(1, "r-1", "read_file", "succeeded"),
        tool_entry(2, "r-2", "bash", "running"),
        tool_entry(3, "r-2", "edit_file", "succeeded"),
    ];
    let rows = projection.timeline_rows();
    assert_eq!(
        rows,
        vec![
            TimelineRow::ToolGroup {
                entry_indices: vec![0]
            },
            TimelineRow::ToolGroup {
                entry_indices: vec![1, 2],
            },
        ]
    );
}

#[test]
fn timeline_rows_terminal_without_group_and_phases_stay_single() {
    let mut projection = DesktopProjection::default();
    projection.timeline.entries = vec![
        raw_entry(
            1,
            TimelineEntryKind::AssistantMessage { text: "hi".into() },
            Some("r-1"),
        ),
        raw_entry(
            2,
            TimelineEntryKind::RunState("run streaming_response".into()),
            Some("r-1"),
        ),
        raw_entry(
            3,
            TimelineEntryKind::RunState("approval approved".into()),
            Some("r-1"),
        ),
        terminal_entry(4, ForkBoundary::Failed),
    ];
    let rows = projection.timeline_rows();
    assert_eq!(
        rows,
        vec![
            TimelineRow::Message { entry_index: 0 },
            TimelineRow::RunPhase { entry_index: 1 },
            TimelineRow::RunPhase { entry_index: 2 },
            TimelineRow::RunSummary {
                group: None,
                terminal: 3,
            },
        ]
    );
}

#[test]
fn run_summary_and_footer_texts_map_terminal_boundaries_only() {
    let completed = terminal_entry(1, ForkBoundary::Completed);
    assert_eq!(
        run_summary_texts(&completed, true),
        Some((
            "Ready for review",
            "The run finished. Review the changes from this turn.".to_string()
        ))
    );
    assert_eq!(
        run_summary_texts(&completed, false),
        Some(("Run completed", "The run finished.".to_string()))
    );
    assert_eq!(run_footer_label(&completed), Some("Run completed"));
    assert_eq!(
        run_footer_label(&terminal_entry(2, ForkBoundary::Cancelled)),
        Some("Run cancelled")
    );
    assert_eq!(
        run_footer_label(&terminal_entry(3, ForkBoundary::Failed)),
        Some("Run failed")
    );
    // 非终态（含 Interrupted：无 fork 边界）不产生摘要 / 页脚。
    let phase = raw_entry(
        4,
        TimelineEntryKind::RunState("run interrupted".into()),
        Some("r-1"),
    );
    assert_eq!(run_summary_texts(&phase, false), None);
    assert_eq!(run_footer_label(&phase), None);
}

#[test]
fn failed_run_summary_description_reports_real_reason() {
    let failed_entry = |sequence: u64, label: &str| {
        let mut entry = raw_entry(
            sequence,
            TimelineEntryKind::RunState(label.into()),
            Some("r-1"),
        );
        entry.fork_boundary = Some(ForkBoundary::Failed);
        entry
    };
    // 有原因：摘要卡显示原因原文；原因内部再含分隔符只剥一次前缀。
    assert_eq!(
        run_summary_texts(&failed_entry(1, "run failed · provider timeout"), false),
        Some(("Run failed", "provider timeout".to_string()))
    );
    assert_eq!(
        run_summary_texts(&failed_entry(2, "run failed · a · b"), false),
        Some(("Run failed", "a · b".to_string()))
    );
    // 无原因（live 臂标签）：兜底通用失败文案，不指向不存在的错误详情。
    assert_eq!(
        run_summary_texts(&failed_entry(3, "run failed"), false),
        Some(("Run failed", "The run failed.".to_string()))
    );
    // 剥离失败（非 reducer 格式标签）/ 剥离后为空：同样兜底。
    assert_eq!(
        run_summary_texts(&failed_entry(4, "run terminal"), false),
        Some(("Run failed", "The run failed.".to_string()))
    );
    assert_eq!(
        run_summary_texts(&failed_entry(5, "run failed · "), false),
        Some(("Run failed", "The run failed.".to_string()))
    );
}

#[test]
fn workspace_header_predicates_follow_active_session_and_live_status() {
    let snapshot = snapshot_with_sessions(vec![session_entry("s-1", "Ship it", 10)]);
    let mut projection = DesktopProjection::from_snapshot(&snapshot);
    assert_eq!(projection.workspace_header_title(), None);
    assert_eq!(projection.workspace_header_status(), None);

    projection.select_session("s-1");
    assert_eq!(projection.workspace_header_title(), Some("Ship it"));
    // 空闲会话：无 live 终态可显示（诚实口径，不画 Completed）。
    assert_eq!(projection.workspace_header_status(), None);

    projection.active_runs.push(ActiveRun {
        run_id: "r-1".into(),
        session_id: "s-1".into(),
        started_at_ms: 1,
    });
    assert_eq!(
        projection.workspace_header_status(),
        Some(SessionLiveStatus::Running)
    );
}
