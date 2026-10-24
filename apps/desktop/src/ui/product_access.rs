//! The small product windows share the existing native accessibility bridge.
use super::accessibility::{AxAction, AxBridge, AxNode, AxRect, AxRequest, AxRole, AxTree};
use super::components::button::{Button, ButtonPadding};
use super::components::label::Label;
use super::theme::{dark, font, metrics};
use super::*;
use gpui::{AnyElement, Bounds};

#[derive(Default)]
pub(super) struct PanelAccess {
    bridge: Option<AxBridge>,
    layouts: HashMap<String, ScrollHandle>,
    nodes: Vec<AxNode>,
    scroll: ScrollHandle,
    clips: HashMap<String, ScrollHandle>,
    last_focus: Option<String>,
    error: Option<String>,
}
impl PanelAccess {
    pub(super) fn begin(&mut self) {
        self.nodes.clear();
        self.clips.clear();
    }
    pub(super) fn wrap(
        &mut self,
        id: &str,
        label: &str,
        role: AxRole,
        value: Option<String>,
        enabled: bool,
        focused: bool,
        child: impl IntoElement,
    ) -> AnyElement {
        let handle = self
            .layouts
            .entry(id.into())
            .or_insert_with(ScrollHandle::new)
            .clone();
        let bounds = handle.bounds();
        let mut node = AxNode::new(
            id,
            role,
            label,
            AxRect::new(
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            ),
        )
        .enabled(enabled)
        .focused(focused);
        node.value = value;
        if enabled {
            node = match role {
                AxRole::Button => node.action(AxAction::Focus).action(AxAction::Press),
                AxRole::TextArea => node.action(AxAction::Focus).action(AxAction::SetValue),
                _ => node,
            };
        }
        self.nodes.push(node);
        div()
            .id(SharedString::from(format!("ax-{id}")))
            .flex_none()
            .track_scroll(&handle)
            .child(child)
            .into_any_element()
    }
    pub(super) fn sync<T: Render + 'static>(
        &mut self,
        window: &Window,
        cx: &mut Context<T>,
        access: fn(&mut T) -> &mut PanelAccess,
        action: fn(&mut T, AxRequest, &mut Window, &mut Context<T>),
    ) {
        // GPUI test windows have no native handle. Their layout/focus checks
        // exercise the nodes above; native AX installation needs a real window.
        if !cfg!(test) && self.bridge.is_none() && self.error.is_none() {
            let weak = cx.entity().downgrade();
            let async_window = window.to_async(cx);
            match AxBridge::install(window, move |request| {
                let weak = weak.clone();
                let mut async_window = async_window.clone();
                async_window
                    .foreground_executor()
                    .clone()
                    .spawn(async move {
                        let _ = weak.update_in(&mut async_window, |view, window, cx| {
                            action(view, request, window, cx)
                        });
                    })
                    .detach();
            }) {
                Ok(bridge) => {
                    self.bridge = Some(bridge);
                    cx.notify();
                }
                Err(error) => self.error = Some(error),
            }
        }
        // Read the same tracked bounds after prepaint, including scroll-only frames.
        let weak = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            let _ = weak.update(cx, |view, cx| {
                if access(view).publish(window) {
                    cx.notify();
                }
            });
        });
    }
    pub(super) fn clip(&mut self, id: &str, viewport: &ScrollHandle) {
        self.clips.insert(id.into(), viewport.clone());
    }
    pub(super) fn publish(&mut self, window: &Window) -> bool {
        let viewport = self.scroll.bounds();
        let focused = self
            .nodes
            .iter()
            .find(|node| node.focused && node.enabled)
            .map(|n| n.identifier.clone());
        let mut scrolled = false;
        if focused != self.last_focus {
            if let Some(id) = focused.as_ref() {
                if let Some(layout) = self.layouts.get(id) {
                    let bounds = layout.bounds();
                    if let Some(clip) = self.clips.get(id) {
                        scrolled |= Self::reveal(clip, bounds);
                    }
                    // Nested scrolls update their tracked bounds on the next frame.
                    // Recheck before marking the focus as revealed in both viewports.
                    if !scrolled {
                        scrolled |= Self::reveal(&self.scroll, bounds);
                    }
                    if !scrolled && bounds.size.height > px(0.0) {
                        self.last_focus = focused;
                    }
                }
            } else {
                self.last_focus = focused;
            }
        }
        for node in &mut self.nodes {
            if let Some(layout) = self.layouts.get(&node.identifier) {
                let mut bounds = layout.bounds().intersect(&viewport);
                if let Some(clip) = self.clips.get(&node.identifier) {
                    bounds = bounds.intersect(&clip.bounds());
                }
                node.bounds = AxRect::new(
                    bounds.origin.x.into(),
                    bounds.origin.y.into(),
                    f32::from(bounds.size.width).max(0.0),
                    f32::from(bounds.size.height).max(0.0),
                );
            }
        }
        if let Some(bridge) = &mut self.bridge {
            let size = window.viewport_size();
            let mut tree = AxTree::new(f32::from(size.width), f32::from(size.height));
            tree.children = self
                .nodes
                .iter()
                .filter(|n| n.bounds.width > 0.0 && n.bounds.height > 0.0)
                .cloned()
                .collect();
            if let Err(error) = bridge.update(tree) {
                self.error = Some(error);
            }
        }
        scrolled
    }
    fn reveal(scroll: &ScrollHandle, bounds: Bounds<Pixels>) -> bool {
        let viewport = scroll.bounds();
        if bounds.size.height <= px(0.0) || viewport.size.height <= px(0.0) {
            return false;
        }
        let top = viewport.top() + px(16.0);
        let bottom = viewport.bottom() - px(16.0);
        let delta = if bounds.top() < top || bounds.size.height > bottom - top {
            top - bounds.top()
        } else if bounds.bottom() > bottom {
            bottom - bounds.bottom()
        } else {
            px(0.0)
        };
        let offset = scroll.offset();
        let y = (offset.y + delta).clamp(-scroll.max_offset().height, px(0.0));
        scroll.set_offset(point(offset.x, y));
        scroll.offset() != offset
    }

    pub(super) fn permits(&self, request: &AxRequest) -> bool {
        self.nodes.iter().any(|node| {
            node.identifier == request.identifier
                && node.enabled
                && node.actions.contains(&request.action)
        })
    }

    #[cfg(all(test, feature = "ac05-task-usage"))]
    pub(super) fn node_label(&self, id: &str) -> Option<String> {
        self.nodes
            .iter()
            .find(|node| node.identifier == id)
            .map(|node| node.label.clone())
    }

    #[cfg(all(test, feature = "ac05-task-usage"))]
    pub(super) fn labels(&self) -> Vec<String> {
        self.nodes.iter().map(|node| node.label.clone()).collect()
    }
}

