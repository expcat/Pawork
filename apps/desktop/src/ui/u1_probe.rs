//! R1 Wave C U1 spike：在真实 desktop 组件上实测 GPUI 0.2.2 TestAppContext。
//!
//! 进程内驱动能力（action / focus / key / mouse / scroll / resize / clipboard /
//! 确定性 executor / debug_bounds）写在本模块的 `#[gpui::test]` 里。
//! 不挂 AppView / Platform / socket；IME composing 与 AX 不在本层覆盖。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use gpui::{
    div, point, prelude::*, px, size, ClipboardItem, Context, Entity, EntityInputHandler,
    FocusHandle, Focusable, InteractiveElement, Modifiers, ParentElement, Render, ScrollDelta,
    ScrollHandle, ScrollWheelEvent, StatefulInteractiveElement, Styled, TestAppContext,
    VisualTestContext, Window,
};

use super::components::button::{Button, ButtonVariant};
use super::text_input::{self, TextInput};

const WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(640.), px(360.));

struct ProbeHost {
    input: Entity<TextInput>,
    clicked: bool,
    scrolled: bool,
    scroll: ScrollHandle,
    button_focus: FocusHandle,
}

impl ProbeHost {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            input: cx.new(TextInput::new),
            clicked: false,
            scrolled: false,
            scroll: ScrollHandle::new(),
            button_focus: cx.focus_handle(),
        }
    }
}

impl Render for ProbeHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let button_focus = self.button_focus.clone();
        div()
            .id("u1-probe-root")
            .debug_selector(|| "u1-probe-root".into())
            .size_full()
            .flex()
            .flex_col()
            .child(self.input.clone())
            .child(
                div()
                    .id("u1-probe-button")
                    .debug_selector(|| "u1-probe-button".into())
                    .child(
                        Button::new("u1-probe-click")
                            .variant(ButtonVariant::Raised)
                            .label("U1 Click")
                            .track_focus(&button_focus)
                            .on_click(cx.listener(|host, _event, _window, _cx| {
                                host.clicked = true;
                            })),
                    ),
            )
            .child(
                div()
                    .id("u1-probe-scroll")
                    .debug_selector(|| "u1-probe-scroll".into())
                    .h(px(80.))
                    .w_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .on_scroll_wheel(cx.listener(|host, _event, _window, _cx| {
                        host.scrolled = true;
                    }))
                    .child(div().h(px(400.)).w_full()),
            )
    }
}

impl Focusable for ProbeHost {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

fn mount_probe(cx: &mut TestAppContext) -> (Entity<ProbeHost>, &mut VisualTestContext) {
    cx.update(|cx| {
        super::install_keybindings(cx);
    });
    let (host, cx) = cx.add_window_view(|_window, cx| ProbeHost::new(cx));
    cx.simulate_resize(WINDOW_SIZE);
    cx.refresh().expect("refresh after mount");
    cx.run_until_parked();
    (host, cx)
}

fn focus_composer(host: &Entity<ProbeHost>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let handle = host.read(cx).input.read(cx).focus_handle(cx);
        window.focus(&handle);
        window.activate_window();
    });
    cx.refresh().expect("refresh after focus");
    cx.run_until_parked();
}

fn composer_text(host: &Entity<ProbeHost>, cx: &mut VisualTestContext) -> String {
    host.read_with(cx, |host, cx| host.input.read(cx).text().to_string())
}

fn composer_focused(host: &Entity<ProbeHost>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| {
        host.read(cx)
            .input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    })
}

#[gpui::test]
fn action_dispatch_pastes_into_text_input(cx: &mut TestAppContext) {
    let (host, cx) = mount_probe(cx);
    focus_composer(&host, cx);
    cx.write_to_clipboard(ClipboardItem::new_string("from-action".into()));
    cx.dispatch_action(text_input::Paste);
    cx.run_until_parked();
    assert_eq!(composer_text(&host, cx), "from-action");
}

#[gpui::test]
fn focus_assert_composer_after_explicit_focus(cx: &mut TestAppContext) {
    let (host, cx) = mount_probe(cx);
    assert!(
        !composer_focused(&host, cx),
        "composer starts unfocused until the probe focuses it"
    );
    focus_composer(&host, cx);
    assert!(composer_focused(&host, cx));
}

#[gpui::test]
fn keystrokes_type_and_edit_text_input(cx: &mut TestAppContext) {
    let (host, cx) = mount_probe(cx);
    focus_composer(&host, cx);
    cx.simulate_input("ab");
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    assert_eq!(composer_text(&host, cx), "a");
}

