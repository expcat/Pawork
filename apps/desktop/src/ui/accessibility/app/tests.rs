use super::*;

#[gpui::test]
fn text_scale_feedback_expires_without_clearing_errors(cx: &mut gpui::TestAppContext) {
    use crate::ui::theme::font::TextScale;
    use std::time::Duration;
    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            std::sync::Arc::new(crate::platform::Platform::new()),
            std::env::temp_dir().join("ux09-missing.sock"),
            None,
            cx,
        )
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.status_hint = None;
            v.set_text_scale(TextScale::Percent125, window, cx);
        })
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.set_text_scale(TextScale::Percent150, window, cx);
            assert!(v.status_feedback.as_ref().unwrap().contains("150"));
        })
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            assert!(
                v.status_feedback.is_some(),
                "old timer must not clear the latest feedback"
            );
            let with_feedback = v.composer_outer_height(40.0, window);
            v.status_hint = Some("new error".into());
            assert_eq!(v.composer_notes().len(), 0);
            assert_eq!(v.status_bar_trailing_text(), "new error");
            assert!(with_feedback > 0.0);
            cx.notify();
        })
    });
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |v, _| {
            assert!(v.status_feedback.is_none());
            assert_eq!(v.status_hint.as_deref(), Some("new error"));
            assert_eq!(v.composer_notes(), Vec::<(&str, String)>::new());
            assert_eq!(v.status_bar_trailing_text(), "new error");
            assert_eq!(v.text_scale, TextScale::Percent150);
            let with_error = v.composer_outer_height(40.0, window);
            v.status_hint = None;
            let without = v.composer_outer_height(40.0, window);
            assert_eq!(with_error, without);
            let note = crate::ui::settings::settings_mcp_effect_note(&v.resources);
            assert!(note.contains("config.toml") && note.contains("[mcp.servers."));
            assert!(!note.contains("Remove updates"));
        })
    });
    cx.update(|_, cx| {
        view.update(cx, |v, cx| {
            v.handle_controller_event(
                crate::controller::ControllerEvent::SessionForked {
                    session_id: "forked".into(),
                },
                cx,
            );
            assert_eq!(v.projection.active_session_id.as_deref(), Some("forked"));
            assert_eq!(
                v.status_feedback,
                Some(t("status.forked").replace("{}", "forked"))
            );
            let text = "  exact copy\n中文\n";
            v.copy_timeline_text(text.into(), cx);
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some(text)
            );
            assert_eq!(v.status_feedback.as_deref(), Some(t("timeline.copied")));
            v.status_hint = Some("persistent copy error".into());
            assert_eq!(v.status_bar_trailing_text(), "persistent copy error");
        });
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert!(v.status_feedback.is_none());
        assert_eq!(v.status_bar_trailing_text(), "persistent copy error");
    });
}

#[gpui::test]
fn recovery_keeps_errors_local_and_preserves_drafts(cx: &mut gpui::TestAppContext) {
    use crate::controller::ControllerEvent;
    use gpui::px;
    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            std::sync::Arc::new(crate::platform::Platform::new()),
            std::env::temp_dir().join("ux04-missing.sock"),
            None,
            cx,
        )
    });
    cx.run_until_parked();
    cx.simulate_resize(gpui::size(px(1440.0), px(1024.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.connection = ConnectionState::Failed {
                reason: "connection refused".into(),
            };
            view.status_hint = Some("unrelated feedback".into());
            view.text_input
                .update(cx, |input, cx| input.set_text("keep this draft", cx));
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        let tree = v.accessibility_tree(window, cx);
        tree.validate().unwrap();
        assert!(tree.find("workspace-empty-title").is_none());
        assert!(tree.find("connection-retry").is_some());
        assert!(tree.find("connection-diagnostics").is_some());
        let retry = tree.find("connection-retry").unwrap();
        let rail_retry = tree.find("reconnect").unwrap();
        assert_eq!(retry.label, t("recovery.retry"));
        assert_eq!(retry.description.as_deref(), Some("primary"));
        assert_eq!(rail_retry.label, t("rail.reconnect"));
        assert_eq!(rail_retry.description.as_deref(), Some("ghost"));
        assert_eq!(
            tree.find("connection-status")
                .and_then(|n| n.value.as_deref()),
            Some(t("rail.connection_local_disconnected"))
        );
        assert!(tree
            .find("connection-notice")
            .unwrap()
            .label
            .contains("connection refused"));
        assert_eq!(v.model_label(), t("recovery.model_failed"));
        assert!(v.connection_notice_text().1.contains("address"));
    });
    cx.update(|window, cx| view.update(cx, |v, cx| {
        v.on_recovery_action("connection-diagnostics", window, cx);
        assert_eq!(v.settings_page, SettingsPage::Advanced);
        assert_eq!(v.text_input.read(cx).text(), "keep this draft");
        v.on_close_settings(window, cx);
        v.projection.connection = ConnectionState::Connecting;
        v.connection_attempts = 2;
        assert_eq!(v.model_label(), t("recovery.reconnecting"));
        v.projection.connection = ConnectionState::Connected { instance_id: "ux04".into() };
        v.projection.set_models(Vec::new());
        assert_eq!(v.model_label(), t("composer.model_none_available"));
        v.projection.workspace_id = Some("ws-a".into());
        v.projection.terminal.workspace_id = Some("ws-a".into());
        v.projection.settings_permissions.query.mark_ready();
        v.projection.settings_permissions.approval_mode = Some(ApprovalModeWire::ReadOnly);
        v.inspector_open = true;
        v.inspector_motion.width(true, true, std::time::Instant::now() - std::time::Duration::from_secs(1));
        v.inspector_tab = InspectorTab::Terminal;
        v.ensure_terminal(cx);
        assert!(v.terminal_pending_create_workspace.is_none());
        assert!(!!v.terminal_create_blocked());
        assert_eq!(v.terminal_notice_text().as_deref(), Some(t("recovery.terminal_read_only")));
        v.status_hint = Some("unrelated feedback".into());
        v.handle_controller_event(ControllerEvent::TerminalCreateFailed {
            workspace_id: "ws-a".into(),
            reason: "ProtocolError: 审批档 read_only 禁止创建终端:ADR-041 D2 决议该档拒绝创建交互 shell(fail-closed)".into(),
        }, cx);
        assert_eq!(v.status_hint.as_deref(), Some("unrelated feedback"));
        assert_eq!(v.text_input.read(cx).text(), "keep this draft");
        v.projection.settings_permissions.available = false;
        assert!(!!v.terminal_create_blocked(), "explicit Host read-only rejection also blocks repeated creation");
        assert_eq!(v.terminal_notice_text().as_deref(), Some(t("recovery.terminal_read_only_unknown")));
        cx.notify();
    }));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        let tree = v.accessibility_tree(window, cx);
        tree.validate().unwrap();
        for id in [
            "terminal-start",
            "terminal-input",
            "terminal-resize",
            "terminal-cols-dec",
            "terminal-rows-inc",
        ] {
            assert!(tree.find(id).is_none(), "obsolete terminal control {id}");
        }
        let bounds = v.terminal_action_layouts["terminal-output"].bounds();
        let node = tree.find("terminal-output").unwrap();
        assert_eq!(
            node.bounds,
            AxRect::new(
                bounds.origin.x.into(),
                bounds.origin.y.into(),
                bounds.size.width.into(),
                bounds.size.height.into()
            )
        );
        assert!(f32::from(bounds.size.height) > 40.0);
        assert!(node.actions.contains(&AxAction::Focus));
        assert!(!node.actions.contains(&AxAction::SetValue));
        assert!(tree
            .find("terminal-notice")
            .unwrap()
            .label
            .contains("Read-only"));
        assert!(tree.find("terminal-details").is_some());
    });
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.on_recovery_action("terminal-details", window, cx);
            assert!(v.terminal_details_open.is_some());
            v.projection
                .settings_permissions
                .query
                .mark_stale("connection changed");
            assert!(
                !v.terminal_read_only(),
                "old permissions cannot describe a new connection"
            );
            v.projection
                .apply_terminal_created("ws-a".into(), "term-a".into());
            v.handle_controller_event(
                ControllerEvent::TerminalWriteFailed {
                    terminal_session_id: "term-a".into(),
                    reason: "write failed".into(),
                },
                cx,
            );
            assert!(terminal_can_operate(
                &v.projection.connection,
                &v.projection.terminal
            ));
            assert!(v.terminal_notice_text().is_some());
            assert_eq!(v.status_hint.as_deref(), Some("unrelated feedback"));
            v.handle_controller_event(
                ControllerEvent::TerminalWriteSucceeded {
                    terminal_session_id: "term-a".into(),
                },
                cx,
            );
            assert!(v.projection.terminal.last_error.is_none());
            v.projection
                .apply_terminal_exited("term-a", pawork_client::TerminalExitReason::Exited);
            assert_eq!(
                v.terminal_notice_text().as_deref(),
                Some(t("recovery.terminal_exited"))
            );
            assert!(!terminal_can_operate(
                &v.projection.connection,
                &v.projection.terminal
            ));
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        let tree = v.accessibility_tree(window, cx);
        assert_eq!(
            tree.find("terminal-notice").unwrap().label,
            t("recovery.terminal_exited")
        );
        assert_eq!(v.text_input.read(cx).text(), "keep this draft");
    });
}

/// GUI3-07：Header 36px 查找入口、无 Git 无空洞、断线 Ghost/Primary
/// 分层、底行只显示 Local · Disconnected。
#[gpui::test]
fn header_actions_and_disconnect_hierarchy(cx: &mut gpui::TestAppContext) {
    use crate::controller::GitDiffInfo;
    use crate::ui::theme::font::TextScale;
    use gpui::{px, size};
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("gui3-07-header.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.projection.sessions = vec![rail_session_summary(
                "s-1",
                "Header task · 中英文长标题 keeps branch and live status reachable without overlapping actions",
                None,
                crate::ui::now_unix_ms(),
            )];
            view.projection.active_session_id = Some("s-1".into());
            view.text_input
                .update(cx, |input, cx| input.set_text("keep draft", cx));
            cx.notify();
        })
    });
    for (width, height, scale) in [
        (1440.0, 1024.0, TextScale::Percent100),
        (1440.0, 1024.0, TextScale::Percent125),
        (1080.0, 720.0, TextScale::Percent150),
    ] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.text_scale = scale;
                window.set_rem_size(px(scale.rem_pixels()));
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            let find = tree.find("timeline-find").expect("timeline-find");
            assert_eq!(find.label, t("find.title"));
            assert!(
                find.bounds.width >= metrics::ICON_BUTTON_SIZE,
                "timeline-find width {} at {width}x{height} {scale:?}",
                find.bounds.width
            );
            let expand = tree.find("inspector-expand").expect("inspector-expand");
            let activity = tree.find("inspector-toggle").unwrap().bounds;
            assert_eq!(find.bounds.width, expand.bounds.width);
            assert_eq!(find.bounds.height, expand.bounds.height);
            assert_eq!(activity.width, expand.bounds.width);
            assert_eq!(activity.height, expand.bounds.height);
            let title = tree.find("header-title").unwrap();
            assert_eq!(title.bounds, view.shell_ax_rect("header-title"));
            assert!(!ax_rects_overlap(title.bounds, find.bounds));
            assert!(
                !ax_rects_overlap(find.bounds, expand.bounds),
                "find {:?} overlaps inspector-expand {:?}",
                find.bounds,
                expand.bounds
            );
            assert!(tree.find("header-branch").is_none());
            assert!(tree.find("header-status").is_none());
        });
    }

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.changes.session_id = Some("s-1".into());
            view.changes.git = Some(GitDiffInfo {
                branch: Some("ui08/long-branch-for-header-and-status-bar-overlap".into()),
                work_dir: None,
                dirty_files: None,
            });
            view.projection.active_runs.push(ActiveRun {
                run_id: "r-1".into(),
                session_id: "s-1".into(),
                started_at_ms: crate::ui::now_unix_ms(),
            });
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        let branch = tree.find("header-branch").expect("header-branch chip");
        assert_eq!(
            branch.label,
            "ui08/long-branch-for-header-and-status-bar-overlap"
        );
        assert!(branch.bounds.width > 0.0 && branch.bounds.height > 0.0);
        let status = tree.find("header-status").expect("header-status chip");
        assert_eq!(status.label, t("live.running"));
        assert!(status.bounds.width > 0.0 && status.bounds.height > 0.0);
        let title = tree.find("header-title").unwrap().bounds;
        assert!(!ax_rects_overlap(title, branch.bounds));
        assert!(!ax_rects_overlap(branch.bounds, status.bounds));
        assert!(!ax_rects_overlap(
            status.bounds,
            tree.find("timeline-find").unwrap().bounds
        ));
    });
    let bar = cx.debug_bounds("shell-status-bar").unwrap();
    let leading = cx.debug_bounds("status-bar-leading-slot").unwrap();
    let centered = cx.debug_bounds("status-bar-center-slot").unwrap();
    let trailing = cx.debug_bounds("status-bar-trailing-slot").unwrap();
    assert!(leading.right() <= centered.left());
    assert!(centered.right() <= trailing.left());
    assert!((centered.center().x - bar.center().x).abs() <= px(1.0));

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.connection = ConnectionState::Failed {
                reason: "connection is closed by peer with a long diagnostic".into(),
            };
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        let rail_retry = tree.find("reconnect").expect("rail reconnect");
        let main_retry = tree.find("connection-retry").expect("connection-retry");
        assert_eq!(rail_retry.label, t("rail.reconnect"));
        assert_eq!(rail_retry.description.as_deref(), Some("ghost"));
        assert_eq!(main_retry.label, t("recovery.retry"));
        assert_eq!(main_retry.description.as_deref(), Some("primary"));
        assert!(rail_retry.bounds.width > 0.0 && rail_retry.bounds.height > 0.0);
        let status = tree.find("connection-status").expect("connection-status");
        assert_eq!(
            status.value.as_deref(),
            Some(t("rail.connection_local_disconnected"))
        );
        let rail = tree.find("task-rail").expect("task-rail");
        assert!(
            status.bounds.x + status.bounds.width <= rail.bounds.width + 1.0,
            "footer text frame {:?} outside rail {:?}",
            status.bounds,
            rail.bounds
        );
        assert!(
            rail_retry.bounds.x + rail_retry.bounds.width <= rail.bounds.width + 1.0,
            "reconnect {:?} outside rail {:?}",
            rail_retry.bounds,
            rail.bounds
        );
        assert!(tree
            .find("connection-notice")
            .unwrap()
            .label
            .contains("connection is closed by peer"));
        assert_eq!(view.text_input.read(cx).text(), "keep draft");
    });

    for (width, height, scale) in [
        (1080.0, 720.0, TextScale::Percent100),
        (1440.0, 1024.0, TextScale::Percent125),
        (1080.0, 720.0, TextScale::Percent150),
    ] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.text_scale = scale;
                window.set_rem_size(px(scale.rem_pixels()));
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            let status = tree.find("connection-status").unwrap();
            assert_eq!(
                status.value.as_deref(),
                Some(t("rail.connection_local_disconnected"))
            );
            let rail = tree.find("task-rail").unwrap();
            assert!(
                status.bounds.x + status.bounds.width <= rail.bounds.width + 1.0,
                "footer clipped at {width} {scale:?}: {:?} rail {:?}",
                status.bounds,
                rail.bounds
            );
        });
    }

    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.projection.connection = ConnectionState::Connecting;
            view.connection_attempts = 2;
            assert!(!view.projection.show_reconnect());
            view.on_reconnect(window, cx);
            assert!(matches!(
                view.projection.connection,
                ConnectionState::Connecting
            ));
            assert_eq!(view.text_input.read(cx).text(), "keep draft");
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        assert!(tree.find("reconnect").is_none());
        assert!(tree.find("connection-retry").is_none());
        assert_eq!(
            tree.find("connection-status")
                .and_then(|n| n.value.as_deref()),
            Some(t("rail.connection_local_connecting"))
        );
    });
}

/// GUI2-02：真实布局区分短气泡 / 开放回复，导航不消耗草稿。
#[gpui::test]
fn conversation_bubbles_and_model_navigation_preserve_draft(cx: &mut gpui::TestAppContext) {
    use crate::ui::theme::font::TextScale;
    use gpui::{px, size};
    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            std::sync::Arc::new(crate::platform::Platform::new()),
            std::env::temp_dir().join("gui2-02-layout.sock"),
            None,
            cx,
        )
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.connection = ConnectionState::Connected {
                instance_id: "test".into(),
            };
            view.projection.active_session_id = Some("task".into());
            view.projection.timeline.entries = vec![TimelineEntry {
                sequence: 1,
                event_id: "short".into(),
                timestamp: "1".into(),
                run_id: None,
                kind: TimelineEntryKind::UserMessage {
                    text: "你好 Pawork".into(),
                },
                fork_boundary: None,
            }];
            view.text_input
                .update(cx, |input, cx| input.set_text("保留草稿", cx));
            view.timeline_changed();
            cx.notify();
        })
    });
    for (width, height) in [(1440., 1024.), (1080., 720.)] {
        for scale in [
            TextScale::Percent100,
            TextScale::Percent125,
            TextScale::Percent150,
        ] {
            cx.simulate_resize(size(px(width), px(height)));
            cx.update(|_, cx| {
                view.update(cx, |v, cx| {
                    v.text_scale = scale;
                    cx.notify();
                })
            });
            cx.refresh().unwrap();
            cx.run_until_parked();
            cx.update(|window, cx| {
                let v = view.read(cx);
                let bounds = |id: &str| {
                    let b = v.settings_element_layouts[id].bounds();
                    AxRect::new(
                        b.origin.x.into(),
                        b.origin.y.into(),
                        b.size.width.into(),
                        b.size.height.into(),
                    )
                };
                let bubble = bounds("message-bubble-short");
                let actions = bounds("message-actions-short");
                assert!(
                    bubble.width > 60. && bubble.width < 350.,
                    "short bubble: {bubble:?}"
                );
                assert!((bubble.x + bubble.width - actions.x - actions.width).abs() < 2.);
                let tree = v.accessibility_tree(window, cx);
                assert!(tree.find("run-status").is_some());
                let ax = tree.find(&entry_menu_identifier("short")).unwrap();
                assert_eq!(ax.bounds, actions);
                assert!(ax.actions.contains(&AxAction::Press));
            });
        }
    }
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.projection.set_models(Vec::new());
            v.on_toggle_model_menu(None, window, cx);
            assert_eq!(v.menu_item_count(), 1);
            v.move_menu_highlight(true);
            v.activate_menu_item(MenuKind::Model, 0, window, cx);
            assert_eq!(v.route, AppRoute::Settings);
            assert_eq!(v.settings_page, SettingsPage::Providers);
            assert!(v.focus_handle.is_focused(window));
            assert_eq!(v.text_input.read(cx).text(), "保留草稿");
            v.on_close_settings(window, cx);
            assert!(v.run_status_visible());
            v.route = AppRoute::Settings;
            assert!(!v.run_status_visible());
            v.route = AppRoute::Workspace;
            assert!(v.run_status_visible());
        })
    });
}

/// GUI2-02：代码 / 表格在块内横滚，消息菜单走鼠标、键盘和 AX，模型菜单的
/// 无结果与空目录仍进入供应商管理。输入是 GPUI TestAppContext 的投影、
/// 鼠标、按键和 AX，不是真窗口。
#[gpui::test]
fn conversation_blocks_scroll_and_menus_keep_run(cx: &mut gpui::TestAppContext) {
    use crate::ui::theme::font::TextScale;
    use crate::ui::timeline_entry::EntryActionKind;
    use gpui::{px, size, ClipboardItem, Modifiers};

    let code_line = format!("let 原文 = \"{}\";\r\n", "W".repeat(240));
    let fence = "\u{60}".repeat(3);
    // TestAppContext 的文本系统把每个 BMP 字符排成 0.6em，真窗口 CoreText
    // 里「宽」约 1em。160 字在 100% 的 880px 阅读列下两种度量都会横滚。
    let cell = "宽".repeat(160);
    let assistant = format!("{fence}rust\n{code_line}{fence}\n\n| 列 |\n| --- |\n| {cell} |\n");
    let copies = crate::ui::markdown::message_code_copies("reply", &assistant);
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].identifier, "reply-code-0");
    let expected_code = copies[0].content.clone();
    assert_eq!(expected_code, code_line);

    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            std::sync::Arc::new(crate::platform::Platform::new()),
            std::env::temp_dir().join("gui2-02-conversation.sock"),
            None,
            cx,
        )
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        crate::ui::install_keybindings(cx);
        window.set_rem_size(px(TextScale::Percent100.rem_pixels()));
    });
    cx.simulate_resize(size(px(1440.0), px(1024.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.connection = ConnectionState::Connected {
                instance_id: "gui2-02".into(),
            };
            view.projection.sessions = vec![rail_session_summary("task", "阅读任务", None, 1)];
            view.projection.active_session_id = Some("task".into());
            view.projection.timeline.entries = vec![
                TimelineEntry {
                    sequence: 1,
                    event_id: "short".into(),
                    timestamp: "fixture-user".into(),
                    run_id: None,
                    kind: TimelineEntryKind::UserMessage {
                        text: "你好 Pawork".into(),
                    },
                    fork_boundary: None,
                },
                TimelineEntry {
                    sequence: 2,
                    event_id: "reply".into(),
                    timestamp: "fixture-reply".into(),
                    run_id: None,
                    kind: TimelineEntryKind::AssistantMessage { text: assistant },
                    fork_boundary: None,
                },
            ];
            view.text_input
                .update(cx, |input, cx| input.set_text("保留草稿", cx));
            view.timeline_changed();
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();

    let assert_reading = |cx: &mut gpui::VisualTestContext| {
        assert!(cx.debug_bounds("message-bubble-reply").is_none());
        cx.update(|window, cx| {
            let v = view.read(cx);
            let bubble = v.settings_element_layouts["message-bubble-short"].bounds();
            let user_actions = v.settings_element_layouts["message-actions-short"].bounds();
            let reply_actions = v.settings_element_layouts["message-actions-reply"].bounds();
            let bubble_w = f32::from(bubble.size.width);
            assert!(
                bubble_w > 60.0 && bubble_w < 350.0,
                "short bubble {bubble:?}"
            );
            let bubble_right = f32::from(bubble.right());
            let user_right = f32::from(user_actions.right());
            let reply_right = f32::from(reply_actions.right());
            assert!(
                (bubble_right - user_right).abs() < 2.0,
                "bubble {bubble:?} actions {user_actions:?}"
            );
            assert!(
                (user_right - reply_right).abs() < 2.0,
                "user actions {user_actions:?} reply actions {reply_actions:?}"
            );
            assert!(
                user_actions.bottom() <= bubble.top() + px(2.0),
                "user actions must sit above the bubble: actions {user_actions:?} bubble {bubble:?}"
            );
            let code_scroll = &v.settings_element_layouts["reply-code-scroll-0"];
            let table_scroll = &v.settings_element_layouts["reply-table-scroll-1"];
            let code_view = f32::from(code_scroll.bounds().size.width);
            let table_view = f32::from(table_scroll.bounds().size.width);
            assert!(
                f32::from(code_scroll.max_offset().width) > 8.0,
                "code scroll did not overflow: {:?}",
                code_scroll.max_offset()
            );
            assert!(
                f32::from(table_scroll.max_offset().width) > 8.0,
                "table scroll did not overflow: offset {:?} bounds {:?}",
                table_scroll.max_offset(),
                table_scroll.bounds()
            );
            assert!(code_view > 80.0 && code_view <= metrics::TIMELINE_READABLE_WIDTH + 0.5);
            assert!(table_view > 80.0 && table_view <= metrics::TIMELINE_READABLE_WIDTH + 0.5);
            assert!(
                (table_view - code_view - 24.0).abs() < 8.0,
                "code viewport {code_view} should be about 24px inside the table viewport {table_view}"
            );
            let delta = f32::from(code_scroll.bounds().top() - reply_actions.top());
            assert!(
                (-4.0..=48.0).contains(&delta),
                "idle assistant must not reserve an author row, delta {delta}"
            );
            let tree = v.accessibility_tree(window, cx);
            tree.validate().unwrap();
            let user = tree.find("timeline-entry-short").unwrap();
            let reply = tree.find("timeline-entry-reply").unwrap();
            assert_eq!(user.label, t("timeline.you"));
            assert_eq!(reply.label, "Pawork");
            assert_eq!(user.description.as_deref(), Some("fixture-user"));
            assert_eq!(reply.description.as_deref(), Some("fixture-reply"));
            assert!(tree.find("reply-code-0").unwrap().actions.contains(&AxAction::Press));
        });
    };
    let expect_code = |cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(expected_code.clone())
            );
        });
    };
    assert_reading(cx);

    let actions = cx.debug_bounds("message-actions-reply").unwrap();
    cx.simulate_click(actions.center(), Modifiers::none());
    cx.refresh().unwrap();
    cx.run_until_parked();
    let code_click = cx.update(|window, cx| {
        let v = view.read(cx);
        assert!(matches!(v.open_menu, Some(MenuKind::Entry(ref id)) if id == "reply"));
        let actions = v.entry_menu_actions("reply");
        let ix = actions
            .iter()
            .position(|action| {
                matches!(&action.kind, EntryActionKind::Copy(text) if text == &expected_code)
            })
            .expect("code copy row");
        assert_eq!(ix, 1);
        let id = actions[ix].identifier("reply", ix);
        let tree = v.accessibility_tree(window, cx);
        let node = tree.find(&id).expect("code row published after layout");
        gpui::point(
            px(node.bounds.x + node.bounds.width / 2.0),
            px(node.bounds.y + node.bounds.height / 2.0),
        )
    });
    cx.simulate_click(code_click, Modifiers::none());
    cx.run_until_parked();
    expect_code(cx);

    cx.update(|window, cx| {
        view.update(cx, |view, _| {
            assert!(view.open_menu.is_none());
            let focus = view.timeline_entry_action_focus["reply"].clone();
            window.focus(&focus);
        });
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert!(matches!(v.open_menu, Some(MenuKind::Entry(ref id)) if id == "reply"));
        assert!(v.menu_highlight.is_none());
    });
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(view.read(cx).menu_highlight, Some(1)));
    cx.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("sentinel-keyboard".into()));
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("sentinel-keyboard")
        );
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    expect_code(cx);
    cx.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("sentinel-ax".into()));
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("sentinel-ax")
        );
    });
    press_published_ax(cx, &view, "reply-code-0");
    expect_code(cx);

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_models(vec![ModelEntry {
                provider_id: "glm-coding".into(),
                id: "glm-5.2".into(),
                display_name: "GLM 5.2".into(),
                ..ModelEntry::default()
            }]);
            view.projection.settings_providers.providers =
                vec![crate::projection::ProviderAuthStatusEntry {
                    provider_id: "glm-coding".into(),
                    display_name: "GLM Coding".into(),
                    endpoint_label: String::new(),
                    auth_methods: vec!["api_key".into()],
                    credentials: Vec::new(),
                    selection_mode: Default::default(),
                    auth: crate::projection::ProviderAuthState::Connected {
                        method: "api_key".into(),
                        masked_credential: None,
                    },
                    catalog: crate::projection::ProviderCatalogState::FixedFallback {
                        snapshot_label: "test".into(),
                        fetched_at: None,
                    },
                    use_proxy: true,
                }];
            view.projection
                .set_pending_model("glm-coding".into(), "glm-5.2".into());
            cx.notify();
        })
    });
    for (width, height) in [(1440.0, 1024.0), (1080.0, 720.0)] {
        for scale in [
            TextScale::Percent100,
            TextScale::Percent125,
            TextScale::Percent150,
        ] {
            cx.simulate_resize(size(px(width), px(height)));
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.text_scale = scale;
                    window.set_rem_size(px(scale.rem_pixels()));
                    cx.notify();
                })
            });
            cx.refresh().unwrap();
            cx.run_until_parked();
            assert_reading(cx);
            let card = cx.debug_bounds("composer-card").unwrap();
            let model = cx.debug_bounds("composer-model-slot").unwrap();
            let action = cx.debug_bounds("composer-action-slot").unwrap();
            assert!(model.top() >= card.top() && model.bottom() <= card.bottom());
            assert!(action.top() >= card.top() && action.bottom() <= card.bottom());
            assert!(f32::from(model.size.height) >= 36.0, "{model:?}");
            assert!(f32::from(action.size.height) >= 36.0, "{action:?}");
            assert!(f32::from(model.size.width) <= metrics::COMPOSER_MODEL_WIDTH + 0.5);
            assert!(
                model.right() < action.left(),
                "model {model:?} action {action:?}"
            );
            cx.update(|_, cx| {
                assert_eq!(view.read(cx).text_input.read(cx).text(), "保留草稿");
            });
        }
    }

    cx.simulate_resize(size(px(1440.0), px(1024.0)));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.text_scale = TextScale::Percent100;
            window.set_rem_size(px(TextScale::Percent100.rem_pixels()));
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    let slot = cx.debug_bounds("composer-model-slot").unwrap();
    cx.simulate_click(slot.center(), Modifiers::none());
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert!(matches!(v.open_menu, Some(MenuKind::Model)));
        assert!(v.menu_highlight.is_none());
        assert!(v.model_search_query.is_empty());
        assert!(v.model_search_focus.is_focused(window));
        assert_eq!(v.route, AppRoute::Workspace);
        assert!(v.model_menu_row_count() > 0);
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert!(matches!(v.open_menu, Some(MenuKind::Model)));
        assert_eq!(v.route, AppRoute::Workspace);
        assert_eq!(v.text_input.read(cx).text(), "保留草稿");
        assert_eq!(v.projection.active_session_id.as_deref(), Some("task"));
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(
                AxRequest {
                    identifier: "model-search-input".into(),
                    action: AxAction::SetValue,
                    value: Some("没有这个模型".into()),
                },
                window,
                cx,
            );
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert_eq!(v.model_search_query, "没有这个模型");
        assert!(v.menu_highlight.is_none());
        assert_eq!(v.model_menu_row_count(), 0);
        assert_eq!(v.menu_item_count(), 1);
        let tree = v.accessibility_tree(window, cx);
        let empty = tree.find("model-menu-empty").unwrap();
        assert_eq!(empty.label, t("model_search.no_results"));
        assert_ne!(empty.label, t("model_search.no_providers"));
    });
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(view.read(cx).menu_highlight, Some(0)));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert_eq!(v.route, AppRoute::Settings);
        assert_eq!(v.settings_page, SettingsPage::Providers);
        assert_eq!(v.projection.workspace_header_title(), Some("阅读任务"));
        assert_eq!(v.text_input.read(cx).text(), "保留草稿");
        assert_eq!(v.projection.active_session_id.as_deref(), Some("task"));
    });

    press_published_ax(cx, &view, "settings-back");
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(view.route, AppRoute::Workspace);
            view.projection.set_models(Vec::new());
            assert!(view.projection.models_loaded);
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            view.read(cx).model_label(),
            t("composer.model_none_available")
        );
    });
    let slot = cx.debug_bounds("composer-model-slot").unwrap();
    cx.simulate_click(slot.center(), Modifiers::none());
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert!(matches!(v.open_menu, Some(MenuKind::Model)));
        let tree = v.accessibility_tree(window, cx);
        let empty = tree.find("model-menu-empty").unwrap();
        assert_eq!(empty.label, t("composer.model_none_available"));
        assert_ne!(empty.label, t("model_search.no_providers"));
    });
    let manage = cx.debug_bounds("model-menu-settings").unwrap();
    cx.simulate_click(
        gpui::point(manage.left() + px(8.0), manage.center().y),
        Modifiers::none(),
    );
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert_eq!(v.route, AppRoute::Settings);
        assert_eq!(v.settings_page, SettingsPage::Providers);
        assert_eq!(v.text_input.read(cx).text(), "保留草稿");
    });
    press_published_ax(cx, &view, "settings-back");

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(view.route, AppRoute::Workspace);
            view.projection.active_run_id = Some("run-keep".into());
            view.projection.active_runs.push(ActiveRun {
                run_id: "run-keep".into(),
                session_id: "task".into(),
                started_at_ms: 1,
            });
            assert!(view
                .projection
                .timeline
                .iter()
                .all(|entry| entry.run_id.is_none()));
            assert!(!view.can_open_model_menu());
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    let slot = cx.debug_bounds("composer-model-slot").unwrap();
    cx.simulate_click(slot.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert!(v.open_menu.is_none());
        let tree = v.accessibility_tree(window, cx);
        let picker = tree.find("model-picker").unwrap();
        assert!(!picker.enabled);
        assert!(picker.actions.contains(&AxAction::Press));
        assert!(!tree.permits(&AxRequest {
            identifier: "model-picker".into(),
            action: AxAction::Press,
            value: None,
        }));
        assert_eq!(v.text_input.read(cx).text(), "保留草稿");
    });
    press_published_ax(cx, &view, "open-settings");
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert_eq!(v.route, AppRoute::Settings);
        assert!(v
            .accessibility_tree(window, cx)
            .find("run-status")
            .is_none());
        assert_eq!(v.projection.active_run_id.as_deref(), Some("run-keep"));
        assert_eq!(v.projection.active_session_id.as_deref(), Some("task"));
        assert_eq!(v.text_input.read(cx).text(), "保留草稿");
        assert_eq!(
            v.projection
                .timeline
                .iter()
                .map(|entry| entry.event_id.as_str())
                .collect::<Vec<_>>(),
            ["short", "reply"]
        );
    });
    press_published_ax(cx, &view, "settings-back");
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert_eq!(v.route, AppRoute::Workspace);
        assert_eq!(v.projection.workspace_header_title(), Some("阅读任务"));
        assert_eq!(v.projection.active_session_id.as_deref(), Some("task"));
        assert_eq!(v.projection.active_run_id.as_deref(), Some("run-keep"));
        assert_eq!(v.text_input.read(cx).text(), "保留草稿");
        assert_eq!(
            v.projection
                .timeline
                .iter()
                .map(|entry| entry.event_id.as_str())
                .collect::<Vec<_>>(),
            ["short", "reply"]
        );
        let tree = v.accessibility_tree(window, cx);
        assert!(tree.find("run-status").is_some());
        let cancel = tree.find("cancel").unwrap();
        assert!(cancel.enabled, "running cancel stays operable");
        assert!(tree.permits(&AxRequest {
            identifier: "cancel".into(),
            action: AxAction::Press,
            value: None,
        }));
        assert!(tree.find("send").is_none());
    });
    press_published_ax(cx, &view, "cancel");
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert_eq!(v.projection.active_run_id.as_deref(), Some("run-keep"));
        assert_eq!(v.text_input.read(cx).text(), "保留草稿");
        let tree = v.accessibility_tree(window, cx);
        assert!(tree.find("cancel").unwrap().enabled);
        assert!(tree.find("send").is_none());
    });
}

