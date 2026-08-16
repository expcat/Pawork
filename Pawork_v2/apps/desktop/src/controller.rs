//! Controller 层：唯一业务出口是 pawork-client。
//!
//! 职责：连接握手 + 事件泵、SessionGet 分页加载、SessionCreate / RunStart、
//! 断线通知（重连由 UI 重试触发，重新 connect + 全新 Snapshot；
//! last-ack resume 属波 C / S10）。所有结果经 smol channel 回传 UI。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pawork_client::{
    ActorIdentity, AppCommand, AppEventEnvelope, AppQuery, AppResponse, AppResponseEnvelope,
    ClientError, CommandSource, ConnectOptions, GuiClient, GuiTransportClient, LocalTransport,
    Snapshot, TimelinePage, TransportEndpoint,
};
use serde_json::json;

use crate::projection::sessions_in_snapshot;

const PAGE_LIMIT: u32 = 500;
const MAX_PAGES: usize = 200;

/// UI 消费的控制器事件（经 smol channel 跨线程投递）。
#[derive(Clone, Debug)]
pub enum ControllerEvent {
    Disconnected { reason: String },
    Snapshot(Snapshot),
    TimelineLoaded { session_id: String, page: TimelinePage },
    Event(AppEventEnvelope),
    SessionCreated { session_id: String },
    MessageSent { session_id: String, run_id: String },
    OperationFailed { action: &'static str, reason: String },
}

struct SharedState {
    client: Mutex<Option<GuiClient>>,
    events: Mutex<Option<smol::channel::Sender<ControllerEvent>>>,
}

pub struct DesktopController {
    runtime: tokio::runtime::Handle,
    state: Arc<SharedState>,
}

impl DesktopController {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            runtime,
            state: Arc::new(SharedState {
                client: Mutex::new(None),
                events: Mutex::new(None),
            }),
        }
    }

    fn current_client(&self) -> Option<GuiClient> {
        self.state.client.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// 连接 + 握手 + 订阅，返回首帧 Snapshot 与事件接收端。
    ///
    /// 在 GPUI executor 上 await：握手本体 spawn 到 tokio runtime，仅等待
    /// JoinHandle；事件泵随后常驻 runtime。
    pub async fn connect(
        &self,
        socket: PathBuf,
    ) -> Result<(Snapshot, smol::channel::Receiver<ControllerEvent>), String> {
        let (sender, receiver) = smol::channel::bounded::<ControllerEvent>(512);
        let transport: Arc<dyn GuiTransportClient> = Arc::new(LocalTransport::default());
        let endpoint = TransportEndpoint::Local {
            address: socket.to_string_lossy().into_owned(),
        };
        let options = ConnectOptions {
            timeout_ms: 10_000,
            client_label: Some("pawork-desktop".into()),
            max_frame_bytes: 1024 * 1024,
        };
        let handshake = self
            .runtime
            .spawn(async move { GuiClient::connect(transport, endpoint, options, None).await })
            .await
            .map_err(|error| format!("connect task failed: {error}"))?
            .map_err(|error| error.to_string())?;
        let snapshot = handshake
            .initial_snapshot()
            .ok_or_else(|| "handshake did not deliver an initial snapshot".to_string())?;
        handshake
            .subscribe_all()
            .await
            .map_err(|error| error.to_string())?;

        *self.state.client.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(handshake.clone());
        *self.state.events.lock().unwrap_or_else(|p| p.into_inner()) = Some(sender.clone());

        let pump_client = handshake.clone();
        let pump_events = sender;
        let pump_state = Arc::clone(&self.state);
        self.runtime.spawn(async move {
            loop {
                match pump_client.next_event_timeout(Duration::from_secs(1)).await {
                    Ok(event) => {
                        if pump_events
                            .send(ControllerEvent::Event(event))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(ClientError::Timeout { .. }) => continue,
                    Err(error) => {
                        let reason = error.to_string();
                        *pump_state
                            .client
                            .lock()
                            .unwrap_or_else(|p| p.into_inner()) = None;
                        let _ = pump_events
                            .try_send(ControllerEvent::Disconnected { reason });
                        break;
                    }
                }
            }
        });
        Ok((snapshot, receiver))
    }

    /// 分页加载 session 时间线：SessionGet 按 timeline_after_sequence 链式
    /// 拉取直到 complete；分页期间先到的 live 事件由 projection 按 sequence
    /// 去重（gui-design §4.1 第 3 条）。
    pub fn open_session(&self, session_id: String) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let mut after: Option<u64> = None;
            for _ in 0..MAX_PAGES {
                let query = session_get_query(&session_id, after);
                let response = match client.query(query, command_source(), actor_identity()).await {
                    Ok(response) => response,
                    Err(error) => {
                        try_emit(&events, ControllerEvent::OperationFailed {
                            action: "open session",
                            reason: error.to_string(),
                        });
                        return;
                    }
                };
                let page = match timeline_page(&response) {
                    Ok(Some(page)) => page,
                    Ok(None) => return,
                    Err(reason) => {
                        try_emit(&events, ControllerEvent::OperationFailed {
                            action: "open session",
                            reason,
                        });
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
            try_emit(&events, ControllerEvent::OperationFailed {
                action: "open session",
                reason: format!("timeline exceeded {MAX_PAGES} pages"),
            });
        });
    }

    /// 新建 session：SessionCreate 只回 Accepted（无 session id），重取 snapshot
    /// 挑 updated_at_ms 最新的 session 返回（host gui_host 行为）。
    pub fn create_session(&self, workspace_id: String) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = session_create_command(&workspace_id);
            if let Err(error) =
                client.command(command, command_source(), actor_identity()).await
            {
                try_emit(&events, ControllerEvent::OperationFailed {
                    action: "create session",
                    reason: error.to_string(),
                });
                return;
            }
            match client.snapshot().await {
                Ok(snapshot) => {
                    let latest = sessions_in_snapshot(&snapshot)
                        .into_iter()
                        .map(|session| session.session_id)
                        .next();
                    if events.send(ControllerEvent::Snapshot(snapshot)).await.is_err() {
                        return;
                    }
                    if let Some(session_id) = latest {
                        let _ = events
                            .send(ControllerEvent::SessionCreated { session_id })
                            .await;
                    }
                }
                Err(error) => {
                    try_emit(&events, ControllerEvent::OperationFailed {
                        action: "create session",
                        reason: error.to_string(),
                    });
                }
            }
        });
    }

    /// 发送用户消息：RunStart（model/profile 由服务端默认，模型切换属波 C）。
    pub fn send_message(&self, session_id: String, text: String) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = run_start_command(&session_id, &text);
            match client.command(command, command_source(), actor_identity()).await {
                Ok(response) => match response.response {
                    AppResponse::Accepted { run_id: Some(run_id), .. } => {
                        let run_id = run_id.as_str().to_string();
                        let _ = events
                            .send(ControllerEvent::MessageSent {
                                session_id,
                                run_id,
                            })
                            .await;
                    }
                    other => {
                        try_emit(&events, ControllerEvent::OperationFailed {
                            action: "send message",
                            reason: format!("unexpected response: {other:?}"),
                        });
                    }
                },
                Err(error) => {
                    try_emit(&events, ControllerEvent::OperationFailed {
                        action: "send message",
                        reason: error.to_string(),
                    });
                }
            }
        });
    }

    fn event_sender(&self) -> smol::channel::Sender<ControllerEvent> {
        self.state
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .expect("event channel exists after connect")
    }
}

