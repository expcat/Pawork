//! Plan 的版本化编辑与明确批准，不使用聊天草稿代替计划。
use super::accessibility::{AxRequest, AxRole};
use super::product_access::{edit_input, PanelAccess};
use super::*;
use gpui::{size, Bounds, WindowBounds, WindowOptions};
use pawork_client::{AppCommand, AppQuery};

struct PlanView {
    access: PanelAccess,
    controller: Arc<DesktopController>,
    session: String,
    version: Option<String>,
    status: String,
    saved_title: String,
    saved_steps: String,
    title: Entity<TextInput>,
    steps: Entity<TextInput>,
    reason: Entity<TextInput>,
    busy: bool,
    loaded: bool,
    error: Option<String>,
    focus: [FocusHandle; 5],
}

impl AppView {
    pub(super) fn open_plan(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.projection.active_session_id.clone() else {
            return;
        };
        let controller = self.controller.clone();
        let bounds = Bounds::centered(None, size(px(720.0), px(600.0)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Plan".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    let title = cx.new(|cx| {
                        TextInput::with_placeholder(i18n::t("plan.title"), cx)
                            .id("plan-title")
                            .tab_stop(true)
                    });
                    let steps = cx.new(|cx| {
                        TextInput::with_placeholder(i18n::t("plan.steps"), cx)
                            .code_editor()
                            .id("plan-steps")
                            .tab_stop(true)
                            .height_clamp(180.0, 260.0)
                    });
                    let reason = cx.new(|cx| {
                        TextInput::with_placeholder(i18n::t("plan.reason"), cx)
                            .id("plan-reason")
                            .tab_stop(true)
                    });
                    window.focus(&title.focus_handle(cx));
                    let mut view = PlanView {
                        access: PanelAccess::default(),
                        controller,
                        session,
                        version: None,
                        status: String::new(),
                        saved_title: String::new(),
                        saved_steps: String::new(),
                        title,
                        steps,
                        reason,
                        busy: false,
                        loaded: false,
                        error: None,
                        focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
                    };
                    view.request(
                        Err(AppQuery::PlanGet {
                            session_id: view.session.clone().into(),
                        }),
                        cx,
                    );
                    view
                })
            },
        ) {
            self.status_hint = Some(error.to_string());
        }
    }
}

impl PlanView {
    fn request(&mut self, request: Result<AppCommand, AppQuery>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let task = self.controller.product_request(request);
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
                .and_then(parse_plan);
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok((version, status, title, steps)) => {
                        view.loaded = true;
                        view.version = version;
                        view.status = status;
                        view.saved_title = title.clone();
                        view.saved_steps = steps.clone();
                        view.title.update(cx, |input, cx| input.set_text(title, cx));
                        view.steps.update(cx, |input, cx| input.set_text(steps, cx));
                    }
                    Err(error) => view.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn action(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if index >= 2
            && (self.title.read(cx).text() != self.saved_title
                || self.steps.read(cx).text() != self.saved_steps)
        {
            self.error = Some(i18n::t("plan.unsaved").into());
            cx.notify();
            return;
        }
        let session_id = self.session.clone().into();
        let expected_version = self.version.clone().unwrap_or_default();
        let request = match index {
            0 => Err(AppQuery::PlanGet { session_id }),
            1 => Ok(AppCommand::PlanSave {
                session_id,
                title: self.title.read(cx).text().into(),
                steps: self
                    .steps
                    .read(cx)
                    .text()
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
                expected_version: self.version.clone(),
            }),
            2 => Ok(AppCommand::PlanSubmit {
                session_id,
                expected_version,
            }),
            3 => Ok(AppCommand::PlanApprove {
                session_id,
                expected_version,
            }),
            4 => Ok(AppCommand::PlanReject {
                session_id,
                expected_version,
                reason: self.reason.read(cx).text().into(),
            }),
            _ => return,
        };
        self.request(request, cx);
    }
}

impl Render for PlanView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        install_appkit_tab_monitor(window, cx);
        self.access.begin();
        let mut actions = div().flex().flex_wrap().gap_2();
        for (index, label) in [
            "plan.refresh",
            "plan.save",
            "plan.submit",
            "plan.approve",
            "plan.reject",
        ]
        .iter()
        .enumerate()
        {
            let enabled =
                !self.busy && (index == 0 || self.loaded) && (index < 2 || self.version.is_some());
            let focus = self.focus[index].clone().tab_stop(enabled);
            let button = Button::new(format!("plan-action-{index}"))
                .label(i18n::t(label))
                .track_focus(&focus)
                .disabled(!enabled)
                .on_click(cx.listener(move |view, event, _, cx| {
                    if AppView::click_down_position(event).is_some() {
                        view.action(index, cx);
                    }
                }))
                .on_activate(cx.listener(move |view, _, _, cx| view.action(index, cx)));
            actions = actions.child(self.access.wrap(
                &format!("plan-action-{index}"),
                i18n::t(label),
                AxRole::Button,
                None,
                enabled,
                self.focus[index].is_focused(window),
                button,
            ));
        }
        let title = self.access.wrap(
            "plan-title",
            i18n::t("plan.title"),
            AxRole::TextArea,
            Some(self.title.read(cx).text().into()),
            !self.busy,
            self.title.focus_handle(cx).is_focused(window),
            self.title.clone(),
        );
        let steps = self.access.wrap(
            "plan-steps",
            i18n::t("plan.steps"),
            AxRole::TextArea,
            Some(self.steps.read(cx).text().into()),
            !self.busy,
            self.steps.focus_handle(cx).is_focused(window),
            self.steps.clone(),
        );
        let reason = self.access.wrap(
            "plan-reason",
            i18n::t("plan.reason"),
            AxRole::TextArea,
            Some(self.reason.read(cx).text().into()),
            !self.busy,
            self.reason.focus_handle(cx).is_focused(window),
            self.reason.clone(),
        );
        let status = format!(
            "{} · {}\n{}",
            self.version.as_deref().unwrap_or("—"),
            i18n::t(match self.status.as_str() {
                "draft" => "plan.draft",
                "in_review" => "plan.in_review",
                "changes_requested" => "plan.changes_requested",
                "approved" => "plan.approved",
                "rejected" => "plan.rejected",
                _ => "plan.empty",
            }),
            self.error.as_deref().unwrap_or("")
        );
        let status = self.access.wrap(
            "plan-status",
            &status,
            AxRole::StaticText,
            None,
            false,
            false,
            status.clone(),
        );
        self.access.sync(window, cx, Self::ax_action);
        div()
            .id("plan-panel")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(dark().bg.base)
            .text_color(dark().text.primary)
            .child(status)
            .child(i18n::t("plan.hint"))
            .when(!self.busy, |view| {
                view.child(title).child(steps).child(reason)
            })
            .child(actions)
            .when(self.busy, |view| view.child(i18n::t("plan.loading")))
    }
}

