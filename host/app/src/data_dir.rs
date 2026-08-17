use std::path::{Path, PathBuf};

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

/// `<data_dir>/default/session.db`（S1 不暴露 `--instance`）。
pub fn session_db_path(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join("default").join("session.db")
}

/// `<data_dir>/default/artifacts`：写前快照与回滚 Blob。
pub fn artifact_store_path(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join("default").join("artifacts")
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
    }
}
