//! Changes 面（R8 波 D）：Files / Summary 二级页签、文件清单、DiffView 与
//! Inspector 折叠态的 ActivityPopover。数据全部来自 diff_list_files /
//! diff_get 的 Host 响应；未拉取或失败时诚实显示 unavailable / 错误，不画
//! 演示数据（design/README.md §8.5）。

use std::collections::BTreeMap;

use gpui::{div, prelude::*, px, Context, Div, MouseDownEvent, ScrollHandle};

use crate::controller::{DiffFileDetail, DiffFileSummary, DiffLineKind, GitDiffInfo};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{MenuPanel, MenuRow};
use crate::ui::components::label::Label;
use crate::ui::components::list_row::ListRow;
use crate::ui::theme::{dark, font, metrics};

use super::{AppView, MenuKind};

/// Changes 内容区二级页签（§8.5：字号 17；R6 Wave A：56px 条 + accent
/// 下划线，与顶层 58px 层次区分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ChangesTab {
    #[default]
    Files,
    Summary,
}

/// 文件清单拉取状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ChangesFetch {
    /// 从未拉取（或会话切换清空）。
    Idle,
    Fetching,
    Ready,
    Failed(String),
}

/// 选中文件 diff 的拉取状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DiffFetch {
    Idle,
    Fetching,
    Ready(DiffFileDetail),
    Failed(String),
}

/// Changes 面状态（含滚动句柄；Files / Summary 共用 list_scroll，
/// DiffView 独立，页签切换不丢滚动位置）。
pub(super) struct ChangesPanelState {
    pub tab: ChangesTab,
    epoch: u64,
    diff_epoch: u64,
    pub fetch: ChangesFetch,
    pub session_id: Option<String>,
    pub files: Vec<DiffFileSummary>,
    pub git: Option<GitDiffInfo>,
    pub selected: Option<String>,
    pub diff: DiffFetch,
    /// 断线/重连期间保留最后一次成功数据，但必须显式标成 stale；epoch 同时
    /// 失效，旧响应不能在新连接上把 stale 静默抹掉。
    pub stale_reason: Option<String>,
    pub list_scroll: ScrollHandle,
    pub diff_scroll: ScrollHandle,
}

impl Default for ChangesPanelState {
    fn default() -> Self {
        Self {
            tab: ChangesTab::default(),
            epoch: 0,
            diff_epoch: 0,
            fetch: ChangesFetch::Idle,
            session_id: None,
            files: Vec::new(),
            git: None,
            selected: None,
            diff: DiffFetch::Idle,
            stale_reason: None,
            list_scroll: ScrollHandle::new(),
            diff_scroll: ScrollHandle::new(),
        }
    }
}

impl ChangesPanelState {
    /// 开始一次清单拉取；返回新代次（响应带回，过期丢弃）。
    pub(super) fn begin_refresh(&mut self) -> u64 {
        self.epoch += 1;
        self.fetch = ChangesFetch::Fetching;
        self.stale_reason = None;
        self.epoch
    }

    pub(super) fn mark_failed(&mut self, reason: &str) {
        self.fetch = ChangesFetch::Failed(reason.into());
        self.stale_reason = None;
    }

