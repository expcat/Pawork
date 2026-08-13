//! 四档可替换后端（P17-10）。
//!
//! 每个后端在构造时声明其 canonical 执行位点与信任边界；facade 据此路由，不读
//! Provider 名。Local / Playwright / 本地 MCP 进程属 CoreOwned（必要时经注入的
//! `SandboxBackend` spawn 子进程）；ProviderHosted 属 ExternallyOwned，结果走
//! `ServerToolEvent`。
pub mod local;
pub mod mcp;
pub mod playwright;
pub mod provider_hosted;

pub use local::{LocalBackend, LocalDriver, StubLocalDriver};
pub use mcp::{McpBackend, McpDriver, McpOwnership};
pub use playwright::{PlaywrightBackend, PlaywrightDriver, StubPlaywrightDriver};
pub use provider_hosted::{
    screenshot_event, CanonicalHostedEmitter, HostedComputerEventEmitter, ProviderHostedBackend,
};
