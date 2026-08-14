//! 集成测试：基于真实文件系统的配置加载、错误带路径、路径发现与确定性。

use std::fs;

use pawork_config::{
    config_dir_for_app, locate_workspace_config, ConfigTier, ConfigWarning, Loader,
};

fn tempdir(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("pawork-{prefix}-"))
        .tempdir()
        .expect("create temp dir")
}

fn write(path: &std::path::Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn loads_toml_files_and_merges_by_tier() {
    let root = tempdir("merge");
    let root_path = root.path();
    let global = root_path.join("global").join("config.toml");
    let workspace_dir = root_path.join("ws");
    let workspace = workspace_dir.join(".pawork").join("config.toml");

    write(
        &global,
        r#"
default_provider = "global-provider"
default_model = "global-model"

[[providers]]
id = "openai"
base_url = "https://global.example/v1"
"#,
    );
    write(
        &workspace,
        r#"
default_model = "ws-model"
"#,
    );

    let resolved = Loader::new()
        .with_file(ConfigTier::Global, "global", &global)
        .with_file(ConfigTier::Workspace, "workspace", &workspace)
        .resolve()
        .expect("resolve");

    assert_eq!(
        resolved.config.default_provider.as_deref(),
        Some("global-provider")
    );
    assert_eq!(resolved.config.default_model.as_deref(), Some("ws-model"));
    assert_eq!(resolved.config.providers.len(), 1);
    assert_eq!(
        resolved.config.providers[0].base_url.as_deref(),
        Some("https://global.example/v1")
    );
}

#[test]
fn s0_discover_from_merges_builtin_global_workspace() {
    let root = tempdir("s0");
    let global = root.path().join("global.toml");
    let workspace = root.path().join(".pawork").join("config.toml");

    write(
        &global,
        r#"
default_provider = "global-p"
default_model = "global-m"
trust_workspaces = true
"#,
    );
    write(
        &workspace,
        r#"
default_model = "ws-m"
trust_workspaces = true
"#,
    );

    let resolved = Loader::discover_from(Some(&global), Some(&workspace))
        .resolve()
        .expect("resolve s0 defaults");

    assert_eq!(resolved.config.trust_workspaces, Some(true));
    assert_eq!(resolved.config.default_provider.as_deref(), Some("global-p"));
    assert_eq!(resolved.config.default_model.as_deref(), Some("ws-m"));

    let tiers: Vec<ConfigTier> = resolved.sources.iter().map(|s| s.span.tier).collect();
    assert_eq!(
        tiers,
        vec![
            ConfigTier::Builtin,
            ConfigTier::Global,
            ConfigTier::Workspace,
        ]
    );
    assert!(resolved.warnings.iter().any(|warning| matches!(
        warning,
        ConfigWarning::TrustWorkspacesIgnored {
            tier: ConfigTier::Workspace,
            ..
        }
    )));
}

#[test]
fn file_api_key_is_stripped_and_absent_from_debug() {
    let root = tempdir("secret");
    let global = root.path().join("config.toml");
    write(
        &global,
        r#"
default_provider = "glm-coding"
api_key = "fake-key-must-not-persist"

[[providers]]
id = "glm-coding"
base_url = "https://example.test/v1"
api_key = "fake-provider-key-must-not-persist"
"#,
    );

    let resolved = Loader::discover_from(Some(&global), None)
        .resolve()
        .expect("resolve");

    assert_eq!(
        resolved.config.default_provider.as_deref(),
        Some("glm-coding")
    );
    assert_eq!(resolved.config.providers.len(), 1);
    assert_eq!(resolved.config.providers[0].id, "glm-coding");
    assert_eq!(
        resolved.config.providers[0].base_url.as_deref(),
        Some("https://example.test/v1")
    );
    assert!(!resolved.config.extra.contains_key("api_key"));

    let config_debug = format!("{:?}", resolved.config);
    let resolved_debug = format!("{resolved:?}");
    assert!(!config_debug.contains("api_key"), "{config_debug}");
    assert!(!resolved_debug.contains("api_key"), "{resolved_debug}");
    assert!(!resolved_debug.contains("fake-key-must-not-persist"));
    assert!(!resolved_debug.contains("fake-provider-key-must-not-persist"));
}

#[test]
fn parse_error_carries_path() {
    let root = tempdir("err");
    let bad = root.path().join("bad.toml");
    write(&bad, "this is = = not valid toml [[");

    let err = Loader::new()
        .with_file(ConfigTier::Global, "global", &bad)
        .resolve()
        .expect_err("should fail");

    let path = err.path().expect("error should carry a path");
    assert_eq!(path, bad.as_path());
}

#[test]
fn schema_mismatch_error_carries_path() {
    let root = tempdir("schema");
    let bad = root.path().join("bad.toml");
    // providers 应为数组，这里给字符串触发 schema 校验失败。
    write(&bad, "providers = \"not-an-array\"\n");

    let err = Loader::new()
        .with_file(ConfigTier::Global, "global", &bad)
        .resolve()
        .expect_err("should fail");

    let path = err.path().expect("schema error should carry a path");
    assert_eq!(path, bad.as_path());
}

#[test]
fn locate_workspace_config_finds_nearest() {
    let root = tempdir("locate");
    // canonicalize：macOS /var -> /private/var 符号链接，需与 locate 返回值一致。
    // 与实现一致使用 dunce（Windows 下不带 \\?\ 前缀）。
    let root_path = dunce::canonicalize(root.path()).unwrap_or_else(|_| root.path().to_path_buf());
    let nested = root_path.join("a/b/c");
    fs::create_dir_all(&nested).unwrap();

    // 在中间目录放置工作区配置。
    let mid = root_path.join("a");
    let mid_cfg = mid.join(".pawork").join("config.toml");
    write(&mid_cfg, "default_model = \"mid\"\n");

    let found = locate_workspace_config(&nested).unwrap();
    assert_eq!(found, mid_cfg);
}

#[test]
fn missing_file_is_not_fatal_when_no_source_added() {
    // 不添加任何来源时，resolve 返回默认配置而非错误。
    let resolved = Loader::new().resolve().unwrap();
    assert_eq!(resolved.config, Default::default());
}

#[test]
fn deterministic_regardless_of_file_addition_order() {
    let root = tempdir("det");
    let root_path = root.path();
    let g = root_path.join("global.toml");
    let w = root_path.join("workspace.toml");
    let s = root_path.join("session.toml");
    write(&g, "default_provider = \"g\"\n");
    write(&w, "default_provider = \"w\"\n");
    write(&s, "default_provider = \"s\"\n");

    let order_a = Loader::new()
        .with_file(ConfigTier::Global, "global", &g)
        .with_file(ConfigTier::Workspace, "workspace", &w)
        .with_file(ConfigTier::Session, "session", &s)
        .resolve()
        .unwrap();
    let order_b = Loader::new()
        .with_file(ConfigTier::Session, "session", &s)
        .with_file(ConfigTier::Workspace, "workspace", &w)
        .with_file(ConfigTier::Global, "global", &g)
        .resolve()
        .unwrap();

    assert_eq!(order_a.config, order_b.config);
    assert_eq!(order_a.sources, order_b.sources);
}

#[test]
fn global_config_dir_resolves_to_some_path() {
    // CI 与本机都应能解析出全局目录（依赖 XDG / AppData 等环境变量存在）。
    let dir = config_dir_for_app();
    assert!(
        dir.is_some(),
        "expected a resolvable global config directory"
    );
}
