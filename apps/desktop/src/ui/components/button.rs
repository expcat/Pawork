//! Button 基础组件（R8 波 B 轨 1）。
//!
//! 统一 mod.rs 中散置的手写按钮 div：variant 决定底色 / 文字色 / hover·active
//! 映射（design/README.md §8.1），尺寸与内边距经 builder 透传；除有意新增的
//! hover / active 外保持迁移前视觉不变。菜单浮层化后触发器不再内嵌面板，
//! 面板统一走 Dropdown（轨 2）。

use gpui::{
    div, prelude::*, px, App, ClickEvent, FocusHandle, IntoElement, KeyDownEvent, Pixels, Rems,
    RenderOnce, Rgba, SharedString, Styled, Window,
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
    /// 32×32 圆形图标按钮（Composer Send / Cancel 同槽）。颜色映射与
    /// Primary / Danger 相同，尺寸由 `icon_circle` builder 钉死，不改默认档。
    IconCircle,
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
    text_size: Option<Rems>,
    text_color: Option<Rgba>,
    disabled_text_color: Option<Rgba>,
    padding: ButtonPadding,
    width: Option<Pixels>,
    height: Option<Pixels>,
    max_width: Option<Pixels>,
    bordered: bool,
    radius: Option<f32>,
    center: bool,
    vcenter: bool,
    circle: bool,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
    on_activate: Option<Box<dyn Fn(&KeyDownEvent, &mut Window, &mut App) + 'static>>,
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
            max_width: None,
            bordered: false,
            radius: None,
            center: false,
            vcenter: false,
            circle: false,
            on_click: None,
            on_activate: None,
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

    /// 文字字号（theme::font rem token，随窗口字号档位缩放）。
    pub fn text_size(mut self, size: Rems) -> Self {
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

    /// 限制触发器最大宽度（Composer model 菜单过长名 truncate）。
    pub fn max_width(mut self, width: Pixels) -> Self {
        self.max_width = Some(width);
        self
    }

    /// 1px border.subtle 描边（量图「1px 描边」档，如 scope 全宽行）。
    pub fn bordered(mut self) -> Self {
        self.bordered = true;
        self
    }

    /// 覆盖默认圆角（px 数值；默认非 Ghost 为 rounded_md=6px）。
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(radius);
        self
    }

    /// 内容水平垂直居中（固定宽高角标 / 全宽行的文字定位）。
    pub fn center(mut self) -> Self {
        self.center = true;
        self
    }

    /// 仅垂直居中（文字保持左对齐，如 scope 全宽行：量图文字贴左内缩 12）。
    pub fn vcenter(mut self) -> Self {
        self.vcenter = true;
        self
    }

    /// Composer 动作槽：32×32 圆形图标按钮。不改默认 padding / radius，
    /// Rail / 审批按钮不得走此入口。
    pub fn icon_circle(mut self, size: f32) -> Self {
        if matches!(self.variant, ButtonVariant::Primary) {
            self.variant = ButtonVariant::IconCircle;
        }
        self.circle = true;
        self.padding(ButtonPadding::None)
            .width(gpui::px(size))
            .height(gpui::px(size))
            .radius(size / 2.0)
            .center()
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// 键盘激活（R3 Wave B Slice 5 P2b）：聚焦的 Button 上裸 Enter / Space
    /// 在行级 on_key_down 直接调用激活 handler（调用方保证与 on_click 同一
    /// 激活路径，禁合成 click 兜底——GPUI 把聚焦元素的 keyboard click 挂在
    /// keyup 合成路径，真窗口注入不可达，同 ListRow::on_activate）。是否
    /// stop_propagation 由调用方 handler 决定：Grouping / Scope / Model
    /// 菜单打开时调用方让位（不 stop）让根节点菜单 Enter 接管（spec §3.3）；
    /// disabled 按钮不激活。
    pub fn on_activate(
        mut self,
        handler: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }
}

impl ButtonVariant {
    /// （enabled 底色, hover / active 色）；Ghost 无底色。
    fn colors(self) -> (Option<Rgba>, Rgba) {
        match self {
            Self::Primary | Self::IconCircle => (Some(dark().accent.primary), dark().accent.hover),
            Self::Ghost => (None, dark().surface.raised),
            Self::Danger => (
                Some(dark().semantic.danger_bg),
                dark().semantic.danger_hover,
            ),
            Self::Success => (
                Some(dark().semantic.success_bg),
                dark().semantic.success_hover,
            ),
            Self::Raised => (Some(dark().surface.raised), dark().surface.hover),
        }
    }

    /// enabled 文字色（None = 继承容器）。
    fn text_color(self) -> Option<Rgba> {
        match self {
            Self::Primary | Self::IconCircle | Self::Danger | Self::Success => {
                Some(dark().text.on_accent)
            }
            Self::Raised => Some(dark().text.primary),
            Self::Ghost => None,
        }
    }

    /// disabled 底色（Ghost 无底色）。
    fn disabled_bg(self) -> Option<Rgba> {
        match self {
            Self::Primary | Self::IconCircle | Self::Danger | Self::Success => {
                Some(dark().border.strong)
            }
            Self::Raised => Some(dark().surface.disabled),
            Self::Ghost => None,
        }
    }

    /// disabled 文字色（None = 继承容器）。
    fn disabled_text_color(self) -> Option<Rgba> {
        match self {
            Self::Primary | Self::IconCircle | Self::Danger | Self::Success => {
                Some(dark().text.on_accent)
            }
            Self::Raised => Some(dark().text.disabled),
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
        if self.center {
            button = button.flex().items_center().justify_center();
        }
        if self.vcenter {
            button = button.flex().items_center();
        }
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
            button = button.text_size(size);
        }
        if !matches!(
            self.variant,
            ButtonVariant::Ghost | ButtonVariant::IconCircle
        ) && !self.circle
        {
            button = button.rounded_md();
        }
        if let Some(radius) = self.radius {
            button = button.rounded(px(radius));
        }
        if self.bordered {
            button = button.border_1().border_color(dark().border.subtle);
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
        if let Some(max_width) = self.max_width {
            button = button.max_w(max_width).min_w_0().overflow_hidden();
        }
        if enabled {
            // hover / active 只改背景（基准 §8.1）；active 复用 hover 色。
            button = button
                .hover(move |style| style.bg(hover))
                .active(move |style| style.bg(hover));
        }
        if let Some(label) = self.label {
            if self.max_width.is_some() {
                button = button.child(div().min_w_0().truncate().child(label));
            } else {
                button = button.child(label);
            }
        }
        if let Some(tooltip) = self.tooltip {
            button = button.tooltip(move |_, cx| crate::ui::tooltip_text(tooltip.clone(), cx));
        }
        if let Some(on_click) = self.on_click {
            button = button.on_click(on_click);
        }
        if let Some(on_activate) = self.on_activate {
            button = button.on_key_down(move |event: &KeyDownEvent, window, cx| {
                if enabled
                    && !event.keystroke.modifiers.modified()
                    && (event.keystroke.key == "enter" || event.keystroke.key == "space")
                {
                    on_activate(event, window, cx);
                }
            });
        }
        button
    }
}
