//! CU-18（GUI API 1.27）：computer-use 目标状态查询与授权命令。
//!
//! 只经既有 client query / command 通道；Desktop 不依赖 computer-use
//! （红线不变）。SessionId / RunId 等 domain id 未从 pawork-client
//! re-export，信封经冻结 serde 形状构造（与 session.rs 同口径）；
//! ComputerTargetListData / ComputerTargetIdentity / ComputerGrantKind
//! 已由 client 在 API 1.27 re-export，响应 / 参数按 typed 形状编解码。
//! registry since=1.27 的版本门在 client 发网前自动生效；本模块的旧
//! Host 分支只用于 UI 门控与禁用态文案，不承担安全裁决。
use super::*;

impl DesktopController {
    /// computer_target_* 消息的版本门（旧 Host 禁用入口并提示版本要求）。
    pub fn supports_computer_targets(&self) -> bool {
        self.current_client()
            .is_some_and(|client| client.api_version().minor >= 27)
    }

    /// 拉取会话的 computer-use 目标状态（computer_target_list）。返回是否
    /// 已派出；断线 / 旧 Host 不派出，UI 保留旧数据并停 loading。
    pub fn computer_target_list(&self, session_id: String, request_id: u64) -> bool {
        let Some(client) = self.current_client() else {
            return false;
        };
        if client.api_version().minor < 27 {
            return false;
        }
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let query = serde_json::from_value(json!({
                "method": "computer_target_list",
                "params": { "session_id": session_id }
            }))
            .expect("computer_target_list query shape is frozen");
            let result = match client
                .query(query, command_source(), actor_identity())
                .await
            {
                Ok(response) => parse_computer_target_list_response(&response),
                Err(error) => Err(error.to_string()),
            };
            let event = match result {
                Ok(data) => ControllerEvent::ComputerTargetsLoaded {
                    session_id,
                    request_id,
                    data,
                },
                Err(reason) => ControllerEvent::ComputerTargetsFailed {
                    session_id,
                    request_id,
                    reason,
                },
            };
            let _ = events.send(event).await;
        });
        true
    }

    /// 授权目标（computer_target_approve；kind = once / for_run）。回执
    /// 只回显生效 target 与寿命，权威状态由 UI 收到 Ok 后重查列表收敛。
    /// action_id 为 UI 派出代次，原样随回执带回供在途匹配。
    pub fn computer_target_approve(
        &self,
        session_id: String,
        action_id: u64,
        run_id: String,
        target: pawork_client::ComputerTargetIdentity,
        kind: pawork_client::ComputerGrantKind,
    ) -> bool {
        self.computer_target_command(
            "computer_target_approve",
            json!({
                "session_id": session_id,
                "run_id": run_id,
                "target": target,
                "kind": kind,
            }),
            session_id,
            action_id,
        )
    }

    /// 撤销授权并释放占用（computer_target_revoke，即目标级停止）。
    pub fn computer_target_revoke(
        &self,
        session_id: String,
        action_id: u64,
        run_id: String,
        target: pawork_client::ComputerTargetIdentity,
    ) -> bool {
        self.computer_target_command(
            "computer_target_revoke",
            json!({
                "session_id": session_id,
                "run_id": run_id,
                "target": target,
            }),
            session_id,
            action_id,
        )
    }

    /// 拉取会话的持久化观测证据（observation_list，CU-19）。返回是否已
    /// 派出；after_sequence 为事件序游标（None 从头探测），limit 单位是
    /// 事件数、由 Host 钳制。条目只读事件账本，不触发新截图或输入。
    pub fn observation_list(
        &self,
        session_id: String,
        request_id: u64,
        after_sequence: Option<u64>,
        limit: Option<u32>,
    ) -> bool {
        let Some(client) = self.current_client() else {
            return false;
        };
        if client.api_version().minor < 27 {
            return false;
        }
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let query = serde_json::from_value(json!({
                "method": "observation_list",
                "params": {
                    "session_id": session_id,
                    "after_sequence": after_sequence,
                    "limit": limit,
                }
            }))
            .expect("observation_list query shape is frozen");
            let result = match client
                .query(query, command_source(), actor_identity())
                .await
            {
                Ok(response) => parse_observation_list_response(&response),
                Err(error) => Err(error.to_string()),
            };
            let event = match result {
                Ok(data) => ControllerEvent::ObservationsLoaded {
                    session_id,
                    request_id,
                    data,
                },
                Err(reason) => ControllerEvent::ObservationsFailed {
                    session_id,
                    request_id,
                    reason,
                },
            };
            let _ = events.send(event).await;
        });
        true
    }

    /// 读取单条观测图像的有界字节块（observation_image_read，CU-19）。
    /// data 为 [offset, offset+len) 解码字节的 Base64；eof=false 时调用方
    /// 以回显 offset 续读，整图重组在 UI 侧完成（有界块 × 条目字节数）。
    pub fn observation_image_read(
        &self,
        session_id: String,
        request_id: u64,
        sequence: u64,
        part_index: u32,
        offset: u64,
        limit: u64,
    ) -> bool {
        let Some(client) = self.current_client() else {
            return false;
        };
        if client.api_version().minor < 27 {
            return false;
        }
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let query = serde_json::from_value(json!({
                "method": "observation_image_read",
                "params": {
                    "session_id": session_id,
                    "sequence": sequence,
                    "part_index": part_index,
                    "offset": offset,
                    "limit": limit,
                }
            }))
            .expect("observation_image_read query shape is frozen");
            let result = match client
                .query(query, command_source(), actor_identity())
                .await
            {
                Ok(response) => parse_observation_image_response(&response),
                Err(error) => Err(error.to_string()),
            };
            let event = match result {
                Ok(data) => ControllerEvent::ObservationImageChunk {
                    session_id,
                    request_id,
                    data,
                },
                Err(reason) => ControllerEvent::ObservationImageFailed {
                    session_id,
                    request_id,
                    reason,
                },
            };
            let _ = events.send(event).await;
        });
        true
    }

    /// 停止后台任务（run_cancel，CU-19 状态机的派发入口）。回执 Ok
    ///（Data / Accepted）只表示取消命令被 Host 接受；已停止由 UI 以
    /// run 终态事件确认，回执本身不冒充已停止。
    pub fn computer_stop_run(&self, session_id: String, action_id: u64, run_id: String) -> bool {
        let Some(client) = self.current_client() else {
            return false;
        };
        let events = self.event_sender();
        let command = run_cancel_command(&run_id);
        self.runtime.spawn(async move {
            let result = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => match response.response {
                    // run_cancel 的正常成功回执是 Accepted（无 Data 载荷）。
                    AppResponse::Data(_) | AppResponse::Accepted { .. } => Ok(()),
                    AppResponse::Error(error) => Err(error.message),
                    other => Err(format!("unexpected response: {other:?}")),
                },
                Err(error) => Err(error.to_string()),
            };
            let _ = events
                .send(ControllerEvent::ComputerStopFinished {
                    session_id,
                    action_id,
                    run_id,
                    result,
                })
                .await;
        });
        true
    }

    /// approve / revoke 共用发送路径；返回是否已派出（断线 / 旧 Host
    /// 不派出，调用方保留面板旧状态）。
    fn computer_target_command(
        &self,
        method: &str,
        params: serde_json::Value,
        session_id: String,
        action_id: u64,
    ) -> bool {
        let Some(client) = self.current_client() else {
            return false;
        };
        if client.api_version().minor < 27 {
            return false;
        }
        let events = self.event_sender();
        let command = serde_json::from_value(json!({
            "method": method,
            "params": params,
        }))
        .expect("computer target command shape is frozen");
        self.runtime.spawn(async move {
            let result = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => match response.response {
                    AppResponse::Data(_) => Ok(()),
                    AppResponse::Error(error) => Err(error.message),
                    other => Err(format!("unexpected response: {other:?}")),
                },
                Err(error) => Err(error.to_string()),
            };
            let _ = events
                .send(ControllerEvent::ComputerTargetActionFinished {
                    session_id,
                    action_id,
                    result,
                })
                .await;
        });
        true
    }
}

/// 解包 computer_target_list 信封：Data 为协议 ComputerTargetListData。
fn parse_computer_target_list_response(
    response: &AppResponseEnvelope,
) -> Result<ComputerTargetListData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 observation_list 信封：Data 为协议 ObservationListData。
fn parse_observation_list_response(
    response: &AppResponseEnvelope,
) -> Result<ObservationListData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 observation_image_read 信封：Data 为协议 ObservationImageData。
fn parse_observation_image_response(
    response: &AppResponseEnvelope,
) -> Result<ObservationImageData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}
