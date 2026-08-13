//! 协议集成测试：JSON fixture 驱动的翻译往返、错误帧与 run_loop 输出。

use agent_domain::{CommandId, QueryId, RunId, Timestamp};
use headless_json::translate::{encode_protocol_response, error_frame, translate_request_line};
use headless_json::wire::{
    CompatSource, HeadlessRequest, HeadlessResponse, ProtocolErrorKind, SdkCapability,
    TranslatedRequest,
};
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read fixture {path}: {error}"));
    serde_json::from_str(&text).expect("fixture is valid JSON")
}

#[test]
fn translate_fixture_roundtrip() {
    let cases = fixture("translate_cases.json");
    let cases = cases.as_array().expect("fixture is an array");
    assert!(!cases.is_empty(), "fixture must not be empty");
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let line = case["input_line"].as_str().unwrap();
        let expected = &case["expect"];
        if expected["kind"].as_str() == Some("hello") {
            let error = translate_request_line(line).err_or_panic(name);
            assert_eq!(
                error.kind,
                ProtocolErrorKind::MalformedFrame,
                "{name}: hello must not dispatch"
            );
            continue;
        }
        let translated = translate_request_line(line)
            .unwrap_or_else(|error| panic!("{name}: translate failed: {error}"));
        match (&translated, expected["kind"].as_str().unwrap()) {
            (TranslatedRequest::Command(envelope), "command") => {
                assert_eq!(
                    expected["method"].as_str().unwrap(),
                    serde_json::to_value(&envelope.command).unwrap()["method"]
                        .as_str()
                        .unwrap(),
                    "{name}: command method"
                );
            }
            (TranslatedRequest::Query(envelope), "query") => {
                assert_eq!(
                    expected["method"].as_str().unwrap(),
                    serde_json::to_value(&envelope.query).unwrap()["method"]
                        .as_str()
                        .unwrap(),
                    "{name}: query method"
                );
            }
            (TranslatedRequest::CompatImport(request), "compat_import") => {
                assert_eq!(
                    expected["source"].as_str().unwrap(),
                    request.source.to_string(),
                    "{name}: compat source"
                );
                assert!(request.options.dry_run, "{name}: options preserved");
            }
            (TranslatedRequest::CompatHistory(query), "compat_history") => {
                assert_eq!(
                    expected["limit"].as_u64().unwrap() as u32,
                    query.limit.unwrap()
                );
            }
            (TranslatedRequest::Command(_), kind) => panic!("{name}: unexpected {kind}"),
            (TranslatedRequest::Query(_), kind) => panic!("{name}: unexpected {kind}"),
            (TranslatedRequest::CompatImport(_), kind) => panic!("{name}: unexpected {kind}"),
            (TranslatedRequest::CompatHistory(_), kind) => panic!("{name}: unexpected {kind}"),
        }
    }
}

#[test]
fn hello_is_handshake_not_dispatchable() {
    let line = json!({
        "type": "hello",
        "client_name": "test",
        "client_version": "0.0.0",
        "supported_api_versions": [{"major": 1, "minor": 0}],
        "capabilities": ["sessions"]
    })
    .to_string();
    let error = translate_request_line(&line).expect_err("hello must not dispatch");
    assert_eq!(error.kind, ProtocolErrorKind::MalformedFrame);
}

#[test]
fn error_fixture_kinds_are_explicit() {
    let cases = fixture("error_cases.json");
    for case in cases.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let line = case["input_line"].as_str().unwrap();
        let expected = case["expect_kind"].as_str().unwrap();
        let error = translate_request_line(line).err_or_panic(name);
        let actual = match error.kind {
            ProtocolErrorKind::UnknownRequestType => "unknown_request_type",
            ProtocolErrorKind::MalformedFrame => "malformed_frame",
            ProtocolErrorKind::IncompatibleApiVersion => "incompatible_api_version",
            ProtocolErrorKind::TooLarge => "too_large",
            ProtocolErrorKind::UnsupportedCapability => "unsupported_capability",
            ProtocolErrorKind::NotHandshaked => "not_handshaked",
            ProtocolErrorKind::CompatRejected => "compat_rejected",
            ProtocolErrorKind::Backpressure => "backpressure",
            ProtocolErrorKind::Internal => "internal",
        };
        assert_eq!(expected, actual, "{name}: error kind");
    }
}

