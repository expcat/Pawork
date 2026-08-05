//! `pawork` 唯一正式宿主的进程内装配层。

use std::{
    io::{self, BufRead},
    sync::Arc,
};

use app_service::{AppService, ServiceOperation, ServiceRequest, ServiceResponse};
use cli_command::{Cli, Command, RunCommand};
use cli_renderer::{render, OutputFormat};
use core_api::CommandSource;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostOutcome {
    pub output: String,
    pub exit_code: i32,
}

pub struct CliHost {
    service: Arc<AppService>,
}

impl CliHost {
    pub fn new(service: Arc<AppService>) -> Self {
        Self { service }
    }

    pub async fn execute(&self, cli: Cli) -> HostOutcome {
        let format = if cli.json {
            OutputFormat::Json
        } else {
            OutputFormat::Text
        };

        let (operation, wait_for_signal) = match cli.command {
            Command::Serve(args) => (ServiceOperation::Serve, !args.once),
            Command::Shell => return self.shell(format),
            Command::Run(args) => match args.command {
                Some(RunCommand::Cancel { run_id }) => {
                    (placeholder("run.cancel", vec![run_id]), false)
                }
                None => (
                    ServiceOperation::Run {
                        workspace: args.workspace,
                        prompt: args.prompt,
                        keep_serving: args.serve,
                    },
                    false,
                ),
            },
            Command::Watch => (ServiceOperation::Watch, false),
            Command::Status => (ServiceOperation::Status, false),
            Command::Shutdown => (ServiceOperation::Shutdown, false),
            Command::Doctor => (ServiceOperation::Doctor, false),
            other => (placeholder_for_command(other), false),
        };

        let response = self.dispatch(operation);
        let initial_output = render(&response, format);
        if wait_for_signal {
            if let Err(error) = tokio::signal::ctrl_c().await {
                return HostOutcome {
                    output: format!("{initial_output}\nfailed to listen for Ctrl-C: {error}"),
                    exit_code: 1,
                };
            }
            self.dispatch(ServiceOperation::Shutdown);
        }
        HostOutcome {
            output: initial_output,
            exit_code: i32::from(!response.ok),
        }
    }

    fn dispatch(&self, operation: ServiceOperation) -> ServiceResponse {
        self.service.dispatch(ServiceRequest {
            source: CommandSource::LocalCli {
                terminal_session_id: None,
            },
            operation,
        })
    }

    fn shell(&self, format: OutputFormat) -> HostOutcome {
        let ready = self.dispatch(ServiceOperation::Shell);
        let mut outputs = vec![render(&ready, format)];
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else {
                return HostOutcome {
                    output: outputs.join("\n"),
                    exit_code: 1,
                };
            };
            match line.trim() {
                "/quit" | "/exit" => break,
                "/status" => outputs.push(render(&self.dispatch(ServiceOperation::Status), format)),
                "/doctor" => outputs.push(render(&self.dispatch(ServiceOperation::Doctor), format)),
                "" => {}
                command => outputs.push(render(
                    &self.dispatch(placeholder("shell", vec![command.to_owned()])),
                    format,
                )),
            }
        }
        HostOutcome {
            output: outputs.join("\n"),
            exit_code: 0,
        }
    }
}

fn placeholder(command: &str, arguments: Vec<String>) -> ServiceOperation {
    ServiceOperation::Placeholder {
        command: command.into(),
        arguments,
    }
}

fn placeholder_for_command(command: Command) -> ServiceOperation {
    let name = match command {
        Command::Workspace(_) => "workspace",
        Command::Session(_) => "session",
        Command::Approval(_) => "approval",
        Command::Gui(_) => "gui",
        Command::Remote(_) => "remote",
        Command::Provider(_) => "provider",
        Command::Auth(_) => "auth",
        Command::Plugin(_) => "plugin",
        Command::Mcp(_) => "mcp",
        Command::Models(_) => "models",
        Command::Tools(_) => "tools",
        Command::Service(_) => "service",
        Command::ImportPi { .. } => "import-pi",
        Command::Benchmark => "benchmark",
        Command::Serve(_)
        | Command::Shell
        | Command::Run(_)
        | Command::Watch
        | Command::Status
        | Command::Shutdown
        | Command::Doctor => unreachable!("handled before placeholder mapping"),
    };
    placeholder(name, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[tokio::test]
    async fn doctor_uses_direct_app_service_route() {
        let service = Arc::new(AppService::new("test"));
        let host = CliHost::new(Arc::clone(&service));
        let cli = Cli::try_parse_from(["pawork", "--json", "doctor"]).expect("parse");
        let outcome = host.execute(cli).await;
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(service.source_count("local_cli"), 1);
        let output: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(output["kind"], "doctor");
    }
}
