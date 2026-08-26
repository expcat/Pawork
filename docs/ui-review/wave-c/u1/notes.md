# R1 Wave C / U1 — GPUI 0.2.2 TestAppContext 进程内驱动实测

日期：2026-08-26。对象：`gpui = 0.2.2` `TestAppContext` / `VisualTestContext`，挂在真实 desktop 组件上（`TextInput` + `Button` + overflow 滚动容器；不挂 `AppView` / Platform / socket）。

探针：`apps/desktop/src/ui/u1_probe.rs`（`#[cfg(test)]`，由 `ui/mod.rs` 接线）。验证：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`。

## 能力矩阵

| 能力 | 实测 | 测试函数 | 备注 |
| --- | --- | --- | --- |
| action 分发 | 通过 | `action_dispatch_pastes_into_text_input` | `VisualTestContext::dispatch_action(Paste)` 打到聚焦的 `TextInput` |
| focus 断言 | 通过 | `focus_assert_composer_after_explicit_focus` | `FocusHandle::is_focused`；未 focus 前为 false |
| keystrokes / input | 通过 | `keystrokes_type_and_edit_text_input` | `simulate_input("ab")` + `simulate_keystrokes("backspace")`；走 `EntityInputHandler`，非 IME composing |
| mouse click | 通过 | `mouse_click_focuses_text_input`、`mouse_click_fires_button_handler` | `simulate_click`；前者点 `TextInput` 拉焦点，后者点真实 `Button` 的 `on_click` |
| scroll | 通过 | `scroll_wheel_event_reaches_overflow_container` | 无 `simulate_scroll`；用 `simulate_event(ScrollWheelEvent)`。`FollowScroll` 本身是值对象，探针用 overflow + `ScrollHandle`（与 Inspector 终端滚动同 API） |
| resize | 通过 | `resize_updates_debug_bounds_geometry` | `simulate_resize` 后 `debug_bounds` 尺寸变化 |
| clipboard | 通过 | `clipboard_roundtrip_via_paste_action` | `write_to_clipboard` / `read_from_clipboard` 再 `Paste` 进 `TextInput` |
| 确定性 executor | 通过 | `deterministic_executor_advances_clock_without_real_sleep` | `run_until_parked` 抽干就绪任务；`advance_clock` 才让 `timer` 完成，无真实 sleep |
| layout invariant / debug_bounds | 通过 | `debug_bounds_reports_named_geometry` | `debug_selector` 仅在 `test-support`/`cfg(test)` 写入；生产 noop |
| IME composing | 不支持 | — | TestAppContext 无 composing 模拟；`TextInput::is_composing` 只能走真实 IME（U3） |
| AX / AccessKit | 不支持 | — | 0.2.2 无 AX 树；本层不宣称语义定位 |
| AppView / socket 组件 | 未硬接 | — | `AppView::new` 需要 Platform + socket；U1 用轻量真实组件代替 TaskRail/InputArea 全树 |

## 边界

- `debug_selector` 必须显式调用才会进入 `debug_bounds` map；element `id()` 不会自动登记。
- `simulate_event` 注释要求先 paint；`add_window_view` + `simulate_resize` + `refresh` 后 `debug_bounds` 才有值。
- 滚动没有专用 helper，必须构造 `ScrollWheelEvent`（position 落在 overflow hitbox 内）。
- 本 spike 证明进程内驱动可用，不替代 U2 真窗口 AX / U3 screenshot。
