//! User Hooks 配置加载（P17-1）：Global + Workspace 两级作用域。
//!
//! 目录约定与 profiles 一致：全局 `$global/hooks/*.toml`（tier=Global），
//! workspace `<root>/.pawork/hooks/*.toml`（tier=Workspace）。文件默认按所在
//! tier 得到作用域（Global / 指定 workspace），文件内显式 `scope` 优先。
//!
//! 依赖方向约束（workspace-layout）：本 crate 不依赖 `user-hooks`；handler
//! 以 `serde_json::Value` 原样透传，由消费侧（正式宿主）转换为
//! `user_hooks::config::HookConfig`。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use config_service::ConfigTier;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    io::{read_utf8_bounded_within, sorted_children_within, workspace_relative_key},
    ResourceDiagnosticEntry, ResourceDiagnosticStatus, ResourceDiagnostics, ResourceIssue,
    ResourceKind, ResourceLimits, ResourceOrigin, ResourceProvenance,
};

/// User hook 的 neutral 作用域（与 `user-hooks::HookScope` 的 serde 形式
/// 一致：tagged `kind` 枚举，snake_case）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserHookScope {
    /// 仅在指定 workspace 触发。
    Workspace { workspace_id: String },
    /// 全局（所有 workspace）。
    Global,
}

/// resource-loader 到消费侧的中性 User Hook DTO。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserHookConfig {
    pub id: String,
    /// 触发点名（`user-hooks::TriggerPoint` 的 snake_case 名；由消费侧解析）。
    pub trigger: String,
    pub scope: UserHookScope,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<String>,
    /// 六类 handler 配置的完整 JSON（`user-hooks::HandlerConfig` 形式）。
    pub handler: Value,
    pub provenance: ResourceProvenance,
}

