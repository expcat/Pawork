//! P17-1 生产装配：global + workspace 两级 user hook 配置 → [`UserHookHost`]。
//!
//! 正式宿主（`pawork`）启动时调用 [`assemble_user_hooks`]：
//!
//! 1. 经 `AppService` 注册 / 复用工作区（与 cli-host 打开 workspace 使用
//!    同一个 `WorkspaceAdd` 入口，workspace id 与 run 时完全一致）；
//! 2. 用 `resource-loader` 的 hooks 入口加载 global（`$global/hooks/*.toml`）
//!    与 workspace（`<root>/.pawork/hooks/*.toml`）两级 hook 配置；
//! 3. 按 (tier, source_key) 确定性合并（高 tier 覆盖低 tier），转换并注册
//!    进 [`UserHookHost`]；
//! 4. 注入 `CoreRuntime` 的 run loop（pre-prompt / pre-tool 权威位点回灌）
//!    与 supervisor 的 workspace roots。
//!
//! secret 明文只经注入的 [`SecretBackend`] 解析（生产为 OS Keychain），
//! 配置 / 事件 / 日志全程只存引用；审计写入实例目录 SQLite。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_domain::{ActorId, CommandId, ProviderId, QueryId, Timestamp, WorkspaceId};
use app_service::{
    hook_config_from_resource, AppService, EvalProfile, EvalProfileResolver,
    HookWorkspaceTrustResolver, ProviderResolver, SqliteHookAuditSink, StaticHookRunContext,
    UserHookHost, UserHookHostOptions,
};
use auth_service::SecretBackend;
use cli_command::{Cli, Command};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, CommandSource,
    API_VERSION,
};
use provider_api::ModelProvider;
use resource_loader::{
    load_hooks, ResourceDiagnosticStatus, ResourceLimits, ResourceLoader, ResourceLoaderOptions,
    ResourceRequest, UserHookConfig, WorkspaceRelativePath,
};
use serde_json::Value;
use workspace_service::{TrustState, Workspace, WorkspaceService, WorkspaceSnapshot};

/// 按 ProviderId 解析 canonical `ModelProvider` 的生产解析器：查 `AppService`
/// 的共享 Provider 注册表（正式宿主经 `register_provider` 注入同一批
/// provider）。特殊 id `default` 映射到注册表中 ProviderId 升序的第一个
/// provider（User Hook 默认判定 profile 的兜底落点；未注册任何 provider 时
/// 返回 `None`，判定 fail-closed 并审计可见）。不做任何 Provider 名分支。
struct ServiceProviders {
    service: Arc<AppService>,
}

struct ServiceWorkspaceTrust {
    service: Arc<AppService>,
}

impl HookWorkspaceTrustResolver for ServiceWorkspaceTrust {
    fn is_trusted(&self, workspace_id: &WorkspaceId) -> Option<bool> {
        list_workspace_snapshot(&self.service)
            .ok()?
            .workspaces
            .into_iter()
            .find(|workspace| &workspace.id == workspace_id)
            .map(|workspace| workspace.trust == TrustState::Trusted)
    }
}

impl ProviderResolver for ServiceProviders {
    fn resolve(&self, id: &ProviderId) -> Option<Arc<dyn ModelProvider>> {
        if id.as_str() == "default" {
            self.service.first_provider()
        } else {
            self.service.provider(id)
        }
    }
}

/// 按 workspace + 受限 profile 名解析 [`EvalProfile`] 的生产实现。条目只来自
/// P17-5 ResourceLoader；未知名称、未知 workspace 与缺模型字段均 fail-closed，
/// 绝不静默回退默认模型。
#[derive(Clone, Debug)]
pub struct NamedEvalProfileResolver {
    profiles: BTreeMap<(WorkspaceId, String), EvalProfile>,
}

impl NamedEvalProfileResolver {
    pub fn new() -> Self {
        Self {
            profiles: BTreeMap::new(),
        }
    }

    pub fn insert(
        &mut self,
        workspace_id: WorkspaceId,
        name: impl Into<String>,
        profile: EvalProfile,
    ) {
        self.profiles.insert((workspace_id, name.into()), profile);
    }
}

