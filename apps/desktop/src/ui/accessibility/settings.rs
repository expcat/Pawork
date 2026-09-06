//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::{App, Window};

use super::app::PAD;
use super::{AxAction, AxNode, AxRect, AxRole};
use crate::ui::i18n::t;
use crate::ui::shell_layout;
use crate::ui::theme::metrics;
use crate::ui::{AppView, SettingsPage};

fn settings_nav_ax(
    id: &'static str,
    label: &'static str,
    selected: bool,
    focused: bool,
    rect: AxRect,
) -> AxNode {
    if selected {
        AxNode::new(id, AxRole::StaticText, label, rect)
            .value(t("settings.nav.state_selected"))
            .focused(focused)
    } else {
        AxNode::new(id, AxRole::Button, label, rect)
            .focused(focused)
            .action(AxAction::Press)
    }
}

/// Settings 内容列宽（与 render 的 `SETTINGS_CONTENT_PAD` 同源，
/// OPT-4c / F2）：Rail 外全宽、两侧 32px padding，不保留 820px 上限。
/// 所有 settings_* 页 AX 几何必须经此取值，否则 AX 高亮框会与 render
/// 内容列系统性漂移。
pub(crate) fn settings_content_ax_width(frame: AxRect) -> f32 {
    (frame.width - crate::ui::settings::SETTINGS_CONTENT_PAD * 2.0)
        .max(0.0)
}

