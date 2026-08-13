//! P17-11 真实远程 GUI Transport：TCP + TLS 1.3 加密、Token 认证、按序号
//! 有界续传 / 快照信号、端点发布 / 撤销（revoke）。
//!
//! 在 P13-6 占位接口（[`RemoteGuiTransportProvider`] / [`RemoteGuiConnector`]）
//! 之上落地真实实现（[ADR-028]）：
//!
//! - **加密**：每个端点生成独立自签名证书（内存态私钥），端点地址携带证书
//!   SHA-256 指纹（`#fp=`），客户端按指纹固定证书（certificate pinning），
//!   TLS 1.3 由 rustls 协商（[`tls`]）；
//! - **认证**：信封级 Auth 帧携带 `pawork-token` 凭证，服务端按 **端点独立
//!   凭证**（`publish` 时为每个端点单独生成，互不相同）常量时间校验，拒绝
//!   即关闭并审计；Secret 不落日志（[`session`]）；
//! - **续传 / 快照信号**：DATA 帧带单调 `seq`，交付即回 Ack（携带被确认帧
//!   的 payload 摘要，服务端只接受本会话实际发送且摘要一致的确认）；
//!   服务端以随机签发的 opaque resume identity 隔离有界重放窗口（并绑定
//!   `Auth` label），共享端点 token 与伪造 label 不能恢复别人的会话；重连时
//!   窗口内按序补发、窗口外回快照信号；`last_acked == 0` 一律显式回
//!   [`ResumeOutcome::SnapshotRequired`]（上层据此重新对齐），不猜测补发；
//! - **生命周期**：`publish` 建监听、生成地址与独立凭证；`unpublish` 移除
//!   端点并关闭监听与既有连接；`revoke` 额外置位撤销标志并 **真正失效**
//!   —— 从注册表移除、删除端点凭证文件，既有连接在帧循环中即时断开并审计。
//!
//! 帧内容保持 opaque [`TransportFrame`]，不含任何 Agent 业务逻辑，本地与
//! 远程复用同一 GUI Connection Protocol（[ADR-027]）。已知边界：传输层重放
//! 面向可容忍重放帧的消费方（透明重连）；gui-server 的按会话模型由协议层
//! `global_sequence` resume / Snapshot 负责（[ADR-030]），传输层只提供
//! 有界续传与快照信号。
//!
//! [ADR-027]: ../../docs/adr/ADR-027-local-remote-same-protocol.md
//! [ADR-028]: ../../docs/adr/ADR-028-replaceable-remote-transport.md
//! [ADR-030]: ../../docs/adr/ADR-030-core-sole-source-of-truth.md

mod connection;
mod session;
mod tls;
mod wire;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use client_auth::{Token, TokenStore};
use tokio::net::{TcpListener, TcpStream};

pub use connection::{ClientConnection, ResumeOutcome};
pub use transport_api::{
    ConnectOptions, ConnectionInfo, ConnectionLocality, GuiConnection, GuiListener,
    GuiTransportClient, GuiTransportServer, TransportEndpoint, TransportError, TransportErrorKind,
    TransportFrame,
};
pub use transport_remote_placeholder::{
    RemoteGuiConnector, RemoteGuiTransportProvider, RemotePublishHandle, RemotePublishRequest,
    RemoteTransportDescription,
};

/// Adapter 名（与 P13-6 占位接口一致，替换实现时更换）。
pub const ADAPTER_NAME: &str = "remote";
/// 默认单帧上限（字节），与 `gui-protocol::MAX_PROTOCOL_FRAME_BYTES` 一致。
pub const DEFAULT_MAX_FRAME_BYTES: u64 = 1024 * 1024;
/// 默认有界重放窗口（帧数）。
pub const DEFAULT_RESEND_WINDOW_FRAMES: usize = 1024;
/// 默认未确认 / 未交付缓冲上限（字节；发送窗口与接收队列共用）。
pub const DEFAULT_MAX_BUFFERED_BYTES: u64 = 8 * 1024 * 1024;

fn transport_error(kind: TransportErrorKind, message: impl Into<String>) -> TransportError {
    let retryable = matches!(
        &kind,
        TransportErrorKind::ConnectionFailed
            | TransportErrorKind::ConnectionClosed
            | TransportErrorKind::Timeout
    );
    TransportError {
        kind,
        message: message.into(),
        retryable,
    }
}

