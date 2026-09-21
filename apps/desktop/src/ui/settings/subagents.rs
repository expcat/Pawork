//! Settings 子代理页。

use super::*;
use crate::projection::{SettingsSubagentsState, SubagentModelRule, SubagentSettingsData};
use crate::ui::components::switch::Switch;

/// 权限 chip 高度（独立于页面主控件，保持紧凑）。
const SUBAGENT_CHIP_HEIGHT: f32 = 28.0;

impl AppView {
    /// 「子代理」页：全局开关 + 并发上限 + 每模型规则（可发起 / 可作为
    /// 子代理 / 功能权限）。所有写动作均为即时全态写（克隆 Host 权威配
    /// 置改一处后回传），回执收敛不乐观更新；模型目录与供应商页同源
    /// （model_catalog），空目录诚实空态。stale / 断线只读。
    pub(super) fn settings_subagents_page_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let writes = self.settings_subagents_writes_enabled();
        let state = self.projection.settings_subagents.clone();
        let status_lines = subagents_status_lines(&state);
        let concurrency_text = self
            .settings_subagents_concurrency_input
            .read(cx)
            .text()
            .to_string();
        let concurrency_value = parse_subagent_max_concurrent(&concurrency_text);
        // RV-08：错误原因逐帧从输入文本推导（render 与 AX 同源）；
        // 空输入按「待输入」处理，只显示范围提示。
        let concurrency_error = Self::subagent_concurrency_error(&concurrency_text);
        let save_enabled = writes && concurrency_value.is_some();
        let concurrency_current = state.settings.max_concurrent.to_string();
        let concurrency_input = self.settings_subagents_concurrency_input.clone();
        let save_focus = self.settings_subagents_save_focus.clone();
        let refresh_focus = self.settings_refresh_focus.clone();

