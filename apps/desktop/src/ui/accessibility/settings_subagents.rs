//! Settings 子代理页 AX：identifier / Press gate / 几何与 render 同源。

use gpui::{App, Focusable, Window};

use super::{AxAction, AxNode, AxRect, AxRole};
use crate::projection::{group_models_by_provider, ConnectionState};
use crate::ui::i18n::t;
use crate::ui::settings::{
    parse_subagent_max_concurrent, settings_subagent_identifier, subagent_permission_label,
    subagents_status_lines, SUBAGENT_PERMISSIONS,
};
use crate::ui::AppView;

impl AppView {
    /// 「子代理」页 AX：全局开关 / 并发输入 + Save（Press）+ 生效边界 +
    /// 每模型规则卡（spawn / delegate 开关、7 项权限 chip、重置）。stale
    /// 时 enabled=false 且 permits 拒绝写动作，与 render 同 gate。
    pub(crate) fn settings_subagents_page_ax(
        &self,
        window: &Window,
        cx: &App,
        frame: AxRect,
    ) -> AxNode {
        let state = &self.projection.settings_subagents;
        let writes = self.settings_subagents_writes_enabled();
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let concurrency_value = parse_subagent_max_concurrent(
            self.settings_subagents_concurrency_input.read(cx).text(),
        );
        let save_enabled = writes && concurrency_value.is_some();
        let refresh_focused =
            self.open_menu.is_none() && self.settings_refresh_focus.is_focused(window);
        let save_focused =
            self.open_menu.is_none() && self.settings_subagents_save_focus.is_focused(window);
        let concurrency_focused = self.open_menu.is_none()
            && self
                .settings_subagents_concurrency_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);

        let mut page = AxNode::new(
            "settings-page",
            AxRole::Group,
            t("settings.subagents.title"),
            frame,
        )
        .child(
            AxNode::new(
                "settings-page-title",
                AxRole::StaticText,
                t("settings.subagents.title"),
                self.settings_element_bounds("settings-page-title"),
            )
            .value(t("settings.subagents.subtitle")),
        )
        .child(
            AxNode::new(
                "settings-refresh",
                AxRole::Button,
                t("settings.refresh"),
                self.settings_element_bounds("settings-refresh"),
            )
            .enabled(connected)
            .focused(refresh_focused)
            .action(AxAction::Press),
        );

        for (kind, label) in subagents_status_lines(state) {
            page = page.child(
                AxNode::new(
                    format!("settings-status-{kind}"),
                    AxRole::StaticText,
                    t("settings.subagents.ax_status"),
                    self.settings_element_bounds(&format!("settings-status-{kind}")),
                )
                .value(label),
            );
        }

        let enabled_focused = self
            .settings_action_focus
            .get("settings-subagents-enabled")
            .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
        page = page
            .child(
                AxNode::new(
                    "settings-subagents-enabled-text",
                    AxRole::StaticText,
                    t("settings.subagents.ax_enabled"),
                    self.settings_element_bounds("settings-subagents-enabled-text"),
                )
                .value(format!(
                    "{} · {}",
                    t("settings.subagents.enabled_label"),
                    t("settings.subagents.enabled_note")
                )),
            )
            .child(
                AxNode::new(
                    "settings-subagents-enabled",
                    AxRole::Button,
                    t("settings.subagents.enabled_label"),
                    self.settings_element_bounds("settings-subagents-enabled"),
                )
                .enabled(writes)
                .focused(enabled_focused)
                .selected(state.settings.enabled)
                .value(if state.settings.enabled {
                    t("settings.subagents.switch_on")
                } else {
                    t("settings.subagents.switch_off")
                })
                .action(AxAction::Press),
            )
            .child(
                AxNode::new(
                    "settings-subagents-concurrency-current",
                    AxRole::StaticText,
                    t("settings.subagents.ax_concurrency"),
                    self.settings_element_bounds("settings-subagents-concurrency-current"),
                )
                .value(
                    t("settings.subagents.concurrency_current")
                        .replace("{}", &state.settings.max_concurrent.to_string()),
                ),
            )
            .child(
                AxNode::new(
                    "settings-subagents-concurrency-input",
                    AxRole::TextArea,
                    t("settings.subagents.concurrency_label"),
                    self.settings_element_bounds("settings-subagents-concurrency-input"),
                )
                .value(
                    self.settings_subagents_concurrency_input
                        .read(cx)
                        .text()
                        .to_string(),
                )
                .enabled(writes)
                .focused(concurrency_focused)
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
            )
            .child(
                AxNode::new(
                    "settings-subagents-save",
                    AxRole::Button,
                    t("settings.save"),
                    self.settings_element_bounds("settings-subagents-save"),
                )
                .enabled(save_enabled)
                .focused(save_focused)
                .action(AxAction::Press),
            )
            .child(
                AxNode::new(
                    "settings-subagents-effect",
                    AxRole::StaticText,
                    t("settings.subagents.ax_effect"),
                    self.settings_element_bounds("settings-subagents-effect"),
                )
                .value(t("settings.subagents.effect_note")),
            );