fn invalid_endpoint(message: impl Into<String>) -> TransportError {
    transport_error(TransportErrorKind::InvalidEndpoint, message)
}

fn connection_closed(message: &str) -> TransportError {
    transport_error(TransportErrorKind::ConnectionClosed, message)
}

fn sanitize(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            sanitized.push(ch);
        } else {
            sanitized.push('-');
        }
    }
    if sanitized.is_empty() {
        sanitized.push_str("endpoint");
    }
    sanitized
}

fn lock(inner: &Registry) -> MutexGuard<'_, HashMap<String, Arc<session::EndpointState>>> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------- 端点地址 ----------

/// 端点地址：`real://<id>?fp=<sha256-hex>#tcp=<host>:<port>`。
#[derive(Debug)]
struct ParsedAddress {
    fingerprint: [u8; 32],
    host: String,
    port: u16,
}

fn parse_endpoint_address(address: &str) -> Result<ParsedAddress, TransportError> {
    let rest = address
        .strip_prefix("real://")
        .ok_or_else(|| invalid_endpoint("remote address must start with real://"))?;
    let (id, rest) = rest
        .split_once('?')
        .ok_or_else(|| invalid_endpoint("remote address is missing query (?fp=...)"))?;
    let (query, fragment) = rest
        .split_once('#')
        .ok_or_else(|| invalid_endpoint("remote address is missing fragment (#tcp=...)"))?;
    if id.is_empty() {
        return Err(invalid_endpoint("remote address has empty endpoint id"));
    }
    let fingerprint_hex = query
        .strip_prefix("fp=")
        .ok_or_else(|| invalid_endpoint("remote address query must be fp=<sha256-hex>"))?;
    let fingerprint = crate::tls::parse_fingerprint(fingerprint_hex)
        .ok_or_else(|| invalid_endpoint("remote address fingerprint must be 64 hex chars"))?;
    let tcp = fragment
        .strip_prefix("tcp=")
        .ok_or_else(|| invalid_endpoint("remote address fragment must be tcp=host:port"))?;
    let (host, port) = tcp
        .rsplit_once(':')
        .ok_or_else(|| invalid_endpoint("tcp fragment must be host:port"))?;
    let port: u16 = port
        .parse()
        .map_err(|_| invalid_endpoint("tcp port must be a u16"))?;
    if host.is_empty() {
        return Err(invalid_endpoint("tcp host must not be empty"));
    }
    Ok(ParsedAddress {
        fingerprint,
        host: host.to_string(),
        port,
    })
}

// ---------- 注册表 ----------

type Registry = Mutex<HashMap<String, Arc<session::EndpointState>>>;
/// 单个逻辑客户端持有的跨重连游标与服务端签发 resume identity。
type ResumeState = Arc<Mutex<HashMap<(String, String), (u64, wire::ResumeIdentity)>>>;

// ---------- 真实 Transport ----------

/// 真实远程 Transport：同一实例可作 Server（`bind`）与 Client（`connect`），
/// 与 Mock 的占位语义一致，但走真实 TCP + TLS 1.3。
#[derive(Debug)]
pub struct RealRemoteTransport {
    registry: Arc<Registry>,
    /// 端点凭证供给基座：每个已发布端点在此派生独立凭证文件。
    token_store: TokenStore,
    client_token: Option<Token>,
    max_frame_bytes: u64,
    resend_window_frames: usize,
    max_buffered_bytes: u64,
    next_id: AtomicU64,
    next_client_connection: AtomicU64,
    /// 同进程连接侧解析端点凭证用：地址 → 端点独立凭证。
    endpoint_tokens: Arc<Mutex<HashMap<String, Token>>>,
    /// 客户端跨重连状态：`(地址, label)` → (last_acked, 服务端签发 identity)。
    resume: ResumeState,
}

