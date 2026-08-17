use std::path::{Path, PathBuf};

/// 缺省实例名（与 V1 `--instance` 默认值对齐）。
pub const DEFAULT_INSTANCE: &str = "default";

/// 会话库所在数据目录。覆盖：`PAWORK_DATA_DIR`。
///
/// 默认与 V1 `instance_dir` 对齐：Windows `%LOCALAPPDATA%\pawork`，
/// 其他平台 `~/.pawork`，再否则临时目录。
pub fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PAWORK_DATA_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if cfg!(windows) {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local).join("pawork");
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".pawork");
    }
    std::env::temp_dir().join("pawork")
}

/// 校验 `--instance`：非空、不能含路径分隔或 `..`。
pub fn normalize_instance(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("instance name must not be empty".into());
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
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
    }

    #[test]
    fn normalize_instance_rejects_path_escape() {
        assert_eq!(normalize_instance("default").expect("ok"), "default");
        assert!(normalize_instance("").is_err());
        assert!(normalize_instance("../x").is_err());
        assert!(normalize_instance("a/b").is_err());
    }
}
