//! Pawork CLI：`chat` / `sessions` / `run` / `models`（含工具活动行与审批）。
//!
//! `--json`（unstable）：stdout 只承载 JSON；文本与日志走 stderr。
//! `--json` 或非 TTY 下审批 fail-closed（一律拒绝）。

mod approval;
mod auth;
mod chat;
mod error;
mod gui;
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
    /// 凭证管理（auth 文件为主，env 为显式 fallback）
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// GUI 服务（S7 最小切片：本机单客户端）
    Gui {
        #[command(subcommand)]
        command: GuiCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionsCommand {
    /// 按更新时间列出未归档会话
    List,
    /// 显示会话元数据与投影消息
    Show { session: String },
}

#[derive(Subcommand, Debug)]
pub enum AuthCommand {
    /// 各通道凭证状态（只显示掩码与来源）
    List,
    /// 从 stdin 读入 API key 并写入 auth 文件 default 条目
    SetKey { provider: String },
    /// OAuth 登录（PKCE 回调或 Device Flow，等待 5 分钟）
    Login { provider: String },
    /// 删除 auth 文件 default 条目（不影响 env fallback）
    Logout { provider: String },
}

#[derive(Subcommand, Debug)]
pub enum GuiCommand {
    /// 启动本机 GUI 服务（单客户端，Unix socket / Named pipe）。
    Serve {
        /// 覆盖默认 socket 路径（默认在 Pawork 数据目录下）。
        #[arg(long)]
        socket: Option<std::path::PathBuf>,
    },
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
    if matches!(&cli.command, Command::Gui { .. }) {
        // GUI 宿主没有终端审批交互面：波 A 一律 fail-closed（写入类工具
        // 按审批模式拒绝），波 C 再接时间线内审批。
        options.approval_host = Some(Arc::new(DenyAllApprovals));
    }
    // 目录 / 凭证命令允许默认 provider 缺凭证（目录兜底装配）。
    let tolerant = matches!(
        &cli.command,
        Command::Models | Command::Sessions { .. } | Command::Auth { .. }
    );
    let mut core = if tolerant {
        AppCore::load_for_catalog(options).await?
    } else {
        AppCore::load(options).await?
    };
    if let Command::Gui { command } = cli.command {
        return gui::run_gui(core, command).await;
    }
    let result = match cli.command {
        Command::Chat { prompt, resume } => {
            chat::run_chat(&mut core, prompt, resume, cli.json).await
        }
        Command::Sessions { command } => sessions::run_sessions(&core, command, cli.json).await,
        Command::Run { prompt } => chat::run_once(&core, &prompt, cli.json).await,
        Command::Models => run_models(&core, cli.json).await,
        Command::Auth { command } => auth::run_auth(&core, command, cli.json).await,
        Command::Gui { .. } => unreachable!("gui command handled before core dispatch"),
    };
    let shutdown = core.shutdown().await;
    result?;
    shutdown?;
    Ok(())
}

async fn run_models(core: &AppCore, json: bool) -> Result<(), CliError> {
    let mut catalog = core.models_overview().await;
    catalog.sort_by(|a, b| {
        a.provider
            .as_str()
            .cmp(b.provider.as_str())
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });
    let current_provider = core.provider_id().as_str().to_string();
    let providers: Vec<String> = catalog
        .iter()
        .map(|entry| entry.provider.as_str().to_string())
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|pair| pair[0] != pair[1])
        .map(|pair| pair[0].clone())
        .chain(catalog.last().map(|e| e.provider.as_str().to_string()))
        .collect();
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
                "current_provider": current_provider,
                "providers": providers,
                "models": models,
            })
        );
    } else {
        // 六通道按通道表顺序展示（无静态条目的通道标注说明），config 自定义
        // provider 追加在后面，保证聚合视图覆盖全部首发通道。
        let mut ordered: Vec<String> = pawork_app::FIRST_PARTY_CHANNELS
            .iter()
            .map(|channel| channel.id.to_string())
            .collect();
        for provider in &providers {
            if !ordered.contains(provider) {
                ordered.push(provider.clone());
            }
        }
        for provider in &ordered {
            let marker = if *provider == current_provider {
                "  (current)"
            } else {
                ""
            };
            println!("provider: {provider}{marker}");
            let entries: Vec<_> = catalog
                .iter()
                .filter(|e| e.provider.as_str() == provider)
                .collect();
            if entries.is_empty() {
                println!("  (no static models; login/set-key 后运行期探测)");
                continue;
            }
            for entry in entries {
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
    fn parses_auth_subcommands() {
        let cli = Cli::try_parse_from(["pawork", "auth", "list"]).expect("parse");
        assert!(matches!(
            cli.command,
            Command::Auth {
                command: AuthCommand::List
            }
        ));

        let cli = Cli::try_parse_from(["pawork", "auth", "set-key", "glm-coding"]).expect("parse");
        match cli.command {
            Command::Auth {
                command: AuthCommand::SetKey { provider },
            } => assert_eq!(provider, "glm-coding"),
            other => panic!("unexpected {other:?}"),
        }

        let cli = Cli::try_parse_from(["pawork", "auth", "login", "chatgpt"]).expect("parse");
        match cli.command {
            Command::Auth {
                command: AuthCommand::Login { provider },
            } => assert_eq!(provider, "chatgpt"),
            other => panic!("unexpected {other:?}"),
        }

        let cli = Cli::try_parse_from(["pawork", "auth", "logout", "xai"]).expect("parse");
        assert!(matches!(
            cli.command,
            Command::Auth {
                command: AuthCommand::Logout { provider }
            } if provider == "xai"
        ));
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
