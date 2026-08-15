//! Pawork CLI：`chat` / `sessions` / `run` / `models`（含工具活动行与审批）。
//!
//! `--json`（unstable）：stdout 只承载 JSON；文本与日志走 stderr。
//! `--json` 或非 TTY 下审批 fail-closed（一律拒绝）。

mod approval;
mod chat;
mod error;
mod render;
mod sessions;

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use pawork_app::{
    parse_approval_mode, AppCore, AppError, AppLoadOptions, ApprovalPromptHost, DenyAllApprovals,
};
use thiserror::Error;

use crate::approval::InteractiveApprovals;

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
    /// 审批强度。默认 `read-only`（沿用 V1：不改模式就不会写入）。
    #[arg(
        long,
        global = true,
        value_name = "MODE",
        help = "always-ask|ask-for-writes|ask-for-dangerous|on-failure|never-ask|read-only"
    )]
    pub approval_mode: Option<String>,
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
    let mut options = AppLoadOptions::from_cli(cli.provider, cli.model);
    options.approval_mode = match cli.approval_mode.as_deref() {
        Some(value) => Some(parse_approval_mode(value).map_err(CliError::Usage)?),
        None => None,
    };
    options.approval_host = Some(approval_host(cli.json));
    let core = AppCore::load(options).await?;
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
    let catalog = core.model_catalog().await;
    if json {
        // unstable：沿 S1 约定，models 数组随 registry 目录演进为对象形状。
        let models: Vec<serde_json::Value> = catalog
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "id": entry.id.as_str(),
                    "provider": entry.provider.as_str(),
                    "display_name": entry.display_name.as_str(),
                    "context_window_tokens": entry.context_window_tokens,
                    "max_output_tokens": entry.max_output_tokens,
                    "pricing": &entry.pricing,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "provider": core.provider_id().as_str(),
                "models": models,
            })
        );
    } else {
        println!("provider: {}", core.provider_id());
        for entry in &catalog {
            let pricing = entry.pricing.as_ref().map_or_else(
                || "pricing n/a".to_string(),
                |pricing| {
                    format!(
                        "${}/${} per M {}",
                        micros_to_currency(pricing.input_per_mtoken_micros),
                        micros_to_currency(pricing.output_per_mtoken_micros),
                        pricing.currency
                    )
                },
            );
            println!(
                "  {:<36} window {:>10}  max output {:>10}  {}",
                entry.id.as_str(),
                display_tokens(entry.context_window_tokens),
                display_tokens(entry.max_output_tokens),
                pricing,
            );
        }
   }
    Ok(())
}

/// 0 表示目录未登记该字段，展示为 `-` 而非误导性的 0。
fn display_tokens(tokens: u64) -> String {
    if tokens == 0 {
        "-".to_string()
    } else {
        tokens.to_string()
    }
}

/// micros/Mtok → 货币单位（仅展示层转换，运算仍用整数 micros）。
fn micros_to_currency(micros: u64) -> String {
    format!("{}", micros as f64 / 1_000_000.0)
}

fn approval_host(json: bool) -> Arc<dyn ApprovalPromptHost> {
    if json || !std::io::stdin().is_terminal() {
        Arc::new(DenyAllApprovals)
    } else {
        Arc::new(InteractiveApprovals)
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
        assert!(cli.approval_mode.is_none());
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

    #[test]
    fn parses_approval_mode_kebab() {
        let cli = Cli::try_parse_from([
            "pawork",
            "--approval-mode",
            "ask-for-writes",
            "run",
            "hi",
        ])
        .expect("parse");
        assert_eq!(cli.approval_mode.as_deref(), Some("ask-for-writes"));
        assert_eq!(
            parse_approval_mode(cli.approval_mode.as_deref().expect("mode"))
                .expect("known"),
            pawork_app::ApprovalMode::AskForWrites
        );
    }

    #[test]
    fn rejects_unknown_approval_mode_string() {
        let err = parse_approval_mode("yolo").expect_err("unknown");
        assert!(err.contains("unknown approval mode"));
    }
}
