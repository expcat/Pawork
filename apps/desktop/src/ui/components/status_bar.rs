//! StatusBar 底部状态行容器（R8 波 B 轨 1）。

use gpui::{div, prelude::*, px, AnyElement, App, IntoElement, RenderOnce, Styled, Window};

use crate::ui::theme::{dark, font, metrics};

/// 底部 24px 状态行：bg.panel + 顶描边 + SM 次要文字。
///
/// F-13 布局：信息串（RunStatusBar）在行内绝对居中。R6 Wave A 已把
/// Inspector 折叠态 Activity 触发器迁至 Workspace Header，StatusBar
/// 不再承载动作。高度与描边不动。
#[derive(IntoElement)]
pub struct StatusBar {
    centered: Option<AnyElement>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self { centered: None }
    }

    /// 行内绝对居中的信息串（忽略右侧触发器宽度，保持真居中）。
    pub fn centered(mut self, child: impl IntoElement) -> Self {
        self.centered = Some(child.into_any_element());
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
            .id("shell-status-bar")
            .debug_selector(|| "shell-status-bar".into())
            .h(px(metrics::STATUS_BAR_HEIGHT))
            .px_3()
            .relative()
            .flex()
            .items_center()
            .justify_end()
            .border_t_1()
            .border_color(dark().border.subtle)
            .bg(dark().bg.panel)
            .text_size(font::SM)
            .text_color(dark().text.secondary)
            .map(|bar| match self.centered {
                Some(centered) => bar.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top_0()
                        .bottom_0()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .justify_center()
                        .overflow_hidden()
                        .px_3()
                        .child(centered),
                ),
                None => bar,
            })
    }
}
