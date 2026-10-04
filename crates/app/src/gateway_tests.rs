use crate::AppCore;
use async_trait::async_trait;
use pawork_control_plane::{UsageLedger, UsageQuery};
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
        &ProviderId::new("opencode-go"),
        "upstream-test-secret",
    )
    .unwrap();
    let config = pawork_workspace::config::PaworkConfig {
        default_provider: Some("opencode-go".into()),
        default_model: Some("model-a".into()),
        providers: vec![pawork_workspace::config::ProviderConfig {
            id: "opencode-go".into(),
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
        .any(|m| m["id"] == "opencode-go/model-a"));
    assert!(!models.to_string().contains("upstream-test-secret"));
    let request = json!({"pawork_usage":{"task":{"id":"novel","title":"作品"},"group":{"id":"part-1","title":"第一部"},"subtask":{"id":"chapter-1","title":"第一章"},"operation":"text","retry_of":null},"model":"opencode-go/model-a","messages":[{"role":"system","content":"中文写作"},{"role":"user","content":"你好"}],"max_tokens":20,"response_format":{"type":"json_schema","json_schema":{"name":"answer","strict":true,"schema":{"type":"object"}}}});
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
    let call_id = normal["pawork_usage"]["call_id"]
        .as_str()
        .expect("usage identity")
        .to_string();
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
    let posted_requests: Vec<_> = upstream_requests
        .iter()
        .filter(|r| r.method == "POST")
        .collect();
    assert_eq!(posted_requests.len(), 2);
    let sessions: Vec<_> = posted_requests
        .iter()
        .map(|r| {
            r.headers
                .get("x-opencode-session")
                .unwrap()
                .to_str()
                .unwrap()
        })
        .collect();
    assert!(sessions.iter().all(|s| !s.is_empty()));
    assert_ne!(sessions[0], sessions[1]);
    let posted = upstream_requests
        .iter()
        .find(|r| r.method == "POST")
        .unwrap();
    let posted: Value = serde_json::from_slice(&posted.body).unwrap();
    assert_eq!(posted["model"], "model-a");
    assert_eq!(posted["response_format"]["json_schema"]["strict"], true);
    assert!(posted.get("session_id").is_none());
    assert!(posted.get("pawork_usage").is_none());
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
    let report: TaskUsageReport = http
        .post(format!("{base}/v1/usage/query"))
        .bearer_auth(&token)
        .json(&json!({"limit":1,"group_by":"model"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (
            report.records.len(),
            report.totals.records,
            report.totals.tokens.input_tokens
        ),
        (1, 2, 6)
    );
    assert_eq!(report.totals.unknown_cost_calls, 2);
    assert!(report.next_cursor.is_some());
    assert!(core
        .usage
        .control
        .ledger
        .get_task_usage("momai", &call_id)
        .await
        .unwrap()
        .is_some());
    let (_, foreign_token) = tokens.issue("yingmai").unwrap();
    let foreign: TaskUsageReport = http
        .post(format!("{base}/v1/usage/query"))
        .bearer_auth(&foreign_token)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(foreign.totals.records, 0);
    assert_eq!(
        http.post(format!("{base}/v1/usage/query"))
            .bearer_auth(&token)
            .json(&json!({"client":"yingmai"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let local = json!({"report_id":"download-1","context":{"task":{"id":"novel","title":"作品"},"operation":"download","group":null,"subtask":null,"retry_of":null},"status":"succeeded","started_at_ms":10,"finished_at_ms":12,"related_call_id":call_id});
    for _ in 0..2 {
        assert_eq!(
            http.post(format!("{base}/v1/usage/operations"))
                .bearer_auth(&token)
                .json(&local)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    let mut fabricated = local.clone();
    fabricated["tokens"] = json!({"input_tokens":1});
    assert_eq!(
        http.post(format!("{base}/v1/usage/operations"))
            .bearer_auth(&token)
            .json(&fabricated)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let report = core
        .usage
        .control
        .ledger
        .task_usage_report(&TaskUsageQuery {
            client: Some("momai".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        (
            report.totals.records,
            report.totals.generation_calls,
            report.totals.tokens.input_tokens
        ),
        (3, 2, 6)
    );
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
    // Token Plan 生图返回普通 JSON，output.choices 携带图片。
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(json!({
            "model":"wan2.7-image",
            "stream":false,
            "messages":[{"role":"user","content":[
                {"type":"text","text":"a red cube on white background"}
            ]}]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output":{"choices":[{"message":{"role":"assistant","content":[
                {"type":"image","image":"https://gen.example/out.png"}
            ]},"finish_reason":"stop"}],"finished":true},
            "usage":{"input_tokens":32,"output_tokens":2,"image_count":1}
        })))
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
    let server = tokio::spawn(serve_gateway(
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
    assert_eq!(completion["usage"]["total_tokens"], 34);
    let log = core
        .usage
        .control
        .ledger
        .get_task_usage(
            "momai",
            completion["pawork_usage"]["call_id"].as_str().unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(log.operation, TaskUsageOperation::Image);
    assert_eq!(log.output_images, Some(1));
    assert!(log.cost.is_none());
    let image_request = upstream
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.method == "POST")
        .unwrap();
    let image_request: Value = serde_json::from_slice(&image_request.body).unwrap();
    assert!(image_request.get("stream_options").is_none());

    // HTTP 200 错误正文不能成为成功，也不能回显上游原文。
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(
            json!({"messages":[{"role":"user","content":[
                {"type":"text","text":"image error"}
            ]}]}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code":"DataInspectionFailed","message":"upstream-test-secret"
        })))
        .expect(1)
        .mount(&upstream)
        .await;
    let failure = http.post(format!("{base}/v1/chat/completions"))
        .bearer_auth(&token)
        .json(&json!({"model":"test/wan2.7-image","messages":[{"role":"user","content":"image error"}]}))
        .send().await.unwrap();
    assert_eq!(failure.status(), 502);
    let failure: Value = failure.json().await.unwrap();
    assert!(failure.get("error").is_some());
    assert!(failure.get("choices").is_none());
    assert!(!failure.to_string().contains("upstream-test-secret"));
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
    let unknown = core
        .usage
        .control
        .ledger
        .get_task_usage("momai", vision["pawork_usage"]["call_id"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unknown.status, TaskUsageStatus::Succeeded);
    assert!(unknown.tokens.is_none() && unknown.cost.is_none());
    let failed_log = core
        .usage
        .control
        .ledger
        .get_task_usage("momai", failure["error"]["call_id"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed_log.status, TaskUsageStatus::Failed);
    assert!(failed_log.tokens.is_none() && failed_log.output_images.is_none());
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
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
    let report = core
        .usage
        .control
        .ledger
        .task_usage_report(&TaskUsageQuery {
            client: Some("editor".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(report.totals.records, 2);
    assert_eq!(report.totals.tokens.input_tokens, 14);
    assert_eq!(report.totals.cancelled, 1);
    assert_eq!(report.totals.failed, 1);
    assert!(report.records.iter().all(|r| r.finished_at_ms.is_some()));
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn gateway_native_video_submits_once_queries_and_redacts_failures() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/compatible-mode/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"model-a"}]})))
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/services/aigc/video-generation/video-synthesis"))
        .and(header("authorization", "Bearer video-provider-secret"))
        .and(header("x-dashscope-async", "enable"))
        .and(body_partial_json(json!({"model":"happyhorse-1.1-t2v","input":{"prompt":"纸船漂流"},"parameters":{"resolution":"720P","ratio":"16:9","duration":5}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"output":{"task_id":"task-123","task_status":"PENDING"}})))
        .expect(1).mount(&upstream).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/tasks/task-123"))
        .and(header("authorization", "Bearer video-provider-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"output":{"task_id":"task-123","task_status":"SUCCEEDED","video_url":"https://video.example/result.mp4"}})))
        .expect(2).mount(&upstream).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/tasks/task-failed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"output":{"task_id":"task-failed","task_status":"FAILED","code":"InvalidParameter","message":"video-provider-secret private-prompt"}})))
        .expect(1).mount(&upstream).await;
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    pawork_auth::store_default_api_key(
        backend.as_ref(),
        &ProviderId::new("qwen-token-plan"),
        "video-provider-secret",
    )
    .unwrap();
    let mut core = AppCore::from_config(
        pawork_workspace::config::PaworkConfig {
            default_provider: Some("qwen-token-plan".into()),
            default_model: Some("model-a".into()),
            providers: vec![pawork_workspace::config::ProviderConfig {
                id: "qwen-token-plan".into(),
                base_url: Some(format!("{}/compatible-mode/v1", upstream.uri())),
                ..Default::default()
            }],
            ..Default::default()
        },
        None,
        None,
        backend.clone(),
    )
    .await
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    core.open_control_plane(temp.path()).unwrap();
    let core = Arc::new(core);
    let tokens = GatewayTokenStore::new(temp.path());
    let (_, token) = tokens.issue("yingmai").unwrap();
    let (_, foreign_token) = tokens.issue("momai").unwrap();
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
    let models: Value = http
        .get(format!("{base}/v1/video/models"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        models["data"][0]["id"],
        "qwen-token-plan/happyhorse-1.1-t2v"
    );
    let submitted = http
        .post(format!("{base}/v1/video/tasks"))
        .bearer_auth(&token)
        .json(&json!({"model":"qwen-token-plan/happyhorse-1.1-t2v","prompt":"纸船漂流"}))
        .send()
        .await
        .unwrap();
    assert_eq!(submitted.status(), 202);
    let submitted: Value = submitted.json().await.unwrap();
    assert_eq!(submitted["id"], "default-api-key.task-123");
    // 切换账号后仍查询提交账号，网关不把任务路由到新选中的凭证。
    let second = pawork_auth::add_api_key_account(
        backend.as_ref(),
        &ProviderId::new("qwen-token-plan"),
        "second",
        "second-provider-secret",
        false,
    )
    .unwrap();
    pawork_auth::select_provider_account(
        backend.as_ref(),
        &ProviderId::new("qwen-token-plan"),
        &second.credential_id,
    )
    .unwrap();
    assert_eq!(submitted["status"], "PENDING");
    assert_eq!(
        http.get(format!("{base}/v1/video/tasks/default-api-key.task-123"))
            .bearer_auth(&foreign_token)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    // 同一视频并发轮询仍只有一次生成，迟到响应不会导致统计写入冲突。
    let poll = || async {
        let response = http
            .get(format!("{base}/v1/video/tasks/default-api-key.task-123"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.json::<Value>().await.unwrap()
    };
    let (ready, second_ready) = tokio::join!(poll(), poll());
    assert_eq!(second_ready["status"], "SUCCEEDED");
    assert_eq!(ready["status"], "SUCCEEDED");
    assert_eq!(ready["url"], "https://video.example/result.mp4");
    let report = core
        .usage
        .control
        .ledger
        .task_usage_report(&TaskUsageQuery {
            client: Some("yingmai".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        (
            report.totals.records,
            report.totals.generation_calls,
            report.totals.planned_video_seconds
        ),
        (3, 1, 5)
    );
    let generated = report
        .records
        .iter()
        .find(|r| r.operation == TaskUsageOperation::Video)
        .unwrap();
    assert_eq!(generated.status, TaskUsageStatus::Succeeded);
    assert!(generated.tokens.is_none() && generated.cost.is_none());
    let reopened =
        pawork_control_plane::SqliteUsageLedger::open(temp.path().join("usage-ledger.sqlite3"))
            .unwrap();
    assert_eq!(
        reopened
            .find_task_usage_video(Some("yingmai"), "default-api-key.task-123")
            .await
            .unwrap()
            .unwrap(),
        *generated
    );
    assert_eq!(
        ready["pawork_usage"]["related_call_id"],
        submitted["pawork_usage"]["call_id"]
    );
    let failed: Value = http
        .get(format!("{base}/v1/video/tasks/default-api-key.task-failed"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(failed["status"], "FAILED");
    assert_eq!(failed["error_code"], "InvalidParameter");
    assert!(!failed.to_string().contains("video-provider-secret"));
    assert!(!failed.to_string().contains("private-prompt"));
    for invalid in [
        json!({"model":"wrong/model","prompt":"纸船漂流"}),
        json!({"model":"qwen-token-plan/happyhorse-1.1-t2v","prompt":""}),
        json!({"model":"qwen-token-plan/happyhorse-1.1-t2v","prompt":"纸船漂流","duration":10}),
    ] {
        assert_eq!(
            http.post(format!("{base}/v1/video/tasks"))
                .bearer_auth(&token)
                .json(&invalid)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    assert_eq!(
        http.get(format!(
            "{base}/v1/video/tasks/default-api-key.task-123?again=true"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .status(),
        400
    );
    assert_eq!(
        http.get(format!("{base}/v1/video/models"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    // 畸形终态、身份错配与带凭证 URL 均拒绝，不能留下租约。
    for (id, response) in [
        ("bad-status", json!({"output":{"task_id":"bad-status","task_status":"FINISHED"}}).to_string()),
        ("bad-identity", json!({"output":{"task_id":"other-task","task_status":"RUNNING"}}).to_string()),
        ("bad-url", json!({"output":{"task_id":"bad-url","task_status":"SUCCEEDED","video_url":"https://video-provider-secret@video.example/result.mp4"}}).to_string()),
        ("no-output", json!({"output":{"task_id":"no-output","task_status":"SUCCEEDED"}}).to_string()),
        ("oversized", json!({"padding":"x".repeat(1024*1024)}).to_string()),
    ] {
        Mock::given(method("GET")).and(path(format!("/api/v1/tasks/{id}")))
            .and(header("authorization", "Bearer video-provider-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_string(response)).expect(1).mount(&upstream).await;
        let response = http.get(format!("{base}/v1/video/tasks/default-api-key.{id}"))
            .bearer_auth(&token).send().await.unwrap();
        assert_eq!(response.status(), 502);
        assert!(!response.text().await.unwrap().contains("video-provider-secret"));
    }
    let missing = http
        .get(format!("{base}/v1/video/tasks/missing-account.task-123"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 503);
    assert_eq!(
        upstream
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
    assert!(core
        .usage
        .control
        .ledger
        .query(&UsageQuery::by_tenant(TenantId::new("thirdparty/yingmai")))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        core.usage.control.pool.active_count_for(
            &TenantId::new("thirdparty/yingmai"),
            &AccountId::new("qwen-token-plan/default-api-key")
        ),
        0
    );
    // 非流式任务等待上游时，断连与 Host 退出都必须取消并释放租约。
    Mock::given(method("GET"))
        .and(path("/api/v1/tasks/task-slow"))
        .and(header("authorization", "Bearer video-provider-secret"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(30))
                .set_body_json(json!({"output":{"task_id":"task-slow","task_status":"RUNNING"}})),
        )
        .expect(2)
        .mount(&upstream)
        .await;
    for (index, shutdown) in [false, true].into_iter().enumerate() {
        use tokio::io::AsyncWriteExt;
        let address = base.trim_start_matches("http://");
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket.write_all(format!("GET /v1/video/tasks/default-api-key.task-slow HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\n\r\n").as_bytes()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while upstream
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|request| request.url.path() == "/api/v1/tasks/task-slow")
                .count()
                <= index
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("video query must reach upstream");
        assert_eq!(
            core.usage.control.pool.active_count_for(
                &TenantId::new("thirdparty/yingmai"),
                &AccountId::new("qwen-token-plan/default-api-key")
            ),
            1
        );
        let _connection = if shutdown {
            cancel.cancel();
            Some(socket)
        } else {
            drop(socket);
            None
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while core.usage.control.pool.active_count_for(
                &TenantId::new("thirdparty/yingmai"),
                &AccountId::new("qwen-token-plan/default-api-key"),
            ) != 0
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled video query must release its lease");
    }
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
