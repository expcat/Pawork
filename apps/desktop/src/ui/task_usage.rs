//! 任务统计窗口；数据只经 Controller → GUI Host 获取。
use super::accessibility::{AxAction, AxRequest, AxRole};
use super::product_access::PanelAccess;
use super::*;
use gpui::{size, AnyElement, Bounds, WindowBounds, WindowOptions};
use pawork_client::{
    TaskUsageGroupBy, TaskUsageOperation, TaskUsageQuery, TaskUsageRecord, TaskUsageReport,
    TaskUsageSource, TaskUsageStatus, TaskUsageTotals,
};

/// 本窗口正文的英中选择，读 `i18n::language()`。
/// 不迁入主目录：字面重合的 key 属于其它页面，不是同一词条。
fn tr(en: &'static str, zh: &'static str) -> &'static str {
    if i18n::language() == i18n::Language::Chinese {
        zh
    } else {
        en
    }
}
fn operation(o: TaskUsageOperation) -> &'static str {
    match o {
        TaskUsageOperation::Text => tr("Text", "文字"),
        TaskUsageOperation::Storyboard => tr("Storyboard", "分镜"),
        TaskUsageOperation::Image => tr("Image", "图片"),
        TaskUsageOperation::Video => tr("Video", "视频"),
        TaskUsageOperation::Coding => tr("Native Run", "原生 Run"),
        TaskUsageOperation::Query => tr("Status query", "状态查询"),
        TaskUsageOperation::Download => tr("Download", "下载"),
        TaskUsageOperation::Import => tr("Import", "导入"),
        TaskUsageOperation::Export => tr("Export", "导出"),
        TaskUsageOperation::Edit => tr("Edit", "编辑"),
    }
}
fn status(s: TaskUsageStatus) -> &'static str {
    match s {
        TaskUsageStatus::Running => tr("Running / unconfirmed", "执行中 / 尚未确认"),
        TaskUsageStatus::Submitted => tr("Submitted", "已提交"),
        TaskUsageStatus::Succeeded => tr("Succeeded", "成功"),
        TaskUsageStatus::Failed => tr("Failed", "失败"),
        TaskUsageStatus::Cancelled => tr("Cancelled", "取消"),
        TaskUsageStatus::Unknown => tr("Unknown", "终态未知"),
    }
}
fn costs(t: &TaskUsageTotals) -> String {
    let mut lines = Vec::new();
    for c in &t.currencies {
        if c.actual_records > 0 {
            lines.push(format!(
                "{} {} {:.6}",
                tr("Known actual", "已知实际"),
                c.currency,
                c.actual_micros as f64 / 1_000_000.0
            ));
        }
        if c.estimated_records > 0 {
            lines.push(format!(
                "{} {} {:.6}",
                tr("Estimated", "估算"),
                c.currency,
                c.estimated_micros as f64 / 1_000_000.0
            ));
        }
    }
    if lines.is_empty() {
        lines.push(
            if t.records > 0 && t.generation_calls == 0 && t.unknown_cost_calls == 0 {
                tr("Excluded from generation cost", "不计生成费用")
            } else {
                tr("No confirmed cost", "费用尚未确认")
            }
            .into(),
        );
    }
    if t.unknown_cost_calls > 0 {
        lines.push(format!(
            "{} {}",
            t.unknown_cost_calls,
            tr("calls with unknown cost", "次调用费用未知")
        ));
    }
    lines.join(" · ")
}
fn totals(t: &TaskUsageTotals) -> String {
    format!(
        "{} {} · {} {} · {} {} / {} {} / {} {}\n{} {} / {} {} · {} {}\n{} {} / {} {}\n{} {} · {} {}\n{}",
        t.records,
        tr("records", "条记录"),
        t.generation_calls,
        tr("generation attempts", "次生成尝试"),
        t.failed,
        tr("failed", "失败"),
        t.cancelled,
        tr("cancelled", "取消"),
        t.unfinished,
        tr("unconfirmed", "未确认"),
        tr("Known input tokens", "已知输入 Token"),
        t.tokens.input_tokens,
        tr("output", "输出"),
        t.tokens.output_tokens,
        t.unknown_token_calls,
        tr("calls with unknown tokens", "次调用 Token 未回传"),
        tr("Cache read", "缓存读 Token"),
        t.tokens.cache_read_tokens,
        tr("write", "写"),
        t.tokens.cache_write_tokens,
        t.output_images,
        tr("known output images", "张已知输出图片"),
        t.planned_video_seconds,
        tr("planned video seconds", "秒视频计划提交"),
        costs(t)
    )
}
#[derive(Clone)]
enum UsageAction {
    Refresh,
    Clear,
    Next,
    Group(TaskUsageGroupBy),
    Operation(Option<TaskUsageOperation>),
    Status(Option<TaskUsageStatus>),
    Range(Option<u64>),
    Drill(TaskUsageQuery),
    Detail(TaskUsageRecord),
    CloseDetail,
    CopyId(String),
}
struct UsageView {
    controller: Arc<DesktopController>,
    query: TaskUsageQuery,
    range_days: Option<u64>,
    report: Option<TaskUsageReport>,
    selected: Option<TaskUsageRecord>,
    busy: bool,
    error: Option<String>,
    generation: u64,
    focus: HashMap<String, FocusHandle>,
    actions: HashMap<String, UsageAction>,
    access: PanelAccess,
}
impl AppView {
    pub(super) fn open_task_usage(&mut self, cx: &mut Context<Self>) {
        let controller = self.controller.clone();
        let text_scale = self.text_scale;
        let bounds = Bounds::centered(None, size(px(1000.0), px(820.0)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(i18n::t("usage.title").into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                window.set_rem_size(px(text_scale.rem_pixels()));
                cx.new(|cx| {
                    let mut view = UsageView {
                        controller,
                        query: Default::default(),
                        range_days: None,
                        report: None,
                        selected: None,
                        busy: false,
                        error: None,
                        generation: 0,
                        focus: HashMap::new(),
                        actions: HashMap::new(),
                        access: Default::default(),
                    };
                    view.request(false, cx);
                    view
                })
            },
        ) {
            self.status_hint = Some(error.to_string());
        }
    }
}
impl UsageView {
    fn request(&mut self, append: bool, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.busy = true;
        self.error = None;
        if !append {
            self.query.cursor = None;
            self.report = None;
            self.selected = None;
        }
        let task = self.controller.task_usage_request(self.query.clone());
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |view, cx| {
                if view.generation != generation {
                    return;
                }
                view.busy = false;
                match result {
                    Err(error) => view.error = Some(error),
                    Ok(mut report) => {
                        if append {
                            if let Some(previous) = view.report.take() {
                                let mut records = previous.records;
                                for r in report.records {
                                    if !records.iter().any(|p| p.id == r.id && p.client == r.client)
                                    {
                                        records.push(r);
                                    }
                                }
                                report.records = records;
                            }
                        }
                        view.report = Some(report);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn perform(&mut self, action: UsageAction, cx: &mut Context<Self>) {
        match action {
            UsageAction::Detail(r) => {
                self.selected = Some(r);
                cx.notify();
                return;
            }
            UsageAction::CloseDetail => {
                self.selected = None;
                cx.notify();
                return;
            }
            UsageAction::CopyId(id) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(id));
                return;
            }
            UsageAction::Clear => {
                self.query = Default::default();
                self.range_days = None;
            }
            UsageAction::Refresh => {}
            UsageAction::Next => {
                let Some(cursor) = self.report.as_ref().and_then(|r| r.next_cursor.clone()) else {
                    return;
                };
                self.query.cursor = Some(cursor);
                self.request(true, cx);
                return;
            }
            UsageAction::Group(by) => self.query.group_by = by,
            UsageAction::Operation(op) => self.query.operation = op,
            UsageAction::Status(s) => self.query.status = s,
            UsageAction::Range(days) => {
                self.range_days = days;
                self.query.started_after_ms =
                    days.map(|d| now_unix_ms().saturating_sub(d * 86_400_000));
                self.query.started_before_ms = None;
            }
            UsageAction::Drill(mut q) => {
                q.group_by = match self.query.group_by {
                    TaskUsageGroupBy::Task => TaskUsageGroupBy::Subtask,
                    TaskUsageGroupBy::Group => TaskUsageGroupBy::Subtask,
                    _ => self.query.group_by,
                };
                self.query = q;
            }
        }
        self.request(false, cx);
    }
    fn button(
        &mut self,
        id: String,
        label: String,
        action: UsageAction,
        selected: bool,
        enabled: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.actions.insert(id.clone(), action.clone());
        let focus = self
            .focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
            .tab_stop(enabled);
        let mouse = action.clone();
        let button = Button::new(id.clone())
            .label(label.clone())
            .track_focus(&focus)
            .disabled(!enabled)
            .variant(if selected {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Ghost
            })
            .on_click(cx.listener(move |view, event, _, cx| {
                if AppView::click_down_position(event).is_some() {
                    view.perform(mouse.clone(), cx);
                }
            }))
            .on_activate(cx.listener(move |view, _, _, cx| view.perform(action.clone(), cx)));
        self.access.wrap(
            &id,
            &label,
            AxRole::Button,
            None,
            enabled,
            focus.is_focused(window),
            button,
        )
    }
    fn text(&mut self, id: &str, label: String) -> AnyElement {
        self.access.wrap(
            id,
            &label,
            AxRole::StaticText,
            None,
            false,
            false,
            div()
                .text_size(font::BODY_SM)
                .text_color(dark().text.secondary)
                .child(label.clone()),
        )
    }
    fn ax_action(&mut self, request: AxRequest, _: &mut Window, cx: &mut Context<Self>) {
        if self.access.permits(&request) && request.action == AxAction::Press {
            if let Some(action) = self.actions.get(&request.identifier).cloned() {
                self.perform(action, cx);
            }
        }
    }
}
impl Render for UsageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        install_appkit_tab_monitor(window, cx);
        self.access.begin();
        self.actions.clear();
        let mut header = div().flex().items_center().justify_between().gap_3().child(
            div()
                .text_size(font::HEADER_TITLE)
                .font_weight(FontWeight::MEDIUM)
                .child(i18n::t("usage.title")),
        );
        let mut actions = div().flex().gap_2();
        for (id, label, action) in [
            (
                "usage-clear",
                tr("All tasks / reset", "全部任务 / 清除筛选"),
                UsageAction::Clear,
            ),
            ("usage-refresh", tr("Refresh", "刷新"), UsageAction::Refresh),
        ] {
            actions = actions.child(self.button(
                id.into(),
                label.into(),
                action,
                false,
                !self.busy,
                window,
                cx,
            ));
        }
        header = header.child(actions);
        let mut filters = div().flex().flex_col().gap_1();
        let mut group = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(tr("Group by", "分组"));
        for (i, (by, label)) in [
            (TaskUsageGroupBy::Task, tr("Task", "完整任务")),
            (TaskUsageGroupBy::Group, tr("Part", "分部")),
            (
                TaskUsageGroupBy::Subtask,
                tr("Chapter / segment", "章节 / 分段"),
            ),
            (TaskUsageGroupBy::Operation, tr("Operation", "操作类型")),
            (TaskUsageGroupBy::Model, tr("Model", "模型")),
            (TaskUsageGroupBy::Client, tr("Client", "客户端")),
        ]
        .into_iter()
        .enumerate()
        {
            group = group.child(self.button(
                format!("usage-group-{i}"),
                label.into(),
                UsageAction::Group(by),
                self.query.group_by == by,
                true,
                window,
                cx,
            ));
        }
        filters = filters.child(group);
        let mut ops = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(tr("Operations", "消耗类型"));
        for (i, op) in [
            None,
            Some(TaskUsageOperation::Text),
            Some(TaskUsageOperation::Storyboard),
            Some(TaskUsageOperation::Image),
            Some(TaskUsageOperation::Video),
            Some(TaskUsageOperation::Coding),
            Some(TaskUsageOperation::Query),
            Some(TaskUsageOperation::Download),
            Some(TaskUsageOperation::Import),
            Some(TaskUsageOperation::Export),
            Some(TaskUsageOperation::Edit),
        ]
        .into_iter()
        .enumerate()
        {
            ops = ops.child(self.button(
                format!("usage-op-{i}"),
                op.map(operation).unwrap_or(tr("All", "全部")).into(),
                UsageAction::Operation(op),
                self.query.operation == op,
                true,
                window,
                cx,
            ));
        }
        filters = filters.child(ops);
        let mut states = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(tr("Status", "状态"));
        for (i, s) in [
            None,
            Some(TaskUsageStatus::Running),
            Some(TaskUsageStatus::Submitted),
            Some(TaskUsageStatus::Succeeded),
            Some(TaskUsageStatus::Failed),
            Some(TaskUsageStatus::Cancelled),
            Some(TaskUsageStatus::Unknown),
        ]
        .into_iter()
        .enumerate()
        {
            states = states.child(self.button(
                format!("usage-status-{i}"),
                s.map(status).unwrap_or(tr("All", "全部")).into(),
                UsageAction::Status(s),
                self.query.status == s,
                true,
                window,
                cx,
            ));
        }
        filters = filters.child(states);
        let mut ranges = div().flex().flex_wrap().items_center().gap_1();
        for (i, (days, label)) in [
            (None, tr("All time", "全部时间")),
            (Some(1), tr("24 hours", "近 24 小时")),
            (Some(7), tr("7 days", "近 7 天")),
            (Some(30), tr("30 days", "近 30 天")),
        ]
        .into_iter()
        .enumerate()
        {
            ranges = ranges.child(self.button(
                format!("usage-range-{i}"),
                label.into(),
                UsageAction::Range(days),
                self.range_days == days,
                true,
                window,
                cx,
            ));
        }
        filters = filters.child(ranges);
        let scope = format!(
            "{} · {} · {} · {}",
            self.query
                .task_id
                .as_deref()
                .unwrap_or(tr("All tasks", "全部任务")),
            self.query
                .group_id
                .as_deref()
                .unwrap_or(tr("All parts", "全部分部")),
            self.query
                .subtask_id
                .as_deref()
                .unwrap_or(tr("All chapters / segments", "全部章节 / 分段")),
            self.query
                .client
                .as_deref()
                .unwrap_or(tr("All clients", "全部客户端"))
        );
        let scope = self.text("usage-scope", scope);
        let mut content = div()
            .id("usage-content")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap_3();
        let show_list = self.selected.is_none();
        if self.busy && show_list {
            content = content.child(self.text("usage-loading", tr("Loading…", "正在查询…").into()));
        }
        if let Some(error) = self.error.clone().filter(|_| show_list) {
            content = content.child(self.text(
                "usage-error",
                format!("{}: {error}", tr("Query failed", "查询失败")),
            ));
        }
        if let Some(report) = self.report.clone().filter(|_| show_list) {
            content=content.child(div().p_3().rounded_md().bg(dark().surface.raised).child(self.text("usage-totals",totals(&report.totals))))
                .child(self.text("usage-coverage",tr("Totals cover every matching record. Media duration is planned, cost is split by currency. Native entries summarize Runs; local operations are client reports.","统计覆盖全部匹配记录。视频秒数为计划值，费用按币种区分。原生条目汇总 Run，本地操作注明客户端报告。").into()));
            if report.totals.records == 0 {
                content=content.child(self.text("usage-empty",tr("No records match. New calls are logged after this update; historical missing usage cannot be recovered.","没有匹配记录。更新后的新调用会被记录，历史缺失用量无法从结果反推。").into()));
            }
            let mut groups = div().flex().flex_col().gap_1();
            for (i, g) in report.groups.into_iter().enumerate() {
                let title = if self.query.group_by == TaskUsageGroupBy::Operation {
                    g.filter
                        .as_ref()
                        .and_then(|q| q.operation)
                        .map(operation)
                        .unwrap_or(g.title.as_str())
                        .to_string()
                } else if g.key.is_empty() {
                    tr("Unassigned task", "未关联任务").into()
                } else {
                    g.title.clone()
                };
                let label = format!(
                    "{title} · {} {} · {} {} · {}",
                    g.totals.records,
                    tr("records", "条记录"),
                    g.totals.generation_calls,
                    tr("generations", "次生成"),
                    costs(&g.totals)
                );
                let action = g
                    .filter
                    .clone()
                    .map(UsageAction::Drill)
                    .unwrap_or(UsageAction::Refresh);
                groups = groups.child(self.button(
                    format!("usage-drill-{i}"),
                    label,
                    action,
                    false,
                    g.filter.is_some(),
                    window,
                    cx,
                ));
            }
            content = content.child(groups).child(tr("Call log", "调用日志"));
            for (i, r) in report.records.iter().enumerate() {
                let title = r
                    .context
                    .as_ref()
                    .and_then(|c| c.subtask.as_ref())
                    .map(|t| t.title.as_str())
                    .or_else(|| r.context.as_ref().map(|c| c.task.title.as_str()))
                    .unwrap_or(tr("Unassigned", "未关联"));
                let usage = r
                    .tokens
                    .as_ref()
                    .map(|u| format!("{} / {} Token", u.input_tokens, u.output_tokens))
                    .unwrap_or_else(|| {
                        if r.operation.is_generation() || r.operation == TaskUsageOperation::Coding
                        {
                            tr("Tokens unknown", "Token 未回传").into()
                        } else {
                            tr("Excluded from generation usage", "不计生成用量").into()
                        }
                    });
                let label = format!(
                    "{title} · {} · {} · {} / {} · {usage} · {}",
                    operation(r.operation),
                    r.client,
                    r.provider.as_deref().unwrap_or("—"),
                    r.model.as_deref().unwrap_or("—"),
                    status(r.status)
                );
                content = content.child(self.button(
                    format!("usage-record-{i}"),
                    label,
                    UsageAction::Detail(r.clone()),
                    false,
                    true,
                    window,
                    cx,
                ));
            }
            content = content.child(self.button(
                "usage-next".into(),
                tr("Load more logs", "加载更多日志").into(),
                UsageAction::Next,
                false,
                !self.busy && report.next_cursor.is_some(),
                window,
                cx,
            ));
        }
        if let Some(r) = self.selected.clone() {
            let metered = r.operation.is_generation() || r.operation == TaskUsageOperation::Coding;
            let task_ref = |item: Option<&pawork_client::UsageTaskRef>| {
                item.map(|t| format!("{} ({})", t.title, t.id))
                    .unwrap_or_else(|| "—".into())
            };
            let context = format!(
                "{}: {}\n{}: {}\n{}: {}\n{}: {}",
                tr("Task", "完整任务"),
                task_ref(r.context.as_ref().map(|c| &c.task)),
                tr("Part", "分部"),
                task_ref(r.context.as_ref().and_then(|c| c.group.as_ref())),
                tr("Chapter / segment", "章节 / 分段"),
                task_ref(r.context.as_ref().and_then(|c| c.subtask.as_ref())),
                tr("Operation", "操作类型"),
                operation(r.operation)
            );
            let token = r
                .tokens
                .as_ref()
                .map(|u| {
                    format!(
                        "{} {} · {} {} · {} {} / {} {}",
                        tr("Input", "输入"),
                        u.input_tokens,
                        tr("output", "输出"),
                        u.output_tokens,
                        tr("cache read", "缓存读"),
                        u.cache_read_tokens,
                        tr("write", "写"),
                        u.cache_write_tokens
                    )
                })
                .unwrap_or_else(|| {
                    if metered {
                        tr("Tokens not reported", "Token 未回传").into()
                    } else {
                        tr("Excluded from generation tokens", "不计生成 Token").into()
                    }
                });
            let source = match r.source {
                TaskUsageSource::Gateway => "Pawork Gateway",
                TaskUsageSource::NativeRun => tr("Native Run summary", "原生 Run 汇总"),
                TaskUsageSource::ClientReport => tr("Client report", "客户端报告"),
            };
            let duration = r
                .finished_at_ms
                .map(|t| format!("{} ms", t.saturating_sub(r.started_at_ms)))
                .unwrap_or(tr("Not confirmed", "尚未确认").into());
            let cost = r
                .cost
                .as_ref()
                .map(|c| {
                    format!(
                        "{:?} {} {:.6}",
                        c.kind,
                        c.value.currency,
                        c.value.amount_micros as f64 / 1_000_000.0
                    )
                })
                .unwrap_or_else(|| {
                    if metered {
                        tr("Cost unknown", "费用未知").into()
                    } else {
                        tr("Excluded from generation cost", "不计生成费用").into()
                    }
                });
            let details=format!("{}: {}\n{}: {} · {} · {}\n{}: {} · {}: {}\n{}\n{}\n{}: {} · {}: {}\n{}: {} · {}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}",
                tr("Call ID","调用 ID"),r.id,tr("Source","来源"),source,r.client,status(r.status),tr("Provider","供应商"),r.provider.as_deref().unwrap_or("—"),tr("Model","模型"),r.model.as_deref().unwrap_or("—"),token,cost,
                tr("Images","输出图片"),r.output_images.map(|v|v.to_string()).unwrap_or_else(|| if r.operation == TaskUsageOperation::Image { tr("Unknown","未知").into() } else { "—".into() }),tr("Planned video seconds","视频计划秒数"),r.planned_video_seconds.map(|v|v.to_string()).unwrap_or("—".into()),
                tr("Started","开始时间"),task_rail::relative_activity(r.started_at_ms,now_unix_ms()),tr("Call duration","调用耗时"),duration,
                tr("Video handle","视频任务 ID"),r.upstream_task_id.as_deref().unwrap_or("—"),tr("Upstream status","供应商状态"),r.upstream_status.map(|s|format!("{s:?}")).unwrap_or("—".into()),
                tr("Related call","关联调用"),r.related_call_id.as_deref().unwrap_or("—"),tr("Retry of","明确补交原调用"),r.context.as_ref().and_then(|c|c.retry_of.as_deref()).unwrap_or("—"));
            let mut detail = div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .border_1()
                .border_color(dark().border.strong)
                .rounded_md()
                .bg(dark().bg.panel)
                .child(self.text("usage-detail-context", context))
                .child(self.text("usage-detail", details));
            if let Some(code) = r.error_code {
                detail = detail.child(self.text("usage-detail-error", code));
            }
            detail = detail.child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.button(
                        "usage-copy-id".into(),
                        tr("Copy call ID", "复制调用 ID").into(),
                        UsageAction::CopyId(r.id),
                        false,
                        true,
                        window,
                        cx,
                    ))
                    .child(self.button(
                        "usage-close-detail".into(),
                        tr("Close details", "关闭详情").into(),
                        UsageAction::CloseDetail,
                        false,
                        true,
                        window,
                        cx,
                    )),
            );
            // 详情先于日志，选中长列表底部行后仍可立即查看。
            content = div()
                .id("usage-detail-content")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .gap_3()
                .child(detail);
        }
        self.access.sync(window, cx, Self::ax_action);
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(dark().bg.base)
            .text_color(dark().text.primary)
            .child(components::focus_ring::track_pointer_input())
            .capture_key_down(components::focus_ring::key_down)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    view.selected = None;
                    cx.notify();
                }
            }))
            .child(header)
            .child(filters)
            .child(scope)
            .child(content)
    }
}

