//! SDK/Headless 通道抽象。
//!
//! [`SdkChannel`] 是 Adapter 与 `pawork` Host 之间唯一的执行通道：真实实现
//! [`PaworkSdkChannel`] 委托 `agent-sdk` 的 [`PaworkClient`]（`pawork headless
//! --json-stdio` NDJSON）；测试与下游集成可用 mock 替换，不 spawn 真实进程。

use std::sync::Arc;

use agent_domain::{ModelId, RunId, SessionId, WorkspaceId};
use agent_sdk::client::{CancelOutcome, RunView, SessionView};
use agent_sdk::{BackpressurePolicy, EventSubscription, PaworkClient, SdkError};
use async_trait::async_trait;
use core_api::{AppCommand, AppQuery, AppResponseEnvelope, EventStream};
use headless_json::SdkCapability;

/// SDK/Headless 通道契约（与 `agent-sdk` 高层 API 同构）。
#[async_trait]
pub trait SdkChannel: Send + Sync {
    async fn create_session(
        &self,
        workspace_id: WorkspaceId,
        title: Option<String>,
    ) -> Result<SessionView, SdkError>;

    async fn open_session(&self, session_id: SessionId) -> Result<SessionView, SdkError>;

    async fn run_start(
        &self,
        session_id: SessionId,
        user_message: String,
        model: Option<ModelId>,
    ) -> Result<RunView, SdkError>;

    async fn cancel(&self, run_id: RunId) -> Result<CancelOutcome, SdkError>;

    async fn run_status(&self, run_id: RunId) -> Result<RunView, SdkError>;

    async fn command(&self, command: AppCommand) -> Result<AppResponseEnvelope, SdkError>;

    async fn query(&self, query: AppQuery) -> Result<AppResponseEnvelope, SdkError>;

    async fn subscribe(
        &self,
        stream: EventStream,
        capacity: usize,
    ) -> Result<EventSubscription, SdkError>;

    async fn capabilities(&self) -> Vec<SdkCapability>;

    async fn instance_id(&self) -> Option<String>;

    fn is_open(&self) -> bool;

    async fn close(&self) -> Result<(), SdkError>;
}

/// 真实通道：委托 `PaworkClient`（`pawork headless --json-stdio`）。
#[derive(Clone)]
pub struct PaworkSdkChannel {
    client: Arc<PaworkClient>,
}

impl PaworkSdkChannel {
    pub fn new(client: PaworkClient) -> Self {
        Self {
            client: Arc::new(client),
        }
    }

    pub fn from_arc(client: Arc<PaworkClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SdkChannel for PaworkSdkChannel {
    async fn create_session(
        &self,
        workspace_id: WorkspaceId,
        title: Option<String>,
    ) -> Result<SessionView, SdkError> {
        self.client.create_session(workspace_id, title).await
    }

    async fn open_session(&self, session_id: SessionId) -> Result<SessionView, SdkError> {
        self.client.open_session(session_id).await
    }

    async fn run_start(
        &self,
        session_id: SessionId,
        user_message: String,
        model: Option<ModelId>,
    ) -> Result<RunView, SdkError> {
        self.client.run_start(session_id, user_message, model).await
    }

    async fn cancel(&self, run_id: RunId) -> Result<CancelOutcome, SdkError> {
        self.client.cancel(run_id).await
    }

    async fn run_status(&self, run_id: RunId) -> Result<RunView, SdkError> {
        self.client.run_status(run_id).await
    }

    async fn command(&self, command: AppCommand) -> Result<AppResponseEnvelope, SdkError> {
        self.client.command(command).await
    }

    async fn query(&self, query: AppQuery) -> Result<AppResponseEnvelope, SdkError> {
        self.client.query(query).await
    }

    async fn subscribe(
        &self,
        stream: EventStream,
        capacity: usize,
    ) -> Result<EventSubscription, SdkError> {
        self.client
            .subscribe(stream, BackpressurePolicy::Drop, capacity)
            .await
    }

    async fn capabilities(&self) -> Vec<SdkCapability> {
        self.client.capabilities().await
    }

    async fn instance_id(&self) -> Option<String> {
        self.client.instance_id().await
    }

    fn is_open(&self) -> bool {
        self.client.is_open()
    }

    async fn close(&self) -> Result<(), SdkError> {
        self.client.close().await
    }
}
