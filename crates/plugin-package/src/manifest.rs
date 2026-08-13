//! Package manifest schema（P17-2）。
//!
//! Manifest 复用各子类型既有 schema，不重定义其语义：
//! - skills / agents / hooks / lsp 子段以 [`ResourceRef`] 声明归档内相对路径或内联
//!   清单；实际加载由各既有 loader 承担（本 crate 只声明 + 分发）。
//! - mcp 子段以 [`McpServerDeclaration`] 内联声明；本地 stdio 一律 sandboxed（见
//!   `mcp-client` 的 sandboxed stdio spawner，restart 不降级）。
//! - monitors 子段以 [`crate::MonitorDeclaration`] 声明稳定 driver/evaluator 入口，
//!   执行统一进入 `monitor-service` → `task-manager`。
//!
//! 为避免 TOML 对 `PathBuf`/枚举的边界问题并给出精确错误，反序列化先进入
//! [`RawManifest`]（字段为 `String` / `toml::Value`），再经 [`PackageManifest::from_raw`]
//! 校验并构造强类型 manifest（与 `resource-loader` 的 Raw→typed 约定一致）。
//!
//! # MCP 声明的有界预算（2026-08 安全复审）
//!
//! package manifest 是未受信输入：MCP stdio / http 声明在进入 `mcp-client` 之前
//! 先强制有界预算（server 数量、名称、command、args、env / headers 键、url 的
//! 长度与数量上限），并拒绝控制字符，防止声明爆炸与异常长负载。

use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::PackageError;
use crate::monitor::MonitorDeclaration;
use crate::scope::{
    parse_version, PackageDependency, PackageId, PackageRelativePath, PackageScope,
};
use crate::secret::SecretRef;
use crate::PACKAGE_MANIFEST_VERSION;

/// 归档内 manifest 文件名。
pub const MANIFEST_FILE_NAME: &str = "package.toml";

/// 单个 package 可声明的 MCP server 数量上限（有界预算：防声明爆炸）。
pub const MAX_MCP_SERVERS_PER_PACKAGE: usize = 32;
/// MCP server 名最大长度（字节）。
pub const MAX_MCP_SERVER_NAME_LEN: usize = 64;
/// stdio command 最大长度（字节）。
pub const MAX_MCP_COMMAND_LEN: usize = 512;
/// stdio 参数个数上限。
pub const MAX_MCP_ARG_COUNT: usize = 32;
/// 单个 stdio 参数最大长度（字节）。
pub const MAX_MCP_ARG_LEN: usize = 512;
/// env / headers secret 定位符映射的最大条目数。
pub const MAX_MCP_SECRET_MAP_ENTRIES: usize = 64;
/// env / headers 键最大长度（字节）。
pub const MAX_MCP_KEY_LEN: usize = 128;
/// http transport url 最大长度（字节）。
pub const MAX_MCP_URL_LEN: usize = 2048;

/// 子资源引用：归档内相对路径或内联清单。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum ResourceRef {
    /// 指向归档内一个相对路径（skill 目录 / profile / hook / lsp 描述文件）。
    Path { path: PackageRelativePath },
    /// 内联清单（JSON 兼容的 TOML 表），由对应 loader 直接解析。
    Inline { manifest: Value },
}

impl ResourceRef {
    /// 若为路径引用，返回该路径；否则 `None`。
    pub fn path(&self) -> Option<&PackageRelativePath> {
        match self {
            Self::Path { path } => Some(path),
            Self::Inline { .. } => None,
        }
    }

    /// 若为内联引用，返回其清单值；否则 `None`。
    pub fn inline(&self) -> Option<&Value> {
        match self {
            Self::Path { .. } => None,
            Self::Inline { manifest } => Some(manifest),
        }
    }
}

/// MCP server 声明（package 内联）。本地 stdio 一律经 Sandbox → Process Runtime
/// 托管；package 不提供「unsandboxed stdio」选项（见 acceptance）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerDeclaration {
    /// 稳定 server 名（不得含 `.`，与 `mcp-client` 的命名空间分隔约束一致）。
    pub name: String,
    pub transport: McpTransportSpec,
    #[serde(default)]
    pub auto_start: bool,
}