    pub(super) fn mark_failed_for_epoch(&mut self, epoch: u64, reason: &str) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.mark_failed(reason);
        true
    }

    pub(super) fn mark_stale(&mut self, reason: &str) {
        self.epoch += 1;
        self.diff_epoch += 1;
        if self.fetch == ChangesFetch::Fetching {
            self.fetch = if self.session_id.is_some() || !self.files.is_empty() {
                ChangesFetch::Ready
            } else {
                ChangesFetch::Idle
            };
        }
        if self.diff == DiffFetch::Fetching {
            self.diff = DiffFetch::Failed(reason.into());
        }
        self.stale_reason = Some(reason.into());
    }

    /// 应用清单响应；选中路径从清单消失时清空选中与 diff。
    pub(super) fn apply_files(
        &mut self,
        epoch: u64,
        session_id: Option<String>,
        files: Vec<DiffFileSummary>,
        git: Option<GitDiffInfo>,
    ) -> bool {
        if epoch != self.epoch {
            return false;
        }
        if !files
            .iter()
            .any(|file| Some(&file.path) == self.selected.as_ref())
        {
            self.selected = None;
            self.diff = DiffFetch::Idle;
        }
        self.session_id = session_id;
        self.files = files;
        self.git = git;
        self.fetch = ChangesFetch::Ready;
        self.stale_reason = None;
        true
    }

    /// 选中文件并开始 diff 拉取；返回新 diff 代次。
    pub(super) fn begin_diff_fetch(&mut self, path: &str) -> u64 {
        self.selected = Some(path.to_string());
        self.diff_epoch += 1;
        self.diff = DiffFetch::Fetching;
        self.diff_epoch
    }

    pub(super) fn mark_diff_failed(&mut self, reason: &str) {
        if self.selected.is_some() {
            self.diff = DiffFetch::Failed(reason.into());
        }
    }

    pub(super) fn mark_diff_failed_for_epoch(
        &mut self,
        epoch: u64,
        path: &str,
        reason: &str,
    ) -> bool {
        if epoch != self.diff_epoch || self.selected.as_deref() != Some(path) {
            return false;
        }
        self.mark_diff_failed(reason);
        true
    }

    /// 应用 diff 响应；代次或选中路径不匹配（用户已改选 / 重新拉清单）时丢弃。
    pub(super) fn apply_diff(
        &mut self,
        epoch: u64,
        path: &str,
        file: Option<DiffFileDetail>,
    ) -> bool {
        if epoch != self.diff_epoch || self.selected.as_deref() != Some(path) {
            return false;
        }
        self.diff = match file {
            Some(file) => DiffFetch::Ready(file),
            None => DiffFetch::Failed("file is no longer part of the diff".into()),
        };
        true
    }

    /// 会话切换：清空旧会话数据（新清单由随后的刷新拉取）。
    pub(super) fn reset_for_session(&mut self) {
        self.epoch += 1;
        self.diff_epoch += 1;
        self.fetch = ChangesFetch::Idle;
        self.session_id = None;
        self.files.clear();
        self.git = None;
        self.selected = None;
        self.diff = DiffFetch::Idle;
        self.stale_reason = None;
    }

    /// (文件数, 总新增, 总删除)。
    pub(super) fn totals(&self) -> (usize, u64, u64) {
        let additions: u64 = self.files.iter().map(|file| file.additions).sum();
        let deletions: u64 = self.files.iter().map(|file| file.deletions).sum();
        (self.files.len(), additions, deletions)
    }

    /// 按 status 分组计数（BTreeMap 保证展示顺序稳定）。
    pub(super) fn status_counts(&self) -> BTreeMap<&str, usize> {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for file in &self.files {
            *counts.entry(file.status.as_str()).or_default() += 1;
        }
        counts
    }

    /// ActivityPopover 首行摘要（§8.5：N files · +A/−D；未就绪显示
    /// unavailable，不显示 0）。
    pub(super) fn activity_summary(&self) -> String {
        if self.stale_reason.is_some() {
            return "stale".into();
        }
        match &self.fetch {
            ChangesFetch::Ready => {
                let (files, additions, deletions) = self.totals();
                let plural = if files == 1 { "" } else { "s" };
                format!("{files} file{plural} · +{additions}/−{deletions}")
            }
            ChangesFetch::Idle | ChangesFetch::Fetching | ChangesFetch::Failed(_) => {
                "unavailable".into()
            }
        }
    }
}

