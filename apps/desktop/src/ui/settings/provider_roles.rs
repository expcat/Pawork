//! Settings「Default models」四角色菜单。

use gpui::{Point, SharedString, Window};

use super::*;
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::MenuKind;

impl AppView {
    /// 「Default models」四默认角色区（OPT-3b / ADR-055 D5）：每行角色名 +
    /// 下拉 + 用途说明；候选 = 已连接 provider 的已启用模型（Host 已按
    /// include_disabled=false 过滤启用集，连接态在此过滤）。选择 / 清除走
    /// SetDefaultRoleModel；conversation 默认失效保持既有显式提示行，不做
    /// 任何静默切换；Vision / Search 落地期只保存选择（说明如实标注）。
    pub(super) fn settings_default_roles_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let unavailable = self.projection.default_model_unavailable();
        let mut section = self
            .settings_element("settings-default-roles")
            .flex()
            .flex_col()
            .gap_3()
            .pt_6()
            .border_t_1()
            .border_color(dark().border.subtle)
            .child(
                self.settings_element("settings-default-roles-title")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(settings_label(t("settings.roles.title")))
                    .child(settings_copy(t("settings.roles.subtitle"))),
            );
        if unavailable {
            section = section.child(
                self.settings_element("settings-default-roles-unavailable")
                    .text_size(font::BASE)
                    .text_color(dark().semantic.danger_text)
                    .child(settings_default_unavailable_note()),
            );
        }
        let mut card = self
            .settings_element("settings-default-roles-card")
            .flex()
            .flex_col();
        for (ix, role) in SettingsRole::ALL.into_iter().enumerate() {
            card = card.child(self.settings_role_row(ix, role, cx));
        }
        section.child(card)
    }

    /// 单个角色行：左侧角色名与用途说明，右侧下拉触发器。可见 / 键盘 / AX 三路径同 identifier、同 gate（断线 / stale /
    /// 该角色写中禁用，disabled 不发布 Press）。
    pub(super) fn settings_role_row(
        &mut self,
        ix: usize,
        role: SettingsRole,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let enabled = self.settings_role_menu_enabled(role);
        let menu_open =
            matches!(self.open_menu, Some(MenuKind::SettingsRole(open)) if open == role);
        let identifier = settings_role_trigger_identifier(role);
        let focus = self
            .settings_action_focus
            .entry(identifier.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click_id = identifier.clone();
        let activate_id = identifier.clone();
        let trigger = Button::new(identifier.clone())
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .width(px(SETTINGS_ROLE_MENU_WIDTH))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(self.settings_role_value_label(role))
            .tooltip(settings_role_description_label(role))
            .disabled(!enabled);
        let trigger = if enabled {
            trigger
                .on_click(
                    cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                        if view.consume_button_key_click(&click_id, event) {
                            return;
                        }
                        let down = Self::click_down_position(event);
                        view.on_toggle_settings_role_menu(role, down, window, cx);
                    }),
                )
                .on_activate(cx.listener(move |view, _event, window, cx| {
                    if view.open_menu.is_some() {
                        // 菜单已开时让位给 root 的菜单 Enter 处理，并重新
                        // 武装 keyup 合成 click 吞除标记。
                        view.note_button_key_activate(&activate_id);
                        return;
                    }
                    view.note_button_key_activate(&activate_id);
                    view.on_toggle_settings_role_menu(role, None, window, cx);
                    cx.stop_propagation();
                }))
        } else {
            trigger
        };
        let mut picker = Dropdown::new(self.settings_element(identifier).child(trigger));
        if menu_open && enabled {
            picker = picker.panel(self.settings_role_menu_element(role, cx));
        }
        div()
            .id(("settings-role-row", ix))
            .flex()
            .items_center()
            .gap_4()
            .py_4()
            .when(ix + 1 < SettingsRole::ALL.len(), |row| {
                row.border_b_1().border_color(dark().border.subtle)
            })
            .child(
                self.settings_element(format!("settings-role-label-{}", role.wire_name()))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(settings_label(role.label()))
                    .child(settings_copy(settings_role_description_label(role))),
            )
            .child(div().flex_none().child(picker))
    }

    /// 角色下拉当前值文案（render 与 AX 同源）：Provider · Model 显示名
    /// （清单缺失时诚实回落原始 id），未设置为 Not set。
    pub(crate) fn settings_role_value_label(&self, role: SettingsRole) -> String {
        let inactive = matches!(role, SettingsRole::Vision | SettingsRole::Search);
        let prefix = if inactive {
            format!("{} · ", t("settings.roles.inactive"))
        } else {
            String::new()
        };
        let Some((provider_id, model_id)) =
            self.projection.settings_providers.role_value(role).cloned()
        else {
            return format!("{prefix}{}", t("settings.roles.not_set"));
        };
        let provider = self
            .projection
            .settings_providers
            .providers
            .iter()
            .find(|entry| entry.provider_id == provider_id)
            .map(|entry| entry.display_name.clone())
            .unwrap_or_else(|| provider_id.clone());
        let model = self
            .projection
            .models
            .iter()
            .find(|model| model.provider_id == provider_id && model.id == model_id)
            .map(|model| model.display_name.clone())
            .unwrap_or_else(|| model_id.clone());
        format!("{prefix}{provider} · {model}")
    }

    /// 角色下拉 gate（render / 键盘 / AX 三路径同源）：写总闸之上，该
    /// 角色写中（SetDefaultRoleModel 在途）禁用。
    pub(crate) fn settings_role_menu_enabled(&self, role: SettingsRole) -> bool {
        self.settings_writes_enabled() && self.settings_role_pending != Some(role)
    }

    /// 角色下拉浮层面板：清除行 + 已连接 provider 分组的启用模型；空候选
    /// 给诚实空态说明，不编造模型。外点关闭 / 键盘与 MenuKind 语义同源。
    fn settings_role_menu_element(
        &mut self,
        role: SettingsRole,
        cx: &mut Context<Self>,
    ) -> MenuPanel {
        let candidates = settings_role_candidates(
            &self.projection.models,
            &self.projection.settings_providers.providers,
        );
        let current = self.projection.settings_providers.role_value(role).cloned();
        let selected_ix = self.menu_selected_index();
        let highlight = self.menu_highlight_effective(selected_ix);
        let menu_id = format!("settings-role-menu-{}", role.wire_name());
        let menu_scroll = self
            .settings_element_layouts
            .entry(menu_id.clone())
            .or_default()
            .clone();
        if menu_scroll.bounds().size.height == px(0.0) {
            // 首次 prepaint 前 ScrollHandle 尚无 viewport/overflow；等本帧
            // 布局完成后定位当前项，再重绘，避免首帧滚入请求被空几何吞掉。
            let view = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = view.update(cx, |view, cx| {
                    if matches!(view.open_menu, Some(MenuKind::SettingsRole(open)) if open == role)
                    {
                        let highlight = view.menu_highlight_effective(view.menu_selected_index());
                        view.scroll_settings_role_menu_to_item(role, highlight);
                        cx.notify();
                    }
                });
            });
        }
        let mut panel = MenuPanel::new(SharedString::from(menu_id))
            .track_scroll(&menu_scroll)
            .dismiss_on_outside(
                cx.listener(move |view, event: &gpui::MouseDownEvent, _, cx| {
                    view.dismiss_menu_on_outside(MenuKind::SettingsRole(role), event.position, cx);
                }),
            );
        panel = panel.child(
            self.settings_element(settings_role_clear_identifier(role))
                .child(
                    MenuRow::new(settings_role_clear_identifier(role))
                        .label(t("settings.roles.clear"))
                        .selected(current.is_none())
                        .highlighted(0 == highlight)
                        .on_click(cx.listener(move |view, _event, _window, cx| {
                            view.on_select_settings_role(role, None, cx);
                        })),
                ),
        );
        if candidates.is_empty() {
            // 无已连接 / 已启用模型：菜单仍可打开，给标题 + 一行指引的
            // 诚实空态；清除行仍可操作，不编造模型。
            return panel.child(
                self.settings_element(format!("settings-role-menu-empty-{}", role.wire_name()))
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
                            .child(t("settings.roles.empty_title")),
                    )
                    .child(
                        div()
                            .text_size(font::XS)
                            .text_color(dark().text.secondary)
                            .child(t("settings.roles.empty_hint")),
                    ),
            );
        }
        let mut item_ix = 1;
        for (provider_id, models) in candidates {
            // 组头显示名取 provider 权威清单；候选已按连接态过滤，此处
            // 回落仅防御清单暂态缺失，不臆造能力。
            let display_name = self
                .projection
                .settings_providers
                .providers
                .iter()
                .find(|entry| entry.provider_id == provider_id)
                .map(|entry| entry.display_name.clone())
                .unwrap_or_else(|| provider_id.to_string());
            panel = panel.child(
                self.settings_element(dynamic_identifier(
                    &format!("settings-role-menu-group-{}", role.wire_name()),
                    &provider_id,
                ))
                .min_h(gpui::rems(1.5))
                .px_2()
                .flex()
                .items_center()
                .min_w_0()
                .truncate()
                .text_size(font::SM)
                .text_color(dark().text.secondary)
                .child(display_name),
            );
            for model in models {
                let selected = current.as_ref().is_some_and(|(provider, id)| {
                    provider == &model.provider_id && id == &model.id
                });
                let item_id = settings_role_item_identifier(role, &model.provider_id, &model.id);
                panel = panel.child(
                    self.settings_element(item_id).child(
                        MenuRow::new(settings_role_item_identifier(
                            role,
                            &model.provider_id,
                            &model.id,
                        ))
                        .label(model.display_name.clone())
                        .selected(selected)
                        .highlighted(item_ix == highlight)
                        .on_click(cx.listener(
                            move |view, _event, _window, cx| {
                                view.on_select_settings_role(
                                    role,
                                    Some((model.provider_id.clone(), model.id.clone())),
                                    cx,
                                );
                            },
                        )),
                    ),
                );
                item_ix += 1;
            }
        }
        panel
    }

    /// 键盘索引只计 Clear 与模型；滚动索引还包含每个供应商组头。
    pub(crate) fn scroll_settings_role_menu_to_item(&self, role: SettingsRole, item: usize) {
        let Some(scroll) = self
            .settings_element_layouts
            .get(&format!("settings-role-menu-{}", role.wire_name()))
        else {
            return;
        };
        if item == 0 {
            scroll.scroll_to_item(0);
            return;
        }
        let mut logical = 1;
        let mut child = 1;
        for (_, models) in settings_role_candidates(
            &self.projection.models,
            &self.projection.settings_providers.providers,
        ) {
            child += 1; // provider header
            if item < logical + models.len() {
                scroll.scroll_to_item(child + item - logical);
                return;
            }
            logical += models.len();
            child += models.len();
        }
    }

    /// 角色菜单触发器 toggle（render 点击 / 键盘激活 / AX Press 三路径
    /// 同源；入口级复核 gate）。
    pub(crate) fn on_toggle_settings_role_menu(
        &mut self,
        role: SettingsRole,
        down_position: Option<Point<Pixels>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_role_menu_enabled(role) {
            return;
        }
        self.toggle_menu(MenuKind::SettingsRole(role), down_position, cx);
        if matches!(self.open_menu, Some(MenuKind::SettingsRole(open)) if open == role) {
            self.settings_element_layouts
                .remove(&format!("settings-role-menu-{}", role.wire_name()));
        }
    }

    /// 角色选择统一入口（菜单点击 / 键盘 Enter / AX Press 三路径同源；
    /// 入口级复核 gate 与候选目录，未知 pair fail-closed）。确认回执由
    /// DefaultRoleModelConfirmed 收敛，不在此乐观改状态。
    pub(crate) fn on_select_settings_role(
        &mut self,
        role: SettingsRole,
        value: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_role_menu_enabled(role) {
            return;
        }
        if let Some((provider_id, model_id)) = &value {
            let selectable = settings_role_menu_entries(
                &self.projection.models,
                &self.projection.settings_providers.providers,
            )
            .iter()
            .any(|model| model.provider_id == *provider_id && model.id == *model_id);
            if !selectable {
                return;
            }
        }
        self.open_menu = None;
        self.menu_highlight = None;
        self.settings_role_pending = Some(role);
        self.controller.set_default_role_model(role, value);
        cx.notify();
    }
    /// 转义后的 "<provider>:<model>" → 原始 pair（以 projection.models 为
    /// 权威，未知 fail-closed；不反解转义）。
    pub(crate) fn settings_default_target_for_escaped(
        &self,
        escaped: &str,
    ) -> Option<(String, String)> {
        self.projection
            .models
            .iter()
            .find(|model| {
                dynamic_identifier("", &format!("{}:{}", model.provider_id, model.id))
                    == format!("-{escaped}")
            })
            .map(|model| (model.provider_id.clone(), model.id.clone()))
    }
}
