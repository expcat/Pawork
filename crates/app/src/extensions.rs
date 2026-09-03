//! S9 波 C：MCP 装配、资源注入与 `@file` 解析。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pawork_auth::locator::{MCP_AUTH_FILE_NAME, MCP_SERVICE_PREFIX};
use pawork_auth::{FileBackend, SecretBackend};
use pawork_domain::ContentPart;
use pawork_engine::InjectedLayer;
use pawork_exec::{
    default_secret_paths, FilesystemPolicy, NativeRestricted, NetworkMode, SandboxPolicy,
};
use pawork_tools::mcp::capabilities::register_server_tools;
use pawork_tools::mcp::config::{McpConfig, McpServerConfig, StdioSandboxRuntime, TransportSpec};
use pawork_tools::mcp::manager::{ConnectionState, ManagedMcpClient};
use pawork_tools::mcp::sandbox::apply_mcp_stdio_env_hygiene;
use pawork_tools::mcp::security::SecretRef;
use pawork_tools::mcp::{McpError, McpPeer};
use pawork_tools::{
    ApplyPatchTool, EditFileTool, FindFilesTool, ListDirectoryTool, ReadFileTool, RunCommandTool,
    SearchTextTool, ToolRegistry, ToolScheduler, ToolSchedulerConfig, WriteFileTool,
};
use pawork_workspace::resources::ResourceInstructionKind;
use pawork_workspace::WorkspaceService;
use serde::Serialize;

use crate::{AppCore, AppError};

