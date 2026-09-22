//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use super::{AxNode, AxRect, AxRole};
use crate::ui::i18n::t;
use crate::ui::AppView;

impl AppView {
    /// 「关于」页 AX（SET-6g）：三项只读事实；`host_data_dir` 非空 gate；
    /// 没有动作节点，也不保留断线前路径。
    pub(crate) fn settings_about_page_ax(&self, frame: AxRect) -> AxNode {
        let mut page = AxNode::new(
            "settings-page",
            AxRole::Group,
            t("settings.about.title"),
            frame,
        )
        .child(
            AxNode::new(
                "settings-page-title",
                AxRole::StaticText,
                t("settings.about.title"),
                self.settings_element_bounds("settings-page-title"),
            )
            .value(t("settings.about.subtitle")),
        );
        for (id, label, value) in self.settings_about_rows().unwrap_or_default() {
            page = page.child(
                AxNode::new(
                    id,
                    AxRole::StaticText,
                    label,
                    self.settings_element_bounds(id),
                )
                .value(value),
            );
        }
        page
    }
}
