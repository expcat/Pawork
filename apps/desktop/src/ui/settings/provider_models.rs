//! 供应商 Manage models 弹层与推理强度。
//!
//! 临时 Host 目录测试跟在本模块底部，不进生产路径。

use gpui::{Focusable, Point, SharedString, Window};

use super::*;
use crate::ui::components::dropdown::MenuPanel;
use crate::ui::components::switch::Switch;
use crate::ui::MenuKind;

impl AppView {
    /// 「Manage models」弹层入口 gate（render / 键盘 / AX 三路径同源）：
    /// 写总闸之上要求 provider 已连接且目录可用——未连接先走认证流程，
    /// 目录不可用无全量可谈（ADR-055 D2）。
    pub(crate) fn settings_manage_models_enabled(
        &self,
        provider: &ProviderAuthStatusEntry,
    ) -> bool {
        self.settings_writes_enabled()
            && matches!(provider.auth, ProviderAuthState::Connected { .. })
            && !matches!(
                provider.catalog,
                crate::projection::ProviderCatalogState::Unavailable { .. }
            )
    }

    /// 「Manage models」弹层触发器 toggle（三路径同源；入口级复核 gate）。
    /// 打开时重查全量目录（include_disabled=true）取权威状态。
    pub(crate) fn on_toggle_settings_models_menu(
        &mut self,
        provider_id: String,
        down_position: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let enabled = self
            .projection
            .settings_providers
            .providers
            .iter()
            .any(|entry| {
                entry.provider_id == provider_id && self.settings_manage_models_enabled(entry)
            });
        if !enabled {
            return;
        }
        self.controller.load_model_catalog();
        self.toggle_menu(
            MenuKind::SettingsProviderModels(provider_id),
            down_position,
            cx,
        );
        if matches!(self.open_menu, Some(MenuKind::SettingsProviderModels(_))) {
            self.focus_model_search(window, cx);
        }
    }

    /// 搜索结果是 render、方向键和 AX 的同一份目录顺序。
    pub(crate) fn settings_filtered_models(&self, provider_id: &str) -> Vec<ModelEntry> {
        self.projection
            .settings_providers
            .model_catalog
            .iter()
            .filter(|model| model.provider_id == provider_id && self.model_matches_search(model))
            .cloned()
            .collect()
    }

