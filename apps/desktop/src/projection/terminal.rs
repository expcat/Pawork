//! Inspector Terminal 投影：滚动文本，不是 VT100 / 本地 PTY。

use pawork_client::TerminalExitReason;
use serde_json::Value;

use super::DesktopProjection;

/// Inspector Terminal 面：滚动文本，不是 VT100 / 本地 PTY。
///
/// cwd 只承载 Host 可证事实：快照缺 cwd 键（旧 Host / 记账缺失）时用
/// [TERMINAL_CWD_UNKNOWN] 诚实占位，不臆造工作区根 "."。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalState {
    pub session_id: Option<String>,
    /// Host snapshot 的 owner_session；Desktop 将其解释为 terminal 所属
    /// workspace，而不是当前打开的 task/session。
    pub workspace_id: Option<String>,
    pub output: String,
    pub columns: u16,
    pub rows: u16,
    /// 仅 workspace 相对路径。
    pub cwd: String,
    /// Host 快照原样给出的 PTY 状态（running / exited / killed）；不从
    /// output 或本地 UI 动作猜测退出态。
    pub runtime_state: Option<String>,
    /// 实时广播被覆写的权威计数；非零时 UI 可诚实提示输出可能不完整。
    pub dropped_events: u64,
    /// Desktop 本连接已收到 Host resize 回执；snapshot 本身不含该事实，
    /// 重连/快照重建后不宣称已确认。
    pub resize_confirmed: bool,
    pub availability: TerminalAvailability,
}

/// Host 快照未提供 cwd 时的诚实占位（区别于「将创建在工作区根」的 "."）。
pub(crate) const TERMINAL_CWD_UNKNOWN: &str = "unknown";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalAvailability {
    Ready,
    Stale { reason: String },
    Failed { reason: String },
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            session_id: None,
            workspace_id: None,
            output: String::new(),
            columns: 80,
            rows: 24,
            cwd: ".".into(),
            runtime_state: None,
            dropped_events: 0,
            resize_confirmed: false,
            availability: TerminalAvailability::Stale {
                reason: "not started".into(),
            },
        }
    }
}

