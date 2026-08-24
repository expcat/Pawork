//! Button 基础组件（R8 波 B 轨 1）。
//!
//! 统一 mod.rs 中散置的手写按钮 div：variant 决定底色 / 文字色 / hover·active
//! 映射（design/README.md §8.1），尺寸与内边距经 builder 透传；除有意新增的
//! hover / active 外保持迁移前视觉不变。菜单浮层化后触发器不再内嵌面板，
//! 面板统一走 Dropdown（轨 2）。

use gpui::{
    div, prelude::*, px, App, ClickEvent, FocusHandle, IntoElement, Pixels, RenderOnce, Rgba,
    SharedString, Styled, Window,
};

use crate::ui::theme::dark;

/// 按钮形态：决定底色、文字色与 hover / active 映射（design/README.md §8.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    /// accent.primary 主按钮；hover → accent.hover；禁用底 border.strong。
    Primary,
    /// 无底色文本按钮；hover → surface.raised；文字色默认继承。
    Ghost,
    /// semantic.danger_bg 危险操作；hover → semantic.danger_hover。
    Danger,
    /// semantic.success_bg 允许操作；hover → semantic.success_hover。
    Success,
    /// surface.raised 控件面；hover → surface.hover。
    Raised,
    /// 方形图标按钮（尺寸由调用方给定）；底色映射同 Raised。
    Icon,
}

/// 内边距档位（迁移前现状值；None 用于固定宽高的图标位）。
#[derive(Debug, Clone, Copy)]
pub enum ButtonPadding {
    /// 不设内边距（固定宽高 / 继承容器）。
    None,
    /// px-2 / py-1（默认）。
    Normal,
    /// px-3 / py-1（Cancel / Send）。
    Wide,
    /// 仅水平内边距（px 数值经 theme::metrics 传入）。
    Horizontal(f32),
}

/// Button 组件：id 必填（active / on_click 依赖 stateful div）。
#[derive(IntoElement)]
pub struct Button {
    id: SharedString,
    variant: ButtonVariant,
    disabled: bool,
    label: Option<SharedString>,
    tooltip: Option<SharedString>,
    focus: Option<FocusHandle>,
    text_size: Option<f32>,
    text_color: Option<Rgba>,
    disabled_text_color: Option<Rgba>,
    padding: ButtonPadding,
    width: Option<Pixels>,
    height: Option<Pixels>,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl Button {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            variant: ButtonVariant::Primary,
            disabled: false,
            label: None,
            tooltip: None,
            focus: None,
            text_size: None,
            text_color: None,
            disabled_text_color: None,
            padding: ButtonPadding::Normal,
            width: None,
            height: None,
            on_click: None,
        }
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// 禁用态：切换底 / 文字色并按基准 §8.1 关闭 hover / active。
    /// on_click 是否挂接由调用方决定（与迁移前逐点一致）。
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// 主路径按钮三件套：tab_stop + track_focus + 聚焦描边。
    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    /// 文字字号（theme::font 数值，渲染时转 px）。
    pub fn text_size(mut self, size: f32) -> Self {
        self.text_size = Some(size);
        self
    }

    /// 覆盖 enabled 文字色（默认按 variant 映射；Ghost 默认继承）。
    pub fn text_color(mut self, color: Rgba) -> Self {
        self.text_color = Some(color);
        self
    }

    /// 覆盖 disabled 文字色（如 terminal-start 保持继承色）。
    pub fn disabled_text_color(mut self, color: Rgba) -> Self {
        self.disabled_text_color = Some(color);
        self
    }

    pub fn padding(mut self, padding: ButtonPadding) -> Self {
        self.padding = padding;
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl ButtonVariant {
    /// （enabled 底色, hover / active 色）；Ghost 无底色。
    fn colors(self) -> (Option<Rgba>, Rgba) {
        match self {
            Self::Primary => (Some(dark().accent.primary), dark().accent.hover),
            Self::Ghost => (None, dark().surface.raised),
            Self::Danger => (
                Some(dark().semantic.danger_bg),
                dark().semantic.danger_hover,
            ),
            Self::Success => (
                Some(dark().semantic.success_bg),
                dark().semantic.success_hover,
            ),
            Self::Raised | Self::Icon => (Some(dark().surface.raised), dark().surface.hover),
        }
    }

    /// enabled 文字色（None = 继承容器）。
    fn text_color(self) -> Option<Rgba> {
        match self {
            Self::Primary | Self::Danger | Self::Success => Some(dark().text.on_accent),
            Self::Raised | Self::Icon => Some(dark().text.primary),
            Self::Ghost => None,
        }
    }

    /// disabled 底色（Ghost 无底色）。
    fn disabled_bg(self) -> Option<Rgba> {
        match self {
            Self::Primary | Self::Danger | Self::Success => Some(dark().border.strong),
            Self::Raised | Self::Icon => Some(dark().surface.disabled),
            Self::Ghost => None,
        }
    }

    /// disabled 文字色（None = 继承容器）。
    fn disabled_text_color(self) -> Option<Rgba> {
        match self {
            Self::Primary | Self::Danger | Self::Success => Some(dark().text.on_accent),
            Self::Raised | Self::Icon => Some(dark().text.disabled),
            Self::Ghost => None,
        }
    }
}

fn focus_ring_style<T: Styled>(this: T) -> T {
    this.border_1().border_color(dark().accent.primary)
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let enabled = !self.disabled;
        let (rest_bg, hover) = self.variant.colors();
        let bg = if enabled {
            rest_bg
        } else {
            self.variant.disabled_bg()
        };
        let text_color = if enabled {
            self.text_color.or_else(|| self.variant.text_color())
        } else {
            self.disabled_text_color
                .or_else(|| self.variant.disabled_text_color())
        };

        let mut button = div().id(self.id);
        if let Some(focus) = self.focus.as_ref() {
            button = button
                .tab_stop(true)
                .track_focus(focus)
                .focus(focus_ring_style);
        }
        button = button.cursor_pointer();
        if let Some(bg) = bg {
            button = button.bg(bg);
        }
        if let Some(color) = text_color {
            button = button.text_color(color);
        }
        if let Some(size) = self.text_size {
            button = button.text_size(px(size));
        }
        if !matches!(self.variant, ButtonVariant::Ghost) {
            button = button.rounded_md();
        }
        button = match self.padding {
            ButtonPadding::None => button,
            ButtonPadding::Normal => button.px_2().py_1(),
            ButtonPadding::Wide => button.px_3().py_1(),
            ButtonPadding::Horizontal(x) => button.px(px(x)),
        };
        if let Some(width) = self.width {
            button = button.w(width);
        }
        if let Some(height) = self.height {
            button = button.h(height);
        }
        if enabled {
            // hover / active 只改背景（基准 §8.1）；active 复用 hover 色。
            button = button
                .hover(move |style| style.bg(hover))
                .active(move |style| style.bg(hover));
        }
        if let Some(label) = self.label {
            button = button.child(label);
        }
        if let Some(tooltip) = self.tooltip {
            button = button.tooltip(move |_, cx| crate::ui::tooltip_text(tooltip.clone(), cx));
        }
        if let Some(on_click) = self.on_click {
            button = button.on_click(on_click);
        }
        button
    }
}
