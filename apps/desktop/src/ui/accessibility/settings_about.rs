//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use super::{AxNode, AxRect, AxRole};
use crate::ui::AppView;

impl AppView {
    /// 「关于」页 AX（SET-6g）：三项只读事实；`host_data_dir` 非空 gate；
    /// 没有动作节点，也不保留断线前路径。
    pub(crate) fn settings_about_page_ax(&self, frame: AxRect) -> AxNode {
        const HEADING_HEIGHT: f32 = 28.0;
        const SUBTITLE_HEIGHT: f32 = 20.0;
        const ROW_HEIGHT: f32 = 40.0;
        let width = (frame.width - 32.0).max(0.0);
        let mut y = frame.y + 16.0 + HEADING_HEIGHT + SUBTITLE_HEIGHT + 8.0;
        let mut page = AxNode::new("settings-page", AxRole::Group, "关于", frame).child(
            AxNode::new(
                "settings-page-title",
                AxRole::StaticText,
                "关于",
                AxRect::new(
                    frame.x + 16.0,
                    frame.y + 16.0,
                    width,
                    HEADING_HEIGHT + SUBTITLE_HEIGHT,
                ),
            )
            .value("Build and current Host connection information"),
        );
        for (id, label, value) in self.settings_about_rows().unwrap_or_default() {
            page = page.child(
                AxNode::new(
                    id,
                    AxRole::StaticText,
                    label,
                    AxRect::new(frame.x + 16.0, y, width, ROW_HEIGHT),
                )
                .value(value),
            );
            y += ROW_HEIGHT;
        }
        page
    }
}
