//! 跨平台配置目录定位与工作区配置发现。

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

/// Pawork 的应用标识，用于推导标准配置目录。
pub const APP_QUALIFIER: &str = "dev";
pub const APP_ORGANIZATION: &str = "pawork";
pub const APP_APPLICATION: &str = "pawork";

/// 全局配置文件名（位于全局配置目录下）。
pub const GLOBAL_CONFIG_FILENAME: &str = "config.toml";

/// 工作区配置目录名（位于工作区根）。
pub const WORKSPACE_CONFIG_DIR: &str = ".pawork";
/// 工作区配置文件名（位于工作区 `.pawork/` 目录下）。
pub const WORKSPACE_CONFIG_FILENAME: &str = "config.toml";

/// 返回该应用的标准全局配置目录。
///
/// 跨平台语义（由 `directories` crate 提供）：
/// - Linux: `~/.config/pawork/`
/// - macOS: `~/Library/Application Support/dev.pawork.pawork/`
/// - Windows: `%APPDATA%\pawork\pawork\config\`
pub fn config_dir_for_app() -> Option<PathBuf> {
    ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_APPLICATION)
        .map(|dirs| dirs.config_dir().to_path_buf())
}

/// 全局配置文件的完整路径。
pub fn global_config_path() -> Option<PathBuf> {
    config_dir_for_app().map(|dir| dir.join(GLOBAL_CONFIG_FILENAME))
}

/// 从给定根目录推导工作区配置文件路径。
pub fn workspace_config_path(root: &Path) -> PathBuf {
    root.join(WORKSPACE_CONFIG_DIR)
        .join(WORKSPACE_CONFIG_FILENAME)
}

/// 从 `start` 向上逐级查找最近的工作区配置文件。
///
/// 返回首个包含 `.pawork/config.toml` 的祖先目录所对应的配置文件路径。
/// 若一直到根都没有找到，返回 `None`。
pub fn locate_workspace_config(start: &Path) -> Option<PathBuf> {
    let canonical = dunce::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    for ancestor in canonical.ancestors() {
        let candidate = workspace_config_path(ancestor);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 默认的搜索根集合（全局 + 工作区），用于在无显式来源时构造加载器输入。
///
/// 顺序仅供可读性；合并阶段会按 source key 重新排序，不依赖此顺序。
pub fn default_search_roots(workspace_root: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(global) = global_config_path() {
        roots.push(global);
    }
    if let Some(root) = workspace_root {
        if let Some(ws) = locate_workspace_config(root) {
            roots.push(ws);
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_config_path_is_stable() {
        let root = Path::new("/tmp/proj");
        let p = workspace_config_path(root);
        assert!(
            p.ends_with(".pawork/config.toml") || p.ends_with(".pawork\\config.toml"),
            "unexpected workspace config path: {p:?}"
        );
    }
}

#[cfg(test)]
mod paths_integration {
    use super::*;

    #[test]
    fn locate_finds_nearest_workspace_config_when_walking_up() {
        use std::fs;
        let tmp = tempfile_dir();
        // canonicalize：macOS 下 /var 是 /private/var 的符号链接，
        // locate 内部 canonicalize 返回 /private/var，需与期望路径一致。
        // 与实现一致使用 dunce（Windows 下不带 \\?\ 前缀）。
        let tmp = dunce::canonicalize(&tmp).unwrap_or(tmp);
        let nested = tmp.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();

        // 在中间目录 a 放置工作区配置。
        let mid = tmp.join("a");
        fs::create_dir_all(mid.join(WORKSPACE_CONFIG_DIR)).unwrap();
        fs::write(
            mid.join(WORKSPACE_CONFIG_DIR)
                .join(WORKSPACE_CONFIG_FILENAME),
            "",
        )
        .unwrap();

        let found = locate_workspace_config(&nested).unwrap();
        assert_eq!(
            found,
            mid.join(WORKSPACE_CONFIG_DIR)
                .join(WORKSPACE_CONFIG_FILENAME)
        );

        fs::remove_dir_all(&tmp).ok();
    }
}

#[cfg(test)]
fn tempfile_dir() -> std::path::PathBuf {
    let mut buf = [0u8; 16];
    // 简单确定性伪随机，避免引入 tempfile 依赖。
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0x9e3779b97f4a7c15) as u64;
    let mut state = seed;
    for byte in buf.iter_mut() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *byte = (state >> 33) as u8;
    }
    let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("pawork-config-test-{hex}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
