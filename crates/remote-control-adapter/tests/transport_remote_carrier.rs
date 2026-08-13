//! P17-12 transport-remote 承载集成证据（仅 dev-dependency，不改其源码）。
//!
//! 在真实 TCP + TLS 1.3 承载（transport-remote）上验证 remote-control-adapter
//! 的受限协议：未认证拒绝、配对/激活/凭证认证、受限查询（SessionGet /
//! RunStatus / PlanStatus 显式可用性标记）、受限命令（RunStart 携带 run_id）、
//! 终态通知推送、Full 透传位显式拒绝 + 审计、按序 Replay、revoke 后显式
//! 告知并关闭连接、Debug/审计不落明文 Secret。

use std::sync::Arc;
use std::time::Duration;

use agent_domain::{CommandId, RunId, SessionId, Timestamp, WorkspaceId};
use app_service::AppService;
use client_auth::TokenStore;
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppResponse, ApprovalDecision,
    CommandSource, RunState, API_VERSION,
};
use remote_control_adapter::{
    AuditEvent, ClientFrame, NotificationPayload, RemoteCommand, RemoteControlService, RemoteQuery,
    ServerFrame, DENY_CONTENT_READ, DENY_FILE_WRITE, DENY_NOT_EXPOSED, DENY_PROVIDER_DIRECT_ACCESS,
    DENY_SESSION_MUTATION, DENY_TOOL_EXECUTION,
};
use subscription_hub::EventHub;
use tempfile::TempDir;
use test_support::{MockProvider, MockScript};
use tokio::time::{timeout, Instant};
use transport_remote::{
    ConnectOptions, GuiConnection, GuiListener, GuiTransportServer, RealRemoteConnector,
    RealRemoteTransport, RealRemoteTransportConfig, RealRemoteTransportProvider,
    RemoteGuiConnector, RemoteGuiTransportProvider, RemotePublishRequest, TransportEndpoint,
    TransportFrame,
};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const MATCH_TIMEOUT: Duration = Duration::from_secs(10);

/// 测试承载：AppService + EventHub + RemoteControlService 发布在
/// transport-remote 真实端点上；事件泵将 canonical 事件注入 Hub。
struct Carrier {
    _temp: TempDir,
    service: RemoteControlService,
    endpoint: TransportEndpoint,
    connector: RealRemoteConnector,
    _listener: Arc<dyn GuiListener>,
}

