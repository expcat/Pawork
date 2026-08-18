//! S9 波 C：MCP 装配、资源注入与 `@file` 解析。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pawork_domain::{ContentPart, TextContent};
use pawork_engine::InjectedLayer;
use pawork_auth::{FileBackend, SecretBackend};
use pawork_exec::{
    default_secret_paths, FilesystemPolicy, NativeRestricted, NetworkMode, SandboxPolicy,
};
use pawork_mcp::capabilities::register_server_tools;
use pawork_mcp::config::{McpConfig, StdioSandboxRuntime, TransportSpec};
use pawork_mcp::sandbox::apply_mcp_stdio_env_hygiene;
use pawork_mcp::manager::{ConnectionState, ManagedMcpClient};
use pawork_mcp::{McpError, McpPeer};
use pawork_resources::{
    CurrentPathKind, ResourceInstructionKind, ResourceLoader, ResourceLoaderOptions,
    ResourceOrigin, ResourceRequest, ResourceSelection, WorkspaceRelativePath,
};
use pawork_tools::{
    ApplyPatchTool, EditFileTool, FindFilesTool, ListDirectoryTool, ReadFileTool, RunCommandTool,
    SearchTextTool, ToolRegistry, ToolScheduler, ToolSchedulerConfig, WriteFileTool,
};
use pawork_workspace::{FileIndex, FileIndexError, IndexOptions, WorkspaceService};
use serde::Serialize;

use crate::{AppCore, AppError};

const AT_FILE_MAX_BYTES: usize = 64 * 1024;

/// `pawork mcp list` 的一行。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub transport: String,
    pub state: String,
    pub tools: Vec<String>,
    pub last_error: Option<String>,
}

/// `@token` 解析出的附件（另作 ContentPart，不拼进 user text）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtAttachment {
    pub query: String,
    pub relative_path: String,
    pub content: String,
    pub truncated: bool,
}

pub(crate) struct McpServerSlot {
    pub name: String,
    pub transport: String,
    pub state: String,
    pub last_error: Option<String>,
    pub tools: Vec<String>,
    pub client: Option<Arc<ManagedMcpClient>>,
}

impl AppCore {
    pub(crate) fn new_file_index() -> FileIndex {
        FileIndex::new(IndexOptions::default())
    }

    pub(crate) fn resource_loader_for(workspaces: WorkspaceService) -> ResourceLoader {
        ResourceLoader::new(
            workspaces,
            ResourceLoaderOptions {
                global_resource_dir: Some(crate::default_data_dir()),
                workspace_resource_dir: ".pawork".into(),
                ..ResourceLoaderOptions::default()
            },
        )
    }

    pub(crate) fn install_builtin_tools(
        &mut self,
        workspaces: &WorkspaceService,
    ) -> Result<(), AppError> {
        let registry = builtin_registry(workspaces)?;
        self.replace_registry(registry);
        Ok(())
    }

    fn replace_registry(&mut self, registry: ToolRegistry) {
        self.descriptors = registry.descriptors();
        self.tool_defs = self
            .descriptors
            .iter()
            .map(|descriptor| pawork_domain::ToolDefinition {
                name: descriptor.name.clone(),
                description: descriptor.description.clone(),
                input_schema: descriptor.input_schema.clone(),
            })
            .collect();
        self.scheduler = Arc::new(ToolScheduler::new(
            registry,
            ToolSchedulerConfig {
                max_concurrent: 8,
                approval_mode: self.approval_mode,
                workspace_trusted: self.workspace_trusted,
            },
        ));
    }

    /// 扫描 file-index，并启动已配置的 MCP server（失败不拖垮装配）。
    pub async fn prime_extensions(&mut self) -> Result<(), AppError> {
        if let Ok(Some(workspace)) = self.workspaces.get(&self.workspace_id) {
            if let Err(error) = self.file_index.scan_workspace(&workspace).await {
                tracing::warn!(error = %error, "file-index scan failed");
            }
        }
        self.start_mcp_servers().await;
        Ok(())
    }

