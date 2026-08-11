//! `pawork` 命令行的稳定解析模型。

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::str::FromStr;

#[derive(Clone, Debug, Parser, PartialEq, Eq)]
#[command(name = "pawork", version, about = "Pawork Core 的唯一正式宿主")]
pub struct Cli {
    /// 输出稳定、机器可解析的 JSON。
    #[arg(long, global = true)]
    pub json: bool,

    /// 选择隔离的 Core 实例。
    #[arg(long, global = true, default_value = "default")]
    pub instance: String,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    Serve(ServeArgs),
    Shell,
    Run(RunArgs),
    Watch,
    Status,
    Shutdown,
    Usage(UsageArgs),
    Workspace(Nested<WorkspaceCommand>),
    Session(Nested<SessionCommand>),
    Approval(Nested<ApprovalCommand>),
    Gui(Nested<GuiCommand>),
    Remote(Nested<RemoteCommand>),
    Provider(Nested<ListCommand>),
    Auth(Nested<AuthCommand>),
    Plugin(Nested<ListCommand>),
    Mcp(Nested<McpCommand>),
    Models(Nested<ListCommand>),
    Tools(Nested<ListCommand>),
    Service(Nested<ServiceCommand>),
    Doctor,
    ImportPi { path: String },
    Benchmark,
}

#[derive(Clone, Debug, Args, PartialEq, Eq)]
pub struct Nested<T: Subcommand> {
    #[command(subcommand)]
    pub command: T,
}

#[derive(Clone, Debug, Args, PartialEq, Eq)]
pub struct ServeArgs {
    /// 初始化后立即退出，供自动化烟雾测试使用。
    #[arg(long)]
    pub once: bool,
}

/// `pawork usage` 参数（P14-8）：默认 legacy 作用域 local / local/default。
#[derive(Clone, Debug, Args, PartialEq, Eq)]
pub struct UsageArgs {
    /// 隔离租户（缺省 `local`）。
    #[arg(long)]
    pub tenant: Option<String>,

    /// 账户（缺省 `local/default`）。
    #[arg(long)]
    pub account: Option<String>,

    /// 过滤到指定 Provider。
    #[arg(long)]
    pub provider: Option<String>,

    /// 凭证元数据 ID（opaque，绝非凭证值）。
    #[arg(long)]
    pub credential: Option<String>,

    /// 过滤到指定模型。
    #[arg(long)]
    pub model: Option<String>,

    /// 窗口：overall | rolling5h | weekly | monthly（可多次指定或以逗号分隔）。
    #[arg(long, value_delimiter = ',')]
    pub window: Vec<UsageWindow>,

    /// 单位：count | token | cost:<3位ASCII币种>（严格解析，无效值在边界报错）。
    #[arg(long)]
    pub unit: Option<UsageUnit>,
}

/// `pawork usage --window` 可选窗口（clap typed enum，边界拒绝无效值）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum UsageWindow {
    Overall,
    Rolling5h,
    Weekly,
    Monthly,
}

/// `pawork usage --unit` 可选单位：count | token | cost:<3位ASCII币种>。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsageUnit {
    Count,
    Token,
    Cost { currency: String },
}

impl FromStr for UsageUnit {
    type Err = String;

    /// 严格解析：token/count 大小写不敏感；`cost:` 后必须恰好 3 位 ASCII 字母
    /// （ISO-4217 形态），其余一律报错，由 clap 在解析边界拒绝。
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("token") {
            Ok(Self::Token)
        } else if trimmed.eq_ignore_ascii_case("count") {
            Ok(Self::Count)
        } else if let Some(currency) = trimmed.strip_prefix("cost:") {
            if currency.len() == 3
                && currency.is_ascii()
                && currency.bytes().all(|byte| byte.is_ascii_alphabetic())
            {
                Ok(Self::Cost {
                    currency: currency.to_string(),
                })
            } else {
                Err(format!(
                    "cost:<币种> 需要恰好 3 位 ASCII 字母，收到 `{currency}`"
                ))
            }
        } else {
            Err(format!(
                "无效单位 `{trimmed}`：支持 count | token | cost:<3位ASCII币种>"
            ))
        }
    }
}

impl UsageArgs {
    /// 缺省 tenant（与 core-api `DEFAULT_QUOTA_TENANT` 一致）。
    pub const DEFAULT_TENANT: &'static str = "local";
    /// 缺省 account（与 core-api `DEFAULT_QUOTA_ACCOUNT` 一致）。
    pub const DEFAULT_ACCOUNT: &'static str = "local/default";