// 小助手：unwrap 或带测试名 panic（避免 Result 方法链破坏断言信息）。
trait UnwrapOrPanic<T> {
    fn err_or_panic(self, name: &str) -> headless_json::ProtocolError;
}

impl<T> UnwrapOrPanic<T> for Result<T, headless_json::ProtocolError> {
    fn err_or_panic(self, name: &str) -> headless_json::ProtocolError {
        self.err()
            .unwrap_or_else(|| panic!("{name}: expected failure"))
    }
}

#[test]
fn event_fixture_encodes_and_roundtrips() {
    let cases = fixture("event_cases.json");
    for case in cases.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let line = case["input_line"].as_str().unwrap();
        let response: HeadlessResponse =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("{name}: parse {e}"));
        let encoded = encode_protocol_response(&response).expect("encode");
        let reparsed: HeadlessResponse = serde_json::from_str(&encoded).expect("reparse");
        assert_eq!(response, reparsed, "{name}: encode roundtrip");
        let HeadlessResponse::Event { envelope } = &reparsed else {
            panic!("{name}: expected event frame");
        };
        assert_eq!(
            case["expect_payload_type"].as_str().unwrap(),
            serde_json::to_value(&envelope.payload).unwrap()["type"]
                .as_str()
                .unwrap(),
            "{name}: payload type"
        );
    }
}

#[test]
fn compat_response_fixtures_roundtrip() {
    let cases = fixture("compat_response_cases.json");
    for case in cases.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let input: HeadlessResponse =
            serde_json::from_value(case["input"].clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let encoded = encode_protocol_response(&input).expect("encode");
        let reparsed: HeadlessResponse = serde_json::from_str(&encoded).expect("reparse");
        assert_eq!(input, reparsed, "{name}: roundtrip");
    }
}

