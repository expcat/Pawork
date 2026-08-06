//! Run 状态机（P3-1）。
//!
//! 定义 Run 的全部状态与合法转换，每次转换都可映射到一个持久化
//! [`AgentEvent`](agent_events::AgentEvent) 变体。本模块为纯逻辑，不执行 IO，
//! 既驱动循环也用于崩溃恢复时按事件重放重建状态。
//!
//! 状态流转见 `docs/architecture/domain-model.md` §3。

use std::fmt;

use serde::{Deserialize, Serialize};

/// Run 在其生命周期内可能处于的状态。
///
/// 终态（[`RunState::is_terminal`]）一旦进入不可再转换。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// 刚创建，尚未开始构建上下文。
    #[default]
    Created,
    /// 正在加载资源与构建上下文（AGENTS.md / Skills / 历史 / 预算）。
    PreparingContext,
    /// 上下文就绪，等待提交给 Provider。
    WaitingForProvider,
    /// Provider 正在流式返回。
    StreamingResponse,
    /// 流式结束，正在收集本轮 tool call。
    CollectingToolCalls,
    /// 等待用户对 tool call 的审批。
    WaitingForApproval,
    /// 正在执行 tool。
    ExecutingTools,
    /// 正在回填 tool result 到消息并准备下一轮。
    AppendingToolResults,
    /// 正常完成（终态）。
    Completed,
    /// 被取消（终态）。
    Cancelled,
    /// 失败（终态）。
    Failed,
    /// 进程异常退出时遗留的未完成 Run（终态，仅恢复期识别）。
    Interrupted,
}

impl RunState {
    /// 是否为终态（不可再转换）。
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::Failed | Self::Interrupted
        )
    }

    /// 是否为可运行态（非终态、非中断）。
    pub const fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

impl fmt::Display for RunState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Created => "created",
            Self::PreparingContext => "preparing_context",
            Self::WaitingForProvider => "waiting_for_provider",
            Self::StreamingResponse => "streaming_response",
            Self::CollectingToolCalls => "collecting_tool_calls",
            Self::WaitingForApproval => "waiting_for_approval",
            Self::ExecutingTools => "executing_tools",
            Self::AppendingToolResults => "appending_tool_results",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        };
        formatter.write_str(name)
    }
}

/// 驱动状态机的命名转换。
///
/// 携带的布尔/枚举载荷描述「这次转换附带的事实」，使状态机无需读取事件
/// 负载即可判定合法性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunTransition {
    /// Created → PreparingContext：Run 正式开始。
    Begin,
    /// PreparingContext → WaitingForProvider：上下文构建完成。
    ContextPrepared,
    /// WaitingForProvider → StreamingResponse：Provider 流开始。
    ProviderStarted,
    /// StreamingResponse → CollectingToolCalls（有 tool call）或 Completed（无）。
    StreamFinished { has_tool_calls: bool },
    /// CollectingToolCalls → WaitingForApproval：需要用户审批。
    ApprovalRequested,
    /// WaitingForApproval → ExecutingTools：审批通过。
    ApprovalGranted,
    /// WaitingForApproval → AppendingToolResults：审批拒绝，回填拒绝结果后继续。
    ApprovalDenied,
    /// CollectingToolCalls → ExecutingTools：无需审批或自动放行。
    ToolsAutoStarted,
    /// ExecutingTools → AppendingToolResults：本轮工具执行完成。
    ToolsCompleted,
    /// AppendingToolResults → WaitingForProvider：结果已回填，进入下一轮。
    ResultsAppended,
    /// 任意非终态 → Completed：循环正常结束。
    Complete,
    /// 任意非终态 → Cancelled：取消（用户或系统）。
    Cancel,
    /// 任意非终态 → Failed：不可恢复错误。
    Fail,
    /// 任意非终态 → Interrupted：仅恢复期由外部标记。
    MarkInterrupted,
}

/// 状态转换失败。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransitionError {
    #[error("illegal transition {transition:?} from terminal state {state}")]
    FromTerminal {
        state: RunState,
        transition: RunTransition,
    },
    #[error("illegal transition {transition:?} from state {state}")]
    Illegal {
        state: RunState,
        transition: RunTransition,
    },
}

