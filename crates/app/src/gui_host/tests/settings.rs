use super::*;
use pawork_testkit::MockProvider;

// ---- SET-2 Host Settings 门面（ADR-046）----

pub(super) async fn settings_adapter(
    base_url: String,
    backend: Arc<pawork_auth::MemoryBackend>,
) -> (GuiHostAdapter, tempfile::TempDir) {
    settings_adapter_for_channel("glm-coding", "glm-5.2", base_url, backend).await
}

pub(super) async fn settings_adapter_for_channel(
    provider_id: &str,
    model_id: &str,
    base_url: String,
    backend: Arc<pawork_auth::MemoryBackend>,
) -> (GuiHostAdapter, tempfile::TempDir) {
    settings_adapter_with_default(provider_id, model_id, base_url, backend, None).await
}

/// 可选在生效配置中注入 default_provider/default_model（SET-5 持久化
/// 默认项）；None 表示未配置默认项。
pub(super) async fn settings_adapter_with_default(
    provider_id: &str,
    model_id: &str,
    base_url: String,
    backend: Arc<pawork_auth::MemoryBackend>,
    default: Option<(&str, &str)>,
) -> (GuiHostAdapter, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let mut config = pawork_workspace::config::PaworkConfig::default();
    config
        .providers
        .push(pawork_workspace::config::ProviderConfig {
            id: provider_id.into(),
            base_url: Some(base_url),
            default: None,
            use_proxy: None,
            disabled_models: Vec::new(),
        });
    if let Some((default_provider, default_model)) = default {
        config.default_provider = Some(default_provider.into());
        config.default_model = Some(default_model.into());
    }
    let core = AppCore::from_parts(
        Arc::new(
            MockProvider::sequence(Vec::new()).with_models(
                pawork_providers::ModelRegistry::builtin()
                    .list()
                    .into_iter()
                    .filter(|entry| entry.provider.as_str() == provider_id)
                    .map(|entry| entry.to_definition())
                    .collect(),
            ),
        ),
        None,
        pawork_domain::ModelId::from(model_id),
        pawork_domain::ProviderId::from(provider_id),
        Some(store),
    )
    .with_state(config, backend as Arc<dyn pawork_auth::SecretBackend>);
    (GuiHostAdapter::new(Arc::new(core)), dir)
}

/// 将 HOME 重定向到临时目录，并在 Drop 时恢复原值（含 panic 路径）。
/// 本文件仅此一处改进程环境；directories 在 Unix 上优先读 HOME，
/// 必须先恢复再删临时目录，避免其它测试读到已释放路径。
/// HOME 重定向测试互斥：libtest 并行会交叉改进程环境。
pub(super) static HOME_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) struct RestoreHome(Option<std::ffi::OsString>);

impl Drop for RestoreHome {
    fn drop(&mut self) {
        #[allow(unused_unsafe)]
        unsafe {
            match self.0.take() {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

/// 设置进程 env 变量并在 Drop 时恢复原值（含 panic 路径）。
pub(super) struct RestoreEnvVar(String, Option<std::ffi::OsString>);

impl Drop for RestoreEnvVar {
    fn drop(&mut self) {
        match self.1.take() {
            Some(value) => {
                crate::testsupport::set_env(&self.0, value.to_str().expect("env value is utf-8"))
            }
            None => crate::testsupport::remove_env(&self.0),
        }
    }
}

#[tokio::test]
async fn provider_auth_status_reports_persisted_default_pair() {
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    // 默认项取自生效配置而非 core 当前选中的 provider/model，
    // 因此注入与 core 选中值不同的默认对。
    let (adapter, _dir) = settings_adapter_with_default(
        "glm-coding",
        "glm-5.2",
        "http://127.0.0.1:1".into(),
        backend,
        Some(("deepseek", "deepseek-chat")),
    )
    .await;
    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(pawork_domain::ProviderId::from("glm-coding")),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    assert_eq!(
        status["default"],
        serde_json::json!({
            "provider_id": "deepseek",
            "model_id": "deepseek-chat",
        })
    );
}

#[tokio::test]
async fn provider_auth_status_reports_null_default_when_unconfigured() {
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(pawork_domain::ProviderId::from("glm-coding")),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    assert!(
        status["default"].is_null(),
        "default must be null: {status}"
    );
}

#[tokio::test]
async fn set_default_model_updates_status_default_within_same_session() {
    // 写盘目标经 HOME 重定向到临时目录，避免污染真实全局配置。
    // RestoreHome 必须在 tempfile 之后声明：Drop 先恢复 HOME，再删临时目录。
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let provider = pawork_domain::ProviderId::from("glm-coding");

    let before = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(provider.clone()),
        }))
        .await
        .expect("status before set");
    let AppResponse::Data(before) = before else {
        panic!("ProviderAuthStatus must return Data: {before:?}")
    };
    assert!(before["default"].is_null(), "fresh config has no default");

    let response = adapter
        .command(&command_envelope(AppCommand::SetDefaultModel {
            provider_id: provider.clone(),
            model_id: "glm-5.2".into(),
        }))
        .await
        .expect("set default model");
    let AppResponse::Data(data) = response else {
        panic!("SetDefaultModel must return Data: {response:?}")
    };
    assert_eq!(data["provider_id"], "glm-coding");
    assert_eq!(data["model_id"], "glm-5.2");

    // 同会话重查：内存生效配置已同步为新 pair，无需 Host 重启。
    let after = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(provider),
        }))
        .await
        .expect("status after set");
    let AppResponse::Data(after) = after else {
        panic!("ProviderAuthStatus must return Data: {after:?}")
    };
    assert_eq!(
        after["default"],
        serde_json::json!({ "provider_id": "glm-coding", "model_id": "glm-5.2" })
    );

    // 写盘确实落在重定向后的全局配置文件。
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        persisted.contains("default_provider = \"glm-coding\""),
        "persisted config misses default pair: {persisted}"
    );
}

#[tokio::test]
async fn set_proxy_url_updates_general_settings_within_same_session() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let proxy = "http://127.0.0.1:7890";

    let before = adapter
        .query(&query_envelope(AppQuery::GeneralSettings))
        .await
        .expect("general settings before set");
    let AppResponse::Data(before) = before else {
        panic!("GeneralSettings must return Data: {before:?}")
    };
    assert!(
        before["proxy_url"].is_null(),
        "fresh config has no proxy_url"
    );

    let response = adapter
        .command(&command_envelope(AppCommand::SetProxyUrl {
            proxy_url: Some(proxy.into()),
        }))
        .await
        .expect("set proxy url");
    let AppResponse::Data(data) = response else {
        panic!("SetProxyUrl must return Data: {response:?}")
    };
    assert_eq!(data["proxy_url"], proxy);

    let after = adapter
        .query(&query_envelope(AppQuery::GeneralSettings))
        .await
        .expect("general settings after set");
    let AppResponse::Data(after) = after else {
        panic!("GeneralSettings must return Data: {after:?}")
    };
    assert_eq!(after["proxy_url"], proxy);

    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        persisted.contains("proxy_url = \"http://127.0.0.1:7890\""),
        "persisted config misses proxy_url: {persisted}"
    );
}

#[tokio::test]
async fn clear_proxy_url_updates_general_settings_to_null() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    adapter
        .command(&command_envelope(AppCommand::SetProxyUrl {
            proxy_url: Some("http://127.0.0.1:7890".into()),
        }))
        .await
        .expect("seed proxy url");

    let response = adapter
        .command(&command_envelope(AppCommand::SetProxyUrl {
            proxy_url: None,
        }))
        .await
        .expect("clear proxy url");
    let AppResponse::Data(data) = response else {
        panic!("SetProxyUrl clear must return Data: {response:?}")
    };
    assert!(
        data["proxy_url"].is_null(),
        "clear receipt must be null: {data}"
    );

    let after = adapter
        .query(&query_envelope(AppQuery::GeneralSettings))
        .await
        .expect("general settings after clear");
    let AppResponse::Data(after) = after else {
        panic!("GeneralSettings must return Data: {after:?}")
    };
    assert!(
        after["proxy_url"].is_null(),
        "requery after clear must be null: {after}"
    );

    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        !persisted.contains("proxy_url"),
        "cleared config still has proxy_url: {persisted}"
    );
}

/// ADR-052 SET-6h：开关写盘 + 同会话内存生效 + 回执即写后状态。
#[tokio::test]
async fn set_provider_use_proxy_persists_and_syncs_effective_config() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    let response = adapter
        .command(&command_envelope(AppCommand::SetProviderUseProxy {
            provider_id: pawork_domain::ProviderId::from("glm-coding"),
            use_proxy: false,
        }))
        .await
        .expect("set provider use_proxy");
    let AppResponse::Data(data) = response else {
        panic!("SetProviderUseProxy must return Data: {response:?}")
    };
    assert_eq!(data["provider_id"], "glm-coding");
    assert_eq!(data["use_proxy"], false);

    // 同会话重查：providers[] 生效值已同步为 false（生效值 = 未显式 false）。
    let after = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: None,
        }))
        .await
        .expect("status after set");
    let AppResponse::Data(after) = after else {
        panic!("ProviderAuthStatus must return Data: {after:?}")
    };
    let entry = after["providers"]
        .as_array()
        .expect("providers array")
        .iter()
        .find(|entry| entry["provider_id"] == "glm-coding")
        .expect("glm-coding entry");
    assert_eq!(entry["use_proxy"], false);

    // 写盘落在重定向后的全局配置 `[[providers]]` 条目。
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        persisted.contains("use_proxy = false"),
        "persisted config misses use_proxy: {persisted}"
    );
}

/// ADR-052 SET-6h 失败路径：未知 provider fail-closed，不落盘。
#[tokio::test]
async fn set_provider_use_proxy_rejects_unknown_provider() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    let error = adapter
        .command(&command_envelope(AppCommand::SetProviderUseProxy {
            provider_id: pawork_domain::ProviderId::from("no-such-provider"),
            use_proxy: false,
        }))
        .await
        .expect_err("unknown provider must fail");
    assert_eq!(error.code, "unknown_provider", "error: {error:?}");

    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).unwrap_or_default();
    assert!(
        !persisted.contains("no-such-provider"),
        "unknown provider must not persist: {persisted}"
    );
}

