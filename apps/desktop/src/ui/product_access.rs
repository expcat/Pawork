//! The small product windows share the existing native accessibility bridge.
use super::accessibility::{AxAction, AxBridge, AxNode, AxRect, AxRequest, AxRole, AxTree};
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
