//! GUI 连接管理器：多 GUI 在线管理。
//!
//! [`ConnectionManager`] 登记一个 CLI/Core 实例上的全部 GUI 连接
//! （[`GuiClientSession`]），维护心跳、订阅与每连接独立的**有界**事件队列：
//!
//! - **心跳超时断线清理，但绝不取消 Run**：断线只影响连接记录与事件投递，
//!   Run 由 `RunCancel` 显式取消（ADR-026）。
//! - **慢客户端隔离**：每连接队列容量固定（默认 1024），满则标记
//!   `session.lagged` 并丢弃**新**事件，绝不阻塞发布者或其他 GUI。
//! - 订阅按 `subscription_id` 去重；`streams` 为空表示订阅全部事件流。

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use pawork_domain::{ConnectionId, GuiClientId, Timestamp};
use pawork_protocol::{ActorIdentity, AppEventEnvelope, EventStream, GlobalSequence, GuiCapability};
use thiserror::Error;
use tokio::sync::mpsc;
use pawork_transport::ConnectionLocality;

/// 默认心跳超时：连接在超时内没有任何入站帧即视为断线。
pub const DEFAULT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);
/// 默认每连接事件队列容量。
pub const DEFAULT_QUEUE_CAPACITY: usize = 1024;

/// 连接管理器配置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionManagerConfig {
    /// 心跳超时（`is_timed_out` 判定窗口）。
    pub heartbeat_timeout: Duration,
    /// 每连接事件队列容量（有界，满则标记 Lagged）。
    pub queue_capacity: usize,
}

impl Default for ConnectionManagerConfig {
    fn default() -> Self {
        Self {
            heartbeat_timeout: DEFAULT_HEARTBEAT_TIMEOUT,
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
        }
    }
}

/// 连接管理器错误。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ManagerError {
    #[error("GUI client {0} is not registered")]
    UnknownClient(GuiClientId),
    #[error("GUI client {0} is already registered")]
    AlreadyRegistered(GuiClientId),
    #[error("GUI client {client_id} fell behind and events were dropped (slow client)")]
    Lagged { client_id: GuiClientId },
    #[error("event queue for GUI client {0} is closed")]
    ChannelClosed(GuiClientId),
}

/// 一次 GUI 连接的注册输入（握手成功后的会话元数据）。
#[derive(Clone, Debug, PartialEq)]
pub struct ClientRegistration {
    pub client_id: GuiClientId,
    pub connection_id: ConnectionId,
    /// 握手声明的客户端名（如 `desktop` / `protocol-test-gui`）。
    pub name: String,
    /// 握手声明的客户端版本。
    pub version: String,
    pub locality: ConnectionLocality,
    /// 认证后的身份；握手凭证未携带身份时为 `None`（后续接入时填充）。
    pub identity: Option<ActorIdentity>,
    /// 服务端按能力筛选后授予的 capabilities。
    pub capabilities: Vec<GuiCapability>,
    pub connected_at: Timestamp,
}

/// 一条 GUI 订阅（`subscription_id` 唯一；`streams` 为空表示全部事件流）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuiSubscription {
    pub subscription_id: String,
    pub streams: Vec<EventStream>,
}

/// 在线 GUI 客户端会话（管理器的只读视图；`register` 返回值中可读）。
#[derive(Clone, Debug, PartialEq)]
pub struct GuiClientSession {
    pub client_id: GuiClientId,
    pub connection_id: ConnectionId,
    pub name: String,
    pub version: String,
    pub locality: ConnectionLocality,
    pub identity: Option<ActorIdentity>,
    pub capabilities: Vec<GuiCapability>,
    pub connected_at: Timestamp,
    pub last_heartbeat_at: Timestamp,
    pub last_ack: Option<GlobalSequence>,
    pub subscriptions: Vec<GuiSubscription>,
    /// 事件队列曾因消费过慢而丢事件（慢客户端标记；不阻塞任何发布者）。
    pub lagged: bool,
}

struct SessionEntry {
    session: GuiClientSession,
    queue: mpsc::Sender<AppEventEnvelope>,
}

struct Inner {
    config: ConnectionManagerConfig,
    sessions: BTreeMap<GuiClientId, SessionEntry>,
}

