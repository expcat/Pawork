//! Switch 开关基础组件（OPT-3c / ADR-055）。
//!
//! 轨道 + 圆点两态控件：开 = accent.primary 轨道、圆点靠右；关 =
//! border.strong 轨道、圆点靠左。交互合同与 Button 同构：id 必填，
//! on_click / on_activate（Enter / Space）由调用方汇聚到同一入口并保证
//! disabled 不发布动作；状态文案（On / Off）由调用方并排渲染，AX value
//! 由 AX 层发布。

use gpui::{
    div, prelude::*, px, App, ClickEvent, FocusHandle, IntoElement, KeyDownEvent, RenderOnce,
    SharedString, Window,
};

use crate::ui::components::focus_ring::focus_ring;
use crate::ui::theme::dark;

/// 轨道宽（含 2px 内缩的圆点行程）。
pub const SWITCH_TRACK_WIDTH: f32 = 36.0;
/// 轨道高。
pub const SWITCH_TRACK_HEIGHT: f32 = 20.0;
/// 圆点直径。
pub const SWITCH_DOT_SIZE: f32 = 16.0;

/// Switch 组件：id 必填（点击 / 焦点依赖 stateful div）。
#[derive(IntoElement)]
pub struct Switch {
    id: SharedString,
    checked: bool,
    disabled: bool,
    tooltip: Option<SharedString>,
    focus: Option<FocusHandle>,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
    on_activate: Option<Box<dyn Fn(&KeyDownEvent, &mut Window, &mut App) + 'static>>,
}

impl Switch {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            checked: false,
            disabled: false,
            tooltip: None,
            focus: None,
            on_click: None,
            on_activate: None,
        }
    }

    /// 开态（轨道主色、圆点靠右）。
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// 禁用态：不响应 hover / 点击 / 键盘激活。
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// 主路径控件三件套：tab_stop + track_focus + 聚焦描边（同 Button）。
    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// 键盘激活（Enter / Space）：与 Button 同构，调用方保证与 on_click
    /// 同一入口，并自行衔接 keyup 合成 click 的吞除标记。
    pub fn on_activate(
        mut self,
        handler: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let enabled = !self.disabled;
        // 轨道色：开 = 主色；关 / 禁用 = strong 描边色（禁用不另开色档）。
        let track_bg = if self.checked {
            dark().accent.primary
        } else {
            dark().border.strong
        };
        let dot_color = if self.checked {
            dark().text.on_accent
        } else {
            dark().text.secondary
        };
        let mut switch = div()
            .id(self.id)
            .w(px(SWITCH_TRACK_WIDTH))
            .h(px(SWITCH_TRACK_HEIGHT))
            .flex()
            .items_center()
            .px(px(2.0))
            .rounded(px(SWITCH_TRACK_HEIGHT / 2.0))
            .bg(track_bg)
            .when(self.checked, |track| track.justify_end())
            .child(
                div()
                    .w(px(SWITCH_DOT_SIZE))
                    .h(px(SWITCH_DOT_SIZE))
                    .rounded_full()
                    .bg(dot_color),
            );
        if let Some(focus) = self.focus.as_ref() {
            switch = switch.tab_stop(true).track_focus(focus).relative();
        }
        // 聚焦描边以覆盖层绘制（零布局参与，见 components/focus_ring.rs），
        // 否则 2px 边框会挤占固定轨道的内容盒、推动圆点位移。
        if self
            .focus
            .as_ref()
            .is_some_and(|focus| focus.is_focused(window))
        {
            switch = switch.child(focus_ring(px(SWITCH_TRACK_HEIGHT / 2.0)));
        }
        if enabled {
            switch = switch
                .cursor_pointer()
                .hover(|style| style.bg(dark().accent.hover));
        }
        if let Some(tooltip) = self.tooltip {
            switch = switch.tooltip(move |_, cx| crate::ui::tooltip_text(tooltip.clone(), cx));
        }
        if enabled {
            if let Some(on_click) = self.on_click {
                switch = switch.on_click(on_click);
            }
        }
        if let Some(on_activate) = self.on_activate {
            switch = switch.on_key_down(move |event: &KeyDownEvent, window, cx| {
                if enabled
                    && !event.keystroke.modifiers.modified()
                    && (event.keystroke.key == "enter" || event.keystroke.key == "space")
                {
                    on_activate(event, window, cx);
                }
            });
        }
        switch
    }
}
