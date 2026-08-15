use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{LogRecord, MetricSnapshot, Redactor};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDiagnostic {
    pub name: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDiagnostic {
    pub name: String,
    pub version: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpDiagnostic {
    pub name: String,
    pub transport: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashDiagnostic {
    pub timestamp_unix_ms: u64,
    pub component: String,
    pub error_code: String,
    pub summary: String,
}

/// 可进入诊断包的日志字段白名单。刻意不包含 message、任意 fields、文件内容或 Tool Output。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticLog {
    pub timestamp_unix_ms: u64,
    pub level: String,
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

impl From<LogRecord> for DiagnosticLog {
    fn from(record: LogRecord) -> Self {
        Self {
            timestamp_unix_ms: record.timestamp_unix_ms,
            level: record.level,
            component: record.component,
            workspace_id: record.workspace_id,
            session_id: record.session_id,
            run_id: record.run_id,
            provider: record.provider,
            model: record.model,
            tool_call_id: record.tool_call_id,
            trace_id: record.trace_id,
            duration_ms: record.duration_ms,
            error_code: record.error_code,
        }
    }
}

/// 诊断包的显式 allowlist 输入；类型中没有用户消息、文件内容或 Tool Output 字段。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticInput {
    pub core_version: String,
    pub providers: Vec<ProviderDiagnostic>,
    pub models: Vec<String>,
    pub database_schema_version: u32,
    pub plugins: Vec<PluginDiagnostic>,
    pub mcp_servers: Vec<McpDiagnostic>,
    pub logs: Vec<DiagnosticLog>,
    pub metrics: MetricSnapshot,
    pub crashes: Vec<CrashDiagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagnosticLimits {
    pub max_logs: usize,
    pub max_crashes: usize,
    pub max_bytes: usize,
}

impl Default for DiagnosticLimits {
    fn default() -> Self {
        Self {
            max_logs: 500,
            max_crashes: 10,
            max_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticBundle {
    pub format_version: u32,
    pub generated_at_unix_ms: u64,
    pub core_version: String,
    pub os: String,
    pub architecture: String,
    pub providers: Vec<ProviderDiagnostic>,
    pub models: Vec<String>,
    pub database_schema_version: u32,
    pub plugins: Vec<PluginDiagnostic>,
    pub mcp_servers: Vec<McpDiagnostic>,
    pub logs: Vec<DiagnosticLog>,
    pub metrics: MetricSnapshot,
    pub crashes: Vec<CrashDiagnostic>,
    pub truncated: bool,
}

impl DiagnosticBundle {
    pub fn build(
        mut input: DiagnosticInput,
        redactor: &Redactor,
        limits: DiagnosticLimits,
    ) -> Result<Self, DiagnosticError> {
        let mut truncated = retain_tail(&mut input.logs, limits.max_logs);
        truncated |= retain_tail(&mut input.crashes, limits.max_crashes);
        let bundle = Self {
            format_version: 1,
            generated_at_unix_ms: now_unix_ms(),
            core_version: input.core_version,
            os: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            providers: input.providers,
            models: input.models,
            database_schema_version: input.database_schema_version,
            plugins: input.plugins,
            mcp_servers: input.mcp_servers,
            logs: input.logs,
            metrics: input.metrics,
            crashes: input.crashes,
            truncated,
        };

        let mut value = serde_json::to_value(bundle)?;
        redact_json(&mut value, None, redactor);
        let mut bundle: Self = serde_json::from_value(value)?;
        while serialized_len(&bundle)? > limits.max_bytes {
            if !bundle.logs.is_empty() {
                bundle.logs.remove(0);
            } else if !bundle.crashes.is_empty() {
                bundle.crashes.remove(0);
            } else {
                return Err(DiagnosticError::BundleTooLarge {
                    max_bytes: limits.max_bytes,
                });
            }
            bundle.truncated = true;
        }
        Ok(bundle)
    }

    /// 以 create-new + fsync 写出单个离线 JSON 文件；不覆盖已有文件。
    ///
    /// 最终路径直接使用 `create_new` 打开，消除「先 exists 再 rename」的 TOCTOU 覆盖窗口。
    pub fn export(&self, destination: impl AsRef<Path>) -> Result<(), DiagnosticError> {
        let destination = destination.as_ref();
        if let Some(parent) = destination.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(DiagnosticError::Io)?;
            }
        }
        if destination
            .file_name()
            .and_then(|name| name.to_str())
            .is_none()
        {
            return Err(DiagnosticError::InvalidDestination(destination.into()));
        }

        let bytes = serde_json::to_vec_pretty(self)?;
        let mut file = match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination)
        {
            Ok(file) => file,
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(DiagnosticError::DestinationExists(destination.into()));
            }
            Err(source) => return Err(DiagnosticError::Io(source)),
        };

        let write_result = (|| -> std::io::Result<()> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(source) = write_result {
            return Err(DiagnosticError::Io(source));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum DiagnosticError {
    #[error("diagnostic bundle exceeds the {max_bytes}-byte limit after safe truncation")]
    BundleTooLarge { max_bytes: usize },
    #[error("diagnostic destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("invalid diagnostic destination: {0}")]
    InvalidDestination(PathBuf),
    #[error("diagnostic serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("diagnostic export failed: {0}")]
    Io(#[from] std::io::Error),
}

fn retain_tail<T>(values: &mut Vec<T>, maximum: usize) -> bool {
    if values.len() <= maximum {
        return false;
    }
    values.drain(0..values.len() - maximum);
    true
}

fn serialized_len(bundle: &DiagnosticBundle) -> Result<usize, serde_json::Error> {
    serde_json::to_vec(bundle).map(|bytes| bytes.len())
}

fn redact_json(value: &mut Value, key: Option<&str>, redactor: &Redactor) {
    match value {
        Value::String(text) => {
            *text = redactor.redact_field(key.unwrap_or_default(), text);
        }
        Value::Array(values) => {
            for value in values {
                redact_json(value, key, redactor);
            }
        }
        Value::Object(values) => {
            for (name, value) in values {
                redact_json(value, Some(name), redactor);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_allowlist_excludes_content_and_redacts_all_strings() {
        let secret = "sk-abcdefghijklmnopqrstuvwxyz";
        let input = DiagnosticInput {
            core_version: "0.0.0".into(),
            providers: vec![ProviderDiagnostic {
                name: format!("provider token={secret}"),
                status: "ready".into(),
            }],
            models: vec!["model-1 cookie=session-secret".into()],
            database_schema_version: 2,
            plugins: Vec::new(),
            mcp_servers: Vec::new(),
            logs: vec![DiagnosticLog {
                timestamp_unix_ms: 1,
                level: "info".into(),
                component: format!("provider Authorization: Bearer {secret}"),
                workspace_id: None,
                session_id: None,
                run_id: None,
                provider: None,
                model: None,
                tool_call_id: None,
                trace_id: None,
                duration_ms: None,
                error_code: None,
            }],
            metrics: MetricSnapshot::default(),
            crashes: Vec::new(),
        };
        let bundle =
            DiagnosticBundle::build(input, &Redactor::default(), DiagnosticLimits::default())
                .expect("bundle");
        let json = serde_json::to_string(&bundle).expect("serialize");
        for forbidden in [
            secret,
            "session-secret",
            "user_message",
            "file_content",
            "tool_output",
        ] {
            assert!(
                !json.contains(forbidden),
                "forbidden content leaked: {forbidden}"
            );
        }
    }

    #[test]
    fn truncates_oldest_logs_to_declared_limit() {
        let logs = (0..3)
            .map(|timestamp| DiagnosticLog {
                timestamp_unix_ms: timestamp,
                level: "info".into(),
                component: "test".into(),
                workspace_id: None,
                session_id: None,
                run_id: None,
                provider: None,
                model: None,
                tool_call_id: None,
                trace_id: None,
                duration_ms: None,
                error_code: None,
            })
            .collect();
        let bundle = DiagnosticBundle::build(
            DiagnosticInput {
                core_version: "0".into(),
                providers: Vec::new(),
                models: Vec::new(),
                database_schema_version: 1,
                plugins: Vec::new(),
                mcp_servers: Vec::new(),
                logs,
                metrics: MetricSnapshot::default(),
                crashes: Vec::new(),
            },
            &Redactor::default(),
            DiagnosticLimits {
                max_logs: 2,
                max_crashes: 0,
                max_bytes: 64 * 1024,
            },
        )
        .expect("bundle");
        assert!(bundle.truncated);
        assert_eq!(bundle.logs.len(), 2);
        assert_eq!(bundle.logs[0].timestamp_unix_ms, 1);
    }

    fn sample_bundle() -> DiagnosticBundle {
        DiagnosticBundle::build(
            DiagnosticInput {
                core_version: "0.0.1".into(),
                providers: Vec::new(),
                models: Vec::new(),
                database_schema_version: 1,
                plugins: Vec::new(),
                mcp_servers: Vec::new(),
                logs: Vec::new(),
                metrics: MetricSnapshot::default(),
                crashes: Vec::new(),
            },
            &Redactor::default(),
            DiagnosticLimits::default(),
        )
        .expect("bundle")
    }

    fn temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "pawork-diagnostics-{}-{}-{name}",
            std::process::id(),
            nanos
        ))
    }

    #[test]
    fn export_refuses_existing_destination_without_overwrite() {
        let dir = temp_path("export-exists");
        fs::create_dir_all(&dir).expect("mkdir");
        let destination = dir.join("bundle.json");
        fs::write(&destination, b"existing").expect("seed");
        let error = sample_bundle()
            .export(&destination)
            .expect_err("must not overwrite");
        assert!(matches!(error, DiagnosticError::DestinationExists(_)));
        assert_eq!(fs::read(&destination).expect("read"), b"existing");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn export_create_new_makes_concurrent_writer_lose_without_overwrite() {
        let dir = temp_path("export-race");
        fs::create_dir_all(&dir).expect("mkdir");
        let destination = dir.join("bundle.json");
        let first = sample_bundle();
        first.export(&destination).expect("first export");
        assert!(destination.exists());
        let original = fs::read(&destination).expect("read first");

        let second = sample_bundle();
        let error = second
            .export(&destination)
            .expect_err("second export must fail");
        assert!(matches!(error, DiagnosticError::DestinationExists(_)));
        assert_eq!(fs::read(&destination).expect("read after race"), original);

        // 并发预占：模拟另一进程 create_new 已占位。
        let racing = dir.join("racing.json");
        let hold = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&racing)
            .expect("preempt");
        let error = sample_bundle()
            .export(&racing)
            .expect_err("preempted destination");
        assert!(matches!(error, DiagnosticError::DestinationExists(_)));
        drop(hold);
        assert_eq!(fs::metadata(&racing).expect("meta").len(), 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bundle_redacts_url_query_nested_json_and_custom_headers() {
        let token = "super-secret-token-xyz";
        let api_key = "api-key-should-hide";
        let nested_secret = "nested-json-secret-999";
        let header_secret = "custom-header-secret-abc";
        let nested = format!(
            r#"{{\"auth\":{{\"token\":\"{nested_secret}\",\"note\":\"api_key={api_key}\"}}}}"#
        );
        let input = DiagnosticInput {
            core_version: "0.0.0".into(),
            providers: vec![ProviderDiagnostic {
                name: format!("https://api.example/v1?token={token}&api_key={api_key}"),
                status: "ready".into(),
            }],
            models: vec![nested],
            database_schema_version: 1,
            plugins: Vec::new(),
            mcp_servers: Vec::new(),
            logs: vec![DiagnosticLog {
                timestamp_unix_ms: 1,
                level: "info".into(),
                component: format!("X-Custom-Token: {header_secret}"),
                workspace_id: None,
                session_id: None,
                run_id: None,
                provider: None,
                model: None,
                tool_call_id: None,
                trace_id: None,
                duration_ms: None,
                error_code: None,
            }],
            metrics: MetricSnapshot::default(),
            crashes: Vec::new(),
        };
        let bundle =
            DiagnosticBundle::build(input, &Redactor::default(), DiagnosticLimits::default())
                .expect("bundle");
        let json = serde_json::to_string(&bundle).expect("serialize");
        for forbidden in [token, api_key, nested_secret, header_secret] {
            assert!(
                !json.contains(forbidden),
                "forbidden content leaked: {forbidden} in {json}"
            );
        }
    }
}
