//! StatusBar 底部状态行容器（R8 波 B 轨 1）。

use gpui::{
    div, prelude::*, px, AnyElement, App, Context, IntoElement, RenderOnce, Styled, Window,
};

use crate::ui::theme::{font, metrics, theme};

impl crate::ui::AppView {
    /// 复制 / 字号等普通反馈共用可替换的 3s 计时任务；不写 status_hint，
    /// 持久错误始终优先，旧任务也不能清除新反馈。
    pub(in crate::ui) fn show_status_feedback(&mut self, text: String, cx: &mut Context<Self>) {
        self.status_feedback = Some(text);
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_secs(3));
        self.status_feedback_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |view, cx| {
                view.status_feedback = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

/// 底部 30px 状态行：bg.panel + 顶描边 + 12/18 辅助文字（GUI4 三栏）。
///
/// 左：项目 / 分支；中：Run 用量（AX 仍只发这一串）；右：连接 / 瞬态反馈。
/// 左右栏不发布 AX，避免与 TaskRail / Composer 同源节点重复。R6 Wave A
/// 起不再承载 Inspector/Activity 动作。
#[derive(IntoElement)]
pub struct StatusBar {
    leading: Option<AnyElement>,
    centered: Option<AnyElement>,
    trailing: Option<AnyElement>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            leading: None,
            centered: None,
            trailing: None,
        }
    }

    pub fn leading(mut self, child: impl IntoElement) -> Self {
        self.leading = Some(child.into_any_element());
        self
    }

    /// 用量占据自身宽度，左右栏等分剩余空间，保持居中且互不覆盖。
    pub fn centered(mut self, child: impl IntoElement) -> Self {
        self.centered = Some(child.into_any_element());
        self
    }

    pub fn trailing(mut self, child: impl IntoElement) -> Self {
        self.trailing = Some(child.into_any_element());
        self
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for StatusBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .id("shell-status-bar")
            .debug_selector(|| "shell-status-bar".into())
            .h(px(metrics::STATUS_BAR_HEIGHT))
            .px_3()
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(theme.border.subtle)
            .bg(theme.bg.panel)
            .text_size(font::META)
            .line_height(font::META_LINE_HEIGHT)
            .text_color(theme.text.secondary)
            .child(status_slot(self.leading, true))
            .when_some(self.centered, |bar, centered| {
                bar.child(
                    div()
                        .debug_selector(|| "status-bar-center-slot".into())
                        .flex_none()
                        .max_w_full()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .justify_center()
                        .overflow_hidden()
                        .px(px(metrics::STATUS_BAR_CENTER_INSET))
                        .child(centered),
                )
            })
            .child(status_slot(self.trailing, false))
    }
}

fn status_slot(child: Option<AnyElement>, leading: bool) -> gpui::Div {
    div()
        .debug_selector(move || {
            if leading {
                "status-bar-leading-slot".into()
            } else {
                "status-bar-trailing-slot".into()
            }
        })
        .relative()
        .occlude()
        .flex()
        .min_w_0()
        .flex_1()
        .items_center()
        .overflow_hidden()
        .when(leading, |slot| slot.justify_start())
        .when(!leading, |slot| slot.justify_end())
        .children(child)
}
