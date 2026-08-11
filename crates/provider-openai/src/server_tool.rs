//! OpenAI Responses wire → canonical Server Tool / Citation 映射（P15-5 夹具）。
//!
//! 本模块只冻结 P15-2 将消费的字段映射，不接入 Chat Completions adapter，也不
//! 执行任何 hosted tool。无法无损映射的 wire 口径统一返回
//! [`ServerToolMappingError`]。

use agent_domain::{Citation, CitationSourceKind, ServerToolEvent, Source, ToolCallId};
use provider_api::ServerToolMappingError;
use serde_json::Value;

/// 把 Responses API 的 `web_search_call` output item 归一为一个生命周期事件。
///
/// OpenAI 用同一个 item type 携带 `status`；不存在
/// `web_search_call_completed` output item。`action` 是服务端执行参数，而 sources
/// 需要调用方显式请求 `include: ["web_search_call.action.sources"]`。
pub fn response_item_to_server_tool_event(
    item: &Value,
) -> Result<ServerToolEvent, ServerToolMappingError> {
    let item_type = required_str(item, "type", "item without `type`")?;
    if item_type != "web_search_call" {
        return Err(ServerToolMappingError::unsupported(format!(
            "unmapped Responses item type `{item_type}`"
        )));
    }

    let id = required_str(item, "id", "web_search_call without `id`")?;
    let status = required_str(item, "status", "web_search_call without `status`")?;
    let tool_call_id = ToolCallId::from(id);
    match status {
        "in_progress" | "searching" => Ok(ServerToolEvent::Started {
            tool_call_id,
            name: "web_search".into(),
            arguments: item.get("action").cloned(),
        }),
        "completed" => Ok(ServerToolEvent::Completed {
            tool_call_id,
            summary: None,
            artifacts: Vec::new(),
        }),
        "failed" => {
            let error = item.get("error");
            Ok(ServerToolEvent::Failed {
                tool_call_id,
                message: error
                    .and_then(|value| value.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                code: error
                    .and_then(|value| value.get("code"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        }
        other => Err(ServerToolMappingError::unsupported(format!(
            "unmapped web_search_call status `{other}`"
        ))),
    }
}

/// 把 `web_search_call.action.sources[]` 元素归一为 [`Source`]。
pub fn web_search_source_to_source(source: &Value) -> Result<Source, ServerToolMappingError> {
    let url = required_str(source, "url", "web search source without `url`")?;
    Ok(Source {
        url: Some(url.to_owned()),
        title: optional_string(source, "title"),
        snippet: optional_string(source, "snippet"),
        raw_metadata: Some(source.clone()),
        ..Source::default()
    })
}

/// 把 Responses `output_text.annotations[].url_citation` 归一为 [`Citation`]。
pub fn url_citation_annotation_to_citation(
    annotation: &Value,
) -> Result<Citation, ServerToolMappingError> {
    let annotation_type = required_str(annotation, "type", "annotation without `type`")?;
    if annotation_type != "url_citation" {
        return Err(ServerToolMappingError::unsupported(format!(
            "unmapped annotation type `{annotation_type}`"
        )));
    }
    Ok(Citation {
        url: optional_string(annotation, "url"),
        title: optional_string(annotation, "title"),
        index: annotation.get("start_index").and_then(Value::as_u64),
        source_kind: CitationSourceKind::Url,
        ..Citation::empty()
    })
}

fn required_str<'a>(
    value: &'a Value,
    key: &str,
    error: &str,
) -> Result<&'a str, ServerToolMappingError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ServerToolMappingError::unsupported(error))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_progress_web_search_maps_action_to_started_arguments() {
        let item = serde_json::json!({
            "type": "web_search_call",
            "id": "ws_1",
            "status": "in_progress",
            "action": {"type": "search", "query": "pawork"}
        });
        let event = response_item_to_server_tool_event(&item).expect("map item");
        assert!(matches!(
            event,
            ServerToolEvent::Started {
                tool_call_id,
                name,
                arguments: Some(arguments),
            } if tool_call_id.as_str() == "ws_1"
                && name == "web_search"
                && arguments["type"] == "search"
                && arguments["query"] == "pawork"
        ));
    }

    #[test]
    fn completed_status_on_same_item_type_maps_to_completed() {
        let item = serde_json::json!({
            "type": "web_search_call",
            "id": "ws_1",
            "status": "completed",
            "action": {"type": "search", "query": "pawork"}
        });
        assert!(matches!(
            response_item_to_server_tool_event(&item).expect("map item"),
            ServerToolEvent::Completed { tool_call_id, summary: None, artifacts }
                if tool_call_id.as_str() == "ws_1" && artifacts.is_empty()
        ));
    }

    #[test]
    fn included_action_source_maps_without_guessing_missing_fields() {
        let source = serde_json::json!({
            "type": "url",
            "url": "https://pawork.dev",
            "title": "Pawork",
            "extra": {"api_key": "redacted-at-store-boundary"}
        });
        let mapped = web_search_source_to_source(&source).expect("map source");
        assert_eq!(mapped.url.as_deref(), Some("https://pawork.dev"));
        assert_eq!(mapped.title.as_deref(), Some("Pawork"));
        assert!(mapped.snippet.is_none());
        assert_eq!(mapped.raw_metadata, Some(source));
    }

    #[test]
    fn url_citation_annotation_maps_flat_responses_fields() {
        let annotation = serde_json::json!({
            "type": "url_citation",
            "url": "https://example.com",
            "title": "Example",
            "start_index": 12,
            "end_index": 42
        });
        let citation = url_citation_annotation_to_citation(&annotation).expect("map annotation");
        assert_eq!(citation.url.as_deref(), Some("https://example.com"));
        assert_eq!(citation.title.as_deref(), Some("Example"));
        assert_eq!(citation.index, Some(12));
        assert_eq!(citation.source_kind, CitationSourceKind::Url);
        assert!(citation.text.is_none());
    }

    #[test]
    fn invented_or_client_item_types_are_unsupported() {
        for item_type in ["web_search_call_completed", "function_call"] {
            let error = response_item_to_server_tool_event(&serde_json::json!({
                "type": item_type,
                "id": "call_1",
                "status": "completed"
            }))
            .expect_err("must not guess an item mapping");
            assert!(error.to_string().contains(item_type));
        }
    }
}
