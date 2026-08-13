//! P18-10 ClientAdapter Host bridge.
//!
//! 所有 adapter 经过同一个 [`AppService`] 和 [`EventHub`]；本层只路由
//! canonical command/query/event，不解释客户端专有 JSON。

use std::sync::Arc;

use agent_domain::{ConnectionId, Timestamp};
use client_adapter_api::{
    AdapterError, AdapterSessionContext, CanonicalClientRequest, CanonicalCoreFrame,
    ClientSessionId, ClientSessionRecord, ClientSessionState, SessionRegistry,
};
use core_api::{AppCommand, GlobalSequence};
use subscription_hub::{EventHub, HubError, HubSubscription};

use crate::AppService;

struct ReattachRequest {
    client_session_id: ClientSessionId,
    ownership_epoch: u64,
    revision: u64,
    connection_id: ConnectionId,
    state: ClientSessionState,
    updated_at: Timestamp,
}

#[derive(Clone)]
pub struct ClientAdapterHost {
    service: Arc<AppService>,
    hub: Arc<EventHub>,
    registry: Arc<SessionRegistry>,
}

impl ClientAdapterHost {
    pub fn new(
        service: Arc<AppService>,
        hub: Arc<EventHub>,
        registry: Arc<SessionRegistry>,
    ) -> Self {
        Self {
            service,
            hub,
            registry,
        }
    }

    pub fn registry(&self) -> &Arc<SessionRegistry> {
        &self.registry
    }

    pub async fn dispatch(
        &self,
        context: AdapterSessionContext,
        request: CanonicalClientRequest,
    ) -> Result<CanonicalCoreFrame, AdapterError> {
        match request {
            CanonicalClientRequest::Command(envelope) => {
                // SessionCreate 例外（显式流程）：引导阶段允许未 attach 的
                // 客户端创建 Core session；attach 之前 Command/Query 一律
                // 要求已 attach。会话创建成功后，客户端用响应中的 core
                // session id 走 Attach，Attach 会核对 session 真实存在。
                if matches!(envelope.command, AppCommand::SessionCreate { .. }) {
                    return Ok(CanonicalCoreFrame::Response(
                        self.service.dispatch_envelope(envelope),
                    ));
                }
                let record = self.require_attached(&context).await?;
                // P17-9 审查阻塞：SessionClientContextReplace 必须落在该 client
                // 绑定的 core session 上。已 attach 的 client 不得跨 session
                // 改写他人上下文；GUI / 裸 AppCommand 没有 attach 绑定，根本到
                // 不了这里（Host authoritative registry 是唯一 ownership 来源，
                // 不在 client 侧另建第二状态源）。
                if let AppCommand::SessionClientContextReplace { session_id, .. } =
                    &envelope.command
                {
                    if record.core_session_id != *session_id {
                        return Err(AdapterError::InvalidFrame(format!(
                            "session_client_context_replace targets a core session not bound to this client session (bound to {})",
                            record.core_session_id
                        )));
                    }
                }
                Ok(CanonicalCoreFrame::Response(
                    self.service.dispatch_envelope(envelope),
                ))
            }
            CanonicalClientRequest::Query(envelope) => {
                self.require_attached(&context).await?;
                Ok(CanonicalCoreFrame::Response(
                    self.service.dispatch_query(envelope),
                ))
            }
            CanonicalClientRequest::Attach(record) => self.attach(context, record).await,
            CanonicalClientRequest::Reattach {
                client_session_id,
                ownership_epoch,
                revision,
                connection_id,
                state,
                updated_at,
            } => {
                self.reattach(
                    context,
                    ReattachRequest {
                        client_session_id,
                        ownership_epoch,
                        revision,
                        connection_id,
                        state,
                        updated_at,
                    },
                )
                .await
            }
            CanonicalClientRequest::Disconnect {
                client_session_id,
                ownership_epoch,
                revision,
                updated_at,
            } => {
                self.disconnect(
                    context,
                    client_session_id,
                    ownership_epoch,
                    revision,
                    updated_at,
                )
                .await
            }
        }
    }

