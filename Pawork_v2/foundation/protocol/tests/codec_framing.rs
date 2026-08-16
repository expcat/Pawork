//! 长度前缀分帧（u32 LE）与帧读写 API（L1 定向测试）。

use std::io::Cursor;

use pawork_domain::{CoreInstanceId, Timestamp};
use pawork_protocol::{GlobalSequence, API_VERSION};
use pawork_protocol::{
    decode_length_prefixed, encode_client_frame, encode_length_prefixed, read_client_frame,
    read_frame, read_server_frame, write_client_frame, write_frame, write_server_frame,
    ClientFrame, HandshakeRequest, ProtocolCodecError, ServerFrame, Snapshot, SnapshotSection,
    SnapshotSectionKind, FRAME_LENGTH_PREFIX_BYTES, MAX_PROTOCOL_FRAME_BYTES,
};
use serde_json::json;

fn handshake_frame() -> ClientFrame {
    ClientFrame::Handshake(HandshakeRequest {
        request_id: "request-1".into(),
        client_name: "desktop".into(),
        client_version: "0.1.0".into(),
        supported_api_versions: vec![API_VERSION],
        capabilities: vec![],
        authentication: None,
    })
}

#[test]
fn length_prefix_is_u32_little_endian() {
    let mut writer = Cursor::new(Vec::new());
    let payload = b"hello";
    write_frame(&mut writer, payload).expect("write frame");
    let bytes = writer.into_inner();
    assert_eq!(bytes.len(), FRAME_LENGTH_PREFIX_BYTES + payload.len());
    assert_eq!(&bytes[..FRAME_LENGTH_PREFIX_BYTES], &5u32.to_le_bytes());
    assert_eq!(&bytes[FRAME_LENGTH_PREFIX_BYTES..], payload);
}

#[test]
fn write_and_read_frame_round_trip() {
    let mut writer = Cursor::new(Vec::new());
    write_frame(&mut writer, b"payload-1").expect("write frame 1");
    write_frame(&mut writer, b"payload-2").expect("write frame 2");

    let mut reader = Cursor::new(writer.into_inner());
    assert_eq!(read_frame(&mut reader).expect("read frame 1"), b"payload-1");
    assert_eq!(read_frame(&mut reader).expect("read frame 2"), b"payload-2");
}

#[test]
fn typed_frame_read_write_round_trip() {
    let mut writer = Cursor::new(Vec::new());
    write_client_frame(&mut writer, &handshake_frame()).expect("write client frame");
    write_server_frame(&mut writer, &ServerFrame::Pong { nonce: 1 }).expect("write server frame");

    let mut reader = Cursor::new(writer.into_inner());
    assert_eq!(
        read_client_frame(&mut reader).expect("read client frame"),
        handshake_frame()
    );
    assert_eq!(
        read_server_frame(&mut reader).expect("read server frame"),
        ServerFrame::Pong { nonce: 1 }
    );
}

#[test]
fn declared_length_over_limit_is_rejected_before_read() {
    // 只提供 4 字节帧头，声明长度超过上限；read_frame 必须在分配前拒绝。
    let mut reader = Cursor::new((MAX_PROTOCOL_FRAME_BYTES as u32 + 1).to_le_bytes().to_vec());
    assert!(matches!(
        read_frame(&mut reader),
        Err(ProtocolCodecError::FrameTooLarge { .. })
    ));
}

#[test]
fn truncated_frame_reports_io_error() {
    let mut reader = Cursor::new(Vec::new());
    assert!(matches!(
        read_frame(&mut reader),
        Err(ProtocolCodecError::Io(_))
    ));

    let mut reader = Cursor::new(3u32.to_le_bytes().to_vec());
    assert!(matches!(
        read_frame(&mut reader),
        Err(ProtocolCodecError::Io(_))
    ));
}

#[test]
fn length_prefix_mismatch_is_rejected() {
    let payload = encode_client_frame(&handshake_frame()).expect("encode payload");
    let mut framed = payload.clone();
    framed.splice(
        ..FRAME_LENGTH_PREFIX_BYTES,
        (payload.len() as u32 + 1).to_le_bytes(),
    );
    assert!(matches!(
        decode_length_prefixed::<ClientFrame>(&framed),
        Err(ProtocolCodecError::FrameLengthMismatch { declared, actual })
            if declared as usize == payload.len() + 1
                && actual == payload.len() - FRAME_LENGTH_PREFIX_BYTES
    ));
}

#[test]
fn fewer_than_four_bytes_is_truncated() {
    assert!(matches!(
        decode_length_prefixed::<ClientFrame>(&[0, 1, 2]),
        Err(ProtocolCodecError::TruncatedFrame)
    ));
}

#[test]
fn encode_and_decode_length_prefixed_round_trip() {
    let frame = handshake_frame();
    let framed = encode_length_prefixed(&frame).expect("encode length prefixed");
    assert_eq!(
        framed.len(),
        FRAME_LENGTH_PREFIX_BYTES + encode_client_frame(&frame).unwrap().len()
    );
    let decoded: ClientFrame = decode_length_prefixed(&framed).expect("decode length prefixed");
    assert_eq!(decoded, frame);
}

#[test]
fn oversized_payload_is_rejected_by_write_frame() {
    let payload = vec![b' '; MAX_PROTOCOL_FRAME_BYTES + 1];
    let mut writer = Cursor::new(Vec::new());
    assert!(matches!(
        write_frame(&mut writer, &payload),
        Err(ProtocolCodecError::FrameTooLarge { .. })
    ));
}

#[test]
fn encode_length_prefixed_rejects_oversized_value() {
    let snapshot = Snapshot {
        instance_id: CoreInstanceId::from("instance-1"),
        snapshot_sequence: GlobalSequence(42),
        generated_at: Timestamp::from_unix_millis(1),
        sections: vec![SnapshotSection {
            kind: SnapshotSectionKind::ActiveRuns,
            revision: 3,
            data: Some(json!({"blob": "x".repeat(MAX_PROTOCOL_FRAME_BYTES)})),
            artifact_id: None,
        }],
    };
    assert!(matches!(
        encode_length_prefixed(&snapshot),
        Err(ProtocolCodecError::FrameTooLarge { .. })
    ));
}
