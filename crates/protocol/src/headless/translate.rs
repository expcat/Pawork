//! JSONL 翻译层：请求行 → 信封/协议入口，响应与事件 → 输出行。
//!
//! 本模块是纯函数翻译，不包含任何业务决策；Host 接线层拿到
//! [`TranslatedRequest`] 后自行分发。所有错误都是显式的 [`HeadlessError`]，
//! unknown / unsupported 有独立类别。

use super::wire::{
    CompatHistoryQuery, CompatImportRequest, HeadlessError, HeadlessRequest, HeadlessResponse,
    ProtocolErrorKind, TranslatedRequest, MAX_FRAME_BYTES,
};
use crate::{AppEventEnvelope, AppResponseEnvelope};
use serde::Serialize;
use serde_json::Value;

/// 解析并翻译一行请求 JSON（不含换行）。
///
/// 失败返回带类别的 [`HeadlessError`]；调用方应把它编码为
/// [`HeadlessResponse::Error`] 帧返回给客户端（见 [`encode_protocol_response`]）。
pub fn translate_request_line(line: &str) -> Result<TranslatedRequest, HeadlessError> {
    let request = parse_request_line(line)?;
    translate_request(&request)
}

/// 解析一行请求 JSON 为 [`HeadlessRequest`]（不含换行）。
///
/// 未知 `type` 给出显式 [`ProtocolErrorKind::UnknownRequestType`] 错误。
/// 运行循环先解析再分发：`hello` 帧走握手路径，其余帧交给
/// [`translate_request`]。
pub fn parse_request_line(line: &str) -> Result<HeadlessRequest, HeadlessError> {
    if line.len() > MAX_FRAME_BYTES {
        return Err(HeadlessError::too_large(
            "request line exceeds MAX_FRAME_BYTES",
        ));
    }
    // 先识别 `type` 判别字段：未知类型给出显式 UnknownRequestType 错误，
    // 而不是笼统的 malformed（unknown/unsupported 必须可区分）。
    let frame: Value = serde_json::from_str(line)
        .map_err(|error| HeadlessError::malformed(format!("request frame: {error}")))?;
    let frame_type = frame
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    const KNOWN_TYPES: &[&str] = &[
        "hello",
        "command",
        "query",
        "compat_import",
        "compat_history",
    ];
    if !KNOWN_TYPES.contains(&frame_type) {
        return Err(HeadlessError::unknown_request(format!(
            "unknown request frame type `{frame_type}`"
        )));
    }
    serde_json::from_value(frame)
        .map_err(|error| HeadlessError::malformed(format!("request frame: {error}")))
}

/// 把请求帧翻译为可分发切片。
pub fn translate_request(request: &HeadlessRequest) -> Result<TranslatedRequest, HeadlessError> {
    match request {
        HeadlessRequest::Hello { .. } => Err(HeadlessError::new(
            ProtocolErrorKind::MalformedFrame,
            "hello is a handshake frame and must be consumed by the host's handshake path, \
             not dispatched",
        )),
        HeadlessRequest::Command { envelope } => {
            validate_envelope_version(envelope.api_version)?;
            Ok(TranslatedRequest::Command(envelope.clone()))
        }
        HeadlessRequest::Query { envelope } => {
            validate_envelope_version(envelope.api_version)?;
            Ok(TranslatedRequest::Query(envelope.clone()))
        }
        HeadlessRequest::CompatImport {
            request_id,
            source,
            content,
            options,
        } => Ok(TranslatedRequest::CompatImport(CompatImportRequest {
            request_id: request_id.clone(),
            source: *source,
            content: content.clone(),
            options: options.clone().unwrap_or_default(),
        })),
        HeadlessRequest::CompatHistory {
            request_id,
            limit,
            cursor,
        } => Ok(TranslatedRequest::CompatHistory(CompatHistoryQuery {
            request_id: request_id.clone(),
            limit: *limit,
            cursor: cursor.clone(),
        })),
    }
}

/// 校验信封 api_version 与协议当前版本 major 兼容（只做 framing 层校验，
/// 最终裁决由 Host 执行）。
fn validate_envelope_version(version: crate::ApiVersion) -> Result<(), HeadlessError> {
    if version.is_compatible_with(crate::API_VERSION) {
        Ok(())
    } else {
        Err(HeadlessError::new(
            ProtocolErrorKind::IncompatibleApiVersion,
            format!(
                "envelope api version {version:?} is incompatible with {:?}",
                crate::API_VERSION
            ),
        ))
    }
}

/// 编码响应信封为一行 JSON（Host 收到命令/查询结果后调用）。
pub fn encode_response_line(response: &AppResponseEnvelope) -> Result<String, HeadlessError> {
    encode(&HeadlessResponse::Response {
        envelope: response.clone(),
    })
}

/// 编码事件信封为一行 JSON（Host 事件流出口）。
pub fn encode_event_line(event: &AppEventEnvelope) -> Result<String, HeadlessError> {
    encode(&HeadlessResponse::Event {
        envelope: event.clone(),
    })
}

/// 编码任意协议响应帧为一行 JSON。
pub fn encode_protocol_response(response: &HeadlessResponse) -> Result<String, HeadlessError> {
    encode(response)
}

/// 编码请求帧为一行 JSON（客户端侧 / 测试用）。
pub fn encode_request(request: &HeadlessRequest) -> Result<String, HeadlessError> {
    encode(request)
}

/// 构造 `error` 帧（未知/不支持/格式错误等的显式响应）。
pub fn error_frame(
    request_id: Option<String>,
    kind: ProtocolErrorKind,
    message: impl Into<String>,
) -> HeadlessResponse {
    HeadlessResponse::Error {
        request_id,
        kind,
        message: message.into(),
    }
}

fn encode<T: Serialize>(value: &T) -> Result<String, HeadlessError> {
    let line = serde_json::to_string(value)
        .map_err(|error| HeadlessError::malformed(format!("encode frame: {error}")))?;
    if line.len() > MAX_FRAME_BYTES {
        return Err(HeadlessError::too_large(
            "encoded frame exceeds MAX_FRAME_BYTES",
        ));
    }
    Ok(line)
}

/// 把任意 JSON 值规范化为单行文本（fixture 比较辅助；不参与线上路径）。
pub fn canonical_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}