#[gpui::test]
fn shift_enter_inserts_newline_via_keymap(cx: &mut TestAppContext) {
    let (host, cx) = mount_probe(cx);
    focus_composer(&host, cx);
    cx.simulate_input("ab");
    cx.simulate_keystrokes("shift-enter");
    cx.run_until_parked();
    assert_eq!(composer_text(&host, cx), "ab\n");
}

#[gpui::test]
fn mouse_click_focuses_text_input(cx: &mut TestAppContext) {
    let (host, cx) = mount_probe(cx);
    let bounds = cx
        .debug_bounds("u1-probe-root")
        .expect("root bounds after first paint");
    let click = point(bounds.origin.x + px(24.), bounds.origin.y + px(12.));
    cx.simulate_click(click, Modifiers::none());
    cx.run_until_parked();
    assert!(
        composer_focused(&host, cx),
        "TextInput on_mouse_down must pull focus back"
    );
}

#[gpui::test]
fn mouse_click_fires_button_handler(cx: &mut TestAppContext) {
    let (host, cx) = mount_probe(cx);
    let bounds = cx
        .debug_bounds("u1-probe-button")
        .expect("button wrapper bounds");
    let click = bounds.center();
    cx.simulate_click(click, Modifiers::none());
    cx.run_until_parked();
    let clicked = host.read_with(cx, |host, _| host.clicked);
    assert!(clicked, "Button on_click must fire from simulate_click");
}

#[gpui::test]
fn scroll_wheel_event_reaches_overflow_container(cx: &mut TestAppContext) {
    let (host, cx) = mount_probe(cx);
    let bounds = cx
        .debug_bounds("u1-probe-scroll")
        .expect("scroll container bounds");
    let before = host.read_with(cx, |host, _| host.scroll.offset());
    cx.simulate_event(ScrollWheelEvent {
        position: point(bounds.origin.x + px(8.), bounds.origin.y + px(8.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
        ..Default::default()
    });
    cx.run_until_parked();
    let (scrolled, after) = host.read_with(cx, |host, _| (host.scrolled, host.scroll.offset()));
    assert!(scrolled, "ScrollWheelEvent must hit on_scroll_wheel");
    assert_ne!(before, after, "overflow_y_scroll offset must change");
}

#[gpui::test]
fn resize_updates_debug_bounds_geometry(cx: &mut TestAppContext) {
    let (_host, cx) = mount_probe(cx);
    let before = cx
        .debug_bounds("u1-probe-root")
        .expect("bounds before resize");
    cx.simulate_resize(size(px(480.), px(240.)));
    cx.refresh().expect("refresh after resize");
    cx.run_until_parked();
    let after = cx
        .debug_bounds("u1-probe-root")
        .expect("bounds after resize");
    assert_ne!(before.size, after.size);
    assert!(after.size.width > px(0.) && after.size.height > px(0.));
}

#[gpui::test]
fn clipboard_roundtrip_via_paste_action(cx: &mut TestAppContext) {
    let (host, cx) = mount_probe(cx);
    focus_composer(&host, cx);
    cx.write_to_clipboard(ClipboardItem::new_string("clip-probe".into()));
    let stored = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("test clipboard stores the written item");
    assert_eq!(stored, "clip-probe");
    cx.dispatch_action(text_input::Paste);
    cx.run_until_parked();
    assert_eq!(composer_text(&host, cx), "clip-probe");
}

#[gpui::test]
fn deterministic_executor_advances_clock_without_real_sleep(cx: &mut TestAppContext) {
    let (_host, cx) = mount_probe(cx);
    let started = cx.executor().now();
    let fired = Arc::new(AtomicBool::new(false));
    let flag = fired.clone();
    cx.executor()
        .spawn(async move {
            flag.store(true, Ordering::SeqCst);
        })
        .detach();
    cx.run_until_parked();
    assert!(
        fired.load(Ordering::SeqCst),
        "run_until_parked must drain ready background tasks"
    );

    let delayed = Arc::new(AtomicBool::new(false));
    let delayed_flag = delayed.clone();
    let executor = cx.executor();
    executor
        .spawn({
            let executor = executor.clone();
            async move {
                executor.timer(Duration::from_millis(250)).await;
                delayed_flag.store(true, Ordering::SeqCst);
            }
        })
        .detach();
    cx.run_until_parked();
    assert!(
        !delayed.load(Ordering::SeqCst),
        "timer must stay pending until advance_clock"
    );
    cx.executor().advance_clock(Duration::from_millis(250));
    cx.run_until_parked();
    assert!(
        delayed.load(Ordering::SeqCst),
        "advance_clock + run_until_parked must complete the timer"
    );
    let elapsed = cx.executor().now().saturating_duration_since(started);
    assert!(
        elapsed >= Duration::from_millis(250),
        "advance_clock must move TestDispatcher time, elapsed={elapsed:?}"
    );
}

#[gpui::test]
fn debug_bounds_reports_named_geometry(cx: &mut TestAppContext) {
    let (_host, cx) = mount_probe(cx);
    let root = cx
        .debug_bounds("u1-probe-root")
        .expect("debug_selector u1-probe-root");
    let scroll = cx
        .debug_bounds("u1-probe-scroll")
        .expect("debug_selector u1-probe-scroll");
    assert!(root.size.width > px(0.) && root.size.height > px(0.));
    assert!(scroll.size.height > px(0.));
    assert!(scroll.origin.y >= root.origin.y);
}

#[gpui::test]
fn shift_select_copy_cut_and_undo_via_actions(cx: &mut TestAppContext) {
    let (host, cx) = mount_probe(cx);
    focus_composer(&host, cx);
    host.update(cx, |host, cx| {
        host.input.update(cx, |input, cx| input.reset_text("abcd", cx));
    });
    cx.dispatch_action(text_input::SelectAll);
    cx.run_until_parked();
    let selected = host.read_with(cx, |host, cx| {
        let input = host.input.read(cx);
        input.selected_range()
    });
    assert_eq!(selected, 0.."abcd".len());
    cx.dispatch_action(text_input::Copy);
    cx.run_until_parked();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("copy writes clipboard");
    assert_eq!(copied, "abcd");
    cx.dispatch_action(text_input::Cut);
    cx.run_until_parked();
    assert_eq!(composer_text(&host, cx), "");
    cx.dispatch_action(text_input::Undo);
    cx.run_until_parked();
    assert_eq!(composer_text(&host, cx), "abcd");
    cx.dispatch_action(text_input::Redo);
    cx.run_until_parked();
    assert_eq!(composer_text(&host, cx), "");
}

#[gpui::test]
fn ime_commit_undo_stacks_once_and_empty_text_is_not_sendable(cx: &mut TestAppContext) {
    // 平台 IME 真实路径（EntityInputHandler）：marked 中间态不入 undo 栈，
    // commit 恰好单次入栈；空白文本不可发送（trim 语义）。
    let (host, cx) = mount_probe(cx);
    focus_composer(&host, cx);
    let input = host.read_with(cx, |host, _| host.input.clone());
    input.update(cx, |input, cx| input.reset_text("", cx));
    let before = input.read_with(cx, |i, _| i.undo_len());
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "ni", None, window, cx);
        });
    });
    let (composing, mid) = input.read_with(cx, |i, _| (i.is_composing(), i.undo_len()));
    assert!(composing);
    assert_eq!(mid, before, "拼音中间态不得入 undo 栈");
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_text_in_range(None, "你", window, cx);
        });
    });
    cx.run_until_parked();
    let stacked = input.read_with(cx, |i, _| i.undo_len());
    assert_eq!(stacked, before + 1, "commit 必须恰好单次入栈");
    assert!(!composer_text(&host, cx).trim().is_empty());
    input.update(cx, |input, cx| input.reset_text("   ", cx));
    assert!(composer_text(&host, cx).trim().is_empty());
}