        // 与 render 同源：只发布已启用模型的规则卡（ADR-063）。
        let catalog =
            crate::projection::subagent_rule_models(&self.projection.settings_providers.model_catalog);
        if catalog.is_empty() {
            return page.child(
                AxNode::new(
                    "settings-subagents-models-empty",
                    AxRole::StaticText,
                    t("settings.subagents.ax_models"),
                    self.settings_element_bounds("settings-subagents-models-empty"),
                )
                .value(t("settings.subagents.models_empty")),
            );
        }
        for (_provider_id, models) in group_models_by_provider(&catalog) {
            for model in &models {
                let provider_id = &model.provider_id;
                let model_id = &model.id;
                let rule = state.effective_rule(provider_id, model_id);
                let has_rule = state.has_rule(provider_id, model_id);
                let name_id = super::dynamic_identifier(
                    "settings-subagent-model-name",
                    &format!("{provider_id}:{model_id}"),
                );
                let name_id_bounds = name_id.clone();
                let mut card = AxNode::new(
                    settings_subagent_identifier("model", provider_id, model_id),
                    AxRole::Group,
                    t("settings.subagents.ax_models"),
                    self.settings_element_bounds(&settings_subagent_identifier(
                        "model",
                        provider_id,
                        model_id,
                    )),
                )
                .child(
                    AxNode::new(
                        name_id,
                        AxRole::StaticText,
                        &model.display_name,
                        self.settings_element_bounds(&name_id_bounds),
                    )
                    // 能力徽标与 render 同源（图像 / 搜索）。
                    .description({
                        let mut detail = format!("{provider_id}/{model_id}");
                        if model.image_input {
                            detail.push_str(&format!(
                                " · {}",
                                t("settings.subagents.capability_image")
                            ));
                        }
                        if model.web_search {
                            detail.push_str(&format!(
                                " · {}",
                                t("settings.subagents.capability_search")
                            ));
                        }
                        detail
                    })
                    .value(if has_rule {
                        t("settings.subagents.custom_badge")
                    } else {
                        t("settings.subagents.default_badge")
                    }),
                );
                card = card
                    .child(self.subagent_switch_ax(
                        &settings_subagent_identifier("spawn", provider_id, model_id),
                        t("settings.subagents.allow_spawn"),
                        rule.allow_spawn,
                        writes,
                        window,
                    ))
                    .child(self.subagent_switch_ax(
                        &settings_subagent_identifier("delegate", provider_id, model_id),
                        t("settings.subagents.allow_delegate"),
                        rule.allow_as_subagent,
                        writes,
                        window,
                    ));
                for permission in SUBAGENT_PERMISSIONS {
                    let granted = rule.permissions.iter().any(|p| p == permission);
                    let chip_id = settings_subagent_identifier(
                        &format!("perm-{permission}"),
                        provider_id,
                        model_id,
                    );
                    let chip_focused = self
                        .settings_action_focus
                        .get(&chip_id)
                        .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                    card = card.child(
                        AxNode::new(
                            chip_id,
                            AxRole::Button,
                            subagent_permission_label(permission),
                            self.settings_element_bounds(&settings_subagent_identifier(
                                &format!("perm-{permission}"),
                                provider_id,
                                model_id,
                            )),
                        )
                        .enabled(writes)
                        .focused(chip_focused)
                        .selected(granted)
                        .action(AxAction::Press),
                    );
                }
                // ADR-063：默认强度 cycle 按钮 + 可选范围 chips（与 render
                // 同源 identifier，Button + selected 表达选中态）。
                let default_id =
                    settings_subagent_identifier("effort-default", provider_id, model_id);
                let default_focused = self
                    .settings_action_focus
                    .get(&default_id)
                    .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                card = card.child(
                    AxNode::new(
                        default_id.clone(),
                        AxRole::Button,
                        t("settings.subagents.effort_default"),
                        self.settings_element_bounds(&default_id),
                    )
                    .enabled(writes)
                    .focused(default_focused)
                    .value(
                        rule.default_effort
                            .clone()
                            .unwrap_or_else(|| t("settings.subagents.effort_auto").to_string()),
                    )
                    .action(AxAction::Press),
                );
                for level in model.effort_options() {
                    let selected = rule.allowed_efforts.iter().any(|l| l == &level);
                    let chip_id = settings_subagent_identifier(
                        &format!("effort-{level}"),
                        provider_id,
                        model_id,
                    );
                    let chip_focused = self
                        .settings_action_focus
                        .get(&chip_id)
                        .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                    card = card.child(
                        AxNode::new(
                            chip_id.clone(),
                            AxRole::Button,
                            t("settings.subagents.effort_allowed"),
                            self.settings_element_bounds(&chip_id),
                        )
                        .enabled(writes)
                        .focused(chip_focused)
                        .selected(selected)
                        .value(level.clone())
                        .action(AxAction::Press),
                    );
                }
                if has_rule {
                    let reset_id = settings_subagent_identifier("reset", provider_id, model_id);
                    let reset_focused = self
                        .settings_action_focus
                        .get(&reset_id)
                        .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                    card = card.child(
                        AxNode::new(
                            reset_id,
                            AxRole::Button,
                            t("settings.subagents.reset"),
                            self.settings_element_bounds(&settings_subagent_identifier(
                                "reset",
                                provider_id,
                                model_id,
                            )),
                        )
                        .enabled(writes)
                        .focused(reset_focused)
                        .action(AxAction::Press),
                    );
                }
                page = page.child(card);
            }
        }
        page
    }

    /// 子代理开关 / 权限 chip 的 AX 节点（Button + selected 表达开关态）。
    fn subagent_switch_ax(
        &self,
        id: &str,
        label: &str,
        selected: bool,
        writes: bool,
        window: &Window,
    ) -> AxNode {
        let focused = self
            .settings_action_focus
            .get(id)
            .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
        AxNode::new(id, AxRole::Button, label, self.settings_element_bounds(id))
            .enabled(writes)
            .focused(focused)
            .selected(selected)
            .value(if selected {
                t("settings.subagents.switch_on")
            } else {
                t("settings.subagents.switch_off")
            })
            .action(AxAction::Press)
    }
}
