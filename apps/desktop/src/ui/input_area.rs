//! UI-4：居中输入卡片（草稿 + 模型 / 发送），外围保留留白。
//! 项目、ContextMeter 与瞬态提示在卡片下方，避免争抢动作行。
//! Send 与 Cancel 同槽互换（element id `composer-action`）；placeholder 只走状态机，
//! Forked / 发送失败等瞬态反馈落 StatusBar 右栏。

use gpui::{
    div, point, prelude::*, px, Context, Corner, Pixels, Point, SharedString, TextRun, Window,
};

use crate::projection::{find_model_entry, ConnectionState, ModelEntry, ProviderAuthStatusEntry};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, ANCHOR_GAP_Y};
use crate::ui::components::icon::{icon, icon_sized, Icon};
use crate::ui::components::label::Label;
use crate::ui::i18n::{t, t2};
use crate::ui::settings::settings_role_candidates;
use crate::ui::theme::{dark, font, metrics};

use super::{AppView, InspectorTab, MenuKind};

/// Stable actions shared by the visual menu, keyboard navigation and AX.
#[derive(Clone, Copy)]
pub(super) enum ComposerAction {
    Files,
    Image,
    ProjectFiles,
    Search,
    Project,
    Browser,
    Resources,
}

impl ComposerAction {
    pub(super) const ALL: [Self; 7] = [
        Self::Files,
        Self::Image,
        Self::ProjectFiles,
        Self::Search,
        Self::Project,
        Self::Browser,
        Self::Resources,
    ];

    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Files => "composer-add-files",
            Self::Image => "composer-add-image",
            Self::ProjectFiles => "composer-add-project-files",
            Self::Search => "composer-add-search",
            Self::Project => "composer-add-project",
            Self::Browser => "composer-add-browser",
            Self::Resources => "composer-add-resources",
        }
    }

    pub(super) fn label(self) -> &'static str {
        t(match self {
            Self::Files => "composer.add_files",
            Self::Image => "files.attach_image",
            Self::ProjectFiles => "composer.project_files",
            Self::Search => "composer.add_search",
            Self::Project => "composer.project_task",
            Self::Browser => "inspector.tab_browser",
            Self::Resources => "inspector.tab_resources",
        })
    }

    fn icon(self) -> Icon {
        match self {
            Self::Files => Icon::File,
            Self::Image => Icon::Link,
            Self::ProjectFiles => Icon::File,
            Self::Search => Icon::Search,
            Self::Project => Icon::Project,
            Self::Browser => Icon::Network,
            Self::Resources => Icon::Resources,
        }
    }
}

/// Composer 伪二级目录：已连接且至少有一个已启用模型的供应商，组头 + 组内
/// 模型同一列表展开。未连接或 0 启用整组不出现。
pub(super) fn composer_model_menu_groups(
    models: &[ModelEntry],
    providers: &[ProviderAuthStatusEntry],
    query: &str,
) -> Vec<(String, Vec<ModelEntry>)> {
    let query = query.trim().to_lowercase();
    settings_role_candidates(models, providers)
        .into_iter()
        .filter_map(|(provider, models)| {
            let models: Vec<_> = models
                .into_iter()
                .filter(|model| {
                    if query == t("model_capability.search").to_lowercase() {
                        return model.web_search;
                    }
                    query.is_empty()
                        || provider.to_lowercase().contains(&query)
                        || model.display_name.to_lowercase().contains(&query)
                        || model.id.to_lowercase().contains(&query)
                        || model_capability_label(model)
                            .to_lowercase()
                            .contains(&query)
                })
                .collect();
            (!models.is_empty()).then_some((provider, models))
        })
        .collect()
}

/// 模型触发器宽 = 名称 + 箭头 + 内边距，钳在命中区与 220 上限之间。
/// Taffy 会把仅有 max_width 的 auto 行撑满上限，必须给明确内容宽。
fn composer_model_chip_width(window: &Window, label: &str) -> f32 {
    let style = window.text_style();
    let text = if label.is_empty() { " " } else { label };
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color: style.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let text_w = f32::from(
        window
            .text_system()
            .shape_line(
                SharedString::from(text.to_string()),
                font::BASE.to_pixels(window.rem_size()),
                &[run],
                None,
            )
            .width,
    );
    let pad_x = f32::from(window.rem_size()) * 0.5 * 2.0;
    (text_w + pad_x + metrics::SPACE_2 + metrics::ICON_SM)
        .ceil()
        .clamp(
            metrics::COMPOSER_FOOTER_CONTROL,
            metrics::COMPOSER_MODEL_WIDTH,
        )
}

/// Composer 模型行只画一行：`display_name`，空则回落 id。raw id 不另占行。
pub(super) fn model_menu_row_title<'a>(display_name: &'a str, id: &'a str) -> &'a str {
    if display_name.is_empty() {
        id
    } else {
        display_name
    }
}

pub(super) fn model_capability_label(model: &ModelEntry) -> String {
    [
        (model.image_input, t("model_capability.image")),
        (model.web_search, t("model_capability.search")),
    ]
    .into_iter()
    .filter_map(|(supported, label)| supported.then_some(label))
    .collect::<Vec<_>>()
    .join(" · ")
}

/// 可点击模型的扁平顺序（组头不计入）；鼠标、键盘与 AX 同源。
pub(super) fn grouped_model_menu_entries(
    models: &[ModelEntry],
    providers: &[ProviderAuthStatusEntry],
    query: &str,
) -> Vec<ModelEntry> {
    composer_model_menu_groups(models, providers, query)
        .into_iter()
        .flat_map(|(_, models)| models)
        .collect()
}

/// 「已连接且 model_list 查询已完成但结果为空」：唯一允许显示「无已启用
/// 模型」空态的判定（ADR-055 D4 缺省过滤后 models 即 Host 启用集）。
/// 加载中（查询未完成）与断线不得误报为全部禁用。
pub(super) fn model_catalog_empty_state(
    connected: bool,
    models_loaded: bool,
    models_empty: bool,
) -> bool {
    connected && models_loaded && models_empty
}

impl AppView {
    pub(super) fn on_composer_add_menu(
        &mut self,
        down: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        self.toggle_menu(MenuKind::ComposerAdd, down, cx);
    }

    pub(super) fn composer_action_enabled(&self, action: ComposerAction) -> bool {
        match action {
            ComposerAction::Files | ComposerAction::Image => {
                !self.composer_loading && !self.composer_sending
            }
            ComposerAction::ProjectFiles => self.can_attach_image() && !self.composer_sending,
            ComposerAction::Search => !self.composer_sending,
            ComposerAction::Project => self.can_create_task(),
            ComposerAction::Browser => true,
            ComposerAction::Resources => matches!(
                self.projection.connection,
                ConnectionState::Connected { .. }
            ),
        }
    }