#[tokio::test]
async fn set_proxy_url_rejects_invalid_url_and_keeps_old_value() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let old = "http://127.0.0.1:7890";
    adapter
        .command(&command_envelope(AppCommand::SetProxyUrl {
            proxy_url: Some(old.into()),
        }))
        .await
        .expect("seed proxy url");
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let seeded = std::fs::read_to_string(&config_path).expect("seed persisted");
    assert!(
        seeded.contains("proxy_url"),
        "seed did not persist: {seeded}"
    );

    let bad = "http://user:s3cret-proxy@not a url";
    let error = adapter
        .command(&command_envelope(AppCommand::SetProxyUrl {
            proxy_url: Some(bad.into()),
        }))
        .await
        .expect_err("invalid proxy must fail closed");
    assert_eq!(error.code, "invalid_proxy_url");
    assert!(
        !error.message.contains(bad) && !error.message.contains("s3cret-proxy"),
        "error leaks proxy URL: {}",
        error.message
    );

    let after = adapter
        .query(&query_envelope(AppQuery::GeneralSettings))
        .await
        .expect("general settings after invalid set");
    let AppResponse::Data(after) = after else {
        panic!("GeneralSettings must return Data: {after:?}")
    };
    assert_eq!(
        after["proxy_url"], old,
        "invalid set must keep old proxy_url"
    );

    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert_eq!(persisted, seeded, "invalid set must not rewrite disk");
}

#[tokio::test]
async fn set_terminal_settings_updates_and_clears_shell_within_same_session() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    let before = adapter
        .query(&query_envelope(AppQuery::TerminalSettings))
        .await
        .expect("terminal settings before set");
    let AppResponse::Data(before) = before else {
        panic!("TerminalSettings must return Data: {before:?}")
    };
    assert_eq!(
        before,
        serde_json::json!({ "shell": None::<String>, "columns": 80, "rows": 24 }),
        "fresh config must report platform defaults"
    );

    #[cfg(unix)]
    let shell = "/bin/sh";
    #[cfg(windows)]
    let shell = "cmd.exe";
    let response = adapter
        .command(&command_envelope(AppCommand::SetTerminalSettings {
            shell: Some(shell.into()),
            columns: 120,
            rows: 40,
        }))
        .await
        .expect("set terminal settings");
    let AppResponse::Data(data) = response else {
        panic!("SetTerminalSettings must return Data: {response:?}")
    };
    assert_eq!(data["shell"], shell);
    assert_eq!(data["columns"], 120);
    assert_eq!(data["rows"], 40);

    let after = adapter
        .query(&query_envelope(AppQuery::TerminalSettings))
        .await
        .expect("terminal settings after set");
    let AppResponse::Data(after) = after else {
        panic!("TerminalSettings must return Data: {after:?}")
    };
    assert_eq!(after["shell"], shell);
    assert_eq!(after["columns"], 120);
    assert_eq!(after["rows"], 40);

    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        persisted.contains("[terminal]"),
        "missing [terminal]: {persisted}"
    );
    assert!(
        persisted.contains(format!("shell = \"{shell}\"").as_str()),
        "persisted config misses shell: {persisted}"
    );

    // ADR-050 D3：shell=null 显式清除回平台默认，columns/rows 保持全态值。
    let response = adapter
        .command(&command_envelope(AppCommand::SetTerminalSettings {
            shell: None,
            columns: 120,
            rows: 40,
        }))
        .await
        .expect("clear terminal shell");
    let AppResponse::Data(data) = response else {
        panic!("SetTerminalSettings clear must return Data: {response:?}")
    };
    assert!(
        data["shell"].is_null(),
        "clear receipt must be null: {data}"
    );

    let after = adapter
        .query(&query_envelope(AppQuery::TerminalSettings))
        .await
        .expect("terminal settings after clear");
    let AppResponse::Data(after) = after else {
        panic!("TerminalSettings must return Data: {after:?}")
    };
    assert!(after["shell"].is_null(), "requery after clear: {after}");
    assert_eq!(after["columns"], 120);
    assert_eq!(after["rows"], 40);

    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        !persisted.contains("shell"),
        "cleared config still has shell key: {persisted}"
    );
    assert!(persisted.contains("columns = 120"), "{persisted}");
}

#[tokio::test]
async fn set_terminal_settings_rejects_invalid_values_and_keeps_old() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    #[cfg(unix)]
    let seeded_shell = "/bin/sh";
    #[cfg(windows)]
    let seeded_shell = "cmd.exe";
    adapter
        .command(&command_envelope(AppCommand::SetTerminalSettings {
            shell: Some(seeded_shell.into()),
            columns: 120,
            rows: 40,
        }))
        .await
        .expect("seed terminal settings");
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let seeded = std::fs::read_to_string(&config_path).expect("seed persisted");

    for bad in [
        AppCommand::SetTerminalSettings {
            shell: Some("/definitely/missing/pawork-shell".into()),
            columns: 120,
            rows: 40,
        },
        AppCommand::SetTerminalSettings {
            shell: Some(seeded_shell.into()),
            columns: 1,
            rows: 40,
        },
        AppCommand::SetTerminalSettings {
            shell: Some(seeded_shell.into()),
            columns: 120,
            rows: 2000,
        },
    ] {
        let error = adapter
            .command(&command_envelope(bad))
            .await
            .expect_err("invalid terminal settings must fail closed");
        assert_eq!(error.code, "invalid_terminal_settings");
    }

    let after = adapter
        .query(&query_envelope(AppQuery::TerminalSettings))
        .await
        .expect("terminal settings after invalid set");
    let AppResponse::Data(after) = after else {
        panic!("TerminalSettings must return Data: {after:?}")
    };
    assert_eq!(after["shell"], seeded_shell);
    assert_eq!(after["columns"], 120);
    assert_eq!(after["rows"], 40);

    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert_eq!(persisted, seeded, "invalid set must not rewrite disk");
}

#[tokio::test]
async fn terminal_create_applies_configured_shell_and_size() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    #[cfg(unix)]
    let configured_shell = "/usr/bin/true";
    #[cfg(windows)]
    let configured_shell = "cmd.exe";
    let mut config = pawork_workspace::config::PaworkConfig::default();
    config.terminal = Some(pawork_workspace::config::TerminalConfig {
        shell: Some(configured_shell.into()),
        columns: Some(97),
        rows: Some(31),
    });
    let mut core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(Vec::new())),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    )
    .with_state(
        config,
        Arc::new(pawork_auth::MemoryBackend::new()) as Arc<dyn pawork_auth::SecretBackend>,
    );
    core.attach_workspace(dir.path()).expect("attach workspace");
    core.configure_approval(
        crate::ApprovalMode::AskForDangerous,
        true,
        Arc::new(crate::DenyAllApprovals),
    );
    let adapter = GuiHostAdapter::new(Arc::new(core));

    let created = adapter
        .command(&command_envelope(AppCommand::TerminalCreate {
            workspace_id: WorkspaceId::from("ws-default"),
            working_directory: None,
        }))
        .await
        .expect("terminal_create");
    let AppResponse::Data(payload) = created else {
        panic!("terminal_create must return Data: {created:?}")
    };
    let terminal_id = payload["terminal_session_id"]
        .as_str()
        .expect("terminal_session_id")
        .to_string();

    // ADR-050 D4：size 生效值来自配置（pixel 0 由 PtyWindowSize::default 保持）。
    let owner = pawork_exec::OwnerSessionId::new("ws-default");
    let snapshot = adapter
        .pty
        .snapshot(&pawork_exec::TerminalId::new(&terminal_id), &owner)
        .expect("snapshot");
    assert_eq!(snapshot.size.cols, 97);
    assert_eq!(snapshot.size.rows, 31);
    assert_eq!(snapshot.size.pixel_width, 0);
    assert_eq!(snapshot.size.pixel_height, 0);

    // 配置 shell 真被用于 spawn：/usr/bin/true 立即以 exit_code=0 退出
    //（默认交互 shell 不会立即退出）。
    #[cfg(unix)]
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let snapshot = adapter
                .pty
                .snapshot(&pawork_exec::TerminalId::new(&terminal_id), &owner)
                .expect("snapshot");
            if snapshot.state == pawork_exec::PtySessionState::Exited {
                assert_eq!(snapshot.exit_code, Some(0));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "configured shell /usr/bin/true must exit promptly"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

#[tokio::test]
async fn set_approval_mode_updates_permissions_settings_within_same_session() {
    let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home_dir = tempfile::tempdir().unwrap();
    let _restore = RestoreHome(std::env::var_os("HOME"));
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("HOME", home_dir.path());
    }
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    let before = adapter
        .query(&query_envelope(AppQuery::PermissionsSettings))
        .await
        .expect("permissions settings before set");
    let AppResponse::Data(before) = before else {
        panic!("PermissionsSettings must return Data: {before:?}")
    };
    assert_eq!(before["approval_mode"], "read_only");
    assert_eq!(before["workspace_trusted"], false);
    assert!(before["trust_workspaces_global"].is_null());
    // ADR-048 D1（实现期修订）：透出 Host 权威 attached workspace_id。
    let attached = adapter.core.read().await.workspace_id().to_string();
    assert_eq!(before["workspace_id"], attached.as_str());

    let response = adapter
        .command(&command_envelope(AppCommand::SetApprovalMode {
            mode: "ask_for_writes".into(),
        }))
        .await
        .expect("set approval mode");
    let AppResponse::Data(data) = response else {
        panic!("SetApprovalMode must return Data: {response:?}")
    };
    assert_eq!(data["approval_mode"], "ask_for_writes");

    let config_path = pawork_workspace::config::global_config_path().unwrap();
    let restored = pawork_workspace::config::Loader::discover_from(Some(&config_path), None)
        .resolve()
        .unwrap();
    assert_eq!(
        restored.config.approval_mode,
        Some(crate::ApprovalMode::AskForWrites)
    );
    let mut options = crate::AppLoadOptions::default();
    options.workspace_root = Some(_dir.path().to_path_buf());
    options.data_dir = Some(_dir.path().join("restart-data"));
    options.auth_backend = Some(Arc::new(pawork_auth::MemoryBackend::new()));
    let restarted = AppCore::load_for_catalog(options.clone()).await.unwrap();
    assert_eq!(restarted.approval_mode(), crate::ApprovalMode::AskForWrites);
    drop(restarted);
    options.approval_mode = Some(crate::ApprovalMode::ReadOnly);
    let overridden = AppCore::load_for_catalog(options).await.unwrap();
    assert_eq!(overridden.approval_mode(), crate::ApprovalMode::ReadOnly);
    drop(overridden);
    let disk_before = std::fs::read_to_string(&config_path).unwrap();

    // 未知值 fail-closed：Error 且旧值保留（ADR-048 D2）。
    let error = adapter
        .command(&command_envelope(AppCommand::SetApprovalMode {
            mode: "yolo".into(),
        }))
        .await
        .expect_err("unknown approval mode must fail closed");
    assert_eq!(error.code, "invalid_approval_mode");
    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), disk_before);
    // 损坏配置不可被覆盖，内存保持已确认值。
    std::fs::write(&config_path, "[broken").unwrap();
    let error = adapter
        .command(&command_envelope(AppCommand::SetApprovalMode {
            mode: "never_ask".into(),
        }))
        .await
        .expect_err("write failure");
    assert_eq!(error.code, "config_write");
    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), "[broken");

    let after = adapter
        .query(&query_envelope(AppQuery::PermissionsSettings))
        .await
        .expect("permissions settings after set");
    let AppResponse::Data(after) = after else {
        panic!("PermissionsSettings must return Data: {after:?}")
    };
    assert_eq!(after["approval_mode"], "ask_for_writes");
    assert_eq!(after["workspace_trusted"], false);
    // ToolScheduler 必须同步 Arc-swap，否则之后启动的 run 仍走旧 ReadOnly 闸门。
    assert_eq!(
        adapter.core.read().await.scheduler_approval_snapshot(),
        (crate::ApprovalMode::AskForWrites, false)
    );
}

