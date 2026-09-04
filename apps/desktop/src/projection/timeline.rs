//! Timeline 渲染行组装与摘要文案（render / AX 同源）。

use pawork_client::{TimelineItemKind, TimelinePage};

use super::{DesktopProjection, ForkBoundary, TimelineEntry, TimelineEntryKind};

/// Timeline 渲染行（R4 Wave A F-08 组装纯数据）：连续 ToolCall（同 run
/// 相邻）合并为 tool activity 组；run 终态条目（fork_boundary 单点判型）
/// 与紧邻其前的 tool 组合成 Run 摘要区域。索引指向 timeline.entries，
/// 行序与条目序一致；approval 卡由 UI 作为 list 末项另行附加，不占行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineRow {
    /// user / assistant 消息条目。
    Message { entry_index: usize },
    /// error 条目（Diagnostic）。
    Error { entry_index: usize },
    /// tool activity 组：同 run 相邻的连续 ToolCall 条目。
    ToolGroup { entry_indices: Vec<usize> },
    /// 非终态 RunState 中间相位行（disabled 单行，不纳入摘要；Interrupted
    /// 无 fork 边界，同按本行处理）。
    RunPhase { entry_index: usize },
    /// run 终态摘要区域：终态条目 + 紧邻前文 tool 组（可无）。
    RunSummary {
        group: Option<Vec<usize>>,
        terminal: usize,
    },
}

/// timeline 行组装的 run 终态判定：reducer 的 fork_boundary 是唯一定义源
/// （历史 RunCompleted/Cancelled/Failed 与 live 对应态；Interrupted 无边
/// 界），禁止对 kind 文案做字符串匹配。
fn is_run_terminal(entry: &TimelineEntry) -> bool {
    entry.fork_boundary.is_some()
}

/// Failed 终态摘要原因：protocol reducer 历史臂（RunFailed）把 provider
/// 失败原因写进 RunState 标签 `run failed · {reason}`，live 臂（RunChanged）
/// 只有 `run failed` 无原因。此处仅剥离前缀取原因原文（原因内部再含
/// ` · ` 不受影响）；标签格式变化须与 protocol reducer 同批调整。
fn failed_run_reason(label: &str) -> Option<&str> {
    label
        .strip_prefix("run failed · ")
        .filter(|reason| !reason.is_empty())
}

/// Run 摘要卡内容（F-08 诚实文案）：无权威数据用通用描述，禁止编造
/// 耗时 / 数字；失败原因取 reducer 标签原文；非终态条目返回 None。
pub fn run_summary_texts(entry: &TimelineEntry) -> Option<(&'static str, String)> {
    match entry.fork_boundary {
        Some(ForkBoundary::Completed) => Some((
            "Ready for review",
            "The run finished. Review the changes from this turn.".to_string(),
        )),
        Some(ForkBoundary::Cancelled) => Some((
            "Run cancelled",
            "The run was cancelled. Output from this turn is preserved.".to_string(),
        )),
        // 失败摘要是唯一的失败原因出口（Error 仅来自 Diagnostic，RunFailed
        // 不产生 Error 条目）：有原因用原文，无原因 / 标签剥离失败走通用
        // 兜底，不指向不存在的"上方错误详情"。
        Some(ForkBoundary::Failed) => Some((
            "Run failed",
            match &entry.kind {
                TimelineEntryKind::RunState(label) => failed_run_reason(label)
                    .map(str::to_string)
                    .unwrap_or_else(|| "The run failed.".to_string()),
                _ => "The run failed.".to_string(),
            },
        )),
        None => None,
    }
}

/// Timeline 页脚终态词（§4.4：completed / cancelled / failed；非终态 None）。
pub fn run_footer_label(entry: &TimelineEntry) -> Option<&'static str> {
    match entry.fork_boundary {
        Some(ForkBoundary::Completed) => Some("Run completed"),
        Some(ForkBoundary::Cancelled) => Some("Run cancelled"),
        Some(ForkBoundary::Failed) => Some("Run failed"),
        None => None,
    }
}

