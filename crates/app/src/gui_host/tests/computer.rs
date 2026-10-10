//! CU-17：观测证据查询与 computer-use 目标接入的定向回归。

use super::*;
use pawork_domain::{EventId, EventSequence, ToolCallId, ToolResultContent};
use pawork_protocol::{
    ComputerAppFamily, ComputerAppIdentity, ComputerGrantKind, ComputerTargetIdentity,
    MAX_COMPUTER_TARGET_LIST_TARGETS, MAX_OBSERVATION_LIST_RESPONSE_BYTES,
    MAX_OBSERVATION_READ_BYTES, V1_26,
};

/// 构造带图像 part 的 ToolExecutionCompleted 事件并落库。
async fn seed_observation(
    adapter: &GuiHostAdapter,
    session: &SessionId,
    run_id: &str,
    sequence: u64,
    parent: Option<EventId>,
    image_bytes: &[u8],
    image_count: usize,
) -> EventId {
    let call_event = EventId::from(format!("evt-call-{sequence}"));
    // projection 要求 completion 引用已知 tool call：先落 ToolCallStarted。
    append(
        adapter,
        session,
        run_id,
        sequence,
        parent,
        call_event.clone(),
        AgentEvent::ToolCallStarted {
            tool_call_id: ToolCallId::from(format!("call-{sequence}")),
            name: "computer".into(),
        },
    )
    .await;
    let payload = AgentEvent::ToolExecutionCompleted {
        tool_call_id: ToolCallId::from(format!("call-{sequence}")),
        result: ToolResultContent {
            tool_call_id: ToolCallId::from(format!("call-{sequence}")),
            tool_name: Some("computer".into()),
            content: std::iter::once(pawork_domain::ContentPart::Text(
                pawork_domain::TextContent {
                    text: "captured".into(),
                },
            ))
            .chain((0..image_count).map(|_| {
                pawork_domain::ContentPart::Image(pawork_domain::ImageContent {
                    source: pawork_domain::ImageSource::Base64(
                        crate::services::extension::base64_encode(image_bytes),
                    ),
                    media_type: "image/png".into(),
                    alt_text: None,
                })
            }))
            .collect(),
            is_error: false,
            metadata: serde_json::json!({
                "observation": {"image_width": 1280, "image_height": 720}
            }),
            artifacts: Vec::new(),
        },
    };
    append(
        adapter,
        session,
        run_id,
        sequence + 1,
        Some(call_event),
        EventId::from(format!("evt-obs-{}", sequence + 1)),
        payload,
    )
    .await;
    EventId::from(format!("evt-obs-{}", sequence + 1))
}

async fn append(
    adapter: &GuiHostAdapter,
    session: &SessionId,
    run_id: &str,
    sequence: u64,
    parent: Option<EventId>,
    event_id: EventId,
    payload: AgentEvent,
) {
    let mut envelope = AgentEventEnvelope::new(
        event_id,
        session.clone(),
        RunId::from(run_id),
        EventSequence::new(sequence),
        Timestamp::from_unix_millis(1_700_000_000 + sequence),
        payload,
    );
    if let Some(parent) = parent {
        envelope = envelope.with_parent(parent);
    }
    let core = adapter.core.read().await;
    let store = core.store().expect("store");
    // 写入当前 active branch，fork 切换后的事件才会落在支线上。
    let branch = store
        .get_session(session)
        .await
        .expect("session")
        .active_branch;
    store
        .append_event(branch, envelope)
        .await
        .expect("append event");
}

/// 无图像事件（parent 链起点）。
async fn seed_text_event(adapter: &GuiHostAdapter, session: &SessionId, sequence: u64) -> EventId {
    let event_id = EventId::from(format!("evt-text-{sequence}"));
    let envelope = AgentEventEnvelope::new(
        event_id.clone(),
        session.clone(),
        RunId::from("run-1"),
        EventSequence::new(sequence),
        Timestamp::from_unix_millis(1_700_000_000 + sequence),
        AgentEvent::MessageCommitted {
            message: Message {
                id: MessageId::from(format!("msg-{sequence}")),
                role: MessageRole::User,
                content: vec![ContentPart::Text(TextContent {
                    text: "seed".into(),
                })],
                metadata: MessageMetadata::default(),
            },
        },
    );
    adapter
        .core
        .read()
        .await
        .store()
        .expect("store")
        .append_event(pawork_storage::session::DEFAULT_BRANCH_ID, envelope)
        .await
        .expect("append event");
    event_id
}

async fn adapter_with_session() -> (GuiHostAdapter, tempfile::TempDir, SessionId) {
    let (core, dir, _unbound) = core_with_turn().await;
    let adapter = GuiHostAdapter::new(core);
    let session = {
        let core = adapter.core.read().await;
        core.session
            .create_session_with_workspace(&core, "cu17", WorkspaceId::from("ws-cu17"))
            .await
            .expect("workspace-bound session")
    };
    (adapter, dir, session)
}

async fn adapter_other_session(adapter: &GuiHostAdapter) -> SessionId {
    let core = adapter.core.read().await;
    core.create_session("other").await.expect("session")
}