#[tokio::test]
async fn workspace_trust_toggles_session_trust_for_attached_workspace() {
    let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home_dir = tempfile::tempdir().unwrap();
    let _restore = RestoreHome(std::env::var_os("HOME"));
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("HOME", home_dir.path());
    }
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    adapter
        .core
        .write()
        .await
        .attach_workspace(_dir.path())
        .unwrap();
    let workspace_id = adapter.core.read().await.workspace_id().clone();

    let response = adapter
        .command(&command_envelope(AppCommand::WorkspaceTrust {
            workspace_id: workspace_id.clone(),
            trusted: true,
        }))
        .await
        .expect("workspace trust");
    let AppResponse::Data(data) = response else {
        panic!("WorkspaceTrust must return Data: {response:?}")
    };
    assert_eq!(data["workspace_trusted"], true);

    let after = adapter
        .query(&query_envelope(AppQuery::PermissionsSettings))
        .await
        .expect("permissions settings after trust");
    let AppResponse::Data(after) = after else {
        panic!("PermissionsSettings must return Data: {after:?}")
    };
    assert_eq!(after["workspace_trusted"], true);
    let config_path = pawork_workspace::config::global_config_path().unwrap();
    assert!(config_path.is_file());
    let other_dir = tempfile::tempdir().unwrap();
    let mut options = crate::AppLoadOptions::default();
    options.workspace_root = Some(_dir.path().to_path_buf());
    options.data_dir = Some(other_dir.path().join("restart-data"));
    options.auth_backend = Some(Arc::new(pawork_auth::MemoryBackend::new()));
    let mut restarted = AppCore::load_for_catalog(options.clone()).await.unwrap();
    assert!(restarted.workspace_trusted());
    restarted.attach_workspace(other_dir.path()).unwrap();
    assert!(
        !restarted.workspace_trusted(),
        "trust must not escape to another project"
    );
    let original = adapter
        .core
        .read()
        .await
        .workspace_by_id(&workspace_id)
        .unwrap();
    assert!(restarted.workspace_trusted_for_roots(&original.roots));
    drop(restarted);
    options.trust_workspaces = Some(false);
    let overridden = AppCore::load_for_catalog(options).await.unwrap();
    assert!(!overridden.workspace_trusted());
    drop(overridden);
    // 之后启动的 run 克隆新 scheduler Arc（check_gate 用 config.workspace_trusted）。
    assert!(adapter.core.read().await.workspace_trusted());
    assert_eq!(
        adapter.core.read().await.scheduler_approval_snapshot(),
        (crate::ApprovalMode::ReadOnly, true)
    );
}

#[tokio::test]
async fn workspace_trust_rejects_mismatched_workspace_id_fail_closed() {
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    let error = adapter
        .command(&command_envelope(AppCommand::WorkspaceTrust {
            workspace_id: WorkspaceId::from("ws-other"),
            trusted: true,
        }))
        .await
        .expect_err("mismatched workspace must fail closed");
    assert_eq!(error.code, "unknown_workspace");

    let after = adapter
        .query(&query_envelope(AppQuery::PermissionsSettings))
        .await
        .expect("permissions settings after mismatch");
    let AppResponse::Data(after) = after else {
        panic!("PermissionsSettings must return Data: {after:?}")
    };
    assert_eq!(
        after["workspace_trusted"], false,
        "trust must stay old value"
    );
    assert_eq!(
        adapter.core.read().await.scheduler_approval_snapshot(),
        (crate::ApprovalMode::ReadOnly, false),
        "fail-closed must not rebuild scheduler trust"
    );
}

#[tokio::test]
async fn auth_set_api_key_verifies_replaces_and_masks_end_to_end() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let secret = "sk-live-plaintext-1234567890abcd";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", &format!("Bearer {secret}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "data": [{ "id": "glm-5.2" }] })),
        )
        // hyper 对幂等 GET 在连接被对端关闭时会自动重发一次，
        // 计数只要求「至少一次携带候选 key 的已认证请求」。
        .expect(1..)
        .mount(&server)
        .await;

    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, dir) = settings_adapter(server.uri(), backend).await;
    {
        let mut core = adapter.core.write().await;
        core.config.proxy_url = Some("http://[invalid-proxy".into());
        core.set_provider_use_proxy("glm-coding", false);
    }
    let mut events = adapter.subscribe_events();

    let response = adapter
        .command(&command_envelope(AppCommand::AuthSetApiKey {
            provider_id: pawork_domain::ProviderId::from("glm-coding"),
            api_key: pawork_protocol::ApiKeySecret::new(secret),
        }))
        .await
        .expect("verify-then-replace succeeds");
    let AppResponse::Data(data) = response else {
        panic!("AuthSetApiKey must return Data: {response:?}")
    };
    assert_eq!(data["provider_id"], "glm-coding");
    assert_eq!(data["method"], "api_key");
    assert!(data["verified_at"].as_str().is_some());
    let response_wire = serde_json::to_string(&data).expect("serialize response");
    assert!(!response_wire.contains(secret), "response leaks plaintext");

    let event = events.try_recv().expect("AuthChanged::Succeeded event");
    let event_wire = serde_json::to_string(&event).expect("serialize event");
    assert!(
        event_wire.contains("\"succeeded\""),
        "missing succeeded state"
    );
    assert!(!event_wire.contains(secret), "event leaks plaintext");

    server.verify().await;

    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(pawork_domain::ProviderId::from("glm-coding")),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    let entry = &status["providers"][0];
    assert_eq!(entry["provider_id"], "glm-coding");
    assert_eq!(entry["display_name"], "GLM Coding");
    assert_eq!(entry["auth_methods"], serde_json::json!(["api_key"]));
    assert_eq!(entry["auth"]["type"], "connected");
    assert_eq!(entry["auth"]["method"], "api_key");
    let masked = entry["auth"]["masked_credential"].as_str().expect("masked");
    assert!(!masked.contains(secret), "status leaks plaintext: {masked}");

    // UI-6b: additive keys and account commands do not wait for a running
    // Run's core read guard. The cached provider remains a snapshot until next Run.
    let second_secret = "sk-second-account-1234567890wxyz";
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", &format!("Bearer {second_secret}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"data":[{"id":"glm-5.2"}]})),
        )
        .mount(&server)
        .await;
    let guard = adapter.core.read().await;
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        adapter.command(&command_envelope(AppCommand::AuthAccountAddApiKey {
            provider_id: "glm-coding".into(),
            display_name: "Work".into(),
            api_key: pawork_protocol::ApiKeySecret::new(second_secret),
        })),
    )
    .await
    .expect("add must not wait for core write lock")
    .unwrap();
    let inventory =
        pawork_auth::list_provider_accounts(guard.auth_backend().as_ref(), &"glm-coding".into())
            .unwrap();
    assert_eq!(inventory.accounts.len(), 2);
    let account_id = inventory.accounts[1].credential_id.clone();
    let selection = command_envelope(AppCommand::AuthAccountSelect {
        provider_id: "glm-coding".into(),
        credential_id: account_id.clone(),
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        adapter.command(&selection),
    )
    .await
    .expect("select must not wait for running Run")
    .unwrap();
    assert!(guard.provider_needs_rebuild());
    assert!(adapter
        .command(&command_envelope(AppCommand::AuthAccountRemove {
            provider_id: "glm-coding".into(),
            credential_id: account_id
        }))
        .await
        .is_err());
    drop(guard);
    let mut removed_events = adapter.subscribe_events();
    adapter
        .command(&command_envelope(AppCommand::AuthAccountRemove {
            provider_id: "glm-coding".into(),
            credential_id: pawork_auth::LEGACY_API_KEY_ID.into(),
        }))
        .await
        .expect("remove only the unselected account");
    let event_wire = serde_json::to_string(&removed_events.try_recv().unwrap()).unwrap();
    assert!(
        event_wire.contains("\"succeeded\""),
        "remaining account stays connected"
    );
    assert!(!event_wire.contains(second_secret));
    let mut old_query = query_envelope(AppQuery::ProviderAuthStatus {
        provider_id: Some("glm-coding".into()),
    });
    old_query.api_version = pawork_protocol::ApiVersion::new(1, 14);
    let AppResponse::Data(old_status) = adapter.query(&old_query).await.unwrap() else {
        panic!("status");
    };
    assert!(old_status["providers"][0]["credentials"][0]
        .get("credential_id")
        .is_none());

    // 同 provider/model 连接成功后下一轮必须重装配，不能继续使用旧 Mock adapter。
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", &format!("Bearer {second_secret}")))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(concat!(
                    "data: {\"id\":\"account-run\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"selected account\"},\"finish_reason\":null}]}\n\n",
                    "data: {\"id\":\"account-run\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: [DONE]\n\n"
                )),
        )
        .expect(1)
        .mount(&server)
        .await;
    let session = adapter
        .core
        .read()
        .await
        .create_session_unbound("auth refresh")
        .await
        .unwrap();
    let run = command_envelope(AppCommand::RunStart {
        session_id: session,
        user_message: "hi".into(),
        model: Some("glm-5.2".into()),
        provider: Some("glm-coding".into()),
        profile: None,
    });
    assert!(adapter.core.read().await.provider_needs_rebuild());
    let mut run_events = adapter.subscribe_events();
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = adapter.command(&run).await.unwrap()
    else {
        panic!("run must be accepted");
    };
    wait_run_completed(&mut run_events, &run_id).await;
    server.verify().await;
    {
        let core = adapter.core.read().await;
        assert!(!core.provider_needs_rebuild());
        assert_eq!(
            core.credential.as_ref().unwrap().expose_secret(),
            second_secret
        );
    }
    adapter
        .command(&command_envelope(AppCommand::AuthRemove {
            provider_id: "glm-coding".into(),
        }))
        .await
        .expect("remove stored key");
    assert!(adapter.core.read().await.provider_needs_rebuild());
    let retry = command_envelope(run.command);
    // 文件凭证移除后允许恢复 env fallback，但不能复用被移除的 adapter/key。
    if let Ok(response) = adapter.command(&retry).await {
        assert!(matches!(response, AppResponse::Accepted { .. }));
        let core = adapter.core.read().await;
        assert!(!core.provider_needs_rebuild());
        assert!(core.credential.as_ref().unwrap().expose_secret() != second_secret);
    }

    // ADR-046 D6 Secret 负断言：命令完成后，临时目录内任何持久化文件
    //（command ledger / session.db 及其 -wal/-shm）都不得含明文——
    // ledger 只缓存脱敏响应信封，请求 payload 不落盘。
    for entry in std::fs::read_dir(dir.path()).expect("read tempdir") {
        let path = entry.expect("tempdir entry").path();
        let bytes = std::fs::read(&path).expect("read persisted file");
        let persisted = String::from_utf8_lossy(&bytes);
        assert!(
            !persisted.contains(secret) && !persisted.contains(second_secret),
            "persisted file {} leaks plaintext",
            path.display()
        );
    }
}

