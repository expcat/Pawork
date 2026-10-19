//! GUI4：空状态视觉容器。不发布 AX；调用方保留原 identifier 与文案。

use gpui::{
    div, prelude::*, px, AnyElement, App, IntoElement, Pixels, RenderOnce, Rgba, SharedString,
    Styled, Window,
};

use crate::ui::components::icon::{icon_sized, Icon};
use crate::ui::components::label::Label;
use crate::ui::i18n::t;
use crate::ui::theme::{dark, font, metrics, theme};

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

    pub fn labeled(
        icon: Icon,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        title_color: Rgba,
    ) -> Self {
        Self::new()
            .icon(icon)
            .icon_size(px(metrics::ICON_SIZE))
            .child(Label::new(title).size(font::BASE).color(title_color))
            .child(
                Label::new(description)
                    .size(font::SM)
                    .color(dark().text.secondary),
            )
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn icon_size(mut self, size: impl Into<Pixels>) -> Self {
        self.icon_size = size.into();
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

pub fn stale_notice(reason: impl AsRef<str>) -> gpui::Div {
    let reason = reason.as_ref();
    div()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(dark().semantic.warning_text)
        .text_size(font::XS)
        .text_color(dark().semantic.warning_text)
        .child(format!("{} · {reason}", t("common.stale_data")))
}