    /// 方向键移动真实 Switch 焦点，Enter 仍由同一 Switch handler 激活。
    /// 根键盘入口需在普通 MenuRow 分派前调用。
    pub(crate) fn handle_settings_models_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(MenuKind::SettingsProviderModels(provider_id)) = self.open_menu.clone() else {
            return false;
        };
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.modified() || !matches!(key, "up" | "down" | "enter") {
            return false;
        }
        if self.model_search_input.read(cx).is_composing() {
            return false;
        }
        let models = self.settings_filtered_models(&provider_id);
        let current = models.iter().position(|model| {
            self.settings_action_focus
                .get(&settings_model_switch_identifier(&provider_id, &model.id))
                .is_some_and(|focus| focus.is_focused(window))
        });
        if key == "enter" {
            if current.is_none()
                && !self
                    .model_search_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            {
                return false;
            }
            // 正常 Switch Enter 已在行级消费；此臂覆盖搜索输入和根动作分派。
            if let Some(model) = current
                .and_then(|ix| models.get(ix))
                .or_else(|| models.first())
            {
                self.on_toggle_provider_model(provider_id, model.id.clone(), cx);
            }
        } else if !models.is_empty() {
            let next = match (current, key) {
                (Some(ix), "down") => (ix + 1) % models.len(),
                (Some(ix), _) => (ix + models.len() - 1) % models.len(),
                (None, "down") => 0,
                (None, _) => models.len() - 1,
            };
            let id = settings_model_switch_identifier(&provider_id, &models[next].id);
            if let Some(focus) = self.settings_action_focus.get(&id) {
                window.focus(focus);
            }
            if let Some(scroll) = self.settings_element_layouts.get(&format!(
                "{}-list",
                settings_models_menu_identifier(&provider_id)
            )) {
                scroll.scroll_to_item(next);
            }
        }
        cx.notify();
        cx.stop_propagation();
        true
    }

    /// 推理强度控件统一派发（click / 键盘 / AX 三路径同源；ADR-063）：
    /// identifier 解析出动作与目标模型；对照全量目录还原，未知
    /// fail-closed。非强度控件返回 false，由调用方走既有路径。
    pub(crate) fn dispatch_settings_models_control(
        &mut self,
        identifier: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        match parse_settings_models_control(identifier) {
            Some(SettingsModelsControl::EffortDefault(escaped)) => {
                if let Some((provider_id, model_id)) =
                    self.settings_model_target_for_escaped(&escaped)
                {
                    self.on_settings_model_effort_default(provider_id, model_id, cx);
                }
                true
            }
            Some(SettingsModelsControl::EffortToggle(level, escaped)) => {
                if let Some((provider_id, model_id)) =
                    self.settings_model_target_for_escaped(&escaped)
                {
                    self.on_settings_model_effort_toggle(provider_id, model_id, level, cx);
                }
                true
            }
            Some(SettingsModelsControl::EffortReset(escaped)) => {
                if let Some((provider_id, model_id)) =
                    self.settings_model_target_for_escaped(&escaped)
                {
                    self.on_settings_model_effort_reset(provider_id, model_id, cx);
                }
                true
            }
            _ => false,
        }
    }

    /// set_model_reasoning 写 gate（render / 键盘 / AX 同源）：设置可写、
    /// 弹层开在该 provider、无在途写、目标在全量目录内。
    fn settings_model_reasoning_enabled(&self, provider_id: &str, model_id: &str) -> bool {
        self.settings_writes_enabled()
            && matches!(&self.open_menu, Some(MenuKind::SettingsProviderModels(open)) if open == provider_id)
            && !self
                .projection
                .settings_providers
                .model_write_pending
                .as_ref()
                .is_some_and(|pending| pending.targets(provider_id))
            && self
                .projection
                .settings_providers
                .model_catalog
                .iter()
                .any(|model| model.provider_id == provider_id && model.id == model_id)
    }

    /// set_model_reasoning 全态写公共路径：置在途 → 派出；回执 /
    /// 失败收敛，不乐观更新。
    fn commit_model_reasoning(
        &mut self,
        provider_id: String,
        model_id: String,
        default_effort: Option<String>,
        manual_efforts: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_model_reasoning_enabled(&provider_id, &model_id) {
            return;
        }
        self.projection.settings_providers.model_write_pending =
            Some(crate::projection::ProviderModelWrite::Reasoning {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
            });
        self.controller
            .set_model_reasoning(provider_id, model_id, default_effort, manual_efforts);
        cx.notify();
    }

    /// 默认强度 cycle（ADR-063）：候选 = 生效范围（手动 > 目录），未知 =
    /// 全量词汇；None（自动）→ 首个候选 → 依次 → None。
    pub(crate) fn on_settings_model_effort_default(
        &mut self,
        provider_id: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self
            .projection
            .settings_providers
            .model_catalog
            .iter()
            .find(|model| model.provider_id == provider_id && model.id == model_id)
            .cloned()
        else {
            return;
        };
        let candidates = model.effort_options();
        if candidates.is_empty() {
            return;
        }
        let next_default = match &model.default_effort {
            None => Some(candidates[0].clone()),
            Some(current) => match candidates.iter().position(|level| level == current) {
                Some(ix) if ix + 1 < candidates.len() => Some(candidates[ix + 1].clone()),
                _ => None,
            },
        };
        self.commit_model_reasoning(
            provider_id,
            model_id,
            next_default,
            model.manual_efforts.clone(),
            cx,
        );
    }

    /// 手动范围 chip 切换（ADR-063）：在生效范围（手动 > 目录，皆无 =
    /// 空集）上翻转该项并写为手动声明；翻转后默认强度不再落在范围内时
    /// 一并清默认（Host 校验 default ∈ 范围，UI 先行保持一致）。
    pub(crate) fn on_settings_model_effort_toggle(
        &mut self,
        provider_id: String,
        model_id: String,
        level: &'static str,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self
            .projection
            .settings_providers
            .model_catalog
            .iter()
            .find(|model| model.provider_id == provider_id && model.id == model_id)
            .cloned()
        else {
            return;
        };
        let mut manual = model
            .effective_efforts()
            .map(|efforts| efforts.to_vec())
            .unwrap_or_default();
        if let Some(index) = manual.iter().position(|l| l == level) {
            manual.remove(index);
        } else {
            manual.push(level.to_string());
        }
        // 以 canonical 词汇序稳定写回（与目录声明顺序无关）。
        manual.sort_by_key(|l| {
            crate::projection::EFFORT_LEVELS
                .iter()
                .position(|known| known == l)
                .unwrap_or(usize::MAX)
        });
        let default = model
            .default_effort
            .clone()
            .filter(|current| manual.iter().any(|l| l == current));
        self.commit_model_reasoning(provider_id, model_id, default, Some(manual), cx);
    }

    /// 清除手动范围（ADR-063）：supported_efforts 回落 None；默认强度仍
    /// 落在目录声明内才保留，否则一并清除（Host 按目录校验）。
    pub(crate) fn on_settings_model_effort_reset(
        &mut self,
        provider_id: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self
            .projection
            .settings_providers
            .model_catalog
            .iter()
            .find(|model| model.provider_id == provider_id && model.id == model_id)
            .cloned()
        else {
            return;
        };
        let default = model.default_effort.clone().filter(|current| {
            model
                .catalog_efforts
                .as_ref()
                .is_none_or(|catalog| catalog.iter().any(|l| l == current))
        });
        self.commit_model_reasoning(provider_id, model_id, default, None, cx);
    }

    /// 弹层内单模型 Switch 切换（三路径同源；入口级复核 gate 与全量目录
    /// 条目，未知 pair fail-closed）。不乐观改状态：Switch 以 Host 回执
    /// 为准翻转，并本地对齐 Composer，不再重探全通道目录。
    pub(crate) fn on_toggle_provider_model(
        &mut self,
        provider_id: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_writes_enabled() {
            return;
        }
        if !matches!(&self.open_menu, Some(MenuKind::SettingsProviderModels(open)) if open == &provider_id)
        {
            return;
        }
        if self
            .projection
            .settings_providers
            .model_write_pending
            .is_some()
        {
            return;
        }
        let Some(entry) = self
            .projection
            .settings_providers
            .model_catalog
            .iter()
            .find(|model| model.provider_id == provider_id && model.id == model_id)
        else {
            return;
        };
        let target = !entry.enabled;
        self.projection.settings_providers.model_write_pending =
            Some(crate::projection::ProviderModelWrite::Model {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
            });
        self.controller
            .set_model_enabled(provider_id, model_id, target);
        cx.notify();
    }

    /// 弹层单模型推理强度行（ADR-063）：来源徽标（目录 / 手动 / 未知）
    /// + 默认强度 cycle 按钮 + 范围 chips；有手动声明时附「重置范围」。
    /// render 与 AX 同源：controls 的 identifier 即 AX 节点 id。
    fn settings_model_effort_row(
        &mut self,
        provider_id: &str,
        model: &ModelEntry,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let model_id = model.id.clone();
        let source = if model.manual_efforts.is_some() {
            t("settings.providers.effort_source_manual")
        } else if model.catalog_efforts.is_some() {
            t("settings.providers.effort_source_catalog")
        } else {
            t("settings.providers.effort_source_unknown")
        };
        // chips 选中态 = 生效范围（手动 > 目录）；皆无 = 全不选（不限）。
        let selected: Vec<String> = model
            .effective_efforts()
            .map(|efforts| efforts.to_vec())
            .unwrap_or_default();
        let mut row = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(
                Label::new(t("settings.providers.effort_label"))
                    .size(font::XS)
                    .color(dark().text.secondary),
            )
            .child(
                div()
                    .flex_none()
                    .px_1()
                    .rounded(px(4.0))
                    .bg(dark().surface.raised)
                    .child(
                        Label::new(source)
                            .size(font::XS)
                            .color(dark().text.tertiary),
                    ),
            );
        // 默认强度 cycle 按钮。
        let default_id = settings_model_effort_default_identifier(provider_id, &model_id);
        let default_focus = self
            .settings_action_focus
            .entry(default_id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let default_label = model
            .default_effort
            .clone()
            .unwrap_or_else(|| t("settings.providers.effort_default_auto").to_string());
        let default_click_id = default_id.clone();
        let default_activate_id = default_id.clone();
        let default_button = Button::new(default_id.clone())
            .track_focus(&default_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_MODEL_EFFORT_CHIP_HEIGHT))
            .radius(6.0)
            .text_size(font::XS)
            .label(default_label)
            .tooltip(t("settings.providers.effort_default_tooltip"))
            .disabled(!enabled)
            .on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_button_key_click(&default_click_id, event) {
                    return;
                }
                view.dispatch_settings_models_control(&default_click_id, cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_button_key_activate(&default_activate_id);
                view.dispatch_settings_models_control(&default_activate_id, cx);
                cx.stop_propagation();
            }));
        row = row.child(
            self.settings_element(default_id)
                .flex_none()
                .child(default_button),
        );
        // 范围 chips。
        for level in model.effort_options() {
            let selected_chip = selected.iter().any(|l| l == &level);
            let chip_id = settings_model_effort_chip_identifier(&level, provider_id, &model_id);
            let focus = self
                .settings_action_focus
                .entry(chip_id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let click_id = chip_id.clone();
            let activate_id = chip_id.clone();
            let chip = Button::new(chip_id.clone())
                .track_focus(&focus)
                .variant(ButtonVariant::Raised)
                .height(px(SETTINGS_MODEL_EFFORT_CHIP_HEIGHT))
                .radius(6.0)
                .text_size(font::XS)
                .label(level.clone())
                .text_color(if selected_chip {
                    dark().accent.primary
                } else {
                    dark().text.secondary
                })
                .tooltip(t("settings.providers.effort_range_tooltip"))
                .disabled(!enabled)
                .on_click(cx.listener(move |view, event, _window, cx| {
                    if view.consume_button_key_click(&click_id, event) {
                        return;
                    }
                    view.dispatch_settings_models_control(&click_id, cx);
                }))
                .on_activate(cx.listener(move |view, _event, _window, cx| {
                    view.note_button_key_activate(&activate_id);
                    view.dispatch_settings_models_control(&activate_id, cx);
                    cx.stop_propagation();
                }));
            row = row.child(self.settings_element(chip_id).flex_none().child(chip));
        }
        // 手动范围存在时提供「重置范围」（清手动声明，回落目录 / 不限）。
        if model.manual_efforts.is_some() {
            let reset_id = settings_model_effort_reset_identifier(provider_id, &model_id);
            let reset_focus = self
                .settings_action_focus
                .entry(reset_id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let reset_click_id = reset_id.clone();
            let reset_activate_id = reset_id.clone();
            let reset = Button::new(reset_id.clone())
                .track_focus(&reset_focus)
                .variant(ButtonVariant::Ghost)
                .height(px(SETTINGS_MODEL_EFFORT_CHIP_HEIGHT))
                .radius(6.0)
                .text_size(font::XS)
                .label(t("settings.subagents.reset"))
                .tooltip(t("settings.providers.effort_range_tooltip"))
                .disabled(!enabled)
                .on_click(cx.listener(move |view, event, _window, cx| {
                    if view.consume_button_key_click(&reset_click_id, event) {
                        return;
                    }
                    view.dispatch_settings_models_control(&reset_click_id, cx);
                }))
                .on_activate(cx.listener(move |view, _event, _window, cx| {
                    view.note_button_key_activate(&reset_activate_id);
                    view.dispatch_settings_models_control(&reset_activate_id, cx);
                    cx.stop_propagation();
                }));
            row = row.child(self.settings_element(reset_id).flex_none().child(reset));
        }
        row.into_any_element()
    }

    /// 弹层 Enable all / Disable all（三路径同源；空目录禁用——Host 全关
    /// 对空目录 fail-closed catalog_unavailable，UI 不发即防错；写在途
    /// 禁用防重复提交）。
    pub(crate) fn on_toggle_provider_models_all(
        &mut self,
        provider_id: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_writes_enabled() {
            return;
        }
        if !matches!(&self.open_menu, Some(MenuKind::SettingsProviderModels(open)) if open == &provider_id)
        {
            return;
        }
        if self
            .projection
            .settings_providers
            .model_write_pending
            .is_some()
        {
            return;
        }
        let has_models = self
            .projection
            .settings_providers
            .model_catalog
            .iter()
            .any(|model| model.provider_id == provider_id);
        if !has_models {
            return;
        }
        self.projection.settings_providers.model_write_pending =
            Some(crate::projection::ProviderModelWrite::All {
                provider_id: provider_id.clone(),
            });
        self.controller
            .set_provider_models_enabled(provider_id, enabled);
        cx.notify();
    }

    /// 「Manage models」弹层面板（OPT-3a / ADR-055 D2/D3）：标题 +
    /// Enable all / Disable all + 每模型一行 Switch；空目录给诚实空态 +
    /// Refresh catalog（复用页级刷新路径）；写在途禁用对应控件。外点
    /// 关闭 / Escape 与 MenuKind 语义同源。
    pub(super) fn settings_models_menu_element(
        &mut self,
        provider_id: &str,
        cx: &mut Context<Self>,
    ) -> MenuPanel {
        let writes = self.settings_writes_enabled();
        let pending = self
            .projection
            .settings_providers
            .model_write_pending
            .as_ref()
            .is_some_and(|pending| pending.targets(provider_id));
        let models: Vec<ModelEntry> = self
            .projection
            .settings_providers
            .model_catalog
            .iter()
            .filter(|model| model.provider_id == provider_id)
            .cloned()
            .collect();
        let filtered_models = self.settings_filtered_models(provider_id);
        let dismiss_provider = provider_id.to_string();
        let menu_id = settings_models_menu_identifier(provider_id);
        let menu_scroll = self
            .settings_element_layouts
            .entry(menu_id.clone())
            .or_default()
            .clone();
        let list_id = format!("{menu_id}-list");
        let list_scroll = self
            .settings_element_layouts
            .entry(list_id.clone())
            .or_default()
            .clone();
        let panel = MenuPanel::new(SharedString::from(menu_id))
            .track_scroll(&menu_scroll)
            .max_height(SETTINGS_MODELS_MENU_MAX_HEIGHT)
            .dismiss_on_outside(
                cx.listener(move |view, event: &gpui::MouseDownEvent, _, cx| {
                    view.dismiss_menu_on_outside(
                        MenuKind::SettingsProviderModels(dismiss_provider.clone()),
                        event.position,
                        cx,
                    );
                }),
            );
        // 头行：标题 + Enable all / Disable all（空目录两者禁用）。
        let all_enabled = writes && !pending && !models.is_empty();
        let mut header = div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .px_1()
            .py_1()
            .child(
                self.settings_element(format!("settings-models-heading-{provider_id}"))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(font::SM)
                            .text_color(dark().text.primary)
                            .child(format!(
                                "{} · {}",
                                t("settings.providers.models_title"),
                                provider_id
                            )),
                    )
                    .child(
                        div()
                            .text_size(font::XS)
                            .text_color(dark().text.secondary)
                            .child(self.model_provider_status(provider_id)),
                    )
                    .child(settings_copy(crate::ui::i18n::t2(
                        "settings.providers.models_count",
                        &models
                            .iter()
                            .filter(|model| model.enabled)
                            .count()
                            .to_string(),
                        &models.len().to_string(),
                    ))),
            )
            .child(
                self.settings_element(format!("settings-models-scope-{provider_id}"))
                    .text_size(font::XS)
                    .text_color(dark().text.secondary)
                    .child(t("settings.providers.catalog_scope")),
            );
        let mut bulk_actions = div().flex().flex_wrap().gap_2();
        for (identifier, label, target) in [
            (
                settings_models_enable_all_identifier(provider_id),
                t("settings.providers.models_enable_all"),
                true,
            ),
            (
                settings_models_disable_all_identifier(provider_id),
                t("settings.providers.models_disable_all"),
                false,
            ),
        ] {
            let focus = self
                .settings_action_focus
                .entry(identifier.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let click_id = identifier.clone();
            let click_provider = provider_id.to_string();
            let activate_id = identifier.clone();
            let activate_provider = provider_id.to_string();
            let button = Button::new(identifier.clone())
                .track_focus(&focus)
                .variant(ButtonVariant::Raised)
                .height(px(SETTINGS_CONTROL_HEIGHT))
                .vcenter()
                .radius(6.0)
                .bordered()
                .text_size(font::SM)
                .label(label)
                .disabled(!all_enabled)
                .on_click(cx.listener(move |view, event, _window, cx| {
                    if view.consume_button_key_click(&click_id, event) {
                        return;
                    }
                    view.on_toggle_provider_models_all(click_provider.clone(), target, cx);
                }))
                .on_activate(cx.listener(move |view, _event, _window, cx| {
                    view.note_button_key_activate(&activate_id);
                    view.on_toggle_provider_models_all(activate_provider.clone(), target, cx);
                    cx.stop_propagation();
                }));
            bulk_actions =
                bulk_actions.child(self.settings_element(identifier).flex_none().child(button));
        }
        let refresh_id = settings_models_refresh_identifier(provider_id);
        let refresh_focus = self
            .settings_action_focus
            .entry(refresh_id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let refresh_click_id = refresh_id.clone();
        let refresh_activate_id = refresh_id.clone();
        let refresh = Button::new(refresh_id.clone())
            .track_focus(&refresh_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::SM)
            .label(t("settings.providers.models_refresh"))
            .disabled(!writes)
            .on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_button_key_click(&refresh_click_id, event) {
                    return;
                }
                view.on_refresh_settings(cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_button_key_activate(&refresh_activate_id);
                view.on_refresh_settings(cx);
                cx.stop_propagation();
            }));
        bulk_actions =
            bulk_actions.child(self.settings_element(refresh_id).flex_none().child(refresh));
        header = header
            .child(self.model_search_element(cx))
            .child(bulk_actions);
        let mut content = div()
            .w(px(SETTINGS_MODELS_MENU_WIDTH))
            .max_h(px(SETTINGS_MODELS_MENU_MAX_HEIGHT
                - 2.0 * metrics::MENU_PADDING
                - 2.0))
            .flex()
            .flex_col()
            .child(header);
        if models.is_empty() {
            // 空目录诚实空态：Enable / Disable all 已禁用；Refresh 在头行
            // 常驻（R07），空态不再重复画一枚。
            return panel.child(
                content.child(
                    self.settings_element(format!("settings-models-empty-{provider_id}"))
                        .flex()
                        .flex_col()
                        .gap(px(metrics::SPACE_1))
                        .px_2()
                        .py(px(metrics::SPACE_2))
                        .min_w_0()
                        .child(
                            div()
                                .text_size(font::SM)
                                .text_color(dark().text.primary)
                                .child(t("settings.providers.models_empty_title")),
                        )
                        .child(
                            div()
                                .text_size(font::XS)
                                .text_color(dark().text.secondary)
                                .child(t("settings.providers.models_empty_hint")),
                        ),
                ),
            );
        }
        if filtered_models.is_empty() {
            return panel.child(
                content.child(
                    self.settings_element(format!("settings-models-no-results-{provider_id}"))
                        .px_2()
                        .py_3()
                        .text_size(font::SM)
                        .text_color(dark().text.secondary)
                        .child(t("model_search.no_results")),
                ),
            );
        }
        let mut list = div()
            .id(SharedString::from(list_id))
            .track_scroll(&list_scroll)
            .min_h_0()
            .flex_shrink()
            .overflow_y_scroll()
            .on_scroll_wheel(cx.listener(|_view, _event, _window, cx| cx.notify()));
        for model in filtered_models {
            let switch_id = settings_model_switch_identifier(provider_id, &model.id);
            let focus = self
                .settings_action_focus
                .entry(switch_id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let click_id = switch_id.clone();
            let click_provider = provider_id.to_string();
            let click_model = model.id.clone();
            let activate_id = switch_id.clone();
            let activate_provider = provider_id.to_string();
            let activate_model = model.id.clone();
            let row_switch = Switch::new(switch_id.clone())
                .track_focus(&focus)
                .checked(model.enabled)
                .disabled(!writes || pending)
                .tooltip(format!(
                    "{}\n{}/{}",
                    model.display_name, provider_id, model.id
                ))
                .on_click(cx.listener(move |view, event, _window, cx| {
                    if view.consume_button_key_click(&click_id, event) {
                        return;
                    }
                    view.on_toggle_provider_model(click_provider.clone(), click_model.clone(), cx);
                }))
                .on_activate(cx.listener(move |view, _event, _window, cx| {
                    view.note_button_key_activate(&activate_id);
                    view.on_toggle_provider_model(
                        activate_provider.clone(),
                        activate_model.clone(),
                        cx,
                    );
                    cx.stop_propagation();
                }));
            // ADR-063：启用行 + 推理强度行（默认 cycle + 范围 chips +
            // 来源徽标）。写在途 / 只读时禁用。
            let effort_row = model.text.then(|| {
                self.settings_model_effort_row(provider_id, &model, writes && !pending, cx)
            });
            // ADR-064：目录能力徽标（图像 / 生图 / 视频 / 搜索）——目录
            // 透明度，非可交互控件；与 Composer 徽标同一标签源。
            let capability_label = crate::ui::input_area::model_capability_label(&model);
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .min_w_0()
                    .py_2()
                    .border_t_1()
                    .border_color(dark().border.subtle)
                    .px_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .min_h(gpui::rems(2.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!("model-label-{switch_id}")))
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_x_scroll()
                                    .child(
                                        div()
                                            .text_size(font::SM)
                                            .text_color(dark().text.primary)
                                            .child(model.display_name.clone()),
                                    )
                                    .when(model.display_name != model.id, |label| {
                                        label.child(
                                            div()
                                                .text_size(font::XS)
                                                .text_color(dark().text.tertiary)
                                                .child(model.id.clone()),
                                        )
                                    }),
                            )
                            .child(
                                self.settings_element(switch_id)
                                    .flex_none()
                                    .child(row_switch),
                            ),
                    )
                    .when(!capability_label.is_empty(), |row| {
                        row.child(
                            div()
                                .text_size(font::XS)
                                .text_color(dark().text.secondary)
                                .child(capability_label),
                        )
                    })
                    .children(effort_row),
            );
        }
        content = content.child(list);
        panel.child(content)
    }
    /// 转义后的 "<provider>:<model>" → 原始 pair（以全量目录为权威，
    /// 未知 fail-closed；不反解转义）。AX 派发用（OPT-3a 弹层 Switch）。
    pub(crate) fn settings_model_target_for_escaped(
        &self,
        escaped: &str,
    ) -> Option<(String, String)> {
        self.projection
            .settings_providers
            .model_catalog
            .iter()
            .find(|model| {
                dynamic_identifier("", &format!("{}:{}", model.provider_id, model.id))
                    == format!("-{escaped}")
            })
            .map(|model| (model.provider_id.clone(), model.id.clone()))
    }
}