fn observation_query(
    session: &SessionId,
    sequence: u64,
    part_index: u32,
    offset: u64,
    limit: u64,
) -> AppQuery {
    AppQuery::ObservationImageRead {
        session_id: session.clone(),
        sequence,
        part_index,
        offset,
        limit,
    }
}

#[tokio::test]
async fn observation_list_reports_entries_and_chunked_reads_reassemble() {
    let (adapter, _dir, session) = adapter_with_session().await;
    let first = seed_text_event(&adapter, &session, 1).await;
    let image: &[u8] = &[11, 22, 33, 44, 55];
    seed_observation(&adapter, &session, "run-1", 2, Some(first), image, 1).await;

    let response = adapter
        .query(&query_envelope(AppQuery::ObservationList {
            session_id: session.clone(),
            after_sequence: None,
            limit: None,
        }))
        .await
        .expect("observation list");
    let AppResponse::Data(data) = response else {
        panic!("observation list returns data");
    };
    let entries = data["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1, "only the image part is listed: {data}");
    let entry = &entries[0];
    assert_eq!(entry["sequence"], json!(3));
    assert_eq!(entry["event_id"], "evt-obs-3");
    assert_eq!(entry["run_id"], "run-1");
    assert_eq!(entry["tool_call_id"], "call-2");
    assert_eq!(entry["tool_name"], "computer");
    assert_eq!(entry["part_index"], json!(1));
    assert_eq!(entry["media_type"], "image/png");
    assert_eq!(entry["byte_length"], json!(5));
    assert_eq!(entry["image_width"], json!(1280));
    assert_eq!(entry["image_height"], json!(720));
    assert_eq!(entry["captured_at_ms"], json!(1_700_000_003));
    assert_eq!(entry["status"], "historical");
    assert_eq!(data["complete"], json!(true), "single page is complete");

    // 两次分块读（3 + 2 字节）重组原图像；读取是纯持久化查询，
    // 不触发任何 provider / 原生后端调用。
    let first_chunk = adapter
        .query(&query_envelope(observation_query(&session, 3, 1, 0, 3)))
        .await
        .expect("first chunk");
    let AppResponse::Data(first) = first_chunk else {
        panic!("chunk read returns data");
    };
    assert_eq!(first["offset"], json!(0));
    assert_eq!(first["eof"], json!(false));
    let second_chunk = adapter
        .query(&query_envelope(observation_query(&session, 3, 1, 3, 3)))
        .await
        .expect("second chunk");
    let AppResponse::Data(second) = second_chunk else {
        panic!("chunk read returns data");
    };
    assert_eq!(second["offset"], json!(3));
    assert_eq!(second["eof"], json!(true), "clamped to byte_length");
    let reassembled = [
        first["data"].as_str().expect("base64"),
        second["data"].as_str().expect("base64"),
    ]
    .concat();
    assert_eq!(
        crate::services::extension::base64_decode(&reassembled).expect("decode"),
        image
    );
}

#[tokio::test]
async fn observation_failures_fail_closed() {
    let (adapter, _dir, session) = adapter_with_session().await;
    let other = adapter_other_session(&adapter).await;
    let first = seed_text_event(&adapter, &session, 1).await;
    seed_observation(&adapter, &session, "run-1", 2, Some(first), &[1, 2, 3], 1).await;

    async fn expect_code(adapter: &GuiHostAdapter, query: AppQuery, code: &str) {
        let error = adapter
            .query(&query_envelope(query))
            .await
            .expect_err("rejected");
        assert_eq!(error.code, code, "{error:?}");
    }
    // 跨 session：他 session 的同号事件不在本 lineage。
    expect_code(&adapter, observation_query(&other, 3, 1, 0, 3), "not_found").await;
    // part_index 越界。
    expect_code(
        &adapter,
        observation_query(&session, 3, 9, 0, 3),
        "not_found",
    )
    .await;
    // limit 0 / 超限。
    expect_code(
        &adapter,
        observation_query(&session, 3, 1, 0, 0),
        "invalid_query",
    )
    .await;
    expect_code(
        &adapter,
        observation_query(&session, 3, 1, 0, MAX_OBSERVATION_READ_BYTES + 1),
        "invalid_query",
    )
    .await;
    // offset 越界（>= byte_length）。
    expect_code(
        &adapter,
        observation_query(&session, 3, 1, 3, 1),
        "invalid_query",
    )
    .await;
    // 旧 minor 发网前拒绝。
    let mut envelope = query_envelope(AppQuery::ObservationList {
        session_id: session.clone(),
        after_sequence: None,
        limit: None,
    });
    envelope.api_version = V1_26;
    let error = adapter.query(&envelope).await.expect_err("1.26 rejected");
    assert_eq!(error.code, "unsupported", "{error:?}");
    // 其余四个 1.27 入口走各自 handler 的版本闸，旧 minor 同样在发网前拒绝。
    let mut image = query_envelope(observation_query(&session, 3, 1, 0, 3));
    image.api_version = V1_26;
    let error = adapter
        .query(&image)
        .await
        .expect_err("1.26 image read rejected");
    assert_eq!(error.code, "unsupported", "{error:?}");
    let mut targets = query_envelope(AppQuery::ComputerTargetList {
        session_id: session.clone(),
    });
    targets.api_version = V1_26;
    let error = adapter
        .query(&targets)
        .await
        .expect_err("1.26 target list rejected");
    assert_eq!(error.code, "unsupported", "{error:?}");
    let mut approve = command_envelope(AppCommand::ComputerTargetApprove(
        pawork_protocol::ComputerTargetApproveParams {
            session_id: session.clone(),
            run_id: RunId::from("run-old"),
            target: application_target("com.example.App"),
            kind: ComputerGrantKind::ForRun,
        },
    ));
    approve.api_version = V1_26;
    let error = adapter
        .command(&approve)
        .await
        .expect_err("1.26 approve rejected");
    assert_eq!(error.code, "unsupported", "{error:?}");
    let mut revoke = command_envelope(AppCommand::ComputerTargetRevoke(
        pawork_protocol::ComputerTargetRevokeParams {
            session_id: session.clone(),
            run_id: RunId::from("run-old"),
            target: application_target("com.example.App"),
        },
    ));
    revoke.api_version = V1_26;
    let error = adapter
        .command(&revoke)
        .await
        .expect_err("1.26 revoke rejected");
    assert_eq!(error.code, "unsupported", "{error:?}");
}

fn application_target(bundle: &str) -> ComputerTargetIdentity {
    ComputerTargetIdentity::Application {
        app: ComputerAppIdentity {
            bundle_id: bundle.into(),
            family: ComputerAppFamily::Appkit,
        },
    }
}

#[tokio::test]
async fn computer_targets_approve_revoke_and_run_lifecycle() {
    let (adapter, _dir, session) = adapter_with_session().await;
    let run = RunId::from("run-cu17");
    adapter
        .core
        .read()
        .await
        .open_target_scope(&pawork_computer_use::target::Scope::new(
            "ws-cu17", "run-cu17",
        ));
    adapter.runs.register(
        ActiveGuiRun {
            run_id: run.clone(),
            session_id: session.clone(),
            workspace_id: WorkspaceId::from("ws-cu17"),
            workspace_roots: Vec::new(),
            started_at_ms: 1,
        },
        CancellationToken::new(),
    );

    // 未知 run：fail-closed unknown_run。
    let unknown = command_envelope(AppCommand::ComputerTargetApprove(
        pawork_protocol::ComputerTargetApproveParams {
            session_id: session.clone(),
            run_id: RunId::from("run-missing"),
            target: application_target("com.example.App"),
            kind: ComputerGrantKind::ForRun,
        },
    ));
    let error = adapter.command(&unknown).await.expect_err("unknown run");
    assert_eq!(error.code, "unknown_run", "{error:?}");
    // protected bundle：永不授权。
    let protected = command_envelope(AppCommand::ComputerTargetApprove(
        pawork_protocol::ComputerTargetApproveParams {
            session_id: session.clone(),
            run_id: run.clone(),
            target: application_target("dev.pawork.desktop"),
            kind: ComputerGrantKind::ForRun,
        },
    ));
    let error = adapter.command(&protected).await.expect_err("protected");
    assert_eq!(error.code, "forbidden_target", "{error:?}");

    // 无归属会话：空表 + workspace_id null。
    let response = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: SessionId::from("session-none"),
        }))
        .await
        .expect("empty list");
    let AppResponse::Data(data) = response else {
        panic!("target list returns data");
    };
    assert_eq!(data["targets"], json!([]));
    assert!(data.get("workspace_id").is_none());

    // 正常授权：回执回显生效 target 与寿命；列表出现条目。
    let approve = command_envelope(AppCommand::ComputerTargetApprove(
        pawork_protocol::ComputerTargetApproveParams {
            session_id: session.clone(),
            run_id: run.clone(),
            target: application_target("com.example.App"),
            kind: ComputerGrantKind::ForRun,
        },
    ));
    let response = adapter.command(&approve).await.expect("approve");
    let AppResponse::Data(receipt) = response else {
        panic!("approve returns receipt");
    };
    assert_eq!(receipt["kind"], "for_run");
    assert_eq!(receipt["target"]["kind"], "application");

    let response = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: session.clone(),
        }))
        .await
        .expect("target list");
    let AppResponse::Data(data) = response else {
        panic!("target list returns data");
    };
    assert_eq!(data["workspace_id"], "ws-cu17");
    let targets = data["targets"].as_array().expect("targets");
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    assert_eq!(target["grant"], "for_run");
    assert_eq!(target["scope"]["run_id"], "run-cu17");
    assert_eq!(target["scope"]["workspace_id"], "ws-cu17");
    assert_eq!(target["dispatch_paused"], json!(false));
    let capabilities = target["capabilities"].as_array().expect("capabilities");
    assert_eq!(capabilities.len(), 10, "CU-02 ten-action matrix");

    // 幂等重授收敛；撤销后条目整体消失（无绑定无授权）。
    adapter
        .command(&approve)
        .await
        .expect("re-approve idempotent");
    let revoke = command_envelope(AppCommand::ComputerTargetRevoke(
        pawork_protocol::ComputerTargetRevokeParams {
            session_id: session.clone(),
            run_id: run.clone(),
            target: application_target("com.example.App"),
        },
    ));
    let response = adapter.command(&revoke).await.expect("revoke");
    assert!(matches!(response, AppResponse::Data(_)));
    let response = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: session.clone(),
        }))
        .await
        .expect("target list after revoke");
    let AppResponse::Data(data) = response else {
        panic!("target list returns data");
    };
    assert_eq!(data["targets"], json!([]), "{data}");

    // 终态关闭竞态窗口（审查修复 1）：close 发生在 run 仍登记时，
    // 模拟 chat_turn 终态块先于 runs.remove 的窗口——late approve 必须
    // 按 unknown_run 拒绝，不能重新授权（每次新 envelope，避开幂等重放
    // 掩盖真实行为）。
    adapter
        .command(&command_envelope(AppCommand::ComputerTargetApprove(
            pawork_protocol::ComputerTargetApproveParams {
                session_id: session.clone(),
                run_id: run.clone(),
                target: application_target("com.example.App"),
                kind: ComputerGrantKind::ForRun,
            },
        )))
        .await
        .expect("approve before terminal close");
    adapter
        .core
        .read()
        .await
        .close_target_scope(&pawork_computer_use::target::Scope::new(
            "ws-cu17", "run-cu17",
        ));
    let error = adapter
        .command(&command_envelope(AppCommand::ComputerTargetApprove(
            pawork_protocol::ComputerTargetApproveParams {
                session_id: session.clone(),
                run_id: run.clone(),
                target: application_target("com.example.App"),
                kind: ComputerGrantKind::ForRun,
            },
        )))
        .await
        .expect_err("late approve rejected");
    assert_eq!(error.code, "unknown_run", "{error:?}");
    adapter.runs.remove(&run);
    let response = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: session.clone(),
        }))
        .await
        .expect("target list after run end");
    let AppResponse::Data(data) = response else {
        panic!("target list returns data");
    };
    assert_eq!(data["targets"], json!([]));
    let error = adapter
        .command(&command_envelope(AppCommand::ComputerTargetRevoke(
            pawork_protocol::ComputerTargetRevokeParams {
                session_id: session.clone(),
                run_id: run.clone(),
                target: application_target("com.example.App"),
            },
        )))
        .await
        .expect_err("revoke after run end");
    assert_eq!(error.code, "unknown_run", "{error:?}");
}

