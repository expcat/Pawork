//! 共享测试夹具：固定 cwd 解析、session 归属、以及不依赖 app-service 的 Core mock。
#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use client_adapter_api::{
    AdapterError, CanonicalClientRequest, CanonicalCoreFrame, ClientSessionId,
    InMemorySessionRegistryStore, SessionRegistry,
};
use client_codex_app_server::{
    CodexAppServerAdapterFactory, CodexAppServerHost, CoreDispatcher, CwdResolver, RuntimeIdentity,
    SessionResolver,
};
use core_api::{AppCommand, AppEventEnvelope, AppResponse, AppResponseEnvelope, API_VERSION};
use serde_json::{json, Value};

pub const TEST_CWD: &str = "/tmp/pawork-codex";
pub const TEST_WORKSPACE: &str = "ws-codex";

pub struct FixedCwdResolver;

#[async_trait]
impl CwdResolver for FixedCwdResolver {
    async fn resolve(&self, cwd: &str) -> Result<agent_domain::WorkspaceId, AdapterError> {
        if cwd != TEST_CWD {
            return Err(AdapterError::InvalidFrame(format!(
                "cwd `{cwd}` is not a registered workspace"
            )));
        }
        Ok(agent_domain::WorkspaceId::from(TEST_WORKSPACE))
    }
}

pub struct FixedSessionResolver(pub ClientSessionId);

#[async_trait]
impl SessionResolver for FixedSessionResolver {
    async fn resolve_client_session(&self, _event: &AppEventEnvelope) -> Option<ClientSessionId> {
        Some(self.0.clone())
    }
}

/// 不接触 app-service 的 Core 替身：只回 Data(session_id) / Accepted(run_id)。
pub struct MockCore {
    next_session: AtomicU64,
    next_run: AtomicU64,
}

impl MockCore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            next_session: AtomicU64::new(1),
            next_run: AtomicU64::new(1),
        })
    }
}

#[async_trait]
impl CoreDispatcher for MockCore {
    async fn dispatch(
        &self,
        request: CanonicalClientRequest,
    ) -> Result<CanonicalCoreFrame, AdapterError> {
        let CanonicalClientRequest::Command(envelope) = request else {
            return Err(AdapterError::ProtocolUnsupported(
                "mock core only handles canonical commands".into(),
            ));
        };
        let request_id = agent_domain::QueryId::from(envelope.command_id.as_str());
        let response = match envelope.command {
            AppCommand::SessionCreate { .. } | AppCommand::SessionFork { .. } => {
                let n = self.next_session.fetch_add(1, Ordering::SeqCst);
                AppResponse::Data(json!({ "session_id": format!("thr_{n}") }))
            }
            AppCommand::SessionOpen { session_id } => {
                AppResponse::Data(json!({ "session_id": session_id.as_str() }))
            }
            AppCommand::RunStart { .. } => {
                let n = self.next_run.fetch_add(1, Ordering::SeqCst);
                AppResponse::Accepted {
                    command_id: envelope.command_id.clone(),
                    run_id: Some(agent_domain::RunId::from(format!("turn_{n}"))),
                }
            }
            AppCommand::RunCancel { .. }
            | AppCommand::SessionCompact { .. }
            | AppCommand::ToolApprove { .. } => AppResponse::Accepted {
                command_id: envelope.command_id.clone(),
                run_id: None,
            },
            other => {
                return Err(AdapterError::ProtocolUnsupported(format!(
                    "mock core does not handle {other:?}"
                )));
            }
        };
        Ok(CanonicalCoreFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id,
            responded_at: agent_domain::Timestamp::from_unix_millis(1),
            response,
        }))
    }
}

pub fn test_runtime() -> RuntimeIdentity {
    RuntimeIdentity {
        user_agent: "pawork-codex-app-server/0.0.0".into(),
        codex_home: "pawork://codex-home".into(),
        platform_family: "pawork".into(),
        platform_os: "test".into(),
    }
}

pub async fn new_host() -> CodexAppServerHost {
    let store = Arc::new(InMemorySessionRegistryStore::default());
    let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
    let factory = CodexAppServerAdapterFactory::with_defaults(
        Arc::clone(&registry),
        Arc::new(FixedCwdResolver),
        Arc::new(FixedSessionResolver(ClientSessionId::new("thr_1"))),
    );
    CodexAppServerHost::with_runtime(factory, registry, MockCore::new(), test_runtime())
}

pub fn initialize_params() -> Value {
    json!({
        "clientInfo": {
            "name": "pawork-test",
            "title": "Pawork Codex Test",
            "version": "0.0.0"
        }
    })
}

pub fn initialize_params_experimental() -> Value {
    json!({
        "clientInfo": {
            "name": "pawork-test",
            "version": "0.0.0"
        },
        "capabilities": {
            "experimentalApi": true
        }
    })
}

pub async fn handshake(host: &CodexAppServerHost) {
    host.handle_request(json!(0), "initialize", Some(initialize_params()))
        .await
        .expect("initialize");
    host.handle_notification("initialized", None)
        .await
        .expect("initialized");
}

pub fn fixture(path: &str) -> Value {
    let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    serde_json::from_str(&std::fs::read_to_string(&full).expect("read fixture"))
        .expect("fixture is valid JSON")
}
