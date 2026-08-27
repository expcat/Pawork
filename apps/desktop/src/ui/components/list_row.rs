//! ListRow 列表行基础组件（R8 波 B 轨 1）。
//!
//! Task 行 / 项目头行的可点区域统一：选中态背景取现状值（active=raised /
//! 否则 bg.panel / 头行无底色），hover 按基准 §8.1（ghost 行 → surface.raised，
//! raised 行 → surface.hover），active 复用 hover 色。

use gpui::{
    AnyElement, App, ClickEvent, FocusHandle, IntoElement, KeyDownEvent, RenderOnce, SharedString,
    Styled, Window, div, prelude::*, px,
};

use crate::ui::theme::{dark, metrics};

/// 列表行形态。
#[derive(Debug, Clone, Copy)]
pub enum ListRowKind {
    /// Task 行：px-2 / py-1 / rounded-sm；选中 = surface.raised，否则 bg.panel。
    Task { selected: bool },
    /// 项目头行（可点标题区）：flex-1 行内布局，无底色。
    ProjectHeader,
}

/// 可点列表行。
#[derive(IntoElement)]
pub struct ListRow {
    id: SharedString,
    kind: ListRowKind,
    focus: Option<FocusHandle>,
    children: Vec<AnyElement>,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
    on_activate: Option<Box<dyn Fn(&KeyDownEvent, &mut Window, &mut App) + 'static>>,
}

impl ListRow {
    /// Task 行。
    pub fn task(id: impl Into<SharedString>, selected: bool) -> Self {
        Self::new(id, ListRowKind::Task { selected })
    }

    /// 项目头行（可点标题区）。
    pub fn project_header(id: impl Into<SharedString>) -> Self {
        Self::new(id, ListRowKind::ProjectHeader)
    }

    fn new(id: impl Into<SharedString>, kind: ListRowKind) -> Self {
        Self {
            id: id.into(),
            kind,
            focus: None,
            children: Vec::new(),
            on_click: None,
            on_activate: None,
        }
    }

    /// 键盘焦点三件套（R3 Wave B rail 导航）：tab_stop + track_focus +
    /// 聚焦描边（tab_index 档位由调用方的 focus handle 携带）。
    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// 键盘激活（R3 Wave B Slice 4）：裸 Enter / Space 在行级 key_down
    /// 直接调用激活 handler——GPUI 对聚焦行的合成 keyboard click 挂在
    /// keyup 上，真窗口注入取证不可达（enter-gap.json enter_gap=1），
    /// 不以合成 click 兜底。handler 与 on_click 走同一激活路径。是否
    /// stop_propagation 由调用方 handler 决定（Slice 5：菜单打开时调用方
    /// 让位不 stop，让根节点菜单 Enter / ↑↓ 接管；处理激活时调用方自己
    /// cx.stop_propagation()）。
    pub fn on_activate(
        mut self,
        handler: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for ListRow {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut row = div().id(self.id).cursor_pointer();
        if let Some(focus) = self.focus.as_ref() {
            row = row
                .tab_stop(true)
                .track_focus(focus)
                .focus(|style| style.border_1().border_color(dark().accent.primary));
        }
        let hover = match self.kind {
            ListRowKind::Task { selected } => {
                // flex_row + min_w_0：让子项 flex_1/truncate 拿到 Definite 宽度
                // （R8 波 C 长标题截断依赖此约束）。R3 Wave A：行高 44（量图
                // 43–44 取 44）+ 内容垂直居中；选中面 rounded_sm(4) 不变。
                row = row
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_w_0()
                    .h(px(metrics::RAIL_TASK_ROW_HEIGHT))
                    .px_2()
                    .rounded_sm();
                if selected {
                    row = row.bg(dark().surface.raised);
                    dark().surface.hover
                } else {
                    row = row.bg(dark().bg.panel);
                    dark().surface.raised
                }
            }
            ListRowKind::ProjectHeader => {
                // R3 Wave A：项目头行高对齐任务行（44），chevron + 名称垂直居中。
                row = row
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .h(px(metrics::RAIL_TASK_ROW_HEIGHT))
                    .rounded_sm();
                dark().surface.raised
            }
        };
        row = row.hover(move |style| style.bg(hover));
        row = row.active(move |style| style.bg(hover));
        for child in self.children {
            row = row.child(child);
        }
        if let Some(on_click) = self.on_click {
            row = row.on_click(on_click);
        }
        if let Some(on_activate) = self.on_activate {
            row = row.on_key_down(move |event: &KeyDownEvent, window, cx| {
                // 仅裸 Enter / Space 激活；带修饰键（cmd-enter 审批、
                // shift-enter 换行语义）不接管。
                if !event.keystroke.modifiers.modified()
                    && (event.keystroke.key == "enter" || event.keystroke.key == "space")
                {
                    on_activate(event, window, cx);
                }
            });
        }
        row
    }
}
