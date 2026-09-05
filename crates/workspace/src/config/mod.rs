//! Pawork 配置服务。
//!
//! 提供确定性的配置层级合并：内置 < 用户全局 < profile < 工作区 < session < run。
//! 合并不依赖文件扫描顺序，只按每层固定的内部 `source key` 排序；同层按 source key
//! 升序合并，后合并者覆盖先合并者。递归 object 合并，标量/数组整体替换。
//!
//! S9 起 Session / Run 有一等 API（[`Loader::with_session`] / [`Loader::with_run`]）。
//! `discover` / `discover_from` 仍只自动加入 Builtin + Global 文件 + Workspace 文件；
//! Profile 仍由 `profile =` 与 `profiles[]` 派生（插在 Global 与 Workspace 之间）。
//! 配置 schema 不含 `api_key`；凭证 env 名与 auth 文件定位统一由
//! `pawork-auth` locator 定义，config 不再承载过渡实现。

mod error;
mod loader;
mod merge;
mod paths;
mod schema;
mod writer;

use serde::{Deserialize, Serialize};

pub use error::{ConfigError, ConfigParseError};
pub use loader::{
    ConfigSource, ConfigWarning, LoadedSource, LoadedSourceSpan, Loader, ResolvedConfig,
};
pub use merge::{merge_ordered, ConfigValue, Merge};
pub use paths::{
    config_dir_for_app, default_search_roots, global_config_path, locate_workspace_config,
    workspace_config_path,
};
pub use schema::{
    ModelConfig, PaworkConfig, ProfileConfig, ProfileOverrides, ProviderConfig, RunOverrides,
    SessionOverrides, TerminalConfig,
};
pub use writer::{
    write_default_model_pair, write_mcp_server_remove, write_provider_use_proxy,
    write_proxy_url,
    write_terminal_settings,
};

/// 配置层级。
///
/// 优先级：`Builtin < Global < Profile < Workspace < Session < Run`。
/// 同一层级内仍可能存在多个来源（例如多个工作区根），此时按 `source key`
/// 升序合并，保证结果与扫描顺序无关。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigTier {
    /// 内置默认值，优先级最低。
    Builtin,
    /// 用户全局配置。
    Global,
    /// 用户 Profile 配置。
    Profile,
    /// 工作区配置。
    Workspace,
    /// 当前 Session 配置。
    Session,
    /// 单次 Run 参数，优先级最高。
    Run,
}

impl ConfigTier {
    /// 数值越大优先级越高；用于跨 crate 构造稳定排序键。
    pub const fn priority(self) -> u8 {
        match self {
            ConfigTier::Builtin => 0,
            ConfigTier::Global => 1,
            ConfigTier::Profile => 2,
            ConfigTier::Workspace => 3,
            ConfigTier::Session => 4,
            ConfigTier::Run => 5,
        }
    }

    /// 该层级内置的稳定排序键，同层多来源合并时使用。
    pub const fn source_key(self) -> &'static str {
        match self {
            ConfigTier::Builtin => "builtin",
            ConfigTier::Global => "global",
            ConfigTier::Profile => "profile",
            ConfigTier::Workspace => "workspace",
            ConfigTier::Session => "session",
            ConfigTier::Run => "run",
        }
    }

    /// 稳定的诊断标识。
    pub const fn as_str(self) -> &'static str {
        self.source_key()
    }
}
