//! Anthropic Messages wire → canonical Server Tool / Citation 映射（P15-5 夹具）。
//!
//! 本模块只冻结 P15-3 将消费的字段映射。server-executed tools 使用
//! `server_tool_use`，不能与客户端 `tool_use` 混淆；无法无损映射的定位口径返回
//! [`ServerToolMappingError`]。

use agent_domain::{Citation, CitationSourceKind, ServerToolEvent, Source, ToolCallId};
use provider_api::ServerToolMappingError;
use serde_json::Value;

/// 把 citation object 归一为 [`Citation`]。
pub fn citation_block_to_citation(block: &Value) -> Result<Citation, ServerToolMappingError> {
    let citation_type = required_str(block, "type", "citation without `type`")?;
    match citation_type {
        "char_location" => Ok(Citation {
            text: optional_string(block, "cited_text"),
            document_index: block.get("document_index").and_then(Value::as_u64),
            source_kind: CitationSourceKind::Document,
            ..Citation::empty()
        }),
        "web_search_result_location" => Ok(Citation {
            url: optional_string(block, "url"),
            title: optional_string(block, "title"),
            text: optional_string(block, "cited_text"),
            source_kind: CitationSourceKind::WebSearch,
            ..Citation::empty()
        }),
        other => Err(ServerToolMappingError::unsupported(format!(
            "unmapped citation type `{other}`"
        ))),
    }
}

/// 把 `server_tool_use` block 归一为 [`ServerToolEvent::Started`]。
///
/// `name` 必须来自请求中声明的 server tool 白名单；普通 `tool_use` 即便同名也
/// 始终属于客户端函数，不能进入 Provider transcript 通道。
pub fn server_tool_use_to_started(
    block: &Value,
    server_tool_names: &[&str],
) -> Result<ServerToolEvent, ServerToolMappingError> {
    let block_type = required_str(block, "type", "block without `type`")?;
    if block_type != "server_tool_use" {
        return Err(ServerToolMappingError::unsupported(format!(
            "unmapped block type `{block_type}`"
        )));
    }
    let id = required_str(block, "id", "server_tool_use without `id`")?;
    let name = required_str(block, "name", "server_tool_use without `name`")?;
    if !server_tool_names.contains(&name) {
        return Err(ServerToolMappingError::unsupported(format!(
            "`{name}` is not a declared server tool"
        )));
    }
    Ok(ServerToolEvent::Started {
        tool_call_id: ToolCallId::from(id),
        name: name.to_owned(),
        arguments: block.get("input").cloned(),
    })
}

/// 把 `web_search_result` 归一为 [`Source`]。
pub fn web_search_result_to_source(result: &Value) -> Result<Source, ServerToolMappingError> {
    let result_type = required_str(result, "type", "web search result without `type`")?;
    if result_type != "web_search_result" {
        return Err(ServerToolMappingError::unsupported(format!(
            "unmapped web search result type `{result_type}`"
        )));
    }
    let url = required_str(result, "url", "web_search_result without `url`")?;
    Ok(Source {
        url: Some(url.to_owned()),
        title: optional_string(result, "title"),
        raw_metadata: Some(result.clone()),
        ..Source::default()
    })
}

