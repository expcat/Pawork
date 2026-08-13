//! Codex App Server 宿主：stdio JSONL 会话环与 initialize/initialized 握手状态机。
//!
//! 本层**不**依赖 `app-service`。Core 经 [`CoreDispatcher`] 注入（与 ACP 的
//! CwdResolver/SessionResolver 同类 seam）；生产接线由 orchestrator 后续完成。
//! 传输默认 stdio JSONL；本地 socket 仅作实验性说明，本 crate 不实现 websocket 鉴权。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent_domain::{ConnectionId, SessionId};
use async_trait::async_trait;
use client_adapter_api::{
    AdapterError, CanonicalClientRequest, CanonicalCoreFrame, CapabilitySnapshot, ClientAdapter,
    ClientCapability, ClientFrame, ClientProtocol, ClientSessionId, ClientSessionRecord,
    ClientSessionState, SessionRegistry, CLIENT_ADAPTER_SCHEMA_VERSION,
};
use core_api::{AppCommand, AppResponse};
use serde_json::{json, Value};

use crate::adapter::{
    CodexAppServerAdapter, CodexAppServerAdapterFactory, NegotiatedCodexAdapter,
    CAP_EXPERIMENTAL_API,
};
use crate::map::{self, ThreadLineage};
use crate::now_timestamp;
use crate::wire::{
    is_server_request, InitializeParams, InitializeResult, JsonRpcError, JsonRpcErrorResponse,
    JsonRpcId, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, ParamsExt,
    ERROR_INVALID_PARAMS, ERROR_INVALID_REQUEST,
};
use crate::{HOST_AGENT_NAME, HOST_AGENT_VERSION, PROTOCOL_NAME, PROTOCOL_VERSION};

/// Core 命令/查询分发（宿主注入；不在本 crate 构造 AppService）。
#[async_trait]
pub trait CoreDispatcher: Send + Sync {
    async fn dispatch(
        &self,
        request: CanonicalClientRequest,
    ) -> Result<CanonicalCoreFrame, AdapterError>;
}

/// 握手状态：未初始化 → 已回 initialize、等待 `initialized` → 就绪。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeState {
    Uninitialized,
    WaitingForInitialized,
    Ready,
}

struct PendingApproval {
    turn_id: String,
    item_id: String,
}

/// 可选 in-crate JSONL 会话环。
pub struct CodexAppServerHost {
    factory: CodexAppServerAdapterFactory,
    registry: Arc<SessionRegistry>,
    dispatcher: Arc<dyn CoreDispatcher>,
    negotiated: Mutex<Option<NegotiatedCodexAdapter>>,
    handshake: Mutex<HandshakeState>,
    connection_id: ConnectionId,
    lineage: Mutex<BTreeMap<String, ThreadLineage>>,
    session_contexts: Mutex<BTreeMap<ClientSessionId, (u64, u64)>>,
    pending_approvals: Mutex<BTreeMap<String, PendingApproval>>,
    next_server_request_id: AtomicU64,
    ingress_saturated: AtomicBool,
    runtime: RuntimeIdentity,
}

/// initialize 响应中的运行时身份（不含 Secret）。
#[derive(Clone, Debug)]
pub struct RuntimeIdentity {
    pub user_agent: String,
    pub codex_home: String,
    pub platform_family: String,
    pub platform_os: String,
}

impl Default for RuntimeIdentity {
    fn default() -> Self {
        Self {
            user_agent: format!("{HOST_AGENT_NAME}/{HOST_AGENT_VERSION}"),
            codex_home: "pawork://codex-home".into(),
            platform_family: "pawork".into(),
            platform_os: std::env::consts::OS.into(),
        }
    }
}

