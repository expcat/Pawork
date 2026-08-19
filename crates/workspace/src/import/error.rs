//! 兼容加载器的错误类型。
//!
//! 所有错误消息只允许包含相对路径与字段名，绝不包含文件正文、命令参数或
//! Secret 明文；解析层的问题走 [`super::model::CompatIssue`] 而非本错误类型。

use std::path::PathBuf;

/// 加载 / 应用阶段的硬错误。单个源文件的格式问题不会走到这里：
/// 它们被隔离为诊断项（[`super::model::CompatIssue`]），保证一个坏文件
/// 不会拖垮整批导入。
#[derive(Debug, thiserror::Error)]
pub enum CompatError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("out of bounds path: {0}")]
    OutOfBounds(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("limit exceeded: {0}")]
    LimitExceeded(String),

    #[error("unsafe target path: {0}")]
    UnsafeTarget(String),
}

impl CompatError {
    pub(crate) fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
