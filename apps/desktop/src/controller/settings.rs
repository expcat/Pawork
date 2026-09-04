//! Settings / 认证查询与写命令；Data 载荷走 protocol 类型。

use super::*;

impl DesktopController {
    pub fn load_models(&self) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let query = model_list_query();
            match client
                .query(query, command_source(), actor_identity())
                .await
            {
                Ok(response) => match parse_models(&response) {
                    Ok(models) => {
                        let _ = events.send(ControllerEvent::ModelsLoaded(models)).await;
                    }
                    Err(reason) => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "load models",
                            reason,
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "load models",
                        reason: error.to_string(),
                    },
                ),
            }
        });
    }

    /// 拉取 Settings「模型与供应商」页只读状态（provider_auth_status，
    /// provider_id=None → 全部）。返回是否已派出（断线时由 UI 保留 stale
    /// 只读结果，不进入 loading）。
    pub fn load_provider_status(&self) -> bool {
        let Some(client) = self.current_client() else {
            return false;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            match client
                .query(
                    provider_auth_status_query(),
                    command_source(),
                    actor_identity(),
                )
                .await
            {
                Ok(response) => match parse_provider_status_response(&response) {
                    Ok(providers) => {
                        let _ = events
                            .send(ControllerEvent::ProviderStatusLoaded(providers))
                            .await;
                    }
                    Err(reason) => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "load provider status",
                            reason,
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "load provider status",
                        reason: error.to_string(),
                    },
                ),
            }
        });
        true
    }

    /// 设为默认模型（set_default_model，非重放命令）。Data 确认后发
    /// `DefaultModelConfirmed`（Composer 同步）并重查 provider_auth_status
    /// 取回权威 default；Error / 传输失败经 OperationFailed 呈现，不动
    /// UI 现有状态。
    pub fn set_default_model(&self, provider_id: String, model_id: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "set default model".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = set_default_model_command(&provider_id, &model_id);
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "set default model",
                            reason: error.to_string(),
                        },
                    );
                    return;
                }
            };
            let confirmed = match parse_default_model_confirmation(&response) {
                Ok(confirmed) => confirmed,
                Err(reason) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "set default model",
                            reason,
                        },
                    );
                    return;
                }
            };
            let _ = events
                .send(ControllerEvent::DefaultModelConfirmed(confirmed))
                .await;
            // 确认后重查权威 provider 状态（含 default）；失败走既有
            // load provider status 通道，UI 保留现有只读列表。
            match client
                .query(
                    provider_auth_status_query(),
                    command_source(),
                    actor_identity(),
                )
                .await
            {
                Ok(response) => match parse_provider_status_response(&response) {
                    Ok(data) => {
                        let _ = events
                            .send(ControllerEvent::ProviderStatusLoaded(data))
                            .await;
                    }
                    Err(reason) => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "load provider status",
                            reason,
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "load provider status",
                        reason: error.to_string(),
                    },
                ),
            }
        });
    }

    /// 拉取 Settings「通用」页（general_settings）。返回是否已派出
    ///（断线时由 UI 保留 stale 只读结果，不进入 loading）。
    pub fn load_general_settings(&self) -> bool {
        let Some(client) = self.current_client() else {
            return false;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            match client
                .query(general_settings_query(), command_source(), actor_identity())
                .await
            {
                Ok(response) => match parse_general_settings_response(&response) {
                    Ok(proxy_url) => {
                        let _ = events
                            .send(ControllerEvent::GeneralSettingsLoaded(proxy_url))
                            .await;
                    }
                    Err(reason) => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "load general settings",
                            reason,
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "load general settings",
                        reason: error.to_string(),
                    },
                ),
            }
        });
        true
    }

    /// 设置或清除 Global `proxy_url`（set_proxy_url）。Data 确认后发
    /// `ProxyUrlConfirmed`（回执即写后状态）；Error / 传输失败经
    /// OperationFailed 呈现，不动 UI 现有生效值。
    pub fn set_proxy_url(&self, proxy_url: Option<String>) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "set proxy url".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = set_proxy_url_command(proxy_url.as_deref());
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "set proxy url",
                            reason: error.to_string(),
                        },
                    );
                    return;
                }
            };
            match parse_general_settings_response(&response) {
                Ok(confirmed) => {
                    let _ = events
                        .send(ControllerEvent::ProxyUrlConfirmed(confirmed))
                        .await;
                }
                Err(reason) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "set proxy url",
                        reason,
                    },
                ),
            }
        });
    }

    /// 拉取 Settings「权限与审批」页（permissions_settings，SET-6b）。返回
    /// 是否已派出（断线时由 UI 保留 stale 只读结果，不进入 loading）。
    pub fn load_permissions_settings(&self) -> bool {
        let Some(client) = self.current_client() else {
            return false;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            match client
                .query(
                    permissions_settings_query(),
                    command_source(),
                    actor_identity(),
                )
                .await
            {
                Ok(response) => match parse_permissions_settings_response(&response) {
                    Ok(data) => {
                        let _ = events
                            .send(ControllerEvent::PermissionsSettingsLoaded(data))
                            .await;
                    }
                    Err(reason) => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "load permissions settings",
                            reason,
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "load permissions settings",
                        reason: error.to_string(),
                    },
                ),
            }
        });
        true
    }

    /// 会话内切换审批模式（set_approval_mode，ADR-048 D2：不持久化、只
    /// 影响之后启动的 run）。Data 回执即写后状态；Error / 传输失败经
    /// OperationFailed 呈现，不动 UI 现有生效值。
    pub fn set_approval_mode(&self, mode: ApprovalModeWire) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "set approval mode".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        let owned_mode = mode.as_str().to_string();
        self.runtime.spawn(async move {
            let command = set_approval_mode_command(&owned_mode);
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "set approval mode",
                            reason: error.to_string(),
                        },
                    );
                    return;
                }
            };
            match parse_approval_mode_confirmation(&response) {
                Ok(mode) => {
                    let _ = events
                        .send(ControllerEvent::ApprovalModeConfirmed { mode })
                        .await;
                }
                Err(reason) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "set approval mode",
                        reason,
                    },
                ),
            }
        });
    }

    /// 会话内信任切换（workspace_trust，ADR-048 D3：workspace_id 必须匹配
    /// 当前 attached workspace，由 Host 校验；不写盘）。Data 回执即写后
    /// 状态；失败 fail-closed 不动 UI 现有生效值。
    pub fn set_workspace_trust(&self, workspace_id: &str, trusted: bool) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "set workspace trust".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        let owned_workspace = workspace_id.to_string();
        self.runtime.spawn(async move {
            let command = set_workspace_trust_command(&owned_workspace, trusted);
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "set workspace trust",
                            reason: error.to_string(),
                        },
                    );
                    return;
                }
            };
            match parse_workspace_trust_confirmation(&response) {
                Ok(trusted) => {
                    let _ = events
                        .send(ControllerEvent::WorkspaceTrustConfirmed { trusted })
                        .await;
                }
                Err(reason) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "set workspace trust",
                        reason,
                    },
                ),
            }
        });
    }

    /// 拉取 Settings「终端」页（terminal_settings，SET-6d / ADR-050 D2）。
    /// 返回是否已派出（断线时由 UI 保留 stale 只读结果，不进入 loading）。
    pub fn load_terminal_settings(&self) -> bool {
        let Some(client) = self.current_client() else {
            return false;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            match client
                .query(
                    terminal_settings_query(),
                    command_source(),
                    actor_identity(),
                )
                .await
            {
                Ok(response) => match parse_terminal_settings_response(&response) {
                    Ok(data) => {
                        let _ = events
                            .send(ControllerEvent::TerminalSettingsLoaded(data))
                            .await;
                    }
                    Err(reason) => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "load terminal settings",
                            reason,
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "load terminal settings",
                        reason: error.to_string(),
                    },
                ),
            }
        });
        true
    }

    /// 终端默认值全态写（set_terminal_settings，SET-6d / ADR-050 D3）：
    /// shell/columns/rows 三字段必填全态回传（shell=null 清除回平台默认）。
    /// Data 回执即写后状态经 `TerminalSettingsConfirmed` 投递；Error /
    /// 传输失败经 OperationFailed 呈现，不动 UI 现有生效值。
    pub fn set_terminal_settings(&self, shell: Option<String>, columns: u16, rows: u16) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "set terminal settings".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = set_terminal_settings_command(shell.as_deref(), columns, rows);
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "set terminal settings",
                            reason: error.to_string(),
                        },
                    );
                    return;
                }
            };
            match parse_terminal_settings_response(&response) {
                Ok(data) => {
                    let _ = events
                        .send(ControllerEvent::TerminalSettingsConfirmed(data))
                        .await;
                }
                Err(reason) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "set terminal settings",
                        reason,
                    },
                ),
            }
        });
    }

    /// 发起 OAuth 授权（auth_start）。响应只携带 verification_url /
    /// user_code / expires_at，进度经 AuthChanged 事件收敛。
    pub fn auth_start(&self, provider_id: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "start provider auth".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = auth_start_command(&provider_id, "oauth");
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "start provider auth",
                            reason: error.to_string(),
                        },
                    );
                    return;
                }
            };
            match parse_auth_started(&response) {
                Ok(data) => {
                    let _ = events
                        .send(ControllerEvent::AuthStarted {
                            provider_id,
                            data,
                        })
                        .await;
                }
                Err(reason) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "start provider auth",
                        reason,
                    },
                ),
            }
        });
    }

    /// 提交并验证 API key（auth_set_api_key，非重放命令）。明文只在本次
    /// 调用栈上转成冻结 wire 命令后即弃：不写日志、不进事件 / projection /
    /// 持久状态；结果（含失败原因）由 Host 经 AuthChanged 下发。
    pub fn auth_set_api_key(&self, provider_id: String, api_key: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "verify api key".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = auth_set_api_key_command(&provider_id, &api_key);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "verify api key",
                        reason: error.to_string(),
                    },
                );
            }
            // 成功路径无回执事件：Host 已先经 AuthChanged::Succeeded 下发
            // 脱敏凭证，UI 状态由事件泵收敛。
        });
    }

    /// 取消进行中的 OAuth 等待（auth_cancel；对 api_key 验证无效，Host
    /// 返回结构化错误）。Cancelled 事件到达后 UI 复位。
    pub fn auth_cancel(&self, provider_id: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "cancel provider auth".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = auth_cancel_command(&provider_id);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "cancel provider auth",
                        reason: error.to_string(),
                    },
                );
            }
        });
    }

    /// 移除凭证（auth_remove；env 来源凭证由 Host 拒绝并说明）。Removed
    /// 事件到达后 UI 复位，失败经 OperationFailed 呈现。
    pub fn auth_remove(&self, provider_id: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "remove provider auth".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = auth_remove_command(&provider_id);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "remove provider auth",
                        reason: error.to_string(),
                    },
                );
            }
        });
    }
}