#[gpui::test]
fn wave_b_keymap_bindings_dispatch_through_keystrokes(cx: &mut TestAppContext) {
    // R5 Wave B 键位表的真实链路：keystroke → keymap → action → handler。
    let (host, cx) = mount_probe(cx);
    focus_composer(&host, cx);
    host.update(cx, |host, cx| {
        host.input.update(cx, |input, cx| input.reset_text("abcd", cx));
    });
    cx.simulate_keystrokes("cmd-a");
    cx.run_until_parked();
    let selected = host.read_with(cx, |host, cx| host.input.read(cx).selected_range());
    assert_eq!(selected, 0..4);
    cx.simulate_keystrokes("cmd-c");
    cx.run_until_parked();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("cmd-c writes clipboard");
    assert_eq!(copied, "abcd");
    cx.simulate_keystrokes("cmd-x");
    cx.run_until_parked();
    assert_eq!(composer_text(&host, cx), "");
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(composer_text(&host, cx), "abcd");
    cx.simulate_keystrokes("cmd-shift-z");
    cx.run_until_parked();
    assert_eq!(composer_text(&host, cx), "");
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-left");
    cx.run_until_parked();
    let selected = host.read_with(cx, |host, cx| host.input.read(cx).selected_range());
    assert_eq!(selected, 3..4);
    cx.simulate_keystrokes("shift-home");
    cx.run_until_parked();
    let selected = host.read_with(cx, |host, cx| host.input.read(cx).selected_range());
    assert_eq!(selected, 0..4);
}
