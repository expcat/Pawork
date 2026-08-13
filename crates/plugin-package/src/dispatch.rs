//! 子资源分发（P17-2）。
//!
//! 解包并校验后，把六类子资源按固定顺序分发到 [`PackageDispatchSink`]——由宿主
//! 实现为对各既有 loader 的调用（`resource-loader` 加载 skills/agents/hooks/lsp、
//! `mcp-client` 用 sandboxed stdio spawner 托管 MCP stdio、`monitor-service` 注册
//! monitor）。本 crate **不复制运行时**：只产出分发描述符，注册由 sink（= 各 loader）
//! 承担。
//!
//! 分发顺序固定（skills → agents → hooks → mcp → lsp → monitors），遇首个错误即
//! 停止；事务化回滚由 marketplace（P17-3）在 host 侧实现。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::archive::PackageArchive;
use crate::error::PackageError;
use crate::manifest::{McpServerDeclaration, ResourceRef};
use crate::monitor::MonitorDeclaration;
use crate::scope::{PackageProvenance, PackageRelativePath};

/// 分发来源记录。
pub type DispatchProvenance = PackageProvenance;

/// Skill 分发：路径引用（skill 目录由 resource-loader 加载）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDispatch {
    pub provenance: DispatchProvenance,
    pub path: PackageRelativePath,
}

/// Agent profile 分发：路径或内联。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfileDispatch {
    pub provenance: DispatchProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PackageRelativePath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<Value>,
}

/// Hook 分发：内联或路径。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookDispatch {
    pub provenance: DispatchProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PackageRelativePath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<Value>,
}

/// MCP server 分发。`sandboxed` 对 stdio server 恒为 true——package 触达的本地
/// stdio 一律经 Sandbox → Process Runtime 托管（restart 不降级）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpDispatch {
    pub provenance: DispatchProvenance,
    pub server: McpServerDeclaration,
    pub sandboxed: bool,
}

/// LSP 分发：内联或路径。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageServerDispatch {
    pub provenance: DispatchProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PackageRelativePath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<Value>,
}

/// Monitor 分发：稳定 driver 入口 + 唯一 lifecycle（task-manager）。宿主据此构造
/// `monitor_service::Monitor` 并注册。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorDispatch {
    pub provenance: DispatchProvenance,
    pub declaration: MonitorDeclaration,
}

/// 已规划的分发集合。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchPlan {
    pub skills: Vec<SkillDispatch>,
    pub agents: Vec<AgentProfileDispatch>,
    pub hooks: Vec<HookDispatch>,
    pub mcp: Vec<McpDispatch>,
    pub lsp: Vec<LanguageServerDispatch>,
    pub monitors: Vec<MonitorDispatch>,
}

impl DispatchPlan {
    /// 由已校验的归档 manifest 构造分发计划。
    pub fn from_archive(archive: &PackageArchive) -> Self {
        let manifest = &archive.manifest;
        let provenance = PackageProvenance::new(
            manifest.id.clone(),
            manifest.version.clone(),
            manifest.scope.clone(),
        );
        let mut plan = Self::default();
        for resource in &manifest.skills {
            if let ResourceRef::Path { path } = resource {
                plan.skills.push(SkillDispatch {
                    provenance: provenance.clone(),
                    path: path.clone(),
                });
            }
        }
        plan.agents = manifest
            .agents
            .iter()
            .map(|resource| AgentProfileDispatch {
                provenance: provenance.clone(),
                path: resource.path().cloned(),
                inline: resource.inline().cloned(),
            })
            .collect();
        plan.hooks = manifest
            .hooks
            .iter()
            .map(|resource| HookDispatch {
                provenance: provenance.clone(),
                path: resource.path().cloned(),
                inline: resource.inline().cloned(),
            })
            .collect();
        plan.mcp = manifest
            .mcp
            .iter()
            .map(|server| {
                let sandboxed = server.is_stdio();
                McpDispatch {
                    provenance: provenance.clone(),
                    server: server.clone(),
                    sandboxed,
                }
            })
            .collect();
        plan.lsp = manifest
            .lsp
            .iter()
            .map(|resource| LanguageServerDispatch {
                provenance: provenance.clone(),
                path: resource.path().cloned(),
                inline: resource.inline().cloned(),
            })
            .collect();
        plan.monitors = manifest
            .monitors
            .iter()
            .map(|declaration| MonitorDispatch {
                provenance: provenance.clone(),
                declaration: declaration.clone(),
            })
            .collect();
        plan
    }

