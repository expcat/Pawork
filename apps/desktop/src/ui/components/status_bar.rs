//! StatusBar 底部状态行容器（R8 波 B 轨 1）。

use gpui::{div, prelude::*, px, AnyElement, App, IntoElement, RenderOnce, Styled, Window};

use crate::ui::theme::{dark, font, metrics};

/// 底部 24px 状态行：bg.panel + 顶描边 + XS 次要文字。
#[derive(IntoElement)]
pub struct StatusBar {
    children: Vec<AnyElement>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for StatusBar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .h(px(metrics::STATUS_BAR_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(dark().border.subtle)
            .bg(dark().bg.panel)
            .text_size(px(font::XS))
            .text_color(dark().text.secondary)
            .children(self.children)
    }
}