/// CLI 与子 run 不进 GUI 注册表。投影状态恰为 running、且同一 host 上的
/// scope 仍开着时，approve / list / revoke 才看得到。父会话的授权不会
/// 出现在另一个会话里。
#[tokio::test]
async fn computer_targets_follow_projection_runs_without_gui_registry() {
    use pawork_domain::{MessageId, StopReason, TokenUsage};

    let (adapter, _dir, session) = adapter_with_session().await;
    let run = "run-cli";
    append(
        &adapter,
        &session,
        run,
        1,
        None,
        EventId::from("evt-cli-start"),
        AgentEvent::RunStarted {
            trigger_message_id: MessageId::from("msg-cli"),
        },
    )
    .await;
    let wrong = pawork_computer_use::target::Scope::new("ws-unbound", run);
    adapter.core.read().await.open_target_scope(&wrong);
    let approve = |run_id: &str, session: &SessionId| {
        command_envelope(AppCommand::ComputerTargetApprove(
            pawork_protocol::ComputerTargetApproveParams {
                session_id: session.clone(),
                run_id: RunId::from(run_id),
                target: application_target("com.example.App"),
                kind: ComputerGrantKind::ForRun,
            },
        ))
    };
    let error = adapter
        .command(&approve(run, &session))
        .await
        .expect_err("bound session cannot use ws-unbound");
    assert_eq!(error.code, "unknown_run", "{error:?}");
    adapter.core.read().await.close_target_scope(&wrong);

    let scope = pawork_computer_use::target::Scope::new("ws-cu17", run);
    adapter.core.read().await.open_target_scope(&scope);
    adapter
        .command(&approve(run, &session))
        .await
        .expect("projection run approves without a gui registry entry");
    let response = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: session.clone(),
        }))
        .await
        .expect("list");
    let AppResponse::Data(data) = response else {
        panic!("target list returns data");
    };
    assert_eq!(data["workspace_id"], "ws-cu17");
    assert_eq!(data["targets"].as_array().expect("targets").len(), 1);

    let child = adapter
        .core
        .read()
        .await
        .create_session("child")
        .await
        .expect("child");
    let error = adapter
        .command(&approve(run, &child))
        .await
        .expect_err("parent run is not the child's run");
    assert_eq!(error.code, "unknown_run", "{error:?}");
    let error = adapter
        .command(&command_envelope(AppCommand::ComputerTargetRevoke(
            pawork_protocol::ComputerTargetRevokeParams {
                session_id: child.clone(),
                run_id: RunId::from(run),
                target: application_target("com.example.App"),
            },
        )))
        .await
        .expect_err("parent run revoke is not the child's run");
    assert_eq!(error.code, "unknown_run", "{error:?}");
    let response = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: child.clone(),
        }))
        .await
        .expect("child list");
    let AppResponse::Data(child_data) = response else {
        panic!("child list returns data");
    };
    assert_eq!(child_data["targets"], json!([]), "{child_data}");

    let unbound_run = "run-unbound";
    append(
        &adapter,
        &child,
        unbound_run,
        1,
        None,
        EventId::from("evt-child-start"),
        AgentEvent::RunStarted {
            trigger_message_id: MessageId::from("msg-child"),
        },
    )
    .await;
    let child_scope = pawork_computer_use::target::Scope::new("ws-unbound", unbound_run);
    adapter.core.read().await.open_target_scope(&child_scope);
    adapter
        .command(&approve(unbound_run, &child))
        .await
        .expect("unbound session uses ws-unbound");
    let response = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: session.clone(),
        }))
        .await
        .expect("parent list after child grant");
    let AppResponse::Data(data) = response else {
        panic!("parent list returns data");
    };
    let parent_targets = data["targets"].as_array().expect("targets");
    assert_eq!(parent_targets.len(), 1, "{data}");
    assert_eq!(parent_targets[0]["scope"]["run_id"], run);
    let response = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: child.clone(),
        }))
        .await
        .expect("child list after its own grant");
    let AppResponse::Data(child_data) = response else {
        panic!("child list returns data");
    };
    assert_eq!(child_data["workspace_id"], "ws-unbound");
    assert_eq!(child_data["targets"].as_array().expect("targets").len(), 1);
    assert_eq!(child_data["targets"][0]["scope"]["run_id"], unbound_run);

    adapter.core.read().await.close_target_scope(&scope);
    let error = adapter
        .command(&approve(run, &session))
        .await
        .expect_err("closed scope");
    assert_eq!(error.code, "unknown_run", "{error:?}");

    adapter.core.read().await.open_target_scope(&scope);
    append(
        &adapter,
        &session,
        run,
        2,
        Some(EventId::from("evt-cli-start")),
        EventId::from("evt-cli-done"),
        AgentEvent::RunCompleted {
            stop_reason: StopReason::Completed,
            usage: TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
        },
    )
    .await;
    let error = adapter
        .command(&approve(run, &session))
        .await
        .expect_err("completed projection stays unauthorized");
    assert_eq!(error.code, "unknown_run", "{error:?}");
}