impl AppView {
    /// Changes 页内容（二级页签 + Files / Summary）。
    pub(super) fn changes_element(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.changes.tab;
        let secondary_tab =
            |id: &'static str, label: &'static str, current: bool, target: ChangesTab| {
                div()
                    .id(id)
                    .relative()
                    .w(px(metrics::CHANGES_TAB_WIDTH))
                    .h(px(metrics::CHANGES_TAB_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tab_stop(true)
                    .track_focus(&self.changes_tab_focus[target as usize])
                    .focus(|style| style.border_1().border_color(dark().accent.primary))
                    .text_size(px(font::BODY_SM))
                    .text_color(if current {
                        dark().text.primary
                    } else {
                        dark().text.secondary
                    })
                    .hover(move |style| style.text_color(dark().text.primary))
                    .child(div().child(label))
                    .when(current, |tab| {
                        tab.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .w_full()
                                .h(px(metrics::TAB_UNDERLINE_HEIGHT))
                                .bg(dark().accent.primary),
                        )
                    })
                    .on_click(cx.listener(move |view, event, _window, cx| {
                        if view.consume_button_key_click(id, event) {
                            return;
                        }
                        view.on_select_changes_tab(target, cx);
                    }))
            };
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .pl_3()
            .pr_2()
            .h(px(metrics::CHANGES_TAB_HEIGHT))
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(secondary_tab(
                "changes-tab-files",
                "Files",
                tab == ChangesTab::Files,
                ChangesTab::Files,
            ))
            .child(secondary_tab(
                "changes-tab-summary",
                "Summary",
                tab == ChangesTab::Summary,
                ChangesTab::Summary,
            ))
            .child(div().flex_1())
            .child(
                Button::new("changes-refresh")
                    .variant(ButtonVariant::Ghost)
                    .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                    .text_size(font::XS)
                    .text_color(dark().text.secondary)
                    .label("↻")
                    .tooltip("Refresh changes")
                    .track_focus(&self.changes_refresh_focus)
                    .on_click(cx.listener(|view, event, _window, cx| {
                        if view.consume_button_key_click("changes-refresh", event) {
                            return;
                        }
                        view.refresh_changes(cx);
                    })),
            );
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(dark().border.subtle)
                    .text_size(px(font::XS))
                    .text_color(dark().text.tertiary)
                    .child("Current working tree · filtered to latest task paths"),
            )
            .when_some(self.changes.stale_reason.clone(), |block, reason| {
                block.child(
                    div()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(dark().semantic.warning_text)
                        .text_size(px(font::XS))
                        .text_color(dark().semantic.warning_text)
                        .child(format!("Stale data · {reason}")),
                )
            })
            .when_some(self.changes_session_mismatch(), |block, data_session| {
                // host diff_* 固定解析 latest 会话（P2-1）：数据会话与当前
                // 查看会话不一致时如实标注，不静默张冠李戴。
                block.child(
                    div()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(dark().border.subtle)
                        .text_size(px(font::XS))
                        .text_color(dark().text.tertiary)
                        .child(format!(
                            "Showing changes for latest session {data_session} — not the active session."
                        )),
                )
            })
            .child(match tab {
                ChangesTab::Files => self.changes_files_element(cx).into_any_element(),
                ChangesTab::Summary => self.changes_summary_element().into_any_element(),
            })
    }

    /// Changes 数据会话与当前查看会话不一致时返回数据会话 id（如实标注用）。
    fn changes_session_mismatch(&self) -> Option<String> {
        session_mismatch(
            self.changes.session_id.as_deref(),
            self.projection.active_session_id.as_deref(),
        )
        .map(str::to_string)
    }

    fn changes_files_element(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.changes.fetch {
            ChangesFetch::Idle => changes_placeholder("Changes unavailable.").into_any_element(),
            ChangesFetch::Fetching if self.changes.files.is_empty() => {
                changes_placeholder("Loading changes…").into_any_element()
            }
            ChangesFetch::Failed(reason) => {
                changes_placeholder_colored(reason.clone(), dark().semantic.danger_text)
                    .into_any_element()
            }
            ChangesFetch::Fetching | ChangesFetch::Ready => {
                if self.changes.files.is_empty() {
                    if self.changes.session_id.is_none() {
                        changes_placeholder("No active session.").into_any_element()
                    } else {
                        changes_placeholder("No changes in this session yet.").into_any_element()
                    }
                } else {
                    self.changes_file_list_element(cx).into_any_element()
                }
            }
        };
        div().flex().flex_col().flex_1().min_h_0().child(body)
    }