/// 多 GUI 连接管理器：注册 / 心跳 / 订阅 / 有界事件队列。
///
/// 内部为 `Mutex<BTreeMap>`，全部方法同步、廉价；跨任务共享请用 `Arc`。
pub struct ConnectionManager {
    inner: Mutex<Inner>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self::with_config(ConnectionManagerConfig::default())
    }

    pub fn with_config(config: ConnectionManagerConfig) -> Self {
        Self {
            inner: Mutex::new(Inner {
                config,
                sessions: BTreeMap::new(),
            }),
        }
    }

    /// 当前配置（副本）。
    pub fn config(&self) -> ConnectionManagerConfig {
        lock(&self.inner).config.clone()
    }

    /// 注册一条 GUI 连接，返回该连接的事件队列接收端（有界）。
    ///
    /// 队列发送端由管理器持有；`unregister`（或断线）后接收端返回 `None`。
    pub fn register(
        &self,
        registration: ClientRegistration,
    ) -> Result<mpsc::Receiver<AppEventEnvelope>, ManagerError> {
        let mut inner = lock(&self.inner);
        if inner.sessions.contains_key(&registration.client_id) {
            return Err(ManagerError::AlreadyRegistered(registration.client_id));
        }
        let (queue, receiver) = mpsc::channel(inner.config.queue_capacity);
        let connected_at = registration.connected_at;
        let session = GuiClientSession {
            client_id: registration.client_id.clone(),
            connection_id: registration.connection_id,
            name: registration.name,
            version: registration.version,
            locality: registration.locality,
            identity: registration.identity,
            capabilities: registration.capabilities,
            connected_at,
            last_heartbeat_at: connected_at,
            last_ack: None,
            subscriptions: Vec::new(),
            lagged: false,
        };
        inner
            .sessions
            .insert(registration.client_id, SessionEntry { session, queue });
        Ok(receiver)
    }

    /// 注销连接（心跳超时 / 对端关闭 / 显式断线）。只移除连接记录与队列，
    /// **不取消任何 Run**。返回被移除的会话。
    pub fn unregister(&self, client_id: &GuiClientId) -> Option<GuiClientSession> {
        lock(&self.inner)
            .sessions
            .remove(client_id)
            .map(|entry| entry.session)
    }

    /// 当前会话数。
    pub fn count(&self) -> usize {
        lock(&self.inner).sessions.len()
    }

    /// 会话只读副本（按 client_id 排序）。
    pub fn session(&self, client_id: &GuiClientId) -> Option<GuiClientSession> {
        lock(&self.inner)
            .sessions
            .get(client_id)
            .map(|entry| entry.session.clone())
    }

    /// 记录一次活跃证据（心跳 / Pong / 任意入站帧），刷新 `last_heartbeat_at`。
    pub fn heartbeat(&self, client_id: &GuiClientId, now: Timestamp) -> Result<(), ManagerError> {
        let mut inner = lock(&self.inner);
        let entry = inner
            .sessions
            .get_mut(client_id)
            .ok_or_else(|| ManagerError::UnknownClient(client_id.clone()))?;
        entry.session.last_heartbeat_at = now;
        Ok(())
    }

    /// 记录客户端确认（`Ack`），供重连时按 `last_global_sequence` 恢复。
    pub fn ack(
        &self,
        client_id: &GuiClientId,
        sequence: GlobalSequence,
    ) -> Result<(), ManagerError> {
        let mut inner = lock(&self.inner);
        let entry = inner
            .sessions
            .get_mut(client_id)
            .ok_or_else(|| ManagerError::UnknownClient(client_id.clone()))?;
        let previous = entry.session.last_ack;
        if previous.is_none_or(|previous| sequence > previous) {
            entry.session.last_ack = Some(sequence);
        }
        Ok(())
    }

    /// 会话最近确认序列（重连握手 resume 上下文用）。
    pub fn last_ack(&self, client_id: &GuiClientId) -> Option<GlobalSequence> {
        lock(&self.inner)
            .sessions
            .get(client_id)
            .and_then(|entry| entry.session.last_ack)
    }

    /// 添加订阅；同 `subscription_id` 重复订阅时替换 streams。
    pub fn subscribe(
        &self,
        client_id: &GuiClientId,
        subscription_id: &str,
        streams: Vec<EventStream>,
    ) -> Result<(), ManagerError> {
        let mut inner = lock(&self.inner);
        let entry = inner
            .sessions
            .get_mut(client_id)
            .ok_or_else(|| ManagerError::UnknownClient(client_id.clone()))?;
        let subscription = GuiSubscription {
            subscription_id: subscription_id.to_string(),
            streams,
        };
        if let Some(existing) = entry
            .session
            .subscriptions
            .iter_mut()
            .find(|item| item.subscription_id == subscription_id)
        {
            *existing = subscription;
        } else {
            entry.session.subscriptions.push(subscription);
        }
        Ok(())
    }

    /// 移除订阅（幂等：不存在也返回 Ok）。
    pub fn unsubscribe(
        &self,
        client_id: &GuiClientId,
        subscription_id: &str,
    ) -> Result<(), ManagerError> {
        let mut inner = lock(&self.inner);
        let entry = inner
            .sessions
            .get_mut(client_id)
            .ok_or_else(|| ManagerError::UnknownClient(client_id.clone()))?;
        entry
            .session
            .subscriptions
            .retain(|item| item.subscription_id != subscription_id);
        Ok(())
    }

    /// 事件是否应投递给该连接：无订阅不投递；任一订阅 streams 为空（全量）或
    /// 包含该事件流则投递。
    pub fn should_forward(&self, client_id: &GuiClientId, stream: &EventStream) -> bool {
        let inner = lock(&self.inner);
        let Some(entry) = inner.sessions.get(client_id) else {
            return false;
        };
        entry.session.subscriptions.iter().any(|subscription| {
            subscription.streams.is_empty() || subscription.streams.contains(stream)
        })
    }

    /// 向连接的有界队列投递事件；队列满时标记 `lagged` 并返回
    /// [`ManagerError::Lagged`]（**新**事件被丢弃，不阻塞发布者）。
    pub fn enqueue(
        &self,
        client_id: &GuiClientId,
        event: AppEventEnvelope,
    ) -> Result<(), ManagerError> {
        let mut inner = lock(&self.inner);
        let entry = inner
            .sessions
            .get_mut(client_id)
            .ok_or_else(|| ManagerError::UnknownClient(client_id.clone()))?;
        match entry.queue.try_send(event) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                entry.session.lagged = true;
                Err(ManagerError::Lagged {
                    client_id: client_id.clone(),
                })
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(ManagerError::ChannelClosed(client_id.clone()))
            }
        }
    }

    /// 标记连接已落后（broadcast 订阅丢失事件时使用；不加新协议帧）。
    pub fn mark_lagged(&self, client_id: &GuiClientId) -> Result<(), ManagerError> {
        let mut inner = lock(&self.inner);
        let entry = inner
            .sessions
            .get_mut(client_id)
            .ok_or_else(|| ManagerError::UnknownClient(client_id.clone()))?;
        entry.session.lagged = true;
        Ok(())
    }

    /// 连接是否已超过心跳超时（未注销前的判定；清理由调用方执行）。
    pub fn is_timed_out(&self, client_id: &GuiClientId, now: Timestamp) -> bool {
        let inner = lock(&self.inner);
        let Some(entry) = inner.sessions.get(client_id) else {
            return false;
        };
        let timeout = inner.config.heartbeat_timeout.as_millis() as u64;
        now.as_unix_millis()
            .saturating_sub(entry.session.last_heartbeat_at.as_unix_millis())
            >= timeout
    }
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