impl TerminalState {
    pub(crate) fn from_snapshot(entry: &Value) -> Option<Self> {
        let session_id = entry
            .get("terminal_session_id")
            .or_else(|| entry.get("id"))
            .and_then(Value::as_str)?;
        let workspace_id = entry
            .get("owner_session")
            .or_else(|| entry.get("workspace_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let runtime_state = entry
            .get("state")
            .and_then(Value::as_str)
            .map(str::to_string);
        let cwd = entry
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|cwd| !cwd.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| TERMINAL_CWD_UNKNOWN.to_string());
        let availability = match runtime_state.as_deref() {
            Some("running") => TerminalAvailability::Ready,
            Some(state @ ("exited" | "killed")) => TerminalAvailability::Stale {
                reason: format!("terminal {state}"),
            },
            Some(state) => TerminalAvailability::Stale {
                reason: format!("terminal state {state}"),
            },
            None => TerminalAvailability::Stale {
                reason: "terminal state unavailable".into(),
            },
        };
        Some(Self {
            session_id: Some(session_id.to_string()),
            workspace_id,
            columns: entry
                .get("columns")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(80),
            rows: entry
                .get("rows")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(24),
            runtime_state,
            dropped_events: entry
                .get("dropped_events")
                .or_else(|| entry.get("dropped"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            availability,
            cwd,
            ..Self::default()
        })
    }

    fn mark_stale(&mut self, reason: impl Into<String>) {
        self.availability = TerminalAvailability::Stale {
            reason: reason.into(),
        };
    }

    pub fn mark_failed(&mut self, reason: impl Into<String>) {
        self.availability = TerminalAvailability::Failed {
            reason: reason.into(),
        };
    }

    pub fn availability_label(&self) -> String {
        match &self.availability {
            TerminalAvailability::Ready => {
                self.runtime_state.clone().unwrap_or_else(|| "ready".into())
            }
            TerminalAvailability::Stale { reason } => format!("stale · {reason}"),
            TerminalAvailability::Failed { reason } => format!("failed · {reason}"),
        }
    }
}
pub(super) fn parse_terminal_sessions(data: &Value) -> Vec<TerminalState> {
    match data {
        Value::Array(entries) => entries
            .iter()
            .filter_map(TerminalState::from_snapshot)
            .collect(),
        Value::Object(_) => TerminalState::from_snapshot(data).into_iter().collect(),
        _ => Vec::new(),
    }
}

impl DesktopProjection {
    pub fn apply_terminal_output(&mut self, terminal_session_id: &str, delta: &str) -> bool {
        if let Some(terminal) = self
            .terminals
            .iter_mut()
            .find(|terminal| terminal.session_id.as_deref() == Some(terminal_session_id))
        {
            terminal.output.push_str(delta);
            // Replay 可能在当前 snapshot 之后补到 terminal 的历史输出。
            // exited/killed 等 snapshot 终态是更强事实，不能被旧输出复活。
            if terminal
                .runtime_state
                .as_deref()
                .is_none_or(|state| state == "running")
            {
                terminal.runtime_state = Some("running".into());
                terminal.availability = TerminalAvailability::Ready;
            }
            if self.terminal.session_id.as_deref() == Some(terminal_session_id) {
                self.terminal = terminal.clone();
                return true;
            }
            return false;
        }
        // TerminalOutput 可以先于 create 回执抵达。先按 id 缓存；在
        // TerminalCreated 给出权威 workspace 前不展示，避免任务切换期间串屏。
        let terminal = TerminalState {
            session_id: Some(terminal_session_id.to_string()),
            output: delta.to_string(),
            runtime_state: Some("running".into()),
            availability: TerminalAvailability::Ready,
            ..TerminalState::default()
        };
        self.terminals.push(terminal);
        false
    }

    pub fn apply_terminal_created(&mut self, workspace_id: String, terminal_session_id: String) {
        // 同 workspace 的无 id 占位只用于显示 create failure；成功后由真实
        // terminal 取代，避免占位在确定性选择中遮住新会话。
        self.terminals.retain(|terminal| {
            terminal.session_id.is_some()
                || terminal.workspace_id.as_deref() != Some(workspace_id.as_str())
        });
        let mut terminal = self
            .terminals
            .iter()
            .find(|terminal| terminal.session_id.as_deref() == Some(terminal_session_id.as_str()))
            .cloned()
            .or_else(|| {
                (self.terminal.session_id.as_deref() == Some(terminal_session_id.as_str()))
                    .then(|| self.terminal.clone())
            })
            .unwrap_or_else(|| TerminalState {
                session_id: Some(terminal_session_id.clone()),
                ..TerminalState::default()
            });
        // create 回执只补身份与运行态；Host 可能先广播首段 shell prompt，
        // 这里若重置整状态会清掉已经到达的 output。
        terminal.workspace_id = Some(workspace_id.clone());
        terminal.runtime_state = Some("running".into());
        terminal.availability = TerminalAvailability::Ready;
        if let Some(existing) = self
            .terminals
            .iter_mut()
            .find(|existing| existing.session_id.as_deref() == Some(terminal_session_id.as_str()))
        {
            *existing = terminal.clone();
        } else {
            self.terminals.push(terminal.clone());
        }
        if self.active_workspace_id() == Some(workspace_id.as_str())
            || self.terminal.workspace_id.as_deref() == Some(workspace_id.as_str())
        {
            self.terminal = terminal;
        }
    }

    /// 新建终端初始尺寸（ADR-050 D4）：create 回执后按 terminal_settings
    /// 生效值覆盖投影默认 80×24（尺寸仍在途——只写 columns/rows，不置
    /// resize_confirmed；随后那次 terminal_resize 的回执才确认）。
    pub fn apply_terminal_initial_size(
        &mut self,
        terminal_session_id: &str,
        columns: u16,
        rows: u16,
    ) -> bool {
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.columns = columns;
            terminal.rows = rows;
        })
    }

    pub fn mark_terminal_ready(&mut self, terminal_session_id: &str) -> bool {
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.runtime_state = Some("running".into());
            terminal.availability = TerminalAvailability::Ready;
        })
    }

    /// ADR-045：live 终态事件与快照 state 同口径——runtime_state 记录
    /// exited/killed/failed，availability 诚实降级 stale（旧输出不再复活，
    /// 见 apply_terminal_output 的终态闸门）。
    pub fn apply_terminal_exited(
        &mut self,
        terminal_session_id: &str,
        reason: TerminalExitReason,
    ) -> bool {
        let state = match reason {
            TerminalExitReason::Exited => "exited",
            TerminalExitReason::Killed => "killed",
            TerminalExitReason::Failed => "failed",
        };
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.runtime_state = Some(state.into());
            terminal.availability = TerminalAvailability::Stale {
                reason: format!("terminal {state}"),
            };
        })
    }

    /// terminal_close 清理已退出终端的回执：Host 已注销（该路径无 live
    /// 事件），本地同步移除；当前终端被移除时回到 not started 占位。
    pub fn remove_terminal(&mut self, terminal_session_id: &str) -> bool {
        let existed = self
            .terminals
            .iter()
            .any(|terminal| terminal.session_id.as_deref() == Some(terminal_session_id));
        if !existed {
            return false;
        }
        self.terminals
            .retain(|terminal| terminal.session_id.as_deref() != Some(terminal_session_id));
        if self.terminal.session_id.as_deref() == Some(terminal_session_id) {
            self.terminal = TerminalState::default();
        }
        true
    }

    pub fn mark_terminal_failed(
        &mut self,
        terminal_session_id: &str,
        reason: impl Into<String>,
    ) -> bool {
        let reason = reason.into();
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.mark_failed(reason.clone());
        })
    }

    /// write/resize 的瞬态失败归因：终端本体仍 running（Host 事实未变）
    /// 时不降级可用性——wire 无 live exit/failure 事件，一次 IO 失败不能
    /// 把可写终端锁死，报错交给调用方的 status_hint；非 running（含状态
    /// 未知）保持既有 Failed 归因。
    pub fn note_terminal_io_failed(
        &mut self,
        terminal_session_id: &str,
        reason: impl Into<String>,
    ) -> bool {
        let running = self
            .terminals
            .iter()
            .find(|terminal| terminal.session_id.as_deref() == Some(terminal_session_id))
            .or_else(|| {
                (self.terminal.session_id.as_deref() == Some(terminal_session_id))
                    .then(|| &self.terminal)
            })
            .is_some_and(|terminal| terminal.runtime_state.as_deref() == Some("running"));
        if running {
            return false;
        }
        let reason = reason.into();
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.mark_failed(reason.clone());
        })
    }

    /// create 回执不带 cwd（wire 冻结）；成功后由 UI 把请求 cwd 补到新
    /// 终端上，避免本地显示退回默认 "."。
    pub fn apply_terminal_cwd(&mut self, terminal_session_id: &str, cwd: &str) -> bool {
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.cwd = cwd.to_string();
        })
    }

    /// terminal_create 尚无 terminal id，按请求 workspace 保存失败归属；
    /// 用户切回该 workspace 时仍能看到真实原因，且不会污染当前 workspace。
    pub fn mark_terminal_create_failed(&mut self, workspace_id: &str, reason: impl Into<String>) {
        let reason = reason.into();
        let mut failed = self
            .terminals
            .iter()
            .find(|terminal| {
                terminal.workspace_id.as_deref() == Some(workspace_id)
                    && terminal.session_id.is_none()
            })
            .cloned()
            .unwrap_or_else(|| TerminalState {
                workspace_id: Some(workspace_id.to_string()),
                ..TerminalState::default()
            });
        failed.mark_failed(reason);
        if let Some(existing) = self.terminals.iter_mut().find(|terminal| {
            terminal.workspace_id.as_deref() == Some(workspace_id) && terminal.session_id.is_none()
        }) {
            *existing = failed.clone();
        } else {
            self.terminals.push(failed.clone());
        }
        if self.active_workspace_id() == Some(workspace_id)
            || self.terminal.workspace_id.as_deref() == Some(workspace_id)
        {
            self.terminal = failed;
        }
    }

    pub fn apply_terminal_resize(
        &mut self,
        terminal_session_id: &str,
        columns: u16,
        rows: u16,
    ) -> bool {
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.columns = columns;
            terminal.rows = rows;
            terminal.resize_confirmed = true;
            terminal.runtime_state = Some("running".into());
            terminal.availability = TerminalAvailability::Ready;
        })
    }

    fn update_terminal(
        &mut self,
        terminal_session_id: &str,
        update: impl Fn(&mut TerminalState),
    ) -> bool {
        let mut found = false;
        if let Some(terminal) = self
            .terminals
            .iter_mut()
            .find(|terminal| terminal.session_id.as_deref() == Some(terminal_session_id))
        {
            update(terminal);
            found = true;
            if self.terminal.session_id.as_deref() == Some(terminal_session_id) {
                self.terminal = terminal.clone();
            }
        } else if self.terminal.session_id.as_deref() == Some(terminal_session_id) {
            update(&mut self.terminal);
            self.terminals.push(self.terminal.clone());
            found = true;
        }
        found
    }

    pub fn select_terminal_for_workspace(&mut self, workspace_id: Option<&str>) -> bool {
        let current = self.terminal.session_id.as_deref();
        let selected = current
            .and_then(|id| {
                self.terminals.iter().find(|terminal| {
                    terminal.session_id.as_deref() == Some(id)
                        && terminal.workspace_id.as_deref() == workspace_id
                })
            })
            .cloned()
            .or_else(|| {
                self.terminals
                    .iter()
                    .filter(|terminal| terminal.workspace_id.as_deref() == workspace_id)
                    .min_by_key(|terminal| {
                        (
                            usize::from(terminal.runtime_state.as_deref() != Some("running")),
                            terminal.session_id.clone().unwrap_or_default(),
                        )
                    })
                    .cloned()
            })
            .unwrap_or_else(|| TerminalState {
                workspace_id: workspace_id.map(str::to_string),
                ..TerminalState::default()
            });
        let changed = self.terminal != selected;
        self.terminal = selected;
        changed
    }

    pub fn mark_terminals_stale(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        for terminal in &mut self.terminals {
            terminal.mark_stale(reason.clone());
        }
        self.terminal.mark_stale(reason);
    }

    pub(super) fn restore_terminal_availability(&mut self) {
        for terminal in &mut self.terminals {
            terminal.availability = match terminal.runtime_state.as_deref() {
                Some("running") => TerminalAvailability::Ready,
                Some(state) => TerminalAvailability::Stale {
                    reason: format!("terminal {state}"),
                },
                None => TerminalAvailability::Stale {
                    reason: "terminal state unavailable".into(),
                },
            };
        }
        if let Some(current) = self
            .terminals
            .iter()
            .find(|terminal| terminal.session_id == self.terminal.session_id)
        {
            self.terminal = current.clone();
        }
    }
}