fn parse_plan(data: serde_json::Value) -> Result<(Option<String>, String, String, String), String> {
    if data.is_null() {
        return Ok((None, "none".into(), String::new(), String::new()));
    }
    let field = |key| {
        data[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "Invalid Plan response".to_string())
    };
    let version = field("version")?;
    let status = field("review_status")?;
    let title = field("title")?;
    let steps = data["steps"]
        .as_array()
        .ok_or_else(|| "Invalid Plan steps".to_string())?
        .iter()
        .map(|step| {
            step["text"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "Invalid Plan step".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    if version.is_empty()
        || !matches!(
            status.as_str(),
            "draft" | "in_review" | "changes_requested" | "approved" | "rejected"
        )
    {
        return Err("Invalid Plan response".into());
    }
    Ok((Some(version), status, title, steps))
}

impl PlanView {
    fn ax_action(&mut self, request: AxRequest, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.access.permits(&request) {
            return;
        }
        let input = match request.identifier.as_str() {
            "plan-title" => Some(&self.title),
            "plan-steps" => Some(&self.steps),
            "plan-reason" => Some(&self.reason),
            _ => None,
        };
        if let Some(input) = input {
            edit_input(input, request, window, cx);
        } else if let Some(index) = request
            .identifier
            .strip_prefix("plan-action-")
            .and_then(|s| s.parse().ok())
        {
            self.action(index, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn plan_window_keyboard_reaches_fields_and_actions(cx: &mut gpui::TestAppContext) {
        let platform = Arc::new(Platform::new());
        let (owner, host) = cx.add_window_view(|_, cx| {
            AppView::new(
                platform,
                std::env::temp_dir().join("plan-focus.sock"),
                None,
                cx,
            )
        });
        let before = host.windows();
        host.update(|_, cx| {
            owner.update(cx, |view, cx| {
                view.projection.active_session_id = Some("plan-focus".into());
                view.open_plan(cx);
            })
        });
        let panel = host
            .windows()
            .into_iter()
            .find(|id| !before.contains(id))
            .unwrap();
        let mut panel = gpui::VisualTestContext::from_window(panel, host);
        panel.run_until_parked();
        let plan = panel.update(|window, _| window.root::<PlanView>().unwrap().unwrap());
        panel.update(|window, cx| {
            plan.update(cx, |view, cx| {
                view.busy = false;
                view.loaded = true;
                window.focus(&view.title.focus_handle(cx));
                cx.notify();
            })
        });
        panel.refresh().unwrap();
        panel.update(|window, cx| {
            let view = plan.read(cx);
            for expected in [
                view.steps.focus_handle(cx),
                view.reason.focus_handle(cx),
                view.focus[0].clone(),
            ] {
                window.focus_next();
                assert!(expected.is_focused(window));
            }
            window.focus_prev();
            assert!(view.reason.focus_handle(cx).is_focused(window));
        });
    }
}