#[cfg(all(test, feature = "ac05-task-usage"))]
mod tests {
    use std::io::Write;
    use std::os::unix::fs::FileTypeExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::*;

    struct Host {
        child: Child,
        root: PathBuf,
        log: PathBuf,
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn fixture_bin() -> PathBuf {
        let path = std::env::var("PAWORK_UI_FIXTURE")
            .map(PathBuf::from)
            .unwrap_or_default();
        assert!(
            path.is_file(),
            "PAWORK_UI_FIXTURE must be the built ui_fixture example, got {}",
            path.display()
        );
        path
    }

    fn log_tail(path: &Path) -> String {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut lines: Vec<&str> = text.lines().rev().take(40).collect();
        lines.reverse();
        lines.join("\n")
    }

    fn data_names(root: &Path) -> String {
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(root.join("data")) {
            for entry in entries.flatten() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        names.join(",")
    }

    fn sqlite(db: &Path, sql: &str) -> Result<String, String> {
        let mut last = String::new();
        for _ in 0..20 {
            let mut child = Command::new("/usr/bin/sqlite3")
                .arg(db)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| format!("spawn sqlite3: {error}"))?;
            let mut stdin = child.stdin.take().expect("sqlite stdin");
            stdin
                .write_all(format!(".timeout 2000\n{sql}\n").as_bytes())
                .map_err(|error| format!("sqlite stdin: {error}"))?;
            drop(stdin);
            let output = child
                .wait_with_output()
                .map_err(|error| error.to_string())?;
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if output.status.success() {
                return Ok(text);
            }
            last = text;
            if last.contains("locked") || last.contains("busy") {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            return Err(last);
        }
        Err(format!("sqlite stayed busy: {last}"))
    }

    fn spawn_host() -> Host {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/tmp/pawork-ac05-task-usage/root-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("root");
        let binary = fixture_bin();
        let seed = Command::new(&binary)
            .arg("seed")
            .arg("--root")
            .arg(&root)
            .current_dir("/tmp")
            .output()
            .expect("spawn seed");
        assert!(
            seed.status.success(),
            "ui_fixture seed failed: {}{}",
            String::from_utf8_lossy(&seed.stdout),
            String::from_utf8_lossy(&seed.stderr)
        );
        let evidence = PathBuf::from("/tmp/pawork-ac05-task-usage/evidence");
        std::fs::create_dir_all(&evidence).expect("evidence");
        let log = evidence.join(format!("host-{}.log", std::process::id()));
        let stderr = std::fs::File::create(&log).expect("host log");
        let child = Command::new(&binary)
            .arg("serve")
            .arg("--root")
            .arg(&root)
            .current_dir("/tmp")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .expect("spawn serve");
        let mut host = Host { child, root, log };
        wait_ready(&mut host);
        host
    }

    fn wait_ready(host: &mut Host) {
        let socket = host.root.join("data/pawork-gui.sock");
        let token = host.root.join("data/gui.token");
        let db = host.root.join("data/usage-ledger.sqlite3");
        let start = Instant::now();
        loop {
            if let Some(status) = host.child.try_wait().expect("serve status") {
                panic!("serve exited {status}\n{}", log_tail(&host.log));
            }
            let socket_ok = std::fs::metadata(&socket)
                .map(|meta| meta.file_type().is_socket())
                .unwrap_or(false);
            let token_ok = std::fs::metadata(&token)
                .map(|meta| meta.len() > 0)
                .unwrap_or(false);
            if socket_ok && token_ok && db.is_file() {
                match sqlite(
                    &db,
                    "SELECT count(*) FROM sqlite_master WHERE name='task_usage_calls';",
                ) {
                    Ok(text) if text.trim() == "1" => return,
                    Ok(_) | Err(_) if start.elapsed() < Duration::from_secs(30) => {}
                    Ok(text) => panic!(
                        "task_usage_calls missing: {text} files={} log={}",
                        data_names(&host.root),
                        log_tail(&host.log)
                    ),
                    Err(error) => panic!(
                        "ledger schema: {error} files={} log={}",
                        data_names(&host.root),
                        log_tail(&host.log)
                    ),
                }
            }
            if start.elapsed() > Duration::from_secs(30) {
                panic!(
                    "host not ready files={} log={}",
                    data_names(&host.root),
                    log_tail(&host.log)
                );
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    fn insert_ledger(host: &Host) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis() as u64;
        let recent = now - 120_000;
        let pads_at = now - 2 * 86_400_000;
        let older_at = now - 40 * 86_400_000;
        let mut sql = String::from("BEGIN IMMEDIATE;\n");
        {
            let mut add = |id: &str,
                           source: &str,
                           operation: &str,
                           started: u64,
                           status: &str,
                           context: bool,
                           tokens: Option<(u64, u64)>,
                           images: Option<u64>,
                           seconds: Option<u64>,
                           provider: Option<&str>,
                           model: Option<&str>,
                           error: Option<&str>| {
                push_row(
                    &mut sql,
                    id,
                    started,
                    operation,
                    ledger_row(
                        id, source, operation, started, status, context, tokens, images, seconds,
                        provider, model, error,
                    ),
                );
            };
            add(
                "text-known",
                "gateway",
                "text",
                recent,
                "succeeded",
                true,
                Some((100, 40)),
                None,
                None,
                Some("fixture"),
                Some("fixture-model"),
                None,
            );
            add(
                "text-unknown",
                "gateway",
                "text",
                recent,
                "succeeded",
                true,
                None,
                None,
                None,
                Some("fixture"),
                Some("fixture-model"),
                None,
            );
            add(
                "text-failed",
                "gateway",
                "text",
                recent,
                "failed",
                true,
                None,
                None,
                None,
                Some("fixture"),
                Some("fixture-model"),
                Some("upstream_timeout"),
            );
            add(
                "image-known",
                "gateway",
                "image",
                recent,
                "succeeded",
                true,
                Some((30, 0)),
                Some(2),
                None,
                Some("fixture"),
                Some("fixture-model"),
                None,
            );
            add(
                "image-unknown",
                "gateway",
                "image",
                recent,
                "succeeded",
                true,
                None,
                None,
                None,
                Some("fixture"),
                Some("fixture-model"),
                None,
            );
            add(
                "video-plan",
                "gateway",
                "video",
                recent,
                "succeeded",
                true,
                Some((10, 4)),
                None,
                Some(8),
                Some("fixture"),
                Some("fixture-model"),
                None,
            );
            add(
                "query-status",
                "gateway",
                "query",
                recent,
                "succeeded",
                true,
                None,
                None,
                None,
                Some("fixture"),
                Some("fixture-model"),
                None,
            );
            add(
                "download-1",
                "client_report",
                "download",
                recent,
                "succeeded",
                true,
                None,
                None,
                None,
                None,
                None,
                None,
            );
            add(
                "coding-open",
                "native_run",
                "coding",
                recent,
                "succeeded",
                false,
                None,
                None,
                None,
                Some("pawork"),
                Some("native"),
                None,
            );
            for index in 0..=90 {
                let id = format!("pad-{index:02}");
                add(
                    &id,
                    "client_report",
                    "download",
                    pads_at,
                    "succeeded",
                    true,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                );
            }
            add(
                "older-page",
                "gateway",
                "text",
                older_at,
                "succeeded",
                true,
                Some((4, 1)),
                None,
                None,
                Some("fixture"),
                Some("fixture-model"),
                None,
            );
        }
        sql.push_str("COMMIT;\n");
        let db = host.root.join("data/usage-ledger.sqlite3");
        sqlite(&db, &sql).unwrap_or_else(|error| panic!("insert ledger: {error}"));
        let count = sqlite(&db, "SELECT COUNT(*) FROM task_usage_calls;").unwrap();
        assert_eq!(count.trim(), "101", "seeded ledger count");
    }

    fn ledger_row(
        id: &str,
        source: &str,
        operation: &str,
        started: u64,
        status: &str,
        with_context: bool,
        tokens: Option<(u64, u64)>,
        images: Option<u64>,
        seconds: Option<u64>,
        provider: Option<&str>,
        model: Option<&str>,
        error_code: Option<&str>,
    ) -> serde_json::Value {
        let context = with_context.then(|| {
            serde_json::json!({
                "task": {"id": "novel", "title": "Novel"},
                "group": {"id": "part-1", "title": "Part 1"},
                "subtask": {"id": "ch-1", "title": "ch-1"},
                "operation": operation,
                "retry_of": null
            })
        });
        let tokens = tokens.map(|(input_tokens, output_tokens)| {
            serde_json::json!({
                "input_tokens": input_tokens,
                "output_tokens": output_tokens
            })
        });
        serde_json::json!({
            "id": id,
            "client": "ac05",
            "source": source,
            "context": context,
            "operation": operation,
            "provider": provider,
            "model": model,
            "started_at_ms": started,
            "finished_at_ms": started + 1_000,
            "status": status,
            "tokens": tokens,
            "cost": null,
            "output_images": images,
            "planned_video_seconds": seconds,
            "upstream_task_id": null,
            "upstream_status": null,
            "related_call_id": null,
            "error_code": error_code
        })
    }

    fn push_row(
        sql: &mut String,
        id: &str,
        started: u64,
        operation: &str,
        payload: serde_json::Value,
    ) {
        let payload = payload.to_string().replace("'", "''");
        sql.push_str("INSERT INTO task_usage_calls ");
        sql.push_str("(client,id,started_at_ms,upstream_task_id,operation,payload) VALUES (");
        sql.push_str(&format!(
            "'ac05','{id}',{started},NULL,'{operation}','{payload}');\n"
        ));
    }

    fn until_connected(
        cx: &mut gpui::VisualTestContext,
        view: &gpui::Entity<AppView>,
        host: &mut Host,
    ) {
        let start = Instant::now();
        loop {
            cx.refresh().expect("draw");
            cx.run_until_parked();
            let connected = cx.update(|_, app| {
                matches!(
                    view.read(app).projection.connection,
                    crate::projection::ConnectionState::Connected { .. }
                )
            });
            if connected {
                return;
            }
            if start.elapsed() > Duration::from_secs(20) {
                panic!("desktop did not connect\n{}", log_tail(&host.log));
            }
            if let Some(status) = host.child.try_wait().expect("serve status") {
                panic!("serve exited {status}\n{}", log_tail(&host.log));
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    fn click(cx: &mut gpui::VisualTestContext, id: &'static str) {
        cx.refresh().expect("draw");
        cx.run_until_parked();
        let bounds = cx
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("missing {id}"));
        cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        cx.run_until_parked();
    }

    fn focus_enter(panel: &mut gpui::VisualTestContext, usage: &gpui::Entity<UsageView>, id: &str) {
        panel.update(|window, cx| {
            let handle = usage
                .read(cx)
                .focus
                .get(id)
                .cloned()
                .unwrap_or_else(|| panic!("missing focus {id}"));
            window.focus(&handle);
        });
        panel.refresh().expect("draw");
        panel.simulate_keystrokes("enter");
    }

    fn ax_press(panel: &mut gpui::VisualTestContext, usage: &gpui::Entity<UsageView>, id: &str) {
        panel.update(|window, cx| {
            usage.update(cx, |view, cx| {
                view.ax_action(
                    crate::ui::accessibility::AxRequest {
                        identifier: id.into(),
                        action: crate::ui::accessibility::AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        panel.run_until_parked();
    }

    fn wait_usage(
        panel: &mut gpui::VisualTestContext,
        usage: &gpui::Entity<UsageView>,
        host: &Host,
        what: &str,
        mut ready: impl FnMut(&UsageView) -> bool,
    ) {
        let start = Instant::now();
        loop {
            panel.refresh().expect("draw");
            panel.run_until_parked();
            let (done, error) = panel.update(|_, cx| {
                let view = usage.read(cx);
                (ready(view), view.error.clone())
            });
            if done {
                return;
            }
            if start.elapsed() > Duration::from_secs(20) {
                panic!("{what}: error={error:?}\n{}", log_tail(&host.log));
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    fn assert_full_totals(view: &UsageView) {
        let report = view.report.as_ref().expect("report");
        assert_eq!(report.totals.records, 101);
        assert_eq!(report.totals.generation_calls, 7);
        assert_eq!(report.totals.failed, 1);
        assert_eq!(report.totals.unknown_token_calls, 4);
        assert_eq!(report.totals.unknown_cost_calls, 8);
        assert_eq!(report.totals.tokens.input_tokens, 144);
        assert_eq!(report.totals.tokens.output_tokens, 45);
        assert_eq!(report.totals.output_images, 2);
        assert_eq!(report.totals.planned_video_seconds, 8);
    }

    fn assert_first_page(view: &UsageView) {
        assert_full_totals(view);
        let report = view.report.as_ref().expect("report");
        assert_eq!(report.records.len(), 100);
        assert_eq!(
            report.records.last().map(|record| record.id.as_str()),
            Some("pad-00")
        );
        assert!(report.next_cursor.is_some());
        assert!(report
            .records
            .iter()
            .all(|record| record.id != "older-page"));
        let totals = view.access.node_label("usage-totals").expect("totals");
        assert!(totals.contains("101 records"), "{totals}");
        assert!(totals.contains("7 generation attempts"), "{totals}");
        assert!(totals.contains("Known input tokens 144"), "{totals}");
        assert!(totals.contains("output 45"), "{totals}");
        assert!(totals.contains("4 calls with unknown tokens"), "{totals}");
        assert!(totals.contains("8 calls with unknown cost"), "{totals}");
        assert!(totals.contains("2 known output images"), "{totals}");
        assert!(totals.contains("8 planned video seconds"), "{totals}");
    }

    #[gpui::test]
    fn usage_window_matches_seeded_ledger(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        let mut host = spawn_host();
        insert_ledger(&host);
        let platform = std::sync::Arc::new(Platform::new());
        let socket = host.root.join("data/pawork-gui.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        until_connected(cx, &view, &mut host);
        let api = cx.update(|_, app| {
            view.read(app)
                .handshake_info
                .as_ref()
                .expect("handshake")
                .api_version
                .clone()
        });
        let minor: u32 = api
            .split('.')
            .nth(1)
            .and_then(|part| part.parse().ok())
            .unwrap_or(0);
        assert!(minor >= 26, "host api {api}");
        let before = cx.windows();
        click(cx, "task-usage-open");
        let panel_id = cx
            .windows()
            .into_iter()
            .find(|id| !before.contains(id))
            .expect("usage window");
        let mut panel = gpui::VisualTestContext::from_window(panel_id, cx);
        let usage = panel.update(|window, _| window.root::<UsageView>().unwrap().unwrap());
        wait_usage(&mut panel, &usage, &host, "first page", |view| {
            view.report
                .as_ref()
                .is_some_and(|report| report.records.len() == 100 && report.totals.records == 101)
        });
        panel.update(|_, cx| assert_first_page(usage.read(cx)));
        let stored_title = panel.window_title();
        let platform_title = panel.update(|window, _| window.window_title());
        if let Some(title) = stored_title.as_ref().filter(|title| !title.is_empty()) {
            assert_eq!(title, "Task usage");
        }
        if !platform_title.is_empty() {
            assert_eq!(platform_title, "Task usage");
        }
        crate::ui::i18n::set_language(crate::ui::i18n::Language::Chinese);
        wait_usage(&mut panel, &usage, &host, "chinese refresh", |view| {
            view.access.node_label("usage-refresh").as_deref() == Some("刷新")
        });
        panel.update(|_, cx| {
            let labels = usage.read(cx).access.labels();
            for header in [
                "Group by",
                "Operations",
                "Status",
                "Call log",
                "分组",
                "消耗类型",
                "状态",
                "调用日志",
            ] {
                assert!(
                    !labels.iter().any(|label| label == header),
                    "{header} in {labels:?}"
                );
            }
            let totals = usage
                .read(cx)
                .access
                .node_label("usage-totals")
                .expect("totals");
            assert!(totals.contains("条记录"), "{totals}");
        });
        assert_eq!(panel.window_title(), stored_title);
        assert_eq!(
            panel.update(|window, _| window.window_title()),
            platform_title
        );
        crate::ui::i18n::set_language(crate::ui::i18n::Language::English);
        wait_usage(&mut panel, &usage, &host, "english refresh", |view| {
            view.access.node_label("usage-refresh").as_deref() == Some("Refresh")
        });
        focus_enter(&mut panel, &usage, "usage-op-1");
        wait_usage(&mut panel, &usage, &host, "text filter", |view| {
            view.report
                .as_ref()
                .is_some_and(|report| report.records.len() == 4)
        });
        panel.update(|_, cx| {
            let mut ids: Vec<_> = usage
                .read(cx)
                .report
                .as_ref()
                .expect("report")
                .records
                .iter()
                .map(|record| record.id.clone())
                .collect();
            ids.sort();
            assert_eq!(
                ids,
                ["older-page", "text-failed", "text-known", "text-unknown"]
            );
        });
        ax_press(&mut panel, &usage, "usage-status-4");
        wait_usage(&mut panel, &usage, &host, "failed text", |view| {
            view.report
                .as_ref()
                .is_some_and(|report| report.records.len() == 1)
        });
        panel.update(|_, cx| {
            let record = &usage.read(cx).report.as_ref().expect("report").records[0];
            assert_eq!(record.id, "text-failed");
            assert_eq!(record.error_code.as_deref(), Some("upstream_timeout"));
        });
        click(&mut panel, "usage-clear");
        wait_usage(&mut panel, &usage, &host, "clear filters", |view| {
            view.report
                .as_ref()
                .is_some_and(|report| report.records.len() == 100 && report.next_cursor.is_some())
        });
        panel.update(|_, cx| assert_first_page(usage.read(cx)));
        let drill = panel.update(|_, cx| {
            let view = usage.read(cx);
            ["usage-drill-0", "usage-drill-1"]
                .into_iter()
                .find(|id| {
                    view.access
                        .node_label(id)
                        .is_some_and(|label| label.contains("Novel"))
                })
                .expect("novel drill")
        });
        click(&mut panel, drill);
        wait_usage(&mut panel, &usage, &host, "novel drill", |view| {
            let scoped = view
                .access
                .node_label("usage-scope")
                .is_some_and(|scope| scope.contains("novel"));
            let chapter = view
                .access
                .node_label("usage-drill-0")
                .is_some_and(|label| label.contains("ch-1"));
            scoped
                && chapter
                && view.report.as_ref().is_some_and(|report| {
                    report.records.len() == 100
                        && report.next_cursor.is_none()
                        && report
                            .records
                            .iter()
                            .all(|record| record.id != "coding-open")
                })
        });
        click(&mut panel, "usage-clear");
        wait_usage(&mut panel, &usage, &host, "clear before next", |view| {
            view.report
                .as_ref()
                .is_some_and(|report| report.records.len() == 100 && report.next_cursor.is_some())
        });
        ax_press(&mut panel, &usage, "usage-next");
        wait_usage(&mut panel, &usage, &host, "next page", |view| {
            view.report.as_ref().is_some_and(|report| {
                report.records.len() == 101
                    && report
                        .records
                        .iter()
                        .any(|record| record.id == "older-page")
            })
        });
        click(&mut panel, "usage-range-1");
        wait_usage(&mut panel, &usage, &host, "24h", |view| {
            view.report.as_ref().is_some_and(|report| {
                report.records.len() == 9
                    && report.totals.generation_calls == 6
                    && report.totals.tokens.input_tokens == 140
                    && report.totals.tokens.output_tokens == 44
                    && report
                        .records
                        .iter()
                        .all(|record| record.id != "older-page")
                    && report
                        .records
                        .iter()
                        .any(|record| record.id == "coding-open")
            })
        });
        let unknown = panel.update(|_, cx| {
            let view = usage.read(cx);
            (0..9).find_map(|index| {
                let id = format!("usage-record-{index}");
                let label = view.access.node_label(&id)?;
                (label.contains("Text") && label.contains("Tokens unknown")).then_some(id)
            })
        });
        let unknown = unknown.expect("tokens-unknown text row");
        ax_press(&mut panel, &usage, &unknown);
        wait_usage(&mut panel, &usage, &host, "unknown detail", |view| {
            view.selected
                .as_ref()
                .is_some_and(|record| record.id == "text-unknown")
        });
        panel.update(|_, cx| {
            let detail = usage
                .read(cx)
                .access
                .node_label("usage-detail")
                .expect("detail");
            assert!(detail.contains("Tokens not reported"), "{detail}");
            assert!(detail.contains("Cost unknown"), "{detail}");
            assert!(!detail.contains("0 / 0"), "{detail}");
            assert!(!detail.contains("Input 0"), "{detail}");
        });
        click(&mut panel, "usage-copy-id");
        let copied = panel.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some("text-unknown"));
        panel.simulate_keystrokes("escape");
        panel.update(|_, cx| {
            assert!(
                usage.read(cx).selected.is_none(),
                "escape did not clear the detail"
            );
        });
        click(&mut panel, "usage-range-0");
        wait_usage(
            &mut panel,
            &usage,
            &host,
            "all time before reject",
            |view| {
                view.error.is_none()
                    && view
                        .report
                        .as_ref()
                        .is_some_and(|report| report.records.len() == 100)
            },
        );
        let db = host.root.join("data/usage-ledger.sqlite3");
        let backup = PathBuf::from("/tmp/pawork-ac05-task-usage/evidence/usage-ledger.sqlite3");
        let _ = std::fs::remove_file(&backup);
        sqlite(&db, &format!(".backup {}", backup.display()))
            .unwrap_or_else(|error| panic!("backup: {error}"));
        sqlite(
            &db,
            "UPDATE task_usage_calls SET payload='broken' WHERE id='pad-00';",
        )
        .unwrap_or_else(|error| panic!("corrupt: {error}"));
        click(&mut panel, "usage-refresh");
        wait_usage(&mut panel, &usage, &host, "query failed", |view| {
            view.report.is_none()
                && view
                    .access
                    .node_label("usage-error")
                    .is_some_and(|label| label.contains("Query failed"))
        });
        let query_error = panel.update(|_, cx| {
            let view = usage.read(cx);
            assert!(view.report.is_none());
            let labels = view.access.labels();
            assert!(
                labels
                    .iter()
                    .all(|label| !label.contains("No records match")),
                "{labels:?}"
            );
            view.access.node_label("usage-error").expect("query error")
        });
        let evidence = PathBuf::from("/tmp/pawork-ac05-task-usage/evidence/result.txt");
        std::fs::write(
            &evidence,
            format!(
                "api={api}\nrecords=101\ngeneration=7\nfailed=1\nunknown_tokens=4\nunknown_cost=8\ninput=144\noutput=45\nimages=2\nplanned_video_seconds=8\nstored_title={stored_title:?}\nplatform_title={platform_title:?}\nplatform_title_note=TestWindow get_title stays empty unless set_title stored one\nquery_error={query_error}\n"
            ),
        )
        .expect("evidence");
    }
}