    pub(super) fn composer_action_hint(&self, action: ComposerAction) -> &'static str {
        match action {
            ComposerAction::Files | ComposerAction::Image => t("composer.local_files"),
            ComposerAction::Search => t("composer.search_hint"),
            _ => "",
        }
    }

    pub(super) fn activate_composer_action(
        &mut self,
        action: ComposerAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.composer_action_enabled(action) {
            return;
        }
        self.close_open_menu(cx);
        match action {
            ComposerAction::ProjectFiles => self.on_attach_image(window, cx),
            ComposerAction::Files => self.pick_composer_attachments(false, window, cx),
            ComposerAction::Image => self.pick_composer_attachments(true, window, cx),
            ComposerAction::Search => self.toggle_composer_search(cx),
            ComposerAction::Project => self.on_project_task_menu(None, window, cx),
            ComposerAction::Browser | ComposerAction::Resources => {
                let tab = if matches!(action, ComposerAction::Browser) {
                    InspectorTab::Browser
                } else {
                    InspectorTab::Resources
                };
                self.select_inspector_tab(tab, cx);
                if !self.inspector_open {
                    self.on_toggle_inspector(window, cx);
                }
            }
        }
        cx.notify();
    }

    fn composer_add_menu_element(&mut self, cx: &mut Context<Self>) -> MenuPanel {
        let scroll = self
            .settings_element_layouts
            .entry("composer-add-menu".into())
            .or_default()
            .clone();
        let highlight = self.menu_highlight_effective(self.menu_selected_index());
        let mut list = div().w(px(320.0)).flex().flex_col();
        for (ix, action) in ComposerAction::ALL.into_iter().enumerate() {
            let enabled = self.composer_action_enabled(action);
            let hint = self.composer_action_hint(action);
            let mut row = self
                .settings_element(action.id())
                .w_full()
                .px_2()
                .py_2()
                .rounded(px(metrics::CONTROL_RADIUS))
                .bg(if enabled && ix == highlight {
                    dark().surface.raised
                } else {
                    dark().bg.menu
                })
                .text_color(if enabled {
                    dark().text.primary
                } else {
                    dark().text.ghost
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon_sized(action.icon(), px(metrics::ICON_SM)))
                        .child(action.label()),
                )
                .when(!hint.is_empty(), |row| {
                    row.child(
                        div()
                            .text_size(font::XS)
                            .text_color(dark().text.secondary)
                            .whitespace_normal()
                            .child(hint),
                    )
                });
            if enabled {
                row = row
                    .cursor_pointer()
                    .hover(|style| style.bg(dark().surface.hover))
                    .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                        if *hovered {
                            view.menu_highlight = Some(ix);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.activate_composer_action(action, window, cx)
                    }));
            }
            list = list.child(row);
        }
        MenuPanel::new("composer-add-menu")
            .track_scroll(&scroll)
            .max_height(400.0)
            .dismiss_on_outside(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::ComposerAdd, event.position, cx)
            }))
            .child(list)
    }

    pub(super) fn can_attach_image(&self) -> bool {
        self.projection.active_session_id.is_some()
            && self.projection.active_workspace_id().is_some()
            && matches!(
                self.projection.connection,
                ConnectionState::Connected { .. }
            )
    }

    pub(super) fn composer_element(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let can_send = self.can_send(cx);
        let can_cancel = self.can_cancel();
        let can_switch_model = self.can_switch_model();
        let can_open_model_menu = self.can_open_model_menu();
        let model_menu_open =
            matches!(self.open_menu, Some(MenuKind::Model)) && can_open_model_menu;
        let composer_hint = self.composer_placeholder_hint();
        let context_visible = self.composer_context_meter_visible();
        let workspace_label = if self.composer_workspace_no_project() {
            t("composer.no_project_chip").to_string()
        } else {
            self.composer_workspace_label()
        };
        let meta_height = Self::composer_meta_height(window);
        let attachment_focus = self
            .settings_action_focus
            .entry("composer-attach".into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let attachment = Button::new("composer-attach")
            .track_focus(&attachment_focus)
            .variant(ButtonVariant::Ghost)
            .height(px(metrics::COMPOSER_FOOTER_CONTROL))
            .width(px(metrics::COMPOSER_FOOTER_CONTROL))
            .child(icon_sized(Icon::Plus, px(metrics::ICON_SM)))
            .tooltip(t("composer.add"))
            .on_click(cx.listener(|view, event, _window, cx| {
                if !view.consume_button_key_click("composer-attach", event) {
                    view.on_composer_add_menu(Self::click_down_position(event), cx);
                }
            }))
            .on_activate(cx.listener(|view, _, _window, cx| {
                view.note_button_key_activate("composer-attach");
                if view.open_menu.is_some() {
                    return;
                }
                view.on_composer_add_menu(None, cx);
                cx.stop_propagation();
            }));
        let mut attachment = Dropdown::new(attachment)
            .panel_anchor(Corner::BottomLeft, point(px(0.0), px(-ANCHOR_GAP_Y)));
        if matches!(self.open_menu, Some(MenuKind::ComposerAdd)) {
            attachment = attachment.panel(self.composer_add_menu_element(cx));
        }
        let attachment = self
            .settings_element("composer-attach")
            .flex_none()
            .child(attachment);
        let input_focused = self.composer_focus_handle(cx).is_focused(window);
        self.sync_composer_placeholder(composer_hint.clone(), cx);

        let model_tooltip = if can_switch_model {
            SharedString::from(
                self.projection
                    .effective_model()
                    .map(|(provider, id)| t2("composer.model_tooltip", provider, id))
                    .unwrap_or_else(|| t("composer.model_tooltip_none").to_string()),
            )
        } else {
            SharedString::from(self.model_disabled_reason())
        };
        let model_focus = self.model_focus.clone();
        let model_label = self.model_label();
        let model_chip_width = composer_model_chip_width(window, &model_label);
        let mut model_button = Button::new("model-picker")
            .track_focus(&model_focus)
            .variant(ButtonVariant::Raised)
            .text_color(dark().text.primary)
            .disabled(!can_open_model_menu)
            .child(
                div()
                    .flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(px(metrics::SPACE_2))
                    .child(div().truncate().child(model_label))
                    .child(
                        div()
                            .flex_none()
                            .child(icon_sized(Icon::ChevronDown, px(metrics::ICON_SM))),
                    ),
            )
            .tooltip(model_tooltip)
            .height(px(metrics::COMPOSER_FOOTER_CONTROL))
            .width(px(model_chip_width))
            .max_width(px(metrics::COMPOSER_MODEL_WIDTH))
            .vcenter();
        // 全关空态下按钮保持可点：菜单从触发器上方打开给诚实说明行，
        // 但不提供任何可选项（选择路径仍由 can_switch_model fail-closed）。
        if can_open_model_menu {
            model_button = model_button
                .on_click(cx.listener(|view, event, window, cx| {
                    if view.consume_button_key_click("model-picker", event) {
                        return;
                    }
                    let down = Self::click_down_position(event);
                    view.on_toggle_model_menu(down, window, cx);
                }))
                .on_activate(cx.listener(|view, _event, window, cx| {
                    if view.open_menu.is_some() {
                        // 菜单已开时让位给 root 的菜单 Enter 处理，并重新
                        // 武装 keyup 合成 click 吞除标记。
                        view.note_button_key_activate("model-picker");
                        return;
                    }
                    view.note_button_key_activate("model-picker");
                    view.on_toggle_model_menu(None, window, cx);
                    cx.stop_propagation();
                }));
        }
        // Composer 紧邻窗口底部，model menu 明确从触发器上方打开；anchored
        // 仍负责贴合窗口边界，MenuPanel 负责长列表内部滚动。
        let mut model_picker = Dropdown::new(model_button).panel_anchor(
            Corner::BottomLeft,
            point(px(metrics::ZERO), px(-ANCHOR_GAP_Y)),
        );
        if model_menu_open {
            model_picker = model_picker.panel(self.model_menu_element(cx));
        }

        // ADR-063（API 1.21）：推理强度 chip——展示本轮生效强度（自动 =
        // 模型默认），菜单选择只影响下一轮 RunStart。gate 与模型切换同源
        //（已连接、空闲、已有生效模型）。
        let can_open_effort_menu = self.can_open_effort_menu();
        let effort_menu_open =
            matches!(self.open_menu, Some(MenuKind::Effort)) && can_open_effort_menu;
        let effort_label = self.effort_chip_label();
        let effort_tooltip = if can_open_effort_menu {
            SharedString::from(t("composer.effort_tooltip").to_string())
        } else {
            SharedString::from(self.model_disabled_reason())
        };
        let effort_focus = self.effort_focus.clone();
        let effort_chip_width = composer_model_chip_width(window, &effort_label);
        let mut effort_button = Button::new("effort-picker")
            .track_focus(&effort_focus)
            .variant(ButtonVariant::Raised)
            .text_color(dark().text.primary)
            .disabled(!can_open_effort_menu)
            .child(
                div()
                    .flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(px(metrics::SPACE_2))
                    .child(div().truncate().child(effort_label.clone()))
                    .child(
                        div()
                            .flex_none()
                            .child(icon_sized(Icon::ChevronDown, px(metrics::ICON_SM))),
                    ),
            )
            .tooltip(effort_tooltip)
            .height(px(metrics::COMPOSER_FOOTER_CONTROL))
            .width(px(effort_chip_width))
            .max_width(px(metrics::COMPOSER_MODEL_WIDTH))
            .vcenter();
        if can_open_effort_menu {
            effort_button = effort_button
                .on_click(cx.listener(|view, event, window, cx| {
                    if view.consume_button_key_click("effort-picker", event) {
                        return;
                    }
                    let down = Self::click_down_position(event);
                    view.on_toggle_effort_menu(down, window, cx);
                }))
                .on_activate(cx.listener(|view, _event, window, cx| {
                    if view.open_menu.is_some() {
                        view.note_button_key_activate("effort-picker");
                        return;
                    }
                    view.note_button_key_activate("effort-picker");
                    view.on_toggle_effort_menu(None, window, cx);
                    cx.stop_propagation();
                }));
        }
        let mut effort_picker = Dropdown::new(effort_button).panel_anchor(
            Corner::BottomLeft,
            point(px(metrics::ZERO), px(-ANCHOR_GAP_Y)),
        );
        if effort_menu_open {
            effort_picker = effort_picker.panel(self.effort_menu_element(cx));
        }

        let running = self.projection.active_run_id.is_some();
        let action_slot = if running {
            let action_focus = self.composer_action_focus.clone();
            let cancel_tooltip = if can_cancel {
                SharedString::from("Cancel run (Cmd+.)")
            } else {
                SharedString::from(self.cancel_disabled_reason())
            };
            let mut cancel = Button::new("composer-action")
                .variant(ButtonVariant::Danger)
                .icon_circle(metrics::COMPOSER_SEND_SIZE)
                .disabled(!can_cancel)
                .track_focus(&action_focus)
                .child(icon(Icon::Cancel).text_color(dark().text.on_accent))
                .tooltip(cancel_tooltip);
            if can_cancel {
                cancel = cancel
                    .on_click(cx.listener(|view, event, window, cx| {
                        if view.consume_button_key_click("composer-action", event) {
                            return;
                        }
                        view.on_cancel_clicked(window, cx);
                    }))
                    .on_activate(cx.listener(|view, _event, window, cx| {
                        view.note_button_key_activate("composer-action");
                        view.on_cancel_clicked(window, cx);
                        cx.stop_propagation();
                    }));
            }
            cancel.into_any_element()
        } else {
            let action_focus = self.composer_action_focus.clone();
            let send_tooltip = if can_send {
                SharedString::from(t("composer.send_tooltip"))
            } else {
                SharedString::from(self.send_disabled_reason())
            };
            let mut send = Button::new("composer-action")
                .variant(ButtonVariant::Primary)
                .icon_circle(metrics::COMPOSER_SEND_SIZE)
                .disabled(!can_send)
                .track_focus(&action_focus)
                .child(icon(Icon::Send).text_color(dark().text.on_accent))
                .tooltip(send_tooltip);
            if can_send {
                send = send
                    .on_click(cx.listener(|view, event, _window, cx| {
                        if view.consume_button_key_click("composer-action", event) {
                            return;
                        }
                        if view.text_input.read(cx).is_composing() {
                            return;
                        }
                        view.send_current_message(cx);
                    }))
                    .on_activate(cx.listener(|view, _event, _window, cx| {
                        if view.text_input.read(cx).is_composing() {
                            return;
                        }
                        view.note_button_key_activate("composer-action");
                        view.send_current_message(cx);
                        cx.stop_propagation();
                    }));
            }
            send.into_any_element()
        };

        let options = self.current_composer_options();
        let has_options = !options.attachments.is_empty() || options.web_search.is_some();
        let attachments = self.composer_attachments_element(cx);
        let card = div()
            .id("composer-card")
            .track_scroll(&self.composer_layouts["composer-card"])
            .debug_selector(|| "composer-card".into())
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(metrics::COMPOSER_GAP))
            .p(px(metrics::COMPOSER_PAD))
            .min_h(px(metrics::COMPOSER_PANEL_MIN_HEIGHT))
            .max_h(px(
                metrics::COMPOSER_PANEL_MAX_HEIGHT + if has_options { 108.0 } else { 0.0 }
            ))
            .border_1()
            .border_color(if input_focused {
                dark().accent.primary
            } else {
                dark().border.subtle
            })
            .rounded(px(metrics::COMPOSER_RADIUS))
            .bg(dark().surface.raised)
            .shadow_sm()
            .when(has_options, |card| card.child(attachments))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .min_h(px(metrics::COMPOSER_INPUT_MIN_HEIGHT))
                    .child(div().flex_1().min_w_0().child(self.text_input.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(metrics::SPACE_2))
                    .h(px(metrics::COMPOSER_SEND_SIZE))
                    .flex_none()
                    .child(attachment)
                    .child(
                        div()
                            .id("composer-model-slot")
                            .track_scroll(&self.composer_layouts["model-picker"])
                            .debug_selector(|| "composer-model-slot".into())
                            .w(px(model_chip_width))
                            .h(px(metrics::COMPOSER_FOOTER_CONTROL))
                            .flex_none()
                            .child(model_picker),
                    )
                    .child(
                        div()
                            .id("composer-effort-slot")
                            .track_scroll(&self.composer_layouts["effort-picker"])
                            .debug_selector(|| "composer-effort-slot".into())
                            .w(px(effort_chip_width))
                            .h(px(metrics::COMPOSER_FOOTER_CONTROL))
                            .flex_none()
                            .child(effort_picker),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("composer-action-slot")
                            .debug_selector(|| "composer-action-slot".into())
                            .w(px(metrics::COMPOSER_SEND_SIZE))
                            .h(px(metrics::COMPOSER_SEND_SIZE))
                            .flex_none()
                            .child(action_slot),
                    ),
            );

        let project_label = div()
            .id("composer-workspace")
            .track_scroll(&self.composer_layouts["composer-workspace"])
            .px(px(metrics::SPACE_2))
            .rounded(px(metrics::CONTROL_RADIUS))
            .bg(dark().bg.panel)
            .text_size(font::XS)
            .text_color(dark().text.secondary)
            .child(workspace_label);
        let mut project = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(metrics::SPACE_2))
            .child(project_label);
        if self.composer_project_task_visible() {
            let can_create = self.can_create_task();
            let mut button = Button::new("composer-project-task")
                .track_focus(&self.project_task_focus)
                .variant(ButtonVariant::Ghost)
                .padding(ButtonPadding::None)
                .height(px(meta_height))
                .text_size(font::XS)
                .label(self.composer_project_task_label())
                .disabled(!can_create);
            if !can_create {
                button = button.tooltip(SharedString::from(self.add_task_disabled_reason()));
            }
            button = button
                .on_click(cx.listener(|view, event, window, cx| {
                    if view.consume_button_key_click("composer-project-task", event) {
                        return;
                    }
                    view.on_project_task_menu(Self::click_down_position(event), window, cx);
                }))
                .on_activate(cx.listener(|view, _, window, cx| {
                    view.note_button_key_activate("composer-project-task");
                    if view.open_menu.is_some() {
                        return;
                    }
                    view.on_project_task_menu(None, window, cx);
                    cx.stop_propagation();
                }));
            let mut picker = Dropdown::new(button)
                .panel_anchor(Corner::BottomLeft, point(px(0.0), px(-ANCHOR_GAP_Y)));
            if matches!(self.open_menu, Some(MenuKind::ProjectTask)) {
                picker = picker.panel(self.scope_menu_element(cx));
            }
            project = project.child(
                div()
                    .id("composer-project-task-layout")
                    .track_scroll(&self.composer_layouts["composer-project-task"])
                    .child(picker),
            );
        }
        let mut meta = div()
            .id("composer-meta")
            .debug_selector(|| "composer-meta".into())
            .track_scroll(&self.composer_layouts["composer-meta"])
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(metrics::SPACE_2))
            .min_h(px(meta_height))
            .flex_none()
            .child(project);
        if context_visible {
            meta = meta.child(
                div()
                    .id("composer-context")
                    .track_scroll(&self.composer_layouts["composer-context"])
                    .text_size(font::XS)
                    .text_color(dark().text.tertiary)
                    .child(self.projection.context_meter_label()),
            );
        }
        let mut column = div()
            .w_full()
            .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
            .flex()
            .flex_col()
            .gap(px(metrics::COMPOSER_META_GAP))
            .child(card)
            .child(meta);
        for (id, note) in self.composer_notes() {
            column = column.child(
                div().id(id).flex().h(px(meta_height)).items_center().child(
                    div().flex_1().min_w_0().flex().child(
                        div().truncate().child(
                            Label::new(note)
                                .size(font::XS)
                                .color(dark().semantic.warning_text),
                        ),
                    ),
                ),
            );
        }
        div()
            .flex()
            .flex_col()
            .items_center()
            .flex_none()
            .px(px(metrics::COMPOSER_OUTER_X))
            .pt(px(metrics::COMPOSER_OUTER_TOP))
            .pb(px(metrics::COMPOSER_OUTER_BOTTOM))
            .child(column)
    }

    pub(super) fn composer_meta_height(window: &Window) -> f32 {
        (metrics::COMPOSER_META_HEIGHT * f32::from(window.rem_size()) / 16.0).max(28.0)
    }

    /// Render 与 AX 共用可见说明及顺序；失败提示不再挤压模型或发送。
    pub(super) fn composer_notes(&self) -> Vec<(&'static str, String)> {
        let mut notes = Vec::new();
        if self.active_task_hidden_by_filter() {
            notes.push(("composer-filter-hint", t("rail.task_hidden").into()));
        }
        notes
    }

    pub(super) fn composer_options_height(&self) -> f32 {
        let options = self.current_composer_options();
        if options.attachments.is_empty() && options.web_search.is_none() {
            return 0.0;
        }
        self.settings_element_layouts
            .get("composer-options")
            .map(|handle| f32::from(handle.bounds().size.height))
            .unwrap_or(0.0)
            + metrics::COMPOSER_GAP
    }

    pub(super) fn composer_outer_height(&self, input_height: f32, window: &Window) -> f32 {
        Self::composer_panel_height(input_height)
            + self.composer_options_height()
            + metrics::COMPOSER_OUTER_TOP
            + metrics::COMPOSER_OUTER_BOTTOM
            + metrics::COMPOSER_META_GAP
            + self.composer_meta_layout_height(window)
            + (metrics::COMPOSER_META_GAP + Self::composer_meta_height(window))
                * self.composer_notes().len() as f32
    }

    pub(super) fn composer_meta_layout_height(&self, window: &Window) -> f32 {
        f32::from(self.composer_layouts["composer-meta"].bounds().size.height)
            .max(Self::composer_meta_height(window))
    }

    pub(super) fn composer_project_task_visible(&self) -> bool {
        self.projection.active_session_id.is_none()
            || self.composer_file_tools_unavailable_visible()
    }

    /// Render / AX 共用：无项目任务用正向绑定文案，无任务空态沿用新建入口。
    pub(super) fn composer_project_task_label(&self) -> &'static str {
        if self.composer_file_tools_unavailable_visible() {
            t("composer.bind_project")
        } else {
            t("composer.project_task")
        }
    }

    /// 目录缺 window 或 0 哨兵视为未知，不画 ContextMeter。
    pub(super) fn composer_context_meter_visible(&self) -> bool {
        let Some((provider, id)) = self.projection.effective_model() else {
            return false;
        };
        self.projection.models.iter().any(|entry| {
            entry.provider_id == *provider
                && entry.id == *id
                && entry.context_window_tokens.is_some_and(|window| window > 0)
        })
    }

    pub(super) fn on_project_task_menu(
        &mut self,
        down: Option<Point<Pixels>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_create_task() {
            return;
        }
        self.toggle_menu(MenuKind::ProjectTask, down, cx);
        self.pending_scope_menu_scroll = matches!(self.open_menu, Some(MenuKind::ProjectTask));
    }

    pub(super) fn model_label(&self) -> String {
        match self.projection.connection {
            ConnectionState::Connecting => return self.connection_notice_text().0.into(),
            ConnectionState::Disconnected { .. } => return t("recovery.model_offline").into(),
            ConnectionState::Failed { .. } => return t("recovery.model_failed").into(),
            ConnectionState::Connected { .. } => {}
        }
        if self.model_catalog_empty() {
            // 全关 / 空目录：不回退展示旧默认对（Host 已清除角色默认）。
            return t("composer.model_none_available").into();
        }
        match self.projection.effective_model() {
            Some((provider, id)) => self
                .projection
                .models
                .iter()
                .find(|entry| entry.provider_id == *provider && entry.id == *id)
                .map(|entry| entry.display_name.clone())
                .unwrap_or_else(|| format!("{provider} / {id}")),
            None if self.projection.models.is_empty() => t("composer.model_loading").into(),
            None => t("composer.model_select").into(),
        }
    }

    fn model_disabled_reason(&self) -> String {
        if self.projection.active_run_id.is_some() {
            t("composer.model_disabled_running").into()
        } else if self.model_catalog_empty() {
            t("composer.model_disabled_empty").into()
        } else if self.projection.models.is_empty() {
            t("composer.model_disabled_loading").into()
        } else {
            t("composer.model_disabled_offline").into()
        }
    }

    pub(super) fn model_matches_search(&self, model: &ModelEntry) -> bool {
        let query = self.model_search_query.trim().to_lowercase();
        model.display_name.to_lowercase().contains(&query)
            || model.id.to_lowercase().contains(&query)
            || model_capability_label(model)
                .to_lowercase()
                .contains(&query)
    }

    pub(super) fn composer_model_row_title<'a>(&self, model: &'a ModelEntry) -> &'a str {
        model_menu_row_title(&model.display_name, &model.id)
    }

    pub(super) fn composer_model_groups(&self) -> Vec<(String, Vec<ModelEntry>)> {
        composer_model_menu_groups(
            &self.projection.models,
            &self.projection.settings_providers.providers,
            &self.model_search_query,
        )
    }

    pub(super) fn filtered_model_entries(&self) -> Vec<ModelEntry> {
        grouped_model_menu_entries(
            &self.projection.models,
            &self.projection.settings_providers.providers,
            &self.model_search_query,
        )
    }

    pub(super) fn model_menu_row_count(&self) -> usize {
        self.filtered_model_entries().len()
    }

    /// 可点击模型行在滚动列表中的子下标（组头占一位，管理入口不在列表内）。
    pub(super) fn model_menu_scroll_child_index(&self, item: usize) -> Option<usize> {
        let mut logical = 0;
        let mut child = 0;
        for (_, models) in self.composer_model_groups() {
            child += 1;
            if item < logical + models.len() {
                return Some(child + (item - logical));
            }
            logical += models.len();
            child += models.len();
        }
        None
    }

    pub(super) fn provider_display_name(&self, provider_id: &str) -> String {
        self.projection
            .settings_providers
            .providers
            .iter()
            .find(|entry| entry.provider_id == provider_id)
            .map(|entry| entry.display_name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| provider_id.to_string())
    }

    pub(super) fn model_provider_status(&self, provider_id: &str) -> String {
        use crate::projection::{ProviderCatalogState, ProviderStatusLabels};
        let state = &self.projection.settings_providers;
        if state.query.stale_reason.is_some() {
            return t("model_search.status_unknown").into();
        }
        let Some(provider) = state
            .providers
            .iter()
            .find(|entry| entry.provider_id == provider_id)
        else {
            return t("model_search.status_unknown").into();
        };
        let source = match &provider.catalog {
            ProviderCatalogState::Remote { .. } => t("model_search.remote"),
            ProviderCatalogState::FixedFallback { .. } => t("model_search.fallback"),
            ProviderCatalogState::Unavailable { .. } => t("model_search.unavailable"),
        };
        format!("{} · {source}", provider.auth_label())
    }

    pub(super) fn focus_model_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model_search_query.clear();
        self.model_search_input
            .update(cx, |input, cx| input.reset_text(String::new(), cx));
        self.model_menu_scroll = gpui::ScrollHandle::new();
        self.menu_highlight = None;
        self.pending_model_menu_scroll = matches!(self.open_menu, Some(MenuKind::Model));
        window.focus(&self.model_search_focus);
        cx.notify();
    }

    pub(super) fn clear_model_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model_search_input
            .update(cx, |input, cx| input.reset_text(String::new(), cx));
        window.focus(&self.model_search_focus);
        cx.notify();
    }

    pub(super) fn model_search_element(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let placeholder = if matches!(self.open_menu, Some(MenuKind::Model)) {
            t("model_search.placeholder_providers")
        } else {
            t("model_search.placeholder")
        };
        self.model_search_input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, cx);
        });
        let clear_focus = self
            .settings_action_focus
            .entry("model-search-clear".into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let clear = Button::new("model-search-clear")
            .track_focus(&clear_focus)
            .variant(ButtonVariant::Ghost)
            .text_size(font::SM)
            .label(t("model_search.clear"))
            .disabled(self.model_search_query.is_empty())
            .on_click(cx.listener(|view, _, window, cx| view.clear_model_search(window, cx)))
            .on_activate(cx.listener(|view, _, window, cx| {
                view.clear_model_search(window, cx);
                cx.stop_propagation();
            }));
        let input = self.model_search_input.clone();
        div()
            .flex()
            .items_center()
            .gap_1()
            .pb_2()
            .child(
                self.settings_element("model-search-input")
                    .flex_1()
                    .min_w_0()
                    .child(input),
            )
            .child(
                self.settings_element("model-search-clear")
                    .flex_none()
                    .child(clear),
            )
    }

    fn model_settings_entry(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let ix = self.model_menu_row_count();
        let focus = self
            .settings_action_focus
            .entry("model-menu-settings".into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let button = Button::new("model-menu-settings")
            .track_focus(&focus)
            .variant(ButtonVariant::Ghost)
            .label(t("model_search.manage"))
            .on_click(cx.listener(|view, _, window, cx| view.on_manage_composer_models(window, cx)))
            .on_activate(cx.listener(|view, _, window, cx| {
                view.on_manage_composer_models(window, cx);
                cx.stop_propagation();
            }));
        self.settings_element("model-menu-settings")
            .mt_2()
            .border_t_1()
            .border_color(dark().border.subtle)
            .when(self.menu_highlight == Some(ix), |row| {
                row.bg(dark().surface.raised)
            })
            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                if *hovered {
                    view.menu_highlight = Some(ix);
                    cx.notify();
                }
            }))
            .child(button)
    }

    /// 伪二级：组头与模型同一列表展开；未连接或 0 启用整组不出现。
    fn model_menu_element(&mut self, cx: &mut Context<Self>) -> MenuPanel {
        let highlight = self.menu_highlight_effective(self.menu_selected_index());
        let menu_scroll = self
            .settings_element_layouts
            .entry("model-menu".into())
            .or_default()
            .clone();
        let mut panel = MenuPanel::new("model-menu")
            .track_scroll(&menu_scroll)
            .max_height(480.0)
            .dismiss_on_outside(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::Model, event.position, cx);
            }));
        let content = div()
            .w(px(340.0))
            .flex()
            .flex_col()
            .child(self.model_search_element(cx))
            .when(self.model_menu_row_count() > 0, |content| {
                content.child(
                    self.settings_element("model-capability-help")
                        .px_2()
                        .py_1()
                        .text_size(font::XS)
                        .text_color(dark().text.secondary)
                        .whitespace_normal()
                        .child(t("model_capability.help")),
                )
            });
        if self.model_menu_row_count() == 0 {
            let (title, hint) = if self.projection.models.is_empty() {
                (
                    t("composer.model_none_available"),
                    t("composer.model_menu_empty"),
                )
            } else if self.model_search_query.trim().is_empty() {
                (t("model_search.no_providers"), t("model_search.manage"))
            } else {
                (t("model_search.no_results"), t("model_search.clear"))
            };
            return panel.child(
                content
                    .child(
                        self.settings_element("model-menu-empty")
                            .py_2()
                            .text_size(font::SM)
                            .whitespace_normal()
                            .text_color(dark().text.secondary)
                            .child(format!("{title}\n{hint}")),
                    )
                    .child(self.model_settings_entry(cx)),
            );
        }
        let mut list = div()
            .id("model-menu-list")
            .max_h(px(280.0))
            .overflow_y_scroll()
            .track_scroll(&self.model_menu_scroll);
        let mut ix = 0;
        for (provider_id, models) in self.composer_model_groups() {
            list = list.child(self.model_menu_group_header(&provider_id));
            for model in models {
                list = list.child(self.model_menu_model_row(ix, highlight, model, cx));
                ix += 1;
            }
        }
        panel = panel.child(content.child(list).child(self.model_settings_entry(cx)));
        panel
    }

    fn model_menu_group_header(&mut self, provider_id: &str) -> gpui::Stateful<gpui::Div> {
        let title = self.provider_display_name(provider_id);
        self.settings_element(format!("model-menu-group-{provider_id}"))
            .min_h(gpui::rems(1.5))
            .px_2()
            .pt_2()
            .flex()
            .items_center()
            .min_w_0()
            .truncate()
            .text_size(font::SM)
            .text_color(dark().text.secondary)
            .child(title)
    }

    fn model_menu_model_row(
        &mut self,
        ix: usize,
        highlight: usize,
        model: ModelEntry,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let selected = self
            .projection
            .effective_model()
            .is_some_and(|(provider, id)| *provider == model.provider_id && *id == model.id);
        let row_id = format!("model-{}-{}", model.provider_id, model.id);
        let title = model_menu_row_title(&model.display_name, &model.id);
        let capabilities = model_capability_label(&model);
        self.settings_element(row_id)
            .w_full()
            .py_2()
            .px_2()
            .rounded(px(metrics::CONTROL_RADIUS))
            .bg(if selected || ix == highlight {
                dark().surface.raised
            } else {
                dark().bg.menu
            })
            .hover(|style| style.bg(dark().surface.hover))
            .active(|style| style.bg(dark().surface.pressed))
            .cursor_pointer()
            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                if *hovered && view.menu_highlight != Some(ix) {
                    view.menu_highlight = Some(ix);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(metrics::SPACE_4))
                            .flex_none()
                            .when(selected, |slot| {
                                slot.child(
                                    icon_sized(Icon::Check, px(metrics::ICON_SM))
                                        .text_color(dark().accent.primary),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(font::SM)
                            .text_color(dark().text.primary)
                            .whitespace_normal()
                            .child(title.to_string()),
                    )
                    .when(!capabilities.is_empty(), |row| {
                        row.child(
                            div()
                                .flex_none()
                                .text_size(font::XS)
                                .text_color(dark().text.secondary)
                                .child(capabilities),
                        )
                    }),
            )
            .on_click(cx.listener(move |view, _, window, cx| {
                view.on_select_model(model.clone(), cx);
                window.focus(&view.model_focus);
            }))
    }

    /// 推理强度菜单选项（render / 键盘 / AX 同源）：行 0 = 自动（None，
    /// Host 回落模型默认），其后为当前生效模型的可选范围；范围未知时给
    /// 全量 canonical 词汇（ADR-063）。
    pub(super) fn effort_menu_options(&self) -> Vec<Option<String>> {
        let model = self
            .projection
            .effective_model()
            .and_then(|(provider, id)| find_model_entry(&self.projection.models, provider, id));
        let mut options = vec![None];
        match model {
            Some(model) => {
                for level in model.effort_options() {
                    options.push(Some(level));
                }
            }
            None => {
                for level in crate::projection::EFFORT_LEVELS {
                    options.push(Some(level.to_string()));
                }
            }
        }
        options
    }

    /// 强度 chip 文案：显式强度名或「自动」。
    pub(super) fn effort_chip_label(&self) -> String {
        self.projection
            .effective_effort()
            .map(str::to_string)
            .unwrap_or_else(|| t("composer.effort_auto").to_string())
    }

    /// 强度菜单开关（gate 复核与 render 同源）。
    pub(super) fn on_toggle_effort_menu(
        &mut self,
        down_position: Option<Point<Pixels>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_open_effort_menu() {
            return;
        }
        self.toggle_menu(MenuKind::Effort, down_position, cx);
    }

    /// 选择强度（None = 自动）；即时生效到 Composer 状态，随下一轮
    /// RunStart 发出。
    pub(super) fn on_select_effort(&mut self, effort: Option<String>, cx: &mut Context<Self>) {
        if !self.can_open_effort_menu() {
            return;
        }
        self.projection.pending_effort = effort;
        self.open_menu = None;
        self.menu_highlight = None;
        cx.notify();
    }

    /// 强度菜单面板：自动 + 当前模型可选范围，行少无需滚动。
    fn effort_menu_element(&mut self, cx: &mut Context<Self>) -> MenuPanel {
        let highlight = self.menu_highlight_effective(self.menu_selected_index());
        let current = self.projection.effective_effort().map(str::to_string);
        let options = self.effort_menu_options();
        let menu_scroll = self
            .settings_element_layouts
            .entry("effort-menu".into())
            .or_default()
            .clone();
        let panel = MenuPanel::new("effort-menu")
            .track_scroll(&menu_scroll)
            .max_height(320.0)
            .dismiss_on_outside(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::Effort, event.position, cx);
            }));
        let mut list = div().w(px(200.0)).flex().flex_col();
        for (ix, option) in options.iter().enumerate() {
            let selected = *option == current;
            let label = match option {
                Some(level) => level.clone(),
                None => t("composer.effort_auto").to_string(),
            };
            let row_id = format!("effort-{}", option.as_deref().unwrap_or("auto"));
            let option_click = option.clone();
            list = list.child(
                self.settings_element(row_id)
                    .w_full()
                    .py_2()
                    .px_2()
                    .rounded(px(metrics::CONTROL_RADIUS))
                    .bg(if selected || ix == highlight {
                        dark().surface.raised
                    } else {
                        dark().bg.menu
                    })
                    .hover(|style| style.bg(dark().surface.hover))
                    .active(|style| style.bg(dark().surface.pressed))
                    .cursor_pointer()
                    .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                        if *hovered && view.menu_highlight != Some(ix) {
                            view.menu_highlight = Some(ix);
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(metrics::SPACE_4)).flex_none().when(
                                selected,
                                |slot| {
                                    slot.child(
                                        icon_sized(Icon::Check, px(metrics::ICON_SM))
                                            .text_color(dark().accent.primary),
                                    )
                                },
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(font::SM)
                                    .text_color(dark().text.primary)
                                    .child(label),
                            ),
                    )
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.on_select_effort(option_click.clone(), cx);
                        window.focus(&view.effort_focus);
                    })),
            );
        }
        panel.child(list)
    }

    /// Composer 空输入 placeholder：只走连接/session/run 状态机，不被
    /// status_hint 覆盖（瞬态反馈改落 StatusBar 右栏）。
    pub(super) fn composer_placeholder_hint(&self) -> String {
        composer_placeholder_hint(
            &self.projection.connection,
            self.projection.active_run_id.is_some(),
        )
    }

    /// 已连接且 model_list 已完成但为空（全关 / 无目录）：唯一呈现
    /// 「无已启用模型」空态的状态。
    pub(super) fn model_catalog_empty(&self) -> bool {
        model_catalog_empty_state(
            matches!(
                self.projection.connection,
                ConnectionState::Connected { .. }
            ),
            self.projection.models_loaded,
            self.projection.models.is_empty(),
        )
    }

    fn send_disabled_reason(&self) -> String {
        if self.pending_home_send.is_some() {
            t("composer.send_disabled_starting").into()
        } else if self.projection.active_run_id.is_some() {
            t("composer.placeholder_running").into()
        } else {
            match &self.projection.connection {
                ConnectionState::Connected { .. } => {
                    if self.model_catalog_empty() {
                        t("composer.send_disabled_no_models").into()
                    } else {
                        t("composer.send_disabled_empty").into()
                    }
                }
                ConnectionState::Connecting => t("composer.placeholder_waiting").into(),
                ConnectionState::Disconnected { .. } => {
                    t("composer.placeholder_disconnected").into()
                }
                ConnectionState::Failed { .. } => t("composer.placeholder_connect_failed").into(),
            }
        }
    }

    fn sync_composer_placeholder(&self, hint: String, cx: &mut Context<Self>) {
        self.text_input.update(cx, |input, cx| {
            input.set_placeholder(hint, cx);
        });
    }

    /// 面板常态总高：border + pad×2 + 输入行 + gap + 动作槽。
    pub(super) fn composer_panel_height(input_height: f32) -> f32 {
        (metrics::COMPOSER_BORDER
            + metrics::COMPOSER_PAD * 2.0
            + input_height.max(metrics::COMPOSER_INPUT_MIN_HEIGHT)
            + metrics::COMPOSER_GAP
            + metrics::COMPOSER_SEND_SIZE)
            .clamp(
                metrics::COMPOSER_PANEL_MIN_HEIGHT,
                metrics::COMPOSER_PANEL_MAX_HEIGHT,
            )
    }

    pub(super) fn composer_workspace_label(&self) -> String {
        if let Some(session_id) = &self.projection.active_session_id {
            if let Some(session) = self
                .projection
                .sessions
                .iter()
                .find(|session| &session.session_id == session_id)
            {
                return t("composer.workspace_scope").replace(
                    "{}",
                    &self
                        .projection
                        .workspace_name(session.workspace_id.as_deref()),
                );
            }
        }
        match self.scope_workspace_id.as_deref() {
            Some(id) => t("composer.workspace_scope")
                .replace("{}", &self.projection.workspace_name(Some(id))),
            None => t("composer.no_project_chip").into(),
        }
    }

    /// Composer footer 的项目上下文是否为「无项目」：active session 优先
    ///（ADR-054 D1 无归属会话），否则回落当前 scope；All projects 同样
    /// 视为无项目上下文（新建任务将不绑定 workspace）。
    pub(super) fn composer_workspace_no_project(&self) -> bool {
        if let Some(session_id) = self.projection.active_session_id.as_deref() {
            if let Some(session) = self
                .projection
                .sessions
                .iter()
                .find(|session| session.session_id == session_id)
            {
                return session.workspace_id.is_none();
            }
        }
        self.scope_workspace_id.is_none()
    }

    /// 当前激活会话无项目：元信息行显示正向绑定入口，不再画警告色限制。
    /// 无 active session 时不算（还没有任务上下文）。
    pub(super) fn composer_file_tools_unavailable_visible(&self) -> bool {
        self.projection
            .active_session_id
            .as_deref()
            .and_then(|session_id| {
                self.projection
                    .sessions
                    .iter()
                    .find(|session| session.session_id == session_id)
            })
            .is_some_and(|session| session.workspace_id.is_none())
    }

    pub(super) fn on_toggle_model_menu(
        &mut self,
        down_position: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_open_model_menu() {
            return;
        }
        self.toggle_menu(MenuKind::Model, down_position, cx);
        if matches!(self.open_menu, Some(MenuKind::Model)) {
            self.focus_model_search(window, cx);
            self.controller.load_provider_status();
        }
    }

    pub(super) fn on_select_model(&mut self, model: ModelEntry, cx: &mut Context<Self>) {
        if !self.can_switch_model() {
            return;
        }
        self.projection
            .set_pending_model(model.provider_id, model.id);
        self.open_menu = None;
        self.menu_highlight = None;
        cx.notify();
    }

    fn cancel_disabled_reason(&self) -> String {
        if self.projection.active_run_id.is_none() {
            "No active run to cancel.".into()
        } else {
            "Cancel needs a live connection.".into()
        }
    }
}