/// MCP 传输声明。package 携带的 secret 一律以 [`SecretRef`] 定位符声明（只含
/// backend 键名，不持久化明文），由宿主在安装时绑定到 `SecretBackend` 并即时
/// 解析（与 `mcp-client` 的 SecretRef 模型一致；Debug / 归档 / roundtrip 无 token）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransportSpec {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, SecretRef>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, SecretRef>,
    },
}

impl McpServerDeclaration {
    /// 是否为本地 stdio server（必须 sandboxed 托管）。
    pub fn is_stdio(&self) -> bool {
        matches!(self.transport, McpTransportSpec::Stdio { .. })
    }

    /// 校验声明自洽（server 名合法、stdio command 非空、http url 合法 scheme）并
    /// 强制 MCP 有界预算（command / args / env / headers / url 的长度与数量上限，
    /// 拒绝控制字符）。
    pub fn validate(&self) -> Result<(), PackageError> {
        validate_mcp_server_name(&self.name)?;
        let context = format!("mcp.{}", self.name);
        match &self.transport {
            McpTransportSpec::Stdio { command, args, env } => {
                if command.trim().is_empty() {
                    return Err(PackageError::field(
                        context,
                        "stdio transport requires a non-empty command",
                    ));
                }
                validate_bounded_text(&context, "command", command, MAX_MCP_COMMAND_LEN)?;
                validate_args(&context, args)?;
                validate_env_keys(&context, env)?;
                validate_secret_map(&format!("{context}.env"), env)?;
            }
            McpTransportSpec::Http { url, headers } => {
                validate_bounded_text(&context, "url", url, MAX_MCP_URL_LEN)?;
                let scheme = http_scheme(url).ok_or_else(|| {
                    PackageError::field(context.clone(), "http transport url must include a scheme")
                })?;
                if !matches!(scheme, "http" | "https") {
                    return Err(PackageError::field(
                        context,
                        "http transport requires an http/https url",
                    ));
                }
                validate_header_keys(&context, headers)?;
                validate_secret_map(&format!("{context}.headers"), headers)?;
            }
        }
        Ok(())
    }
}

/// 校验 transport 的 secret 定位符映射（env / headers 真实字段），错误字段名
/// 带上 server 与变量名上下文。
fn validate_secret_map(
    prefix: &str,
    map: &BTreeMap<String, SecretRef>,
) -> Result<(), PackageError> {
    for (key, reference) in map {
        reference.validate().map_err(|error| match error {
            PackageError::ManifestField { field, message } => PackageError::ManifestField {
                field: format!("{prefix}.{key}.{field}"),
                message,
            },
            other => other,
        })?;
    }
    Ok(())
}

/// 取 URL 的 scheme 部分，不引入 `url` 依赖；MCP transport 仅需 scheme 校验。
fn http_scheme(url: &str) -> Option<&str> {
    let url = url.trim();
    let end = url.find("://")?;
    let scheme = &url[..end];
    if scheme.is_empty()
        || scheme
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '+'))
    {
        return None;
    }
    Some(scheme)
}

