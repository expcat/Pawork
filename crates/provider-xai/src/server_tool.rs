//! xAI Responses wire → canonical Server Tool / Citation 映射（P15-5 夹具）。
//!
//! 这里只冻结 P15-4 将消费的 wire 字段，不把 Responses API 接入现有
//! Chat Completions transport，也不执行任何 server-side tool。

use agent_domain::{Citation, CitationSourceKind, ServerToolEvent, ToolCallId};
use provider_api::ServerToolMappingError;
use serde_json::Value;

/// 把 xAI Responses 的 server-side output item 归一为一个生命周期事件。
pub fn response_item_to_server_tool_event(
    item: &Value,
) -> Result<ServerToolEvent, ServerToolMappingError> {
    let item_type = required_str(item, "type", "item without `type`")?;
    let name = match item_type {
        "web_search_call" => "web_search",
        "x_search_call" => "x_search",
        "code_interpreter_call" => "code_interpreter",
        "file_search_call" => "file_search",
        "mcp_call" => "mcp",
        other => {
            return Err(ServerToolMappingError::unsupported(format!(
                "unmapped xAI Responses item type `{other}`"
            )))
        }
    };
    let id = ToolCallId::from(required_str(item, "id", "server tool item without `id`")?);
    let status = required_str(item, "status", "server tool item without `status`")?;
    match status {
        "in_progress" | "searching" => Ok(ServerToolEvent::Started {
            tool_call_id: id,
            name: name.into(),
            arguments: item
                .get("action")
                .or_else(|| item.get("arguments"))
                .cloned(),
        }),
        "completed" => Ok(ServerToolEvent::Completed {
            tool_call_id: id,
            summary: None,
            artifacts: Vec::new(),
        }),
        "failed" => {
            let error = item.get("error");
            Ok(ServerToolEvent::Failed {
                tool_call_id: id,
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
            "unmapped xAI server tool status `{other}`"
        ))),
    }
}

/// 把 response 顶层 `citations[]` 的单个 URL 归一为 [`Citation`]。
pub fn citation_url_to_citation(value: &Value) -> Result<Citation, ServerToolMappingError> {
    let url = value
        .as_str()
        .ok_or_else(|| ServerToolMappingError::unsupported("citation entry is not a URL string"))?;
    Ok(Citation {
        url: Some(url.to_owned()),
        source_kind: CitationSourceKind::Url,
        ..Citation::empty()
    })
}

/// 把 Responses `output_text.annotations[].url_citation` 归一为 [`Citation`]。
pub fn url_citation_annotation_to_citation(
    annotation: &Value,
) -> Result<Citation, ServerToolMappingError> {
    let annotation_type = required_str(annotation, "type", "annotation without `type`")?;
    if annotation_type != "url_citation" {
        return Err(ServerToolMappingError::unsupported(format!(
            "unmapped xAI annotation type `{annotation_type}`"
        )));
    }
    Ok(Citation {
        index: annotation.get("start_index").and_then(Value::as_u64),
        url: annotation
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned),
        title: annotation
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_server_side_item_types_map_without_provider_names() {
        for (item_type, expected_name) in [
            ("web_search_call", "web_search"),
            ("x_search_call", "x_search"),
            ("code_interpreter_call", "code_interpreter"),
            ("file_search_call", "file_search"),
            ("mcp_call", "mcp"),
        ] {
            let event = response_item_to_server_tool_event(&serde_json::json!({
                "type": item_type,
                "id": format!("{expected_name}-1"),
                "status": "in_progress",
                "arguments": {"query": "pawork"}
            }))
            .expect("map item");
            assert!(matches!(
                event,
                ServerToolEvent::Started { name, .. } if name == expected_name
            ));
        }
    }

    #[test]
    fn top_level_citation_urls_do_not_guess_metadata() {
        let citation = citation_url_to_citation(&Value::String(
            "collections://collection-1/document-1".into(),
        ))
        .expect("map URL");
        assert_eq!(
            citation.url.as_deref(),
            Some("collections://collection-1/document-1")
        );
        assert!(citation.title.is_none());
        assert!(citation.snippet.is_none());
        assert_eq!(citation.source_kind, CitationSourceKind::Url);
    }

    #[test]
    fn structured_inline_annotation_maps_position_and_label() {
        let citation = url_citation_annotation_to_citation(&serde_json::json!({
            "type": "url_citation",
            "url": "https://x.ai/news",
            "title": "1",
            "start_index": 37,
            "end_index": 76
        }))
        .expect("map annotation");
        assert_eq!(citation.url.as_deref(), Some("https://x.ai/news"));
        assert_eq!(citation.title.as_deref(), Some("1"));
        assert_eq!(citation.index, Some(37));
    }

    #[test]
    fn local_function_call_and_non_string_citation_are_unsupported() {
        let error = response_item_to_server_tool_event(&serde_json::json!({
            "type": "function_call",
            "id": "fc_1",
            "status": "completed"
        }))
        .expect_err("local function call must not map to server tool");
        assert!(error.to_string().contains("function_call"));

        let error = citation_url_to_citation(&serde_json::json!({"url": "https://x.ai"}))
            .expect_err("top-level citations use URL strings");
        assert!(error.to_string().contains("URL string"));
    }
}