    fn changes_file_list_element(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .id("changes-file-list")
            .flex()
            .flex_col()
            .track_scroll(&self.changes.list_scroll)
            .overflow_y_scroll()
            .max_h(px(metrics::CHANGES_FILE_LIST_MAX_HEIGHT))
            .border_b_1()
            .border_color(dark().border.subtle);
        for file in &self.changes.files {
            let selected = Some(&file.path) == self.changes.selected.as_ref();
            let path = file.path.clone();
            let focus = self.changes_file_focus.get(&path).cloned();
            let mut row = ListRow::task(format!("changes-file-{}", file.path), selected)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(font::SM))
                        .text_color(dark().text.primary)
                        .child(path.clone()),
                )
                .child(
                    Label::new(file.status.clone())
                        .size(font::XS)
                        .color(dark().text.tertiary),
                )
                .child(
                    Label::new(format!("+{}/−{}", file.additions, file.deletions))
                        .size(font::XS)
                        .color(dark().text.secondary),
                );
            if let Some(focus) = focus {
                row = row.track_focus(&focus);
            }
            list = list.child(row.on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_row_key_click(&path, event) {
                    return;
                }
                view.on_select_diff_file(&path, cx);
            })));
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(list)
            .child(self.diff_view_element())
    }

    /// DiffView：hunk 头 raised 底 + secondary 字；行级语义着色（§8.5），
    /// 等宽字体、长行横向滚动。
    fn diff_view_element(&self) -> impl IntoElement {
        match &self.changes.diff {
            DiffFetch::Idle => {
                changes_placeholder("Select a file to view its diff.").into_any_element()
            }
            DiffFetch::Fetching => changes_placeholder("Loading diff…").into_any_element(),
            DiffFetch::Failed(reason) => {
                changes_placeholder_colored(reason.clone(), dark().semantic.danger_text)
                    .into_any_element()
            }
            DiffFetch::Ready(file) => {
                let mut body = div()
                    .id("diff-view")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.changes.diff_scroll)
                    .overflow_y_scroll()
                    .overflow_x_scroll()
                    .font_family(font::MONO)
                    .text_size(px(font::SM));
                if file.binary {
                    body = body.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_color(dark().text.secondary)
                            .child("Binary file — not rendered."),
                    );
                } else if file.hunks.is_empty() {
                    body = body.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_color(dark().text.secondary)
                            .child("No hunks in response."),
                    );
                } else {
                    for hunk in &file.hunks {
                        body = body.child(
                            div()
                                .w_full()
                                .px_2()
                                .py_1()
                                .bg(dark().surface.raised)
                                .text_color(dark().text.secondary)
                                .whitespace_nowrap()
                                .child(hunk.header.clone()),
                        );
                        for line in &hunk.lines {
                            let (prefix, bg, color) = match line.kind {
                                DiffLineKind::Addition => {
                                    ('+', dark().semantic.success_bg, dark().text.on_accent)
                                }
                                DiffLineKind::Deletion => {
                                    ('-', dark().semantic.danger_bg, dark().text.on_accent)
                                }
                                DiffLineKind::Context => {
                                    (' ', dark().bg.panel, dark().text.primary)
                                }
                            };
                            body = body.child(
                                div()
                                    .w_full()
                                    .px_2()
                                    .bg(bg)
                                    .text_color(color)
                                    .whitespace_nowrap()
                                    .child(format!("{prefix}{}", line.text)),
                            );
                        }
                    }
                }
                body.into_any_element()
            }
        }
    }

    fn changes_summary_element(&self) -> impl IntoElement {
        let body = match &self.changes.fetch {
            ChangesFetch::Idle => changes_placeholder("Changes unavailable."),
            ChangesFetch::Fetching if self.changes.files.is_empty() => {
                changes_placeholder("Loading changes…")
            }
            ChangesFetch::Failed(reason) => {
                changes_placeholder_colored(reason.clone(), dark().semantic.danger_text)
            }
            ChangesFetch::Fetching | ChangesFetch::Ready => {
                let (files, additions, deletions) = self.changes.totals();
                let status_line = self
                    .changes
                    .status_counts()
                    .into_iter()
                    .map(|(status, count)| format!("{status} {count}"))
                    .collect::<Vec<_>>()
                    .join(" · ");
                let session = self
                    .changes
                    .session_id
                    .clone()
                    .unwrap_or_else(|| "no session".into());
                let git = self.changes.git.clone().unwrap_or_default();
                let branch = git.branch.unwrap_or_else(|| "unknown".into());
                let dirty_files = git
                    .dirty_files
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "unknown".into());
                let work_dir = git.work_dir.unwrap_or_else(|| "unknown".into());
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2()
                    .text_size(px(font::SM))
                    .child(summary_row("Session", session))
                    .child(summary_row("Files", files.to_string()))
                    .child(summary_row("Lines", format!("+{additions} / −{deletions}")))
                    .when(!status_line.is_empty(), |block| {
                        block.child(summary_row("By status", status_line))
                    })
                    .child(summary_row("Branch", branch))
                    .child(summary_row("Dirty files", dirty_files))
                    .child(summary_row("Work dir", work_dir))
            }
        };
        div().flex().flex_col().flex_1().min_h_0().child(body)
    }

    /// ActivityPopover（Inspector 折叠态，R6 Wave A 起由 Workspace Header
    /// 触发器弹出；§8.2 浮层形态，§8.5 宽约 320px）。
    /// 面板标题 Activity；Changes 摘要来自真实 diff 数据。Agent 状态分区
    /// 属 S11 面，不画假数据。
    pub(super) fn activity_popover_element(&self, cx: &mut Context<Self>) -> MenuPanel {
        let summary = self.changes.activity_summary();
        let mismatch = self.changes_session_mismatch();
        let panel = MenuPanel::new("activity-popover")
            .max_height(metrics::ACTIVITY_POPOVER_HEIGHT + 8.0)
            .dismiss_on_outside(cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                view.dismiss_menu_on_outside(MenuKind::Activity, event.position, cx);
            }))
            .child(
                div()
                    .w(px(metrics::ACTIVITY_POPOVER_WIDTH))
                    .h(px(metrics::ACTIVITY_POPOVER_HEIGHT))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .child(
                        Label::new("Activity")
                            .size(font::BODY)
                            .color(dark().text.primary),
                    )
                    .child(
                        Label::new("Changes")
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    )
                    .child(
                        MenuRow::new("activity-open-changes")
                            .label(summary)
                            .highlighted(self.menu_highlight_effective(0) == 0)
                            .on_click(cx.listener(|view, _event, window, cx| {
                                view.on_activity_open_changes(window, cx);
                            })),
                    ),
            );
        // 与 Changes 面板同一 P2-1 标注：数据来自 latest 会话时如实说明。
        match mismatch {
            Some(data_session) => panel.child(
                div()
                    .px_1()
                    .pb_1()
                    .text_size(px(font::XS))
                    .text_color(dark().text.tertiary)
                    .child(format!("from latest session {data_session}")),
            ),
            None => panel,
        }
    }
}