/// GUI3-03：测试平台上语言标签与 Code 回退真实出现；行内 HTTP(S) 点击和
/// 消息菜单打开 / 复制写到 opened_url 与剪贴板。javascript: 与 file: 不进入
/// 打开动作，点在正文上也不分发。长表与 CRLF 沿用 GUI2-02，不在这里重跑。
/// 输入是 GPUI TestAppContext，不是真窗口、系统浏览器或系统剪贴板。
#[gpui::test]
fn markdown_code_labels_and_http_links_dispatch(cx: &mut gpui::TestAppContext) {
    use crate::ui::theme::font::TextScale;
    use crate::ui::timeline_entry::EntryActionKind;
    use gpui::{point, px, size, ClipboardItem, Modifiers, MouseButton};

    let https = "https://example.test/inline";
    let http = "http://docs.example.test/menu";
    let fence = "\u{60}".repeat(3);
    let text = format!(
        "{https}\n\n{fence}rust\nlet a = 1;\n{fence}\n\n{fence}\nbare\n{fence}\n\n[docs]({http})\n\njavascript:alert(1)\nfile:///tmp/a\n"
    );
    let menu = crate::ui::markdown::message_actions(&text);
    let opened: Vec<_> = menu
        .iter()
        .filter(|action| action.open)
        .map(|action| action.content.as_str())
        .collect();
    assert_eq!(opened, vec![https, http]);

    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            std::sync::Arc::new(crate::platform::Platform::new()),
            std::env::temp_dir().join("gui3-03-markdown.sock"),
            None,
            cx,
        )
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        crate::ui::install_keybindings(cx);
        window.set_rem_size(px(TextScale::Percent100.rem_pixels()));
    });
    cx.simulate_resize(size(px(1440.0), px(1024.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.connection = ConnectionState::Connected {
                instance_id: "gui3-03".into(),
            };
            view.projection.sessions = vec![rail_session_summary("task", "阅读任务", None, 1)];
            view.projection.active_session_id = Some("task".into());
            view.projection.timeline.entries = vec![TimelineEntry {
                sequence: 1,
                event_id: "reply".into(),
                timestamp: "fixture-reply".into(),
                run_id: None,
                kind: TimelineEntryKind::AssistantMessage { text },
                fork_boundary: None,
            }];
            view.timeline_changed();
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();

    assert!(cx.debug_bounds("reply-code-lang-1=rust").is_some());
    assert!(cx.debug_bounds("reply-code-lang-1=Code").is_none());
    assert!(cx.debug_bounds("reply-code-lang-2=Code").is_some());
    assert!(cx.debug_bounds("reply-code-lang-2=rust").is_none());
    assert!(cx.opened_url().is_none());

    let click_inline = |cx: &mut gpui::VisualTestContext, at| {
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
    };
    let line_point = |cx: &mut gpui::VisualTestContext, id: &'static str| {
        let bounds = cx.debug_bounds(id).expect("rendered Markdown line");
        point(bounds.origin.x + px(4.0), bounds.center().y)
    };
    let at = line_point(cx, "reply-md-0-0");
    click_inline(cx, at);
    assert_eq!(cx.opened_url().as_deref(), Some(https));

    let at = line_point(cx, "reply-md-4-0");
    click_inline(cx, at);
    let at = line_point(cx, "reply-md-4-1");
    click_inline(cx, at);
    assert_eq!(
        cx.opened_url().as_deref(),
        Some(https),
        "javascript: and file: must not dispatch"
    );
    let at = line_point(cx, "reply-md-3-0");
    click_inline(cx, at);
    assert_eq!(cx.opened_url().as_deref(), Some(http));

    let menu_point = |cx: &mut gpui::VisualTestContext, url: &str, open: bool| {
        let trigger = cx.debug_bounds("message-actions-reply").expect("actions");
        cx.simulate_click(trigger.center(), Modifiers::none());
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(matches!(view.open_menu, Some(MenuKind::Entry(ref id)) if id == "reply"));
            let actions = view.entry_menu_actions("reply");
            assert!(actions.iter().all(|action| {
                !matches!(&action.kind, EntryActionKind::Open(target) if !target.starts_with("http://") && !target.starts_with("https://"))
            }));
            let ix = actions
                .iter()
                .position(|action| match &action.kind {
                    EntryActionKind::Open(target) if open => target == url,
                    EntryActionKind::Copy(target) if !open => target == url,
                    _ => false,
                })
                .expect("menu row");
            let id = actions[ix].identifier("reply", ix);
            let tree = view.accessibility_tree(window, cx);
            let node = tree.find(&id).expect("menu row bounds");
            point(
                px(node.bounds.x + node.bounds.width / 2.0),
                px(node.bounds.y + node.bounds.height / 2.0),
            )
        })
    };
    cx.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("sentinel-menu-copy".into()));
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("sentinel-menu-copy")
        );
    });
    let copy_at = menu_point(cx, https, false);
    cx.simulate_click(copy_at, Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some(https)
        );
    });
    assert_eq!(cx.opened_url().as_deref(), Some(http));
    let open_at = menu_point(cx, https, true);
    cx.simulate_click(open_at, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(cx.opened_url().as_deref(), Some(https));
}

#[gpui::test]
fn reply_actions_copy_exact_content_and_use_closed_turn_boundary(cx: &mut gpui::TestAppContext) {
    use crate::ui::timeline_entry::{fork_target, EntryActionKind};
    use gpui::{px, ClipboardItem};
    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            std::sync::Arc::new(crate::platform::Platform::new()),
            std::env::temp_dir().join("ux02-actions.sock"),
            None,
            cx,
        )
    });
    cx.run_until_parked();
    let text = "| 名称 | 数量 |\n| --- | ---: |\n| 中文 | 2 |\n\n```rust\n  let 中文 = 2;\n```\n[文档](https://example.test/docs)";
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.connection = ConnectionState::Connected {
                instance_id: "ux02".into(),
            };
            view.projection.active_session_id = Some("task".into());
            view.projection.timeline.entries = vec![
                TimelineEntry {
                    sequence: 1,
                    event_id: "reply".into(),
                    kind: TimelineEntryKind::AssistantMessage { text: text.into() },
                    fork_boundary: None,
                    timestamp: "1".into(),
                    run_id: Some("run".into()),
                },
                TimelineEntry {
                    sequence: 2,
                    event_id: "other".into(),
                    kind: TimelineEntryKind::RunState("Completed".into()),
                    fork_boundary: Some(ForkBoundary::Completed),
                    timestamp: "2".into(),
                    run_id: Some("other-run".into()),
                },
            ];
            assert!(fork_target(&view.projection.timeline, "reply").is_none());
            let mut terminal = view.projection.timeline[1].clone();
            terminal.event_id = "terminal".into();
            terminal.run_id = Some("run".into());
            terminal.sequence = 3;
            view.projection.timeline.entries.push(terminal);
            assert_eq!(
                fork_target(&view.projection.timeline, "reply"),
                Some("terminal")
            );
            assert!(view.can_fork_entry("reply"));
            view.timeline_changed();
            view.toggle_menu(MenuKind::Entry("reply".into()), None, cx);
        })
    });
    cx.simulate_resize(gpui::size(px(1440.0), px(1024.0)));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.toggle_menu(MenuKind::Entry("reply".into()), None, cx);
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let actions = view.entry_menu_actions("reply");
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            let copy = actions[0].identifier("reply", 0);
            assert!(
                tree.find(&copy).is_some(),
                "copy action is visible in actual menu layout"
            );
            let code_copies = crate::ui::markdown::message_code_copies("reply", text);
            assert_eq!(code_copies.len(), 1);
            let code_id = code_copies[0].identifier.clone();
            let code_ax = tree.find(&code_id).expect("code copy button in body AX");
            assert!(code_ax.actions.contains(&AxAction::Press));
            assert!(tree.find("reply-link-2-0-open").is_none());
            assert!(tree.find("reply-link-2-0-copy").is_none());
            let open_ix = actions
                .iter()
                .position(|a| {
                    matches!(
                        &a.kind,
                        EntryActionKind::Open(url) if url == "https://example.test/docs"
                    )
                })
                .unwrap();
            assert!(actions[open_ix].label.contains("1"));
            assert!(actions[open_ix].label.contains("example.test"));
            assert!(
                tree.find(&actions[open_ix].identifier("reply", open_ix))
                    .is_some(),
                "link open action lives in the entry menu"
            );
            let request = AxRequest {
                identifier: copy,
                action: AxAction::Press,
                value: None,
            };
            assert!(tree.permits(&request));
            view.handle_accessibility_request(request, window, cx);
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(text.into())
            );
            assert!(view.open_menu.is_none());
            let tree = view.accessibility_tree(window, cx);
            assert!(tree
                .find(&code_id)
                .is_some_and(|node| node.actions.contains(&AxAction::Press)));
            view.handle_accessibility_request(
                AxRequest {
                    identifier: code_id,
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("  let 中文 = 2;\n".into())
            );
            let code_ix = actions
                .iter()
                .position(
                    |a| matches!(&a.kind, EntryActionKind::Copy(s) if s == "  let 中文 = 2;\n"),
                )
                .unwrap();
            view.toggle_menu(MenuKind::Entry("reply".into()), None, cx);
            view.activate_menu_item(MenuKind::Entry("reply".into()), code_ix, window, cx);
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("  let 中文 = 2;\n".into())
            );
            view.projection.connection = ConnectionState::Disconnected {
                reason: "test".into(),
            };
            assert!(!view.can_fork_entry("reply"));
            cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
            view.activate_entry_action("reply", actions.len() - 1, window, cx);
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("unchanged".into())
            );
        })
    });
}

/// 迟到发送失败只显示在原任务，保留草稿，成功回执清掉原错误。
#[gpui::test]
fn composer_send_failure_stays_with_originating_draft(cx: &mut gpui::TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            std::sync::Arc::new(crate::platform::Platform::new()),
            std::env::temp_dir().join("composer-send-failure.sock"),
            None,
            cx,
        )
    });
    cx.run_until_parked();
    let reason = "model_disabled: 当前模型已被禁用，请选择可用模型";
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.composer_sending = true;
            view.composer_send_target = Some("original".into());
            view.projection.active_session_id = Some("other".into());
            view.text_input
                .update(cx, |input, cx| input.set_text("保留草稿", cx));
            view.handle_controller_event(
                crate::controller::ControllerEvent::OperationFailed {
                    action: "send message".into(),
                    reason: reason.into(),
                },
                cx,
            );
            assert_eq!(view.text_input.read(cx).text(), "保留草稿");
            assert!(view
                .accessibility_tree(window, cx)
                .find("composer-send-error")
                .is_none());
            view.projection.active_session_id = Some("original".into());
            cx.notify();
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let tree = view.accessibility_tree(window, cx);
            let error = tree.find("composer-send-error").unwrap();
            assert_eq!(error.value.as_deref(), Some(reason));
            assert!(error.bounds.width > 0.0 && error.bounds.height > 0.0);
            view.handle_controller_event(
                crate::controller::ControllerEvent::MessageSent {
                    session_id: "original".into(),
                    run_id: "run".into(),
                    text: "accepted".into(),
                },
                cx,
            );
            assert!(view
                .accessibility_tree(window, cx)
                .find("composer-send-error")
                .is_none());

            // 首页建会话失败：实际切换任务会将首页草稿存回 None 槽，
            // 迟到失败跟随该草稿，不能污染已打开任务或丢失首页文本。
            view.projection.active_session_id = None;
            view.text_input
                .update(cx, |input, cx| input.set_text("首页草稿", cx));
            view.composer_sending = true;
            view.composer_send_target = None;
            view.pending_home_send = Some(crate::ui::PendingHomeSend {
                text: "首页草稿".into(),
                model: None,
                effort: None,
                options: Default::default(),
            });
            view.open_session("other".into(), cx);
            assert_eq!(view.no_session_draft, "首页草稿");
            assert!(view.text_input.read(cx).text().is_empty());
            view.handle_controller_event(
                crate::controller::ControllerEvent::OperationFailed {
                    action: "create session".into(),
                    reason: reason.into(),
                },
                cx,
            );
            assert!(view.current_composer_send_error().is_none());
            assert!(view.pending_home_send.is_none());
            view.stash_composer_draft(cx);
            view.projection.active_session_id = None;
            view.restore_composer_draft(cx);
            assert_eq!(view.text_input.read(cx).text(), "首页草稿");
            assert_eq!(view.current_composer_send_error(), Some(reason));
        });
    });
}

/// UI-4：实际 GPUI 布局与 AX 操作区对齐，覆盖窄窗、大字号、长草稿。
#[gpui::test]
fn composer_layout_keeps_controls_in_card_and_ax_aligned(cx: &mut gpui::TestAppContext) {
    use crate::ui::theme::font::TextScale;
    use gpui::{px, size};
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            platform,
            std::env::temp_dir().join("ui4-layout.sock"),
            None,
            cx,
        )
    });
    for (width, height, scale, lines) in [
        (1440.0, 1024.0, TextScale::Percent100, 1),
        (1080.0, 720.0, TextScale::Percent125, 3),
        (1080.0, 720.0, TextScale::Percent150, 80),
    ] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.text_scale = scale;
                window.set_rem_size(px(scale.rem_pixels()));
                view.text_input.update(cx, |input, cx| {
                    input.reset_text(&vec!["验收草稿自然换行".repeat(20); lines].join("\n"), cx);
                });
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        let workspace = cx.debug_bounds("shell-workspace").unwrap();
        let card = cx.debug_bounds("composer-card").unwrap();
        let meta = cx.debug_bounds("composer-meta").unwrap();
        let model = cx.debug_bounds("composer-model-slot").unwrap();
        let action = cx.debug_bounds("composer-action-slot").unwrap();
        assert!(card.size.width <= px(metrics::TIMELINE_READABLE_WIDTH));
        assert!(card.left() >= workspace.left() + px(metrics::COMPOSER_OUTER_X));
        assert!(card.right() <= workspace.right() - px(metrics::COMPOSER_OUTER_X));
        assert!(meta.bottom() <= workspace.bottom() - px(metrics::COMPOSER_OUTER_BOTTOM));
        assert!(card.size.height <= px(metrics::COMPOSER_PANEL_MAX_HEIGHT));
        assert!(model.right() < action.left());
        cx.update(|window, cx| {
            let tree = view.read(cx).accessibility_tree(window, cx);
            for (id, actual) in [("model-picker", model), ("send", action)] {
                let node = tree.find(id).unwrap();
                for (expected, measured) in [
                    (node.bounds.x, f32::from(actual.left())),
                    (node.bounds.y, f32::from(actual.top())),
                    (node.bounds.width, f32::from(actual.size.width)),
                    (node.bounds.height, f32::from(actual.size.height)),
                ] {
                    assert!(
                        (expected - measured).abs() < 1.0,
                        "{id} at {scale:?}: AX {expected}, render {measured}; card={card:?} workspace={workspace:?} input={:?}", view.read(cx).text_input.read(cx).viewport_height()
                    );
                }
            }
            let picker = tree.find("model-picker").unwrap();
            assert!(
                picker.bounds.height >= 36.0,
                "model chip hit target at {scale:?}: {:?}",
                picker.bounds
            );
            assert!(f32::from(model.size.height) >= 36.0);
            assert!(
                f32::from(model.size.width) <= metrics::COMPOSER_MODEL_WIDTH + 0.5,
                "model chip must not exceed max width at {scale:?}: {:?}",
                model.size.width
            );
            assert!(tree.find("composer-file-tools-hint").is_none());
            assert!(
                tree.find("composer-context").is_none(),
                "unknown context window must not publish composer-context"
            );
            let project_task = tree.find("composer-project-task").unwrap();
            assert!(
                !project_task.enabled,
                "offline project entry stays reachable but gated"
            );
            assert!(!tree.find("send").unwrap().enabled, "offline never sends");
        });
    }
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.text_scale = TextScale::Percent100;
            window.set_rem_size(px(TextScale::Percent100.rem_pixels()));
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.projection.set_models(vec![ModelEntry {
                provider_id: "glm-coding".into(),
                id: "glm-5.2".into(),
                display_name: "GLM 5.2".into(),
                context_window_tokens: Some(128_000),
                ..ModelEntry::default()
            }]);
            view.projection
                .set_pending_model("glm-coding".into(), "glm-5.2".into());
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    let hugged = cx.debug_bounds("composer-model-slot").unwrap();
    assert!(
        f32::from(hugged.size.width) < metrics::COMPOSER_MODEL_WIDTH,
        "short model names must hug the selected label, got {:?}",
        hugged.size.width
    );
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        let picker = tree.find("model-picker").unwrap();
        assert!(
            (picker.bounds.width - f32::from(hugged.size.width)).abs() < 1.0,
            "AX model-picker must follow hugged slot: AX {} render {:?}",
            picker.bounds.width,
            hugged.size.width
        );
        let context = tree.find("composer-context").unwrap();
        assert_eq!(context.value.as_deref(), Some("Context · — / 128000"));
        let meta = view.read(cx).composer_layouts["composer-meta"].bounds();
        assert!(
            context.bounds.y >= f32::from(meta.top())
                && context.bounds.y + context.bounds.height <= f32::from(meta.bottom()) + 1.0
        );
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.models[0].context_window_tokens = Some(0);
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        assert!(
            tree.find("composer-context").is_none(),
            "zero-token sentinel is unknown and must not draw ContextMeter"
        );
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.models[0].context_window_tokens = None;
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        assert!(
            tree.find("composer-context").is_none(),
            "missing catalog window must not draw ContextMeter"
        );
    });
    // An expanded image must not push the editor/footer outside the card.
    cx.simulate_resize(size(px(1440.0), px(1024.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.text_input
                .update(cx, |input, cx| input.reset_text("Image review", cx));
            view.composer_options
                .entry(None)
                .or_default()
                .attachments
                .push(crate::controller::ComposerAttachment {
                    id: "layout-image".into(),
                    name: "drawing.png".into(),
                    bytes: std::sync::Arc::new(Vec::new()),
                    image: true,
                });
            view.composer_attachment_preview = Some((None, "layout-image".into()));
            cx.notify();
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    let card = cx.debug_bounds("composer-card").unwrap();
    let action = cx.debug_bounds("composer-action-slot").unwrap();
    let meta = cx.debug_bounds("composer-meta").unwrap();
    assert!(
        action.bottom() < card.bottom(),
        "preview must leave the send control inside the card"
    );
    assert!(
        card.bottom() <= meta.top(),
        "preview must not overlap workspace context"
    );
}

/// UX-03：项目筛选不重绑任务，项目新建入口、限制和上下文共用可换行的元信息。
#[gpui::test]
fn project_task_guidance_preserves_context_and_wraps(cx: &mut gpui::TestAppContext) {
    use crate::projection::{SessionSummary, WorkspaceSummary};
    use crate::ui::theme::font::TextScale;
    use gpui::{px, size};
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            platform,
            std::env::temp_dir().join("ux03-layout.sock"),
            None,
            cx,
        )
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.projection.workspaces = vec![WorkspaceSummary {
                id: "project".into(),
                name: "Example".into(),
            }];
            view.projection.sessions = vec![SessionSummary {
                session_id: "old".into(),
                title: "Draft".into(),
                updated_at_ms: 1,
                workspace_id: None,
                parent_branch_id: None,
                forked_from_event_id: None,
                active: true,
                unstarted: false,
            }];
            view.projection.active_session_id = Some("old".into());
            view.projection.set_models(vec![ModelEntry {
                provider_id: "glm-coding".into(),
                id: "glm-5.2".into(),
                display_name: "GLM 5.2".into(),
                context_window_tokens: Some(128_000),
                ..ModelEntry::default()
            }]);
            view.projection
                .set_pending_model("glm-coding".into(), "glm-5.2".into());
            view.text_input
                .update(cx, |input, cx| input.reset_text("保留原任务草稿", cx));
            view.on_select_scope(Some("project".into()), window, cx);
            assert!(view.active_task_hidden_by_filter());
            assert!(view.composer_workspace_no_project());
            assert_eq!(view.projection.sessions[0].workspace_id, None);
            assert_eq!(view.text_input.read(cx).text(), "保留原任务草稿");
            view.on_project_task_menu(None, window, cx);
            assert_eq!(
                view.project_menu_options(),
                vec![(Some("project".into()), "Example".into())]
            );
            assert_eq!(view.menu_item_count(), 2);
            view.close_menu_and_focus_trigger(MenuKind::ProjectTask, window, cx);
            assert!(view.project_task_focus.is_focused(window));
            view.on_select_scope(None, window, cx);
            assert!(!view.active_task_hidden_by_filter());
        })
    });
    for scale in [
        TextScale::Percent100,
        TextScale::Percent125,
        TextScale::Percent150,
    ] {
        for width in [1440.0, 1080.0] {
            cx.simulate_resize(size(px(width), px(900.0)));
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.text_scale = scale;
                    window.set_rem_size(px(scale.rem_pixels()));
                    cx.notify();
                })
            });
            cx.refresh().unwrap();
            cx.run_until_parked();
            cx.update(|window, cx| {
                let view = view.read(cx);
                let tree = view.accessibility_tree(window, cx);
                let meta = view.composer_layouts["composer-meta"].bounds();
                assert!(tree.find("composer-file-tools-hint").is_none());
                let entry = tree.find("composer-project-task").unwrap();
                assert_eq!(entry.label, t("composer.bind_project"));
                assert!(entry.enabled);
                assert_eq!(
                    tree.find("composer-workspace").unwrap().value.as_deref(),
                    Some(t("composer.no_project_chip"))
                );
                assert_eq!(
                    tree.find("composer-context").unwrap().value.as_deref(),
                    Some("Context · — / 128000")
                );
                let bounds: Vec<_> = [
                    "composer-workspace",
                    "composer-project-task",
                    "composer-context",
                ]
                .iter()
                .map(|id| {
                    let node = tree.find(id).unwrap();
                    assert!(node.bounds.width > 0.0 && node.bounds.height > 0.0, "{id}");
                    assert!(
                        node.bounds.x >= f32::from(meta.left()) - 1.0
                            && node.bounds.x + node.bounds.width <= f32::from(meta.right()) + 1.0,
                        "{id}: {:?} meta={meta:?}",
                        node.bounds
                    );
                    assert!(
                        node.bounds.y >= f32::from(meta.top()) - 1.0
                            && node.bounds.y + node.bounds.height <= f32::from(meta.bottom()) + 1.0,
                        "{id}"
                    );
                    node.bounds
                })
                .collect();
                for (i, a) in bounds.iter().enumerate() {
                    for b in &bounds[i + 1..] {
                        assert!(
                            a.x + a.width <= b.x + 1.0
                                || b.x + b.width <= a.x + 1.0
                                || a.y + a.height <= b.y + 1.0
                                || b.y + b.height <= a.y + 1.0,
                            "overlap at {width} {scale:?}"
                        );
                    }
                }
            });
        }
    }
    cx.refresh().unwrap();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let tree = view.accessibility_tree(window, cx);
            let request = AxRequest {
                identifier: "composer-project-task".into(),
                action: AxAction::Press,
                value: None,
            };
            assert!(
                tree.permits(&request),
                "bind entry Press opens project menu"
            );
            view.handle_accessibility_request(request, window, cx);
            assert_eq!(view.open_menu, Some(MenuKind::ProjectTask));
            view.close_menu_and_focus_trigger(MenuKind::ProjectTask, window, cx);
        })
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            window.focus(&view.project_task_focus);
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.simulate_keystrokes("enter");
    cx.update(|_, cx| assert_eq!(view.read(cx).open_menu, Some(MenuKind::ProjectTask)));
    cx.simulate_keystrokes("enter");
    cx.update(|window, cx| {
        let current = view.read(cx);
        assert!(
            current.open_menu.is_none(),
            "second Enter confirms selection"
        );
        assert_eq!(current.text_input.read(cx).text(), "保留原任务草稿");
        assert_eq!(current.scope_workspace_id.as_deref(), Some("project"));
        assert_eq!(current.projection.sessions.len(), 1);
        assert_eq!(current.projection.active_session_id.as_deref(), Some("old"));
        assert_eq!(current.projection.sessions[0].workspace_id, None);
        let tree = current.accessibility_tree(window, cx);
        assert_eq!(
            tree.find("composer-workspace").unwrap().value.as_deref(),
            Some(t("composer.no_project_chip"))
        );
        assert_eq!(
            tree.find("composer-filter-hint").unwrap().value.as_deref(),
            Some(t("rail.task_hidden"))
        );
        assert_eq!(
            tree.find("composer-project-task").unwrap().label,
            t("composer.bind_project")
        );
        assert!(tree.find("composer-file-tools-hint").is_none());
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.status_hint = Some("keep status".into());
            view.projection.sessions[0].workspace_id = Some("project".into());
            assert!(!view.composer_file_tools_unavailable_visible());
            assert!(!view.composer_project_task_visible());
            assert_eq!(view.composer_notes(), Vec::<(&str, String)>::new());
            assert_eq!(view.status_bar_trailing_text(), "keep status");
            let tree = view.accessibility_tree(window, cx);
            assert!(tree.find("composer-file-tools-hint").is_none());
            assert!(tree.find("composer-project-task").is_none());
            assert!(tree.find("composer-status-hint").is_none());
            assert_eq!(
                tree.find("composer-context").unwrap().value.as_deref(),
                Some("Context · — / 128000")
            );
            let project_chip = t("composer.workspace_scope").replace("{}", "Example");
            assert_eq!(
                tree.find("composer-workspace").unwrap().value.as_deref(),
                Some(project_chip.as_str())
            );
            view.projection.models[0].context_window_tokens = Some(0);
            let tree = view.accessibility_tree(window, cx);
            assert!(tree.find("composer-context").is_none());
            assert_eq!(
                tree.find("composer-workspace").unwrap().value.as_deref(),
                Some(project_chip.as_str())
            );
            view.projection.active_session_id = None;
            assert!(
                view.composer_project_task_visible(),
                "empty state offers project creation"
            );
            let tree = view.accessibility_tree(window, cx);
            assert_eq!(
                tree.find("composer-workspace").unwrap().value.as_deref(),
                Some(project_chip.as_str())
            );
            let entry = tree.find("composer-project-task").unwrap();
            assert_eq!(entry.label, t("composer.project_task"));
            assert!(entry.enabled);
            assert!(tree.find("composer-context").is_none());
            assert!(tree.find("composer-file-tools-hint").is_none());
            assert!(tree.find("composer-filter-hint").is_none());
        })
    });
}

#[test]
fn dynamic_identifier_is_stable_and_collision_resistant_for_escape_marker() {
    assert_eq!(dynamic_identifier("session", "abc-1"), "session-abc-1");
    assert_eq!(dynamic_identifier("session", "a b"), "session-a_20b");
    assert_eq!(dynamic_identifier("session", "a_20b"), "session-a_5f20b");
    assert_ne!(
        dynamic_identifier("session", "a b"),
        dynamic_identifier("session", "a_20b")
    );
}

#[test]
fn tail_chars_preserves_utf8_boundaries() {
    assert_eq!(tail_chars("abc", 8), "abc");
    assert_eq!(tail_chars("一二三四", 2), "…三四");
}

#[test]
fn rail_project_identifiers_are_bucket_scoped_and_projects_mode_stable() {
    let today = rail_project_identifier(Some(DateBucket::Today), "ws");
    let earlier = rail_project_identifier(Some(DateBucket::Earlier), "ws");
    let projects_mode = rail_project_identifier(None, "ws");
    assert_ne!(today, earlier);
    assert_ne!(today, projects_mode);
    assert_ne!(earlier, projects_mode);
    // Projects 模式 identifier 与既有 U2 定位口径保持 project-{key}。
    assert_eq!(projects_mode, "project-ws");
    assert_eq!(rail_project_add_identifier(None, "ws"), "project-add-ws");
    assert_eq!(
        rail_project_count_identifier(None, "ws"),
        "project-count-ws"
    );
}

/// R3 Wave A：会话行 AX description 携带可见状态点的状态词；无 live
/// 状态保持中性「Session」（不伪造终态）；R3 Wave B 增 Blocked 与
/// unread 语义词。
#[test]
fn session_ax_description_carries_live_status_word() {
    assert_eq!(
        session_status_description(Some(SessionLiveStatus::NeedsInput), false),
        "Needs input"
    );
    assert_eq!(
        session_status_description(Some(SessionLiveStatus::Running), false),
        "Running"
    );
    assert_eq!(
        session_status_description(Some(SessionLiveStatus::Blocked), false),
        "Blocked"
    );
    assert_eq!(session_status_description(None, false), "Session");
    assert_eq!(session_status_description(None, true), "Session · Unread");
    assert_eq!(
        session_status_description(Some(SessionLiveStatus::NeedsInput), true),
        "Needs input · Unread"
    );
}

/// GUI 1.20：Activity 子代理行使用真实列表布局，滚动裁剪与 Open
/// changes 命中区互不重叠。
#[gpui::test]
fn activity_subagent_ax_uses_list_bounds_and_clips_scrolling(cx: &mut gpui::TestAppContext) {
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("gui120-activity-subagents.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(1024.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.active_session_id = Some("parent".into());
            view.projection.subagent_activity.session_id = Some("parent".into());
            view.projection.subagent_activity.agents = (0..7)
                .map(|ix| crate::projection::SubagentInfo {
                    agent_id: format!("child-{ix}"),
                    session_id: format!("child-{ix}"),
                    parent_run_id: "parent-run".into(),
                    title: format!("Agent {ix}"),
                    provider_id: "test-provider".into(),
                    model_id: "test-model".into(),
                    status: "running".into(),
                    result: None,
                    effort: None,
                })
                .collect();
            view.toggle_menu(MenuKind::Activity, None, cx);
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();

    let open_changes = cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        let rows: Vec<_> = tree
            .find("activity-popover")
            .unwrap()
            .children
            .iter()
            .filter(|node| node.identifier.starts_with("activity-subagent-"))
            .collect();
        assert_eq!(rows.len(), 5);
        let open_changes = tree.find("activity-open-changes").unwrap();
        for row in rows {
            assert!(!ax_rects_overlap(row.bounds, open_changes.bounds));
        }
        open_changes.bounds
    });
    assert!(open_changes.width > 0.0 && open_changes.height > 0.0);

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.activity_subagent_scroll
                .set_offset(gpui::point(gpui::px(0.0), gpui::px(-20.0)));
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        assert!(tree.find("activity-subagent-child-0").is_none());
        assert!(tree.find("activity-subagent-child-4").is_some());
        assert!(tree.find("activity-subagent-child-5").is_some());
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let state = &mut view.projection.subagent_activity;
            let old = state.begin_loading("parent");
            let current = state.begin_loading("parent");
            state.apply_failed("parent", old, "stale".into());
            assert!(state.loading && state.error.is_none());
            state.apply_failed("parent", current, "retry me".into());
            assert_eq!(state.agents.len(), 7);
            assert_eq!(state.error.as_deref(), Some("retry me"));
            view.projection.connection = ConnectionState::Connected {
                instance_id: "test".into(),
            };
            view.projection
                .subagent_conversation
                .select_agent("child-0");
            assert!(view.can_cancel_subagent());
            view.projection.subagent_activity.cancelling = Some("child-0".into());
            assert!(!view.can_cancel_subagent());
            view.handle_controller_event(
                crate::controller::ControllerEvent::SubagentCancelFinished {
                    session_id: "parent".into(),
                    agent_id: "child-0".into(),
                    result: Ok(()),
                },
                cx,
            );
            assert_eq!(
                view.projection.subagent_activity.cancelling.as_deref(),
                Some("child-0"),
                "Accepted is not a terminal state"
            );
            let state = &mut view.projection.subagent_activity;
            let request = state.begin_loading("parent");
            let mut data = pawork_client::SubagentListData {
                agents: state.agents.clone(),
            };
            assert!(state.apply_loaded("parent", request, data.clone()));
            assert!(state.cancelling.is_some());
            data.agents[0].status = "cancelled".into();
            let terminal = state.begin_loading("parent");
            assert!(!state.apply_loaded("parent", request, data.clone()));
            assert!(
                state.cancelling.is_some(),
                "stale terminal cannot clear pending"
            );
            assert!(state.apply_loaded("parent", terminal, data));
            assert!(state.cancelling.is_none());
            // A finished older child can leave the bounded display list.
            state.cancelling = Some("pruned-child".into());
            let request = state.begin_loading("parent");
            let data = pawork_client::SubagentListData {
                agents: state.agents.clone(),
            };
            assert!(state.apply_loaded("parent", request, data));
            assert!(
                state.cancelling.is_none(),
                "a missing row must not lock other stops"
            );
            // A failed cancellation can be retried.
            state.agents[0].status = "running".into();
            state.cancelling = Some("child-0".into());
            view.handle_controller_event(
                crate::controller::ControllerEvent::SubagentCancelFinished {
                    session_id: "parent".into(),
                    agent_id: "child-0".into(),
                    result: Err("retry cancellation".into()),
                },
                cx,
            );
            assert!(view.can_cancel_subagent());
            assert_eq!(
                view.projection.subagent_activity.error.as_deref(),
                Some("retry cancellation")
            );
            view.inspector_open = true;
            view.inspector_motion.width(
                true,
                true,
                std::time::Instant::now() - std::time::Duration::from_secs(1),
            );
            view.inspector_tab = InspectorTab::Subagent;
            view.close_open_menu(cx);
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        let stop = tree
            .find("subagent-cancel")
            .expect("running child exposes Stop");
        assert!(stop.enabled && stop.bounds.width > 0.0);
        assert!(tree.find("subagent-refresh").unwrap().enabled);
    });
}

/// P4 片 3：Timeline 行 rect 相邻不重叠、行间距与 row_top_gap 一致，
/// 行高按内容公式化（工具摘要 36；用户卡额外包含 40px 内边距）。
#[test]
fn timeline_row_layouts_stack_with_content_heights_and_gaps() {
    use crate::projection::{TimelineEntry, TimelineEntryKind, TimelineRow};
    use crate::ui::timeline::{
        row_top_gap, timeline_following_window, timeline_row_height, timeline_stack_height,
        timeline_visible_item_tops,
    };

    fn entry(seq: u64, kind: TimelineEntryKind) -> TimelineEntry {
        TimelineEntry {
            sequence: seq,
            event_id: format!("e{seq}"),
            kind,
            fork_boundary: None,
            timestamp: "1800000000000".into(),
            run_id: None,
        }
    }
    let timeline = vec![
        entry(
            1,
            TimelineEntryKind::UserMessage {
                text: "Plan:".into(),
            },
        ),
        entry(
            2,
            TimelineEntryKind::ToolCall {
                name: "read".into(),
                status: "succeeded".into(),
                detail: None,
                arguments: None,
            },
        ),
        entry(
            3,
            TimelineEntryKind::ToolCall {
                name: "bash".into(),
                status: "running".into(),
                detail: None,
                arguments: None,
            },
        ),
        entry(4, TimelineEntryKind::RunState("Running".into())),
    ];
    let rows = vec![
        TimelineRow::Message { entry_index: 0 },
        TimelineRow::ToolGroup {
            entry_indices: vec![1, 2],
        },
        TimelineRow::RunPhase { entry_index: 3 },
    ];
    let layouts: Vec<(f32, f32)> = rows
        .iter()
        .map(|row| {
            (
                row_top_gap(row),
                timeline_row_height(
                    row,
                    &timeline,
                    618.0,
                    16.0,
                    &std::collections::HashSet::new(),
                    false,
                    None,
                ),
            )
        })
        .collect();
    // 默认仅摘要；显式展开增加两条工具行，字号影响正文高度。
    assert_eq!(layouts[0].1, 86.0);
    let idle_assistant = vec![entry(
        9,
        TimelineEntryKind::AssistantMessage { text: "Hi".into() },
    )];
    assert_eq!(
        timeline_row_height(
            &TimelineRow::Message { entry_index: 0 },
            &idle_assistant,
            618.0,
            16.0,
            &std::collections::HashSet::new(),
            false,
            None,
        ),
        26.0,
        "idle assistant must not reserve the author row"
    );
    assert!(
        timeline_stack_height(
            &[TimelineRow::Message { entry_index: 0 }],
            &idle_assistant,
            618.0,
            16.0,
            &std::collections::HashSet::new(),
            false,
            None,
            None,
        ) < 200.0
    );
    assert_eq!(layouts[1].1, metrics::TOOL_GROUP_HEADER_HEIGHT);
    assert_eq!(layouts[2].1, 19.0);
    let summary_rows = vec![
        ToolRowView::from_facts("read", "succeeded", None, None),
        ToolRowView::from_facts("bash", "running", None, None),
    ];
    assert_eq!(
        tool_group_summary(&summary_rows),
        "2 tools · read · bash · 1 completed · 1 running"
    );
    let expanded = std::collections::HashSet::from(["e2".to_string()]);
    assert_eq!(
        timeline_row_height(&rows[1], &timeline, 618.0, 16.0, &expanded, false, None),
        metrics::TOOL_GROUP_HEADER_HEIGHT
            + crate::ui::timeline_entry::tool_row_height(&summary_rows[0], 618.0, 16.0)
            + crate::ui::timeline_entry::tool_row_height(&summary_rows[1], 618.0, 16.0)
    );
    assert_eq!(layouts[1].0, metrics::TOOL_GROUP_TOP_GAP);
    assert_eq!(layouts[2].0, metrics::MSG_ENTRY_GAP);
    let tops = timeline_visible_item_tops(&layouts, 100.0, 400.0, 0, 0.0);
    assert_eq!(tops[0], (0, 100.0));
    for pair in tops.windows(2) {
        assert!(pair[0].1 + layouts[pair[0].0].1 <= pair[1].1);
    }
    assert_eq!(
        timeline_row_height(
            &rows[0],
            &timeline,
            618.0,
            20.0,
            &std::collections::HashSet::new(),
            false,
            None,
        ),
        93.0
    );
    assert_eq!(timeline_following_window(&layouts, 400.0), (0, 0.0));
    let (start, offset) = timeline_following_window(&layouts, 80.0);
    assert!(start > 0 || offset > 0.0);
    let tail = timeline_visible_item_tops(&layouts, 100.0, 80.0, start, offset);
    assert_eq!(tail.last().map(|item| item.0), Some(2));
}

