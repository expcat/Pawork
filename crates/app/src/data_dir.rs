use std::ffi::OsString;
use std::path::{Path, PathBuf};

use pawork_domain::{DegradeEvent, DegradeKind, DegradeSeverity};
use serde_json::json;

/// 缺省实例名（与 V1 `--instance` 默认值对齐）。
pub const DEFAULT_INSTANCE: &str = "default";

/// `default_data_dir` 的可观测结果：路径 + 可选 HOME 回退降级事件。
#[derive(Clone, Debug, PartialEq)]
pub struct DataDirOutcome {
    pub path: PathBuf,
    pub degrade: Option<DegradeEvent>,
}

/// 会话库所在数据目录。覆盖：`PAWORK_DATA_DIR`。
///
/// 默认与 V1 `instance_dir` 对齐：Windows `%LOCALAPPDATA%\pawork`，
/// 其他平台 `~/.pawork`，再否则临时目录。
pub fn default_data_dir() -> PathBuf {
    default_data_dir_outcome().path
}

/// Consume a [`DataDirOutcome`]: emit the HOME-fallback warning once at the
/// structured sink, then return the path. Path-only helpers stay silent so
/// `attach_workspace` / ops / GUI do not duplicate `AppCore::load_with`.
pub fn consume_data_dir_outcome(outcome: DataDirOutcome) -> PathBuf {
    if let Some(degrade) = &outcome.degrade {
        tracing::warn!(
            code = %degrade.code(),
            severity = degrade.severity.as_str(),
            path = %outcome.path.display(),
            "{}",
            degrade.message
        );
    }
    outcome.path
}

/// 与 [`default_data_dir`] 同路径选择，HOME 缺失回退时附带 DegradeEvent。
pub fn default_data_dir_outcome() -> DataDirOutcome {
    resolve_data_dir_outcome(
        std::env::var("PAWORK_DATA_DIR").ok(),
        if cfg!(windows) {
            std::env::var("LOCALAPPDATA").ok()
        } else {
            None
        },
        std::env::var_os("HOME"),
        std::env::temp_dir(),
    )
}

fn resolve_data_dir_outcome(
    pawork_data_dir: Option<String>,
    local_app_data: Option<String>,
    home: Option<OsString>,
    temp_dir: PathBuf,
) -> DataDirOutcome {
    if let Some(dir) = pawork_data_dir {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return DataDirOutcome {
                path: PathBuf::from(trimmed),
                degrade: None,
            };
        }
    }
    if cfg!(windows) {
        if let Some(local) = local_app_data {
            return DataDirOutcome {
                path: PathBuf::from(local).join("pawork"),
                degrade: None,
            };
        }
    }
    if let Some(home) = home {
        return DataDirOutcome {
            path: PathBuf::from(home).join(".pawork"),
            degrade: None,
        };
    }
    let path = temp_dir.join("pawork");
    let degrade = DegradeEvent::new(
        DegradeKind::HomeDirFallback,
        DegradeSeverity::Warning,
        "HOME is unset; falling back to the process temp directory",
        json!({ "path": path.display().to_string() }),
    );
    DataDirOutcome {
        path: path.clone(),
        degrade: Some(degrade),
    }
}

/// Test seam for exercising the real HOME-fallback resolution from other
/// modules' unit tests without touching process environment variables.
#[cfg(test)]
pub(crate) fn data_dir_outcome_for_test(
    pawork_data_dir: Option<String>,
    local_app_data: Option<String>,
    home: Option<OsString>,
    temp_dir: PathBuf,
) -> DataDirOutcome {
    resolve_data_dir_outcome(pawork_data_dir, local_app_data, home, temp_dir)
}

/// 校验 `--instance`：trim 后仅允许 `[A-Za-z0-9._-]`；空、空白、分号、
/// 换行、引号、`/`、`..` 一律拒绝。`default` 保持原语义（合法标识）。
pub fn normalize_instance(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("instance name must not be empty".into());
    }
    let allowed = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !allowed || name.contains("..") {
        return Err(format!("invalid instance name `{name}`"));
    }
    Ok(name)
}

/// `<data_dir>/<instance>`。
pub fn instance_dir(data_dir: impl AsRef<Path>, instance: &str) -> PathBuf {
    data_dir.as_ref().join(instance)
}

/// `<data_dir>/default/session.db`。
pub fn session_db_path(data_dir: impl AsRef<Path>) -> PathBuf {
    session_db_path_for(data_dir, DEFAULT_INSTANCE)
}

/// `<data_dir>/<instance>/session.db`。
pub fn session_db_path_for(data_dir: impl AsRef<Path>, instance: &str) -> PathBuf {
    instance_dir(data_dir, instance).join("session.db")
}

/// `<data_dir>/default/artifacts`：写前快照与回滚 Blob。
pub fn artifact_store_path(data_dir: impl AsRef<Path>) -> PathBuf {
    artifact_store_path_for(data_dir, DEFAULT_INSTANCE)
}

/// `<data_dir>/<instance>/artifacts`。
pub fn artifact_store_path_for(data_dir: impl AsRef<Path>, instance: &str) -> PathBuf {
    instance_dir(data_dir, instance).join("artifacts")
}

/// `<data_dir>/<instance>/protected`：PWB1 ReasoningProtector 存储根。
pub fn protected_store_path_for(data_dir: impl AsRef<Path>, instance: &str) -> PathBuf {
    instance_dir(data_dir, instance).join("protected")
}

