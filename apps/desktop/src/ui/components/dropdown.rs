//! Dropdown 浮层菜单基础组件（R8 波 B 轨 2）。
//!
//! 菜单形态与关闭语义按 design/README.md §8.2：面板经 deferred(anchored())
//! 浮层化（不占布局流，开合不改变下层内容位置），occlude() 拦截下层点击与
//! 滚轮；关闭路径 = 选择选项 / 再点触发器 / Escape / 外点（on_mouse_down_out，
//! vendored gpui 0.2.2 已核实存在）。Escape 由宿主根节点 on_key_down 冒泡承接：
//! 面板经 deferred 绘制、自身不可聚焦，组件层 on_key_down 不可达，故不提供
//! Escape API。选项行 hover 取值按 §8.1。

use gpui::{
    anchored, deferred, div, point, prelude::*, px, AnchoredPositionMode, AnyElement, App,
    ClickEvent, Corner, IntoElement, MouseDownEvent, Pixels, Point, RenderOnce, SharedString,
    Styled, Window,
};

use crate::ui::theme::{dark, font, metrics};

/// 浮层与触发器的垂直间距。
pub const ANCHOR_GAP_Y: f32 = metrics::MENU_ANCHOR_GAP;
/// 面板最大高度：菜单项超出时面板自身滚动（§8.2）。
pub const MENU_MAX_HEIGHT: f32 = 240.0;

/// MenuRow 菜单选项行（§8.1：选中项用 check + label，不用整行亮蓝；
/// 禁用行 text.ghost 且无交互）。
#[derive(IntoElement)]
pub struct MenuRow {
    id: SharedString,
    label: SharedString,
    selected: bool,
    highlighted: bool,
    disabled: bool,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl MenuRow {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: SharedString::default(),
            selected: false,
            highlighted: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// 键盘高亮行（R3 Wave B 菜单 ↑/↓）：未选中行用 surface.raised（与
    /// hover 同 token）；选中行保持 accent.primary 不叠加（§8.1）。
    pub fn highlighted(mut self, highlighted: bool) -> Self {
        self.highlighted = highlighted;
        self
    }

    /// 禁用行（如不可用 Fork）：text.ghost、无 hover / 点击。
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
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

impl RenderOnce for MenuRow {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let marker = if self.selected { "✓" } else { "" };
        let mut row = div()
            .id(self.id)
            .flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap_2()
            .h(px(metrics::MENU_ROW_HEIGHT))
            .px_2()
            .rounded(px(metrics::CONTROL_RADIUS))
            .text_size(font::BASE)
            .child(
                div()
                    .w(px(metrics::SPACE_4))
                    .flex_none()
                    .text_color(dark().accent.primary)
                    .child(marker),
            );
        if self.disabled {
            return row
                .text_color(dark().text.ghost)
                .child(div().flex_1().min_w_0().truncate().child(self.label));
        }
        row = row
            .cursor_pointer()
            .bg(if self.selected || self.highlighted {
                dark().surface.raised
            } else {
                dark().bg.menu
            });
        row = row
            .hover(|style| style.bg(dark().surface.hover))
            .active(|style| style.bg(dark().surface.pressed));
        row = row.child(div().flex_1().min_w_0().truncate().child(self.label));
        if let Some(on_click) = self.on_click {
            row = row.on_click(on_click);
        }
        row
    }
}

/// MenuPanel 浮层菜单面板：220–360px / 8px padding / 6px radius / menu shadow，
/// occlude() 拦截下层点击与滚轮，外点关闭经回调交还宿主。
#[derive(IntoElement)]
pub struct MenuPanel {
    id: SharedString,
    children: Vec<AnyElement>,
    max_height: f32,
    on_outside_click: Option<Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>>,
}

impl MenuPanel {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            children: Vec::new(),
            max_height: MENU_MAX_HEIGHT,
            on_outside_click: None,
        }
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        for child in children {
            self.children.push(child.into_any_element());
        }
        self
    }

    /// 覆盖菜单默认最大高度。Activity 这类较高的浮层可按冻结视觉合同放宽，
    /// 普通下拉菜单仍保持 240px 并在面板内滚动。
    pub fn max_height(mut self, max_height: f32) -> Self {
        self.max_height = max_height;
        self
    }

    /// 外点关闭（on_mouse_down_out；面板存在期间挂窗口级捕获监听）。
    pub fn dismiss_on_outside(
        mut self,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_outside_click = Some(Box::new(listener));
        self
    }
}

impl RenderOnce for MenuPanel {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut panel = div()
            .id(self.id)
            .p(px(metrics::MENU_PADDING))
            .rounded(px(metrics::INPUT_MENU_RADIUS))
            .bg(dark().bg.menu)
            .border_1()
            .border_color(dark().border.strong)
            .min_w(px(metrics::MENU_MIN_WIDTH))
            .max_w(px(metrics::MENU_MAX_WIDTH))
            .max_h(px(self.max_height))
            .overflow_y_scroll()
            .shadow_md()
            // 拦截面板命中区内的下层点击与滚轮（§8.2 滚轮无穿透）。
            .occlude();
        if let Some(listener) = self.on_outside_click {
            panel = panel.on_mouse_down_out(listener);
        }
        for child in self.children {
            panel = panel.child(child);
        }
        panel
    }
}

/// Dropdown：触发器（复用 Button）+ 打开时的 deferred(anchored()) 浮层面板。
/// 包装层为 flex-col，浮层锚点即触发器正下方（绝对定位不占布局流）。
#[derive(IntoElement)]
pub struct Dropdown {
    trigger: AnyElement,
    panel: Option<AnyElement>,
    panel_anchor: Option<(Corner, Point<Pixels>)>,
}

impl Dropdown {
    pub fn new(trigger: impl IntoElement) -> Self {
        Self {
            trigger: trigger.into_any_element(),
            panel: None,
            panel_anchor: None,
        }
    }

    /// 打开时挂载浮层面板（关闭时不渲染）。
    pub fn panel(mut self, panel: impl IntoElement) -> Self {
        self.panel = Some(panel.into_any_element());
        self
    }

    /// 以触发器包装层的局部坐标明确锚定面板。默认仍是触发器左下方；
    /// Workspace Header 的 Activity 用右下角坐标保证浮层与触发器右边缘对齐。
    pub fn panel_anchor(mut self, corner: Corner, position: Point<Pixels>) -> Self {
        self.panel_anchor = Some((corner, position));
        self
    }
}

impl RenderOnce for Dropdown {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let panel_anchor = self.panel_anchor;
        div()
            .flex()
            .flex_col()
            .child(self.trigger)
            .when_some(self.panel, |dropdown, panel| {
                let mut anchor = anchored()
                    .anchor(Corner::TopLeft)
                    .offset(point(px(metrics::ZERO), px(ANCHOR_GAP_Y)));
                if let Some((corner, position)) = panel_anchor {
                    anchor = anchor
                        .anchor(corner)
                        .position_mode(AnchoredPositionMode::Local)
                        .position(position);
                }
                dropdown.child(deferred(anchor.child(panel)))
            })
    }
}
