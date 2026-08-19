use async_trait::async_trait;
use pawork_domain::AgentEventEnvelope;
use pawork_engine::{AgentEventSink, EngineError};
use pawork_storage::session::SessionStore;

/// persist-first：先 `append_event`，成功后再交给渲染 sink。
///
/// `branch_id` 必须是该 session 当前 active branch（fork 后不再默认 `main`）。
pub struct PersistThenRender<'a> {
    pub store: &'a SessionStore,
    pub render: &'a dyn AgentEventSink,
    pub branch_id: String,
}

#[async_trait]
impl AgentEventSink for PersistThenRender<'_> {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        self.store
            .append_event(&self.branch_id, envelope.clone())
            .await
            .map_err(|error| EngineError::sink(error.to_string()))?;
        self.render.emit(envelope).await
    }
}