#[tokio::test]
async fn go_key_verification_uses_authenticated_usage_and_preserves_old_key_on_failure() {
    use pawork_auth::SecretBackend as _;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let old = "sk-go-old-test-credential";
    backend.store("pawork.opencode-go", "default", old).unwrap();
    let (adapter, _dir) = settings_adapter(server.uri(), backend.clone()).await;
    {
        let mut core = adapter.core.write().await;
        core.config
            .providers
            .push(pawork_workspace::config::ProviderConfig {
                id: "opencode-go".into(),
                base_url: Some(server.uri()),
                use_proxy: Some(false),
                ..Default::default()
            });
    }
    let mut events = adapter.subscribe_events();
    let candidate = "sk-go-new-test-credential";
    for response in [
        ResponseTemplate::new(401).set_body_string(candidate),
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"data": [{"id": "public-model"}]})),
    ] {
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/usage"))
            .and(header("authorization", format!("Bearer {candidate}")))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let error = adapter
            .command(&command_envelope(AppCommand::AuthSetApiKey {
                provider_id: "opencode-go".into(),
                api_key: pawork_protocol::ApiKeySecret::new(candidate),
            }))
            .await
            .expect_err("public catalog / invalid key must not verify");
        assert_eq!(error.code, "auth_verify");
        assert!(!error.message.contains(candidate));
        assert_eq!(backend.get("pawork.opencode-go", "default").unwrap(), old);
        let event = serde_json::to_string(&events.try_recv().unwrap()).unwrap();
        assert!(event.contains("failed"));
        assert!(!event.contains(candidate));
        server.verify().await;
        server.reset().await;
    }
    let usage = serde_json::json!({"status": "rate-limited", "percent": 100, "resetsAt": "2026-09-08T20:00:00.000Z"});
    Mock::given(method("GET"))
        .and(path("/usage"))
        .and(header("authorization", format!("Bearer {candidate}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "usage": {"rolling": usage, "weekly": usage, "monthly": usage}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let response = adapter
        .command(&command_envelope(AppCommand::AuthSetApiKey {
            provider_id: "opencode-go".into(),
            api_key: pawork_protocol::ApiKeySecret::new(candidate),
        }))
        .await
        .expect("rate-limited is still an authenticated subscription");
    assert!(!serde_json::to_string(&response)
        .unwrap()
        .contains(candidate));
    assert_eq!(
        backend.get("pawork.opencode-go", "default").unwrap(),
        candidate
    );
    server.verify().await;
}

#[tokio::test]
async fn go_account_quota_routes_next_run_and_preserves_manual_selection() {
    use pawork_auth::{add_api_key_account, list_provider_accounts, ProviderAccountSelectionMode};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let provider = "opencode-go".into();
    let first_key = "sk-g2-alpha-private-123456789";
    let second_key = "sk-g2-beta-private-987654321";
    let first = add_api_key_account(backend.as_ref(), &provider, "Alpha", first_key, true).unwrap();
    let second =
        add_api_key_account(backend.as_ref(), &provider, "Beta", second_key, false).unwrap();
    let (adapter, dir) = settings_adapter_for_channel(
        "opencode-go",
        "glm-5.3-flash",
        server.uri(),
        backend.clone(),
    )
    .await;
    adapter
        .core
        .write()
        .await
        .set_provider_use_proxy("opencode-go", false);
    let body = |percent| {
        let window = json!({"status": if percent == 100 {"rate-limited"} else {"ok"},
            "percent": percent, "resetsAt": "2099-09-09T12:30:00.000Z"});
        json!({"usage": {"rolling": window, "weekly": window, "monthly": window}})
    };
    for (key, percent) in [(first_key, 100), (second_key, 25)] {
        Mock::given(method("GET"))
            .and(path("/usage"))
            .and(header("authorization", format!("Bearer {key}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body(percent)))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"glm-5.3-flash"}]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST")).and(path("/chat/completions"))
        .and(header("authorization", format!("Bearer {second_key}")))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream")
            .set_body_string(concat!(
                "data: {\"id\":\"g2\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"G2_QUOTA_OK\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"g2\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n")))
        .expect(1).mount(&server).await;
    let query = pawork_protocol::QuotaOverviewQuery {
        provider_id: Some(provider.clone()),
        credential_id: Some(second.credential_id.clone()),
        unit: Some(pawork_protocol::QuotaUnit::Percent),
        ..pawork_protocol::QuotaOverviewQuery::default_local()
    };
    let envelope = query_envelope(AppQuery::QuotaOverview {
        query: query.clone(),
    });
    let AppResponse::Data(data) = adapter.query(&envelope).await.unwrap() else {
        panic!("quota data");
    };
    let view: pawork_protocol::QuotaOverviewView = serde_json::from_value(data.clone()).unwrap();
    assert_eq!(view.windows.len(), 3);
    assert!(view.windows.iter().all(
        |entry| matches!(&entry.read, pawork_protocol::WindowReadView::Ok { snapshot, .. }
        if snapshot.values.used == pawork_protocol::QuotaMeasure::Exact(25))
    ));
    assert!(!data.to_string().contains(second_key));
    let requests_before = server.received_requests().await.unwrap().len();
    let mut old = envelope.clone();
    old.api_version = pawork_protocol::V1_15;
    assert_eq!(adapter.query(&old).await.unwrap_err().code, "unsupported");
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        requests_before
    );
    let mut wrong = query.clone();
    wrong.account_id = "another-account".into();
    assert!(adapter
        .query(&query_envelope(AppQuery::QuotaOverview { query: wrong }))
        .await
        .is_err());
    let AppResponse::Data(legacy) = adapter
        .query(&query_envelope(AppQuery::QuotaOverview {
            query: pawork_protocol::QuotaOverviewQuery {
                provider_id: Some(provider.clone()),
                ..pawork_protocol::QuotaOverviewQuery::default_local()
            },
        }))
        .await
        .unwrap()
    else {
        panic!("legacy quota");
    };
    assert!(legacy.get("ledger").is_some());
    let mode = command_envelope(AppCommand::AuthAccountSetSelectionMode {
        provider_id: provider.clone(),
        mode: pawork_protocol::ProviderAccountSelectionMode::WhenExhausted,
    });
    adapter.command(&mode).await.unwrap();
    let mut old_status = query_envelope(AppQuery::ProviderAuthStatus {
        provider_id: Some(provider.clone()),
    });
    old_status.api_version = pawork_protocol::V1_15;
    let AppResponse::Data(old_status) = adapter.query(&old_status).await.unwrap() else {
        panic!("status");
    };
    assert!(old_status["providers"][0].get("selection_mode").is_none());
    let session = adapter
        .core
        .read()
        .await
        .create_session_unbound("G2 quota routing")
        .await
        .unwrap();
    let mut events = adapter.subscribe_events();
    let AppResponse::Accepted {
        run_id: Some(run), ..
    } = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session.clone(),
            user_message: "hi".into(),
            model: Some("glm-5.3-flash".into()),
            provider: Some(provider.clone()),
            profile: None,
        }))
        .await
        .unwrap()
    else {
        panic!("accepted");
    };
    wait_run_completed(&mut events, &run).await;
    let inventory = list_provider_accounts(backend.as_ref(), &provider).unwrap();
    assert_eq!(
        inventory.selected_credential_id.as_deref(),
        Some(second.credential_id.as_str())
    );
    assert_eq!(
        inventory.selection_mode,
        ProviderAccountSelectionMode::WhenExhausted
    );
    server.verify().await;
    let snapshot = adapter
        .core
        .read()
        .await
        .store()
        .unwrap()
        .projection_snapshot(&session)
        .await
        .unwrap();
    assert!(serde_json::to_string(&snapshot.messages)
        .unwrap()
        .contains("G2_QUOTA_OK"));
    adapter
        .command(&command_envelope(AppCommand::AuthAccountSelect {
            provider_id: provider.clone(),
            credential_id: first.credential_id.clone(),
        }))
        .await
        .unwrap();
    let inventory = list_provider_accounts(backend.as_ref(), &provider).unwrap();
    assert_eq!(
        inventory.selection_mode,
        ProviderAccountSelectionMode::Manual
    );
    let requests_before = server.received_requests().await.unwrap().len();
    assert!(adapter
        .core
        .read()
        .await
        .select_account_for_run(&CancellationToken::new())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        requests_before
    );
    // Exhausted aliases of the same subscription, malformed/expired readings,
    // and upstream failures must never cause a speculative switch.
    let mut expired = body(25);
    for window in ["rolling", "weekly", "monthly"] {
        expired["usage"][window]["resetsAt"] = json!("2000-01-01T00:00:00.000Z");
    }
    for response in [
        body(100),
        json!({"usage":{"rolling":{"status":"ok","percent":25,"resetsAt":"bad"}}}),
        expired,
    ] {
        server.reset().await;
        for (key, value) in [(first_key, body(100)), (second_key, response)] {
            Mock::given(method("GET"))
                .and(path("/usage"))
                .and(header("authorization", format!("Bearer {key}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(value))
                .mount(&server)
                .await;
        }
        pawork_auth::set_provider_account_selection_mode(
            backend.as_ref(),
            &provider,
            ProviderAccountSelectionMode::WhenExhausted,
        )
        .unwrap();
        assert!(adapter
            .core
            .read()
            .await
            .select_account_for_run(&CancellationToken::new())
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            list_provider_accounts(backend.as_ref(), &provider)
                .unwrap()
                .selected_credential_id
                .as_deref(),
            Some(first.credential_id.as_str())
        );
    }
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(ResponseTemplate::new(401).set_body_string(first_key))
        .mount(&server)
        .await;
    assert!(adapter
        .core
        .read()
        .await
        .select_account_for_run(&CancellationToken::new())
        .await
        .unwrap()
        .is_none());
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(&path).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains(first_key) && !text.contains(second_key));
        }
    }
}

