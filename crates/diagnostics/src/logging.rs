use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use tracing::{field::Visit, Event, Level, Subscriber};
use tracing_subscriber::Layer;

const REDACTED: &str = "[REDACTED]";

#[derive(Clone)]
pub struct Redactor {
    sensitive_key: Regex,
    replacements: Vec<Regex>,
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new(Vec::<String>::new()).expect("built-in redaction patterns are valid")
    }
}

impl Redactor {
    pub fn new<I, S>(custom_patterns: I) -> Result<Self, regex::Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let sensitive_key = Regex::new(
            r"(?i)^(authorization|proxy[_-]?authorization|cookie|set[_-]?cookie|api[_-]?key|(?:[a-z0-9]+[_-])*(?:token|secret|password)|oauth(?:[_-]?code)?)$",
        )?;
        let mut replacements = vec![
            Regex::new(r"(?i)\b(?:proxy[-_ ]?)?authorization\s*[:=]\s*[^\r\n]+")?,
            Regex::new(r"(?i)\b(?:set[-_ ]?)?cookie\s*[:=]\s*[^\r\n]+")?,
            Regex::new(r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]+")?,
            Regex::new(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b")?,
            // 不要求单词边界：Secret 可能嵌在 source_key/error_code（例如
            // `load_failed_sk-...`）中，`_` 属于 regex word character，会绕过 `\b`。
            Regex::new(r"(?:sk|rk|pk|api)[-_][A-Za-z0-9_-]{12,}")?,
            // URL query：?token=...&api_key=...
            Regex::new(
                r#"(?i)(?:[?&])(?:api[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|password|oauth[_-]?code)=([^&#\s]+)"#,
            )?,
            // key=value / key:value，覆盖普通/转义 JSON 与自定义 *Token header。
            // `\\?["']?` 同时接受 `"token"` 与 `\"token\"` 两种键边界。
            Regex::new(
                r#"(?i)(?:authorization|cookie|set-cookie|api[-_ ]?key|access[-_ ]?token|refresh[-_ ]?token|(?:[a-z0-9]+[-_])*(?:token|secret|password)|oauth[-_ ]?code)\\?["']?\s*[:=]\s*\\?["']?[^\s,;\\\"'&#}]+"#,
            )?,
        ];
        for pattern in custom_patterns {
            replacements.push(Regex::new(pattern.as_ref())?);
        }
        Ok(Self {
            sensitive_key,
            replacements,
        })
    }

