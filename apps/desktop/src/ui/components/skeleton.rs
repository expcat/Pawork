//! GUI3-08：Changes / Resources 加载骨架。静态条，不循环闪动。

use gpui::{div, prelude::*, px, IntoElement, SharedString};

use crate::ui::theme::{dark, metrics};

const BAR_WIDTHS: [f32; 4] = [220.0, 168.0, 196.0, 132.0];

/// 左对齐骨架行；调用方保留原有 loading 标题，骨架只替代说明段落的空场。
pub fn loading_skeleton(id: impl Into<SharedString>) -> impl IntoElement {
    let mut block = div()
        .id(id.into())
        .flex()
        .flex_col()
        .gap(px(metrics::SPACE_2))
        .w_full()
        .items_start();
    for (index, width) in BAR_WIDTHS.into_iter().enumerate() {
        block = block.child(
            div()
                .id(SharedString::from(format!("skeleton-bar-{index}")))
                .h(px(metrics::SKELETON_BAR_HEIGHT))
                .w(px(width))
                .rounded(px(metrics::CONTROL_RADIUS))
                .bg(dark().surface.hover),
        );
    }
    block
}