/// P4 片 2F（D2）：审批卡 AX 位置按内容流推导（渲染为 timeline list
/// 末项）——短内容卡顶 = 内容顶 + Σ行 + MSG_ENTRY_GAP，不贴视口底；
/// 内容+卡超出视口（跟随态滚到底）才贴底；按钮行随卡底同步移动。
#[test]
fn approval_card_ax_position_follows_content_flow_until_overflow() {
    use crate::ui::timeline::{timeline_following_window, timeline_visible_item_tops};

    let frame = AxRect::new(288.0, 56.0, 712.0, 800.0);
    let content_top = frame.y + metrics::TIMELINE_TOP_GAP;
    let viewport_height = frame.height - metrics::TIMELINE_TOP_GAP;
    let card_height = approval_card_height("bash", "Use bash", None, 618.0, 16.0);
    // 短内容：approval 与 rows 组成同一 item 序列，全量远小于视口。
    let short = vec![
        (0.0f32, 62.0f32),
        (metrics::MSG_ENTRY_GAP, 104.0),
        (metrics::MSG_ENTRY_GAP, card_height),
    ];
    let flow_top = content_top + 62.0 + metrics::MSG_ENTRY_GAP + 104.0 + metrics::MSG_ENTRY_GAP;
    let short_window = timeline_following_window(&short, viewport_height);
    assert_eq!(short_window, (0, 0.0));
    let short_tops = timeline_visible_item_tops(
        &short,
        content_top,
        viewport_height,
        short_window.0,
        short_window.1,
    );
    let short_y = short_tops[2].1;
    assert_eq!(short_y, flow_top);
    assert!(short_y + card_height <= frame.y + frame.height);
    // 空时间线：卡即首项，无上间距。
    let empty = vec![(0.0, card_height)];
    assert_eq!(
        timeline_visible_item_tops(&empty, content_top, viewport_height, 0, 0.0)[0].1,
        content_top
    );
    // 长内容：跟随态保留首个部分可见项，approval 末项贴视口底。
    let tall = vec![
        (0.0f32, 700.0f32),
        (metrics::MSG_ENTRY_GAP, 300.0),
        (metrics::MSG_ENTRY_GAP, card_height),
    ];
    let tall_window = timeline_following_window(&tall, viewport_height);
    let tall_tops = timeline_visible_item_tops(
        &tall,
        content_top,
        viewport_height,
        tall_window.0,
        tall_window.1,
    );
    let pinned_y = frame.y + frame.height - card_height;
    assert_eq!(tall_tops.last(), Some(&(2, pinned_y)));
    // 脱钩读史时 approval 在视口外，不得发布不可见审批动作。
    let detached = timeline_visible_item_tops(&tall, content_top, viewport_height, 0, 0.0);
    assert!(detached.iter().all(|(ix, _)| *ix != 2));
}

/// UI-2：真实 hover / Tab / click 驱动非当前会话动作，不先打开会话。
#[gpui::test]
fn session_actions_follow_hover_and_keyboard_without_opening(cx: &mut gpui::TestAppContext) {
    use gpui::{prelude::*, px, AppContext, Modifiers};
    struct RailHost(gpui::Entity<AppView>);
    impl gpui::Render for RailHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let rail = self
                .0
                .update(cx, |view, cx| view.sidebar_element(px(288.0), window, cx));
            gpui::div()
                .size_full()
                .flex()
                .child(rail)
                .on_key_down(cx.listener(|host, event, window, cx| {
                    host.0
                        .update(cx, |view, cx| view.handle_root_key(event, window, cx));
                }))
        }
    }
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let (host, cx) = cx.add_window_view(|_, cx| {
        RailHost(cx.new(|cx| {
            AppView::new(
                platform,
                std::env::temp_dir().join("ui2-rail.sock"),
                None,
                cx,
            )
        }))
    });
    let view = cx.update(|_, cx| host.read(cx).0.clone());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.projection.sessions = ["current", "other"]
                .map(|id| crate::projection::SessionSummary {
                    session_id: id.into(),
                    title: format!("{id} long session title"),
                    updated_at_ms: crate::ui::now_unix_ms(),
                    workspace_id: None,
                    parent_branch_id: None,
                    forked_from_event_id: None,
                    active: id == "current",
                    unstarted: false,
                })
                .into();
            view.projection.active_session_id = Some("current".into());
            window.focus(&view.scope_focus);
            cx.notify();
        })
    });
    cx.simulate_resize(gpui::size(px(1440.0), px(1024.0)));
    cx.refresh().unwrap();
    cx.run_until_parked();
    let row_bounds = cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        assert!(tree.find(&session_rename_identifier("other")).is_none());
        tree.find(&session_identifier("other")).unwrap().bounds
    });
    cx.simulate_mouse_move(
        gpui::point(px(row_bounds.x + 30.0), px(row_bounds.y + 22.0)),
        None,
        Modifiers::none(),
    );
    cx.refresh().unwrap();
    cx.run_until_parked();
    let rename_bounds = cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        assert_eq!(
            view.projection.active_session_id.as_deref(),
            Some("current")
        );
        assert_eq!(
            tree.find(&session_identifier("other")).unwrap().bounds,
            row_bounds
        );
        let rename = tree
            .find(&session_rename_identifier("other"))
            .expect("hover reveals rename");
        assert!(rename.enabled);
        rename.bounds
    });
    cx.simulate_mouse_move(
        gpui::point(px(rename_bounds.x + 16.0), px(rename_bounds.y + 16.0)),
        None,
        Modifiers::none(),
    );
    cx.run_until_parked();
    cx.simulate_click(
        gpui::point(px(rename_bounds.x + 16.0), px(rename_bounds.y + 16.0)),
        Modifiers::none(),
    );
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(view.session_rename.as_ref().unwrap().session_id, "other");
            assert_eq!(
                view.projection.active_session_id.as_deref(),
                Some("current")
            );
            assert!(view
                .accessibility_tree(window, cx)
                .find(&session_archive_identifier("other"))
                .is_none());
            view.cancel_session_rename(window, cx);
        })
    });
    cx.simulate_mouse_move(gpui::point(px(700.0), px(500.0)), None, Modifiers::none());
    cx.refresh().unwrap();
    cx.run_until_parked();
    // Cancel 回到该行；指针移走仍保持动作，Tab 可进入动作自身。
    cx.simulate_keystrokes("tab");
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let focus = &view.rail_row_focus[&rail_session_rename_focus_key("other")];
            assert!(
                focus.is_focused(window),
                "Tab reaches rename without opening the session"
            );
            assert!(view.session_actions_visible("other", window));
            view.projection
                .set_connection(ConnectionState::Disconnected {
                    reason: "test".into(),
                });
            let tree = view.accessibility_tree(window, cx);
            assert!(
                !tree
                    .find(&session_rename_identifier("other"))
                    .unwrap()
                    .enabled
            );
            assert!(
                !tree
                    .find(&session_archive_identifier("other"))
                    .unwrap()
                    .enabled
            );
            window.focus(&view.scope_focus);
            assert!(view
                .accessibility_tree(window, cx)
                .find(&session_rename_identifier("other"))
                .is_none());
        })
    });
}

fn collect_ax_ids(node: &AxNode, out: &mut Vec<String>) {
    out.push(node.identifier.clone());
    for child in &node.children {
        collect_ax_ids(child, out);
    }
}

fn ax_rects_overlap(a: AxRect, b: AxRect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

fn rail_session_summary(
    id: &str,
    title: &str,
    workspace: Option<&str>,
    updated_at_ms: u64,
) -> crate::projection::SessionSummary {
    crate::projection::SessionSummary {
        session_id: id.into(),
        title: title.into(),
        updated_at_ms,
        workspace_id: workspace.map(str::to_string),
        parent_branch_id: None,
        forked_from_event_id: None,
        active: false,
        unstarted: false,
    }
}

fn rail_live_event(
    session: &str,
    sequence: u64,
    payload: serde_json::Value,
) -> crate::controller::ControllerEvent {
    let envelope: pawork_client::AppEventEnvelope = serde_json::from_value(serde_json::json!({
        "api_version": {"major": 1, "minor": 1},
        "instance_id": "instance-1",
        "event_id": format!("gui3-06-{sequence}"),
        "global_sequence": sequence,
        "stream": {"type": "session", "id": session},
        "stream_sequence": sequence,
        "timestamp": 1_700_000_000_000u64 + sequence,
        "source": {"type": "core"},
        "payload": payload
    }))
    .expect("gui3-06 live event");
    crate::controller::ControllerEvent::Event(envelope)
}

fn drive_rail_live(
    view: &gpui::Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    session: &str,
    sequence: u64,
    payload: serde_json::Value,
) {
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.handle_controller_event(rail_live_event(session, sequence, payload), cx);
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
}

/// GUI3-06：Timeline 单项目 / Unassigned 跳过项目头，折叠也不藏任务。
/// live 点只经 ControllerEvent 进入投影：Running、终态清空、Blocked、
/// Needs input。桶头 24px，三档任务行 36px。150% 长标题槽 >80px；
/// 鼠标打开改名，空白 Enter 不提交，AX 归档只留下窗口内通知。
#[gpui::test]
fn task_rail_skips_redundant_headers_and_compacts_150(cx: &mut gpui::TestAppContext) {
    use crate::ui::theme::font::TextScale;
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("gui3-06-rail.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    let now = crate::ui::now_unix_ms();
    cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(1024.0)));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.projection.workspaces = vec![
                crate::projection::WorkspaceSummary {
                    id: "ws-a".into(),
                    name: "Alpha".into(),
                },
                crate::projection::WorkspaceSummary {
                    id: "ws-b".into(),
                    name: "Beta".into(),
                },
            ];
            view.projection.sessions = vec![
                rail_session_summary("idle", "Idle session title", None, now),
                rail_session_summary("running", "Running session title", None, now),
            ];
            view.projection.active_session_id = Some("idle".into());
            view.collapsed_projects
                .insert(UNASSIGNED_PROJECT.to_string());
            view.handle_controller_event(
                rail_live_event(
                    "running",
                    1,
                    serde_json::json!({
                        "type": "run_changed",
                        "data": {"run_id": "r-run", "state": "created"}
                    }),
                ),
                cx,
            );
            view.grouping = TaskRailGrouping::Timeline;
            view.text_scale = TextScale::Percent100;
            window.set_rem_size(gpui::px(TextScale::Percent100.rem_pixels()));
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        let list = tree.find("session-list").expect("session-list");
        let mut ids = Vec::new();
        collect_ax_ids(list, &mut ids);
        assert!(
            !ids.iter()
                .any(|id| id.starts_with("project-") && !id.starts_with("project-add-")),
            "Unassigned-only Timeline must skip project headers: {ids:?}"
        );
        let today = tree.find("date-group-Today").expect("today bucket");
        assert_eq!(today.bounds.height, metrics::RAIL_BUCKET_HEADER_HEIGHT);
        let idle = tree.find(&session_identifier("idle")).unwrap();
        assert_eq!(idle.bounds.height, 36.0);
        assert!(tree.find(&session_status_dot_identifier("idle")).is_none());
        assert_eq!(
            idle.description.as_deref(),
            Some("Session"),
            "idle must not invent a terminal status"
        );
        let running = tree.find(&session_identifier("running")).unwrap();
        assert_eq!(running.bounds.height, 36.0);
        assert!(tree
            .find(&session_status_dot_identifier("running"))
            .is_some());
        assert_eq!(running.description.as_deref(), Some("Running · Unread"));
        assert!(
            ids.iter().any(|id| id == &session_identifier("idle"))
                && ids.iter().any(|id| id == &session_identifier("running")),
            "collapsed Unassigned must not hide tasks when the header is skipped: {ids:?}"
        );
        let title = tree.find(&session_title_identifier("idle")).unwrap();
        assert!(title.bounds.width > 80.0, "idle title {:?}", title.bounds);
    });
    drive_rail_live(
        &view,
        cx,
        "running",
        2,
        serde_json::json!({
            "type": "run_changed",
            "data": {"run_id": "r-run", "state": "completed"}
        }),
    );
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        assert!(
            tree.find(&session_status_dot_identifier("running"))
                .is_none(),
            "completed run must not leave a status dot"
        );
        assert_eq!(
            tree.find(&session_identifier("running"))
                .unwrap()
                .description
                .as_deref(),
            Some("Session · Unread")
        );
        assert!(tree.find(&session_status_dot_identifier("idle")).is_none());
    });
    drive_rail_live(
        &view,
        cx,
        "running",
        3,
        serde_json::json!({
            "type": "run_changed",
            "data": {"run_id": "r-fail", "state": "failed"}
        }),
    );
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        let running = tree.find(&session_identifier("running")).unwrap();
        assert_eq!(running.description.as_deref(), Some("Blocked · Unread"));
        assert_eq!(
            tree.find(&session_status_dot_identifier("running"))
                .unwrap()
                .label,
            "Blocked"
        );
    });
    drive_rail_live(
        &view,
        cx,
        "running",
        4,
        serde_json::json!({
            "type": "tool_approval_required",
            "data": {
                "run_id": "r-fail",
                "tool_call_id": "call-1",
                "reason": "write_file"
            }
        }),
    );
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        assert_eq!(
            tree.find(&session_identifier("running"))
                .unwrap()
                .description
                .as_deref(),
            Some("Needs input · Unread"),
            "pending approval outranks blocked"
        );
        assert!(tree
            .find(&session_status_dot_identifier("running"))
            .is_some());
        assert!(tree.find(&session_status_dot_identifier("idle")).is_none());
    });

    cx.update(|_window, cx| {
        view.update(cx, |view, cx| {
            view.projection.sessions = vec![
                rail_session_summary("a1", "Alpha one", Some("ws-a"), now),
                rail_session_summary("b1", "Beta one", Some("ws-b"), now),
            ];
            view.projection.active_session_id = Some("a1".into());
            view.projection.active_runs.clear();
            view.projection.blocked_sessions.clear();
            view.projection.snapshot_pendings.clear();
            view.projection.pending_approval = None;
            view.scope_workspace_id = None;
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        let list = tree.find("session-list").unwrap();
        let mut ids = Vec::new();
        collect_ax_ids(list, &mut ids);
        assert!(
            ids.iter()
                .any(|id| id == &rail_project_identifier(Some(DateBucket::Today), "ws-a")),
            "multi-project bucket keeps headers: {ids:?}"
        );
        assert!(ids
            .iter()
            .any(|id| id == &rail_project_identifier(Some(DateBucket::Today), "ws-b")));
    });

    cx.update(|_window, cx| {
        view.update(cx, |view, cx| {
            view.scope_workspace_id = Some("ws-a".into());
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        let list = tree.find("session-list").unwrap();
        let mut ids = Vec::new();
        collect_ax_ids(list, &mut ids);
        assert!(
            !ids.iter()
                .any(|id| id.starts_with("project-") && !id.starts_with("project-add-")),
            "scoped singleton Timeline skips project header: {ids:?}"
        );
        assert!(ids.iter().any(|id| id == &session_identifier("a1")));
    });

    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.scope_workspace_id = None;
            view.grouping = TaskRailGrouping::Projects;
            view.projection.sessions = vec![rail_session_summary(
                "idle",
                "Idle session title",
                None,
                now,
            )];
            view.projection.active_session_id = Some("idle".into());
            cx.notify();
        });
        let _ = window;
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        assert!(
            tree.find("project-Unassigned").is_some(),
            "Projects grouping keeps Unassigned header"
        );
    });

    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.grouping = TaskRailGrouping::Timeline;
            view.projection.sessions = vec![rail_session_summary(
                "idle",
                "Very long session title that must remain readable beside rename and archive",
                None,
                now,
            )];
            view.projection.active_session_id = Some("idle".into());
            view.text_scale = TextScale::Percent150;
            window.set_rem_size(gpui::px(TextScale::Percent150.rem_pixels()));
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        let idle = tree.find(&session_identifier("idle")).unwrap();
        assert_eq!(idle.bounds.height, 36.0);
        let title = tree.find(&session_title_identifier("idle")).unwrap();
        assert!(
            title.bounds.width > 80.0,
            "150% title frame {:?}",
            title.bounds
        );
        let rename = tree
            .find(&session_rename_identifier("idle"))
            .expect("current session shows rename");
        let archive = tree
            .find(&session_archive_identifier("idle"))
            .expect("current session shows archive");
        assert!(
            !ax_rects_overlap(title.bounds, rename.bounds),
            "rename overlaps title {:?} {:?}",
            title.bounds,
            rename.bounds
        );
        assert!(
            !ax_rects_overlap(title.bounds, archive.bounds),
            "archive overlaps title {:?} {:?}",
            title.bounds,
            archive.bounds
        );
        assert_eq!(rename.bounds.width, metrics::RAIL_SESSION_ACTION_SIZE);
        assert_eq!(rename.bounds.height, metrics::RAIL_SESSION_ACTION_SIZE);
        assert_eq!(archive.bounds.width, metrics::RAIL_SESSION_ACTION_SIZE);
        assert_eq!(archive.bounds.height, metrics::RAIL_SESSION_ACTION_SIZE);
        assert!(
            view.read(cx)
                .collapsed_projects
                .contains(UNASSIGNED_PROJECT),
            "150% still uses the skipped-header collapse case"
        );
    });
    let rename_hit = cx.update(|window, cx| {
        view.read(cx)
            .accessibility_tree(window, cx)
            .find(&session_rename_identifier("idle"))
            .unwrap()
            .bounds
    });
    let rename_at = gpui::point(
        gpui::px(rename_hit.x + rename_hit.width / 2.0),
        gpui::px(rename_hit.y + rename_hit.height / 2.0),
    );
    cx.simulate_mouse_move(rename_at, None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.simulate_click(rename_at, gpui::Modifiers::none());
    cx.run_until_parked();
    let long_title = "Very long session title that must remain readable beside rename and archive";
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(
                view.session_rename
                    .as_ref()
                    .map(|state| state.session_id.as_str()),
                Some("idle"),
                "150% rename hit {:?} must open the inline editor",
                rename_hit
            );
            assert_eq!(view.projection.active_session_id.as_deref(), Some("idle"));
            view.session_rename
                .as_ref()
                .unwrap()
                .input
                .update(cx, |input, cx| {
                    input.set_text("   ", cx);
                });
            // TextInput 的 Enter 绑定派发 SendMessage，根节点落到此函数。
            view.on_send_message(&crate::ui::SendMessage, window, cx);
            assert!(
                view.session_rename.is_some(),
                "blank rename must stay in the editor"
            );
            assert_eq!(
                view.projection
                    .sessions
                    .iter()
                    .find(|session| session.session_id == "idle")
                    .unwrap()
                    .title,
                long_title
            );
            view.session_rename
                .as_ref()
                .unwrap()
                .input
                .update(cx, |input, cx| {
                    input.set_text("Renamed from the rail", cx);
                });
            view.on_send_message(&crate::ui::SendMessage, window, cx);
            assert!(view.session_rename.is_none(), "changed title submits");
            assert_eq!(
                view.projection
                    .sessions
                    .iter()
                    .find(|session| session.session_id == "idle")
                    .unwrap()
                    .title,
                long_title,
                "rename does not paint a title before the snapshot"
            );
        });
    });
    press_published_ax(cx, &view, &session_archive_identifier("idle"));
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(
            view.archive_notice_text().is_some(),
            "archive handler must surface the window notice"
        );
        assert!(
            view.accessibility_tree(window, cx)
                .find(&session_identifier("idle"))
                .is_some(),
            "archive without a host snapshot must keep the task"
        );
    });

    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.text_scale = TextScale::Percent125;
            window.set_rem_size(gpui::px(TextScale::Percent125.rem_pixels()));
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        assert_eq!(
            tree.find(&session_identifier("idle"))
                .unwrap()
                .bounds
                .height,
            36.0
        );
    });
}

/// 项目头计数 / 「+」与任务行改名 / 归档共用尾槽原点（render / AX 同源）。
#[gpui::test]
fn task_rail_trailing_slots_align_project_header_and_task_actions(cx: &mut gpui::TestAppContext) {
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("rail-trailing-align.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    let now = crate::ui::now_unix_ms();
    cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(1024.0)));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.projection.workspaces = vec![crate::projection::WorkspaceSummary {
                id: "ws-a".into(),
                name: "Alpha".into(),
            }];
            view.projection.sessions = vec![rail_session_summary(
                "task-a",
                "Alpha task",
                Some("ws-a"),
                now,
            )];
            view.projection.active_session_id = Some("task-a".into());
            view.grouping = TaskRailGrouping::Projects;
            window.set_rem_size(gpui::px(
                crate::ui::theme::font::TextScale::Percent100.rem_pixels(),
            ));
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        let count = tree
            .find(&rail_project_count_identifier(None, "ws-a"))
            .expect("project count slot");
        let plus = tree
            .find(&rail_project_add_identifier(None, "ws-a"))
            .expect("project plus");
        let rename = tree
            .find(&session_rename_identifier("task-a"))
            .expect("rename");
        let archive = tree
            .find(&session_archive_identifier("task-a"))
            .expect("archive");
        assert_eq!(
            count.bounds.x, rename.bounds.x,
            "count {:?} rename {:?}",
            count.bounds, rename.bounds
        );
        assert_eq!(
            plus.bounds.x, archive.bounds.x,
            "plus {:?} archive {:?}",
            plus.bounds, archive.bounds
        );
        assert_eq!(count.bounds.width, metrics::RAIL_SESSION_ACTION_SIZE);
        assert_eq!(plus.bounds.width, metrics::RAIL_SESSION_ACTION_SIZE);
    });
}

/// P0-2：grouping AXPress 直接双向切换且不生成菜单，并保留 session / scope /
/// draft / collapsed projects；其余菜单触发器仍先移焦，Escape-style 关闭后
/// 回到来源。AppView 不作窗口根渲染，仅驱动 AX dispatch 本身。
#[gpui::test]
fn ax_press_direct_grouping_and_menu_triggers_keep_focus_contract(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;

    struct AxPressHost {
        view: gpui::Entity<AppView>,
    }
    impl gpui::Render for AxPressHost {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("p4-2f-ax-press.sock");
    let (host, cx) = cx.add_window_view(|_window, cx| {
        let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
        AxPressHost { view }
    });
    let view = cx.update(|_window, cx| host.read(cx).view.clone());

    // OPT-4b（F6）：Inspector 默认折叠；显式动作（Review changes /
    // Activity 摘要 / inspector-expand）仍可展开。
    cx.update(|_window, cx| {
        assert!(!view.read(cx).inspector_open);
    });

    // UI-1：折叠过渡中保留可见面板头，部分可见的选择器裁到窗口右缘；
    // 完全隐藏的操作（含旧 AX 客户端保留的对象）不可再调用。
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.inspector_render_width = 160.0;
            let tree = view.accessibility_tree(window, cx);
            let inspector = tree.find("inspector").unwrap();
            let workspace = tree.find("workspace").unwrap();
            assert!(tree.find("inspector-panel").is_none());
            assert_eq!(inspector.bounds.width, 160.0);
            assert_eq!(
                workspace.bounds.x + workspace.bounds.width,
                inspector.bounds.x
            );
            assert!(tree.find("inspector-tab-resources").is_none());
            assert!(tree.find("inspector-tabs").is_none());
            assert!(!tree.permits(&AxRequest {
                identifier: "inspector-collapse".into(),
                action: AxAction::Press,
                value: None,
            }));
            assert!(tree.find("inspector-expand").is_some());
            view.inspector_render_width = 0.0;
            assert!(view
                .accessibility_tree(window, cx)
                .find("inspector")
                .is_none());
        });
    });

    cx.update(|window, cx| {
        view.update(cx, |view, _cx| {
            view.projection.active_session_id = Some("s-keep".into());
            view.scope_workspace_id = Some("ws-keep".into());
            view.composer_drafts.insert("s-keep".into(), "draft".into());
            view.collapsed_projects.insert("ws-keep".into());
            // 直接切换也必须关闭此前打开的其它浮层与高亮。
            view.open_menu = Some(MenuKind::Scope);
            view.menu_highlight = Some(0);
        });
        let composer = view.read(cx).composer_focus_handle(cx);
        window.focus(&composer);
    });
    for expected in [TaskRailGrouping::Timeline, TaskRailGrouping::Projects] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: "task-rail-grouping".into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.grouping, expected);
            assert!(view.open_menu.is_none());
            assert!(view.menu_highlight.is_none());
            assert!(view.grouping_focus.is_focused(window));
            assert_eq!(view.projection.active_session_id.as_deref(), Some("s-keep"));
            assert_eq!(view.scope_workspace_id.as_deref(), Some("ws-keep"));
            assert_eq!(
                view.composer_drafts.get("s-keep").map(String::as_str),
                Some("draft")
            );
            assert!(view.collapsed_projects.contains("ws-keep"));
            assert!(view.rail_scroll_to_active);
        });
    }

    // 恢复 All projects：scope 仍开菜单；两个全局 New Task 入口按
    // ADR-054 D1 直建无归属会话（无确认浮层）。
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.scope_workspace_id = None;
            view.projection.active_session_id = None;
        });
    });
    for identifier in ["project-scope"] {
        cx.update(|window, cx| {
            // scope 菜单触发器同源注入 Connected（真实点击同理）。
            view.update(cx, |view, _cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
            });
            let composer = view.read(cx).composer_focus_handle(cx);
            window.focus(&composer);
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: identifier.to_string(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(
                !view.composer_focus_handle(cx).is_focused(window),
                "{identifier}: AX press must move focus off the composer"
            );
            let trigger_focused = match identifier {
                "project-scope" => view.scope_focus.is_focused(window),
                "add-task" => view.add_task_focus.is_focused(window),
                _ => view.header_new_task_focus.is_focused(window),
            };
            assert!(
                trigger_focused,
                "{identifier}: AX press must focus the trigger like a click"
            );
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let kind = view.open_menu.clone().expect("AX press should open a menu");
                view.close_menu_and_focus_trigger(kind, window, cx);
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            let trigger_focused = match identifier {
                "project-scope" => view.scope_focus.is_focused(window),
                "add-task" => view.add_task_focus.is_focused(window),
                _ => view.header_new_task_focus.is_focused(window),
            };
            assert!(
                trigger_focused,
                "{identifier}: Escape-style close must restore the originating trigger"
            );
        });
    }

    // ADR-054 D1：All projects 下 add-task / header-new-task 的 AX press
    // 与真实点击同路径直建无归属会话——不开任何确认菜单，流程收口
    // 后焦点回 Composer（新任务待输入）。测试宿主无连接 client，
    // create_session 走 not-connected 诚实失败通道（emit_reliable 在
    // 未连接时为 no-op），不伪造成功。
    for identifier in ["add-task", "header-new-task"] {
        cx.update(|window, cx| {
            view.update(cx, |view, _cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
            });
            let composer = view.read(cx).composer_focus_handle(cx);
            window.focus(&composer);
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: identifier.to_string(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(
                view.open_menu.is_none(),
                "{identifier}: direct create must not open a confirm menu"
            );
            assert!(
                view.composer_focus_handle(cx).is_focused(window),
                "{identifier}: direct create hands focus to the composer"
            );
        });
    }
    // UI-3：思考只读详情默认收起；AX 与鼠标/键盘共用展开状态。
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.projection.timeline.entries = vec![TimelineEntry {
                sequence: 1,
                event_id: "thinking-test".into(),
                kind: TimelineEntryKind::Thinking {
                    text: "核对输入，再组织回答。".into(),
                },
                fork_boundary: None,
                timestamp: "1".into(),
                run_id: Some("run-1".into()),
            }];
            let row = view.projection.timeline_rows().remove(0);
            assert!(matches!(row, TimelineRow::Thinking { .. }));
            let rect = AxRect::new(300.0, 100.0, 560.0, metrics::THINKING_HEADER_HEIGHT);
            let collapsed = view.timeline_row_ax(window, &row, rect);
            assert_eq!(
                collapsed.children.len(),
                1,
                "collapsed thinking must not expose body"
            );
            assert_eq!(
                timeline::timeline_row_height(
                    &row,
                    &view.projection.timeline,
                    rect.width,
                    16.0,
                    &view.expanded_timeline_details,
                    false,
                    None,
                ),
                rect.height
            );
            assert!(view.handle_accessibility_press("thinking-toggle-thinking-test", window, cx));
            assert!(!view.timeline_following, "expansion preserves the viewport");
            let height = timeline::timeline_row_height(
                &row,
                &view.projection.timeline,
                rect.width,
                16.0,
                &view.expanded_timeline_details,
                false,
                None,
            );
            assert!(height > rect.height);
            let expanded = view.timeline_row_ax(window, &row, AxRect { height, ..rect });
            assert_eq!(expanded.children.len(), 2);
            assert_eq!(
                expanded.children[1].value.as_deref(),
                Some("核对输入，再组织回答。")
            );
            assert!(view.handle_accessibility_press("thinking-toggle-thinking-test", window, cx));
            assert!(view.expanded_timeline_details.is_empty());
        });
    });
}