pub(crate) const AT_FILE_MAX_BYTES: usize = 64 * 1024;

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
                approval_mode: self.approval.mode(),
                workspace_trusted: self.approval.workspace_trusted(),
            },
        ));
    }

    /// 扫描 file-index，并启动已配置的 MCP server（失败不拖垮装配）。
    pub async fn prime_extensions(&mut self) -> Result<(), AppError> {
        if let Ok(Some(workspace)) = self
            .extensions
            .workspaces
            .get(&self.extensions.workspace_id)
        {
            if let Err(error) = self.extensions.file_index.scan_workspace(&workspace).await {
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
        let Ok(Some(workspace)) = self
            .extensions
            .workspaces
            .get(&self.extensions.workspace_id)
        else {
            return;
        };
        let runtime = stdio_runtime(&workspace.roots, self.approval.workspace_trusted());
        let mut registry = match builtin_registry(&self.extensions.workspaces) {
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
            if !self.approval.workspace_trusted() {
                slots.push(McpServerSlot {
                    name: name.clone(),
                    transport,
                    state: "configured".into(),
                    last_error: Some("MCP auto-start is disabled in an untrusted workspace".into()),
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
                        server.trusted && self.approval.workspace_trusted(),
                        self.approval.workspace_trusted(),
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
        self.extensions.mcp_servers = slots;
    }

    pub fn mcp_list(&self) -> Vec<McpServerStatus> {
        self.extensions.mcp_list(self)
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

    /// ADR-049 D2：移除 MCP server 的内存同步（写盘与清密由调用方完成）。
    /// 定序：生效配置 extra 同步 → shutdown 该 slot client（best-effort，
    /// 失败不阻断，盘已为权威）→ 删 slot → 重建 registry 去除该 server 工具。
    /// 进行中 run 已快照的工具不回溯撤销（快照语义）。
    pub(crate) async fn remove_mcp_server(&mut self, name: &str) -> Result<(), AppError> {
        if let Some(serde_json::Value::Object(mcp)) = self.config.extra.get_mut("mcp") {
            if let Some(serde_json::Value::Object(servers)) = mcp.get_mut("servers") {
                servers.remove(name);
            }
        }
        if let Some(index) = self
            .extensions
            .mcp_servers
            .iter()
            .position(|slot| slot.name == name)
        {
            let slot = self.extensions.mcp_servers.remove(index);
            if let Some(client) = &slot.client {
                if let Err(error) = client.shutdown().await {
                    tracing::warn!(
                        error = %error,
                        server = %name,
                        "mcp client shutdown failed during removal"
                    );
                }
            }
        }
        let config = mcp_config_from_pawork(&self.config)?;
        let mut registry = builtin_registry(&self.extensions.workspaces)?;
        let mut reregister_failed = Vec::new();
        for slot in &self.extensions.mcp_servers {
            // 仅重建此前确实注册进 registry 的工具（descriptors 为准）：
            // 非 auto-start / 装配失败 / 仅 test 过的 slot 不额外注册。
            let registered = slot.tools.iter().any(|tool| {
                self.descriptors
                    .iter()
                    .any(|descriptor| &descriptor.name == tool)
            });
            if !registered {
                continue;
            }
            let (Some(client), Some(server)) = (slot.client.clone(), config.server(&slot.name))
            else {
                continue;
            };
            let peer: Arc<dyn McpPeer> = client;
            if let Err(error) = register_server_tools(
                &mut registry,
                &slot.name,
                peer,
                server.permissions.clone(),
                server.trusted && self.approval.workspace_trusted(),
                self.approval.workspace_trusted(),
            )
            .await
            {
                tracing::warn!(
                    error = %error,
                    server = %slot.name,
                    "mcp tool re-registration failed during removal"
                );
                reregister_failed.push(slot.name.clone());
            }
        }
        // 重注册失败的 slot 同步清空 tools，mcp_list 不谎报未注册工具。
        for name in reregister_failed {
            if let Some(slot) = self
                .extensions
                .mcp_servers
                .iter_mut()
                .find(|slot| slot.name == name)
            {
                slot.tools.clear();
            }
        }
        self.replace_registry(registry);
        Ok(())
    }

    async fn test_one_mcp(&mut self, name: &str) -> Result<(), AppError> {
        let config = mcp_config_from_pawork(&self.config)?;
        let server = config
            .server(name)
            .ok_or_else(|| AppError::Import(format!("unknown MCP server '{name}'")))?
            .clone();
        let workspace = self
            .extensions
            .workspaces
            .get(&self.extensions.workspace_id)?
            .ok_or_else(|| AppError::Import("workspace is not attached".into()))?;
        if matches!(server.transport, TransportSpec::Stdio { .. })
            && !self.approval.workspace_trusted()
        {
            return Err(AppError::Mcp(McpError::PermissionDenied(format!(
                "stdio MCP server '{name}' cannot start in an untrusted workspace"
            ))));
        }
        let runtime = stdio_runtime(&workspace.roots, self.approval.workspace_trusted());
        let client =
            Arc::new(server.build_client(name.to_string(), mcp_secret_backend(), runtime)?);
        client.ping().await?;
        let tools = client
            .list_tools()
            .await?
            .into_iter()
            .map(|tool| pawork_tools::mcp::capabilities::namespaced_name(name, &tool.name))
            .collect();
        let health = client.health().await;
        let state = match health.state {
            ConnectionState::Connected => "connected",
            ConnectionState::Connecting => "connecting",
            ConnectionState::Failed => "failed",
            ConnectionState::Disconnected => "disconnected",
        };
        if let Some(slot) = self
            .extensions
            .mcp_servers
            .iter_mut()
            .find(|slot| slot.name == name)
        {
            slot.state = state.into();
            slot.tools = tools;
            slot.last_error = health.last_error.map(|error| error.to_string());
            slot.client = Some(client);
        } else {
            self.extensions.mcp_servers.push(McpServerSlot {
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

    pub(crate) async fn load_injected_layers_for_session(
        &self,
        session_id: &pawork_domain::SessionId,
    ) -> Vec<InjectedLayer> {
        let Ok(workspace) = self.workspace_for_session(session_id) else {
            return Vec::new();
        };
        self.extensions.load_injected_layers(self, &workspace).await
    }

    #[cfg(test)]
    pub(crate) async fn load_injected_layers_for_current(&self) -> Vec<InjectedLayer> {
        let Ok(workspace) = self.workspace_by_id(self.workspace_id()) else {
            return Vec::new();
        };
        self.extensions.load_injected_layers(self, &workspace).await
    }

    /// 把 `@token` 解析为 file-index 命中，正文作为独立 Text part。
    pub async fn expand_at_refs(
        &self,
        session_id: Option<&pawork_domain::SessionId>,
        text: &str,
    ) -> Result<Vec<ContentPart>, AppError> {
        let workspace = match session_id {
            Some(session_id) => self.workspace_for_session_or_unbound(session_id)?,
            None => self
                .workspace_by_id(self.workspace_id())
                .unwrap_or_else(|_| crate::unbound_workspace()),
        };
        self.extensions.expand_at_refs(&workspace, text).await
    }

    pub async fn complete_at(&self, query: &str, limit: usize) -> Result<Vec<String>, AppError> {
        let workspace = self
            .workspace_by_id(self.workspace_id())
            .unwrap_or_else(|_| crate::unbound_workspace());
        self.extensions.complete_at(&workspace, query, limit).await
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.extensions.workspace_root()
    }

    pub(crate) async fn shutdown_mcp(&self) {
        self.extensions.shutdown_mcp().await
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

pub(crate) fn mcp_config_from_pawork(
    config: &pawork_workspace::config::PaworkConfig,
) -> Result<McpConfig, McpError> {
    match config.extra.get("mcp") {
        Some(value) => McpConfig::from_value(value),
        None => Ok(McpConfig::default()),
    }
}

/// ADR-049 D2：定位移除 `<name>` 时应清理的 SecretRef——仅收集
/// `pawork.mcp.<name>` service 下的引用；指向其它 server 的 `pawork.mcp.*`
/// 引用不属于本次清理范围（跳过）；非 `pawork.mcp.*` 命名空间的引用
/// fail-closed（Err，调用方不得继续写盘清密）。
pub(crate) fn mcp_server_secrets_for_removal(
    name: &str,
    server: &McpServerConfig,
) -> Result<Vec<SecretRef>, AppError> {
    let expected_service = format!("{MCP_SERVICE_PREFIX}{name}");
    let references = match &server.transport {
        TransportSpec::Stdio { env, .. } => env.values(),
        TransportSpec::Http { headers, .. } => headers.values(),
    };
    let mut owned = Vec::new();
    for reference in references {
        if reference.service() == expected_service {
            owned.push(reference.clone());
        } else if !reference.service().starts_with(MCP_SERVICE_PREFIX) {
            return Err(AppError::Mcp(McpError::Secret(format!(
                "secret service '{}' is outside the pawork.mcp.* namespace",
                reference.service()
            ))));
        }
    }
    Ok(owned)
}

/// ADR-049 D2：删除已定位的 MCP SecretRef。`SecretBackend::delete` 幂等
/// （NotFound 视为已清理）；其余错误如实上抛，由调用方按阶段回执。
pub(crate) fn clear_mcp_server_secrets(references: &[SecretRef]) -> Result<(), AppError> {
    let backend = mcp_secret_backend();
    for reference in references {
        match backend.delete(reference.service(), reference.account()) {
            Ok(()) | Err(pawork_auth::AuthError::NotFound) => {}
            Err(error) => return Err(AppError::Auth(error)),
        }
    }
    Ok(())
}

/// Independent MCP secret store next to auth.json. Never the Provider FileBackend.
pub(crate) fn mcp_secret_backend() -> Arc<dyn SecretBackend> {
    let path = FileBackend::new().path().with_file_name(MCP_AUTH_FILE_NAME);
    Arc::new(FileBackend::with_path(path))
}

fn stdio_runtime(roots: &[PathBuf], trusted: bool) -> Option<StdioSandboxRuntime> {
    if roots.is_empty() {
        return None;
    }
    let mut policy = SandboxPolicy {
        filesystem: FilesystemPolicy {
            read_roots: roots.to_vec(),
            write_roots: if trusted { roots.to_vec() } else { Vec::new() },
            deny: default_secret_paths(),
        },
        network_mode: NetworkMode::Hint,
        allow_spawn: true,
        ..SandboxPolicy::default()
    };
    apply_mcp_stdio_env_hygiene(&mut policy);
    StdioSandboxRuntime::new(Arc::new(NativeRestricted::new()), policy, roots.to_vec()).ok()
}

pub(crate) fn discover_skill_ids(dir: &Path) -> BTreeSet<String> {
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

pub(crate) fn instruction_kind_name(kind: ResourceInstructionKind) -> &'static str {
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

pub(crate) fn at_tokens(text: &str) -> Vec<String> {
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
                    .trim_end_matches(|ch: char| matches!(ch, '.' | ',' | ';' | ':' | '/'))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn http_server(headers: BTreeMap<String, SecretRef>) -> McpServerConfig {
        McpServerConfig {
            transport: TransportSpec::Http {
                url: "https://mcp.example.com/mcp".into(),
                headers,
            },
            auto_start: false,
            timeout_ms: None,
            restart: Default::default(),
            permissions: Default::default(),
            trusted: false,
        }
    }

    #[test]
    fn mcp_server_secrets_for_removal_collects_skips_and_fails_closed() {
        // 非 pawork.mcp.* 命名空间：fail-closed（Err），调用方不得继续写盘清密。
        let mut headers = BTreeMap::new();
        headers.insert(
            "Authorization".to_string(),
            SecretRef::new("other.service", "cred-1"),
        );
        assert!(matches!(
            mcp_server_secrets_for_removal("demo", &http_server(headers)),
            Err(AppError::Mcp(McpError::Secret(_)))
        ));

        // 其它 server 的 pawork.mcp.* 引用：跳过，不收集。
        let mut headers = BTreeMap::new();
        headers.insert(
            "Authorization".to_string(),
            SecretRef::new("pawork.mcp.other", "cred-1"),
        );
        let collected =
            mcp_server_secrets_for_removal("demo", &http_server(headers)).expect("skip others");
        assert!(collected.is_empty());

        // 本 server 的引用：全部收集（多 header / 多 account 一并纳入）。
        let mut headers = BTreeMap::new();
        headers.insert(
            "Authorization".to_string(),
            SecretRef::new("pawork.mcp.demo", "cred-1"),
        );
        headers.insert(
            "X-Custom".to_string(),
            SecretRef::new("pawork.mcp.demo", "cred-2"),
        );
        let collected = mcp_server_secrets_for_removal("demo", &http_server(headers))
            .expect("collect own refs");
        assert_eq!(collected.len(), 2);
        assert!(collected
            .iter()
            .all(|reference| reference.service() == "pawork.mcp.demo"));
    }
}