impl Default for NamedEvalProfileResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl EvalProfileResolver for NamedEvalProfileResolver {
    fn resolve(&self, workspace_id: Option<&WorkspaceId>, profile: &str) -> Option<EvalProfile> {
        self.profiles
            .get(&(workspace_id?.clone(), profile.to_string()))
            .cloned()
    }
}

/// P17-5 主 run profile 解析器：按 (workspace, name) 返回 loader 已校验的
/// 不可变 AgentProfileV2。复用 P17-1 ResourceLoader 加载（不重复解析逻辑、
/// 不做 Provider 名分支）；未知 / 跨 workspace 一律 fail-closed。
#[derive(Clone, Debug, Default)]
pub struct NamedRunProfileResolver {
    profiles: BTreeMap<(WorkspaceId, String), agent_domain::AgentProfileV2>,
}

impl NamedRunProfileResolver {
    pub fn new() -> Self {
        Self {
            profiles: BTreeMap::new(),
        }
    }

    pub fn insert(
        &mut self,
        workspace_id: WorkspaceId,
        name: impl Into<String>,
        profile: agent_domain::AgentProfileV2,
    ) {
        self.profiles.insert((workspace_id, name.into()), profile);
    }

    /// 已注册的 (workspace, profile 名) 数量（装配诊断）。
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

impl app_service::RunProfileResolver for NamedRunProfileResolver {
    fn resolve(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<app_service::ResolvedRunProfile, app_service::ProfileResolveError> {
        match self.profiles.get(&(workspace_id.clone(), name.to_string())) {
            Some(profile) => Ok(app_service::ResolvedRunProfile {
                workspace_id: workspace_id.clone(),
                profile: profile.clone(),
            }),
            None => Err(app_service::ProfileResolveError::Unknown {
                name: name.to_string(),
                workspace: workspace_id.clone(),
            }),
        }
    }
}

/// 装配结果（供宿主注入与诊断）。
#[derive(Debug)]
pub struct UserHookAssembly {
    pub host: Arc<UserHookHost>,
    /// 已注册的 hook id（global + workspace，去重后确定性排序）。
    pub hook_ids: Vec<String>,
    /// 被高 tier 覆盖的 hook id 与来源（诊断）。
    pub overridden: Vec<(String, String)>,
    /// 装配解析出的 (workspace_id, root)。
    pub workspace_ids: Vec<(WorkspaceId, PathBuf)>,
    /// 资源加载诊断（损坏文件等，不阻断其余 hooks）。
    pub diagnostics: Vec<String>,
}

/// 从 CLI 参数推导装配 workspace roots：`run --workspace` 或当前目录、
/// `shell` 当前目录；其余命令不装配 workspace hooks（仅 global）。
pub fn cli_workspace_roots(cli: &Cli) -> Vec<PathBuf> {
    fn current_dir() -> Option<PathBuf> {
        std::env::current_dir().ok()
    }
    match &cli.command {
        Command::Run(args) => args
            .workspace
            .as_deref()
            .map(PathBuf::from)
            .or_else(current_dir)
            .into_iter()
            .collect(),
        Command::Shell => current_dir().into_iter().collect(),
        _ => Vec::new(),
    }
}

/// 生产装配 User Hooks 宿主。失败（审计库无法打开等）返回错误，调用方降级
/// 为仅 global 或不装配（不阻断宿主启动）。
#[allow(clippy::too_many_arguments)]
pub fn assemble_user_hooks(
    service: Arc<AppService>,
    workspace_roots: &[PathBuf],
    global_resource_dir: Option<PathBuf>,
    audit_db_path: PathBuf,
    secret_backend: Arc<dyn SecretBackend>,
) -> Result<UserHookAssembly, String> {
    // 1) 工作区：复用已有（按 root 路径）或注册（与 cli-host 同一入口）。
    let resolved = resolve_workspaces(&service, workspace_roots)?;
    let workspace_snapshot = list_workspace_snapshot(&service)?;

    // 2) 配置加载：global + 每个 workspace root。
    let mut candidates: Vec<UserHookConfig> = Vec::new();
    let mut diagnostics = Vec::new();
    let mut overridden = Vec::new();
    for (id, root) in &resolved {
        let resolution = load_hooks(
            global_resource_dir.as_deref(),
            std::slice::from_ref(root),
            ".pawork",
            id.as_str(),
            ResourceLimits::default(),
        );
        for issue in &resolution.diagnostics.issues {
            diagnostics.push(format!("{}: {}", issue.code, issue.message));
        }
        for entry in &resolution.diagnostics.entries {
            if entry.status == ResourceDiagnosticStatus::Overridden {
                overridden.push((
                    entry.resource_id.clone(),
                    entry.provenance.source_key.clone(),
                ));
            }
        }
        candidates.extend(resolution.hooks);
    }
    // 无 workspace 时也必须加载 global hooks（headless / serve 等场景）。
    if resolved.is_empty() {
        let resolution = load_hooks(
            global_resource_dir.as_deref(),
            &[],
            ".pawork",
            "",
            ResourceLimits::default(),
        );
        for issue in &resolution.diagnostics.issues {
            diagnostics.push(format!("{}: {}", issue.code, issue.message));
        }
        for entry in &resolution.diagnostics.entries {
            if entry.status == ResourceDiagnosticStatus::Overridden {
                overridden.push((
                    entry.resource_id.clone(),
                    entry.provenance.source_key.clone(),
                ));
            }
        }
        candidates.extend(resolution.hooks);
    }

    // 3) 合并：按 (tier, source_key) 确定性排序，同 id 高 tier 覆盖低 tier；
    //    完全同源（跨 workspace 的 global 重复）不算覆盖。
    candidates.sort_by(|left, right| {
        left.provenance
            .tier
            .cmp(&right.provenance.tier)
            .then_with(|| left.provenance.source_key.cmp(&right.provenance.source_key))
    });
    let mut effective: BTreeMap<String, UserHookConfig> = BTreeMap::new();
    for candidate in candidates {
        if let Some(prev) = effective.get(&candidate.id) {
            if prev.provenance.source_key != candidate.provenance.source_key {
                overridden.push((candidate.id.clone(), prev.provenance.source_key.clone()));
            }
        }
        effective.insert(candidate.id.clone(), candidate);
    }

    // 4) 宿主装配 + 注册。
    let default_eval = EvalProfile {
        provider_id: ProviderId::from("default"),
        model: agent_domain::ModelId::from("default"),
        system_prompt: None,
        reasoning_effort: None,
        budget: None,
        tool_rules: agent_domain::ProfileToolRules::default(),
        isolation: agent_domain::ProfileIsolation::None,
    };
    let profiles = load_eval_profiles(
        &workspace_snapshot,
        &resolved,
        global_resource_dir.clone(),
        &mut diagnostics,
    )?;
    let mut options = UserHookHostOptions::new(
        workspace_roots.to_vec(),
        Arc::new(ServiceProviders {
            service: Arc::clone(&service),
        }),
        default_eval,
        Arc::new(profiles),
        secret_backend,
    );
    options.workspace_trust = Some(Arc::new(ServiceWorkspaceTrust {
        service: Arc::clone(&service),
    }));
    options.audit_sink = Arc::new(
        SqliteHookAuditSink::open(&audit_db_path)
            .map_err(|error| format!("open hook audit db {}: {error}", audit_db_path.display()))?,
    );
    options.run_context = Arc::new(StaticHookRunContext::new("hook-host", "hook-host"));
    let mut host = UserHookHost::new(options).map_err(|error| error.to_string())?;
    let mut hook_ids = Vec::new();
    for hook in effective.into_values() {
        let config = hook_config_from_resource(
            hook.id.clone(),
            Value::String(hook.trigger.clone()),
            serde_json::to_value(&hook.scope).map_err(|error| error.to_string())?,
            hook.enabled,
            hook.lifecycle.clone(),
            hook.handler.clone(),
        )
        .map_err(|error| format!("hook `{}`: {error}", hook.id))?;
        host.register(config)
            .map_err(|error| format!("register hook `{}`: {error}", hook.id))?;
        hook_ids.push(hook.id);
    }
    let host = Arc::new(host);

    // 5) 注入 run loop（pre-prompt / pre-tool 权威位点）与 workspace roots。
    service.set_user_hooks(Arc::clone(&host));
    service.set_workspace_roots(workspace_roots.to_vec());

    Ok(UserHookAssembly {
        host,
        hook_ids,
        overridden,
        workspace_ids: resolved,
        diagnostics,
    })
}

fn load_eval_profiles(
    snapshot: &WorkspaceSnapshot,
    resolved: &[(WorkspaceId, PathBuf)],
    global_resource_dir: Option<PathBuf>,
    diagnostics: &mut Vec<String>,
) -> Result<NamedEvalProfileResolver, String> {
    let workspaces = WorkspaceService::from_snapshot(snapshot.clone())
        .map_err(|error| format!("restore workspace snapshot for hook profiles: {error}"))?;
    let loader = ResourceLoader::new(
        workspaces,
        ResourceLoaderOptions {
            global_resource_dir,
            ..ResourceLoaderOptions::default()
        },
    );
    let mut resolver = NamedEvalProfileResolver::new();
    for (workspace_id, root) in resolved {
        let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| &workspace.id == workspace_id)
        else {
            diagnostics.push(format!(
                "hook_profile_workspace_unknown: workspace `{workspace_id}` is unavailable"
            ));
            continue;
        };
        let Some(root_index) = workspace
            .roots
            .iter()
            .position(|candidate| candidate.path == canonical_root)
        else {
            diagnostics.push(format!(
                "hook_profile_root_unknown: {} is not registered in workspace `{workspace_id}`",
                root.display()
            ));
            continue;
        };
        let bundle = loader
            .load(&ResourceRequest::new(
                workspace_id.clone(),
                root_index,
                WorkspaceRelativePath::default(),
            ))
            .map_err(|error| format!("load P17-5 profiles for `{workspace_id}`: {error}"))?;
        for issue in &bundle.diagnostics.issues {
            diagnostics.push(format!("{}: {}", issue.code, issue.message));
        }
        for loaded in bundle.profiles_v2 {
            if loaded.profile.isolation == agent_domain::ProfileIsolation::None {
                diagnostics.push(format!(
                    "hook_profile_isolation_missing: profile `{}` is not restricted",
                    loaded.profile.name
                ));
                continue;
            }
            let Some(provider) = loaded.profile.model.provider.clone() else {
                diagnostics.push(format!(
                    "hook_profile_model_missing: profile `{}` has no provider",
                    loaded.profile.name
                ));
                continue;
            };
            let Some(model) = loaded.profile.model.name.clone() else {
                diagnostics.push(format!(
                    "hook_profile_model_missing: profile `{}` has no model",
                    loaded.profile.name
                ));
                continue;
            };
            let system_prompt = match loaded.profile.prompt.instructions.as_deref() {
                Some(instructions) if !instructions.trim().is_empty() => {
                    format!("{}\n\n{instructions}", loaded.profile.prompt.system)
                }
                _ => loaded.profile.prompt.system.clone(),
            };
            resolver.insert(
                workspace_id.clone(),
                loaded.profile.name.clone(),
                EvalProfile::restricted(
                    ProviderId::from(provider),
                    agent_domain::ModelId::from(model),
                    system_prompt,
                    loaded.profile.effort,
                    loaded.profile.tools,
                    loaded.profile.max_turns,
                    loaded.profile.isolation,
                ),
            );
        }
    }
    Ok(resolver)
}

/// P17-5 生产装配：主 run profile 解析器。复用 P17-1 的 ResourceLoader 加载
/// 每个工作区的 `ResourceBundle.profiles_v2`（loader 已校验引用 / 无明文
/// secret），按 (workspace, profile 名) 注册不可变 AgentProfileV2。失败不阻断
/// 宿主启动，仅降级为不注入解析器（RunStart 携带 profile 名时 fail-closed）。
pub fn assemble_run_profiles(
    service: &Arc<AppService>,
    workspace_roots: &[PathBuf],
    global_resource_dir: Option<PathBuf>,
) -> Result<NamedRunProfileResolver, String> {
    let snapshot = list_workspace_snapshot(service)
        .map_err(|error| format!("snapshot workspaces for run profiles: {error}"))?;
    let resolved = resolve_workspaces(service, workspace_roots)?;
    let workspaces = WorkspaceService::from_snapshot(snapshot.clone())
        .map_err(|error| format!("restore workspace snapshot for run profiles: {error}"))?;
    let loader = ResourceLoader::new(
        workspaces,
        ResourceLoaderOptions {
            global_resource_dir,
            ..ResourceLoaderOptions::default()
        },
    );
    let mut resolver = NamedRunProfileResolver::new();
    for (workspace_id, root) in &resolved {
        let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| &workspace.id == workspace_id)
        else {
            continue;
        };
        let Some(root_index) = workspace
            .roots
            .iter()
            .position(|candidate| candidate.path == canonical_root)
        else {
            continue;
        };
        let bundle = loader
            .load(&ResourceRequest::new(
                workspace_id.clone(),
                root_index,
                WorkspaceRelativePath::default(),
            ))
            .map_err(|error| format!("load P17-5 profiles for `{workspace_id}`: {error}"))?;
        for loaded in bundle.profiles_v2 {
            // 主 run 不施加 hook 那种 isolation!=None 过滤：None 隔离合法；Container
            // 等不满足的隔离在 RunStart 由 app-service fail-closed 拦截。
            resolver.insert(workspace_id.clone(), loaded.profile.name.clone(), loaded.profile);
        }
    }
    Ok(resolver)
}