fn default_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Default)]
pub struct HookResolution {
    pub hooks: Vec<UserHookConfig>,
    pub diagnostics: ResourceDiagnostics,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookFile {
    id: Option<String>,
    trigger: String,
    #[serde(default)]
    scope: Option<UserHookScope>,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    lifecycle: Option<String>,
    handler: Value,
}

/// 加载全部 user hook 配置（Global + 各 workspace root），同 id 高 tier
/// 覆盖低 tier，输出按 (tier, source_key, id) 确定性排序。
///
/// 生产宿主（`apps/pawork`）直接调用此入口装配 user hooks：`workspace_id`
/// 是 workspace 目录缺省 scope 归属的 workspace id（多 root 场景逐 root
/// 调用后由消费侧按同一排序规则合并）。
pub fn load_hooks(
    global_resource_dir: Option<&Path>,
    workspace_roots: &[PathBuf],
    workspace_resource_dir: &str,
    workspace_id: &str,
    limits: ResourceLimits,
) -> HookResolution {
    let mut candidates = Vec::new();
    let mut diagnostics = ResourceDiagnostics::default();
    if let Some(global_root) = global_resource_dir {
        load_directory(
            &global_root.join("hooks"),
            global_root,
            ConfigTier::Global,
            None,
            workspace_id,
            limits,
            &mut candidates,
            &mut diagnostics,
        );
    }
    for (root_index, root) in workspace_roots.iter().enumerate() {
        load_directory(
            &root.join(workspace_resource_dir).join("hooks"),
            root,
            ConfigTier::Workspace,
            Some(root_index),
            workspace_id,
            limits,
            &mut candidates,
            &mut diagnostics,
        );
    }
    candidates.sort_by(|left, right| {
        left.provenance
            .tier
            .cmp(&right.provenance.tier)
            .then_with(|| left.provenance.source_key.cmp(&right.provenance.source_key))
    });
    let mut effective: BTreeMap<String, UserHookConfig> = BTreeMap::new();
    for candidate in candidates {
        if let Some(overridden) = effective.insert(candidate.id.clone(), candidate) {
            diagnostics.entries.push(ResourceDiagnosticEntry {
                kind: ResourceKind::UserHook,
                resource_id: overridden.id,
                status: ResourceDiagnosticStatus::Overridden,
                provenance: overridden.provenance,
            });
        }
    }
    let hooks: Vec<UserHookConfig> = effective
        .into_values()
        .inspect(|hook| {
            diagnostics.entries.push(ResourceDiagnosticEntry {
                kind: ResourceKind::UserHook,
                resource_id: hook.id.clone(),
                status: ResourceDiagnosticStatus::Loaded,
                provenance: hook.provenance.clone(),
            });
        })
        .collect();
    diagnostics.sort_deterministically();
    HookResolution { hooks, diagnostics }
}

// This boundary carries the immutable loader context plus its two output
// accumulators. A context refactor belongs to P17-1; keep the local lint scoped
// here so P17-2 consumers can run clippy without command-line suppression.
#[allow(clippy::too_many_arguments)]
fn load_directory(
    directory: &Path,
    source_root: &Path,
    tier: ConfigTier,
    root_index: Option<usize>,
    workspace_id: &str,
    limits: ResourceLimits,
    candidates: &mut Vec<UserHookConfig>,
    diagnostics: &mut ResourceDiagnostics,
) {
    let paths = match sorted_children_within(directory, source_root, limits.max_resources_per_kind)
    {
        Ok(paths) => paths,
        Err(error) => {
            diagnostics.issues.push(ResourceIssue::error(
                error.code(),
                format!("hooks directory could not be read: {error}"),
            ));
            return;
        }
    };
    for path in paths {
        if path.extension().and_then(|value| value.to_str()) != Some("toml") {
            continue;
        }
        let relative = workspace_relative_key(&path, source_root);
        let source_key = root_index.map_or_else(
            || format!("global:hook:{relative}"),
            |index| format!("workspace:{index:08}:hook:{relative}"),
        );
        let provenance = ResourceProvenance::new(
            tier,
            source_key.clone(),
            root_index.map_or_else(
                || ResourceOrigin::Global {
                    relative_path: relative.clone(),
                },
                |index| ResourceOrigin::Workspace {
                    root_index: index,
                    relative_path: relative.clone(),
                },
            ),
        );
        match parse_hook(
            &path,
            source_root,
            provenance,
            workspace_id,
            limits.max_file_bytes,
        ) {
            Ok(hook) => candidates.push(hook),
            Err(issue) => diagnostics.issues.push(issue),
        }
    }
}

fn parse_hook(
    path: &Path,
    source_root: &Path,
    provenance: ResourceProvenance,
    workspace_id: &str,
    max_file_bytes: u64,
) -> Result<UserHookConfig, ResourceIssue> {
    let fallback_id = file_stem(path);
    let source_key = provenance.source_key.clone();
    let content = read_utf8_bounded_within(path, source_root, max_file_bytes).map_err(|error| {
        ResourceIssue::error(
            error.code(),
            format!("user hook could not be loaded: {error}"),
        )
        .for_resource(ResourceKind::UserHook, &fallback_id, source_key.clone())
    })?;
    let file: HookFile = crate::io::parse_toml_resource(
        &content,
        "user_hook_invalid",
        "user hook has invalid TOML syntax or unsupported fields",
    )
    .map_err(|issue| {
        issue.for_resource(ResourceKind::UserHook, &fallback_id, source_key.clone())
    })?;
    let id = file.id.unwrap_or(fallback_id);
    // 作用域：文件内显式声明优先；否则按文件所在 tier 默认（workspace 目录
    // 默认归属该 workspace）。
    let scope = file.scope.unwrap_or_else(|| {
        if provenance.tier == ConfigTier::Workspace {
            UserHookScope::Workspace {
                workspace_id: workspace_id.to_string(),
            }
        } else {
            UserHookScope::Global
        }
    });
    Ok(UserHookConfig {
        id,
        trigger: file.trigger,
        scope,
        enabled: file.enabled,
        lifecycle: file.lifecycle,
        handler: file.handler,
        provenance,
    })
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("hook")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use config_service::ConfigTier;

    fn limits() -> ResourceLimits {
        ResourceLimits::default()
    }

    fn write_hook(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write hook");
    }

    #[test]
    fn global_and_workspace_hooks_load_with_tier_default_scopes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let global = temp.path().join("global");
        let workspace = temp.path().join("ws");
        write_hook(
            &global,
            "hooks/notify.toml",
            r#"
trigger = "run_started"
[handler]
kind = "http"
url = "https://example.com/hook"
"#,
        );
        write_hook(
            &workspace,
            ".pawork/hooks/ws-only.toml",
            r#"
id = "ws-hook"
trigger = "pre_tool_use"
[handler]
kind = "command"
program = "/bin/echo"
"#,
        );
        let resolution = load_hooks(
            Some(&global),
            &[workspace.clone()],
            ".pawork",
            "workspace-1",
            limits(),
        );
        assert_eq!(resolution.hooks.len(), 2, "global + workspace 各一条");
        let global_hook = resolution
            .hooks
            .iter()
            .find(|hook| hook.id == "notify")
            .expect("global hook");
        assert_eq!(global_hook.scope, UserHookScope::Global);
        assert_eq!(global_hook.provenance.tier, ConfigTier::Global);
        let ws_hook = resolution
            .hooks
            .iter()
            .find(|hook| hook.id == "ws-hook")
            .expect("workspace hook");
        assert_eq!(
            ws_hook.scope,
            UserHookScope::Workspace {
                workspace_id: "workspace-1".into()
            },
            "workspace 目录缺省 scope 归属该 workspace"
        );
        assert_eq!(ws_hook.provenance.tier, ConfigTier::Workspace);
    }

