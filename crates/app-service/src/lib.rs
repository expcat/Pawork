//! CLI 与未来 GUI 共享的进程内应用服务入口。

use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard},
    time::Instant,
};

use core_api::CommandSource;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Starting,
    Ready,
    ShuttingDown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub version: String,
    pub instance: String,
    pub process_id: u32,
    pub lifecycle: LifecycleState,
    pub uptime_millis: u64,
    pub commands_handled: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServiceOperation {
    Serve,
    Run {
        workspace: Option<String>,
        prompt: Option<String>,
        keep_serving: bool,
    },
    Shell,
    Watch,
    Status,
    Shutdown,
    Doctor,
    Placeholder {
        command: String,
        arguments: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServiceRequest {
    pub source: CommandSource,
    pub operation: ServiceOperation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceResponse {
    pub ok: bool,
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

struct State {
    lifecycle: LifecycleState,
    commands_handled: u64,
    sources: BTreeMap<String, u64>,
}

/// 单进程内共享的应用服务。CLI 直接持有此对象，不通过 socket 回连自身。
pub struct AppService {
    instance: String,
    started_at: Instant,
    state: Mutex<State>,
}

impl AppService {
    pub fn new(instance: impl Into<String>) -> Self {
        Self {
            instance: instance.into(),
            started_at: Instant::now(),
            state: Mutex::new(State {
                lifecycle: LifecycleState::Starting,
                commands_handled: 0,
                sources: BTreeMap::new(),
            }),
        }
    }

    pub fn dispatch(&self, request: ServiceRequest) -> ServiceResponse {
        let source = source_name(&request.source);
        {
            let mut state = self.state();
            state.commands_handled = state.commands_handled.saturating_add(1);
            *state.sources.entry(source.to_owned()).or_default() += 1;
        }

        match request.operation {
            ServiceOperation::Serve => {
                self.state().lifecycle = LifecycleState::Ready;
                ServiceResponse {
                    ok: true,
                    kind: "serve".into(),
                    message: format!("Pawork Core instance '{}' is ready", self.instance),
                    data: serde_json::to_value(self.status()).expect("status is serializable"),
                }
            }
            ServiceOperation::Run {
                workspace,
                prompt,
                keep_serving,
            } => {
                self.state().lifecycle = LifecycleState::Ready;
                ServiceResponse {
                    ok: true,
                    kind: "run".into(),
                    message: "run command accepted by the in-process app-service skeleton".into(),
                    data: json!({
                        "workspace": workspace,
                        "prompt_present": prompt.is_some(),
                        "keep_serving": keep_serving,
                        "implementation_phase": "P3"
                    }),
                }
            }
            ServiceOperation::Shell => response("shell", "interactive shell is ready"),
            ServiceOperation::Watch => response("watch", "event watch route is ready"),
            ServiceOperation::Status => ServiceResponse {
                ok: true,
                kind: "status".into(),
                message: "Core status".into(),
                data: serde_json::to_value(self.status()).expect("status is serializable"),
            },
            ServiceOperation::Shutdown => {
                self.state().lifecycle = LifecycleState::ShuttingDown;
                response("shutdown", "Core shutdown requested")
            }
            ServiceOperation::Doctor => {
                let report = self.doctor();
                ServiceResponse {
                    ok: report.ok,
                    kind: "doctor".into(),
                    message: if report.ok {
                        "all available host checks passed".into()
                    } else {
                        "one or more checks failed".into()
                    },
                    data: serde_json::to_value(report).expect("doctor report is serializable"),
                }
            }
            ServiceOperation::Placeholder { command, arguments } => ServiceResponse {
                ok: true,
                kind: command.clone(),
                message: format!("'{command}' command route is available"),
                data: json!({ "arguments": arguments, "implementation_phase": "later" }),
            },
        }
    }

    pub fn status(&self) -> ServiceStatus {
        let state = self.state();
        ServiceStatus {
            version: env!("CARGO_PKG_VERSION").into(),
            instance: self.instance.clone(),
            process_id: std::process::id(),
            lifecycle: state.lifecycle.clone(),
            uptime_millis: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            commands_handled: state.commands_handled,
        }
    }

    pub fn doctor(&self) -> DoctorReport {
        let project_directories = directories::ProjectDirs::from("", "", "Pawork");
        let current_directory = std::env::current_dir();
        let checks = vec![
            DoctorCheck {
                name: "host".into(),
                ok: true,
                detail: "CLI and Core share the pawork process".into(),
            },
            DoctorCheck {
                name: "app_service".into(),
                ok: true,
                detail: "direct in-process command router is available".into(),
            },
            DoctorCheck {
                name: "runtime".into(),
                ok: true,
                detail: format!("process {} is running", std::process::id()),
            },
            DoctorCheck {
                name: "project_directories".into(),
                ok: project_directories.is_some(),
                detail: project_directories.map_or_else(
                    || "platform config/data directories are unavailable".into(),
                    |paths| {
                        format!(
                            "config={} data={}",
                            paths.config_dir().display(),
                            paths.data_dir().display()
                        )
                    },
                ),
            },
            DoctorCheck {
                name: "current_directory".into(),
                ok: current_directory.is_ok(),
                detail: current_directory.map_or_else(
                    |error| format!("current directory unavailable: {error}"),
                    |path| path.display().to_string(),
                ),
            },
        ];
        DoctorReport {
            ok: checks.iter().all(|check| check.ok),
            checks,
        }
    }

    pub fn source_count(&self, source: &str) -> u64 {
        self.state()
            .sources
            .get(source)
            .copied()
            .unwrap_or_default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn response(kind: &str, message: &str) -> ServiceResponse {
    ServiceResponse {
        ok: true,
        kind: kind.into(),
        message: message.into(),
        data: Value::Null,
    }
}

fn source_name(source: &CommandSource) -> &'static str {
    match source {
        CommandSource::LocalCli { .. } => "local_cli",
        CommandSource::LocalGui { .. } => "local_gui",
        CommandSource::RemoteGui { .. } => "remote_gui",
        CommandSource::Automation => "automation",
        CommandSource::Plugin => "plugin",
        CommandSource::Mcp => "mcp",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_cli_requests_in_process_and_tracks_source() {
        let service = AppService::new("test");
        let response = service.dispatch(ServiceRequest {
            source: CommandSource::LocalCli {
                terminal_session_id: Some("terminal-1".into()),
            },
            operation: ServiceOperation::Status,
        });
        assert!(response.ok);
        assert_eq!(service.source_count("local_cli"), 1);
        assert_eq!(service.status().commands_handled, 1);
    }

    #[test]
    fn doctor_reports_same_process_host_and_router() {
        let service = AppService::new("test");
        let report = service.doctor();
        assert!(report.ok);
        assert_eq!(report.checks.len(), 5);
        assert!(report.checks.iter().all(|check| check.ok));
    }
}