/// 复用已有 workspace（按 root 路径匹配）或注册新 workspace；返回
/// (workspace_id, root) 列表。与 cli-host `ensure_workspace` 同一 id 来源，
/// 保证 run 时 workspace 作用域匹配。
fn resolve_workspaces(
    service: &AppService,
    roots: &[PathBuf],
) -> Result<Vec<(WorkspaceId, PathBuf)>, String> {
    let existing = list_workspaces(service);
    let mut out = Vec::new();
    for root in roots {
        let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let path = canonical_root.to_string_lossy().into_owned();
        let id = existing
            .iter()
            .find(|(_, existing_path)| existing_path == &path)
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| {
                add_workspace(service, &path).unwrap_or_else(|error| {
                    tracing::warn!(
                        "workspace hook assembly skipped {}: {error}",
                        root.display()
                    );
                    WorkspaceId::new(format!("unresolved-{}", root.display()))
                })
            });
        out.push((id, root.clone()));
    }
    Ok(out)
}

fn list_workspace_snapshot(service: &AppService) -> Result<WorkspaceSnapshot, String> {
    let response = service.dispatch_query(envelope_query(AppQuery::WorkspaceList));
    let core_api::AppResponse::Data(value) = response.response else {
        return Err("workspace list did not return data".into());
    };
    let workspaces: Vec<Workspace> = serde_json::from_value(value)
        .map_err(|error| format!("decode workspace trust snapshot: {error}"))?;
    Ok(WorkspaceSnapshot { workspaces })
}