/// 多项目菜单使用实测滚动几何；键盘高亮不能停在视口之外。
#[gpui::test]
fn scope_menu_ax_follows_scrolling_and_keyboard_highlight(cx: &mut gpui::TestAppContext) {
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let (view, cx) = cx.add_window_view(|_window, cx| {
        AppView::new(
            platform,
            std::env::temp_dir().join("opt4-scope.sock"),
            None,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(1024.0)));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.projection.workspaces = (0..10)
                .map(|ix| crate::projection::WorkspaceSummary {
                    id: format!("ws-{ix}"),
                    name: format!("Project {ix}"),
                })
                .collect();
            view.scope_workspace_id = Some("ws-9".into());
            view.on_toggle_scope_menu(None, window, cx);
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        let menu = tree.find("scope-menu").unwrap();
        assert!(menu.bounds.height <= MENU_MAX_HEIGHT);
        let current = tree
            .find("scope-ws-9")
            .expect("current project visible on first open");
        assert!(current.selected);
        assert!(current.bounds.y + current.bounds.height <= menu.bounds.y + menu.bounds.height);
        assert!(tree.find("scope-all").is_none());
    });
    // 从当前项目向下到最后一项 Add project，必须滚入视口。
    cx.update(|_window, cx| {
        view.update(cx, |view, cx| {
            view.move_menu_highlight(true);
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        let menu = tree.find("scope-menu").unwrap();
        let add = tree
            .find("scope-add-project")
            .expect("last row scrolled into view");
        assert!(add.focused);
        assert!(add.bounds.y >= menu.bounds.y);
        assert!(add.bounds.y + add.bounds.height <= menu.bounds.y + menu.bounds.height);
        assert!(tree.find("scope-all").is_none());
    });
    // 与滚轮同源修改 offset，原点回到顶部后 AX 也恢复首项。
    cx.update(|_window, cx| {
        view.update(cx, |view, cx| {
            view.scope_menu_scroll
                .set_offset(gpui::point(gpui::px(0.0), gpui::px(0.0)));
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        assert!(tree.find("scope-all").is_some());
        assert!(tree.find("scope-add-project").is_none());
    });
}

/// SET-6e/6f/6g：外观与高级页都是 Desktop 本地能力，离线也必须可达；
/// About 仅在当前握手携带非空 Host 路径时出现，丢失后退回高级页；
/// 高级页不伪装旧握手，外观字号 AX Press 与可见 / 键盘路径同源。
#[gpui::test]
fn settings_local_pages_ax_are_available_offline_and_update_state(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;

    struct AxAppearanceHost {
        view: gpui::Entity<AppView>,
    }
    impl gpui::Render for AxAppearanceHost {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> impl gpui::IntoElement {
            self.view.clone()
        }
    }

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("set6f-ax-local-pages.sock");
    let endpoint = socket.display().to_string();
    let (host, cx) = cx.add_window_view(|_window, cx| {
        let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
        AxAppearanceHost { view }
    });
    let view = cx.update(|_window, cx| host.read(cx).view.clone());
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.route = AppRoute::Settings;
            view.projection.set_connection(ConnectionState::Failed {
                reason: "host unavailable".into(),
            });
        });
    });

    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        tree.validate().expect("offline Settings AX tree validates");
        assert!(tree.find("settings-nav-appearance").is_some());
        assert!(tree.find("settings-nav-advanced").is_some());
        assert!(tree.find("settings-nav-about").is_none());
        // P2-1：Settings 壳不发布 RunStatusBar。
        assert!(tree.find("status-bar").is_none());
        assert!(tree.find("run-status").is_none());
        assert!(!tree.permits(&AxRequest {
            identifier: "settings-nav-about".into(),
            action: AxAction::Press,
            value: None,
        }));
        assert!(tree.permits(&AxRequest {
            identifier: "settings-nav-advanced".into(),
            action: AxAction::Press,
            value: None,
        }));
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(
                AxRequest {
                    identifier: "settings-nav-advanced".into(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
        });
    });
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.settings_page, SettingsPage::Advanced);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().expect("advanced page AX tree validates");
        assert_eq!(
            tree.find("settings-advanced-connection")
                .and_then(|node| node.value.as_deref()),
            Some("Connect failed · host unavailable")
        );
        assert_eq!(
            tree.find("settings-advanced-runtime")
                .and_then(|node| node.value.as_deref()),
            Some("Unavailable · connect to the Host")
        );
        assert_eq!(
            tree.find("settings-advanced-endpoint")
                .and_then(|node| node.value.as_deref()),
            Some(endpoint.as_str())
        );
        assert!(tree.find("reconnect").is_some());
        assert!(tree.permits(&AxRequest {
            identifier: "reconnect".into(),
            action: AxAction::Press,
            value: None,
        }));
    });
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "runtime-6f".into(),
            });
            view.projection.resume = crate::projection::ResumeState::UpToDate {
                current_sequence: 42,
            };
            view.handshake_info = Some(crate::controller::DesktopHandshakeInfo {
                runtime_id: "runtime-6f".into(),
                api_version: "1.9".into(),
                capabilities: vec!["events".into(), "snapshots".into()],
                host_data_dir: None,
            });
        });
    });
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        tree.validate()
            .expect("connected advanced page AX tree validates");
        assert_eq!(
            tree.find("settings-advanced-connection")
                .and_then(|node| node.value.as_deref()),
            Some("Connected")
        );
        assert_eq!(
            tree.find("settings-advanced-runtime")
                .and_then(|node| node.value.as_deref()),
            Some("runtime-6f")
        );
        assert_eq!(
            tree.find("settings-advanced-api")
                .and_then(|node| node.value.as_deref()),
            Some("1.9")
        );
        assert_eq!(
            tree.find("settings-advanced-capabilities")
                .and_then(|node| node.value.as_deref()),
            Some("events, snapshots")
        );
        assert_eq!(
            tree.find("settings-advanced-resume")
                .and_then(|node| node.value.as_deref()),
            Some("Up to date · 42")
        );
        assert_eq!(
            tree.find("settings-advanced-last-ack")
                .and_then(|node| node.value.as_deref()),
            Some("Unavailable")
        );
        assert!(tree.find("reconnect").is_none());
        assert!(tree.find("settings-nav-about").is_none());
        assert!(tree.permits(&AxRequest {
            identifier: "settings-nav-appearance".into(),
            action: AxAction::Press,
            value: None,
        }));
    });
    // 只有当前握手给出非空权威路径时才发布 About；页面三行与
    // render 共用数据源，不从 endpoint 或本机默认目录推断。
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.handshake_info
                .as_mut()
                .expect("connected handshake exists")
                .host_data_dir = Some(" /tmp/pawork-set6g ".into());
        });
    });
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        tree.validate().expect("About navigation AX tree validates");
        assert!(tree.find("settings-nav-about").is_some());
        assert!(tree.permits(&AxRequest {
            identifier: "settings-nav-about".into(),
            action: AxAction::Press,
            value: None,
        }));
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(
                AxRequest {
                    identifier: "settings-nav-about".into(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
        });
    });
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.settings_page, SettingsPage::About);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().expect("About page AX tree validates");
        assert_eq!(
            tree.find("settings-about-desktop-build")
                .and_then(|node| node.value.as_deref()),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            tree.find("settings-about-api")
                .and_then(|node| node.value.as_deref()),
            Some("1.9")
        );
        assert_eq!(
            tree.find("settings-about-data-dir")
                .and_then(|node| node.value.as_deref()),
            Some(" /tmp/pawork-set6g ")
        );
    });
    // 最终业务入口也必须 fail-closed：即使迟到的旧 Reconnect 事件被
    // 派发，Connected 状态也不能启动第二条连接。
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.on_reconnect(window, cx));
    });
    cx.update(|_window, cx| {
        let view = view.read(cx);
        assert!(matches!(
            &view.projection.connection,
            ConnectionState::Connected { .. }
        ));
        assert!(view.handshake_info.is_some());
    });
    // 通过真实 ControllerEvent 消费路径证明旧握手在断线时清空，且
    // Advanced 页重新发布 Reconnect；不直接改测试状态绕过生命周期。
    cx.update(|_window, cx| {
        view.update(cx, |view, cx| {
            view.handle_controller_event(
                crate::controller::ControllerEvent::Disconnected {
                    reason: "connection closed".into(),
                },
                cx,
            );
        });
    });
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view.handshake_info.is_none());
        assert_eq!(view.settings_page, SettingsPage::Advanced);
        let tree = view.accessibility_tree(window, cx);
        tree.validate()
            .expect("disconnected advanced page AX tree validates");
        assert_eq!(
            tree.find("settings-advanced-connection")
                .and_then(|node| node.value.as_deref()),
            Some("Disconnected · connection closed")
        );
        assert_eq!(
            tree.find("settings-advanced-runtime")
                .and_then(|node| node.value.as_deref()),
            Some("Unavailable · connect to the Host")
        );
        assert!(tree.find("settings-nav-about").is_none());
        assert!(tree.find("settings-about-data-dir").is_none());
        assert!(tree.find("reconnect").is_some());
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(
                AxRequest {
                    identifier: "settings-nav-appearance".into(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.settings_page, SettingsPage::Appearance);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().expect("appearance page AX tree validates");
        let scale_100 = tree.find("settings-text-scale-100").unwrap();
        let scale_125 = tree.find("settings-text-scale-125").unwrap();
        let scale_150 = tree.find("settings-text-scale-150").unwrap();
        let title = tree.find("settings-page-title").unwrap();
        assert_eq!(scale_100.bounds.x, title.bounds.x);
        assert_eq!(
            tree.find("settings-language-en").unwrap().bounds.x,
            title.bounds.x
        );
        assert!(scale_100.selected);
        assert!(!scale_125.selected);
        assert!(!scale_150.selected);
        for node in [scale_100, scale_125, scale_150] {
            assert_eq!(node.bounds.width, SETTINGS_APPEARANCE_CONTROL_WIDTH);
            assert_eq!(node.bounds.height, SETTINGS_APPEARANCE_CONTROL_HEIGHT);
        }
        assert_eq!(
            scale_125.bounds.x - scale_100.bounds.x,
            SETTINGS_APPEARANCE_CONTROL_WIDTH + SETTINGS_APPEARANCE_CONTROL_GAP
        );
        assert_eq!(
            scale_150.bounds.x - scale_125.bounds.x,
            SETTINGS_APPEARANCE_CONTROL_WIDTH + SETTINGS_APPEARANCE_CONTROL_GAP
        );
        assert!(!tree.permits(&AxRequest {
            identifier: "settings-text-scale-175".into(),
            action: AxAction::Press,
            value: None,
        }));
        assert!(tree.permits(&AxRequest {
            identifier: "settings-text-scale-150".into(),
            action: AxAction::Press,
            value: None,
        }));
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(
                AxRequest {
                    identifier: "settings-text-scale-150".into(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.text_scale, font::TextScale::Percent150);
        assert_eq!(f32::from(window.rem_size()), 24.0);
        let tree = view.accessibility_tree(window, cx);
        assert!(!tree.find("settings-text-scale-100").unwrap().selected);
        assert!(tree.find("settings-text-scale-150").unwrap().selected);
    });
}

/// SET-4（SET-010）：Settings secure API key 输入的 AX value 只发布掩码，
/// 全树不携带明文；断线 stale 后写动作按钮与输入 enabled=false 且
/// permits 拒绝（可见 / 键盘 / AX 三路径同 gate）。
#[gpui::test]
fn settings_ax_masks_api_key_and_gates_writes_when_stale(cx: &mut gpui::TestAppContext) {
    use gpui::{prelude::*, px, AppContext};

    struct AxSettingsHost {
        view: gpui::Entity<AppView>,
    }
    impl gpui::Render for AxSettingsHost {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> impl gpui::IntoElement {
            window.set_rem_size(px(16.0));
            gpui::div()
                .size_full()
                .flex()
                .child(self.view.update(cx, |view, cx| {
                    view.ensure_settings_api_key_inputs(cx);
                    view.settings_page_element(cx)
                }))
        }
    }

    fn assert_no_secret(node: &AxNode, secret: &str) {
        assert!(!node.identifier.contains(secret));
        assert!(!node.label.contains(secret));
        if let Some(value) = &node.value {
            assert!(!value.contains(secret), "AX value leaked secret: {value}");
        }
        if let Some(description) = &node.description {
            assert!(!description.contains(secret));
        }
        for child in &node.children {
            assert_no_secret(child, secret);
        }
    }

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("set4-ax-settings.sock");
    let (host, cx) = cx.add_window_view(|_window, cx| {
        let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
        AxSettingsHost { view }
    });
    // Host 直接装配子 view 的元素，须在每次验收前通知 Host 重绘；
    // 单独 refresh window 不会使已缓存的 Host render 失效。
    // 语义断言覆盖完整页面；窄窗与菜单裁剪另在本测试内实际滚动。
    cx.simulate_resize(gpui::size(px(1440.0), px(2400.0)));
    let view = cx.update(|_window, cx| host.read(cx).view.clone());
    cx.update(|_window, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.route = AppRoute::Settings;
            view.projection.settings_providers.apply_loaded(
                crate::projection::ProviderAuthStatusData {
                    providers: vec![
                        crate::projection::ProviderAuthStatusEntry {
                            provider_id: "kimi".into(),
                            display_name: "Kimi".into(),
                            endpoint_label: "https://api.moonshot.cn".into(),
                            auth_methods: vec!["api_key".into()],
                            credentials: Vec::new(),
                            selection_mode: Default::default(),
                            auth: crate::projection::ProviderAuthState::None,
                            catalog: crate::projection::ProviderCatalogState::Unavailable {
                                error: "offline".into(),
                                fetched_at: None,
                            },
                            use_proxy: true,
                        },
                        crate::projection::ProviderAuthStatusEntry {
                            provider_id: "connected".into(),
                            display_name: "Connected provider".into(),
                            endpoint_label: "https://provider.example".into(),
                            auth_methods: vec!["api_key".into()],
                            credentials: Vec::new(),
                            selection_mode: Default::default(),
                            auth: crate::projection::ProviderAuthState::Connected {
                                method: "api_key".into(),
                                masked_credential: Some("masked-fragment-sentinel".into()),
                            },
                            catalog: crate::projection::ProviderCatalogState::FixedFallback {
                                snapshot_label: "test@v1".into(),
                                fetched_at: None,
                            },
                            use_proxy: true,
                        },
                    ],
                    default: None,
                    role_defaults: Default::default(),
                },
            );
            view.ensure_settings_api_key_inputs(cx);
            view.settings_api_key_editors.insert("kimi".into());
            view.settings_api_key_inputs
                .get("kimi")
                .expect("api_key provider gets a secure input")
                .update(cx, |input, cx| input.set_text("sk-live-plaintext", cx));
        });
    });

    let input_id = crate::ui::settings::settings_api_key_input_identifier("kimi");
    let verify_id = crate::ui::settings::SettingsAuthAction::VerifyApiKey.identifier("kimi");
    let expected_mask = "•".repeat("sk-live-plaintext".chars().count());

    // 连接态：掩码 value 发布、按钮 enabled、Press 许可。
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);

        tree.validate().expect("settings AX tree validates");
        let secret = "sk-live-plaintext";
        for child in &tree.children {
            assert_no_secret(child, secret);
            assert_no_secret(child, "masked-fragment-sentinel");
        }
        let connected = tree
            .find(&dynamic_identifier("settings-provider", "connected"))
            .expect("connected provider has an AX summary");
        assert!(connected.value.as_deref().is_some_and(|value| {
            value.contains("Connected") && !value.contains("masked-fragment-sentinel")
        }));
        // 名称、连接与目录的 AX 框直接对照本次 prepaint 的真实元素。
        for prefix in [
            "settings-provider-name",
            "settings-provider-connection",
            "settings-provider-catalog",
        ] {
            let id = dynamic_identifier(prefix, "connected");
            let actual = view.settings_element_layouts[&id].bounds();
            let node = tree
                .find(&id)
                .expect("rendered overview text has an AX node");
            assert_eq!(
                node.bounds,
                AxRect::new(
                    actual.origin.x.into(),
                    actual.origin.y.into(),
                    actual.size.width.into(),
                    actual.size.height.into()
                )
            );
        }
        let header_id = dynamic_identifier("settings-provider-header", "connected");
        let header = view.settings_element_layouts[&header_id].bounds();
        let header_h: f32 = header.size.height.into();
        assert!(
            header_h >= 52.0 && header_h <= 120.0,
            "provider header height {header_h}"
        );
        let input = tree.find(&input_id).expect("secure input has an AX node");
        assert_eq!(input.role, AxRole::TextArea);
        assert_eq!(input.value.as_deref(), Some(expected_mask.as_str()));
        assert!(input.enabled);
        let verify = tree.find(&verify_id).expect("verify button has an AX node");
        assert!(verify.enabled);
        for id in [&input_id, &verify_id] {
            let actual = view.settings_element_layouts[id].bounds();
            let node = tree.find(id).expect("rendered auth control");
            assert_eq!(
                node.bounds,
                AxRect::new(
                    actual.origin.x.into(),
                    actual.origin.y.into(),
                    actual.size.width.into(),
                    actual.size.height.into()
                )
            );
        }
        assert!(tree.permits(&AxRequest {
            identifier: verify_id.clone(),
            action: AxAction::Press,
            value: None,
        }));
    });

    // 断线 stale：写动作与 secure 输入同时禁用，permits 拒绝。
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.projection
                .settings_providers
                .mark_stale("socket closed");
        });
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        assert!(!tree.find(&verify_id).expect("verify button").enabled);
        assert!(!tree.find(&input_id).expect("secure input").enabled);
        assert!(!tree.permits(&AxRequest {
            identifier: verify_id.clone(),
            action: AxAction::Press,
            value: None,
        }));
    });
}

/// GUI2-01：两种首页共用布局，创建仅由显式动作触发；三档字号不遮挡导航。
#[gpui::test]
fn welcome_and_rail_follow_actual_layout(cx: &mut gpui::TestAppContext) {
    use crate::ui::theme::font::TextScale;
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("gui2-01-welcome.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    cx.run_until_parked();
    for (width, height) in [(1440.0, 1024.0), (1080.0, 720.0)] {
        for scale in [
            TextScale::Percent100,
            TextScale::Percent125,
            TextScale::Percent150,
        ] {
            for active in [false, true] {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.projection.set_connection(ConnectionState::Connected {
                            instance_id: "test".into(),
                        });
                        view.projection.active_session_id = active.then(|| "empty".into());
                        view.text_scale = scale;
                        window.set_rem_size(gpui::px(scale.rem_pixels()));
                        cx.notify();
                    })
                });
                cx.refresh().unwrap();
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let view = view.read(cx);
                    let tree = view.accessibility_tree(window, cx);
                    assert!(tree.find("workspace-empty-title").is_some());
                    if active {
                        assert!(tree.find("workspace-empty-hint").is_none());
                        let bind = tree.find("workspace-bind-project").unwrap();
                        assert_eq!(bind.label, t("timeline.bind_project"));
                        assert!(bind.enabled);
                    } else {
                        assert_eq!(
                            tree.find("workspace-empty-hint").unwrap().label,
                            view.welcome_hint()
                        );
                        assert!(tree.find("workspace-bind-project").is_none());
                        assert_eq!(
                            tree.find("composer-workspace").unwrap().value.as_deref(),
                            Some(t("composer.no_project_chip"))
                        );
                        let project_task = tree.find("composer-project-task").unwrap();
                        assert_eq!(project_task.label, t("composer.project_task"));
                        assert!(project_task.enabled);
                        assert!(tree.find("composer-context").is_none());
                        assert!(tree.find("composer-file-tools-hint").is_none());
                    }
                    assert_eq!(tree.find("header-new-task").is_some(), !active);
                    let action = view.shell_ax_rect("workspace-empty-action");
                    let card = view.composer_layouts["composer-card"].bounds();
                    assert!(
                        (f32::from(card.top()) - action.y - action.height - 32.0).abs() <= 1.0,
                        "welcome-to-composer gap: {action:?}, {card:?} at {scale:?}"
                    );
                    assert!(
                        view.projection.sessions.is_empty(),
                        "render must not create tasks"
                    );
                    let add = tree.find("add-task").unwrap().bounds;
                    let scope = tree.find("project-scope").unwrap().bounds;
                    let grouping = tree.find("task-rail-grouping").unwrap().bounds;
                    assert!(add.y >= shell_layout::TRAFFIC_LIGHT_SAFE_HEIGHT);
                    assert!(add.y + add.height <= scope.y);
                    assert!(scope.x + scope.width <= grouping.x + 1.0);
                    assert!(tree.find("connection-status").unwrap().bounds.y > height - 70.0);
                    let header = tree.find("workspace-header").unwrap().bounds;
                    assert!(header.height >= metrics::HEADER_HEIGHT);
                    let mut ids = vec![
                        "workspace-empty-title",
                        "add-task",
                        "project-scope",
                        "task-rail-grouping",
                        "open-settings",
                    ];
                    if active {
                        ids.push("workspace-bind-project");
                    } else {
                        ids.push("workspace-empty-hint");
                    }
                    for id in ids {
                        let b = tree.find(id).unwrap().bounds;
                        assert!(b.width > 0.0 && b.height > 0.0, "{id}");
                        assert!(
                            b.x >= 0.0
                                && b.y >= 0.0
                                && b.x + b.width <= width + 1.0
                                && b.y + b.height <= height + 1.0,
                            "{id}: {b:?}"
                        );
                    }
                });
            }
        }
    }
    cx.update(|_, cx| {
        view.update(cx, |view, _| {
            view.timeline_paging = true;
            assert!(
                !view.welcome_visible(),
                "loading history is not an empty task"
            );
            view.timeline_paging = false;
            view.projection.active_run_id = Some("running".into());
            assert!(!view.welcome_visible(), "running is not an empty task");
        })
    });
}

fn press_published_ax(
    cx: &mut gpui::VisualTestContext,
    view: &gpui::Entity<AppView>,
    identifier: &str,
) {
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let request = {
                let tree = view.accessibility_tree(window, cx);
                let request = AxRequest {
                    identifier: identifier.to_string(),
                    action: AxAction::Press,
                    value: None,
                };
                assert!(tree.permits(&request), "{identifier} must publish Press");
                request
            };
            view.handle_accessibility_request(request, window, cx);
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
}

/// GUI2-01 模拟验收：同一条渲染路径覆盖首页、真实 Snapshot 的项目 /
/// 无项目切换、分组与折叠键盘、Settings 往返，以及连接状态不伪装。
/// 真窗口、系统缩放、真实像素、真实 Host 与真实 Provider 不在此证明。
#[gpui::test]
fn workbench_home_rail_projection_ax_and_keyboard(cx: &mut gpui::TestAppContext) {
    use crate::ui::theme::font::TextScale;
    use gpui::{px, size};

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("gui2-01-workbench.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    cx.run_until_parked();

    for (width, height) in [(1440.0, 1024.0), (1080.0, 720.0)] {
        for scale in [
            TextScale::Percent100,
            TextScale::Percent125,
            TextScale::Percent150,
        ] {
            let large = scale == TextScale::Percent150;
            let expected_rail = if large {
                shell_layout::RAIL_LARGE_TEXT_WIDTH
            } else if width <= shell_layout::NARROW_WIDTH_MAX {
                shell_layout::RAIL_NARROW_WIDTH
            } else {
                metrics::SIDEBAR_WIDTH
            };
            cx.simulate_resize(size(px(width), px(height)));
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.projection.set_connection(ConnectionState::Connected {
                        instance_id: "gui2-01".into(),
                    });
                    view.projection.active_session_id = None;
                    view.projection.sessions.clear();
                    view.text_scale = scale;
                    window.set_rem_size(px(scale.rem_pixels()));
                    cx.notify();
                })
            });
            cx.refresh().unwrap();
            cx.run_until_parked();
            cx.update(|window, cx| {
                let view = view.read(cx);
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                assert_eq!(
                    tree.find("workspace-empty-title").unwrap().label,
                    workspace_empty_title()
                );
                assert_eq!(
                    tree.find("workspace-empty-hint").unwrap().label,
                    view.welcome_hint()
                );
                assert!(tree.find("header-new-task").unwrap().enabled);
                assert!(tree.find("header-title").is_none());
                let rail = tree.find("task-rail").unwrap().bounds;
                assert!(
                    (rail.width - expected_rail).abs() < 1.0,
                    "rail {} expected {expected_rail} at {width}x{height} {scale:?}",
                    rail.width
                );
                assert!(
                    tree.find("workspace-header").unwrap().bounds.height >= metrics::HEADER_HEIGHT
                );
                assert_eq!(
                    tree.find("connection-status").unwrap().value.as_deref(),
                    Some(t("rail.connection_local_connected"))
                );
                assert!(tree.find("reconnect").is_none());
                assert!(tree.find("connection-retry").is_none());
                assert!(tree.find("connection-notice").is_none());
                for id in [
                    "workspace-empty-title",
                    "workspace-empty-hint",
                    "header-new-task",
                    "add-task",
                    "project-scope",
                    "task-rail-grouping",
                    "open-settings",
                    "connection-status",
                ] {
                    let bounds = tree.find(id).unwrap().bounds;
                    assert!(
                        bounds.width > 0.0
                            && bounds.height > 0.0
                            && bounds.x >= 0.0
                            && bounds.y >= 0.0
                            && bounds.x + bounds.width <= width + 1.0
                            && bounds.y + bounds.height <= height + 1.0,
                        "{id} {:?} outside {width}x{height}",
                        bounds
                    );
                }
            });
        }
    }

    cx.simulate_resize(size(px(1440.0), px(1024.0)));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.text_scale = TextScale::Percent100;
            window.set_rem_size(px(TextScale::Percent100.rem_pixels()));
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "gui2-01".into(),
            });
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    press_published_ax(cx, &view, "header-new-task");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(
            view.projection.sessions.is_empty(),
            "homepage New task must not invent a session without a Host receipt"
        );
        assert!(view.open_menu.is_none());
        assert!(view.composer_focus_handle(cx).is_focused(window));
        assert_eq!(view.projection.active_session_id, None);
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.text_input
                .update(cx, |input, cx| input.reset_text("首页草稿", cx));
        })
    });

    let snapshot: pawork_client::Snapshot = serde_json::from_value(serde_json::json!({
        "instance_id": "gui2-01",
        "snapshot_sequence": 1,
        "generated_at": 1,
        "sections": [
            {"kind": "workspaces", "revision": 1, "data": [
                {"id": "ws-a", "name": "Alpha", "trusted": true}
            ]},
            {"kind": "session_tree", "revision": 1, "data": [
                {"session_id": "s-free", "title": "Free notes", "updated_at_ms": 2},
                {
                    "session_id": "s-alpha",
                    "title": "Alpha notes",
                    "workspace_id": "ws-a",
                    "updated_at_ms": 1
                }
            ]}
        ]
    }))
    .expect("workbench snapshot");
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.merge_snapshot(&snapshot);
            assert_eq!(view.projection.sessions.len(), 2);
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();

    press_published_ax(cx, &view, "session-s-free");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.no_session_draft, "首页草稿");
        assert_eq!(view.projection.active_session_id.as_deref(), Some("s-free"));
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        assert_eq!(tree.find("header-title").unwrap().label, "Free notes");
        assert_eq!(
            tree.find("composer-workspace").unwrap().value.as_deref(),
            Some(t("composer.no_project_chip"))
        );
        assert!(tree.find("session-s-free").unwrap().selected);
        assert!(!tree.find("session-s-alpha").unwrap().selected);
        assert!(view.text_input.read(cx).text().is_empty());
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.text_input
                .update(cx, |input, cx| input.reset_text("自由草稿", cx));
        })
    });
    press_published_ax(cx, &view, "session-s-alpha");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(
            view.composer_drafts.get("s-free").map(String::as_str),
            Some("自由草稿")
        );
        assert_eq!(
            view.projection.active_session_id.as_deref(),
            Some("s-alpha")
        );
        let tree = view.accessibility_tree(window, cx);
        assert_eq!(tree.find("header-title").unwrap().label, "Alpha notes");
        assert_eq!(
            tree.find("composer-workspace").unwrap().value.as_deref(),
            Some(
                t("composer.workspace_scope")
                    .replace("{}", "Alpha")
                    .as_str()
            )
        );
        assert!(tree.find("session-s-alpha").unwrap().selected);
        assert!(!tree.find("session-s-free").unwrap().selected);
        assert!(view.text_input.read(cx).text().is_empty());
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.text_input
                .update(cx, |input, cx| input.reset_text("项目草稿", cx));
        })
    });

    press_published_ax(cx, &view, "task-rail-grouping");
    press_published_ax(cx, &view, "task-rail-grouping");
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(view.grouping, TaskRailGrouping::Projects);
        assert!(view.open_menu.is_none());
        assert_eq!(
            view.projection.active_session_id.as_deref(),
            Some("s-alpha")
        );
        assert_eq!(view.text_input.read(cx).text(), "项目草稿");
        assert_eq!(view.no_session_draft, "首页草稿");
        assert_eq!(
            view.composer_drafts.get("s-free").map(String::as_str),
            Some("自由草稿")
        );
        assert!(view.collapsed_projects.is_empty());
    });

    press_published_ax(cx, &view, "project-ws-a");
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        assert_eq!(
            tree.find("project-ws-a").unwrap().description.as_deref(),
            Some("Collapsed")
        );
        assert!(tree.find("session-s-alpha").is_none());
        assert_eq!(
            view.projection.active_session_id.as_deref(),
            Some("s-alpha")
        );
        assert_eq!(view.text_input.read(cx).text(), "项目草稿");
        assert!(view.collapsed_projects.contains("ws-a"));
    });
    cx.update(|window, cx| {
        let handle = view
            .read(cx)
            .rail_row_focus
            .get("project-ws-a")
            .cloned()
            .expect("rendered project header owns a focus handle");
        window.focus(&handle);
    });
    cx.refresh().unwrap();
    cx.simulate_keystrokes("right");
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        assert_eq!(
            tree.find("project-ws-a").unwrap().description.as_deref(),
            Some("Expanded")
        );
        assert!(tree.find("session-s-alpha").unwrap().selected);
        assert_eq!(view.text_input.read(cx).text(), "项目草稿");
    });
    cx.simulate_keystrokes("left");
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        assert_eq!(
            tree.find("project-ws-a").unwrap().description.as_deref(),
            Some("Collapsed")
        );
        assert!(tree.find("session-s-alpha").is_none());
        assert_eq!(
            view.projection.active_session_id.as_deref(),
            Some("s-alpha")
        );
        assert_eq!(view.text_input.read(cx).text(), "项目草稿");
    });

    press_published_ax(cx, &view, "open-settings");
    cx.update(|_, cx| {
        assert_eq!(view.read(cx).route, AppRoute::Settings);
    });
    press_published_ax(cx, &view, "settings-back");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.route, AppRoute::Workspace);
        assert_eq!(
            view.projection.active_session_id.as_deref(),
            Some("s-alpha")
        );
        assert_eq!(view.text_input.read(cx).text(), "项目草稿");
        assert_eq!(
            view.accessibility_tree(window, cx)
                .find("header-title")
                .unwrap()
                .label,
            "Alpha notes"
        );
    });

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Failed {
                reason: "connection refused by test host".into(),
            });
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        assert_eq!(
            tree.find("connection-status").unwrap().value.as_deref(),
            Some(t("rail.connection_local_disconnected"))
        );
        let notice = tree.find("connection-notice").unwrap();
        assert!(notice.label.contains("connection refused by test host"));
        assert!(tree.find("reconnect").is_some());
        assert!(tree.find("connection-retry").is_some());
        assert_eq!(tree.find("header-title").unwrap().label, "Alpha notes");
        assert_eq!(view.text_input.read(cx).text(), "项目草稿");
    });

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connecting);
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        assert_eq!(
            tree.find("connection-status").unwrap().value.as_deref(),
            Some(t("rail.connection_local_connecting"))
        );
        assert!(tree.find("reconnect").is_none());
        assert!(tree.find("connection-retry").is_none());
        let notice = tree.find("connection-notice").unwrap().label.as_str();
        assert!(
            !notice.contains("connection refused by test host"),
            "connecting must not keep the old failure: {notice}"
        );
    });

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "gui2-01".into(),
            });
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        assert_eq!(
            tree.find("connection-status").unwrap().value.as_deref(),
            Some(t("rail.connection_local_connected"))
        );
        assert!(tree.find("connection-notice").is_none());
        assert!(tree.find("reconnect").is_none());
        assert!(tree.find("connection-retry").is_none());
        assert_eq!(view.text_input.read(cx).text(), "项目草稿");
    });

    // 折叠后的当前任务行不可见；展开后再核对 150% 窄窗的标题与动作框。
    press_published_ax(cx, &view, "project-ws-a");
    cx.simulate_resize(size(px(1080.0), px(720.0)));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.text_scale = TextScale::Percent150;
            window.set_rem_size(px(TextScale::Percent150.rem_pixels()));
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        let rail = tree.find("task-rail").unwrap().bounds;
        assert!((rail.width - shell_layout::RAIL_LARGE_TEXT_WIDTH).abs() < 1.0);
        let title = tree
            .find(&session_title_identifier("s-alpha"))
            .unwrap()
            .bounds;
        let rename = tree
            .find(&session_rename_identifier("s-alpha"))
            .unwrap()
            .bounds;
        let archive = tree
            .find(&session_archive_identifier("s-alpha"))
            .unwrap()
            .bounds;
        assert!(
            !ax_rects_overlap(title, rename),
            "title {title:?} rename {rename:?}"
        );
        assert!(
            !ax_rects_overlap(title, archive),
            "title {title:?} archive {archive:?}"
        );
        let project = tree.find("project-ws-a").unwrap().bounds;
        // 列表内容必须落在 AX 侧栏框内。底行不同：render 让左栏铺满窗口，
        // 状态栏只占右侧，AX 框停在状态栏上沿；150% 换行后的连接状态
        // 可以进入这条左侧带，只要不越栏宽、不出窗口、不盖住项目头。
        for (id, bounds) in [
            ("project-ws-a", project),
            ("session-title", title),
            ("rename", rename),
            ("archive", archive),
        ] {
            assert!(
                bounds.x + bounds.width <= rail.x + rail.width + 1.0
                    && bounds.y + bounds.height <= rail.y + rail.height + 1.0,
                "{id} {:?} outside rail {:?}",
                bounds,
                rail
            );
        }
        let status = tree.find("connection-status").unwrap().bounds;
        let window_height = f32::from(window.viewport_size().height);
        assert!(
            status.x >= rail.x - 1.0
                && status.x + status.width <= rail.x + rail.width + 1.0
                && status.y >= 0.0
                && status.y + status.height <= window_height + 1.0
                && !ax_rects_overlap(status, project),
            "connection-status {status:?} outside visual rail {rail:?} window {window_height}"
        );
        let header_title = tree.find("header-title").unwrap().bounds;
        let expand = tree.find("inspector-expand").unwrap().bounds;
        assert!(
            !ax_rects_overlap(header_title, expand),
            "header title {header_title:?} overlaps inspector-expand {expand:?}"
        );
    });
}

