//! Panel 面板容器（R8 波 B 轨 1）。
//!
//! 侧栏 / Inspector 的壳：bg.panel + 侧描边 + 固定宽，背景 / 描边 / 圆角取
//! 现状值，抽壳不改视觉。

use gpui::{div, prelude::*, AnyElement, App, IntoElement, Pixels, RenderOnce, Styled, Window};

use crate::ui::theme::dark;

/// Panel 基础面板容器。
#[derive(IntoElement)]
pub struct Panel {
    border_left: bool,
    border_right: bool,
    width: Option<Pixels>,
    gap_2: bool,
    padding_2: bool,
    children: Vec<AnyElement>,
}

impl Panel {
    /// 左描边面板（Inspector）。
    pub fn side_left(width: Pixels) -> Self {
        Self::new(Some(width), true, false)
    }

    /// 右描边面板（TaskRail 侧栏；gap-2 / p-2）。
    pub fn side_right(width: Pixels) -> Self {
        let mut panel = Self::new(Some(width), false, true);
        panel.gap_2 = true;
        panel.padding_2 = true;
        panel
    }

    fn new(width: Option<Pixels>, border_left: bool, border_right: bool) -> Self {
        Self {
            border_left,
            border_right,
            width,
            gap_2: false,
            padding_2: false,
            children: Vec::new(),
        }
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }
}

impl RenderOnce for Panel {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut panel = div().flex().flex_col().h_full().bg(dark().bg.panel);
        if let Some(width) = self.width {
            // 固定宽侧栏必须拒绝 flex shrink；否则 Workspace 内长文本的
            // min-content 宽度会把真实 Inspector 挤窄，而 AX 仍报告合同宽度。
            panel = panel.w(width).flex_none();
        }
        if self.border_left {
            panel = panel.border_l_1();
        }
        if self.border_right {
            panel = panel.border_r_1();
        }
        panel = panel.border_color(dark().border.subtle);
        if self.gap_2 {
            panel = panel.gap_2();
        }
        if self.padding_2 {
            panel = panel.p_2();
        }
        for child in self.children {
            panel = panel.child(child);
        }
        panel
    }
}
