//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::{App, Window};

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

/// Unrendered/offscreen page nodes cannot expose actions at stale coordinates.
fn visible_settings_page(mut page: AxNode) -> AxNode {
    page.children
        .retain(|node| node.bounds.width > 0.0 && node.bounds.height > 0.0);
    page
}

impl AppView {
    /// Settings 左栏：返回、查找、分组导航。几何与 render 同源
    ///（Panel p_2 + gap_2 + 固定行高）；OPT-4d（F4）选中不改变行位。
    pub(crate) fn settings_rail_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        const TITLE_HEIGHT: f32 = 28.0;
        let pad = f32::from(window.rem_size()) * 0.5; // Panel p_2 / gap_2
        let title_y = pad + shell_layout::TRAFFIC_LIGHT_SAFE_HEIGHT + pad;
        let back_y = title_y + TITLE_HEIGHT + pad;
        let search_y = back_y + metrics::RAIL_TOP_ROW_HEIGHT + pad;
        let width = (frame.width - pad * 2.0 - 1.0).max(0.0);
        let current_page = self.settings_effective_page();
        let search_rect = AxRect::new(
            frame.x + pad,
            frame.y + search_y,
            width,
            metrics::SETTINGS_SEARCH_INPUT_HEIGHT,
        );
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
            AxRect::new(frame.x + pad, frame.y + title_y, width, TITLE_HEIGHT),
        ))
        .child(
            AxNode::new(
                "settings-back",
                AxRole::Button,
                t("settings.back_tooltip"),
                AxRect::new(
                    frame.x + pad,
                    frame.y + back_y,
                    width,
                    metrics::RAIL_TOP_ROW_HEIGHT,
                ),
            )
            .focused(self.settings_back_focus.is_focused(window))
            .action(AxAction::Press),
        );
        for node in self.settings_search_rail_ax(window, cx, search_rect) {
            rail = rail.child(node);
        }
        let mut cursor_y = search_y + metrics::SETTINGS_SEARCH_INPUT_HEIGHT + pad;
        for slot in self.settings_nav_slots() {
            match slot {
                crate::ui::settings::SettingsNavSlot::Group { id, label_key } => {
                    rail = rail.child(AxNode::new(
                        id,
                        AxRole::StaticText,
                        t(label_key),
                        AxRect::new(
                            frame.x + pad,
                            frame.y + cursor_y,
                            width,
                            metrics::SETTINGS_NAV_GROUP_HEIGHT,
                        ),
                    ));
                    cursor_y += metrics::SETTINGS_NAV_GROUP_HEIGHT + pad;
                }
                crate::ui::settings::SettingsNavSlot::Page {
                    page,
                    id,
                    label_key,
                } => {
                    let focused = self.open_menu.is_none()
                        && match page {
                            SettingsPage::Providers => {
                                self.settings_nav_providers_focus.is_focused(window)
                            }
                            SettingsPage::General => {
                                self.settings_nav_general_focus.is_focused(window)
                            }
                            SettingsPage::Permissions => {
                                self.settings_nav_permissions_focus.is_focused(window)
                            }
                            SettingsPage::Tools => self.settings_nav_tools_focus.is_focused(window),
                            SettingsPage::Terminal => {
                                self.settings_nav_terminal_focus.is_focused(window)
                            }
                            SettingsPage::Subagents => {
                                self.settings_nav_subagents_focus.is_focused(window)
                            }
                            SettingsPage::Appearance => {
                                self.settings_nav_appearance_focus.is_focused(window)
                            }
                            SettingsPage::Advanced => {
                                self.settings_nav_advanced_focus.is_focused(window)
                            }
                            SettingsPage::About => self.settings_nav_about_focus.is_focused(window),
                        };
                    rail = rail.child(settings_nav_ax(
                        id,
                        t(label_key),
                        current_page == page,
                        focused,
                        AxRect::new(
                            frame.x + pad,
                            frame.y + cursor_y,
                            width,
                            crate::ui::settings::SETTINGS_NAV_HEIGHT,
                        ),
                    ));
                    cursor_y += crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
                }
            }
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
            return visible_settings_page(self.settings_general_page_ax(window, cx, frame));
        }
        if self.settings_page == SettingsPage::Permissions
            && self.projection.settings_permissions.query.available
        {
            return visible_settings_page(self.settings_permissions_page_ax(window, cx, frame));
        }
        if self.settings_page == SettingsPage::Tools && self.resources.available {
            return visible_settings_page(self.settings_tools_page_ax(window, frame));
        }
        if self.settings_page == SettingsPage::Terminal
            && self.projection.settings_terminal.query.available
        {
            return visible_settings_page(self.settings_terminal_page_ax(window, cx, frame));
        }
        if self.settings_page == SettingsPage::Subagents
            && self.projection.settings_subagents.query.available
        {
            return visible_settings_page(self.settings_subagents_page_ax(window, cx, frame));
        }
        if self.settings_page == SettingsPage::Appearance {
            return visible_settings_page(self.settings_appearance_page_ax(window, frame));
        }
        if self.settings_page == SettingsPage::Advanced {
            return visible_settings_page(self.settings_advanced_page_ax(window, frame));
        }
        if self.settings_page == SettingsPage::About {
            if self.settings_about_rows().is_some() {
                return visible_settings_page(self.settings_about_page_ax(frame));
            }
            return visible_settings_page(self.settings_advanced_page_ax(window, frame));
        }
        self.settings_providers_page_ax(window, cx, frame)
    }
}

#[cfg(test)]
mod tests;