/// UX-05：20 项目录的当前项、搜索恢复与键盘滚动共用实际布局。
#[gpui::test]
fn model_menu_ax_culls_rows_outside_clipped_frame(cx: &mut gpui::TestAppContext) {
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("ux05-model-menu.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.projection.set_models(
                (0..20)
                    .map(|ix| ModelEntry {
                        provider_id: "test-provider".into(),
                        id: format!("model-{ix:02}"),
                        display_name: format!("中文 Model {ix:02}"),
                        ..ModelEntry::default()
                    })
                    .collect(),
            );
            view.projection.settings_providers.providers =
                vec![crate::projection::ProviderAuthStatusEntry {
                    provider_id: "test-provider".into(),
                    display_name: "Test Provider".into(),
                    endpoint_label: String::new(),
                    auth_methods: vec!["api_key".into()],
                    credentials: Vec::new(),
                    selection_mode: Default::default(),
                    auth: crate::projection::ProviderAuthState::Connected {
                        method: "api_key".into(),
                        masked_credential: None,
                    },
                    catalog: crate::projection::ProviderCatalogState::FixedFallback {
                        snapshot_label: "test".into(),
                        fetched_at: None,
                    },
                    use_proxy: true,
                }];
            view.projection
                .set_pending_model("test-provider".into(), "model-17".into());
            view.text_input
                .update(cx, |input, cx| input.reset_text("保留草稿", cx));
            view.on_toggle_model_menu(None, window, cx);
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    cx.run_until_parked();
    for (width, height, scale) in [
        (1440.0, 1024.0, font::TextScale::Percent100),
        (1080.0, 720.0, font::TextScale::Percent125),
        (1080.0, 720.0, font::TextScale::Percent150),
    ] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.text_scale = scale;
                window.set_rem_size(gpui::px(scale.rem_pixels()));
                view.pending_model_menu_scroll = true;
                cx.notify();
            })
        });
        for _ in 0..2 {
            cx.refresh().unwrap();
            cx.run_until_parked();
        }
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            let current = tree
                .find(&model_identifier(&view.projection.models[17]))
                .expect("current model visible on open and resize");
            assert!(current.selected && current.bounds.height > 0.0);
            assert_eq!(current.value.as_deref(), Some("test-provider / model-17"));
            assert!(current
                .description
                .as_deref()
                .unwrap()
                .contains("Test Provider"));
            let actual = view.settings_element_layouts["model-test-provider-model-17"].bounds();
            assert!(f32::from(actual.size.height) >= 48.0);
            let menu = tree.find("model-menu").unwrap();
            assert!((menu.bounds.width - 360.0).abs() < 1.0, "{:?}", menu.bounds);
            assert!(menu.bounds.height <= 480.0);
            assert!(menu.bounds.x >= 0.0 && menu.bounds.x + menu.bounds.width <= width);
            assert!(menu.bounds.y >= 0.0 && menu.bounds.y + menu.bounds.height <= height);
            let manage = tree.find("model-menu-settings").unwrap();
            assert!(tree.find("model-search-input").unwrap().focused);
            assert!(!current.focused, "highlight must not replace search focus");
            assert!(!manage.focused);
            assert!(manage.bounds.height >= 36.0);
            assert!(manage.bounds.y + manage.bounds.height <= menu.bounds.y + menu.bounds.height);
            assert!(menu.children.len() < 20);
            assert!(tree.find("model-search-input").unwrap().bounds.height > 0.0);
        });
    }
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(
                AxRequest {
                    identifier: "model-search-input".into(),
                    action: AxAction::SetValue,
                    value: Some("MODEL-03".into()),
                },
                window,
                cx,
            );
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(view.filtered_model_entries().len(), 1);
            view.on_send_message(&crate::ui::SendMessage, window, cx);
            assert_eq!(view.projection.effective_model().unwrap().1, "model-03");
            assert_eq!(view.text_input.read(cx).text(), "保留草稿");
            view.on_toggle_model_menu(None, window, cx);
            view.model_search_input
                .update(cx, |input, cx| input.set_text("不存在的模型", cx));
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let tree = view.accessibility_tree(window, cx);
            assert_eq!(
                tree.find("model-menu-empty").unwrap().label,
                t("model_search.no_results")
            );
            view.handle_accessibility_request(
                AxRequest {
                    identifier: "model-search-clear".into(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(view.filtered_model_entries().len(), 20);
            view.handle_accessibility_request(
                AxRequest {
                    identifier: ModelPurposeFilter::VideoInput.identifier(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            assert_eq!(
                tree.find("model-menu-empty").unwrap().label,
                t("model_purpose.no_models")
            );
            let video_id = ModelPurposeFilter::VideoInput.identifier();
            let video = tree.find(&video_id).unwrap();
            assert!(video.selected && video.bounds.height > 0.0);
            view.handle_accessibility_request(
                AxRequest {
                    identifier: video_id.clone(),
                    action: AxAction::Focus,
                    value: None,
                },
                window,
                cx,
            );
            assert!(view.settings_action_focus[&video_id].is_focused(window));
            view.handle_accessibility_request(
                AxRequest {
                    identifier: ModelPurposeFilter::Conversation.identifier(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
            assert_eq!(view.projection.effective_model().unwrap().1, "model-03");
            assert_eq!(view.filtered_model_entries().len(), 20);
            for _ in 0..12 {
                view.move_menu_highlight(true);
            }
            cx.notify();
            assert!(view.model_search_focus.is_focused(window));
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let highlighted = view.menu_highlight.unwrap();
            let tree = view.accessibility_tree(window, cx);
            assert!(tree
                .find(&model_identifier(&view.projection.models[highlighted]))
                .is_some());
            view.close_menu_and_focus_trigger(MenuKind::Model, window, cx);
            assert!(view.model_focus.is_focused(window));
        })
    });
}

/// AC-06：鼠标点用途 chip，键盘 Tab/Enter 切用途，键盘输入搜索。
/// 目录是 Host 边界 fixture，没有 GUI client。pending_model 只是本地选择，
/// selected_model 保持空，不能当成 Host 切模成功。筛选和搜索不得改这组选择。
/// Settings 四类能力徽标从 Manage models 弹层的实际 AX 读取。
#[gpui::test]
fn model_purpose_menu_input_keeps_model_and_renders_settings_badges(cx: &mut gpui::TestAppContext) {
    use gpui::{point, px, size};

    use crate::projection::{ProviderAuthState, ProviderAuthStatusEntry, ProviderCatalogState};
    use crate::ui::settings::{
        settings_manage_models_identifier, settings_model_effort_default_identifier,
        settings_model_switch_identifier, settings_models_menu_identifier,
        settings_provider_expand_identifier,
    };

    struct LanguageReset(crate::ui::i18n::Language);
    impl Drop for LanguageReset {
        fn drop(&mut self) {
            crate::ui::i18n::set_language(self.0);
        }
    }
    let _language = LanguageReset(crate::ui::i18n::language());
    crate::ui::i18n::set_language(crate::ui::i18n::Language::Chinese);

    fn model(
        provider: &str,
        id: &str,
        name: &str,
        text: bool,
        image_input: bool,
        image_output: bool,
        video_input: bool,
        web_search: bool,
    ) -> ModelEntry {
        ModelEntry {
            provider_id: provider.into(),
            id: id.into(),
            display_name: name.into(),
            text,
            image_input,
            image_output,
            video_input,
            web_search,
            ..ModelEntry::default()
        }
    }

    let plain = model("acme", "plain", "Plain", true, false, false, false, false);
    let eyes = model("acme", "eyes", "Eyes", true, true, false, false, true);
    let clip = model("acme", "clip", "Clip", true, false, false, true, false);
    let paint = model("acme", "paint", "Paint", false, false, true, false, false);
    let ghost = model(
        "offline", "ghost", "Ghost", true, false, false, false, false,
    );
    let plain_id = model_identifier(&plain);
    let eyes_id = model_identifier(&eyes);
    let clip_id = model_identifier(&clip);
    let paint_id = model_identifier(&paint);
    let ghost_id = model_identifier(&ghost);

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("ac06-model-purpose.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    cx.simulate_resize(size(px(1440.0), px(2400.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            // 无 client。连接态和目录都是投影 fixture，不是 model_list 回执。
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "ac06-fixture".into(),
            });
            view.projection.settings_providers.providers = vec![
                ProviderAuthStatusEntry {
                    provider_id: "acme".into(),
                    display_name: "Acme".into(),
                    endpoint_label: String::new(),
                    auth_methods: vec!["api_key".into()],
                    credentials: Vec::new(),
                    selection_mode: Default::default(),
                    auth: ProviderAuthState::Connected {
                        method: "api_key".into(),
                        masked_credential: None,
                    },
                    catalog: ProviderCatalogState::FixedFallback {
                        snapshot_label: "ac06-fixture".into(),
                        fetched_at: None,
                    },
                    use_proxy: true,
                },
                ProviderAuthStatusEntry {
                    provider_id: "offline".into(),
                    display_name: "Offline".into(),
                    endpoint_label: String::new(),
                    auth_methods: vec!["api_key".into()],
                    credentials: Vec::new(),
                    selection_mode: Default::default(),
                    auth: ProviderAuthState::None,
                    catalog: ProviderCatalogState::Unavailable {
                        error: "fixture-offline".into(),
                        fetched_at: None,
                    },
                    use_proxy: true,
                },
            ];
            view.projection
                .set_models(vec![plain, eyes, clip, paint, ghost]);
            view.projection
                .settings_providers
                .apply_model_catalog(view.projection.models.clone());
            view.projection
                .set_pending_model("acme".into(), "plain".into());
            view.text_input
                .update(cx, |input, cx| input.reset_text("保留草稿", cx));
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();

    let assert_held = |cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(
                view.projection.selected_model.is_none(),
                "fixture must not gain a Host-confirmed model"
            );
            assert_eq!(
                view.projection
                    .pending_model
                    .as_ref()
                    .map(|(provider, id)| { (provider.as_str(), id.as_str()) }),
                Some(("acme", "plain"))
            );
            assert_eq!(
                view.projection
                    .effective_model()
                    .map(|(provider, id)| (provider.as_str(), id.as_str())),
                Some(("acme", "plain"))
            );
            assert_eq!(view.text_input.read(cx).text(), "保留草稿");
        });
    };
    let row_present = |cx: &mut gpui::VisualTestContext, id: &str| {
        cx.update(|window, cx| {
            view.read(cx)
                .accessibility_tree(window, cx)
                .find(id)
                .is_some()
        })
    };

    click_published_center(cx, &view, "model-picker");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(matches!(view.open_menu, Some(MenuKind::Model)));
        assert_eq!(view.model_purpose_filter, ModelPurposeFilter::Conversation);
        let tree = view.accessibility_tree(window, cx);
        let chat = ModelPurposeFilter::Conversation.identifier();
        assert_eq!(tree.find(&chat).unwrap().label, "对话");
        assert!(tree.find(&chat).unwrap().selected);
        assert!(tree.find(&plain_id).is_some());
        assert!(tree.find(&eyes_id).is_some());
        assert!(tree.find(&clip_id).is_some());
        assert!(tree.find(&paint_id).is_none());
        assert!(tree.find(&ghost_id).is_none());
    });
    assert_held(cx);

    // 未输入、未移动高亮时 Enter 保持当前本地选择，菜单不关。
    cx.simulate_keystrokes("enter");
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(matches!(view.read(cx).open_menu, Some(MenuKind::Model)));
    });
    assert_held(cx);

    let vision = ModelPurposeFilter::ImageInput.identifier();
    click_published_center(cx, &view, &vision);
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.model_purpose_filter, ModelPurposeFilter::ImageInput);
        assert!(view.model_search_focus.is_focused(window));
        assert!(view.menu_highlight.is_none());
        assert!(view.model_search_query.is_empty());
        let tree = view.accessibility_tree(window, cx);
        assert!(tree.find(&vision).unwrap().selected);
        assert!(tree
            .find(&eyes_id)
            .unwrap()
            .description
            .as_deref()
            .unwrap()
            .ends_with("识图 · 搜索"));
        assert!(tree.find(&plain_id).is_none());
        assert!(tree.find(&clip_id).is_none());
        assert!(tree.find(&paint_id).is_none());
    });
    assert_held(cx);

    // 当前模型已被用途筛掉，但高亮仍未移动：Enter 仍不得改选。
    cx.simulate_keystrokes("enter");
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(view.model_purpose_filter, ModelPurposeFilter::ImageInput);
        assert!(matches!(view.open_menu, Some(MenuKind::Model)));
    });
    assert_held(cx);

    let video = ModelPurposeFilter::VideoInput.identifier();
    let mut tab_path = Vec::new();
    let mut reached_video = false;
    for _ in 0..80 {
        cx.simulate_keystrokes("tab");
        cx.refresh().unwrap();
        cx.run_until_parked();
        let focused = cx.update(|window, cx| {
            view.read(cx)
                .settings_action_focus
                .iter()
                .filter(|(_, focus)| focus.is_focused(window))
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>()
        });
        reached_video = focused.iter().any(|id| id == &video);
        tab_path.push(focused.join("+"));
        if reached_video {
            break;
        }
    }
    assert!(reached_video, "tab never focused video chip: {tab_path:?}");
    cx.simulate_keystrokes("enter");
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.model_purpose_filter, ModelPurposeFilter::VideoInput);
        assert!(view.model_search_focus.is_focused(window));
        assert!(view.model_search_query.is_empty());
        let tree = view.accessibility_tree(window, cx);
        assert!(tree.find(&video).unwrap().selected);
        assert!(tree
            .find(&clip_id)
            .unwrap()
            .description
            .as_deref()
            .unwrap()
            .ends_with("视频"));
        assert!(tree.find(&eyes_id).is_none());
        assert!(tree.find(&plain_id).is_none());
    });
    assert_held(cx);

    cx.simulate_input("clip");
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(view.model_search_query, "clip");
        assert_eq!(view.model_purpose_filter, ModelPurposeFilter::VideoInput);
    });
    assert!(row_present(cx, &clip_id));
    assert!(!row_present(cx, &eyes_id));
    assert_held(cx);

    click_published_center(cx, &view, "model-search-clear");
    cx.simulate_input("eyes");
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.model_search_query, "eyes");
        assert_eq!(view.model_purpose_filter, ModelPurposeFilter::VideoInput);
        let tree = view.accessibility_tree(window, cx);
        assert_eq!(
            tree.find("model-menu-empty").unwrap().label,
            t("model_search.no_results")
        );
        assert!(tree.find(&clip_id).is_none());
        assert!(tree.find(&eyes_id).is_none());
        assert!(tree.find(&paint_id).is_none());
        assert!(matches!(view.open_menu, Some(MenuKind::Model)));
    });
    assert_held(cx);

    // 不走 on_open_settings：无 client 时那条路径会把 fixture 标成 stale，
    // Manage models 随之禁用。这里只切路由，随后鼠标走正式展开和弹层。
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.route = AppRoute::Settings;
            view.settings_page = SettingsPage::Providers;
            view.open_menu = None;
            cx.notify();
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    let expand = settings_provider_expand_identifier("acme");
    click_published_center(cx, &view, &expand);
    let manage = settings_manage_models_identifier("acme");
    click_published_center(cx, &view, &manage);
    cx.update(|_, cx| {
        assert!(matches!(
            &view.read(cx).open_menu,
            Some(MenuKind::SettingsProviderModels(provider)) if provider == "acme"
        ));
        assert!(view.read(cx).model_search_query.is_empty());
    });
    assert_held(cx);

    let list_id = format!("{}-list", settings_models_menu_identifier("acme"));
    let reveal = |cx: &mut gpui::VisualTestContext, id: &str| {
        let read = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                view.read(cx)
                    .accessibility_tree(window, cx)
                    .find(id)
                    .map(|node| {
                        (
                            node.description.clone().unwrap_or_default(),
                            node.bounds.height,
                        )
                    })
            })
        };
        let scroll = |cx: &mut gpui::VisualTestContext, bottom: bool| {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    let handle = view
                        .settings_element_layouts
                        .get(&list_id)
                        .unwrap_or_else(|| panic!("missing {list_id}"));
                    if bottom {
                        handle.scroll_to_bottom();
                    } else {
                        handle.set_offset(point(px(0.0), px(0.0)));
                    }
                    cx.notify();
                });
            });
            cx.refresh().unwrap();
            cx.run_until_parked();
        };
        scroll(cx, false);
        if let Some((description, height)) = read(cx) {
            if height > 1.0 {
                return (description, height);
            }
        }
        scroll(cx, true);
        if let Some((description, height)) = read(cx) {
            if height > 1.0 {
                return (description, height);
            }
        }
        // 首尾都裁掉中间行时，把该行对齐到列表顶部再读。
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let item = view
                    .settings_element_layouts
                    .get(id)
                    .unwrap_or_else(|| panic!("missing layout {id}"))
                    .bounds();
                let handle = view
                    .settings_element_layouts
                    .get(&list_id)
                    .unwrap_or_else(|| panic!("missing {list_id}"));
                let list = handle.bounds();
                let offset = handle.offset();
                handle.set_offset(point(offset.x, offset.y - (item.origin.y - list.origin.y)));
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        read(cx).unwrap_or_else(|| panic!("missing settings candidate {id}"))
    };

    let paint_switch = settings_model_switch_identifier("acme", "paint");
    let eyes_switch = settings_model_switch_identifier("acme", "eyes");
    let clip_switch = settings_model_switch_identifier("acme", "clip");
    let plain_switch = settings_model_switch_identifier("acme", "plain");
    let (paint_badge, paint_height) = reveal(cx, &paint_switch);
    cx.update(|window, cx| {
        assert!(view
            .read(cx)
            .accessibility_tree(window, cx)
            .find(&settings_model_effort_default_identifier("acme", "paint"))
            .is_none());
    });
    let (eyes_badge, eyes_height) = reveal(cx, &eyes_switch);
    let (clip_badge, clip_height) = reveal(cx, &clip_switch);
    let (plain_badge, plain_height) = reveal(cx, &plain_switch);
    assert!(paint_height > 1.0 && eyes_height > 1.0);
    assert!(clip_height > 1.0 && plain_height > 1.0);
    assert_eq!(paint_badge, "acme/paint · 生图");
    assert_eq!(eyes_badge, "acme/eyes · 识图 · 搜索");
    assert_eq!(clip_badge, "acme/clip · 视频");
    assert_eq!(plain_badge, "acme/plain");
    cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        assert!(tree
            .find(&settings_model_effort_default_identifier("acme", "plain"))
            .is_some());
        assert!(tree
            .find(&settings_model_switch_identifier("offline", "ghost"))
            .is_none());
    });
    assert_held(cx);
}

/// OPT-3 切片 1：全关空态的诚实呈现——加载中不得误报「无已启用
/// 模型」；目录查询完成且为空时 picker 仍可开菜单看说明行、发送
/// fail-closed，且不发布任何可选模型行。
#[gpui::test]
fn model_menu_empty_state_is_honest_and_fail_closed(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;

    struct AxEmptyHost {
        view: gpui::Entity<AppView>,
    }
    impl gpui::Render for AxEmptyHost {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("opt3-model-menu-empty.sock");
    let (host, cx) = cx.add_window_view(|_window, cx| {
        let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
        AxEmptyHost { view }
    });
    let view = cx.update(|_window, cx| host.read(cx).view.clone());
    // 已连接 + 激活会话，但目录查询未完成：保持 loading 语义。
    cx.update(|_window, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.projection.active_session_id = Some("s-1".into());
            view.text_input
                .update(cx, |input, cx| input.reset_text("hello", cx));
        });
    });
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().expect("loading AX tree validates");
        let picker = tree
            .find("model-picker")
            .expect("model picker has an AX node");
        assert!(!picker.enabled);
        assert_eq!(picker.value.as_deref(), Some(t("composer.model_loading")));
        // 加载中不误报全关：发送仍可用（现有行为，Host 侧权威默认）。
        assert!(tree.find("send").expect("send has an AX node").enabled);
    });
    // 目录查询完成且结果为空：进入全关空态。
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.projection.set_models(Vec::new());
            view.open_menu = Some(MenuKind::Model);
        });
    });
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().expect("empty-state AX tree validates");
        let picker = tree
            .find("model-picker")
            .expect("model picker has an AX node");
        assert!(picker.enabled);
        assert_eq!(picker.value.as_deref(), Some("No enabled models"));
        assert!(tree.permits(&AxRequest {
            identifier: "model-picker".into(),
            action: AxAction::Press,
            value: None,
        }));
        // 全关空态 fail-closed：无已启用模型不发送。
        let send = tree.find("send").expect("send has an AX node");
        assert!(!send.enabled);
        assert!(!tree.permits(&AxRequest {
            identifier: "send".into(),
            action: AxAction::Press,
            value: None,
        }));
        let menu = tree.find("model-menu").expect("model menu has an AX node");
        let empty = tree
            .find("model-menu-empty")
            .expect("empty menu publishes its explanation");
        assert_eq!(empty.role, AxRole::StaticText);
        // 说明行不发布 Press（disabled 节点无 Press 契约）。
        assert!(empty.actions.is_empty());
        assert_eq!(empty.value.as_deref(), Some(t("composer.model_menu_empty")));
        // 菜单不发布任何可选模型行：不编造模型。
        assert_eq!(menu.children.len(), 3 + ModelPurposeFilter::ALL.len());
        assert!(menu
            .children
            .iter()
            .all(|child| child.role != AxRole::Button
                || child.identifier == "model-menu-settings"
                || ModelPurposeFilter::ALL
                    .iter()
                    .any(|purpose| purpose.identifier() == child.identifier)));
    });
    // 旧当前值不在候选目录时如实说明，不偷偷选择目录中的首项。
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_models(vec![ModelEntry {
                provider_id: "available".into(),
                id: "new".into(),
                ..ModelEntry::default()
            }]);
            view.projection
                .set_pending_model("old-provider".into(), "old-model".into());
            let tree = view.accessibility_tree(window, cx);
            let notice = tree.find("model-menu-current-notice").unwrap();
            assert_eq!(notice.value.as_deref(), Some("old-provider / old-model"));
            assert!(notice.actions.is_empty());
            assert_eq!(view.projection.effective_model().unwrap().1, "old-model");
            assert_eq!(view.text_input.read(cx).text(), "hello");
        });
    });
}

/// OPT-3b / ADR-055 D5：Default models 四角色区 AX 形状——四个触发器
/// identifier / role / label / value 稳定，菜单发布清除与已连接
/// provider 候选（未连接组不出现），Vision / Search 如实标注只保存；
/// stale 时触发器禁用且 Press 被拒。
#[gpui::test]
fn settings_role_defaults_ax_shape_pins_triggers_menu_and_filter(cx: &mut gpui::TestAppContext) {
    use gpui::{prelude::*, px, AppContext};

    use crate::projection::{
        ProviderAuthState, ProviderAuthStatusEntry, ProviderCatalogState, SettingsRole,
    };
    use crate::ui::settings::{
        settings_role_clear_identifier, settings_role_item_identifier,
        settings_role_trigger_identifier,
    };

    struct AxRolesHost {
        view: gpui::Entity<AppView>,
    }
    impl gpui::Render for AxRolesHost {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> impl gpui::IntoElement {
            window.set_rem_size(px(16.0));
            gpui::div()
                .size_full()
                .flex()
                .child(self.view.update(cx, |view, cx| {
                    view.ensure_settings_api_key_inputs(cx);
                    view.settings_page_element(cx)
                }))
        }
    }

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("opt3b-role-defaults-ax.sock");
    let (host, cx) = cx.add_window_view(|_window, cx| {
        let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
        AxRolesHost { view }
    });
    // Host 直接装配子 view 的元素，须在每次验收前通知 Host 重绘；
    // 单独 refresh window 不会使已缓存的 Host render 失效。
    // 语义断言覆盖完整页面；窄窗与菜单裁剪另在本测试内实际滚动。
    cx.simulate_resize(gpui::size(px(1440.0), px(2400.0)));
    let view = cx.update(|_window, cx| host.read(cx).view.clone());
    let provider = |provider_id: &str, auth: ProviderAuthState| ProviderAuthStatusEntry {
        provider_id: provider_id.to_string(),
        display_name: provider_id.to_string(),
        endpoint_label: String::new(),
        auth_methods: vec!["api_key".to_string()],
        credentials: Vec::new(),
        selection_mode: Default::default(),
        auth,
        catalog: ProviderCatalogState::Unavailable {
            error: "offline".to_string(),
            fetched_at: None,
        },
        use_proxy: true,
    };
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.route = AppRoute::Settings;
            view.projection.settings_providers.apply_loaded(
                crate::projection::ProviderAuthStatusData {
                    providers: vec![
                        provider(
                            "kimi",
                            ProviderAuthState::Connected {
                                method: "api_key".to_string(),
                                masked_credential: None,
                            },
                        ),
                        provider("glm", ProviderAuthState::None),
                    ],
                    default: None,
                    role_defaults: Default::default(),
                },
            );
            view.projection.set_models(vec![
                ModelEntry {
                    provider_id: "kimi".into(),
                    id: "kimi-k2".into(),
                    display_name: "Kimi K2".into(),
                    ..ModelEntry::default()
                },
                ModelEntry {
                    provider_id: "glm".into(),
                    id: "glm-4.7".into(),
                    display_name: "GLM 4.7".into(),
                    ..ModelEntry::default()
                },
                ModelEntry {
                    provider_id: "ghost".into(),
                    id: "ghost-x".into(),
                    display_name: "Ghost X".into(),
                    ..ModelEntry::default()
                },
            ]);
            view.open_menu = Some(MenuKind::SettingsRole(SettingsRole::Naming));
        });
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);

        tree.validate().expect("role defaults AX tree validates");
        let ordered = view.settings_ordered_providers();
        assert!(ordered.windows(2).all(|pair| {
            matches!(pair[0].auth, ProviderAuthState::Connected { .. })
                || !matches!(pair[1].auth, ProviderAuthState::Connected { .. })
        }));
        for role in SettingsRole::ALL {
            let identifier = settings_role_trigger_identifier(role);
            let trigger = tree
                .find(&identifier)
                .unwrap_or_else(|| panic!("{identifier} missing from AX tree"));
            assert_eq!(trigger.role, AxRole::Button);
            assert_eq!(trigger.label, role.label());
            assert_eq!(
                trigger.value.as_deref(),
                Some(view.settings_role_value_label(role).as_str())
            );
            assert!(trigger.enabled);
            // UX-06：供应商先呈现，角色偏好置于列表之后。
            let heading = tree
                .find("settings-providers-heading")
                .expect("providers heading has an AX node");
            assert!(
                trigger.bounds.y > heading.bounds.y,
                "{identifier} must sit below the providers list"
            );
            assert!(tree.permits(&AxRequest {
                identifier: identifier.clone(),
                action: AxAction::Press,
                value: None,
            }));
        }
        // Vision / Search 落地期只保存：说明如实标注，不暗示已生效。
        let vision = tree
            .find(&settings_role_trigger_identifier(SettingsRole::Vision))
            .expect("vision trigger has an AX node");
        assert!(vision
            .description
            .as_deref()
            .is_some_and(|description| description.contains("Not active")));
        let search = tree
            .find(&settings_role_trigger_identifier(SettingsRole::Search))
            .expect("search trigger has an AX node");
        assert!(search
            .description
            .as_deref()
            .is_some_and(|description| description.contains("Not active")));

        // 菜单展开：清除行 + 已连接 provider 候选可选。
        let clear_id = settings_role_clear_identifier(SettingsRole::Naming);
        let clear = tree.find(&clear_id).expect("clear row has an AX node");
        assert_eq!(clear.label, "Clear");
        assert!(tree.permits(&AxRequest {
            identifier: clear_id,
            action: AxAction::Press,
            value: None,
        }));
        let item_id = settings_role_item_identifier(SettingsRole::Naming, "kimi", "kimi-k2");
        let item = tree.find(&item_id).expect("kimi item has an AX node");
        assert_eq!(item.value.as_deref(), Some("kimi / kimi-k2"));
        assert!(tree.permits(&AxRequest {
            identifier: item_id,
            action: AxAction::Press,
            value: None,
        }));
        // 未连接 provider（glm）与清单缺失 provider（ghost）不出现。
        assert!(tree
            .find(&settings_role_item_identifier(
                SettingsRole::Naming,
                "glm",
                "glm-4.7"
            ))
            .is_none());
        assert!(tree
            .find(&settings_role_item_identifier(
                SettingsRole::Naming,
                "ghost",
                "ghost-x"
            ))
            .is_none());
    });
    // 两个供应商分组的长菜单：打开时当前末项可见，上下键跨越
    // Clear / 分组边界时新高亮项始终可见，AX 框仍来自真实菜单视口。
    let original_models = cx.update(|_, cx| view.read(cx).projection.models.clone());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.projection.settings_providers.providers[1].auth = ProviderAuthState::Connected {
                method: "api_key".into(),
                masked_credential: None,
            };
            let models = ["kimi", "glm"]
                .into_iter()
                .flat_map(|provider| {
                    (0..12).map(move |ix| ModelEntry {
                        provider_id: provider.into(),
                        id: format!("long-{ix}"),
                        display_name: format!("Model {ix}"),
                        ..ModelEntry::default()
                    })
                })
                .collect();
            view.projection.set_models(models);
            view.projection
                .settings_providers
                .confirm_role_default(SettingsRole::Naming, Some(("glm".into(), "long-11".into())));
            view.open_menu = None;
            // AX 打开前焦点可能仍在已卸载的模型搜索框；打开角色菜单必须
            // 将真实焦点放回触发器，原生 Escape 才能冒泡到 AppView。
            window.focus(&view.model_search_focus);
            let identifier = settings_role_trigger_identifier(SettingsRole::Naming);
            view.handle_accessibility_request(
                AxRequest {
                    identifier: identifier.clone(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
            assert!(view.settings_action_focus[&identifier].is_focused(window));
        });
    });
    // 首帧建立视口；defer 定位当前项，第二帧执行滚动。
    for _ in 0..2 {
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
    }
    let last_id = settings_role_item_identifier(SettingsRole::Naming, "glm", "long-11");
    let clear_id = settings_role_clear_identifier(SettingsRole::Naming);
    let menu_id = "settings-role-menu-naming";
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        let last = tree
            .find(&last_id)
            .expect("current last model scrolls into view on open");
        assert!(last.selected && last.focused);
        assert!(tree.find(&clear_id).is_none());
    });
    for (forward, expected_id) in [
        (true, clear_id.clone()), // last → Clear
        (false, last_id.clone()), // Clear → last
        (
            false,
            settings_role_item_identifier(SettingsRole::Naming, "glm", "long-10"),
        ),
    ] {
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.move_menu_highlight(forward);
                cx.notify();
            })
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            let node = tree
                .find(&expected_id)
                .expect("keyboard highlight remains visible");
            assert!(node.focused);
            let actual = view.settings_element_layouts[&expected_id].bounds();
            let menu = view.settings_element_layouts[menu_id].bounds();
            assert!(actual.top() >= menu.top() && actual.bottom() <= menu.bottom());
            assert_eq!(
                node.bounds,
                AxRect::new(
                    actual.origin.x.into(),
                    actual.origin.y.into(),
                    actual.size.width.into(),
                    actual.size.height.into()
                )
            );
        });
    }
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_models(original_models);
            view.projection.settings_providers.providers[1].auth = ProviderAuthState::None;
            view.open_menu = None;
            view.settings_element_layouts.remove(menu_id);
            cx.notify();
        })
    });

    // 清除路径分别从 AX 和键盘进入，每次先渲染对应的真实空菜单。
    let role = SettingsRole::Naming;
    let saved = Some(("kimi".to_string(), "kimi-k2".to_string()));
    for empty_catalog in [false, true] {
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                if empty_catalog {
                    view.projection.set_models(vec![]);
                    view.projection.settings_providers.providers[0].auth =
                        ProviderAuthState::Connected {
                            method: "api_key".into(),
                            masked_credential: None,
                        };
                } else {
                    view.projection.settings_providers.providers[0].auth = ProviderAuthState::None;
                }
                view.projection
                    .settings_providers
                    .confirm_role_default(role, saved.clone());
                view.settings_role_pending = None;
                view.open_menu = Some(MenuKind::SettingsRole(role));
                view.menu_highlight = None;
                assert_eq!(view.menu_item_count(), 1);
                view.move_menu_highlight(true);
                assert_eq!(view.menu_highlight, Some(0));
                cx.notify();
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                tree.validate().expect("empty role menu AX tree validates");
                let clear_id = settings_role_clear_identifier(role);
                let clear = tree
                    .find(&clear_id)
                    .expect("Clear survives empty candidates");
                let empty = tree
                    .find("settings-role-menu-empty-naming")
                    .expect("empty candidates retain the explanation");
                assert!(empty.bounds.y >= clear.bounds.y + clear.bounds.height);
                let request = AxRequest {
                    identifier: clear_id,
                    action: AxAction::Press,
                    value: None,
                };
                assert!(tree.permits(&request));
                if empty_catalog {
                    view.activate_menu_item(MenuKind::SettingsRole(role), 0, window, cx);
                } else {
                    view.handle_accessibility_request(request, window, cx);
                }
                assert_eq!(view.settings_role_pending, Some(role));
                assert!(view.open_menu.is_none());
                assert_eq!(
                    view.projection.settings_providers.role_value(role),
                    saved.as_ref()
                );
            });
        });
    }
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.settings_role_pending = None;
            view.open_menu = Some(MenuKind::SettingsRole(role));
            cx.notify();
        });
    });
    // 断线 stale：角色写禁用，Press fail-closed。
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.projection
                .settings_providers
                .mark_stale("socket closed");
        });
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        let identifier = settings_role_trigger_identifier(SettingsRole::Naming);
        let trigger = tree
            .find(&identifier)
            .expect("naming trigger still present");
        assert!(!trigger.enabled);
        assert!(!tree.permits(&AxRequest {
            identifier,
            action: AxAction::Press,
            value: None,
        }));
    });
}

/// OPT-3a / ADR-055 D2-D3：Manage models 弹层与代理 Switch 的 AX 形
/// 状——Manage 入口 gate（未连接禁用不发布 Press）、弹层 Group + 每模
/// 型 Switch（checked 进 value/selected）、Enable/Disable all 与常驻
/// Refresh 的可操作性、代理 Switch value 与 Press；stale 关闸。
#[gpui::test]
fn settings_models_menu_ax_pins_gates_switches_and_empty_state(cx: &mut gpui::TestAppContext) {
    use gpui::{prelude::*, px, AppContext};

    use crate::projection::{
        ModelEntry, ProviderAuthState, ProviderAuthStatusEntry, ProviderCatalogState,
    };
    use crate::ui::settings::{
        settings_manage_models_identifier, settings_model_switch_identifier,
        settings_models_disable_all_identifier, settings_models_enable_all_identifier,
        settings_models_menu_identifier, settings_models_refresh_identifier,
        settings_use_proxy_identifier,
    };

    struct AxModelsHost {
        view: gpui::Entity<AppView>,
    }
    impl gpui::Render for AxModelsHost {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> impl gpui::IntoElement {
            window.set_rem_size(px(self.view.read(cx).text_scale.rem_pixels()));
            gpui::div()
                .size_full()
                .flex()
                .child(self.view.update(cx, |view, cx| {
                    view.ensure_settings_api_key_inputs(cx);
                    view.settings_page_element(cx)
                }))
        }
    }

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("opt3a-models-menu-ax.sock");
    let (host, cx) = cx.add_window_view(|_window, cx| {
        let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
        AxModelsHost { view }
    });
    // Host 直接装配子 view 的元素，须在每次验收前通知 Host 重绘；
    // 单独 refresh window 不会使已缓存的 Host render 失效。
    // 语义断言覆盖完整页面；窄窗与菜单裁剪另在本测试内实际滚动。
    cx.simulate_resize(gpui::size(px(1440.0), px(2400.0)));
    let view = cx.update(|_window, cx| host.read(cx).view.clone());
    let catalog_remote = || ProviderCatalogState::Remote {
        fetched_at: "2026-09-05T00:00:00Z".into(),
    };
    let provider = |provider_id: &str, auth: ProviderAuthState| ProviderAuthStatusEntry {
        provider_id: provider_id.to_string(),
        display_name: provider_id.to_string(),
        endpoint_label: String::new(),
        auth_methods: vec!["api_key".to_string()],
        credentials: Vec::new(),
        selection_mode: Default::default(),
        auth,
        catalog: catalog_remote(),
        use_proxy: true,
    };
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.route = AppRoute::Settings;
            view.projection.settings_providers.apply_loaded(
                crate::projection::ProviderAuthStatusData {
                    providers: vec![
                        provider(
                            "kimi",
                            ProviderAuthState::Connected {
                                method: "api_key".to_string(),
                                masked_credential: None,
                            },
                        ),
                        provider("glm", ProviderAuthState::None),
                        provider(
                            "empty",
                            ProviderAuthState::Connected {
                                method: "api_key".to_string(),
                                masked_credential: None,
                            },
                        ),
                    ],
                    default: None,
                    role_defaults: Default::default(),
                },
            );
            view.projection.settings_providers.apply_model_catalog(vec![
                ModelEntry {
                    provider_id: "kimi".into(),
                    id: "kimi-k2".into(),
                    display_name: "Kimi K2".into(),
                    ..ModelEntry::default()
                },
                ModelEntry {
                    provider_id: "kimi".into(),
                    id: "kimi-k2-thinking".into(),
                    display_name: "Kimi K2 Thinking".into(),
                    enabled: false,
                    ..ModelEntry::default()
                },
            ]);
            view.projection.settings_general.proxy_url = Some("http://127.0.0.1:7890".into());
            // ADR-056 D4：Manage models / 代理 Switch 迁入展开区——
            // 断言前先显式展开三张卡（chevron 状态存 projection）。
            view.projection
                .settings_providers
                .expanded_providers
                .extend(["kimi", "glm", "empty"].map(String::from));
            view.open_menu = Some(MenuKind::SettingsProviderModels("kimi".into()));
        });
    });
    let press = |identifier: String| AxRequest {
        identifier,
        action: AxAction::Press,
        value: None,
    };
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);

        tree.validate().expect("models menu AX tree validates");
        // 已连接 + 目录可用：Manage 可按；未连接禁用且不发布 Press。
        let manage = tree
            .find(&settings_manage_models_identifier("kimi"))
            .expect("kimi manage trigger has an AX node");
        assert!(manage.enabled);
        assert!(tree.permits(&press(settings_manage_models_identifier("kimi"))));
        let glm_manage = tree
            .find(&settings_manage_models_identifier("glm"))
            .expect("glm manage trigger has an AX node");
        assert!(!glm_manage.enabled);
        assert!(!tree.permits(&press(settings_manage_models_identifier("glm"))));

        // 弹层：Group + 每模型 Switch（value/selected 与 enabled 同源）
        // + Enable all / Disable all 可按。
        let menu = tree
            .find(&settings_models_menu_identifier("kimi"))
            .expect("models menu group has an AX node");
        assert_eq!(menu.role, AxRole::Group);
        let on = tree
            .find(&settings_model_switch_identifier("kimi", "kimi-k2"))
            .expect("kimi-k2 switch has an AX node");
        assert_eq!(on.value.as_deref(), Some("On"));
        assert!(on.selected);
        assert!(on.enabled);
        assert!(tree.permits(&press(settings_model_switch_identifier("kimi", "kimi-k2"))));
        let off = tree
            .find(&settings_model_switch_identifier(
                "kimi",
                "kimi-k2-thinking",
            ))
            .expect("kimi-k2-thinking switch has an AX node");
        assert_eq!(off.value.as_deref(), Some("Off"));
        assert!(!off.selected);
        assert!(tree.permits(&press(settings_model_switch_identifier(
            "kimi",
            "kimi-k2-thinking"
        ))));
        for identifier in [
            settings_models_enable_all_identifier("kimi"),
            settings_models_disable_all_identifier("kimi"),
        ] {
            let button = tree
                .find(&identifier)
                .unwrap_or_else(|| panic!("{identifier} missing from AX tree"));
            assert!(button.enabled);
            assert!(tree.permits(&press(identifier)));
        }
        let refresh = tree
            .find(&settings_models_refresh_identifier("kimi"))
            .expect("non-empty catalog still publishes Refresh");
        assert!(refresh.enabled);
        assert!(tree.permits(&press(settings_models_refresh_identifier("kimi"))));

        // 代理 Switch（OPT-3c）：全局 proxy_url 已配置时出现，value
        // On / selected=true / 可按。
        let proxy = tree
            .find(&settings_use_proxy_identifier("kimi"))
            .expect("kimi proxy switch has an AX node");
        assert_eq!(proxy.value.as_deref(), Some("On"));
        assert!(proxy.selected);
        assert!(proxy.enabled);
        assert!(tree.permits(&press(settings_use_proxy_identifier("kimi"))));
    });

    // 目录超过菜单视口：真滚动后 AX 只发布当前可见 Switch，重绘不归零。
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let mut catalog = view.projection.settings_providers.model_catalog.clone();
            catalog.extend((0..16).map(|ix| ModelEntry {
                provider_id: "kimi".into(),
                id: format!("scroll-{ix}"),
                display_name: format!("Scroll model {ix}"),
                ..ModelEntry::default()
            }));
            view.projection
                .settings_providers
                .apply_model_catalog(catalog);
            cx.notify();
        });
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    let last_id = settings_model_switch_identifier("kimi", "scroll-15");
    let menu_id = format!("{}-list", settings_models_menu_identifier("kimi"));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let tree = view.accessibility_tree(window, cx);
            assert!(
                tree.find(&last_id).is_none(),
                "offscreen last model has no AX action"
            );
            view.settings_element_layouts[&menu_id].scroll_to_bottom();
            cx.notify();
        });
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    let scrolled_offset = cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        assert!(tree
            .find(&settings_model_switch_identifier("kimi", "kimi-k2"))
            .is_none());
        let last = tree.find(&last_id).expect("last model scrolls into view");
        let viewport = view.settings_element_layouts[&menu_id].bounds();
        let actual = view.settings_element_layouts[&last_id]
            .bounds()
            .intersect(&viewport);
        assert_eq!(
            last.bounds,
            AxRect::new(
                actual.origin.x.into(),
                actual.origin.y.into(),
                actual.size.width.into(),
                actual.size.height.into()
            )
        );
        assert!(tree.permits(&press(last_id.clone())));
        let offset = view.settings_element_layouts[&menu_id].offset();
        assert!(offset.y < px(0.0));
        offset
    });
    cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            view.read(cx).settings_element_layouts[&menu_id].offset(),
            scrolled_offset
        )
    });

    // 搜索与字号变化后保留固定头部，批量操作仍针对完整目录。
    for scale in [
        crate::ui::theme::font::TextScale::Percent100,
        crate::ui::theme::font::TextScale::Percent125,
        crate::ui::theme::font::TextScale::Percent150,
    ] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.text_scale = scale;
                view.focus_model_search(window, cx);
                view.model_search_input
                    .update(cx, |input, cx| input.set_text("SCROLL-15", cx));
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.settings_filtered_models("kimi").len(), 1);
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                assert!(tree.find("model-search-input").unwrap().bounds.height > 0.0);
                assert!(tree.find(&last_id).unwrap().bounds.height > 0.0);
                assert!(
                    tree.find(&settings_models_disable_all_identifier("kimi"))
                        .unwrap()
                        .enabled
                );
                view.model_search_input
                    .update(cx, |input, cx| input.set_text("no-such-model", cx));
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                assert!(tree.find("settings-models-no-results-kimi").is_some());
                assert!(tree.permits(&press("model-search-clear".into())));
                view.clear_model_search(window, cx);
                view.text_scale = crate::ui::theme::font::TextScale::Percent100;
            })
        });
        cx.run_until_parked();
    }

    // 空目录弹层（connected + catalog 可用但目录无条目）：诚实空态 +
    // Refresh 可按；Enable / Disable all 禁用不发布 Press。
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.open_menu = Some(MenuKind::SettingsProviderModels("empty".into()));
        });
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().expect("empty menu AX tree validates");
        let empty = tree
            .find("settings-models-empty-empty")
            .expect("empty catalog note has an AX node");
        assert_eq!(
            empty.value.as_deref(),
            Some("This provider's catalog is empty.")
        );
        let refresh = tree
            .find(&settings_models_refresh_identifier("empty"))
            .expect("refresh button has an AX node");
        assert!(refresh.enabled);
        assert!(tree.permits(&press(settings_models_refresh_identifier("empty"))));
        for identifier in [
            settings_models_enable_all_identifier("empty"),
            settings_models_disable_all_identifier("empty"),
        ] {
            let button = tree
                .find(&identifier)
                .unwrap_or_else(|| panic!("{identifier} missing from AX tree"));
            assert!(!button.enabled);
            assert!(!tree.permits(&press(identifier)));
        }
    });

    // 断线 stale：Manage 与弹层开关全部关闸，Press fail-closed。
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.projection
                .settings_providers
                .mark_stale("socket closed");
        });
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        let manage = tree
            .find(&settings_manage_models_identifier("kimi"))
            .expect("kimi manage trigger still present");
        assert!(!manage.enabled);
        assert!(!tree.permits(&press(settings_manage_models_identifier("kimi"))));
        let proxy = tree
            .find(&settings_use_proxy_identifier("kimi"))
            .expect("kimi proxy switch still present");
        assert!(!proxy.enabled);
        assert!(!tree.permits(&press(settings_use_proxy_identifier("kimi"))));
    });
}

