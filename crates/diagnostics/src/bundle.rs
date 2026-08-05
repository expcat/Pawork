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

    /// 以 create-new + fsync + rename 写出单个离线 JSON 文件；不覆盖已有文件。
    pub fn export(&self, destination: impl AsRef<Path>) -> Result<(), DiagnosticError> {
        let destination = destination.as_ref();
        if destination.exists() {
            return Err(DiagnosticError::DestinationExists(destination.into()));
        }
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| DiagnosticError::InvalidDestination(destination.into()))?;
        let temporary = parent.join(format!(
            ".{file_name}.tmp-{}-{}",
            std::process::id(),
            now_unix_ms()
        ));
        let bytes = serde_json::to_vec_pretty(self)?;
        let result = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, destination)?;
            Ok::<(), std::io::Error>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(DiagnosticError::Io)
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
}
