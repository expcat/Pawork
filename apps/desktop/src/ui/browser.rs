//! 浏览器面板的 GPUI 胶水；原生网页、导航与生命周期在 pawork-browser。

use std::{collections::HashMap, rc::Rc, time::Duration};

use gpui::{
    canvas, div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, ScrollHandle, Window,
};
use pawork_browser::{BrowserState, BrowserView};
use raw_window_handle::HasWindowHandle;

use super::{
    accessibility::{AxAction, AxNode, AxRect, AxRole},
    components::button::{Button, ButtonPadding, ButtonVariant},
    i18n::t,
    icon,
    theme::{dark, font},
    AppRoute, AppView, Icon, InspectorTab, TextInput,
};

pub(super) struct BrowserPanel {
    pub open: bool,
    pub input: Entity<TextInput>,
    pub focus: FocusHandle,
    pub native: Option<Rc<BrowserView>>,
    pub state: BrowserState,
    error: Option<String>,
    layouts: HashMap<&'static str, ScrollHandle>,
    controls: HashMap<&'static str, FocusHandle>,
    poll: Option<gpui::Task<()>>,
}

impl BrowserPanel {
    pub fn new(cx: &mut Context<AppView>) -> Self {
        let input = cx.new(|cx| {
            TextInput::with_placeholder(t("browser.placeholder"), cx)
                .id("browser-address")
                .height_clamp(32.0, 32.0)
        });
        let focus = input.read(cx).focus_handle(cx).tab_stop(true);
        Self {
            open: false,
            input,
            focus,
            native: None,
            state: BrowserState::default(),
            error: None,
            layouts: [
                "browser-back",
                "browser-forward",
                "browser-reload",
                "browser-address",
                "browser-go",
                "browser-page",
                "browser-status",
            ]
            .into_iter()
            .map(|id| (id, ScrollHandle::new()))
            .collect(),
            controls: [
                "browser-back",
                "browser-forward",
                "browser-reload",
                "browser-go",
            ]
            .into_iter()
            .map(|id| (id, cx.focus_handle().tab_stop(true)))
            .collect(),
            poll: None,
        }
    }

    pub fn close(&mut self, cx: &mut Context<AppView>) {
        self.open = false;
        if let Some(native) = self.native.take() {
            native.set_visible(false);
        }
        self.poll = None;
        self.state = BrowserState::default();
        self.error = None;
        self.input.update(cx, |input, cx| input.set_text("", cx));
    }

    fn actions(&self) -> [(&'static str, &'static str, Icon, bool); 4] {
        [
            (
                "browser-back",
                "browser.back",
                Icon::ChevronLeft,
                self.state.can_go_back,
            ),
            (
                "browser-forward",
                "browser.forward",
                Icon::ChevronRight,
                self.state.can_go_forward,
            ),
            (
                "browser-reload",
                if self.state.loading {
                    "browser.stop"
                } else {
                    "browser.reload"
                },
                if self.state.loading {
                    Icon::Cancel
                } else {
                    Icon::Refresh
                },
                self.native.is_some(),
            ),
            ("browser-go", "browser.go", Icon::ChevronRight, true),
        ]
    }

    fn status(&self) -> String {
        self.error
            .as_ref()
            .or(self.state.error.as_ref())
            .cloned()
            .unwrap_or_else(|| {
                if self.state.loading {
                    t("browser.loading").into()
                } else {
                    self.state.title.clone()
                }
            })
    }
}

