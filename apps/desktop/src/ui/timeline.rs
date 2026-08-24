//! Timeline 容器：gpui list() 变高虚拟化（R8 波 C）。
//!
//! 跟随语义自 FollowScroll 重映射为 ListState 驱动：Bottom 对齐下
//! logical_scroll_top == None 即钉底（跟随态），scroll handler 的
//! is_scrolled 翻真表示用户脱钩；回底 = scroll_to 末项底，布局时自动
//! 重挂。projection 有条目替换语义，timeline 任何变化统一 reset(new_count)
//! （splice 不安全）；脱钩读史时恢复 reset 前偏移，视口不跳。

use gpui::{div, list, prelude::*, Context, ListOffset, ListState, Pixels, WeakEntity, Window};

use crate::projection::ConnectionState;
use crate::ui::components::button::{Button, ButtonVariant};
use crate::ui::components::follow_scroll::BackToBottom;

use super::{AppView, MenuKind};

/// list() 视口外上下方向的预渲染量（px，非视觉尺寸；仅影响滚动顺滑度）。
pub(super) const TIMELINE_OVERDRAW: f32 = 200.0;

/// 挂接 list 滚动跟随（AppView::new 时设置一次；WeakEntity 不构成引用环）。
pub(super) fn install_scroll_follow(state: &ListState, view: &WeakEntity<AppView>) {
    state.set_scroll_handler({
        let view = view.clone();
        move |event, _window, cx| {
            // Bottom 对齐下贴底钉住时 logical_scroll_top == None → is_scrolled=false。
            let following = !event.is_scrolled;
            view.update(cx, |view, cx| {
                if view.timeline_following != following {
                    view.timeline_following = following;
                    cx.notify();
                }
            })
            .ok();
        }
    });
}

/// timeline 数据 / 宽度变化 → reset 计数并恢复脱钩视口（render 前调用）。
fn sync_list(view: &mut AppView) {
    let count =
        view.projection.timeline.len() + usize::from(view.projection.pending_approval.is_some());
    if view.timeline_list_rev == view.timeline_rev && view.timeline_list_count == count {
        return;
    }
    // 条目「···」菜单浮层锚在条目内：reset 使高度缓存失效、条目可能被虚拟化
    // 卸载，开着则先关（close on reset）。
    if matches!(view.open_menu, Some(MenuKind::Entry(_))) {
        view.open_menu = None;
    }
    let previous = view.timeline_list.logical_scroll_top();
    view.timeline_list.reset(count);
    if !view.timeline_following {
        // 脱钩读史：恢复 reset 前偏移（item_ix 越界钳制到新末项）。
        view.timeline_list.scroll_to(ListOffset {
            item_ix: previous.item_ix.min(count),
            offset_in_item: previous.offset_in_item,
        });
    }
    view.timeline_list_rev = view.timeline_rev;
    view.timeline_list_count = count;
}

impl AppView {
    pub(super) fn timeline_area(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        sync_list(self);
        let fork_available = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_session_id.is_some();
        // 条目交互经 cx.processor 捕获实体：list() 的 render_item 只收 index，
        // 无 ViewContext，Entity::update 模式在条内重建 listener。
        let entries = list(
            self.timeline_list.clone(),
            cx.processor(
                move |view: &mut AppView,
                      ix: usize,
                      _window: &mut Window,
                      cx: &mut Context<AppView>| {
                    let len = view.projection.timeline.len();
                    if ix < len {
                        let menu_open = matches!(
                            &view.open_menu,
                            Some(MenuKind::Entry(open_id))
                                if open_id == &view.projection.timeline[ix].event_id
                        );
                        let can_fork =
                            fork_available && view.projection.timeline[ix].is_fork_boundary();
                        let element = view.timeline_entry_element(
                            &view.projection.timeline[ix],
                            menu_open,
                            can_fork,
                            cx,
                        );
                        if ix > 0 {
                            div().pt_1().child(element).into_any_element()
                        } else {
                            element.into_any_element()
                        }
                    } else {
                        let card = view.approval_card_element(cx);
                        if len > 0 {
                            div().pt_1().child(card).into_any_element()
                        } else {
                            card.into_any_element()
                        }
                    }
                },
            ),
        )
        .flex_1()
        .px_3()
        .py_2();
        // 脱钩时右下浮出回底控件（§8.3）；跟随态隐藏。
        let following = self.timeline_following;
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .child(entries)
            .when(!following, |area| {
                area.child(BackToBottom::new(
                    Button::new("timeline-back-to-bottom")
                        .variant(ButtonVariant::Raised)
                        .label("↓ 回到底部")
                        .on_click(cx.listener(|view, _event, _window, cx| {
                            view.timeline_jump_to_bottom();
                            cx.notify();
                        })),
                ))
            })
    }

    /// 回底并重挂跟随：scroll_to 越界钳制到末项底，Bottom 对齐布局时
    /// 视口填不满自动置回钉底态（logical_scroll_top = None）。
    pub(super) fn timeline_jump_to_bottom(&mut self) {
        self.timeline_list.scroll_to(ListOffset {
            item_ix: self.timeline_list.item_count(),
            offset_in_item: Pixels::ZERO,
        });
        self.timeline_following = true;
    }
}