/// 有界文本预算：长度上限 + 拒绝控制字符（防注入 / 异常长负载）。
fn validate_bounded_text(
    context: &str,
    field: &str,
    value: &str,
    max_len: usize,
) -> Result<(), PackageError> {
    if value.len() > max_len {
        return Err(PackageError::field(
            format!("{context}.{field}"),
            format!("exceeds maximum length of {max_len} bytes"),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(PackageError::field(
            format!("{context}.{field}"),
            "must not contain control characters",
        ));
    }
    Ok(())
}

/// stdio args 预算：数量上限 + 逐项长度 / 控制字符校验。
fn validate_args(context: &str, args: &[String]) -> Result<(), PackageError> {
    if args.len() > MAX_MCP_ARG_COUNT {
        return Err(PackageError::field(
            format!("{context}.args"),
            format!("exceeds maximum of {MAX_MCP_ARG_COUNT} arguments"),
        ));
    }
    for (index, arg) in args.iter().enumerate() {
        validate_bounded_text(context, &format!("args[{index}]"), arg, MAX_MCP_ARG_LEN)?;
    }
    Ok(())
}

/// env 键预算：条目数上限 + POSIX 变量名字符集 [A-Za-z_][A-Za-z0-9_]*。
fn validate_env_keys(context: &str, env: &BTreeMap<String, SecretRef>) -> Result<(), PackageError> {
    if env.len() > MAX_MCP_SECRET_MAP_ENTRIES {
        return Err(PackageError::field(
            format!("{context}.env"),
            format!("exceeds maximum of {MAX_MCP_SECRET_MAP_ENTRIES} entries"),
        ));
    }
    for key in env.keys() {
        if key.len() > MAX_MCP_KEY_LEN || !is_env_key(key) {
            return Err(PackageError::field(
                format!("{context}.env.{key}"),
                format!("env key must match [A-Za-z_][A-Za-z0-9_]* and be at most {MAX_MCP_KEY_LEN} bytes"),
            ));
        }
    }
    Ok(())
}

fn is_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(first) if first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

/// http header 键预算：条目数上限 + RFC 7230 token 字符集。
fn validate_header_keys(
    context: &str,
    headers: &BTreeMap<String, SecretRef>,
) -> Result<(), PackageError> {
    if headers.len() > MAX_MCP_SECRET_MAP_ENTRIES {
        return Err(PackageError::field(
            format!("{context}.headers"),
            format!("exceeds maximum of {MAX_MCP_SECRET_MAP_ENTRIES} entries"),
        ));
    }
    for key in headers.keys() {
        if key.len() > MAX_MCP_KEY_LEN || !key.bytes().all(is_header_token_char) {
            return Err(PackageError::field(
                format!("{context}.headers.{key}"),
                format!("header key must be an RFC 7230 token and at most {MAX_MCP_KEY_LEN} bytes"),
            ));
        }
    }
    Ok(())
}

fn is_header_token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// 已校验的 Package manifest。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageManifest {
    pub manifest_version: u32,
    pub id: PackageId,
    pub name: String,
    pub version: Version,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<PackageRelativePath>,
    #[serde(default)]
    pub scope: PackageScope,
    #[serde(default)]
    pub dependencies: Vec<PackageDependency>,
    #[serde(default)]
    pub skills: Vec<ResourceRef>,
    #[serde(default)]
    pub agents: Vec<ResourceRef>,
    #[serde(default)]
    pub hooks: Vec<ResourceRef>,
    #[serde(default)]
    pub mcp: Vec<McpServerDeclaration>,
    #[serde(default)]
    pub lsp: Vec<ResourceRef>,
    #[serde(default)]
    pub monitors: Vec<MonitorDeclaration>,
}

impl PackageManifest {
    /// 校验 manifest 自洽（schema 版本、name、依赖、子段）。
    pub fn validate(&self) -> Result<(), PackageError> {
        if self.manifest_version != PACKAGE_MANIFEST_VERSION {
            return Err(PackageError::UnsupportedSchemaVersion {
                found: self.manifest_version,
                expected: PACKAGE_MANIFEST_VERSION,
            });
        }
        if self.name.trim().is_empty() {
            return Err(PackageError::field(
                "name",
                "package name must not be empty",
            ));
        }
        for dependency in &self.dependencies {
            dependency.validate()?;
        }
        if self.mcp.len() > MAX_MCP_SERVERS_PER_PACKAGE {
            return Err(PackageError::field(
                "mcp",
                format!("package declares more than {MAX_MCP_SERVERS_PER_PACKAGE} mcp servers"),
            ));
        }
        for server in &self.mcp {
            server.validate()?;
        }
        for monitor in &self.monitors {
            monitor.validate()?;
        }
        // skill 目录引用：只接受 Path（skill 需目录形态的 manifest.toml + SKILL.md）。
        for (index, skill) in self.skills.iter().enumerate() {
            if skill.path().is_none() {
                return Err(PackageError::field(
                    format!("skills[{index}]"),
                    "skill entries must use a path reference (skill directory)",
                ));
            }
        }
        Ok(())
    }

    /// 该 package 是否声明了任一扩展类型。
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
            && self.agents.is_empty()
            && self.hooks.is_empty()
            && self.mcp.is_empty()
            && self.lsp.is_empty()
            && self.monitors.is_empty()
    }

    /// 子资源总数（六类之和）。
    pub fn resource_count(&self) -> usize {
        self.skills.len()
            + self.agents.len()
            + self.hooks.len()
            + self.mcp.len()
            + self.lsp.len()
            + self.monitors.len()
    }
}