pub(super) fn composer_send_allowed(
    connected: bool,
    running: bool,
    catalog_empty: bool,
    has_text: bool,
    home_send_pending: bool,
) -> bool {
    connected && !running && !catalog_empty && has_text && !home_send_pending
}

fn composer_placeholder_hint(connection: &ConnectionState, running: bool) -> String {
    if running {
        return t("composer.placeholder_running").into();
    }
    match connection {
        ConnectionState::Connected { .. } => t("composer.placeholder_message").into(),
        ConnectionState::Connecting => t("composer.placeholder_waiting").into(),
        ConnectionState::Disconnected { .. } => t("composer.placeholder_disconnected").into(),
        ConnectionState::Failed { .. } => t("composer.placeholder_connect_failed").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        composer_model_menu_groups, composer_placeholder_hint, composer_send_allowed,
        grouped_model_menu_entries, model_catalog_empty_state, model_menu_row_title, AppView,
    };
    use crate::projection::{
        ConnectionState, ModelEntry, ProviderAuthState, ProviderAuthStatusEntry,
        ProviderCatalogState,
    };
    use crate::ui::theme::metrics;

    fn model_entry(provider_id: &str, id: &str, display_name: &str) -> ModelEntry {
        ModelEntry {
            provider_id: provider_id.into(),
            id: id.into(),
            display_name: display_name.into(),
            ..ModelEntry::default()
        }
    }

    fn provider_entry(provider_id: &str, auth: ProviderAuthState) -> ProviderAuthStatusEntry {
        ProviderAuthStatusEntry {
            provider_id: provider_id.into(),
            display_name: provider_id.into(),
            endpoint_label: String::new(),
            auth_methods: vec!["api_key".into()],
            credentials: Vec::new(),
            selection_mode: Default::default(),
            auth,
            catalog: ProviderCatalogState::Unavailable {
                error: "offline".into(),
                fetched_at: None,
            },
            use_proxy: true,
        }
    }

    fn connected(provider_id: &str) -> ProviderAuthStatusEntry {
        provider_entry(
            provider_id,
            ProviderAuthState::Connected {
                method: "api_key".into(),
                masked_credential: None,
            },
        )
    }

    #[test]
    fn composer_placeholder_hint_follows_connection_and_run_state() {
        let connected = ConnectionState::Connected {
            instance_id: "dev".into(),
        };
        assert_eq!(
            composer_placeholder_hint(&connected, false),
            "Message Pawork…"
        );
        assert_eq!(
            composer_placeholder_hint(&connected, true),
            "Run in progress — sending is disabled. Cancel remains available."
        );
        assert_eq!(
            composer_placeholder_hint(&ConnectionState::Connecting, false),
            "Waiting for connection…"
        );
        assert_eq!(
            composer_placeholder_hint(
                &ConnectionState::Disconnected {
                    reason: "lost".into(),
                },
                false,
            ),
            "Disconnected — click Reconnect before sending."
        );
        assert_eq!(
            composer_placeholder_hint(
                &ConnectionState::Failed {
                    reason: "boom".into(),
                },
                false,
            ),
            "Connect failed — click Reconnect."
        );
        // 瞬态 status_hint 不再覆盖 placeholder 状态机（改落 StatusBar）。
        assert_ne!(composer_placeholder_hint(&connected, false), "Forked · s-1");
        assert!(composer_send_allowed(true, false, false, true, false));
        assert!(!composer_send_allowed(true, false, false, true, true));
        assert!(!composer_send_allowed(true, false, false, false, false));
        assert!(!composer_send_allowed(true, true, false, true, false));
    }

    #[test]
    fn composer_panel_height_clamps_across_input_sizes() {
        let idle = AppView::composer_panel_height(metrics::COMPOSER_INPUT_MIN_HEIGHT);
        assert_eq!(idle, metrics::COMPOSER_PANEL_MIN_HEIGHT);
        let mid = AppView::composer_panel_height(80.0);
        assert!(mid > idle);
        assert!(mid < metrics::COMPOSER_PANEL_MAX_HEIGHT);
        let capped = AppView::composer_panel_height(400.0);
        assert_eq!(capped, metrics::COMPOSER_PANEL_MAX_HEIGHT);
    }

    #[test]
    fn model_catalog_empty_state_requires_connected_loaded_and_empty() {
        // 只有「已连接 + 目录查询已完成 + 结果为空」才呈现全关空态。
        assert!(model_catalog_empty_state(true, true, true));
        // 加载中：查询未完成，不得把空 models 误报为全部禁用。
        assert!(!model_catalog_empty_state(true, false, true));
        // 断线：即使上一连接曾加载为空，也回到 offline 文案。
        assert!(!model_catalog_empty_state(false, true, true));
        // 已加载且有启用模型：正常分组菜单。
        assert!(!model_catalog_empty_state(true, true, false));
    }

    #[test]
    fn model_menu_selected_follows_effective_model() {
        let mut models = [
            model_entry("openai", "gpt-4.1", "GPT-4.1"),
            model_entry("anthropic", "opus", "Opus"),
            model_entry("openai", "gpt-4.1-mini", "GPT-4.1 mini"),
            model_entry("glm-coding", "glm-5.3", ""),
        ];
        let providers = [
            connected("openai"),
            connected("anthropic"),
            connected("glm-coding"),
        ];
        let entries = grouped_model_menu_entries(&models, &providers, "");
        assert_eq!(
            entries
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-4.1", "gpt-4.1-mini", "opus", "glm-5.3"]
        );
        let untitled = entries
            .iter()
            .find(|entry| entry.id == "glm-5.3")
            .expect("empty display_name still appears in the grouped menu");
        assert_eq!(
            model_menu_row_title(&untitled.display_name, &untitled.id),
            "glm-5.3"
        );
        let selected = Some(("anthropic".to_string(), "opus".to_string()));
        let selected_ix = selected
            .as_ref()
            .and_then(|(provider, id)| {
                entries
                    .iter()
                    .position(|entry| entry.provider_id == *provider && entry.id == *id)
            })
            .unwrap_or(0);
        assert_eq!(selected_ix, 2);
        assert_eq!(entries[selected_ix].display_name, "Opus");
        let none_ix = None::<(String, String)>
            .as_ref()
            .and_then(|(provider, id)| {
                entries
                    .iter()
                    .position(|entry| entry.provider_id == *provider && entry.id == *id)
            })
            .unwrap_or(0);
        assert_eq!(none_ix, 0);
        let groups = composer_model_menu_groups(&models, &providers, "");
        assert_eq!(
            groups
                .iter()
                .map(|(provider, _)| provider.as_str())
                .collect::<Vec<_>>(),
            ["openai", "anthropic", "glm-coding"]
        );
        assert_eq!(
            composer_model_menu_groups(&models, &providers, "gpt-4.1-mini")
                .iter()
                .map(|(provider, models)| (provider.as_str(), models.len()))
                .collect::<Vec<_>>(),
            [("openai", 1)]
        );
        assert_eq!(
            grouped_model_menu_entries(&models, &providers, "glm-5.3")
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["glm-5.3"]
        );
        assert!(composer_model_menu_groups(&models, &providers, "claude").is_empty());
        models[0].web_search = true;
        models[1].display_name = "Search without hosted capability".into();
        assert_eq!(
            grouped_model_menu_entries(
                &models,
                &providers,
                crate::ui::i18n::t("model_capability.search")
            )
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
            ["gpt-4.1"]
        );
    }

    #[test]
    fn composer_model_menu_hides_disconnected_and_empty_providers() {
        let models = [
            model_entry("openai", "gpt-4.1", "GPT-4.1"),
            model_entry("anthropic", "opus", "Opus"),
            model_entry("ghost", "ghost-x", "Ghost"),
        ];
        let providers = [
            connected("openai"),
            provider_entry("anthropic", ProviderAuthState::None),
        ];
        let groups = composer_model_menu_groups(&models, &providers, "");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, "openai");
        assert_eq!(groups[0].1[0].id, "gpt-4.1");
        assert!(composer_model_menu_groups(&models, &providers, "opus").is_empty());
        assert_eq!(
            grouped_model_menu_entries(&models, &providers, "openai").len(),
            1
        );
    }
}
