//! Pawork Plugin Package 格式（P17-2）。
//!
//! 一个可安装包（manifest + 归档）聚合六种扩展类型：Skills、Agents（profile）、
//! Hooks（[用户钩子](../../plan/P17-1-user-hooks.md)）、MCP server 声明、LSP server
//! 声明、Monitors（监视器声明）。Package 只做**聚合、校验、作用域绑定**，复用各类型
//! 既有的子 manifest 与运行时，不重定义其语义：
//!
//! - Skills / Agents / Hooks / LSP 子段声明相对路径或内联清单，由各既有 loader 加载；
//!   本 crate 不复制运行时，统一经 [`dispatch::PackageDispatchSink`] 把子资源分发到
//!   对应 loader（`resource-loader` / MCP / LSP / monitor-service）。
//! - MCP 子段中本地 stdio server 一律经 Sandbox Runtime → Process Runtime 托管
//!   （见 acceptance：restart 不得 unsandboxed）；本 crate 只声明，由 `mcp-client` 的
//!   sandboxed stdio spawner 实际执行。
//! - Monitors 子段只声明配置 / trigger / permissions / lifecycle / required capability，
//!   稳定 driver/evaluator 入口指向 `monitor_service`；实际执行统一进入
//!   `monitor-service` → `task-manager`（P16-6 / P16-10），package 不自带运行时。
//!
//! 依赖方向（[workspace-layout](../../docs/architecture/workspace-layout.md)）：
//! `agent-domain → plugin-package → resource-loader`；被 P17-3 marketplace 依赖。

pub mod archive;
pub mod conflict;
pub mod dispatch;
pub mod error;
mod fs_safe;
pub mod manifest;
pub mod monitor;
pub mod scope;
pub mod secret;

pub use archive::{read_archive, verify_archive, write_archive, PackageArchive};
pub use conflict::{
    detect_conflicts, ConflictIssue, ConflictKind, ConflictReport, ConflictScope, LoadedPackage,
};
pub use dispatch::{
    install_package, AgentProfileDispatch, DispatchPlan, DispatchSummary, HookDispatch,
    LanguageServerDispatch, McpDispatch, MonitorDispatch, PackageDispatchSink,
    RecordingDispatchSink, SkillDispatch,
};
pub use error::PackageError;
pub use manifest::{
    McpServerDeclaration, McpTransportSpec, PackageManifest, ResourceRef, MANIFEST_FILE_NAME,
};
pub use monitor::{MonitorDeclaration, MonitorDriverEntry, MonitorLifecycle, MonitorPermissions};
pub use scope::{
    PackageDependency, PackageId, PackageProvenance, PackageRelativePath, PackageScope,
};
pub use secret::SecretRef;

/// 当前 package manifest schema 版本。
pub const PACKAGE_MANIFEST_VERSION: u32 = 1;
