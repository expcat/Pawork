use std::collections::BTreeMap;

use pawork_api::{
    CanonicalModelRequest, PromptCachePreference, RequestBudget, ResponseFormat, ToolChoice,
    ToolDefinition,
};
use pawork_domain::{
    ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId, TextContent,
};
use pawork_providers::responses::{to_responses_body, ResponsesWireOptions};

#[test]
fn responses_body_preserves_canonical_tools_and_blocks_reserved_overrides() {
    let mut request = CanonicalModelRequest {
        request_id: pawork_domain::RequestId::new("r1"),
        model: ModelId::new("test-model"),
        messages: vec![Message {
            id: MessageId::new("m1"), role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: "hi".into() })],
            metadata: MessageMetadata::default(),
        }],
        tools: vec![ToolDefinition { name: "read".into(), description: "read".into(), input_schema: serde_json::json!({"type":"object"}) }],
        hosted_tools: Vec::new(), extensions: Vec::new(), tool_choice: ToolChoice::Auto,
        thinking: None, reasoning: None, temperature: None, max_output_tokens: Some(64),
        stop_sequences: Vec::new(), response_format: ResponseFormat::Text,
        prompt_cache: PromptCachePreference::Automatic, budget: RequestBudget::default(),
        provider_options: BTreeMap::from([
            ("model".into(), serde_json::json!("attacker")),
            ("top_p".into(), serde_json::json!(0.8)),
        ]), trace_id: None,
    };
    let body = to_responses_body(
        &request,
        Vec::new(),
        ResponsesWireOptions { store: Some(false), include_encrypted_reasoning: true },
    );
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["input"][0]["content"][0]["text"], "hi");
    assert_eq!(body["tools"][0]["name"], "read");
    assert_eq!(body["store"], false);
    assert_eq!(body["top_p"], 0.8);

    request.provider_options.insert("authorization".into(), serde_json::json!("secret"));
    let body = to_responses_body(&request, Vec::new(), ResponsesWireOptions::default());
    assert!(body.get("authorization").is_none());
}