/// 量出单条 entry 的真实序列化字节，按预算边界推导事件条目数——
/// 不硬编码 entry JSON 大小，预算行为随常量与形状自校准。
async fn measure_entry_bytes(adapter: &GuiHostAdapter, session: &SessionId) -> usize {
    let response = adapter
        .query(&query_envelope(AppQuery::ObservationList {
            session_id: session.clone(),
            after_sequence: None,
            limit: Some(10),
        }))
        .await
        .expect("probe list");
    let AppResponse::Data(data) = response else {
        panic!("observation list returns data");
    };
    serde_json::to_vec(&data["entries"][0])
        .expect("entry serializes")
        .len()
}

#[tokio::test]
async fn observation_list_budget_stops_at_event_boundary_and_resumes() {
    let (adapter, _dir, session) = adapter_with_session().await;
    let first = seed_text_event(&adapter, &session, 1).await;
    seed_observation(&adapter, &session, "run-1", 2, Some(first.clone()), &[9], 1).await;
    let entry_bytes = measure_entry_bytes(&adapter, &session).await;
    // 单事件贴近但不超过预算；两事件合计必然超预算 → 停在事件边界。
    // 余量吸收 part_index 位数随条数增长带来的每条字节差。
    let per_event = MAX_OBSERVATION_LIST_RESPONSE_BYTES / entry_bytes - 64;
    seed_observation(
        &adapter,
        &session,
        "run-1",
        4,
        Some(first.clone()),
        &[7],
        per_event,
    )
    .await;
    seed_observation(&adapter, &session, "run-1", 6, Some(first), &[7], per_event).await;

    let response = adapter
        .query(&query_envelope(AppQuery::ObservationList {
            session_id: session.clone(),
            after_sequence: None,
            limit: None,
        }))
        .await
        .expect("first page");
    let AppResponse::Data(page) = response else {
        panic!("observation list returns data");
    };
    let entries = page["entries"].as_array().expect("entries");
    assert_eq!(
        entries.len(),
        per_event + 1,
        "first big event + probe entry"
    );
    assert_eq!(page["complete"], json!(false), "budget stops the page");
    let next = page["next_sequence"].as_u64().expect("cursor");

    // 续读从游标后的下一事件开始：第二个大事件单独在预算内，整页完成。
    let response = adapter
        .query(&query_envelope(AppQuery::ObservationList {
            session_id: session.clone(),
            after_sequence: Some(next),
            limit: None,
        }))
        .await
        .expect("second page");
    let AppResponse::Data(page) = response else {
        panic!("observation list returns data");
    };
    let entries = page["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), per_event, "second big event complete");
    assert_eq!(page["complete"], json!(true), "{page}");
    assert!(page.get("next_sequence").is_none(), "{page}");
}

#[tokio::test]
async fn observation_list_single_oversized_event_fails_closed() {
    let (adapter, _dir, session) = adapter_with_session().await;
    let first = seed_text_event(&adapter, &session, 1).await;
    seed_observation(&adapter, &session, "run-1", 2, Some(first.clone()), &[9], 1).await;
    let entry_bytes = measure_entry_bytes(&adapter, &session).await;
    let oversized = MAX_OBSERVATION_LIST_RESPONSE_BYTES / entry_bytes + 10;
    seed_observation(&adapter, &session, "run-1", 4, Some(first), &[7], oversized).await;

    // 页内首个条目事件单独超预算：显式拒绝，不静默跳过或截断。
    let error = adapter
        .query(&query_envelope(AppQuery::ObservationList {
            session_id: session.clone(),
            after_sequence: Some(4),
            limit: None,
        }))
        .await
        .expect_err("oversized event rejected");
    assert_eq!(error.code, "response_too_large", "{error:?}");
}

#[tokio::test]
async fn computer_target_list_rejects_beyond_cap() {
    let (adapter, _dir, session) = adapter_with_session().await;
    let run = RunId::from("run-cu17-cap");
    adapter
        .core
        .read()
        .await
        .open_target_scope(&pawork_computer_use::target::Scope::new(
            "ws-cu17",
            "run-cu17-cap",
        ));
    adapter.runs.register(
        ActiveGuiRun {
            run_id: run.clone(),
            session_id: session.clone(),
            workspace_id: WorkspaceId::from("ws-cu17"),
            workspace_roots: Vec::new(),
            started_at_ms: 1,
        },
        CancellationToken::new(),
    );
    for index in 0..=MAX_COMPUTER_TARGET_LIST_TARGETS {
        adapter
            .command(&command_envelope(AppCommand::ComputerTargetApprove(
                pawork_protocol::ComputerTargetApproveParams {
                    session_id: session.clone(),
                    run_id: run.clone(),
                    target: application_target(&format!("com.example.cap-{index}")),
                    kind: ComputerGrantKind::ForRun,
                },
            )))
            .await
            .expect("approve");
    }
    let error = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: session.clone(),
        }))
        .await
        .expect_err("cap exceeded");
    assert_eq!(error.code, "response_too_large", "{error:?}");
}

