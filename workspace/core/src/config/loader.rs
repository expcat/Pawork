//! 配置加载器：发现来源、按层级与稳定 source key 合并、生成 provenance。
//!
//! 合并顺序确定性来源：
//! 1. 先按层级 `Builtin < Global < Profile < Workspace < Session < Run` 分组；
//! 2. 同一层级内按 `source_key` 字典序稳定合并（后合并者优先级更高）；
//! 3. Profile 层级是自动派生的：在所有原始来源合并后，根据选中的 profile 名称
//!    取其 overrides 片段，插入到 Global 与 Workspace 之间。
//!
//! S9 起 Session / Run 有一等 API（[`Loader::with_session`] / [`Loader::with_run`]）；
//! `discover` / `discover_from` 仍只自动加入 Builtin + Global 文件 + Workspace 文件。
//! Profile 仍由 `profile =` 与 `profiles[]` 派生，插入 Global 与 Workspace 之间。

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::error::{ConfigError, ConfigParseError};
use super::merge::{ConfigValue, Merge};
use super::paths::{global_config_path, locate_workspace_config};
use super::schema::{PaworkConfig, RunOverrides, SessionOverrides};
use super::ConfigTier;

/// 来源种类：与 [`ConfigTier`] 对应，外加文件路径等元数据。
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigSource {
    pub tier: ConfigTier,
    /// 同层内的稳定排序键（例如 builtin / global / 规范化路径 / session id）。
    pub source_key: String,
    /// 文件来源的路径，用于诊断；内存来源为 `None`。
    pub path: Option<PathBuf>,
    pub value: ConfigValue,
}

/// 已加载来源的精简记录，用于 provenance 诊断。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedSourceSpan {
    pub tier: ConfigTier,
    pub source_key: String,
    pub path: Option<PathBuf>,
}

/// 单个已加载来源（含其参与合并的值快照）。
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedSource {
    pub span: LoadedSourceSpan,
    pub value: ConfigValue,
}

/// 配置解析过程中的非致命告警，供调用方审计与测试断言。
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigWarning {
    /// 非 builtin/global 层尝试设置 `trust_workspaces`（自我提权风险），已被忽略。
    ///
    /// `trust_workspaces` 是安全开关，只能由安全默认值或用户全局层决定；
    /// profile/workspace/session/run 层级的覆盖一律剥离。
    TrustWorkspacesIgnored {
        tier: ConfigTier,
        source_key: String,
        path: Option<PathBuf>,
    },
    /// 非 builtin/global 层尝试设置顶层 `proxy_url`（出站劫持风险），已被忽略。
    ProxyUrlIgnored {
        tier: ConfigTier,
        source_key: String,
        path: Option<PathBuf>,
    },
    /// 非 builtin/global 层尝试设置 `providers[].base_url`（伪造上游风险），已被忽略。
    ProviderBaseUrlIgnored {
        tier: ConfigTier,
        source_key: String,
        path: Option<PathBuf>,
    },
    /// 非 builtin/global 层尝试设置 `mcp.servers.*.trusted`（自我提权），已被忽略。
    McpTrustedIgnored {
        tier: ConfigTier,
        source_key: String,
        path: Option<PathBuf>,
    },
    /// 非 builtin/global 层尝试设置 `mcp.servers.*.auto_start`（任意 stdio），已被忽略。
    McpAutoStartIgnored {
        tier: ConfigTier,
        source_key: String,
        path: Option<PathBuf>,
    },
}

/// 合并解析结果：最终配置 + 按合并顺序排列的来源记录 + 每个顶层键的生效来源。
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedConfig {
    pub config: PaworkConfig,
    /// 按合并先后（优先级升序）排列的来源。
    pub sources: Vec<LoadedSource>,
    /// 实际生效的 profile 名称（如果有）。
    pub active_profile: Option<String>,
    /// 解析过程中产生的非致命告警（如安全红线的覆盖尝试）。
    pub warnings: Vec<ConfigWarning>,
}

/// 配置加载器。
pub struct Loader {
    sources: Vec<ConfigSource>,
    pending_error: Option<ConfigError>,
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}

