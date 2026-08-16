//! GUI Transport 的业务无关抽象与本机实现。
//!
//! Transport 只搬运有界字节帧。GUI Connection Protocol 的编解码位于
//! `pawork-protocol`，因此 Local Adapter 不依赖任何 Agent 领域类型。
//! remote provider / connector 与 memory 测试矩阵延后到 S10。

mod api;
mod local;

pub use api::*;
pub use local::{LocalTransport, DEFAULT_MAX_FRAME_BYTES};
