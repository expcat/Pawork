//! P18-12 golden tests：Claude Messages wire protocol → canonical 事件的
//! 稳定映射基线。golden 期望值以 canonical JSON 内联断言（不引入额外依赖），
//! 覆盖 text / thinking / tool / usage / stop、signed thinking continuity、
//! SDK permission / subagent / hook / task 生命周期与 cancel / error。

use std::sync::Arc;

use agent_domain::{TokenUsage, ToolCallId};
use client_adapter_api::{
    CapabilitySnapshot, ClientCapability, ClientProtocol, CLIENT_ADAPTER_SCHEMA_VERSION,
};
use client_claude_gateway::{
    decode_frame, map_sse_event, protect_pending_signed, ClaudeGatewayAdapterFactory,
    ClaudeStreamState, ControlEvent, GatewayEvent, GatewayPermissionDecision,
    InMemorySignedThinkingProtector, SignedThinkingMaterial, SignedThinkingProtector, SseParser,
};
use provider_api::{ProviderErrorKind, ProviderStreamEvent};
use serde_json::json;

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

fn drive(state: &mut ClaudeStreamState, frames: &[&str]) -> Vec<GatewayEvent> {
    let mut parser = SseParser::new();
    let mut events = Vec::new();
    for frame in frames {
        for parsed in parser.push(frame) {
            let frame = parsed.expect("valid sse frame");
            let event = decode_frame(&frame).expect("decode frame");
            events.extend(map_sse_event(state, &event).expect("map event"));
        }
    }
    events
}