impl RealRemoteTransport {
    pub fn new(config: RealRemoteTransportConfig) -> Self {
        Self {
            registry: Arc::new(Mutex::new(HashMap::new())),
            token_store: config.token_store,
            client_token: config.client_token,
            max_frame_bytes: config.max_frame_bytes,
            resend_window_frames: config.resend_window_frames,
            max_buffered_bytes: config.max_buffered_bytes,
            next_id: AtomicU64::new(0),
            next_client_connection: AtomicU64::new(0),
            endpoint_tokens: Arc::new(Mutex::new(HashMap::new())),
            resume: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 发布端点：绑定 loopback 临时端口、生成 TLS 身份与地址、登记注册表。
    pub(crate) async fn publish_endpoint(
        &self,
        name: &str,
    ) -> Result<RemotePublishHandle, TransportError> {
        let seq = self.next_id.fetch_add(1, Ordering::Relaxed);
        let id = format!("{}-{seq}", sanitize(name));
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|error| {
            transport_error(
                TransportErrorKind::BindFailed,
                format!("failed to bind loopback listener: {error}"),
            )
        })?;
        let bound_addr = listener.local_addr().map_err(|error| {
            transport_error(
                TransportErrorKind::Internal,
                format!("failed to read listener address: {error}"),
            )
        })?;
        let identity = crate::tls::generate_identity(&id)?;
        let address = format!(
            "real://{id}?fp={}#tcp={}:{}",
            identity.fingerprint_hex,
            bound_addr.ip(),
            bound_addr.port()
        );
        // 端点独立凭证：每个端点生成互不相同的 token（文件 + 内存态），
        // 凭证文件供客户端侧按端点读取；revoke 时删除文件，凭证真正失效。
        let credential_file =
            TokenStore::new(endpoint_credential_path(self.token_store.path(), &id));
        let credential = credential_file.generate().map_err(|error| {
            transport_error(
                TransportErrorKind::BindFailed,
                format!("failed to provision endpoint credential: {error}"),
            )
        })?;
        let state = Arc::new(session::EndpointState::new(
            id.clone(),
            address.clone(),
            identity,
            credential.clone(),
            credential_file,
            self.max_frame_bytes,
            self.max_buffered_bytes,
            self.resend_window_frames,
        ));
        *state.listener_slot.lock().expect("slot lock") = Some(listener);
        let mut registry = lock(&self.registry);
        if registry.contains_key(&address) {
            drop(state.listener_slot.lock().expect("slot lock").take());
            let _ = state.credential_file.delete();
            return Err(transport_error(
                TransportErrorKind::BindFailed,
                format!("remote endpoint {address:?} is already published"),
            ));
        }
        registry.insert(address.clone(), state);
        self.endpoint_tokens
            .lock()
            .expect("endpoint tokens lock")
            .insert(address.clone(), credential);
        Ok(RemotePublishHandle {
            id,
            endpoint: TransportEndpoint::Remote {
                address,
                adapter: ADAPTER_NAME.into(),
            },
        })
    }

    /// 取消发布端点：从注册表移除（新连接失败）、关闭 listener 与已建立连接，
    /// 并销毁该次发布的端点凭证。
    pub(crate) fn unpublish_endpoint(&self, handle_id: &str) -> Result<(), TransportError> {
        let mut registry = lock(&self.registry);
        let address = registry
            .iter()
            .find(|(_, state)| state.id == handle_id)
            .map(|(address, _)| address.clone())
            .ok_or_else(|| {
                transport_error(
                    TransportErrorKind::Internal,
                    format!("unknown remote publish handle {handle_id:?}"),
                )
            })?;
        let state = registry.remove(&address).expect("found above");
        // 同步关闭监听 socket：新 TCP 连接立即被拒绝；连接 reader 观察
        // published=false 后关闭既有连接。
        state.published.store(false, Ordering::Release);
        // 端点凭证一并销毁：unpublish 后该凭证不再可用（重新发布生成新凭证）。
        state.credential_file.delete().ok();
        self.endpoint_tokens
            .lock()
            .expect("endpoint tokens lock")
            .remove(&address);
        drop(state.listener_slot.lock().expect("slot lock").take());
        state.listener_closed.notify_one();
        Ok(())
    }

    /// 撤销端点：**真正失效** —— 置位 revoke 标志、从注册表移除（新连接
    /// 失败）、关闭监听、删除端点凭证文件；已建立连接在帧循环内即时断开。
    pub(crate) fn revoke_endpoint(&self, handle_id: &str) -> Result<(), TransportError> {
        let state = {
            let registry = lock(&self.registry);
            registry
                .iter()
                .find(|(_, state)| state.id == handle_id)
                .map(|(_, state)| Arc::clone(state))
                .ok_or_else(|| {
                    transport_error(
                        TransportErrorKind::Internal,
                        format!("unknown remote publish handle {handle_id:?}"),
                    )
                })?
        };
        state.revoked.store(true, Ordering::Release);
        {
            let mut registry = lock(&self.registry);
            registry.remove(&state.address);
        }
        state.published.store(false, Ordering::Release);
        self.endpoint_tokens
            .lock()
            .expect("endpoint tokens lock")
            .remove(&state.address);
        // 销毁端点凭证：即使凭证已泄露，revoke 后也无法再通过认证。
        state.credential_file.delete().ok();
        drop(state.listener_slot.lock().expect("slot lock").take());
        state.listener_closed.notify_one();
        tracing::warn!(
            endpoint = %handle_id,
            address = %state.address,
            "remote endpoint revoked; established connections will close"
        );
        Ok(())
    }

    /// 端点独立凭证（连接侧解析用；revoke / unpublish 后返回 `None`）。
    pub fn endpoint_token(&self, address: &str) -> Option<Token> {
        self.endpoint_tokens
            .lock()
            .expect("endpoint tokens lock")
            .get(address)
            .cloned()
    }

    /// 诊断：会话当前已确认水位（label 客户端已交付的最高服务端序号）。
    pub fn acked_sequence(&self, address: &str, label: &str) -> Option<u64> {
        lock(&self.registry)
            .get(address)
            .and_then(|state| state.session_window(label))
            .map(|window| window.lock().expect("window lock").acked())
    }

    /// 诊断：会话当前缓冲的未确认帧数。
    pub fn buffered_frames(&self, address: &str, label: &str) -> Option<usize> {
        lock(&self.registry)
            .get(address)
            .and_then(|state| state.session_window(label))
            .map(|window| window.lock().expect("window lock").buffered())
    }

    /// 诊断：会话当前缓冲的未确认帧总字节数。
    pub fn buffered_bytes(&self, address: &str, label: &str) -> Option<u64> {
        lock(&self.registry)
            .get(address)
            .and_then(|state| state.session_window(label))
            .map(|window| window.lock().expect("window lock").buffered_bytes())
    }

    async fn connect_typed_with_token(
        &self,
        endpoint: &TransportEndpoint,
        options: &ConnectOptions,
        token: &Token,
    ) -> Result<ClientConnection, TransportError> {
        self.connect_typed_with_token_and_resume(endpoint, options, token, Arc::clone(&self.resume))
            .await
    }

    async fn connect_typed_with_token_and_resume(
        &self,
        endpoint: &TransportEndpoint,
        options: &ConnectOptions,
        token: &Token,
        resume: ResumeState,
    ) -> Result<ClientConnection, TransportError> {
        let TransportEndpoint::Remote { address, adapter } = endpoint else {
            return Err(invalid_endpoint(
                "RealRemoteTransport requires TransportEndpoint::Remote",
            ));
        };
        if adapter != ADAPTER_NAME {
            return Err(invalid_endpoint(format!(
                "RealRemoteTransport only handles adapter {ADAPTER_NAME:?}, got {adapter:?}"
            )));
        }
        let parsed = parse_endpoint_address(address)?;
        let tcp = match tokio::time::timeout(
            Duration::from_millis(options.timeout_ms),
            TcpStream::connect((parsed.host.as_str(), parsed.port)),
        )
        .await
        {
            Ok(Ok(tcp)) => tcp,
            Ok(Err(error)) => {
                return Err(transport_error(
                    TransportErrorKind::ConnectionFailed,
                    format!("TCP connect to {address:?} failed: {error}"),
                ));
            }
            Err(_) => {
                return Err(transport_error(
                    TransportErrorKind::Timeout,
                    format!("TCP connect to {address:?} timed out"),
                ));
            }
        };
        let connection = session::client_handshake(
            tcp,
            parsed.fingerprint,
            token,
            options,
            resume,
            address.clone(),
            &self.next_client_connection,
            self.max_buffered_bytes,
        )
        .await?;
        Ok(connection)
    }

    async fn connect_with_token(
        &self,
        endpoint: &TransportEndpoint,
        options: &ConnectOptions,
        token: &Token,
    ) -> Result<Box<dyn GuiConnection>, TransportError> {
        Ok(Box::new(
            self.connect_typed_with_token(endpoint, options, token)
                .await?,
        ))
    }
}

/// [`RealRemoteTransport`] 的配置。
#[derive(Debug)]
pub struct RealRemoteTransportConfig {
    /// 端点凭证供给基座（CLI 侧 token 文件路径）：每个已发布端点在此派生
    /// 独立凭证文件，互不相同。
    pub token_store: TokenStore,
    /// 客户端认证凭证（`GuiTransportClient::connect` 使用；缺省则拒绝连接）。
    pub client_token: Option<Token>,
    /// 单帧上限（字节）。
    pub max_frame_bytes: u64,
    /// 服务端有界重放窗口（帧数）。
    pub resend_window_frames: usize,
    /// 未确认 / 未交付缓冲上限（字节；服务端重放窗口与两侧接收队列共用）。
    pub max_buffered_bytes: u64,
}

impl RealRemoteTransportConfig {
    pub fn new(token_store: TokenStore, client_token: Option<Token>) -> Self {
        Self {
            token_store,
            client_token,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            resend_window_frames: DEFAULT_RESEND_WINDOW_FRAMES,
            max_buffered_bytes: DEFAULT_MAX_BUFFERED_BYTES,
        }
    }
}

/// 端点凭证文件路径：`<base>.d/<endpoint-id>/token`（`<base>` 为配置的
/// token 文件路径）。
fn endpoint_credential_path(base: &std::path::Path, endpoint_id: &str) -> std::path::PathBuf {
    let dir_name = format!(
        "{}.d",
        base.file_name().unwrap_or_default().to_string_lossy()
    );
    base.parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(dir_name)
        .join(endpoint_id)
        .join("token")
}

#[async_trait]
impl GuiTransportServer for RealRemoteTransport {
    async fn bind(
        &self,
        endpoint: TransportEndpoint,
    ) -> Result<Box<dyn GuiListener>, TransportError> {
        let TransportEndpoint::Remote { address, adapter } = endpoint else {
            return Err(invalid_endpoint(
                "RealRemoteTransport requires TransportEndpoint::Remote",
            ));
        };
        if adapter != ADAPTER_NAME {
            return Err(invalid_endpoint(format!(
                "RealRemoteTransport only handles adapter {ADAPTER_NAME:?}, got {adapter:?}"
            )));
        }
        let state = lock(&self.registry).get(&address).cloned().ok_or_else(|| {
            transport_error(
                TransportErrorKind::BindFailed,
                format!("no published endpoint at {address:?}"),
            )
        })?;
        if state.bound.swap(true, Ordering::AcqRel) {
            return Err(transport_error(
                TransportErrorKind::BindFailed,
                format!("remote endpoint {address:?} is already bound"),
            ));
        }
        // 监听 socket 留在共享槽位：`accept` 时借用、`unpublish` 时关闭。
        Ok(Box::new(RealRemoteListener {
            state,
            closed: AtomicBool::new(false),
        }))
    }
}

#[async_trait]
impl GuiTransportClient for RealRemoteTransport {
    async fn connect(
        &self,
        endpoint: TransportEndpoint,
        options: ConnectOptions,
    ) -> Result<Box<dyn GuiConnection>, TransportError> {
        let token = self.client_token.clone().ok_or_else(|| {
            transport_error(
                TransportErrorKind::AuthenticationFailed,
                "no client token configured; use RealRemoteConnector with a token",
            )
        })?;
        self.connect_with_token(&endpoint, &options, &token).await
    }
}

/// 已绑定端点的监听器：每次 `accept` 完成 TLS + 认证 + 续传后返回连接。
pub struct RealRemoteListener {
    state: Arc<session::EndpointState>,
    closed: AtomicBool,
}

impl Drop for RealRemoteListener {
    fn drop(&mut self) {
        // 释放单占用；监听 socket 留在共享槽位，允许该端点再次 bind。
        self.state.bound.store(false, Ordering::Release);
    }
}

#[async_trait]
impl GuiListener for RealRemoteListener {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("listener is closed"));
        }
        let listener = self
            .state
            .listener_slot
            .lock()
            .expect("slot lock")
            .take()
            .ok_or_else(|| connection_closed("listener socket is not available"))?;
        let accepted = tokio::select! {
            result = listener.accept() => Some(result),
            _ = self.state.listener_closed.notified() => None,
        };
        {
            let mut slot = self.state.listener_slot.lock().expect("slot lock");
            if self.state.published.load(Ordering::Acquire) && slot.is_none() {
                *slot = Some(listener);
            }
        }
        let Some(accepted) = accepted else {
            return Err(connection_closed("listener is closed"));
        };
        let (tcp, _) = accepted.map_err(|error| {
            transport_error(
                TransportErrorKind::ConnectionFailed,
                format!("accept failed: {error}"),
            )
        })?;
        let connection = session::server_handshake(tcp, Arc::clone(&self.state)).await?;
        Ok(Box::new(connection))
    }

    async fn close(&self) -> Result<(), TransportError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        // 打断 pending accept 并释放底层 socket。close 后该 listener 不可复用；
        // publish 生命周期仍由 provider registry 持有并可 unpublish/revoke。
        drop(self.state.listener_slot.lock().expect("slot lock").take());
        self.state.published.store(false, Ordering::Release);
        self.state.listener_closed.notify_one();
        self.state.bound.store(false, Ordering::Release);
        Ok(())
    }
}