impl AppView {
    /// Settings 左栏（SET-3）：返回按钮 + 首页导航项。几何与
    /// settings.rs render 同源（Panel p_2 + 36px 安全区 + gap_2）；
    /// OPT-4d（F4）后导航项两态共用同一外壳几何，选中不改变行位。
    pub(crate) fn settings_rail_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        const TITLE_HEIGHT: f32 = 28.0;
        let title_y = PAD + shell_layout::TRAFFIC_LIGHT_SAFE_HEIGHT + PAD;
        let back_y = title_y + TITLE_HEIGHT + PAD;
        let nav_y = back_y + metrics::RAIL_TOP_ROW_HEIGHT + PAD;
        let width = (frame.width - PAD * 2.0).max(0.0);
        let general_available = self.projection.settings_general.query.available;
        let permissions_available = self.projection.settings_permissions.query.available;
        let tools_available = self.resources.available;
        let terminal_available = self.projection.settings_terminal.query.available;
        let about_available = self.settings_about_rows().is_some();
        let current_page = match self.settings_page {
            SettingsPage::General if !general_available => SettingsPage::Providers,
            SettingsPage::Permissions if !permissions_available => SettingsPage::Providers,
            SettingsPage::Tools if !tools_available => SettingsPage::Providers,
            SettingsPage::Terminal if !terminal_available => SettingsPage::Providers,
            SettingsPage::About if !about_available => SettingsPage::Advanced,
            page => page,
        };
        let mut rail = AxNode::new(
            "settings-rail",
            AxRole::Group,
            t("settings.rail_title"),
            frame,
        )
            .child(AxNode::new(
                "settings-rail-title",
                AxRole::StaticText,
                t("settings.rail_title"),
                AxRect::new(frame.x + PAD, frame.y + title_y, width, TITLE_HEIGHT),
            ))
            .child(
                AxNode::new(
                    "settings-back",
                    AxRole::Button,
                    t("settings.back_tooltip"),
                    AxRect::new(
                        frame.x + PAD,
                        frame.y + back_y,
                        width,
                        metrics::RAIL_TOP_ROW_HEIGHT,
                    ),
                )
                .focused(self.settings_back_focus.is_focused(window))
                .action(AxAction::Press),
            )
            .child(settings_nav_ax(
                "settings-nav-providers",
                t("settings.nav.providers"),
                current_page == SettingsPage::Providers,
                self.open_menu.is_none() && self.settings_nav_providers_focus.is_focused(window),
                AxRect::new(
                    frame.x + PAD,
                    frame.y + nav_y,
                    width,
                    metrics::RAIL_TOP_ROW_HEIGHT,
                ),
            ));
        if general_available {
            let general_y = nav_y + metrics::RAIL_TOP_ROW_HEIGHT + PAD;
            rail = rail.child(settings_nav_ax(
                "settings-nav-general",
                t("settings.nav.general"),
                current_page == SettingsPage::General,
                self.open_menu.is_none() && self.settings_nav_general_focus.is_focused(window),
                AxRect::new(
                    frame.x + PAD,
                    frame.y + general_y,
                    width,
                    metrics::RAIL_TOP_ROW_HEIGHT,
                ),
            ));
        }
        if permissions_available {
            // 几何与 render 同源：通用项之后递增一行（无通用项时紧随
            // 供应商项）。
            let mut permissions_y = nav_y + metrics::RAIL_TOP_ROW_HEIGHT + PAD;
            if general_available {
                permissions_y += metrics::RAIL_TOP_ROW_HEIGHT + PAD;
            }
            rail = rail.child(settings_nav_ax(
                "settings-nav-permissions",
                t("settings.nav.permissions"),
                current_page == SettingsPage::Permissions,
                self.open_menu.is_none() && self.settings_nav_permissions_focus.is_focused(window),
                AxRect::new(
                    frame.x + PAD,
                    frame.y + permissions_y,
                    width,
                    metrics::RAIL_TOP_ROW_HEIGHT,
                ),
            ));
        }
        if tools_available {
            // 几何与 render 同源：权限项之后递增一行（按可用项累计）。
            let mut tools_y = nav_y + metrics::RAIL_TOP_ROW_HEIGHT + PAD;
            if general_available {
                tools_y += metrics::RAIL_TOP_ROW_HEIGHT + PAD;
            }
            if permissions_available {
                tools_y += metrics::RAIL_TOP_ROW_HEIGHT + PAD;
            }
            rail = rail.child(settings_nav_ax(
                "settings-nav-tools",
                t("settings.nav.tools"),
                current_page == SettingsPage::Tools,
                self.open_menu.is_none() && self.settings_nav_tools_focus.is_focused(window),
                AxRect::new(
                    frame.x + PAD,
                    frame.y + tools_y,
                    width,
                    metrics::RAIL_TOP_ROW_HEIGHT,
                ),
            ));
        }
        if terminal_available {
            // 几何与 render 同源：工具项之后递增一行（按可用项累计）。
            let mut terminal_y = nav_y + metrics::RAIL_TOP_ROW_HEIGHT + PAD;
            if general_available {
                terminal_y += metrics::RAIL_TOP_ROW_HEIGHT + PAD;
            }
            if permissions_available {
                terminal_y += metrics::RAIL_TOP_ROW_HEIGHT + PAD;
            }
            if tools_available {
                terminal_y += metrics::RAIL_TOP_ROW_HEIGHT + PAD;
            }
            rail = rail.child(settings_nav_ax(
                "settings-nav-terminal",
                t("settings.nav.terminal"),
                current_page == SettingsPage::Terminal,
                self.open_menu.is_none() && self.settings_nav_terminal_focus.is_focused(window),
                AxRect::new(
                    frame.x + PAD,
                    frame.y + terminal_y,
                    width,
                    metrics::RAIL_TOP_ROW_HEIGHT,
                ),
            ));
        }
        // SET-6e 外观是 Desktop 本地能力，始终在所有 Host 可用页之后
        // 显示；位置按实际可见项累计，与 render 同源。
        let mut appearance_y = nav_y + metrics::RAIL_TOP_ROW_HEIGHT + PAD;
        if general_available {
            appearance_y += metrics::RAIL_TOP_ROW_HEIGHT + PAD;
        }
        if permissions_available {
            appearance_y += metrics::RAIL_TOP_ROW_HEIGHT + PAD;
        }
        if tools_available {
            appearance_y += metrics::RAIL_TOP_ROW_HEIGHT + PAD;
        }
        if terminal_available {
            appearance_y += metrics::RAIL_TOP_ROW_HEIGHT + PAD;
        }
        rail = rail.child(settings_nav_ax(
            "settings-nav-appearance",
            t("settings.nav.appearance"),
            current_page == SettingsPage::Appearance,
            self.open_menu.is_none() && self.settings_nav_appearance_focus.is_focused(window),
            AxRect::new(
                frame.x + PAD,
                frame.y + appearance_y,
                width,
                metrics::RAIL_TOP_ROW_HEIGHT,
            ),
        ));
        let advanced_y = appearance_y + metrics::RAIL_TOP_ROW_HEIGHT + PAD;
        rail = rail.child(settings_nav_ax(
            "settings-nav-advanced",
            t("settings.nav.advanced"),
            current_page == SettingsPage::Advanced,
            self.open_menu.is_none() && self.settings_nav_advanced_focus.is_focused(window),
            AxRect::new(
                frame.x + PAD,
                frame.y + advanced_y,
                width,
                metrics::RAIL_TOP_ROW_HEIGHT,
            ),
        ));
        if about_available {
            let about_y = advanced_y + metrics::RAIL_TOP_ROW_HEIGHT + PAD;
            rail = rail.child(settings_nav_ax(
                "settings-nav-about",
                t("settings.nav.about"),
                current_page == SettingsPage::About,
                self.open_menu.is_none() && self.settings_nav_about_focus.is_focused(window),
                AxRect::new(
                    frame.x + PAD,
                    frame.y + about_y,
                    width,
                    metrics::RAIL_TOP_ROW_HEIGHT,
                ),
            ));
        }
        rail
    }

    /// Settings 全宽内容（SET-4）：标题 / 状态行 / Provider 卡片。卡片含
    /// 只读事实行、secure 输入（value 恒为掩码，SET-010）与写动作按钮；
    /// 按钮与可见路径同 identifier / 同 gate（stale 时 enabled=false 且
    /// permits 拒绝写动作）。
    pub(crate) fn settings_page_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        if self.settings_page == SettingsPage::General
            && self.projection.settings_general.query.available
        {
            return self.settings_general_page_ax(window, cx, frame);
        }
        if self.settings_page == SettingsPage::Permissions
            && self.projection.settings_permissions.query.available
        {
            return self.settings_permissions_page_ax(window, cx, frame);
        }
        if self.settings_page == SettingsPage::Tools && self.resources.available {
            return self.settings_tools_page_ax(window, frame);
        }
        if self.settings_page == SettingsPage::Terminal
            && self.projection.settings_terminal.query.available
        {
            return self.settings_terminal_page_ax(window, cx, frame);
        }
        if self.settings_page == SettingsPage::Appearance {
            return self.settings_appearance_page_ax(window, frame);
        }
        if self.settings_page == SettingsPage::Advanced {
            return self.settings_advanced_page_ax(window, frame);
        }
        if self.settings_page == SettingsPage::About {
            if self.settings_about_rows().is_some() {
                return self.settings_about_page_ax(frame);
            }
            return self.settings_advanced_page_ax(window, frame);
        }
        self.settings_providers_page_ax(window, cx, frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::AppRoute;

    /// OPT-4d（F4）：导航选中态不参与布局——同一导航项在选中 / 未选中
    /// 两态下 AX frame 原点与尺寸完全一致（选中切换只换背景 / 角色，
    /// 不得移动行位）。
    #[gpui::test]
    fn settings_nav_ax_frames_stay_put_across_selection_change(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;

        struct NavHost {
            view: gpui::Entity<AppView>,
        }
        impl gpui::Render for NavHost {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                gpui::div()
            }
        }

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("opt4d-nav-frames.sock");
        let (host, cx) = cx.add_window_view(|_window, cx| {
            let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
            NavHost { view }
        });
        let view = cx.update(|_window, cx| host.read(cx).view.clone());
        let frame = AxRect::new(0.0, 0.0, 288.0, 1024.0);
        let nav_rows = |rail: &AxNode| -> Vec<(String, AxRole, AxRect)> {
            rail.children
                .iter()
                .filter(|child| child.identifier.starts_with("settings-nav-"))
                .map(|child| {
                    (
                        child.identifier.clone(),
                        child.role,
                        child.bounds,
                    )
                })
                .collect()
        };

        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.route = AppRoute::Settings;
                view.settings_page = SettingsPage::Appearance;
            });
        });
        let appearance_selected =
            cx.update(|window, cx| nav_rows(&view.read(cx).settings_rail_ax(window, frame)));
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.settings_page = SettingsPage::Providers;
            });
        });
        let providers_selected =
            cx.update(|window, cx| nav_rows(&view.read(cx).settings_rail_ax(window, frame)));

        // 两态确实不同（选中项从 Appearance 换到 Providers），但任何
        // 导航行的 frame 不得移动。
        let appearance_row_before = appearance_selected
            .iter()
            .find(|(id, _, _)| id == "settings-nav-appearance");
        let appearance_row_after = providers_selected
            .iter()
            .find(|(id, _, _)| id == "settings-nav-appearance");
        assert_eq!(appearance_row_before.map(|(_, role, _)| *role), Some(AxRole::StaticText));
        assert_eq!(appearance_row_after.map(|(_, role, _)| *role), Some(AxRole::Button));
        assert_eq!(appearance_selected.len(), providers_selected.len());
        for ((id_a, _, rect_a), (id_b, _, rect_b)) in
            appearance_selected.iter().zip(providers_selected.iter())
        {
            assert_eq!(id_a, id_b);
            assert_eq!(
                rect_a, rect_b,
                "nav row moved on selection change: {id_a}"
            );
        }
    }
}