/// AC-02 产品路径用的临时 Host。只在本测试模块里说话，不改生产 API。
/// 搜索、鼠标和键盘断言由同一条回归在下一步接上。
#[cfg(all(test, unix))]
mod model_catalog_host {
    use std::fs;
    use std::io::{self, Read, Write};
    use std::net::Shutdown;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use serde_json::{json, Value};

    #[derive(Clone, Debug, PartialEq)]
    pub(super) struct CapturedCommand {
        pub method: String,
        pub params: Value,
    }

    pub(super) struct ModelCatalogHost {
        socket: PathBuf,
        _dir: tempfile::TempDir,
        commands: Arc<Mutex<Vec<CapturedCommand>>>,
        labels: Arc<Mutex<Vec<String>>>,
        error: Arc<Mutex<Option<String>>>,
        shutdown: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl ModelCatalogHost {
        /// 绑定短寿命 socket 并写同目录 token。返回时 listener 已经在听。
        pub(super) fn spawn() -> io::Result<Self> {
            let dir = tempfile::Builder::new()
                .prefix("pawork-ac02-")
                .tempdir_in("/tmp")?;
            let socket = dir.path().join("pawork-gui-ac02.sock");
            let token = crate::platform::token_path_for_socket(&socket);
            fs::write(&token, b"ac02\n")?;
            let listener = UnixListener::bind(&socket)?;
            listener.set_nonblocking(true)?;

            let commands = Arc::new(Mutex::new(Vec::new()));
            let labels = Arc::new(Mutex::new(Vec::new()));
            let error = Arc::new(Mutex::new(None));
            let shutdown = Arc::new(AtomicBool::new(false));
            let thread_commands = Arc::clone(&commands);
            let thread_labels = Arc::clone(&labels);
            let thread_error = Arc::clone(&error);
            let thread_shutdown = Arc::clone(&shutdown);
            let thread = thread::spawn(move || {
                if let Err(failure) =
                    accept_one(listener, &thread_shutdown, &thread_commands, &thread_labels)
                {
                    *lock(&thread_error) = Some(failure.to_string());
                }
            });
            Ok(Self {
                socket,
                _dir: dir,
                commands,
                labels,
                error,
                shutdown,
                thread: Some(thread),
            })
        }

        pub(super) fn socket(&self) -> &Path {
            &self.socket
        }

        pub(super) fn commands(&self) -> Vec<CapturedCommand> {
            lock(&self.commands).clone()
        }

        pub(super) fn labels(&self) -> Vec<String> {
            lock(&self.labels).clone()
        }

        pub(super) fn error(&self) -> Option<String> {
            lock(&self.error).clone()
        }
    }