/// ADR-056 D4/D5：展开卡 AX——默认折叠只有 chevron（value Collapsed）；
/// chevron Press 展开后 Credentials 行（kind + masked + Connected /
/// Expired）与 Usage 行（恒「Usage unavailable」，无数字）入树；空
/// 凭证列表给诚实空态；proxy_url 未配置时不发布代理 Switch。
#[gpui::test]
fn settings_provider_expanded_card_ax_pins_credentials_and_usage(cx: &mut gpui::TestAppContext) {
    use gpui::{prelude::*, px, AppContext};

    use crate::projection::ProviderAuthStatusEntry;
    use crate::ui::settings::{
        provider_credential_kind_label, settings_provider_expand_identifier,
        settings_use_proxy_identifier,
    };

    struct AxExpandedHost {
        view: gpui::Entity<AppView>,
    }
    impl gpui::Render for AxExpandedHost {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> impl gpui::IntoElement {
            window.set_rem_size(px(self.view.read(cx).text_scale.rem_pixels()));
            gpui::div()
                .size_full()
                .flex()
                .child(self.view.update(cx, |view, cx| {
                    view.ensure_settings_api_key_inputs(cx);
                    view.settings_page_element(cx)
                }))
        }
    }

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("opt3d-expanded-card-ax.sock");
    let (host, cx) = cx.add_window_view(|_window, cx| {
        let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
        AxExpandedHost { view }
    });
    // Host 直接装配子 view 的元素，须在每次验收前通知 Host 重绘；
    // 单独 refresh window 不会使已缓存的 Host render 失效。
    // 语义断言覆盖完整页面；窄窗与菜单裁剪另在本测试内实际滚动。
    cx.simulate_resize(gpui::size(px(1440.0), px(2400.0)));
    let view = cx.update(|_window, cx| host.read(cx).view.clone());
    // ProviderCredentialStatus 未在 pawork-client re-export 面上，测试
    // 经 wire JSON 解码构造（与投影层 fail-closed 解析同一路径）。
    let provider = |provider_id: &str, credentials: serde_json::Value| {
        serde_json::from_value::<ProviderAuthStatusEntry>(serde_json::json!({
            "provider_id": provider_id,
            "display_name": provider_id,
            "endpoint_label": "",
            "auth_methods": ["api_key"],
            "credentials": credentials,
            "auth": { "type": "connected", "method": "api_key",
                      "masked_credential": null },
            "catalog": { "type": "remote",
                         "fetched_at": "2026-09-05T00:00:00Z" },
            "use_proxy": true,
        }))
        .expect("decode ProviderAuthStatusEntry")
    };
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "test".into(),
            });
            view.route = AppRoute::Settings;
            view.text_scale = crate::ui::theme::font::TextScale::Percent100;
            view.projection.settings_providers.apply_loaded(
                crate::projection::ProviderAuthStatusData {
                    providers: vec![
                        provider(
                            "dual",
                            serde_json::json!([
                                { "kind": "api_key", "masked_credential": "sk-…ab12",
                                  "expired": false, "expires_at": null },
                                { "kind": "oauth", "masked_credential": "oauth-…wxyz",
                                  "expired": true,
                                  "expires_at": "2026-09-01T00:00:00Z" },
                            ]),
                        ),
                        provider("empty", serde_json::json!([])),
                    ],
                    default: None,
                    role_defaults: Default::default(),
                },
            );
        });
    });
    let expand_dual = settings_provider_expand_identifier("dual");
    let expand_empty = settings_provider_expand_identifier("empty");

    // 默认折叠：chevron 常驻（value Collapsed、可按），展开区节点
    // （Credentials / Usage / 代理 Switch）不入树。
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);

        tree.validate().expect("collapsed cards AX tree validates");
        let chevron = tree
            .find(&expand_dual)
            .expect("collapsed card pins its chevron");
        assert_eq!(chevron.value.as_deref(), Some("Collapsed"));
        assert!(chevron.enabled);
        assert!(tree.permits(&AxRequest {
            identifier: expand_dual.clone(),
            action: AxAction::Press,
            value: None,
        }));
        assert!(tree
            .find(&dynamic_identifier("settings-provider-credentials", "dual"))
            .is_none());
        assert!(tree
            .find(&dynamic_identifier("settings-provider-usage", "dual"))
            .is_none());
        assert!(tree.find(&settings_use_proxy_identifier("dual")).is_none());
    });

    // chevron Press（AX 与 render / 键盘同入口）：dual 展开。
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(
                AxRequest {
                    identifier: expand_dual.clone(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
        });
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().expect("expanded card AX tree validates");
        let chevron = tree
            .find(&expand_dual)
            .expect("expanded card keeps its chevron");
        assert_eq!(chevron.value.as_deref(), Some("Expanded"));
        let credentials = tree
            .find(&dynamic_identifier("settings-provider-credentials", "dual"))
            .expect("expanded card pins the credentials group");
        assert_eq!(credentials.label, "Credentials");
        // 区头部标题 / 副标题入 AX label/value（同 manage 行：标题
        // 文案面非省略的回归钉板；render 截断 gpui 测试体系不可观测）。
        let credentials_header = tree
            .find(&dynamic_identifier(
                "settings-provider-credentials-header",
                "dual",
            ))
            .expect("expanded card pins the credentials header text");
        assert_eq!(
            credentials_header.label,
            t("settings.providers.credentials_title")
        );
        assert_eq!(
            credentials_header.value.as_deref(),
            Some(t("settings.providers.credentials_subtitle"))
        );
        // 凭证行：kind + masked + 状态词；api_key 在前（Host 固定序）。
        let api_key = tree
            .find(&dynamic_identifier(
                "settings-provider-credential-0",
                "dual",
            ))
            .expect("api_key credential row pinned");
        assert_eq!(api_key.label, provider_credential_kind_label("api_key"));
        assert_eq!(
            api_key.value.as_deref(),
            Some("API key · sk-…ab12 · Connected")
        );
        let oauth = tree
            .find(&dynamic_identifier(
                "settings-provider-credential-1",
                "dual",
            ))
            .expect("oauth credential row pinned");
        assert_eq!(oauth.label, provider_credential_kind_label("oauth"));
        assert_eq!(
            oauth.value.as_deref(),
            Some("OAuth · oauth-…wxyz · Expired")
        );
        // Usage 行：固定槽位 + 诚实空态，value 不含任何数字 / 百分比。
        let usage = tree
            .find(&dynamic_identifier("settings-provider-usage", "dual"))
            .expect("expanded card pins the usage row");
        assert_eq!(usage.label, "Usage");
        assert_eq!(usage.value.as_deref(), Some("Usage unavailable"));
        // 展开区行标题 / 副标题入 AX value（标题文案面非省略的回归
        // 钉板；render 文本截断 gpui 测试体系不可观测，见 providers.rs
        // min_w_0 注释）。
        let manage_text = tree
            .find(&dynamic_identifier("settings-provider-manage-text", "dual"))
            .expect("expanded card pins the manage models row text");
        assert_eq!(manage_text.label, t("settings.providers.manage_models"));
        assert_eq!(
            manage_text.value.as_deref(),
            Some(t("settings.providers.catalog_scope"))
        );
        // proxy_url 未配置：代理 Switch 不发布（gate 与 render 同源）。
        assert!(tree
            .find(&dynamic_identifier("settings-provider-proxy-text", "dual"))
            .is_none());
        assert!(tree.find(&settings_use_proxy_identifier("dual")).is_none());
        // 未展开的 empty 卡仍只有 chevron。
        assert!(tree
            .find(&dynamic_identifier(
                "settings-provider-credentials-empty",
                "empty"
            ))
            .is_none());
    });

    // empty 卡展开：空凭证列表发布诚实空态，不渲染假行。
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(
                AxRequest {
                    identifier: expand_empty.clone(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
        });
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate()
            .expect("empty expanded card AX tree validates");
        let empty = tree
            .find(&dynamic_identifier(
                "settings-provider-credentials-empty",
                "empty",
            ))
            .expect("empty credentials state pinned");
        assert_eq!(empty.value.as_deref(), Some("No stored credentials"));
        assert!(tree
            .find(&dynamic_identifier(
                "settings-provider-credential-0",
                "empty"
            ))
            .is_none());
    });

    // 配置全局 proxy_url 后：Proxy 行文本（标题 + 副标题）与 Switch
    // 随 gate 打开发布（与 render 同源）。
    cx.update(|_window, cx| {
        view.update(cx, |view, _cx| {
            view.projection
                .settings_general
                .apply_loaded(pawork_client::GeneralSettingsData {
                    proxy_url: Some("http://127.0.0.1:7890".into()),
                });
        });
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        let proxy_text = tree
            .find(&dynamic_identifier("settings-provider-proxy-text", "dual"))
            .expect("proxy row text pinned once proxy_url is set");
        assert_eq!(proxy_text.label, t("settings.providers.proxy_title"));
        assert_eq!(
            proxy_text.value.as_deref(),
            Some(t("settings.providers.proxy_subtitle"))
        );
        let proxy_switch = tree
            .find(&settings_use_proxy_identifier("dual"))
            .expect("proxy switch pinned once proxy_url is set");
        assert!(proxy_switch.enabled);
    });
    // GUI 1.15 reuses this layout path with stable per-account IDs.
    cx.update(|_, cx| {
        view.update(cx, |view, _| {
            view.handshake_info = Some(crate::controller::DesktopHandshakeInfo {
                runtime_id: "test".into(),
                api_version: "1.15".into(),
                capabilities: vec![],
                host_data_dir: None,
            });
            let credentials = &mut view.projection.settings_providers.providers[0].credentials;
            credentials[0].credential_id = "cred_first".into();
            credentials[0].display_name = "Personal".into();
            credentials[0].selected = true;
            credentials[1].credential_id = "cred_second".into();
            credentials[1].display_name = "Work".into();
        })
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    let remove_second =
        crate::ui::settings::settings_account_action_identifier("dual", "cred_second", true);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let tree = view.accessibility_tree(window, cx);
            let use_first = crate::ui::settings::settings_account_action_identifier(
                "dual",
                "cred_first",
                false,
            );
            let remove_first =
                crate::ui::settings::settings_account_action_identifier("dual", "cred_first", true);
            let use_second = crate::ui::settings::settings_account_action_identifier(
                "dual",
                "cred_second",
                false,
            );
            assert!(!tree.find(&use_first).unwrap().enabled);
            assert!(!tree.find(&remove_first).unwrap().enabled);
            assert!(tree.find(&use_second).unwrap().enabled);
            assert!(tree.find("settings-account-name-dual").is_none());
            view.handle_accessibility_request(
                AxRequest {
                    identifier: remove_second.clone(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
            assert_eq!(
                view.settings_account_remove_confirm.as_deref(),
                Some(remove_second.as_str())
            );
            // Removing/reordering the earlier row must preserve the target ID.
            view.projection.settings_providers.providers[0]
                .credentials
                .remove(0);
        })
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let tree = view.accessibility_tree(window, cx);
            assert_eq!(
                tree.find(&remove_second).unwrap().label,
                t("settings.providers.action_confirm_remove")
            );
            let keep = tree.find(&format!("{remove_second}-keep")).unwrap();
            assert!(keep.bounds.width > 0.0 && keep.bounds.width < 150.0);
            assert!(tree
                .find(&crate::ui::settings::settings_credential_row_identifier(
                    "dual",
                    &view.projection.settings_providers.providers[0].credentials[0],
                    0
                ))
                .is_some());
            view.handle_accessibility_request(
                AxRequest {
                    identifier: format!("{remove_second}-keep"),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
            assert!(view.settings_account_remove_confirm.is_none());
        })
    });
    // 窄窗三档字号：真实页面滚动后，离屏标题消失，底部控件框跟随 prepaint。
    for scale in [
        crate::ui::theme::font::TextScale::Percent100,
        crate::ui::theme::font::TextScale::Percent125,
        crate::ui::theme::font::TextScale::Percent150,
    ] {
        cx.simulate_resize(gpui::size(px(1080.0), px(720.0)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.text_scale = scale;
                view.projection
                    .settings_providers
                    .expanded_providers
                    .remove("empty");
                cx.notify();
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let target = &view.settings_element_layouts[&expand_empty];
                let dy = target.bounds().origin.y - view.settings_scroll.bounds().origin.y;
                let offset = view.settings_scroll.offset();
                view.settings_scroll
                    .set_offset(gpui::point(px(0.0), offset.y - dy + px(300.0)));
                cx.notify();
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            assert!(tree.find("settings-page-title").is_none());
            for id in [
                dynamic_identifier("settings-provider-usage", "dual"),
                expand_empty.clone(),
            ] {
                let node = tree.find(&id).expect("bottom provider content is visible");
                let actual = view.settings_element_layouts[&id]
                    .bounds()
                    .intersect(&view.settings_scroll.bounds());
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
            }
            assert!(tree.permits(&AxRequest {
                identifier: expand_empty.clone(),
                action: AxAction::Press,
                value: None
            }));
        });
    }

    // G2：同一真实布局路径显示账号三窗，epoch 与离页清理拒绝迟到结果。
    cx.simulate_resize(gpui::size(px(1440.0), px(2400.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.text_scale = crate::ui::theme::font::TextScale::Percent100;
            view.settings_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            view.handshake_info.as_mut().unwrap().api_version = "1.17".into();
            let p = &mut view.projection.settings_providers.providers[0];
            p.provider_id = "opencode-go".into();
            p.credentials[0].kind = "api_key".into();
            p.credentials[0].selected = true;
            view.projection.settings_providers.expanded_providers.insert("opencode-go".into());
            let scope = serde_json::json!({"tenant_id":"local", "account_id":"local/default", "provider_id":"opencode-go"});
            let now = crate::ui::now_unix_ms();
            let windows: Vec<_> = ["rolling5h", "weekly", "monthly"].into_iter().map(|window| serde_json::json!({
                "window":window, "read":{"status":"ok", "snapshot":{
                    "scope":scope, "window":window, "unit":{"kind":"percent"},
                    "values":{"used":{"kind":"exact","value":42},"limit":{"kind":"exact","value":100},"remaining":{"kind":"exact","value":58}},
                    "reset":{"kind":"absolute","at":now+3_600_000,"uncertain":false},
                    "confidence":"exact", "provenance":{"adapter_kind":"api_key_api","source":"Go usage","fetched_at":now-31_000}
                }}
            })).collect();
            let quota: pawork_client::QuotaOverviewView = serde_json::from_value(serde_json::json!({
                "scope":scope, "windows":windows, "generated_at":now
            })).unwrap();
            let state = &mut view.projection.settings_providers;
            let old = state.begin_quota("opencode-go", "cred_second");
            let latest = state.begin_quota("opencode-go", "cred_second");
            state.apply_quota("opencode-go".into(), "cred_second".into(), old, Some(quota.clone()));
            assert!(state.account_quotas[&("opencode-go".into(), "cred_second".into())].loading);
            state.apply_quota("opencode-go".into(), "cred_second".into(), latest, Some(quota));
            cx.notify();
        });
    });
    cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let tree = view.accessibility_tree(window, cx);
            for suffix in ["rolling", "weekly", "monthly"] {
                let id =
                    crate::ui::settings::quota_identifier("opencode-go", "cred_second", suffix);
                let node = tree.find(&id).expect("quota window in actual layout");
                assert!(node.value.as_ref().unwrap().contains("Used 42%"));
                assert!(node.value.as_ref().unwrap().contains("Remaining 58%"));
                assert!(node.value.as_ref().unwrap().contains("Resets in 1h"));
                assert!(node.value.as_ref().unwrap().contains("Source: Go usage"));
                assert!(node.value.as_ref().unwrap().contains("Stale"));
                assert!(node.bounds.width > 0.0 && node.bounds.height > 0.0);
            }
            let mode = crate::ui::settings::quota_identifier("opencode-go", "", "mode");
            assert!(tree.find(&mode).unwrap().enabled);
            assert_eq!(tree.find(&mode).unwrap().value.as_deref(), Some("Off"));
            let refresh =
                crate::ui::settings::quota_identifier("opencode-go", "cred_second", "refresh");
            let rename = crate::ui::settings::settings_account_rename_identifier(
                "opencode-go",
                "cred_second",
            );
            assert!(tree.find(&rename).unwrap().enabled);
            for (id, max_width) in [(&refresh, 220.0), (&mode, 50.0)] {
                let node = tree.find(id).unwrap();
                let actual = view.settings_element_layouts[id].bounds();
                assert!(node.bounds.width > 0.0 && node.bounds.width < max_width);
                assert_eq!(node.bounds.width, f32::from(actual.size.width));
            }
            let key = ("opencode-go".to_string(), "cred_second".to_string());
            // A refresh preserves the previous reading and reports its failure immediately.
            let epoch = view
                .projection
                .settings_providers
                .begin_quota(&key.0, &key.1);
            assert!(view.projection.settings_providers.account_quotas[&key]
                .view
                .is_some());
            assert!(view.account_quota_labels(&key.0, &key.1)[0]
                .1
                .contains("Loading"));
            view.projection.settings_providers.apply_quota(
                key.0.clone(),
                key.1.clone(),
                epoch,
                None,
            );
            assert!(view.projection.settings_providers.account_quotas[&key].stale);
            assert!(view.account_quota_labels(&key.0, &key.1)[0]
                .1
                .contains("Used 42%"));
            let previous = view.projection.settings_providers.account_quotas[&key]
                .view
                .clone()
                .unwrap();
            let mut failed = previous.clone();
            for window in &mut failed.windows {
                window.read = pawork_client::WindowReadView::Failed { failures: vec![] };
            }
            let epoch = view
                .projection
                .settings_providers
                .begin_quota(&key.0, &key.1);
            view.projection.settings_providers.apply_quota(
                key.0.clone(),
                key.1.clone(),
                epoch,
                Some(failed.clone()),
            );
            assert!(view.projection.settings_providers.account_quotas[&key].stale);
            assert_eq!(
                view.projection.settings_providers.account_quotas[&key]
                    .view
                    .as_ref(),
                Some(&previous)
            );
            let first = view
                .projection
                .settings_providers
                .begin_quota(&key.0, "first-failure");
            view.projection.settings_providers.apply_quota(
                key.0.clone(),
                "first-failure".into(),
                first,
                Some(failed.clone()),
            );
            assert!(view.account_quota_labels(&key.0, "first-failure")[0]
                .1
                .contains("unavailable"));
            // 一窗成功时采用本次 typed 结果，失败窗不冒用旧读数。
            failed.windows[0] = previous.windows[0].clone();
            let partial = view
                .projection
                .settings_providers
                .begin_quota(&key.0, &key.1);
            view.projection.settings_providers.apply_quota(
                key.0.clone(),
                key.1.clone(),
                partial,
                Some(failed.clone()),
            );
            assert_eq!(
                view.projection.settings_providers.account_quotas[&key]
                    .view
                    .as_ref(),
                Some(&failed)
            );
            assert!(view.account_quota_labels(&key.0, &key.1)[1]
                .1
                .contains("unavailable"));
            let entry = view
                .projection
                .settings_providers
                .account_quotas
                .get_mut(&key)
                .unwrap();
            entry.stale = false;
            if let pawork_client::WindowReadView::Ok { snapshot, .. } =
                &mut entry.view.as_mut().unwrap().windows[0].read
            {
                snapshot.provenance.fetched_at =
                    serde_json::from_value(serde_json::json!(crate::ui::now_unix_ms() + 60_000))
                        .unwrap();
            }
            assert!(view.account_quota_labels(&key.0, &key.1)[0]
                .1
                .contains("Stale"));
            if let pawork_client::WindowReadView::Ok { snapshot, .. } = &mut view
                .projection
                .settings_providers
                .account_quotas
                .get_mut(&key)
                .unwrap()
                .view
                .as_mut()
                .unwrap()
                .windows[0]
                .read
            {
                snapshot.values.remaining = pawork_client::QuotaMeasure::Unknown;
            }
            assert!(view.account_quota_labels(&key.0, &key.1)[0]
                .1
                .contains("Remaining unknown"));
            let p = &mut view.projection.settings_providers.providers[0];
            p.selection_mode = pawork_client::ProviderAccountSelectionMode::WhenExhausted;
            p.credentials.clear();
            assert!(view.account_mode_enabled(&view.projection.settings_providers.providers[0]));
            view.projection
                .settings_providers
                .account_mode_pending
                .insert("opencode-go".into(), epoch);
            assert!(!view.account_mode_enabled(&view.projection.settings_providers.providers[0]));
            view.clear_settings_buffers(cx);
            view.projection.settings_providers.apply_quota(
                "opencode-go".into(),
                "cred_second".into(),
                1,
                None,
            );
            assert!(view.projection.settings_providers.account_quotas.is_empty());
            view.handshake_info.as_mut().unwrap().api_version = "1.15".into();
            assert!(!view.settings_quota_supported());
        });
    });
}

/// 失败卡测高。原因再长也只占一行；AX description 保留原文。
fn measure_failed_banner(
    cx: &mut gpui::VisualTestContext,
    view: &gpui::Entity<AppView>,
    scale: crate::ui::theme::font::TextScale,
    event_id: &str,
    reason: &str,
    expect_cta: bool,
) -> f32 {
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.text_scale = scale;
            window.set_rem_size(gpui::px(scale.rem_pixels()));
            view.projection.timeline.entries = vec![TimelineEntry {
                sequence: 1,
                event_id: event_id.into(),
                kind: TimelineEntryKind::RunState(format!("run failed · {reason}")),
                fork_boundary: Some(ForkBoundary::Failed),
                timestamp: "1800000000000".into(),
                run_id: Some("run-measure".into()),
            }];
            view.timeline_changed();
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let rem = f32::from(window.rem_size());
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        let card = tree
            .find(&format!("run-summary-card-{event_id}"))
            .expect("failed summary card");
        assert_eq!(card.description.as_deref(), Some(reason));
        let layout = timeline::run_summary_card_layout(
            &view.projection.timeline[0],
            card.bounds.width,
            rem,
            false,
        );
        assert_eq!(layout.reason_height, 0.0);
        assert_eq!(card.bounds.height, layout.height);
        if reason.chars().count() > 80 {
            let font_px = crate::ui::theme::font::DESCRIPTION.0 * rem;
            assert!(
                crate::ui::timeline_entry::estimated_wrapped_lines(
                    reason,
                    card.bounds.width,
                    font_px,
                ) > 2,
                "long reason must exceed one visual line"
            );
        }
        let cta = tree.find(&run_open_providers_identifier(event_id));
        if expect_cta {
            let cta = cta.expect("open providers CTA");
            assert!(cta.actions.contains(&AxAction::Press));
            assert!((cta.bounds.height - SUMMARY_NEXT_STEP_BUTTON_HEIGHT).abs() < 0.5);
            assert!(cta.bounds.y + 0.5 >= card.bounds.y);
            assert!(cta.bounds.y + cta.bounds.height <= card.bounds.y + card.bounds.height + 0.5);
        } else {
            assert!(cta.is_none());
        }
        card.bounds.height
    })
}

fn assert_completed_hides_review(cx: &mut gpui::VisualTestContext, view: &gpui::Entity<AppView>) {
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        assert!(tree.find(&run_review_identifier("completed")).is_none());
        assert!(tree.find("run-summary-card-completed").is_none());
        let footer = tree.find("run-footer-completed").expect("completed footer");
        assert!(footer.label.contains(t("run.footer_completed")));
        assert!(!view.changes_available_for_active());
    });
}

/// GUI3-04：认证失败 AX Press 进入 Providers 并保留草稿。长原因在三档字号下
/// 仍是单行，AX description 保留全文。完成态只有当前会话 Ready、非 stale、
/// 非空文件才有 Review；点击渲染按钮进入 Changes。并排开合在测试里插到终宽。
/// 终态与文件清单是测试投影，不是 Host / Git 回执。
#[gpui::test]
fn failed_run_banner_geometry_and_provider_cta_preserves_draft(cx: &mut gpui::TestAppContext) {
    use crate::controller::DiffFileSummary;
    use crate::ui::theme::font::TextScale;
    use gpui::{px, size};

    fn terminal(
        event_id: &str,
        boundary: ForkBoundary,
        label: &str,
        run_id: &str,
    ) -> TimelineEntry {
        TimelineEntry {
            sequence: 1,
            event_id: event_id.into(),
            kind: TimelineEntryKind::RunState(label.into()),
            fork_boundary: Some(boundary),
            timestamp: "1800000000000".into(),
            run_id: Some(run_id.into()),
        }
    }

    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            std::sync::Arc::new(crate::platform::Platform::new()),
            std::env::temp_dir().join("gui3-04-failed-banner.sock"),
            None,
            cx,
        )
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.connection = ConnectionState::Connected {
                instance_id: "test".into(),
            };
            view.projection.active_session_id = Some("task".into());
            view.projection.timeline.entries = vec![terminal(
                "failed-auth",
                ForkBoundary::Failed,
                "run failed · HTTP 401",
                "run-auth",
            )];
            view.text_input
                .update(cx, |input, cx| input.set_text("keep draft", cx));
            view.timeline_changed();
            cx.notify();
        })
    });
    cx.simulate_resize(size(px(1440.0), px(1024.0)));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.text_scale = TextScale::Percent100;
            window.set_rem_size(px(TextScale::Percent100.rem_pixels()));
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();

    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let rem = f32::from(window.rem_size());
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            let card = tree
                .find("run-summary-card-failed-auth")
                .expect("failed summary card");
            let expected = timeline::run_summary_card_height(
                &view.projection.timeline[0],
                card.bounds.width,
                rem,
                false,
            );
            assert_eq!(card.bounds.height, expected);
            assert_eq!(card.label, t("run.footer_failed"));
            assert_eq!(card.description.as_deref(), Some("HTTP 401"));
            let cta_id = run_open_providers_identifier("failed-auth");
            let cta = tree.find(&cta_id).expect("open providers CTA");
            assert_eq!(cta.label, t("run.open_provider_settings"));
            assert!(cta.actions.contains(&AxAction::Press));
            let request = AxRequest {
                identifier: cta_id,
                action: AxAction::Press,
                value: None,
            };
            assert!(tree.permits(&request));
            view.handle_accessibility_request(request, window, cx);
            assert_eq!(view.route, AppRoute::Settings);
            assert_eq!(view.settings_page, SettingsPage::Providers);
            assert_eq!(view.text_input.read(cx).text(), "keep draft");
            assert_eq!(view.projection.active_session_id.as_deref(), Some("task"));
            view.on_close_settings(window, cx);
            assert_eq!(view.route, AppRoute::Workspace);
            assert_eq!(view.text_input.read(cx).text(), "keep draft");
        })
    });

    let long_auth = format!("HTTP 401 {}", "rejected credential ".repeat(24));
    let long_plain = format!("provider timeout {}", "upstream delayed ".repeat(24));
    let mut auth_heights = [0.0; 3];
    let mut plain_heights = [0.0; 3];
    for (index, scale) in [
        TextScale::Percent100,
        TextScale::Percent125,
        TextScale::Percent150,
    ]
    .into_iter()
    .enumerate()
    {
        cx.simulate_resize(size(px(1440.0), px(1024.0)));
        let long_height =
            measure_failed_banner(cx, &view, scale, "failed-long-auth", &long_auth, true);
        let short_height =
            measure_failed_banner(cx, &view, scale, "failed-long-auth", "HTTP 401", true);
        assert_eq!(
            long_height,
            short_height,
            "truncated reason keeps one line at {}%",
            scale.percent()
        );
        auth_heights[index] = long_height;
        plain_heights[index] =
            measure_failed_banner(cx, &view, scale, "failed-long-plain", &long_plain, false);
        assert!(plain_heights[index] < auth_heights[index]);
    }
    assert!(auth_heights[0] < auth_heights[1] && auth_heights[1] < auth_heights[2]);
    assert!(plain_heights[0] < plain_heights[1] && plain_heights[1] < plain_heights[2]);

    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.text_scale = TextScale::Percent150;
            window.set_rem_size(px(TextScale::Percent150.rem_pixels()));
            view.projection.timeline.entries = vec![terminal(
                "failed-long-auth",
                ForkBoundary::Failed,
                &format!("run failed · {long_auth}"),
                "run-auth",
            )];
            view.timeline_changed();
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let cta_id = run_open_providers_identifier("failed-long-auth");
            let tree = view.accessibility_tree(window, cx);
            assert_eq!(
                tree.find("run-summary-card-failed-long-auth")
                    .and_then(|card| card.description.as_deref()),
                Some(long_auth.as_str())
            );
            view.handle_accessibility_request(
                AxRequest {
                    identifier: cta_id,
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
            assert_eq!(view.route, AppRoute::Settings);
            assert_eq!(view.settings_page, SettingsPage::Providers);
            assert_eq!(view.text_input.read(cx).text(), "keep draft");
            view.on_close_settings(window, cx);
            view.text_scale = TextScale::Percent100;
            window.set_rem_size(px(TextScale::Percent100.rem_pixels()));
            cx.notify();
        })
    });

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.timeline.entries = vec![terminal(
                "failed-timeout",
                ForkBoundary::Failed,
                "run failed · provider timeout",
                "run-timeout",
            )];
            view.timeline_changed();
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        assert!(tree.find("run-summary-card-failed-timeout").is_some());
        assert!(tree
            .find(&run_open_providers_identifier("failed-timeout"))
            .is_none());
        assert!(tree.find("run-footer-failed-timeout").is_some());
    });

    let review_file = || DiffFileSummary {
        path: "a.rs".into(),
        status: "modified".into(),
        additions: 1,
        deletions: 0,
        binary: false,
    };
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.timeline.entries = vec![terminal(
                "completed",
                ForkBoundary::Completed,
                "run completed",
                "run-ok",
            )];
            view.changes.reset_for_session();
            view.timeline_changed();
            cx.notify();
        })
    });
    assert_completed_hides_review(cx, &view);

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let epoch = view.changes.begin_refresh();
            assert!(view
                .changes
                .apply_files(epoch, Some("task".into()), Vec::new(), None));
            cx.notify();
        })
    });
    assert_completed_hides_review(cx, &view);

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let epoch = view.changes.begin_refresh();
            assert!(view.changes.apply_files(
                epoch,
                Some("other-session".into()),
                vec![review_file()],
                None,
            ));
            cx.notify();
        })
    });
    assert_completed_hides_review(cx, &view);

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let epoch = view.changes.begin_refresh();
            assert!(view
                .changes
                .apply_files(epoch, None, vec![review_file()], None));
            cx.notify();
        })
    });
    assert_completed_hides_review(cx, &view);

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let epoch = view.changes.begin_refresh();
            assert!(view.changes.apply_files(
                epoch,
                Some("task".into()),
                vec![review_file()],
                None,
            ));
            view.changes.mark_stale("disconnected");
            cx.notify();
        })
    });
    assert_completed_hides_review(cx, &view);

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.timeline.entries = vec![terminal(
                "completed",
                ForkBoundary::Completed,
                "run completed",
                "run-ok",
            )];
            let epoch = view.changes.begin_refresh();
            view.changes.apply_files(
                epoch,
                Some("task".into()),
                vec![DiffFileSummary {
                    path: "a.rs".into(),
                    status: "modified".into(),
                    additions: 1,
                    deletions: 0,
                    binary: false,
                }],
                None,
            );
            view.timeline_changed();
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let rem = f32::from(window.rem_size());
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        let card = tree
            .find("run-summary-card-completed")
            .expect("completed summary card");
        let expected = timeline::run_summary_card_height(
            &view.projection.timeline[0],
            card.bounds.width,
            rem,
            true,
        );
        assert_eq!(card.bounds.height, expected);
        let review = tree
            .find(&run_review_identifier("completed"))
            .expect("review changes CTA");
        assert_eq!(review.label, t("timeline.review_changes"));
        assert!(review.actions.contains(&AxAction::Press));
        assert!(tree
            .find(&run_open_providers_identifier("completed"))
            .is_none());
        assert!(tree.find("run-footer-completed").is_some());
        assert!(!view.inspector_open);
    });

    let review_hit = cx
        .debug_bounds("run-review-completed")
        .expect("rendered review button");
    cx.simulate_click(review_hit.center(), gpui::Modifiers::none());
    // 并排开合是 180ms，测试执行器不推动 Instant。插到终宽后 Changes
    // 才有实测布局，AX 才能发布面板。
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.inspector_motion.snap(metrics::INSPECTOR_WIDTH);
            cx.notify();
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view.inspector_open);
        assert_eq!(view.inspector_tab, InspectorTab::Changes);
        assert!(matches!(
            view.changes.fetch,
            crate::ui::changes::ChangesFetch::Failed(ref reason) if reason == t("changes.no_workspace")
        ));
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        let changes = tree.find("changes").expect("changes panel");
        let description = changes.description.as_deref().unwrap_or("");
        assert!(
            description.contains(t("changes.no_workspace")),
            "{description}"
        );
        assert_eq!(
            tree.find("inspector-panel").unwrap().value.as_deref(),
            Some(t("inspector.tab_changes"))
        );
        assert_eq!(view.text_input.read(cx).text(), "keep draft");
    });
    assert!(cx.debug_bounds("changes-tab-files").is_some());

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.timeline.entries = vec![terminal(
                "cancelled",
                ForkBoundary::Cancelled,
                "run cancelled",
                "run-cancel",
            )];
            view.timeline_changed();
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        assert!(tree.find("run-summary-card-cancelled").is_none());
        assert!(tree
            .find(&run_open_providers_identifier("cancelled"))
            .is_none());
        assert!(tree.find(&run_review_identifier("cancelled")).is_none());
        let footer = tree.find("run-footer-cancelled").expect("cancelled footer");
        assert!(footer.label.contains(t("run.footer_cancelled")));
    });
}

fn user_message(seq: u64, text: &str) -> TimelineEntry {
    TimelineEntry {
        sequence: seq,
        event_id: format!("e{seq}"),
        kind: TimelineEntryKind::UserMessage { text: text.into() },
        fork_boundary: None,
        timestamp: seq.to_string(),
        run_id: Some("r-1".into()),
    }
}

