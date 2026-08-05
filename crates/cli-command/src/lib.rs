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
    Publish,
    Unpublish,
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
    Install,
    Start,
    Stop,
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
            &["pawork", "approval", "approve", "tool-1"],
            &["pawork", "provider", "list"],
            &["pawork", "auth", "login", "openai"],
            &["pawork", "plugin", "list"],
            &["pawork", "mcp", "doctor"],
        ];
        for command in commands {
            Cli::try_parse_from(*command).expect("nested command parses");
        }
    }
}
