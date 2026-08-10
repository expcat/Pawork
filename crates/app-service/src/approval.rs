//! 工具审批注册表（P13-1）。
//!
//! 按 `tool_call_id` 提供异步审批等待通道（oneshot），供
//! [`agent_engine::LoopContext::request_approval`] 回调等待用户决策；
//! `ToolApprove` 命令经 [`ApprovalRegistry::decide`] 投递决策。
//!
//! 竞态处理：审批决策先于循环注册到达时进入有界 `queued` 表，注册时立即解析。
//! `ApproveForRun` 记录 run 级放行，后续同 run 的 tool call 自动批准。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use agent_domain::{RunId, ToolCallId};
use core_api::ApprovalDecision;
use thiserror::Error;
use tokio::sync::oneshot;

/// 同一时刻最多挂起的审批数（超限注册返回 [`ApprovalError::Capacity`]）。
pub const MAX_PENDING_APPROVALS: usize = 1024;
/// 先于注册到达的决策缓存上限。
pub const MAX_QUEUED_DECISIONS: usize = 1024;

/// 一次挂起的审批请求。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingApproval {
    pub run_id: RunId,
    pub tool_call_id: ToolCallId,
    pub reason: String,
}

/// 注册结果：挂起等待，或已被（先到的）决策立即解析。
#[derive(Debug)]
pub enum Registration {
    Pending(oneshot::Receiver<ApprovalDecision>),
    Resolved(ApprovalDecision),
}

#[derive(Debug, Error)]
pub enum ApprovalError {
    #[error("no pending approval for tool call {0}")]
    NotFound(String),
    #[error("approval for tool call {tool_call_id} belongs to run {expected}, not {actual}")]
    RunMismatch {
        tool_call_id: String,
        expected: String,
        actual: String,
    },
    #[error("too many pending approvals; respond to pending requests first")]
    Capacity,
    #[error("approval for tool call {0} was already decided")]
    AlreadyDecided(String),
}

struct PendingEntry {
    run_id: RunId,
    #[allow(dead_code)]
    reason: String,
    sender: oneshot::Sender<ApprovalDecision>,
}

struct QueuedDecision {
    run_id: RunId,
    decision: ApprovalDecision,
}

struct Inner {
    pending: BTreeMap<ToolCallId, PendingEntry>,
    queued: BTreeMap<ToolCallId, QueuedDecision>,
    run_approved: BTreeSet<RunId>,
    /// 已投递决策的 (run, tool_call) 集合，重复 decide 报 AlreadyDecided。
    decided: BTreeSet<(RunId, ToolCallId)>,
}

/// 按 tool_call_id 的异步审批注册表。
pub struct ApprovalRegistry {
    inner: Mutex<Inner>,
}

impl ApprovalRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                pending: BTreeMap::new(),
                queued: BTreeMap::new(),
                run_approved: BTreeSet::new(),
                decided: BTreeSet::new(),
            }),
        }
    }

    /// 注册一次审批等待；返回接收器或（若决策已先到）直接解析结果。
    pub fn register(
        &self,
        run_id: RunId,
        tool_call_id: ToolCallId,
        reason: String,
    ) -> Result<Registration, ApprovalError> {
        let mut inner = lock(&self.inner);
        if inner.run_approved.contains(&run_id) {
            return Ok(Registration::Resolved(ApprovalDecision::ApproveForRun));
        }
        if let Some(queued) = inner.queued.remove(&tool_call_id) {
            if queued.run_id == run_id {
                return Ok(Registration::Resolved(queued.decision));
            }
            inner.queued.insert(tool_call_id.clone(), queued);
        }
        if inner.pending.len() >= MAX_PENDING_APPROVALS {
            return Err(ApprovalError::Capacity);
        }
        let (sender, receiver) = oneshot::channel();
        inner.pending.insert(
            tool_call_id.clone(),
            PendingEntry {
                run_id,
                reason,
                sender,
            },
        );
        Ok(Registration::Pending(receiver))
    }

    /// 投递用户决策（`ToolApprove` 命令入口）。尚未注册的 tool call 先入队，
    /// 注册时立即解析，消除「命令先于循环注册」的竞态。
    pub fn decide(
        &self,
        run_id: &RunId,
        tool_call_id: &ToolCallId,
        decision: ApprovalDecision,
    ) -> Result<(), ApprovalError> {
        let mut inner = lock(&self.inner);
        if inner
            .decided
            .contains(&(run_id.clone(), tool_call_id.clone()))
        {
            return Err(ApprovalError::AlreadyDecided(tool_call_id.to_string()));
        }
        if let Some(entry) = inner.pending.remove(tool_call_id) {
            if &entry.run_id != run_id {
                let expected = entry.run_id.to_string();
                inner.pending.insert(tool_call_id.clone(), entry);
                return Err(ApprovalError::RunMismatch {
                    tool_call_id: tool_call_id.to_string(),
                    expected,
                    actual: run_id.to_string(),
                });
            }
            let _ = entry.sender.send(decision.clone());
            inner.decided.insert((run_id.clone(), tool_call_id.clone()));
            if decision == ApprovalDecision::ApproveForRun {
                inner.run_approved.insert(run_id.clone());
            }
            return Ok(());
        }
        if inner.queued.contains_key(tool_call_id)
            && inner
                .queued
                .get(tool_call_id)
                .is_some_and(|q| q.run_id == *run_id)
        {
            return Err(ApprovalError::AlreadyDecided(tool_call_id.to_string()));
        }
        if inner.queued.len() >= MAX_QUEUED_DECISIONS {
            if let Some(oldest) = inner.queued.keys().next().cloned() {
                inner.queued.remove(&oldest);
            }
        }
        inner.queued.insert(
            tool_call_id.clone(),
            QueuedDecision {
                run_id: run_id.clone(),
                decision,
            },
        );
        inner.decided.insert((run_id.clone(), tool_call_id.clone()));
        Ok(())
    }

    /// 当前挂起审批数。
    pub fn pending_count(&self) -> usize {
        lock(&self.inner).pending.len()
    }

    /// 该 run 是否已获 run 级放行。
    pub fn is_run_approved(&self, run_id: &RunId) -> bool {
        lock(&self.inner).run_approved.contains(run_id)
    }

    /// run 结束/取消/失败时清理其挂起与排队条目，防止泄漏。
    pub fn clear_run(&self, run_id: &RunId) {
        let mut inner = lock(&self.inner);
        inner.pending.retain(|_, entry| &entry.run_id != run_id);
        inner.queued.retain(|_, queued| &queued.run_id != run_id);
        inner.run_approved.remove(run_id);
        inner
            .decided
            .retain(|(decided_run, _)| decided_run != run_id);
    }
}

