//! 远程控制服务：配对/认证 → 门禁分类 → canonical 信封转发 → 通知推送。
//!
//! 事实源约束：所有读写一律经 AppService 的 canonical 信封（CommandSource
//! 为 Automation，身份为 AuthenticatedClient + device_id），本服务不维护
//! 任何会话/运行状态副本；计划状态查询在 Core 未暴露专用查询时仅返回
//! 显式可用性标记。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_domain::{ActorId, CommandId, QueryId, Timestamp};
use app_service::AppService;
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, AppResponse,
    CommandSource, API_VERSION,
};
use subscription_hub::{EventHub, HubError, HubSubscription};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::warn;
use transport_api::{GuiConnection, TransportErrorKind, TransportFrame};

use crate::audit::{AuditEvent, AuditLog};
use crate::gate::{self, Verdict};
use crate::notify::NotificationLog;
use crate::now_unix_ms;
use crate::pairing::{PairingConfig, PairingError, PairingRegistry};
use crate::wire::{self, ClientFrame, RemoteQuery, ServerFrame};

/// 转发到 Core 的身份 subject。
pub const SUBJECT: &str = "remote-control";

/// 连接收尾时等待出站刷新的上限。
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// 远程控制服务配置（所有容量均有界）。
#[derive(Clone, Debug)]
pub struct RemoteControlConfig {
    /// 应用层帧字节上限（transport 另有自身上限，二者取严）。
    pub max_frame_bytes: u64,
    /// 每连接出站队列容量（有界；溢出以显式 PushGap 告知）。
    pub outbound_capacity: usize,
    /// 连接级认证失败上限，超过即关闭连接（暴力破解缓解）。
    pub max_auth_failures: u32,
    /// 配对/设备策略。
    pub pairing: PairingConfig,
    /// 通知环形缓冲容量。
    pub notification_capacity: usize,
    /// 通知去重集合容量。
    pub dedup_capacity: usize,
    /// 审计环形缓冲容量。
    pub audit_capacity: usize,
}

impl Default for RemoteControlConfig {
    fn default() -> Self {
        Self {
            max_frame_bytes: 1024 * 1024,
            outbound_capacity: 1024,
            max_auth_failures: 5,
            pairing: PairingConfig::default(),
            notification_capacity: crate::notify::DEFAULT_NOTIFICATION_CAPACITY,
            dedup_capacity: crate::notify::DEFAULT_DEDUP_CAPACITY,
            audit_capacity: crate::audit::DEFAULT_AUDIT_CAPACITY,
        }
    }
}

/// 连接关闭原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// 对端正常关闭。
    ClientClosed,
    /// transport 层错误。
    TransportError(TransportErrorKind),
    /// 帧超过应用层上限。
    ProtocolViolation,
    /// 设备凭证被宿主吊销。
    Revoked,
    /// 认证失败次数超限。
    AuthFailuresExceeded,
}

/// 一次连接的服务摘要。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionSummary {
    /// 认证通过后的 device_id（未认证连接为 None）。
    pub device_id: Option<String>,
    pub frames_handled: u64,
    pub close_reason: CloseReason,
}

enum ConnState {
    PreAuth { failures: u32 },
    Authenticated { device_id: String },
}

impl ConnState {
    fn device_id(&self) -> Option<&str> {
        match self {
            ConnState::PreAuth { .. } => None,
            ConnState::Authenticated { device_id } => Some(device_id),
        }
    }

    fn actor(&self) -> String {
        match self {
            ConnState::PreAuth { .. } => "anonymous".to_string(),
            ConnState::Authenticated { device_id } => device_id.clone(),
        }
    }

    fn failures_mut(&mut self) -> Option<&mut u32> {
        match self {
            ConnState::PreAuth { failures } => Some(failures),
            ConnState::Authenticated { .. } => None,
        }
    }

    fn into_device_id(self) -> Option<String> {
        match self {
            ConnState::PreAuth { .. } => None,
            ConnState::Authenticated { device_id } => Some(device_id),
        }
    }
}

/// 远程控制服务（克隆廉价：共享配对注册表/通知日志/审计日志）。
#[derive(Clone)]
pub struct RemoteControlService {
    app_service: Arc<AppService>,
    hub: Arc<EventHub>,
    pairing: PairingRegistry,
    notifications: NotificationLog,
    audit: AuditLog,
    config: RemoteControlConfig,
    next_internal_id: Arc<AtomicU64>,
}