/// `<data_dir>/<instance>/usage-ledger.sqlite3`。
pub fn usage_ledger_path_for(data_dir: impl AsRef<Path>, instance: &str) -> PathBuf {
    instance_dir(data_dir, instance).join("usage-ledger.sqlite3")
}

/// `<data_dir>/<instance>/audit.jsonl`。
pub fn audit_log_path_for(data_dir: impl AsRef<Path>, instance: &str) -> PathBuf {
    instance_dir(data_dir, instance).join("audit.jsonl")
}

/// `<data_dir>/<instance>/tasks.json`。
pub fn tasks_snapshot_path_for(data_dir: impl AsRef<Path>, instance: &str) -> PathBuf {
    instance_dir(data_dir, instance).join("tasks.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_db_path_uses_default_instance() {
        let path = session_db_path("/tmp/pawork-data");
        assert!(
            path.ends_with("default/session.db") || path.ends_with("default\\session.db"),
            "{path:?}"
        );
        let artifacts = artifact_store_path("/tmp/pawork-data");
        assert!(
            artifacts.ends_with("default/artifacts") || artifacts.ends_with("default\\artifacts"),
            "{artifacts:?}"
        );
        let named = session_db_path_for("/tmp/pawork-data", "dev");
        assert!(
            named.ends_with("dev/session.db") || named.ends_with("dev\\session.db"),
            "{named:?}"
        );
        let protected = protected_store_path_for("/tmp/pawork-data", "dev");
        assert!(
            protected.ends_with("dev/protected") || protected.ends_with("dev\\protected"),
            "{protected:?}"
        );
    }

    #[test]
    fn normalize_instance_rejects_path_escape() {
        assert_eq!(normalize_instance("default").expect("ok"), "default");
        assert_eq!(normalize_instance("  default  ").expect("trim"), "default");
        assert_eq!(normalize_instance("dev").expect("ok"), "dev");
        assert_eq!(
            normalize_instance("my-inst_1.0").expect("ok"),
            "my-inst_1.0"
        );
        assert!(normalize_instance("").is_err());
        assert!(normalize_instance("   ").is_err());
        assert!(normalize_instance("../x").is_err());
        assert!(normalize_instance("a/b").is_err());
        assert!(normalize_instance("a b").is_err());
        assert!(normalize_instance("a;b").is_err());
        assert!(normalize_instance("a\nb").is_err());
        assert!(normalize_instance("a'b").is_err());
        assert!(normalize_instance("a\"b").is_err());
        assert!(normalize_instance("..").is_err());
        assert!(normalize_instance("foo..bar").is_err());
        assert!(normalize_instance("evil;rm -rf").is_err());
    }

    #[test]
    fn missing_home_falls_back_to_temp_with_degrade_event() {
        let expected = PathBuf::from("/tmp/process-temp/pawork");
        let subscriber = crate::testsupport::RecordingSubscriber::new();
        let outcome = tracing::subscriber::with_default(subscriber.clone(), || {
            resolve_data_dir_outcome(
                None,
                None,
                None,
                PathBuf::from("/tmp/process-temp"),
            )
        });
        assert_eq!(outcome.path, expected);
        let degrade = outcome.degrade.expect("HOME fallback must emit DegradeEvent");
        assert_eq!(degrade.code(), "degrade.home_dir_fallback");
        assert_eq!(degrade.kind, DegradeKind::HomeDirFallback);
        assert_eq!(degrade.severity, DegradeSeverity::Warning);
        assert_eq!(degrade.details["path"], json!(expected.display().to_string()));
        let events = subscriber.events();
        assert!(
            events.iter().all(|event| {
                event.fields.get("code").map(String::as_str) != Some("degrade.home_dir_fallback")
            }),
            "resolve_data_dir_outcome must stay silent: {events:?}"
        );
    }

    #[test]
    fn consume_data_dir_outcome_warns_once_and_path_helper_stays_silent() {
        let expected = PathBuf::from("/tmp/process-temp/pawork");
        let outcome = resolve_data_dir_outcome(
            None,
            None,
            None,
            PathBuf::from("/tmp/process-temp"),
        );
        let subscriber = crate::testsupport::RecordingSubscriber::new();
        let path = tracing::subscriber::with_default(subscriber.clone(), || {
            consume_data_dir_outcome(outcome.clone())
        });
        assert_eq!(path, expected);
        let events = subscriber.events();
        let emitted: Vec<_> = events
            .iter()
            .filter(|event| {
                event.fields.get("code").map(String::as_str) == Some("degrade.home_dir_fallback")
            })
            .collect();
        assert_eq!(emitted.len(), 1, "HOME fallback must warn once: {events:?}");
        let emitted = emitted[0];
        assert_eq!(emitted.level, "WARN");
        assert!(emitted.message.contains("HOME is unset"), "{emitted:?}");
        assert_eq!(
            emitted.fields.get("severity").map(String::as_str),
            Some("warning"),
            "{emitted:?}"
        );
        assert_eq!(
            emitted.fields.get("path").map(String::as_str),
            Some(expected.display().to_string().as_str()),
            "{emitted:?}"
        );

        let subscriber = crate::testsupport::RecordingSubscriber::new();
        tracing::subscriber::with_default(subscriber.clone(), || {
            let _ = consume_data_dir_outcome(DataDirOutcome {
                path: expected.clone(),
                degrade: None,
            });
        });
        assert!(
            subscriber.events().iter().all(|event| {
                event.fields.get("code").map(String::as_str) != Some("degrade.home_dir_fallback")
            }),
            "consume_data_dir_outcome must stay silent when degrade is absent: {:?}",
            subscriber.events()
        );
    }
}
