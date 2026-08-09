//! Pawork 配置服务。
//!
//! 提供确定性的配置层级合并：内置 < 用户全局 < profile < 工作区 < session < run。
//! 合并不依赖文件扫描顺序，只按每层固定的内部 `source key` 排序；同层按 source key
//! 升序合并，后合并者覆盖先合并者。递归 object 合并，标量/数组整体替换。
//!
//! 参见 `docs/features/context.md` 的「Resource 优先级」与 `plan/P1-1-config.md`。

mod error;
mod loader;
mod merge;
mod paths;
mod schema;

pub use error::{ConfigError, ConfigParseError};
pub use loader::{
    ConfigSource, ConfigWarning, LoadedSource, LoadedSourceSpan, Loader, ResolvedConfig,
};
pub use merge::{merge_ordered, ConfigValue, Merge};
pub use paths::{config_dir_for_app, default_search_roots, locate_workspace_config};
pub use schema::{ModelConfig, PaworkConfig, ProviderConfig, RunOverrides, SessionOverrides};

/// 配置层级。
///
/// 优先级：`Builtin < Global < Profile < Workspace < Session < Run`。
/// 同一层级内仍可能存在多个来源（例如多个工作区根），此时按 `source key`
/// 升序合并，保证结果与扫描顺序无关。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    /// 该层级内置的稳定排序键，同层多来源合并时使用。
    pub fn source_key(self) -> &'static str {
        match self {
            ConfigTier::Builtin => "builtin",
            ConfigTier::Global => "global",
            ConfigTier::Profile => "profile",
            ConfigTier::Workspace => "workspace",
            ConfigTier::Session => "session",
            ConfigTier::Run => "run",
        }
    }
}
