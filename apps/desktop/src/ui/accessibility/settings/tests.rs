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
                .update(|window, cx| nav_rows(&view.read(cx).settings_rail_ax(window, cx, frame)));
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
            clear.right() <= view.settings_scroll.bounds().right()
                && input.bounds().size.width <= px(320.0)
                && save.left() - child.right() <= px(24.0),
            "bounded editor and adjacent actions: clear {:?}, viewport {:?}",
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
            view.projection.settings_permissions.approval_mode = Some(ApprovalModeWire::AlwaysAsk);
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
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| matches!(
            r.target,
            SearchTarget::Page(SettingsPage::Appearance | SettingsPage::Advanced)
                | SearchTarget::Panel(crate::ui::InspectorTab::Browser)
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

fn set_settings_query(view: &gpui::Entity<AppView>, cx: &mut gpui::VisualTestContext, text: &str) {
    let text = text.to_string();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.settings_search_input
                .update(cx, |input, cx| input.set_text(text, cx));
        });
    });
    cx.run_until_parked();
    flush_settings_frames(cx);
}

fn settings_rail(view: &gpui::Entity<AppView>, cx: &mut gpui::VisualTestContext) -> AxNode {
    cx.update(|window, cx| {
        view.read(cx)
            .settings_rail_ax(window, cx, AxRect::new(0.0, 0.0, 1440.0, 1024.0))
    })
}

fn settings_page(view: &gpui::Entity<AppView>, cx: &mut gpui::VisualTestContext) -> AxNode {
    cx.update(|window, cx| {
        view.read(cx)
            .settings_page_ax(window, cx, AxRect::new(0.0, 0.0, 1440.0, 1024.0))
    })
}

fn ax_ids(node: &AxNode) -> Vec<String> {
    let mut ids = vec![node.identifier.clone()];
    for child in &node.children {
        ids.extend(ax_ids(child));
    }
    ids
}

fn ax_blob(node: &AxNode) -> String {
    let mut blob = format!(
        "{}\n{}\n{}\n",
        node.label,
        node.value.as_deref().unwrap_or(""),
        node.description.as_deref().unwrap_or("")
    );
    for child in &node.children {
        blob.push_str(&ax_blob(child));
    }
    blob
}

/// 审批 radio 与供应商连接 / 详情行不在 settings_search_entries
/// （目录是 settings-approval-mode-header 与 settings-providers-heading）。
/// locate_settings_entry 随后会 focus_settings_row，未知 id 聚焦查找框。
/// 这里只走产品滚入 try_scroll_settings_row。调用方之后的 window.focus 是程序聚焦。
fn reveal_settings_row(
    view: &gpui::Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    row_id: &str,
) {
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let _ = view.try_scroll_settings_row(row_id);
            cx.notify();
        });
    });
    flush_settings_frames(cx);
}

fn assert_in_window(bounds: gpui::Bounds<gpui::Pixels>, id: &str) {
    let left = f32::from(bounds.origin.x);
    let top = f32::from(bounds.origin.y);
    let right = left + f32::from(bounds.size.width);
    let bottom = top + f32::from(bounds.size.height);
    assert!(
        right - left > 1.0 && bottom - top > 1.0,
        "{id} is not laid out: {bounds:?}"
    );
    assert!(
        left >= -1.0 && top >= -1.0 && right <= 1441.0 && bottom <= 1025.0,
        "{id} is outside the window: {bounds:?}"
    );
}

