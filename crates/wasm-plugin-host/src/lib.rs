//! Pawork capability-based WASM Component plugin host.
//!
//! Phase 10（P10-2 / P10-3 / P10-4 / P10-5）的最小完整实现：
//! - [`host::WasmPluginHost`]：wasmtime 27 async Component Model 宿主，固定
//!   `invoke(string) -> string` JSON ABI；加载/卸载、fuel、`StoreLimits` 内存、
//!   epoch 驱动的超时与取消、每插件独立 Store 的崩溃隔离。
//! - [`trust::TrustStore`]：Ed25519 验签，签名绑定 manifest 规范化 JSON +
//!   组件字节 blake3 摘要。
//! - [`state::InMemoryPluginStateStore`]：按 plugin+scope 隔离、乐观 revision、
//!   带配额的可注入状态存储。
//! - [`registry`]：命名空间化的工具/命令注册（统一 `ExternalPlugin`、不覆盖同名）。
//! - [`runtime::PluginRuntime`]：原子协调 component 与工具/命令/hook 的发布、派发和撤销。
//!
//! 安全边界：默认 Linker 不注入 WASI/host import，插件对 OS 文件/网络/进程零访问。

pub mod config;
pub mod host;
pub mod registry;
pub mod runtime;
pub mod state;
pub mod trust;

pub use config::{HostConfig, HostConfigError};
pub use host::{LoadedPlugin, WasmPluginHost};
pub use registry::{
    external_tool_name, ExternalCommandCaller, ExternalPluginToolAdapter, ExternalToolCaller,
    NamespacedToolRegistry, PluginCommandRegistry,
};
pub use runtime::PluginRuntime;
pub use state::{InMemoryPluginStateStore, PluginStateError, PluginStateStore};
pub use trust::{SignatureError, TrustStore};
