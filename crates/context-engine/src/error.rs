//! context-engine 的错误类型。

use thiserror::Error;

/// 构建上下文或构造 Token 估算器时发生的错误。
///
/// 目前唯一来源是无法为目标模型获得精确 tokenizer（如 OpenAI 系模型名无法识别或
/// BPE 数据加载失败）；此时调用方应回退到 [`crate::HeuristicEstimator`]。
///
/// 注意：错误明细用 `detail`（而非 thiserror 保留的 `source`），避免要求字段实现
/// `std::error::Error`。
#[derive(Debug, Error)]
pub enum ContextBuildError {
    /// 指定模型无法获得精确 tokenizer。
    #[error("token estimator unavailable for model `{model}`: {detail}")]
    TokenizerUnavailable { model: String, detail: String },
}

impl ContextBuildError {
    pub(crate) fn tokenizer_unavailable(model: &str, detail: impl Into<String>) -> Self {
        Self::TokenizerUnavailable {
            model: model.to_string(),
            detail: detail.into(),
        }
    }
}
