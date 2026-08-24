//! Label / Badge 文本基础组件（R8 波 B 轨 1）。
//!
//! 统一 mod.rs 中散置的单行文本 span：色阶走 text.* token，视觉与迁移前一致。
//! Badge 是状态语义别名（连接 / 运行状态），默认 XS + text.secondary。

use gpui::{div, prelude::*, px, App, IntoElement, RenderOnce, Rgba, SharedString, Styled, Window};

use crate::ui::theme::{dark, font};

/// 单行静态文本；字号 / 颜色由调用方经 theme token 指定。
#[derive(IntoElement)]
pub struct Label {
    text: SharedString,
    size: Option<f32>,
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

    pub fn size(mut self, size: f32) -> Self {
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
            label = label.text_size(px(size));
        }
        if let Some(color) = self.color {
            label = label.text_color(color);
        }
        label.child(self.text)
    }
}

/// 状态徽标文本（连接状态 / 运行状态）；默认 XS + text.secondary。
#[derive(IntoElement)]
pub struct Badge {
    text: SharedString,
}

impl Badge {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        Label::new(self.text)
            .size(font::XS)
            .color(dark().text.secondary)
    }
}