    /// Command/Query 前置检查：session 必须已 attach（存在且非 Disconnected），
    /// 且调用方上下文与 authoritative 记录的 protocol / capability snapshot /
    /// ownership epoch / revision 完全匹配。任一不满足即拒绝，不做降级。
    /// 返回 authoritative 记录，供调用方做 command 维度的进一步核验（如
    /// SessionClientContextReplace 的跨 session 边界检查）。
    async fn require_attached(
        &self,
        context: &AdapterSessionContext,
    ) -> Result<ClientSessionRecord, AdapterError> {
        let record = self
            .registry
            .get(&context.client_session_id)
            .await
            .ok_or_else(|| AdapterError::UnknownSession(context.client_session_id.clone()))?;
        if record.state == ClientSessionState::Disconnected {
            return Err(AdapterError::SessionNotAttached(
                context.client_session_id.clone(),
            ));
        }
        ensure_binding(context, &record)?;
        ensure_owner(context, &record)?;
        Ok(record)
    }

    /// Attach 核对 Core session 真实存在（SessionCreate 例外流程：先经 Host
    /// 派发 SessionCreate 拿到 core session id，再 Attach）。协议宿主传回的
    /// 协商上下文是唯一可信来源：异常 adapter 无法用伪造的 protocol /
    /// capability snapshot / connection / ownership 注册会话。
    async fn attach(
        &self,
        context: AdapterSessionContext,
        record: ClientSessionRecord,
    ) -> Result<CanonicalCoreFrame, AdapterError> {
        if record.client_session_id != context.client_session_id {
            return Err(AdapterError::InvalidFrame(format!(
                "attach client_session_id {:?} does not match negotiated context {:?}",
                record.client_session_id, context.client_session_id
            )));
        }
        if record.connection_id != context.connection_id {
            return Err(AdapterError::InvalidFrame(format!(
                "attach connection_id {:?} does not match negotiated context {:?}",
                record.connection_id, context.connection_id
            )));
        }
        if record.protocol != *context.adapter.protocol() {
            return Err(AdapterError::InvalidFrame(format!(
                "attach protocol {:?} does not match negotiated adapter protocol {:?}",
                record.protocol,
                context.adapter.protocol()
            )));
        }
        if record.capabilities != *context.adapter.capabilities() {
            return Err(AdapterError::InvalidFrame(
                "attach capability snapshot does not match negotiated adapter".into(),
            ));
        }
        if record.ownership_epoch != context.ownership_epoch || record.revision != context.revision
        {
            return Err(AdapterError::InvalidFrame(format!(
                "attach ownership {}/{} does not match negotiated context {}/{}",
                record.ownership_epoch, record.revision, context.ownership_epoch, context.revision
            )));
        }
        if !self
            .service
            .router()
            .aggregate()
            .session_exists(&record.core_session_id)
        {
            return Err(AdapterError::CoreSessionNotFound(
                record.core_session_id.clone(),
            ));
        }
        self.registry.register(record.clone()).await?;
        Ok(CanonicalCoreFrame::SessionState(record))
    }

    async fn reattach(
        &self,
        context: AdapterSessionContext,
        request: ReattachRequest,
    ) -> Result<CanonicalCoreFrame, AdapterError> {
        ensure_request_handle(
            &context,
            &request.client_session_id,
            request.ownership_epoch,
            request.revision,
        )?;
        let record = self
            .registry
            .get(&request.client_session_id)
            .await
            .ok_or_else(|| AdapterError::UnknownSession(request.client_session_id.clone()))?;
        ensure_binding(&context, &record)?;
        if !self
            .service
            .router()
            .aggregate()
            .session_exists(&record.core_session_id)
        {
            return Err(AdapterError::CoreSessionNotFound(
                record.core_session_id.clone(),
            ));
        }
        let record = self
            .registry
            .claim(
                &request.client_session_id,
                request.ownership_epoch,
                request.revision,
                request.connection_id,
                request.state,
                request.updated_at,
            )
            .await?;
        Ok(CanonicalCoreFrame::SessionState(record))
    }

