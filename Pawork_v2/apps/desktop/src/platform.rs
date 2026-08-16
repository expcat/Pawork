//! 平台层：默认 socket 路径发现 + tokio Runtime 宿主。
//!
//! 只做这两件事；不触碰 GUI 与业务协议（gui-design 四层约束）。

use std::path::PathBuf;

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
/// 语义镜像 pawork_app::default_data_dir（对照 Pawork_v2/host/app/src/data_dir.rs
/// 与 Pawork_v2/host/cli/src/gui.rs 的 serve 端路径），但按分层约束不依赖
/// pawork-app crate：PAWORK_DATA_DIR →（Windows）%LOCALAPPDATA%/pawork →
/// $HOME/.pawork → 临时目录/pawork。
pub fn default_socket_path() -> PathBuf {
    default_data_dir().join("pawork-gui.sock")
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

    #[test]
    fn default_socket_path_lives_under_data_dir() {
        let path = default_socket_path();
        assert!(
            path.ends_with("pawork-gui.sock"),
            "unexpected socket path: {path:?}"
        );
    }
}