impl DesktopProjection {
    pub fn apply_timeline_page(&mut self, page: &TimelinePage) {
        for item in &page.items {
            // 条目语义（去重 / committed 替换 / tool 双键回填）走 protocol
            // reducer；这里只保留历史条目携带的 UI 态副作用。
            self.timeline.apply_item(item);
            match &item.kind {
                TimelineItemKind::RunCompleted
                | TimelineItemKind::RunCancelled
                | TimelineItemKind::RunFailed => {
                    // run 终态可证明该 run 不再有未决议审批；历史中的工具
                    // 完成 / 审批响应则可能属于同 run 的更早工具，不能据此
                    // 清除 snapshot 权威的当前 pending。
                    self.clear_pending_for_run(item.run_id.as_deref());
                }
                _ => {}
            }
        }
    }
    pub fn timeline_rows(&self) -> Vec<TimelineRow> {
        let entries = &self.timeline.entries;
        let mut rows = Vec::new();
        let mut ix = 0;
        while ix < entries.len() {
            match &entries[ix].kind {
                TimelineEntryKind::UserMessage { .. }
                | TimelineEntryKind::AssistantMessage { .. } => {
                    rows.push(TimelineRow::Message { entry_index: ix });
                    ix += 1;
                }
                TimelineEntryKind::Error(_) => {
                    rows.push(TimelineRow::Error { entry_index: ix });
                    ix += 1;
                }
                TimelineEntryKind::ToolCall { .. } => {
                    let run_id = entries[ix].run_id.clone();
                    let mut group = vec![ix];
                    ix += 1;
                    while ix < entries.len() {
                        let next = &entries[ix];
                        if !matches!(next.kind, TimelineEntryKind::ToolCall { .. })
                            || next.run_id != run_id
                        {
                            break;
                        }
                        group.push(ix);
                        ix += 1;
                    }
                    // 紧邻其后的 run 终态条目吸收该组为摘要区域；终态必须
                    // 与本组同 run（含 None==None 的未知 run 近邻），防止
                    // 跨 run 吞并（审查 P2）。
                    if ix < entries.len()
                        && is_run_terminal(&entries[ix])
                        && entries[ix].run_id == run_id
                    {
                        rows.push(TimelineRow::RunSummary {
                            group: Some(group),
                            terminal: ix,
                        });
                        ix += 1;
                    } else {
                        rows.push(TimelineRow::ToolGroup {
                            entry_indices: group,
                        });
                    }
                }
                TimelineEntryKind::RunState(_) => {
                    if is_run_terminal(&entries[ix]) {
                        rows.push(TimelineRow::RunSummary {
                            group: None,
                            terminal: ix,
                        });
                    } else {
                        rows.push(TimelineRow::RunPhase { entry_index: ix });
                    }
                    ix += 1;
                }
            }
        }
        rows
    }
    /// MessageSent 本地乐观回显：wire 对 MessageCommitted 返回 None（用户
    /// 消息不进实时流），发送回执即上屏，重选 / 重连后由快照重放的持久化
    /// 行替换。只在 active session 追加；是否追加以返回值告知调用方 bump
    /// 时间线代次。禁止改 protocol 共享 reducer——这里直接 push。
    pub fn note_user_echo(
        &mut self,
        session_id: &str,
        run_id: &str,
        text: &str,
        now_ms: u64,
    ) -> bool {
        if self.active_session_id.as_deref() != Some(session_id) {
            return false;
        }
        // 借用当前最大 wire sequence（entries 升序）：不进 seen、不占号段，
        // 后续 wire 事件严格更大，insert_entry 有序插入自然落在 echo 之后；
        // 重复 wire 事件仍被 seen 去重，不会双插。entries 为空时兜底 0。
        let sequence = self
            .timeline
            .entries
            .last()
            .map(|entry| entry.sequence)
            .unwrap_or(0);
        self.timeline.entries.push(TimelineEntry {
            sequence,
            event_id: format!("local-echo-{run_id}"),
            kind: TimelineEntryKind::UserMessage {
                text: text.to_string(),
            },
            fork_boundary: None,
            timestamp: now_ms.to_string(),
            run_id: Some(run_id.to_string()),
        });
        true
    }

}
