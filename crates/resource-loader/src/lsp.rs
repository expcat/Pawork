//! 工作区作用域的 Language Server descriptor 资源。

use std::path::{Path, PathBuf};

use config_service::ConfigTier;
use serde::{Deserialize, Serialize};

use crate::{
    io::{read_utf8_bounded_within, sorted_children_within, workspace_relative_key},
    ResourceDiagnosticEntry, ResourceDiagnosticStatus, ResourceDiagnostics, ResourceIssue,
    ResourceKind, ResourceLimits, ResourceOrigin, ResourceProvenance,
};

/// 与 lsp-runtime 中性对接的配置 DTO；resource-loader 不依赖运行时 crate。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageServerResource {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub language: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub initialization_options: Option<serde_json::Value>,
    #[serde(default)]
    pub settings: Option<serde_json::Value>,
    #[serde(default)]
    pub restart_on_crash: Option<bool>,
    #[serde(default)]
    pub max_restarts: Option<u32>,
    #[serde(skip)]
    pub provenance: Option<ResourceProvenance>,
}

pub(crate) fn load_language_servers(
    workspace_roots: &[PathBuf],
    workspace_resource_dir: &str,
    limits: ResourceLimits,
) -> (Vec<LanguageServerResource>, ResourceDiagnostics) {
    let mut resources = Vec::new();
    let mut diagnostics = ResourceDiagnostics::default();
    for (root_index, root) in workspace_roots.iter().enumerate() {
        load_directory(
            &root.join(workspace_resource_dir).join("lsp"),
            root,
            root_index,
            limits,
            &mut resources,
            &mut diagnostics,
        );
    }
    resources.sort_by(|left, right| {
        left.id.cmp(&right.id).then_with(|| {
            left.provenance
                .as_ref()
                .map(|item| &item.source_key)
                .cmp(&right.provenance.as_ref().map(|item| &item.source_key))
        })
    });
    diagnostics.sort_deterministically();
    (resources, diagnostics)
}

fn load_directory(
    directory: &Path,
    root: &Path,
    root_index: usize,
    limits: ResourceLimits,
    resources: &mut Vec<LanguageServerResource>,
    diagnostics: &mut ResourceDiagnostics,
) {
    let paths = match sorted_children_within(directory, root, limits.max_resources_per_kind) {
        Ok(paths) => paths,
        Err(error) => {
            diagnostics.issues.push(ResourceIssue::error(
                error.code(),
                format!("language server directory could not be read: {error}"),
            ));
            return;
        }
    };
    for path in paths {
        if path.extension().and_then(|value| value.to_str()) != Some("toml") {
            continue;
        }
        let relative = workspace_relative_key(&path, root);
        let source_key = format!("workspace:{root_index:08}:lsp:{relative}");
        let provenance = ResourceProvenance::new(
            ConfigTier::Workspace,
            source_key.clone(),
            ResourceOrigin::Workspace {
                root_index,
                relative_path: relative,
            },
        );
        let content = match read_utf8_bounded_within(&path, root, limits.max_file_bytes) {
            Ok(content) => content,
            Err(error) => {
                diagnostics.issues.push(
                    ResourceIssue::error(
                        error.code(),
                        format!("language server resource could not be loaded: {error}"),
                    )
                    .for_resource(
                        ResourceKind::LanguageServer,
                        "unknown",
                        source_key,
                    ),
                );
                continue;
            }
        };
        let mut resource: LanguageServerResource = match toml::from_str(&content) {
            Ok(resource) => resource,
            Err(_) => {
                diagnostics.issues.push(
                    ResourceIssue::error(
                        "language_server_invalid",
                        "language server resource is not valid TOML",
                    )
                    .for_resource(
                        ResourceKind::LanguageServer,
                        "unknown",
                        source_key,
                    ),
                );
                continue;
            }
        };
        if resource.id.trim().is_empty()
            || resource.command.trim().is_empty()
            || resource.language.trim().is_empty()
        {
            diagnostics.issues.push(
                ResourceIssue::error(
                    "language_server_invalid",
                    "language server id, command, and language must be non-empty",
                )
                .for_resource(
                    ResourceKind::LanguageServer,
                    resource.id,
                    source_key,
                ),
            );
            continue;
        }
        resource.provenance = Some(provenance.clone());
        diagnostics.entries.push(ResourceDiagnosticEntry {
            kind: ResourceKind::LanguageServer,
            resource_id: resource.id.clone(),
            status: ResourceDiagnosticStatus::Loaded,
            provenance,
        });
        resources.push(resource);
    }
}