#[test]
fn unknown_and_malformed_requests_produce_explicit_error_frames() {
    let unknown =
        translate_request_line(r#"{"type":"teleport"}"#).expect_err("unknown type must fail");
    assert_eq!(unknown.kind, ProtocolErrorKind::UnknownRequestType);
    let frame = error_frame(None, unknown.kind, unknown.message);
    let line = encode_protocol_response(&frame).expect("encode");
    assert!(line.contains(r#""type":"error""#), "error frame: {line}");
    assert!(line.contains("unknown_request_type"), "error frame: {line}");

    let malformed = translate_request_line("{nope").expect_err("malformed must fail");
    assert_eq!(malformed.kind, ProtocolErrorKind::MalformedFrame);
}

#[test]
fn compat_source_labels_are_stable() {
    assert_eq!(CompatSource::Claude.as_str(), "claude");
    assert_eq!(CompatSource::Codex.as_str(), "codex");
    assert_eq!(CompatSource::Grok.as_str(), "grok");
    assert_eq!(CompatSource::Cursor.as_str(), "cursor");
}

mod stdio_tests {
    use super::*;
    use headless_json::stdio::{run_loop, LoopConfig};
    use headless_json::translate::encode_request;
    use headless_json::wire::HelloRequest;
    use std::collections::VecDeque;

    struct EchoHandler {
        /// poll_event 待投递的事件帧（测试事件交错路径）。
        pending_events: VecDeque<HeadlessResponse>,
        /// 握手次数（重复 hello 由 Host 接线层策略决定，本 handler 幂等应答）。
        handshakes: u32,
    }

    impl EchoHandler {
        fn new() -> Self {
            Self {
                pending_events: VecDeque::new(),
                handshakes: 0,
            }
        }
    }

    #[async_trait::async_trait]
    impl headless_json::stdio::Handler for EchoHandler {
        async fn handshake(&mut self, hello: HelloRequest) -> HeadlessResponse {
            self.handshakes += 1;
            HeadlessResponse::HelloAck {
                instance_id: "test-instance".into(),
                negotiated: hello.supported_api_versions[0],
                granted: hello.capabilities,
            }
        }

        async fn handle(&mut self, request: TranslatedRequest) -> Vec<HeadlessResponse> {
            match request {
                TranslatedRequest::Command(envelope) => vec![HeadlessResponse::Response {
                    envelope: core_api::AppResponseEnvelope {
                        api_version: core_api::API_VERSION,
                        request_id: QueryId::from(envelope.command_id.as_str()),
                        responded_at: Timestamp::from_unix_millis(1),
                        response: core_api::AppResponse::Accepted {
                            command_id: envelope.command_id,
                            run_id: None,
                        },
                    },
                }],
                TranslatedRequest::Query(envelope) => vec![HeadlessResponse::Response {
                    envelope: core_api::AppResponseEnvelope {
                        api_version: core_api::API_VERSION,
                        request_id: envelope.request_id,
                        responded_at: Timestamp::from_unix_millis(1),
                        response: core_api::AppResponse::Data(json!({"ok": true})),
                    },
                }],
                TranslatedRequest::CompatImport(request) => vec![HeadlessResponse::Error {
                    request_id: Some(request.request_id),
                    kind: ProtocolErrorKind::UnsupportedCapability,
                    message: "not wired in test handler".into(),
                }],
                TranslatedRequest::CompatHistory(query) => {
                    vec![HeadlessResponse::CompatHistoryResult {
                        request_id: query.request_id,
                        entries: vec![],
                        cursor: None,
                    }]
                }
            }
        }

        async fn poll_event(&mut self) -> Option<HeadlessResponse> {
            self.pending_events.pop_front()
        }
    }

    fn sample_command_line() -> String {
        let request = HeadlessRequest::Command {
            envelope: core_api::AppCommandEnvelope {
                api_version: core_api::API_VERSION,
                command_id: CommandId::from("cmd-1"),
                source: core_api::CommandSource::Automation,
                identity: core_api::ActorIdentity::Automation {
                    name: "test".into(),
                },
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(0),
                command: core_api::AppCommand::RunCancel {
                    run_id: RunId::from("run-9"),
                },
            },
        };
        encode_request(&request).expect("encode")
    }

    fn sample_hello_line() -> String {
        let hello = json!({
            "type": "hello",
            "client_name": "sdk-test",
            "client_version": "0.0.0",
            "supported_api_versions": [{"major": 1, "minor": 0}],
            "capabilities": ["sessions", "streaming"]
        });
        hello.to_string()
    }

    /// api_version 与当前协议不兼容的 command 帧（翻译阶段即失败）。
    fn bad_version_command_line() -> String {
        let request = HeadlessRequest::Command {
            envelope: core_api::AppCommandEnvelope {
                api_version: core_api::ApiVersion { major: 9, minor: 0 },
                command_id: CommandId::from("cmd-bad-version"),
                source: core_api::CommandSource::Automation,
                identity: core_api::ActorIdentity::Automation {
                    name: "test".into(),
                },
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(0),
                command: core_api::AppCommand::RunCancel {
                    run_id: RunId::from("run-9"),
                },
            },
        };
        encode_request(&request).expect("encode")
    }

    #[tokio::test]
    async fn run_loop_translates_requests_to_responses() {
        let input = format!("{}\n{}\n", sample_hello_line(), sample_command_line());
        let mut output = Vec::new();
        let mut handler = EchoHandler::new();
        run_loop(
            tokio::io::BufReader::new(input.as_bytes()),
            &mut output,
            LoopConfig::default(),
            &mut handler,
        )
        .await
        .expect("run loop");
        let lines: Vec<&str> = std::str::from_utf8(&output)
            .unwrap()
            .trim_end()
            .split('\n')
            .collect();
        assert_eq!(lines.len(), 2, "hello_ack + response per request");
        let frame: HeadlessResponse = serde_json::from_str(lines[1]).expect("response frame");
        match frame {
            HeadlessResponse::Response { envelope } => {
                assert!(matches!(
                    envelope.response,
                    core_api::AppResponse::Accepted { .. }
                ));
            }
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_loop_handshakes_hello() {
        let input = format!("{}\n", sample_hello_line());
        let mut output = Vec::new();
        let mut handler = EchoHandler::new();
        run_loop(
            tokio::io::BufReader::new(input.as_bytes()),
            &mut output,
            LoopConfig::default(),
            &mut handler,
        )
        .await
        .expect("run loop");
        assert_eq!(handler.handshakes, 1, "hello consumed by handshake path");
        let frame: HeadlessResponse =
            serde_json::from_str(std::str::from_utf8(&output).unwrap().trim()).expect("frame");
        match frame {
            HeadlessResponse::HelloAck {
                instance_id,
                negotiated,
                granted,
            } => {
                assert_eq!(instance_id, "test-instance");
                assert_eq!(negotiated, core_api::API_VERSION);
                assert_eq!(
                    granted,
                    vec![SdkCapability::Sessions, SdkCapability::Streaming]
                );
            }
            other => panic!("expected hello_ack, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_loop_interleaves_events_with_responses() {
        let input = format!("{}\n{}\n", sample_hello_line(), sample_command_line());
        let mut output = Vec::new();
        let mut handler = EchoHandler::new();
        handler.pending_events.push_back(HeadlessResponse::Event {
            envelope: core_api::AppEventEnvelope {
                api_version: core_api::API_VERSION,
                instance_id: agent_domain::CoreInstanceId::from("test-instance"),
                event_id: agent_domain::EventId::from("evt-1"),
                global_sequence: core_api::GlobalSequence(1),
                stream: core_api::EventStream::Global,
                stream_sequence: 1,
                timestamp: Timestamp::from_unix_millis(1),
                source: core_api::EventSource::Core,
                payload: core_api::AppEvent::CoreReady {
                    handle: core_api::ApiHandle {
                        instance_id: agent_domain::CoreInstanceId::from("test-instance"),
                        api_version: core_api::API_VERSION,
                    },
                },
            },
        });
        run_loop(
            tokio::io::BufReader::new(input.as_bytes()),
            &mut output,
            LoopConfig::default(),
            &mut handler,
        )
        .await
        .expect("run loop");
        let lines: Vec<&str> = std::str::from_utf8(&output)
            .unwrap()
            .trim_end()
            .split('\n')
            .collect();
        assert_eq!(lines.len(), 3, "hello_ack + response + event: {lines:?}");
        let frames: Vec<HeadlessResponse> = lines
            .iter()
            .map(|line| serde_json::from_str(line).expect("frame"))
            .collect();
        assert!(
            frames
                .iter()
                .any(|f| matches!(f, HeadlessResponse::HelloAck { .. })),
            "hello_ack frame present: {frames:?}"
        );
        assert!(
            frames
                .iter()
                .any(|f| matches!(f, HeadlessResponse::Response { .. })),
            "response frame present: {frames:?}"
        );
        assert!(
            frames
                .iter()
                .any(|f| matches!(f, HeadlessResponse::Event { .. })),
            "event frame present: {frames:?}"
        );
    }

    #[tokio::test]
    async fn run_loop_stops_cleanly_on_eof() {
        let mut output = Vec::new();
        let mut handler = EchoHandler::new();
        run_loop(
            tokio::io::BufReader::new(&b""[..]),
            &mut output,
            LoopConfig::default(),
            &mut handler,
        )
        .await
        .expect("empty input must exit cleanly");
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn run_loop_rejects_command_before_handshake() {
        let input = format!("{}\n", sample_command_line());
        let mut output = Vec::new();
        let mut handler = EchoHandler::new();
        run_loop(
            tokio::io::BufReader::new(input.as_bytes()),
            &mut output,
            LoopConfig::default(),
            &mut handler,
        )
        .await
        .expect("run loop");
        assert_eq!(handler.handshakes, 0, "no hello was sent");
        let lines: Vec<&str> = std::str::from_utf8(&output)
            .unwrap()
            .trim_end()
            .split('\n')
            .collect();
        assert_eq!(lines.len(), 1);
        let frame: HeadlessResponse = serde_json::from_str(lines[0]).expect("error frame");
        match frame {
            HeadlessResponse::Error {
                request_id,
                kind,
                message,
            } => {
                assert_eq!(
                    kind,
                    ProtocolErrorKind::NotHandshaked,
                    "pre-handshake request must be rejected explicitly"
                );
                assert_eq!(request_id.as_deref(), Some("cmd-1"));
                assert!(message.contains("hello"), "message: {message}");
            }
            other => panic!("expected NotHandshaked error frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_loop_translate_error_keeps_request_id() {
        // 握手后发一个翻译失败的帧（api_version 不兼容）：error 帧必须保留
        // request_id（command_id），客户端才能按 id 关联错误。
        let input = format!("{}\n{}\n", sample_hello_line(), bad_version_command_line());
        let mut output = Vec::new();
        let mut handler = EchoHandler::new();
        run_loop(
            tokio::io::BufReader::new(input.as_bytes()),
            &mut output,
            LoopConfig::default(),
            &mut handler,
        )
        .await
        .expect("run loop");
        let lines: Vec<&str> = std::str::from_utf8(&output)
            .unwrap()
            .trim_end()
            .split('\n')
            .collect();
        assert_eq!(lines.len(), 2, "hello_ack + error: {lines:?}");
        let frame: HeadlessResponse = serde_json::from_str(lines[1]).expect("error frame");
        match frame {
            HeadlessResponse::Error {
                request_id,
                kind,
                message,
            } => {
                assert_eq!(
                    kind,
                    ProtocolErrorKind::IncompatibleApiVersion,
                    "translate failure surfaces its explicit kind"
                );
                assert_eq!(
                    request_id.as_deref(),
                    Some("cmd-bad-version"),
                    "known request_id must survive translate failure"
                );
                assert!(message.contains("incompatible"), "message: {message}");
            }
            other => panic!("expected error frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_loop_produces_error_frame_for_bad_line() {
        let input = "{not json}\n";
        let mut output = Vec::new();
        let mut handler = EchoHandler::new();
        run_loop(
            tokio::io::BufReader::new(input.as_bytes()),
            &mut output,
            LoopConfig::default(),
            &mut handler,
        )
        .await
        .expect("run loop keeps going after bad line");
        let lines: Vec<&str> = std::str::from_utf8(&output)
            .unwrap()
            .trim_end()
            .split('\n')
            .collect();
        assert_eq!(lines.len(), 1);
        let frame: HeadlessResponse = serde_json::from_str(lines[0]).expect("error frame");
        assert!(
            matches!(
                frame,
                HeadlessResponse::Error {
                    kind: ProtocolErrorKind::MalformedFrame,
                    ..
                }
            ),
            "expected malformed error frame, got {frame:?}"
        );
    }

    #[tokio::test]
    async fn stdio_writer_backpressure_rejects_unflushed_burst() {
        use headless_json::stdio::StdioWriter;
        let config = LoopConfig {
            batch_mode: true,
            max_frame_bytes: 100,
        };
        let mut writer = StdioWriter::new(Vec::new(), config);
        let frame = HeadlessResponse::CompatHistoryResult {
            request_id: "ch-1".into(),
            entries: vec![],
            cursor: None,
        };
        writer.write_frame(&frame).await.expect("first frame fits");
        let error = writer
            .write_frame(&frame)
            .await
            .expect_err("second frame overflows");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        writer.flush().await.expect("flush drains pending bytes");
        assert_eq!(writer.pending_bytes(), 0);
    }

    #[tokio::test]
    async fn stream_mode_flushes_every_frame() {
        use headless_json::stdio::StdioWriter;
        let config = LoopConfig {
            batch_mode: false,
            max_frame_bytes: 1024,
        };
        let mut writer = StdioWriter::new(Vec::new(), config);
        let frame = HeadlessResponse::Error {
            request_id: None,
            kind: ProtocolErrorKind::Internal,
            message: "x".into(),
        };
        writer.write_frame(&frame).await.expect("write");
        assert_eq!(
            writer.pending_bytes(),
            0,
            "streaming mode flushes per frame"
        );
    }
}