fn stream_json(events: &[GatewayEvent]) -> serde_json::Value {
    serde_json::to_value(
        events
            .iter()
            .filter_map(|event| match event {
                GatewayEvent::Stream(stream) => Some(stream.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
    )
    .expect("serialize provider stream events")
}

#[test]
fn messages_stream_golden_maps_to_canonical_provider_events() {
    let mut state = ClaudeStreamState::default();
    let events = drive(
        &mut state,
        &[
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\",\"model\":\"claude-sonnet\",\"usage\":{\"input_tokens\":25,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"Read\",\"input\":{}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"file\\\":\\\"a.txt\\\"}\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ],
    );

    assert!(
        events
            .iter()
            .all(|event| matches!(event, GatewayEvent::Stream(_))),
        "every wire event must map to the canonical provider boundary: {events:?}"
    );
    assert_eq!(
        stream_json(&events),
        json!([
            {"type": "response_started", "data": {"response_id": "msg_01"}},
            {"type": "text_delta", "data": "Hi"},
            {"type": "tool_call_started", "data": {"id": "toolu_1", "name": "Read"}},
            {"type": "tool_call_arguments_delta", "data": {"id": "toolu_1", "json": "{\"file\":\"a.txt\"}"}},
            {"type": "tool_call_completed", "data": {"id": "toolu_1"}},
            {"type": "usage_updated", "data": {
                "input_tokens": 25,
                "output_tokens": 7,
                "cache_read_tokens": 0,
                "cache_write_tokens": 0
            }},
            {"type": "response_completed", "data": {"kind": "tool_use"}},
        ])
    );
    assert!(state.finished);
}

#[tokio::test]
async fn signed_thinking_golden_negotiates_capability_and_protects_blob() {
    let protector: Arc<InMemorySignedThinkingProtector> =
        Arc::new(InMemorySignedThinkingProtector::new());
    let factory = ClaudeGatewayAdapterFactory::with_defaults(Some(protector.clone()));
    let negotiated = factory
        .create_concrete(snapshot(&["events", "reasoning.signed_continuity"]))
        .expect("negotiate");
    assert!(negotiated.adapter.reasoning_supported());

    let mut state = negotiated.adapter.stream_state();
    let events = drive(
        &mut state,
        &[
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_02\",\"model\":\"claude-opus\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"planning step\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"planning step\",\"signature\":\"SIG-GOLDEN-SECRET\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":12}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ],
    );
    assert_eq!(
        stream_json(&events),
        json!([
            {"type": "response_started", "data": {"response_id": "msg_02"}},
            {"type": "thinking_delta", "data": "planning step"},
            {"type": "usage_updated", "data": {
                "input_tokens": 10,
                "output_tokens": 12,
                "cache_read_tokens": 0,
                "cache_write_tokens": 0
            }},
            {"type": "response_completed", "data": {"kind": "completed"}},
        ])
    );

    let protected = protect_pending_signed(&mut state)
        .await
        .expect("protect signed material");
    assert_eq!(protected.len(), 1);
    let GatewayEvent::Stream(ProviderStreamEvent::ReasoningItem(item)) = &protected[0] else {
        panic!("expected canonical reasoning item, got {protected:?}");
    };
    assert_eq!(item.id.as_str(), "claude-reasoning-1");
    assert_eq!(
        item.continuation_metadata["anthropic_block_kind"],
        json!("thinking")
    );

    let encoded = serde_json::to_string(item).expect("serialize reasoning item");
    assert!(encoded.contains("claude-signed-0"));
    assert!(!encoded.contains("SIG-GOLDEN-SECRET"));

    let payload = protector
        .resolve(&item.protected_blob_ref)
        .await
        .expect("resolve protected blob");
    let material: SignedThinkingMaterial =
        serde_json::from_slice(&payload).expect("decode protected payload");
    match material {
        SignedThinkingMaterial::Thinking { signature } => {
            assert_eq!(signature, "SIG-GOLDEN-SECRET");
        }
        SignedThinkingMaterial::Redacted { .. } => panic!("expected thinking material"),
    }
}

#[test]
fn sdk_control_and_lifecycle_golden() {
    let mut state = ClaudeStreamState::default();
    let events = drive(
        &mut state,
        &[
            "event: hook_event\ndata: {\"type\":\"hook_event\",\"event\":{\"hook_name\":\"SubagentStart\",\"session_id\":\"sess-1\",\"agent_id\":\"agent-2\",\"parent_agent_id\":\"agent-1\"}}\n\n",
            "event: control_request\ndata: {\"type\":\"control_request\",\"request_id\":\"req-1\",\"request\":{\"subtype\":\"can_use_tool\",\"tool_name\":\"Bash\",\"input\":{\"command\":\"ls\"},\"tool_use_id\":\"call-1\"}}\n\n",
            "event: control_response\ndata: {\"type\":\"control_response\",\"response\":{\"request_id\":\"req-1\",\"response\":{\"subtype\":\"success\",\"request_id\":\"req-1\",\"response\":{\"behavior\":\"allow\"}}}}\n\n",
            "event: user\ndata: {\"type\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"call-1\",\"content\":\"ok\"}]}\n\n",
            "event: result\ndata: {\"type\":\"result\",\"subtype\":\"success\",\"result\":\"done\"}\n\n",
        ],
    );
    assert_eq!(
        events,
        vec![
            GatewayEvent::Control(ControlEvent::SubagentStarted {
                session_id: Some("sess-1".into()),
                agent_id: Some("agent-2".into()),
                parent_agent_id: Some("agent-1".into()),
            }),
            GatewayEvent::Control(ControlEvent::PermissionRequested {
                request_id: "req-1".into(),
                tool_name: "Bash".into(),
                input: json!({"command": "ls"}),
                tool_call_id: Some(ToolCallId::from("call-1")),
            }),
            GatewayEvent::Control(ControlEvent::PermissionDecided {
                request_id: "req-1".into(),
                decision: GatewayPermissionDecision::Allowed,
            }),
            GatewayEvent::Control(ControlEvent::ToolResultSubmitted {
                tool_use_id: "call-1".into(),
                is_error: false,
            }),
            GatewayEvent::Control(ControlEvent::RunResult {
                result_type: Some("success".into()),
            }),
        ]
    );
}

#[test]
fn cancel_and_upstream_error_golden() {
    let mut state = ClaudeStreamState::default();
    let events = drive(
        &mut state,
        &[
            "data: {\"type\":\"aborted\"}\n\n",
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"upstream busy\"}}\n\n",
        ],
    );
    assert!(matches!(
        &events[0],
        GatewayEvent::Control(ControlEvent::Interrupted { reason: Some(reason) })
            if reason == "aborted"
    ));
    assert!(matches!(
        &events[1],
        GatewayEvent::Stream(ProviderStreamEvent::Error(error))
            if error.kind == ProviderErrorKind::Cancelled
    ));
    assert!(matches!(
        &events[2],
        GatewayEvent::Error(error)
            if error.kind == ProviderErrorKind::RateLimited && error.message == "upstream busy"
    ));
    assert!(state.finished);
}

#[test]
fn usage_rolls_up_input_from_start_and_output_from_delta() {
    let mut state = ClaudeStreamState::default();
    let events = drive(
        &mut state,
        &[
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":40,\"cache_read_input_tokens\":5,\"cache_creation_input_tokens\":2}}}\n\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":9}}\n\n",
        ],
    );
    assert!(matches!(
        &events[1],
        GatewayEvent::Stream(ProviderStreamEvent::UsageUpdated(TokenUsage {
            input_tokens: 40,
            output_tokens: 9,
            cache_read_tokens: 5,
            cache_write_tokens: 2,
        }))
    ));
}