#[tokio::test]
async fn computer_target_list_rejects_oversized_response_bytes() {
    let (adapter, _dir, session) = adapter_with_session().await;
    let run = RunId::from("run-cu17-bytes");
    adapter
        .core
        .read()
        .await
        .open_target_scope(&pawork_computer_use::target::Scope::new(
            "ws-cu17",
            "run-cu17-bytes",
        ));
    adapter.runs.register(
        ActiveGuiRun {
            run_id: run.clone(),
            session_id: session.clone(),
            workspace_id: WorkspaceId::from("ws-cu17"),
            workspace_roots: Vec::new(),
            started_at_ms: 1,
        },
        CancellationToken::new(),
    );
    // 单条超长 bundle_id：条目数在限内，但序列化响应超字节预算。
    let huge_bundle = "x".repeat(600_000);
    adapter
        .command(&command_envelope(AppCommand::ComputerTargetApprove(
            pawork_protocol::ComputerTargetApproveParams {
                session_id: session.clone(),
                run_id: run.clone(),
                target: application_target(&huge_bundle),
                kind: ComputerGrantKind::ForRun,
            },
        )))
        .await
        .expect("approve huge target");
    let error = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: session.clone(),
        }))
        .await
        .expect_err("response budget exceeded");
    assert_eq!(error.code, "response_too_large", "{error:?}");
}

