//! 语言服务描述符与内置预设。

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 传输方式。首版仅 stdio；socket 为未来显式配置预留枚举位。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LspTransport {
    /// JSON-RPC over 进程 stdio（默认）。
    #[default]
    Stdio,
    /// 预留：JSON-RPC over socket（如 clangd `--socket`）。当前未实现 spawn，仅占位。
    Socket,
}

/// LSP `workspace/didChangeWorkspaceFolders` 的工作区文件夹。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceFolder {
    /// file:// URI。
    pub uri: String,
    /// 显示名。
    pub name: String,
}

/// 语言服务描述符：纯领域类型，不绑定具体实现。
///
/// 描述「如何启动 + 如何同步 + 如何重启」一个语言服务；由
/// [`crate::transport::ServerSpawner`] 在生产侧桥接到 sandbox/process spawn。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageServerDescriptor {
    /// 逻辑 id（如 `"rust-analyzer"`），用于多服务注册与诊断归属。
    pub id: String,
    /// 可执行命令（如 `"rust-analyzer"`）。由生产 spawner 解析为绝对路径。
    pub command: String,
    /// 命令行参数。
    pub args: Vec<String>,
    /// 传输方式。
    #[serde(default)]
    pub transport: LspTransport,
    /// 额外环境变量。
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// 规范语言 id（如 `"rust"`、`"python"`），用于 `textDocumentItem.languageId`。
    pub language: String,
    /// 该服务覆盖的文件扩展名（不含点，如 `["rs"]`）。
    #[serde(default)]
    pub extensions: Vec<String>,
    /// `initialize` 的 `initializationOptions`。
    #[serde(default)]
    pub initialization_options: Option<Value>,
    /// `workspace/didChangeConfiguration` 的 settings。
    #[serde(default)]
    pub settings: Option<Value>,
    /// `initialize` 的工作区文件夹。
    #[serde(default)]
    pub workspace_folder: Option<WorkspaceFolder>,
    /// 等待 `initialize` 响应的超时。
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout: Duration,
    /// 等待 `shutdown` 响应的超时。
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout: Duration,
    /// 崩溃后是否自动重启。
    #[serde(default = "default_restart_on_crash")]
    pub restart_on_crash: bool,
    /// 最大连续重启次数；超过则进入 Failed。
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
}

fn default_startup_timeout() -> Duration {
    Duration::from_secs(30)
}
fn default_shutdown_timeout() -> Duration {
    Duration::from_secs(10)
}
fn default_restart_on_crash() -> bool {
    true
}
fn default_max_restarts() -> u32 {
    5
}

impl LanguageServerDescriptor {
    /// 新建描述符并填充合理默认值。
    pub fn new(
        id: impl Into<String>,
        command: impl Into<String>,
        language: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            command: command.into(),
            args: Vec::new(),
            transport: LspTransport::Stdio,
            env: Vec::new(),
            language: language.into(),
            extensions: Vec::new(),
            initialization_options: None,
            settings: None,
            workspace_folder: None,
            startup_timeout: default_startup_timeout(),
            shutdown_timeout: default_shutdown_timeout(),
            restart_on_crash: default_restart_on_crash(),
            max_restarts: default_max_restarts(),
        }
    }

    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_extensions<I, S>(mut self, exts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.extensions = exts.into_iter().map(Into::into).collect();
        self
    }

    /// 是否负责给定扩展名（小写比较，忽略前导点）。
    pub fn handles_extension(&self, ext: &str) -> bool {
        let norm = ext.trim_start_matches('.').to_ascii_lowercase();
        self.extensions
            .iter()
            .any(|e| e.eq_ignore_ascii_case(&norm))
    }

    /// 是否负责给定语言 id（小写比较）。
    pub fn handles_language(&self, language: &str) -> bool {
        self.language.eq_ignore_ascii_case(language)
    }
}