fn try_emit(
    events: &smol::channel::Sender<ControllerEvent>,
    event: ControllerEvent,
) {
    let _ = events.try_send(event);
}

/// source / identity 占位：服务端 host_stamp_command / host_stamp_query 会统一
/// 覆盖为 LocalGui + LocalUser（Pawork_v2/host/gui-server/src/session.rs），
/// 客户端只填必填信封字段，不伪造本地身份。
fn command_source() -> CommandSource {
    CommandSource::Automation
}

fn actor_identity() -> ActorIdentity {
    ActorIdentity::System
}

/// WorkspaceId / SessionId 等 domain id 未从 pawork-client re-export；命令与
/// 查询经冻结的 serde 形状（method/params）构造，避免引入第二个业务依赖。
fn session_create_command(workspace_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "session_create",
        "params": { "workspace_id": workspace_id }
    }))
    .expect("session_create command shape is frozen")
}

fn session_get_query(session_id: &str, after: Option<u64>) -> AppQuery {
    serde_json::from_value(json!({
        "method": "session_get",
        "params": {
            "session_id": session_id,
            "timeline_after_sequence": after,
            "timeline_limit": PAGE_LIMIT
        }
    }))
    .expect("session_get query shape is frozen")
}

fn run_start_command(session_id: &str, text: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "run_start",
        "params": { "session_id": session_id, "user_message": text }
    }))
    .expect("run_start command shape is frozen")
}

fn timeline_page(response: &AppResponseEnvelope) -> Result<Option<TimelinePage>, String> {
    match &response.response {
        AppResponse::Data(data) => match data.get("timeline_page") {
            Some(page) => serde_json::from_value::<TimelinePage>(page.clone())
                .map(Some)
                .map_err(|error| format!("decode timeline page: {error}")),
            None => Ok(None),
        },
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}
