//! CLI 与 GUI 共享的进程内应用服务门面（P13-1）。
//!
//! [`AppService`] 是唯一正式宿主（`pawork`）持有的门面：保留 legacy
//! [`AppService::dispatch`]（[`ServiceRequest`] → [`ServiceResponse`]）的同时，
//! 把真实命令/查询路由委托给统一 [`CommandRouter`]（`dispatch_envelope` /
//! `dispatch_query`），CLI 与 GUI 走同一入口、同一错误协议。

mod aggregate;
mod approval;
mod error;
mod idempotency;
mod rate_limit;
mod router;
mod supervisor;

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use agent_domain::{ActorId, ArtifactId, SessionId, WorkspaceId};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQueryEnvelope, AppResponse,
    AppResponseEnvelope, CommandSource, API_VERSION,
};
use provider_api::ModelProvider;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use aggregate::{
    AggregateError, AggregateState, ApprovalRecord, ApprovalStatus, ArtifactRecord, ProviderRecord,
    RunRecord, SessionRecord, Snapshot, TerminalRecord,
};
pub use approval::{ApprovalError, ApprovalRegistry, PendingApproval, Registration};
pub use error::AppServiceError;
pub use idempotency::{IdempotencyCheck, IdempotencyError, IdempotencyStats, IdempotencyStore};
pub use rate_limit::{DeltaKind, RateLimiter, RateLimiterStats};
pub use router::{source_name, CommandRouter, RouterConfig};
pub use supervisor::{
    CancelOutcome, RunRequest, RunSupervisor, RunSupervisorStats, SuperviseError,
};

use crate::error::now_timestamp;

use artifact_store::{ArtifactStore, BlobId};

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

/// Artifact 流式读取结果（P13-8）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactReadResult {
    /// 聚合记录的 artifact 总字节数。
    pub byte_length: u64,
    /// `[offset, offset + data.len())` 的 payload 分片（`limit == 0` 时读到文件尾）。
    pub data: Vec<u8>,
    /// 本片是否已覆盖文件尾（或 `offset` 已超尾）。
    pub eof: bool,
}

struct State {
    lifecycle: LifecycleState,
    commands_handled: u64,
    sources: BTreeMap<String, u64>,
}

/// 单进程内共享的应用服务门面。CLI 直接持有此对象，不通过 socket 回连自身。
pub struct AppService {
    instance: String,
    started_at: Instant,
    state: Mutex<State>,
    router: CommandRouter,
    artifact_store: Option<Arc<ArtifactStore>>,
}

impl AppService {
    pub fn new(instance: impl Into<String>) -> Self {
        Self::build(instance, None)
    }

    /// 携带内容寻址 Blob Store 构造（P13-8 接线）；`AppService::new` 等价于
    /// store 为 `None`（此时 `artifact_read` 返回 `Unavailable`）。
    pub fn with_artifact_store(instance: impl Into<String>, store: Arc<ArtifactStore>) -> Self {
        Self::build(instance, Some(store))
    }

    fn build(instance: impl Into<String>, artifact_store: Option<Arc<ArtifactStore>>) -> Self {
        let instance = instance.into();
        Self {
            instance: instance.clone(),
            started_at: Instant::now(),
            state: Mutex::new(State {
                lifecycle: LifecycleState::Starting,
                commands_handled: 0,
                sources: BTreeMap::new(),
            }),
            router: CommandRouter::new(RouterConfig {
                instance: instance.clone(),
                ..RouterConfig::default()
            }),
            artifact_store,
        }
    }