/// 产品子窗口共用的滚动表单壳。字段和动作由调用方标成 flex_none，避免被压扁。
pub(super) fn product_panel(id: &'static str, access: &PanelAccess) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .track_scroll(&access.scroll)
        .child(components::focus_ring::track_pointer_input())
        .capture_key_down(components::focus_ring::key_down)
        .size_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .p(px(24.0))
        .bg(dark().bg.base)
        .text_color(dark().text.primary)
}

pub(super) fn product_heading(
    access: &mut PanelAccess,
    icon: Icon,
    title: impl Into<SharedString>,
    note: impl Into<SharedString>,
) -> AnyElement {
    let title = title.into();
    let note = note.into();
    let label = format!("{title}\n{note}");
    let heading = div()
        .flex()
        .flex_col()
        .flex_none()
        .gap_2()
        .w_full()
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(12.0))
                .text_size(font::TITLE)
                .line_height(font::from_pixels(28.0))
                .child(components::icon::icon(icon))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(dark().text.primary)
                .child(title),
        )
        .child(product_note(note));
    access.wrap(
        "product-heading",
        &label,
        AxRole::StaticText,
        None,
        false,
        false,
        heading,
    )
}

pub(super) fn product_note(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .w_full()
        .flex_none()
        .whitespace_normal()
        .text_size(font::BASE)
        .line_height(font::DESCRIPTION_LINE_HEIGHT)
        .text_color(dark().text.secondary)
        .child(text.into())
}

pub(super) fn product_error(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .w_full()
        .flex_none()
        .whitespace_normal()
        .text_size(font::BASE)
        .line_height(font::DESCRIPTION_LINE_HEIGHT)
        .text_color(dark().semantic.danger_text)
        .child(text.into())
}

/// 字段名始终可见；控件本身不参与 flex shrink。
pub(super) fn product_field(label: &'static str, control: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap_2()
        .w_full()
        .child(
            Label::new(label)
                .size(font::BASE)
                .color(dark().text.secondary),
        )
        .child(control)
}

pub(super) fn product_actions() -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .py(px(2.0))
}

pub(super) fn product_action(button: Button) -> Button {
    button
        .height(px(metrics::ICON_BUTTON_SIZE))
        .vcenter()
        .padding(ButtonPadding::Horizontal(metrics::SPACE_3))
}

pub(super) fn edit_input(
    input: &Entity<TextInput>,
    request: AxRequest,
    window: &mut Window,
    cx: &mut App,
) {
    match request.action {
        AxAction::Focus => window.focus(&input.focus_handle(cx)),
        AxAction::SetValue => input.update(cx, |input, cx| {
            input.set_text(request.value.unwrap_or_default(), cx)
        }),
        _ => {}
    }
}