#[tokio::test]
async fn auth_set_api_key_verify_failure_keeps_old_credential() {
    use pawork_auth::SecretBackend as _;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let old_secret = "sk-old-secret-00000000000000";
    let new_secret = "sk-new-invalid-1234567890ab";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(401).set_body_string("invalid api key"))
        .expect(1)
        .mount(&server)
        .await;

    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    backend
        .store("pawork.glm-coding", "default", old_secret)
        .expect("seed old credential");
    let (adapter, _dir) = settings_adapter(server.uri(), backend.clone()).await;
    let mut events = adapter.subscribe_events();

    let error = adapter
        .command(&command_envelope(AppCommand::AuthSetApiKey {
            provider_id: pawork_domain::ProviderId::from("glm-coding"),
            api_key: pawork_protocol::ApiKeySecret::new(new_secret),
        }))
        .await
        .expect_err("verification must fail closed");
    assert_eq!(error.code, "auth_verify");
    assert!(!error.message.contains(new_secret), "error leaks plaintext");

    assert_eq!(
        backend
            .get("pawork.glm-coding", "default")
            .expect("old credential retained"),
        old_secret,
        "failed verification must not replace the stored key"
    );
    let event = events.try_recv().expect("AuthChanged::Failed event");
    let event_wire = serde_json::to_string(&event).expect("serialize event");
    assert!(event_wire.contains("\"failed\""), "missing failed state");
    assert!(!event_wire.contains(new_secret), "event leaks plaintext");
    server.verify().await;
}

// ---- SET-6c 工具与 MCP（ADR-049）----

/// 构造带 MCP 段生效配置的 adapter：merged 视图经 extra 注入（模拟
/// loader 已发现 Global 层），盘上内容由测试自行播种保持一致。
pub(super) async fn mcp_settings_adapter(
    mcp: serde_json::Value,
) -> (GuiHostAdapter, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let mut config = pawork_workspace::config::PaworkConfig::default();
    config
        .providers
        .push(pawork_workspace::config::ProviderConfig {
            id: "glm-coding".into(),
            base_url: Some("http://127.0.0.1:1".into()),
            default: None,
            use_proxy: None,
            disabled_models: Vec::new(),
        });
    config.extra.insert("mcp".into(), mcp);
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(Vec::new())),
        None,
        pawork_domain::ModelId::from("glm-5.2"),
        pawork_domain::ProviderId::from("glm-coding"),
        Some(store),
    )
    .with_state(config, backend as Arc<dyn pawork_auth::SecretBackend>);
    (GuiHostAdapter::new(Arc::new(core)), dir)
}

#[tokio::test]
async fn mcp_server_remove_clears_disk_secret_and_memory() {
    // 写盘与 mcp-auth.json 目标均经 HOME 重定向到临时目录。
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));

    // Global 盘上播种 demo + keep（demo 带一个 SecretRef header）。
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config dir");
    let seeded = r#"trust_workspaces = true

[mcp.servers.demo]
transport = { kind = "http", url = "https://mcp.example.com/mcp", headers = { Authorization = { service = "pawork.mcp.demo", account = "cred-1" } } }

[mcp.servers.keep]
transport = { kind = "http", url = "https://keep.example.com/mcp" }
"#;
    std::fs::write(&config_path, seeded).expect("seed global config");

    let (adapter, _dir) = mcp_settings_adapter(serde_json::json!({
        "servers": {
            "demo": {
                "transport": {
                    "kind": "http",
                    "url": "https://mcp.example.com/mcp",
                    "headers": {
                        "Authorization": {
                            "service": "pawork.mcp.demo",
                            "account": "cred-1"
                        }
                    }
                }
            },
            "keep": {
                "transport": { "kind": "http", "url": "https://keep.example.com/mcp" }
            }
        }
    }))
    .await;

    let secret_backend = crate::extensions::mcp_secret_backend();
    secret_backend
        .store("pawork.mcp.demo", "cred-1", "sk-mcp-value")
        .expect("store mcp secret");

    let response = adapter
        .command(&command_envelope(AppCommand::McpServerRemove {
            name: "demo".into(),
        }))
        .await
        .expect("mcp_server_remove");
    let AppResponse::Data(data) = response else {
        panic!("McpServerRemove must return Data: {response:?}")
    };
    let names: Vec<&str> = data["servers"]
        .as_array()
        .expect("servers array")
        .iter()
        .map(|server| server["name"].as_str().expect("server name"))
        .collect();
    assert_eq!(names, vec!["keep"]);

    // 盘：demo 条目消失；未知字段与其它 server 原样保留。
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        !persisted.contains("demo"),
        "demo must be gone: {persisted}"
    );
    assert!(persisted.contains("trust_workspaces = true"));
    // toml 序列化会把单键子表折叠为 [mcp.servers.keep.transport] 形态的
    // header，按前缀断言，不写死 header 形态。
    assert!(
        persisted.contains("mcp.servers.keep"),
        "keep must be preserved: {persisted}"
    );

    // 密：pawork.mcp.demo 下的 SecretRef 已清理。
    assert!(matches!(
        secret_backend.get("pawork.mcp.demo", "cred-1"),
        Err(pawork_auth::AuthError::NotFound)
    ));

    // 内存：同会话重查 mcp_list 不再含 demo。
    let list = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list after remove");
    let AppResponse::Data(list) = list else {
        panic!("McpList must return Data: {list:?}")
    };
    let list_wire = serde_json::to_string(&list).expect("serialize list");
    assert!(
        !list_wire.contains("demo"),
        "mcp_list must not contain demo: {list_wire}"
    );
    assert!(list_wire.contains("keep"));
}

#[tokio::test]
async fn mcp_server_remove_unknown_name_fails_closed() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));

    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config dir");
    let seeded = r#"[mcp.servers.demo]
transport = { kind = "http", url = "https://mcp.example.com/mcp", headers = { Authorization = { service = "pawork.mcp.demo", account = "cred-1" } } }
"#;
    std::fs::write(&config_path, seeded).expect("seed global config");

    let (adapter, _dir) = mcp_settings_adapter(serde_json::json!({
        "servers": {
            "demo": {
                "transport": {
                    "kind": "http",
                    "url": "https://mcp.example.com/mcp",
                    "headers": {
                        "Authorization": {
                            "service": "pawork.mcp.demo",
                            "account": "cred-1"
                        }
                    }
                }
            }
        }
    }))
    .await;

    let secret_backend = crate::extensions::mcp_secret_backend();
    secret_backend
        .store("pawork.mcp.demo", "cred-1", "sk-mcp-value")
        .expect("store mcp secret");

    let error = adapter
        .command(&command_envelope(AppCommand::McpServerRemove {
            name: "ghost".into(),
        }))
        .await
        .expect_err("unknown server must fail closed");
    assert_eq!(error.code, "unknown_mcp_server");

    // 三处皆不动：盘字节一致、SecretRef 保留、内存 mcp_list 仍含 demo。
    let after = std::fs::read_to_string(&config_path).expect("config after");
    assert_eq!(seeded, after);
    assert_eq!(
        secret_backend
            .get("pawork.mcp.demo", "cred-1")
            .expect("secret must be kept"),
        "sk-mcp-value"
    );
    let list = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list after failure");
    let AppResponse::Data(list) = list else {
        panic!("McpList must return Data: {list:?}")
    };
    assert!(
        serde_json::to_string(&list)
            .expect("serialize list")
            .contains("demo"),
        "demo must remain in mcp_list: {list}"
    );
}

#[tokio::test]
async fn mcp_test_unknown_name_fails_closed_and_keeps_list() {
    let (adapter, _dir) = mcp_settings_adapter(serde_json::json!({
        "servers": {
            "demo": { "transport": { "kind": "http", "url": "http://127.0.0.1:1/mcp" } }
        }
    }))
    .await;

    let before = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list before");
    let error = adapter
        .command(&command_envelope(AppCommand::McpTest {
            name: "ghost".into(),
        }))
        .await
        .expect_err("unknown server must fail closed");
    assert_eq!(error.code, "unknown_mcp_server");
    let after = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list after");
    assert_eq!(
        serde_json::to_string(&before).expect("serialize before"),
        serde_json::to_string(&after).expect("serialize after"),
        "mcp_list must be unchanged by the failed test"
    );
}