async fn setup(name: &str) -> (Carrier, WorkspaceId, SessionId) {
    let temp = TempDir::new().expect("tempdir");
    let app = Arc::new(AppService::new(name));
    app.register_provider(Arc::new(MockProvider::new(
        MockScript::new().text("ok").complete(),
    )));
    let hub = Arc::new(EventHub::new());
    let service = RemoteControlService::new(Arc::clone(&app), Arc::clone(&hub));

    // 事件泵：AppService 事件队列 -> EventHub -> 各连接通知推送。
    let pump_app = Arc::clone(&app);
    let pump_hub = Arc::clone(&hub);
    tokio::spawn(async move {
        loop {
            for envelope in pump_app.drain_events() {
                pump_hub.publish(envelope);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });

    // transport-remote 承载：发布 + 绑定 + 接受循环（不修改其源码）。
    let token_store = TokenStore::new(temp.path().join("carrier.token"));
    let transport = Arc::new(RealRemoteTransport::new(RealRemoteTransportConfig::new(
        token_store,
        None,
    )));
    let provider = RealRemoteTransportProvider::new(Arc::clone(&transport));
    let handle = provider
        .publish(RemotePublishRequest {
            name: format!("{name}-endpoint"),
        })
        .await
        .expect("publish");
    let listener: Arc<dyn GuiListener> =
        Arc::from(transport.bind(handle.endpoint.clone()).await.expect("bind"));
    let accept_listener = Arc::clone(&listener);
    let accept_service = service.clone();
    tokio::spawn(async move {
        while let Ok(connection) = accept_listener.accept().await {
            let serving = accept_service.clone();
            tokio::spawn(async move {
                let _summary = serving.serve_connection(connection).await;
            });
        }
    });

    let connector = RealRemoteConnector::new(Arc::clone(&transport), None);

    // 宿主侧引导：workspace + session（经 canonical 信封，非远程通道）。
    let root = temp.path().join("workspace");
    std::fs::create_dir_all(&root).expect("create workspace dir");
    let workspace = app.dispatch_envelope(host_envelope(
        "carrier-ws-add",
        AppCommand::WorkspaceAdd {
            root_path: root.to_string_lossy().into_owned(),
        },
    ));
    let AppResponse::Data(workspace_value) = workspace.response else {
        panic!("WorkspaceAdd 应成功: {:?}", workspace.response);
    };
    let workspace_id = WorkspaceId::from(
        workspace_value
            .get("id")
            .and_then(|value| value.as_str())
            .expect("workspace id"),
    );
    let created = app.dispatch_envelope(host_envelope(
        "carrier-session-create",
        AppCommand::SessionCreate {
            workspace_id: workspace_id.clone(),
            title: Some("carrier".into()),
        },
    ));
    let AppResponse::Data(session_value) = created.response else {
        panic!("SessionCreate 应成功: {:?}", created.response);
    };
    let session_id = SessionId::from(
        session_value
            .get("session_id")
            .and_then(|value| value.as_str())
            .expect("session id"),
    );

    (
        Carrier {
            _temp: temp,
            service,
            endpoint: handle.endpoint,
            connector,
            _listener: listener,
        },
        workspace_id,
        session_id,
    )
}

fn host_envelope(command_id: &str, command: AppCommand) -> AppCommandEnvelope {
    AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from(command_id.to_string()),
        source: CommandSource::Automation,
        identity: ActorIdentity::Automation {
            name: "carrier-bootstrap".into(),
        },
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(1),
        command,
    }
}

async fn connect(carrier: &Carrier) -> Box<dyn GuiConnection> {
    carrier
        .connector
        .connect(
            &carrier.endpoint,
            ConnectOptions {
                timeout_ms: 5_000,
                client_label: Some("carrier-test".into()),
                max_frame_bytes: 1024 * 1024,
            },
        )
        .await
        .expect("connect")
}

async fn send_frame(conn: &(impl GuiConnection + ?Sized), frame: ClientFrame) {
    let bytes = serde_json::to_vec(&frame).expect("encode client frame");
    conn.send(TransportFrame::new(bytes)).await.expect("send");
}

/// 接收帧直至命中谓词（跳过穿插的通知 / PushGap 帧）。
async fn recv_matching(
    conn: &(impl GuiConnection + ?Sized),
    mut matches: impl FnMut(&ServerFrame) -> bool,
) -> ServerFrame {
    let deadline = Instant::now() + MATCH_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(remaining > Duration::ZERO, "recv_matching timeout");
        let frame = timeout(remaining, conn.receive())
            .await
            .expect("recv timeout")
            .expect("receive frame");
        let frame: ServerFrame =
            serde_json::from_slice(frame.as_bytes()).expect("decode server frame");
        if matches(&frame) {
            return frame;
        }
    }
}

/// 请求/响应 RPC：发送帧后等待同 request_id 的响应（跳过通知类帧）。
async fn rpc(
    conn: &(impl GuiConnection + ?Sized),
    request_id: &str,
    frame: ClientFrame,
) -> ServerFrame {
    send_frame(conn, frame).await;
    let wanted = request_id.to_string();
    recv_matching(conn, move |frame| match frame {
        ServerFrame::PairChallenge { request_id, .. }
        | ServerFrame::Activated { request_id, .. }
        | ServerFrame::Authenticated { request_id, .. }
        | ServerFrame::Response { request_id, .. }
        | ServerFrame::Denied { request_id, .. }
        | ServerFrame::Replayed { request_id, .. } => *request_id == wanted,
        ServerFrame::Error {
            request_id: Some(request_id),
            ..
        } => *request_id == wanted,
        _ => false,
    })
    .await
}

