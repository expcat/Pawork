//! 平台原生硬隔离后端。
//!
//! 每个后端模块的纯函数（profile / argv / 配置生成、能力探测）在所有平台编译，
//! 以支持 L0 三平台单测；spawn 后端与 FFI 探测按 `cfg` 门控，仅在对应平台编译。
//!
//! 边界声明：landlock LSM 规则集应用与 AppContainer 受限令牌 spawn 均需扩展
//! process-runtime（pre_exec hook / `EXTENDED_STARTUPINFO`），本阶段不伪称硬隔离——
//! 这些能力要么以纯函数 + 真实探测呈现，要么在 [`crate::SandboxSelector::pick`]
//! 中明确降级并写入 [`crate::BackendSelection`] 供审计。

pub mod linux;
pub mod macos;
pub mod windows;