    /// legacy 入口：`ServiceOperation` → `ServiceResponse`。
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
            } => self.run_operation(request.source, workspace, prompt, keep_serving),
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

    /// 统一命令入口（CLI 与 GUI 同协议）。
    pub fn dispatch_envelope(&self, envelope: AppCommandEnvelope) -> AppResponseEnvelope {
        self.router.dispatch(envelope)
    }

    /// 统一查询入口。
    pub fn dispatch_query(&self, envelope: AppQueryEnvelope) -> AppResponseEnvelope {
        self.router.dispatch_query(envelope)
    }

    pub fn router(&self) -> &CommandRouter {
        &self.router
    }

    /// 注册 Provider 实现（测试注入 / 正式宿主后续由 provider-runtime 注入）。
    pub fn register_provider(&self, provider: Arc<dyn ModelProvider>) -> agent_domain::ProviderId {
        self.router.register_provider(provider)
    }

    /// 冲刷并取回已限流合并的应用事件。
    pub fn drain_events(&self) -> Vec<core_api::AppEventEnvelope> {
        self.router.drain_events()
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
                detail: "unified command router is available".into(),
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

    /// 按 Artifact ID 流式读取 payload（P13-8）。
    ///
    /// 语义：
    /// - aggregate 无记录 → [`AppServiceError::NotFound`]；
    /// - 未配置 store → [`AppServiceError::Unavailable`]；
    /// - `artifact_id` 非 64-hex（[`BlobId::from_str`] 失败）→ [`AppServiceError::NotFound`]；
    /// - `limit == 0` → 读到文件尾；
    /// - `offset >= byte_length` → 空 `data` + `eof = true`；
    /// - 否则读 `[offset, offset + limit)`，`eof = offset + len >= byte_length`。
    pub async fn artifact_read(
        &self,
        artifact_id: &ArtifactId,
        offset: u64,
        limit: u64,
    ) -> Result<ArtifactReadResult, AppServiceError> {
        let record = self
            .router()
            .aggregate()
            .artifact(artifact_id)
            .ok_or_else(|| AppServiceError::NotFound(format!("artifact {artifact_id}")))?;
        let byte_length = record.byte_length;
        let store = self.artifact_store.as_ref().ok_or_else(|| {
            AppServiceError::Unavailable("artifact store is not configured".into())
        })?;
        let blob_id = BlobId::from_str(artifact_id.as_str())
            .map_err(|_| AppServiceError::NotFound(format!("artifact {artifact_id}")))?;
        if offset >= byte_length {
            return Ok(ArtifactReadResult {
                byte_length,
                data: Vec::new(),
                eof: true,
            });
        }
        // `offset < byte_length` 保证 read_limit >= 1，不会触发 EmptyRange。
        let read_limit = if limit == 0 {
            byte_length - offset
        } else {
            limit.min(byte_length - offset)
        };
        let data = store.read_range(&blob_id, offset, read_limit).await?;
        let eof = offset + data.len() as u64 >= byte_length;
        Ok(ArtifactReadResult {
            byte_length,
            data,
            eof,
        })
    }

    fn run_operation(
        &self,
        source: CommandSource,
        workspace: Option<String>,
        prompt: Option<String>,
        keep_serving: bool,
    ) -> ServiceResponse {
        let Some(prompt) = prompt else {
            return failed_response(
                "run",
                "run command requires a prompt",
                json!({ "implementation_phase": "P13-1" }),
            );
        };
        let prompt_present = !prompt.trim().is_empty();
        if !prompt_present {
            return failed_response(
                "run",
                "run command requires a non-empty prompt",
                json!({ "implementation_phase": "P13-1" }),
            );
        }

        // 1) 解析 workspace：指定路径时优先复用已有 workspace，否则新建。
        let workspace_id = match workspace {
            Some(path) => match self.find_workspace_by_root(&path) {
                Some(workspace) => workspace.id,
                None => match self.add_workspace(&source, &path) {
                    Ok(id) => id,
                    Err(response) => return response,
                },
            },
            None => {
                let default_id = WorkspaceId::from("default");
                if self.router.aggregate().workspace(&default_id).is_none() {
                    let cwd = std::env::current_dir()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| ".".into());
                    match self.add_workspace(&source, &cwd) {
                        Ok(id) => id,
                        Err(response) => return response,
                    }
                } else {
                    default_id
                }
            }
        };

        // 2) 创建会话。
        let session_id = match self.router.dispatch(self.envelope(
            &source,
            AppCommand::SessionCreate {
                workspace_id: workspace_id.clone(),
                title: Some("CLI run".into()),
            },
        )) {
            AppResponseEnvelope {
                response: AppResponse::Data(value),
                ..
            } => SessionId::from(
                value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            AppResponseEnvelope {
                response: AppResponse::Error(context),
                ..
            } => {
                return failed_response(
                    "run",
                    &context.message,
                    json!({ "workspace_id": workspace_id }),
                );
            }
            other => {
                return failed_response(
                    "run",
                    &format!("unexpected session create response: {other:?}"),
                    json!({ "workspace_id": workspace_id }),
                );
            }
        };

        // 3) 启动 Run（无凭据/未注册 Provider 时返回结构化错误，不 panic）。
        match self.router.dispatch(self.envelope(
            &source,
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: prompt,
                model: None,
            },
        )) {
            AppResponseEnvelope {
                response: AppResponse::Accepted { .. },
                ..
            } => ServiceResponse {
                ok: true,
                kind: "run".into(),
                message: "run command accepted by the in-process app-service".into(),
                data: json!({
                    "workspace_id": workspace_id,
                    "session_id": session_id,
                    "run_id": self.router.last_started_run(),
                    "prompt_present": true,
                    "keep_serving": keep_serving,
                    "implementation_phase": "P13-1"
                }),
            },
            AppResponseEnvelope {
                response: AppResponse::Error(context),
                ..
            } => failed_response(
                "run",
                &context.message,
                json!({
                    "workspace_id": workspace_id,
                    "session_id": session_id,
                    "error": context,
                }),
            ),
            other => failed_response(
                "run",
                &format!("unexpected run start response: {other:?}"),
                json!({ "workspace_id": workspace_id, "session_id": session_id }),
            ),
        }
    }

    fn add_workspace(
        &self,
        source: &CommandSource,
        path: &str,
    ) -> Result<WorkspaceId, ServiceResponse> {
        match self.router.dispatch(self.envelope(
            source,
            AppCommand::WorkspaceAdd {
                root_path: path.to_string(),
            },
        )) {
            AppResponseEnvelope {
                response: AppResponse::Data(value),
                ..
            } => Ok(WorkspaceId::from(
                value.get("id").and_then(Value::as_str).unwrap_or_default(),
            )),
            AppResponseEnvelope {
                response: AppResponse::Error(context),
                ..
            } => Err(failed_response(
                "run",
                &context.message,
                json!({ "workspace_path": path }),
            )),
            other => Err(failed_response(
                "run",
                &format!("unexpected workspace add response: {other:?}"),
                json!({ "workspace_path": path }),
            )),
        }
    }

    fn find_workspace_by_root(&self, path: &str) -> Option<workspace_service::Workspace> {
        let canonical = std::fs::canonicalize(path).ok();
        self.router
            .aggregate()
            .workspace_list()
            .into_iter()
            .find(|workspace| {
                workspace.roots.iter().any(|root| {
                    root.path.to_string_lossy() == path
                        || canonical
                            .as_ref()
                            .is_some_and(|canonical| canonical == &root.path)
                })
            })
    }

    fn envelope(&self, source: &CommandSource, command: AppCommand) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: agent_domain::CommandId::from(self.router.aggregate().next_id("cmd")),
            source: source.clone(),
            identity: ActorIdentity::LocalUser {
                actor_id: ActorId::from("local-cli"),
                display_name: None,
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: now_timestamp(),
            command,
        }
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

fn failed_response(kind: &str, message: &str, data: Value) -> ServiceResponse {
    ServiceResponse {
        ok: false,
        kind: kind.into(),
        message: message.into(),
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::Timestamp;
    use core_api::AppQuery;

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

    #[test]
    fn unified_dispatch_handles_commands_and_queries() {
        let service = AppService::new("test");
        let source = CommandSource::Automation;
        let identity = ActorIdentity::Automation {
            name: "test".into(),
        };
        let query = service.dispatch_query(AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: agent_domain::QueryId::from("q-1"),
            source: source.clone(),
            identity: identity.clone(),
            issued_at: Timestamp::from_unix_millis(1),
            query: AppQuery::WorkspaceList,
        });
        assert!(matches!(query.response, AppResponse::Data(_)));
        assert_eq!(service.router().source_stats().get("automation"), Some(&1));
    }
}