    pub fn redact(&self, value: &str) -> String {
        let mut redacted = value.to_owned();
        for pattern in &self.replacements {
            redacted = pattern
                .replace_all(&redacted, |captures: &Captures<'_>| {
                    let matched = &captures[0];
                    let lower = matched.to_ascii_lowercase();
                    if lower.starts_with("bearer ") {
                        format!("Bearer {REDACTED}")
                    } else if matched.starts_with('?') || matched.starts_with('&') {
                        // 保留 query 分隔符，仅遮蔽取值。
                        let sep = &matched[..1];
                        if let Some(eq) = matched.find('=') {
                            format!("{sep}{}={REDACTED}", &matched[1..eq])
                        } else {
                            REDACTED.to_owned()
                        }
                    } else {
                        REDACTED.to_owned()
                    }
                })
                .into_owned();
        }
        redacted
    }

    pub fn redact_field(&self, name: &str, value: &str) -> String {
        if self.sensitive_key.is_match(name) {
            REDACTED.into()
        } else {
            self.redact(value)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecord {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct LogBuffer {
    capacity: usize,
    records: Arc<Mutex<VecDeque<LogRecord>>>,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            records: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
        }
    }

    pub fn snapshot(&self) -> Vec<LogRecord> {
        self.records().iter().cloned().collect()
    }

    fn push(&self, record: LogRecord) {
        if self.capacity == 0 {
            return;
        }
        let mut records = self.records();
        while records.len() >= self.capacity {
            records.pop_front();
        }
        records.push_back(record);
    }

    fn records(&self) -> MutexGuard<'_, VecDeque<LogRecord>> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sampling {
    pub trace_every: u64,
    pub debug_every: u64,
    pub info_every: u64,
}

impl Default for Sampling {
    fn default() -> Self {
        Self {
            trace_every: 10,
            debug_every: 5,
            info_every: 1,
        }
    }
}

#[derive(Default)]
struct SamplingCounts {
    trace: u64,
    debug: u64,
    info: u64,
}

#[derive(Clone)]
pub struct StructuredLogLayer {
    buffer: LogBuffer,
    redactor: Redactor,
    sampling: Sampling,
    counts: Arc<Mutex<SamplingCounts>>,
}

impl StructuredLogLayer {
    pub fn new(buffer: LogBuffer, redactor: Redactor, sampling: Sampling) -> Self {
        Self {
            buffer,
            redactor,
            sampling,
            counts: Arc::new(Mutex::new(SamplingCounts::default())),
        }
    }

    fn should_record(&self, level: &Level) -> bool {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match *level {
            Level::TRACE => every(&mut counts.trace, self.sampling.trace_every),
            Level::DEBUG => every(&mut counts.debug, self.sampling.debug_every),
            Level::INFO => every(&mut counts.info, self.sampling.info_every),
            Level::WARN | Level::ERROR => true,
        }
    }
}

impl<S> Layer<S> for StructuredLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _context: tracing_subscriber::layer::Context<'_, S>) {
        let metadata = event.metadata();
        if !self.should_record(metadata.level()) {
            return;
        }
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let mut fields = BTreeMap::new();
        for (name, value) in visitor.fields {
            fields.insert(name.clone(), self.redactor.redact_field(&name, &value));
        }
        let take = |name: &str, fields: &mut BTreeMap<String, String>| fields.remove(name);
        let component = take("component", &mut fields).unwrap_or_else(|| metadata.target().into());
        let duration_ms = take("duration_ms", &mut fields).and_then(|value| value.parse().ok());
        self.buffer.push(LogRecord {
            timestamp_unix_ms: now_unix_ms(),
            level: metadata.level().as_str().to_ascii_lowercase(),
            component,
            workspace_id: take("workspace_id", &mut fields),
            session_id: take("session_id", &mut fields),
            run_id: take("run_id", &mut fields),
            provider: take("provider", &mut fields),
            model: take("model", &mut fields),
            tool_call_id: take("tool_call_id", &mut fields),
            trace_id: take("trace_id", &mut fields),
            duration_ms,
            error_code: take("error_code", &mut fields),
            message: take("message", &mut fields),
            fields,
        });
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields.insert(field.name().into(), value.into());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields.insert(field.name().into(), value.to_string());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields.insert(field.name().into(), value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields.insert(field.name().into(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        self.fields
            .insert(field.name().into(), format!("{value:?}"));
    }
}

fn every(counter: &mut u64, interval: u64) -> bool {
    *counter = counter.saturating_add(1);
    interval != 0 && (*counter - 1) % interval == 0
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
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    #[test]
    fn redacts_headers_tokens_cookies_oauth_jwt_and_custom_patterns() {
        let redactor = Redactor::new([r"CUSTOM-[0-9]+"] as [&str; 1]).expect("redactor");
        let cases = [
            ("Authorization: Bearer abc.def", "abc.def"),
            ("Authorization: Basic dXNlcjpwYXNz", "dXNlcjpwYXNz"),
            ("Cookie: first=one; second=two", "second=two"),
            ("oauth_code=code-1", "code-1"),
            (
                "sk-abcdefghijklmnopqrstuvwxyz",
                "sk-abcdefghijklmnopqrstuvwxyz",
            ),
            (
                "load_failed_sk-abcdefghijklmnopqrstuvwxyz",
                "sk-abcdefghijklmnopqrstuvwxyz",
            ),
            (
                "eyJabcdefgh.ijklmnop.qrstuvwx",
                "eyJabcdefgh.ijklmnop.qrstuvwx",
            ),
            ("CUSTOM-123", "CUSTOM-123"),
            (
                "https://api.example/v1?token=url-token-secret&api_key=url-api-key-secret",
                "url-token-secret",
            ),
            (
                "https://api.example/v1?token=url-token-secret&api_key=url-api-key-secret",
                "url-api-key-secret",
            ),
            (
                r#"{\"auth\":{\"token\":\"nested-json-secret\",\"note\":\"api_key=nested-api-key\"}}"#,
                "nested-json-secret",
            ),
            (
                r#"{\"auth\":{\"token\":\"nested-json-secret\",\"note\":\"api_key=nested-api-key\"}}"#,
                "nested-api-key",
            ),
            (
                "X-Custom-Token: custom-header-secret",
                "custom-header-secret",
            ),
        ];
        for (input, secret) in cases {
            let output = redactor.redact(input);
            assert!(
                !output.contains(secret),
                "secret leaked: {secret} from {input:?} -> {output:?}"
            );
        }
        assert_eq!(redactor.redact_field("api_key", "plain-secret"), REDACTED);
        assert_eq!(
            redactor.redact_field("context_tokens", "128"),
            "128",
            "non-secret token metrics must remain observable"
        );
    }

    #[test]
    fn tracing_layer_redacts_every_field_and_keeps_bounded_tail() {
        let buffer = LogBuffer::new(1);
        let layer = StructuredLogLayer::new(
            buffer.clone(),
            Redactor::default(),
            Sampling {
                trace_every: 1,
                debug_every: 1,
                info_every: 1,
            },
        );
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                component = "provider",
                authorization = "Bearer top-secret",
                message = "token=hidden-one"
            );
            tracing::warn!(component = "tool", cookie = "hidden-two", message = "safe");
        });
        let records = buffer.snapshot();
        assert_eq!(records.len(), 1);
        let json = serde_json::to_string(&records).expect("logs serialize");
        assert!(!json.contains("top-secret"));
        assert!(!json.contains("hidden-one"));
        assert!(!json.contains("hidden-two"));
    }

    #[test]
    fn sampling_never_drops_warnings_or_errors() {
        let buffer = LogBuffer::new(16);
        let layer = StructuredLogLayer::new(
            buffer.clone(),
            Redactor::default(),
            Sampling {
                trace_every: 0,
                debug_every: 0,
                info_every: 0,
            },
        );
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("drop");
            tracing::warn!("keep warning");
            tracing::error!("keep error");
        });
        assert_eq!(buffer.snapshot().len(), 2);
    }
}