#[tokio::test]
async fn mcp_test_unreachable_http_fails_closed_and_keeps_slot_state() {
    let ws = tempfile::tempdir().expect("workspace tempdir");
    let (adapter, _dir) = mcp_settings_adapter(serde_json::json!({
        "servers": {
            "demo": { "transport": { "kind": "http", "url": "http://127.0.0.1:1/mcp" } }
        }
    }))
    .await;
    // test_one_mcp 需要已附加 workspace 才会走到建连；预置 connected slot，
    // 断言失败路径不覆盖既有 slot 状态（fail-closed）。
    {
        let mut core = adapter.core.write().await;
        core.attach_workspace(ws.path()).expect("attach workspace");
        core.extensions
            .mcp_servers
            .push(crate::extensions::McpServerSlot {
                name: "demo".into(),
                transport: "http".into(),
                state: "connected".into(),
                last_error: None,
                tools: Vec::new(),
                client: None,
            });
    }

    let before = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list before");
    let error = adapter
        .command(&command_envelope(AppCommand::McpTest {
            name: "demo".into(),
        }))
        .await
        .expect_err("unreachable http server must fail closed");
    assert_eq!(error.code, "app_error");
    let after = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list after");
    assert_eq!(
        serde_json::to_string(&before).expect("serialize before"),
        serde_json::to_string(&after).expect("serialize after"),
        "slot state must be unchanged by the failed test"
    );
    let AppResponse::Data(list) = after else {
        panic!("McpList must return Data: {after:?}")
    };
    let demo = list["servers"]
        .as_array()
        .expect("servers array")
        .iter()
        .find(|server| server["name"] == "demo")
        .expect("demo slot retained");
    assert_eq!(demo["state"], "connected");
    assert_eq!(demo["last_error"], serde_json::Value::Null);
}

#[tokio::test]
async fn mcp_server_remove_same_name_in_workspace_layer_fails_closed() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));

    // Global 盘上播种 demo；workspace 层再定义同名 demo（跨层同名）。
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config dir");
    let seeded = r#"[mcp.servers.demo]
transport = { kind = "http", url = "https://mcp.example.com/mcp", headers = { Authorization = { service = "pawork.mcp.demo", account = "cred-1" } } }
"#;
    std::fs::write(&config_path, seeded).expect("seed global config");
    let ws = tempfile::tempdir().expect("workspace tempdir");
    let ws_config = ws.path().join(".pawork").join("config.toml");
    std::fs::create_dir_all(ws_config.parent().expect("ws config parent"))
        .expect("create ws config dir");
    std::fs::write(
        &ws_config,
        "[mcp.servers.demo]\ntransport = { kind = \"http\", url = \"https://workspace.example.com/mcp\" }\n",
    )
    .expect("seed workspace config");

    let (adapter, _dir) = mcp_settings_adapter(serde_json::json!({
        "servers": {
            "demo": {
                "transport": {
                    "kind": "http",
                    "url": "https://mcp.example.com/mcp",
                    "headers": {
                        "Authorization": {
                            "service": "pawork.mcp.demo",
                            "account": "cred-1"
                        }
                    }
                }
            }
        }
    }))
    .await;
    adapter
        .core
        .write()
        .await
        .attach_workspace(ws.path())
        .expect("attach workspace");

    let secret_backend = crate::extensions::mcp_secret_backend();
    secret_backend
        .store("pawork.mcp.demo", "cred-1", "sk-mcp-value")
        .expect("store mcp secret");

    let error = adapter
        .command(&command_envelope(AppCommand::McpServerRemove {
            name: "demo".into(),
        }))
        .await
        .expect_err("cross-layer same name must fail closed");
    assert_eq!(error.code, "mcp_server_defined_in_other_layers");
    assert!(
        error.message.contains("also defined"),
        "message must state the server is also defined elsewhere: {}",
        error.message
    );
    assert!(
        error.message.contains("workspace"),
        "message must name the other layer: {}",
        error.message
    );

    // 三处皆不动：盘字节一致、SecretRef 保留、内存 mcp_list 仍含 demo。
    let after = std::fs::read_to_string(&config_path).expect("config after");
    assert_eq!(seeded, after);
    assert!(ws_config.is_file(), "workspace layer untouched");
    assert_eq!(
        secret_backend
            .get("pawork.mcp.demo", "cred-1")
            .expect("secret must be kept"),
        "sk-mcp-value"
    );
    let list = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list after failure");
    let AppResponse::Data(list) = list else {
        panic!("McpList must return Data: {list:?}")
    };
    assert!(
        serde_json::to_string(&list)
            .expect("serialize list")
            .contains("demo"),
        "demo must remain in mcp_list: {list}"
    );
}

// ---- SET-4 A3：xAI 双认证（auth_set_api_key 走 verify-then-replace 门）----

#[tokio::test]
async fn xai_auth_set_api_key_main_path_connects_via_api_key() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let secret = "xai-live-key-1234567890abcdef";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", &format!("Bearer {secret}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "data": [{ "id": "grok-4" }] })),
        )
        .expect(1..)
        .mount(&server)
        .await;

    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) =
        settings_adapter_for_channel("xai", "grok-4", server.uri(), backend).await;

    let response = adapter
        .command(&command_envelope(AppCommand::AuthSetApiKey {
            provider_id: pawork_domain::ProviderId::from("xai"),
            api_key: pawork_protocol::ApiKeySecret::new(secret),
        }))
        .await
        .expect("xai api key verify-then-replace succeeds");
    let AppResponse::Data(data) = response else {
        panic!("AuthSetApiKey must return Data: {response:?}")
    };
    assert_eq!(data["provider_id"], "xai");
    assert_eq!(data["method"], "api_key");
    server.verify().await;

    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(pawork_domain::ProviderId::from("xai")),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    let entry = &status["providers"][0];
    assert_eq!(entry["provider_id"], "xai");
    assert_eq!(
        entry["auth_methods"],
        serde_json::json!(["oauth", "api_key"])
    );
    // 双认证通道按实际存储形态展示：api key 凭证在，显示 method api_key。
    assert_eq!(entry["auth"]["type"], "connected");
    assert_eq!(entry["auth"]["method"], "api_key");
}

#[tokio::test]
async fn xai_auth_set_api_key_keeps_stored_oauth_credential() {
    use pawork_auth::SecretBackend as _;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let secret = "xai-replacement-key-0000000001";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "data": [{ "id": "grok-4" }] })),
        )
        .expect(1..)
        .mount(&server)
        .await;

    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let provider = pawork_domain::ProviderId::from("xai");
    pawork_auth::store_default_oauth_token(
        backend.as_ref(),
        provider.clone(),
        &pawork_auth::TokenSet {
            access_token: "xai-old-oauth-access".into(),
            refresh_token: Some("xai-old-oauth-refresh".into()),
            id_token: None,
            expires_in: Some(3600),
            token_type: "Bearer".into(),
            scope: Some("grok-cli:access".into()),
        },
    )
    .expect("seed old oauth credential");

    let (adapter, _dir) =
        settings_adapter_for_channel("xai", "grok-4", server.uri(), backend.clone()).await;
    adapter
        .command(&command_envelope(AppCommand::AuthSetApiKey {
            provider_id: provider.clone(),
            api_key: pawork_protocol::ApiKeySecret::new(secret),
        }))
        .await
        .expect("writing api key must succeed");

    // ADR-056 D1 共存语义：api key 写入不删除 OAuth default 条目。
    assert!(
        pawork_auth::load_default_oauth_meta(backend.as_ref(), &provider)
            .expect("load meta")
            .is_some(),
        "oauth meta must survive api key write"
    );
    assert!(
        pawork_auth::load_default_oauth_credential(backend.as_ref(), &provider)
            .expect("load credential")
            .is_some(),
        "oauth credential must survive api key write"
    );
    assert_eq!(
        backend
            .get("pawork.xai", "default")
            .expect("api key stored"),
        secret
    );

    // GUI 写入路径下 credentials 列表透出双凭证（固定序：api_key 在前）。
    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(provider.clone()),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    let credentials = status["providers"][0]["credentials"]
        .as_array()
        .expect("credentials array");
    assert_eq!(credentials.len(), 2, "dual credentials: {credentials:?}");
    assert_eq!(credentials[0]["kind"], "api_key");
    assert_eq!(credentials[1]["kind"], "oauth");
    server.verify().await;
}

// ---- ADR-056 OPT-3d：provider_auth_status.credentials 列表 ----

#[tokio::test]
async fn provider_auth_status_lists_dual_credentials_in_fixed_order() {
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let provider = pawork_domain::ProviderId::from("xai");
    pawork_auth::store_default_api_key(backend.as_ref(), &provider, "xai-stored-key-00000001")
        .expect("seed api key");
    pawork_auth::store_default_oauth_token(
        backend.as_ref(),
        provider,
        &pawork_auth::TokenSet {
            access_token: "xai-oauth-access-0002".into(),
            refresh_token: Some("xai-oauth-refresh-0002".into()),
            id_token: None,
            expires_in: Some(3600),
            token_type: "Bearer".into(),
            scope: None,
        },
    )
    .expect("seed oauth token");
    let (adapter, _dir) =
        settings_adapter_for_channel("xai", "grok-4", "http://127.0.0.1:1".into(), backend).await;

    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(pawork_domain::ProviderId::from("xai")),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    let entry = &status["providers"][0];
    let credentials = entry["credentials"].as_array().expect("credentials array");
    assert_eq!(credentials.len(), 2, "dual credentials: {credentials:?}");
    // 固定序：api_key 在前、oauth 在后；api key 的 expires_at 为 null 但键保留。
    assert_eq!(credentials[0]["kind"], "api_key");
    assert_eq!(credentials[0]["expired"], false);
    assert_eq!(credentials[0]["expires_at"], serde_json::json!(null));
    assert!(credentials[0]["masked_credential"].as_str().is_some());
    assert_eq!(credentials[1]["kind"], "oauth");
    assert_eq!(credentials[1]["expired"], false);
    assert!(
        credentials[1]["expires_at"]
            .as_str()
            .is_some_and(|value| value.ends_with('Z')),
        "oauth expires_at must be ISO-8601: {credentials:?}"
    );
    assert!(credentials[1]["masked_credential"].as_str().is_some());
    // 诚实性边界：明文不入列。
    let wire = serde_json::to_string(&status).expect("serialize status");
    assert!(
        !wire.contains("xai-stored-key-00000001"),
        "credentials leak plaintext"
    );
    assert!(
        !wire.contains("xai-oauth-access-0002"),
        "credentials leak plaintext"
    );
}