/// 给定当前状态与转换，返回合法的新状态；非法转换返回 [`TransitionError`]。
pub fn transition(from: RunState, t: RunTransition) -> Result<RunState, TransitionError> {
    use RunState as S;
    use RunTransition as T;
    if from.is_terminal() {
        return Err(TransitionError::FromTerminal {
            state: from,
            transition: t,
        });
    }
    let next = match (from, t) {
        (S::Created, T::Begin) => S::PreparingContext,
        (S::PreparingContext, T::ContextPrepared) => S::WaitingForProvider,
        (S::WaitingForProvider, T::ProviderStarted) => S::StreamingResponse,
        (S::StreamingResponse, T::StreamFinished { has_tool_calls }) => {
            if has_tool_calls {
                S::CollectingToolCalls
            } else {
                S::Completed
            }
        }
        (S::CollectingToolCalls, T::ApprovalRequested) => S::WaitingForApproval,
        (S::CollectingToolCalls, T::ToolsAutoStarted) => S::ExecutingTools,
        (S::WaitingForApproval, T::ApprovalGranted) => S::ExecutingTools,
        (S::WaitingForApproval, T::ApprovalDenied) => S::AppendingToolResults,
        (S::ExecutingTools, T::ToolsCompleted) => S::AppendingToolResults,
        (S::AppendingToolResults, T::ResultsAppended) => S::WaitingForProvider,
        // 终态入口（任意活跃态）
        (_, T::Complete) => S::Completed,
        (_, T::Cancel) => S::Cancelled,
        (_, T::Fail) => S::Failed,
        (_, T::MarkInterrupted) => S::Interrupted,
        _ => {
            return Err(TransitionError::Illegal {
                state: from,
                transition: t,
            })
        }
    };
    Ok(next)
}

/// 该转换「应该」产生的 [`AgentEvent`](agent_events::AgentEvent) 逻辑类别。
///
/// 用于让循环在转换后立即持久化对应事件，实现「每次转换都有事件」。
/// 个别转换（如 `ToolsAutoStarted`）没有独立事件，返回 [`EventHint::None`]，
/// 由循环按子事件（ToolExecutionStarted 等）自行持久化。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventHint {
    RunStarted,
    ContextPrepared,
    ProviderRequestStarted,
    RunCompleted,
    RunCancelled,
    RunFailed,
    MessageCommitted,
    ToolApprovalRequested,
    None,
}

/// 返回某转换对应的持久化事件类别提示。
pub fn event_hint(t: RunTransition) -> EventHint {
    use RunTransition as T;
    match t {
        T::Begin => EventHint::RunStarted,
        T::ContextPrepared => EventHint::ContextPrepared,
        T::ProviderStarted => EventHint::ProviderRequestStarted,
        T::StreamFinished {
            has_tool_calls: false,
        } => EventHint::RunCompleted,
        T::StreamFinished {
            has_tool_calls: true,
        } => EventHint::None,
        T::ApprovalRequested => EventHint::ToolApprovalRequested,
        T::ResultsAppended => EventHint::MessageCommitted,
        T::Complete => EventHint::RunCompleted,
        T::Cancel => EventHint::RunCancelled,
        T::Fail => EventHint::RunFailed,
        T::ApprovalGranted | T::ApprovalDenied | T::ToolsAutoStarted | T::ToolsCompleted => {
            EventHint::None
        }
        T::MarkInterrupted => EventHint::None,
    }
}

/// 有状态的 Run 状态机：封装当前状态并提供事件化转换。
#[derive(Clone, Debug, Default)]
pub struct RunStateMachine {
    state: RunState,
    /// 已发生的转换次数（便于断言「每次转换都事件化」）。
    transitions: u64,
}

impl RunStateMachine {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从指定状态恢复（用于崩溃恢复从事件重放）。
    pub fn from_state(state: RunState) -> Self {
        Self {
            state,
            transitions: 0,
        }
    }

    pub const fn state(&self) -> RunState {
        self.state
    }

    pub const fn transition_count(&self) -> u64 {
        self.transitions
    }