/// 数据会话与查看会话均为 Some 且不同 → Some(数据会话 id)；否则 None。
fn session_mismatch<'a>(data: Option<&'a str>, active: Option<&str>) -> Option<&'a str> {
    let data = data?;
    let active = active?;
    (data != active).then_some(data)
}

fn changes_placeholder(text: impl Into<String>) -> Div {
    changes_placeholder_colored(text, dark().text.secondary)
}

fn changes_placeholder_colored(text: impl Into<String>, color: gpui::Rgba) -> Div {
    div()
        .flex()
        .flex_1()
        .items_start()
        .justify_start()
        .p_2()
        .text_size(px(font::SM))
        .text_color(color)
        .child(text.into())
}

fn summary_row(label: &str, value: String) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .gap_2()
        .child(
            div()
                .w(px(metrics::SUMMARY_LABEL_WIDTH))
                .text_color(dark().text.secondary)
                .child(label.to_string()),
        )
        .child(div().min_w_0().child(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_state(files: Vec<DiffFileSummary>) -> ChangesPanelState {
        let mut state = ChangesPanelState::default();
        let epoch = state.begin_refresh();
        assert!(state.apply_files(epoch, Some("s-1".into()), files, None));
        state
    }

    #[test]
    fn activity_summary_formats_ready_counts() {
        let state = ready_state(vec![
            DiffFileSummary {
                path: "a.rs".into(),
                status: "modified".into(),
                additions: 3,
                deletions: 1,
                binary: false,
            },
            DiffFileSummary {
                path: "b.rs".into(),
                status: "added".into(),
                additions: 9,
                deletions: 4,
                binary: false,
            },
        ]);
        assert_eq!(state.activity_summary(), "2 files · +12/−5");
    }

    #[test]
    fn activity_summary_uses_singular_file_and_honest_unavailable() {
        let state = ready_state(vec![DiffFileSummary {
            path: "only.rs".into(),
            status: "modified".into(),
            additions: 2,
            deletions: 0,
            binary: false,
        }]);
        assert_eq!(state.activity_summary(), "1 file · +2/−0");

        assert_eq!(
            ChangesPanelState::default().activity_summary(),
            "unavailable"
        );
        let mut fetching = ChangesPanelState::default();
        fetching.begin_refresh();
        assert_eq!(fetching.activity_summary(), "unavailable");
        let mut failed = ChangesPanelState::default();
        failed.mark_failed("boom");
        assert_eq!(failed.activity_summary(), "unavailable");
    }

    #[test]
    fn secondary_tab_defaults_to_files() {
        assert_eq!(ChangesPanelState::default().tab, ChangesTab::Files);
    }

    #[test]
    fn apply_files_rejects_stale_epoch_and_drops_missing_selection() {
        let mut state = ChangesPanelState::default();
        let first = state.begin_refresh();
        let second = state.begin_refresh();
        assert!(!state.apply_files(first, Some("s-1".into()), Vec::new(), None));
        assert!(state.apply_files(
            second,
            Some("s-1".into()),
            vec![DiffFileSummary {
                path: "a.rs".into(),
                status: "modified".into(),
                additions: 1,
                deletions: 0,
                binary: false
            }],
            None
        ));

        let epoch = state.begin_diff_fetch("a.rs");
        assert!(state.apply_diff(
            epoch,
            "a.rs",
            Some(DiffFileDetail {
                path: "a.rs".into(),
                previous_path: None,
                status: "modified".into(),
                binary: false,
                additions: 1,
                deletions: 0,
                hunks: Vec::new(),
            })
        ));
        // 新清单里选中路径消失：清空选中与 diff。
        let refresh = state.begin_refresh();
        assert!(state.apply_files(refresh, Some("s-1".into()), Vec::new(), None));
        assert_eq!(state.selected, None);
        assert_eq!(state.diff, DiffFetch::Idle);
    }

    #[test]
    fn apply_diff_rejects_stale_epoch_or_mismatched_selection() {
        let mut state = ChangesPanelState::default();
        let epoch = state.begin_diff_fetch("a.rs");
        let detail = || DiffFileDetail {
            path: "a.rs".into(),
            previous_path: None,
            status: "modified".into(),
            binary: false,
            additions: 1,
            deletions: 0,
            hunks: Vec::new(),
        };
        assert!(!state.apply_diff(epoch + 1, "a.rs", Some(detail())));
        assert!(!state.apply_diff(epoch, "b.rs", Some(detail())));
        assert!(state.apply_diff(epoch, "a.rs", None));
        assert_eq!(
            state.diff,
            DiffFetch::Failed("file is no longer part of the diff".into())
        );
    }

    #[test]
    fn session_mismatch_only_flags_different_sessions() {
        // 两侧同为 None / 一侧 None / 两侧相同 → 不标注。
        assert_eq!(session_mismatch(None, None), None);
        assert_eq!(session_mismatch(Some("s-1"), None), None);
        assert_eq!(session_mismatch(None, Some("s-1")), None);
        assert_eq!(session_mismatch(Some("s-1"), Some("s-1")), None);
        // 数据会话（latest 解析结果）与查看会话不同 → 返回数据会话 id。
        assert_eq!(session_mismatch(Some("s-2"), Some("s-1")), Some("s-2"));
    }

    #[test]
    fn disconnect_keeps_ready_changes_stale_and_invalidates_old_responses() {
        let mut state = ready_state(vec![DiffFileSummary {
            path: "a.rs".into(),
            status: "modified".into(),
            additions: 1,
            deletions: 0,
            binary: false,
        }]);
        let old_epoch = state.epoch;
        state.mark_stale("connection lost");
        assert_eq!(state.fetch, ChangesFetch::Ready);
        assert_eq!(state.files.len(), 1);
        assert_eq!(state.activity_summary(), "stale");
        assert!(!state.mark_failed_for_epoch(old_epoch, "late failure"));
    }
}
