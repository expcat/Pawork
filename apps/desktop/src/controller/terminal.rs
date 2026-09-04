//! Inspector Terminal 命令。

use super::*;

impl DesktopController {
    pub fn terminal_create(&self, workspace_id: String, cwd: Option<String>) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::TerminalCreateFailed {
                workspace_id,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = match terminal_create_command(&workspace_id, cwd.as_deref()) {
                Ok(command) => command,
                Err(reason) => {
                    let _ = events
                        .send(ControllerEvent::TerminalCreateFailed {
                            workspace_id,
                            reason,
                        })
                        .await;
                    return;
                }
            };
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => match terminal_session_id(&response) {
                    Some(terminal_session_id) => {
                        let _ = events
                            .send(ControllerEvent::TerminalCreated {
                                workspace_id,
                                terminal_session_id,
                            })
                            .await;
                    }
                    None => {
                        let _ = events
                            .send(ControllerEvent::TerminalCreateFailed {
                                workspace_id,
                                reason: format!("unexpected response: {:?}", response.response),
                            })
                            .await;
                    }
                },
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::TerminalCreateFailed {
                            workspace_id,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    pub fn terminal_write(&self, terminal_session_id: String, data: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::TerminalWriteFailed {
                terminal_session_id,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = terminal_write_command(&terminal_session_id, &data);
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) if !matches!(&response.response, AppResponse::Error(_)) => {
                    let _ = events
                        .send(ControllerEvent::TerminalWriteSucceeded {
                            terminal_session_id,
                        })
                        .await;
                }
                Ok(response) => {
                    let _ = events
                        .send(ControllerEvent::TerminalWriteFailed {
                            terminal_session_id,
                            reason: format!("server returned {:?}", response.response),
                        })
                        .await;
                }
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::TerminalWriteFailed {
                            terminal_session_id,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    pub fn terminal_resize(&self, terminal_session_id: String, columns: u16, rows: u16) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::TerminalResizeFailed {
                terminal_session_id,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = terminal_resize_command(&terminal_session_id, columns, rows);
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) if !matches!(&response.response, AppResponse::Error(_)) => {
                    let _ = events
                        .send(ControllerEvent::TerminalResizeSucceeded {
                            terminal_session_id,
                            columns,
                            rows,
                        })
                        .await;
                }
                Ok(response) => {
                    let _ = events
                        .send(ControllerEvent::TerminalResizeFailed {
                            terminal_session_id,
                            reason: format!("server returned {:?}", response.response),
                        })
                        .await;
                }
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::TerminalResizeFailed {
                            terminal_session_id,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    /// ADR-045：终止（running）或清理（exited/killed tombstone）终端会话。
    /// 成功仅发 Succeeded 回执：running 的终态由 live TerminalExited 刷新，
    /// exited 清理由 UI 在回执后本地移除条目，不在此重复改 projection。
    pub fn terminal_close(&self, terminal_session_id: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::TerminalCloseFailed {
                terminal_session_id,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = terminal_close_command(&terminal_session_id);
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) if !matches!(&response.response, AppResponse::Error(_)) => {
                    let _ = events
                        .send(ControllerEvent::TerminalCloseSucceeded {
                            terminal_session_id,
                        })
                        .await;
                }
                Ok(response) => {
                    let _ = events
                        .send(ControllerEvent::TerminalCloseFailed {
                            terminal_session_id,
                            reason: format!("server returned {:?}", response.response),
                        })
                        .await;
                }
                Err(error) => {
                    // ADR-045：Close 的目标已从 Host 注册表消失（如本端此前
                    // 的 Stop 已就地注销）——not_found 是「条目不存在」的
                    // 权威确认，清理目标确定达成，按成功收敛让 UI 移除本地
                    // 条目，不把诚实 not_found 当失败卡死面板。
                    if matches!(
                        &error,
                        ClientError::Protocol(protocol)
                            if protocol.code == ProtocolErrorCode::RequestNotFound
                    ) {
                        let _ = events
                            .send(ControllerEvent::TerminalCloseSucceeded {
                                terminal_session_id,
                            })
                            .await;
                        return;
                    }
                    let _ = events
                        .send(ControllerEvent::TerminalCloseFailed {
                            terminal_session_id,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }
}
