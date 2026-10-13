//! Composer AX 树。

use super::*;

impl AppView {
    pub(super) fn composer_input_ax_max() -> f32 {
        (metrics::COMPOSER_PANEL_MAX_HEIGHT
            - metrics::COMPOSER_BORDER
            - metrics::COMPOSER_PAD * 2.0
            - metrics::COMPOSER_GAP
            - metrics::COMPOSER_SEND_SIZE)
            .max(metrics::COMPOSER_INPUT_MIN_HEIGHT)
    }

    pub(crate) fn model_search_ax(&self, window: &Window, menu_id: &str) -> AxNode {
        let input = self.settings_menu_element_bounds("model-search-input", menu_id);
        let clear = self.settings_menu_element_bounds("model-search-clear", menu_id);
        let mut clear_node = AxNode::new(
            "model-search-clear",
            AxRole::Button,
            t("model_search.clear"),
            clear,
        )
        .enabled(!self.model_search_query.is_empty())
        .focused(
            self.settings_action_focus
                .get("model-search-clear")
                .is_some_and(|f| f.is_focused(window)),
        );
        if !self.model_search_query.is_empty() {
            clear_node = clear_node.action(AxAction::Press);
        }
        let placeholder = if matches!(self.open_menu, Some(MenuKind::Model)) {
            t("model_search.placeholder_providers")
        } else {
            t("model_search.placeholder")
        };
        AxNode::new(
            "model-search",
            AxRole::Group,
            placeholder,
            AxRect::new(
                input.x,
                input.y,
                (clear.x + clear.width - input.x).max(0.0),
                input.height.max(clear.height),
            ),
        )
        .child(
            AxNode::new("model-search-input", AxRole::TextArea, placeholder, input)
                .value(&self.model_search_query)
                .focused(self.model_search_focus.is_focused(window))
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
        )
        .child(clear_node)
    }