impl AppView {
    pub(super) fn browser_pointer_focus(&self) -> impl IntoElement {
        let native = self.browser.native.clone();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window.on_mouse_event(move |_: &gpui::MouseDownEvent, phase, _, _| {
                    // WebKit 内的点击由 AppKit 分发；这里仅接到 GPUI 壳上的点击。
                    if phase == gpui::DispatchPhase::Capture {
                        if let Some(native) = &native {
                            if native.is_focused() {
                                native.focus_parent();
                            }
                        }
                    }
                });
            },
        )
        .absolute()
    }

    pub(super) fn focus_browser_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(native) = &self.browser.native {
            native.focus_parent();
        }
        window.focus(&self.browser.focus);
        window.dispatch_action(Box::new(super::text_input::SelectAll), cx);
        cx.notify();
    }

    pub(super) fn sync_browser_session(&mut self, cx: &mut Context<Self>) {
        let session = self.projection.active_session_id.clone();
        if self.browser_session == session {
            return;
        }
        if let Some(native) = &self.browser.native {
            native.set_visible(false);
        }
        self.browser.poll = None;
        let next = self
            .browser_sessions
            .remove(&session)
            .unwrap_or_else(|| BrowserPanel::new(cx));
        let previous = std::mem::replace(&mut self.browser, next);
        if previous.open {
            self.browser_sessions
                .insert(self.browser_session.take(), previous);
        }
        self.browser_session = session;
        self.inspector_open_tabs
            .retain(|tool| *tool != InspectorTab::Browser);
        if self.browser.open {
            self.remember_inspector_tab(InspectorTab::Browser);
        } else if self.inspector_tab == InspectorTab::Browser {
            self.inspector_tab = InspectorTab::Home;
        }
    }

    pub(super) fn ensure_browser_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser.native.is_some() && self.browser.poll.is_none() {
            self.browser.poll = Some(cx.spawn_in(window, async move |this, cx| loop {
                smol::Timer::after(Duration::from_millis(250)).await;
                if this
                    .update_in(cx, |view, window, cx| view.sync_browser_state(window, cx))
                    .is_err()
                {
                    break;
                }
            }));
        }
    }

    pub(super) fn browser_visible(&self) -> bool {
        self.route == AppRoute::Workspace
            && self.inspector_open
            && self.inspector_tab == InspectorTab::Browser
            && self.open_menu.is_none()
            && !self.quick_search.open
    }

    pub(super) fn browser_action(
        &mut self,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.route != AppRoute::Workspace
            || !self.inspector_open
            || self.inspector_tab != InspectorTab::Browser
        {
            return;
        }
        if action == "browser-go" {
            let url = match pawork_browser::normalize_url(self.browser.input.read(cx).text()) {
                Ok(url) => url,
                Err(error) => {
                    self.browser.error = Some(error);
                    cx.notify();
                    return;
                }
            };
            if self.browser.native.is_none() {
                let native = HasWindowHandle::window_handle(window)
                    .map_err(|error| error.to_string())
                    .and_then(BrowserView::new);
                match native {
                    Ok(native) => self.browser.native = Some(Rc::new(native)),
                    Err(error) => {
                        self.browser.error = Some(error);
                        cx.notify();
                        return;
                    }
                }
                self.ensure_browser_poll(window, cx);
            }
            self.browser.error = self
                .browser
                .native
                .as_ref()
                .and_then(|native| native.navigate(&url).err());
            if self.browser.error.is_none() {
                self.browser
                    .input
                    .update(cx, |input, cx| input.set_text(url, cx));
            }
        } else if let Some(native) = &self.browser.native {
            match action {
                "browser-back" if self.browser.state.can_go_back => native.back(),
                "browser-forward" if self.browser.state.can_go_forward => native.forward(),
                "browser-reload" if self.browser.state.loading => native.stop(),
                "browser-reload" => native.reload(),
                _ => return,
            }
            self.browser.error = None;
        }
        self.sync_browser_state(window, cx);
        cx.notify();
    }

    fn sync_browser_state(&mut self, _window: &Window, cx: &mut Context<Self>) {
        let Some(native) = &self.browser.native else {
            return;
        };
        let state = native.state();
        let url_changed = self.browser.state.url != state.url;
        if self.browser.state != state {
            self.browser.state = state;
            cx.notify();
        }
        if url_changed
            && !self.browser.state.url.is_empty()
            && self.browser.error.is_none()
            && self.browser.input.read(cx).text() != self.browser.state.url
        {
            let url = self.browser.state.url.clone();
            self.browser
                .input
                .update(cx, |input, cx| input.set_text(url, cx));
        }
    }

    pub(super) fn browser_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.browser.input.update(cx, |input, cx| {
            input.set_placeholder(t("browser.placeholder"), cx)
        });
        let mut toolbar = div()
            .flex()
            .items_center()
            .gap_1()
            .p_2()
            .flex_none()
            .w_full();
        for (id, label, glyph, enabled) in self.browser.actions() {
            if id == "browser-go" {
                toolbar = toolbar.child(
                    div()
                        .id("browser-address-layout")
                        .flex_1()
                        .min_w_0()
                        .track_scroll(&self.browser.layouts["browser-address"])
                        .child(self.browser.input.clone()),
                );
            }
            toolbar = toolbar.child(
                div()
                    .id(gpui::SharedString::from(format!("{id}-layout")))
                    .flex_none()
                    .track_scroll(&self.browser.layouts[id])
                    .child(
                        Button::new(id)
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::None)
                            .width(px(28.0))
                            .height(px(32.0))
                            .center()
                            .child(icon(glyph))
                            .tooltip(t(label))
                            .track_focus(&self.browser.controls[id])
                            .disabled(!enabled)
                            .on_click(cx.listener(move |view, event, window, cx| {
                                if !view.consume_button_key_click(id, event) {
                                    view.browser_action(id, window, cx);
                                }
                            }))
                            .on_activate(cx.listener(move |view, _, window, cx| {
                                if view.open_menu.is_none() {
                                    view.note_button_key_activate(id);
                                    view.browser_action(id, window, cx);
                                    cx.stop_propagation();
                                }
                            })),
                    ),
            );
        }
        let native = self.browser.native.clone();
        let visible = self.browser_visible();
        let empty = native.is_none();
        let status = self.browser.status();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(toolbar)
            .child(
                div()
                    .id("browser-page-layout")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .track_scroll(&self.browser.layouts["browser-page"])
                    .when(empty, |body| {
                        body.flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .p_4()
                            .text_size(font::SM)
                            .text_color(dark().text.secondary)
                            .child(icon(Icon::Network))
                            .child(t("browser.empty"))
                    })
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                // GPUI 的父级裁剪不会自动约束 AppKit 子视图。
                                if let Some(native) = &native {
                                    let clipped = bounds.intersect(&window.content_mask().bounds);
                                    native.set_bounds(
                                        f32::from(clipped.origin.x) as f64,
                                        f32::from(clipped.origin.y) as f64,
                                        f32::from(clipped.size.width).max(0.0) as f64,
                                        f32::from(clipped.size.height).max(0.0) as f64,
                                    );
                                    native.set_visible(
                                        visible
                                            && clipped.size.width > px(0.0)
                                            && clipped.size.height > px(0.0),
                                    );
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .when(!status.is_empty(), |body| {
                body.child(
                    div()
                        .id("browser-status-layout")
                        .track_scroll(&self.browser.layouts["browser-status"])
                        .flex_none()
                        .px_2()
                        .py_1()
                        .text_size(font::SM)
                        .text_color(
                            if self.browser.error.is_some() || self.browser.state.error.is_some() {
                                dark().semantic.warning_text
                            } else {
                                dark().text.secondary
                            },
                        )
                        .child(status),
                )
            })
    }

    pub(super) fn browser_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let rect = |id: &str| {
            let bounds = self.browser.layouts[id].bounds();
            AxRect::new(
                bounds.origin.x.into(),
                bounds.origin.y.into(),
                bounds.size.width.into(),
                bounds.size.height.into(),
            )
        };
        let mut node = AxNode::new("browser", AxRole::Group, t("inspector.tab_browser"), frame)
            .child(
                AxNode::new(
                    "browser-address",
                    AxRole::TextArea,
                    t("browser.address"),
                    rect("browser-address"),
                )
                .value(self.browser.input.read(cx).text())
                .focused(self.browser.focus.is_focused(window))
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
            );
        for (id, label, _, enabled) in self.browser.actions() {
            node = node.child(
                AxNode::new(id, AxRole::Button, t(label), rect(id))
                    .enabled(enabled)
                    .focused(self.browser.controls[id].is_focused(window))
                    .action(AxAction::Press),
            );
        }
        let status = self.browser.status();
        if !status.is_empty() {
            node = node.child(AxNode::new(
                "browser-status",
                AxRole::StaticText,
                status,
                rect("browser-status"),
            ));
        }
        node
    }
}

