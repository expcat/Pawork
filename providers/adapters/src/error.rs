//! 模型目录错误类型（迁自 V1 `model-registry::error`）。

use thiserror::Error;

/// 模型目录操作错误。
#[derive(Debug, Error)]
pub enum RegistryError {
    /// 指定的模型 ID 或别名未在目录中找到。
    #[error("model not found: {0}")]
    NotFound(String),

    /// 注册时遇到重复别名。
    #[error("duplicate alias {alias} already maps to {existing}")]
    DuplicateAlias { alias: String, existing: String },

    /// 重复注册的模型 ID（`extend_with` 的显式覆盖路径除外）。
    #[error("duplicate model id: {0}")]
    DuplicateModelId(String),
}
