//! 按输入方式显示的焦点描边（零布局参与）。
//!
//! GPUI border 参与盒模型，动态添加会压缩内容或改变尺寸。这里使用
//! absolute + inset_0 覆盖层，无 hitbox、不 track_focus、不参与布局。
//! 控件持焦且使用键盘导航时才绘制中性描边；鼠标点击不显示。
//! 业务选中态与焦点独立，禁止用动态 border / padding 改变控件几何。

use gpui::{
    div, prelude::*, px, AbsoluteLength, App, Global, IntoElement, KeyDownEvent, MouseDownEvent,
    RenderOnce, Window,
};

use crate::ui::theme::{dark, metrics};

/// Desktop 为单窗口；仅记录输入方式，不改变 FocusHandle 或业务状态。
#[derive(Default)]
struct KeyboardFocus(bool);
impl Global for KeyboardFocus {}

fn keyboard_focus_visible(cx: &App) -> bool {
    cx.try_global::<KeyboardFocus>()
        .is_some_and(|state| state.0)
}

pub fn set_keyboard_focus(visible: bool, window: &mut Window, cx: &mut App) {
    if keyboard_focus_visible(cx) != visible {
        cx.set_global(KeyboardFocus(visible));
        window.refresh();
    }
}

/// 窗口 capture 阶段更新；菜单 occlude 或子控件吞事件也不能留下旧描边。
pub fn track_pointer_input() -> impl IntoElement {
    gpui::canvas(
        |_, _, _| (),
        |_, _, window, _| {
            window.on_mouse_event(|_: &MouseDownEvent, phase, window, cx| {
                if phase == gpui::DispatchPhase::Capture {
                    set_keyboard_focus(false, window, cx);
                }
            });
        },
    )
    .absolute()
}

pub fn key_down(event: &KeyDownEvent, window: &mut Window, cx: &mut App) {
    if matches!(
        event.keystroke.key.as_str(),
        "tab" | "up" | "down" | "left" | "right" | "home" | "end"
    ) {
        set_keyboard_focus(true, window, cx);
    }
}

#[derive(IntoElement)]
pub struct FocusRing(AbsoluteLength);

/// 仅应在控件持焦时挂载；输入方式在实际渲染时统一决定是否绘制。
pub fn focus_ring(radius: impl Into<AbsoluteLength>) -> FocusRing {
    FocusRing(radius.into())
}

impl RenderOnce for FocusRing {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .absolute()
            .inset_0()
            .rounded(self.0)
            .when(keyboard_focus_visible(cx), |ring| {
                ring.border(px(metrics::FOCUS_RING_WIDTH))
                    .border_color(dark().text.secondary)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, Context, FocusHandle, Modifiers, Render};

    #[gpui::test]
    fn pointer_focus_stays_functional_without_ring_and_keyboard_restores_it(
        cx: &mut gpui::TestAppContext,
    ) {
        struct Host(FocusHandle);
        impl Render for Host {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .track_focus(&self.0)
                    .child(track_pointer_input())
                    .capture_key_down(key_down)
                    .on_key_down(|event, window, _| {
                        if event.keystroke.key == "tab" {
                            window.focus_next();
                        }
                    })
                    .child(
                        div()
                            .id("control")
                            .size(px(100.0))
                            .occlude()
                            .tab_stop(true)
                            .track_focus(&self.0)
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|host, _, window, cx| {
                                    window.focus(&host.0);
                                    cx.stop_propagation();
                                }),
                            ),
                    )
            }
        }
        let (host, cx) = cx.add_window_view(|_, cx| Host(cx.focus_handle()));
        cx.simulate_click(gpui::point(px(30.0), px(30.0)), Modifiers::none());
        cx.update(|window, cx| {
            assert!(host.read(cx).0.is_focused(window));
            assert!(!keyboard_focus_visible(cx));
        });
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| {
            assert!(host.read(cx).0.is_focused(window));
            assert!(keyboard_focus_visible(cx));
        });
        cx.simulate_click(gpui::point(px(30.0), px(30.0)), Modifiers::none());
        cx.update(|window, cx| {
            assert!(host.read(cx).0.is_focused(window));
            assert!(!keyboard_focus_visible(cx));
        });
    }
}
