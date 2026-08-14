//! Pawork CLI：`chat` / `sessions` / `run` / `models`。
//!
//! `--json`（unstable）：stdout 只承载 JSON；文本与日志走 stderr。

mod chat;
mod error;
mod render;
mod sessions;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use pawork_app::{AppCore, AppError, AppLoadOptions};
use thiserror::Error;

pub use error::format_provider_error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    App(#[from] AppError),
    #[error("{0}")]
    Turn(String),
    #[error("{0}")]
    Usage(String),
    #[error("已取消")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Parser, Debug)]
#[command(name = "pawork", version, about = "Pawork — 可对话 CLI（会话可落盘）")]
pub struct Cli {
    /// 覆盖 config 中的 default_provider（配置里的 provider id）
    #[arg(long, short = 'p', global = true)]
    pub provider: Option<String>,
    /// 覆盖 config 中的 default_model（上游 model id）
    #[arg(long, short = 'm', global = true)]
    pub model: Option<String>,
    /// 机器可读输出（unstable）。chat/run：stdout 为 AgentEventEnvelope JSONL。
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// 流式多轮对话
    Chat {
        /// 单次提问后退出（非 REPL）
        #[arg(long)]
        prompt: Option<String>,
        /// 续聊：完整 session id、唯一前缀，或 `latest`
        #[arg(long)]
        resume: Option<String>,
    },
    /// 列出 / 查看已落盘会话
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
    },
    /// 非交互单次任务（unstable `--json` 输出 envelope JSONL）
    Run {
        /// 用户提示（位置参数）
        prompt: String,
    },
    /// 列出当前 provider 的模型目录
    Models,
}

#[derive(Subcommand, Debug)]
pub enum SessionsCommand {
    /// 按更新时间列出未归档会话
    List,
    /// 显示会话元数据与投影消息
    Show { session: String },
}

pub async fn run() -> ExitCode {
    match run_inner().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

async fn run_inner() -> Result<(), CliError> {
    let cli = Cli::parse();
    let core = AppCore::load(AppLoadOptions::from_cli(cli.provider, cli.model)).await?;
    let result = match cli.command {
        Command::Chat { prompt, resume } => {
            chat::run_chat(&core, prompt, resume, cli.json).await
        }
        Command::Sessions { command } => sessions::run_sessions(&core, command, cli.json).await,
        Command::Run { prompt } => chat::run_once(&core, &prompt, cli.json).await,
        Command::Models => run_models(&core, cli.json).await,
    };
    let shutdown = core.shutdown().await;
    result?;
    shutdown?;
    Ok(())
}

async fn run_models(core: &AppCore, json: bool) -> Result<(), CliError> {
    match core.list_models().await {
        Ok(models) => {
            if json {
                let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
                println!(
                    "{}",
                    serde_json::json!({
                        "provider": core.provider_id().as_str(),
                        "models": ids,
                    })
                );
            } else {
                println!("provider: {}", core.provider_id());
                for model in models {
                    println!("{}", model.id);
                }
            }
            Ok(())
        }
        Err(err) => Err(CliError::Turn(format_provider_error(&err))),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn parses_global_provider_model_and_chat_prompt() {
        let cli = Cli::try_parse_from([
            "pawork",
            "--provider",
            "opencode-go",
            "--model",
            "deepseek-v4-pro",
            "chat",
            "--prompt",
            "hi",
        ])
        .expect("parse");
        assert_eq!(cli.provider.as_deref(), Some("opencode-go"));
        assert_eq!(cli.model.as_deref(), Some("deepseek-v4-pro"));
        assert!(!cli.json);
        match cli.command {
            Command::Chat { prompt, resume } => {
                assert_eq!(prompt.as_deref(), Some("hi"));
                assert!(resume.is_none());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_sessions_run_resume_and_json() {
        let cli = Cli::try_parse_from([
            "pawork",
            "--json",
            "chat",
            "--resume",
            "latest",
        ])
        .expect("parse");
        assert!(cli.json);
        match cli.command {
            Command::Chat { resume, .. } => assert_eq!(resume.as_deref(), Some("latest")),
            other => panic!("unexpected {other:?}"),
        }

        let cli = Cli::try_parse_from(["pawork", "sessions", "list"]).expect("parse");
        assert!(matches!(
            cli.command,
            Command::Sessions {
                command: SessionsCommand::List
            }
        ));

        let cli = Cli::try_parse_from(["pawork", "sessions", "show", "ses-1"]).expect("parse");
        match cli.command {
            Command::Sessions {
                command: SessionsCommand::Show { session },
            } => assert_eq!(session, "ses-1"),
            other => panic!("unexpected {other:?}"),
        }

        let cli = Cli::try_parse_from(["pawork", "run", "explain this"]).expect("parse");
        match cli.command {
            Command::Run { prompt } => assert_eq!(prompt, "explain this"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_models_with_short_flags() {
        let cli = Cli::try_parse_from(["pawork", "-p", "glm-coding", "models"]).expect("parse");
        assert_eq!(cli.provider.as_deref(), Some("glm-coding"));
        assert!(matches!(cli.command, Command::Models));
    }
}