        let refresh = Button::new("settings-refresh")
            .track_focus(&refresh_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BASE)
            .label(t("settings.refresh"))
            .tooltip(t("settings.subagents.refresh_tooltip"))
            .disabled(!connected)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-refresh", event) {
                    return;
                }
                view.on_refresh_settings(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-refresh");
                view.on_refresh_settings(cx);
                cx.stop_propagation();
            }));

        let save = Button::new("settings-subagents-save")
            .track_focus(&save_focus)
            .variant(ButtonVariant::Primary)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BASE)
            .label(t("settings.save"))
            .tooltip(t("settings.subagents.save_tooltip"))
            .disabled(!save_enabled)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-subagents-save", event) {
                    return;
                }
                view.on_settings_subagents_save_concurrency(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-subagents-save");
                view.on_settings_subagents_save_concurrency(cx);
                cx.stop_propagation();
            }));

        let enabled_switch = self.subagent_switch(
            "settings-subagents-enabled",
            state.settings.enabled,
            t("settings.subagents.enabled_tooltip"),
            writes,
            cx,
            |view, cx| {
                view.on_settings_subagents_toggle_enabled(cx);
            },
        );
        let enabled_state = if state.settings.enabled {
            t("settings.subagents.switch_on")
        } else {
            t("settings.subagents.switch_off")
        };

        let mut content = settings_column().child(
            div()
                .flex()
                .items_start()
                .gap_6()
                .child(self.settings_heading(
                    t("settings.subagents.title"),
                    t("settings.subagents.subtitle"),
                ))
                .child(
                    self.settings_element("settings-refresh")
                        .flex_none()
                        .child(refresh),
                ),
        );
        for (kind, line) in status_lines {
            content = content.child(self.settings_status(kind, line));
        }

        // RV-08：并发输入旁常显允许范围提示；非法非空输入追加 danger
        // 色原因，恢复合法值后逐帧清除（不截断、不自动保存）。
        let mut concurrency_column = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(settings_copy(t("settings.subagents.concurrency_label")))
            .child(
                self.settings_element("settings-subagents-concurrency-input")
                    .flex()
                    .w(px(112.0))
                    .when(!writes, |el| el.opacity(0.55))
                    .child(concurrency_input),
            )
            .child(
                self.settings_element("settings-subagents-concurrency-hint")
                    .child(
                        Label::new(t("settings.subagents.concurrency_hint"))
                            .size(font::BODY_SM)
                            .color(dark().text.tertiary),
                    ),
            );
        if let Some(error) = concurrency_error {
            concurrency_column = concurrency_column.child(
                self.settings_element("settings-subagents-concurrency-error")
                    .child(
                        Label::new(error)
                            .size(font::BODY_SM)
                            .color(dark().semantic.danger_text),
                    ),
            );
        }
        // 通用区：全局开关 + 并发上限。
        let general = settings_section()
            .child(settings_label(t("settings.subagents.general_label")))
            .child(
                div()
                    .id("settings-subagents-enabled-row")
                    .flex()
                    .items_center()
                    .gap_4()
                    .px_4()
                    .py_4()
                    .border_1()
                    .border_color(dark().border.subtle)
                    .rounded(px(6.0))
                    .child(
                        self.settings_element("settings-subagents-enabled-text")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(settings_label(t("settings.subagents.enabled_label")))
                            .child(settings_copy(t("settings.subagents.enabled_note"))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .child(
                                Label::new(enabled_state)
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            )
                            // AX bounds 同源：开关须包 settings_element。
                            .child(
                                self.settings_element("settings-subagents-enabled")
                                    .flex_none()
                                    .child(enabled_switch),
                            ),
                    ),
            )
            .child(self.settings_note(
                "settings-subagents-concurrency-current",
                t("settings.subagents.concurrency_current").replace("{}", &concurrency_current),
            ))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(concurrency_column)
                    .child(
                        self.settings_element("settings-subagents-save")
                            .flex_none()
                            .child(save),
                    ),
            );

        content = content.child(general).child(
            settings_section().child(
                self.settings_element("settings-subagents-effect")
                    .w_full()
                    .child(settings_copy(t("settings.subagents.effect_note"))),
            ),
        );

        // 模型区：按 provider 分组，每模型一行卡片。
        let models_section = settings_section()
            .child(settings_label(t("settings.subagents.models_section")))
            .child(self.settings_note(
                "settings-subagents-models-note",
                t("settings.subagents.models_note"),
            ));
        // ADR-063：只列「模型与供应商」中已启用的模型（未启用不出现在
        // 子代理配置中；派发 / AX 同源用 subagent_rule_models）。
        let catalog = crate::projection::subagent_rule_models(
            &self.projection.settings_providers.model_catalog,
        );
        let models_section = if catalog.is_empty() {
            models_section.child(self.settings_note(
                "settings-subagents-models-empty",
                t("settings.subagents.models_empty"),
            ))
        } else {
            let mut section = models_section;
            for (provider_id, models) in group_models_by_provider(&catalog) {
                section = section.child(self.settings_note(
                    dynamic_identifier("settings-subagent-provider", &provider_id),
                    provider_id.clone(),
                ));
                for model in models {
                    let row = self.subagent_model_row(&model, &state, writes, cx);
                    section = section.child(row);
                }
            }
            section
        };

        content.child(models_section)
    }

    /// 通用开关（全局 enabled / 模型 allow_spawn / allow_as_subagent 共用
    /// 形态）：focus 入口统一走 settings_action_focus，click 与键盘 activate
    /// 同一 handler。
    fn subagent_switch(
        &mut self,
        id: &str,
        checked: bool,
        tooltip: &'static str,
        writes: bool,
        cx: &mut Context<Self>,
        on_toggle: impl Fn(&mut Self, &mut Context<Self>) + Clone + 'static,
    ) -> impl IntoElement {
        let id = id.to_string();
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let on_toggle_activate = on_toggle.clone();
        Switch::new(id.clone())
            .track_focus(&focus)
            .checked(checked)
            .tooltip(tooltip)
            .disabled(!writes)
            .on_click(cx.listener(move |view, _event, _window, cx| {
                on_toggle(view, cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                on_toggle_activate(view, cx);
                cx.stop_propagation();
            }))
    }

    /// 模型规则卡片：显示名 + 默认/自定义徽标 + 两个开关 + 7 项功能
    /// 权限 chip + 自定义规则时的「重置为默认」。
    fn subagent_model_row(
        &mut self,
        model: &ModelEntry,
        state: &SettingsSubagentsState,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let provider_id = model.provider_id.clone();
        let model_id = model.id.clone();
        let rule = state.effective_rule(&provider_id, &model_id);
        let has_rule = state.has_rule(&provider_id, &model_id);
        let badge = if has_rule {
            t("settings.subagents.custom_badge")
        } else {
            t("settings.subagents.default_badge")
        };
        let row_id = settings_subagent_identifier("model", &provider_id, &model_id);

        let spawn_switch = self.subagent_switch(
            &settings_subagent_identifier("spawn", &provider_id, &model_id),
            rule.allow_spawn,
            t("settings.subagents.allow_spawn_tooltip"),
            writes,
            cx,
            {
                let provider = provider_id.clone();
                let model = model_id.clone();
                move |view, cx| {
                    view.on_settings_subagents_toggle_spawn(provider.clone(), model.clone(), cx);
                }
            },
        );
        let delegate_switch = self.subagent_switch(
            &settings_subagent_identifier("delegate", &provider_id, &model_id),
            rule.allow_as_subagent,
            t("settings.subagents.allow_delegate_tooltip"),
            writes,
            cx,
            {
                let provider = provider_id.clone();
                let model = model_id.clone();
                move |view, cx| {
                    view.on_settings_subagents_toggle_delegate(provider.clone(), model.clone(), cx);
                }
            },
        );

        let mut name_row = div().flex().items_center().gap_2().child(
            self.settings_element(dynamic_identifier(
                "settings-subagent-model-name",
                &format!("{provider_id}:{model_id}"),
            ))
            .flex_1()
            .min_w_0()
            .child(
                Label::new(model.display_name.clone())
                    .size(font::BODY)
                    .color(dark().text.primary),
            ),
        );
        // ADR-063：目录能力徽标（图像识别 / 搜索），只展示真实能力位。
        for (capable, label) in [
            (model.image_input, t("settings.subagents.capability_image")),
            (model.web_search, t("settings.subagents.capability_search")),
        ] {
            if capable {
                name_row = name_row.child(
                    div()
                        .flex_none()
                        .px_1()
                        .border_1()
                        .border_color(dark().border.subtle)
                        .rounded(px(4.0))
                        .child(Label::new(label).size(font::XS).color(dark().text.tertiary)),
                );
            }
        }
        name_row = name_row.child(Label::new(badge).size(font::XS).color(dark().text.tertiary));
        let mut card = self
            .settings_element(row_id)
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .border_1()
            .border_color(dark().border.subtle)
            .rounded(px(6.0))
            .bg(dark().surface.raised)
            .child(name_row)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .child(
                                Label::new(t("settings.subagents.allow_spawn"))
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            )
                            .child(spawn_switch),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .child(
                                Label::new(t("settings.subagents.allow_delegate"))
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            )
                            .child(delegate_switch),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .map(|chips| {
                        let mut row = chips;
                        for permission in SUBAGENT_PERMISSIONS {
                            let granted = rule.permissions.iter().any(|p| p == permission);
                            let chip = self.subagent_permission_chip(
                                permission,
                                granted,
                                &provider_id,
                                &model_id,
                                writes,
                                cx,
                            );
                            row = row.child(chip);
                        }
                        row
                    }),
            )
            // ADR-063：子代理推理强度行——默认强度 cycle（自动 → 候选依次）
            // + 可选范围 chips（空 = 不限）。
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .child(
                                Label::new(t("settings.subagents.effort_default"))
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            )
                            .child(self.subagent_effort_default_button(
                                &rule,
                                &provider_id,
                                &model_id,
                                writes,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .child(
                                Label::new(t("settings.subagents.effort_allowed"))
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            )
                            .map(|chips| {
                                let mut row = chips;
                                for level in model.effort_options() {
                                    let selected = rule.allowed_efforts.iter().any(|l| l == &level);
                                    let chip = self.subagent_effort_chip(
                                        &level,
                                        selected,
                                        &provider_id,
                                        &model_id,
                                        writes,
                                        cx,
                                    );
                                    row = row.child(chip);
                                }
                                row
                            }),
                    ),
            );

        if has_rule {
            let reset_id = settings_subagent_identifier("reset", &provider_id, &model_id);
            let reset_focus = self
                .settings_action_focus
                .entry(reset_id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let reset_click_id = reset_id.clone();
            let reset_provider = provider_id.clone();
            let reset_model = model_id.clone();
            let reset = Button::new(reset_id.clone())
                .track_focus(&reset_focus)
                .variant(ButtonVariant::Raised)
                .height(px(SUBAGENT_CHIP_HEIGHT))
                .radius(6.0)
                .text_size(font::BODY_SM)
                .label(t("settings.subagents.reset"))
                .tooltip(t("settings.subagents.reset_tooltip"))
                .disabled(!writes)
                .on_click(cx.listener(move |view, event, _window, cx| {
                    if view.consume_button_key_click(&reset_click_id, event) {
                        return;
                    }
                    view.on_settings_subagents_reset(
                        reset_provider.clone(),
                        reset_model.clone(),
                        cx,
                    );
                }));
            card = card.child(self.settings_element(reset_id).flex_none().child(reset));
        }

        card.into_any_element()
    }

    /// 单项功能权限 chip：已授予用 Primary、未授予用 Raised，click 与键盘
    /// activate 同一 dispatch 路径。
    fn subagent_permission_chip(
        &mut self,
        permission: &'static str,
        granted: bool,
        provider_id: &str,
        model_id: &str,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let action = format!("perm-{permission}");
        let id = settings_subagent_identifier(&action, provider_id, model_id);
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let variant = if granted {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Raised
        };
        let click_id = id.clone();
        let activate_id = id.clone();
        Button::new(id.clone())
            .track_focus(&focus)
            .variant(variant)
            .height(px(SUBAGENT_CHIP_HEIGHT))
            .radius(6.0)
            .text_size(font::BODY_SM)
            .label(subagent_permission_label(permission))
            .tooltip(subagent_permission_label(permission))
            .disabled(!writes)
            .on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_button_key_click(&click_id, event) {
                    return;
                }
                view.dispatch_settings_subagent_control(&click_id, cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_button_key_activate(&activate_id);
                view.dispatch_settings_subagent_control(&activate_id, cx);
                cx.stop_propagation();
            }))
    }

    /// 子代理默认强度 cycle 按钮（ADR-063）：label 为当前默认（自动 /
    /// canonical 名），click 与键盘 activate 走同一 dispatch。
    fn subagent_effort_default_button(
        &mut self,
        rule: &SubagentModelRule,
        provider_id: &str,
        model_id: &str,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = settings_subagent_identifier("effort-default", provider_id, model_id);
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let label = rule
            .default_effort
            .clone()
            .unwrap_or_else(|| t("settings.subagents.effort_auto").to_string());
        let click_id = id.clone();
        let activate_id = id.clone();
        Button::new(id.clone())
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SUBAGENT_CHIP_HEIGHT))
            .radius(6.0)
            .text_size(font::BODY_SM)
            .label(label)
            .tooltip(t("settings.subagents.effort_default_tooltip"))
            .disabled(!writes)
            .on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_button_key_click(&click_id, event) {
                    return;
                }
                view.dispatch_settings_subagent_control(&click_id, cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_button_key_activate(&activate_id);
                view.dispatch_settings_subagent_control(&activate_id, cx);
                cx.stop_propagation();
            }))
    }

    /// 可选强度范围 chip（ADR-063）：选中用 Primary、未选中用 Raised；
    /// 全不选 = 不限。
    fn subagent_effort_chip(
        &mut self,
        level: &str,
        selected: bool,
        provider_id: &str,
        model_id: &str,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = settings_subagent_identifier(&format!("effort-{level}"), provider_id, model_id);
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let variant = if selected {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Raised
        };
        let level = level.to_string();
        let click_id = id.clone();
        let activate_id = id.clone();
        Button::new(id.clone())
            .track_focus(&focus)
            .variant(variant)
            .height(px(SUBAGENT_CHIP_HEIGHT))
            .radius(6.0)
            .text_size(font::BODY_SM)
            .label(level.clone())
            .tooltip(t("settings.subagents.effort_allowed_tooltip"))
            .disabled(!writes)
            .on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_button_key_click(&click_id, event) {
                    return;
                }
                view.dispatch_settings_subagent_control(&click_id, cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_button_key_activate(&activate_id);
                view.dispatch_settings_subagent_control(&activate_id, cx);
                cx.stop_propagation();
            }))
    }

    /// 可见控件 → 单一派发入口（click / 键盘 / AX 三路径同源）：identifier
    /// 解析出动作与目标模型；对照全量目录还原，未知 fail-closed。
    pub(crate) fn dispatch_settings_subagent_control(
        &mut self,
        identifier: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(control) = parse_settings_subagent_control(identifier) else {
            return;
        };
        let escaped = match &control {
            SettingsSubagentControl::ToggleSpawn(escaped)
            | SettingsSubagentControl::ToggleDelegate(escaped)
            | SettingsSubagentControl::TogglePermission(_, escaped)
            | SettingsSubagentControl::CycleEffort(escaped)
            | SettingsSubagentControl::ToggleEffort(_, escaped)
            | SettingsSubagentControl::Reset(escaped) => escaped,
        };
        // 与 render 同源：只在已启用模型上解析目标（ADR-063）。
        let catalog = crate::projection::subagent_rule_models(
            &self.projection.settings_providers.model_catalog,
        );
        let Some((provider_id, model_id)) = settings_subagent_target_for_escaped(&catalog, escaped)
        else {
            return;
        };
        match control {
            SettingsSubagentControl::ToggleSpawn(_) => {
                self.on_settings_subagents_toggle_spawn(provider_id, model_id, cx)
            }
            SettingsSubagentControl::ToggleDelegate(_) => {
                self.on_settings_subagents_toggle_delegate(provider_id, model_id, cx)
            }
            SettingsSubagentControl::TogglePermission(permission, _) => {
                self.on_settings_subagents_toggle_permission(provider_id, model_id, permission, cx)
            }
            SettingsSubagentControl::CycleEffort(_) => {
                self.on_settings_subagents_cycle_effort(provider_id, model_id, cx)
            }
            SettingsSubagentControl::ToggleEffort(level, _) => {
                self.on_settings_subagents_toggle_effort(provider_id, model_id, level, cx)
            }
            SettingsSubagentControl::Reset(_) => {
                self.on_settings_subagents_reset(provider_id, model_id, cx)
            }
        }
    }

    /// 全态写公共路径：clone 权威配置改一处 → 置在途 → 回传 Host；
    /// 回执 / 失败收敛，不乐观更新。
    fn commit_subagent_settings(
        &mut self,
        mutate: impl FnOnce(&mut SubagentSettingsData),
        cx: &mut Context<Self>,
    ) {
        if !self.settings_subagents_writes_enabled() {
            return;
        }
        let mut next = self.projection.settings_subagents.settings.clone();
        mutate(&mut next);
        self.projection.settings_subagents.write_pending = true;
        self.controller.set_subagent_settings(next);
        cx.notify();
    }

    /// 全局开关（即时全态写）。
    pub(crate) fn on_settings_subagents_toggle_enabled(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.projection.settings_subagents.settings.enabled;
        self.commit_subagent_settings(move |settings| settings.enabled = enabled, cx);
    }

    /// 并发上限 Save（输入解析同源；畸形 / 越界 fail-closed 不发）。
    pub(crate) fn on_settings_subagents_save_concurrency(&mut self, cx: &mut Context<Self>) {
        let Some(value) = parse_subagent_max_concurrent(
            self.settings_subagents_concurrency_input.read(cx).text(),
        ) else {
            return;
        };
        self.commit_subagent_settings(move |settings| settings.max_concurrent = value, cx);
    }

    /// RV-08：并发输入错误文案（render 与 AX 同源）：非空且解析失败
    /// （畸形 / 越界）返回本地化原因；空输入按「待输入」处理，只显示
    /// 范围提示。逐帧从输入文本推导，无持久错误状态。
    pub(crate) fn subagent_concurrency_error(text: &str) -> Option<&'static str> {
        if text.trim().is_empty() || parse_subagent_max_concurrent(text).is_some() {
            None
        } else {
            Some(t("settings.subagents.concurrency_error"))
        }
    }

    /// 「可发起子代理」开关（无显式规则时回落默认值再翻转）。
    pub(crate) fn on_settings_subagents_toggle_spawn(
        &mut self,
        provider_id: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        self.commit_subagent_settings(
            move |settings| {
                let rule = upsert_subagent_rule(settings, &provider_id, &model_id);
                rule.allow_spawn = !rule.allow_spawn;
            },
            cx,
        );
    }

    /// 「可作为子代理」开关。
    pub(crate) fn on_settings_subagents_toggle_delegate(
        &mut self,
        provider_id: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        self.commit_subagent_settings(
            move |settings| {
                let rule = upsert_subagent_rule(settings, &provider_id, &model_id);
                rule.allow_as_subagent = !rule.allow_as_subagent;
            },
            cx,
        );
    }

    /// 单项功能权限 chip：已授予则移除，未授予则加入。
    pub(crate) fn on_settings_subagents_toggle_permission(
        &mut self,
        provider_id: String,
        model_id: String,
        permission: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.commit_subagent_settings(
            move |settings| {
                let rule = upsert_subagent_rule(settings, &provider_id, &model_id);
                if let Some(index) = rule.permissions.iter().position(|p| p == permission) {
                    rule.permissions.remove(index);
                } else {
                    rule.permissions.push(permission.to_string());
                }
            },
            cx,
        );
    }

    /// 默认强度 cycle（ADR-063）：候选 = 规则 allowed_efforts（非空）否则
    /// 模型生效范围；None（自动）→ 首个候选 → 依次 → None。模型不在已
    /// 启用目录时 fail-closed 不写。
    pub(crate) fn on_settings_subagents_cycle_effort(
        &mut self,
        provider_id: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        let catalog = crate::projection::subagent_rule_models(
            &self.projection.settings_providers.model_catalog,
        );
        let Some(model) = catalog
            .iter()
            .find(|model| model.provider_id == provider_id && model.id == model_id)
        else {
            return;
        };
        let rule = self
            .projection
            .settings_subagents
            .effective_rule(&provider_id, &model_id);
        let candidates: Vec<String> = if rule.allowed_efforts.is_empty() {
            model.effort_options()
        } else {
            rule.allowed_efforts.clone()
        };
        if candidates.is_empty() {
            return;
        }
        let next = match &rule.default_effort {
            None => Some(candidates[0].clone()),
            Some(current) => match candidates.iter().position(|level| level == current) {
                Some(ix) if ix + 1 < candidates.len() => Some(candidates[ix + 1].clone()),
                // 不在候选内（或已是末位）→ 回 None（自动）。
                _ => None,
            },
        };
        self.commit_subagent_settings(
            move |settings| {
                let rule = upsert_subagent_rule(settings, &provider_id, &model_id);
                rule.default_effort = next;
            },
            cx,
        );
    }

    /// 可选范围 chip 切换（ADR-063）：移除当前默认强度所选项时一并清
    /// 默认（Host 校验 default ∈ allowed，UI 先行保持一致）。
    pub(crate) fn on_settings_subagents_toggle_effort(
        &mut self,
        provider_id: String,
        model_id: String,
        level: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.commit_subagent_settings(
            move |settings| {
                let rule = upsert_subagent_rule(settings, &provider_id, &model_id);
                if let Some(index) = rule.allowed_efforts.iter().position(|l| l == level) {
                    rule.allowed_efforts.remove(index);
                    if rule.default_effort.as_deref() == Some(level) {
                        rule.default_effort = None;
                    }
                } else {
                    rule.allowed_efforts.push(level.to_string());
                }
            },
            cx,
        );
    }

    /// 「重置为默认」：移除显式规则，回落默认（允许发起 / 可作为子代理 /
    /// 全量功能权限）。
    pub(crate) fn on_settings_subagents_reset(
        &mut self,
        provider_id: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        self.commit_subagent_settings(
            move |settings| {
                settings
                    .models
                    .retain(|rule| !(rule.provider_id == provider_id && rule.model_id == model_id));
            },
            cx,
        );
    }
}

