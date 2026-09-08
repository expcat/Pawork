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
    /// Settings 左栏（SET-3）：返回按钮 + 首页导航项。几何与
    /// settings.rs render 同源（Panel p_2 + 36px 安全区 + gap_2）；
    /// OPT-4d（F4）后导航项两态共用同一外壳几何，选中不改变行位。
    pub(crate) fn settings_rail_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        const TITLE_HEIGHT: f32 = 28.0;
        let pad = f32::from(window.rem_size()) * 0.5; // Panel p_2 / gap_2
        let title_y = pad + shell_layout::TRAFFIC_LIGHT_SAFE_HEIGHT + pad;
        let back_y = title_y + TITLE_HEIGHT + pad;
        let nav_y = back_y + metrics::RAIL_TOP_ROW_HEIGHT + pad;
        let width = (frame.width - pad * 2.0 - 1.0).max(0.0);
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
        )
        .child(settings_nav_ax(
            "settings-nav-providers",
            t("settings.nav.providers"),
            current_page == SettingsPage::Providers,
            self.open_menu.is_none() && self.settings_nav_providers_focus.is_focused(window),
            AxRect::new(
                frame.x + pad,
                frame.y + nav_y,
                width,
                crate::ui::settings::SETTINGS_NAV_HEIGHT,
            ),
        ));
        if general_available {
            let general_y = nav_y + crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
            rail = rail.child(settings_nav_ax(
                "settings-nav-general",
                t("settings.nav.general"),
                current_page == SettingsPage::General,
                self.open_menu.is_none() && self.settings_nav_general_focus.is_focused(window),
                AxRect::new(
                    frame.x + pad,
                    frame.y + general_y,
                    width,
                    crate::ui::settings::SETTINGS_NAV_HEIGHT,
                ),
            ));
        }
        if permissions_available {
            // 几何与 render 同源：通用项之后递增一行（无通用项时紧随
            // 供应商项）。
            let mut permissions_y = nav_y + crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
            if general_available {
                permissions_y += crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
            }
            rail = rail.child(settings_nav_ax(
                "settings-nav-permissions",
                t("settings.nav.permissions"),
                current_page == SettingsPage::Permissions,
                self.open_menu.is_none() && self.settings_nav_permissions_focus.is_focused(window),
                AxRect::new(
                    frame.x + pad,
                    frame.y + permissions_y,
                    width,
                    crate::ui::settings::SETTINGS_NAV_HEIGHT,
                ),
            ));
        }
        if tools_available {
            // 几何与 render 同源：权限项之后递增一行（按可用项累计）。
            let mut tools_y = nav_y + crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
            if general_available {
                tools_y += crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
            }
            if permissions_available {
                tools_y += crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
            }
            rail = rail.child(settings_nav_ax(
                "settings-nav-tools",
                t("settings.nav.tools"),
                current_page == SettingsPage::Tools,
                self.open_menu.is_none() && self.settings_nav_tools_focus.is_focused(window),
                AxRect::new(
                    frame.x + pad,
                    frame.y + tools_y,
                    width,
                    crate::ui::settings::SETTINGS_NAV_HEIGHT,
                ),
            ));
        }
        if terminal_available {
            // 几何与 render 同源：工具项之后递增一行（按可用项累计）。
            let mut terminal_y = nav_y + crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
            if general_available {
                terminal_y += crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
            }
            if permissions_available {
                terminal_y += crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
            }
            if tools_available {
                terminal_y += crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
            }
            rail = rail.child(settings_nav_ax(
                "settings-nav-terminal",
                t("settings.nav.terminal"),
                current_page == SettingsPage::Terminal,
                self.open_menu.is_none() && self.settings_nav_terminal_focus.is_focused(window),
                AxRect::new(
                    frame.x + pad,
                    frame.y + terminal_y,
                    width,
                    crate::ui::settings::SETTINGS_NAV_HEIGHT,
                ),
            ));
        }
        // SET-6e 外观是 Desktop 本地能力，始终在所有 Host 可用页之后
        // 显示；位置按实际可见项累计，与 render 同源。
        let mut appearance_y = nav_y + crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
        if general_available {
            appearance_y += crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
        }
        if permissions_available {
            appearance_y += crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
        }
        if tools_available {
            appearance_y += crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
        }
        if terminal_available {
            appearance_y += crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
        }
        rail = rail.child(settings_nav_ax(
            "settings-nav-appearance",
            t("settings.nav.appearance"),
            current_page == SettingsPage::Appearance,
            self.open_menu.is_none() && self.settings_nav_appearance_focus.is_focused(window),
            AxRect::new(
                frame.x + pad,
                frame.y + appearance_y,
                width,
                crate::ui::settings::SETTINGS_NAV_HEIGHT,
            ),
        ));
        let advanced_y = appearance_y + crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
        rail = rail.child(settings_nav_ax(
            "settings-nav-advanced",
            t("settings.nav.advanced"),
            current_page == SettingsPage::Advanced,
            self.open_menu.is_none() && self.settings_nav_advanced_focus.is_focused(window),
            AxRect::new(
                frame.x + pad,
                frame.y + advanced_y,
                width,
                crate::ui::settings::SETTINGS_NAV_HEIGHT,
            ),
        ));
        if about_available {
            let about_y = advanced_y + crate::ui::settings::SETTINGS_NAV_HEIGHT + pad;
            rail = rail.child(settings_nav_ax(
                "settings-nav-about",
                t("settings.nav.about"),
                current_page == SettingsPage::About,
                self.open_menu.is_none() && self.settings_nav_about_focus.is_focused(window),
                AxRect::new(
                    frame.x + pad,
                    frame.y + about_y,
                    width,
                    crate::ui::settings::SETTINGS_NAV_HEIGHT,
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
mod tests {
    use super::*;
    use crate::ui::AppRoute;

    /// OPT-4d（F4）：导航选中态不参与布局——同一导航项在选中 / 未选中
    /// 两态下 AX frame 原点与尺寸完全一致（选中切换只换背景 / 角色，
    /// 不得移动行位）。
    #[gpui::test]
    fn settings_nav_ax_frames_stay_put_across_selection_change(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;

        struct NavHost {
            view: gpui::Entity<AppView>,
        }
        impl gpui::Render for NavHost {
            fn render(
                &mut self,
                window: &mut Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                self.view.update(cx, |view, cx| {
                    view.settings_rail_element(gpui::px(288.0), window, cx)
                })
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
                .map(|child| (child.identifier.clone(), child.role, child.bounds))
                .collect()
        };

        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.route = AppRoute::Settings;
                view.settings_page = SettingsPage::Appearance;
            });
        });
        for rem in [16.0, 20.0, 24.0] {
            cx.update(|window, _cx| window.set_rem_size(gpui::px(rem)));
            let mut previous = None;
            for page in [SettingsPage::Appearance, SettingsPage::Providers] {
                cx.update(|_window, cx| {
                    view.update(cx, |view, cx| {
                        view.settings_page = page;
                        cx.notify();
                    });
                });
                cx.refresh().unwrap();
                cx.run_until_parked();
                let rows = cx
                    .update(|window, cx| nav_rows(&view.read(cx).settings_rail_ax(window, frame)));
                for (id, _, ax) in &rows {
                    let selector = [
                        "settings-nav-providers",
                        "settings-nav-general",
                        "settings-nav-permissions",
                        "settings-nav-tools",
                        "settings-nav-terminal",
                        "settings-nav-appearance",
                        "settings-nav-advanced",
                        "settings-nav-about",
                    ]
                    .into_iter()
                    .find(|candidate| *candidate == id)
                    .unwrap();
                    let actual = cx.debug_bounds(selector).expect("rendered navigation row");
                    assert_eq!(
                        *ax,
                        AxRect::new(
                            actual.origin.x.into(),
                            actual.origin.y.into(),
                            actual.size.width.into(),
                            actual.size.height.into(),
                        ),
                        "{id} at rem={rem}"
                    );
                }
                if let Some(previous) = previous {
                    let previous: Vec<(String, AxRole, AxRect)> = previous;
                    for ((id, _, before), (_, _, after)) in previous.iter().zip(&rows) {
                        assert_eq!(before, after, "{id} moved on selection change");
                    }
                }
                previous = Some(rows);
            }
        }
    }
    /// UI-5：真实布局下输入、按钮与滚动后的 AX 同步，断线仍拒绝写入。
    #[gpui::test]
    fn settings_controls_follow_layout_and_scroll(cx: &mut gpui::TestAppContext) {
        use crate::projection::ConnectionState;
        use crate::ui::theme::font::TextScale;
        use gpui::{point, px, size, Focusable, Modifiers};
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                platform,
                std::env::temp_dir().join("ui5-settings-layout.sock"),
                None,
                cx,
            )
        });
        for (width, height, scale) in [
            (1440.0, 1024.0, TextScale::Percent100),
            (1080.0, 720.0, TextScale::Percent125),
            (1080.0, 720.0, TextScale::Percent150),
        ] {
            cx.simulate_resize(size(px(width), px(height)));
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.route = AppRoute::Settings;
                    view.settings_page = SettingsPage::Terminal;
                    view.text_scale = scale;
                    window.set_rem_size(px(scale.rem_pixels()));
                    view.projection.settings_terminal.query.mark_ready();
                    view.projection.settings_terminal.columns = 80;
                    view.projection.settings_terminal.rows = 24;
                    view.projection
                        .set_connection(ConnectionState::Disconnected {
                            reason: "layout test".into(),
                        });
                    view.settings_scroll.set_offset(point(px(0.0), px(0.0)));
                    cx.notify();
                })
            });
            cx.refresh().unwrap();
            cx.run_until_parked();
            for id in [
                "settings-refresh",
                "settings-terminal-shell-input",
                "settings-terminal-columns-input",
                "settings-terminal-rows-input",
            ] {
                let actual = cx.debug_bounds(id).expect(id);
                cx.update(|window, cx| {
                    let tree = view.read(cx).settings_page_ax(
                        window,
                        cx,
                        AxRect::new(0.0, 0.0, 1440.0, 1024.0),
                    );
                    let node = tree.find(id).expect(id);
                    assert_eq!(
                        node.bounds,
                        AxRect::new(
                            actual.origin.x.into(),
                            actual.origin.y.into(),
                            actual.size.width.into(),
                            actual.size.height.into()
                        ),
                        "{id} at {scale:?}"
                    );
                    assert!(!node.enabled, "offline setting cannot write");
                });
            }
            // Mouse coordinates from the AX tree must focus the actual editor.
            let input = cx.debug_bounds("settings-terminal-columns-input").unwrap();
            cx.simulate_click(input.center(), Modifiers::none());
            cx.update(|window, cx| {
                assert!(view
                    .read(cx)
                    .settings_terminal_columns_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window));
            });
        }
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.settings_page = SettingsPage::General;
                view.projection.settings_general.query.mark_ready();
                view.settings_proxy_input.update(cx, |input, cx| {
                    input.reset_text("http://127.0.0.1:38081", cx)
                });
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = view.read(cx);
            let input = &view.settings_element_layouts["settings-proxy-input"];
            let child = input.bounds_for_item(0).unwrap();
            let save = view.settings_element_layouts["settings-proxy-save"].bounds();
            let clear = view.settings_element_layouts["settings-proxy-clear"].bounds();
            assert!(
                (clear.right() - view.settings_scroll.bounds().right()).abs() < px(1.0),
                "clear {:?}, viewport {:?}",
                clear,
                view.settings_scroll.bounds()
            );
            assert!(
                child.right() <= save.left(),
                "input {:?}, wrapper {:?}, save {:?}",
                child,
                input.bounds(),
                save
            );
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.settings_page = SettingsPage::Permissions;
                view.projection.settings_permissions.query.mark_ready();
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.settings_scroll.scroll_to_bottom();
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let tree =
                view.read(cx)
                    .settings_page_ax(window, cx, AxRect::new(0.0, 0.0, 1440.0, 1024.0));
            assert!(
                tree.find("settings-page-title").is_none(),
                "scrolled-out heading is absent"
            );
            let trust = tree
                .find("settings-workspace-trust")
                .expect("trust control scrolls into view");
            assert!(!trust.enabled);
            let viewport = view.read(cx).settings_scroll.bounds();
            assert!(trust.bounds.y >= f32::from(viewport.top()));
            assert!(trust.bounds.y + trust.bounds.height <= f32::from(viewport.bottom()));
        });
    }
}