    pub fn total(&self) -> usize {
        self.skills.len()
            + self.agents.len()
            + self.hooks.len()
            + self.mcp.len()
            + self.lsp.len()
            + self.monitors.len()
    }
}

/// 分发 sink：宿主实现为对各既有 loader 的调用。本 crate 提供
/// [`RecordingDispatchSink`] 用于定向测试。
pub trait PackageDispatchSink {
    fn install_skill(&mut self, dispatch: &SkillDispatch) -> Result<(), PackageError>;
    fn install_agent_profile(
        &mut self,
        dispatch: &AgentProfileDispatch,
    ) -> Result<(), PackageError>;
    fn install_hook(&mut self, dispatch: &HookDispatch) -> Result<(), PackageError>;
    fn install_mcp_server(&mut self, dispatch: &McpDispatch) -> Result<(), PackageError>;
    fn install_language_server(
        &mut self,
        dispatch: &LanguageServerDispatch,
    ) -> Result<(), PackageError>;
    fn install_monitor(&mut self, dispatch: &MonitorDispatch) -> Result<(), PackageError>;
}

/// 分发结果摘要（每类计数）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchSummary {
    pub skills: usize,
    pub agents: usize,
    pub hooks: usize,
    pub mcp: usize,
    pub lsp: usize,
    pub monitors: usize,
}

/// 按 skills → agents → hooks → mcp → lsp → monitors 固定顺序分发到 sink。
/// 遇首个错误即停止并返回 [`PackageError::Dispatch`]；事务化回滚由宿主负责。
pub fn install_package(
    archive: &PackageArchive,
    sink: &mut dyn PackageDispatchSink,
) -> Result<DispatchSummary, PackageError> {
    let plan = DispatchPlan::from_archive(archive);
    for dispatch in &plan.skills {
        sink.install_skill(dispatch)
            .map_err(|error| dispatch_error("skills", dispatch.path.to_posix_string(), error))?;
    }
    for dispatch in &plan.agents {
        sink.install_agent_profile(dispatch)
            .map_err(|error| dispatch_error("agents", agent_key(dispatch), error))?;
    }
    for dispatch in &plan.hooks {
        sink.install_hook(dispatch)
            .map_err(|error| dispatch_error("hooks", hook_key(dispatch), error))?;
    }
    for dispatch in &plan.mcp {
        sink.install_mcp_server(dispatch)
            .map_err(|error| dispatch_error("mcp", &dispatch.server.name, error))?;
    }
    for dispatch in &plan.lsp {
        sink.install_language_server(dispatch)
            .map_err(|error| dispatch_error("lsp", lsp_key(dispatch), error))?;
    }
    for dispatch in &plan.monitors {
        sink.install_monitor(dispatch).map_err(|error| {
            dispatch_error("monitors", dispatch.declaration.monitor_id.as_str(), error)
        })?;
    }
    Ok(DispatchSummary {
        skills: plan.skills.len(),
        agents: plan.agents.len(),
        hooks: plan.hooks.len(),
        mcp: plan.mcp.len(),
        lsp: plan.lsp.len(),
        monitors: plan.monitors.len(),
    })
}

fn dispatch_error(
    sink: &'static str,
    resource: impl Into<String>,
    error: PackageError,
) -> PackageError {
    PackageError::Dispatch {
        sink,
        resource: resource.into(),
        message: error.to_string(),
    }
}

fn agent_key(dispatch: &AgentProfileDispatch) -> String {
    dispatch
        .path
        .as_ref()
        .map(|path| path.to_posix_string())
        .or_else(|| {
            dispatch
                .inline
                .as_ref()
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .map(|name| format!("inline:{name}"))
        })
        .unwrap_or_else(|| "<agent>".into())
}