#[tokio::test]
async fn observation_entries_with_invalid_base64_fail_closed() {
    let (adapter, _dir, session) = adapter_with_session().await;
    let first = seed_text_event(&adapter, &session, 1).await;
    // 直接落一条 Base64 字符非法的图像事件：列表不得收录该条目，
    // 读取整串拒绝（与全量解码同口径，切片合法也不放行）。
    let call_event = EventId::from("evt-call-invalid");
    append(
        &adapter,
        &session,
        "run-1",
        2,
        Some(first),
        call_event.clone(),
        AgentEvent::ToolCallStarted {
            tool_call_id: ToolCallId::from("call-invalid"),
            name: "computer".into(),
        },
    )
    .await;
    append(
        &adapter,
        &session,
        "run-1",
        3,
        Some(call_event),
        EventId::from("evt-obs-invalid"),
        AgentEvent::ToolExecutionCompleted {
            tool_call_id: ToolCallId::from("call-invalid"),
            result: ToolResultContent {
                tool_call_id: ToolCallId::from("call-invalid"),
                tool_name: Some("computer".into()),
                content: vec![pawork_domain::ContentPart::Image(
                    pawork_domain::ImageContent {
                        source: pawork_domain::ImageSource::Base64("AAAA!!!!".into()),
                        media_type: "image/png".into(),
                        alt_text: None,
                    },
                )],
                is_error: false,
                metadata: serde_json::json!({}),
                artifacts: Vec::new(),
            },
        },
    )
    .await;
    let response = adapter
        .query(&query_envelope(AppQuery::ObservationList {
            session_id: session.clone(),
            after_sequence: None,
            limit: None,
        }))
        .await
        .expect("list");
    let AppResponse::Data(data) = response else {
        panic!("observation list returns data");
    };
    assert_eq!(data["entries"], json!([]), "invalid base64 is not listed");
    let error = adapter
        .query(&query_envelope(observation_query(&session, 3, 0, 0, 3)))
        .await
        .expect_err("invalid base64 read rejected");
    assert_eq!(error.code, "app_error", "{error:?}");
    assert!(error.message.contains("not valid base64"), "{error:?}");
}

