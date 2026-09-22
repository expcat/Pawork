//! GUI4：空状态视觉容器。不发布 AX；调用方保留原 identifier 与文案。

use gpui::{div, prelude::*, px, AnyElement, App, IntoElement, Pixels, RenderOnce, Styled, Window};

use crate::ui::components::icon::{icon_sized, Icon};
use crate::ui::theme::{metrics, theme};

/// 居中空状态：可选图标 + 调用方子节点（标题 / 说明 / 动作 / 骨架）。
#[derive(IntoElement)]
pub struct EmptyState {
    icon: Option<Icon>,
    icon_size: Pixels,
    gap: Pixels,
    pad_x: Pixels,
    pad_y: Pixels,
    children: Vec<AnyElement>,
}

impl EmptyState {
    pub fn new() -> Self {
        Self {
            icon: None,
            icon_size: px(32.0),
            gap: px(metrics::SPACE_2),
            pad_x: px(metrics::SPACE_6),
            pad_y: px(metrics::SPACE_6),
            children: Vec::new(),
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn icon_size(mut self, size: impl Into<Pixels>) -> Self {
        self.icon_size = size.into();
        self
    }

    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = px(gap);
        self
    }

    pub fn px(mut self, pad: f32) -> Self {
        self.pad_x = px(pad);
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }
}

impl Default for EmptyState {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut root = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap(self.gap)
            .px(self.pad_x)
            .py(self.pad_y);
        if let Some(icon) = self.icon {
            root = root.child(icon_sized(icon, self.icon_size).text_color(theme(cx).text.tertiary));
        }
        for child in self.children {
            root = root.child(child);
        }
        root
    }
}