    impl Drop for ModelCatalogHost {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Release);
            if let Ok(stream) = UnixStream::connect(&self.socket) {
                let _ = stream.shutdown(Shutdown::Both);
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    struct CatalogModel {
        id: &'static str,
        display_name: &'static str,
        enabled: bool,
    }

    fn accept_one(
        listener: UnixListener,
        shutdown: &AtomicBool,
        commands: &Mutex<Vec<CapturedCommand>>,
        labels: &Mutex<Vec<String>>,
    ) -> io::Result<()> {
        loop {
            if shutdown.load(Ordering::Acquire) {
                return Ok(());
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    if shutdown.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    return serve(stream, shutdown, commands, labels);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn serve(
        mut stream: UnixStream,
        shutdown: &AtomicBool,
        commands: &Mutex<Vec<CapturedCommand>>,
        labels: &Mutex<Vec<String>>,
    ) -> io::Result<()> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        stream.set_write_timeout(Some(Duration::from_millis(500)))?;
        let Some(bytes) = read_frame(&mut stream, shutdown)? else {
            return Ok(());
        };
        let frame: Value = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if frame.get("type").and_then(Value::as_str) != Some("handshake") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "first frame was not a handshake",
            ));
        }
        push_label(labels, "handshake");
        let request_id = frame
            .pointer("/data/request_id")
            .and_then(Value::as_str)
            .unwrap_or("handshake");
        let versions = frame
            .pointer("/data/supported_api_versions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let Some(api) = negotiate(&versions) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "client offered no major-1 api version",
            ));
        };
        write_frame(&mut stream, &handshake_accepted(request_id, &api))?;
        write_frame(&mut stream, &empty_snapshot())?;