fn lock(inner: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::{CoreInstanceId, EventId, RunId};
    use pawork_protocol::{AppEvent, EventSource, RunState, API_VERSION};

    fn now(millis: u64) -> Timestamp {
        Timestamp::from_unix_millis(millis)
    }

    fn registration(client_id: &str) -> ClientRegistration {
        ClientRegistration {
            client_id: GuiClientId::from(client_id),
            connection_id: ConnectionId::from(format!("conn-{client_id}")),
            name: "test-gui".into(),
            version: "0.0.1".into(),
            locality: ConnectionLocality::InProcess,
            identity: None,
            capabilities: vec![GuiCapability::Events, GuiCapability::Snapshots],
            connected_at: now(0),
        }
    }

    fn envelope(sequence: u64) -> AppEventEnvelope {
        AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("test-instance"),
            event_id: EventId::from(format!("event-{sequence}")),
            global_sequence: GlobalSequence(sequence),
            stream: EventStream::Run(RunId::from("run-1")),
            stream_sequence: sequence,
            timestamp: now(sequence),
            source: EventSource::Core,
            payload: AppEvent::RunChanged {
                run_id: RunId::from("run-1"),
                state: RunState::StreamingResponse,
            },
        }
    }

    #[test]
    fn register_tracks_session_and_duplicates_are_rejected() {
        let manager = ConnectionManager::new();
        let mut receiver = manager.register(registration("a")).expect("register");
        assert_eq!(manager.count(), 1);
        let session = manager.session(&GuiClientId::from("a")).expect("session");
        assert_eq!(session.name, "test-gui");
        assert_eq!(session.version, "0.0.1");
        assert_eq!(session.locality, ConnectionLocality::InProcess);
        assert_eq!(
            session.capabilities,
            vec![GuiCapability::Events, GuiCapability::Snapshots]
        );
        assert_eq!(session.connected_at, now(0));
        assert_eq!(session.last_heartbeat_at, now(0));
        assert_eq!(session.last_ack, None);
        assert!(!session.lagged);
        assert!(matches!(
            manager.register(registration("a")),
            Err(ManagerError::AlreadyRegistered(ref id)) if id == &GuiClientId::from("a")
        ));

        // unregister 后队列接收端关闭。
        manager.unregister(&GuiClientId::from("a"));
        assert_eq!(manager.count(), 0);
        assert!(manager.unregister(&GuiClientId::from("a")).is_none());
        // 发送端随注销释放：队列接收端断开。
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn heartbeat_refreshes_and_timeout_is_detected() {
        let manager = ConnectionManager::with_config(ConnectionManagerConfig {
            heartbeat_timeout: Duration::from_millis(100),
            queue_capacity: 4,
        });
        let client_id = GuiClientId::from("a");
        manager.register(registration("a")).expect("register");
        assert!(!manager.is_timed_out(&client_id, now(99)));
        assert!(manager.is_timed_out(&client_id, now(100)));

        manager.heartbeat(&client_id, now(100)).expect("heartbeat");
        assert!(!manager.is_timed_out(&client_id, now(199)));
        assert!(manager.is_timed_out(&client_id, now(200)));
    }

    #[test]
    fn ack_records_monotonic_last_ack() {
        let manager = ConnectionManager::new();
        let client_id = GuiClientId::from("a");
        manager.register(registration("a")).expect("register");
        manager.ack(&client_id, GlobalSequence(3)).expect("ack");
        manager
            .ack(&client_id, GlobalSequence(2))
            .expect("ack older ignored");
        assert_eq!(manager.last_ack(&client_id), Some(GlobalSequence(3)));
    }

    #[test]
    fn subscribe_unsubscribe_and_stream_filtering() {
        let manager = ConnectionManager::new();
        let client_id = GuiClientId::from("a");
        manager.register(registration("a")).expect("register");
        let run_stream = EventStream::Run(RunId::from("run-1"));
        let other_stream = EventStream::Run(RunId::from("run-2"));

        // 未订阅不投递。
        assert!(!manager.should_forward(&client_id, &run_stream));

        // 空 streams = 全量。
        manager
            .subscribe(&client_id, "sub-1", vec![])
            .expect("subscribe all");
        assert!(manager.should_forward(&client_id, &run_stream));
        assert!(manager.should_forward(&client_id, &other_stream));

        // 同 id 重复订阅替换。
        manager
            .subscribe(&client_id, "sub-1", vec![run_stream.clone()])
            .expect("resubscribe");
        assert!(manager.should_forward(&client_id, &run_stream));
        assert!(!manager.should_forward(&client_id, &other_stream));

        // unsubscribe 幂等。
        manager
            .unsubscribe(&client_id, "sub-1")
            .expect("unsubscribe");
        manager
            .unsubscribe(&client_id, "sub-1")
            .expect("idempotent unsubscribe");
        assert!(!manager.should_forward(&client_id, &run_stream));
    }

    #[test]
    fn bounded_queue_marks_lagged_without_blocking() {
        let manager = ConnectionManager::with_config(ConnectionManagerConfig {
            heartbeat_timeout: Duration::from_secs(30),
            queue_capacity: 2,
        });
        let client_id = GuiClientId::from("a");
        let mut receiver = manager.register(registration("a")).expect("register");

        manager.enqueue(&client_id, envelope(1)).expect("first");
        manager.enqueue(&client_id, envelope(2)).expect("second");
        assert_eq!(
            manager.enqueue(&client_id, envelope(3)),
            Err(ManagerError::Lagged {
                client_id: client_id.clone()
            })
        );
        assert!(manager.session(&client_id).expect("session").lagged);

        // 队列仍有界保留前两条；慢客户端不阻塞其他连接（可继续注册）。
        let other = GuiClientId::from("b");
        let mut _other_receiver = manager.register(registration("b")).expect("register other");
        manager
            .enqueue(&other, envelope(10))
            .expect("other client unaffected");
        assert_eq!(
            _other_receiver
                .try_recv()
                .expect("other event")
                .global_sequence,
            GlobalSequence(10)
        );

        assert_eq!(
            receiver.try_recv().expect("first buffered").global_sequence,
            GlobalSequence(1)
        );
        assert_eq!(
            receiver
                .try_recv()
                .expect("second buffered")
                .global_sequence,
            GlobalSequence(2)
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn unregister_closes_queue_and_enqueue_reports_unknown() {
        let manager = ConnectionManager::new();
        let client_id = GuiClientId::from("a");
        let mut receiver = manager.register(registration("a")).expect("register");
        manager.unregister(&client_id);
        assert_eq!(
            manager.enqueue(&client_id, envelope(1)),
            Err(ManagerError::UnknownClient(client_id.clone()))
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
    }
}
