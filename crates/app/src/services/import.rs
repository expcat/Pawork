//! Import 领域服务：compat 配置导入与会话 import/export。
//!
//! 说明：Import 域本身无可变状态（操作对象为 workspace 与 session store），
//! 因此 R4 阶段 1 的 ImportService 是无状态服务，装配状态仍由 AppCore 持有。

use std::path::Path;

use pawork_domain::SessionId;
use pawork_storage::session::{ExternalSource as SessionExternalSource, SessionExport};
use pawork_workspace::config::ConfigTier;
use pawork_workspace::import::{
    scan_local_sessions as scan_workspace_local_sessions, CompatLoader, ExternalSource,
    GlobalSource, ImportStatus, LocalSessionFile, LocalSessionRoots, LocalSessionSource,
};

use crate::import_host::{
    apply_payload, home_dir, snapshot_files, snapshots_match, CompatImportItemView,
    CompatImportPreview, CompatImportReport, FileSnapshot, SessionImportFormat,
    SessionImportOutcome,
};
use crate::{AppCore, AppError};

pub(crate) struct ImportService;

impl ImportService {
    /// 只读发现本机会话文件(Claude Code / Codex rollout)。
    ///
    /// `home_root` 为 None 时走 workspace 的 directories 解析;Some 用于测试与
    /// 隔离环境。只列路径与大小,不读取内容;解析与 Secret 扫描仍在后续
    /// import_session_file(compat)路径完成。
    pub fn scan_local_sessions(
        &self,
        source: LocalSessionSource,
        home_root: Option<&Path>,
    ) -> Result<Vec<LocalSessionFile>, AppError> {
        let roots = match home_root {
            Some(home) => LocalSessionRoots::from_home(home),
            None => LocalSessionRoots::detect()?,
        };
        scan_workspace_local_sessions(source, &roots).map_err(AppError::from)
    }

    pub fn preview_compat_import(
        &self,
        core: &AppCore,
        tool: ExternalSource,
        global_root: Option<&Path>,
    ) -> Result<CompatImportPreview, AppError> {
        let (plan, source_files, _) = self.scan_compat(core, tool, global_root)?;
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
        core: &AppCore,
        tool: ExternalSource,
        global_root: Option<&Path>,
    ) -> Result<CompatImportReport, AppError> {
        let workspace = core
            .workspace_root()
            .ok_or_else(|| AppError::Import("workspace is not attached".into()))?
            .to_path_buf();
        let (plan, source_files, snapshots) = self.scan_compat(core, tool, global_root)?;
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
                pawork_workspace::import::ImportCategory::UserHook
                | pawork_workspace::import::ImportCategory::PermissionRule => {
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
        core: &AppCore,
        tool: ExternalSource,
        global_root: Option<&Path>,
    ) -> Result<
        (
            pawork_workspace::import::CompatPlan,
            Vec<std::path::PathBuf>,
            Vec<FileSnapshot>,
        ),
        AppError,
    > {
        let workspace = core
            .workspace_root()
            .ok_or_else(|| AppError::Import("workspace is not attached".into()))?;
        let home = match global_root {
            Some(path) => path.to_path_buf(),
            None => home_dir()?,
        };
        let globals = [GlobalSource::new(tool, home.clone())];
        let loader = CompatLoader::default();
        let mut plan = loader.scan(
            Some(workspace),
            &globals,
            Some(&core.extensions.workspace_id),
        )?;
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
        core: &AppCore,
        spec: Option<&str>,
    ) -> Result<(SessionId, SessionExport), AppError> {
        let session = match spec {
            Some(spec) => core.resolve_session(spec).await?,
            None => core.resolve_session("latest").await?,
        };
        let export = core.store()?.export_session(&session).await?;
        Ok((session, export))
    }

    pub async fn import_session_file(
        &self,
        core: &AppCore,
        path: &Path,
        format: SessionImportFormat,
        compat_source: Option<SessionExternalSource>,
    ) -> Result<SessionImportOutcome, AppError> {
        let store = core.store()?;
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

#[cfg(test)]
mod tests {
    use pawork_workspace::import::LocalSessionSource;

    #[tokio::test]
    async fn compat_import_writes_pawork_files_and_keeps_source_mtime() {
        let workspace = tempfile::tempdir().expect("workspace");
        let home = tempfile::tempdir().expect("home");
        std::fs::write(workspace.path().join("CLAUDE.md"), "keep diffs small\n")
            .expect("claude md");
        let (mut core, _store) = crate::testsupport::mock_core(Vec::new()).await;
        core.attach_workspace(workspace.path()).expect("attach");
        let source = workspace.path().join("CLAUDE.md");
        let before = std::fs::metadata(&source).expect("meta").modified().ok();
        let before_bytes = std::fs::read(&source).expect("bytes");
        let report = core
            .apply_compat_import(
                pawork_workspace::import::ExternalSource::Claude,
                Some(home.path()),
            )
            .expect("import");
        assert!(report.sources_unchanged);
        assert_eq!(
            std::fs::metadata(&source).expect("meta").modified().ok(),
            before
        );
        assert_eq!(std::fs::read(&source).expect("bytes"), before_bytes);
        let imported = std::fs::read_to_string(workspace.path().join(".pawork/instructions.md"))
            .expect("instructions");
        assert!(imported.contains("keep diffs small"), "{imported}");
    }

    #[tokio::test]
    async fn scan_local_sessions_lists_files_without_reading_content() {
        let home = tempfile::tempdir().expect("home");
        let claude = home.path().join(".claude/projects/demo");
        std::fs::create_dir_all(&claude).expect("claude dirs");
        std::fs::write(claude.join("session-a.jsonl"), "pending content\n")
            .expect("claude session");
        let codex = home.path().join(".codex/sessions/2026");
        std::fs::create_dir_all(&codex).expect("codex dirs");
        std::fs::write(codex.join("rollout-b.jsonl"), "pending content\n").expect("codex rollout");
        std::fs::write(codex.join("plain.jsonl"), "pending content\n").expect("non-rollout");

        let (core, _store) = crate::testsupport::mock_core(Vec::new()).await;
        let claude_files = core
            .scan_local_sessions(LocalSessionSource::Claude, Some(home.path()))
            .expect("claude scan");
        assert_eq!(claude_files.len(), 1);
        assert!(claude_files[0].path.ends_with("session-a.jsonl"));
        assert_eq!(claude_files[0].size_bytes, "pending content\n".len() as u64);

        let codex_files = core
            .scan_local_sessions(LocalSessionSource::Codex, Some(home.path()))
            .expect("codex scan");
        assert_eq!(codex_files.len(), 1);
        assert!(codex_files[0].path.ends_with("rollout-b.jsonl"));

        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn session_export_v3_round_trip() {
        let (core, _store) = crate::testsupport::mock_core(Vec::new()).await;
        let session = core.create_session("export-me").await.expect("create");
        let (_, export) = core
            .export_session_doc(Some(session.as_str()))
            .await
            .expect("export");
        assert_eq!(
            export.schema_version,
            pawork_storage::session::EXPORT_SCHEMA_VERSION
        );
        let dir = tempfile::tempdir().expect("import store");
        let path = dir.path().join("session.db");
        let (store, _) = pawork_storage::session::SessionStore::open(&path)
            .await
            .expect("store");
        store
            .import_session(&export, &export.tenant_id, &export.principal_id)
            .await
            .expect("import");
        let imported = store.get_session(&session).await.expect("imported session");
        assert_eq!(imported.title, "export-me");
        store.shutdown().await.expect("shutdown import store");
        core.shutdown().await.expect("shutdown");
    }
}
