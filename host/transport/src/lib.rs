//! GUI Transport 的业务无关抽象与本机 / 进程内 / 远程实现。
//!
//! Transport 只搬运有界字节帧。GUI Connection Protocol 的编解码位于
//! `pawork-protocol`，因此 Adapter 不依赖任何 Agent 领域类型（remote 仅使用
//! `pawork-protocol::client_auth` 的 token）。
//!
//! feature：
//! - `local`（默认）：Unix Domain Socket / Named Pipe
//! - `memory`：进程内 [`MemoryTransport`] 与 Mock Remote（不拉 rustls）
//! - `remote`：TCP + TLS 1.3（rustls 严格锁在本 feature）

mod api;
#[cfg(feature = "local")]
mod local;
#[cfg(feature = "memory")]
mod memory;
#[cfg(feature = "remote")]
mod remote;

pub use api::*;

#[cfg(feature = "local")]
pub use local::LocalTransport;

#[cfg(feature = "memory")]
pub use memory::{
    MemoryListener, MemoryTransport, MockRemoteConnector, MockRemoteListener, MockRemoteTransport,
    MockRemoteTransportProvider, MOCK_ADAPTER,
};

#[cfg(feature = "remote")]
pub use remote::{
    ClientConnection, RealRemoteConnector, RealRemoteListener, RealRemoteTransport,
    RealRemoteTransportConfig, RealRemoteTransportProvider, ResumeOutcome, ADAPTER_NAME,
    DEFAULT_MAX_BUFFERED_BYTES, DEFAULT_RESEND_WINDOW_FRAMES,
};