impl AppView {
    pub(super) fn dispatch_browser_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((session, run_id, request_id, action)) = self.browser_request.take() else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            smol::Timer::after(Duration::from_millis(1)).await;
            let operation = this.update_in(cx, |view, window, cx| {
                if view.projection.active_session_id.as_deref() != Some(&session)
                    || view.projection.active_run_id.as_deref() != Some(&run_id)
                {
                    return Err("Task is no longer visible".to_string());
                }
                let name = action["action"].as_str().unwrap_or("");
                if name == "close" {
                    view.browser.close(cx);
                    view.inspector_open_tabs
                        .retain(|tab| *tab != InspectorTab::Browser);
                    if view.inspector_tab == InspectorTab::Browser {
                        view.inspector_tab = InspectorTab::Home;
                    }
                    cx.notify();
                    return Ok(None);
                }
                view.browser.open = true;
                view.remember_inspector_tab(InspectorTab::Browser);
                view.inspector_tab = InspectorTab::Browser;
                view.inspector_open = true;
                view.close_open_menu(cx);
                if view.browser.native.is_none() {
                    if name != "navigate" {
                        return Err("Browser has no page; navigate first".into());
                    }
                    let native = HasWindowHandle::window_handle(window)
                        .map_err(|e| e.to_string())
                        .and_then(BrowserView::new)?;
                    view.browser.native = Some(Rc::new(native));
                    view.ensure_browser_poll(window, cx);
                }
                let native = view.browser.native.clone().unwrap();
                match name {
                    "navigate" => native.navigate(action["url"].as_str().unwrap_or(""))?,
                    "back" if native.state().can_go_back => native.back(),
                    "forward" if native.state().can_go_forward => native.forward(),
                    "reload" => native.reload(),
                    "read" | "click" | "type" => {}
                    _ => return Err("Unsupported action or unavailable history".into()),
                }
                cx.notify();
                Ok(Some(native))
            });
            let result: Result<serde_json::Value, String> = match operation {
                Ok(Ok(Some(_native))) => {
                    let name = action["action"].as_str().unwrap_or("");
                    if matches!(name, "navigate" | "back" | "forward" | "reload") {
                        smol::Timer::after(Duration::from_millis(200)).await;
                        let started = std::time::Instant::now();
                        loop {
                            let loading = this.update_in(cx, |view, _window, _cx| {
                                view.browser
                                    .native
                                    .as_ref()
                                    .map(|native| native.state().loading)
                                    .unwrap_or(false)
                            });
                            if !matches!(loading, Ok(true))
                                || started.elapsed() >= Duration::from_secs(15)
                            {
                                break;
                            }
                            smol::Timer::after(Duration::from_millis(100)).await;
                        }
                    }
                    let state = this
                        .update_in(cx, |view, _window, _cx| {
                            view.browser
                                .native
                                .as_ref()
                                .map(|native| native.state())
                                .ok_or_else(|| "Browser page closed".to_string())
                        })
                        .map_err(|e| e.to_string())
                        .and_then(|state| state);
                    match state {
                        Err(error) => Err(error),
                        Ok(state) => {
                            if let Some(error) = state.error {
                                Err(error)
                            } else if state.loading {
                                Err("Page is still loading; read again later".into())
                            } else {
                                let (send, receive) = smol::channel::bounded(1);
                                let callback = move |result| {
                                    let _ = send.try_send(result);
                                };
                                let started = this
                                    .update_in(cx, |view, _window, _cx| {
                                        let Some(native) = view.browser.native.clone() else {
                                            return Err("Browser page closed".to_string());
                                        };
                                        match name {
                                            "click" => native.click(
                                                action["selector"].as_str().unwrap_or(""),
                                                callback,
                                            ),
                                            "type" => native.type_text(
                                                action["selector"].as_str().unwrap_or(""),
                                                action["text"].as_str().unwrap_or(""),
                                                callback,
                                            ),
                                            _ => native.read_page(callback),
                                        }
                                        Ok(())
                                    })
                                    .map_err(|e| e.to_string())
                                    .and_then(|started| started);
                                if let Err(error) = started {
                                    Err(error)
                                } else {
                                    let result = smol::future::or(
                                        async {
                                            receive.recv().await.unwrap_or_else(|_| {
                                                Err("Browser callback closed".into())
                                            })
                                        },
                                        async {
                                            smol::Timer::after(Duration::from_secs(5)).await;
                                            Err("Browser did not return a result".into())
                                        },
                                    )
                                    .await;
                                    result.and_then(|json| {
                                        serde_json::from_str(&json).map_err(|e| e.to_string())
                                    })
                                }
                            }
                        }
                    }
                }
                Ok(Ok(None)) => Ok(serde_json::json!({"closed":true})),
                Ok(Err(error)) => Err(error),
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |view, cx| {
                let reply = match result {
                    Ok(data) => serde_json::json!({"ok":true,"data":data}),
                    Err(error) => serde_json::json!({"ok":false,"error":error}),
                };
                view.controller.browser_respond(request_id, reply);
                cx.notify();
            });
        })
        .detach();
    }
}