// ---------- Provider / Connector ----------

/// CLI 侧的远程端点生命周期 Adapter（真实实现）。
#[derive(Debug)]
pub struct RealRemoteTransportProvider {
    transport: Arc<RealRemoteTransport>,
}

impl RealRemoteTransportProvider {
    pub fn new(transport: Arc<RealRemoteTransport>) -> Self {
        Self { transport }
    }

    pub fn transport(&self) -> &Arc<RealRemoteTransport> {
        &self.transport
    }

    /// 撤销端点凭证：新连接被拒、已建立连接断开，并写审计日志（不含 Secret）。
    pub async fn revoke(&self, handle_id: &str) -> Result<(), TransportError> {
        self.transport.revoke_endpoint(handle_id)
    }
}

#[async_trait]
impl RemoteGuiTransportProvider for RealRemoteTransportProvider {
    fn describe(&self) -> RemoteTransportDescription {
        RemoteTransportDescription {
            adapter: ADAPTER_NAME.into(),
            display_name: "Real Remote Transport (TCP + TLS 1.3)".into(),
        }
    }

    async fn publish(
        &self,
        request: RemotePublishRequest,
    ) -> Result<RemotePublishHandle, TransportError> {
        self.transport.publish_endpoint(&request.name).await
    }

    async fn unpublish(&self, handle_id: &str) -> Result<(), TransportError> {
        self.transport.unpublish_endpoint(handle_id)
    }