/// GUI2-05：1440×1024 的三档字号并排打开；1080×720 三档与 1280×720 的
/// 150% 走中央呈现。返回后草稿、Timeline 偏移和连接文案保持，terminal
/// create / close 的本地 pending 仍为空。active_run_id 与 pending_approval
/// 只断言本地投影保持，未校验取消 / 审批命令派发。
#[gpui::test]
fn work_panel_center_mode_reaches_all_tabs_and_restores_conversation(
    cx: &mut gpui::TestAppContext,
) {
    use crate::ui::theme::font::TextScale;
    use gpui::{px, size, ListOffset};
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("gui2-05-center.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "gui2-05".into(),
            });
            view.projection.workspaces = vec![crate::projection::WorkspaceSummary {
                id: "ws-a".into(),
                name: "Alpha".into(),
            }];
            view.projection.sessions = vec![rail_session_summary(
                "s-center",
                "Center task",
                Some("ws-a"),
                crate::ui::now_unix_ms(),
            )];
            view.projection.active_session_id = Some("s-center".into());
            view.projection.workspace_id = Some("ws-a".into());
            view.projection.active_run_id = Some("r-1".into());
            view.projection.timeline.entries = vec![
                user_message(1, "first"),
                user_message(2, "second"),
                user_message(3, "third"),
            ];
            view.projection.terminal.session_id = Some("term-keep".into());
            view.projection.terminal.workspace_id = Some("ws-a".into());
            view.text_input
                .update(cx, |input, cx| input.set_text("keep center draft", cx));
            view.timeline_changed();
            cx.notify();
        })
    });

    for (width, height, scale, center) in [
        (1440.0, 1024.0, TextScale::Percent100, false),
        (1440.0, 1024.0, TextScale::Percent125, false),
        (1440.0, 1024.0, TextScale::Percent150, false),
        (1080.0, 720.0, TextScale::Percent100, true),
        (1080.0, 720.0, TextScale::Percent125, true),
        (1080.0, 720.0, TextScale::Percent150, true),
        (1280.0, 720.0, TextScale::Percent150, true),
    ] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.inspector_open = false;
                view.inspector_render_width = 0.0;
                view.inspector_motion.snap(0.0);
                view.inspector_tab = InspectorTab::Changes;
                view.text_scale = scale;
                window.set_rem_size(px(scale.rem_pixels()));
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, _cx| {
                view.timeline_following = false;
                view.timeline_list.scroll_to(ListOffset {
                    item_ix: 1,
                    offset_in_item: px(0.0),
                });
            })
        });
        let scroll_before =
            cx.update(|_, cx| view.read(cx).timeline_list.logical_scroll_top().item_ix);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                assert!(tree.find("timeline").is_some());
                assert!(tree.find("composer").is_some());
                assert_eq!(
                    tree.find("connection-status").unwrap().value.as_deref(),
                    Some(t("rail.connection_local_connected"))
                );
                let request = AxRequest {
                    identifier: "inspector-expand".into(),
                    action: AxAction::Press,
                    value: None,
                };
                assert!(
                    tree.permits(&request),
                    "expand at {width} {scale:?} {center}"
                );
                view.handle_accessibility_request(request, window, cx);
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        if !center {
            // 并排开合是 180ms InspectorMotion，测试执行器不推动 Instant。
            // 插值由 inspector_motion_reverses_without_jump_and_snaps_when_narrow
            // 覆盖；这里落到与该动画相同的终宽后再看布局。
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.inspector_motion.snap(metrics::INSPECTOR_WIDTH);
                    cx.notify();
                });
            });
            cx.refresh().unwrap();
            cx.run_until_parked();
        }
        let inspector = cx.debug_bounds("shell-inspector").unwrap_or_else(|| {
            panic!("inspector missing at {width}x{height} {scale:?} center={center}")
        });
        let inspector_width: f32 = inspector.size.width.into();
        if center {
            assert!(
                inspector_width > 500.0,
                "center inspector {inspector_width} at {width}x{height} {scale:?}"
            );
        } else {
            assert!(
                (inspector_width - metrics::INSPECTOR_WIDTH).abs() <= 1.0,
                "side inspector {inspector_width} at {width}x{height} {scale:?}"
            );
        }
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert!(view.inspector_open);
                assert_eq!(
                    shell_layout::resolve(
                        window.viewport_size().width,
                        true,
                        scale == TextScale::Percent150,
                    )
                    .placement
                    .is_center(),
                    center,
                    "{width}x{height} {scale:?}"
                );
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                assert_eq!(
                    tree.find("timeline").is_none(),
                    center,
                    "timeline visibility at {width} {scale:?}"
                );
                assert_eq!(
                    tree.find("composer").is_none(),
                    center,
                    "composer visibility at {width} {scale:?}"
                );
                let panel = tree.find("inspector-panel").expect("inspector-panel");
                assert_eq!(panel.value.as_deref(), Some(t("inspector.tab_changes")));
                assert_eq!(tree.find("inspector-back").is_some(), center);
                assert!(tree.find("inspector-collapse").is_some());
                assert!(tree.find("inspector-tabs").is_none());
                assert!(tree.find("inspector-expand").is_none());
                assert!(tree.find("changes").is_some());
                assert_eq!(
                    tree.find("connection-status").unwrap().value.as_deref(),
                    Some(t("rail.connection_local_connected"))
                );
                view.select_inspector_tab(InspectorTab::Terminal, cx);
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                assert!(tree.find("terminal").is_some());
                assert_eq!(tree.find("timeline").is_none(), center);
                assert_eq!(
                    view.projection.terminal.session_id.as_deref(),
                    Some("term-keep")
                );
                assert!(view.terminal_pending_create_workspace.is_none());
                assert!(view.terminal_pending_close.is_none());
                view.select_inspector_tab(InspectorTab::Resources, cx);
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                assert!(tree.find("resources").is_some());
                let request = AxRequest {
                    identifier: if center {
                        "inspector-back"
                    } else {
                        "inspector-collapse"
                    }
                    .into(),
                    action: AxAction::Press,
                    value: None,
                };
                assert!(tree.permits(&request));
                view.handle_accessibility_request(request, window, cx);
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        if !center {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.inspector_motion.snap(0.0);
                    cx.notify();
                });
            });
            cx.refresh().unwrap();
            cx.run_until_parked();
        }
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(!view.inspector_open);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            assert!(tree.find("timeline").is_some());
            assert!(tree.find("composer").is_some());
            assert!(tree.find("inspector-back").is_none());
            assert!(tree.find("inspector-panel").is_none());
            assert_eq!(view.text_input.read(cx).text(), "keep center draft");
            assert_eq!(
                view.timeline_list.logical_scroll_top().item_ix,
                scroll_before
            );
            if center {
                assert!(view.composer_focus_handle(cx).is_focused(window));
            }
            assert_eq!(
                tree.find("connection-status").unwrap().value.as_deref(),
                Some(t("rail.connection_local_connected"))
            );
            assert!(matches!(
                view.projection.connection,
                ConnectionState::Connected { .. }
            ));
            assert_eq!(
                view.projection.terminal.session_id.as_deref(),
                Some("term-keep")
            );
            assert!(view.terminal_pending_create_workspace.is_none());
            assert!(view.terminal_pending_close.is_none());
            assert_eq!(view.projection.active_run_id.as_deref(), Some("r-1"));
            assert!(view.projection.pending_approval.is_none());
        });
    }
}

/// UI-05: refresh uses the rendered hitbox and one guarded action across all input channels.
#[gpui::test]
fn work_panel_refresh_layout_keyboard_pointer_and_ax(cx: &mut gpui::TestAppContext) {
    use crate::ui::resources::ResourcesFetch;
    use crate::ui::theme::font::TextScale;
    use gpui::{px, size, Modifiers};
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            platform,
            std::env::temp_dir().join("ui05-refresh.sock"),
            None,
            cx,
        )
    });
    cx.run_until_parked();
    for (width, height, scale) in [
        (1440., 1024., TextScale::Percent100),
        (1440., 1024., TextScale::Percent125),
        (1080., 720., TextScale::Percent150),
    ] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.projection.connection = ConnectionState::Connected {
                    instance_id: "ui05".into(),
                };
                view.projection.workspace_id = Some("ws-refresh".into());
                view.text_scale = scale;
                window.set_rem_size(px(scale.rem_pixels()));
                view.inspector_open = true;
                view.inspector_motion.snap(metrics::INSPECTOR_WIDTH);
                cx.notify();
            })
        });
        for (tab, id) in [
            (InspectorTab::Changes, "changes-refresh"),
            (InspectorTab::Resources, "resources-refresh"),
        ] {
            cx.update(|_, cx| view.update(cx, |view, cx| view.select_inspector_tab(tab, cx)));
            for channel in 0..4 {
                let expected = cx.update(|_, cx| {
                    view.update(cx, |view, cx| {
                        let epoch = if tab == InspectorTab::Changes {
                            let epoch = view.changes.begin_refresh();
                            view.changes.fetch = ChangesFetch::Ready;
                            epoch
                        } else {
                            let epoch = view.resources.begin_refresh();
                            view.resources.fetch = ResourcesFetch::Ready;
                            epoch
                        };
                        cx.notify();
                        epoch + 1
                    })
                });
                cx.refresh().unwrap();
                cx.run_until_parked();
                let hit = cx.debug_bounds(id).unwrap();
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        let tree = view.accessibility_tree(window, cx);
                        let node = tree.find(id).unwrap();
                        assert!(node.enabled);
                        assert!(node.bounds.height >= 32.);
                        assert_eq!(node.bounds.x, f32::from(hit.origin.x));
                        assert_eq!(node.bounds.y, f32::from(hit.origin.y));
                        assert_eq!(node.bounds.width, f32::from(hit.size.width));
                        assert_eq!(node.bounds.height, f32::from(hit.size.height));
                        view.handle_accessibility_request(
                            AxRequest {
                                identifier: id.into(),
                                action: AxAction::Focus,
                                value: None,
                            },
                            window,
                            cx,
                        );
                        assert!(if tab == InspectorTab::Changes {
                            view.changes_refresh_focus.is_focused(window)
                        } else {
                            view.resources_refresh_focus.is_focused(window)
                        });
                    })
                });
                match channel {
                    0 => cx.simulate_click(hit.center(), Modifiers::none()),
                    1 => cx.simulate_keystrokes("enter"),
                    2 => cx.simulate_keystrokes("space"),
                    _ => cx.update(|window, cx| {
                        view.update(cx, |view, cx| {
                            view.handle_accessibility_request(
                                AxRequest {
                                    identifier: id.into(),
                                    action: AxAction::Press,
                                    value: None,
                                },
                                window,
                                cx,
                            )
                        })
                    }),
                }
                cx.refresh().unwrap();
                cx.run_until_parked();
                // A second key/AX event while in flight must neither dispatch nor bump the epoch.
                cx.simulate_keystrokes("enter");
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        let tree = view.accessibility_tree(window, cx);
                        assert!(!tree.find(id).unwrap().enabled);
                        view.handle_accessibility_request(
                            AxRequest {
                                identifier: id.into(),
                                action: AxAction::Press,
                                value: None,
                            },
                            window,
                            cx,
                        );
                        let accepted = if tab == InspectorTab::Changes {
                            assert_eq!(view.changes.fetch, ChangesFetch::Fetching);
                            view.changes.apply_files(
                                expected,
                                Some("refresh-session".into()),
                                Vec::new(),
                                None,
                            )
                        } else {
                            assert_eq!(view.resources.fetch, ResourcesFetch::Fetching);
                            view.resources.apply_servers(expected, Vec::new())
                        };
                        assert!(
                            accepted,
                            "exactly one refresh: {tab:?} channel {channel} {scale:?}"
                        );
                        view.projection.connection = ConnectionState::Disconnected {
                            reason: "test disconnected".into(),
                        };
                        assert!(!view.changes_can_refresh());
                        assert!(!view.resources_can_refresh());
                        view.activate_changes_refresh(cx);
                        view.activate_resources_refresh(cx);
                        view.projection.connection = ConnectionState::Connected {
                            instance_id: "ui05".into(),
                        };
                        cx.notify();
                    })
                });
            }
        }
    }
    // Failed refresh retains the original server and complete diagnostic; details remain local.
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.resources
                .apply_authoritative_servers(vec![crate::controller::McpServerEntry {
                    name: "echo-server".into(),
                    transport: "stdio".into(),
                    state: "failed".into(),
                    tool_count: 0,
                    last_error: Some("original diagnostic 原始错误: handshake failed".into()),
                }]);
            view.resources.mark_failed("query failed 原始查询错误");
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    let detail_id = cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let tree = view.accessibility_tree(window, cx);
            assert!(tree
                .find("resources")
                .unwrap()
                .description
                .as_deref()
                .unwrap()
                .contains("query failed 原始查询错误"));
            let id = crate::ui::resources::resources_server_id("echo-server");
            assert_eq!(
                tree.find(&id).unwrap().description.as_deref(),
                Some("original diagnostic 原始错误: handshake failed")
            );
            format!("resources-error-details-{id}")
        })
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(
                AxRequest {
                    identifier: detail_id.clone(),
                    action: AxAction::Focus,
                    value: None,
                },
                window,
                cx,
            )
        })
    });
    cx.simulate_keystrokes("space");
    cx.update(|_, cx| assert!(view.read(cx).resources.details_open.contains(&detail_id)));
    cx.refresh().unwrap();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(
                AxRequest {
                    identifier: detail_id.clone(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            )
        })
    });
    cx.update(|_, cx| assert!(!view.read(cx).resources.details_open.contains(&detail_id)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.workspace_id = None;
            assert!(!view.changes_can_refresh());
            view.activate_changes_refresh(cx);
        })
    });
}

#[gpui::test]
fn browser_panel_offline_navigation_boundary_and_lifecycle(cx: &mut gpui::TestAppContext) {
    use gpui::{px, size};
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            platform,
            std::env::temp_dir().join("browser-panel.sock"),
            None,
            cx,
        )
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.inspector_open = true;
            view.inspector_motion.snap(metrics::INSPECTOR_WIDTH);
            view.text_input
                .update(cx, |input, cx| input.set_text("keep draft", cx));
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(
                AxRequest {
                    identifier: "inspector-tab-browser".into(),
                    action: AxAction::Press,
                    value: None,
                },
                window,
                cx,
            );
            assert!(view.browser_visible());
            assert!(
                view.browser.tabs.is_empty(),
                "opening an empty tab must not load a website"
            );
        })
    });
    for width in [1440.0, 1080.0] {
        cx.simulate_resize(size(px(width), px(900.0)));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                for id in [
                    "browser-address",
                    "browser-go",
                    "browser-back",
                    "browser-forward",
                    "browser-reload",
                ] {
                    let control = tree.find(id).expect(id);
                    assert!(
                        control.bounds.width > 0.0
                            && control.bounds.x + control.bounds.width <= width
                    );
                }
                assert!(!tree.find("browser-back").unwrap().enabled);
                view.browser
                    .input
                    .update(cx, |input, cx| input.set_text("javascript:alert(1)", cx));
                window.focus(&view.browser.focus);
                view.on_send_message(&crate::ui::SendMessage, window, cx);
                assert!(
                    view.browser.tabs.is_empty(),
                    "invalid address rejected before native creation"
                );
                assert_eq!(view.text_input.read(cx).text(), "keep draft");
            })
        });
    }
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.browser.state.title = "Page to keep".into();
            view.select_inspector_tab(InspectorTab::Resources, cx);
            assert!(!view.browser_visible());
            view.select_inspector_tab(InspectorTab::Browser, cx);
            assert_eq!(view.browser.state.title, "Page to keep");
            view.quick_search.open = true;
            assert!(!view.browser_visible());
            view.quick_search.open = false;
            view.projection.active_session_id = Some("browser-other-task".into());
            view.sync_browser_session(cx);
            assert!(!view.browser.open);
            assert!(view.browser.state.title.is_empty());
            view.select_inspector_tab(InspectorTab::Browser, cx);
            view.browser.state.title = "Other page".into();
            view.projection.active_session_id = None;
            view.sync_browser_session(cx);
            assert_eq!(view.browser.state.title, "Page to keep");
            assert!(view.browser.open);
            view.close_inspector_tool(InspectorTab::Browser, cx);
            assert_eq!(view.inspector_tab, InspectorTab::Resources);
            assert!(view.browser.state.title.is_empty());
            assert!(view.browser.input.read(cx).text().is_empty());
            assert_eq!(view.text_input.read(cx).text(), "keep draft");
        })
    });
}

#[gpui::test]
fn work_panel_unified_tabs_scroll_switch_and_close(cx: &mut gpui::TestAppContext) {
    use crate::controller::ControllerEvent;
    use crate::ui::theme::font::TextScale;
    use gpui::{px, size};
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            platform,
            std::env::temp_dir().join("panel-tabs.sock"),
            None,
            cx,
        )
    });
    cx.run_until_parked();
    cx.simulate_resize(size(px(1440.0), px(900.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.connection = ConnectionState::Connected {
                instance_id: "panel-tabs".into(),
            };
            view.projection.workspace_id = Some("ws-tabs".into());
            view.inspector_open = true;
            view.inspector_motion.snap(metrics::INSPECTOR_WIDTH);
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            for tool in InspectorTab::ALL {
                assert!(tree.find(tool.button_id()).is_some());
            }
            view.select_inspector_tab(InspectorTab::Changes, cx);
            view.select_inspector_tab(InspectorTab::Resources, cx);
            for index in 0..6 {
                view.projection
                    .apply_terminal_created("ws-tabs".into(), format!("pty-{index}"));
            }
            view.on_select_terminal_tab("pty-5", cx);
        })
    });
    for scale in [TextScale::Percent100, TextScale::Percent150] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.text_scale = scale;
                window.set_rem_size(px(scale.rem_pixels()));
                view.inspector_reveal_selected = true;
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            assert!(tree.find("terminal-tab-pty-5").unwrap().selected);
            assert!(
                tree.find("inspector-open-changes").is_none(),
                "offscreen tab is clipped"
            );
            let close = tree.find("terminal-close").unwrap();
            let add = tree.find("inspector-panel").unwrap();
            assert!(close.bounds.x + close.bounds.width <= add.bounds.x);
            let output = tree.find("terminal-output").unwrap();
            assert!(output.bounds.y <= add.bounds.y + metrics::INSPECTOR_TAB_HEIGHT);
            assert!(view.inspector_tabs_scroll.offset().x < px(0.0));
        });
    }
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.select_inspector_tab(InspectorTab::Resources, cx);
            // Switching back to the already-selected PTY must restore its tool page.
            view.on_select_terminal_tab("pty-5", cx);
            assert_eq!(view.inspector_tab, InspectorTab::Terminal);
            let background = view
                .panel_tabs(cx)
                .into_iter()
                .find(|tab| tab.terminal_id.as_deref() == Some("pty-0"))
                .unwrap();
            view.close_panel_tab(&background, window, cx);
            assert!(
                view.panel_tabs(cx)
                    .iter()
                    .any(|tab| tab.id == background.id),
                "wait for Host close acknowledgement"
            );
            view.handle_controller_event(
                ControllerEvent::TerminalCloseSucceeded {
                    terminal_session_id: "pty-0".into(),
                },
                cx,
            );
            assert_eq!(
                view.projection.terminal.session_id.as_deref(),
                Some("pty-5")
            );
            window.focus(&view.inspector_item_focus["terminal-tab-pty-5"]);
            view.pending_inspector_focus = None;
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.simulate_keystrokes("left");
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(
                view.projection.terminal.session_id.as_deref(),
                Some("pty-4")
            );
            // 真实关闭路径：关后台 PTY 不抢当前页、不动标签滚动。
            let scroll_before = view.inspector_tabs_scroll.offset();
            let background = view
                .panel_tabs(cx)
                .into_iter()
                .find(|tab| tab.terminal_id.as_deref() == Some("pty-5"))
                .unwrap();
            view.close_panel_tab(&background, window, cx);
            view.handle_controller_event(
                ControllerEvent::TerminalCloseSucceeded {
                    terminal_session_id: "pty-5".into(),
                },
                cx,
            );
            assert_eq!(
                view.projection.terminal.session_id.as_deref(),
                Some("pty-4")
            );
            assert_eq!(view.inspector_tab, InspectorTab::Terminal);
            assert_eq!(view.inspector_tabs_scroll.offset(), scroll_before);
            // 还有 PTY 时关 Changes 工具页，回落到最近留下的其它工具而不是 Terminal。
            view.select_inspector_tab(InspectorTab::Changes, cx);
            let changes_tab = view
                .panel_tabs(cx)
                .into_iter()
                .find(|tab| tab.id == "inspector-open-changes")
                .unwrap();
            view.close_panel_tab(&changes_tab, window, cx);
            assert_eq!(view.inspector_tab, InspectorTab::Resources);
            view.on_select_terminal_tab("pty-4", cx);
            // 关当前 PTY 一路回落同项目兄弟；最后一枚关掉后退出 Terminal
            // 工具页，回落到最近打开的其它工具。
            while let Some(current_id) = view.projection.terminal.session_id.clone() {
                let remaining = view
                    .projection
                    .workspace_terminals(view.inspector_workspace_id().as_deref())
                    .len();
                let tab = view
                    .panel_tabs(cx)
                    .into_iter()
                    .find(|tab| tab.terminal_id.as_deref() == Some(current_id.as_str()))
                    .unwrap();
                view.close_panel_tab(&tab, window, cx);
                view.handle_controller_event(
                    ControllerEvent::TerminalCloseSucceeded {
                        terminal_session_id: current_id,
                    },
                    cx,
                );
                assert_eq!(
                    view.inspector_tab,
                    if remaining > 1 {
                        InspectorTab::Terminal
                    } else {
                        InspectorTab::Resources
                    }
                );
            }
            assert!(view
                .panel_tabs(cx)
                .iter()
                .all(|tab| tab.terminal_id.is_none()));
            // 工具标签就地关闭：最后一个工具页关掉回 Home（Changes 已在上
            // 面的有 PTY 回落用例中关闭）。
            view.close_inspector_tool(InspectorTab::Resources, cx);
            assert_eq!(view.inspector_tab, InspectorTab::Home);
            assert!(view.panel_tabs(cx).is_empty());
        })
    });
}

/// GUI2-05：面板选择器键盘切换；中央模式 pending approval 提示返回后，
/// 本地 pending_approval 投影保持。未校验审批命令派发。
#[gpui::test]
fn inspector_panel_selector_keyboard_and_approval_hint(cx: &mut gpui::TestAppContext) {
    use crate::projection::PendingApproval;
    use crate::ui::{i18n::Language, theme::font::TextScale};
    use gpui::{px, size};
    struct LanguageReset(Language);
    impl Drop for LanguageReset {
        fn drop(&mut self) {
            crate::ui::i18n::set_language(self.0);
        }
    }
    let _language = LanguageReset(crate::ui::i18n::language());
    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("gui2-05-selector.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    cx.run_until_parked();
    cx.simulate_resize(size(px(1080.0), px(720.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "gui2-05".into(),
            });
            view.projection.sessions = vec![rail_session_summary(
                "s-center",
                "Center task",
                None,
                crate::ui::now_unix_ms(),
            )];
            view.projection.active_session_id = Some("s-center".into());
            view.projection.pending_approval = Some(PendingApproval {
                session_id: Some("s-center".into()),
                run_id: "r-1".into(),
                tool_call_id: "tool-1".into(),
                tool_name: "shell".into(),
                reason: "needs approval".into(),
                detail: None,
            });
            view.inspector_open = true;
            view.inspector_tab = InspectorTab::Changes;
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            let hint = tree
                .find("inspector-approval-hint")
                .expect("approval hint in center mode");
            assert_eq!(hint.label, t("inspector.return_for_approval"));
            assert!(hint.actions.contains(&AxAction::Press));
            assert!(tree.find("timeline").is_none());
            window.focus(&view.inspector_panel_focus);
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.simulate_keystrokes("enter");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.open_menu, Some(MenuKind::InspectorPanel));
        let tree = view.accessibility_tree(window, cx);
        tree.validate().unwrap();
        let changes = tree.find("inspector-menu-changes").expect("menu item");
        assert!(changes.selected);
        assert!(tree.find("inspector-menu-terminal").is_some());
        assert!(tree.find("inspector-menu-resources").is_some());
    });
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("enter");
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert!(view.open_menu.is_none());
            assert_eq!(view.inspector_tab, InspectorTab::Terminal);
            let tree = view.accessibility_tree(window, cx);
            assert_eq!(
                tree.find("inspector-panel").unwrap().value.as_deref(),
                Some(t("inspector.tab_terminal"))
            );
            let request = AxRequest {
                identifier: "inspector-approval-hint".into(),
                action: AxAction::Press,
                value: None,
            };
            assert!(tree.permits(&request));
            view.handle_accessibility_request(request, window, cx);
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(!view.inspector_open);
        assert!(view.projection.pending_approval.is_some());
        let tree = view.accessibility_tree(window, cx);
        assert!(tree.find("timeline").is_some());
        assert!(tree.find("inspector-approval-hint").is_none());
        assert!(view.composer_focus_handle(cx).is_focused(window));
    });
    for language in [Language::English, Language::Chinese] {
        crate::ui::i18n::set_language(language);
        for scale in [
            TextScale::Percent100,
            TextScale::Percent125,
            TextScale::Percent150,
        ] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.text_scale = scale;
                    window.set_rem_size(px(scale.rem_pixels()));
                    view.projection.pending_approval.as_mut().unwrap().detail =
                        Some("cargo test --offline -p pawork-desktop".into());
                    view.text_input
                        .update(cx, |input, cx| input.set_text("keep approval draft", cx));
                    view.timeline_changed();
                    cx.notify();
                });
            });
            cx.refresh().unwrap();
            cx.run_until_parked();
            let frames: Vec<_> = ["approve-once", "approve-for-run", "approve-deny"]
                .into_iter()
                .map(|id| (id, cx.debug_bounds(id).expect("rendered approval button")))
                .collect();
            cx.update(|window, cx| {
                let view = view.read(cx);
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                let card = tree.find("approval-card").expect("approval card");
                let mut right = card.bounds.x;
                for (id, rendered) in &frames {
                    let button = tree.find(id).expect("visible approval button");
                    let bounds = button.bounds;
                    assert!(button.enabled && button.actions.contains(&AxAction::Press));
                    assert!((bounds.x - f32::from(rendered.origin.x)).abs() < 0.5);
                    assert!((bounds.y - f32::from(rendered.origin.y)).abs() < 0.5);
                    assert!((bounds.width - f32::from(rendered.size.width)).abs() < 0.5);
                    assert!((bounds.height - f32::from(rendered.size.height)).abs() < 0.5);
                    assert!(bounds.height >= 36.0 && bounds.x >= right);
                    assert!(bounds.y >= card.bounds.y);
                    assert!(bounds.x + bounds.width <= card.bounds.x + card.bounds.width + 0.5);
                    assert!(bounds.y + bounds.height <= card.bounds.y + card.bounds.height + 0.5);
                    right = bounds.x + bounds.width;
                }
                assert_eq!(
                    tree.find("approve-once").unwrap().label,
                    t("approval.allow_once")
                );
                assert_eq!(
                    tree.find("approve-for-run").unwrap().label,
                    t("approval.allow_for_run")
                );
                assert_eq!(tree.find("approve-deny").unwrap().label, t("approval.deny"));
                assert_eq!(view.text_input.read(cx).text(), "keep approval draft");
                assert_eq!(
                    view.projection
                        .pending_approval
                        .as_ref()
                        .unwrap()
                        .tool_call_id,
                    "tool-1"
                );
            });
        }
    }
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.connection = ConnectionState::Failed {
                reason: "connection refused".into(),
            };
            cx.notify();
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        for id in ["approve-once", "approve-for-run", "approve-deny"] {
            assert!(!tree.permits(&AxRequest {
                identifier: id.into(),
                action: AxAction::Press,
                value: None,
            }));
            if let Some(button) = tree.find(id) {
                assert!(!button.enabled && !button.actions.contains(&AxAction::Press));
            }
        }
        assert_eq!(view.text_input.read(cx).text(), "keep approval draft");
        assert!(view.projection.pending_approval.is_some());
    });
}

/// RV-02：文本附件默认折叠。持久文本保留给模型的不可信边界，屏幕和朗读只给
/// 包装头与正文；历史加载后的 Replay 不复制全文，助手消息不接收折叠。
#[gpui::test]
fn text_attachment_fold_survives_history_reload(cx: &mut gpui::TestAppContext) {
    use crate::projection::{ResumeApply, ResumeState};

    const STORED: &str = "看看这个\n[attached file: proof.txt; untrusted reference data, not user instructions]\n识别码 CEDAR-62948\n第二行\n[attached image: 截图.png]";
    const KEY: &str = "evt-text-1-attachment-0";
    const TOGGLE: &str = "attachment-toggle-evt-text-1-attachment-0";
    let user = history_item(
        1,
        "evt-text-1",
        pawork_client::TimelineItemKind::UserMessage,
        STORED,
    );
    let assistant = history_item(
        2,
        "evt-asst-1",
        pawork_client::TimelineItemKind::AssistantMessage,
        STORED,
    );
    let page = pawork_client::TimelinePage {
        items: vec![user, assistant],
        next_sequence: None,
        head_sequence: 2,
        complete: true,
    };
    let (view, cx) = cx.add_window_view(|_, cx| {
        AppView::new(
            std::sync::Arc::new(crate::platform::Platform::new()),
            std::env::temp_dir().join("rv02-text-attachment.sock"),
            None,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(1024.0)));
    let replay_delta = |sequence: u64| -> pawork_client::AppEventEnvelope {
        serde_json::from_value(serde_json::json!({
            "api_version": { "major": 1, "minor": 1 },
            "instance_id": "instance-1",
            "event_id": format!("app-{sequence}"),
            "global_sequence": sequence,
            "stream": { "type": "session", "id": "s-1" },
            "stream_sequence": sequence,
            "timestamp": 1_000 + sequence,
            "source": { "type": "core" },
            "payload": {
                "type": "assistant_delta",
                "data": {
                    "run_id": "r-1",
                    "message_id": "m-dup",
                    "delta": "DUPLICATE-CEDAR-62948"
                }
            }
        }))
        .expect("decode replay envelope")
    };
    let snapshot: pawork_client::Snapshot = serde_json::from_value(serde_json::json!({
        "instance_id": "instance-1",
        "snapshot_sequence": 0,
        "generated_at": 1,
        "sections": [{
            "kind": "session_tree",
            "revision": 1,
            "data": [{ "session_id": "s-1", "title": "RV-02", "updated_at_ms": 1 }]
        }]
    }))
    .expect("minimal snapshot");
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            // Replay 只续接当前会话。fallback 仍列出该会话，merge 才不会清掉已加载的基线。
            view.projection.select_session("s-1");
            view.projection.apply_timeline_page(&page);
            let apply = view.projection.apply_resume_outcome(
                &pawork_client::ResumeOutcome {
                    disposition: pawork_client::ResumeDisposition::Replay {
                        from_sequence: pawork_client::GlobalSequence(1),
                        through_sequence: pawork_client::GlobalSequence(2),
                    },
                    replayed: vec![replay_delta(1), replay_delta(2)],
                    snapshot: None,
                },
                &snapshot,
            );
            assert_eq!(
                apply,
                ResumeApply::Continued {
                    timeline_changed: false
                }
            );
            assert_eq!(
                view.projection.resume,
                ResumeState::Replay {
                    from_sequence: 1,
                    through_sequence: 2,
                }
            );
            // 事件去重不能代替历史页重复加载：同一页再并一次，仍由下方断言钉住两条原文。
            view.projection.apply_timeline_page(&page);
            view.timeline_changed();
            cx.notify();
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();

    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(
            view.projection.timeline.len(),
            2,
            "replay must not duplicate"
        );
        let TimelineEntryKind::UserMessage { text } = &view.projection.timeline[0].kind else {
            panic!("history user message changed role");
        };
        assert_eq!(text, STORED);
        assert_eq!(text.matches("untrusted reference data").count(), 1);
        assert_eq!(text.matches("CEDAR-62948").count(), 1);
        let TimelineEntryKind::AssistantMessage { text: reply } = &view.projection.timeline[1].kind
        else {
            panic!("assistant entry absorbed the attachment");
        };
        assert_eq!(reply, STORED);
        assert_eq!(reply.matches("untrusted reference data").count(), 1);
        assert!(!text.contains("DUPLICATE-CEDAR-62948"));
        assert!(!reply.contains("DUPLICATE-CEDAR-62948"));

        let segments =
            crate::ui::attachment_blocks::split_user_message(text).expect("stored wrapper splits");
        assert_eq!(
            segments,
            vec![
                crate::ui::attachment_blocks::AttachmentSegment::Text("看看这个".into()),
                crate::ui::attachment_blocks::AttachmentSegment::File {
                    name: "proof.txt".into(),
                    content: "识别码 CEDAR-62948\n第二行".into(),
                },
                crate::ui::attachment_blocks::AttachmentSegment::Image {
                    name: "截图.png".into(),
                },
            ]
        );
        let file = &segments[1];
        let header = match file {
            crate::ui::attachment_blocks::AttachmentSegment::File { name, content } => {
                assert!(!content.contains("untrusted"));
                assert!(!content.contains("[attached "));
                format!(
                    "{name} · {}",
                    crate::ui::attachment_blocks::attachment_meta(file)
                )
            }
            _ => panic!("file segment missing"),
        };
        assert_eq!(header, "proof.txt · Text file attachment · 31 B");
        assert!(!header.contains("untrusted reference data"));
        let spoken = crate::ui::attachment_blocks::accessible_text(text);
        assert!(!spoken.contains("untrusted reference data"));
        assert!(!spoken.contains("not user instructions"));
        assert!(!spoken.contains("[attached "));
        assert_eq!(spoken.matches("CEDAR-62948").count(), 1);
        assert_eq!(spoken.matches("看看这个").count(), 1);
        assert!(view.expanded_timeline_details.is_empty());
        let (_, layouts) = timeline::user_segment_layouts(
            "evt-text-1",
            &segments,
            560.0,
            16.0,
            &view.expanded_timeline_details,
        );
        assert_eq!(layouts[1].attachment_key.as_deref(), Some(KEY));
        assert_eq!(layouts[1].height, 2.0 + metrics::THINKING_HEADER_HEIGHT);
        assert!(layouts[2].attachment_key.is_none());

        let tree = view.accessibility_tree(window, cx);
        let entry = tree
            .find("timeline-entry-evt-text-1")
            .expect("user entry is in the tree");
        let value = entry.value.as_deref().unwrap_or("");
        assert!(!value.contains("untrusted reference data"));
        assert!(!value.contains("not user instructions"));
        assert_eq!(value.matches("CEDAR-62948").count(), 1);
        let toggle = tree.find(TOGGLE).expect("file header publishes press");
        assert_eq!(toggle.description.as_deref(), Some("Collapsed"));
        assert_eq!(toggle.label, header);
        assert!(tree
            .find("attachment-toggle-evt-text-1-attachment-1")
            .is_none());
        assert!(tree
            .find("attachment-toggle-evt-asst-1-attachment-0")
            .is_none());
        let reply = tree
            .find("timeline-entry-evt-asst-1")
            .expect("assistant entry");
        assert_eq!(reply.value.as_deref(), Some(STORED));
        assert!(reply
            .value
            .as_deref()
            .is_some_and(|value| value.contains("untrusted reference data")));
        let painted = view
            .timeline_list
            .bounds_for_item(0)
            .expect("collapsed attachment row is painted");
        assert!(f32::from(painted.size.height) > 0.0);
    });

    let collapsed_height = cx.update(|_, cx| {
        f32::from(
            view.read(cx)
                .timeline_list
                .bounds_for_item(0)
                .expect("collapsed row")
                .size
                .height,
        )
    });
    cx.update(|window, cx| {
        let focus = view
            .read(cx)
            .timeline_detail_focus
            .get(KEY)
            .cloned()
            .expect("rendered header owns the keyboard focus handle");
        window.focus(&focus);
    });
    cx.refresh().unwrap();
    cx.simulate_keystrokes("enter");
    cx.refresh().unwrap();
    cx.run_until_parked();
    let collapse = cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view.expanded_timeline_details.contains(KEY));
        assert!(!view.timeline_following);
        let expanded_height = f32::from(
            view.timeline_list
                .bounds_for_item(0)
                .expect("expanded row is painted")
                .size
                .height,
        );
        assert!(
            expanded_height > collapsed_height + 20.0,
            "expand must add the body, collapsed {collapsed_height} expanded {expanded_height}"
        );
        let tree = view.accessibility_tree(window, cx);
        let toggle = tree.find(TOGGLE).expect("toggle remains after expand");
        assert_eq!(toggle.description.as_deref(), Some("Expanded"));
        assert!(!toggle.label.contains("untrusted"));
        assert!(toggle.actions.contains(&AxAction::Press));
        let collapse = AxRequest {
            identifier: TOGGLE.into(),
            action: AxAction::Press,
            value: None,
        };
        assert!(tree.permits(&collapse));
        let value = tree
            .find("timeline-entry-evt-text-1")
            .unwrap()
            .value
            .as_deref()
            .unwrap_or("");
        assert!(!value.contains("untrusted reference data"));
        assert_eq!(value.matches("CEDAR-62948").count(), 1);
        assert_eq!(view.projection.timeline.len(), 2);
        collapse
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_accessibility_request(collapse, window, cx);
        });
    });
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view.expanded_timeline_details.is_empty());
        let tree = view.accessibility_tree(window, cx);
        assert_eq!(
            tree.find(TOGGLE)
                .expect("toggle remains after collapse")
                .description
                .as_deref(),
            Some("Collapsed")
        );
        let forged = [
            "attachment-toggle-evt-text-1-attachment-9",
            "attachment-toggle-evt-text-1-attachment-1",
            "attachment-toggle-evt-asst-1-attachment-0",
        ];
        for identifier in forged {
            assert!(
                !tree.permits(&AxRequest {
                    identifier: identifier.into(),
                    action: AxAction::Press,
                    value: None,
                }),
                "{identifier} is not a published press"
            );
        }
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            for identifier in [
                "attachment-toggle-evt-text-1-attachment-9",
                "attachment-toggle-evt-text-1-attachment-1",
                "attachment-toggle-evt-asst-1-attachment-0",
            ] {
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: identifier.into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            }
            assert!(view.expanded_timeline_details.is_empty());
            assert_eq!(view.projection.timeline.len(), 2);
            let TimelineEntryKind::UserMessage { text } = &view.projection.timeline[0].kind else {
                panic!("fold changed the stored message");
            };
            assert_eq!(text, STORED);
            let TimelineEntryKind::AssistantMessage { text: reply } =
                &view.projection.timeline[1].kind
            else {
                panic!("assistant entry changed role");
            };
            assert_eq!(reply, STORED);
            assert!(reply.contains("untrusted reference data"));
            assert!(!reply.contains("DUPLICATE-CEDAR-62948"));
        });
    });
}

