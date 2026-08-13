//! P18-12 security tests：身份头伪造 / 缺失 fail-closed、tenant binding 只信
//! 受信上下文、signed thinking 明文永不进入错误 / Debug / canonical 事件，
//! 以及保护器缺失 / 能力未协商的显式失败路径。

use std::sync::Arc;

use agent_domain::{AgentId, PrincipalId, SessionId, TenantId};
use client_adapter_api::{
    CapabilitySnapshot, ClientCapability, ClientProtocol, CLIENT_ADAPTER_SCHEMA_VERSION,
};
use client_claude_gateway::{
    bind_tenant, decode_frame, extract_identity, map_sse_event, protect_pending_signed,
    ClaudeGatewayAdapterFactory, ClaudeGatewayError, ClaudeStreamState, GatewayEvent, HeaderPair,
    InMemorySignedThinkingProtector, SseFrame, SseParser, TrustedTenantContext, HEADER_AGENT_ID,
    HEADER_PARENT_AGENT_ID, HEADER_SESSION_ID,
};
use provider_api::ProviderStreamEvent;

const SECRET_SIGNATURE: &str = "SIG-ATTACKER-CONTROLLED-SECRET";

fn snapshot(capabilities: &[&str]) -> CapabilitySnapshot {
    CapabilitySnapshot {
        schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
        protocol: ClientProtocol::new("claude-gateway"),
        protocol_version: "1".into(),
        client_version: "2.0.0".into(),
        revision: 1,
        capabilities: capabilities
            .iter()
            .map(|name| ClientCapability::new(*name))
            .collect(),
    }
}

fn trusted(tenant: &str, principal: &str) -> TrustedTenantContext {
    TrustedTenantContext::try_new(TenantId::from(tenant), PrincipalId::from(principal))
        .expect("trusted context")
}

#[test]
fn identity_header_forgery_fails_closed() {
    // 缺失 session：必须失败。
    assert!(matches!(
        extract_identity([
            HeaderPair::new(HEADER_AGENT_ID, "agent-1"),
            HeaderPair::new(HEADER_PARENT_AGENT_ID, "agent-0"),
        ]),
        Err(ClaudeGatewayError::MissingIdentityHeader(HEADER_SESSION_ID))
    ));

    // 大小写不同的重复 header：必须失败。
    assert!(matches!(
        extract_identity([
            HeaderPair::new(HEADER_SESSION_ID, "sess-1"),
            HeaderPair::new("X-Claude-Code-Session-Id", "sess-2"),
        ]),
        Err(ClaudeGatewayError::DuplicateIdentityHeader(
            HEADER_SESSION_ID
        ))
    ));

    // 空白 / 控制字符 / 超长：必须失败。
    for forged in ["", "   ", "sess\u{0}injected", &"x".repeat(300)] {
        assert!(matches!(
            extract_identity([HeaderPair::new(HEADER_SESSION_ID, forged)]),
            Err(ClaudeGatewayError::MalformedIdentityHeader(
                HEADER_SESSION_ID
            ))
        ));
    }

    // agent 自引用 / parent 无 agent：必须失败。
    assert!(matches!(
        extract_identity([
            HeaderPair::new(HEADER_SESSION_ID, "sess-1"),
            HeaderPair::new(HEADER_AGENT_ID, "agent-1"),
            HeaderPair::new(HEADER_PARENT_AGENT_ID, "agent-1"),
        ]),
        Err(ClaudeGatewayError::InvalidAgentTree(_))
    ));
    assert!(matches!(
        extract_identity([
            HeaderPair::new(HEADER_SESSION_ID, "sess-1"),
            HeaderPair::new(HEADER_PARENT_AGENT_ID, "agent-0"),
        ]),
        Err(ClaudeGatewayError::InvalidAgentTree(_))
    ));
}