    async fn start_mcp_servers(&mut self) {
        let Ok(config) = mcp_config_from_pawork(&self.config) else {
            return;
        };
        if config.servers.is_empty() {
            return;
        }
        let Ok(Some(workspace)) = self.workspaces.get(&self.workspace_id) else {
            return;
        };
        let runtime = stdio_runtime(&workspace.roots, self.workspace_trusted);
        let mut registry = match builtin_registry(&self.workspaces) {
            Ok(registry) => registry,
            Err(error) => {
                tracing::warn!(error = %error, "rebuild builtin registry for MCP failed");
                return;
            }
        };
        let mut slots = Vec::new();
        let mcp_backend = mcp_secret_backend();
        for (name, server) in &config.servers {
            let transport = server.transport.kind().to_string();
            if !server.auto_start {
                slots.push(McpServerSlot {
                    name: name.clone(),
                    transport,
                    state: "configured".into(),
                    last_error: None,
                    tools: Vec::new(),
                    client: None,
                });
                continue;
            }
            if !self.workspace_trusted {
                slots.push(McpServerSlot {
                    name: name.clone(),
                    transport,
                    state: "configured".into(),
                    last_error: Some(
                        "MCP auto-start is disabled in an untrusted workspace".into(),
                    ),
                    tools: Vec::new(),
                    client: None,
                });
                continue;
            }
            match server.build_client(name.clone(), mcp_backend.clone(), runtime.clone()) {
                Ok(client) => {
                    let client = Arc::new(client);
                    let peer: Arc<dyn McpPeer> = client.clone();
                    match register_server_tools(
                        &mut registry,
                        name,
                        peer,
                        server.permissions.clone(),
                        server.trusted && self.workspace_trusted,
                        self.workspace_trusted,
                    )
                    .await
                    {
                        Ok(descriptors) => {
                            let tools = descriptors.into_iter().map(|item| item.name).collect();
                            slots.push(McpServerSlot {
                                name: name.clone(),
                                transport,
                                state: "connected".into(),
                                last_error: None,
                                tools,
                                client: Some(client),
                            });
                        }
                        Err(error) => {
                            slots.push(McpServerSlot {
                                name: name.clone(),
                                transport,
                                state: "failed".into(),
                                last_error: Some(error.to_string()),
                                tools: Vec::new(),
                                client: Some(client),
                            });
                        }
                    }
                }
                Err(error) => {
                    slots.push(McpServerSlot {
                        name: name.clone(),
                        transport,
                        state: "failed".into(),
                        last_error: Some(error.to_string()),
                        tools: Vec::new(),
                        client: None,
                    });
                }
            }
        }
        self.replace_registry(registry);
        self.mcp_servers = slots;
    }

    pub fn mcp_list(&self) -> Vec<McpServerStatus> {
        if self.mcp_servers.is_empty() {
            if let Ok(config) = mcp_config_from_pawork(&self.config) {
                return config
                    .servers
                    .iter()
                    .map(|(name, server)| McpServerStatus {
                        name: name.clone(),
                        transport: server.transport.kind().to_string(),
                        state: if server.auto_start {
                            "configured".into()
                        } else {
                            "configured".into()
                        },
                        tools: Vec::new(),
                        last_error: None,
                    })
                    .collect();
            }
        }
        self.mcp_servers
            .iter()
            .map(|slot| McpServerStatus {
                name: slot.name.clone(),
                transport: slot.transport.clone(),
                state: slot.state.clone(),
                tools: slot.tools.clone(),
                last_error: slot.last_error.clone(),
            })
            .collect()
    }

    pub async fn mcp_test(&mut self, name: Option<&str>) -> Result<Vec<McpServerStatus>, AppError> {
        let config = mcp_config_from_pawork(&self.config)?;
        let names: Vec<String> = match name {
            Some(name) => vec![name.to_string()],
            None => config.servers.keys().cloned().collect(),
        };
        if names.is_empty() {
            return Ok(Vec::new());
        }
        for name in names {
            self.test_one_mcp(&name).await?;
        }
        Ok(self.mcp_list())
    }