async fn pair_and_activate(
    conn: &(impl GuiConnection + ?Sized),
    session_probe: &SessionId,
) -> (String, String, String) {
    let challenge = rpc(
        conn,
        "pair",
        ClientFrame::Pair {
            request_id: "pair".into(),
            device_label: "test phone".into(),
        },
    )
    .await;
    let pairing_code = match challenge {
        ServerFrame::PairChallenge {
            pairing_code,
            expires_in_ms,
            ..
        } => {
            assert!(expires_in_ms > 0, "配对挑战必须携带有效期");
            pairing_code
        }
        other => panic!("expected pair challenge, got {other:?}"),
    };
    let activated = rpc(
        conn,
        "activate",
        ClientFrame::Activate {
            request_id: "activate".into(),
            pairing_code: pairing_code.clone(),
        },
    )
    .await;
    let (device_id, credential) = match activated {
        ServerFrame::Activated {
            device_id,
            credential,
            ..
        } => (device_id, credential),
        other => panic!("expected activated, got {other:?}"),
    };
    // 认证后受限查询立即可用。
    let probe = rpc(
        conn,
        "probe",
        ClientFrame::Query {
            request_id: "probe".into(),
            query: RemoteQuery::SessionGet {
                session_id: session_probe.clone(),
            },
        },
    )
    .await;
    assert!(
        matches!(
            probe,
            ServerFrame::Response {
                response: AppResponse::Data(_),
                ..
            }
        ),
        "认证后 SessionGet 应返回 Data: {probe:?}"
    );
    (pairing_code, device_id, credential)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restricted_protocol_end_to_end_over_transport_remote_carrier() {
    let (carrier, _workspace_id, session_id) = setup("rc-carrier").await;
    let conn = connect(&carrier).await;

    // ---- 1. 未认证操作一律拒绝（不泄漏任何数据）。----
    let pre_auth_query = rpc(
        conn.as_ref(),
        "pre-q",
        ClientFrame::Query {
            request_id: "pre-q".into(),
            query: RemoteQuery::SessionGet {
                session_id: session_id.clone(),
            },
        },
    )
    .await;
    match pre_auth_query {
        ServerFrame::Error {
            request_id, code, ..
        } => {
            assert_eq!(request_id.as_deref(), Some("pre-q"));
            assert_eq!(code, "authentication_required");
        }
        other => panic!("expected authentication_required, got {other:?}"),
    }
    let pre_auth_command = rpc(
        conn.as_ref(),
        "pre-c",
        ClientFrame::Command {
            request_id: "pre-c".into(),
            command: RemoteCommand::RunStart {
                session_id: session_id.clone(),
                user_message: "should not run".into(),
                model: None,
                profile: None,
            },
        },
    )
    .await;
    match pre_auth_command {
        ServerFrame::Error { code, .. } => assert_eq!(code, "authentication_required"),
        other => panic!("expected authentication_required, got {other:?}"),
    }

    // ---- 2. 配对与激活。----
    let (pairing_code, device_id, credential) = pair_and_activate(conn.as_ref(), &session_id).await;

    // ---- 3. 受限查询：PlanStatus 显式可用性标记（不伪造计划）。----
    let plan_status = rpc(
        conn.as_ref(),
        "plan",
        ClientFrame::Query {
            request_id: "plan".into(),
            query: RemoteQuery::PlanStatus {
                session_id: session_id.clone(),
            },
        },
    )
    .await;
    match plan_status {
        ServerFrame::Response {
            response: AppResponse::Data(value),
            ..
        } => {
            assert_eq!(
                value.get("session_id").and_then(|v| v.as_str()),
                Some(session_id.as_str())
            );
            assert_eq!(value.get("plan"), Some(&serde_json::Value::Null));
            assert_eq!(
                value.get("plan_availability").and_then(|v| v.as_str()),
                Some("not_exposed_by_core")
            );
            assert!(value.get("session").is_some());
        }
        other => panic!("expected plan status data, got {other:?}"),
    }

    // ---- 4. 受限命令：RunStart -> Accepted{run_id: Some}。----
    let run_start = rpc(
        conn.as_ref(),
        "run",
        ClientFrame::Command {
            request_id: "run".into(),
            command: RemoteCommand::RunStart {
                session_id: session_id.clone(),
                user_message: "hello from remote".into(),
                model: None,
                profile: None,
            },
        },
    )
    .await;
    let run_id: RunId = match run_start {
        ServerFrame::Response {
            response:
                AppResponse::Accepted {
                    run_id: Some(run_id),
                    ..
                },
            ..
        } => run_id,
        other => panic!("expected accepted with run_id, got {other:?}"),
    };
    let run_status = rpc(
        conn.as_ref(),
        "status",
        ClientFrame::Query {
            request_id: "status".into(),
            query: RemoteQuery::RunStatus {
                run_id: run_id.clone(),
            },
        },
    )
    .await;
    assert!(
        matches!(
            run_status,
            ServerFrame::Response {
                response: AppResponse::Data(_),
                ..
            }
        ),
        "RunStatus 应返回 Data: {run_status:?}"
    );

    // ---- 5. 通知推送：Run 终态映射为 RunFinished(Completed)。----
    let expected_run = run_id.clone();
    let finished = recv_matching(conn.as_ref(), |frame| match frame {
        ServerFrame::Notification {
            payload:
                NotificationPayload::RunFinished {
                    run_id,
                    state: RunState::Completed,
                },
            ..
        } => *run_id == expected_run,
        _ => false,
    })
    .await;
    match finished {
        ServerFrame::Notification { seq, payload, .. } => {
            assert!(seq >= 1);
            assert!(matches!(
                payload,
                NotificationPayload::RunFinished {
                    state: RunState::Completed,
                    ..
                }
            ));
        }
        _ => unreachable!(),
    }

    // ---- 6. Full 透传位：显式拒绝 + 稳定拒绝码。----
    let denied_workspace = WorkspaceId::from("ws-denied");
    let denied_cases: Vec<(String, ClientFrame, &str, &str)> = vec![
        (
            "deny-run-tool".into(),
            ClientFrame::Command {
                request_id: "deny-run-tool".into(),
                command: RemoteCommand::Full {
                    command: AppCommand::RunTool {
                        run_id: run_id.clone(),
                        tool_name: "shell".into(),
                        input: serde_json::json!({"cmd": "ls"}),
                    },
                },
            },
            DENY_TOOL_EXECUTION,
            "run_tool",
        ),
        (
            "deny-auth-start".into(),
            ClientFrame::Command {
                request_id: "deny-auth-start".into(),
                command: RemoteCommand::Full {
                    command: AppCommand::AuthStart {
                        provider_id: agent_domain::ProviderId::from("mock"),
                        flow: "api_key".into(),
                    },
                },
            },
            DENY_PROVIDER_DIRECT_ACCESS,
            "auth_start",
        ),
        (
            "deny-terminal".into(),
            ClientFrame::Command {
                request_id: "deny-terminal".into(),
                command: RemoteCommand::Full {
                    command: AppCommand::TerminalCreate {
                        workspace_id: denied_workspace.clone(),
                        working_directory: None,
                    },
                },
            },
            DENY_FILE_WRITE,
            "terminal_create",
        ),
        (
            "deny-git-stage".into(),
            ClientFrame::Command {
                request_id: "deny-git-stage".into(),
                command: RemoteCommand::Full {
                    command: AppCommand::GitStage {
                        workspace_id: denied_workspace.clone(),
                        paths: Vec::new(),
                    },
                },
            },
            DENY_FILE_WRITE,
            "git_stage",
        ),
        (
            "deny-session-create".into(),
            ClientFrame::Command {
                request_id: "deny-session-create".into(),
                command: RemoteCommand::Full {
                    command: AppCommand::SessionCreate {
                        workspace_id: denied_workspace.clone(),
                        title: None,
                    },
                },
            },
            DENY_SESSION_MUTATION,
            "session_create",
        ),
        (
            "deny-run-retry".into(),
            ClientFrame::Command {
                request_id: "deny-run-retry".into(),
                command: RemoteCommand::Full {
                    command: AppCommand::RunRetry {
                        run_id: run_id.clone(),
                    },
                },
            },
            DENY_NOT_EXPOSED,
            "run_retry",
        ),
        (
            "deny-snapshot".into(),
            ClientFrame::Query {
                request_id: "deny-snapshot".into(),
                query: RemoteQuery::Full {
                    query: AppQuery::SnapshotFetch,
                },
            },
            DENY_CONTENT_READ,
            "snapshot_fetch",
        ),
        (
            "deny-model-list".into(),
            ClientFrame::Query {
                request_id: "deny-model-list".into(),
                query: RemoteQuery::Full {
                    query: AppQuery::ModelList { provider_id: None },
                },
            },
            DENY_PROVIDER_DIRECT_ACCESS,
            "model_list",
        ),
    ];
    for (request_id, frame, expected_code, expected_operation) in denied_cases {
        let response = rpc(conn.as_ref(), &request_id, frame).await;
        match response {
            ServerFrame::Denied {
                request_id: denied_id,
                code,
                reason,
                operation,
            } => {
                assert_eq!(denied_id, request_id);
                assert_eq!(code, expected_code, "operation {expected_operation}");
                assert_eq!(operation, expected_operation);
                assert!(!reason.is_empty());
            }
            other => panic!("expected denied for {expected_operation}, got {other:?}"),
        }
    }

    // ToolApprove 属于允许集（run 已结束：Core 返回结构化响应而非门禁拒绝）。
    let tool_approve = rpc(
        conn.as_ref(),
        "approve",
        ClientFrame::Command {
            request_id: "approve".into(),
            command: RemoteCommand::ToolApprove {
                run_id: run_id.clone(),
                tool_call_id: agent_domain::ToolCallId::from("tc-1"),
                decision: ApprovalDecision::ApproveOnce,
            },
        },
    )
    .await;
    assert!(
        matches!(tool_approve, ServerFrame::Response { .. }),
        "ToolApprove 属于允许集，应转发到 Core: {tool_approve:?}"
    );

    // ---- 7. Replay：按序、含此前推送过的通知。----
    let replayed = rpc(
        conn.as_ref(),
        "replay",
        ClientFrame::Replay {
            request_id: "replay".into(),
            from_seq: 1,
        },
    )
    .await;
    match replayed {
        ServerFrame::Replayed { notifications, .. } => {
            assert!(!notifications.is_empty(), "replay 应至少含 RunFinished");
            let mut previous = 0u64;
            for notification in &notifications {
                assert!(notification.seq > previous, "replay 必须按序");
                previous = notification.seq;
            }
            assert_eq!(notifications.first().expect("first").seq, 1);
        }
        other => panic!("expected replayed, got {other:?}"),
    }

    // ---- 8. revoke：下一帧显式告知并关闭连接。----
    assert!(carrier.service.revoke_device(&device_id), "revoke 应生效");
    send_frame(
        conn.as_ref(),
        ClientFrame::Query {
            request_id: "post-revoke".into(),
            query: RemoteQuery::SessionGet {
                session_id: session_id.clone(),
            },
        },
    )
    .await;
    let revoked = recv_matching(conn.as_ref(), |frame| {
        matches!(frame, ServerFrame::Revoked { .. })
    })
    .await;
    match revoked {
        ServerFrame::Revoked {
            device_id: revoked_id,
            reason,
        } => {
            assert_eq!(revoked_id, device_id);
            assert!(!reason.is_empty());
        }
        _ => unreachable!(),
    }
    let closed = timeout(IO_TIMEOUT, conn.receive())
        .await
        .expect("close timeout");
    assert!(closed.is_err(), "revoke 后连接应被关闭");

    // 已吊销设备无法再认证。
    let conn2 = connect(&carrier).await;
    let re_auth = rpc(
        conn2.as_ref(),
        "re-auth",
        ClientFrame::Authenticate {
            request_id: "re-auth".into(),
            device_id: device_id.clone(),
            credential: credential.clone(),
        },
    )
    .await;
    match re_auth {
        ServerFrame::Error { code, .. } => assert_eq!(code, "authentication_failed"),
        other => panic!("revoked device must not authenticate, got {other:?}"),
    }

    // ---- 9. 审计与 Secret 卫生。----
    let entries = carrier.service.audit().entries();
    let has_event =
        |wanted: &dyn Fn(&AuditEvent) -> bool| entries.iter().any(|record| wanted(&record.event));
    assert!(has_event(&|event| matches!(
        event,
        AuditEvent::OperationDenied { code, operation }
            if code == DENY_TOOL_EXECUTION && operation == "run_tool"
    )));
    assert!(has_event(&|event| matches!(
        event,
        AuditEvent::OperationDenied { code, operation }
            if code == DENY_FILE_WRITE && operation == "terminal_create"
    )));
    assert!(has_event(&|event| matches!(
        event,
        AuditEvent::OperationDenied { code, operation }
            if code == DENY_CONTENT_READ && operation == "snapshot_fetch"
    )));
    assert!(has_event(&|event| matches!(
        event,
        AuditEvent::CommandDispatched { operation, .. } if operation == "run_start"
    )));
    assert!(has_event(&|event| matches!(
        event,
        AuditEvent::QueryDispatched { operation, .. } if operation == "plan_status"
    )));
    assert!(has_event(&|event| matches!(
        event,
        AuditEvent::DeviceRevoked { device_id: revoked, .. } if *revoked == device_id
    )));
    assert!(has_event(&|event| matches!(
        event,
        AuditEvent::AuthenticationRequired { operation } if operation == "session_get"
    )));
    assert!(has_event(&|event| matches!(
        event,
        AuditEvent::PairingCodeIssued { .. }
    )));

    // Secret 不出现在 Debug / 审计输出中。
    let registry_debug = format!("{:?}", carrier.service.pairing());
    assert!(!registry_debug.contains(&pairing_code));
    assert!(!registry_debug.contains(&credential));
    let audit_debug = format!("{entries:?}");
    assert!(!audit_debug.contains(&pairing_code));
    assert!(!audit_debug.contains(&credential));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stored_credential_reauthenticates_on_new_connection() {
    let (carrier, _workspace_id, session_id) = setup("rc-carrier-reauth").await;
    let conn1 = connect(&carrier).await;
    let (_pairing_code, device_id, credential) =
        pair_and_activate(conn1.as_ref(), &session_id).await;
    conn1.close().await.expect("client close");

    let conn2 = connect(&carrier).await;
    // 错误凭证：结构化失败（不回显 Secret 细节）。
    let bad_auth = rpc(
        conn2.as_ref(),
        "bad",
        ClientFrame::Authenticate {
            request_id: "bad".into(),
            device_id: device_id.clone(),
            credential: "not-the-credential".into(),
        },
    )
    .await;
    match bad_auth {
        ServerFrame::Error { code, .. } => assert_eq!(code, "authentication_failed"),
        other => panic!("expected authentication_failed, got {other:?}"),
    }
    // 正确凭证：认证成功并可查询。
    let good_auth = rpc(
        conn2.as_ref(),
        "good",
        ClientFrame::Authenticate {
            request_id: "good".into(),
            device_id: device_id.clone(),
            credential: credential.clone(),
        },
    )
    .await;
    match good_auth {
        ServerFrame::Authenticated {
            device_id: authenticated,
            ..
        } => assert_eq!(authenticated, device_id),
        other => panic!("expected authenticated, got {other:?}"),
    }
    let probe = rpc(
        conn2.as_ref(),
        "probe2",
        ClientFrame::Query {
            request_id: "probe2".into(),
            query: RemoteQuery::SessionGet {
                session_id: session_id.clone(),
            },
        },
    )
    .await;
    assert!(matches!(
        probe,
        ServerFrame::Response {
            response: AppResponse::Data(_),
            ..
        }
    ));
    // 认证成功记录落审计。
    let authenticated_again = carrier.service.audit().entries().iter().any(|record| {
        matches!(
            &record.event,
            AuditEvent::DeviceAuthenticated { device_id: audited } if audited == &device_id
        )
    });
    assert!(authenticated_again);
}