#[test]
fn tenant_never_derives_from_headers() {
    // 攻击者伪造 tenant header：被忽略，身份不受影响；tenant 只来自受信上下文。
    let identity = extract_identity([
        HeaderPair::new(HEADER_SESSION_ID, "sess-1"),
        HeaderPair::new("x-claude-code-tenant-id", "tenant-evil"),
        HeaderPair::new("x-pawork-tenant-id", "tenant-evil"),
    ])
    .expect("identity");

    // 同一受信租户 × 不同伪造 tenant header：tenant 恒定。
    let trusted = trusted("tenant-trusted", "user-1");
    let binding = bind_tenant(&identity, &trusted).expect("bind");
    assert_eq!(binding.tenant.tenant_id.as_str(), "tenant-trusted");
    assert_eq!(binding.tenant.principal_id.as_str(), "user-1");
    assert_eq!(binding.session_id, SessionId::from("sess-1"));
    assert_eq!(binding.agent_id, None);
}

#[test]
fn tenant_binding_fails_closed_without_trusted_context() {
    let identity =
        extract_identity([HeaderPair::new(HEADER_SESSION_ID, "sess-1")]).expect("identity");
    for (tenant, principal) in [("", "user-1"), ("tenant-a", "   "), (" \t ", "")] {
        assert!(matches!(
            TrustedTenantContext::try_new(TenantId::from(tenant), PrincipalId::from(principal)),
            Err(ClaudeGatewayError::MissingTenantContext(_))
        ));
    }
    // 没有任何 API 路径能从 identity / header 推导 tenant。
    let binding = bind_tenant(&identity, &trusted("tenant-a", "user-1")).expect("bind");
    assert_eq!(binding.tenant.tenant_id.as_str(), "tenant-a");
    assert_eq!(binding.agent_id, None);
    assert_eq!(binding.parent_agent_id, None);
    // subagent 键只随 header 变化，与 tenant 正交。
    let subagent = extract_identity([
        HeaderPair::new(HEADER_SESSION_ID, "sess-2"),
        HeaderPair::new(HEADER_AGENT_ID, "agent-2"),
        HeaderPair::new(HEADER_PARENT_AGENT_ID, "agent-1"),
    ])
    .expect("subagent");
    let subagent_binding = bind_tenant(&subagent, &trusted("tenant-a", "user-1")).expect("bind");
    assert_eq!(subagent_binding.tenant, binding.tenant);
    assert_eq!(subagent_binding.agent_id, Some(AgentId::from("agent-2")));
    assert_eq!(
        subagent_binding.parent_agent_id,
        Some(AgentId::from("agent-1"))
    );
}

fn signed_stop_frame(index: usize) -> SseFrame {
    SseFrame {
        event: Some("content_block_stop".into()),
        data: format!(
            r#"{{"type":"content_block_stop","index":{index},"content_block":{{"type":"thinking","thinking":"hmm","signature":"{SECRET_SIGNATURE}"}}}}"#
        ),
    }
}

#[test]
fn signed_thinking_without_negotiated_capability_fails_and_never_leaks() {
    // 默认状态 = fail-closed（能力未协商）。
    let mut state = ClaudeStreamState::default();
    let event = decode_frame(&signed_stop_frame(0)).expect("decode");
    let error = map_sse_event(&mut state, &event).expect_err("must fail closed");
    assert!(matches!(
        error,
        ClaudeGatewayError::SignedThinkingNotNegotiated(capability)
            if capability == "reasoning.signed_continuity"
    ));
    assert!(!format!("{error}").contains(SECRET_SIGNATURE));
    assert!(!format!("{error:?}").contains(SECRET_SIGNATURE));
    assert_eq!(state.pending_signed_count(), 0);

    // 经 factory 协商（未声明 reasoning 能力）同样 fail-closed。
    let factory = ClaudeGatewayAdapterFactory::with_defaults(None);
    let negotiated = factory
        .create_concrete(snapshot(&["events"]))
        .expect("negotiate without reasoning capability");
    assert!(!negotiated.adapter.reasoning_supported());
    let mut negotiated_state = negotiated.adapter.stream_state();
    let error = map_sse_event(&mut negotiated_state, &event)
        .expect_err("negotiated-but-absent capability must fail");
    assert!(matches!(
        error,
        ClaudeGatewayError::SignedThinkingNotNegotiated(_)
    ));
}

