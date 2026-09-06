//! 聚焦描边覆盖层（零布局参与）。
//!
//! GPUI/taffy 的 border 参与盒模型计算：在 `.focus(...)` 样式里给控件加
//! 边框，聚焦瞬间弹性尺寸控件会整体膨胀、固定尺寸控件会压缩内容盒，两者
//! 都表现为布局位移（OPT-4d 设置导航微移的根因）。聚焦描边统一走本覆盖
//! 层：`absolute + inset_0` 贴住控件 padding box 内缘，纯绘制——无
//! hitbox（不入命中测试）、不 track_focus（不进 tab 序与派发树）、不参与
//! 布局；render 期按 `focus.is_focused(window)` 挂载 / 卸载，控件尺寸与
//! 内容坐标全程不变。
//!
//! 约定：任何可聚焦控件需要焦点描边时把控件设为 `relative()` 并挂载本
//! 层；禁止在 `.focus(...)` / `.hover(...)` 等动态样式里设置 border /
//! margin / padding 等参与布局的属性。

use gpui::{AbsoluteLength, Div, Styled, div, px};

use crate::ui::theme::{dark, metrics};

/// 贴住控件内缘的聚焦描边层（`FOCUS_RING_WIDTH` accent.primary，圆角随
/// 控件外壳）。仅应在控件持焦时挂载。
pub fn focus_ring(radius: impl Into<AbsoluteLength> + Clone) -> Div {
    div()
        .absolute()
        .inset_0()
        .rounded(radius)
        .border(px(metrics::FOCUS_RING_WIDTH))
        .border_color(dark().accent.primary)
}