    async fn test_one_mcp(&mut self, name: &str) -> Result<(), AppError> {
        let config = mcp_config_from_pawork(&self.config)?;
        let server = config
            .server(name)
            .ok_or_else(|| AppError::Import(format!("unknown MCP server '{name}'")))?
            .clone();
        let workspace = self.workspaces.get(&self.workspace_id)?.ok_or_else(|| {
            AppError::Import("workspace is not attached".into())
        })?;
        if matches!(server.transport, TransportSpec::Stdio { .. }) && !self.workspace_trusted
        {
            return Err(AppError::Mcp(McpError::PermissionDenied(format!(
                "stdio MCP server '{name}' cannot start in an untrusted workspace"
            ))));
        }
        let runtime = stdio_runtime(&workspace.roots, self.workspace_trusted);
        let client = Arc::new(server.build_client(
            name.to_string(),
            mcp_secret_backend(),
            runtime,
        )?);
        client.ping().await?;
        let tools = client
            .list_tools()
            .await?
            .into_iter()
            .map(|tool| pawork_mcp::capabilities::namespaced_name(name, &tool.name))
            .collect();
        let health = client.health().await;
        let state = match health.state {
            ConnectionState::Connected => "connected",
            ConnectionState::Connecting => "connecting",
            ConnectionState::Failed => "failed",
            ConnectionState::Disconnected => "disconnected",
        };
        if let Some(slot) = self.mcp_servers.iter_mut().find(|slot| slot.name == name) {
            slot.state = state.into();
            slot.tools = tools;
            slot.last_error = health.last_error.map(|error| error.to_string());
            slot.client = Some(client);
        } else {
            self.mcp_servers.push(McpServerSlot {
                name: name.to_string(),
                transport: server.transport.kind().to_string(),
                state: state.into(),
                last_error: health.last_error.map(|error| error.to_string()),
                tools,
                client: Some(client),
            });
        }
        Ok(())
    }

    pub(crate) fn load_injected_layers(&self) -> Vec<InjectedLayer> {
        let Some(loader) = &self.resource_loader else {
            return Vec::new();
        };
        let mut selection = ResourceSelection {
            profile: self.config.profile.clone(),
            ..ResourceSelection::default()
        };
        if let Some(root) = self.workspace_roots.first() {
            selection
                .active_skills
                .extend(discover_skill_ids(&root.join(".pawork/skills")));
        }
        selection
            .active_skills
            .extend(discover_skill_ids(&crate::default_data_dir().join("skills")));
        let request = ResourceRequest {
            workspace_id: self.workspace_id.clone(),
            root_index: 0,
            current_path: WorkspaceRelativePath::default(),
            current_path_kind: CurrentPathKind::Directory,
            selection,
        };
        match loader.load(&request) {
            Ok(bundle) => bundle
                .instructions
                .into_iter()
                .filter(|instruction| {
                    self.workspace_trusted
                        || !matches!(
                            instruction.provenance.origin,
                            ResourceOrigin::Workspace { .. }
                        )
                })
                .map(|instruction| InjectedLayer {
                    kind: instruction_kind_name(instruction.kind).into(),
                    resource_id: instruction.resource_id,
                    content: instruction.content,
                })
                .collect(),
            Err(error) => {
                tracing::warn!(error = %error, "resource load failed");
                Vec::new()
            }
        }
    }

    /// 把 `@token` 解析为 file-index 命中，正文作为独立 Text part。
    pub fn expand_at_refs(&self, text: &str) -> Result<Vec<ContentPart>, AppError> {
        let mut parts = vec![ContentPart::Text(TextContent {
            text: text.to_string(),
        })];
        for query in at_tokens(text) {
            if let Some(attachment) = self.resolve_at_query(&query)? {
                let marker = if attachment.truncated {
                    "truncated"
                } else {
                    "complete"
                };
                parts.push(ContentPart::Text(TextContent {
                    text: format!(
                        "[attached file: {path} ({marker})]\n{body}",
                        path = attachment.relative_path,
                        body = attachment.content
                    ),
                }));
            }
        }
        Ok(parts)
    }

    pub fn complete_at(&self, query: &str, limit: usize) -> Result<Vec<String>, AppError> {
        let files = self
            .file_index
            .search(&self.workspace_id, query, limit)
            .or_else(|error| match error {
                FileIndexError::WorkspaceNotIndexed(_) => Ok(Vec::new()),
                other => Err(other),
            })?;
        Ok(files
            .into_iter()
            .map(|file| file.key.relative_path)
            .collect())
    }

    fn resolve_at_query(&self, query: &str) -> Result<Option<AtAttachment>, AppError> {
        let matches = self.complete_at(query, 5)?;
        let Some(relative_path) = matches.first().cloned() else {
            return Ok(None);
        };
        let root = self
            .workspace_roots
            .first()
            .ok_or_else(|| AppError::Import("workspace is not attached".into()))?;
        if Path::new(&relative_path).is_absolute()
            || Path::new(&relative_path)
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(AppError::Import(format!(
                "@file path escaped workspace: {relative_path}"
            )));
        }
        let path = root.join(&relative_path);
        let bytes = std::fs::read(&path)?;
        let truncated = bytes.len() > AT_FILE_MAX_BYTES;
        let slice = if truncated {
            &bytes[..AT_FILE_MAX_BYTES]
        } else {
            &bytes
        };
        let content = String::from_utf8_lossy(slice).into_owned();
        Ok(Some(AtAttachment {
            query: query.to_string(),
            relative_path,
            content,
            truncated,
        }))
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_roots.first().map(PathBuf::as_path)
    }

    pub(crate) async fn shutdown_mcp(&self) {
        for slot in &self.mcp_servers {
            if let Some(client) = &slot.client {
                let _ = client.shutdown().await;
            }
        }
    }
}

