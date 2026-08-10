//! 轻量内存状态聚合（P13-1）。
//!
//! 聚合 workspace / session / run / approval / provider 以及 diff、artifact、
//! terminal、GUI client 等查询面数据，支撑 `AppQuery` 与 `SnapshotFetch`。
//! 纯内存、线程安全；持久化与事件重放在后续 Phase（P13-2/P19）接入。

use std::collections::BTreeMap;
use std::sync::RwLock;

use agent_domain::{
    ArtifactId, ConnectionId, EventId, GuiClientId, ProviderId, RunId, SessionId,
    TerminalSessionId, Timestamp, ToolCallId, WorkspaceId,
};
use core_api::CommandSource;
use diff_service::DiffFile;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use workspace_service::Workspace;

use crate::error::now_timestamp;

#[derive(Debug, Error)]
pub enum AggregateError {
    #[error("workspace not found: {0}")]
    WorkspaceNotFound(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("run not found: {0}")]
    RunNotFound(String),
    #[error("provider not found: {0}")]
    ProviderNotFound(String),
    #[error("artifact already exists: {0}")]
    ArtifactExists(String),
    #[error("aggregate state poisoned")]
    Poisoned,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: SessionId,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub created_at: Timestamp,
    pub revision: u64,
    pub open: bool,
    pub compacted: bool,
    pub run_count: u64,
    pub message_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<EventId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub model: agent_domain::ModelId,
    pub provider_id: ProviderId,
    pub source: CommandSource,
    pub state: core_api::RunState,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    pub message_count: u64,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Decided(core_api::ApprovalDecision),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub run_id: RunId,
    pub tool_call_id: ToolCallId,
    pub reason: String,
    pub status: ApprovalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderRecord {
    pub provider_id: ProviderId,
    pub status: core_api::ProviderStatus,
    pub authenticated: bool,
    pub model_count: usize,
    pub registered_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_flow: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub artifact_id: ArtifactId,
    pub media_type: String,
    pub byte_length: u64,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuiClientRecord {
    pub client_id: GuiClientId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<ConnectionId>,
    pub connected: bool,
    pub last_seen_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerminalRecord {
    pub terminal_session_id: TerminalSessionId,
    pub workspace_id: WorkspaceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    pub output_bytes: u64,
    pub columns: u16,
    pub rows: u16,
}

/// SnapshotFetch 的完整快照数据。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub revision: u64,
    pub core_ready: bool,
    pub workspaces: Vec<Workspace>,
    pub sessions: Vec<SessionRecord>,
    pub runs: Vec<RunRecord>,
    pub approvals: Vec<ApprovalRecord>,
    pub providers: Vec<ProviderRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub gui_clients: Vec<GuiClientRecord>,
    pub terminals: Vec<TerminalRecord>,
}

struct Inner {
    revision: u64,
    core_ready: bool,
    next_id: u64,
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    sessions: BTreeMap<SessionId, SessionRecord>,
    runs: BTreeMap<RunId, RunRecord>,
    approvals: BTreeMap<ToolCallId, ApprovalRecord>,
    providers: BTreeMap<ProviderId, ProviderRecord>,
    artifacts: BTreeMap<ArtifactId, ArtifactRecord>,
    gui_clients: BTreeMap<GuiClientId, GuiClientRecord>,
    terminals: BTreeMap<TerminalSessionId, TerminalRecord>,
    diffs: BTreeMap<WorkspaceId, Vec<DiffFile>>,
    git_stages: BTreeMap<WorkspaceId, Vec<String>>,
}

/// 轻量内存聚合，支撑 AppQuery 与 Snapshot。
pub struct AggregateState {
    inner: RwLock<Inner>,
}

impl AggregateState {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                revision: 0,
                core_ready: false,
                next_id: 0,
                workspaces: BTreeMap::new(),
                sessions: BTreeMap::new(),
                runs: BTreeMap::new(),
                approvals: BTreeMap::new(),
                providers: BTreeMap::new(),
                artifacts: BTreeMap::new(),
                gui_clients: BTreeMap::new(),
                terminals: BTreeMap::new(),
                diffs: BTreeMap::new(),
                git_stages: BTreeMap::new(),
            }),
        }
    }

    /// 生成顺序递增的领域 ID（`<prefix>-<n>`）。
    pub fn next_id(&self, prefix: &str) -> String {
        let mut inner = write(&self.inner);
        next_id_locked(&mut inner, prefix)
    }

    pub fn mark_core_ready(&self) {
        let mut inner = write(&self.inner);
        inner.core_ready = true;
        inner.revision += 1;
    }

    // ---------- workspace ----------

    pub fn record_workspace(&self, workspace: Workspace) {
        let mut inner = write(&self.inner);
        inner.workspaces.insert(workspace.id.clone(), workspace);
        inner.revision += 1;
    }

    pub fn workspace(&self, workspace_id: &WorkspaceId) -> Option<Workspace> {
        read(&self.inner).workspaces.get(workspace_id).cloned()
    }

    pub fn workspace_list(&self) -> Vec<Workspace> {
        read(&self.inner).workspaces.values().cloned().collect()
    }

    // ---------- session ----------

    pub fn create_session(
        &self,
        workspace_id: WorkspaceId,
        title: String,
        now: Timestamp,
    ) -> Result<SessionRecord, AggregateError> {
        let mut inner = write(&self.inner);
        if !inner.workspaces.contains_key(&workspace_id) {
            return Err(AggregateError::WorkspaceNotFound(workspace_id.to_string()));
        }
        let session_id = SessionId::from(next_id_locked(&mut inner, "session"));
        let record = SessionRecord {
            session_id: session_id.clone(),
            workspace_id,
            title: if title.trim().is_empty() {
                "Untitled".into()
            } else {
                title
            },
            created_at: now,
            revision: 1,
            open: true,
            compacted: false,
            run_count: 0,
            message_count: 0,
            forked_from: None,
            parent_event_id: None,
        };
        inner.sessions.insert(session_id, record.clone());
        inner.revision += 1;
        Ok(record)
    }

    pub fn get_session(&self, session_id: &SessionId) -> Option<SessionRecord> {
        read(&self.inner).sessions.get(session_id).cloned()
    }

    pub fn session_exists(&self, session_id: &SessionId) -> bool {
        read(&self.inner).sessions.contains_key(session_id)
    }

    pub fn open_session(&self, session_id: &SessionId) -> Result<SessionRecord, AggregateError> {
        let mut inner = write(&self.inner);
        let record = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AggregateError::SessionNotFound(session_id.to_string()))?;
        record.open = true;
        record.revision += 1;
        let record = record.clone();
        inner.revision += 1;
        Ok(record)
    }

    pub fn compact_session(&self, session_id: &SessionId) -> Result<SessionRecord, AggregateError> {
        let mut inner = write(&self.inner);
        let record = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AggregateError::SessionNotFound(session_id.to_string()))?;
        record.compacted = true;
        record.revision += 1;
        let record = record.clone();
        inner.revision += 1;
        Ok(record)
    }

    pub fn fork_session(
        &self,
        session_id: &SessionId,
        parent_event_id: EventId,
    ) -> Result<SessionRecord, AggregateError> {
        let mut inner = write(&self.inner);
        let parent = inner
            .sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| AggregateError::SessionNotFound(session_id.to_string()))?;
        let child_id = SessionId::from(next_id_locked(&mut inner, "session"));
        let record = SessionRecord {
            session_id: child_id.clone(),
            workspace_id: parent.workspace_id.clone(),
            title: format!("{} (fork)", parent.title),
            created_at: now_timestamp(),
            revision: 1,
            open: true,
            compacted: false,
            run_count: 0,
            message_count: 0,
            forked_from: Some(session_id.clone()),
            parent_event_id: Some(parent_event_id),
        };
        inner.sessions.insert(child_id, record.clone());
        inner.revision += 1;
        Ok(record)
    }

    // ---------- run ----------

    pub fn record_run(
        &self,
        run_id: RunId,
        session_id: SessionId,
        model: agent_domain::ModelId,
        provider_id: ProviderId,
        source: CommandSource,
        now: Timestamp,
    ) -> Result<RunRecord, AggregateError> {
        let mut inner = write(&self.inner);
        let session = inner
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| AggregateError::SessionNotFound(session_id.to_string()))?;
        session.run_count += 1;
        session.revision += 1;
        let record = RunRecord {
            run_id: run_id.clone(),
            session_id,
            model,
            provider_id,
            source,
            state: core_api::RunState::Created,
            created_at: now,
            started_at: None,
            message_count: 0,
            revision: 1,
        };
        inner.runs.insert(run_id, record.clone());
        inner.revision += 1;
        Ok(record)
    }

    pub fn set_run_state(
        &self,
        run_id: &RunId,
        state: core_api::RunState,
    ) -> Result<(), AggregateError> {
        let mut inner = write(&self.inner);
        let record = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| AggregateError::RunNotFound(run_id.to_string()))?;
        if record.started_at.is_none() && state != core_api::RunState::Created {
            record.started_at = Some(now_timestamp());
        }
        record.state = state;
        record.revision += 1;
        inner.revision += 1;
        Ok(())
    }

    pub fn add_message(&self, run_id: &RunId) -> Result<(), AggregateError> {
        let mut inner = write(&self.inner);
        let record = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| AggregateError::RunNotFound(run_id.to_string()))?;
        record.message_count += 1;
        record.revision += 1;
        inner.revision += 1;
        Ok(())
    }

    pub fn get_run(&self, run_id: &RunId) -> Option<RunRecord> {
        read(&self.inner).runs.get(run_id).cloned()
    }

    /// 移除 run 记录（启动失败回滚用），并同步回退 session 的 run 计数。
    pub fn remove_run(&self, run_id: &RunId) {
        let mut inner = write(&self.inner);
        if let Some(record) = inner.runs.remove(run_id) {
            if let Some(session) = inner.sessions.get_mut(&record.session_id) {
                session.run_count = session.run_count.saturating_sub(1);
                session.revision += 1;
            }
            inner.revision += 1;
        }
    }

    pub fn runs(&self) -> Vec<RunRecord> {
        read(&self.inner).runs.values().cloned().collect()
    }

    // ---------- approval ----------

    pub fn record_approval(
        &self,
        run_id: RunId,
        tool_call_id: ToolCallId,
        reason: String,
        status: ApprovalStatus,
    ) -> Result<(), AggregateError> {
        let mut inner = write(&self.inner);
        if !inner.runs.contains_key(&run_id) {
            return Err(AggregateError::RunNotFound(run_id.to_string()));
        }
        let decided_at = match &status {
            ApprovalStatus::Decided(_) => Some(now_timestamp()),
            ApprovalStatus::Pending => None,
        };
        inner.approvals.insert(
            tool_call_id.clone(),
            ApprovalRecord {
                run_id,
                tool_call_id,
                reason,
                status,
                decided_at,
            },
        );
        inner.revision += 1;
        Ok(())
    }

    pub fn decide_approval(
        &self,
        run_id: &RunId,
        tool_call_id: &ToolCallId,
        decision: core_api::ApprovalDecision,
    ) -> Result<(), AggregateError> {
        let mut inner = write(&self.inner);
        if !inner.runs.contains_key(run_id) {
            return Err(AggregateError::RunNotFound(run_id.to_string()));
        }
        let record = inner
            .approvals
            .entry(tool_call_id.clone())
            .or_insert_with(|| ApprovalRecord {
                run_id: run_id.clone(),
                tool_call_id: tool_call_id.clone(),
                reason: "decided before registration".into(),
                status: ApprovalStatus::Pending,
                decided_at: None,
            });
        record.status = ApprovalStatus::Decided(decision);
        record.decided_at = Some(now_timestamp());
        inner.revision += 1;
        Ok(())
    }

    pub fn clear_run_approvals(&self, run_id: &RunId) {
        let mut inner = write(&self.inner);
        inner.approvals.retain(|_, record| &record.run_id != run_id);
        inner.revision += 1;
    }

    pub fn approvals(&self) -> Vec<ApprovalRecord> {
        read(&self.inner).approvals.values().cloned().collect()
    }

    // ---------- provider ----------

    pub fn record_provider(
        &self,
        provider_id: ProviderId,
        authenticated: bool,
        model_count: usize,
    ) {
        let mut inner = write(&self.inner);
        let status = if authenticated {
            core_api::ProviderStatus::Ready
        } else {
            core_api::ProviderStatus::AuthenticationRequired
        };
        inner.providers.insert(
            provider_id.clone(),
            ProviderRecord {
                provider_id,
                status,
                authenticated,
                model_count,
                registered_at: now_timestamp(),
                auth_flow: None,
            },
        );
        inner.revision += 1;
    }

    pub fn set_provider_status(
        &self,
        provider_id: &ProviderId,
        status: core_api::ProviderStatus,
    ) -> Result<(), AggregateError> {
        let mut inner = write(&self.inner);
        let record = inner
            .providers
            .get_mut(provider_id)
            .ok_or_else(|| AggregateError::ProviderNotFound(provider_id.to_string()))?;
        record.status = status.clone();
        record.authenticated = status != core_api::ProviderStatus::AuthenticationRequired;
        inner.revision += 1;
        Ok(())
    }

    pub fn record_auth_flow(&self, provider_id: &ProviderId, flow: &str) {
        let mut inner = write(&self.inner);
        let record = inner
            .providers
            .entry(provider_id.clone())
            .or_insert_with(|| ProviderRecord {
                provider_id: provider_id.clone(),
                status: core_api::ProviderStatus::AuthenticationRequired,
                authenticated: false,
                model_count: 0,
                registered_at: now_timestamp(),
                auth_flow: None,
            });
        record.auth_flow = Some(flow.to_string());
        record.status = core_api::ProviderStatus::AuthenticationRequired;
        record.authenticated = false;
        inner.revision += 1;
    }

    pub fn providers(&self) -> Vec<ProviderRecord> {
        read(&self.inner).providers.values().cloned().collect()
    }

    // ---------- diff ----------

    pub fn seed_diff(
        &self,
        workspace_id: &WorkspaceId,
        files: Vec<DiffFile>,
    ) -> Result<(), AggregateError> {
        let mut inner = write(&self.inner);
        if !inner.workspaces.contains_key(workspace_id) {
            return Err(AggregateError::WorkspaceNotFound(workspace_id.to_string()));
        }
        inner.diffs.insert(workspace_id.clone(), files);
        inner.revision += 1;
        Ok(())
    }

    pub fn diffs(&self, workspace_id: &WorkspaceId) -> Vec<DiffFile> {
        read(&self.inner)
            .diffs
            .get(workspace_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn diff_file(&self, workspace_id: &WorkspaceId, path: &str) -> Option<DiffFile> {
        read(&self.inner)
            .diffs
            .get(workspace_id)
            .and_then(|files| files.iter().find(|file| file.path == path))
            .cloned()
    }

    pub fn record_git_stage(&self, workspace_id: WorkspaceId, paths: Vec<String>) {
        let mut inner = write(&self.inner);
        inner
            .git_stages
            .entry(workspace_id)
            .or_default()
            .extend(paths);
        inner.revision += 1;
    }

    // ---------- artifact ----------

    pub fn put_artifact(
        &self,
        artifact_id: ArtifactId,
        byte_length: u64,
        media_type: String,
    ) -> Result<(), AggregateError> {
        let mut inner = write(&self.inner);
        if inner.artifacts.contains_key(&artifact_id) {
            return Err(AggregateError::ArtifactExists(artifact_id.to_string()));
        }
        inner.artifacts.insert(
            artifact_id.clone(),
            ArtifactRecord {
                artifact_id,
                media_type,
                byte_length,
                created_at: now_timestamp(),
            },
        );
        inner.revision += 1;
        Ok(())
    }

    pub fn artifact(&self, artifact_id: &ArtifactId) -> Option<ArtifactRecord> {
        read(&self.inner).artifacts.get(artifact_id).cloned()
    }

    pub fn artifacts(&self) -> Vec<ArtifactRecord> {
        read(&self.inner).artifacts.values().cloned().collect()
    }

    // ---------- gui client ----------

    pub fn note_gui_connect(&self, client_id: GuiClientId, connection_id: ConnectionId) {
        let mut inner = write(&self.inner);
        inner.gui_clients.insert(
            client_id.clone(),
            GuiClientRecord {
                client_id,
                connection_id: Some(connection_id),
                connected: true,
                last_seen_at: now_timestamp(),
            },
        );
        inner.revision += 1;
    }

    /// GUI 断线仅更新连接记录；不取消任何 Run（由 RunCancel 显式取消）。
    pub fn note_gui_disconnect(&self, client_id: &GuiClientId) {
        let mut inner = write(&self.inner);
        inner
            .gui_clients
            .entry(client_id.clone())
            .or_insert_with(|| GuiClientRecord {
                client_id: client_id.clone(),
                connection_id: None,
                connected: false,
                last_seen_at: now_timestamp(),
            });
        if let Some(record) = inner.gui_clients.get_mut(client_id) {
            record.connected = false;
            record.connection_id = None;
            record.last_seen_at = now_timestamp();
        }
        inner.revision += 1;
    }

    pub fn gui_clients(&self) -> Vec<GuiClientRecord> {
        read(&self.inner).gui_clients.values().cloned().collect()
    }

    // ---------- terminal ----------

    pub fn record_terminal(
        &self,
        workspace_id: WorkspaceId,
        terminal_session_id: TerminalSessionId,
        working_directory: Option<String>,
    ) {
        let mut inner = write(&self.inner);
        inner.terminals.insert(
            terminal_session_id.clone(),
            TerminalRecord {
                terminal_session_id,
                workspace_id,
                working_directory,
                output_bytes: 0,
                columns: 0,
                rows: 0,
            },
        );
        inner.revision += 1;
    }

    pub fn record_terminal_output(&self, terminal_session_id: &TerminalSessionId, data: &str) {
        let mut inner = write(&self.inner);
        if let Some(record) = inner.terminals.get_mut(terminal_session_id) {
            record.output_bytes = record.output_bytes.saturating_add(data.len() as u64);
        }
        inner.revision += 1;
    }

    pub fn record_terminal_resize(
        &self,
        terminal_session_id: &TerminalSessionId,
        columns: u16,
        rows: u16,
    ) {
        let mut inner = write(&self.inner);
        if let Some(record) = inner.terminals.get_mut(terminal_session_id) {
            record.columns = columns;
            record.rows = rows;
        }
        inner.revision += 1;
    }

    // ---------- snapshot ----------

    pub fn snapshot(&self) -> Snapshot {
        let inner = read(&self.inner);
        Snapshot {
            revision: inner.revision,
            core_ready: inner.core_ready,
            workspaces: inner.workspaces.values().cloned().collect(),
            sessions: inner.sessions.values().cloned().collect(),
            runs: inner.runs.values().cloned().collect(),
            approvals: inner.approvals.values().cloned().collect(),
            providers: inner.providers.values().cloned().collect(),
            artifacts: inner.artifacts.values().cloned().collect(),
            gui_clients: inner.gui_clients.values().cloned().collect(),
            terminals: inner.terminals.values().cloned().collect(),
        }
    }

    pub fn revision(&self) -> u64 {
        read(&self.inner).revision
    }
}

impl Default for AggregateState {
    fn default() -> Self {
        Self::new()
    }
}

fn next_id_locked(inner: &mut Inner, prefix: &str) -> String {
    inner.next_id += 1;
    format!("{prefix}-{}", inner.next_id)
}

fn read(inner: &RwLock<Inner>) -> std::sync::RwLockReadGuard<'_, Inner> {
    inner
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write(inner: &RwLock<Inner>) -> std::sync::RwLockWriteGuard<'_, Inner> {
    inner
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