impl RemoteControlService {
    pub fn new(app_service: Arc<AppService>, hub: Arc<EventHub>) -> Self {
        Self::with_config(app_service, hub, RemoteControlConfig::default())
    }

    pub fn with_config(
        app_service: Arc<AppService>,
        hub: Arc<EventHub>,
        config: RemoteControlConfig,
    ) -> Self {
        Self {
            app_service,
            hub,
            pairing: PairingRegistry::with_config(config.pairing.clone()),
            notifications: NotificationLog::with_capacity(
                config.notification_capacity,
                config.dedup_capacity,
            ),
            audit: AuditLog::with_capacity(config.audit_capacity),
            config,
            next_internal_id: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn pairing(&self) -> &PairingRegistry {
        &self.pairing
    }

    pub fn notifications(&self) -> &NotificationLog {
        &self.notifications
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    pub fn config(&self) -> &RemoteControlConfig {
        &self.config
    }

    /// 吊销设备凭证：后续认证立即失败；进行中的连接在下一次收到帧时
    /// 被显式告知并关闭。返回是否发生状态变化。
    pub fn revoke_device(&self, device_id: &str) -> bool {
        let revoked = self.pairing.revoke(device_id);
        if revoked {
            let remaining_active = self.pairing.active_device_ids().len();
            self.audit.record(
                "host",
                AuditEvent::DeviceRevoked {
                    device_id: device_id.to_string(),
                    remaining_active,
                },
            );
        }
        revoked
    }

    /// 服务一条远程连接。承载无关：接受任意 GuiConnection 实现
    /// （transport-remote 承载集成证据见 tests/transport_remote_carrier.rs）。
    pub async fn serve_connection(&self, connection: Box<dyn GuiConnection>) -> ConnectionSummary {
        let conn: Arc<dyn GuiConnection> = Arc::from(connection);
        let (out_tx, out_rx) = mpsc::channel::<ServerFrame>(self.config.outbound_capacity);

        let send_task = tokio::spawn(send_loop(Arc::clone(&conn), out_rx));
        // 通知泵延迟到首次认证成功（PreAuth → Authenticated）后才启动：
        // PreAuth 期间绝不对对端推送任何 Notification（RunFinished /
        // ApprovalRequested / …），fail-closed；已连接客户端认证后可经 Replay
        // 从共享通知日志补齐认证前的历史通知，无需 PreAuth 阶段实时推送。
        let mut pump_task: Option<tokio::task::JoinHandle<()>> = None;

        let mut state = ConnState::PreAuth { failures: 0 };
        let mut frames_handled = 0u64;
        let close_reason = loop {
            let frame = match conn.receive().await {
                Ok(frame) => frame,
                Err(error) => {
                    break if error.kind == TransportErrorKind::ConnectionClosed {
                        CloseReason::ClientClosed
                    } else {
                        CloseReason::TransportError(error.kind)
                    };
                }
            };
            frames_handled += 1;
            if frame.as_bytes().len() as u64 > self.config.max_frame_bytes {
                self.audit.record(
                    state.actor(),
                    AuditEvent::ProtocolViolation {
                        detail: "frame exceeds remote-control max_frame_bytes".to_string(),
                    },
                );
                break CloseReason::ProtocolViolation;
            }
            let client_frame = match wire::decode_client_frame(frame.as_bytes()) {
                Ok(client_frame) => client_frame,
                Err(error) => {
                    self.audit.record(
                        state.actor(),
                        AuditEvent::ProtocolViolation {
                            detail: error.to_string(),
                        },
                    );
                    send(
                        &out_tx,
                        ServerFrame::Error {
                            request_id: None,
                            code: "protocol_violation".to_string(),
                            message: "frame is not a valid remote-control client frame".to_string(),
                        },
                    )
                    .await;
                    continue;
                }
            };
            let was_authenticated = state.device_id().is_some();
            if let Some(reason) = self.handle_frame(&out_tx, &mut state, client_frame).await {
                break reason;
            }
            // 首次认证成功（PreAuth → Authenticated）才启动通知推送；
            // PreAuth 连接永不推送 Notification（fail-closed，P17-12）。
            // 订阅在本任务内同步创建后传入泵：认证成功与订阅之间不再隔
            // tokio::spawn 调度窗口，Core 在认证后发布的事件不会被错过。
            if !was_authenticated && state.device_id().is_some() && pump_task.is_none() {
                let subscription = self.hub.subscribe();
                pump_task = Some(tokio::spawn(pump_notifications(
                    subscription,
                    self.notifications.clone(),
                    self.audit.clone(),
                    out_tx.clone(),
                )));
            }
        };

        self.audit.record(
            state.actor(),
            AuditEvent::ConnectionClosed {
                reason: format!("{close_reason:?}"),
            },
        );
        drop(out_tx);
        // 等待出站刷新（慢客户端不无限等待）。
        if tokio::time::timeout(FLUSH_TIMEOUT, send_task)
            .await
            .is_err()
        {
            // 超时不 abort：任务会随通道关闭自行退出；此处仅记录。
            warn!("remote-control: outbound flush timed out on connection close");
        }
        if let Some(pump) = pump_task {
            pump.abort();
        }
        let _ = conn.close().await;

        ConnectionSummary {
            device_id: state.into_device_id(),
            frames_handled,
            close_reason,
        }
    }

    /// 处理一帧；返回 Some(关闭原因) 时结束连接。
    async fn handle_frame(
        &self,
        out: &mpsc::Sender<ServerFrame>,
        state: &mut ConnState,
        frame: ClientFrame,
    ) -> Option<CloseReason> {
        match frame {
            ClientFrame::Pair {
                request_id,
                device_label,
            } => {
                if state.device_id().is_some() {
                    send(
                        out,
                        error_frame(
                            Some(request_id),
                            "already_authenticated",
                            "connection is already authenticated",
                        ),
                    )
                    .await;
                    return None;
                }
                match self.pairing.issue_pairing(&device_label, now_unix_ms()) {
                    Ok(issued) => {
                        self.audit.record(
                            "anonymous",
                            AuditEvent::PairingCodeIssued {
                                pairing_id: issued.pairing_id.clone(),
                                device_label,
                            },
                        );
                        send(
                            out,
                            ServerFrame::PairChallenge {
                                request_id,
                                pairing_id: issued.pairing_id,
                                pairing_code: issued.pairing_code,
                                expires_in_ms: issued.expires_in_ms,
                            },
                        )
                        .await;
                    }
                    Err(PairingError::CapacityExhausted) => {
                        self.audit.record(
                            "anonymous",
                            AuditEvent::AuthenticationFailed {
                                reason: "pairing_capacity_exhausted".to_string(),
                            },
                        );
                        send(
                            out,
                            error_frame(
                                Some(request_id),
                                "pairing_capacity_exhausted",
                                "pending pairing capacity exhausted; retry later",
                            ),
                        )
                        .await;
                    }
                    Err(other) => {
                        send(
                            out,
                            error_frame(Some(request_id), "pairing_error", &other.to_string()),
                        )
                        .await;
                    }
                }
                None
            }
            ClientFrame::Activate {
                request_id,
                pairing_code,
            } => {
                if state.device_id().is_some() {
                    send(
                        out,
                        error_frame(
                            Some(request_id),
                            "already_authenticated",
                            "connection is already authenticated",
                        ),
                    )
                    .await;
                    return None;
                }
                match self.pairing.activate(&pairing_code, now_unix_ms()) {
                    Ok(activation) => {
                        self.audit.record(
                            "anonymous",
                            AuditEvent::DevicePaired {
                                device_id: activation.device_id.clone(),
                                device_label: activation.device_label.clone(),
                            },
                        );
                        self.audit.record(
                            &activation.device_id,
                            AuditEvent::DeviceAuthenticated {
                                device_id: activation.device_id.clone(),
                            },
                        );
                        *state = ConnState::Authenticated {
                            device_id: activation.device_id.clone(),
                        };
                        send(
                            out,
                            ServerFrame::Activated {
                                request_id,
                                device_id: activation.device_id,
                                credential: activation.credential,
                            },
                        )
                        .await;
                    }
                    Err(error) => {
                        return self
                            .handle_auth_failure(out, state, request_id, &error.to_string())
                            .await;
                    }
                }
                None
            }
            ClientFrame::Authenticate {
                request_id,
                device_id,
                credential,
            } => {
                if state.device_id().is_some() {
                    send(
                        out,
                        error_frame(
                            Some(request_id),
                            "already_authenticated",
                            "connection is already authenticated",
                        ),
                    )
                    .await;
                    return None;
                }
                match self
                    .pairing
                    .authenticate(&device_id, &credential, now_unix_ms())
                {
                    Ok(()) => {
                        self.audit.record(
                            &device_id,
                            AuditEvent::DeviceAuthenticated {
                                device_id: device_id.clone(),
                            },
                        );
                        *state = ConnState::Authenticated {
                            device_id: device_id.clone(),
                        };
                        send(
                            out,
                            ServerFrame::Authenticated {
                                request_id,
                                device_id,
                            },
                        )
                        .await;
                    }
                    Err(error) => {
                        return self
                            .handle_auth_failure(out, state, request_id, &error.to_string())
                            .await;
                    }
                }
                None
            }
            ClientFrame::Command {
                request_id,
                command,
            } => {
                let Some(device_id) = state.device_id().map(str::to_string) else {
                    self.audit.record(
                        "anonymous",
                        AuditEvent::AuthenticationRequired {
                            operation: command.operation().to_string(),
                        },
                    );
                    send(
                        out,
                        error_frame(
                            Some(request_id),
                            "authentication_required",
                            "authenticate before issuing commands",
                        ),
                    )
                    .await;
                    return None;
                };
                if !self.pairing.is_active(&device_id) {
                    send(
                        out,
                        ServerFrame::Revoked {
                            device_id,
                            reason: "device credential has been revoked".to_string(),
                        },
                    )
                    .await;
                    return Some(CloseReason::Revoked);
                }
                let operation = command.operation();
                let app_command = command.into_app_command();
                match gate::classify_command(&app_command) {
                    Verdict::Allow => {
                        let envelope = self.command_envelope(&device_id, app_command);
                        let command_id = envelope.command_id.as_str().to_string();
                        let response = self.app_service.dispatch_envelope(envelope);
                        self.audit.record(
                            &device_id,
                            AuditEvent::CommandDispatched {
                                command_id,
                                operation: operation.to_string(),
                            },
                        );
                        send(
                            out,
                            ServerFrame::Response {
                                request_id,
                                response: response.response,
                            },
                        )
                        .await;
                    }
                    Verdict::Deny { code, reason } => {
                        self.audit.record(
                            &device_id,
                            AuditEvent::OperationDenied {
                                code: code.to_string(),
                                operation: operation.to_string(),
                            },
                        );
                        send(
                            out,
                            ServerFrame::Denied {
                                request_id,
                                code: code.to_string(),
                                reason: reason.to_string(),
                                operation: operation.to_string(),
                            },
                        )
                        .await;
                    }
                }
                None
            }
            ClientFrame::Query { request_id, query } => {
                let Some(device_id) = state.device_id().map(str::to_string) else {
                    self.audit.record(
                        "anonymous",
                        AuditEvent::AuthenticationRequired {
                            operation: query.operation().to_string(),
                        },
                    );
                    send(
                        out,
                        error_frame(
                            Some(request_id),
                            "authentication_required",
                            "authenticate before issuing queries",
                        ),
                    )
                    .await;
                    return None;
                };
                if !self.pairing.is_active(&device_id) {
                    send(
                        out,
                        ServerFrame::Revoked {
                            device_id,
                            reason: "device credential has been revoked".to_string(),
                        },
                    )
                    .await;
                    return Some(CloseReason::Revoked);
                }
                let operation = query.operation();
                match &query {
                    RemoteQuery::PlanStatus { session_id } => {
                        // 计划状态：Core 单一事实源。经 SessionGet 代理；Core 未
                        // 暴露专用 plan 查询前返回显式可用性标记，绝不伪造计划。
                        let envelope = self.query_envelope(
                            &device_id,
                            AppQuery::SessionGet {
                                session_id: session_id.clone(),
                            },
                        );
                        let internal_request_id = envelope.request_id.as_str().to_string();
                        let response = self.app_service.dispatch_query(envelope);
                        self.audit.record(
                            &device_id,
                            AuditEvent::QueryDispatched {
                                request_id: internal_request_id,
                                operation: "plan_status".to_string(),
                            },
                        );
                        let response = match response.response {
                            AppResponse::Data(session) => AppResponse::Data(serde_json::json!({
                                "session_id": session_id,
                                "session": session,
                                "plan": null,
                                "plan_availability": "not_exposed_by_core",
                            })),
                            other => other,
                        };
                        send(
                            out,
                            ServerFrame::Response {
                                request_id,
                                response,
                            },
                        )
                        .await;
                    }
                    _ => {
                        let app_query = query
                            .as_app_query()
                            .expect("non-PlanStatus queries always map to canonical queries");
                        match gate::classify_query(&app_query) {
                            Verdict::Allow => {
                                let envelope = self.query_envelope(&device_id, app_query);
                                let internal_request_id = envelope.request_id.as_str().to_string();
                                let response = self.app_service.dispatch_query(envelope);
                                self.audit.record(
                                    &device_id,
                                    AuditEvent::QueryDispatched {
                                        request_id: internal_request_id,
                                        operation: operation.to_string(),
                                    },
                                );
                                send(
                                    out,
                                    ServerFrame::Response {
                                        request_id,
                                        response: response.response,
                                    },
                                )
                                .await;
                            }
                            Verdict::Deny { code, reason } => {
                                self.audit.record(
                                    &device_id,
                                    AuditEvent::OperationDenied {
                                        code: code.to_string(),
                                        operation: operation.to_string(),
                                    },
                                );
                                send(
                                    out,
                                    ServerFrame::Denied {
                                        request_id,
                                        code: code.to_string(),
                                        reason: reason.to_string(),
                                        operation: operation.to_string(),
                                    },
                                )
                                .await;
                            }
                        }
                    }
                }
                None
            }
            ClientFrame::Replay {
                request_id,
                from_seq,
            } => {
                let Some(device_id) = state.device_id().map(str::to_string) else {
                    self.audit.record(
                        "anonymous",
                        AuditEvent::AuthenticationRequired {
                            operation: "replay".to_string(),
                        },
                    );
                    send(
                        out,
                        error_frame(
                            Some(request_id),
                            "authentication_required",
                            "authenticate before replaying notifications",
                        ),
                    )
                    .await;
                    return None;
                };
                if !self.pairing.is_active(&device_id) {
                    send(
                        out,
                        ServerFrame::Revoked {
                            device_id,
                            reason: "device credential has been revoked".to_string(),
                        },
                    )
                    .await;
                    return Some(CloseReason::Revoked);
                }
                match self.notifications.replay(from_seq) {
                    Ok(notifications) => {
                        self.audit.record(
                            &device_id,
                            AuditEvent::ReplayServed {
                                from_seq,
                                count: notifications.len(),
                            },
                        );
                        send(
                            out,
                            ServerFrame::Replayed {
                                request_id,
                                notifications,
                            },
                        )
                        .await;
                    }
                    Err(gap) => {
                        self.audit.record(
                            &device_id,
                            AuditEvent::ReplayGapServed {
                                requested_from: gap.requested_from,
                                earliest_available: gap.earliest_available,
                            },
                        );
                        send(
                            out,
                            ServerFrame::ReplayGap {
                                request_id: Some(request_id),
                                requested_from: gap.requested_from,
                                earliest_available: gap.earliest_available,
                            },
                        )
                        .await;
                    }
                }
                None
            }
        }
    }

    /// 认证失败统一处理：审计 + 错误帧；超过连接级上限则关闭。
    async fn handle_auth_failure(
        &self,
        out: &mpsc::Sender<ServerFrame>,
        state: &mut ConnState,
        request_id: String,
        reason: &str,
    ) -> Option<CloseReason> {
        let failures = state.failures_mut().expect("auth frames only run pre-auth");
        *failures += 1;
        self.audit.record(
            "anonymous",
            AuditEvent::AuthenticationFailed {
                reason: reason.to_string(),
            },
        );
        send(
            out,
            error_frame(Some(request_id), "authentication_failed", reason),
        )
        .await;
        if *failures >= self.config.max_auth_failures {
            Some(CloseReason::AuthFailuresExceeded)
        } else {
            None
        }
    }

    fn command_envelope(&self, device_id: &str, command: AppCommand) -> AppCommandEnvelope {
        let sequence = self.next_internal_id.fetch_add(1, Ordering::Relaxed);
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from(format!("rc-command-{sequence}")),
            source: CommandSource::Automation,
            identity: ActorIdentity::AuthenticatedClient {
                actor_id: ActorId::from(device_id.to_string()),
                subject: SUBJECT.to_string(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(now_unix_ms()),
            command,
        }
    }

    fn query_envelope(&self, device_id: &str, query: AppQuery) -> AppQueryEnvelope {
        let sequence = self.next_internal_id.fetch_add(1, Ordering::Relaxed);
        AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from(format!("rc-query-{sequence}")),
            source: CommandSource::Automation,
            identity: ActorIdentity::AuthenticatedClient {
                actor_id: ActorId::from(device_id.to_string()),
                subject: SUBJECT.to_string(),
            },
            issued_at: Timestamp::from_unix_millis(now_unix_ms()),
            query,
        }
    }
}

/// 出站发送循环：单一 mpsc → 单一发送者，保证帧序。
async fn send_loop(conn: Arc<dyn GuiConnection>, mut receiver: mpsc::Receiver<ServerFrame>) {
    while let Some(frame) = receiver.recv().await {
        let bytes = match wire::encode_server_frame(&frame) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        if conn.send(TransportFrame::new(bytes)).await.is_err() {
            let _ = conn.close().await;
            break;
        }
    }
}

/// 事件泵：EventHub → 通知映射 → 通知日志 → 每连接有界出站队列。
///
/// 订阅句柄由调用方（serve_connection）在首次认证成功时同步创建并传入，
/// 泵内部不再订阅：避免 spawn 调度延迟造成认证后事件丢失。
///
/// 背压策略（有界 + 显式）：出站队列满时丢弃当条推送并累积缺口区间，
/// 下一次成功发送前以 PushGap 帧显式告知客户端（通知本体仍在日志中，
/// 可用 Replay 补齐）。Hub 订阅滞后（Lagged）时审计并告警：被错过的事件
/// 无法映射，通知序列空间保持连续，客户端以查询重建状态。
async fn pump_notifications(
    mut subscription: HubSubscription,
    notifications: NotificationLog,
    audit: AuditLog,
    out: mpsc::Sender<ServerFrame>,
) {
    let mut pending_gap: Option<(u64, u64)> = None;
    loop {
        let envelope = match subscription.recv().await {
            Ok(envelope) => envelope,
            Err(HubError::Lagged { missed }) => {
                audit.record("system", AuditEvent::HubLagged { missed });
                warn!(
                    missed,
                    "remote-control: event hub lagged; missed events are not mapped"
                );
                continue;
            }
            Err(HubError::Closed) => break,
            Err(HubError::Empty) => continue,
            // `recv` 目前不会产生 replay 错误；仍显式 fail-closed，避免 Hub API
            // 演进后把不可恢复的序列缺口静默当作正常流继续处理。
            Err(HubError::ReplayUnavailable {
                requested_from,
                earliest_available,
            }) => {
                let from_seq = requested_from.0;
                let to_seq = earliest_available.0.saturating_sub(1);
                audit.record(
                    "system",
                    AuditEvent::PushGap {
                        from_seq,
                        to_seq,
                        reason: "hub_replay_unavailable".to_string(),
                    },
                );
                let _ = out
                    .send(ServerFrame::PushGap {
                        from_seq,
                        to_seq,
                        reason: "hub_replay_unavailable".to_string(),
                    })
                    .await;
                break;
            }
        };
        let Some(notification) = notifications.push_mapped(envelope) else {
            continue;
        };
        let seq = notification.seq;
        // 先补发此前背压造成的显式缺口标记。
        if let Some((from_seq, to_seq)) = pending_gap.take() {
            match out.try_send(ServerFrame::PushGap {
                from_seq,
                to_seq,
                reason: "outbound_backlog".to_string(),
            }) {
                Ok(()) => {
                    audit.record(
                        "system",
                        AuditEvent::PushGap {
                            from_seq,
                            to_seq,
                            reason: "outbound_backlog".to_string(),
                        },
                    );
                }
                Err(TrySendError::Full(_)) => {
                    pending_gap = Some((from_seq, to_seq));
                }
                Err(TrySendError::Closed(_)) => break,
            }
        }
        let frame = ServerFrame::Notification {
            seq: notification.seq,
            event_id: notification.event_id,
            occurred_at_ms: notification.occurred_at_ms,
            payload: notification.payload,
        };
        match out.try_send(frame) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                pending_gap = Some(match pending_gap {
                    Some((from_seq, _)) => (from_seq, seq),
                    None => (seq, seq),
                });
            }
            Err(TrySendError::Closed(_)) => break,
        }
    }
}

/// 发送请求/响应帧（允许阻塞至队列有空位；背压丢弃仅用于通知推送）。
async fn send(out: &mpsc::Sender<ServerFrame>, frame: ServerFrame) {
    let _ = out.send(frame).await;
}

fn error_frame(request_id: Option<String>, code: &str, message: &str) -> ServerFrame {
    ServerFrame::Error {
        request_id,
        code: code.to_string(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteControlService;
    use crate::notify::NotificationPayload;
    use crate::wire::{ClientFrame, ServerFrame};

    use std::sync::Arc;
    use std::time::Duration;

    use agent_domain::{CoreInstanceId, EventId, RunId, Timestamp, ToolCallId};
    use app_service::AppService;
    use core_api::{
        AppEvent, AppEventEnvelope, EventSource, EventStream, GlobalSequence, RunState,
    };
    use subscription_hub::EventHub;
    use tokio::time::{sleep, timeout};
    use transport_api::{
        ConnectOptions, GuiConnection, GuiListener, GuiTransportClient, GuiTransportServer,
        TransportEndpoint,
    };
    use transport_memory::MemoryTransport;

    /// PreAuth 期间即使 Core 持续发出 RunFinished / ApprovalRequested，
    /// 未认证对端也收不到任何 Notification（fail-closed，P17-12）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preauth_receives_no_notification_even_when_core_emits() {
        let app = Arc::new(AppService::new("rc-preauth-gate"));
        let hub = Arc::new(EventHub::new());
        let service = RemoteControlService::new(Arc::clone(&app), Arc::clone(&hub));

        let transport = MemoryTransport::new();
        let endpoint = TransportEndpoint::Memory {
            channel: "preauth-gate".into(),
        };
        let listener = transport.bind(endpoint.clone()).await.expect("bind");
        let listener: Arc<dyn GuiListener> = Arc::from(listener);
        let accept_service = service.clone();
        tokio::spawn(async move {
            while let Ok(connection) = listener.accept().await {
                let serving = accept_service.clone();
                tokio::spawn(async move {
                    let _summary = serving.serve_connection(connection).await;
                });
            }
        });

        // ---- PreAuth 对端：连接但不认证。----
        let preauth = transport
            .connect(
                endpoint.clone(),
                ConnectOptions {
                    timeout_ms: 2_000,
                    client_label: Some("preauth".into()),
                    max_frame_bytes: 1024 * 1024,
                },
            )
            .await
            .expect("connect");

        // Core 在 PreAuth 阶段持续发出两类会映射为 Notification 的事件。
        hub.publish(run_finished_event("preauth-run", 1));
        hub.publish(approval_event("preauth-approval", 2));
        // 给（不存在的）通知泵一个窗口：仍不应有任何帧到达 PreAuth 对端。
        sleep(Duration::from_millis(150)).await;
        if let Ok(Ok(frame)) = timeout(Duration::from_millis(300), preauth.as_ref().receive()).await
        {
            let frame: ServerFrame =
                serde_json::from_slice(frame.as_bytes()).expect("decode server frame");
            panic!("PreAuth 对端不应收到任何帧，收到 {frame:?}");
        }

        // 存活性检查：同一 PreAuth 连接仍可收到 auth-gate 拒绝响应，
        // 证明上面的“无通知”来自 fail-closed 门禁，而非死连接。
        let replay_response = rpc(
            preauth.as_ref(),
            "preauth-replay",
            ClientFrame::Replay {
                request_id: "preauth-replay".into(),
                from_seq: 1,
            },
        )
        .await;
        match replay_response {
            ServerFrame::Error {
                request_id, code, ..
            } => {
                assert_eq!(request_id.as_deref(), Some("preauth-replay"));
                assert_eq!(code, "authentication_required");
            }
            other => panic!("expected authentication_required, got {other:?}"),
        }

        // ---- 正向对照：认证后同一 Hub 事件会被推送。----
        let authed = transport
            .connect(
                endpoint.clone(),
                ConnectOptions {
                    timeout_ms: 2_000,
                    client_label: Some("authed".into()),
                    max_frame_bytes: 1024 * 1024,
                },
            )
            .await
            .expect("connect");
        // 配对 + 激活（凭配对码兑换，无需宿主额外审批）。
        let challenge = rpc(
            authed.as_ref(),
            "pair",
            ClientFrame::Pair {
                request_id: "pair".into(),
                device_label: "control".into(),
            },
        )
        .await;
        let pairing_code = match challenge {
            ServerFrame::PairChallenge {
                pairing_code,
                expires_in_ms,
                ..
            } => {
                assert!(expires_in_ms > 0);
                pairing_code
            }
            other => panic!("expected pair challenge, got {other:?}"),
        };
        let activated = rpc(
            authed.as_ref(),
            "activate",
            ClientFrame::Activate {
                request_id: "activate".into(),
                pairing_code,
            },
        )
        .await;
        match activated {
            ServerFrame::Activated { device_id, .. } => {
                assert!(!device_id.is_empty());
            }
            other => panic!("expected activated, got {other:?}"),
        }

        // 确定性锁定订阅时点：Replay 帧在 serve_connection 中严格晚于同步
        // subscribe() 处理，收到 Replayed 响应即保证订阅已建立——无需 sleep，
        // 并锁死「认证成功到订阅之间丢失事件」的竞态回归。
        let replayed = rpc(
            authed.as_ref(),
            "authed-replay-barrier",
            ClientFrame::Replay {
                request_id: "authed-replay-barrier".into(),
                from_seq: 1,
            },
        )
        .await;
        match replayed {
            ServerFrame::Replayed { request_id, .. } => {
                assert_eq!(request_id, "authed-replay-barrier");
            }
            other => panic!("expected replayed barrier, got {other:?}"),
        }
        hub.publish(run_finished_event("authed-run", 3));
        let notification = recv_matching(authed.as_ref(), |frame| match frame {
            ServerFrame::Notification {
                payload: NotificationPayload::RunFinished { run_id, .. },
                ..
            } => *run_id == RunId::from("authed-run"),
            _ => false,
        })
        .await;
        match notification {
            ServerFrame::Notification { seq, payload, .. } => {
                assert!(seq >= 1);
                assert!(matches!(
                    payload,
                    NotificationPayload::RunFinished {
                        state: RunState::Completed,
                        ..
                    }
                ));
            }
            _ => unreachable!(),
        }

        let _ = preauth.close().await;
        let _ = authed.close().await;
    }

    async fn rpc(conn: &dyn GuiConnection, request_id: &str, frame: ClientFrame) -> ServerFrame {
        let bytes = serde_json::to_vec(&frame).expect("encode client frame");
        conn.send(transport_api::TransportFrame::new(bytes))
            .await
            .expect("send");
        let wanted = request_id.to_string();
        recv_matching(conn, move |frame| match frame {
            ServerFrame::PairChallenge { request_id, .. }
            | ServerFrame::Activated { request_id, .. }
            | ServerFrame::Authenticated { request_id, .. }
            | ServerFrame::Response { request_id, .. }
            | ServerFrame::Denied { request_id, .. }
            | ServerFrame::Replayed { request_id, .. } => *request_id == wanted,
            ServerFrame::Error {
                request_id: Some(request_id),
                ..
            } => *request_id == wanted,
            _ => false,
        })
        .await
    }

    async fn recv_matching(
        conn: &dyn GuiConnection,
        mut matches: impl FnMut(&ServerFrame) -> bool,
    ) -> ServerFrame {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(remaining > Duration::ZERO, "recv_matching timeout");
            let frame = timeout(remaining, conn.receive())
                .await
                .expect("recv timeout")
                .expect("receive frame");
            let frame: ServerFrame =
                serde_json::from_slice(frame.as_bytes()).expect("decode server frame");
            if matches(&frame) {
                return frame;
            }
        }
    }

    fn run_finished_event(run_id: &str, sequence: u64) -> AppEventEnvelope {
        AppEventEnvelope {
            api_version: core_api::API_VERSION,
            instance_id: CoreInstanceId::from("rc-preauth-gate"),
            event_id: EventId::from(format!("run-finished-{sequence}")),
            global_sequence: GlobalSequence(sequence),
            stream: EventStream::Run(RunId::from(run_id)),
            stream_sequence: sequence,
            timestamp: Timestamp::from_unix_millis(sequence),
            source: EventSource::Core,
            payload: AppEvent::RunChanged {
                run_id: RunId::from(run_id),
                state: RunState::Completed,
            },
        }
    }

    fn approval_event(prefix: &str, sequence: u64) -> AppEventEnvelope {
        AppEventEnvelope {
            api_version: core_api::API_VERSION,
            instance_id: CoreInstanceId::from("rc-preauth-gate"),
            event_id: EventId::from(format!("approval-{prefix}-{sequence}")),
            global_sequence: GlobalSequence(sequence),
            stream: EventStream::Global,
            stream_sequence: sequence,
            timestamp: Timestamp::from_unix_millis(sequence),
            source: EventSource::Core,
            payload: AppEvent::ToolApprovalRequired {
                run_id: RunId::from(format!("{prefix}-run")),
                tool_call_id: ToolCallId::from(format!("{prefix}-tool")),
                reason: "needs approval".into(),
            },
        }
    }
}
