use crate::AppCore;
use async_trait::async_trait;
use pawork_control_plane::UsageQuery;
use pawork_domain::*;
use pawork_gateway::{serve_gateway, GatewayTokenStore};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::net::TcpListener;
use wiremock::{
    matchers::{body_partial_json, header, method, path},
    Mock, MockServer, ResponseTemplate,
};

#[tokio::test]
async fn gateway_http_routes_models_completions_and_auth_without_exposing_provider_secrets() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":[{"id":"model-a","context_length":8192}]})),
        )
        .mount(&upstream)
        .await;
    Mock::given(method("POST")).and(path("/v1/chat/completions")).and(header("authorization","Bearer upstream-test-secret"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-type","text/event-stream").set_body_string(concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"你好\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
                "data: [DONE]\n\n"
            ))).expect(2).mount(&upstream).await;
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    pawork_auth::store_default_api_key(
        backend.as_ref(),
        &ProviderId::new("test"),
        "upstream-test-secret",
    )
    .unwrap();
    let config = pawork_workspace::config::PaworkConfig {
        default_provider: Some("test".into()),
        default_model: Some("model-a".into()),
        providers: vec![pawork_workspace::config::ProviderConfig {
            id: "test".into(),
            base_url: Some(format!("{}/v1", upstream.uri())),
            ..Default::default()
        }],
        ..Default::default()
    };
    let core = Arc::new(
        AppCore::from_config(config, None, None, backend)
            .await
            .unwrap(),
    );
    let temp = tempfile::tempdir().unwrap();
    let tokens = GatewayTokenStore::new(temp.path());
    let (info, token) = tokens.issue("momai").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let cancel = CancellationToken::new();
    let server = tokio::spawn(serve_gateway(
        core.clone(),
        listener,
        tokens.clone(),
        cancel.clone(),
    ));
    let http = reqwest::Client::new();
    let models = http
        .get(format!("{base}/v1/models"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(models.status(), 200);
    let models: Value = models.json().await.unwrap();
    assert!(models["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == "test/model-a"));
    assert!(!models.to_string().contains("upstream-test-secret"));
    let request = json!({"model":"test/model-a","messages":[{"role":"system","content":"中文写作"},{"role":"user","content":"你好"}],"max_tokens":20,"response_format":{"type":"json_schema","json_schema":{"name":"answer","strict":true,"schema":{"type":"object"}}}});
    let normal = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(&token)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(normal.status(), 200);
    let normal: Value = normal.json().await.unwrap();
    assert_eq!(normal["choices"][0]["message"]["content"], "你好");
    assert_eq!(normal["usage"]["total_tokens"], 5);
    let mut streaming = request.clone();
    streaming["stream"] = json!(true);
    streaming["stream_options"] = json!({"include_usage":true});
    let response = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(&token)
        .json(&streaming)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    assert!(
        text.contains("你好")
            && text.contains("\"total_tokens\":5")
            && text.ends_with("data: [DONE]\n\n")
    );
    let upstream_requests = upstream.received_requests().await.unwrap();
    let posted = upstream_requests
        .iter()
        .find(|r| r.method == "POST")
        .unwrap();
    let posted: Value = serde_json::from_slice(&posted.body).unwrap();
    assert_eq!(posted["model"], "model-a");
    assert_eq!(posted["response_format"]["json_schema"]["strict"], true);
    let records = core
        .usage
        .control
        .ledger
        .query(&UsageQuery::by_tenant(TenantId::new("thirdparty/momai")))
        .await
        .unwrap();
    assert_eq!(records.len(), 2);
    assert!(records
        .iter()
        .all(|r| r.input_tokens == 3 && r.output_tokens == 2));
    assert!(!serde_json::to_string(&records)
        .unwrap()
        .contains("upstream-test-secret"));
    assert_eq!(
        http.get(format!("{base}/v1/models"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.get(format!("{base}/v1/models"))
            .bearer_auth(&token)
            .header("Host", "evil.test")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.get(format!("{base}/v1/models"))
            .bearer_auth(&token)
            .header("Origin", "https://evil.test")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    for invalid in [
        json!({"model":"test/missing","messages":[{"role":"user","content":"hi"}]}),
        json!({"model":"test/model-a","messages":[],"tools":[]}),
    ] {
        let response = http
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth(&token)
            .json(&invalid)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_client_error());
    }
    tokens.revoke(&info.id).unwrap();
    assert_eq!(
        http.get(format!("{base}/v1/models"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

/// ADR-064（Gateway v1.1）：用途过滤、能力位、多模态与生图透传回归。
/// 上游目录含 text 模型与图像生成模型（wan2.7-image 走 default 表升级
/// image_output 并收窄 text）；本地 mock wire 不访问真实厂商。
#[tokio::test]
async fn gateway_models_purpose_filter_and_multimodal_gates() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data":[
                {"id":"model-a","context_length":8192},
                {"id":"model-vision","supports_image_in":true},
                {"id":"wan2.7-image"}
            ]
        })))
        .mount(&upstream)
        .await;
    // wan 生图流式响应：content 数组携带 image part（qwen compatible 形状）。
    let image_chunk = r#"{"choices":[{"index":0,"delta":{"content":[{"type":"image","image":"https://gen.example/out.png"}]},"finish_reason":null}]}"#;
    let finish_chunk = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(json!({
            "model":"wan2.7-image",
            "messages":[{"role":"user","content":[
                {"type":"text","text":"a red cube on white background"}
            ]}]
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!(
                    "data: {image_chunk}\n\ndata: {finish_chunk}\n\ndata: [DONE]\n\n"
                )),
        )
        .mount(&upstream)
        .await;
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    pawork_auth::store_default_api_key(
        backend.as_ref(),
        &ProviderId::new("test"),
        "upstream-test-secret",
    )
    .unwrap();
    let config = pawork_workspace::config::PaworkConfig {
        default_provider: Some("test".into()),
        default_model: Some("model-a".into()),
        providers: vec![pawork_workspace::config::ProviderConfig {
            id: "test".into(),
            base_url: Some(format!("{}/v1", upstream.uri())),
            ..Default::default()
        }],
        ..Default::default()
    };
    let core = Arc::new(
        AppCore::from_config(config, None, None, backend)
            .await
            .unwrap(),
    );
    let temp = tempfile::tempdir().unwrap();
    let tokens = GatewayTokenStore::new(temp.path());
    let (_info, token) = tokens.issue("momai").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let cancel = CancellationToken::new();
    let _server = tokio::spawn(serve_gateway(
        core.clone(),
        listener,
        tokens.clone(),
        cancel.clone(),
    ));
    let http = reqwest::Client::new();

    // 缺省 = v1 兼容：只回 text 模型；条目带能力位。
    let models: Value = http
        .get(format!("{base}/v1/models"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert_eq!(ids, ["test/model-a", "test/model-vision"]);
    let entry = &models["data"][0];
    assert_eq!(entry["capabilities"]["text"], true);
    assert_eq!(entry["capabilities"]["image_output"], false);

    // ?purpose=image_output：只回图像生成模型，能力位如实。
    let generators: Value = http
        .get(format!("{base}/v1/models?purpose=image_output"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = generators["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert_eq!(ids, ["test/wan2.7-image"]);
    assert_eq!(generators["data"][0]["capabilities"]["image_output"], true);
    assert_eq!(generators["data"][0]["capabilities"]["text"], false);

    // 交叉过滤：text + image_output 交集为空（wan 已收窄 text）。
    let both: Value = http
        .get(format!(
            "{base}/v1/models?purpose=text&purpose=image_output"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(both["data"].as_array().unwrap().len(), 0);

    // 未知用途 / 未知参数 fail-closed。
    for query in ["purpose=bogus", "capability=image_output"] {
        assert_eq!(
            http.get(format!("{base}/v1/models?{query}"))
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }

    // 多模态输入：非视觉模型的 image_url part 走 capability_gate 400。
    let vision_request = json!({
        "model":"test/model-a",
        "messages":[{"role":"user","content":[
            {"type":"text","text":"describe"},
            {"type":"image_url","image_url":{"url":"https://example.com/a.png"}}
        ]}]
    });
    assert_eq!(
        http.post(format!("{base}/v1/chat/completions"))
            .bearer_auth(&token)
            .json(&vision_request)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    // 非法视频 URL 同样 400。
    let bad_video = json!({
        "model":"test/model-a",
        "messages":[{"role":"user","content":[
            {"type":"video_url","video_url":{"url":"ftp://example.com/a.mp4"}}
        ]}]
    });
    assert_eq!(
        http.post(format!("{base}/v1/chat/completions"))
            .bearer_auth(&token)
            .json(&bad_video)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );

    // web_search: true + 无 WebSearch 证据的模型 → 400。
    let search_request = json!({
        "model":"test/model-a",
        "messages":[{"role":"user","content":"hi"}],
        "web_search": true
    });
    assert_eq!(
        http.post(format!("{base}/v1/chat/completions"))
            .bearer_auth(&token)
            .json(&search_request)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );

    // 图像生成模型：非流式响应透传 message.images（additive）。
    let generate = json!({
        "model":"test/wan2.7-image",
        "messages":[{"role":"user","content":"a red cube on white background"}]
    });
    let completion: Value = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(&token)
        .json(&generate)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let message = &completion["choices"][0]["message"];
    assert_eq!(message["content"], "");
    assert_eq!(message["images"][0]["url"], "https://gen.example/out.png");
    assert_eq!(completion["choices"][0]["finish_reason"], "stop");
    // 常规内嵌图片可以超过 8 KiB；上游收到完整 data URL，仍受 2 MiB body 闸门约束。
    let image_url = format!("data:image/png;base64,{}", "AAAA".repeat(4096));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(json!({
            "model":"model-vision",
            "messages":[{"role":"user","content":[
                {"type":"image_url","image_url":{"url":image_url}}
            ]}]
        })))
        .respond_with(ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"image received\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"))
        .expect(1)
        .mount(&upstream)
        .await;
    let vision: Value = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(&token)
        .json(
            &json!({"model":"test/model-vision","messages":[{"role":"user","content":[
                {"type":"image_url","image_url":{"url":image_url}}
            ]}]}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(vision["choices"][0]["message"]["content"], "image received");
    cancel.cancel();
}

struct WaitingProvider;
#[async_trait]
impl ModelProvider for WaitingProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("mock")
    }
    async fn list_models(
        &self,
        _: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        Ok(vec![ModelDefinition {
            id: ModelId::new("wait"),
            display_name: "Wait".into(),
            context_window_tokens: 8192,
            max_output_tokens: 2048,
            capabilities: pawork_domain::ModelCapabilities {
                text: true,
                ..Default::default()
            },
        }])
    }
    async fn stream(
        &self,
        request: &CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        sink.emit(ProviderStreamEvent::UsageUpdated(TokenUsage {
            input_tokens: 7,
            ..Default::default()
        }))
        .await?;
        sink.emit(ProviderStreamEvent::TextDelta("started".into()))
            .await?;
        if request
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .any(|p| matches!(p, ContentPart::Text(t) if t.text == "timeout"))
        {
            cancel.cancel();
            return Err(ProviderError::new(
                ProviderErrorKind::Timeout,
                "test timeout",
            ));
        }
        cancel.cancelled().await;
        Err(ProviderError::cancelled("cancelled"))
    }
}
#[tokio::test]
async fn gateway_disconnect_cancels_upstream_and_records_partial_usage() {
    let core = Arc::new(AppCore::from_parts(
        Arc::new(WaitingProvider),
        None,
        ModelId::new("wait"),
        ProviderId::new("mock"),
        None,
    ));
    let temp = tempfile::tempdir().unwrap();
    let tokens = GatewayTokenStore::new(temp.path());
    let (_, token) = tokens.issue("editor").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let cancel = CancellationToken::new();
    let server = tokio::spawn(serve_gateway(
        core.clone(),
        listener,
        tokens,
        cancel.clone(),
    ));
    let http = reqwest::Client::new();
    let timeout_response = http.post(format!("{base}/v1/chat/completions"))
            .bearer_auth(&token)
            .json(&json!({"model":"mock/wait","stream":true,"messages":[{"role":"user","content":"timeout"}]}))
            .send().await.unwrap().text().await.unwrap();
    assert!(timeout_response.contains("\"code\":\"timeout\""));
    assert!(timeout_response.ends_with("data: [DONE]\n\n"));
    let mut response=http.post(format!("{base}/v1/chat/completions")).bearer_auth(&token).json(&json!({"model":"mock/wait","stream":true,"messages":[{"role":"user","content":"wait"}]})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(String::from_utf8_lossy(&response.chunk().await.unwrap().unwrap()).contains("started"));
    drop(response);
    drop(http);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let records = core
                .usage
                .control
                .ledger
                .query(&UsageQuery::by_tenant(TenantId::new("thirdparty/editor")))
                .await
                .unwrap();
            if records.len() == 2 {
                assert_eq!(records[0].input_tokens, 7);
                assert_eq!(
                    core.usage.control.pool.active_count_for(
                        &TenantId::new("thirdparty/editor"),
                        &AccountId::new("mock/environment")
                    ),
                    0
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("disconnect must stop and account upstream");
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
