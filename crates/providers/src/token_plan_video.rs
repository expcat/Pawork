//! Token Plan 原生异步文生视频；复用 HTTP、凭证与取消边界，不轮询、不重试。
use crate::net::http::{read_json_stream, HttpClient};
use crate::ApiKeyChannelConfig;
use pawork_domain::{
    CancellationToken, CredentialKind, ProviderError, ProviderErrorKind, ResolvedCredential,
    VideoGenerationTask, VideoTaskStatus,
};
use serde_json::{json, Value};

pub const TOKEN_PLAN_VIDEO_MODEL: &str = "happyhorse-1.1-t2v";

pub struct TokenPlanVideoClient {
    client: HttpClient,
    base: String,
    credential: ResolvedCredential,
}

impl TokenPlanVideoClient {
    pub fn new(
        config: ApiKeyChannelConfig,
        credential: ResolvedCredential,
    ) -> Result<Self, ProviderError> {
        let raw_base = config
            .base_url
            .trim_end_matches('/')
            .strip_suffix("/compatible-mode/v1")
            .filter(|base| !base.is_empty())
            .ok_or_else(|| {
                invalid("Token Plan native video requires its compatible-mode base URL")
            })?
            .to_string();
        let base = reqwest::Url::parse(&raw_base)
            .map_err(|_| invalid("invalid Token Plan video base URL"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || raw_base
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(invalid("invalid Token Plan video base URL"));
        }
        let base = base.as_str().trim_end_matches('/').to_string();
        if config.preset.id != "qwen-token-plan"
            || config
                .http
                .extra_headers
                .iter()
                .any(|(name, _)| crate::is_credential_header(name))
        {
            return Err(invalid("unsupported Token Plan video configuration"));
        }
        if credential.kind() != CredentialKind::ApiKey
            || credential.expose_secret().trim().is_empty()
        {
            return Err(ProviderError::new(
                ProviderErrorKind::Authentication,
                "Token Plan video requires an API key",
            ));
        }
        let mut http = config.http;
        if let Some(timeout) = config.request_timeout {
            http.timeout = Some(timeout);
        }
        Ok(Self {
            client: HttpClient::new(http)?,
            base,
            credential,
        })
    }

    fn headers(&self) -> Vec<(String, String)> {
        vec![(
            "Authorization".into(),
            format!("Bearer {}", self.credential.expose_secret()),
        )]
    }

    /// 首版固定 720P / 16:9 / 5 秒，与已验证的官方契约一致。
    pub async fn submit(
        &self,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<VideoGenerationTask, ProviderError> {
        if prompt.trim().is_empty() || prompt.chars().count() > 8000 {
            return Err(invalid("video prompt must contain 1-8000 characters"));
        }
        let mut headers = self.headers();
        headers.push(("X-DashScope-Async".into(), "enable".into()));
        let stream = self.client.post_stream_with_headers(
            &format!("{}/api/v1/services/aigc/video-generation/video-synthesis", self.base),
            json!({"model":TOKEN_PLAN_VIDEO_MODEL,"input":{"prompt":prompt},"parameters":{"resolution":"720P","ratio":"16:9","duration":5}}),
            None, &headers, cancel.clone(),
        ).await?;
        normalize_task(read_json_stream(stream, cancel).await?)
    }

    pub async fn query(
        &self,
        id: &str,
        cancel: CancellationToken,
    ) -> Result<VideoGenerationTask, ProviderError> {
        if !valid_task_id(id) {
            return Err(invalid("invalid video task id"));
        }
        let stream = self
            .client
            .get_stream_with_headers(
                &format!("{}/api/v1/tasks/{id}", self.base),
                &self.headers(),
                cancel.clone(),
            )
            .await?;
        let task = normalize_task(read_json_stream(stream, cancel).await?)?;
        if task.id != id {
            return Err(malformed("video task identity mismatch"));
        }
        Ok(task)
    }
}

pub fn valid_task_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn normalize_task(value: Value) -> Result<VideoGenerationTask, ProviderError> {
    if value.get("error").is_some() || value.get("code").is_some() {
        return Err(malformed("video API returned an error body"));
    }
    let output = value
        .get("output")
        .ok_or_else(|| malformed("video response has no task"))?;
    let id = output
        .get("task_id")
        .and_then(Value::as_str)
        .filter(|id| valid_task_id(id))
        .ok_or_else(|| malformed("video response has no valid task id"))?;
    let status: VideoTaskStatus = serde_json::from_value(output["task_status"].clone())
        .map_err(|_| malformed("video response has no valid task status"))?;
    let url = if status == VideoTaskStatus::Succeeded {
        let raw = output
            .get("video_url")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("successful video task has no output"))?;
        let parsed = reqwest::Url::parse(raw).map_err(|_| malformed("invalid video output URL"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || raw.len() > 8192
            || raw.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(malformed("invalid video output URL"));
        }
        Some(raw.to_string())
    } else {
        None
    };
    // 原始 message / prompt 不回显；仅返回经过白名单筛选的失败码。
    let code = output.get("code").and_then(Value::as_str).filter(|code| {
        matches!(
            *code,
            "InvalidParameter"
                | "ModelNotSupported"
                | "AccessDenied"
                | "QuotaExhausted"
                | "Throttling"
                | "InvalidApiKey"
                | "DataInspectionFailed"
        )
    });
    Ok(VideoGenerationTask {
        id: id.to_string(),
        model: TOKEN_PLAN_VIDEO_MODEL.into(),
        status,
        url,
        error_code: code.map(str::to_string),
    })
}
fn invalid(message: &'static str) -> ProviderError {
    ProviderError::new(ProviderErrorKind::InvalidRequest, message)
}
fn malformed(message: &'static str) -> ProviderError {
    ProviderError::new(ProviderErrorKind::MalformedResponse, message)
}