fn validate_mcp_server_name(name: &str) -> Result<(), PackageError> {
    if name.is_empty() {
        return Err(PackageError::field(
            "mcp",
            "mcp server name must not be empty",
        ));
    }
    if name.contains('.') {
        return Err(PackageError::field(
            format!("mcp.{name}"),
            "mcp server name must not contain '.' (namespace separator)",
        ));
    }
    if name.len() > MAX_MCP_SERVER_NAME_LEN {
        return Err(PackageError::field(
            format!("mcp.{name}"),
            format!("mcp server name exceeds maximum length of {MAX_MCP_SERVER_NAME_LEN} bytes"),
        ));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(PackageError::field(
            format!("mcp.{name}"),
            "mcp server name must contain only ASCII alphanumerics, '-' or '_'",
        ));
    }
    Ok(())
}

/// TOML 反序列化的中间结构。字符串字段先原样保留，再在 [`PackageManifest::from_raw`]
/// 中校验并转换，避免 PathBuf/枚举在 TOML 中的边界行为与不精确错误。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    manifest_version: u32,
    id: PackageId,
    name: String,
    version: String,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    entrypoint: Option<PackageRelativePath>,
    #[serde(default)]
    scope: PackageScope,
    #[serde(default)]
    dependencies: Vec<PackageDependency>,
    #[serde(default)]
    skills: Vec<ResourceRef>,
    #[serde(default)]
    agents: Vec<ResourceRef>,
    #[serde(default)]
    hooks: Vec<ResourceRef>,
    #[serde(default)]
    mcp: Vec<McpServerDeclaration>,
    #[serde(default)]
    lsp: Vec<ResourceRef>,
    #[serde(default)]
    monitors: Vec<MonitorDeclaration>,
}