#[tokio::test]
async fn provider_auth_status_marks_expired_oauth_credential() {
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let provider = pawork_domain::ProviderId::from("xai");
    // expires_in = 0：到期时刻即写入时刻，查询时已过当前时刻。
    pawork_auth::store_default_oauth_token(
        backend.as_ref(),
        provider,
        &pawork_auth::TokenSet {
            access_token: "xai-oauth-access-0003".into(),
            refresh_token: Some("xai-oauth-refresh-0003".into()),
            id_token: None,
            expires_in: Some(0),
            token_type: "Bearer".into(),
            scope: None,
        },
    )
    .expect("seed oauth token");
    let (adapter, _dir) =
        settings_adapter_for_channel("xai", "grok-4", "http://127.0.0.1:1".into(), backend).await;

    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(pawork_domain::ProviderId::from("xai")),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    let credentials = status["providers"][0]["credentials"]
        .as_array()
        .expect("credentials array");
    assert_eq!(
        credentials.len(),
        1,
        "only the oauth entry: {credentials:?}"
    );
    assert_eq!(credentials[0]["kind"], "oauth");
    assert_eq!(credentials[0]["expired"], true);
    assert!(credentials[0]["expires_at"].as_str().is_some());
}

#[tokio::test]
async fn provider_auth_status_env_fallback_not_listed_as_credential() {
    let _restore_env = RestoreEnvVar(
        "PAWORK_API_KEY_GLM_CODING".into(),
        std::env::var_os("PAWORK_API_KEY_GLM_CODING"),
    );
    crate::testsupport::set_env("PAWORK_API_KEY_GLM_CODING", "sk-env-fallback-0001");
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(pawork_domain::ProviderId::from("glm-coding")),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    let entry = &status["providers"][0];
    // env 命中仍是 provider 级 Connected（masked null），但 env fallback
    // 不是存储凭证，不入 credentials 列表。
    assert_eq!(entry["auth"]["type"], "connected");
    assert_eq!(entry["auth"]["method"], "api_key");
    assert_eq!(entry["auth"]["masked_credential"], serde_json::json!(null));
    assert_eq!(
        entry["credentials"],
        serde_json::json!([]),
        "env fallback is not a stored credential"
    );
    let wire = serde_json::to_string(&status).expect("serialize status");
    assert!(!wire.contains("sk-env-fallback-0001"), "env value leaks");
}

#[tokio::test]
async fn xai_api_key_verification_flight_rejects_auth_cancel_and_remove() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let secret = "xai-cancel-guard-key-000000001";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(3000))
                .set_body_json(serde_json::json!({ "data": [{ "id": "grok-4" }] })),
        )
        .expect(1..)
        .mount(&server)
        .await;

    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) =
        settings_adapter_for_channel("xai", "grok-4", server.uri(), backend).await;
    let mut events = adapter.subscribe_events();

    let set_envelope = command_envelope(AppCommand::AuthSetApiKey {
        provider_id: pawork_domain::ProviderId::from("xai"),
        api_key: pawork_protocol::ApiKeySecret::new(secret),
    });
    let (set_outcome, cancel_outcome) = tokio::join!(adapter.command(&set_envelope), async {
        // 等待 api-key 验证 flight 真正登记（auth_state 报 connecting）再取消，
        // 避免竞态下取消落在 flight 登记之前。
        for _ in 0..150 {
            let status = adapter
                .query(&query_envelope(AppQuery::ProviderAuthStatus {
                    provider_id: Some(pawork_domain::ProviderId::from("xai")),
                }))
                .await
                .expect("auth status poll");
            let AppResponse::Data(status) = status else {
                panic!("ProviderAuthStatus must return Data: {status:?}")
            };
            if status["providers"][0]["auth"]["type"] == "connecting" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let remove_error = adapter
            .command(&command_envelope(AppCommand::AuthRemove {
                provider_id: pawork_domain::ProviderId::from("xai"),
            }))
            .await
            .expect_err("remove must not race with credential verification");
        assert_eq!(remove_error.code, "busy");
        adapter
            .command(&command_envelope(AppCommand::AuthCancel {
                provider_id: pawork_domain::ProviderId::from("xai"),
            }))
            .await
    },);

    // D3：api-key 验证 flight 不可取消——拒绝取消且登记保留，验证本身完成。
    let cancel_error = cancel_outcome.expect_err("cancel of api-key flight must be rejected");
    assert_eq!(cancel_error.code, "unsupported");
    let response = set_outcome.expect("verification must complete despite rejected cancel attempt");
    let AppResponse::Data(data) = response else {
        panic!("AuthSetApiKey must return Data: {response:?}")
    };
    assert_eq!(data["method"], "api_key");

    // 拒绝取消不发 Cancelled；事件流首个认证事件是验证成功。
    let event = events.try_recv().expect("AuthChanged event");
    let event_wire = serde_json::to_string(&event).expect("serialize event");
    assert!(
        event_wire.contains("\"succeeded\""),
        "expected Succeeded event first: {event_wire}"
    );
    assert!(
        !event_wire.contains("\"cancelled\""),
        "rejected cancel must not emit Cancelled: {event_wire}"
    );
    adapter
        .command(&command_envelope(AppCommand::AuthRemove {
            provider_id: pawork_domain::ProviderId::from("xai"),
        }))
        .await
        .expect("remove succeeds once verification has finished");
    assert!(adapter.auth_flights.lock().expect("flights").is_empty());
    server.verify().await;
}

// ---- ADR-055 OPT-3a/3b：模型启用集与默认角色 ----

/// 启停 roundtrip + model_list 两态（缺省过滤禁用、include_disabled 全量）。
#[tokio::test]
async fn set_model_enabled_roundtrip_and_model_list_states() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let provider = pawork_domain::ProviderId::from("glm-coding");

    let response = adapter
        .command(&command_envelope(AppCommand::SetModelEnabled {
            provider_id: provider.clone(),
            model_id: "glm-5.2".into(),
            enabled: false,
        }))
        .await
        .expect("disable model");
    let AppResponse::Data(data) = response else {
        panic!("SetModelEnabled must return Data: {response:?}")
    };
    assert_eq!(data["provider_id"], "glm-coding");
    assert_eq!(data["model_id"], "glm-5.2");
    assert_eq!(data["enabled"], false);
    assert_eq!(data["cleared_roles"], serde_json::json!([]));

    // 缺省 model_list 不含禁用模型。
    let list = adapter
        .query(&query_envelope(AppQuery::ModelList {
            provider_id: Some(provider.clone()),
            include_disabled: false,
        }))
        .await
        .expect("model list default");
    let AppResponse::Data(list) = list else {
        panic!("ModelList must return Data: {list:?}")
    };
    let entries = list.as_array().expect("entries");
    assert!(
        entries.iter().all(|entry| entry["id"] != "glm-5.2"),
        "disabled model must be filtered by default: {list}"
    );

    // include_disabled=true 取全量目录并标注 enabled=false。
    let full = adapter
        .query(&query_envelope(AppQuery::ModelList {
            provider_id: Some(provider.clone()),
            include_disabled: true,
        }))
        .await
        .expect("model list full");
    let AppResponse::Data(full) = full else {
        panic!("ModelList must return Data: {full:?}")
    };
    let entries = full.as_array().expect("entries");
    let disabled_entry = entries
        .iter()
        .find(|entry| entry["id"] == "glm-5.2")
        .expect("include_disabled must keep glm-5.2");
    assert_eq!(disabled_entry["enabled"], false);
    assert!(
        entries
            .iter()
            .filter(|entry| entry["id"] != "glm-5.2")
            .all(|entry| entry["enabled"] == true),
        "other entries must be enabled: {full}"
    );

    // 禁用模型不能再设为默认（D4）。
    let error = adapter
        .command(&command_envelope(AppCommand::SetDefaultModel {
            provider_id: provider.clone(),
            model_id: "glm-5.2".into(),
        }))
        .await
        .expect_err("disabled default must fail closed");
    assert_eq!(error.code, "model_disabled");

    // 重新启用：denylist 移除，缺省列表恢复。
    let response = adapter
        .command(&command_envelope(AppCommand::SetModelEnabled {
            provider_id: provider.clone(),
            model_id: "glm-5.2".into(),
            enabled: true,
        }))
        .await
        .expect("re-enable model");
    let AppResponse::Data(data) = response else {
        panic!("SetModelEnabled must return Data: {response:?}")
    };
    assert_eq!(data["enabled"], true);
    assert_eq!(data["cleared_roles"], serde_json::json!([]));
    let list = adapter
        .query(&query_envelope(AppQuery::ModelList {
            provider_id: Some(provider),
            include_disabled: false,
        }))
        .await
        .expect("model list after re-enable");
    let AppResponse::Data(list) = list else {
        panic!("ModelList must return Data: {list:?}")
    };
    assert!(
        list.as_array()
            .expect("entries")
            .iter()
            .any(|entry| entry["id"] == "glm-5.2" && entry["enabled"] == true),
        "re-enabled model must be back: {list}"
    );
}

