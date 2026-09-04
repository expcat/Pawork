//! Session / workspace / run / fork 命令。

use super::*;

impl DesktopController {
    /// 分页加载 session 时间线：SessionGet 按 timeline_after_sequence 链式
    /// 拉取直到 complete；分页期间先到的 live 事件由 projection 按 sequence
    /// 去重（gui-design §4.1 第 3 条）。
    pub fn open_session(&self, session_id: String) {
        let Some(client) = self.current_client() else {
            if let Some(events) = self.try_event_sender() {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "open session",
                        reason: "not connected".into(),
                    },
                );
            }
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let mut after: Option<u64> = None;
            for _ in 0..MAX_PAGES {
                let query = session_get_query(&session_id, after);
                let response = match client
                    .query(query, command_source(), actor_identity())
                    .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "open session",
                                reason: error.to_string(),
                            },
                        );
                        return;
                    }
                };
                let page = match timeline_page(&response) {
                    Ok(Some(page)) => page,
                    Ok(None) => {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "open session",
                                reason: "session_get response carried no timeline page".into(),
                            },
                        );
                        return;
                    }
                    Err(reason) => {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "open session",
                                reason,
                            },
                        );
                        return;
                    }
                };
                let complete = page.complete;
                after = page.next_sequence;
                if events
                    .send(ControllerEvent::TimelineLoaded {
                        session_id: session_id.clone(),
                        page,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                if complete {
                    return;
                }
            }
            try_emit(
                &events,
                ControllerEvent::OperationFailed {
                    action: "open session",
                    reason: format!("timeline exceeded {MAX_PAGES} pages"),
                },
            );
        });
    }

    /// 新建 session：SessionCreate 只回 Accepted（无 session id），重取 snapshot
    /// 挑 updated_at_ms 最新的 session 返回（host gui_host 行为）。
    pub fn create_session(&self, workspace_id: String) {
        let Some(client) = self.current_client() else {
            if let Some(events) = self.try_event_sender() {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "create session",
                        reason: "not connected".into(),
                    },
                );
            }
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = session_create_command(&workspace_id);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "create session",
                        reason: error.to_string(),
                    },
                );
                return;
            }
            match client.snapshot().await {
                Ok(snapshot) => {
                    let latest = sessions_in_snapshot(&snapshot)
                        .into_iter()
                        .map(|session| session.session_id)
                        .next();
                    if events
                        .send(ControllerEvent::Snapshot(snapshot))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    if let Some(session_id) = latest {
                        let _ = events
                            .send(ControllerEvent::SessionCreated { session_id })
                            .await;
                    } else {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "create session",
                                reason: "host accepted SessionCreate but snapshot has no sessions"
                                    .into(),
                            },
                        );
                    }
                }
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "create session",
                            reason: error.to_string(),
                        },
                    );
                }
            }
        });
    }

    /// 选择一个真实目录作为当前项目；成功后重取 snapshot，让 UI 只消费
    /// Host 的 canonical workspace 结果，不在 Desktop 侧猜名称或 id。
    pub fn open_workspace(&self, root_path: PathBuf) {
        let Some(client) = self.current_client() else {
            if let Some(events) = self.try_event_sender() {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "open project",
                        reason: "not connected".into(),
                    },
                );
            }
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = workspace_add_command(&root_path);
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "open project",
                            reason: error.to_string(),
                        },
                    );
                    return;
                }
            };
            let Some((workspace_id, name)) = workspace_opened(&response) else {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "open project",
                        reason: format!("unexpected response: {:?}", response.response),
                    },
                );
                return;
            };
            match client.snapshot().await {
                Ok(snapshot) => {
                    if events
                        .send(ControllerEvent::Snapshot(snapshot))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let _ = events
                        .send(ControllerEvent::WorkspaceOpened { workspace_id, name })
                        .await;
                }
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "open project",
                            reason: error.to_string(),
                        },
                    );
                }
            }
        });
    }

    /// 发送用户消息：RunStart。可选 `(provider, model)` 只影响下一轮。
    pub fn send_message(&self, session_id: String, text: String, model: Option<(String, String)>) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = run_start_command(&session_id, &text, model.as_ref());
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => match response.response {
                    AppResponse::Accepted {
                        run_id: Some(run_id),
                        ..
                    } => {
                        let run_id = run_id.as_str().to_string();
                        let _ = events
                            .send(ControllerEvent::MessageSent {
                                session_id,
                                run_id,
                                text,
                            })
                            .await;
                    }
                    other => {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "send message",
                                reason: format!("unexpected response: {other:?}"),
                            },
                        );
                    }
                },
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "send message",
                            reason: error.to_string(),
                        },
                    );
                }
            }
        });
    }

    pub fn cancel_run(&self, run_id: String) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = run_cancel_command(&run_id);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "cancel run",
                        reason: error.to_string(),
                    },
                );
            }
        });
    }

    pub fn approve(&self, run_id: String, tool_call_id: String, decision: &str) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        let command = tool_approve_command(&run_id, &tool_call_id, decision);
        self.runtime.spawn(async move {
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "approve tool",
                        reason: error.to_string(),
                    },
                );
            }
        });
    }

    /// 主动断开：关窗 / `--probe-smoke` 重连前调用。不发 RunCancel（ADR-026）。
    pub async fn disconnect(&self) {
        let client = self
            .state
            .client
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(client) = client {
            let _ = client.close().await;
        }
    }

    /// 给 `--probe` 用的同步目录查询：不经 UI channel。
    pub async fn fetch_models(&self) -> Result<Vec<ModelEntry>, String> {
        let client = self
            .current_client()
            .ok_or_else(|| "not connected".to_string())?;
        let response = client
            .query(model_list_query(), command_source(), actor_identity())
            .await
            .map_err(|error| error.to_string())?;
        parse_models(&response)
    }

    /// 对 Timeline 某条 event_id 发 SessionFork。Host 仍可能 unsupported，
    /// 错误走既有 OperationFailed，不改 host/app。
    pub fn fork_session(&self, session_id: String, parent_event_id: String) {
        let Some(client) = self.current_client() else {
            if let Some(events) = self.try_event_sender() {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "fork session",
                        reason: "not connected".into(),
                    },
                );
            }
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = session_fork_command(&session_id, &parent_event_id);
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => match &response.response {
                    AppResponse::Error(_) => {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "fork session",
                                reason: "server returned an error response".into(),
                            },
                        );
                    }
                    AppResponse::Accepted { .. } | AppResponse::Data(_) => {
                        let hinted = forked_session_id(&response);
                        match client.snapshot().await {
                            Ok(snapshot) => {
                                let latest = hinted.or_else(|| {
                                    sessions_in_snapshot(&snapshot)
                                        .into_iter()
                                        .map(|session| session.session_id)
                                        .next()
                                });
                                if events
                                    .send(ControllerEvent::Snapshot(snapshot))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                                if let Some(session_id) = latest {
                                    let _ = events
                                        .send(ControllerEvent::SessionForked { session_id })
                                        .await;
                                }
                            }
                            Err(error) => try_emit(
                                &events,
                                ControllerEvent::OperationFailed {
                                    action: "fork session",
                                    reason: error.to_string(),
                                },
                            ),
                        }
                    }
                    other => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "fork session",
                            reason: format!("unexpected response: {other:?}"),
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "fork session",
                        reason: error.to_string(),
                    },
                ),
            }
        });
    }
}