    async fn disconnect(
        &self,
        context: AdapterSessionContext,
        client_session_id: ClientSessionId,
        ownership_epoch: u64,
        revision: u64,
        updated_at: Timestamp,
    ) -> Result<CanonicalCoreFrame, AdapterError> {
        ensure_request_handle(&context, &client_session_id, ownership_epoch, revision)?;
        let record = self
            .registry
            .get(&client_session_id)
            .await
            .ok_or_else(|| AdapterError::UnknownSession(client_session_id.clone()))?;
        ensure_binding(&context, &record)?;
        // Disconnect 只标记状态，保留记录供 Reattach；清理归 P18-14。
        let record = self
            .registry
            .transition(
                &client_session_id,
                ownership_epoch,
                revision,
                ClientSessionState::Disconnected,
                updated_at,
            )
            .await?;
        Ok(CanonicalCoreFrame::SessionState(record))
    }

    pub fn subscribe(&self) -> HubSubscription {
        self.hub.subscribe()
    }

    pub fn replay(
        &self,
        from: GlobalSequence,
        to: Option<GlobalSequence>,
    ) -> Result<Vec<CanonicalCoreFrame>, HubError> {
        self.hub
            .replay(from, to)
            .map(|events| events.into_iter().map(CanonicalCoreFrame::Event).collect())
    }
}

fn ensure_binding(
    context: &AdapterSessionContext,
    record: &ClientSessionRecord,
) -> Result<(), AdapterError> {
    if record.protocol != *context.adapter.protocol()
        || record.capabilities != *context.adapter.capabilities()
    {
        return Err(AdapterError::InvalidFrame(format!(
            "session binding mismatch: negotiated adapter protocol {:?} / capability snapshot \
             vs authoritative record protocol {:?}",
            context.adapter.protocol(),
            record.protocol
        )));
    }
    Ok(())
}

fn ensure_owner(
    context: &AdapterSessionContext,
    record: &ClientSessionRecord,
) -> Result<(), AdapterError> {
    if record.ownership_epoch == context.ownership_epoch && record.revision == context.revision {
        Ok(())
    } else {
        Err(AdapterError::StaleOwner {
            client_session_id: context.client_session_id.clone(),
            expected_epoch: record.ownership_epoch,
            expected_revision: record.revision,
            actual_epoch: context.ownership_epoch,
            actual_revision: context.revision,
        })
    }
}