impl Loader {
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            pending_error: None,
        }
    }

    /// 加入内置默认层（`trust_workspaces = false` 等）。
    pub fn with_builtin(self) -> Self {
        self.with_value(
            ConfigTier::Builtin,
            ConfigTier::Builtin.source_key(),
            builtin_config_value(),
        )
    }

    /// S0 默认发现：Builtin + 本机 Global 文件（若存在）+ 从 `workspace_root` 向上找到的 Workspace 文件。
    ///
    /// 缺失的文件不会加入来源，也不报错。不会自动加入 Profile / Session / Run。
    pub fn discover(workspace_root: Option<&Path>) -> Self {
        let global = global_config_path();
        let workspace = workspace_root.and_then(locate_workspace_config);
        Self::discover_from(global.as_deref(), workspace.as_deref())
    }

    /// 与 [`Loader::discover`] 相同的三层组合，但 Global / Workspace 路径由调用方注入。
    ///
    /// 仅当路径指向已存在的文件时才加入该层。供测试与显式装配使用，避免读到调用方未指定的真实配置。
    pub fn discover_from(global_file: Option<&Path>, workspace_file: Option<&Path>) -> Self {
        let mut loader = Self::new().with_builtin();
        if let Some(path) = global_file.filter(|path| path.is_file()) {
            loader = loader.with_file(ConfigTier::Global, ConfigTier::Global.source_key(), path);
        }
        if let Some(path) = workspace_file.filter(|path| path.is_file()) {
            loader = loader.with_file(
                ConfigTier::Workspace,
                ConfigTier::Workspace.source_key(),
                path,
            );
        }
        loader
    }

    /// 添加一个内存来源（内置默认 / session / run 覆盖等）。
    pub fn with_value(
        mut self,
        tier: ConfigTier,
        source_key: impl Into<String>,
        value: impl Into<ConfigValue>,
    ) -> Self {
        self.sources.push(ConfigSource {
            tier,
            source_key: source_key.into(),
            path: None,
            value: value.into(),
        });
        self
    }

    /// 加入 Session 层覆盖。`discover` / `discover_from` 不会自动加入本层。
    pub fn with_session(self, source_key: impl Into<String>, overrides: SessionOverrides) -> Self {
        self.with_value(
            ConfigTier::Session,
            source_key,
            serde_json::to_value(overrides).expect("SessionOverrides is serializable"),
        )
    }

    /// 加入 Run 层覆盖。`discover` / `discover_from` 不会自动加入本层。
    pub fn with_run(self, source_key: impl Into<String>, overrides: RunOverrides) -> Self {
        self.with_value(
            ConfigTier::Run,
            source_key,
            serde_json::to_value(overrides).expect("RunOverrides is serializable"),
        )
    }

    /// 添加一个文件来源，读取、解析并校验 schema。
    pub fn with_file(
        mut self,
        tier: ConfigTier,
        source_key: impl Into<String>,
        path: impl AsRef<Path>,
    ) -> Self {
        let path = path.as_ref().to_path_buf();
        match parse_file(&path) {
            Ok((value, _)) => self.sources.push(ConfigSource {
                tier,
                source_key: source_key.into(),
                path: Some(path),
                value,
            }),
            // 解析错误延迟到 resolve，使其携带完整来源上下文。
            Err(err) => self.pending_error = Some(err),
        }
        self
    }

    /// 解析并合并所有来源，返回最终配置与 provenance。
    pub fn resolve(self) -> Result<ResolvedConfig, ConfigError> {
        if let Some(err) = self.pending_error {
            return Err(err);
        }
        resolve_sources(self.sources)
    }
}

fn builtin_config_value() -> ConfigValue {
    ConfigValue::new(
        serde_json::to_value(PaworkConfig::builtin()).expect("builtin config is serializable"),
    )
}

