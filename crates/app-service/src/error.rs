//! 统一错误转换（P13-1）。
//!
//! 所有 Command/Query 处理错误最终转换为 [`core_api::ErrorContext`] 并包装为
//! [`core_api::AppResponse::Error`]，保证 CLI 与 GUI 看到同一错误协议，且不泄漏
//! Secret 或未经脱敏的响应正文。

use std::collections::BTreeMap;

use agent_domain::{ErrorCategory, ErrorContext, QueryId, RunId, Timestamp};
use core_api::{ApiVersion, AppCommandEnvelope, AppResponse, AppResponseEnvelope, API_VERSION};
use thiserror::Error;

use crate::aggregate::AggregateError;
use crate::approval::ApprovalError;
use crate::idempotency::IdempotencyError;
use crate::supervisor::SuperviseError;

/// app-service 层统一错误。
#[derive(Debug, Error)]
pub enum AppServiceError {
    #[error("api version {found:?} is incompatible with {expected:?}")]
    IncompatibleApiVersion {
        found: ApiVersion,
        expected: ApiVersion,
    },
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("authentication required: {0}")]
    Authentication(String),
    #[error("authorization denied: {0}")]
    Authorization(String),
    #[error("identity resolution failed: {0}")]
    Identity(String),
    #[error("rate limited")]
    RateLimited { retry_after_ms: Option<u64> },
    #[error("resource exhausted: {0}")]
    ResourceExhausted(String),
    #[error("service unavailable: {0}")]
    Unavailable(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("no async runtime available; dispatch inside a tokio runtime")]
    NoRuntime,
    #[error(transparent)]
    Workspace(#[from] workspace_service::WorkspaceError),
    #[error(transparent)]
    Aggregate(#[from] AggregateError),
    #[error(transparent)]
    Supervise(#[from] SuperviseError),
    #[error(transparent)]
    Approval(#[from] ApprovalError),
    #[error(transparent)]
    Idempotency(#[from] IdempotencyError),
    #[error(transparent)]
    ArtifactStore(#[from] artifact_store::ArtifactStoreError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl AppServiceError {
    /// 转换为可跨边界传递的 [`ErrorContext`]（不含 Secret）。
    pub fn error_context(&self) -> ErrorContext {
        let (category, retryable, retry_after_ms, message) = match self {
            Self::IncompatibleApiVersion { found, expected } => (
                ErrorCategory::InvalidRequest,
                false,
                None,
                format!("api version {found:?} is incompatible with {expected:?}"),
            ),
            Self::InvalidRequest(message) => {
                (ErrorCategory::InvalidRequest, false, None, message.clone())
            }
            Self::NotFound(message) => (ErrorCategory::NotFound, false, None, message.clone()),
            Self::Conflict(message) => (ErrorCategory::Conflict, false, None, message.clone()),
            Self::Authentication(message) => {
                (ErrorCategory::Authentication, false, None, message.clone())
            }
            Self::Authorization(message) => {
                (ErrorCategory::Authorization, false, None, message.clone())
            }
            Self::Identity(message) => (ErrorCategory::Authorization, false, None, message.clone()),
            Self::RateLimited { retry_after_ms } => (
                ErrorCategory::RateLimit,
                true,
                *retry_after_ms,
                "request is rate limited; retry later".into(),
            ),
            Self::ResourceExhausted(message) => (
                ErrorCategory::ResourceExhausted,
                true,
                None,
                message.clone(),
            ),
            Self::Unavailable(message) => (ErrorCategory::Unavailable, true, None, message.clone()),
            Self::Internal(message) => (ErrorCategory::Internal, false, None, message.clone()),
            Self::NoRuntime => (
                ErrorCategory::Unavailable,
                true,
                None,
                "no async runtime available".into(),
            ),
            Self::Workspace(error) => return workspace_error_context(error),
            Self::Aggregate(error) => return aggregate_error_context(error),
            Self::Supervise(error) => return supervise_error_context(error),
            Self::Approval(error) => return approval_error_context(error),
            Self::Idempotency(error) => return idempotency_error_context(error),
            Self::ArtifactStore(error) => return artifact_store_error_context(error),
            Self::Json(error) => (ErrorCategory::Internal, false, None, error.to_string()),
        };
        ErrorContext {
            category,
            message,
            retryable,
            retry_after_ms,
            diagnostics: BTreeMap::new(),
        }
    }
}

fn workspace_error_context(error: &workspace_service::WorkspaceError) -> ErrorContext {
    use workspace_service::WorkspaceError as E;
    let (category, retryable) = match error {
        E::AlreadyExists(_) => (ErrorCategory::Conflict, false),
        E::NotFound(_) | E::RootNotFound(_) => (ErrorCategory::NotFound, false),
        E::NoRoots
        | E::InvalidRoot { .. }
        | E::RootIsNotDirectory(_)
        | E::InvalidGitDir { .. }
        | E::MalformedGitFile(_) => (ErrorCategory::InvalidRequest, false),
        _ => (ErrorCategory::Internal, false),
    };
    ErrorContext {
        category,
        message: error.to_string(),
        retryable,
        retry_after_ms: None,
        diagnostics: BTreeMap::new(),
    }
}

fn aggregate_error_context(error: &AggregateError) -> ErrorContext {
    let category = match error {
        AggregateError::WorkspaceNotFound(_)
        | AggregateError::SessionNotFound(_)
        | AggregateError::RunNotFound(_)
        | AggregateError::ProviderNotFound(_) => ErrorCategory::NotFound,
        AggregateError::ArtifactExists(_)
        | AggregateError::StaleClientContext { .. }
        | AggregateError::ClientContextConflict { .. } => ErrorCategory::Conflict,
        AggregateError::InvalidClientContext(_) => ErrorCategory::InvalidRequest,
        AggregateError::Poisoned => ErrorCategory::Internal,
    };
    ErrorContext {
        category,
        message: error.to_string(),
        retryable: false,
        retry_after_ms: None,
        diagnostics: BTreeMap::new(),
    }
}

fn supervise_error_context(error: &SuperviseError) -> ErrorContext {
    let (category, retryable) = match error {
        SuperviseError::NotFound(_) => (ErrorCategory::NotFound, false),
        SuperviseError::AlreadyExists(_)
        | SuperviseError::StillActive(_)
        | SuperviseError::Completed(_) => (ErrorCategory::Conflict, false),
        SuperviseError::Capacity(_) => (ErrorCategory::ResourceExhausted, true),
        SuperviseError::PolicyDenied(_) => (ErrorCategory::Authorization, false),
        // P17-5：background run 缺 TaskManager 属配置缺失，不可重试。
        SuperviseError::BackgroundUnavailable(_) => (ErrorCategory::Unavailable, false),
    };
    ErrorContext {
        category,
        message: error.to_string(),
        retryable,
        retry_after_ms: None,
        diagnostics: BTreeMap::new(),
    }
}

fn approval_error_context(error: &ApprovalError) -> ErrorContext {
    let (category, retryable) = match error {
        ApprovalError::NotFound(_) => (ErrorCategory::NotFound, false),
        ApprovalError::RunMismatch { .. } | ApprovalError::AlreadyDecided(_) => {
            (ErrorCategory::Conflict, false)
        }
        ApprovalError::Capacity => (ErrorCategory::ResourceExhausted, true),
    };
    ErrorContext {
        category,
        message: error.to_string(),
        retryable,
        retry_after_ms: None,
        diagnostics: BTreeMap::new(),
    }
}

fn idempotency_error_context(error: &IdempotencyError) -> ErrorContext {
    ErrorContext {
        category: ErrorCategory::Conflict,
        message: error.to_string(),
        retryable: false,
        retry_after_ms: None,
        diagnostics: BTreeMap::new(),
    }
}

fn artifact_store_error_context(error: &artifact_store::ArtifactStoreError) -> ErrorContext {
    use artifact_store::ArtifactStoreError as E;
    let (category, retryable) = match error {
        E::InvalidBlobId(_) | E::UnknownBlob { .. } => (ErrorCategory::NotFound, false),
        // 其余（I/O、完整性校验失败、存储损坏等）属于服务端内部错误。
        _ => (ErrorCategory::Internal, false),
    };
    ErrorContext {
        category,
        message: error.to_string(),
        retryable,
        retry_after_ms: None,
        diagnostics: BTreeMap::new(),
    }
}

/// 构造 Accepted 响应并携带该命令确定启动的 run id（RunStart 专用：
/// 并发来源各自从自己的响应取 run id，不依赖全局 `last_started_run`）。
pub fn accepted_response_with_run(
    envelope: &AppCommandEnvelope,
    run_id: Option<RunId>,
) -> AppResponseEnvelope {
    AppResponseEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from(envelope.command_id.as_str()),
        responded_at: now_timestamp(),
        response: AppResponse::Accepted {
            command_id: envelope.command_id.clone(),
            run_id,
        },
    }
}

/// 构造查询的 Data 响应。
pub fn data_response(request_id: &QueryId, data: serde_json::Value) -> AppResponseEnvelope {
    AppResponseEnvelope {
        api_version: API_VERSION,
        request_id: request_id.clone(),
        responded_at: now_timestamp(),
        response: AppResponse::Data(data),
    }
}

/// 构造统一 Error 响应。
pub fn error_response(request_id: &QueryId, error: &AppServiceError) -> AppResponseEnvelope {
    AppResponseEnvelope {
        api_version: API_VERSION,
        request_id: request_id.clone(),
        responded_at: now_timestamp(),
        response: AppResponse::Error(error.error_context()),
    }
}

/// Unix epoch 起的当前毫秒时间戳。
pub fn now_timestamp() -> Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    Timestamp::from_unix_millis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0),
    )
}
