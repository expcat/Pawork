//! 轻量内存状态聚合（P13-1）。
//!
//! 聚合 workspace / session / run / approval / provider 以及 diff、artifact、
//! terminal、GUI client 等查询面数据，支撑 `AppQuery` 与 `SnapshotFetch`。
//! 纯内存、线程安全；持久化与事件重放在后续 Phase（P13-2/P19）接入。

use std::collections::BTreeMap;
use std::sync::RwLock;

use agent_domain::{
    ArtifactId, EventId, PrincipalId, ProviderId, RunId, SessionId, TenantId, TerminalSessionId,
    Timestamp, ToolCallId, WorkspaceId,
};
use core_api::{ClientContextSnapshot, CommandSource};
use diff_service::DiffFile;
use serde::{Deserialize, Serialize};
use tenant_service::IdentityContext;
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
    #[error("invalid client context: {0}")]
    InvalidClientContext(String),
    #[error(
        "stale client context for session {session_id}: received revision {received}, current revision {current}"
    )]
    StaleClientContext {
        session_id: String,
        received: u64,
        current: u64,
    },
    #[error("client context revision {revision} conflicts for session {session_id}")]
    ClientContextConflict { session_id: String, revision: u64 },
    #[error("aggregate state poisoned")]
    Poisoned,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: SessionId,
    /// 归属租户（P18-2：Session 创建与查询必须携带身份上下文）。
    pub tenant_id: TenantId,
    /// 创建主体。
    pub principal_id: PrincipalId,
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
    /// 归属租户（P18-2：usage 记账与查询按此隔离）。
    pub tenant_id: TenantId,
    /// 发起主体。
    pub principal_id: PrincipalId,
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
    pub terminals: Vec<TerminalRecord>,
}

struct Inner {
    revision: u64,
    core_ready: bool,
    next_id: u64,
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    sessions: BTreeMap<SessionId, SessionRecord>,
    client_contexts: BTreeMap<SessionId, ClientContextSnapshot>,
    runs: BTreeMap<RunId, RunRecord>,
    approvals: BTreeMap<ToolCallId, ApprovalRecord>,
    providers: BTreeMap<ProviderId, ProviderRecord>,
    artifacts: BTreeMap<ArtifactId, ArtifactRecord>,
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
                client_contexts: BTreeMap::new(),
                runs: BTreeMap::new(),
                approvals: BTreeMap::new(),
                providers: BTreeMap::new(),
                artifacts: BTreeMap::new(),
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

    /// 向后兼容包装（P18-2 前调用方，如 snapshot-service 测试）：
    /// 未显式携带身份时以 local/default + local/user 创建。
    pub fn create_session(
        &self,
        workspace_id: WorkspaceId,
        title: String,
        now: Timestamp,
    ) -> Result<SessionRecord, AggregateError> {
        self.create_session_with_identity(workspace_id, title, now, &IdentityContext::local())
    }