fn builtin_registry(workspaces: &WorkspaceService) -> Result<ToolRegistry, AppError> {
    let mut registry = ToolRegistry::new();
    registry.extend([
        Arc::new(ReadFileTool::new(workspaces.clone())) as Arc<dyn pawork_domain::AgentTool>,
        Arc::new(ListDirectoryTool::new(workspaces.clone())) as Arc<dyn pawork_domain::AgentTool>,
        Arc::new(SearchTextTool::new(workspaces.clone())) as Arc<dyn pawork_domain::AgentTool>,
        Arc::new(FindFilesTool::new(workspaces.clone())) as Arc<dyn pawork_domain::AgentTool>,
        Arc::new(WriteFileTool::new(workspaces.clone())) as Arc<dyn pawork_domain::AgentTool>,
        Arc::new(EditFileTool::new(workspaces.clone())) as Arc<dyn pawork_domain::AgentTool>,
        Arc::new(ApplyPatchTool::new(workspaces.clone())) as Arc<dyn pawork_domain::AgentTool>,
        Arc::new(RunCommandTool::new(workspaces.clone())) as Arc<dyn pawork_domain::AgentTool>,
    ])?;
    Ok(registry)
}

fn mcp_config_from_pawork(config: &pawork_config::PaworkConfig) -> Result<McpConfig, McpError> {
    match config.extra.get("mcp") {
        Some(value) => McpConfig::from_value(value),
        None => Ok(McpConfig::default()),
    }
}

/// Independent MCP secret store next to auth.json. Never the Provider FileBackend.
fn mcp_secret_backend() -> Arc<dyn SecretBackend> {
    let path = FileBackend::new().path().with_file_name("mcp-auth.json");
    Arc::new(FileBackend::with_path(path))
}

fn stdio_runtime(roots: &[PathBuf], trusted: bool) -> Option<StdioSandboxRuntime> {
    if roots.is_empty() {
        return None;
    }
    let mut policy = SandboxPolicy {
        filesystem: FilesystemPolicy {
            read_roots: roots.to_vec(),
            write_roots: if trusted {
                roots.to_vec()
            } else {
                Vec::new()
            },
            deny: default_secret_paths(),
        },
        network_mode: NetworkMode::Hint,
        allow_spawn: true,
        ..SandboxPolicy::default()
    };
    apply_mcp_stdio_env_hygiene(&mut policy);
    StdioSandboxRuntime::new(Arc::new(NativeRestricted::new()), policy, roots.to_vec()).ok()
}

fn discover_skill_ids(dir: &Path) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return ids;
    };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                ids.insert(name.to_string());
            }
        }
    }
    ids
}

fn instruction_kind_name(kind: ResourceInstructionKind) -> &'static str {
    match kind {
        ResourceInstructionKind::AgentProfile => "agent_profile",
        ResourceInstructionKind::UserGlobalInstructions => "user_global_instructions",
        ResourceInstructionKind::WorkspaceInstructions => "workspace_instructions",
        ResourceInstructionKind::RootAgentsFile => "root_agents_file",
        ResourceInstructionKind::PathAgentsFile => "path_agents_file",
        ResourceInstructionKind::ActiveSkill => "active_skill",
        ResourceInstructionKind::PromptTemplate => "prompt_template",
        ResourceInstructionKind::SessionInstructions => "session_instructions",
        ResourceInstructionKind::RunInstructions => "run_instructions",
    }
}

fn at_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '@' {
            index += 1;
            let start = index;
            while index < chars.len()
                && (chars[index].is_ascii_alphanumeric()
                    || matches!(chars[index], '_' | '-' | '.' | '/'))
            {
                index += 1;
            }
            if index > start {
                let mut token: String = chars[start..index].iter().collect();
                token = token
                    .trim_end_matches(|ch: char| matches!(ch, '.' | ',' | ';' | ':' | '/' ))
                    .to_string();
                if !token.is_empty() {
                    tokens.push(token);
                }
            }
        } else {
            index += 1;
        }
    }
    tokens
}