    /// 应用一次转换，返回新状态与应持久化的事件类别。
    pub fn apply(&mut self, t: RunTransition) -> Result<(RunState, EventHint), TransitionError> {
        let hint = event_hint(t);
        let next = transition(self.state, t)?;
        self.state = next;
        self.transitions += 1;
        Ok((next, hint))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_runs_full_loop_and_completes() {
        let mut sm = RunStateMachine::new();
        assert_eq!(sm.state(), RunState::Created);
        let (s, h) = sm.apply(RunTransition::Begin).unwrap();
        assert_eq!(s, RunState::PreparingContext);
        assert_eq!(h, EventHint::RunStarted);

        let (s, h) = sm.apply(RunTransition::ContextPrepared).unwrap();
        assert_eq!(s, RunState::WaitingForProvider);
        assert_eq!(h, EventHint::ContextPrepared);

        let (s, h) = sm.apply(RunTransition::ProviderStarted).unwrap();
        assert_eq!(s, RunState::StreamingResponse);
        assert_eq!(h, EventHint::ProviderRequestStarted);

        // 本轮有 tool call
        let (s, h) = sm
            .apply(RunTransition::StreamFinished {
                has_tool_calls: true,
            })
            .unwrap();
        assert_eq!(s, RunState::CollectingToolCalls);
        assert_eq!(h, EventHint::None);

        // 需要审批 → 通过 → 执行 → 回填 → 下一轮
        let (s, _) = sm.apply(RunTransition::ApprovalRequested).unwrap();
        assert_eq!(s, RunState::WaitingForApproval);
        let (s, _) = sm.apply(RunTransition::ApprovalGranted).unwrap();
        assert_eq!(s, RunState::ExecutingTools);
        let (s, _) = sm.apply(RunTransition::ToolsCompleted).unwrap();
        assert_eq!(s, RunState::AppendingToolResults);
        let (s, h) = sm.apply(RunTransition::ResultsAppended).unwrap();
        assert_eq!(s, RunState::WaitingForProvider);
        assert_eq!(h, EventHint::MessageCommitted);

        // 第二轮无 tool call → 完成
        let (s, _) = sm.apply(RunTransition::ProviderStarted).unwrap();
        assert_eq!(s, RunState::StreamingResponse);
        let (s, h) = sm
            .apply(RunTransition::StreamFinished {
                has_tool_calls: false,
            })
            .unwrap();
        assert_eq!(s, RunState::Completed);
        assert_eq!(h, EventHint::RunCompleted);
        assert!(sm.state().is_terminal());
    }

    #[test]
    fn auto_start_without_approval_is_allowed() {
        let mut sm = RunStateMachine::from_state(RunState::CollectingToolCalls);
        let (s, _) = sm.apply(RunTransition::ToolsAutoStarted).unwrap();
        assert_eq!(s, RunState::ExecutingTools);
    }

    #[test]
    fn denial_appends_result_and_loops_back() {
        let mut sm = RunStateMachine::from_state(RunState::CollectingToolCalls);
        sm.apply(RunTransition::ApprovalRequested).unwrap();
        let (s, _) = sm.apply(RunTransition::ApprovalDenied).unwrap();
        assert_eq!(s, RunState::AppendingToolResults);
        let (s, _) = sm.apply(RunTransition::ResultsAppended).unwrap();
        assert_eq!(s, RunState::WaitingForProvider);
    }

    #[test]
    fn cancel_and_fail_reachable_from_any_active_state() {
        for state in [
            RunState::Created,
            RunState::PreparingContext,
            RunState::WaitingForProvider,
            RunState::StreamingResponse,
            RunState::CollectingToolCalls,
            RunState::WaitingForApproval,
            RunState::ExecutingTools,
            RunState::AppendingToolResults,
        ] {
            let mut sm = RunStateMachine::from_state(state);
            let (s, h) = sm.apply(RunTransition::Cancel).unwrap();
            assert_eq!(s, RunState::Cancelled);
            assert_eq!(h, EventHint::RunCancelled);

            let mut sm = RunStateMachine::from_state(state);
            let (s, h) = sm.apply(RunTransition::Fail).unwrap();
            assert_eq!(s, RunState::Failed);
            assert_eq!(h, EventHint::RunFailed);
        }
    }

    #[test]
    fn terminal_states_reject_all_transitions() {
        for terminal in [
            RunState::Completed,
            RunState::Cancelled,
            RunState::Failed,
            RunState::Interrupted,
        ] {
            let mut sm = RunStateMachine::from_state(terminal);
            for t in [
                RunTransition::Begin,
                RunTransition::ContextPrepared,
                RunTransition::ProviderStarted,
                RunTransition::StreamFinished {
                    has_tool_calls: true,
                },
                RunTransition::Complete,
            ] {
                let err = sm.apply(t).unwrap_err();
                assert!(
                    matches!(err, TransitionError::FromTerminal { state, .. } if state == terminal),
                    "{terminal:?} 应拒绝 {t:?}"
                );
            }
        }
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        // Created 不能直接跳到 ProviderStarted
        let mut sm = RunStateMachine::from_state(RunState::Created);
        assert!(matches!(
            sm.apply(RunTransition::ProviderStarted).unwrap_err(),
            TransitionError::Illegal { .. }
        ));
        // WaitingForProvider 不能直接执行工具
        let mut sm = RunStateMachine::from_state(RunState::WaitingForProvider);
        assert!(matches!(
            sm.apply(RunTransition::ToolsAutoStarted).unwrap_err(),
            TransitionError::Illegal { .. }
        ));
    }

    #[test]
    fn mark_interrupted_only_for_recovery() {
        let mut sm = RunStateMachine::from_state(RunState::StreamingResponse);
        let (s, h) = sm.apply(RunTransition::MarkInterrupted).unwrap();
        assert_eq!(s, RunState::Interrupted);
        assert_eq!(h, EventHint::None);
        assert!(sm.state().is_terminal());
    }

    #[test]
    fn run_state_round_trips_and_displays() {
        for state in [
            RunState::Created,
            RunState::PreparingContext,
            RunState::WaitingForProvider,
            RunState::StreamingResponse,
            RunState::CollectingToolCalls,
            RunState::WaitingForApproval,
            RunState::ExecutingTools,
            RunState::AppendingToolResults,
            RunState::Completed,
            RunState::Cancelled,
            RunState::Failed,
            RunState::Interrupted,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let decoded: RunState = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, state);
            assert!(!format!("{state}").is_empty());
        }
    }
}
