//! GUI3-08：Changes / Resources 加载骨架。静态条，不循环闪动。

use gpui::{div, prelude::*, px, App, IntoElement, RenderOnce, SharedString, Window};

use crate::ui::theme::{metrics, theme};

const BAR_WIDTHS: [f32; 4] = [220.0, 168.0, 196.0, 132.0];

/// 左对齐骨架行；调用方保留原有 loading 标题，骨架只替代说明段落的空场。
pub fn loading_skeleton(id: impl Into<SharedString>) -> LoadingSkeleton {
    LoadingSkeleton { id: id.into() }
}

#[derive(IntoElement)]
pub struct LoadingSkeleton {
    id: SharedString,
}

impl RenderOnce for LoadingSkeleton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let bar_color = theme(cx).surface.hover;
        #[cfg(test)]
        let selector_id = self.id.to_string();
        let mut block = div()
            .id(self.id)
            .flex()
            .flex_col()
            .gap(px(metrics::SPACE_2))
            .w_full()
            .items_start();
        for (index, width) in BAR_WIDTHS.into_iter().enumerate() {
            let row = div()
                .id(SharedString::from(format!("skeleton-bar-{index}")))
                .h(px(metrics::SKELETON_BAR_HEIGHT))
                .w(px(width))
                .rounded(px(metrics::CONTROL_RADIUS))
                .bg(bar_color);
            // debug_bounds 只认 selector，不认 element id。生产构建不加。
            #[cfg(test)]
            let row = row.debug_selector({
                let selector = format!("{selector_id}/skeleton-bar-{index}");
                move || selector
            });
            block = block.child(row);
        }
        block
    }
}