/// D3：禁用命中 conversation/naming 角色默认对 → 同批清除并如实回报。
#[tokio::test]
async fn set_model_enabled_clears_role_defaults() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let provider = pawork_domain::ProviderId::from("glm-coding");
    let pair = pawork_protocol::DefaultModelPair {
        provider_id: "glm-coding".into(),
        model_id: "glm-5.2".into(),
    };

    adapter
        .command(&command_envelope(AppCommand::SetDefaultModel {
            provider_id: provider.clone(),
            model_id: "glm-5.2".into(),
        }))
        .await
        .expect("seed default pair");
    adapter
        .command(&command_envelope(AppCommand::SetDefaultRoleModel {
            role: "naming".into(),
            value: Some(pair.clone()),
        }))
        .await
        .expect("seed naming pair");

    let response = adapter
        .command(&command_envelope(AppCommand::SetModelEnabled {
            provider_id: provider.clone(),
            model_id: "glm-5.2".into(),
            enabled: false,
        }))
        .await
        .expect("disable model");
    let AppResponse::Data(data) = response else {
        panic!("SetModelEnabled must return Data: {response:?}")
    };
    assert_eq!(
        data["cleared_roles"],
        serde_json::json!(["conversation", "naming"])
    );

    // 内存生效配置同步：default 与 role_defaults.naming 均为 null。
    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(provider),
        }))
        .await
        .expect("status after disable");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    assert!(status["default"].is_null(), "cleared default: {status}");
    assert!(
        status["role_defaults"]["naming"].is_null(),
        "cleared naming: {status}"
    );
    assert!(
        status["role_defaults"]["vision"].is_null() && status["role_defaults"]["search"].is_null(),
        "untouched roles stay null: {status}"
    );

    // 角色键对确实从 Global 配置移除。
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        !persisted.contains("default_provider") && !persisted.contains("naming_provider"),
        "cleared role keys must be removed: {persisted}"
    );
}

/// D3 回归：清除判定以盘上持久化配置为准。内存生效配置被 CLI
/// `--provider/--model` 覆盖（不落盘）时，禁用覆盖值指向的 provider/model
/// 不得误删盘上真实的默认对，内存覆盖值也不得被清掉。
#[tokio::test]
async fn disable_keeps_persisted_role_pairs_under_memory_override() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    // 内存生效配置默认对 = (glm-coding, glm-5.2)，模拟 CLI 覆盖。
    let (adapter, _dir) = settings_adapter_with_default(
        "glm-coding",
        "glm-5.2",
        "http://127.0.0.1:1".into(),
        backend,
        Some(("glm-coding", "glm-5.2")),
    )
    .await;
    // 盘上持久化默认对是另一 provider 的真实用户配置。
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    pawork_workspace::config::write_model_pair(
        &config_path,
        "default_provider",
        "default_model",
        Some(("deepseek", "deepseek-chat")),
    )
    .expect("seed persisted default pair");
    // 另一实例已保存的禁用项尚未进入本 Host 内存，单项操作必须保留。
    pawork_workspace::config::write_provider_disabled_models(
        &config_path,
        "glm-coding",
        &["temporarily-unlisted-model".into()],
    )
    .expect("seed newer persisted denylist");
    let provider = pawork_domain::ProviderId::from("glm-coding");

    // 单模型禁用：覆盖值命中内存默认对，但盘上默认对不属于该 provider。
    let response = adapter
        .command(&command_envelope(AppCommand::SetModelEnabled {
            provider_id: provider.clone(),
            model_id: "glm-5.2".into(),
            enabled: false,
        }))
        .await
        .expect("disable model");
    let AppResponse::Data(data) = response else {
        panic!("SetModelEnabled must return Data: {response:?}")
    };
    assert_eq!(data["cleared_roles"], serde_json::json!([]));
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        persisted.contains("temporarily-unlisted-model"),
        "single-model disable must preserve newer persisted choices: {persisted}"
    );
    assert!(
        persisted.contains("default_provider = \"deepseek\"")
            && persisted.contains("default_model = \"deepseek-chat\""),
        "persisted default pair must survive memory-override disable: {persisted}"
    );

    // 内存覆盖值保留（诚实显示当前生效默认，哪怕已禁用）。
    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(provider.clone()),
        }))
        .await
        .expect("status after disable");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    assert_eq!(
        status["default"],
        serde_json::json!({"provider_id": "glm-coding", "model_id": "glm-5.2"}),
        "memory override pair must stay: {status}"
    );

    // 恢复后走全关路径：同样不得误删盘上默认对。
    adapter
        .command(&command_envelope(AppCommand::SetModelEnabled {
            provider_id: provider.clone(),
            model_id: "glm-5.2".into(),
            enabled: true,
        }))
        .await
        .expect("re-enable model");
    let response = adapter
        .command(&command_envelope(AppCommand::SetProviderModelsEnabled {
            provider_id: provider.clone(),
            enabled: false,
        }))
        .await
        .expect("disable all glm-coding models");
    let AppResponse::Data(data) = response else {
        panic!("SetProviderModelsEnabled must return Data: {response:?}")
    };
    assert_eq!(data["cleared_roles"], serde_json::json!([]));
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        persisted.contains("default_provider = \"deepseek\"")
            && persisted.contains("default_model = \"deepseek-chat\""),
        "persisted default pair must survive disable-all: {persisted}"
    );
    assert!(
        persisted.contains("temporarily-unlisted-model"),
        "disable-all must retain disabled models absent from today's catalog: {persisted}"
    );
    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(provider),
        }))
        .await
        .expect("status after disable-all");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    assert_eq!(
        status["default"],
        serde_json::json!({"provider_id": "glm-coding", "model_id": "glm-5.2"}),
        "memory override pair must stay after disable-all: {status}"
    );
}

/// 全关按聚合目录展开；空目录 catalog_unavailable fail-closed 不写盘。
#[tokio::test]
async fn set_provider_models_enabled_expands_catalog_and_empty_fails_closed() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend.clone()).await;
    let provider = pawork_domain::ProviderId::from("glm-coding");

    let response = adapter
        .command(&command_envelope(AppCommand::SetProviderModelsEnabled {
            provider_id: provider.clone(),
            enabled: false,
        }))
        .await
        .expect("disable all glm-coding models");
    let AppResponse::Data(data) = response else {
        panic!("SetProviderModelsEnabled must return Data: {response:?}")
    };
    assert_eq!(data["provider_id"], "glm-coding");
    assert_eq!(data["enabled"], false);
    assert_eq!(data["cleared_roles"], serde_json::json!([]));

    // 展开写入静态目录全部模型。
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        persisted.contains("disabled_models") && persisted.contains("\"glm-5.2\""),
        "full disable must expand the catalog: {persisted}"
    );
    let full = adapter
        .query(&query_envelope(AppQuery::ModelList {
            provider_id: Some(provider.clone()),
            include_disabled: true,
        }))
        .await
        .expect("model list full");
    let AppResponse::Data(full) = full else {
        panic!("ModelList must return Data: {full:?}")
    };
    assert!(
        full.as_array()
            .expect("entries")
            .iter()
            .all(|entry| entry["enabled"] == false),
        "all entries disabled: {full}"
    );

    // 再全开：denylist 清空，缺省列表恢复。
    adapter
        .command(&command_envelope(AppCommand::SetProviderModelsEnabled {
            provider_id: provider,
            enabled: true,
        }))
        .await
        .expect("re-enable all");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        !persisted.contains("disabled_models"),
        "re-enable must clear the denylist: {persisted}"
    );

    // 空目录（无静态条目、无探测结果）fail-closed：catalog_unavailable，
    // 不写盘。
    let (adapter, _dir) = settings_adapter_for_channel(
        "custom-empty",
        "model-x",
        "http://127.0.0.1:1".into(),
        backend,
    )
    .await;
    let error = adapter
        .command(&command_envelope(AppCommand::SetProviderModelsEnabled {
            provider_id: pawork_domain::ProviderId::from("custom-empty"),
            enabled: false,
        }))
        .await
        .expect_err("empty catalog must fail closed");
    assert_eq!(error.code, "catalog_unavailable", "error: {error:?}");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        !persisted.contains("custom-empty"),
        "empty catalog must not write: {persisted}"
    );
}

/// 角色设/清回执与未知 role fail-closed；禁用模型不能设为角色默认。
#[tokio::test]
async fn set_default_role_model_set_clear_unknown_role_and_disabled_value() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let provider = pawork_domain::ProviderId::from("glm-coding");
    let pair = pawork_protocol::DefaultModelPair {
        provider_id: "glm-coding".into(),
        model_id: "glm-5.2".into(),
    };

    let response = adapter
        .command(&command_envelope(AppCommand::SetDefaultRoleModel {
            role: "naming".into(),
            value: Some(pair.clone()),
        }))
        .await
        .expect("set naming pair");
    let AppResponse::Data(data) = response else {
        panic!("SetDefaultRoleModel must return Data: {response:?}")
    };
    assert_eq!(data["role"], "naming");
    assert_eq!(data["value"]["provider_id"], "glm-coding");
    assert_eq!(data["value"]["model_id"], "glm-5.2");
    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(provider.clone()),
        }))
        .await
        .expect("status after set");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    assert_eq!(
        status["role_defaults"]["naming"],
        serde_json::json!({ "provider_id": "glm-coding", "model_id": "glm-5.2" })
    );

    let response = adapter
        .command(&command_envelope(AppCommand::SetDefaultRoleModel {
            role: "naming".into(),
            value: None,
        }))
        .await
        .expect("clear naming pair");
    let AppResponse::Data(data) = response else {
        panic!("SetDefaultRoleModel clear must return Data: {response:?}")
    };
    assert_eq!(data["role"], "naming");
    assert!(data["value"].is_null(), "clear receipt is null: {data}");
    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(provider.clone()),
        }))
        .await
        .expect("status after clear");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    assert!(status["role_defaults"]["naming"].is_null(), "{status}");
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        !persisted.contains("naming_provider"),
        "cleared naming pair must not persist: {persisted}"
    );

    // 未知 role fail-closed。
    let error = adapter
        .command(&command_envelope(AppCommand::SetDefaultRoleModel {
            role: "nope".into(),
            value: None,
        }))
        .await
        .expect_err("unknown role must fail closed");
    assert_eq!(error.code, "unknown_role", "error: {error:?}");

    // 禁用模型不能设为角色默认（D4/D5 设置校验）。
    adapter
        .command(&command_envelope(AppCommand::SetModelEnabled {
            provider_id: provider,
            model_id: "glm-5.2".into(),
            enabled: false,
        }))
        .await
        .expect("disable glm-5.2");
    let error = adapter
        .command(&command_envelope(AppCommand::SetDefaultRoleModel {
            role: "vision".into(),
            value: Some(pair),
        }))
        .await
        .expect_err("disabled role value must fail closed");
    assert_eq!(error.code, "model_disabled", "error: {error:?}");
}
