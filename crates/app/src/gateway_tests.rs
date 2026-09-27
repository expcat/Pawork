use crate::AppCore;
use async_trait::async_trait;
use pawork_control_plane::UsageQuery;
use pawork_domain::*;
use pawork_gateway::{serve_gateway, GatewayTokenStore};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::net::TcpListener;
use wiremock::{
    matchers::{header, method, path},
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
            capabilities: Default::default(),
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
