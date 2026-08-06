//! # Pawork Policy Engine（P4-9）
//!
//! 统一的策略裁决入口，负责：
//! - 审批模式（[`ApprovalMode`]）与裁决结果（[`PolicyDecision`]）；
//! - 工作区文件路径安全解析（[`resolve_workspace_path`]）：防穿越、绝对路径、
//!   symlink 跳出、`.git` 内部、设备/FIFO/socket 与 TOCTOU；
//! - Shell 高风险命令识别（[`classify_command`]）；
//! - 综合 [`PolicyEngine`]：按模式 + 能力 + 信任 + 命令风险给出裁决。
//!
//! 路径安全函数收 `roots` 切片，**不依赖** workspace-service，保持解耦。

mod decision;
mod engine;
mod mode;
mod path;
mod shell;

pub use decision::{ApprovalPrompt, CommandRisk, ExecutionConstraints, PolicyDecision, RiskLevel};
pub use engine::{PolicyEngine, PolicyInput};
pub use mode::ApprovalMode;
pub use path::{resolve_workspace_path, PathSafetyError, ResolvedPath};
pub use shell::classify_command;