    #[test]
    fn workspace_tier_overrides_global_with_same_id_and_records_diagnostic() {
        let temp = tempfile::tempdir().expect("tempdir");
        let global = temp.path().join("global");
        let workspace = temp.path().join("ws");
        let body = r#"
trigger = "run_completed"
[handler]
kind = "command"
program = "/bin/echo"
"#;
        write_hook(&global, "hooks/same.toml", body);
        write_hook(&workspace, ".pawork/hooks/same.toml", body);
        let resolution = load_hooks(
            Some(&global),
            &[workspace],
            ".pawork",
            "workspace-1",
            limits(),
        );
        assert_eq!(resolution.hooks.len(), 1, "同 id 只保留一条有效配置");
        assert_eq!(
            resolution.hooks[0].provenance.tier,
            ConfigTier::Workspace,
            "高 tier（workspace）覆盖低 tier（global）"
        );
        assert!(resolution.diagnostics.entries.iter().any(|entry| {
            entry.kind == ResourceKind::UserHook
                && entry.status == ResourceDiagnosticStatus::Overridden
        }));
    }

    #[test]
    fn explicit_scope_wins_over_tier_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("ws");
        write_hook(
            &workspace,
            ".pawork/hooks/explicit.toml",
            r#"
trigger = "session_start"
[scope]
kind = "global"
[handler]
kind = "http"
url = "https://example.com/global"
"#,
        );
        let resolution = load_hooks(None, &[workspace], ".pawork", "workspace-1", limits());
        assert_eq!(resolution.hooks.len(), 1);
        assert_eq!(
            resolution.hooks[0].scope,
            UserHookScope::Global,
            "文件内显式 scope 优先于 tier 默认"
        );
    }

    #[test]
    fn invalid_hook_is_isolated_and_id_falls_back_to_file_stem() {
        let temp = tempfile::tempdir().expect("tempdir");
        let global = temp.path().join("global");
        write_hook(&global, "hooks/broken.toml", "trigger = [not toml");
        write_hook(
            &global,
            "hooks/no-id.toml",
            r#"
trigger = "run_started"
[handler]
kind = "command"
program = "/bin/true"
"#,
        );
        let resolution = load_hooks(Some(&global), &[], ".pawork", "workspace-1", limits());
        assert_eq!(resolution.hooks.len(), 1, "损坏文件被隔离，不拖垮整批");
        assert_eq!(resolution.hooks[0].id, "no-id", "缺省 id 取文件名 stem");
        let issue = resolution
            .diagnostics
            .issues
            .iter()
            .find(|issue| issue.code == "user_hook_invalid")
            .expect("损坏文件必须产出诊断");
        assert!(!issue.message.contains("not toml"), "诊断不回显原文");
    }
}
