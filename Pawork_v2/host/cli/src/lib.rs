//! Pawork CLI 入口：`chat` / `models`，全局 `--provider` / `--model`。

mod chat;
mod error;
mod render;

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
#[command(name = "pawork", version, about = "Pawork — 最小可对话 CLI")]
pub struct Cli {
    /// 覆盖 config 中的 default_provider（配置里的 provider id）
    #[arg(long, short = 'p', global = true)]
    pub provider: Option<String>,
    /// 覆盖 config 中的 default_model（上游 model id）
    #[arg(long, short = 'm', global = true)]
    pub model: Option<String>,
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
    },
    /// 列出当前 provider 的模型目录
    Models,
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
    let core = AppCore::load(AppLoadOptions::from_cli(cli.provider, cli.model))?;
    match cli.command {
        Command::Chat { prompt } => chat::run_chat(&core, prompt).await,
        Command::Models => run_models(&core).await,
    }
}

async fn run_models(core: &AppCore) -> Result<(), CliError> {
    match core.list_models().await {
        Ok(models) => {
            println!("provider: {}", core.provider_id());
            for model in models {
                println!("{}", model.id);
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
        match cli.command {
            Command::Chat { prompt } => assert_eq!(prompt.as_deref(), Some("hi")),
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