/// fork 后 active lineage 之外的同 session 图像不可读，也不能被支线上
/// 更晚的同 part 图像冒充。两侧 tool call 落在 fork 点之前，完成事件分别落在
/// main 与 side；读取 main 的 sequence 时，lineage 上的下一条恰好是 side 的图。
#[tokio::test]
async fn observation_fork_lineage_does_not_alias_later_image() {
    let (adapter, _dir, session) = adapter_with_session().await;
    let root = seed_text_event(&adapter, &session, 1).await;
    let main_call = EventId::from("evt-call-main");
    append(
        &adapter,
        &session,
        "run-main",
        2,
        Some(root),
        main_call.clone(),
        AgentEvent::ToolCallStarted {
            tool_call_id: ToolCallId::from("call-main"),
            name: "computer".into(),
        },
    )
    .await;
    let side_call = EventId::from("evt-call-side");
    append(
        &adapter,
        &session,
        "run-side",
        3,
        Some(main_call),
        side_call.clone(),
        AgentEvent::ToolCallStarted {
            tool_call_id: ToolCallId::from("call-side"),
            name: "computer".into(),
        },
    )
    .await;
    let boundary = EventId::from("evt-boundary");
    append(
        &adapter,
        &session,
        "run-1",
        4,
        Some(side_call),
        boundary.clone(),
        AgentEvent::CompactionCompleted {
            summary_message_id: MessageId::from("fork-boundary"),
            compacted_through: EventSequence::new(0),
        },
    )
    .await;
    adapter
        .core
        .read()
        .await
        .store()
        .expect("store")
        .fork_from_event(&session, "side", &boundary)
        .await
        .expect("fork");

    let main_image: &[u8] = &[1, 2, 3, 4];
    append(
        &adapter,
        &session,
        "run-main",
        5,
        Some(boundary.clone()),
        EventId::from("evt-obs-main"),
        image_completed("call-main", main_image),
    )
    .await;
    assert_eq!(
        list_sequences(&adapter, &session).await,
        vec![5],
        "main image is visible before the switch"
    );
    assert_eq!(read_image(&adapter, &session, 5).await, main_image);

    adapter
        .core
        .read()
        .await
        .store()
        .expect("store")
        .switch_branch(&session, "side")
        .await
        .expect("switch to side");
    let side_image: &[u8] = &[9, 9, 9, 9];
    append(
        &adapter,
        &session,
        "run-side",
        6,
        Some(boundary),
        EventId::from("evt-obs-side"),
        image_completed("call-side", side_image),
    )
    .await;

    assert_eq!(
        list_sequences(&adapter, &session).await,
        vec![6],
        "fork lineage lists only its own image"
    );
    let error = adapter
        .query(&query_envelope(observation_query(&session, 5, 1, 0, 8)))
        .await
        .expect_err("main image is off the fork lineage");
    assert_eq!(error.code, "not_found", "{error:?}");
    assert_eq!(read_image(&adapter, &session, 6).await, side_image);

    adapter
        .core
        .read()
        .await
        .store()
        .expect("store")
        .switch_branch(&session, pawork_storage::session::DEFAULT_BRANCH_ID)
        .await
        .expect("switch back to main");
    assert_eq!(list_sequences(&adapter, &session).await, vec![5]);
    assert_eq!(read_image(&adapter, &session, 5).await, main_image);
    let error = adapter
        .query(&query_envelope(observation_query(&session, 6, 1, 0, 8)))
        .await
        .expect_err("side image is off the main lineage");
    assert_eq!(error.code, "not_found", "{error:?}");
}

