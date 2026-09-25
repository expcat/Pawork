//! Explicit goal controls; closing the panel does not stop the Host's goal.
use super::accessibility::{AxRequest, AxRole};
use super::product_access::{edit_input, PanelAccess};
use super::*;
use gpui::{size, Bounds, WindowBounds, WindowOptions};
use pawork_client::{AppCommand, AppQuery};

struct GoalView {
    controller: Arc<DesktopController>,
    session: String,
    goal: Option<String>,
    status: String,
    summary: String,
    fields: [Entity<TextInput>; 5],
    focus: [FocusHandle; 7],
    access: PanelAccess,
    busy: bool,
    loaded: bool,
    error: Option<String>,
}
impl AppView {
    pub(super) fn open_goal(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.projection.active_session_id.clone() else {
            return;
        };
        let controller = self.controller.clone();
        let bounds = Bounds::centered(None, size(px(760.0), px(680.0)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(i18n::t("goal.title").into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |_, cx| {
                cx.new(|cx| {
                    let fields = std::array::from_fn(|i| {
                        cx.new(|cx| {
                            let field = TextInput::with_placeholder(
                                i18n::t(
                                    [
                                        "goal.objective",
                                        "goal.criteria",
                                        "goal.budget",
                                        "goal.runs",
                                        "goal.direction",
                                    ][i],
                                ),
                                cx,
                            )
                            .tab_stop(true)
                            .id([
                                "goal-objective",
                                "goal-criteria",
                                "goal-budget",
                                "goal-runs",
                                "goal-direction",
                            ][i]);
                            if i == 1 {
                                field.code_editor().height_clamp(100.0, 150.0)
                            } else {
                                field
                            }
                        })
                    });
                    let mut view = GoalView {
                        controller,
                        session,
                        goal: None,
                        status: "none".into(),
                        summary: String::new(),
                        fields,
                        focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
                        access: PanelAccess::default(),
                        busy: false,
                        loaded: false,
                        error: None,
                    };
                    view.action(0, cx);
                    cx.spawn(async move |this, cx| loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2))
                            .await;
                        if this
                            .update(cx, |view, cx| {
                                if !view.busy {
                                    view.refresh(cx);
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    })
                    .detach();
                    view
                })
            },
        ) {
            self.status_hint = Some(error.to_string());
        }
    }
}
impl GoalView {
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.request(
            Err(AppQuery::GoalGet {
                session_id: self.session.clone().into(),
            }),
            cx,
        );
    }
    fn request(&mut self, request: Result<AppCommand, AppQuery>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let is_command = request.is_ok();
        if is_command {
            self.error = None;
        }
        let task = self.controller.product_request(request);
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Err(error) => view.error = Some(error),
                    Ok(data) if !valid_goal(&data) => {
                        view.error = Some("Invalid goal response".into());
                    }
                    Ok(data) => {
                        view.loaded = true;
                        let goal = data["goal_id"].as_str().map(str::to_owned);
                        if goal != view.goal {
                            if let Some(title) = data["title"].as_str() {
                                view.fields[0]
                                    .update(cx, |field, cx| field.set_text(title.to_owned(), cx));
                            }
                            if let Some(criteria) = data["criteria"].as_array() {
                                let text = criteria
                                    .iter()
                                    .filter_map(|c| c["description"].as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                view.fields[1].update(cx, |field, cx| field.set_text(text, cx));
                            }
                        }
                        view.goal = goal;
                        view.status = data["status"].as_str().unwrap_or("none").into();
                        view.summary = if data.is_null() {
                            i18n::t("goal.empty").into()
                        } else {
                            format!(
                                "{} · {} {}/{} · {} {}/{}\n{}",
                                i18n::t(match view.status.as_str() {
                                    "active" => "goal.active",
                                    "paused" => "goal.paused",
                                    "achieved" => "goal.finished",
                                    "abandoned" => "goal.abandoned",
                                    _ => "goal.empty",
                                }),
                                i18n::t("goal.tokens_used"),
                                data["used_tokens"],
                                data["budget_tokens"],
                                i18n::t("goal.runs_used"),
                                data["used_runs"],
                                data["max_runs"],
                                match data["pause_reason"].as_str() {
                                    Some("token_budget_exhausted") =>
                                        i18n::t("goal.budget_exhausted"),
                                    Some("run_limit_reached") => i18n::t("goal.run_limit_reached"),
                                    Some("user_paused") => i18n::t("goal.user_paused"),
                                    Some(
                                        "host_stopped" | "host_restarted; explicit resume required",
                                    ) => i18n::t("goal.host_stopped"),
                                    Some("run_stopped") => i18n::t("goal.run_stopped"),
                                    Some(reason) => reason,
                                    None => "",
                                }
                            )
                        };
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn enabled(&self, index: usize) -> bool {
        if self.busy {
            return false;
        }
        match index {
            0 => true,
            1 => self.loaded && matches!(self.status.as_str(), "none" | "achieved" | "abandoned"),
            2 => self.status == "active",
            3 => self.status == "paused",
            _ => matches!(self.status.as_str(), "active" | "paused"),
        }
    }
    fn action(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.enabled(index) {
            return;
        }
        if index == 0 {
            self.refresh(cx);
            return;
        }
        let session_id = self.session.clone().into();
        let goal_id = self.goal.clone().unwrap_or_default().into();
        let (mut budget_tokens, mut max_runs) = (0, 0);
        if matches!(index, 1 | 3) {
            budget_tokens = self.fields[2]
                .read(cx)
                .text()
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
            max_runs = self.fields[3]
                .read(cx)
                .text()
                .trim()
                .parse::<u32>()
                .unwrap_or(0);
            if budget_tokens == 0 || !(1..=100).contains(&max_runs) {
                self.error = Some(i18n::t("goal.invalid_budget").into());
                cx.notify();
                return;
            }
        }
        let command = match index {
            1 => AppCommand::GoalStart {
                session_id,
                title: self.fields[0].read(cx).text().into(),
                criteria: self.fields[1]
                    .read(cx)
                    .text()
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
                budget_tokens,
                max_runs,
            },
            2 => AppCommand::GoalPause {
                session_id,
                goal_id,
            },
            3 => AppCommand::GoalResume {
                session_id,
                goal_id,
                budget_tokens,
                max_runs,
            },
            4 => AppCommand::GoalSteer {
                session_id,
                goal_id,
                input: self.fields[4].read(cx).text().into(),
            },
            5 | 6 => AppCommand::GoalFinish {
                session_id,
                goal_id,
                outcome: if index == 5 { "achieved" } else { "abandoned" }.into(),
                reason: None,
            },
            _ => return,
        };
        self.request(Ok(command), cx);
    }
    fn ax_action(&mut self, request: AxRequest, window: &mut Window, cx: &mut Context<Self>) {
        if !self.access.permits(&request) {
            return;
        }
        if let Some(index) = request
            .identifier
            .strip_prefix("goal-field-")
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|i| *i < 5)
        {
            edit_input(&self.fields[index], request, window, cx);
        } else if let Some(index) = request
            .identifier
            .strip_prefix("goal-action-")
            .and_then(|s| s.parse().ok())
        {
            self.action(index, cx);
        }
    }
}

fn valid_goal(data: &serde_json::Value) -> bool {
    data.is_null()
        || (data["goal_id"].as_str().is_some_and(|id| !id.is_empty())
            && data["title"].is_string()
            && data["criteria"]
                .as_array()
                .is_some_and(|items| items.iter().all(|c| c["description"].is_string()))
            && matches!(
                data["status"].as_str(),
                Some("active" | "paused" | "achieved" | "abandoned")
            )
            && data["used_tokens"].is_u64()
            && data["used_runs"].is_u64())
}
impl Render for GoalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        install_appkit_tab_monitor(window, cx);
        self.access.begin();
        let status = format!("{}\n{}", self.summary, self.error.as_deref().unwrap_or(""));
        let status = self.access.wrap(
            "goal-status",
            &status,
            AxRole::StaticText,
            None,
            false,
            false,
            status.clone(),
        );
        let mut body = div()
            .id("goal-panel")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(dark().bg.base)
            .text_color(dark().text.primary)
            .child(status)
            .child(i18n::t("goal.hint"));
        for (i, label) in [
            "goal.objective",
            "goal.criteria",
            "goal.budget",
            "goal.runs",
            "goal.direction",
        ]
        .iter()
        .enumerate()
        {
            body = body.child(self.access.wrap(
                &format!("goal-field-{i}"),
                i18n::t(label),
                AxRole::TextArea,
                Some(self.fields[i].read(cx).text().into()),
                true,
                self.fields[i].focus_handle(cx).is_focused(window),
                self.fields[i].clone(),
            ));
        }
        let mut actions = div().flex().flex_wrap().gap_2();
        for (i, label) in [
            "plan.refresh",
            "goal.start",
            "goal.pause",
            "goal.resume",
            "goal.steer",
            "goal.achieved",
            "goal.abandon",
        ]
        .iter()
        .enumerate()
        {
            let enabled = self.enabled(i);
            let focus = self.focus[i].clone().tab_stop(enabled);
            let button = Button::new(format!("goal-action-{i}"))
                .label(i18n::t(label))
                .track_focus(&focus)
                .disabled(!enabled)
                .on_click(cx.listener(move |view, event, _, cx| {
                    if AppView::click_down_position(event).is_some() {
                        view.action(i, cx);
                    }
                }))
                .on_activate(cx.listener(move |view, _, _, cx| view.action(i, cx)));
            actions = actions.child(self.access.wrap(
                &format!("goal-action-{i}"),
                i18n::t(label),
                AxRole::Button,
                None,
                enabled,
                self.focus[i].is_focused(window),
                button,
            ));
        }
        self.access.sync(window, cx, Self::ax_action);
        body.child(actions)
    }
}
