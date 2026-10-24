//! Label 文本基础组件（R8 波 B 轨 1）。
//!
//! 统一 mod.rs 中散置的单行文本 span：色阶走 text.* token，视觉与迁移前一致。

use gpui::{
    div, prelude::*, App, IntoElement, Rems, RenderOnce, Rgba, SharedString, Styled, Window,
};

use crate::ui::theme::font;

/// 单行静态文本；字号 / 颜色由调用方经 theme token 指定。
#[derive(IntoElement)]
pub struct Label {
    text: SharedString,
    size: Option<Rems>,
    line_height: Option<Rems>,
    color: Option<Rgba>,
}

impl Label {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            size: None,
            line_height: None,
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

    /// 与字号同用 rem，供说明和分区标题指定对应行高。
    pub fn line_height(mut self, line_height: Rems) -> Self {
        self.line_height = Some(line_height);
        self
    }
}

impl RenderOnce for Label {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut label = div();
        if let Some(size) = self.size {
            label = label.text_size(size).line_height(
                self.line_height
                    .unwrap_or_else(|| font::control_line_height(size)),
            );
        } else if let Some(line_height) = self.line_height {
            label = label.line_height(line_height);
        }
        if let Some(color) = self.color {
            label = label.text_color(color);
        }
        label.child(self.text)
    }
}
