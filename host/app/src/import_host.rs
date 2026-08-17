//! S9 波 C：compat 配置导入与会话 import/export 宿主包装。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use pawork_compat::mcp::McpServerConfig as CompatMcpServer;
use pawork_compat::{
    CompatLoader, CompatPayload, ExternalSource, GlobalSource, ImportCategory, ImportStatus,
};
use pawork_config::{workspace_config_path, ConfigTier, PaworkConfig};
use pawork_domain::SessionId;
use pawork_session::{
    CompatImportReport as SessionCompatReport, ExternalSource as SessionExternalSource,
    PiImportReport, SessionExport,
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
struct FileSnapshot {
    path: PathBuf,
    modified: Option<SystemTime>,
    bytes: Vec<u8>,
}

impl AppCore {
    pub fn preview_compat_import(
        &self,
        tool: ExternalSource,
        global_root: Option<&Path>,
    ) -> Result<CompatImportPreview, AppError> {
        let (plan, source_files, _) = self.scan_compat(tool, global_root)?;
        Ok(CompatImportPreview {
            tool: tool.as_str().to_string(),
            preview: plan.preview(),
            fingerprint: plan.fingerprint.clone(),
            items: plan
                .items
                .iter()
                .map(|item| CompatImportItemView {
                    id: item.id.clone(),
                    category: item.category.as_str().to_string(),
                    status: format!("{:?}", item.status).to_ascii_lowercase(),
                    relative_path: item.source.relative_path.clone(),
                    requires_review: item.requires_review,
                })
                .collect(),
            source_files,
        })
    }

    pub fn apply_compat_import(
        &self,
        tool: ExternalSource,
        global_root: Option<&Path>,
    ) -> Result<CompatImportReport, AppError> {
        let workspace = self
            .workspace_root()
            .ok_or_else(|| AppError::Import("workspace is not attached".into()))?
            .to_path_buf();
        let (plan, source_files, snapshots) = self.scan_compat(tool, global_root)?;
        let preview = plan.preview();
        let fingerprint = plan.fingerprint.clone();
        let output_dir = workspace.join(".pawork/compat").join(tool.as_str());
        let export = CompatLoader::default().export_plan(&plan, &output_dir)?;

        let mut applied = Vec::new();
        let mut skipped = Vec::new();
        for item in &plan.items {
            if item.source.external != tool {
                skipped.push(format!("{} (other source)", item.id));
                continue;
            }
            if item.status != ImportStatus::Imported {
                skipped.push(format!("{} ({:?})", item.id, item.status));
                continue;
            }
            match item.category {
                ImportCategory::UserHook | ImportCategory::PermissionRule => {
                    skipped.push(format!("{} (hooks/permissions not imported)", item.id));
                }
                _ => match item.payload.as_ref() {
                    Some(payload) => {
                        apply_payload(&workspace, payload)?;
                        applied.push(item.id.clone());
                    }
                    None => skipped.push(format!("{} (empty payload)", item.id)),
                },
            }
        }

        let sources_unchanged = snapshots_match(&snapshots)?;
        if !sources_unchanged {
            return Err(AppError::Import(format!(
                "source files changed during import: {}",
                source_files
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        Ok(CompatImportReport {
            tool: tool.as_str().to_string(),
            preview,
            fingerprint,
            applied,
            skipped,
            plan_path: export.plan_path,
            sources_unchanged,
        })
    }

    fn scan_compat(
        &self,
        tool: ExternalSource,
        global_root: Option<&Path>,
    ) -> Result<(pawork_compat::CompatPlan, Vec<PathBuf>, Vec<FileSnapshot>), AppError> {
        let workspace = self
            .workspace_root()
            .ok_or_else(|| AppError::Import("workspace is not attached".into()))?;
        let home = match global_root {
            Some(path) => path.to_path_buf(),
            None => home_dir()?,
        };
        let globals = [GlobalSource::new(tool, home.clone())];
        let loader = CompatLoader::default();
        let mut plan = loader.scan(Some(workspace), &globals, Some(&self.workspace_id))?;
        plan.items.retain(|item| item.source.external == tool);
        plan.sources.retain(|source| *source == tool);
        plan.sort_deterministically();

        let mut source_files = Vec::new();
        for item in &plan.items {
            let root = match item.source.tier {
                ConfigTier::Workspace => workspace.to_path_buf(),
                _ => home.clone(),
            };
            let path = root.join(&item.source.relative_path);
            if path.is_file() {
                source_files.push(path);
            }
        }
        source_files.sort();
        source_files.dedup();
        let snapshots = snapshot_files(&source_files)?;
        Ok((plan, source_files, snapshots))
    }

    pub async fn export_session_doc(
        &self,
        spec: Option<&str>,
    ) -> Result<(SessionId, SessionExport), AppError> {
        let session = match spec {
            Some(spec) => self.resolve_session(spec).await?,
            None => self.resolve_session("latest").await?,
        };
        let export = self.store()?.export_session(&session).await?;
        Ok((session, export))
    }

    pub async fn import_session_file(
        &self,
        path: &Path,
        format: SessionImportFormat,
        compat_source: Option<SessionExternalSource>,
    ) -> Result<SessionImportOutcome, AppError> {
        let store = self.store()?;
        match format {
            SessionImportFormat::Export => {
                let text = tokio::fs::read_to_string(path).await?;
                let export = SessionExport::from_json(&text)?;
                store
                    .import_session(&export, &export.tenant_id, &export.principal_id)
                    .await?;
                Ok(SessionImportOutcome::Export {
                    session_id: export.session_id,
                })
            }
            SessionImportFormat::Compat => {
                let source = compat_source.ok_or_else(|| {
                    AppError::Import(
                        "sessions import --format compat requires --source claude|codex|grok|cursor"
                            .into(),
                    )
                })?;
                let report = store.import_compat_from_file(source, path).await?;
                Ok(SessionImportOutcome::Compat(report))
            }
            SessionImportFormat::Pi => {
                let report = store.import_pi_jsonl(path).await?;
                Ok(SessionImportOutcome::Pi(report))
            }
        }
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

fn apply_payload(workspace: &Path, payload: &CompatPayload) -> Result<(), AppError> {
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
        toml::from_str::<PaworkConfig>(&text)
            .map_err(|error| AppError::Import(format!("parse {}: {error}", config_path.display())))?
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

fn snapshot_files(paths: &[PathBuf]) -> Result<Vec<FileSnapshot>, AppError> {
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

fn snapshots_match(snapshots: &[FileSnapshot]) -> Result<bool, AppError> {
    for snapshot in snapshots {
        let meta = std::fs::metadata(&snapshot.path)?;
        let bytes = std::fs::read(&snapshot.path)?;
        if bytes != snapshot.bytes || meta.modified().ok() != snapshot.modified {
            return Ok(false);
        }
    }
    Ok(true)
}

fn home_dir() -> Result<PathBuf, AppError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Import("HOME is not set".into()))
}