static CONNECTION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl CodexAppServerHost {
    pub fn new(
        factory: CodexAppServerAdapterFactory,
        registry: Arc<SessionRegistry>,
        dispatcher: Arc<dyn CoreDispatcher>,
    ) -> Self {
        Self::with_runtime(factory, registry, dispatcher, RuntimeIdentity::default())
    }

    pub fn with_runtime(
        factory: CodexAppServerAdapterFactory,
        registry: Arc<SessionRegistry>,
        dispatcher: Arc<dyn CoreDispatcher>,
        runtime: RuntimeIdentity,
    ) -> Self {
        Self {
            factory,
            registry,
            dispatcher,
            negotiated: Mutex::new(None),
            handshake: Mutex::new(HandshakeState::Uninitialized),
            connection_id: ConnectionId::from(format!(
                "codex-connection-{}-{}",
                std::process::id(),
                CONNECTION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            )),
            lineage: Mutex::new(BTreeMap::new()),
            session_contexts: Mutex::new(BTreeMap::new()),
            pending_approvals: Mutex::new(BTreeMap::new()),
            next_server_request_id: AtomicU64::new(1),
            ingress_saturated: AtomicBool::new(false),
            runtime,
        }
    }

    pub fn handshake_state(&self) -> HandshakeState {
        *self.handshake.lock().expect("codex handshake mutex")
    }

    pub fn is_initialized(&self) -> bool {
        self.handshake_state() == HandshakeState::Ready
    }

    pub fn degraded_capabilities(&self) -> Vec<ClientCapability> {
        self.negotiated
            .lock()
            .expect("codex negotiated mutex")
            .as_ref()
            .map(|negotiated| negotiated.degraded.clone())
            .unwrap_or_default()
    }

    pub fn set_ingress_saturated(&self, saturated: bool) {
        self.ingress_saturated.store(saturated, Ordering::SeqCst);
    }

    pub fn record_lineage(&self, thread_id: impl Into<String>, lineage: ThreadLineage) {
        self.lineage
            .lock()
            .expect("codex lineage mutex")
            .insert(thread_id.into(), lineage);
    }

    pub fn lineage_of(&self, thread_id: &str) -> ThreadLineage {
        self.lineage
            .lock()
            .expect("codex lineage mutex")
            .get(thread_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn adapter(&self) -> Result<Arc<CodexAppServerAdapter>, JsonRpcError> {
        self.negotiated
            .lock()
            .expect("codex negotiated mutex")
            .as_ref()
            .map(|negotiated| Arc::clone(&negotiated.adapter))
            .ok_or_else(JsonRpcError::not_initialized)
    }

    /// 处理一行 JSONL，返回零或多行出站 JSONL（响应 / 通知 / 服务器请求）。
    pub async fn handle_line(&self, line: &str) -> Vec<String> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let value = match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => value,
            Err(error) => {
                return vec![JsonRpcMessage::Error(JsonRpcErrorResponse {
                    id: Value::Null,
                    error: JsonRpcError::new(crate::wire::ERROR_PARSE, error.to_string()),
                })
                .to_jsonl()];
            }
        };
        let message = match JsonRpcMessage::parse(value) {
            Ok(message) => message,
            Err(error) => {
                return vec![JsonRpcMessage::Error(JsonRpcErrorResponse {
                    id: Value::Null,
                    error,
                })
                .to_jsonl()];
            }
        };
        self.handle_message(message)
            .await
            .into_iter()
            .map(|message| message.to_jsonl())
            .collect()
    }

    pub async fn handle_message(&self, message: JsonRpcMessage) -> Vec<JsonRpcMessage> {
        if self.ingress_saturated.load(Ordering::SeqCst) {
            let id = match &message {
                JsonRpcMessage::Request(request) => request.id.clone(),
                JsonRpcMessage::Response(response) => response.id.clone(),
                JsonRpcMessage::Error(error) => error.id.clone(),
                JsonRpcMessage::Notification(_) => Value::Null,
            };
            return vec![JsonRpcMessage::Error(JsonRpcErrorResponse {
                id,
                error: JsonRpcError::overloaded(),
            })];
        }
        match message {
            JsonRpcMessage::Request(request) => match self
                .handle_request(request.id.clone(), &request.method, request.params)
                .await
            {
                Ok(result) => vec![JsonRpcMessage::Response(JsonRpcResponse {
                    id: request.id,
                    result,
                })],
                Err(error) => vec![JsonRpcMessage::Error(JsonRpcErrorResponse {
                    id: request.id,
                    error,
                })],
            },
            JsonRpcMessage::Notification(notification) => {
                if let Err(error) = self
                    .handle_notification(&notification.method, notification.params)
                    .await
                {
                    vec![JsonRpcMessage::Error(JsonRpcErrorResponse {
                        id: Value::Null,
                        error,
                    })]
                } else {
                    Vec::new()
                }
            }
            JsonRpcMessage::Response(response) => {
                if let Err(error) = self
                    .handle_client_response(response.id, Ok(response.result))
                    .await
                {
                    vec![JsonRpcMessage::Error(JsonRpcErrorResponse {
                        id: Value::Null,
                        error,
                    })]
                } else {
                    Vec::new()
                }
            }
            JsonRpcMessage::Error(error_response) => {
                let _ = self
                    .handle_client_response(error_response.id, Err(error_response.error))
                    .await;
                Vec::new()
            }
        }
    }

    pub async fn handle_request(
        &self,
        id: JsonRpcId,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, JsonRpcError> {
        if self.ingress_saturated.load(Ordering::SeqCst) {
            return Err(JsonRpcError::overloaded());
        }
        if method == "initialize" {
            return self.initialize(params).await;
        }
        self.require_ready()?;
        let adapter = self.adapter()?;
        let params = params.unwrap_or(Value::Null);
        let frame = ClientFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: match &id {
                Value::Null => "codex-null".into(),
                other => other.to_string(),
            },
            method: method.into(),
            payload: params.clone(),
            extensions: Default::default(),
        };
        let request = adapter
            .decode(frame)
            .await
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        match request {
            CanonicalClientRequest::Command(envelope) => match &envelope.command {
                AppCommand::SessionCreate { .. } => {
                    self.thread_start(method, CanonicalClientRequest::Command(envelope))
                        .await
                }
                AppCommand::SessionFork { session_id, .. } => {
                    let source = session_id.as_str().to_string();
                    self.thread_fork(method, source, CanonicalClientRequest::Command(envelope))
                        .await
                }
                AppCommand::SessionOpen { .. } => {
                    self.thread_open(method, CanonicalClientRequest::Command(envelope))
                        .await
                }
                AppCommand::RunStart { .. }
                | AppCommand::RunCancel { .. }
                | AppCommand::SessionCompact { .. }
                | AppCommand::ToolApprove { .. } => {
                    self.dispatch_command(method, CanonicalClientRequest::Command(envelope))
                        .await
                }
                other => Err(JsonRpcError::new(
                    crate::wire::ERROR_METHOD_NOT_FOUND,
                    format!("method `{method}` decodes to unsupported canonical command {other:?}"),
                )),
            },
            CanonicalClientRequest::Reattach { .. } => self.reattach(request).await,
            CanonicalClientRequest::Disconnect { .. } => self.disconnect(request).await,
            CanonicalClientRequest::Query(_) | CanonicalClientRequest::Attach(_) => {
                Err(JsonRpcError::new(
                    crate::wire::ERROR_METHOD_NOT_FOUND,
                    format!("method `{method}` has no host handler for this canonical request"),
                ))
            }
        }
    }

    pub async fn handle_notification(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), JsonRpcError> {
        if self.ingress_saturated.load(Ordering::SeqCst) {
            return Err(JsonRpcError::overloaded());
        }
        if method == "initialized" {
            return self.initialized(params).await;
        }
        self.require_ready()?;
        Err(JsonRpcError::new(
            crate::wire::ERROR_METHOD_NOT_FOUND,
            format!("unknown Codex notification `{method}`"),
        ))
    }

    /// 把 canonical 事件编码为出站线协议消息（通知或 server→client 请求）。
    pub async fn encode_event(
        &self,
        envelope: core_api::AppEventEnvelope,
    ) -> Result<JsonRpcMessage, JsonRpcError> {
        self.require_ready()?;
        let adapter = self.adapter()?;
        if let core_api::AppEvent::ToolApprovalRequired {
            run_id,
            tool_call_id,
            ..
        } = &envelope.payload
        {
            let thread_id = adapter
                .resolve_thread(&envelope)
                .await
                .ok_or_else(|| {
                    JsonRpcError::new(crate::wire::ERROR_INTERNAL, "approval event has no thread")
                })?
                .0;
            let params = adapter
                .approval_request(&envelope.payload, &thread_id)
                .await
                .map_err(|error| map::jsonrpc_error_for(&error))?;
            let request_id = self.next_server_request_id.fetch_add(1, Ordering::Relaxed);
            self.pending_approvals
                .lock()
                .expect("codex approvals mutex")
                .insert(
                    request_id.to_string(),
                    PendingApproval {
                        turn_id: run_id.as_str().to_string(),
                        item_id: tool_call_id.as_str().to_string(),
                    },
                );
            let value = serde_json::to_value(&params).map_err(|error| {
                JsonRpcError::new(crate::wire::ERROR_INTERNAL, error.to_string())
            })?;
            return Ok(JsonRpcMessage::Request(JsonRpcRequest {
                id: json!(request_id),
                method: "item/commandExecution/requestApproval".into(),
                params: Some(value),
            }));
        }
        let frame = adapter
            .encode(CanonicalCoreFrame::Event(envelope))
            .await
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        let message = if is_server_request(&frame.method) {
            JsonRpcMessage::Request(JsonRpcRequest {
                id: json!(frame.request_id),
                method: frame.method,
                params: Some(frame.payload),
            })
        } else {
            JsonRpcMessage::Notification(JsonRpcNotification {
                method: frame.method,
                params: Some(frame.payload),
            })
        };
        Ok(message)
    }

    async fn initialize(&self, params: Option<Value>) -> Result<Value, JsonRpcError> {
        if self.handshake_state() != HandshakeState::Uninitialized {
            return Err(JsonRpcError::already_initialized());
        }
        let params = serde_json::from_value::<InitializeParams>(params.unwrap_or(Value::Null))
            .map_err(|error| JsonRpcError::new(ERROR_INVALID_PARAMS, error.to_string()))?;
        params
            .reject_unknown("initialize")
            .map_err(|message| JsonRpcError::new(ERROR_INVALID_PARAMS, message))?;
        params
            .client_info
            .reject_unknown("initialize.clientInfo")
            .map_err(|message| JsonRpcError::new(ERROR_INVALID_PARAMS, message))?;
        let mut capabilities = BTreeSet::new();
        capabilities.insert(ClientCapability::new(crate::adapter::CAP_COMPACTION));
        if params
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.experimental_api)
        {
            capabilities.insert(ClientCapability::new(CAP_EXPERIMENTAL_API));
        }
        if let Some(declared) = &params.capabilities {
            for extra in declared.extra.keys() {
                capabilities.insert(ClientCapability::new(extra.clone()));
            }
        }
        let snapshot = CapabilitySnapshot {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new(PROTOCOL_NAME),
            protocol_version: PROTOCOL_VERSION.into(),
            client_version: params.client_info.version.clone(),
            revision: 1,
            capabilities,
        };
        let negotiated = self
            .factory
            .create_concrete(snapshot)
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        *self.negotiated.lock().expect("codex negotiated mutex") = Some(negotiated);
        *self.handshake.lock().expect("codex handshake mutex") =
            HandshakeState::WaitingForInitialized;
        serde_json::to_value(InitializeResult {
            user_agent: self.runtime.user_agent.clone(),
            codex_home: self.runtime.codex_home.clone(),
            platform_family: self.runtime.platform_family.clone(),
            platform_os: self.runtime.platform_os.clone(),
        })
        .map_err(|error| JsonRpcError::new(crate::wire::ERROR_INTERNAL, error.to_string()))
    }

    async fn initialized(&self, params: Option<Value>) -> Result<(), JsonRpcError> {
        match self.handshake_state() {
            HandshakeState::Uninitialized => Err(JsonRpcError::not_initialized()),
            HandshakeState::Ready => Err(JsonRpcError::already_initialized()),
            HandshakeState::WaitingForInitialized => {
                if let Some(Value::Object(fields)) = params {
                    if !fields.is_empty() {
                        return Err(JsonRpcError::new(
                            ERROR_INVALID_PARAMS,
                            format!(
                                "unsupported params fields for `initialized`: {}",
                                fields.keys().cloned().collect::<Vec<_>>().join(",")
                            ),
                        ));
                    }
                }
                *self.handshake.lock().expect("codex handshake mutex") = HandshakeState::Ready;
                Ok(())
            }
        }
    }

    fn require_ready(&self) -> Result<(), JsonRpcError> {
        if self.handshake_state() == HandshakeState::Ready {
            Ok(())
        } else {
            Err(JsonRpcError::not_initialized())
        }
    }

    async fn thread_start(
        &self,
        method: &str,
        request: CanonicalClientRequest,
    ) -> Result<Value, JsonRpcError> {
        let response = self
            .dispatcher
            .dispatch(request)
            .await
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        let session_id = session_id_from_response(&response)?;
        self.register_thread(&session_id, ThreadLineage::default())
            .await?;
        let lineage = self.lineage_of(&session_id);
        match response {
            CanonicalCoreFrame::Response(envelope) => {
                map::response_to_result(method, &envelope, Some(&session_id), &lineage)
                    .map_err(|error| map::jsonrpc_error_for(&error))
            }
            _ => Err(JsonRpcError::new(
                crate::wire::ERROR_INTERNAL,
                "thread/start did not produce a command response",
            )),
        }
    }

    async fn thread_fork(
        &self,
        method: &str,
        source_thread_id: String,
        request: CanonicalClientRequest,
    ) -> Result<Value, JsonRpcError> {
        let response = self
            .dispatcher
            .dispatch(request)
            .await
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        let session_id = session_id_from_response(&response)?;
        let lineage = ThreadLineage {
            parent_thread_id: None,
            forked_from_id: Some(source_thread_id),
        };
        self.register_thread(&session_id, lineage.clone()).await?;
        match response {
            CanonicalCoreFrame::Response(envelope) => {
                map::response_to_result(method, &envelope, Some(&session_id), &lineage)
                    .map_err(|error| map::jsonrpc_error_for(&error))
            }
            _ => Err(JsonRpcError::new(
                crate::wire::ERROR_INTERNAL,
                "thread/fork did not produce a command response",
            )),
        }
    }

    async fn thread_open(
        &self,
        method: &str,
        request: CanonicalClientRequest,
    ) -> Result<Value, JsonRpcError> {
        let response = self
            .dispatcher
            .dispatch(request)
            .await
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        let session_id = session_id_from_response(&response)?;
        if self
            .registry
            .get(&ClientSessionId::new(&session_id))
            .await
            .is_none()
        {
            self.register_thread(&session_id, ThreadLineage::default())
                .await?;
        }
        let lineage = self.lineage_of(&session_id);
        match response {
            CanonicalCoreFrame::Response(envelope) => {
                map::response_to_result(method, &envelope, Some(&session_id), &lineage)
                    .map_err(|error| map::jsonrpc_error_for(&error))
            }
            _ => Err(JsonRpcError::new(
                crate::wire::ERROR_INTERNAL,
                "thread/resume did not produce a command response",
            )),
        }
    }

    async fn dispatch_command(
        &self,
        method: &str,
        request: CanonicalClientRequest,
    ) -> Result<Value, JsonRpcError> {
        let response = self
            .dispatcher
            .dispatch(request)
            .await
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        match response {
            CanonicalCoreFrame::Response(envelope) => {
                map::response_to_result(method, &envelope, None, &ThreadLineage::default())
                    .map_err(|error| map::jsonrpc_error_for(&error))
            }
            CanonicalCoreFrame::Error(frame) => Err(JsonRpcError::new(
                map::jsonrpc_code_for_frame(&frame),
                frame.message,
            )),
            _ => Err(JsonRpcError::new(
                crate::wire::ERROR_INTERNAL,
                "command dispatch did not produce a response",
            )),
        }
    }

    async fn reattach(&self, request: CanonicalClientRequest) -> Result<Value, JsonRpcError> {
        let CanonicalClientRequest::Reattach {
            client_session_id,
            ownership_epoch,
            revision,
            connection_id: _,
            state,
            updated_at,
        } = request
        else {
            return Err(JsonRpcError::new(
                ERROR_INVALID_REQUEST,
                "expected reattach request",
            ));
        };
        let record = self
            .registry
            .claim(
                &client_session_id,
                ownership_epoch,
                revision,
                self.connection_id.clone(),
                state,
                updated_at,
            )
            .await
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        self.session_contexts
            .lock()
            .expect("codex session contexts mutex")
            .insert(
                record.client_session_id.clone(),
                (record.ownership_epoch, record.revision),
            );
        let lineage = self.lineage_of(record.client_session_id.0.as_str());
        Ok(map::thread_result(
            record.client_session_id.0.as_str(),
            &lineage,
        ))
    }

    async fn disconnect(&self, request: CanonicalClientRequest) -> Result<Value, JsonRpcError> {
        let CanonicalClientRequest::Disconnect {
            client_session_id,
            ownership_epoch,
            revision,
            updated_at,
        } = request
        else {
            return Err(JsonRpcError::new(
                ERROR_INVALID_REQUEST,
                "expected disconnect request",
            ));
        };
        let record = self
            .registry
            .transition(
                &client_session_id,
                ownership_epoch,
                revision,
                ClientSessionState::Disconnected,
                updated_at,
            )
            .await
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        self.session_contexts
            .lock()
            .expect("codex session contexts mutex")
            .insert(
                record.client_session_id.clone(),
                (record.ownership_epoch, record.revision),
            );
        Ok(json!({}))
    }

    async fn handle_client_response(
        &self,
        id: JsonRpcId,
        result: Result<Value, JsonRpcError>,
    ) -> Result<(), JsonRpcError> {
        self.require_ready()?;
        let key = match &id {
            Value::Null => "null".into(),
            other => other.to_string(),
        };
        let pending = self
            .pending_approvals
            .lock()
            .expect("codex approvals mutex")
            .remove(&key)
            .ok_or_else(|| {
                JsonRpcError::new(
                    ERROR_INVALID_REQUEST,
                    "approval response does not match a pending server request",
                )
            })?;
        let value = result?;
        let adapter = self.adapter()?;
        let request = adapter
            .decode_approval_response(value, &pending.turn_id, &pending.item_id, &key)
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        let _ = self
            .dispatcher
            .dispatch(request)
            .await
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        Ok(())
    }

    async fn register_thread(
        &self,
        session_id: &str,
        lineage: ThreadLineage,
    ) -> Result<(), JsonRpcError> {
        let adapter = self.adapter()?;
        let client_session_id = ClientSessionId::new(session_id);
        let record = ClientSessionRecord {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: adapter.protocol().clone(),
            client_session_id: client_session_id.clone(),
            core_session_id: SessionId::from(session_id),
            connection_id: self.connection_id.clone(),
            ownership_epoch: 1,
            revision: 1,
            state: ClientSessionState::Subscribed,
            capabilities: adapter.capabilities().clone(),
            updated_at: now_timestamp(),
        };
        self.registry
            .register(record.clone())
            .await
            .map_err(|error| map::jsonrpc_error_for(&error))?;
        self.session_contexts
            .lock()
            .expect("codex session contexts mutex")
            .insert(client_session_id, (record.ownership_epoch, record.revision));
        self.record_lineage(session_id, lineage);
        Ok(())
    }
}

fn session_id_from_response(frame: &CanonicalCoreFrame) -> Result<String, JsonRpcError> {
    match frame {
        CanonicalCoreFrame::Response(envelope) => match &envelope.response {
            AppResponse::Data(value) => value
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    JsonRpcError::new(
                        crate::wire::ERROR_INTERNAL,
                        "session command response did not carry session_id",
                    )
                }),
            _ => Err(JsonRpcError::new(
                crate::wire::ERROR_INTERNAL,
                "session command did not return Data(session_id)",
            )),
        },
        _ => Err(JsonRpcError::new(
            crate::wire::ERROR_INTERNAL,
            "session command did not produce a response envelope",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_runtime_identity_has_no_secrets() {
        let runtime = RuntimeIdentity::default();
        assert!(runtime.user_agent.contains(HOST_AGENT_NAME));
        assert!(!runtime.codex_home.contains("sk-"));
        assert!(!runtime.codex_home.contains("token"));
    }
}