    pub(super) fn composer_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let width = (frame.width - metrics::COMPOSER_OUTER_X * 2.0)
            .max(0.0)
            .min(metrics::TIMELINE_READABLE_WIDTH);
        let column_x = frame.x + (frame.width - width) / 2.0;
        let meta_height = Self::composer_meta_height(window);
        let card_height = frame.height
            - metrics::COMPOSER_OUTER_TOP
            - metrics::COMPOSER_OUTER_BOTTOM
            - metrics::COMPOSER_META_GAP
            - self.composer_meta_layout_height(window)
            - (metrics::COMPOSER_META_GAP + meta_height) * self.composer_notes().len() as f32;
        let card = AxRect::new(
            column_x,
            frame.y + metrics::COMPOSER_OUTER_TOP,
            width,
            card_height,
        );
        let actual_card = self.composer_layouts["composer-card"].bounds();
        let card = if actual_card.size.height > gpui::px(0.0) {
            AxRect::new(
                actual_card.origin.x.into(),
                actual_card.origin.y.into(),
                actual_card.size.width.into(),
                actual_card.size.height.into(),
            )
        } else {
            card
        };
        let pad = metrics::COMPOSER_PAD + metrics::COMPOSER_BORDER / 2.0;
        let input_y = card.y + pad + self.composer_options_height();
        let footer_y = card.y + card.height - pad - metrics::COMPOSER_SEND_SIZE;
        let input_height = (footer_y - metrics::COMPOSER_GAP - input_y).max(0.0);
        let action_x = card.x + card.width - pad - metrics::COMPOSER_SEND_SIZE;
        let input_width = (card.width - pad * 2.0).max(0.0);
        let meta_y = card.y + card.height + metrics::COMPOSER_META_GAP;
        let meta_width = (width - metrics::SPACE_2) / 2.0;
        let running = self.projection.active_run_id.is_some();
        let current_model = self.model_label();
        let input_focus = self.text_input.read(cx).focus_handle(cx);
        // AXValue 恒为纯文本：空输入即空串，placeholder 不得回退进 value
        // （R4 U2 composer-cleared / R5 r5-1 契约）。
        let input_value = self.text_input.read(cx).text().to_string();
        let mut composer = AxNode::new("composer", AxRole::Group, "Composer", frame)
            .child(
                AxNode::new(
                    "composer-attach",
                    AxRole::Button,
                    t("composer.add"),
                    self.settings_menu_element_bounds("composer-attach", "composer-attach"),
                )
                .value(if matches!(self.open_menu, Some(MenuKind::ComposerAdd)) {
                    "Expanded"
                } else {
                    "Collapsed"
                })
                .description(t("composer.add"))
                .focused(
                    self.open_menu.is_none()
                        && self
                            .settings_action_focus
                            .get("composer-attach")
                            .is_some_and(|focus| focus.is_focused(window)),
                )
                .action(AxAction::Press),
            )
            .child(
                AxNode::new(
                    "composer-input",
                    AxRole::TextArea,
                    t("composer.message"),
                    AxRect::new(card.x + pad, input_y, input_width, input_height),
                )
                .value(input_value)
                .focused(self.open_menu.is_none() && input_focus.is_focused(window))
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
            )
            .child(
                AxNode::new(
                    "model-picker",
                    AxRole::Button,
                    t("composer.model"),
                    AxRect::new(
                        card.x + pad,
                        footer_y,
                        metrics::COMPOSER_MODEL_WIDTH,
                        metrics::COMPOSER_FOOTER_CONTROL,
                    ),
                )
                .value(current_model)
                .enabled(self.can_open_model_menu())
                // R7 Wave A：model 菜单打开时 AX 焦点移交高亮项（同 grouping）。
                .focused(self.open_menu.is_none() && self.model_focus.is_focused(window))
                .action(AxAction::Press),
            )
            // ADR-063：推理强度 chip；初始几何随后被实测布局循环校正
            // （composer_layouts["effort-picker"]）。
            .child(
                AxNode::new(
                    "effort-picker",
                    AxRole::Button,
                    t("composer.effort"),
                    AxRect::new(
                        card.x + pad + metrics::COMPOSER_MODEL_WIDTH + metrics::SPACE_2,
                        footer_y,
                        metrics::COMPOSER_MODEL_WIDTH,
                        metrics::COMPOSER_FOOTER_CONTROL,
                    ),
                )
                .value(self.effort_chip_label())
                .enabled(self.can_open_effort_menu())
                .focused(self.open_menu.is_none() && self.effort_focus.is_focused(window))
                .action(AxAction::Press),
            )
            // OPT-D：项目上下文槽（无项目 chip / Workspace · 名称）与可选
            // ContextMeter；无项目正向入口与 render 同源，不画警告色限制。
            .child(
                AxNode::new(
                    "composer-workspace",
                    AxRole::StaticText,
                    "Workspace context",
                    AxRect::new(column_x, meta_y, meta_width, meta_height),
                )
                .value(if self.composer_workspace_no_project() {
                    t("composer.no_project_chip").to_string()
                } else {
                    self.composer_workspace_label()
                }),
            );
        let options = self.current_composer_options();
        for attachment in options.attachments {
            let id = format!("composer-remove-{}", attachment.id);
            // RV-01：预览开/关按钮与移除按钮为独立可 Press 节点。
            let preview_id = format!("composer-preview-{}", attachment.id);
            composer = composer.child(
                AxNode::new(
                    preview_id.clone(),
                    AxRole::Button,
                    t("composer.preview_attachment"),
                    self.settings_menu_element_bounds(&preview_id, "composer-options"),
                )
                .value(attachment.name.clone())
                .action(AxAction::Press),
            );
            composer = composer.child(
                AxNode::new(
                    id.clone(),
                    AxRole::Button,
                    format!("{} · {}", t("composer.remove_attachment"), attachment.name),
                    self.settings_menu_element_bounds(&id, "composer-options"),
                )
                .enabled(!self.composer_sending)
                .action(AxAction::Press),
            );
        }
        // RV-03：附件错误文案进 AX（与可见错误行同源）。
        if let Some(error) = options.attachment_error.as_ref() {
            composer = composer.child(
                AxNode::new(
                    "composer-attach-error",
                    AxRole::StaticText,
                    "Attachment error",
                    self.settings_menu_element_bounds("composer-options", "composer-options"),
                )
                .value(self.composer_attachment_error_text(error)),
            );
        }
        if let Some(error) = self.current_composer_send_error() {
            composer = composer.child(
                AxNode::new(
                    "composer-send-error",
                    AxRole::StaticText,
                    t("composer.send_failed"),
                    self.settings_menu_element_bounds("composer-send-error", "composer-options"),
                )
                .value(error),
            );
        }
        if let Some(attachment) = self.composer_attachment_preview_target() {
            composer = composer.child(
                AxNode::new(
                    "composer-preview-close",
                    AxRole::Button,
                    t("composer.preview_close"),
                    self.settings_menu_element_bounds(
                        "composer-preview-close",
                        "composer-attachment-preview",
                    ),
                )
                .action(AxAction::Press),
            );
            composer = composer.child(
                AxNode::new(
                    "composer-attachment-preview",
                    AxRole::StaticText,
                    "Attachment preview",
                    self.settings_menu_element_bounds(
                        "composer-attachment-preview",
                        "composer-options",
                    ),
                )
                .value(format!(
                    "{} · {}",
                    attachment.name,
                    self.composer_attachment_meta_label(&attachment)
                )),
            );
        }
        for (index, video) in options.video_urls.iter().enumerate() {
            let id = format!("composer-video-{index}");
            composer = composer.child(AxNode::new(
                &id,
                AxRole::StaticText,
                format!("{}: {}", t("video.title"), video.url),
                self.settings_menu_element_bounds(&id, "composer-options"),
            ));
        }
        if options.web_search.is_some() {
            composer = composer.child(
                AxNode::new(
                    "composer-search-toggle",
                    AxRole::Button,
                    t(if options.web_search == Some(true) {
                        "composer.search_on"
                    } else {
                        "composer.search_off"
                    }),
                    self.settings_menu_element_bounds("composer-search-toggle", "composer-options"),
                )
                .enabled(!self.composer_sending)
                .action(AxAction::Press),
            );
        }
        if let Some(error) = self.composer_capability_error() {
            composer = composer.child(AxNode::new(
                "composer-capability-hint",
                AxRole::StaticText,
                error,
                self.settings_menu_element_bounds("composer-options", "composer-options"),
            ));
        }
        if self.composer_context_meter_visible() {
            composer = composer.child(
                AxNode::new(
                    "composer-context",
                    AxRole::StaticText,
                    "Context",
                    AxRect::new(
                        column_x + meta_width + metrics::SPACE_2,
                        meta_y,
                        meta_width,
                        meta_height,
                    ),
                )
                .value(self.projection.context_meter_label()),
            );
        }
        if self.composer_project_task_visible() {
            let mut node = AxNode::new(
                "composer-project-task",
                AxRole::Button,
                self.composer_project_task_label(),
                frame,
            )
            .description(self.add_task_disabled_reason())
            .enabled(self.can_create_task())
            .focused(self.open_menu.is_none() && self.project_task_focus.is_focused(window));
            if self.can_create_task() {
                node = node.action(AxAction::Press);
            }
            composer = composer.child(node);
        }
        // 元信息可以换行，AX 只使用同帧实测控件框。
        for node in &mut composer.children {
            if let Some(handle) = self.composer_layouts.get(node.identifier.as_str()) {
                let bounds = handle.bounds();
                node.bounds = AxRect::new(
                    bounds.origin.x.into(),
                    bounds.origin.y.into(),
                    bounds.size.width.into(),
                    bounds.size.height.into(),
                );
            }
        }
        for (index, (id, note)) in self.composer_notes().into_iter().enumerate() {
            composer = composer.child(
                AxNode::new(
                    id,
                    AxRole::StaticText,
                    "Status",
                    AxRect::new(
                        column_x,
                        meta_y
                            + self.composer_meta_layout_height(window)
                            + metrics::COMPOSER_META_GAP
                            + (metrics::COMPOSER_META_GAP + meta_height) * index as f32,
                        width,
                        meta_height,
                    ),
                )
                .value(note),
            );
        }
        let action_rect = AxRect::new(
            action_x,
            footer_y,
            metrics::COMPOSER_SEND_SIZE,
            metrics::COMPOSER_SEND_SIZE,
        );
        if running {
            composer = composer.child(
                AxNode::new("cancel", AxRole::Button, "Cancel run", action_rect)
                    .enabled(self.can_cancel())
                    .focused(
                        self.open_menu.is_none() && self.composer_action_focus.is_focused(window),
                    )
                    .action(AxAction::Press),
            );
        } else {
            composer = composer.child(
                AxNode::new("send", AxRole::Button, t("composer.send"), action_rect)
                    .enabled(self.can_send(cx))
                    .focused(
                        self.open_menu.is_none() && self.composer_action_focus.is_focused(window),
                    )
                    .action(AxAction::Press),
            );
        }
        if matches!(self.open_menu, Some(MenuKind::Model)) {
            let highlight = self.menu_highlight_effective(self.menu_selected_index());
            let bounds = |id: &str| self.settings_menu_element_bounds(id, "model-menu");
            let mut menu = AxNode::new("model-menu", AxRole::Group, "Models", bounds("model-menu"))
                .child(self.model_search_ax(window, "model-menu"));
            for purpose in ModelPurposeFilter::ALL {
                let id = purpose.identifier();
                let focused = self
                    .settings_action_focus
                    .get(&id)
                    .is_some_and(|focus| focus.is_focused(window));
                menu = menu.child(
                    AxNode::new(
                        id.clone(),
                        AxRole::Button,
                        t(purpose.label_key()),
                        bounds(&id),
                    )
                    .selected(self.model_purpose_filter == purpose)
                    .focused(focused)
                    .action(AxAction::Press)
                    .action(AxAction::Focus),
                );
            }
            if self.model_menu_row_count() > 0 {
                menu = menu.child(AxNode::new(
                    "model-capability-help",
                    AxRole::StaticText,
                    t("model_capability.help"),
                    bounds("model-capability-help"),
                ));
            }
            if self.model_menu_row_count() == 0 {
                let (title, hint) = self.model_menu_empty_text();
                menu = menu.child(
                    AxNode::new(
                        "model-menu-empty",
                        AxRole::StaticText,
                        title,
                        bounds("model-menu-empty"),
                    )
                    .value(hint),
                );
            } else {
                let mut ix = 0;
                for (provider_id, models) in self.composer_model_groups() {
                    let group_id = format!("model-menu-group-{provider_id}");
                    if let Some(layout) = self.settings_element_layouts.get(&group_id) {
                        let rect = layout.bounds().intersect(&self.model_menu_scroll.bounds());
                        if rect.size.width > gpui::px(0.0) && rect.size.height > gpui::px(0.0) {
                            menu = menu.child(AxNode::new(
                                group_id,
                                AxRole::StaticText,
                                self.provider_display_name(&provider_id),
                                AxRect::new(
                                    rect.origin.x.into(),
                                    rect.origin.y.into(),
                                    rect.size.width.into(),
                                    rect.size.height.into(),
                                ),
                            ));
                        }
                    }
                    for model in models {
                        let id = format!("model-{}-{}", model.provider_id, model.id);
                        if let Some(layout) = self.settings_element_layouts.get(&id) {
                            let rect = layout.bounds().intersect(&self.model_menu_scroll.bounds());
                            if rect.size.width > gpui::px(0.0) && rect.size.height > gpui::px(0.0) {
                                let selected =
                                    self.projection.effective_model().is_some_and(|current| {
                                        current.0 == model.provider_id && current.1 == model.id
                                    });
                                let title = self.composer_model_row_title(&model);
                                let mut item = AxNode::new(
                                    model_identifier(&model),
                                    AxRole::Button,
                                    title,
                                    AxRect::new(
                                        rect.origin.x.into(),
                                        rect.origin.y.into(),
                                        rect.size.width.into(),
                                        rect.size.height.into(),
                                    ),
                                )
                                .selected(selected)
                                .description(
                                    super::super::super::input_area::model_capability_label(&model),
                                )
                                .focused(ix == highlight)
                                .enabled(self.can_switch_model());
                                if title != model.id {
                                    item = item.value(&model.id);
                                }
                                if self.can_switch_model() {
                                    item = item.action(AxAction::Press);
                                }
                                menu = menu.child(item);
                            }
                        }
                        ix += 1;
                    }
                }
            }
            menu = menu.child(
                AxNode::new(
                    "model-menu-settings",
                    AxRole::Button,
                    t("model_search.manage"),
                    bounds("model-menu-settings"),
                )
                .focused(highlight == self.model_menu_row_count())
                .action(AxAction::Press),
            );
            composer = composer.child(menu);
        }
        if matches!(self.open_menu, Some(MenuKind::ComposerAdd)) {
            let bounds = |id: &str| self.settings_menu_element_bounds(id, "composer-add-menu");
            let mut menu = AxNode::new(
                "composer-add-menu",
                AxRole::Group,
                t("composer.add"),
                bounds("composer-add-menu"),
            );
            for (ix, action) in super::super::super::input_area::ComposerAction::ALL
                .into_iter()
                .enumerate()
            {
                let rect = bounds(action.id());
                if rect.width <= 0.0 || rect.height <= 0.0 {
                    continue;
                }
                let enabled = self.composer_action_enabled(action);
                let mut row = AxNode::new(action.id(), AxRole::Button, action.label(), rect)
                    .description(self.composer_action_hint(action))
                    .enabled(enabled)
                    .focused(
                        enabled && ix == self.menu_highlight_effective(self.menu_selected_index()),
                    );
                if enabled {
                    row = row.action(AxAction::Press);
                }
                menu = menu.child(row);
            }
            composer = composer.child(menu);
        }
        // ADR-063：强度菜单 AX 与 render 同源（effort_menu_options），
        // 几何取 MenuPanel 实测布局（settings_element_layouts）。
        if matches!(self.open_menu, Some(MenuKind::Effort)) {
            let highlight = self.menu_highlight_effective(self.menu_selected_index());
            let bounds = |id: &str| self.settings_menu_element_bounds(id, "effort-menu");
            let mut menu = AxNode::new(
                "effort-menu",
                AxRole::Group,
                t("composer.effort"),
                bounds("effort-menu"),
            );
            let current = self.projection.effective_effort().map(str::to_string);
            let enabled = self.can_open_effort_menu();
            for (ix, option) in self.effort_menu_options().iter().enumerate() {
                let row_id = format!("effort-{}", option.as_deref().unwrap_or("auto"));
                let rect = bounds(&row_id);
                if rect.width <= 0.0 || rect.height <= 0.0 {
                    continue;
                }
                let label = match option {
                    Some(level) => level.clone(),
                    None => t("composer.effort_auto").to_string(),
                };
                let mut item = AxNode::new(row_id, AxRole::Button, label, rect)
                    .selected(*option == current)
                    .focused(ix == highlight)
                    .enabled(enabled);
                if enabled {
                    item = item.action(AxAction::Press);
                }
                menu = menu.child(item);
            }
            composer = composer.child(menu);
        }
        composer
    }
}