/// GUI2-06：空态与结果文案、导航 gate、定位焦点、非空查询离开、
/// 审批 radio / 代理 Switch 的聚焦与 pending、错误分层。
/// 无 Host 时当前值不变只说明本地投影没有乐观更新。
/// 滚入是产品 try_scroll_settings_row；window.focus 不是 focus_settings_row。
#[gpui::test]
fn settings_search_gates_focus_controls_and_error_layers(cx: &mut gpui::TestAppContext) {
    use crate::projection::{
        ApprovalModeWire, ConnectionState, ProviderAuthState, ProviderAuthStatusEntry,
        ProviderCatalogState,
    };
    use crate::ui::i18n::{self, Language};
    use crate::ui::theme::font::TextScale;
    use gpui::{px, size, Modifiers};
    use pawork_client::ProviderCredentialStatus;

    struct LanguageReset(Language);
    impl Drop for LanguageReset {
        fn drop(&mut self) {
            i18n::set_language(self.0);
        }
    }
    let _language = LanguageReset(i18n::language());
    i18n::set_language(Language::English);

    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            std::sync::Arc::new(crate::platform::Platform::new()),
            std::env::temp_dir().join("gui2-06-settings-gates.sock"),
            None,
            cx,
        )
    });
    cx.simulate_resize(size(px(1440.0), px(1024.0)));
    cx.update(|window, cx| {
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
            cx.notify();
        });
    });
    flush_settings_frames(cx);

    let nav = ax_ids(&settings_rail(&view, cx));
    for id in [
        "settings-nav-group-workspace",
        "settings-nav-general",
        "settings-nav-permissions",
        "settings-nav-tools",
        "settings-nav-terminal",
    ] {
        assert!(
            !nav.iter().any(|item| item == id),
            "{id} is visible offline"
        );
    }
    assert!(nav.iter().any(|id| id == "settings-nav-providers"));
    assert!(nav.iter().any(|id| id == "settings-nav-appearance"));

    set_settings_query(&view, cx, "zzzz-no-such-setting");
    let empty = settings_rail(&view, cx)
        .find("settings-search-empty")
        .expect("english empty state")
        .clone();
    assert_eq!(empty.label, "No matching settings");
    assert!(empty.bounds.height > 0.0);
    assert!(ax_ids(&settings_rail(&view, cx))
        .iter()
        .all(|id| !id.starts_with("settings-nav-")));

    set_settings_query(&view, cx, "font size");
    let font = settings_rail(&view, cx)
        .find("settings-search-result-Appearance-settings-appearance-text-size")
        .expect("english text size result")
        .clone();
    assert_eq!(font.label, "Text size");
    assert_eq!(font.value.as_deref(), Some("Appearance"));
    assert!(font.bounds.height > 0.0);

    set_settings_query(&view, cx, "Approvals");
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(view.settings_search_input.read(cx).text(), "Approvals");
        assert!(view
            .settings_search_results()
            .iter()
            .all(|entry| entry.page != SettingsPage::Permissions));
    });
    assert!(ax_ids(&settings_rail(&view, cx)).iter().all(|id| {
        id != "settings-nav-permissions" && !id.starts_with("settings-search-result-Permissions")
    }));

    i18n::set_language(Language::Chinese);
    set_settings_query(&view, cx, "没有这个设置zzzz");
    assert_eq!(
        settings_rail(&view, cx)
            .find("settings-search-empty")
            .expect("chinese empty state")
            .label,
        "没有匹配的设置"
    );
    set_settings_query(&view, cx, "字号");
    let font = settings_rail(&view, cx)
        .find("settings-search-result-Appearance-settings-appearance-text-size")
        .expect("chinese text size result")
        .clone();
    assert_eq!(font.label, "字号");
    assert_eq!(font.value.as_deref(), Some("外观"));
    set_settings_query(&view, cx, "审批");
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(view.settings_search_input.read(cx).text(), "审批");
        assert!(view
            .settings_search_results()
            .iter()
            .all(|entry| entry.page != SettingsPage::Permissions));
    });
    assert!(ax_ids(&settings_rail(&view, cx)).iter().all(|id| {
        id != "settings-nav-permissions" && !id.starts_with("settings-search-result-Permissions")
    }));

    set_settings_query(&view, cx, "字号");
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let scale = view.text_scale;
            view.submit_settings_search(window, cx);
            assert_eq!(view.text_scale, scale);
        });
    });
    flush_settings_frames(cx);
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.text_scale, TextScale::Percent100);
        assert!(view
            .settings_appearance_focus
            .get("settings-text-scale-100")
            .is_some_and(|focus| focus.is_focused(window)));
    });

    i18n::set_language(Language::English);
    set_settings_query(&view, cx, "font size");
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.submit_settings_search(window, cx);
            assert_eq!(view.text_scale, TextScale::Percent100);
        });
    });
    flush_settings_frames(cx);
    cx.update(|window, cx| {
        assert_eq!(view.read(cx).text_scale, TextScale::Percent100);
        assert!(view
            .read(cx)
            .settings_appearance_focus
            .get("settings-text-scale-100")
            .is_some_and(|focus| focus.is_focused(window)));
        assert!(!view.read(cx).settings_search_query.trim().is_empty());
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.on_close_settings(window, cx);
            assert_eq!(view.route, AppRoute::Workspace);
            assert!(view.settings_search_query.is_empty());
            assert!(view.settings_search_input.read(cx).text().is_empty());
            assert_eq!(view.text_input.read(cx).text(), "保留草稿");
        });
    });

    const APPROVAL_ROWS: [&str; 5] = [
        "settings-approval-mode-always_ask",
        "settings-approval-mode-ask_for_writes",
        "settings-approval-mode-ask_for_dangerous",
        "settings-approval-mode-never_ask",
        "settings-approval-mode-read_only",
    ];
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.route = AppRoute::Settings;
            view.settings_page = SettingsPage::Permissions;
            view.text_scale = TextScale::Percent100;
            window.set_rem_size(px(16.0));
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.projection.settings_permissions.query.mark_ready();
            view.projection.settings_permissions.approval_mode = Some(ApprovalModeWire::AlwaysAsk);
            view.settings_scroll
                .set_offset(gpui::point(px(0.0), px(0.0)));
            cx.notify();
        });
    });
    flush_settings_frames(cx);
    reveal_settings_row(&view, cx, "settings-approval-mode-ask_for_writes");
    let page = settings_page(&view, cx);
    assert!(!page
        .find("settings-approval-mode-always_ask")
        .expect("current approval row")
        .actions
        .contains(&AxAction::Press));
    assert!(page
        .find("settings-approval-mode-ask_for_writes")
        .expect("ask for writes row")
        .actions
        .contains(&AxAction::Press));
    let row = cx
        .debug_bounds("settings-approval-mode-ask_for_writes")
        .expect("ask for writes bounds");
    cx.simulate_click(row.center(), Modifiers::none());
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view
            .settings_permissions_focus
            .get("settings-approval-mode-ask_for_writes")
            .is_some_and(|focus| focus.is_focused(window)));
        assert_eq!(
            view.projection.settings_permissions.approval_mode,
            Some(ApprovalModeWire::AlwaysAsk)
        );
        assert!(view.pending_button_key_activate.is_none());
    });
    for key in ["enter", "space"] {
        cx.update(|_, cx| {
            view.update(cx, |view, _cx| view.pending_button_key_activate = None);
        });
        cx.simulate_keystrokes(key);
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(
                view.pending_button_key_activate.as_deref(),
                Some("settings-approval-mode-ask_for_writes"),
                "{key}"
            );
            assert_eq!(
                view.projection.settings_permissions.approval_mode,
                Some(ApprovalModeWire::AlwaysAsk),
                "{key} does not update the local mode"
            );
        });
    }

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection
                .settings_permissions
                .mark_stale("connection lost");
            view.pending_button_key_activate = None;
            cx.notify();
        });
    });
    flush_settings_frames(cx);
    reveal_settings_row(&view, cx, "settings-approval-mode-ask_for_writes");
    let page = settings_page(&view, cx);
    for id in APPROVAL_ROWS {
        assert!(
            !page.find(id).expect(id).actions.contains(&AxAction::Press),
            "{id}"
        );
    }
    cx.update(|window, cx| {
        view.update(cx, |view, _cx| {
            let focus = view
                .settings_permissions_focus
                .get("settings-approval-mode-ask_for_writes")
                .cloned()
                .expect("approval focus");
            window.focus(&focus);
        });
    });
    for key in ["enter", "space"] {
        cx.simulate_keystrokes(key);
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(view.pending_button_key_activate.is_none(), "{key}");
            assert_eq!(
                view.projection.settings_permissions.approval_mode,
                Some(ApprovalModeWire::AlwaysAsk)
            );
        });
    }

    for scale in [
        TextScale::Percent100,
        TextScale::Percent125,
        TextScale::Percent150,
    ] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.text_scale = scale;
                window.set_rem_size(px(scale.rem_pixels()));
                view.settings_scroll
                    .set_offset(gpui::point(px(0.0), px(0.0)));
                cx.notify();
            });
        });
        flush_settings_frames(cx);
        for id in APPROVAL_ROWS {
            reveal_settings_row(&view, cx, id);
            assert_in_window(cx.debug_bounds(id).expect(id), id);
            cx.update(|window, cx| {
                view.update(cx, |view, _cx| {
                    let focus = view
                        .settings_permissions_focus
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| panic!("{id} is not focusable at {scale:?}"));
                    window.focus(&focus);
                    assert!(focus.is_focused(window), "{id} at {scale:?}");
                });
            });
        }
    }

    let provider = |auth, catalog| ProviderAuthStatusEntry {
        selection_mode: Default::default(),
        provider_id: "glm".into(),
        display_name: "GLM".into(),
        endpoint_label: "https://secret.example/v1".into(),
        auth_methods: vec!["api_key".into()],
        credentials: vec![ProviderCredentialStatus {
            credential_id: "cred-1".into(),
            display_name: "primary".into(),
            selected: true,
            kind: "api_key".into(),
            masked_credential: "sk-secret-wxyz".into(),
            expired: false,
            expires_at: None,
        }],
        auth,
        catalog,
        use_proxy: false,
    };
    let unavailable = ProviderCatalogState::Unavailable {
        error: "catalog boom raw".into(),
        fetched_at: None,
    };
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.settings_page = SettingsPage::Providers;
            view.text_scale = TextScale::Percent100;
            window.set_rem_size(px(16.0));
            view.projection.settings_providers.providers = vec![provider(
                ProviderAuthState::Error {
                    message: "token rejected at https://raw.example".into(),
                },
                unavailable.clone(),
            )];
            view.projection
                .settings_providers
                .expanded_providers
                .clear();
            view.projection.settings_general.proxy_url = None;
            view.settings_scroll
                .set_offset(gpui::point(px(0.0), px(0.0)));
            cx.notify();
        });
    });
    flush_settings_frames(cx);
    reveal_settings_row(&view, cx, "settings-provider-connection-glm");
    let page = settings_page(&view, cx);
    assert_eq!(
        page.find("settings-provider-connection-glm")
            .and_then(|node| node.value.as_deref()),
        Some("Connection error")
    );
    assert_eq!(
        page.find("settings-provider-catalog-glm")
            .and_then(|node| node.value.as_deref()),
        Some("Catalog unavailable")
    );
    assert!(page.find("settings-provider-details-glm").is_none());
    let overview = ax_blob(page.find("settings-provider-glm").expect("collapsed card"));
    for secret in [
        "token rejected",
        "https://raw.example",
        "https://secret.example/v1",
        "sk-secret-wxyz",
        "catalog boom raw",
    ] {
        assert!(!overview.contains(secret), "{secret} leaked into overview");
    }

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection
                .settings_providers
                .expanded_providers
                .insert("glm".into());
            cx.notify();
        });
    });
    flush_settings_frames(cx);
    reveal_settings_row(&view, cx, "settings-provider-details-glm");
    let details = settings_page(&view, cx)
        .find("settings-provider-details-glm")
        .expect("auth error details")
        .value
        .clone();
    assert_eq!(
        details.as_deref(),
        Some("Connection error · token rejected at https://raw.example")
    );
    assert!(!details
        .as_deref()
        .unwrap_or("")
        .contains("catalog boom raw"));

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.settings_providers.providers = vec![provider(
                ProviderAuthState::Connected {
                    method: "api_key".into(),
                    masked_credential: Some("sk-secret-wxyz".into()),
                },
                unavailable,
            )];
            cx.notify();
        });
    });
    flush_settings_frames(cx);
    reveal_settings_row(&view, cx, "settings-provider-catalog-glm");
    reveal_settings_row(&view, cx, "settings-provider-details-glm");
    let page = settings_page(&view, cx);
    assert_eq!(
        page.find("settings-provider-connection-glm")
            .and_then(|node| node.value.as_deref()),
        Some("Connected")
    );
    assert_eq!(
        page.find("settings-provider-catalog-glm")
            .and_then(|node| node.value.as_deref()),
        Some("Catalog unavailable")
    );
    assert_eq!(
        page.find("settings-provider-details-glm")
            .and_then(|node| node.value.as_deref()),
        Some("Catalog unavailable · catalog boom raw")
    );

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.settings_general.proxy_url = Some("http://127.0.0.1:9".into());
            view.projection.settings_providers.providers[0].use_proxy = false;
            cx.notify();
        });
    });
    flush_settings_frames(cx);
    reveal_settings_row(&view, cx, "settings-action-use-proxy-glm");
    let switch = settings_page(&view, cx)
        .find("settings-action-use-proxy-glm")
        .expect("proxy switch")
        .clone();
    assert_eq!(switch.value.as_deref(), Some("Off"));
    assert!(switch.enabled);
    assert!(switch.actions.contains(&AxAction::Press));
    let bounds = cx
        .debug_bounds("settings-action-use-proxy-glm")
        .expect("proxy switch bounds");
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view
            .settings_action_focus
            .get("settings-action-use-proxy-glm")
            .is_some_and(|focus| focus.is_focused(window)));
        assert!(!view.projection.settings_providers.providers[0].use_proxy);
    });
    cx.update(|_, cx| {
        view.update(cx, |view, _cx| view.pending_button_key_activate = None);
    });
    cx.simulate_keystrokes("enter");
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(
            view.pending_button_key_activate.as_deref(),
            Some("settings-action-use-proxy-glm")
        );
        assert!(!view.projection.settings_providers.providers[0].use_proxy);
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection
                .settings_providers
                .mark_stale("connection lost");
            view.pending_button_key_activate = None;
            cx.notify();
        });
    });
    flush_settings_frames(cx);
    reveal_settings_row(&view, cx, "settings-action-use-proxy-glm");
    let switch = settings_page(&view, cx)
        .find("settings-action-use-proxy-glm")
        .expect("stale proxy switch")
        .clone();
    assert_eq!(switch.value.as_deref(), Some("Off"));
    assert!(!switch.enabled);
    assert!(!switch.actions.contains(&AxAction::Press));
    cx.update(|window, cx| {
        view.update(cx, |view, _cx| {
            let focus = view
                .settings_action_focus
                .get("settings-action-use-proxy-glm")
                .cloned()
                .expect("proxy focus");
            window.focus(&focus);
        });
    });
    for key in ["enter", "space"] {
        cx.simulate_keystrokes(key);
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(view.pending_button_key_activate.is_none(), "{key}");
            assert!(!view.projection.settings_providers.providers[0].use_proxy);
        });
    }
}