impl PackageManifest {
    /// 从 TOML 文本解析并校验 manifest。
    pub fn from_toml_str(text: &str) -> Result<Self, PackageError> {
        let raw: RawManifest =
            toml::from_str(text).map_err(|error| PackageError::ManifestToml(error.to_string()))?;
        let manifest = Self::from_raw(raw)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// 序列化为 TOML 文本（稳定排序，便于内容寻址归档）。
    pub fn to_toml_string(&self) -> Result<String, PackageError> {
        let value = toml::Value::try_from(self)
            .map_err(|error| PackageError::ManifestToml(error.to_string()))?;
        toml::to_string_pretty(&value)
            .map_err(|error| PackageError::ManifestToml(error.to_string()))
    }

    fn from_raw(raw: RawManifest) -> Result<Self, PackageError> {
        let version = parse_version(&raw.version)?;
        Ok(Self {
            manifest_version: raw.manifest_version,
            id: raw.id,
            name: raw.name,
            version,
            license: raw.license,
            description: raw.description,
            entrypoint: raw.entrypoint,
            scope: raw.scope,
            dependencies: raw.dependencies,
            skills: raw.skills,
            agents: raw.agents,
            hooks: raw.hooks,
            mcp: raw.mcp,
            lsp: raw.lsp,
            monitors: raw.monitors,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::{MonitorDeclaration, MonitorDriverEntry, MonitorLifecycle};
    use agent_domain::MonitorId;

    fn sample_toml() -> &'static str {
        r#"
manifest_version = 1
id = "acme.toolkit"
name = "ACME Toolkit"
version = "1.2.0"
license = "MIT"

[scope]
kind = "global"

[[skills]]
from = "path"
path = "skills/search"

[[hooks]]
from = "inline"
manifest = { id = "notify", trigger = "run_started", scope = { kind = "global" }, handler = { kind = "command", program = "/bin/true" } }

[[mcp]]
name = "fs"
auto_start = true
[mcp.transport]
kind = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem"]

[[lsp]]
from = "path"
path = "lsp/rust.toml"

[[monitors]]
monitor_id = "watch-build"
source = "file_change"
lifecycle = "task_manager"
required_capability = ["fs"]
[monitors.driver]
kind = "monitor_service.evaluate"
[monitors.config]
kind = "file_change"
paths = ["target/debug/app"]
"#
    }

    #[test]
    fn parses_and_validates_full_manifest() {
        let manifest = PackageManifest::from_toml_str(sample_toml()).expect("parse");
        assert_eq!(manifest.id.as_str(), "acme.toolkit");
        assert_eq!(manifest.version, Version::new(1, 2, 0));
        assert_eq!(manifest.resource_count(), 5);
        assert!(manifest.skills[0].path().is_some());
        assert!(manifest.mcp[0].is_stdio());
        assert_eq!(manifest.scope, PackageScope::Global);
        manifest.validate().expect("validate");
    }

    #[test]
    fn rejects_unsupported_schema_version() {
        let toml = sample_toml().replace("manifest_version = 1", "manifest_version = 7");
        let err = PackageManifest::from_toml_str(&toml).unwrap_err();
        assert!(matches!(err, PackageError::UnsupportedSchemaVersion { .. }));
    }

    #[test]
    fn rejects_skill_inline_entry() {
        let toml = r#"
manifest_version = 1
id = "acme.bad"
name = "Bad"
version = "0.1.0"
[[skills]]
from = "inline"
manifest = { id = "x" }
"#;
        let err = PackageManifest::from_toml_str(toml).unwrap_err();
        assert!(err.to_string().contains("skill entries must use a path"));
    }

    #[test]
    fn rejects_dotted_mcp_name_and_empty_command() {
        let toml = r#"
manifest_version = 1
id = "acme.bad"
name = "Bad"
version = "0.1.0"
[[mcp]]
name = "a.b"
[mcp.transport]
kind = "stdio"
command = "x"
"#;
        let err = PackageManifest::from_toml_str(toml).unwrap_err();
        assert!(err.to_string().contains("namespace separator"));

        let toml = r#"
manifest_version = 1
id = "acme.bad"
name = "Bad"
version = "0.1.0"
[[mcp]]
name = "fs"
[mcp.transport]
kind = "stdio"
command = " "
"#;
        let err = PackageManifest::from_toml_str(toml).unwrap_err();
        assert!(err.to_string().contains("non-empty command"));
    }

    #[test]
    fn toml_round_trip_preserves_manifest() {
        let manifest = PackageManifest::from_toml_str(sample_toml()).expect("parse");
        let text = manifest.to_toml_string().expect("serialize");
        let back = PackageManifest::from_toml_str(&text).expect("parse back");
        assert_eq!(manifest, back);
    }

    #[test]
    fn mcp_env_and_headers_are_secret_refs_without_plaintext() {
        const TOKEN: &str = "sk-live-token-0123456789";
        let manifest = PackageManifest {
            manifest_version: crate::PACKAGE_MANIFEST_VERSION,
            id: PackageId::new("acme.secrets").unwrap(),
            name: "Secrets".into(),
            version: Version::new(1, 0, 0),
            license: None,
            description: None,
            entrypoint: None,
            scope: PackageScope::Global,
            dependencies: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            hooks: Vec::new(),
            mcp: vec![
                McpServerDeclaration {
                    name: "fs".into(),
                    transport: McpTransportSpec::Stdio {
                        command: "mcp-server".into(),
                        args: Vec::new(),
                        env: BTreeMap::from([(
                            "API_KEY".into(),
                            SecretRef::new("pawork.mcp.fs", "cred-1"),
                        )]),
                    },
                    auto_start: false,
                },
                McpServerDeclaration {
                    name: "remote".into(),
                    transport: McpTransportSpec::Http {
                        url: "https://example.com/mcp".into(),
                        headers: BTreeMap::from([(
                            "X-Api-Key".into(),
                            SecretRef::new("pawork.mcp.remote", "cred-2"),
                        )]),
                    },
                    auto_start: false,
                },
            ],
            lsp: Vec::new(),
            monitors: Vec::new(),
        };

        // 归档序列化只含定位符，不含 token。
        let toml = manifest.to_toml_string().expect("serialize");
        assert!(toml.contains("pawork.mcp.fs"));
        assert!(toml.contains("cred-1"));
        assert!(!toml.contains(TOKEN));

        // roundtrip 保真（只含定位符）。
        let back = PackageManifest::from_toml_str(&toml).expect("parse back");
        assert_eq!(back, manifest);

        // Debug 输出同样不含 token（值本身是定位符，构造上无法携带明文）。
        assert!(!format!("{manifest:?}").contains(TOKEN));
        assert!(format!("{manifest:?}").contains("cred-1"));
    }

    #[test]
    fn mcp_env_and_headers_secret_refs_are_validated_on_real_fields() {
        // env / headers 是 SecretRef 的真实承载字段：定位符必须通过
        // 非空 / 长度 / 字符集 / 明显 token 校验，经 manifest validate 强制。
        let bad_env = PackageManifest {
            manifest_version: crate::PACKAGE_MANIFEST_VERSION,
            id: PackageId::new("acme.secrets").unwrap(),
            name: "Secrets".into(),
            version: Version::new(1, 0, 0),
            license: None,
            description: None,
            entrypoint: None,
            scope: PackageScope::Global,
            dependencies: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            hooks: Vec::new(),
            mcp: vec![McpServerDeclaration {
                name: "fs".into(),
                transport: McpTransportSpec::Stdio {
                    command: "mcp-server".into(),
                    args: Vec::new(),
                    env: BTreeMap::from([(
                        "API_KEY".into(),
                        SecretRef::new("sk-live-token", "cred-1"),
                    )]),
                },
                auto_start: false,
            }],
            lsp: Vec::new(),
            monitors: Vec::new(),
        };
        let err = bad_env.validate().unwrap_err();
        assert!(
            err.to_string().contains("mcp.fs.env.API_KEY.service"),
            "{err}"
        );
        assert!(err.to_string().contains("secret ref locator"), "{err}");

        let bad_headers = PackageManifest {
            manifest_version: crate::PACKAGE_MANIFEST_VERSION,
            id: PackageId::new("acme.secrets").unwrap(),
            name: "Secrets".into(),
            version: Version::new(1, 0, 0),
            license: None,
            description: None,
            entrypoint: None,
            scope: PackageScope::Global,
            dependencies: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            hooks: Vec::new(),
            mcp: vec![McpServerDeclaration {
                name: "remote".into(),
                transport: McpTransportSpec::Http {
                    url: "https://example.com/mcp".into(),
                    headers: BTreeMap::from([(
                        "X-Api-Key".into(),
                        SecretRef::new("ghp_abcdefghijklmnopqrstuvwxyz", "cred-2"),
                    )]),
                },
                auto_start: false,
            }],
            lsp: Vec::new(),
            monitors: Vec::new(),
        };
        let err = bad_headers.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("mcp.remote.headers.X-Api-Key.service"),
            "{err}"
        );
    }

    #[test]
    fn mcp_secret_refs_are_validated_via_toml_parse() {
        // TOML 路径同样强制校验：env 值携带明显 token 形态时解析失败。
        let toml = r#"
manifest_version = 1
id = "acme.bad"
name = "Bad"
version = "0.1.0"
[[mcp]]
name = "fs"
[mcp.transport]
kind = "stdio"
command = "npx"
[mcp.transport.env]
API_KEY = { service = "sk-live-token", account = "cred-1" }
"#;
        let err = PackageManifest::from_toml_str(toml).unwrap_err();
        assert!(err.to_string().contains("secret ref locator"), "{err}");

        // 合法定位符正常解析。
        let toml = r#"
manifest_version = 1
id = "acme.ok"
name = "Ok"
version = "0.1.0"
[[mcp]]
name = "fs"
[mcp.transport]
kind = "stdio"
command = "npx"
[mcp.transport.env]
API_KEY = { service = "pawork.mcp.fs", account = "cred-1" }
"#;
        PackageManifest::from_toml_str(toml).expect("valid secret refs parse");
    }

    #[test]
    fn empty_manifest_reports_empty() {
        let toml = r#"
manifest_version = 1
id = "acme.empty"
name = "Empty"
version = "0.1.0"
"#;
        let manifest = PackageManifest::from_toml_str(toml).expect("parse");
        assert!(manifest.is_empty());
    }

    #[test]
    fn monitor_declaration_constructs() {
        let _ = MonitorDeclaration::new(
            MonitorId::new("m"),
            MonitorDriverEntry::new("monitor_service.evaluate"),
            MonitorLifecycle::TaskManager,
        );
    }

    /// 构造只含一个 MCP server 声明的 manifest（有界预算测试夹具）。
    fn mcp_manifest(server: McpServerDeclaration) -> PackageManifest {
        PackageManifest {
            manifest_version: crate::PACKAGE_MANIFEST_VERSION,
            id: PackageId::new("acme.budget").unwrap(),
            name: "Budget".into(),
            version: Version::new(1, 0, 0),
            license: None,
            description: None,
            entrypoint: None,
            scope: PackageScope::Global,
            dependencies: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            hooks: Vec::new(),
            mcp: vec![server],
            lsp: Vec::new(),
            monitors: Vec::new(),
        }
    }

    fn stdio_server(name: &str, command: &str, args: Vec<String>) -> McpServerDeclaration {
        McpServerDeclaration {
            name: name.into(),
            transport: McpTransportSpec::Stdio {
                command: command.into(),
                args,
                env: Default::default(),
            },
            auto_start: false,
        }
    }

    #[test]
    fn rejects_overlong_or_bad_charset_mcp_name() {
        let long = "a".repeat(MAX_MCP_SERVER_NAME_LEN + 1);
        let err = mcp_manifest(stdio_server(&long, "npx", Vec::new()))
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("exceeds maximum length"), "{err}");

        let err = mcp_manifest(stdio_server("bad name", "npx", Vec::new()))
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("ASCII alphanumerics"), "{err}");
    }

