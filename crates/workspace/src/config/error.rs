//! 配置加载与解析错误。

use std::path::PathBuf;

use thiserror::Error;

/// 单个配置文件解析失败。
///
/// 始终携带出错的文件路径，便于定位。
#[derive(Debug, Error)]
pub enum ConfigParseError {
    /// TOML 解析失败（语法错误等）。
    #[error("failed to parse config at {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    /// 反序列化到 schema 失败（类型/结构不匹配）。
    #[error("config at {path} does not match schema: {source}")]
    Schema {
        path: PathBuf,
        #[source]
        source: Box<serde_json::Error>,
    },
}

/// 配置加载过程中的整体错误。
#[derive(Debug, Error)]
pub enum ConfigError {
    /// 某个来源文件解析失败，携带该来源的路径。
    #[error(transparent)]
    Parse(#[from] ConfigParseError),

    /// IO 错误，携带受影响的路径。
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: Box<std::io::Error>,
    },

    /// 写盘时 TOML 序列化失败（SET-2 Global 层默认项 writer）。
    #[error("failed to write config at {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: Box<toml::ser::Error>,
    },
}

impl ConfigParseError {
    pub fn path(&self) -> &std::path::Path {
        match self {
            ConfigParseError::Toml { path, .. } | ConfigParseError::Schema { path, .. } => path,
        }
    }
}

impl ConfigError {
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            ConfigError::Parse(e) => Some(e.path()),
            ConfigError::Io { path, .. } | ConfigError::Write { path, .. } => Some(path),
        }
    }
}