#[test]
fn signature_plaintext_never_reaches_debug_or_canonical_events() {
    let mut state = ClaudeStreamState::new(true);
    // signature_delta 以明文流式到达：只累积进受保护状态。
    let parser_frames = SseParser::new().push(&format!(
        "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"signature_delta\",\"signature\":\"{SECRET_SIGNATURE}\"}}}}\n\n"
    ));
    let event = decode_frame(parser_frames[0].as_ref().expect("valid")).expect("decode");
    let events = map_sse_event(&mut state, &event).expect("map");
    assert!(
        events.is_empty(),
        "signature deltas emit no canonical event"
    );

    let debug = format!("{state:?}");
    assert!(debug.contains("signature_parts: 1"));
    assert!(!debug.contains(SECRET_SIGNATURE));

    // content_block_stop 捕获后，pending 结构与整体 Debug 仍不泄露。
    let stop = decode_frame(&SseFrame {
        event: Some("content_block_stop".into()),
        data: "{\"type\":\"content_block_stop\",\"index\":0}".into(),
    })
    .expect("decode stop");
    map_sse_event(&mut state, &stop).expect("capture from signature parts");
    let debug = format!("{state:?}");
    assert!(debug.contains("pending_signed: 1"));
    assert!(!debug.contains(SECRET_SIGNATURE));
}

#[tokio::test]
async fn protector_unavailable_fails_closed_without_leaking_material() {
    // 能力已协商但保护器未注入：保护显式失败，错误信息不含材料。
    let mut state = ClaudeStreamState::new(true);
    let event = decode_frame(&signed_stop_frame(0)).expect("decode");
    map_sse_event(&mut state, &event).expect("capture");
    let error = protect_pending_signed(&mut state)
        .await
        .expect_err("no protector injected must fail");
    assert!(matches!(
        error,
        ClaudeGatewayError::SignedThinkingProtectorUnavailable(_)
    ));
    assert!(!format!("{error}").contains(SECRET_SIGNATURE));
    assert!(!format!("{error:?}").contains(SECRET_SIGNATURE));
}

#[tokio::test]
async fn protected_blob_path_keeps_material_out_of_canonical_output() {
    let protector = Arc::new(InMemorySignedThinkingProtector::new());
    let factory = ClaudeGatewayAdapterFactory::with_defaults(Some(protector.clone()));
    let negotiated = factory
        .create_concrete(snapshot(&["events", "reasoning.signed_continuity"]))
        .expect("negotiate");
    let mut state = negotiated.adapter.stream_state();
    let event = decode_frame(&signed_stop_frame(0)).expect("decode");
    map_sse_event(&mut state, &event).expect("capture");
    let protected = protect_pending_signed(&mut state).await.expect("protect");

    let GatewayEvent::Stream(ProviderStreamEvent::ReasoningItem(item)) = &protected[0] else {
        panic!("expected reasoning item");
    };
    for surface in [
        format!("{item:?}"),
        serde_json::to_string(item).expect("serialize item"),
        format!("{protected:?}"),
        format!("{state:?}"),
    ] {
        assert!(
            !surface.contains(SECRET_SIGNATURE),
            "secret leaked into surface: {surface}"
        );
    }
}

#[test]
fn malformed_frames_fail_closed() {
    // data 不是 JSON / event 名与 data type 不一致：显式失败，不静默跳过。
    assert!(matches!(
        decode_frame(&SseFrame {
            event: None,
            data: "not-json".into(),
        }),
        Err(ClaudeGatewayError::MalformedSse(_))
    ));
    assert!(matches!(
        decode_frame(&SseFrame {
            event: Some("message_start".into()),
            data: "{\"type\":\"ping\"}".into(),
        }),
        Err(ClaudeGatewayError::MalformedSse(_))
    ));
    assert!(matches!(
        decode_frame(&SseFrame {
            event: None,
            data: "{\"no_type\":true}".into(),
        }),
        Err(ClaudeGatewayError::MalformedEvent(_, _))
    ));
}

#[test]
fn unknown_sdk_events_are_preserved_not_silently_dropped() {
    let mut state = ClaudeStreamState::default();
    let event = decode_frame(&SseFrame {
        event: None,
        data: "{\"type\":\"future_sdk_event\",\"opaque\":true}".into(),
    })
    .expect("decode");
    let events = map_sse_event(&mut state, &event).expect("map");
    assert_eq!(
        events,
        vec![GatewayEvent::Unmapped {
            event_type: "future_sdk_event".into(),
        }]
    );
}
