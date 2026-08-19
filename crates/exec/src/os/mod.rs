//! 平台进程树与沙箱后端。
//!
//! 纯函数（profile / argv / 配置生成、能力探测）在所有平台编译；
//! spawn 后端与 FFI 探测按 `cfg` 门控。

pub mod linux;
pub mod macos;
pub mod windows;
