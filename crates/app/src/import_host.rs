//! S9 波 C：compat 配置导入与会话 import/export 宿主包装。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use pawork_domain::SessionId;
use pawork_storage::session::{
    CompatImportReport as SessionCompatReport, ExternalSource as SessionExternalSource,
    PiImportReport, SessionExport,
};
use pawork_workspace::config::{workspace_config_path, PaworkConfig};
use pawork_workspace::import::mcp::McpServerConfig as CompatMcpServer;
use pawork_workspace::import::{
    CompatPayload, ExternalSource, LocalSessionFile, LocalSessionSource,
};
use serde::Serialize;

use crate::{AppCore, AppError};

/// `pawork import <tool>` 的来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompatTool(pub ExternalSource);

impl CompatTool {
    pub fn parse(name: &str) -> Result<ExternalSource, AppError> {
        match name.trim().to_ascii_lowercase().as_str() {
            "claude" => Ok(ExternalSource::Claude),
            "codex" => Ok(ExternalSource::Codex),
            "grok" => Ok(ExternalSource::Grok),
            "cursor" => Ok(ExternalSource::Cursor),
            "pi" => Ok(ExternalSource::Pi),
            other => Err(AppError::Import(format!(
                "unknown import tool '{other}' (claude|codex|grok|cursor|pi)"
            ))),
        }
    }
}

/// 配置导入预览（确认前）。
#[derive(Clone, Debug, Serialize)]
pub struct CompatImportPreview {
    pub tool: String,
    pub preview: String,
    pub fingerprint: String,
    pub items: Vec<CompatImportItemView>,
    pub source_files: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CompatImportItemView {
    pub id: String,
    pub category: String,
    pub status: String,
    pub relative_path: String,
    pub requires_review: bool,
}

/// 配置导入结果。
#[derive(Clone, Debug, Serialize)]
pub struct CompatImportReport {
    pub tool: String,
    pub preview: String,
    pub fingerprint: String,
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
    pub plan_path: PathBuf,
    pub sources_unchanged: bool,
}

/// 会话导入格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionImportFormat {
    Export,
    Compat,
    Pi,
}

impl SessionImportFormat {
    pub fn parse(name: &str) -> Result<Self, AppError> {
        match name.trim().to_ascii_lowercase().as_str() {
            "export" => Ok(Self::Export),
            "compat" => Ok(Self::Compat),
            "pi" => Ok(Self::Pi),
            other => Err(AppError::Import(format!(
                "unknown session import format '{other}' (export|compat|pi)"
            ))),
        }
    }
}

#[derive(Clone, Debug)]
pub enum SessionImportOutcome {
    Export { session_id: String },
    Compat(SessionCompatReport),
    Pi(PiImportReport),
}

#[derive(Clone, Debug)]
pub(crate) struct FileSnapshot {
    pub(crate) path: PathBuf,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) bytes: Vec<u8>,
}

impl AppCore {
    /// 只读发现本机会话文件(`sessions import --from` 消费;home_root 主要用于测试)。
    pub fn scan_local_sessions(
        &self,
        source: LocalSessionSource,
        home_root: Option<&Path>,
    ) -> Result<Vec<LocalSessionFile>, AppError> {
        self.imports.scan_local_sessions(source, home_root)
    }

    pub fn preview_compat_import(
        &self,
        tool: ExternalSource,
        global_root: Option<&Path>,
    ) -> Result<CompatImportPreview, AppError> {
        self.imports.preview_compat_import(self, tool, global_root)
    }

    pub fn apply_compat_import(
        &self,
        tool: ExternalSource,
        global_root: Option<&Path>,
    ) -> Result<CompatImportReport, AppError> {
        self.imports.apply_compat_import(self, tool, global_root)
    }

    pub async fn export_session_doc(
        &self,
        spec: Option<&str>,
    ) -> Result<(SessionId, SessionExport), AppError> {
        self.imports.export_session_doc(self, spec).await
    }

    pub async fn import_session_file(
        &self,
        path: &Path,
        format: SessionImportFormat,
        compat_source: Option<SessionExternalSource>,
    ) -> Result<SessionImportOutcome, AppError> {
        self.imports
            .import_session_file(self, path, format, compat_source)
            .await
    }
}

pub fn parse_session_source(name: &str) -> Result<SessionExternalSource, AppError> {
    match name.trim().to_ascii_lowercase().as_str() {
        "claude" => Ok(SessionExternalSource::Claude),
        "codex" => Ok(SessionExternalSource::Codex),
        "grok" => Ok(SessionExternalSource::Grok),
        "cursor" => Ok(SessionExternalSource::Cursor),
        other => Err(AppError::Import(format!(
            "unknown session source '{other}' (claude|codex|grok|cursor)"
        ))),
    }
}

