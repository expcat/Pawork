//! Label 文本基础组件（R8 波 B 轨 1）。
//!
//! 统一 mod.rs 中散置的单行文本 span：色阶走 text.* token，视觉与迁移前一致。

use gpui::{
    App, IntoElement, Rems, RenderOnce, Rgba, SharedString, Styled, Window, div, prelude::*,
};

/// 单行静态文本；字号 / 颜色由调用方经 theme token 指定。
#[derive(IntoElement)]
pub struct Label {
    text: SharedString,
    size: Option<Rems>,
    color: Option<Rgba>,
}

impl Label {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            size: None,
            color: None,
        }
    }

    pub fn size(mut self, size: Rems) -> Self {
        self.size = Some(size);
        self
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Label {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut label = div();
        if let Some(size) = self.size {
            label = label.text_size(size);
        }
        if let Some(color) = self.color {
            label = label.text_color(color);
        }
        label.child(self.text)
    }
}
