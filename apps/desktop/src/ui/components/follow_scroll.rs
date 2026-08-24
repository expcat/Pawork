//! FollowScroll 跟随滚动与回底控件（R8 波 B 轨 2）。
//!
//! 封装原 mod.rs 内联的 follow bool + 贴底判定（design/README.md §8.3）：
//! 用户上滚脱钩自动跟随，滚回底部自动重挂；脱钩时滚动区右下浮出
//! 「↓ 回到底部」控件，点击滚底并重挂，跟随态隐藏。

use gpui::{div, prelude::*, px, AnyElement, App, ScrollHandle, Window};

use crate::ui::theme::metrics;

/// 跟随滚动状态（值对象，由宿主 View 持有）。
pub struct FollowScroll {
    handle: ScrollHandle,
    following: bool,
}

impl FollowScroll {
    pub fn new() -> Self {
        Self {
            handle: ScrollHandle::new(),
            following: true,
        }
    }

    /// 绑定到滚动容器的 track_scroll。
    pub fn handle(&self) -> &ScrollHandle {
        &self.handle
    }

    /// 是否跟随（脱钩后为 false）。
    pub fn is_following(&self) -> bool {
        self.following
    }

    /// 贴底判定（原 mod.rs 自由函数逻辑逐值迁移）。
    pub fn is_scrolled_to_bottom(&self) -> bool {
        let max = self.handle.max_offset().height;
        let y = self.handle.offset().y;
        max <= px(metrics::SCROLL_EPSILON) || y <= px(metrics::SCROLL_BOTTOM_SLACK) - max
    }

    /// 用户滚动后重估挂钩（§8.3：滚回底部自动重挂，上滚脱钩）。
    ///
    /// gpui 0.2.2 滚轮分发：内部偏移应用与本监听同在 Bubble 相，监听按注册
    /// 逆序分发，内部应用注册在后故先执行——本监听读到的是已应用（未钳制）
    /// 的 offset（vendored 核实：div.rs paint_mouse_listeners 2061 先于
    /// paint_scroll_listener 2417 注册，window.rs 3705 rev 逆序），直接按
    /// 贴底同式判定，不做 delta 投影（投影会把 delta 计两次）。
    pub fn on_scroll_wheel(&mut self) {
        self.following = self.is_scrolled_to_bottom();
    }

    /// 新内容到达：未贴底则脱钩（调用时机与迁移前一致——apply 之前）。
    pub fn content_arriving(&mut self) {
        if !self.is_scrolled_to_bottom() {
            self.following = false;
        }
    }

    /// 仍在跟随时随新内容滚到底。
    pub fn follow_new_content(&mut self) {
        if self.following {
            self.handle.scroll_to_bottom();
        }
    }

    /// 滚到底并重新挂接（回底控件点击 / 终端跟随重置）。
    pub fn jump_to_bottom(&mut self) {
        self.handle.scroll_to_bottom();
        self.following = true;
    }

    /// 换新滚动容器并回到跟随态（打开 / 切换 session 的 Timeline）。
    pub fn reset(&mut self) {
        self.handle = ScrollHandle::new();
        self.following = true;
    }
}

/// 回底控件容器：绝对定位在滚动区右下（bottom-2 / right-2，§8.3），
/// 控件本体（surface.raised 底 / text.primary 字 / rounded_md / hover）由调用方
/// 以 Button 传入。
#[derive(IntoElement)]
pub struct BackToBottom {
    child: AnyElement,
}

impl BackToBottom {
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
        }
    }
}

impl RenderOnce for BackToBottom {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().absolute().bottom_2().right_2().child(self.child)
    }
}