pub(crate) fn apply_payload(workspace: &Path, payload: &CompatPayload) -> Result<(), AppError> {
    match payload {
        CompatPayload::Instructions { body, .. } => {
            append_instructions(workspace, body)?;
        }
        CompatPayload::Skill { manifest, body } => {
            let dir = workspace.join(".pawork/skills").join(&manifest.id);
            std::fs::create_dir_all(&dir)?;
            let manifest_text = toml::to_string_pretty(manifest)
                .map_err(|error| AppError::Import(format!("serialize skill manifest: {error}")))?;
            std::fs::write(dir.join("manifest.toml"), manifest_text)?;
            std::fs::write(dir.join("SKILL.md"), body)?;
        }
        CompatPayload::McpServer { name, server, .. } => {
            merge_mcp_server(&workspace_config_path(workspace), name, server)?;
        }
        CompatPayload::AgentProfile { profile } => {
            if profile.prompt.system.trim().is_empty() {
                return Ok(());
            }
            let dir = workspace.join(".pawork/profiles");
            std::fs::create_dir_all(&dir)?;
            let text = profile_toml(profile)?;
            std::fs::write(dir.join(format!("{}.toml", profile.name)), text)?;
        }
        CompatPayload::UserHook { .. } | CompatPayload::PermissionRule { .. } => {}
    }
    Ok(())
}

fn append_instructions(workspace: &Path, body: &str) -> Result<(), AppError> {
    let path = workspace.join(".pawork/instructions.md");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut existing = if path.is_file() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    if !existing.is_empty() {
        existing.push('\n');
    }
    existing.push_str(body);
    if !existing.ends_with('\n') {
        existing.push('\n');
    }
    std::fs::write(path, existing)?;
    Ok(())
}

fn merge_mcp_server(
    config_path: &Path,
    name: &str,
    server: &CompatMcpServer,
) -> Result<(), AppError> {
    let mut config = if config_path.is_file() {
        let text = std::fs::read_to_string(config_path)?;
        toml::from_str::<PaworkConfig>(&text).map_err(|error| {
            AppError::Import(format!("parse {}: {error}", config_path.display()))
        })?
    } else {
        PaworkConfig::default()
    };
    let json = serde_json::to_value(server)
        .map_err(|error| AppError::Import(format!("serialize MCP server: {error}")))?;
    let mcp = config
        .extra
        .entry("mcp".into())
        .or_insert_with(|| serde_json::json!({ "servers": {} }));
    if !mcp.is_object() {
        *mcp = serde_json::json!({ "servers": {} });
    }
    let servers = mcp
        .as_object_mut()
        .expect("mcp object")
        .entry("servers")
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        *servers = serde_json::json!({});
    }
    servers
        .as_object_mut()
        .expect("servers object")
        .insert(name.to_string(), json);
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(&config)
        .map_err(|error| AppError::Import(format!("serialize config: {error}")))?;
    std::fs::write(config_path, text)?;
    Ok(())
}

fn profile_toml(profile: &pawork_domain::AgentProfileV2) -> Result<String, AppError> {
    let mut table = toml::Table::new();
    table.insert("schema".into(), toml::Value::String("v2".into()));
    table.insert("name".into(), toml::Value::String(profile.name.clone()));
    let mut prompt = toml::Table::new();
    prompt.insert(
        "system".into(),
        toml::Value::String(profile.prompt.system.clone()),
    );
    if let Some(extra) = &profile.prompt.instructions {
        prompt.insert("instructions".into(), toml::Value::String(extra.clone()));
    }
    table.insert("prompt".into(), toml::Value::Table(prompt));
    if profile.model.provider.is_some() || profile.model.name.is_some() {
        let mut model = toml::Table::new();
        if let Some(provider) = &profile.model.provider {
            model.insert("provider".into(), toml::Value::String(provider.clone()));
        }
        if let Some(name) = &profile.model.name {
            model.insert("name".into(), toml::Value::String(name.clone()));
        }
        table.insert("model".into(), toml::Value::Table(model));
    }
    toml::to_string_pretty(&table)
        .map_err(|error| AppError::Import(format!("serialize profile: {error}")))
}

pub(crate) fn snapshot_files(paths: &[PathBuf]) -> Result<Vec<FileSnapshot>, AppError> {
    let mut snapshots = Vec::new();
    for path in paths {
        let meta = std::fs::metadata(path)?;
        snapshots.push(FileSnapshot {
            path: path.clone(),
            modified: meta.modified().ok(),
            bytes: std::fs::read(path)?,
        });
    }
    Ok(snapshots)
}

pub(crate) fn snapshots_match(snapshots: &[FileSnapshot]) -> Result<bool, AppError> {
    for snapshot in snapshots {
        let meta = std::fs::metadata(&snapshot.path)?;
        let bytes = std::fs::read(&snapshot.path)?;
        if bytes != snapshot.bytes || meta.modified().ok() != snapshot.modified {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn home_dir() -> Result<PathBuf, AppError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Import("HOME is not set".into()))
}