fn ensure_request_handle(
    context: &AdapterSessionContext,
    client_session_id: &ClientSessionId,
    ownership_epoch: u64,
    revision: u64,
) -> Result<(), AdapterError> {
    if context.client_session_id != *client_session_id
        || context.ownership_epoch != ownership_epoch
        || context.revision != revision
    {
        return Err(AdapterError::InvalidFrame(
            "request session handle does not match negotiated context".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{ActorId, QueryId, SessionId, WorkspaceId};
    use client_adapter_api::{
        CapabilitySnapshot, ClientAdapterFactory, ClientCapability, ClientProtocol,
        ClientSessionRecord, InMemorySessionRegistryStore, MockClientAdapterFactory,
        CLIENT_ADAPTER_SCHEMA_VERSION,
    };
    use core_api::{
        ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, AppResponse,
        CommandSource, API_VERSION,
    };
    use core_api::ClientContextSnapshot;

    fn snapshot() -> CapabilitySnapshot {
        CapabilitySnapshot {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new("mock"),
            protocol_version: "1".into(),
            client_version: "test".into(),
            revision: 1,
            capabilities: [ClientCapability::new("events")].into_iter().collect(),
        }
    }

    fn adapter() -> std::sync::Arc<dyn client_adapter_api::ClientAdapter> {
        let factory = MockClientAdapterFactory::new(
            ClientProtocol::new("mock"),
            [ClientCapability::new("events")],
        );
        factory.create(snapshot()).expect("negotiated adapter")
    }

    fn session_context(
        adapter: std::sync::Arc<dyn client_adapter_api::ClientAdapter>,
        epoch: u64,
        revision: u64,
    ) -> AdapterSessionContext {
        AdapterSessionContext {
            adapter,
            client_session_id: ClientSessionId::new("client-1"),
            connection_id: ConnectionId::from("connection-1"),
            ownership_epoch: epoch,
            revision,
        }
    }

    fn attach_record(core_session_id: SessionId, epoch: u64, revision: u64) -> ClientSessionRecord {
        ClientSessionRecord {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new("mock"),
            client_session_id: ClientSessionId::new("client-1"),
            core_session_id,
            connection_id: ConnectionId::from("connection-1"),
            ownership_epoch: epoch,
            revision,
            state: ClientSessionState::Loaded,
            capabilities: snapshot(),
            updated_at: Timestamp::from_unix_millis(1),
        }
    }

    fn query() -> CanonicalClientRequest {
        CanonicalClientRequest::Query(AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from("query-1"),
            source: CommandSource::LocalCli {
                terminal_session_id: None,
            },
            identity: ActorIdentity::LocalUser {
                actor_id: ActorId::from("adapter-user"),
                display_name: None,
            },
            issued_at: Timestamp::from_unix_millis(1),
            query: AppQuery::WorkspaceList,
        })
    }

    async fn attach_flow(
        service: &Arc<AppService>,
        host: &ClientAdapterHost,
        context: &AdapterSessionContext,
    ) {
        let response = host.dispatch(context.clone(), query()).await;
        assert!(matches!(response, Err(AdapterError::UnknownSession(_))));

        // 工作区由 Host 侧预置（不经 adapter 通道）：attach 前的引导例外
        // 只放行 SessionCreate，其余 Command/Query 仍要求已 attach。
        let workspace_response = service.dispatch_envelope(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: agent_domain::CommandId::from("cmd-workspace"),
            source: CommandSource::LocalCli {
                terminal_session_id: None,
            },
            identity: ActorIdentity::LocalUser {
                actor_id: ActorId::from("adapter-user"),
                display_name: None,
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(2),
            command: AppCommand::WorkspaceAdd {
                root_path: std::env::temp_dir().to_string_lossy().into_owned(),
            },
        });
        let AppResponse::Data(workspace) = workspace_response.response else {
            panic!("expected workspace data");
        };
        let workspace_id =
            WorkspaceId::from(workspace["id"].as_str().expect("workspace id in data"));

        // SessionCreate 例外流程：未 attach 时允许创建 Core session。
        let response = host
            .dispatch(
                context.clone(),
                CanonicalClientRequest::Command(AppCommandEnvelope {
                    api_version: API_VERSION,
                    command_id: agent_domain::CommandId::from("cmd-session"),
                    source: CommandSource::LocalCli {
                        terminal_session_id: None,
                    },
                    identity: ActorIdentity::LocalUser {
                        actor_id: ActorId::from("adapter-user"),
                        display_name: None,
                    },
                    expected_revision: None,
                    idempotency_key: None,
                    issued_at: Timestamp::from_unix_millis(3),
                    command: AppCommand::SessionCreate {
                        workspace_id,
                        title: Some("adapter-session".into()),
                    },
                }),
            )
            .await
            .expect("session create via exception flow");
        let CanonicalCoreFrame::Response(session_response) = response else {
            panic!("expected response");
        };
        let AppResponse::Data(session) = session_response.response else {
            panic!("expected session data");
        };
        let core_session_id =
            SessionId::from(session["session_id"].as_str().expect("session_id in data"));

        let response = host
            .dispatch(
                context.clone(),
                CanonicalClientRequest::Attach(attach_record(
                    core_session_id.clone(),
                    context.ownership_epoch,
                    context.revision,
                )),
            )
            .await
            .expect("attach after session create");
        let CanonicalCoreFrame::SessionState(record) = response else {
            panic!("expected session state");
        };
        assert_eq!(record.core_session_id, core_session_id);
        assert_eq!(record.state, ClientSessionState::Loaded);
    }

    fn client_context_snapshot() -> ClientContextSnapshot {
        ClientContextSnapshot {
            revision: 1,
            active_document: None,
            open_documents: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn context_replace(session_id: SessionId) -> CanonicalClientRequest {
        CanonicalClientRequest::Command(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: agent_domain::CommandId::from("cmd-context"),
            source: CommandSource::LocalCli {
                terminal_session_id: None,
            },
            identity: ActorIdentity::LocalUser {
                actor_id: ActorId::from("adapter-user"),
                display_name: None,
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(4),
            command: AppCommand::SessionClientContextReplace {
                session_id,
                snapshot: client_context_snapshot(),
            },
        })
    }

    #[tokio::test]
    async fn adapter_host_routes_only_canonical_queries() {
        let service = Arc::new(AppService::new("adapter-test"));
        let hub = Arc::new(EventHub::new());
        let store = Arc::new(InMemorySessionRegistryStore::default());
        let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
        let host = ClientAdapterHost::new(service.clone(), hub, registry);
        attach_flow(&service, &host, &session_context(adapter(), 1, 1)).await;
        let context = session_context(adapter(), 1, 1);
        let response = host.dispatch(context, query()).await.expect("dispatch");
        assert!(matches!(response, CanonicalCoreFrame::Response(_)));
    }

    #[tokio::test]
    async fn adapter_host_command_and_query_require_attached_matching_session() {
        let service = Arc::new(AppService::new("adapter-test"));
        let hub = Arc::new(EventHub::new());
        let store = Arc::new(InMemorySessionRegistryStore::default());
        let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
        let host = ClientAdapterHost::new(service.clone(), hub, registry.clone());
        attach_flow(&service, &host, &session_context(adapter(), 1, 1)).await;

        // 未 attach 的陌生 session 拒绝。
        let unknown = AdapterSessionContext {
            adapter: adapter(),
            client_session_id: ClientSessionId::new("client-unknown"),
            connection_id: ConnectionId::from("connection-1"),
            ownership_epoch: 7,
            revision: 7,
        };
        assert!(matches!(
            host.dispatch(unknown, query()).await,
            Err(AdapterError::UnknownSession(_))
        ));

        // 陈旧 ownership 拒绝。
        let stale = session_context(adapter(), 0, 0);
        assert!(matches!(
            host.dispatch(stale, query()).await,
            Err(AdapterError::StaleOwner { .. })
        ));

        // 伪造协议/capability 的异常 adapter 拒绝。
        let mut forged_snapshot = snapshot();
        forged_snapshot.protocol = ClientProtocol::new("acp");
        let forged = MockClientAdapterFactory::new(
            ClientProtocol::new("acp"),
            [ClientCapability::new("events")],
        )
        .create(forged_snapshot)
        .expect("forged adapter");
        assert!(matches!(
            host.dispatch(session_context(forged, 1, 1), query()).await,
            Err(AdapterError::InvalidFrame(_))
        ));

        // 断连 session 拒绝（记录保留，供 reattach）。
        registry
            .transition(
                &ClientSessionId::new("client-1"),
                1,
                1,
                ClientSessionState::Disconnected,
                Timestamp::from_unix_millis(9),
            )
            .await
            .expect("disconnect");
        let disconnected = session_context(adapter(), 1, 1);
        assert!(matches!(
            host.dispatch(disconnected, query()).await,
            Err(AdapterError::SessionNotAttached(_))
        ));
    }

    #[tokio::test]
    async fn adapter_host_attach_rejects_forged_binding_and_unknown_core_session() {
        let service = Arc::new(AppService::new("adapter-test"));
        let hub = Arc::new(EventHub::new());
        let store = Arc::new(InMemorySessionRegistryStore::default());
        let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
        let host = ClientAdapterHost::new(service.clone(), hub, registry);
        let context = session_context(adapter(), 1, 1);

        // Core 中不存在的 session 拒绝 attach。
        let mut phantom = attach_record(SessionId::from("phantom-session"), 1, 1);
        assert!(matches!(
            host.dispatch(
                context.clone(),
                CanonicalClientRequest::Attach(phantom.clone())
            )
            .await,
            Err(AdapterError::CoreSessionNotFound(_))
        ));

        // 伪造 protocol 的 attach 记录拒绝。
        phantom.protocol = ClientProtocol::new("acp");
        assert!(matches!(
            host.dispatch(
                context.clone(),
                CanonicalClientRequest::Attach(phantom.clone())
            )
            .await,
            Err(AdapterError::InvalidFrame(_))
        ));

        // 伪造 capability snapshot 的 attach 记录拒绝。
        let mut forged_caps = attach_record(SessionId::from("phantom-session"), 1, 1);
        forged_caps
            .capabilities
            .capabilities
            .insert(ClientCapability::new("approval"));
        assert!(matches!(
            host.dispatch(context.clone(), CanonicalClientRequest::Attach(forged_caps))
                .await,
            Err(AdapterError::InvalidFrame(_))
        ));

        // 伪造 ownership/connection 的 attach 记录拒绝。
        let mut forged_owner = attach_record(SessionId::from("phantom-session"), 9, 9);
        forged_owner.connection_id = ConnectionId::from("connection-forged");
        assert!(matches!(
            host.dispatch(context, CanonicalClientRequest::Attach(forged_owner))
                .await,
            Err(AdapterError::InvalidFrame(_))
        ));
    }

    #[tokio::test]
    async fn adapter_host_disconnect_keeps_record_for_reattach() {
        let service = Arc::new(AppService::new("adapter-test"));
        let hub = Arc::new(EventHub::new());
        let store = Arc::new(InMemorySessionRegistryStore::default());
        let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
        let host = ClientAdapterHost::new(service.clone(), hub, registry);
        let context = session_context(adapter(), 1, 1);
        attach_flow(&service, &host, &context).await;

        let response = host
            .dispatch(
                context.clone(),
                CanonicalClientRequest::Disconnect {
                    client_session_id: ClientSessionId::new("client-1"),
                    ownership_epoch: 1,
                    revision: 1,
                    updated_at: Timestamp::from_unix_millis(10),
                },
            )
            .await
            .expect("disconnect");
        let CanonicalCoreFrame::SessionState(record) = response else {
            panic!("expected session state");
        };
        assert_eq!(record.state, ClientSessionState::Disconnected);
        assert_eq!(record.revision, 2);

        // 记录保留（供 reattach），但断连后 Command/Query 拒绝。
        assert_eq!(
            host.registry()
                .get(&ClientSessionId::new("client-1"))
                .await
                .expect("record retained")
                .state,
            ClientSessionState::Disconnected
        );
        assert!(matches!(
            host.dispatch(context.clone(), query()).await,
            Err(AdapterError::SessionNotAttached(_))
        ));

        // Reattach：以断连后的 revision 重新 claim，epoch/revision 前进。
        let reattached = session_context(adapter(), 1, 2);
        let response = host
            .dispatch(
                reattached.clone(),
                CanonicalClientRequest::Reattach {
                    client_session_id: ClientSessionId::new("client-1"),
                    ownership_epoch: 1,
                    revision: 2,
                    connection_id: ConnectionId::from("connection-2"),
                    state: ClientSessionState::Subscribed,
                    updated_at: Timestamp::from_unix_millis(11),
                },
            )
            .await
            .expect("reattach");
        let CanonicalCoreFrame::SessionState(record) = response else {
            panic!("expected session state");
        };
        assert_eq!((record.ownership_epoch, record.revision), (2, 3));
        assert_eq!(record.connection_id.as_str(), "connection-2");

        // 重新 attach 后 Command/Query 恢复；新 ownership (2,3) 生效。
        assert!(matches!(
            host.dispatch(session_context(adapter(), 2, 3), query())
                .await,
            Ok(CanonicalCoreFrame::Response(_))
        ));
    }

    #[tokio::test]
    async fn adapter_host_replay_returns_published_events() {
        use agent_domain::CoreInstanceId;
        use core_api::{AppEvent, AppEventEnvelope, EventSource, EventStream, GlobalSequence};

        let service = Arc::new(AppService::new("adapter-test"));
        let hub = Arc::new(EventHub::new());
        let store = Arc::new(InMemorySessionRegistryStore::default());
        let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
        let host = ClientAdapterHost::new(service.clone(), hub.clone(), registry);

        let published = hub.publish(AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("instance-1"),
            event_id: agent_domain::EventId::from("event-1"),
            global_sequence: GlobalSequence(0),
            stream: EventStream::Session(SessionId::from("core-session")),
            stream_sequence: 1,
            timestamp: Timestamp::from_unix_millis(1),
            source: EventSource::Core,
            payload: AppEvent::SessionChanged {
                session_id: SessionId::from("core-session"),
                revision: 1,
            },
        });
        assert_eq!(published, 0, "no subscribers yet");

        let frames = host
            .replay(GlobalSequence(1), Some(GlobalSequence(1)))
            .expect("replay");
        assert_eq!(frames.len(), 1);
        let CanonicalCoreFrame::Event(event) = &frames[0] else {
            panic!("expected event frame");
        };
        assert_eq!(event.event_id.as_str(), "event-1");
    }

    #[tokio::test]
    async fn adapter_host_reattach_claims_authoritative_session() {
        let service = Arc::new(AppService::new("adapter-test"));
        let hub = Arc::new(EventHub::new());
        let store = Arc::new(InMemorySessionRegistryStore::default());
        let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
        let host = ClientAdapterHost::new(service.clone(), hub, registry.clone());
        let context = session_context(adapter(), 1, 1);
        attach_flow(&service, &host, &context).await;
        registry
            .transition(
                &ClientSessionId::new("client-1"),
                1,
                1,
                ClientSessionState::Disconnected,
                Timestamp::from_unix_millis(2),
            )
            .await
            .expect("disconnect for reattach");

        let response = host
            .dispatch(
                session_context(adapter(), 1, 2),
                CanonicalClientRequest::Reattach {
                    client_session_id: ClientSessionId::new("client-1"),
                    ownership_epoch: 1,
                    revision: 2,
                    connection_id: ConnectionId::from("connection-2"),
                    state: ClientSessionState::Subscribed,
                    updated_at: Timestamp::from_unix_millis(3),
                },
            )
            .await
            .expect("reattach");
        let CanonicalCoreFrame::SessionState(record) = response else {
            panic!("expected session state");
        };
        assert_eq!(record.ownership_epoch, 2);
        assert_eq!(record.revision, 3);
        assert_eq!(record.connection_id.as_str(), "connection-2");

        // 旧 handle (1,2) 已失效，新 handle (2,3) 恢复 Command/Query。
        assert!(matches!(
            host.dispatch(session_context(adapter(), 1, 2), query())
                .await,
            Err(AdapterError::StaleOwner { .. })
        ));
        assert!(matches!(
            host.dispatch(session_context(adapter(), 2, 3), query())
                .await,
            Ok(CanonicalCoreFrame::Response(_))
        ));
    }

    #[tokio::test]
    async fn adapter_host_rejects_cross_session_client_context_replace() {
        // P17-9 审查阻塞：已 attach 到 core session A 的 client 不得把上下文
        // 写到 session B。Host authoritative registry 在派发前核验
        // client_session→core_session 绑定，跨 session 写直接拒绝，aggregate
        // 不被触达。GUI / 裸 AppCommand 没有 attach 绑定，根本到不了这里。
        let service = Arc::new(AppService::new("adapter-test"));
        let hub = Arc::new(EventHub::new());
        let store = Arc::new(InMemorySessionRegistryStore::default());
        let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
        let host = ClientAdapterHost::new(service.clone(), hub, registry);
        let context = session_context(adapter(), 1, 1);
        attach_flow(&service, &host, &context).await;

        // 绑定到 client-1 的真实 core session 写入：放行（落到 aggregate）。
        let bound = host
            .registry()
            .get(&ClientSessionId::new("client-1"))
            .await
            .expect("attached record");
        let ok = host
            .dispatch(context.clone(), context_replace(bound.core_session_id.clone()))
            .await
            .expect("own-session context replace allowed");
        assert!(matches!(ok, CanonicalCoreFrame::Response(_)));

        // 跨 session：目标 session 不属于该 client 绑定，Host authoritative
        // registry 在派发前拒绝。
        let cross = host
            .dispatch(
                context.clone(),
                context_replace(SessionId::from("session-not-mine")),
            )
            .await;
        assert!(
            matches!(cross, Err(AdapterError::InvalidFrame(_))),
            "cross-session client context replace must be rejected by authoritative registry"
        );
    }
}