impl Default for ApprovalRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn lock(inner: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn decision_resolves_pending_receiver() {
        let registry = ApprovalRegistry::new();
        let registration = registry
            .register(
                RunId::from("run-1"),
                ToolCallId::from("call-1"),
                "needs approval".into(),
            )
            .expect("register");
        let Registration::Pending(receiver) = registration else {
            panic!("expected pending");
        };
        registry
            .decide(
                &RunId::from("run-1"),
                &ToolCallId::from("call-1"),
                ApprovalDecision::ApproveOnce,
            )
            .expect("decide");
        assert_eq!(
            receiver.await.expect("decision"),
            ApprovalDecision::ApproveOnce
        );
        assert_eq!(registry.pending_count(), 0);
    }

    #[tokio::test]
    async fn decision_before_register_resolves_immediately() {
        let registry = ApprovalRegistry::new();
        registry
            .decide(
                &RunId::from("run-1"),
                &ToolCallId::from("call-1"),
                ApprovalDecision::Deny,
            )
            .expect("queue decision");
        let registration = registry
            .register(
                RunId::from("run-1"),
                ToolCallId::from("call-1"),
                "reason".into(),
            )
            .expect("register");
        match registration {
            Registration::Resolved(ApprovalDecision::Deny) => {}
            other => panic!("expected resolved deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn approve_for_run_auto_approves_following_calls() {
        let registry = ApprovalRegistry::new();
        let run_id = RunId::from("run-1");
        let registration = registry
            .register(run_id.clone(), ToolCallId::from("call-1"), "r".into())
            .expect("register");
        let Registration::Pending(receiver) = registration else {
            panic!("expected pending");
        };
        registry
            .decide(
                &run_id,
                &ToolCallId::from("call-1"),
                ApprovalDecision::ApproveForRun,
            )
            .expect("decide");
        assert_eq!(
            receiver.await.expect("decision"),
            ApprovalDecision::ApproveForRun
        );
        let second = registry
            .register(run_id.clone(), ToolCallId::from("call-2"), "r".into())
            .expect("register");
        assert!(
            matches!(
                second,
                Registration::Resolved(ApprovalDecision::ApproveForRun)
            ),
            "run 级放行应自动批准后续调用"
        );
    }

    #[tokio::test]
    async fn run_mismatch_and_duplicate_decide_are_rejected() {
        let registry = ApprovalRegistry::new();
        registry
            .register(RunId::from("run-1"), ToolCallId::from("call-1"), "r".into())
            .expect("register");
        assert!(matches!(
            registry.decide(
                &RunId::from("run-2"),
                &ToolCallId::from("call-1"),
                ApprovalDecision::ApproveOnce,
            ),
            Err(ApprovalError::RunMismatch { .. })
        ));
        registry
            .decide(
                &RunId::from("run-1"),
                &ToolCallId::from("call-1"),
                ApprovalDecision::Deny,
            )
            .expect("decide");
        assert!(matches!(
            registry.decide(
                &RunId::from("run-1"),
                &ToolCallId::from("call-1"),
                ApprovalDecision::Deny,
            ),
            Err(ApprovalError::AlreadyDecided(_))
        ));
    }

    #[tokio::test]
    async fn clear_run_removes_pending_and_queued() {
        let registry = ApprovalRegistry::new();
        let run_id = RunId::from("run-1");
        registry
            .register(run_id.clone(), ToolCallId::from("call-1"), "r".into())
            .expect("register");
        registry
            .decide(
                &RunId::from("run-2"),
                &ToolCallId::from("call-2"),
                ApprovalDecision::Deny,
            )
            .expect("queue");
        registry.clear_run(&run_id);
        assert_eq!(registry.pending_count(), 0);
        // 排队决策归属 run-2，不受 run-1 清理影响。
        let registration = registry
            .register(RunId::from("run-2"), ToolCallId::from("call-2"), "r".into())
            .expect("register");
        assert!(matches!(registration, Registration::Resolved(_)));
    }
}
