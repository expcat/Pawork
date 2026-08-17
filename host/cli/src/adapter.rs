//! 把 `AppCore` 装成 `GuiHostAdapter`，并给 ACP 提供 `AcpCommandHost`。

use std::sync::Arc;

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use pawork_app::{AppCore, GuiApprovalHost, GuiHostAdapter};
use pawork_channels::{AcpCommandHost, AcpHostError};
use pawork_domain::{CommandId, QueryId, Timestamp};
use pawork_gui_server::GuiHost;
use pawork_protocol::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppEventEnvelope, AppQueryEnvelope, AppResponse,
    AppResponseEnvelope, CommandSource, API_VERSION,
};
use tokio::sync::RwLock;

static NEXT_COMMAND: AtomicU64 = AtomicU64::new(1);

pub fn adapter_from_locked(core: AppCore, approvals: Arc<GuiApprovalHost>) -> GuiHostAdapter {
    GuiHostAdapter::from_locked(Arc::new(RwLock::new(core)), approvals)
}

pub fn adapter_with_gui_approvals(core: AppCore) -> GuiHostAdapter {
    GuiHostAdapter::with_approvals(Arc::new(core), Arc::new(GuiApprovalHost::new()))
}

pub fn stamp_automation(mut envelope: AppCommandEnvelope, name: &str) -> AppCommandEnvelope {
    envelope.source = CommandSource::Automation;
    envelope.identity = ActorIdentity::Automation {
        name: name.into(),
    };
    envelope
}

pub fn stamp_query(mut envelope: AppQueryEnvelope, name: &str) -> AppQueryEnvelope {
    envelope.source = CommandSource::Automation;
    envelope.identity = ActorIdentity::Automation {
        name: name.into(),
    };
    envelope
}

pub fn wrap_response(request_id: &str, response: AppResponse) -> AppResponseEnvelope {
    AppResponseEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from(request_id),
        responded_at: now_timestamp(),
        response,
    }
}

pub fn command_envelope(command: AppCommand, name: &str) -> AppCommandEnvelope {
    let n = NEXT_COMMAND.fetch_add(1, Ordering::Relaxed);
    stamp_automation(
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from(format!("cli-{name}-{n}")),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: name.into(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: now_timestamp(),
            command,
        },
        name,
    )
}

pub fn now_timestamp() -> Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    Timestamp::from_unix_millis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0),
    )
}

pub struct CliAcpCommandHost {
    adapter: Arc<GuiHostAdapter>,
}

impl CliAcpCommandHost {
    pub fn new(adapter: Arc<GuiHostAdapter>) -> Self {
        Self { adapter }
    }
}

#[async_trait]
impl AcpCommandHost for CliAcpCommandHost {
    async fn dispatch(
        &self,
        command: AppCommandEnvelope,
    ) -> Result<AppResponseEnvelope, AcpHostError> {
        let command = stamp_automation(command, "acp");
        let response = self
            .adapter
            .command(&command)
            .await
            .map_err(|error| AcpHostError::Unavailable(error.to_string()))?;
        Ok(wrap_response(command.command_id.as_str(), response))
    }

    async fn query(&self, query: AppQueryEnvelope) -> Result<AppResponseEnvelope, AcpHostError> {
        let query = stamp_query(query, "acp");
        let response = self
            .adapter
            .query(&query)
            .await
            .map_err(|error| AcpHostError::Unavailable(error.to_string()))?;
        Ok(wrap_response(query.request_id.as_str(), response))
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AppEventEnvelope> {
        self.adapter.subscribe_events()
    }
}