/// 把 resource-loader 的工作区作用域 DTO 转成运行时 descriptor。
///
/// workspace root 由可信宿主传入，配置文件不能自行指定 cwd 或绝对权限根。
pub fn from_resource(
    resource: &resource_loader::LanguageServerResource,
    workspace_root: &std::path::Path,
) -> Result<LanguageServerDescriptor, String> {
    let workspace_uri = url::Url::from_directory_path(workspace_root)
        .map_err(|()| "workspace root cannot be represented as a file URI".to_string())?;
    let mut descriptor = LanguageServerDescriptor::new(
        resource.id.clone(),
        resource.command.clone(),
        resource.language.clone(),
    )
    .with_args(resource.args.clone())
    .with_extensions(resource.extensions.clone());
    descriptor.env = resource.env.clone();
    descriptor.initialization_options = resource.initialization_options.clone();
    descriptor.settings = resource.settings.clone();
    descriptor.restart_on_crash = resource.restart_on_crash.unwrap_or(true);
    descriptor.max_restarts = resource.max_restarts.unwrap_or_else(default_max_restarts);
    descriptor.workspace_folder = Some(WorkspaceFolder {
        uri: workspace_uri.to_string(),
        name: workspace_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_string(),
    });
    Ok(descriptor)
}

/// 内置预设：rust-analyzer / pyright / typescript-language-server / gopls / clangd。
///
/// 仅给出启动命令与语言 / 扩展映射；具体路径解析与沙箱策略由生产 spawner 决定。
pub fn builtin_presets() -> HashMap<String, LanguageServerDescriptor> {
    let mut map = HashMap::new();
    for d in [
        rust_analyzer(),
        pyright(),
        typescript_language_server(),
        gopls(),
        clangd(),
    ] {
        map.insert(d.id.clone(), d);
    }
    map
}

pub fn rust_analyzer() -> LanguageServerDescriptor {
    LanguageServerDescriptor::new("rust-analyzer", "rust-analyzer", "rust").with_extensions(["rs"])
}

pub fn pyright() -> LanguageServerDescriptor {
    LanguageServerDescriptor::new("pyright", "pyright-langserver", "python")
        .with_extensions(["py", "pyi"])
        .with_args(["--stdio"])
}

pub fn typescript_language_server() -> LanguageServerDescriptor {
    LanguageServerDescriptor::new(
        "typescript-language-server",
        "typescript-language-server",
        "typescript",
    )
    .with_extensions(["ts", "tsx", "js", "jsx", "mts", "cts"])
    .with_args(["--stdio"])
}

pub fn gopls() -> LanguageServerDescriptor {
    LanguageServerDescriptor::new("gopls", "gopls", "go")
        .with_extensions(["go"])
        .with_args(["serve"])
}

pub fn clangd() -> LanguageServerDescriptor {
    LanguageServerDescriptor::new("clangd", "clangd", "c")
        .with_extensions(["c", "cpp", "cc", "cxx", "h", "hpp"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_cover_core_languages() {
        let presets = builtin_presets();
        assert!(presets.contains_key("rust-analyzer"));
        assert!(presets.contains_key("pyright"));
        assert!(presets.contains_key("typescript-language-server"));
        assert!(presets.contains_key("gopls"));
        assert!(presets.contains_key("clangd"));
    }

    #[test]
    fn handles_extension_matches_case_insensitive() {
        let d = rust_analyzer();
        assert!(d.handles_extension("rs"));
        assert!(d.handles_extension("RS"));
        assert!(d.handles_extension(".rs"));
        assert!(!d.handles_extension("py"));
    }

    #[test]
    fn handles_language_matches_case_insensitive() {
        let d = pyright();
        assert!(d.handles_language("python"));
        assert!(d.handles_language("Python"));
        assert!(!d.handles_language("rust"));
    }

    #[test]
    fn descriptor_round_trips_serde() {
        let d = gopls();
        let json = serde_json::to_string(&d).unwrap();
        let back: LanguageServerDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, d.id);
        assert_eq!(back.language, d.language);
        assert_eq!(back.transport, LspTransport::Stdio);
    }

    #[test]
    fn resource_descriptor_uses_host_workspace_root() {
        let resource = resource_loader::LanguageServerResource {
            id: "custom".into(),
            command: "custom-ls".into(),
            args: vec!["--stdio".into()],
            language: "custom".into(),
            extensions: vec!["cus".into()],
            env: vec![("SAFE_OPTION".into(), "1".into())],
            initialization_options: Some(serde_json::json!({"x": true})),
            settings: Some(serde_json::json!({"lint": true})),
            restart_on_crash: Some(false),
            max_restarts: Some(0),
            provenance: None,
        };
        let root = std::env::temp_dir();
        let descriptor = from_resource(&resource, &root).expect("descriptor");
        assert_eq!(descriptor.command, "custom-ls");
        assert_eq!(descriptor.max_restarts, 0);
        assert!(!descriptor.restart_on_crash);
        assert!(descriptor
            .workspace_folder
            .as_ref()
            .expect("folder")
            .uri
            .starts_with("file:"));
    }
}