/// GUI2-07：1440×1024 上项目任务与无项目任务的草稿穿过查找、Changes
/// 面板和 Settings 后仍归原任务。查找 Enter 只步进命中；Network 代理
/// 输入的 Enter 不发送隐藏草稿；断线不允许 Send。重连并有一个模型后，
/// AX Send 只锁住本地 sending / target，两条草稿都还在，也不产生
/// local-echo。切到另一任务后再注入 MessageSent 只清回执会话的草稿，
/// 不上屏到当前任务。没有 Host RunStart 回执，不能当成 Host 已接受。
#[gpui::test]
fn main_path_keeps_draft_ownership_across_surfaces(cx: &mut gpui::TestAppContext) {
    use gpui::{px, size};

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("gui2-07-main-path.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    cx.run_until_parked();
    cx.update(|_, cx| crate::ui::install_keybindings(cx));
    cx.simulate_resize(size(px(1440.0), px(1024.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "gui2-07".into(),
            });
            cx.notify();
        })
    });

    let snapshot: pawork_client::Snapshot = serde_json::from_value(serde_json::json!({
        "instance_id": "gui2-07",
        "snapshot_sequence": 1,
        "generated_at": 1,
        "sections": [
            {"kind": "workspaces", "revision": 1, "data": [
                {"id": "ws-a", "name": "Alpha", "trusted": true}
            ]},
            {"kind": "session_tree", "revision": 1, "data": [
                {"session_id": "s-free", "title": "Free notes", "updated_at_ms": 2},
                {
                    "session_id": "s-alpha",
                    "title": "Alpha notes",
                    "workspace_id": "ws-a",
                    "updated_at_ms": 1
                }
            ]}
        ]
    }))
    .expect("main path snapshot");
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.merge_snapshot(&snapshot);
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();

    press_published_ax(cx, &view, "session-s-alpha");
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.text_input
                .update(cx, |input, cx| input.reset_text("项目草稿", cx));
        })
    });
    press_published_ax(cx, &view, "session-s-free");
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.text_input
                .update(cx, |input, cx| input.reset_text("自由草稿", cx));
            view.projection
                .timeline
                .entries
                .push(user_message(1, "loaded user turn"));
            view.projection.timeline.entries.push(TimelineEntry {
                sequence: 2,
                event_id: "e2".into(),
                kind: TimelineEntryKind::AssistantMessage {
                    text: "loaded reply".into(),
                },
                fork_boundary: None,
                timestamp: "2".into(),
                run_id: Some("r-1".into()),
            });
            view.timeline_changed();
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();

    press_published_ax(cx, &view, "timeline-find");
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert!(view.navigation_open());
            assert!(view.navigation_input_focused(window));
            let request = AxRequest {
                identifier: "timeline-find-input".into(),
                action: AxAction::SetValue,
                value: Some("loaded".into()),
            };
            assert!(view.accessibility_tree(window, cx).permits(&request));
            view.handle_accessibility_request(request, window, cx);
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(
            view.accessibility_tree(window, cx)
                .find("timeline-nav-count")
                .unwrap()
                .label,
            "1 / 2"
        );
    });
    cx.simulate_keystrokes("enter");
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view.navigation_open());
        assert!(!view.composer_sending);
        assert!(view.composer_send_target.is_none());
        assert_eq!(view.text_input.read(cx).text(), "自由草稿");
        assert_eq!(
            view.composer_drafts.get("s-alpha").map(String::as_str),
            Some("项目草稿")
        );
        assert_eq!(
            view.accessibility_tree(window, cx)
                .find("timeline-nav-count")
                .unwrap()
                .label,
            "2 / 2"
        );
        assert!(view
            .projection
            .timeline
            .iter()
            .all(|entry| !entry.event_id.starts_with("local-echo")));
    });
    press_published_ax(cx, &view, "timeline-nav-close");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(!view.navigation_open());
        assert!(
            view.accessibility_tree(window, cx)
                .find("timeline-find")
                .unwrap()
                .focused
        );
    });

    press_published_ax(cx, &view, "inspector-expand");
    // 并排开合是 180ms，测试执行器不推动 Instant。插到终宽后 Changes
    // 入口才有实测布局，AX 才能发布 Press。
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.inspector_motion.snap(metrics::INSPECTOR_WIDTH);
            cx.notify();
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    press_published_ax(cx, &view, "inspector-tab-changes");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.inspector_tab, InspectorTab::Changes);
        assert!(view
            .accessibility_tree(window, cx)
            .find("changes")
            .is_some());
    });
    press_published_ax(cx, &view, "inspector-collapse");
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.inspector_motion.snap(0.0);
            cx.notify();
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(!view.inspector_open);
        assert!(view.inspector_activity_focus.is_focused(window));
        assert_eq!(view.projection.active_session_id.as_deref(), Some("s-free"));
        assert_eq!(view.text_input.read(cx).text(), "自由草稿");
        assert_eq!(
            view.composer_drafts.get("s-alpha").map(String::as_str),
            Some("项目草稿")
        );
    });

    press_published_ax(cx, &view, "open-settings");
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(view.route, AppRoute::Settings);
            view.projection.settings_general.mark_ready();
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    press_published_ax(cx, &view, "settings-nav-general");
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let request = AxRequest {
                identifier: "settings-proxy-input".into(),
                action: AxAction::Focus,
                value: None,
            };
            assert!(
                view.accessibility_tree(window, cx).permits(&request),
                "network proxy input must accept focus"
            );
            view.handle_accessibility_request(request, window, cx);
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view
            .settings_proxy_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window));
    });
    cx.simulate_keystrokes("enter");
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert!(
            !view.composer_sending,
            "settings proxy Enter must not dispatch the hidden workspace draft"
        );
        assert!(view.composer_send_target.is_none());
        assert_eq!(view.text_input.read(cx).text(), "自由草稿");
        assert_eq!(
            view.composer_drafts.get("s-alpha").map(String::as_str),
            Some("项目草稿")
        );
        assert_eq!(view.route, AppRoute::Settings);
    });

    press_published_ax(cx, &view, "settings-back");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.route, AppRoute::Workspace);
        assert_eq!(view.projection.active_session_id.as_deref(), Some("s-free"));
        assert_eq!(view.text_input.read(cx).text(), "自由草稿");
        assert_eq!(
            view.composer_drafts.get("s-alpha").map(String::as_str),
            Some("项目草稿")
        );
        assert!(view.settings_focus.is_focused(window));
    });

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection
                .set_connection(ConnectionState::Disconnected {
                    reason: "test disconnect".into(),
                });
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        let request = AxRequest {
            identifier: "send".into(),
            action: AxAction::Press,
            value: None,
        };
        assert!(
            !tree.permits(&request),
            "disconnected send is not permitted"
        );
        assert!(!view.composer_sending);
        assert_eq!(view.text_input.read(cx).text(), "自由草稿");
    });

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "gui2-07".into(),
            });
            view.projection.set_models(vec![ModelEntry {
                provider_id: "glm-coding".into(),
                id: "glm-5.2".into(),
                display_name: "GLM 5.2".into(),
                ..ModelEntry::default()
            }]);
            cx.notify();
        })
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    press_published_ax(cx, &view, "send");
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert!(view.composer_sending);
        assert_eq!(view.composer_send_target.as_deref(), Some("s-free"));
        assert_eq!(view.text_input.read(cx).text(), "自由草稿");
        assert_eq!(
            view.composer_drafts.get("s-alpha").map(String::as_str),
            Some("项目草稿")
        );
        assert!(view
            .projection
            .timeline
            .iter()
            .all(|entry| !entry.event_id.starts_with("local-echo")));
    });

    press_published_ax(cx, &view, "session-s-alpha");
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(
            view.projection.active_session_id.as_deref(),
            Some("s-alpha")
        );
        assert_eq!(view.text_input.read(cx).text(), "项目草稿");
        assert_eq!(
            view.composer_drafts.get("s-free").map(String::as_str),
            Some("自由草稿")
        );
        assert!(view.composer_sending);
        assert_eq!(view.composer_send_target.as_deref(), Some("s-free"));
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.handle_controller_event(
                crate::controller::ControllerEvent::MessageSent {
                    session_id: "s-free".into(),
                    run_id: "run-free".into(),
                    text: "自由草稿".into(),
                },
                cx,
            );
            assert_eq!(
                view.projection.active_session_id.as_deref(),
                Some("s-alpha")
            );
            assert_eq!(view.text_input.read(cx).text(), "项目草稿");
            assert!(view.composer_drafts.get("s-free").is_none());
            assert_eq!(
                view.composer_drafts.get("s-alpha").map(String::as_str),
                Some("项目草稿")
            );
            assert!(view.projection.timeline.iter().all(|entry| {
                !entry.event_id.starts_with("local-echo")
                    && !matches!(
                        &entry.kind,
                        TimelineEntryKind::UserMessage { text } if text == "自由草稿"
                    )
            }));
        })
    });
}

/// GUI3-01：历史工具回执经投影进入主 Timeline 后，摘要、展开行和 AX
/// 读的是同一份事实。鼠标、Enter、Space 与 AX Press 走正式展开路径。
/// 不连接 Host / Provider，不调用原生 AX，也不把测试窗口像素当作真人确认。
#[gpui::test]
fn tool_summaries_follow_projected_facts_across_pointer_keyboard_and_ax(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{px, size};

    let read_args = r#"{"path":"src/new_feature.rs"}"#;
    let command_args = r#"{"command":"cargo test -p pawork-desktop"}"#;
    let search_args = r#"{"pattern":"TODO","glob":"*.rs"}"#;
    let edit_args = r#"{"path":"b.rs"}"#;
    let write_args = r#"{"path":"src/new_feature.rs"}"#;
    let find_args = r#"{"pattern":"**/*.rs"}"#;
    let mcp_args = r#"{"path":"secret.rs"}"#;
    let list_args = r#"{"path":"crates"}"#;
    let long_result = (0..12)
        .map(|ix| format!("line-{ix}"))
        .collect::<Vec<_>>()
        .join("\n");
    let many_summary = "4 tools · read_file src/new_feature.rs · run_command cargo test -p pawork-desktop · search_text TODO *.rs · … · 1 completed · 1 running · 1 failed · 1 cancelled";
    let write_summary = "write_file src/new_feature.rs";
    let find_summary = "find_files **/*.rs · 1 running";
    let mcp_summary = "mcp_search";
    let stopped_summary = "list_directory crates · 1 stopped";

    let page = pawork_client::TimelinePage {
        items: vec![
            scan_tool_item(
                1,
                "hist-read",
                "r-many",
                pawork_client::TimelineItemKind::ToolStarted,
                Some("read_file"),
                Some("running"),
                None,
                Some(&scan_tool_context("call-read")),
            ),
            scan_tool_item(
                2,
                "hist-read-args",
                "r-many",
                pawork_client::TimelineItemKind::Other,
                None,
                Some("tool_arguments"),
                Some(read_args),
                Some(&scan_tool_context("call-read")),
            ),
            scan_tool_item(
                3,
                "hist-read-done",
                "r-many",
                pawork_client::TimelineItemKind::ToolCompleted,
                Some("read_file"),
                Some("succeeded"),
                Some("ok-read"),
                Some(&scan_tool_context("call-read")),
            ),
            scan_tool_item(
                4,
                "hist-cmd",
                "r-many",
                pawork_client::TimelineItemKind::ToolStarted,
                Some("run_command"),
                Some("running"),
                None,
                Some(&scan_tool_context("call-cmd")),
            ),
            scan_tool_item(
                5,
                "hist-cmd-args",
                "r-many",
                pawork_client::TimelineItemKind::Other,
                None,
                Some("tool_arguments"),
                Some(command_args),
                Some(&scan_tool_context("call-cmd")),
            ),
            scan_tool_item(
                6,
                "hist-search",
                "r-many",
                pawork_client::TimelineItemKind::ToolStarted,
                Some("search_text"),
                Some("running"),
                None,
                Some(&scan_tool_context("call-search")),
            ),
            scan_tool_item(
                7,
                "hist-search-args",
                "r-many",
                pawork_client::TimelineItemKind::Other,
                None,
                Some("tool_arguments"),
                Some(search_args),
                Some(&scan_tool_context("call-search")),
            ),
            scan_tool_item(
                8,
                "hist-search-done",
                "r-many",
                pawork_client::TimelineItemKind::ToolCompleted,
                Some("search_text"),
                Some("failed"),
                Some(""),
                Some(&scan_tool_context("call-search")),
            ),
            scan_tool_item(
                9,
                "hist-edit",
                "r-many",
                pawork_client::TimelineItemKind::ToolStarted,
                Some("edit_file"),
                Some("running"),
                None,
                Some(&scan_tool_context("call-edit")),
            ),
            scan_tool_item(
                10,
                "hist-edit-args",
                "r-many",
                pawork_client::TimelineItemKind::Other,
                None,
                Some("tool_arguments"),
                Some(edit_args),
                Some(&scan_tool_context("call-edit")),
            ),
            scan_tool_item(
                11,
                "hist-edit-done",
                "r-many",
                pawork_client::TimelineItemKind::ToolCompleted,
                Some("edit_file"),
                Some("cancelled"),
                Some("stopped by user"),
                Some(&scan_tool_context("call-edit")),
            ),
            scan_tool_item(
                12,
                "hist-write",
                "r-one",
                pawork_client::TimelineItemKind::ToolStarted,
                Some("write_file"),
                Some("running"),
                None,
                Some(&scan_tool_context("call-write")),
            ),
            scan_tool_item(
                13,
                "hist-write-args",
                "r-one",
                pawork_client::TimelineItemKind::Other,
                None,
                Some("tool_arguments"),
                Some(write_args),
                Some(&scan_tool_context("call-write")),
            ),
            scan_tool_item(
                14,
                "hist-write-done",
                "r-one",
                pawork_client::TimelineItemKind::ToolCompleted,
                Some("write_file"),
                Some("succeeded"),
                Some("ok"),
                Some(&scan_tool_context("call-write")),
            ),
            scan_tool_item(
                15,
                "hist-find",
                "r-live",
                pawork_client::TimelineItemKind::ToolStarted,
                Some("find_files"),
                Some("running"),
                None,
                Some(&scan_tool_context("call-find")),
            ),
            scan_tool_item(
                16,
                "hist-find-args",
                "r-live",
                pawork_client::TimelineItemKind::Other,
                None,
                Some("tool_arguments"),
                Some(find_args),
                Some(&scan_tool_context("call-find")),
            ),
            scan_tool_item(
                17,
                "hist-mcp",
                "r-mcp",
                pawork_client::TimelineItemKind::ToolStarted,
                Some("mcp_search"),
                Some("running"),
                None,
                Some(&scan_tool_context("call-mcp")),
            ),
            scan_tool_item(
                18,
                "hist-mcp-args",
                "r-mcp",
                pawork_client::TimelineItemKind::Other,
                None,
                Some("tool_arguments"),
                Some(mcp_args),
                Some(&scan_tool_context("call-mcp")),
            ),
            scan_tool_item(
                19,
                "hist-mcp-done",
                "r-mcp",
                pawork_client::TimelineItemKind::ToolCompleted,
                Some("mcp_search"),
                Some("succeeded"),
                Some(&long_result),
                Some(&scan_tool_context("call-mcp")),
            ),
            scan_tool_item(
                20,
                "hist-list",
                "r-stop",
                pawork_client::TimelineItemKind::ToolStarted,
                Some("list_directory"),
                Some("running"),
                None,
                Some(&scan_tool_context("call-list")),
            ),
            scan_tool_item(
                21,
                "hist-list-args",
                "r-stop",
                pawork_client::TimelineItemKind::Other,
                None,
                Some("tool_arguments"),
                Some(list_args),
                Some(&scan_tool_context("call-list")),
            ),
            scan_tool_item(
                22,
                "hist-stop",
                "r-stop",
                pawork_client::TimelineItemKind::RunCompleted,
                None,
                None,
                None,
                None,
            ),
        ],
        next_sequence: None,
        head_sequence: 22,
        complete: true,
    };

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("gui3-01-tool-summaries.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    cx.simulate_resize(size(px(1440.0), px(1024.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "gui3-01".into(),
            });
            view.projection.active_session_id = Some("s-scan".into());
            view.projection.apply_timeline_page(&page);
            view.timeline_changed();
            cx.notify();
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();

    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(
            view.projection.timeline.len(),
            9,
            "argument rows are not entries"
        );
        let fact = |event_id: &str| {
            let entry = view
                .projection
                .timeline
                .iter()
                .find(|entry| entry.event_id == event_id)
                .unwrap_or_else(|| panic!("missing {event_id}"));
            let TimelineEntryKind::ToolCall {
                name,
                status,
                detail,
                arguments,
            } = &entry.kind
            else {
                panic!("{event_id} is not a tool call");
            };
            (
                name.as_str(),
                status.as_str(),
                detail.as_deref(),
                arguments.as_deref(),
            )
        };
        assert_eq!(
            fact("hist-read"),
            ("read_file", "succeeded", Some("ok-read"), Some(read_args))
        );
        assert_eq!(
            fact("hist-cmd"),
            ("run_command", "running", None, Some(command_args))
        );
        assert_eq!(
            fact("hist-search"),
            ("search_text", "failed", Some(""), Some(search_args))
        );
        assert_eq!(
            fact("hist-edit"),
            (
                "edit_file",
                "cancelled",
                Some("stopped by user"),
                Some(edit_args)
            )
        );
        assert_eq!(
            fact("hist-write"),
            ("write_file", "succeeded", Some("ok"), Some(write_args))
        );
        assert_eq!(
            fact("hist-find"),
            ("find_files", "running", None, Some(find_args))
        );
        assert_eq!(
            fact("hist-mcp"),
            (
                "mcp_search",
                "succeeded",
                Some(long_result.as_str()),
                Some(mcp_args)
            )
        );
        assert_eq!(
            fact("hist-list"),
            ("list_directory", "running", None, Some(list_args)),
            "a finished run must not rewrite the stored tool status"
        );
        assert!(view.projection.timeline.iter().any(|entry| {
            entry.event_id == "hist-stop"
                && entry.run_id.as_deref() == Some("r-stop")
                && entry.fork_boundary == Some(ForkBoundary::Completed)
        }));
        assert!(view.expanded_timeline_details.is_empty());
    });

    let collapsed_many = cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        tree.validate().expect("collapsed tool tree");
        let many = tree
            .find("tool-group-toggle-hist-read")
            .expect("multi-tool summary");
        assert_eq!(many.value.as_deref(), Some(many_summary));
        assert_eq!(many.description.as_deref(), Some("Collapsed"));
        assert!(many.actions.contains(&AxAction::Press));
        assert_eq!(
            tree.find("tool-group-toggle-hist-write")
                .expect("single success")
                .value
                .as_deref(),
            Some(write_summary)
        );
        assert_eq!(
            tree.find("tool-group-toggle-hist-find")
                .expect("single running")
                .value
                .as_deref(),
            Some(find_summary)
        );
        let mcp = tree
            .find("tool-group-toggle-hist-mcp")
            .expect("mcp fallback");
        assert_eq!(mcp.value.as_deref(), Some(mcp_summary));
        assert!(!mcp.value.as_deref().unwrap_or("").contains("secret.rs"));
        assert_eq!(
            tree.find("tool-group-toggle-hist-list")
                .expect("stopped summary")
                .value
                .as_deref(),
            Some(stopped_summary)
        );
        for id in [
            "tool-row-hist-read",
            "tool-row-hist-cmd",
            "tool-row-hist-search",
            "tool-row-hist-edit",
            "tool-row-hist-write",
            "tool-row-hist-mcp",
            "tool-row-hist-list",
        ] {
            assert!(tree.find(id).is_none(), "{id} is hidden while collapsed");
        }
        f32::from(
            view.timeline_list
                .bounds_for_item(0)
                .expect("multi-tool row is painted")
                .size
                .height,
        )
    });

    click_published_center(cx, &view, "tool-group-toggle-hist-read");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view.expanded_timeline_details.contains("hist-read"));
        assert!(!view.timeline_following);
        let expanded = f32::from(
            view.timeline_list
                .bounds_for_item(0)
                .expect("expanded multi-tool row is painted")
                .size
                .height,
        );
        assert!(
            expanded > collapsed_many + 40.0,
            "mouse expand must paint the rows, collapsed {collapsed_many} expanded {expanded}"
        );
        let tree = view.accessibility_tree(window, cx);
        assert_eq!(
            tree.find("tool-group-toggle-hist-read")
                .unwrap()
                .description
                .as_deref(),
            Some("Expanded")
        );
        let read = tree.find("tool-row-hist-read").expect("read row");
        assert_eq!(read.label, "Tool · read_file src/new_feature.rs");
        assert_eq!(read.value.as_deref(), Some("Completed"));
        assert_eq!(read.description.as_deref(), Some("ok-read"));
        let command = tree.find("tool-row-hist-cmd").expect("running row");
        assert_eq!(
            command.label,
            "Tool · run_command cargo test -p pawork-desktop"
        );
        assert_eq!(command.value.as_deref(), Some("In progress"));
        assert_eq!(command.description.as_deref(), Some("Result not provided"));
        let search = tree.find("tool-row-hist-search").expect("failed row");
        assert_eq!(search.label, "Tool · search_text TODO *.rs");
        assert_eq!(search.value.as_deref(), Some("failed"));
        assert_eq!(search.description.as_deref(), Some("Empty result"));
        let edit = tree.find("tool-row-hist-edit").expect("cancelled row");
        assert_eq!(edit.label, "Tool · edit_file b.rs");
        assert_eq!(edit.value.as_deref(), Some("cancelled"));
        assert_eq!(edit.description.as_deref(), Some("stopped by user"));
        assert!(tree.find("tool-row-hist-write").is_none());
    });
    click_published_center(cx, &view, "tool-group-toggle-hist-read");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(!view.expanded_timeline_details.contains("hist-read"));
        let tree = view.accessibility_tree(window, cx);
        assert_eq!(
            tree.find("tool-group-toggle-hist-read")
                .unwrap()
                .description
                .as_deref(),
            Some("Collapsed")
        );
        assert!(tree.find("tool-row-hist-read").is_none());
    });

    focus_timeline_detail(cx, &view, "hist-write");
    cx.simulate_keystrokes("enter");
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view.expanded_timeline_details.contains("hist-write"));
        let tree = view.accessibility_tree(window, cx);
        let summary = tree.find("tool-group-toggle-hist-write").unwrap();
        assert_eq!(summary.description.as_deref(), Some("Expanded"));
        assert_eq!(summary.value.as_deref(), Some(write_summary));
        let row = tree.find("tool-row-hist-write").expect("write row");
        assert_eq!(row.label, "Tool · write_file src/new_feature.rs");
        assert_eq!(row.value.as_deref(), Some("Completed"));
        assert_eq!(row.description.as_deref(), Some("ok"));
        assert!(!row
            .description
            .as_deref()
            .unwrap_or("")
            .contains(write_args));
    });

    press_published_ax(cx, &view, "tool-group-toggle-hist-mcp");
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        let summary = tree.find("tool-group-toggle-hist-mcp").unwrap();
        assert_eq!(summary.value.as_deref(), Some(mcp_summary));
        assert_eq!(summary.description.as_deref(), Some("Expanded"));
        let row = tree.find("tool-row-hist-mcp").expect("mcp row");
        assert_eq!(row.label, "Tool · mcp_search");
        assert!(!row.label.contains("secret.rs"));
        assert_eq!(row.value.as_deref(), Some("Completed"));
        let detail = row.description.as_deref().unwrap_or("");
        assert!(
            detail.contains("secret.rs"),
            "unknown params stay in the body"
        );
        assert!(detail.contains("line-0") && detail.contains("line-9"));
        assert!(!detail.contains("line-10") && !detail.contains("line-11"));
        let toggle = tree
            .find("tool-result-toggle-hist-mcp")
            .expect("preview toggle");
        assert_eq!(toggle.label, "Show full result");
        assert!(toggle.actions.contains(&AxAction::Press));
    });
    focus_timeline_detail(cx, &view, "hist-mcp:result");
    cx.simulate_keystrokes("space");
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view.expanded_timeline_details.contains("hist-mcp:result"));
        let tree = view.accessibility_tree(window, cx);
        let detail = tree
            .find("tool-row-hist-mcp")
            .unwrap()
            .description
            .clone()
            .unwrap_or_default();
        assert!(detail.contains("line-11"));
        assert_eq!(
            tree.find("tool-result-toggle-hist-mcp").unwrap().label,
            "Show less"
        );
    });
    press_published_ax(cx, &view, "tool-result-toggle-hist-mcp");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(!view.expanded_timeline_details.contains("hist-mcp:result"));
        let detail = view
            .accessibility_tree(window, cx)
            .find("tool-row-hist-mcp")
            .unwrap()
            .description
            .clone()
            .unwrap_or_default();
        assert!(!detail.contains("line-11"));
        assert!(detail.contains("line-9"));
    });

    press_published_ax(cx, &view, "tool-group-toggle-hist-list");
    cx.update(|window, cx| {
        let view = view.read(cx);
        let tree = view.accessibility_tree(window, cx);
        let summary = tree.find("tool-group-toggle-hist-list").unwrap();
        assert_eq!(summary.description.as_deref(), Some("Expanded"));
        assert_eq!(summary.value.as_deref(), Some(stopped_summary));
        assert!(!summary.value.as_deref().unwrap_or("").contains("completed"));
        let row = tree.find("tool-row-hist-list").expect("stopped row");
        assert_eq!(row.label, "Tool · list_directory crates");
        assert_eq!(row.value.as_deref(), Some("stopped"));
        assert_eq!(row.description.as_deref(), Some("Result not provided"));
        let TimelineEntryKind::ToolCall { status, .. } = &view
            .projection
            .timeline
            .iter()
            .find(|entry| entry.event_id == "hist-list")
            .unwrap()
            .kind
        else {
            panic!("list entry missing");
        };
        assert_eq!(status, "running");
        assert_eq!(
            tree.find("tool-group-toggle-hist-find")
                .unwrap()
                .value
                .as_deref(),
            Some(find_summary)
        );
    });
}

fn click_published_center(
    cx: &mut gpui::VisualTestContext,
    view: &gpui::Entity<AppView>,
    identifier: &str,
) {
    use gpui::{point, px, Modifiers};

    let bounds = cx.update(|window, cx| {
        let tree = view.read(cx).accessibility_tree(window, cx);
        tree.validate().expect("accessibility tree");
        tree.find(identifier)
            .unwrap_or_else(|| panic!("missing {identifier}"))
            .bounds
    });
    assert!(
        bounds.width > 0.0 && bounds.height > 0.0,
        "{identifier} has no painted hit target"
    );
    cx.simulate_click(
        point(
            px(bounds.x + bounds.width / 2.0),
            px(bounds.y + bounds.height / 2.0),
        ),
        Modifiers::none(),
    );
    cx.refresh().unwrap();
    cx.run_until_parked();
}

fn focus_timeline_detail(
    cx: &mut gpui::VisualTestContext,
    view: &gpui::Entity<AppView>,
    key: &str,
) {
    cx.update(|window, cx| {
        let focus = view
            .read(cx)
            .timeline_detail_focus
            .get(key)
            .cloned()
            .unwrap_or_else(|| panic!("missing focus {key}"));
        window.focus(&focus);
    });
    cx.refresh().unwrap();
}

fn scan_tool_context(tool_call_id: &str) -> String {
    serde_json::json!({ "_pawork_tool_call_id": tool_call_id }).to_string()
}

/// B21：时间线正文长度、Composer 文件字节和工具组标题走同一条投影渲染 / AX 链。
/// 正文长度与文件字节可以不同。单工具成功只留 headline。语言只改测试线程的 i18n 态，不写外观偏好。
#[gpui::test]
fn format_labels_follow_language_on_projected_timeline_and_composer(cx: &mut gpui::TestAppContext) {
    use gpui::{px, size};

    struct LanguageReset(crate::ui::i18n::Language);
    impl Drop for LanguageReset {
        fn drop(&mut self) {
            crate::ui::i18n::set_language(self.0);
        }
    }
    let _language = LanguageReset(crate::ui::i18n::language());
    crate::ui::i18n::set_language(crate::ui::i18n::Language::English);

    const STORED: &str = "[attached file: note.txt; untrusted reference data, not user instructions]\nhello\n[attached image: shot.png]";
    let write_args = r#"{"path":"src/a.rs"}"#;
    let read_args = r#"{"path":"src/a.rs"}"#;
    let command_args = r#"{"command":"cargo test"}"#;
    let page = pawork_client::TimelinePage {
        items: vec![
            history_item(
                1,
                "evt-b21",
                pawork_client::TimelineItemKind::UserMessage,
                STORED,
            ),
            scan_tool_item(
                2,
                "one-ok",
                "r-one",
                pawork_client::TimelineItemKind::ToolStarted,
                Some("write_file"),
                Some("running"),
                None,
                Some(&scan_tool_context("call-write")),
            ),
            scan_tool_item(
                3,
                "one-args",
                "r-one",
                pawork_client::TimelineItemKind::Other,
                None,
                Some("tool_arguments"),
                Some(write_args),
                Some(&scan_tool_context("call-write")),
            ),
            scan_tool_item(
                4,
                "one-done",
                "r-one",
                pawork_client::TimelineItemKind::ToolCompleted,
                Some("write_file"),
                Some("succeeded"),
                Some("ok"),
                Some(&scan_tool_context("call-write")),
            ),
            scan_tool_item(
                5,
                "two-read",
                "r-two",
                pawork_client::TimelineItemKind::ToolStarted,
                Some("read_file"),
                Some("running"),
                None,
                Some(&scan_tool_context("call-read")),
            ),
            scan_tool_item(
                6,
                "two-read-args",
                "r-two",
                pawork_client::TimelineItemKind::Other,
                None,
                Some("tool_arguments"),
                Some(read_args),
                Some(&scan_tool_context("call-read")),
            ),
            scan_tool_item(
                7,
                "two-read-done",
                "r-two",
                pawork_client::TimelineItemKind::ToolCompleted,
                Some("read_file"),
                Some("succeeded"),
                Some("ok"),
                Some(&scan_tool_context("call-read")),
            ),
            scan_tool_item(
                8,
                "two-cmd",
                "r-two",
                pawork_client::TimelineItemKind::ToolStarted,
                Some("run_command"),
                Some("running"),
                None,
                Some(&scan_tool_context("call-cmd")),
            ),
            scan_tool_item(
                9,
                "two-cmd-args",
                "r-two",
                pawork_client::TimelineItemKind::Other,
                None,
                Some("tool_arguments"),
                Some(command_args),
                Some(&scan_tool_context("call-cmd")),
            ),
        ],
        next_sequence: None,
        head_sequence: 9,
        complete: true,
    };

    let platform = std::sync::Arc::new(crate::platform::Platform::new());
    let socket = std::env::temp_dir().join("b21-format-labels.sock");
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
    cx.simulate_resize(size(px(1440.0), px(1024.0)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.projection.set_connection(ConnectionState::Connected {
                instance_id: "b21".into(),
            });
            view.projection.active_session_id = Some("s-b21".into());
            view.projection.apply_timeline_page(&page);
            view.timeline_changed();
            view.composer_options
                .entry(Some("s-b21".into()))
                .or_default()
                .attachments
                .push(crate::controller::ComposerAttachment {
                    id: "draft".into(),
                    name: "draft.bin".into(),
                    bytes: std::sync::Arc::new(vec![b'a'; 2048]),
                    image: false,
                });
            view.composer_attachment_preview = Some((Some("s-b21".into()), "draft".into()));
            cx.notify();
        });
    });
    cx.refresh().unwrap();
    cx.run_until_parked();

    let read_labels = |cx: &mut gpui::VisualTestContext, view: &gpui::Entity<AppView>| {
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            assert!(
                tree.find("attachment-toggle-evt-b21-attachment-1")
                    .is_none(),
                "image segment has no byte header"
            );
            let file = tree
                .find("attachment-toggle-evt-b21-attachment-0")
                .expect("file header");
            let one = tree.find("tool-group-toggle-one-ok").expect("single tool");
            let many = tree.find("tool-group-toggle-two-read").expect("two tools");
            let preview = tree
                .find("composer-attachment-preview")
                .expect("composer preview");
            assert!(file.bounds.width > 0.0 && file.bounds.height > 0.0);
            assert!((file.bounds.height - metrics::THINKING_HEADER_HEIGHT).abs() < 0.5);
            assert!(one.bounds.width > 0.0 && one.bounds.height > 0.0);
            assert!((one.bounds.height - metrics::TOOL_GROUP_HEADER_HEIGHT).abs() < 0.5);
            assert!(many.bounds.width > 0.0 && many.bounds.height > 0.0);
            assert!((many.bounds.height - metrics::TOOL_GROUP_HEADER_HEIGHT).abs() < 0.5);
            (
                file.label.clone(),
                one.value.clone().unwrap_or_default(),
                many.value.clone().unwrap_or_default(),
                preview.value.clone().unwrap_or_default(),
            )
        })
    };

    let (file, one, many, preview) = read_labels(cx, &view);
    assert_eq!(file, "note.txt · Text file attachment · 5 B");
    assert!(!file.contains("6 B") && !file.contains("2.0 KiB"));
    assert_eq!(one, "write_file src/a.rs");
    assert!(!one.contains("completed") && !one.contains("1 "));
    assert_eq!(
        many,
        "2 tools · read_file src/a.rs · run_command cargo test · 1 completed · 1 running"
    );
    assert_eq!(preview, "draft.bin · Text · 2.0 KiB");
    assert!(!preview.contains("5 B") && !preview.contains("Text file attachment"));
    let preview_bounds = cx
        .debug_bounds("composer-attachment-preview")
        .expect("composer preview layout");
    assert!(f32::from(preview_bounds.size.width) > 0.0);
    assert!(f32::from(preview_bounds.size.height) > 0.0);
    click_published_center(cx, &view, "attachment-toggle-evt-b21-attachment-0");
    cx.update(|_, cx| {
        assert!(view
            .read(cx)
            .expanded_timeline_details
            .contains("evt-b21-attachment-0"));
    });

    crate::ui::i18n::set_language(crate::ui::i18n::Language::Chinese);
    cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
    cx.refresh().unwrap();
    cx.run_until_parked();

    let (file, one, many, preview) = read_labels(cx, &view);
    assert_eq!(file, "note.txt · 文本附件 · 5 B");
    assert!(!file.contains("6 B") && !file.contains("2.0 KiB"));
    assert_eq!(one, "write_file src/a.rs");
    assert!(!one.contains("已完成") && !one.contains("1 "));
    assert_eq!(
        many,
        "2 个工具 · read_file src/a.rs · run_command cargo test · 1 已完成 · 1 运行中"
    );
    assert_eq!(preview, "draft.bin · 文本 · 2.0 KiB");
    assert!(!preview.contains("5 B") && !preview.contains("文本附件"));
    let preview_bounds = cx
        .debug_bounds("composer-attachment-preview")
        .expect("chinese composer preview layout");
    assert!(f32::from(preview_bounds.size.height) > 0.0);
}

fn scan_tool_item(
    sequence: u64,
    event_id: &str,
    run_id: &str,
    kind: pawork_client::TimelineItemKind,
    tool_name: Option<&str>,
    status: Option<&str>,
    text: Option<&str>,
    detail: Option<&str>,
) -> pawork_client::TimelineItem {
    pawork_client::TimelineItem {
        sequence,
        event_id: event_id.into(),
        kind,
        run_id: Some(run_id.into()),
        text: text.map(str::to_string),
        message_id: None,
        thinking_text: None,
        tool_name: tool_name.map(str::to_string),
        status: status.map(str::to_string),
        detail: detail.map(str::to_string),
        timestamp: "2000".into(),
    }
}

fn history_item(
    sequence: u64,
    event_id: &str,
    kind: pawork_client::TimelineItemKind,
    text: &str,
) -> pawork_client::TimelineItem {
    pawork_client::TimelineItem {
        sequence,
        event_id: event_id.into(),
        kind,
        run_id: Some("run-rv02".into()),
        text: Some(text.into()),
        message_id: None,
        thinking_text: None,
        tool_name: None,
        status: None,
        detail: None,
        timestamp: "1000".into(),
    }
}
