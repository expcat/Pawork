use std::{
    collections::BTreeMap,
    fmt,
    io::Write,
    sync::{Arc, Mutex},
};

use regex::{Captures, Regex};
use tracing::{field::Visit, Event, Subscriber};
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

/// 全局脱敏 fmt 层：修复 V1 缺口（`StructuredLogLayer` 只进内存 buffer，
/// fmt 输出无脱敏）。每个事件先经 `FieldVisitor` 收集全部字段，再按字段名
/// 与字段值统一走 `Redactor::redact_field`，最后格式化为单行写入注入的
/// writer；构造方持 `Arc<Mutex<dyn Write + Send>>`，不做全局可变状态。
#[derive(Clone)]
pub struct RedactingFmtLayer {
    redactor: Redactor,
    writer: Arc<Mutex<dyn Write + Send>>,
}

impl RedactingFmtLayer {
    pub fn new(redactor: Redactor, writer: Arc<Mutex<dyn Write + Send>>) -> Self {
        Self { redactor, writer }
    }

    fn write_line(&self, line: &str) {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // fmt 输出失败不中断业务；与 tracing fmt 层惯例一致，写错误丢弃。
        let _ = writeln!(writer, "{line}");
    }
}

impl<S> Layer<S> for RedactingFmtLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _context: tracing_subscriber::layer::Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let mut line = format!(
            "{} {}",
            metadata.level().as_str().to_ascii_lowercase(),
            metadata.target(),
        );
        for (name, value) in visitor.fields {
            let redacted = self.redactor.redact_field(&name, &value);
            line.push_str(&format!(" {name}={redacted}"));
        }
        self.write_line(&line);
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
    fn redacting_fmt_layer_masks_secrets_and_keeps_plain_fields() {
        struct Capture(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .map_err(|_| std::io::Error::other("poisoned"))?
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer: Arc<Mutex<dyn std::io::Write + Send>> =
            Arc::new(Mutex::new(Capture(captured.clone())));
        let layer = RedactingFmtLayer::new(Redactor::default(), writer);
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                target: "pawork::fmt_layer",
                component = "provider",
                authorization = "Bearer fmt-bearer-secret",
                api_key = "fmt-field-secret",
                message = "load_failed token=fmt-token-secret",
                detail = "upstream sk-abcdefghijklmnopqrstuvwxyz retry",
                retries = 2_u64,
            );
        });
        let output = String::from_utf8(
            captured
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        )
        .expect("utf-8 output");
        for secret in [
            "fmt-bearer-secret",
            "fmt-field-secret",
            "fmt-token-secret",
            "abcdefghijklmnopqrstuvwxyz",
        ] {
            assert!(
                !output.contains(secret),
                "fmt layer leaked secret {secret:?} in {output:?}"
            );
        }
        assert!(output.contains("pawork::fmt_layer"));
        assert!(output.contains("component=provider"));
        assert!(output.contains("retries=2"));
    }
}
