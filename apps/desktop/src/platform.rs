//! 平台层：默认 socket 路径发现 + tokio Runtime 宿主。
//!
//! 只做这两件事；不触碰 GUI 与业务协议（gui-design 四层约束）。

use std::path::PathBuf;

mod preferences;
pub use preferences::{load_preferences, save_preferences, DesktopPreferences};

/// 桌面壳持有的 tokio Runtime。
///
/// GUI Connection Protocol 的所有异步操作（连接、Command/Query 往返、事件泵）
/// 都跑在这个 multi_thread runtime 上；GPUI 侧只经 channel 消费结果。
/// Runtime 由 Platform 持有，AppView 保存 Arc 保证其存活。
pub struct Platform {
    runtime: tokio::runtime::Runtime,
}

impl Platform {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build pawork-desktop tokio runtime");
        Self { runtime }
    }

    pub fn handle(&self) -> tokio::runtime::Handle {
        self.runtime.handle().clone()
    }

    /// --probe 模式使用：在 runtime 上驱动一个 future 至完成。
    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }
}

impl Default for Platform {
    fn default() -> Self {
        Self::new()
    }
}

/// 默认 GUI socket 路径：<data_dir>/pawork-gui.sock。
///
/// 语义镜像 pawork_app::default_data_dir（对照 host/app/src/data_dir.rs
/// 与 host/cli/src/gui.rs 的 serve 端路径），但按分层约束不依赖
/// pawork-app crate：PAWORK_DATA_DIR →（Windows）%LOCALAPPDATA%/pawork →
/// $HOME/.pawork → 临时目录/pawork。
pub fn default_socket_path() -> PathBuf {
    socket_path_for_instance(None)
}

/// 默认 GUI token 路径：<data_dir>/gui.token（与 A5 `{data_dir}/gui.token` 对齐）。
pub fn default_token_path() -> PathBuf {
    token_path_for_instance(None)
}

/// 非 default 实例：`pawork-gui-{instance}.sock` / `gui-{instance}.token`。
pub fn socket_path_for_instance(instance: Option<&str>) -> PathBuf {
    default_data_dir().join(instance_file_name("pawork-gui", "sock", instance))
}

pub fn token_path_for_instance(instance: Option<&str>) -> PathBuf {
    default_data_dir().join(instance_file_name("gui", "token", instance))
}

/// 按 socket 文件名推断同目录 token（`pawork-gui.sock` → `gui.token`，
/// `pawork-gui-{instance}.sock` → `gui-{instance}.token`）。
pub fn token_path_for_socket(socket: &std::path::Path) -> PathBuf {
    let parent = socket
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir);
    let file_name = socket.file_name().and_then(|name| name.to_str());
    let token_name = match file_name {
        Some(name) if name.starts_with("pawork-gui-") && name.ends_with(".sock") => {
            let instance = &name["pawork-gui-".len()..name.len() - ".sock".len()];
            format!("gui-{instance}.token")
        }
        _ => "gui.token".into(),
    };
    parent.join(token_name)
}

fn instance_file_name(stem: &str, extension: &str, instance: Option<&str>) -> String {
    match instance
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != "default")
    {
        Some(name) => format!("{stem}-{name}.{extension}"),
        None => format!("{stem}.{extension}"),
    }
}

fn default_data_dir() -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn default_socket_path_lives_under_data_dir() {
        let path = default_socket_path();
        assert!(
            path.ends_with("pawork-gui.sock"),
            "unexpected socket path: {path:?}"
        );
    }

    #[test]
    fn default_token_path_aligns_with_a5_gui_token() {
        let path = default_token_path();
        assert!(
            path.ends_with("gui.token"),
            "unexpected token path: {path:?}"
        );
        assert_eq!(
            token_path_for_instance(Some("default")),
            default_token_path()
        );
        assert!(token_path_for_instance(Some("dev")).ends_with("gui-dev.token"));
        assert_eq!(
            token_path_for_socket(&PathBuf::from("/tmp/pawork-gui.sock")),
            PathBuf::from("/tmp/gui.token")
        );
        assert_eq!(
            token_path_for_socket(&PathBuf::from("/tmp/pawork-gui-dev.sock")),
            PathBuf::from("/tmp/gui-dev.token")
        );
    }

    #[test]
    fn desktop_production_pawork_deps_stay_client_only() {
        let manifest = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
        let deps = production_pawork_dependencies(manifest);
        assert_eq!(
            deps,
            ["pawork-client".to_string()].into_iter().collect(),
            "desktop production dependency boundary is client-only"
        );
    }

    #[test]
    fn production_dependency_scan_covers_alias_and_target_tables() {
        let manifest = r#"
[dependencies]
renamed-app = { path = "../../crates/app", package = "pawork-app" }

[target.'cfg(unix)'.dependencies]
pawork-storage = { path = "../../crates/storage" }

[dependencies.renamed-tools]
package = "pawork-tools"
path = "../../crates/tools"

[dev-dependencies]
pawork-engine = { path = "../../crates/engine" }
"#;
        assert_eq!(
            production_pawork_dependencies(manifest),
            ["pawork-app", "pawork-storage", "pawork-tools"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
    }

    fn production_pawork_dependencies(manifest: &str) -> std::collections::BTreeSet<String> {
        let mut in_production_dependencies = false;
        let mut dependencies = std::collections::BTreeSet::new();
        for raw in manifest.lines() {
            let line = raw.trim();
            if line.starts_with('[') && line.ends_with(']') {
                let section = line.trim_matches(['[', ']']);
                let dependency_suffix = section
                    .strip_prefix("dependencies.")
                    .or_else(|| {
                        section
                            .strip_prefix("target.")
                            .and_then(|section| section.rsplit_once(".dependencies."))
                            .map(|(_, dependency)| dependency)
                    })
                    .map(|dependency| dependency.trim_matches(['\'', '"']).to_string());
                in_production_dependencies = section == "dependencies"
                    || (section.starts_with("target.") && section.ends_with(".dependencies"))
                    || dependency_suffix.is_some();
                if let Some(dependency) = dependency_suffix
                    .as_ref()
                    .filter(|dependency| dependency.starts_with("pawork-"))
                {
                    dependencies.insert(dependency.clone());
                }
                continue;
            }
            if !in_production_dependencies || line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, _)) = line.split_once('=') {
                let key = key.trim().trim_matches(['\'', '"']);
                if key.starts_with("pawork-") {
                    dependencies.insert(key.to_string());
                }
            }
            if let Some(package) = line
                .split_once("package")
                .and_then(|(_, field)| field.split_once('='))
                .and_then(|(_, value)| {
                    value
                        .split('"')
                        .nth(1)
                        .filter(|package| package.starts_with("pawork-"))
                })
            {
                dependencies.insert(package.to_string());
            }
        }
        dependencies
    }
}