/// 写路径辅助：取显式规则；无则追加一条默认规则（其余字段保持不动）。
fn upsert_subagent_rule<'a>(
    settings: &'a mut SubagentSettingsData,
    provider_id: &str,
    model_id: &str,
) -> &'a mut SubagentModelRule {
    if let Some(index) = settings
        .models
        .iter()
        .position(|rule| rule.provider_id == provider_id && rule.model_id == model_id)
    {
        return &mut settings.models[index];
    }
    settings.models.push(SubagentModelRule {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
        ..SubagentModelRule::default()
    });
    let last = settings.models.len() - 1;
    &mut settings.models[last]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RV-08 主路径：错误推导合同——空输入按「待输入」不算错误，合法
    /// 值（含边界与两端空白）无错误，畸形 / 越界给出原因；文案内容属
    /// i18n 层，这里只断言语义。
    #[test]
    fn subagent_concurrency_error_requires_non_empty_invalid_input() {
        assert_eq!(AppView::subagent_concurrency_error(""), None);
        assert_eq!(AppView::subagent_concurrency_error("   "), None);
        assert_eq!(AppView::subagent_concurrency_error(" 1 "), None);
        assert_eq!(AppView::subagent_concurrency_error("16"), None);
        assert!(AppView::subagent_concurrency_error("0").is_some());
        assert!(AppView::subagent_concurrency_error("17").is_some());
        assert!(AppView::subagent_concurrency_error("4.5").is_some());
        assert!(AppView::subagent_concurrency_error("abc").is_some());
    }
}
