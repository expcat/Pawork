//! Plan 的版本化编辑与明确批准，不使用聊天草稿代替计划。
use super::accessibility::{AxAction, AxRequest, AxRole};
use super::product_access::{
    edit_input, product_action, product_actions, product_error, product_field, product_heading,
    product_note, product_panel, PanelAccess,
};
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
    _tab_keys: gpui::Subscription,
}

impl AppView {
    pub(super) fn open_plan(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.projection.active_session_id.clone() else {
            return;
        };
        let controller = self.controller.clone();
        let text_scale = self.text_scale;
        let bounds = Bounds::centered(None, size(px(720.0), px(600.0)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(i18n::t("plan.open").into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                window.set_rem_size(px(text_scale.rem_pixels()));
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
                    // CodeEditor 把 Tab 绑成缩进，且该绑定早于 on_key_down。
                    // macOS 真窗口的裸 Tab 仍由 AppKit NSEvent monitor 在进入
                    // GPUI 之前消费，并调用 focus_next / focus_prev。这条拦截
                    // 只补 GPUI 模拟以及没有 NSEvent 的键盘事件，不改变 AppKit
                    // 路径，也不表示 macOS 真窗口的 Tab 故障已经复现或修好。
                    // intercept_keystrokes 是全应用订阅：每个 Plan 窗口各订一次，
                    // stop_propagation 要等全部拦截器跑完才拦 keymap。只处理
                    // 创建这次订阅的窗口，第二个 Plan 才不会把同一次 Tab 走两遍。
                    let owner = window.window_handle();
                    let tab_keys = cx.intercept_keystrokes(move |event, window, cx| {
                        plan_tab_key(owner, event, window, cx);
                    });
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
                        _tab_keys: tab_keys,
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

fn plan_tab_key(
    owner: gpui::AnyWindowHandle,
    event: &gpui::KeystrokeEvent,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    if window.window_handle() != owner {
        return;
    }
    let modifiers = &event.keystroke.modifiers;
    if event.keystroke.key != "tab" || modifiers.control || modifiers.alt || modifiers.platform {
        return;
    }
    if modifiers.shift {
        window.focus_prev();
    } else {
        window.focus_next();
    }
    cx.stop_propagation();
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
        window.set_window_title(i18n::t("plan.open"));
        #[cfg(target_os = "macos")]
        install_appkit_tab_monitor(window, cx);
        self.access.begin();
        let dirty = self.title.read(cx).text() != self.saved_title
            || self.steps.read(cx).text() != self.saved_steps;
        let primary = if dirty || self.version.is_none() {
            1
        } else if self.status == "in_review" {
            3
        } else if matches!(
            self.status.as_str(),
            "draft" | "changes_requested" | "rejected"
        ) {
            2
        } else {
            1
        };
        let mut actions = product_actions();
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
            let button = product_action(Button::new(format!("plan-action-{index}")))
                .label(i18n::t(label))
                .variant(if index == primary && enabled {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Raised
                })
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
        let status_line = format!(
            "{} · {}",
            self.version.as_deref().unwrap_or("—"),
            i18n::t(match self.status.as_str() {
                "draft" => "plan.draft",
                "in_review" => "plan.in_review",
                "changes_requested" => "plan.changes_requested",
                "approved" => "plan.approved",
                "rejected" => "plan.rejected",
                _ => "plan.empty",
            }),
        );
        let mut status_body = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_2()
            .w_full()
            .child(product_note(status_line.clone()));
        if let Some(error) = &self.error {
            status_body = status_body.child(product_error(error.clone()));
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
        let status = format!("{status_line}\n{}", self.error.as_deref().unwrap_or(""));
        let status = self.access.wrap(
            "plan-status",
            &status,
            AxRole::StaticText,
            None,
            false,
            false,
            status_body,
        );
        self.access
            .sync(window, cx, |view| &mut view.access, Self::ax_action);
        product_panel("plan-panel", &self.access)
            .child(product_heading(
                &mut self.access,
                Icon::Plan,
                i18n::t("plan.open"),
                i18n::t("plan.hint"),
            ))
            .child(status)
            .when(!self.busy, |view| {
                view.child(product_field(i18n::t("plan.title"), title))
                    .child(product_field(i18n::t("plan.steps"), steps))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(8.0))
                    .when(!self.busy, |section| {
                        section.child(product_field(i18n::t("plan.reason"), reason))
                    })
                    .child(actions),
            )
            .when(self.busy, |view| {
                view.child(product_note(i18n::t("plan.loading")))
            })
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
            if request.action == AxAction::Focus {
                if let Some(focus) = self.focus.get(index) {
                    window.focus(focus);
                    cx.notify();
                }
            } else if request.action == AxAction::Press {
                self.action(index, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plan_keyboard_host::PlanKeyboardHost;

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
        panel.update(|window, cx| {
            plan.update(cx, |view, cx| {
                view.ax_action(
                    AxRequest {
                        identifier: "plan-action-1".into(),
                        action: AxAction::Focus,
                        value: None,
                    },
                    window,
                    cx,
                );
                assert!(view.focus[1].is_focused(window));
                assert!(!view.busy, "AX Focus must not save a revision");
                assert!(view.version.is_none());
            });
        });
    }

    /// AC-03：正式 open_plan 与按键链。Tab / Shift-Tab 走拦截器里的
    /// focus_next / focus_prev；Enter 走按钮 on_activate。五次动作的请求
    /// 和回执都来自临时 Host，不直接调用 action。
    #[gpui::test]
    fn plan_window_keyboard_runs_actions_from_keystrokes(cx: &mut gpui::TestAppContext) {
        let host = PlanKeyboardHost::spawn(27, None).expect("plan host");
        let (owner, mut app) = open_app(cx, host.socket());
        until_connected(&mut app, &owner, &host);
        // run_app 打开主窗口后把焦点放在 Composer。这里在打开 Plan 前
        // 显式对齐那个句柄；关闭用的是 GPUI remove_window，不覆盖原生
        // traffic-light，也不证明系统会把焦点自动交还。
        let parent_focus = app.update(|window, cx| {
            let focus = owner.update(cx, |view, cx| view.composer_focus_handle(cx));
            window.focus(&focus);
            assert!(
                focus.is_focused(window),
                "composer did not take parent focus"
            );
            focus
        });
        let (panel_id, mut panel, plan) = open_plan_panel(&mut app, &owner, "ses-plan-main");
        until_plan(&mut panel, &plan, &host, "empty plan", |view, _| {
            !view.busy && view.loaded && view.version.is_none() && view.error.is_none()
        });
        assert_eq!(host.plan_methods(), ["plan_get"]);
        assert_eq!(
            panel.window_title().as_deref(),
            Some(super::super::i18n::t("plan.open"))
        );

        assert_eq!(focus_name(&mut panel, &plan), "title");
        tab_expect(&mut panel, &plan, "steps");
        tab_expect(&mut panel, &plan, "reason");
        tab_expect(&mut panel, &plan, "refresh");
        tab_expect(&mut panel, &plan, "save");
        tab_expect(&mut panel, &plan, "title");
        panel.update(|_, cx| {
            let view = plan.read(cx);
            assert!(
                view.steps.read(cx).text().is_empty(),
                "tab indented the steps field"
            );
        });
        assert_eq!(host.plan_methods(), ["plan_get"]);

        panel.simulate_input("Alpha");
        tab_expect(&mut panel, &plan, "steps");
        panel.simulate_input("step-one");
        tab_expect(&mut panel, &plan, "reason");
        panel.update(|_, cx| {
            assert_eq!(plan.read(cx).steps.read(cx).text(), "step-one");
        });
        panel.simulate_input("because");
        tab_expect(&mut panel, &plan, "refresh");
        tab_expect(&mut panel, &plan, "save");
        panel.simulate_keystrokes("enter");
        until_plan(&mut panel, &plan, &host, "saved draft", |view, cx| {
            !view.busy
                && view.version.as_deref() == Some("1")
                && view.status == "draft"
                && view.title.read(cx).text() == "Alpha"
                && view.steps.read(cx).text() == "step-one"
                && view.error.is_none()
        });

        shift_tab_expect(&mut panel, &plan, "refresh");
        panel.simulate_keystrokes("enter");
        until_plan(&mut panel, &plan, &host, "refreshed draft", |view, cx| {
            !view.busy
                && view.error.is_none()
                && view.title.read(cx).text() == "Alpha"
                && host.plan_methods() == ["plan_get", "plan_save", "plan_get"]
        });

        tab_expect(&mut panel, &plan, "save");
        tab_expect(&mut panel, &plan, "submit");
        panel.simulate_keystrokes("enter");
        until_plan(&mut panel, &plan, &host, "submitted", |view, _| {
            !view.busy && view.status == "in_review" && view.error.is_none()
        });
        tab_expect(&mut panel, &plan, "approve");
        panel.simulate_keystrokes("enter");
        until_plan(&mut panel, &plan, &host, "approved", |view, _| {
            !view.busy && view.status == "approved" && view.error.is_none()
        });
        tab_expect(&mut panel, &plan, "reject");
        panel.simulate_keystrokes("enter");
        until_plan(&mut panel, &plan, &host, "rejected", |view, _| {
            !view.busy && view.status == "rejected" && view.error.is_none()
        });

        let commands = host.plan_commands();
        assert_eq!(
            commands
                .iter()
                .map(|command| command.method.as_str())
                .collect::<Vec<_>>(),
            [
                "plan_get",
                "plan_save",
                "plan_get",
                "plan_submit",
                "plan_approve",
                "plan_reject"
            ]
        );
        for command in &commands {
            assert_eq!(command.params["session_id"].as_str(), Some("ses-plan-main"));
        }
        assert_eq!(commands[1].params["title"].as_str(), Some("Alpha"));
        assert_eq!(
            commands[1].params["steps"].as_array().map(|steps| {
                steps
                    .iter()
                    .filter_map(|step| step.as_str())
                    .collect::<Vec<_>>()
            }),
            Some(vec!["step-one"])
        );
        assert!(commands[1].params.get("expected_version").is_none());
        for command in &commands[3..] {
            assert_eq!(command.params["expected_version"].as_str(), Some("1"));
        }
        assert_eq!(commands[5].params["reason"].as_str(), Some("because"));

        assert_eq!(focus_name(&mut panel, &plan), "reject");
        let (other_id, mut other, other_plan) = open_plan_panel(&mut app, &owner, "ses-plan-other");
        until_plan(&mut other, &other_plan, &host, "second plan", |view, _| {
            !view.busy && view.loaded && view.error.is_none()
        });
        assert_eq!(focus_name(&mut panel, &plan), "reject");
        assert_eq!(focus_name(&mut other, &other_plan), "title");
        tab_expect(&mut panel, &plan, "title");
        assert_eq!(
            focus_name(&mut other, &other_plan),
            "title",
            "first window tab moved the second plan"
        );
        tab_expect(&mut other, &other_plan, "steps");
        assert_eq!(
            focus_name(&mut panel, &plan),
            "title",
            "second window tab moved the first plan"
        );
        other.update(|window, _| window.remove_window());
        assert!(!app.windows().contains(&other_id));

        assert!(app.windows().contains(&panel_id));
        panel.simulate_keystrokes("escape");
        assert!(
            app.windows().contains(&panel_id),
            "escape must not close the plan window"
        );
        panel.update(|window, _| window.remove_window());
        assert!(!app.windows().contains(&panel_id));
        app.update(|window, _| {
            assert!(
                parent_focus.is_focused(window),
                "parent composer focus changed"
            );
        });
        assert!(host.error().is_none(), "{:?}", host.error());
    }

    /// 旧 Host 在 product_request 发出前拒绝；未保存时 Submit 不派请求。
    /// Approve / Reject 走同一条未保存守卫。Refresh 回执被 Host 按住时，
    /// busy 会关掉动作的 tab stop，再按 Tab / Enter 不得另派请求。
    /// 只读不是本地灰态：清理后的 Submit 仍派请求，Host 回授权错误。
    #[gpui::test]
    fn plan_window_keyboard_rejects_old_host_unsaved_and_readonly(cx: &mut gpui::TestAppContext) {
        let old = PlanKeyboardHost::spawn(23, None).expect("old host");
        let (owner, mut app) = open_app(cx, old.socket());
        until_connected(&mut app, &owner, &old);
        let (old_panel, mut panel, plan) = open_plan_panel(&mut app, &owner, "ses-plan-old");
        until_plan(&mut panel, &plan, &old, "old host error", |view, _| {
            !view.busy && !view.loaded && view.error.as_deref().is_some_and(|e| e.contains("1.24"))
        });
        assert!(old.plan_methods().is_empty(), "{:?}", old.plan_commands());
        assert_eq!(focus_name(&mut panel, &plan), "title");
        tab_expect(&mut panel, &plan, "steps");
        tab_expect(&mut panel, &plan, "reason");
        tab_expect(&mut panel, &plan, "refresh");
        tab_expect(&mut panel, &plan, "title");
        tab_expect(&mut panel, &plan, "steps");
        tab_expect(&mut panel, &plan, "reason");
        tab_expect(&mut panel, &plan, "refresh");
        panel.simulate_keystrokes("enter");
        until_plan(
            &mut panel,
            &plan,
            &old,
            "refresh stayed local",
            |view, _| !view.busy && view.error.as_deref().is_some_and(|e| e.contains("1.24")),
        );
        assert!(old.plan_methods().is_empty(), "{:?}", old.labels());
        panel.update(|window, _| window.remove_window());
        assert!(!app.windows().contains(&old_panel));

        let host = PlanKeyboardHost::spawn(
            27,
            Some(StoredPlan {
                version: "1".into(),
                status: "draft".into(),
                title: "Alpha".into(),
                steps: vec!["step-one".into()],
            }),
        )
        .expect("plan host");
        app.update(|_, cx| {
            owner.update(cx, |view, cx| {
                view.socket = host.socket().to_path_buf();
                view.start_connect(cx);
            });
        });
        until_connected(&mut app, &owner, &host);
        let (_panel_id, mut panel, plan) = open_plan_panel(&mut app, &owner, "ses-plan-deny");
        until_plan(&mut panel, &plan, &host, "loaded draft", |view, cx| {
            !view.busy
                && view.loaded
                && view.version.as_deref() == Some("1")
                && view.title.read(cx).text() == "Alpha"
                && view.error.is_none()
        });
        let before = host.plan_methods();
        assert_eq!(focus_name(&mut panel, &plan), "title");
        panel.simulate_input("!");
        tab_expect(&mut panel, &plan, "steps");
        tab_expect(&mut panel, &plan, "reason");
        tab_expect(&mut panel, &plan, "refresh");
        tab_expect(&mut panel, &plan, "save");
        tab_expect(&mut panel, &plan, "submit");
        panel.simulate_keystrokes("enter");
        panel.update(|_, cx| {
            let unsaved = super::super::i18n::t("plan.unsaved");
            assert_eq!(plan.read(cx).error.as_deref(), Some(unsaved.as_ref()));
        });
        assert_eq!(host.plan_methods(), before, "unsaved submit dispatched");
        tab_expect(&mut panel, &plan, "approve");
        panel.simulate_keystrokes("enter");
        panel.update(|_, cx| {
            let unsaved = super::super::i18n::t("plan.unsaved");
            assert_eq!(plan.read(cx).error.as_deref(), Some(unsaved.as_ref()));
        });
        assert_eq!(host.plan_methods(), before, "unsaved approve dispatched");
        tab_expect(&mut panel, &plan, "reject");
        panel.simulate_keystrokes("enter");
        panel.update(|_, cx| {
            let unsaved = super::super::i18n::t("plan.unsaved");
            assert_eq!(plan.read(cx).error.as_deref(), Some(unsaved.as_ref()));
        });
        assert_eq!(host.plan_methods(), before, "unsaved reject dispatched");

        shift_tab_expect(&mut panel, &plan, "approve");
        shift_tab_expect(&mut panel, &plan, "submit");
        shift_tab_expect(&mut panel, &plan, "save");
        shift_tab_expect(&mut panel, &plan, "refresh");
        host.pause_responses();
        let held = host.plan_methods();
        panel.simulate_keystrokes("enter");
        let mut held_busy = false;
        for _ in 0..500 {
            panel.refresh().unwrap();
            panel.run_until_parked();
            let (busy, actions_closed) = panel.update(|_, cx| {
                let view = plan.read(cx);
                (
                    view.busy,
                    view.focus.iter().all(|focus| !focus.clone().tab_stop),
                )
            });
            if busy && host.plan_methods().len() == held.len() + 1 {
                assert!(actions_closed, "busy left an action tab stop");
                held_busy = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            held_busy,
            "refresh never stayed busy: {:?} {:?}",
            host.plan_methods(),
            host.error()
        );
        assert_eq!(focus_name(&mut panel, &plan), "refresh");
        panel.simulate_keystrokes("tab");
        assert_eq!(
            focus_name(&mut panel, &plan),
            "refresh",
            "tab left the busy refresh"
        );
        panel.simulate_keystrokes("enter");
        assert_eq!(
            host.plan_methods().len(),
            held.len() + 1,
            "busy enter dispatched"
        );
        assert!(
            panel.update(|_, cx| plan.read(cx).busy),
            "busy cleared before the held receipt"
        );
        host.release_responses();
        until_plan(
            &mut panel,
            &plan,
            &host,
            "refresh cleared edits",
            |view, cx| !view.busy && view.error.is_none() && view.title.read(cx).text() == "Alpha",
        );
        assert_eq!(
            host.plan_methods().len(),
            held.len() + 1,
            "held refresh dispatched twice"
        );
        host.set_readonly(true);
        tab_expect(&mut panel, &plan, "save");
        tab_expect(&mut panel, &plan, "submit");
        panel.simulate_keystrokes("enter");
        until_plan(&mut panel, &plan, &host, "readonly receipt", |view, _| {
            !view.busy
                && view.error.as_deref() == Some("Plan is read only")
                && view.status == "draft"
        });
        let commands = host.plan_commands();
        assert_eq!(
            commands.last().map(|command| command.method.as_str()),
            Some("plan_submit")
        );
        assert_eq!(
            commands
                .last()
                .and_then(|command| command.params["expected_version"].as_str()),
            Some("1")
        );
        assert!(host.error().is_none(), "{:?}", host.error());
    }

    fn open_app<'a>(
        cx: &'a mut gpui::TestAppContext,
        socket: &std::path::Path,
    ) -> (gpui::Entity<AppView>, &'a mut gpui::VisualTestContext) {
        let platform = std::sync::Arc::new(Platform::new());
        cx.add_window_view(|_, cx| AppView::new(platform, socket.to_path_buf(), None, cx))
    }

    fn until_connected(
        app: &mut gpui::VisualTestContext,
        owner: &gpui::Entity<AppView>,
        host: &PlanKeyboardHost,
    ) {
        for _ in 0..500 {
            app.refresh().unwrap();
            app.run_until_parked();
            let connected = app.update(|_, cx| {
                matches!(
                    owner.read(cx).projection.connection,
                    crate::projection::ConnectionState::Connected { .. }
                )
            });
            if connected {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("connect: {:?} {:?}", host.labels(), host.error());
    }

    fn open_plan_panel(
        app: &mut gpui::VisualTestContext,
        owner: &gpui::Entity<AppView>,
        session: &str,
    ) -> (
        gpui::AnyWindowHandle,
        gpui::VisualTestContext,
        gpui::Entity<PlanView>,
    ) {
        let before = app.windows();
        app.update(|_, cx| {
            owner.update(cx, |view, cx| {
                view.projection.active_session_id = Some(session.into());
                view.open_plan(cx);
            });
        });
        let panel_id = app
            .windows()
            .into_iter()
            .find(|id| !before.contains(id))
            .expect("plan window");
        let mut panel = gpui::VisualTestContext::from_window(panel_id, app);
        let plan = panel.update(|window, _| window.root::<PlanView>().unwrap().unwrap());
        (panel_id, panel, plan)
    }

    fn until_plan(
        panel: &mut gpui::VisualTestContext,
        plan: &gpui::Entity<PlanView>,
        host: &PlanKeyboardHost,
        what: &str,
        mut ready: impl FnMut(&PlanView, &gpui::App) -> bool,
    ) {
        for _ in 0..500 {
            panel.refresh().unwrap();
            panel.run_until_parked();
            if panel.update(|_, cx| ready(plan.read(cx), cx)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let detail = panel.update(|_, cx| {
            let view = plan.read(cx);
            format!(
                "busy={} loaded={} version={:?} status={} title={} steps={} error={:?} commands={:?} labels={:?} host={:?}",
                view.busy,
                view.loaded,
                view.version,
                view.status,
                view.title.read(cx).text(),
                view.steps.read(cx).text(),
                view.error,
                host.plan_commands(),
                host.labels(),
                host.error()
            )
        });
        panic!("{what}: {detail}");
    }

    fn focus_name(panel: &mut gpui::VisualTestContext, plan: &gpui::Entity<PlanView>) -> String {
        panel.update(|window, cx| {
            let view = plan.read(cx);
            if view.title.focus_handle(cx).is_focused(window) {
                return "title".into();
            }
            if view.steps.focus_handle(cx).is_focused(window) {
                return "steps".into();
            }
            if view.reason.focus_handle(cx).is_focused(window) {
                return "reason".into();
            }
            for (name, focus) in [
                ("refresh", &view.focus[0]),
                ("save", &view.focus[1]),
                ("submit", &view.focus[2]),
                ("approve", &view.focus[3]),
                ("reject", &view.focus[4]),
            ] {
                if focus.is_focused(window) {
                    return name.into();
                }
            }
            "other".into()
        })
    }

    fn tab_expect(
        panel: &mut gpui::VisualTestContext,
        plan: &gpui::Entity<PlanView>,
        expected: &str,
    ) {
        panel.simulate_keystrokes("tab");
        assert_eq!(focus_name(panel, plan), expected);
    }

    fn shift_tab_expect(
        panel: &mut gpui::VisualTestContext,
        plan: &gpui::Entity<PlanView>,
        expected: &str,
    ) {
        panel.simulate_keystrokes("shift-tab");
        assert_eq!(focus_name(panel, plan), expected);
    }

    #[derive(Clone)]
    struct StoredPlan {
        version: String,
        status: String,
        title: String,
        steps: Vec<String>,
    }

    mod plan_keyboard_host {
        use std::io::{self, Read, Write};
        use std::net::Shutdown;
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Condvar, Mutex};
        use std::thread::{self, JoinHandle};
        use std::time::Duration;

        use serde_json::{json, Value};

        use super::StoredPlan;

        #[derive(Clone, Debug, PartialEq)]
        pub(super) struct Captured {
            pub method: String,
            pub params: Value,
        }

        pub(super) struct PlanKeyboardHost {
            socket: PathBuf,
            _dir: tempfile::TempDir,
            commands: Arc<Mutex<Vec<Captured>>>,
            labels: Arc<Mutex<Vec<String>>>,
            error: Arc<Mutex<Option<String>>>,
            readonly: Arc<AtomicBool>,
            gate: Arc<ResponseGate>,
            shutdown: Arc<AtomicBool>,
            thread: Option<JoinHandle<()>>,
        }

        struct ResponseGate {
            paused: Mutex<bool>,
            cv: Condvar,
            shutdown: Arc<AtomicBool>,
        }

        impl ResponseGate {
            fn wait(&self) {
                let mut paused = lock(&self.paused);
                while *paused && !self.shutdown.load(Ordering::Acquire) {
                    let (guard, _) = self
                        .cv
                        .wait_timeout(paused, Duration::from_millis(50))
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    paused = guard;
                }
            }
        }

        impl PlanKeyboardHost {
            pub(super) fn spawn(minor: u64, plan: Option<StoredPlan>) -> io::Result<Self> {
                let dir = tempfile::Builder::new()
                    .prefix("pawork-ac03-")
                    .tempdir_in("/tmp")?;
                let socket = dir.path().join("pawork-gui-ac03.sock");
                let token = crate::platform::token_path_for_socket(&socket);
                std::fs::write(&token, b"ac03\n")?;
                let listener = UnixListener::bind(&socket)?;
                listener.set_nonblocking(true)?;
                let commands = Arc::new(Mutex::new(Vec::new()));
                let labels = Arc::new(Mutex::new(Vec::new()));
                let error = Arc::new(Mutex::new(None));
                let readonly = Arc::new(AtomicBool::new(false));
                let shutdown = Arc::new(AtomicBool::new(false));
                let gate = Arc::new(ResponseGate {
                    paused: Mutex::new(false),
                    cv: Condvar::new(),
                    shutdown: Arc::clone(&shutdown),
                });
                let plan = Arc::new(Mutex::new(plan));
                let thread = thread::spawn({
                    let commands = Arc::clone(&commands);
                    let labels = Arc::clone(&labels);
                    let error = Arc::clone(&error);
                    let readonly = Arc::clone(&readonly);
                    let gate = Arc::clone(&gate);
                    let shutdown = Arc::clone(&shutdown);
                    move || {
                        if let Err(failure) = accept_one(
                            listener, minor, &shutdown, &commands, &labels, &plan, &readonly, &gate,
                        ) {
                            *lock(&error) = Some(failure.to_string());
                        }
                    }
                });
                Ok(Self {
                    socket,
                    _dir: dir,
                    commands,
                    labels,
                    error,
                    readonly,
                    gate,
                    shutdown,
                    thread: Some(thread),
                })
            }

            pub(super) fn socket(&self) -> &Path {
                &self.socket
            }

            pub(super) fn set_readonly(&self, readonly: bool) {
                self.readonly.store(readonly, Ordering::Release);
            }

            pub(super) fn pause_responses(&self) {
                *lock(&self.gate.paused) = true;
            }

            pub(super) fn release_responses(&self) {
                *lock(&self.gate.paused) = false;
                self.gate.cv.notify_all();
            }

            pub(super) fn plan_commands(&self) -> Vec<Captured> {
                lock(&self.commands)
                    .iter()
                    .filter(|command| command.method.starts_with("plan_"))
                    .cloned()
                    .collect()
            }

            pub(super) fn plan_methods(&self) -> Vec<String> {
                self.plan_commands()
                    .into_iter()
                    .map(|command| command.method)
                    .collect()
            }

            pub(super) fn labels(&self) -> Vec<String> {
                lock(&self.labels).clone()
            }

            pub(super) fn error(&self) -> Option<String> {
                lock(&self.error).clone()
            }
        }

        impl Drop for PlanKeyboardHost {
            fn drop(&mut self) {
                self.shutdown.store(true, Ordering::Release);
                self.release_responses();
                if let Ok(stream) = UnixStream::connect(&self.socket) {
                    let _ = stream.shutdown(Shutdown::Both);
                }
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
            }
        }

        fn accept_one(
            listener: UnixListener,
            minor: u64,
            shutdown: &AtomicBool,
            commands: &Mutex<Vec<Captured>>,
            labels: &Mutex<Vec<String>>,
            plan: &Mutex<Option<StoredPlan>>,
            readonly: &AtomicBool,
            gate: &ResponseGate,
        ) -> io::Result<()> {
            loop {
                if shutdown.load(Ordering::Acquire) {
                    return Ok(());
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        if shutdown.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        return serve(
                            stream, minor, shutdown, commands, labels, plan, readonly, gate,
                        );
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
            }
        }

        fn serve(
            mut stream: UnixStream,
            minor: u64,
            shutdown: &AtomicBool,
            commands: &Mutex<Vec<Captured>>,
            labels: &Mutex<Vec<String>>,
            plan: &Mutex<Option<StoredPlan>>,
            readonly: &AtomicBool,
            gate: &ResponseGate,
        ) -> io::Result<()> {
            stream.set_nonblocking(false)?;
            stream.set_read_timeout(Some(Duration::from_millis(100)))?;
            stream.set_write_timeout(Some(Duration::from_millis(500)))?;
            let Some(bytes) = read_frame(&mut stream, shutdown)? else {
                return Ok(());
            };
            let frame: Value = serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if frame.get("type").and_then(Value::as_str) != Some("handshake") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "first frame was not a handshake",
                ));
            }
            push(labels, "handshake");
            let request_id = frame
                .pointer("/data/request_id")
                .and_then(Value::as_str)
                .unwrap_or("handshake");
            let versions = frame
                .pointer("/data/supported_api_versions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let Some(api) = negotiate(&versions, minor) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "client offered no usable api version",
                ));
            };
            write_frame(&mut stream, &handshake_accepted(request_id, &api))?;
            write_frame(&mut stream, &empty_snapshot())?;
            while let Some(bytes) = read_frame(&mut stream, shutdown)? {
                let frame: Value = match serde_json::from_slice(&bytes) {
                    Ok(frame) => frame,
                    Err(_) => {
                        push(labels, "bad-json");
                        continue;
                    }
                };
                dispatch(
                    &mut stream,
                    &frame,
                    &api,
                    commands,
                    labels,
                    plan,
                    readonly,
                    gate,
                )?;
            }
            Ok(())
        }

        fn dispatch(
            stream: &mut UnixStream,
            frame: &Value,
            api: &Value,
            commands: &Mutex<Vec<Captured>>,
            labels: &Mutex<Vec<String>>,
            plan: &Mutex<Option<StoredPlan>>,
            readonly: &AtomicBool,
            gate: &ResponseGate,
        ) -> io::Result<()> {
            match frame.get("type").and_then(Value::as_str).unwrap_or("") {
                "ack" => push(labels, "ack"),
                "subscribe" => push(labels, "subscribe"),
                "unsubscribe" => push(labels, "unsubscribe"),
                "pong" => push(labels, "pong"),
                "resume" => {
                    push(labels, "resume");
                    let request_id = frame
                        .pointer("/data/request_id")
                        .and_then(Value::as_str)
                        .unwrap_or("resume");
                    write_frame(
                        stream,
                        &json!({
                            "type": "resume",
                            "data": {
                                "request_id": request_id,
                                "disposition": {"type": "up_to_date", "current_sequence": 0}
                            }
                        }),
                    )?;
                }
                "heartbeat" => {
                    push(labels, "heartbeat");
                    let nonce = frame.pointer("/data/nonce").cloned().unwrap_or(json!(0));
                    write_frame(stream, &json!({"type": "pong", "data": {"nonce": nonce}}))?;
                }
                "snapshot_request" => {
                    push(labels, "snapshot_request");
                    write_frame(stream, &empty_snapshot())?;
                }
                "query" => answer_query(stream, frame, api, commands, labels, plan, gate)?,
                "command" => {
                    answer_command(stream, frame, api, commands, labels, plan, readonly, gate)?
                }
                other => push(labels, &format!("frame:{other}")),
            }
            Ok(())
        }

        fn answer_query(
            stream: &mut UnixStream,
            frame: &Value,
            api: &Value,
            commands: &Mutex<Vec<Captured>>,
            labels: &Mutex<Vec<String>>,
            plan: &Mutex<Option<StoredPlan>>,
            gate: &ResponseGate,
        ) -> io::Result<()> {
            let data = frame.get("data");
            let request_id = data
                .and_then(|data| data.get("request_id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let query = data.and_then(|data| data.get("query"));
            let method = query
                .and_then(|query| query.get("method"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let params = query
                .and_then(|query| query.get("params"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            push(labels, &format!("query:{method}"));
            if method.starts_with("plan_") {
                lock(commands).push(Captured {
                    method: method.to_string(),
                    params: params.clone(),
                });
            }
            if method.starts_with("plan_") {
                gate.wait();
            }
            let body = if method == "plan_get" {
                match lock(plan).clone() {
                    Some(plan) => snapshot(&plan),
                    None => Value::Null,
                }
            } else {
                json!({})
            };
            write_frame(stream, &response_frame(api, request_id, body))
        }

        fn answer_command(
            stream: &mut UnixStream,
            frame: &Value,
            api: &Value,
            commands: &Mutex<Vec<Captured>>,
            labels: &Mutex<Vec<String>>,
            plan: &Mutex<Option<StoredPlan>>,
            readonly: &AtomicBool,
            gate: &ResponseGate,
        ) -> io::Result<()> {
            let data = frame.get("data");
            let command_id = data
                .and_then(|data| data.get("command_id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let command = data.and_then(|data| data.get("command"));
            let method = command
                .and_then(|command| command.get("method"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let params = command
                .and_then(|command| command.get("params"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            push(labels, &format!("command:{method}"));
            if method.starts_with("plan_") {
                lock(commands).push(Captured {
                    method: method.to_string(),
                    params: params.clone(),
                });
            }
            if method.starts_with("plan_") {
                gate.wait();
            }
            if method.starts_with("plan_") && readonly.load(Ordering::Acquire) {
                return write_frame(stream, &error_frame(api, command_id, "Plan is read only"));
            }
            let body = match method {
                "plan_save" => {
                    let title = params
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let steps = params
                        .get("steps")
                        .and_then(Value::as_array)
                        .map(|steps| {
                            steps
                                .iter()
                                .filter_map(|step| step.as_str().map(str::to_owned))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let mut slot = lock(plan);
                    let version = slot
                        .as_ref()
                        .map(|plan| plan.version.clone())
                        .unwrap_or_else(|| "1".into());
                    let stored = StoredPlan {
                        version,
                        status: "draft".into(),
                        title,
                        steps,
                    };
                    let body = snapshot(&stored);
                    *slot = Some(stored);
                    body
                }
                "plan_submit" | "plan_approve" | "plan_reject" => {
                    let status = match method {
                        "plan_submit" => "in_review",
                        "plan_approve" => "approved",
                        _ => "rejected",
                    };
                    let mut slot = lock(plan);
                    let Some(stored) = slot.as_mut() else {
                        return write_frame(
                            stream,
                            &error_frame(api, command_id, "Plan is missing"),
                        );
                    };
                    stored.status = status.into();
                    snapshot(stored)
                }
                _ => json!({}),
            };
            write_frame(stream, &response_frame(api, command_id, body))
        }

        fn snapshot(plan: &StoredPlan) -> Value {
            json!({
                "version": plan.version,
                "review_status": plan.status,
                "title": plan.title,
                "steps": plan.steps.iter().map(|text| json!({"text": text})).collect::<Vec<_>>(),
            })
        }

        fn handshake_accepted(request_id: &str, api: &Value) -> Value {
            json!({
                "type": "handshake",
                "data": {
                    "status": "accepted",
                    "request_id": request_id,
                    "selected_api_version": api,
                    "handle": {"instance_id": "ac03", "api_version": api},
                    "client_id": "gui-ac03",
                    "connection_id": "conn-ac03",
                    "resume": {"type": "up_to_date", "current_sequence": 0},
                    "capabilities": ["snapshots"]
                }
            })
        }

        fn empty_snapshot() -> Value {
            json!({
                "type": "snapshot",
                "data": {
                    "instance_id": "ac03",
                    "snapshot_sequence": 0,
                    "generated_at": 0,
                    "sections": []
                }
            })
        }

        fn response_frame(api: &Value, request_id: &str, data: Value) -> Value {
            json!({
                "type": "response",
                "data": {
                    "api_version": api,
                    "request_id": request_id,
                    "responded_at": 0,
                    "response": {"type": "data", "data": data}
                }
            })
        }

        fn error_frame(api: &Value, request_id: &str, message: &str) -> Value {
            json!({
                "type": "response",
                "data": {
                    "api_version": api,
                    "request_id": request_id,
                    "responded_at": 0,
                    "response": {
                        "type": "error",
                        "data": {
                            "category": "authorization",
                            "message": message,
                            "retryable": false
                        }
                    }
                }
            })
        }

        fn negotiate(versions: &[Value], minor_cap: u64) -> Option<Value> {
            let mut best: Option<u64> = None;
            for version in versions {
                let Some(major) = version.get("major").and_then(Value::as_u64) else {
                    continue;
                };
                let Some(minor) = version.get("minor").and_then(Value::as_u64) else {
                    continue;
                };
                if major == 1 && minor <= minor_cap {
                    best = Some(best.map_or(minor, |current| current.max(minor)));
                }
            }
            best.map(|minor| json!({"major": 1, "minor": minor}))
        }

        fn read_frame(
            stream: &mut UnixStream,
            shutdown: &AtomicBool,
        ) -> io::Result<Option<Vec<u8>>> {
            let mut header = [0u8; 4];
            if read_exact(stream, &mut header, shutdown)?.is_none() {
                return Ok(None);
            }
            let len = u32::from_le_bytes(header) as usize;
            if len > 1024 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "frame too large",
                ));
            }
            let mut payload = vec![0u8; len];
            if read_exact(stream, &mut payload, shutdown)?.is_none() {
                return Ok(None);
            }
            Ok(Some(payload))
        }

        fn read_exact(
            stream: &mut UnixStream,
            buf: &mut [u8],
            shutdown: &AtomicBool,
        ) -> io::Result<Option<()>> {
            let mut filled = 0;
            while filled < buf.len() {
                if shutdown.load(Ordering::Acquire) {
                    return Ok(None);
                }
                match stream.read(&mut buf[filled..]) {
                    Ok(0) => return Ok(None),
                    Ok(count) => filled += count,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error)
                        if error.kind() == io::ErrorKind::TimedOut
                            || error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(Some(()))
        }

        fn write_frame(stream: &mut UnixStream, value: &Value) -> io::Result<()> {
            let payload = serde_json::to_vec(value)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let len = u32::try_from(payload.len())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
            stream.write_all(&len.to_le_bytes())?;
            stream.write_all(&payload)?;
            stream.flush()
        }

        fn push(labels: &Mutex<Vec<String>>, label: &str) {
            lock(labels).push(label.to_string());
        }

        fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
            mutex
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }
}