fn list_workspaces(service: &AppService) -> Vec<(WorkspaceId, String)> {
    let response = service.dispatch_query(envelope_query(AppQuery::WorkspaceList));
    let core_api::AppResponse::Data(value) = response.response else {
        return Vec::new();
    };
    let Some(workspaces) = value.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for workspace in workspaces {
        let id = workspace
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(roots) = workspace.get("roots").and_then(Value::as_array) {
            for root in roots {
                if let Some(path) = root.get("path").and_then(Value::as_str) {
                    out.push((WorkspaceId::from(id), path.to_string()));
                }
            }
        }
    }
    out
}

fn add_workspace(service: &AppService, root_path: &str) -> Result<WorkspaceId, String> {
    match service
        .dispatch_envelope(envelope_command(AppCommand::WorkspaceAdd {
            root_path: root_path.to_string(),
        }))
        .response
    {
        core_api::AppResponse::Data(value) => Ok(WorkspaceId::from(
            value.get("id").and_then(Value::as_str).unwrap_or_default(),
        )),
        core_api::AppResponse::Error(context) => Err(context.message),
        other => Err(format!("unexpected workspace add response: {other:?}")),
    }
}

fn envelope_query(query: AppQuery) -> AppQueryEnvelope {
    AppQueryEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from(format!("hook-assembly-{}", now_millis())),
        source: CommandSource::LocalCli {
            terminal_session_id: None,
        },
        identity: ActorIdentity::LocalUser {
            actor_id: ActorId::from("pawork-host"),
            display_name: None,
        },
        issued_at: now_timestamp(),
        query,
    }
}

