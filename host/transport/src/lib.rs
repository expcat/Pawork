//! GUI Transport 的业务无关抽象与本机 / 进程内实现。
//!
//! Transport 只搬运有界字节帧。GUI Connection Protocol 的编解码位于
//! `pawork-protocol`，因此 Adapter 不依赖任何 Agent 领域类型。
//!
//! feature：
//! - `local`（默认）：Unix Domain Socket / Named Pipe
//! - `memory`：进程内 [`MemoryTransport`]

mod api;
#[cfg(feature = "local")]
mod local;
#[cfg(feature = "memory")]
mod memory;

pub use api::*;

#[cfg(feature = "local")]
pub use local::LocalTransport;

#[cfg(feature = "memory")]
pub use memory::{
    MemoryListener, MemoryTransport,
};