    /// tenant/principal 显式版本（P18-2）：生产路径必须使用本方法携带真实身份。
    pub fn create_session_with_identity(
        &self,
        workspace_id: WorkspaceId,
        title: String,
        now: Timestamp,
        identity: &IdentityContext,
    ) -> Result<SessionRecord, AggregateError> {
        let mut inner = write(&self.inner);
        if !inner.workspaces.contains_key(&workspace_id) {
            return Err(AggregateError::WorkspaceNotFound(workspace_id.to_string()));
        }
        let session_id = SessionId::from(next_id_locked(&mut inner, "session"));
        let record = SessionRecord {
            session_id: session_id.clone(),
            tenant_id: identity.tenant_id.clone(),
            principal_id: identity.principal_id.clone(),
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

    /// tenant-scoped 读取（P18-2）：跨租户 session 视同不存在，不泄漏存在性。
    pub fn get_session(
        &self,
        session_id: &SessionId,
        tenant_id: &TenantId,
    ) -> Option<SessionRecord> {
        read(&self.inner)
            .sessions
            .get(session_id)
            .filter(|record| record.tenant_id == *tenant_id)
            .cloned()
    }

    /// tenant-scoped 存在性（P18-2）。
    pub fn session_exists(&self, session_id: &SessionId, tenant_id: &TenantId) -> bool {
        read(&self.inner)
            .sessions
            .get(session_id)
            .is_some_and(|record| record.tenant_id == *tenant_id)
    }

    /// 以单调 revision 全量替换 Host 观察到的 session 上下文。相同 revision
    /// + 相同内容为幂等重放；旧 revision 或同 revision 不同内容 fail-closed。
    pub fn replace_client_context(
        &self,
        session_id: &SessionId,
        snapshot: ClientContextSnapshot,
    ) -> Result<ClientContextSnapshot, AggregateError> {
        snapshot
            .validate()
            .map_err(AggregateError::InvalidClientContext)?;
        let mut inner = write(&self.inner);
        if !inner.sessions.contains_key(session_id) {
            return Err(AggregateError::SessionNotFound(session_id.to_string()));
        }
        if let Some(current) = inner.client_contexts.get(session_id) {
            if snapshot.revision < current.revision {
                return Err(AggregateError::StaleClientContext {
                    session_id: session_id.to_string(),
                    received: snapshot.revision,
                    current: current.revision,
                });
            }
            if snapshot.revision == current.revision {
                if snapshot == *current {
                    return Ok(current.clone());
                }
                return Err(AggregateError::ClientContextConflict {
                    session_id: session_id.to_string(),
                    revision: snapshot.revision,
                });
            }
        }
        inner
            .client_contexts
            .insert(session_id.clone(), snapshot.clone());
        inner.revision += 1;
        Ok(snapshot)
    }

    /// 读取最新 Host 上下文快照；每个 provider turn 在 pre-prompt 时动态读取，
    /// 因此活动 run 无需重启即可消费 IDE 新诊断/选区。
    pub fn client_context(&self, session_id: &SessionId) -> Option<ClientContextSnapshot> {
        read(&self.inner).client_contexts.get(session_id).cloned()
    }

    /// 幂等 materialize：以权威 `session_id` 在聚合中重建会话记录
    /// （跨 host/进程 resume 用，P17-7）。
    ///
    /// 不生成新 id、不改写既有记录的字段：已存在时原样返回现有记录
    /// （并发/重试安全，同 id 多次调用只产生一条记录，不堆积、不留
    /// ghost）。id 形如 `<prefix>-<n>` 时同步推进 next_id 计数，避免后续
    /// `create_session` 生成同号 id 覆盖本记录（进程内唯一性）。
    pub fn materialize_session(
        &self,
        session_id: SessionId,
        workspace_id: WorkspaceId,
        title: String,
        created_at: Timestamp,
    ) -> Result<SessionRecord, AggregateError> {
        let mut inner = write(&self.inner);
        if let Some(existing) = inner.sessions.get(&session_id) {
            return Ok(existing.clone());
        }
        if !inner.workspaces.contains_key(&workspace_id) {
            return Err(AggregateError::WorkspaceNotFound(workspace_id.to_string()));
        }
        let identity = IdentityContext::local();
        let record = SessionRecord {
            session_id: session_id.clone(),
            tenant_id: identity.tenant_id.clone(),
            principal_id: identity.principal_id.clone(),
            workspace_id,
            title: if title.trim().is_empty() {
                "Untitled".into()
            } else {
                title
            },
            created_at,
            revision: 1,
            open: true,
            compacted: false,
            run_count: 0,
            message_count: 0,
            forked_from: None,
            parent_event_id: None,
        };
        if let Some((_, suffix)) = session_id.as_str().rsplit_once('-') {
            if let Ok(n) = suffix.parse::<u64>() {
                inner.next_id = inner.next_id.max(n);
            }
        }
        inner.sessions.insert(session_id, record.clone());
        inner.revision += 1;
        Ok(record)
    }

    pub fn open_session(
        &self,
        session_id: &SessionId,
        tenant_id: &TenantId,
    ) -> Result<SessionRecord, AggregateError> {
        let mut inner = write(&self.inner);
        let record = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AggregateError::SessionNotFound(session_id.to_string()))?;
        if record.tenant_id != *tenant_id {
            return Err(AggregateError::SessionNotFound(session_id.to_string()));
        }
        record.open = true;
        record.revision += 1;
        let record = record.clone();
        inner.revision += 1;
        Ok(record)
    }

    pub fn compact_session(
        &self,
        session_id: &SessionId,
        tenant_id: &TenantId,
    ) -> Result<SessionRecord, AggregateError> {
        let mut inner = write(&self.inner);
        let record = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AggregateError::SessionNotFound(session_id.to_string()))?;
        if record.tenant_id != *tenant_id {
            return Err(AggregateError::SessionNotFound(session_id.to_string()));
        }
        record.compacted = true;
        record.revision += 1;
        let record = record.clone();
        inner.revision += 1;
        Ok(record)
    }

