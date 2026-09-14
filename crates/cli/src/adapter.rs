//! 把 `AppCore` 装成 `GuiHostAdapter`，并给 ACP 提供 `AcpCommandHost`。

use std::sync::Arc;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use crate::channels::{AcpCommandHost, AcpHostError};
use async_trait::async_trait;
use pawork_app::gui_server::GuiHost;
use pawork_app::{AppCore, GuiApprovalHost, GuiHostAdapter};
use pawork_domain::{CommandId, QueryId, Timestamp};
use pawork_protocol::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppEventEnvelope, AppQueryEnvelope, AppResponse,
    AppResponseEnvelope, CommandSource, API_VERSION,
};
use tokio::sync::RwLock;

static NEXT_COMMAND: AtomicU64 = AtomicU64::new(1);

static COMMAND_NAMESPACE: OnceLock<String> = OnceLock::new();

/// 跨进程唯一的命令命名空间。command_ledger 以 (tenant, scope, command_id)
/// 持久幂等：裸计数器（cli-<name>-1…）会让不同进程的不同逻辑命令撞键，
/// 命中旧记录重放出旧 session / 已完成 run 的响应（hang 实证见
/// docs/ROADMAP.md §2.2）。与 client 的 new_request_namespace 同形态。
fn command_namespace() -> &'static str {
    COMMAND_NAMESPACE.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        format!("{:x}-{nanos:x}", std::process::id())
    })
}

pub fn adapter_from_locked(core: AppCore, approvals: Arc<GuiApprovalHost>) -> GuiHostAdapter {
    GuiHostAdapter::from_locked(Arc::new(RwLock::new(core)), approvals)
}

pub fn adapter_with_gui_approvals(core: AppCore) -> GuiHostAdapter {
    GuiHostAdapter::with_approvals(Arc::new(core), Arc::new(GuiApprovalHost::new()))
}

pub fn stamp_automation(mut envelope: AppCommandEnvelope, name: &str) -> AppCommandEnvelope {
    envelope.source = CommandSource::Automation;
    envelope.identity = ActorIdentity::Automation { name: name.into() };
    envelope
}

pub fn stamp_query(mut envelope: AppQueryEnvelope, name: &str) -> AppQueryEnvelope {
    envelope.source = CommandSource::Automation;
    envelope.identity = ActorIdentity::Automation { name: name.into() };
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
            command_id: CommandId::from(format!("cli-{name}-{}-{n}", command_namespace())),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation { name: name.into() },
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

#[cfg(test)]
mod tests {
    use super::*;

    /// command_ledger 跨进程持久幂等：command_id 必须含进程级命名空间，
    /// 否则不同进程的不同逻辑命令会撞键并重放旧响应（pawork run 挂死）。
    #[test]
    fn command_ids_carry_process_namespace_and_stay_unique() {
        let first = command_envelope(
            AppCommand::SessionCreate {
                workspace_id: None,
                title: None,
            },
            "cli-json",
        );
        let second = command_envelope(
            AppCommand::SessionCreate {
                workspace_id: None,
                title: None,
            },
            "cli-json",
        );
        let first_id = first.command_id.as_str();
        let second_id = second.command_id.as_str();
        let namespace = command_namespace();
        assert!(!namespace.is_empty());
        assert!(first_id.starts_with(&format!("cli-cli-json-{namespace}-")));
        assert!(second_id.starts_with(&format!("cli-cli-json-{namespace}-")));
        assert_ne!(first_id, second_id);
    }
}
