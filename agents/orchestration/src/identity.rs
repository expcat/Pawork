//! Worker 角色与 Agent 实例身份（P12-1）。
//!
//! [`AgentInstance`] 是 Supervisor 注册表中每个 worker 的不可变身份记录，
//! 携带 tenant / principal / session / parent 归属，保证 Agent 与账号状态机隔离。

use std::path::PathBuf;

use pawork_domain::{AgentId, PrincipalId, SessionId, TenantId};
use serde::{Deserialize, Serialize};

/// Worker 在编排树中的角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    /// 父代理（根，无 parent）。
    Parent,
    /// 由父代理派生的 worker。
    Worker,
}

/// 一个被监督 Agent 的完整身份记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInstance {
    /// 全局唯一 agent 标识。
    pub agent_id: AgentId,
    /// 所属租户。
    pub tenant_id: TenantId,
    /// 发起主体。
    pub principal_id: PrincipalId,
    /// 父代理；`None` 表示本实例是根。
    pub parent_id: Option<AgentId>,
    /// 角色。
    pub role: WorkerRole,
    /// 会话。
    pub session_id: SessionId,
    /// 独立 worktree 路径（未分配时为 `None`）。
    pub worktree_path: Option<PathBuf>,
    /// 创建时间（Unix epoch 毫秒）。
    pub created_at_ms: u64,
}

impl AgentInstance {
    /// 构造一个 worker 实例（`role = Worker`，`parent_id` 必须为 `Some`）。
    pub fn new_worker(
        agent_id: AgentId,
        tenant_id: TenantId,
        principal_id: PrincipalId,
        parent_id: AgentId,
        session_id: SessionId,
        worktree_path: Option<PathBuf>,
        created_at_ms: u64,
    ) -> Self {
        Self {
            agent_id,
            tenant_id,
            principal_id,
            parent_id: Some(parent_id),
            role: WorkerRole::Worker,
            session_id,
            worktree_path,
            created_at_ms,
        }
    }

    /// 构造一个父实例（根，`role = Parent`，`parent_id = None`）。
    pub fn new_parent(
        agent_id: AgentId,
        tenant_id: TenantId,
        principal_id: PrincipalId,
        session_id: SessionId,
        worktree_path: Option<PathBuf>,
        created_at_ms: u64,
    ) -> Self {
        Self {
            agent_id,
            tenant_id,
            principal_id,
            parent_id: None,
            role: WorkerRole::Parent,
            session_id,
            worktree_path,
            created_at_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (AgentId, TenantId, PrincipalId, SessionId) {
        (
            AgentId::new("agent-1"),
            TenantId::new("tenant-a"),
            PrincipalId::new("principal-1"),
            SessionId::new("session-1"),
        )
    }

    #[test]
    fn new_worker_carries_parent_and_worker_role() {
        let (agent, tenant, principal, session) = ids();
        let instance = AgentInstance::new_worker(
            agent.clone(),
            tenant.clone(),
            principal.clone(),
            AgentId::new("parent-1"),
            session.clone(),
            None,
            1_000,
        );
        assert_eq!(instance.role, WorkerRole::Worker);
        assert_eq!(
            instance.parent_id.as_ref().map(|id| id.as_str()),
            Some("parent-1")
        );
        assert_eq!(instance.agent_id, agent);
        assert_eq!(instance.tenant_id, tenant);
        assert_eq!(instance.principal_id, principal);
        assert_eq!(instance.session_id, session);
        assert_eq!(instance.created_at_ms, 1_000);
        assert!(instance.worktree_path.is_none());
    }

    #[test]
    fn new_parent_has_no_parent_and_parent_role() {
        let (agent, tenant, principal, session) = ids();
        let instance = AgentInstance::new_parent(
            agent.clone(),
            tenant.clone(),
            principal.clone(),
            session,
            Some(PathBuf::from("wt")),
            2_000,
        );
        assert_eq!(instance.role, WorkerRole::Parent);
        assert!(instance.parent_id.is_none());
        assert_eq!(
            instance.worktree_path.as_deref(),
            Some(std::path::Path::new("wt"))
        );
        assert_eq!(instance.created_at_ms, 2_000);
    }

    #[test]
    fn role_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&WorkerRole::Parent).unwrap(),
            "\"parent\""
        );
        assert_eq!(
            serde_json::to_string(&WorkerRole::Worker).unwrap(),
            "\"worker\""
        );
    }
}