    async fn revoke(&self, handle_id: &str) -> Result<(), TransportError> {
        self.transport.revoke_endpoint(handle_id)
    }
}

/// GUI 侧的远程连接 Adapter（真实实现）。
///
/// 凭证解析规则：`token` 为 `Some` 时使用显式凭证（服务端按端点独立凭证
/// 校验，凭证不匹配即拒绝）；为 `None` 时按端点从 transport 解析其独立凭证
/// （同一进程内发布 + 连接使用；revoke / unpublish 后解析失败）。
#[derive(Debug)]
pub struct RealRemoteConnector {
    transport: Arc<RealRemoteTransport>,
    token: Option<Token>,
    /// 该逻辑客户端自己的恢复状态；不同 connector 即使共享 endpoint token
    /// 与 label，也不会互相继承服务端签发的 resume identity。
    resume: ResumeState,
}

impl RealRemoteConnector {
    pub fn new(transport: Arc<RealRemoteTransport>, token: Option<Token>) -> Self {
        Self {
            transport,
            token,
            resume: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 解析连接凭证：显式凭证优先；否则按端点从 transport 解析独立凭证。
    fn resolve_token(&self, address: &str) -> Result<Token, TransportError> {
        match &self.token {
            Some(token) => Ok(token.clone()),
            None => self.transport.endpoint_token(address).ok_or_else(|| {
                transport_error(
                    TransportErrorKind::ConnectionFailed,
                    "endpoint credential is not available here (revoked, unpublished, or published by another process)",
                )
            }),
        }
    }

    /// 连接并返回具体客户端连接类型，便于查询续传 / 快照信号
    /// （[`ClientConnection::resume_outcome`]）。
    pub async fn connect_typed(
        &self,
        endpoint: &TransportEndpoint,
        options: ConnectOptions,
    ) -> Result<ClientConnection, TransportError> {
        let TransportEndpoint::Remote { address, .. } = endpoint else {
            return Err(invalid_endpoint(
                "RealRemoteConnector requires TransportEndpoint::Remote",
            ));
        };
        let token = self.resolve_token(address)?;
        self.transport
            .connect_typed_with_token_and_resume(
                endpoint,
                &options,
                &token,
                Arc::clone(&self.resume),
            )
            .await
    }
}

#[async_trait]
impl RemoteGuiConnector for RealRemoteConnector {
    async fn connect(
        &self,
        endpoint: &TransportEndpoint,
        options: ConnectOptions,
    ) -> Result<Box<dyn GuiConnection>, TransportError> {
        let TransportEndpoint::Remote { address, .. } = endpoint else {
            return Err(invalid_endpoint(
                "RealRemoteConnector requires TransportEndpoint::Remote",
            ));
        };
        let token = self.resolve_token(address)?;
        Ok(Box::new(
            self.transport
                .connect_typed_with_token_and_resume(
                    endpoint,
                    &options,
                    &token,
                    Arc::clone(&self.resume),
                )
                .await?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_address_round_trip_and_rejections() {
        let identity = crate::tls::generate_identity("addr").expect("identity");
        let address = format!(
            "real://my-endpoint-0?fp={}#tcp=127.0.0.1:43210",
            identity.fingerprint_hex
        );
        let parsed = parse_endpoint_address(&address).expect("parse");
        assert!(address.starts_with("real://my-endpoint-0?fp="));
        assert_eq!(parsed.host, "127.0.0.1");
        assert_eq!(parsed.port, 43210);
        assert_eq!(
            parsed.fingerprint,
            crate::tls::parse_fingerprint(&identity.fingerprint_hex).expect("fp")
        );

        for bad in [
            "mock://x?fp=abc#tcp=127.0.0.1:1",
            "real://x#tcp=127.0.0.1:1",
            "real://x?fp=abc",
            "real://?fp=abc#tcp=127.0.0.1:1",
            "real://x?fp=short#tcp=127.0.0.1:1",
            "real://x?fp=abc#tcp=127.0.0.1:notaport",
            "real://x?fp=abc#tcp=:1",
            "real://x?fp=abc#tcp=127.0.0.1",
        ] {
            assert!(
                parse_endpoint_address(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn publish_unpublish_lifecycle_and_diagnostics() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let transport = RealRemoteTransport::new(RealRemoteTransportConfig::new(
            TokenStore::new(temp.path().join("server.token")),
            None,
        ));
        let handle = poll_once(transport.publish_endpoint("lifecycle")).expect("publish");
        assert_eq!(handle.id, "lifecycle-0");
        let address = match &handle.endpoint {
            TransportEndpoint::Remote { address, adapter } => {
                assert_eq!(adapter, ADAPTER_NAME);
                address.clone()
            }
            _ => panic!("expected remote endpoint"),
        };
        assert!(address.starts_with("real://lifecycle-0?fp="));
        assert!(address.contains("#tcp=127.0.0.1:"));
        // 尚无任何连接：会话未建立（会话按连接创建），水位为空。
        assert_eq!(transport.acked_sequence(&address, "lifecycle-gui"), None);
        assert_eq!(transport.buffered_frames(&address, "lifecycle-gui"), None);
        assert_eq!(transport.buffered_bytes(&address, "lifecycle-gui"), None);

        transport.unpublish_endpoint(&handle.id).expect("unpublish");
        assert_eq!(transport.acked_sequence(&address, "lifecycle-gui"), None);
        assert_eq!(transport.endpoint_token(&address), None);
        let error = transport.unpublish_endpoint(&handle.id).expect_err("twice");
        assert_eq!(error.kind, TransportErrorKind::Internal);
    }

    #[test]
    fn invalid_endpoint_kind_and_adapter_are_rejected() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let transport = RealRemoteTransport::new(RealRemoteTransportConfig::new(
            TokenStore::new(temp.path().join("server.token")),
            None,
        ));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            for endpoint in [
                TransportEndpoint::Local {
                    address: "nope".into(),
                },
                TransportEndpoint::Memory {
                    channel: "nope".into(),
                },
                TransportEndpoint::Remote {
                    address: "mock://nope".into(),
                    adapter: "mock".into(),
                },
            ] {
                let error = match transport.bind(endpoint.clone()).await {
                    Err(error) => error,
                    Ok(_) => panic!("bind must reject {endpoint:?}"),
                };
                assert_eq!(error.kind, TransportErrorKind::InvalidEndpoint);
            }
        });
    }

    #[test]
    fn bind_requires_published_endpoint_and_is_single_use() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let transport = RealRemoteTransport::new(RealRemoteTransportConfig::new(
            TokenStore::new(temp.path().join("server.token")),
            None,
        ));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let handle = transport.publish_endpoint("bind").await.expect("publish");
            let _first = transport
                .bind(handle.endpoint.clone())
                .await
                .expect("first bind must succeed");
            let error = match transport.bind(handle.endpoint.clone()).await {
                Err(error) => error,
                Ok(_) => panic!("second bind while first listener is held must fail"),
            };
            assert_eq!(error.kind, TransportErrorKind::BindFailed);
            drop(_first);
            assert!(
                transport.bind(handle.endpoint.clone()).await.is_ok(),
                "rebind after dropping the listener must succeed"
            );
            let missing = TransportEndpoint::Remote {
                address: "real://gone?fp=0000000000000000000000000000000000000000000000000000000000000000#tcp=127.0.0.1:1".into(),
                adapter: ADAPTER_NAME.into(),
            };
            let error = match transport.bind(missing).await {
                Err(error) => error,
                Ok(_) => panic!("bind of missing endpoint must fail"),
            };
            assert_eq!(error.kind, TransportErrorKind::BindFailed);
        });
    }

    #[test]
    fn listener_close_interrupts_pending_accept_and_releases_socket() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let transport = Arc::new(RealRemoteTransport::new(RealRemoteTransportConfig::new(
            TokenStore::new(temp.path().join("server.token")),
            None,
        )));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let handle = transport.publish_endpoint("close").await.expect("publish");
            let parsed = match &handle.endpoint {
                TransportEndpoint::Remote { address, .. } => {
                    parse_endpoint_address(address).expect("parse")
                }
                _ => unreachable!(),
            };
            let listener: Arc<dyn GuiListener> =
                Arc::from(transport.bind(handle.endpoint.clone()).await.expect("bind"));
            let pending = tokio::spawn({
                let listener = Arc::clone(&listener);
                async move { listener.accept().await }
            });
            tokio::task::yield_now().await;
            listener.close().await.expect("close listener");

            let error = match tokio::time::timeout(Duration::from_secs(1), pending)
                .await
                .expect("pending accept must be interrupted")
                .expect("accept task")
            {
                Err(error) => error,
                Ok(_) => panic!("accept must not succeed after close"),
            };
            assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
            assert!(
                TcpStream::connect((parsed.host.as_str(), parsed.port))
                    .await
                    .is_err(),
                "listener socket must be released"
            );
        });
    }

    #[test]
    fn endpoints_get_independent_credentials_and_revoke_destroys_them() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let base = TokenStore::new(temp.path().join("server.token"));
        let transport =
            RealRemoteTransport::new(RealRemoteTransportConfig::new(base.clone(), None));
        let handle_a = poll_once(transport.publish_endpoint("cred-a")).expect("publish a");
        let handle_b = poll_once(transport.publish_endpoint("cred-b")).expect("publish b");
        let TransportEndpoint::Remote {
            address: addr_a, ..
        } = &handle_a.endpoint
        else {
            panic!("expected remote endpoint");
        };
        let TransportEndpoint::Remote {
            address: addr_b, ..
        } = &handle_b.endpoint
        else {
            panic!("expected remote endpoint");
        };

        let token_a = transport.endpoint_token(addr_a).expect("token a");
        let token_b = transport.endpoint_token(addr_b).expect("token b");
        assert_ne!(
            token_a.as_str(),
            token_b.as_str(),
            "endpoints must not share a credential"
        );

        // revoke 后：凭证解析消失、凭证文件删除、注册表移除。
        let credential_file = endpoint_credential_path(base.path(), &handle_a.id);
        assert!(credential_file.exists(), "credential file must exist");
        transport.revoke_endpoint(&handle_a.id).expect("revoke a");
        assert!(
            !credential_file.exists(),
            "revoke must destroy the endpoint credential file"
        );
        assert_eq!(transport.endpoint_token(addr_a), None);
        assert_eq!(transport.acked_sequence(addr_a, "any"), None);
        // 端点 b 不受影响。
        assert!(transport.endpoint_token(addr_b).is_some());
    }

    /// 同步轮询一次 async 结果（测试辅助）。
    fn poll_once<F>(future: F) -> F::Output
    where
        F: std::future::Future,
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(future)
    }
}