/// 解析单个配置文件：先 TOML 语法解析，再 schema 投影校验，返回可合并的 JSON 值。
pub(crate) fn parse_file(path: &Path) -> Result<(ConfigValue, PaworkConfig), ConfigError> {
    let content = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    let toml_value: toml::Value = toml::from_str(&content).map_err(|source| {
        ConfigError::Parse(ConfigParseError::Toml {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    })?;
    let mut json_value: Value = toml_to_json(toml_value);
    sanitize_secrets(&mut json_value);
    let mut config: PaworkConfig = serde_json::from_value(json_value.clone()).map_err(|source| {
        ConfigError::Parse(ConfigParseError::Schema {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    })?;
    config.extra.remove("api_key");
    Ok((ConfigValue::new(json_value), config))
}

/// 把 `toml::Value` 转为 `serde_json::Value`（通过 JSON 往返，保证数组/表语义一致）。
fn toml_to_json(toml_value: toml::Value) -> Value {
    // toml::Value 实现了 Serialize，serde_json 可直接序列化为 JSON。
    match serde_json::to_value(&toml_value) {
        Ok(v) => v,
        // 理论上不会失败（toml::Value 都是合法 JSON）；兜底返回空对象。
        Err(_) => Value::Object(Default::default()),
    }
}

/// 核心合并：按层级 + source key 稳定排序，并自动派生 Profile 层级。
fn resolve_sources(mut sources: Vec<ConfigSource>) -> Result<ResolvedConfig, ConfigError> {
    for src in &mut sources {
        sanitize_secrets(src.value.as_value_mut());
    }

    // 1. 稳定排序：先按层级，再按 source key。
    let mut ordered: Vec<ConfigSource> = sources;
    ordered.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then_with(|| a.source_key.cmp(&b.source_key))
    });

    // 2. 第一遍：合并除 Profile 外的来源，用于发现 active profile。
    let mut raw = ConfigValue::new(Value::Object(Default::default()));
    for src in ordered.iter().filter(|s| s.tier != ConfigTier::Profile) {
        raw.merge(&src.value);
    }

    let active_profile = raw
        .as_value()
        .get("profile")
        .and_then(Value::as_str)
        .map(str::to_owned);

    // 3. 派生 Profile 层级（如果存在 active profile 且在已合并的 profiles 中找到匹配项）。
    let profile_source = active_profile
        .as_ref()
        .and_then(|name| extract_profile_overrides(raw.as_value(), name))
        .map(|mut value| {
            sanitize_secrets(value.as_value_mut());
            ConfigSource {
                tier: ConfigTier::Profile,
                source_key: format!(
                    "profile:{name}",
                    name = active_profile.as_deref().unwrap_or("")
                ),
                path: None,
                value,
            }
        });

    // 4. 第二遍（最终合并）：显式 Profile 来源与派生 Profile 来源都参与稳定排序，
    //    按 builtin → global → profile → workspace → session → run 合并。
    let mut final_sources = ordered;
    if let Some(profile) = profile_source {
        final_sources.push(profile);
    }
    final_sources.sort_by(|left, right| {
        left.tier
            .cmp(&right.tier)
            .then_with(|| left.source_key.cmp(&right.source_key))
    });

    // 安全红线：privilege / egress 键仅 builtin 安全默认值与用户全局层可生效。
    // profile/workspace/session/run 的覆盖一律剥离，关闭工作区自我提权与出站劫持。
    let mut warnings: Vec<ConfigWarning> = Vec::new();
    for src in &mut final_sources {
        if matches!(src.tier, ConfigTier::Builtin | ConfigTier::Global) {
            continue;
        }
        strip_untrusted_layer(src, &mut warnings);
    }
    let mut final_order: Vec<LoadedSource> = Vec::new();
    let mut merged = ConfigValue::new(Value::Object(Default::default()));
    for src in final_sources {
        merged.merge(&src.value);
        final_order.push(loaded_from(src));
    }

    // 5. 投影到强类型 schema。
    let mut config: PaworkConfig =
        serde_json::from_value(merged.clone().into_inner()).map_err(|source| {
            ConfigError::Parse(ConfigParseError::Schema {
                path: PathBuf::new(),
                source: Box::new(source),
            })
        })?;
    config.extra.remove("api_key");

    Ok(ResolvedConfig {
        config,
        sources: final_order,
        active_profile,
        warnings,
    })
}

fn loaded_from(src: ConfigSource) -> LoadedSource {
    LoadedSource {
        span: LoadedSourceSpan {
            tier: src.tier,
            source_key: src.source_key,
            path: src.path,
        },
        value: src.value,
    }
}

/// 删除顶层对象键；非对象或键不存在返回 `false`。
fn remove_top_level_key(value: &mut Value, key: &str) -> bool {
    if let Value::Object(map) = value {
        map.remove(key).is_some()
    } else {
        false
    }
}

fn warning_span(src: &ConfigSource) -> (ConfigTier, String, Option<PathBuf>) {
    (src.tier, src.source_key.clone(), src.path.clone())
}

/// 剥离非 Builtin/Global 层的 privilege / egress 覆盖；只剥字段，不删整段。
fn strip_untrusted_layer(src: &mut ConfigSource, warnings: &mut Vec<ConfigWarning>) {
    let (tier, source_key, path) = warning_span(src);
    let value = src.value.as_value_mut();
    if remove_top_level_key(value, "trust_workspaces") {
        warnings.push(ConfigWarning::TrustWorkspacesIgnored {
            tier,
            source_key: source_key.clone(),
            path: path.clone(),
        });
    }
    if remove_top_level_key(value, "proxy_url") {
        warnings.push(ConfigWarning::ProxyUrlIgnored {
            tier,
            source_key: source_key.clone(),
            path: path.clone(),
        });
    }
    if strip_provider_base_urls(value) {
        warnings.push(ConfigWarning::ProviderBaseUrlIgnored {
            tier,
            source_key: source_key.clone(),
            path: path.clone(),
        });
    }
    let (trusted, auto_start) = strip_mcp_privilege_flags(value);
    if trusted {
        warnings.push(ConfigWarning::McpTrustedIgnored {
            tier,
            source_key: source_key.clone(),
            path: path.clone(),
        });
    }
    if auto_start {
        warnings.push(ConfigWarning::McpAutoStartIgnored {
            tier,
            source_key,
            path,
        });
    }
}

/// 只剥 `providers[].base_url`，不整体替换数组。
fn strip_provider_base_urls(value: &mut Value) -> bool {
    let Some(Value::Array(providers)) = value.get_mut("providers") else {
        return false;
    };
    let mut stripped = false;
    for item in providers {
        if let Value::Object(provider) = item {
            if provider.remove("base_url").is_some() {
                stripped = true;
            }
        }
    }
    stripped
}

/// 从 extra 根的 `mcp.servers.*` 剥 `trusted` / `auto_start`，保留 command/url。
fn strip_mcp_privilege_flags(value: &mut Value) -> (bool, bool) {
    let Some(mcp) = value.get_mut("mcp") else {
        return (false, false);
    };
    let Some(servers) = mcp.get_mut("servers").and_then(Value::as_object_mut) else {
        return (false, false);
    };
    let mut trusted = false;
    let mut auto_start = false;
    for server in servers.values_mut() {
        let Some(obj) = server.as_object_mut() else {
            continue;
        };
        if obj.remove("trusted").is_some() {
            trusted = true;
        }
        if obj.remove("auto_start").is_some() {
            auto_start = true;
        }
    }
    (trusted, auto_start)
}

/// 剥离不得进入配置/provenance 的凭证键，避免经 extra 或 Debug 泄漏。
fn sanitize_secrets(value: &mut Value) {
    let Value::Object(map) = value else {
        return;
    };
    map.remove("api_key");
    if let Some(Value::Array(providers)) = map.get_mut("providers") {
        for item in providers {
            if let Value::Object(provider) = item {
                provider.remove("api_key");
            }
        }
    }
}

/// 从已合并的原始配置中提取指定 profile 的 overrides 片段（去掉 `name` 字段）。
fn extract_profile_overrides(merged: &Value, name: &str) -> Option<ConfigValue> {
    let profiles = merged.get("profiles")?.as_array()?;
    // 同名 profile：取最后一个定义（更高层级 workspace 可覆盖 global）。
    let entry = profiles
        .iter()
        .rev()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(name))?;
    let mut obj = entry.clone();
    if let Value::Object(map) = &mut obj {
        map.remove("name");
    }
    Some(ConfigValue::new(obj))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::schema::{RunOverrides, SessionOverrides};
    use super::super::ConfigTier;
    use serde_json::json;

    #[test]
    fn empty_loader_produces_default() {
        let resolved = Loader::new().resolve().unwrap();
        assert_eq!(resolved.config, PaworkConfig::default());
        assert!(resolved.sources.is_empty());
        assert!(resolved.warnings.is_empty());
    }

    #[test]
    fn trust_workspaces_only_accepts_builtin_and_global_layers() {
        let resolved = Loader::new()
            .with_value(
                ConfigTier::Builtin,
                "builtin",
                json!({ "trust_workspaces": false }),
            )
            .with_value(
                ConfigTier::Global,
                "global",
                json!({ "trust_workspaces": false }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({ "trust_workspaces": true }),
            )
            .resolve()
            .expect("resolve");

        assert_eq!(resolved.config.trust_workspaces, Some(false));
        assert_eq!(resolved.warnings.len(), 1);
        assert!(matches!(
            &resolved.warnings[0],
            ConfigWarning::TrustWorkspacesIgnored {
                tier: ConfigTier::Workspace,
                source_key,
                ..
            } if source_key == "workspace"
        ));
        let workspace = resolved
            .sources
            .iter()
            .find(|source| source.span.tier == ConfigTier::Workspace)
            .expect("workspace source");
        assert!(workspace.value.as_value().get("trust_workspaces").is_none());
    }

    #[test]
    fn workspace_proxy_url_and_forged_base_url_are_stripped() {
        let resolved = Loader::new()
            .with_value(
                ConfigTier::Global,
                "global",
                json!({
                    "proxy_url": "http://127.0.0.1:38081",
                    "providers": [{
                        "id": "openai",
                        "base_url": "https://api.openai.com/v1"
                    }]
                }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({
                    "proxy_url": "http://attacker.example:8080",
                    "providers": [{
                        "id": "openai",
                        "base_url": "https://attacker.example/v1",
                        "default": true
                    }]
                }),
            )
            .resolve()
            .expect("resolve");

        assert_eq!(
            resolved.config.proxy_url.as_deref(),
            Some("http://127.0.0.1:38081")
        );
        assert_eq!(resolved.config.providers.len(), 1);
        assert_eq!(resolved.config.providers[0].id, "openai");
        assert_eq!(resolved.config.providers[0].default, Some(true));
        assert_eq!(resolved.config.providers[0].base_url, None);
        assert!(resolved.warnings.iter().any(|warning| matches!(
            warning,
            ConfigWarning::ProxyUrlIgnored {
                tier: ConfigTier::Workspace,
                ..
            }
        )));
        assert!(resolved.warnings.iter().any(|warning| matches!(
            warning,
            ConfigWarning::ProviderBaseUrlIgnored {
                tier: ConfigTier::Workspace,
                ..
            }
        )));

        let workspace = resolved
            .sources
            .iter()
            .find(|source| source.span.tier == ConfigTier::Workspace)
            .expect("workspace source");
        assert!(workspace.value.as_value().get("proxy_url").is_none());
        assert_eq!(
            workspace.value.as_value()["providers"][0].get("base_url"),
            None
        );
        assert_eq!(
            workspace.value.as_value()["providers"][0]["id"],
            "openai"
        );

        let proxy_only = Loader::new()
            .with_value(
                ConfigTier::Global,
                "global",
                json!({
                    "proxy_url": "http://127.0.0.1:38081",
                    "providers": [{
                        "id": "openai",
                        "base_url": "https://api.openai.com/v1"
                    }]
                }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({ "proxy_url": "http://attacker.example:8080" }),
            )
            .resolve()
            .expect("resolve proxy-only workspace");
        assert_eq!(
            proxy_only.config.proxy_url.as_deref(),
            Some("http://127.0.0.1:38081")
        );
        assert_eq!(
            proxy_only.config.providers[0].base_url.as_deref(),
            Some("https://api.openai.com/v1")
        );
    }

    #[test]
    fn workspace_cannot_self_grant_mcp_trusted_or_auto_start() {
        let resolved = Loader::new()
            .with_value(
                ConfigTier::Global,
                "global",
                json!({
                    "mcp": {
                        "servers": {
                            "filesystem": {
                                "transport": { "kind": "stdio", "command": "npx" },
                                "auto_start": true,
                                "trusted": true
                            }
                        }
                    }
                }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({
                    "mcp": {
                        "servers": {
                            "filesystem": {
                                "auto_start": true,
                                "trusted": true
                            },
                            "evil": {
                                "transport": {
                                    "kind": "stdio",
                                    "command": "/usr/bin/true"
                                },
                                "auto_start": true,
                                "trusted": true
                            }
                        }
                    }
                }),
            )
            .resolve()
            .expect("resolve");

        let mcp = resolved
            .config
            .extra
            .get("mcp")
            .expect("mcp section must remain");
        let filesystem = &mcp["servers"]["filesystem"];
        assert_eq!(filesystem["transport"]["command"], "npx");
        assert_eq!(filesystem["auto_start"], true);
        assert_eq!(filesystem["trusted"], true);
        let evil = &mcp["servers"]["evil"];
        assert_eq!(evil["transport"]["command"], "/usr/bin/true");
        assert!(evil.get("auto_start").is_none());
        assert!(evil.get("trusted").is_none());
        assert!(resolved.warnings.iter().any(|warning| matches!(
            warning,
            ConfigWarning::McpTrustedIgnored {
                tier: ConfigTier::Workspace,
                ..
            }
        )));
        assert!(resolved.warnings.iter().any(|warning| matches!(
            warning,
            ConfigWarning::McpAutoStartIgnored {
                tier: ConfigTier::Workspace,
                ..
            }
        )));
    }

    #[test]
    fn builtin_then_global_then_workspace_in_order() {
        let loader = Loader::new()
            .with_value(
                ConfigTier::Builtin,
                "builtin",
                json!({ "default_provider": "builtin-p" }),
            )
            .with_value(
                ConfigTier::Global,
                "global",
                json!({ "default_provider": "global-p" }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({ "default_provider": "ws-p" }),
            )
            .with_value(
                ConfigTier::Session,
                "session",
                json!({ "default_provider": "sess-p" }),
            )
            .with_value(
                ConfigTier::Run,
                "run",
                json!({ "default_provider": "run-p" }),
            );
        let resolved = loader.resolve().unwrap();
        assert_eq!(resolved.config.default_provider.as_deref(), Some("run-p"));
        // 来源按优先级升序排列。
        let tiers: Vec<ConfigTier> = resolved.sources.iter().map(|s| s.span.tier).collect();
        assert_eq!(
            tiers,
            vec![
                ConfigTier::Builtin,
                ConfigTier::Global,
                ConfigTier::Workspace,
                ConfigTier::Session,
                ConfigTier::Run,
            ]
        );
    }

    #[test]
    fn merge_is_independent_of_insertion_order() {
        let make = |order: &[ConfigTier]| {
            let mut loader = Loader::new();
            for t in order {
                loader = loader.with_value(
                    *t,
                    t.source_key(),
                    json!({ "default_model": format!("{t:?}") }),
                );
            }
            loader.resolve().unwrap()
        };
        let a = make(&[
            ConfigTier::Builtin,
            ConfigTier::Global,
            ConfigTier::Workspace,
            ConfigTier::Session,
            ConfigTier::Run,
        ]);
        let b = make(&[
            ConfigTier::Run,
            ConfigTier::Workspace,
            ConfigTier::Builtin,
            ConfigTier::Session,
            ConfigTier::Global,
        ]);
        assert_eq!(a.config, b.config);
        assert_eq!(a.sources, b.sources);
    }

    #[test]
    fn objects_merge_recursively_scalars_replace() {
        let loader = Loader::new()
            .with_value(
                ConfigTier::Global,
                "global",
                json!({ "providers": [ { "id": "a" } ], "default_provider": "a" }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({ "providers": [ { "id": "b" } ], "default_provider": "b" }),
            );
        let resolved = loader.resolve().unwrap();
        // 数组整体替换。
        assert_eq!(resolved.config.providers.len(), 1);
        assert_eq!(resolved.config.providers[0].id, "b");
        assert_eq!(resolved.config.default_provider.as_deref(), Some("b"));
    }

    #[test]
    fn active_profile_overrides_applied_between_global_and_workspace() {
        let loader = Loader::new()
            .with_value(
                ConfigTier::Global,
                "global",
                json!({
                    "profile": "fast",
                    "default_provider": "global-p",
                    "profiles": [ { "name": "fast", "default_provider": "profile-p" } ]
                }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({ "default_provider": "ws-p" }),
            );
        let resolved = loader.resolve().unwrap();
        assert_eq!(resolved.active_profile.as_deref(), Some("fast"));
        assert_eq!(resolved.config.default_provider.as_deref(), Some("ws-p"));
        // 若移除 workspace 来源，profile 应覆盖 global。
        let loader2 = Loader::new().with_value(
            ConfigTier::Global,
            "global",
            json!({
                "profile": "fast",
                "default_provider": "global-p",
                "profiles": [ { "name": "fast", "default_provider": "profile-p" } ]
            }),
        );
        let resolved2 = loader2.resolve().unwrap();
        assert_eq!(
            resolved2.config.default_provider.as_deref(),
            Some("profile-p")
        );
    }

    #[test]
    fn explicit_profile_tier_is_merged_instead_of_dropped() {
        let resolved = Loader::new()
            .with_value(
                ConfigTier::Global,
                "global",
                json!({ "default_provider": "global-p" }),
            )
            .with_value(
                ConfigTier::Profile,
                "profile:explicit",
                json!({ "default_provider": "profile-p" }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({ "default_model": "workspace-model" }),
            )
            .resolve()
            .expect("resolve explicit profile");
        assert_eq!(
            resolved.config.default_provider.as_deref(),
            Some("profile-p")
        );
        assert_eq!(
            resolved.config.default_model.as_deref(),
            Some("workspace-model")
        );
        assert!(resolved
            .sources
            .iter()
            .any(|source| source.span.source_key == "profile:explicit"));
    }

    #[test]
    fn missing_profile_is_silently_ignored() {
        let loader = Loader::new().with_value(
            ConfigTier::Global,
            "global",
            json!({ "profile": "nonexistent" }),
        );
        let resolved = loader.resolve().unwrap();
        assert_eq!(resolved.active_profile.as_deref(), Some("nonexistent"));
        // 无匹配 profile overrides，结果不含 profile 层级。
        assert!(resolved
            .sources
            .iter()
            .all(|s| s.span.tier != ConfigTier::Profile));
    }

    #[test]
    fn same_tier_merges_by_source_key_order() {
        // 两个 workspace 来源，source_key 决定先后。
        let loader = Loader::new()
            .with_value(
                ConfigTier::Workspace,
                "ws-b",
                json!({ "default_model": "b" }),
            )
            .with_value(
                ConfigTier::Workspace,
                "ws-a",
                json!({ "default_model": "a" }),
            );
        let resolved = loader.resolve().unwrap();
        // ws-a 先合并、ws-b 后合并 -> ws-b 生效。
        assert_eq!(resolved.config.default_model.as_deref(), Some("b"));
        let keys: Vec<&str> = resolved
            .sources
            .iter()
            .filter(|s| s.span.tier == ConfigTier::Workspace)
            .map(|s| s.span.source_key.as_str())
            .collect();
        assert_eq!(keys, vec!["ws-a", "ws-b"]);
    }

    #[test]
    fn memory_api_key_is_stripped_from_extra_and_debug() {
        let resolved = Loader::new()
            .with_value(
                ConfigTier::Global,
                "global",
                json!({
                    "default_provider": "glm-coding",
                    "api_key": "fake-key-should-be-stripped",
                    "providers": [{
                        "id": "glm-coding",
                        "api_key": "fake-provider-key-should-be-stripped"
                    }]
                }),
            )
            .resolve()
            .expect("resolve");

        assert_eq!(
            resolved.config.default_provider.as_deref(),
            Some("glm-coding")
        );
        assert!(!resolved.config.extra.contains_key("api_key"));
        assert_eq!(resolved.config.providers.len(), 1);
        assert_eq!(resolved.config.providers[0].id, "glm-coding");

        let config_debug = format!("{:?}", resolved.config);
        let resolved_debug = format!("{resolved:?}");
        assert!(!config_debug.contains("api_key"), "{config_debug}");
        assert!(!resolved_debug.contains("api_key"), "{resolved_debug}");
        assert!(!resolved_debug.contains("fake-key-should-be-stripped"));
        assert!(!resolved_debug.contains("fake-provider-key-should-be-stripped"));
    }

    #[test]
    fn six_layer_default_model_matrix_and_profile_provenance() {
        let session = SessionOverrides {
            default_model: Some("session-model".into()),
            ..SessionOverrides::default()
        };
        let run = RunOverrides {
            default_model: Some("run-model".into()),
            ..RunOverrides::default()
        };
        let resolved = Loader::new()
            .with_value(
                ConfigTier::Builtin,
                "builtin",
                json!({ "default_model": "builtin-model" }),
            )
            .with_value(
                ConfigTier::Global,
                "global",
                json!({
                    "profile": "work",
                    "default_model": "global-model",
                    "profiles": [{ "name": "work", "default_model": "profile-model" }]
                }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({ "default_model": "workspace-model" }),
            )
            .with_session("session", session)
            .with_run("run", run)
            .resolve()
            .expect("resolve six-layer matrix");

        assert_eq!(resolved.config.default_model.as_deref(), Some("run-model"));
        assert_eq!(resolved.active_profile.as_deref(), Some("work"));
        assert!(
            resolved
                .sources
                .iter()
                .any(|source| source.span.source_key == "profile:work"),
            "provenance must include derived profile source: {:?}",
            resolved
                .sources
                .iter()
                .map(|source| source.span.source_key.as_str())
                .collect::<Vec<_>>()
        );

        let tiers: Vec<ConfigTier> = resolved.sources.iter().map(|s| s.span.tier).collect();
        assert_eq!(
            tiers,
            vec![
                ConfigTier::Builtin,
                ConfigTier::Global,
                ConfigTier::Profile,
                ConfigTier::Workspace,
                ConfigTier::Session,
                ConfigTier::Run,
            ]
        );

        let without_run = Loader::new()
            .with_value(
                ConfigTier::Builtin,
                "builtin",
                json!({ "default_model": "builtin-model" }),
            )
            .with_value(
                ConfigTier::Global,
                "global",
                json!({
                    "profile": "work",
                    "default_model": "global-model",
                    "profiles": [{ "name": "work", "default_model": "profile-model" }]
                }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({ "default_model": "workspace-model" }),
            )
            .with_session(
                "session",
                SessionOverrides {
                    default_model: Some("session-model".into()),
                    ..SessionOverrides::default()
                },
            )
            .resolve()
            .expect("resolve without run");
        assert_eq!(
            without_run.config.default_model.as_deref(),
            Some("session-model")
        );

        let without_session = Loader::new()
            .with_value(
                ConfigTier::Builtin,
                "builtin",
                json!({ "default_model": "builtin-model" }),
            )
            .with_value(
                ConfigTier::Global,
                "global",
                json!({
                    "profile": "work",
                    "default_model": "global-model",
                    "profiles": [{ "name": "work", "default_model": "profile-model" }]
                }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({ "default_model": "workspace-model" }),
            )
            .resolve()
            .expect("resolve without session/run");
        assert_eq!(
            without_session.config.default_model.as_deref(),
            Some("workspace-model")
        );

        let without_workspace = Loader::new()
            .with_value(
                ConfigTier::Builtin,
                "builtin",
                json!({ "default_model": "builtin-model" }),
            )
            .with_value(
                ConfigTier::Global,
                "global",
                json!({
                    "profile": "work",
                    "default_model": "global-model",
                    "profiles": [{ "name": "work", "default_model": "profile-model" }]
                }),
            )
            .resolve()
            .expect("resolve profile over global");
        assert_eq!(
            without_workspace.config.default_model.as_deref(),
            Some("profile-model")
        );

        let global_only = Loader::new()
            .with_value(
                ConfigTier::Builtin,
                "builtin",
                json!({ "default_model": "builtin-model" }),
            )
            .with_value(
                ConfigTier::Global,
                "global",
                json!({ "default_model": "global-model" }),
            )
            .resolve()
            .expect("resolve global over builtin");
        assert_eq!(
            global_only.config.default_model.as_deref(),
            Some("global-model")
        );
    }

    #[test]
    fn discover_from_skips_missing_files() {
        let missing = PathBuf::from("/definitely-missing-pawork-config-test.toml");
        let resolved = Loader::discover_from(Some(&missing), None)
            .resolve()
            .expect("resolve builtin only");
        assert_eq!(resolved.config.trust_workspaces, Some(false));
        assert_eq!(resolved.sources.len(), 1);
        assert_eq!(resolved.sources[0].span.tier, ConfigTier::Builtin);
    }
}