fn image_completed(call_id: &str, bytes: &[u8]) -> AgentEvent {
    AgentEvent::ToolExecutionCompleted {
        tool_call_id: ToolCallId::from(call_id),
        result: ToolResultContent {
            tool_call_id: ToolCallId::from(call_id),
            tool_name: Some("computer".into()),
            content: vec![
                pawork_domain::ContentPart::Text(pawork_domain::TextContent {
                    text: "captured".into(),
                }),
                pawork_domain::ContentPart::Image(pawork_domain::ImageContent {
                    source: pawork_domain::ImageSource::Base64(
                        crate::services::extension::base64_encode(bytes),
                    ),
                    media_type: "image/png".into(),
                    alt_text: None,
                }),
            ],
            is_error: false,
            metadata: serde_json::json!({}),
            artifacts: Vec::new(),
        },
    }
}

async fn list_sequences(adapter: &GuiHostAdapter, session: &SessionId) -> Vec<u64> {
    let response = adapter
        .query(&query_envelope(AppQuery::ObservationList {
            session_id: session.clone(),
            after_sequence: None,
            limit: None,
        }))
        .await
        .expect("observation list");
    let AppResponse::Data(data) = response else {
        panic!("observation list returns data");
    };
    data["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| entry["sequence"].as_u64().expect("sequence"))
        .collect()
}

async fn read_image(adapter: &GuiHostAdapter, session: &SessionId, sequence: u64) -> Vec<u8> {
    let response = adapter
        .query(&query_envelope(observation_query(
            session, sequence, 1, 0, 8,
        )))
        .await
        .expect("image read");
    let AppResponse::Data(data) = response else {
        panic!("image read returns data");
    };
    assert_eq!(data["sequence"], json!(sequence));
    crate::services::extension::base64_decode(data["data"].as_str().expect("base64"))
        .expect("decode")
}

/// 模拟目标只进真实 TargetRegistry 的 requested 集：不绑定窗口、不启动进程。
/// 随后的 list / approve / revoke 走正式 GUI 命令，撤销后条目消失。
#[tokio::test]
async fn fixture_simulated_target_is_listed_approved_and_revoked() {
    let (adapter, _dir, session) = adapter_with_session().await;
    let error = adapter
        .fixture_note_simulated_target("dev.pawork.fixture.simulated")
        .await
        .expect_err("no active run");
    assert!(
        error.contains("expected exactly one active gui run"),
        "{error}"
    );

    let run = RunId::from("run-cu18-sim");
    adapter
        .core
        .read()
        .await
        .open_target_scope(&pawork_computer_use::target::Scope::new(
            "ws-cu17",
            "run-cu18-sim",
        ));
    adapter.runs.register(
        ActiveGuiRun {
            run_id: run.clone(),
            session_id: session.clone(),
            workspace_id: WorkspaceId::from("ws-cu17"),
            workspace_roots: Vec::new(),
            started_at_ms: 1,
        },
        CancellationToken::new(),
    );

    let snapshot = adapter
        .fixture_note_simulated_target("dev.pawork.fixture.simulated")
        .await
        .expect("register simulated target");
    assert_eq!(snapshot["targets"].as_array().expect("targets").len(), 1);
    assert_eq!(
        snapshot["targets"][0]["bundle_id"],
        "dev.pawork.fixture.simulated"
    );
    assert_eq!(snapshot["targets"][0]["grant"], json!(null));
    assert_eq!(snapshot["targets"][0]["bound_window"], json!(null));
    let again = adapter
        .fixture_note_simulated_target("dev.pawork.fixture.simulated")
        .await
        .expect("idempotent register");
    assert_eq!(again["targets"].as_array().expect("targets").len(), 1);

    let response = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: session.clone(),
        }))
        .await
        .expect("list");
    let AppResponse::Data(data) = response else {
        panic!("target list returns data");
    };
    let targets = data["targets"].as_array().expect("targets");
    assert_eq!(targets.len(), 1);
    assert!(targets[0].get("grant").is_none(), "{targets:?}");
    assert!(targets[0].get("bound_window").is_none(), "{targets:?}");
    assert_eq!(
        targets[0]["target"]["app"]["bundle_id"],
        "dev.pawork.fixture.simulated"
    );

    adapter
        .command(&command_envelope(AppCommand::ComputerTargetApprove(
            pawork_protocol::ComputerTargetApproveParams {
                session_id: session.clone(),
                run_id: run.clone(),
                target: application_target("dev.pawork.fixture.simulated"),
                kind: ComputerGrantKind::Once,
            },
        )))
        .await
        .expect("approve");
    let granted = adapter.fixture_target_snapshot().await;
    assert_eq!(granted["targets"][0]["grant"], "once");

    adapter
        .command(&command_envelope(AppCommand::ComputerTargetRevoke(
            pawork_protocol::ComputerTargetRevokeParams {
                session_id: session.clone(),
                run_id: run,
                target: application_target("dev.pawork.fixture.simulated"),
            },
        )))
        .await
        .expect("revoke");
    let response = adapter
        .query(&query_envelope(AppQuery::ComputerTargetList {
            session_id: session,
        }))
        .await
        .expect("list after revoke");
    let AppResponse::Data(data) = response else {
        panic!("target list returns data");
    };
    assert_eq!(data["targets"], json!([]), "{data}");
}