fn hook_key(dispatch: &HookDispatch) -> String {
    dispatch
        .path
        .as_ref()
        .map(|path| path.to_posix_string())
        .or_else(|| {
            dispatch
                .inline
                .as_ref()
                .and_then(|value| value.get("trigger"))
                .and_then(Value::as_str)
                .map(String::from)
        })
        .unwrap_or_else(|| "<hook>".into())
}

fn lsp_key(dispatch: &LanguageServerDispatch) -> String {
    dispatch
        .path
        .as_ref()
        .map(|path| path.to_posix_string())
        .or_else(|| {
            dispatch
                .inline
                .as_ref()
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .map(String::from)
        })
        .unwrap_or_else(|| "<lsp>".into())
}

/// Mock sink：记录每次分发的类别 + 稳定键，用于定向测试「子资源分发到正确 loader」。
#[derive(Default)]
pub struct RecordingDispatchSink {
    pub skills: Vec<String>,
    pub agents: Vec<String>,
    pub hooks: Vec<String>,
    pub mcp: Vec<(String, bool)>,
    pub lsp: Vec<String>,
    pub monitors: Vec<String>,
}

impl RecordingDispatchSink {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PackageDispatchSink for RecordingDispatchSink {
    fn install_skill(&mut self, dispatch: &SkillDispatch) -> Result<(), PackageError> {
        self.skills.push(dispatch.path.to_posix_string());
        Ok(())
    }
    fn install_agent_profile(
        &mut self,
        dispatch: &AgentProfileDispatch,
    ) -> Result<(), PackageError> {
        self.agents.push(agent_key(dispatch));
        Ok(())
    }
    fn install_hook(&mut self, dispatch: &HookDispatch) -> Result<(), PackageError> {
        self.hooks.push(hook_key(dispatch));
        Ok(())
    }
    fn install_mcp_server(&mut self, dispatch: &McpDispatch) -> Result<(), PackageError> {
        self.mcp
            .push((dispatch.server.name.clone(), dispatch.sandboxed));
        Ok(())
    }
    fn install_language_server(
        &mut self,
        dispatch: &LanguageServerDispatch,
    ) -> Result<(), PackageError> {
        self.lsp.push(lsp_key(dispatch));
        Ok(())
    }
    fn install_monitor(&mut self, dispatch: &MonitorDispatch) -> Result<(), PackageError> {
        self.monitors
            .push(dispatch.declaration.monitor_id.as_str().to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{read_archive, write_archive};
    use crate::manifest::PackageManifest;
    use crate::manifest::{McpServerDeclaration, McpTransportSpec, ResourceRef};
    use crate::monitor::{MonitorDeclaration, MonitorDriverEntry, MonitorLifecycle};
    use crate::scope::{PackageId, PackageRelativePath, PackageScope};
    use agent_domain::MonitorId;
    use semver::Version;
    use serde_json::json;
    use std::fs;

    fn build_archive(root: &std::path::Path) -> PackageArchive {
        let skill_dir = root.join("skills/search");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("manifest.toml"),
            "id='search'\nversion='1.0.0'",
        )
        .unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Search").unwrap();
        fs::create_dir_all(root.join("lsp")).unwrap();
        fs::write(
            root.join("lsp/rust.toml"),
            "id='rust-analyzer'\ncommand='rust-analyzer'\nlanguage='rust'\n",
        )
        .unwrap();
        let manifest = PackageManifest {
            manifest_version: crate::PACKAGE_MANIFEST_VERSION,
            id: PackageId::new("acme.toolkit").unwrap(),
            name: "ACME".into(),
            version: Version::new(1, 2, 0),
            license: Some("MIT".into()),
            description: None,
            entrypoint: None,
            scope: PackageScope::Global,
            dependencies: Vec::new(),
            skills: vec![ResourceRef::Path {
                path: PackageRelativePath::new("skills/search").unwrap(),
            }],
            agents: vec![ResourceRef::Inline {
                manifest: json!({"name": "acme-default", "instructions": "be helpful"}),
            }],
            hooks: vec![ResourceRef::Inline {
                manifest: json!({"id": "notify", "trigger": "run_started", "scope": {"kind": "global"}, "handler": {"kind": "command", "program": "/bin/true"}}),
            }],
            mcp: vec![
                McpServerDeclaration {
                    name: "fs".into(),
                    transport: McpTransportSpec::Stdio {
                        command: "npx".into(),
                        args: vec!["-y".into()],
                        env: Default::default(),
                    },
                    auto_start: true,
                },
                McpServerDeclaration {
                    name: "remote".into(),
                    transport: McpTransportSpec::Http {
                        url: "https://example.com/mcp".into(),
                        headers: Default::default(),
                    },
                    auto_start: false,
                },
            ],
            lsp: vec![ResourceRef::Path {
                path: PackageRelativePath::new("lsp/rust.toml").unwrap(),
            }],
            monitors: vec![{
                let mut decl = MonitorDeclaration::new(
                    MonitorId::new("watch-build"),
                    MonitorDriverEntry::new("monitor_service.evaluate"),
                    MonitorLifecycle::TaskManager,
                );
                decl.config = json!({"kind": "file_change", "paths": ["target/debug/app"]});
                decl.source = agent_domain::MonitorSourceKind::FileChange;
                decl
            }],
        };
        write_archive(root, &manifest).expect("write");
        read_archive(root).expect("read")
    }

    #[test]
    fn dispatches_six_types_to_correct_sink_with_correct_scope() {
        let temp = tempfile::tempdir().unwrap();
        let archive = build_archive(temp.path());
        let mut sink = RecordingDispatchSink::new();
        let summary = install_package(&archive, &mut sink).expect("install");

        // 每类分发到对应 loader（recording 字段）。
        assert_eq!(summary.skills, 1);
        assert_eq!(summary.agents, 1);
        assert_eq!(summary.hooks, 1);
        assert_eq!(summary.mcp, 2);
        assert_eq!(summary.lsp, 1);
        assert_eq!(summary.monitors, 1);
        assert_eq!(sink.skills, vec!["skills/search".to_string()]);
        assert_eq!(sink.agents, vec!["inline:acme-default".to_string()]);
        assert_eq!(sink.hooks, vec!["run_started".to_string()]);
        assert_eq!(sink.lsp, vec!["lsp/rust.toml".to_string()]);
        assert_eq!(sink.monitors, vec!["watch-build".to_string()]);

        // stdio server 必须 sandboxed，http 不强制。
        let fs_server = sink.mcp.iter().find(|(name, _)| name == "fs").unwrap();
        assert!(
            fs_server.1,
            "stdio MCP server must be dispatched as sandboxed"
        );
        let remote = sink.mcp.iter().find(|(name, _)| name == "remote").unwrap();
        assert!(!remote.1);

        // 作用域与 resource-loader 一致（global）。
        let plan = DispatchPlan::from_archive(&archive);
        assert_eq!(plan.skills[0].provenance.scope, PackageScope::Global);
    }

    #[test]
    fn install_stops_on_first_sink_error() {
        let temp = tempfile::tempdir().unwrap();
        let archive = build_archive(temp.path());
        struct FailingSink;
        impl PackageDispatchSink for FailingSink {
            fn install_skill(&mut self, _: &SkillDispatch) -> Result<(), PackageError> {
                Err(PackageError::Conflict("boom".into()))
            }
            fn install_agent_profile(
                &mut self,
                _: &AgentProfileDispatch,
            ) -> Result<(), PackageError> {
                Ok(())
            }
            fn install_hook(&mut self, _: &HookDispatch) -> Result<(), PackageError> {
                Ok(())
            }
            fn install_mcp_server(&mut self, _: &McpDispatch) -> Result<(), PackageError> {
                Ok(())
            }
            fn install_language_server(
                &mut self,
                _: &LanguageServerDispatch,
            ) -> Result<(), PackageError> {
                Ok(())
            }
            fn install_monitor(&mut self, _: &MonitorDispatch) -> Result<(), PackageError> {
                Ok(())
            }
        }
        let err = install_package(&archive, &mut FailingSink).unwrap_err();
        assert!(matches!(err, PackageError::Dispatch { sink: "skills", .. }));
    }
}
