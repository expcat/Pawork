//! `pawork` 命令行的稳定解析模型。

use clap::{Args, Parser, Subcommand};

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
}