    #[test]
    fn rejects_stdio_declarations_over_budget() {
        let err = mcp_manifest(stdio_server(
            "fs",
            &"x".repeat(MAX_MCP_COMMAND_LEN + 1),
            Vec::new(),
        ))
        .validate()
        .unwrap_err();
        assert!(err.to_string().contains("mcp.fs.command"), "{err}");

        let err = mcp_manifest(stdio_server("fs", "npx\n--evil", Vec::new()))
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("control characters"), "{err}");

        let args = vec!["arg".to_string(); MAX_MCP_ARG_COUNT + 1];
        let err = mcp_manifest(stdio_server("fs", "npx", args))
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("arguments"), "{err}");

        let err = mcp_manifest(stdio_server(
            "fs",
            "npx",
            vec!["y".repeat(MAX_MCP_ARG_LEN + 1)],
        ))
        .validate()
        .unwrap_err();
        assert!(err.to_string().contains("mcp.fs.args"), "{err}");
    }

    #[test]
    fn rejects_bad_env_and_header_keys() {
        for bad_key in ["BAD-KEY", "1ABC", "KEY WITH SPACE"] {
            let env = BTreeMap::from([(
                bad_key.to_string(),
                SecretRef::new("pawork.mcp.fs", "cred-1"),
            )]);
            let server = McpServerDeclaration {
                name: "fs".into(),
                transport: McpTransportSpec::Stdio {
                    command: "npx".into(),
                    args: Vec::new(),
                    env,
                },
                auto_start: false,
            };
            let err = mcp_manifest(server).validate().unwrap_err();
            assert!(err.to_string().contains("env key"), "{bad_key}: {err}");
        }

        let headers = BTreeMap::from([(
            "X Bad Key".to_string(),
            SecretRef::new("pawork.mcp.remote", "cred-2"),
        )]);
        let server = McpServerDeclaration {
            name: "remote".into(),
            transport: McpTransportSpec::Http {
                url: "https://example.com/mcp".into(),
                headers,
            },
            auto_start: false,
        };
        let err = mcp_manifest(server).validate().unwrap_err();
        assert!(err.to_string().contains("header key"), "{err}");
    }

    #[test]
    fn rejects_overlong_http_url() {
        let url = format!("https://example.com/{}", "p".repeat(MAX_MCP_URL_LEN));
        let server = McpServerDeclaration {
            name: "remote".into(),
            transport: McpTransportSpec::Http {
                url,
                headers: Default::default(),
            },
            auto_start: false,
        };
        let err = mcp_manifest(server).validate().unwrap_err();
        assert!(err.to_string().contains("mcp.remote.url"), "{err}");
    }

    #[test]
    fn rejects_too_many_mcp_servers() {
        let mut manifest = mcp_manifest(stdio_server("s00", "npx", Vec::new()));
        for index in 1..=MAX_MCP_SERVERS_PER_PACKAGE {
            manifest
                .mcp
                .push(stdio_server(&format!("s{index:02}"), "npx", Vec::new()));
        }
        let err = manifest.validate().unwrap_err();
        assert!(err.to_string().contains("mcp servers"), "{err}");
    }
}