    /// 解析后的 tenant（缺省 `local`）。
    pub fn tenant_or_default(&self) -> String {
        self.tenant
            .clone()
            .unwrap_or_else(|| Self::DEFAULT_TENANT.to_string())
    }

    /// 解析后的 account（缺省 `local/default`）。
    pub fn account_or_default(&self) -> String {
        self.account
            .clone()
            .unwrap_or_else(|| Self::DEFAULT_ACCOUNT.to_string())
    }
}

#[derive(Clone, Debug, Args, PartialEq, Eq)]
pub struct RunArgs {
    #[command(subcommand)]
    pub command: Option<RunCommand>,

    #[arg(long)]
    pub workspace: Option<String>,

    #[arg(long)]
    pub prompt: Option<String>,

    #[arg(long)]
    pub serve: bool,
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum RunCommand {
    Cancel { run_id: String },
    Retry { run_id: String },
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum WorkspaceCommand {
    List,
    Add { path: String },
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum SessionCommand {
    List,
    Open { session_id: String },
    Export { session_id: String, output: String },
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum ApprovalCommand {
    List,
    Approve { tool_call_id: String },
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum GuiCommand {
    Clients,
    Disconnect { client_id: String },
    Endpoint,
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum RemoteCommand {
    /// 发布一个远程 GUI 端点（P13-6 占位 Adapter）。
    Publish {
        /// 端点名称（缺省为 CLI 实例名）。
        #[arg(long)]
        name: Option<String>,
    },
    /// 撤销一个已发布的远程 GUI 端点（handle 来自 publish 输出）。
    Unpublish {
        /// publish 返回的 handle id。
        #[arg(long)]
        handle: String,
    },
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum ListCommand {
    List,
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum AuthCommand {
    Login { provider: String },
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum McpCommand {
    Doctor,
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum ServiceCommand {
    /// 注册为系统服务（默认 dry-run，仅打印注册计划；`--apply` 才真正修改系统）。
    Install {
        #[arg(long)]
        apply: bool,
    },
    Start {
        #[arg(long)]
        apply: bool,
    },
    Stop {
        #[arg(long)]
        apply: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_run_and_global_json_after_subcommand() {
        let cli = Cli::try_parse_from([
            "pawork",
            "run",
            "--workspace",
            ".",
            "--prompt",
            "fix tests",
            "--json",
        ])
        .expect("parse run");
        assert!(cli.json);
        assert!(matches!(cli.command, Command::Run(_)));
    }

    #[test]
    fn parses_nested_command_families() {
        let commands: &[&[&str]] = &[
            &["pawork", "workspace", "list"],
            &["pawork", "session", "open", "session-1"],
            &["pawork", "run", "cancel", "run-1"],
            &["pawork", "run", "retry", "run-1"],
            &["pawork", "approval", "approve", "tool-1"],
            &["pawork", "remote", "publish"],
            &["pawork", "remote", "publish", "--name", "edge"],
            &["pawork", "remote", "unpublish", "--handle", "edge-0"],
            &["pawork", "provider", "list"],
            &["pawork", "auth", "login", "openai"],
            &["pawork", "plugin", "list"],
            &["pawork", "mcp", "doctor"],
            &["pawork", "service", "install"],
            &["pawork", "service", "install", "--apply"],
            &["pawork", "service", "start"],
            &["pawork", "service", "stop"],
        ];
        for command in commands {
            Cli::try_parse_from(*command).expect("nested command parses");
        }
    }

    #[test]
    fn service_apply_flag_is_opt_in() {
        let dry = Cli::try_parse_from(["pawork", "service", "install"]).expect("dry-run install");
        let Command::Service(Nested {
            command: ServiceCommand::Install { apply },
        }) = &dry.command
        else {
            panic!("expected service install");
        };
        assert!(!apply, "install must default to dry-run");

        let applied =
            Cli::try_parse_from(["pawork", "service", "install", "--apply"]).expect("apply");
        let Command::Service(Nested {
            command: ServiceCommand::Install { apply },
        }) = &applied.command
        else {
            panic!("expected service install");
        };
        assert!(apply);
    }

    #[test]
    fn remote_publish_name_defaults_to_none_and_unpublish_requires_handle() {
        let publish = Cli::try_parse_from(["pawork", "remote", "publish"]).expect("publish");
        let Command::Remote(Nested {
            command: RemoteCommand::Publish { name },
        }) = &publish.command
        else {
            panic!("expected remote publish");
        };
        assert!(name.is_none(), "name must default to none");

        let named =
            Cli::try_parse_from(["pawork", "remote", "publish", "--name", "edge"]).expect("named");
        let Command::Remote(Nested {
            command: RemoteCommand::Publish { name },
        }) = &named.command
        else {
            panic!("expected remote publish");
        };
        assert_eq!(name.as_deref(), Some("edge"));

        let unpublish =
            Cli::try_parse_from(["pawork", "remote", "unpublish", "--handle", "edge-0"])
                .expect("unpublish");
        let Command::Remote(Nested {
            command: RemoteCommand::Unpublish { handle },
        }) = &unpublish.command
        else {
            panic!("expected remote unpublish");
        };
        assert_eq!(handle, "edge-0");

        // unpublish 缺少 handle 解析失败。
        assert!(Cli::try_parse_from(["pawork", "remote", "unpublish"]).is_err());
    }

    #[test]
    fn parses_usage_defaults_and_filters() {
        let cli = Cli::try_parse_from(["pawork", "usage"]).expect("usage default");
        let Command::Usage(args) = &cli.command else {
            panic!("expected usage command");
        };
        assert_eq!(args.tenant_or_default(), "local");
        assert_eq!(args.account_or_default(), "local/default");
        assert!(args.provider.is_none());
        assert!(args.window.is_empty());
        assert!(args.unit.is_none());

        let filtered = Cli::try_parse_from([
            "pawork",
            "usage",
            "--tenant",
            "acme",
            "--account",
            "acme/team",
            "--provider",
            "anthropic",
            "--model",
            "claude",
            "--window",
            "monthly",
            "--unit",
            "token",
            "--credential",
            "key-123",
        ])
        .expect("usage filtered");
        let Command::Usage(args) = &filtered.command else {
            panic!("expected usage command");
        };
        assert_eq!(args.tenant.as_deref(), Some("acme"));
        assert_eq!(args.account.as_deref(), Some("acme/team"));
        assert_eq!(args.provider.as_deref(), Some("anthropic"));
        assert_eq!(args.model.as_deref(), Some("claude"));
        assert_eq!(args.window, vec![UsageWindow::Monthly]);
        assert_eq!(args.unit, Some(UsageUnit::Token));
        assert_eq!(args.credential.as_deref(), Some("key-123"));
    }

    #[test]
    fn usage_window_is_typed_and_rejects_invalid_values() {
        // 单次指定与逗号分隔等价，多次指定累积。
        let comma = Cli::try_parse_from(["pawork", "usage", "--window", "overall,rolling5h"])
            .expect("comma-separated windows");
        let Command::Usage(args) = &comma.command else {
            panic!("expected usage command");
        };
        assert_eq!(
            args.window,
            vec![UsageWindow::Overall, UsageWindow::Rolling5h]
        );

        let repeated = Cli::try_parse_from([
            "pawork", "usage", "--window", "weekly", "--window", "monthly",
        ])
        .expect("repeated windows");
        let Command::Usage(args) = &repeated.command else {
            panic!("expected usage command");
        };
        assert_eq!(args.window, vec![UsageWindow::Weekly, UsageWindow::Monthly]);

        // 无效窗口在解析边界被拒绝。
        for invalid in ["hourly", "daily", "Monthly ", ""] {
            let parsed = Cli::try_parse_from(["pawork", "usage", "--window", invalid]);
            assert!(parsed.is_err(), "window `{invalid}` must be rejected");
        }
    }

    #[test]
    fn usage_unit_is_strict_and_rejects_invalid_values() {
        // token/count 大小写不敏感。
        for raw in ["token", "TOKEN", "count", "Count"] {
            let cli = Cli::try_parse_from(["pawork", "usage", "--unit", raw]).expect("unit parses");
            let Command::Usage(args) = &cli.command else {
                panic!("expected usage command");
            };
            let expected = if raw.eq_ignore_ascii_case("token") {
                UsageUnit::Token
            } else {
                UsageUnit::Count
            };
            assert_eq!(args.unit, Some(expected), "unit `{raw}`");
        }

        // cost 接受恰好 3 位 ASCII 字母币种。
        let cli =
            Cli::try_parse_from(["pawork", "usage", "--unit", "cost:USD"]).expect("cost parses");
        let Command::Usage(args) = &cli.command else {
            panic!("expected usage command");
        };
        assert_eq!(
            args.unit,
            Some(UsageUnit::Cost {
                currency: "USD".into()
            })
        );

        // 无效单位在解析边界被拒绝。
        for invalid in [
            "tokens",
            "usd",
            "cost",
            "cost:",
            "cost:US",
            "cost:USDollar",
            "cost:USD1",
            "cost:U$D",
        ] {
            let parsed = Cli::try_parse_from(["pawork", "usage", "--unit", invalid]);
            assert!(parsed.is_err(), "unit `{invalid}` must be rejected");
        }
    }
}