    /// 向后兼容包装（P18-2 前调用方）：以 local/default 身份 fork。
    pub fn fork_session(
        &self,
        session_id: &SessionId,
        parent_event_id: EventId,
    ) -> Result<SessionRecord, AggregateError> {
        self.fork_session_with_identity(session_id, parent_event_id, &IdentityContext::local())
    }

    pub fn fork_session_with_identity(
        &self,
        session_id: &SessionId,
        parent_event_id: EventId,
        identity: &IdentityContext,
    ) -> Result<SessionRecord, AggregateError> {
        let mut inner = write(&self.inner);
        let parent = inner
            .sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| AggregateError::SessionNotFound(session_id.to_string()))?;
        // 跨租户 fork 拒绝（fail-closed，不泄漏父 session 存在性）。
        if parent.tenant_id != identity.tenant_id {
            return Err(AggregateError::SessionNotFound(session_id.to_string()));
        }
        let child_id = SessionId::from(next_id_locked(&mut inner, "session"));
        let record = SessionRecord {
            session_id: child_id.clone(),
            tenant_id: parent.tenant_id.clone(),
            principal_id: identity.principal_id.clone(),
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

    /// 向后兼容包装（P18-2 前调用方）：以 local/default + local/user 记录 run。
    pub fn record_run(
        &self,
        run_id: RunId,
        session_id: SessionId,
        model: agent_domain::ModelId,
        provider_id: ProviderId,
        source: CommandSource,
        now: Timestamp,
    ) -> Result<RunRecord, AggregateError> {
        self.record_run_with_identity(
            run_id,
            session_id,
            model,
            provider_id,
            source,
            now,
            &IdentityContext::local(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_run_with_identity(
        &self,
        run_id: RunId,
        session_id: SessionId,
        model: agent_domain::ModelId,
        provider_id: ProviderId,
        source: CommandSource,
        now: Timestamp,
        identity: &IdentityContext,
    ) -> Result<RunRecord, AggregateError> {
        let mut inner = write(&self.inner);
        let session = inner
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| AggregateError::SessionNotFound(session_id.to_string()))?;
        // 跨租户 run 拒绝（fail-closed）。
        if session.tenant_id != identity.tenant_id {
            return Err(AggregateError::SessionNotFound(session_id.to_string()));
        }
        session.run_count += 1;
        session.revision += 1;
        let record = RunRecord {
            run_id: run_id.clone(),
            session_id,
            tenant_id: identity.tenant_id.clone(),
            principal_id: identity.principal_id.clone(),
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

    /// tenant-scoped 读取（P18-2）：跨租户 run 视同不存在。
    pub fn get_run(&self, run_id: &RunId, tenant_id: &TenantId) -> Option<RunRecord> {
        read(&self.inner)
            .runs
            .get(run_id)
            .filter(|record| record.tenant_id == *tenant_id)
            .cloned()
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
            terminals: inner.terminals.values().cloned().collect(),
        }
    }

    /// P18-2 tenant-scoped snapshot：只返回当前租户拥有的 Session、Run 与 Approval。
    /// workspace/provider 等尚未 tenant-bound 的全局目录维持原有视图。
    pub fn snapshot_for_tenant(&self, tenant_id: &TenantId) -> Snapshot {
        let inner = read(&self.inner);
        Snapshot {
            revision: inner.revision,
            core_ready: inner.core_ready,
            workspaces: inner.workspaces.values().cloned().collect(),
            sessions: inner
                .sessions
                .values()
                .filter(|session| session.tenant_id == *tenant_id)
                .cloned()
                .collect(),
            runs: inner
                .runs
                .values()
                .filter(|run| run.tenant_id == *tenant_id)
                .cloned()
                .collect(),
            approvals: inner
                .approvals
                .values()
                .filter(|approval| {
                    inner
                        .runs
                        .get(&approval.run_id)
                        .is_some_and(|run| run.tenant_id == *tenant_id)
                })
                .cloned()
                .collect(),
            providers: inner.providers.values().cloned().collect(),
            artifacts: inner.artifacts.values().cloned().collect(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use core_api::ClientContextSnapshot;
    use workspace_service::{TrustState, WorkspaceRoot};

    fn aggregate_with_session() -> (AggregateState, SessionId) {
        let aggregate = AggregateState::new();
        let workspace_id = WorkspaceId::from("workspace-1");
        aggregate.record_workspace(Workspace {
            id: workspace_id.clone(),
            name: "workspace".into(),
            roots: vec![WorkspaceRoot {
                path: std::path::PathBuf::from("/workspace"),
                git: None,
            }],
            trust: TrustState::Trusted,
            last_accessed_at: Timestamp::from_unix_millis(1),
            revision: 1,
        });
        let session = aggregate
            .create_session(
                workspace_id,
                "session".into(),
                Timestamp::from_unix_millis(1),
            )
            .expect("create session");
        (aggregate, session.session_id)
    }

    fn snapshot(revision: u64) -> ClientContextSnapshot {
        ClientContextSnapshot {
            revision,
            active_document: None,
            open_documents: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn client_context_replace_is_monotonic_and_idempotent() {
        let (aggregate, session_id) = aggregate_with_session();
        aggregate
            .replace_client_context(&session_id, snapshot(1))
            .expect("first snapshot");
        aggregate
            .replace_client_context(&session_id, snapshot(1))
            .expect("identical replay");
        aggregate
            .replace_client_context(&session_id, snapshot(3))
            .expect("newer snapshot");
        assert_eq!(aggregate.client_context(&session_id), Some(snapshot(3)));
        assert!(matches!(
            aggregate.replace_client_context(&session_id, snapshot(2)),
            Err(AggregateError::StaleClientContext { .. })
        ));
    }

    #[test]
    fn client_context_requires_existing_session() {
        let aggregate = AggregateState::new();
        assert!(matches!(
            aggregate.replace_client_context(&SessionId::from("missing"), snapshot(1)),
            Err(AggregateError::SessionNotFound(_))
        ));
    }

    fn diagnostic(uri: &str, message: &str) -> core_api::ClientDiagnostic {
        core_api::ClientDiagnostic {
            document_uri: uri.into(),
            version: None,
            range: core_api::ClientTextRange {
                start: core_api::ClientTextPosition {
                    line: 0,
                    character: 0,
                },
                end: core_api::ClientTextPosition {
                    line: 0,
                    character: 1,
                },
            },
            severity: Some(core_api::ClientDiagnosticSeverity::Error),
            code: None,
            source: None,
            message: message.into(),
        }
    }

    #[test]
    fn client_context_over_limit_and_unsafe_uri_roll_back_without_pollution() {
        // P17-9 审查阻塞：超限 / 不安全 URI 在写入前 validate 失败，
        // 已存快照不被部分写入污染（失败回滚）。
        let (aggregate, session_id) = aggregate_with_session();
        aggregate
            .replace_client_context(&session_id, snapshot(1))
            .expect("seed snapshot");

        // 超限 message：validate 先于写入失败。
        let mut overflow = snapshot(2);
        overflow.diagnostics.push(diagnostic(
            "file:///x.rs",
            &"x".repeat(core_api::MAX_CLIENT_CONTEXT_MESSAGE_BYTES + 1),
        ));
        assert!(matches!(
            aggregate.replace_client_context(&session_id, overflow),
            Err(AggregateError::InvalidClientContext(_))
        ));
        assert_eq!(
            aggregate.client_context(&session_id),
            Some(snapshot(1)),
            "over-limit failure must not pollute the stored snapshot"
        );

        // 不安全 URI scheme：同样在写入前拒绝、不污染。
        let mut unsafe_uri = snapshot(2);
        unsafe_uri
            .diagnostics
            .push(diagnostic("javascript:alert(1)", "safe"));
        assert!(matches!(
            aggregate.replace_client_context(&session_id, unsafe_uri),
            Err(AggregateError::InvalidClientContext(_))
        ));
        assert_eq!(
            aggregate.client_context(&session_id),
            Some(snapshot(1)),
            "unsafe-uri failure must not pollute the stored snapshot"
        );
    }
}
