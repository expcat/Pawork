//! Pawork Model Context Protocol client.
//!
//! `rmcp` is isolated inside this crate. Agent Core consumes canonical [`tool_api`] tools and
//! transport-independent capability snapshots rather than depending on SDK-specific types.

pub mod capabilities;
pub mod config;
mod error;
pub mod manager;
pub mod oauth;
pub mod security;
mod session;
pub mod transport;

pub use error::McpError;
pub use session::{McpPeer, McpServerCapabilities};