        let mut models = vec![
            CatalogModel {
                id: "flash",
                display_name: "GLM Flash",
                enabled: true,
            },
            CatalogModel {
                id: "other",
                display_name: "Other Model",
                enabled: true,
            },
        ];
        while let Some(bytes) = read_frame(&mut stream, shutdown)? {
            let frame: Value = match serde_json::from_slice(&bytes) {
                Ok(frame) => frame,
                Err(_) => {
                    push_label(labels, "bad-json");
                    continue;
                }
            };
            dispatch(&mut stream, &frame, &api, &mut models, commands, labels)?;
        }
        Ok(())
    }

    fn dispatch(
        stream: &mut UnixStream,
        frame: &Value,
        api: &Value,
        models: &mut [CatalogModel],
        commands: &Mutex<Vec<CapturedCommand>>,
        labels: &Mutex<Vec<String>>,
    ) -> io::Result<()> {
        match frame.get("type").and_then(Value::as_str).unwrap_or("") {
            "ack" => push_label(labels, "ack"),
            "subscribe" => push_label(labels, "subscribe"),
            "unsubscribe" => push_label(labels, "unsubscribe"),
            "pong" => push_label(labels, "pong"),
            "resume" => push_label(labels, "resume"),
            "heartbeat" => {
                push_label(labels, "heartbeat");
                let nonce = frame.pointer("/data/nonce").cloned().unwrap_or(json!(0));
                write_frame(stream, &json!({"type": "pong", "data": {"nonce": nonce}}))?;
            }
            "snapshot_request" => {
                push_label(labels, "snapshot_request");
                write_frame(stream, &empty_snapshot())?;
            }
            "query" => answer_query(stream, frame, api, models, labels)?,
            "command" => answer_command(stream, frame, api, models, commands, labels)?,
            other => push_label(labels, &format!("frame:{other}")),
        }
        Ok(())
    }

    fn answer_query(
        stream: &mut UnixStream,
        frame: &Value,
        api: &Value,
        models: &[CatalogModel],
        labels: &Mutex<Vec<String>>,
    ) -> io::Result<()> {
        let data = frame.get("data");
        let request_id = data
            .and_then(|data| data.get("request_id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let query = data.and_then(|data| data.get("query"));
        let method = query
            .and_then(|query| query.get("method"))
            .and_then(Value::as_str)
            .unwrap_or("");
        push_label(labels, &format!("query:{method}"));
        let params = query.and_then(|query| query.get("params"));
        let include_disabled = params
            .and_then(|params| params.get("include_disabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let body = match method {
            "model_list" => Value::Array(model_rows(models, include_disabled)),
            "provider_auth_status" => provider_status(),
            "general_settings" => json!({"proxy_url": null}),
            "permissions_settings" => json!({
                "approval_mode": "ask_for_dangerous",
                "workspace_trusted": false,
                "trust_workspaces_global": null,
                "workspace_id": ""
            }),
            "terminal_settings" => json!({"shell": null, "columns": 80, "rows": 24}),
            "subagent_settings" => json!({}),
            "mcp_list" => json!({"servers": []}),
            _ => json!({}),
        };
        write_frame(stream, &response_frame(api, request_id, body))
    }

    fn answer_command(
        stream: &mut UnixStream,
        frame: &Value,
        api: &Value,
        models: &mut [CatalogModel],
        commands: &Mutex<Vec<CapturedCommand>>,
        labels: &Mutex<Vec<String>>,
    ) -> io::Result<()> {
        let data = frame.get("data");
        let command_id = data
            .and_then(|data| data.get("command_id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let command = data.and_then(|data| data.get("command"));
        let method = command
            .and_then(|command| command.get("method"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let params = command
            .and_then(|command| command.get("params"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        push_label(labels, &format!("command:{method}"));
        lock(commands).push(CapturedCommand {
            method: method.clone(),
            params: params.clone(),
        });
        let body = if method == "set_model_enabled" {
            let provider_id = params
                .get("provider_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let model_id = params
                .get("model_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let enabled = params
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if provider_id == "glm" {
                if let Some(model) = models.iter_mut().find(|model| model.id == model_id) {
                    model.enabled = enabled;
                }
            }
            json!({
                "provider_id": provider_id,
                "model_id": model_id,
                "enabled": enabled,
                "cleared_roles": []
            })
        } else {
            json!({})
        };
        write_frame(stream, &response_frame(api, command_id, body))
    }

    fn model_rows(models: &[CatalogModel], include_disabled: bool) -> Vec<Value> {
        models
            .iter()
            .filter(|model| include_disabled || model.enabled)
            .map(|model| {
                json!({
                    "provider_id": "glm",
                    "id": model.id,
                    "display_name": model.display_name,
                    "enabled": model.enabled
                })
            })
            .collect()
    }

    fn provider_status() -> Value {
        json!({
            "providers": [{
                "provider_id": "glm",
                "display_name": "glm",
                "endpoint_label": "",
                "auth_methods": ["api_key"],
                "credentials": [],
                "auth": {
                    "type": "connected",
                    "method": "api_key",
                    "masked_credential": null
                },
                "catalog": {
                    "type": "remote",
                    "fetched_at": "2026-09-05T00:00:00Z"
                },
                "use_proxy": false
            }],
            "default": null,
            "role_defaults": {
                "naming": null,
                "vision": null,
                "search": null
            }
        })
    }

    fn handshake_accepted(request_id: &str, api: &Value) -> Value {
        json!({
            "type": "handshake",
            "data": {
                "status": "accepted",
                "request_id": request_id,
                "selected_api_version": api,
                "handle": {
                    "instance_id": "ac02",
                    "api_version": api
                },
                "client_id": "gui-ac02",
                "connection_id": "conn-ac02",
                "resume": {"type": "up_to_date", "current_sequence": 0},
                "capabilities": ["snapshots"]
            }
        })
    }

    fn empty_snapshot() -> Value {
        json!({
            "type": "snapshot",
            "data": {
                "instance_id": "ac02",
                "snapshot_sequence": 0,
                "generated_at": 0,
                "sections": []
            }
        })
    }

    fn response_frame(api: &Value, request_id: &str, data: Value) -> Value {
        json!({
            "type": "response",
            "data": {
                "api_version": api,
                "request_id": request_id,
                "responded_at": 0,
                "response": {"type": "data", "data": data}
            }
        })
    }

    fn negotiate(versions: &[Value]) -> Option<Value> {
        let mut best: Option<u64> = None;
        for version in versions {
            let Some(major) = version.get("major").and_then(Value::as_u64) else {
                continue;
            };
            let Some(minor) = version.get("minor").and_then(Value::as_u64) else {
                continue;
            };
            if major == 1 {
                best = Some(best.map_or(minor, |current| current.max(minor)));
            }
        }
        best.map(|minor| json!({"major": 1, "minor": minor}))
    }

    fn read_frame(stream: &mut UnixStream, shutdown: &AtomicBool) -> io::Result<Option<Vec<u8>>> {
        let mut header = [0u8; 4];
        if read_exact(stream, &mut header, shutdown)?.is_none() {
            return Ok(None);
        }
        let len = u32::from_le_bytes(header) as usize;
        if len > 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame too large",
            ));
        }
        let mut payload = vec![0u8; len];
        if read_exact(stream, &mut payload, shutdown)?.is_none() {
            return Ok(None);
        }
        Ok(Some(payload))
    }

    fn read_exact(
        stream: &mut UnixStream,
        buf: &mut [u8],
        shutdown: &AtomicBool,
    ) -> io::Result<Option<()>> {
        let mut filled = 0;
        while filled < buf.len() {
            if shutdown.load(Ordering::Acquire) {
                return Ok(None);
            }
            match stream.read(&mut buf[filled..]) {
                Ok(0) => return Ok(None),
                Ok(count) => filled += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error)
                    if error.kind() == io::ErrorKind::TimedOut
                        || error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
        Ok(Some(()))
    }

    fn write_frame(stream: &mut UnixStream, value: &Value) -> io::Result<()> {
        let payload = serde_json::to_vec(value)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let len = u32::try_from(payload.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
        stream.write_all(&len.to_le_bytes())?;
        stream.write_all(&payload)?;
        stream.flush()
    }

    fn push_label(labels: &Mutex<Vec<String>>, label: &str) {
        lock(labels).push(label.to_string());
    }

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn flash_enabled(view: &crate::ui::AppView) -> Option<bool> {
        view.projection
            .settings_providers
            .model_catalog
            .iter()
            .find(|model| model.provider_id == "glm" && model.id == "flash")
            .map(|model| model.enabled)
    }

    fn model_ids(view: &crate::ui::AppView) -> Vec<&str> {
        view.projection
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect()
    }

    fn painted(view: &crate::ui::AppView, id: &str) -> bool {
        let rect = view.settings_element_bounds(id);
        rect.width > 1.0 && rect.height > 1.0
    }

    fn until_view(
        cx: &mut gpui::VisualTestContext,
        view: &gpui::Entity<crate::ui::AppView>,
        host: &ModelCatalogHost,
        what: &str,
        mut ready: impl FnMut(&crate::ui::AppView) -> bool,
    ) {
        for _ in 0..500 {
            cx.refresh().unwrap();
            cx.run_until_parked();
            if cx.update(|_, app| ready(view.read(app))) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let detail = cx.update(|_, app| {
            let view = view.read(app);
            format!(
                "conn={:?} providers={:?} models={:?} catalog={:?} query={} route={:?} menu={:?} pending={} commands={:?} labels={:?} host={:?}",
                view.projection.connection,
                view.projection
                    .settings_providers
                    .providers
                    .iter()
                    .map(|entry| entry.provider_id.clone())
                    .collect::<Vec<_>>(),
                model_ids(view),
                view.projection
                    .settings_providers
                    .model_catalog
                    .iter()
                    .map(|model| (model.id.clone(), model.enabled))
                    .collect::<Vec<_>>(),
                view.model_search_query,
                view.route,
                view.open_menu,
                view.projection
                    .settings_providers
                    .model_write_pending
                    .is_some(),
                host.commands(),
                host.labels(),
                host.error(),
            )
        });
        panic!("{what}: {detail}");
    }

    fn click_settings(
        cx: &mut gpui::VisualTestContext,
        view: &gpui::Entity<crate::ui::AppView>,
        id: &str,
    ) {
        let center = cx.update(|_, app| {
            let rect = view.read(app).settings_element_bounds(id);
            assert!(
                rect.width > 1.0 && rect.height > 1.0,
                "{id} is not painted ({}x{})",
                rect.width,
                rect.height
            );
            gpui::point(
                gpui::px(rect.x + rect.width / 2.0),
                gpui::px(rect.y + rect.height / 2.0),
            )
        });
        cx.simulate_click(center, gpui::Modifiers::none());
    }

    fn click_flash_switch(
        cx: &mut gpui::VisualTestContext,
        view: &gpui::Entity<crate::ui::AppView>,
    ) {
        let id = crate::ui::settings::settings_model_switch_identifier("glm", "flash");
        let menu = crate::ui::settings::settings_models_menu_identifier("glm");
        let center = cx.update(|_, app| {
            let rect = view.read(app).settings_menu_element_bounds(&id, &menu);
            assert!(
                rect.width > 1.0 && rect.height > 1.0,
                "flash switch is not painted ({}x{})",
                rect.width,
                rect.height
            );
            gpui::point(
                gpui::px(rect.x + rect.width / 2.0),
                gpui::px(rect.y + rect.height / 2.0),
            )
        });
        cx.simulate_click(center, gpui::Modifiers::none());
    }

    fn assert_toggle(command: &CapturedCommand, enabled: bool) {
        assert_eq!(command.method, "set_model_enabled");
        assert_eq!(command.params["provider_id"].as_str(), Some("glm"));
        assert_eq!(command.params["model_id"].as_str(), Some("flash"));
        assert_eq!(command.params["enabled"].as_bool(), Some(enabled));
    }

    /// AC-02：真实 AppView 连临时 Host，搜索 flash 后用渲染出的 Switch
    /// 鼠标关闭、键盘重新启用，并核对两次 set_model_enabled 回执。
    #[gpui::test]
    fn settings_catalog_search_and_switch_receipts(cx: &mut gpui::TestAppContext) {
        use gpui::EntityInputHandler;

        let host = ModelCatalogHost::spawn().expect("catalog host");
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let (view, cx) = cx.add_window_view(|_, cx| {
            crate::ui::AppView::new(platform, host.socket().to_path_buf(), None, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(1024.0)));

        until_view(cx, &view, &host, "connected catalog", |view| {
            matches!(
                view.projection.connection,
                crate::projection::ConnectionState::Connected { .. }
            ) && view
                .projection
                .settings_providers
                .providers
                .iter()
                .any(|entry| entry.provider_id == "glm")
                && flash_enabled(view) == Some(true)
                && model_ids(view).contains(&"flash")
        });

        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.on_open_settings(window, cx));
        });

        let expand = crate::ui::settings::settings_provider_expand_identifier("glm");
        until_view(cx, &view, &host, "expand control", |view| {
            matches!(view.route, crate::ui::AppRoute::Settings) && painted(view, &expand)
        });
        click_settings(cx, &view, &expand);

        let manage = crate::ui::settings::settings_manage_models_identifier("glm");
        until_view(cx, &view, &host, "manage models", |view| {
            painted(view, &manage)
                && view
                    .projection
                    .settings_providers
                    .providers
                    .iter()
                    .any(|entry| {
                        entry.provider_id == "glm" && view.settings_manage_models_enabled(entry)
                    })
        });
        click_settings(cx, &view, &manage);

        until_view(cx, &view, &host, "models menu", |view| {
            matches!(
                &view.open_menu,
                Some(crate::ui::MenuKind::SettingsProviderModels(provider)) if provider == "glm"
            )
        });

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.model_search_input.update(cx, |input, cx| {
                    input.replace_text_in_range(None, "flash", window, cx);
                });
            });
        });
        until_view(cx, &view, &host, "search flash", |view| {
            view.model_search_query == "flash"
                && view
                    .settings_filtered_models("glm")
                    .iter()
                    .map(|model| model.id.as_str())
                    .eq(["flash"])
        });

        click_flash_switch(cx, &view);
        until_view(cx, &view, &host, "mouse disable receipt", |view| {
            flash_enabled(view) == Some(false)
                && view
                    .projection
                    .settings_providers
                    .model_write_pending
                    .is_none()
                && !model_ids(view).contains(&"flash")
                && model_ids(view).contains(&"other")
        });
        let commands = host.commands();
        assert_eq!(commands.len(), 1, "host={:?}", host.error());
        assert_toggle(&commands[0], false);

        let switch_id = crate::ui::settings::settings_model_switch_identifier("glm", "flash");
        cx.update(|window, cx| {
            view.update(cx, |view, _cx| {
                let focus = view
                    .settings_action_focus
                    .get(&switch_id)
                    .expect("switch focus")
                    .clone();
                window.focus(&focus);
                assert!(focus.is_focused(window), "switch did not take focus");
            });
        });
        cx.simulate_keystrokes("space");
        until_view(cx, &view, &host, "keyboard enable receipt", |view| {
            flash_enabled(view) == Some(true)
                && view
                    .projection
                    .settings_providers
                    .model_write_pending
                    .is_none()
                && model_ids(view).contains(&"flash")
                && model_ids(view).contains(&"other")
        });
        let commands = host.commands();
        assert_eq!(
            commands.len(),
            2,
            "labels={:?} host={:?}",
            host.labels(),
            host.error()
        );
        assert_toggle(&commands[0], false);
        assert_toggle(&commands[1], true);
        assert!(host.error().is_none(), "{:?}", host.error());
    }
}