fn envelope_command(command: AppCommand) -> AppCommandEnvelope {
    AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from(format!("hook-assembly-{}", now_millis())),
        source: CommandSource::LocalCli {
            terminal_session_id: None,
        },
        identity: ActorIdentity::LocalUser {
            actor_id: ActorId::from("pawork-host"),
            display_name: None,
        },
        expected_revision: None,
        idempotency_key: None,
        issued_at: now_timestamp(),
        command,
    }
}

fn now_timestamp() -> Timestamp {
    Timestamp::from_unix_millis(now_millis())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use auth_service::MemoryBackend;
    use std::path::Path;
    use tempfile::tempdir;

    fn write_hook(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write hook");
    }

    fn command_hook_body() -> &'static str {
        r#"
trigger = "run_started"
[handler]
kind = "command"
program = "/bin/echo"
"#
    }

    fn restricted_profile_body() -> &'static str {
        r#"
schema = "v2"
name = "hook-reviewer"
max_turns = 2
isolation = "restricted"

[prompt]
system = "Review hook input conservatively."

[model]
provider = "profile-provider"
name = "profile-model"

[tools]
allowed = ["read_file"]
denied = ["shell"]
"#
    }

    #[tokio::test]
    async fn assembly_loads_global_and_workspace_hooks_and_injects_host() {
        let temp = tempdir().expect("tempdir");
        let global = temp.path().join("global");
        let workspace = temp.path().join("ws");
        write_hook(&global, "hooks/global-notify.toml", command_hook_body());
        write_hook(
            &workspace,
            ".pawork/hooks/ws-hook.toml",
            command_hook_body(),
        );

        let service = Arc::new(AppService::new("hook-assembly-test"));
        let assembly = assemble_user_hooks(
            Arc::clone(&service),
            &[workspace.clone()],
            Some(global),
            temp.path().join("audit.sqlite"),
            Arc::new(MemoryBackend::default()),
        )
        .expect("assembly");

        assert!(assembly.hook_ids.contains(&"global-notify".to_string()));
        assert!(assembly.hook_ids.contains(&"ws-hook".to_string()));
        assert_eq!(assembly.overridden.len(), 0);
        assert_eq!(assembly.workspace_ids.len(), 1);
        assert_eq!(assembly.workspace_ids[0].1, workspace);
        // 宿主已注入 run loop（pre-prompt / pre-tool 位点）。
        assert!(service.user_hooks_active());
        assert_eq!(
            service.router().supervisor().workspace_roots(),
            vec![workspace]
        );
    }

    #[tokio::test]
    async fn p17_5_profile_resolver_is_workspace_scoped_and_unknown_fails_closed() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path().join("ws-profile");
        std::fs::create_dir_all(workspace.join(".pawork/profiles")).expect("profile dir");
        std::fs::write(
            workspace.join(".pawork/profiles/hook-reviewer.toml"),
            restricted_profile_body(),
        )
        .expect("write profile");

        let service = Arc::new(AppService::new("hook-profile-resolver"));
        let resolved = resolve_workspaces(&service, std::slice::from_ref(&workspace))
            .expect("resolve workspace");
        let snapshot = list_workspace_snapshot(&service).expect("snapshot");
        let mut diagnostics = Vec::new();
        let resolver = load_eval_profiles(&snapshot, &resolved, None, &mut diagnostics)
            .expect("load P17-5 profile");
        let workspace_id = &resolved[0].0;
        let profile = resolver
            .resolve(Some(workspace_id), "hook-reviewer")
            .expect("known profile resolves");
        assert_eq!(profile.provider_id.as_str(), "profile-provider");
        assert_eq!(profile.model.as_str(), "profile-model");
        assert_eq!(
            profile.system_prompt.as_deref(),
            Some("Review hook input conservatively.")
        );
        assert_eq!(
            profile.reasoning_effort,
            Some(agent_domain::ReasoningEffort::Medium)
        );
        assert_eq!(
            profile.isolation,
            agent_domain::ProfileIsolation::Restricted
        );
        assert_eq!(
            profile.budget.and_then(|budget| budget.max_iterations),
            Some(2)
        );
        assert!(profile.tool_rules.is_allowed("read_file"));
        assert!(profile.tool_rules.is_denied("shell"));
        assert!(resolver.resolve(Some(workspace_id), "missing").is_none());
        assert!(resolver
            .resolve(Some(&WorkspaceId::from("unknown")), "hook-reviewer")
            .is_none());
        assert!(resolver.resolve(None, "hook-reviewer").is_none());
    }

    #[tokio::test]
    async fn assembly_workspace_tier_overrides_global_same_id() {
        let temp = tempdir().expect("tempdir");
        let global = temp.path().join("global");
        let workspace = temp.path().join("ws");
        write_hook(&global, "hooks/same.toml", command_hook_body());
        write_hook(&workspace, ".pawork/hooks/same.toml", command_hook_body());

        let service = Arc::new(AppService::new("hook-assembly-override"));
        let assembly = assemble_user_hooks(
            Arc::clone(&service),
            &[workspace],
            Some(global),
            temp.path().join("audit.sqlite"),
            Arc::new(MemoryBackend::default()),
        )
        .expect("assembly");

        assert_eq!(
            assembly.hook_ids,
            vec!["same".to_string()],
            "同 id 只注册一条"
        );
        assert_eq!(assembly.overridden.len(), 1, "低 tier 覆盖记录可见");
    }

    #[tokio::test]
    async fn assembly_broken_hook_is_isolated_as_diagnostic() {
        let temp = tempdir().expect("tempdir");
        let global = temp.path().join("global");
        write_hook(&global, "hooks/broken.toml", "trigger = [not toml");
        write_hook(&global, "hooks/ok.toml", command_hook_body());

        let service = Arc::new(AppService::new("hook-assembly-isolated"));
        let assembly = assemble_user_hooks(
            Arc::clone(&service),
            &[],
            Some(global),
            temp.path().join("audit.sqlite"),
            Arc::new(MemoryBackend::default()),
        )
        .expect("assembly");

        assert_eq!(assembly.hook_ids, vec!["ok".to_string()]);
        assert!(
            assembly
                .diagnostics
                .iter()
                .any(|message| message.contains("user_hook_invalid")),
            "损坏文件必须产出诊断而不阻断其余 hooks"
        );
    }

    /// 回归：无 workspace（headless / serve 等）时 global hooks 仍必须加载并
    /// 注册进宿主 dispatcher——现代码分支只处理 `resolved` 非空路径时易漏。
    #[tokio::test]
    async fn assembly_without_workspaces_still_registers_global_hooks() {
        let temp = tempdir().expect("tempdir");
        let global = temp.path().join("global");
        write_hook(&global, "hooks/global-notify.toml", command_hook_body());

        let service = Arc::new(AppService::new("hook-global-only"));
        let assembly = assemble_user_hooks(
            Arc::clone(&service),
            &[], // 无 workspace
            Some(global),
            temp.path().join("audit.sqlite"),
            Arc::new(MemoryBackend::default()),
        )
        .expect("assembly must not fail without workspaces");

        assert!(
            assembly.hook_ids.contains(&"global-notify".to_string()),
            "global hooks must load without any workspace: {:?}",
            assembly.hook_ids
        );
        assert!(assembly.workspace_ids.is_empty(), "no workspace resolved");
        // 不止进入候选列表：必须真正注册进宿主 dispatcher（可被派发）。
        assert_eq!(
            assembly.host.dispatcher().registry().len(),
            1,
            "global hook must be registered in the host dispatcher"
        );
    }
}