/// 把完整 `web_search_tool_result` block 归一为按序生命周期事件。
///
/// result 通过 `tool_use_id` 与先前的 `server_tool_use.id` 配对。成功内容先逐项
/// 产生 `SourceAdded`，再产生 `Completed`；错误内容只产生 `Failed`。
pub fn web_search_tool_result_to_events(
    block: &Value,
) -> Result<Vec<ServerToolEvent>, ServerToolMappingError> {
    let block_type = required_str(block, "type", "result block without `type`")?;
    if block_type != "web_search_tool_result" {
        return Err(ServerToolMappingError::unsupported(format!(
            "unmapped result block type `{block_type}`"
        )));
    }
    let id = ToolCallId::from(required_str(
        block,
        "tool_use_id",
        "web_search_tool_result without `tool_use_id`",
    )?);
    let content = block
        .get("content")
        .ok_or_else(|| ServerToolMappingError::unsupported("result block without `content`"))?;

    if let Some(error) = content.as_object() {
        let error_type = error
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if error_type != "web_search_tool_result_error" {
            return Err(ServerToolMappingError::unsupported(format!(
                "unmapped web search result content type `{error_type}`"
            )));
        }
        return Ok(vec![ServerToolEvent::Failed {
            tool_call_id: id,
            message: None,
            code: error
                .get("error_code")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }]);
    }

    let results = content.as_array().ok_or_else(|| {
        ServerToolMappingError::unsupported("web search result content is not an array or error")
    })?;
    let mut events = Vec::with_capacity(results.len() + 1);
    for result in results {
        events.push(ServerToolEvent::SourceAdded {
            tool_call_id: id.clone(),
            source: web_search_result_to_source(result)?,
        });
    }
    events.push(ServerToolEvent::Completed {
        tool_call_id: id,
        summary: None,
        artifacts: Vec::new(),
    });
    Ok(events)
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
    fn supported_citation_locations_map_only_representable_fields() {
        let document = citation_block_to_citation(&serde_json::json!({
            "type": "char_location",
            "cited_text": "Pawork 文档",
            "document_index": 2,
            "start_char_index": 10,
            "end_char_index": 24
        }))
        .expect("map document citation");
        assert_eq!(document.text.as_deref(), Some("Pawork 文档"));
        assert_eq!(document.document_index, Some(2));
        assert_eq!(document.source_kind, CitationSourceKind::Document);

        let web = citation_block_to_citation(&serde_json::json!({
            "type": "web_search_result_location",
            "cited_text": "Pawork",
            "url": "https://pawork.dev",
            "title": "Pawork"
        }))
        .expect("map web citation");
        assert_eq!(web.url.as_deref(), Some("https://pawork.dev"));
        assert_eq!(web.title.as_deref(), Some("Pawork"));
        assert_eq!(web.source_kind, CitationSourceKind::WebSearch);
    }

    #[test]
    fn server_tool_use_block_maps_to_started() {
        let block = serde_json::json!({
            "type": "server_tool_use",
            "id": "srvtoolu_1",
            "name": "web_search",
            "input": {"query": "pawork"}
        });
        let event = server_tool_use_to_started(&block, &["web_search"]).expect("map block");
        assert!(matches!(
            event,
            ServerToolEvent::Started {
                tool_call_id,
                name,
                arguments: Some(arguments),
            } if tool_call_id.as_str() == "srvtoolu_1"
                && name == "web_search"
                && arguments["query"] == "pawork"
        ));
    }

    #[test]
    fn search_result_sources_precede_completed_event() {
        let events = web_search_tool_result_to_events(&serde_json::json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_1",
            "content": [{
                "type": "web_search_result",
                "title": "Pawork",
                "url": "https://pawork.dev",
                "encrypted_content": "opaque"
            }]
        }))
        .expect("map result");
        assert!(matches!(
            &events[0],
            ServerToolEvent::SourceAdded { tool_call_id, source }
                if tool_call_id.as_str() == "srvtoolu_1"
                    && source.url.as_deref() == Some("https://pawork.dev")
        ));
        assert!(matches!(
            &events[1],
            ServerToolEvent::Completed { tool_call_id, .. }
                if tool_call_id.as_str() == "srvtoolu_1"
        ));
    }

    #[test]
    fn search_result_error_object_maps_to_failed() {
        let events = web_search_tool_result_to_events(&serde_json::json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_2",
            "content": {
                "type": "web_search_tool_result_error",
                "error_code": "max_uses_exceeded"
            }
        }))
        .expect("map result error");
        assert!(matches!(
            events.as_slice(),
            [ServerToolEvent::Failed {
                tool_call_id,
                message: None,
                code: Some(code),
            }] if tool_call_id.as_str() == "srvtoolu_2" && code == "max_uses_exceeded"
        ));
    }

    #[test]
    fn client_tool_use_and_unrepresentable_citations_are_unsupported() {
        let error = server_tool_use_to_started(
            &serde_json::json!({
                "type": "tool_use",
                "id": "toolu_1",
                "name": "web_search",
                "input": {}
            }),
            &["web_search"],
        )
        .expect_err("client tool must not map to server tool");
        assert!(error.to_string().contains("tool_use"));

        for citation_type in ["page_location", "content_block_location", "custom"] {
            let error = citation_block_to_citation(&serde_json::json!({"type": citation_type}))
                .expect_err("must not guess citation fields");
            assert!(error.to_string().contains(citation_type));
        }
    }
}
