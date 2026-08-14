use async_trait::async_trait;
use pawork_domain::AgentEventEnvelope;
use pawork_engine::{AgentEventSink, EngineError};
use pawork_session::{SessionStore, DEFAULT_BRANCH_ID};

/// persist-first：先 `append_event`，成功后再交给渲染 sink。
pub struct PersistThenRender<'a> {
    pub store: &'a SessionStore,
    pub render: &'a dyn AgentEventSink,
}

#[async_trait]
impl AgentEventSink for PersistThenRender<'_> {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        self.store
            .append_event(DEFAULT_BRANCH_ID, envelope.clone())
            .await
            .map_err(|error| EngineError::sink(error.to_string()))?;
        self.render.emit(envelope).await
    }
}
