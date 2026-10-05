//! The small product windows share the existing native accessibility bridge.
use super::accessibility::{AxAction, AxBridge, AxNode, AxRect, AxRequest, AxRole, AxTree};
use super::components::button::{Button, ButtonPadding};
use super::components::label::Label;
use super::theme::{dark, font, metrics};
use super::*;
use gpui::AnyElement;

#[derive(Default)]
pub(super) struct PanelAccess {
    bridge: Option<AxBridge>,
    layouts: HashMap<String, ScrollHandle>,
    nodes: Vec<AxNode>,
    error: Option<String>,
}
impl PanelAccess {
    pub(super) fn begin(&mut self) {
        self.nodes.clear();
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
                AxRole::Button => node.action(AxAction::Press),
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
        if let Some(bridge) = &mut self.bridge {
            let size = window.viewport_size();
            let mut tree = AxTree::new(f32::from(size.width), f32::from(size.height));
            tree.children = self.nodes.clone();
            if let Err(error) = bridge.update(tree) {
                self.error = Some(error);
            }
        }
    }
    pub(super) fn permits(&self, request: &AxRequest) -> bool {
        self.nodes.iter().any(|node| {
            node.identifier == request.identifier
                && node.enabled
                && node.actions.contains(&request.action)
        })
    }
}

/// 产品子窗口共用的滚动表单壳。字段和动作由调用方标成 flex_none，避免被压扁。
pub(super) fn product_panel(id: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_4()
        .p_4()
        .bg(dark().bg.base)
        .text_color(dark().text.primary)
}

pub(super) fn product_heading(
    title: impl Into<SharedString>,
    note: impl Into<SharedString>,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap_2()
        .w_full()
        .child(
            div()
                .w_full()
                .text_size(font::TITLE)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(dark().text.primary)
                .child(title.into()),
        )
        .child(product_note(note))
}

pub(super) fn product_note(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .w_full()
        .flex_none()
        .whitespace_normal()
        .text_size(font::BASE)
        .line_height(gpui::rems(1.4))
        .text_color(dark().text.secondary)
        .child(text.into())
}

pub(super) fn product_error(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .w_full()
        .flex_none()
        .whitespace_normal()
        .text_size(font::BASE)
        .line_height(gpui::rems(1.4))
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
    div().flex().flex_none().flex_wrap().items_center().gap_2()
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
