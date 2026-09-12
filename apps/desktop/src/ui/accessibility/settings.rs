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
                let rows = cx.update(|window, cx| {
                    nav_rows(&view.read(cx).settings_rail_ax(window, cx, frame))
                });
                for (id, _, ax) in &rows {
                    let selector = [
                        "settings-nav-group-models",
                        "settings-nav-group-workspace",
                        "settings-nav-group-system",
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
        cx.update(|window, cx| {
            let tree =
                view.read(cx)
                    .settings_page_ax(window, cx, AxRect::new(0.0, 0.0, 1440.0, 1024.0));
            assert!(tree.find("settings-approval-mode-always_ask").is_some());
        });
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

    fn flush_settings_frames(cx: &mut gpui::TestAppContext) {
        for _ in 0..8 {
            cx.refresh().unwrap();
            cx.run_until_parked();
        }
    }

    /// GUI2-06：查找命中切页滚入，不改设置值；不可用页与断线 gate；Esc 清空。
    #[gpui::test]
    fn settings_search_locates_rows_without_writes(cx: &mut gpui::TestAppContext) {
        use crate::projection::{ApprovalModeWire, ConnectionState};
        use crate::ui::install_keybindings;
        use crate::ui::quick_search::SearchTarget;
        use crate::ui::theme::font::TextScale;
        use gpui::{px, size};

        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                std::sync::Arc::new(crate::platform::Platform::new()),
                std::env::temp_dir().join("gui2-06-settings-search.sock"),
                None,
                cx,
            )
        });
        cx.simulate_resize(size(px(1440.0), px(1024.0)));
        cx.update(|window, cx| {
            install_keybindings(cx);
            view.update(cx, |view, cx| {
                view.text_input
                    .update(cx, |input, cx| input.set_text("保留草稿", cx));
                view.route = AppRoute::Settings;
                view.text_scale = TextScale::Percent100;
                window.set_rem_size(px(16.0));
                view.projection
                    .set_connection(ConnectionState::Disconnected {
                        reason: "offline".into(),
                    });
                window.focus(&view.settings_search_focus);
                cx.notify();
            });
        });
        flush_settings_frames(cx);

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.settings_search_input
                    .update(cx, |input, cx| input.set_text("代理", cx));
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(
                view.settings_search_results()
                    .iter()
                    .all(|entry| entry.page != SettingsPage::General),
                "unavailable Network page must not appear"
            );
        });

        cx.update(|_window, cx| {
            view.update(cx, |view, cx| {
                view.settings_search_input
                    .update(cx, |input, cx| input.set_text("字号", cx));
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert!(view
                    .settings_search_results()
                    .iter()
                    .any(|entry| entry.page == SettingsPage::Appearance
                        && entry.row_id == "settings-appearance-text-size"));
                let scale_before = view.text_scale;
                view.submit_settings_search(window, cx);
                assert_eq!(view.text_scale, scale_before);
            });
        });
        flush_settings_frames(cx);
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.settings_page, SettingsPage::Appearance);
            assert_eq!(view.text_scale, TextScale::Percent100);
            let tree = view.settings_page_ax(window, cx, AxRect::new(0.0, 0.0, 1440.0, 1024.0));
            let row = tree
                .find("settings-appearance-text-size")
                .expect("located text size row");
            let viewport = view.settings_scroll.bounds();
            assert!(row.bounds.height > 0.0);
            assert!(row.bounds.y + row.bounds.height > f32::from(viewport.top()));
            assert!(row.bounds.y < f32::from(viewport.bottom()));
            assert!(view
                .status_hint
                .as_deref()
                .map(|hint| !hint.contains("Could not"))
                .unwrap_or(true));
        });

        cx.update(|_window, cx| {
            view.update(cx, |view, cx| {
                view.settings_search_input
                    .update(cx, |input, cx| input.set_text("font size", cx));
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(view
                .settings_search_results()
                .iter()
                .any(|entry| entry.row_id == "settings-appearance-text-size"));
        });

        cx.update(|_window, cx| {
            view.update(cx, |view, cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.projection.settings_general.query.mark_ready();
                view.projection.settings_permissions.query.mark_ready();
                view.projection.settings_permissions.approval_mode =
                    Some(ApprovalModeWire::AlwaysAsk);
                view.settings_search_input
                    .update(cx, |input, cx| input.set_text("proxy", cx));
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert!(view
                    .settings_search_results()
                    .iter()
                    .any(|entry| entry.page == SettingsPage::General));
                view.submit_settings_search(window, cx);
            });
        });
        flush_settings_frames(cx);
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.settings_page, SettingsPage::General);
            assert!(view
                .settings_page_ax(window, cx, AxRect::new(0.0, 0.0, 1440.0, 1024.0))
                .find("settings-proxy-heading")
                .is_some());
        });

        cx.update(|_window, cx| {
            view.update(cx, |view, cx| {
                view.settings_search_input
                    .update(cx, |input, cx| input.set_text("审批", cx));
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let before = view.projection.settings_permissions.approval_mode;
                view.submit_settings_search(window, cx);
                assert_eq!(view.projection.settings_permissions.approval_mode, before);
            });
        });
        flush_settings_frames(cx);
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.settings_page, SettingsPage::Permissions);
            assert_eq!(
                view.projection.settings_permissions.approval_mode,
                Some(ApprovalModeWire::AlwaysAsk)
            );
            assert!(view
                .settings_page_ax(window, cx, AxRect::new(0.0, 0.0, 1440.0, 1024.0))
                .find("settings-approval-mode-header")
                .is_some());
            assert!(view
                .status_hint
                .as_deref()
                .map(|hint| !hint.contains("Could not"))
                .unwrap_or(true));
        });

        cx.update(|_window, cx| {
            view.update(cx, |view, cx| {
                view.settings_search_input
                    .update(cx, |input, cx| input.set_text("模型", cx));
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert!(view
                    .settings_search_results()
                    .iter()
                    .any(|entry| entry.page == SettingsPage::Providers));
                view.submit_settings_search(window, cx);
            });
        });
        flush_settings_frames(cx);
        cx.update(|_, cx| {
            assert_eq!(view.read(cx).settings_page, SettingsPage::Providers);
        });

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert!(view.settings_search_input_focused(window));
                view.clear_settings_search(window, cx);
                assert!(view.settings_search_query.is_empty());
                assert!(view.settings_search_input.read(cx).text().is_empty());
                assert!(view.settings_search_focus.is_focused(window));
            });
        });

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.projection
                    .set_connection(ConnectionState::Disconnected {
                        reason: "offline".into(),
                    });
                view.open_quick_search(window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let results = view.read(cx).quick_search_results();
            assert_eq!(results.len(), 2);
            assert!(results.iter().all(|r| matches!(
                r.target,
                SearchTarget::Page(SettingsPage::Appearance | SettingsPage::Advanced)
            )));
        });

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.close_quick_search(window, cx);
                view.on_close_settings(window, cx);
                assert_eq!(view.route, AppRoute::Workspace);
                assert_eq!(view.text_input.read(cx).text(), "保留草稿");
            });
        });
    }
}
