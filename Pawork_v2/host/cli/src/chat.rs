//! `pawork chat` / `pawork run`：落盘会话上的多轮或单次对话。

use std::io::{self, IsTerminal, Write};

use pawork_api::ProviderErrorKind;
use pawork_app::{session_title_from_text, AppCore};
use pawork_domain::{
    ContentPart, Message, MessageId, MessageRole, RunId, SessionId, TextContent,
};
use pawork_engine::{
    AgentEventSink, CancelHandle, CancelReason, EngineError, NoopProcessTreeCleaner,
};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::error::format_provider_error;
use crate::render::{JsonlSink, TextSink};
use crate::CliError;

pub async fn run_chat(
    core: &AppCore,
    prompt: Option<String>,
    resume: Option<String>,
    json: bool,
) -> Result<(), CliError> {
    if let Some(prompt) = prompt {
        return run_prompt(core, &prompt, resume, json, true).await;
    }
    if json {
        return Err(CliError::Usage(
            "--json 需要 --prompt 或使用 `pawork run`（REPL 不会把 envelope 打到 stdout）".into(),
        ));
    }
    if !io::stdin().is_terminal() {
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let text = line.trim();
        if text.is_empty() {
            return Err(CliError::Usage(
                "非交互模式需要 --prompt 或从 stdin 提供一行问题".into(),
            ));
        }
        return run_prompt(core, text, resume, false, true).await;
    }
    run_repl(core, resume).await
}

pub async fn run_once(core: &AppCore, prompt: &str, json: bool) -> Result<(), CliError> {
    run_prompt(core, prompt, None, json, true).await
}

async fn run_prompt(
    core: &AppCore,
    prompt: &str,
    resume: Option<String>,
    json: bool,
    one_shot: bool,
) -> Result<(), CliError> {
    let (session, mut history, mut next_msg) = open_or_create(core, resume.as_deref(), prompt).await?;
    if !json {
        eprintln!("session {session}");
    }
    run_one_turn(
        core,
        &session,
        &mut history,
        &mut next_msg,
        prompt,
        json,
        one_shot,
    )
    .await
}

async fn run_repl(core: &AppCore, resume: Option<String>) -> Result<(), CliError> {
    let mut session = if let Some(spec) = resume {
        Some(core.resolve_session(&spec).await?)
    } else {
        None
    };
    let mut history = if let Some(id) = &session {
        core.resume_messages(id).await?
    } else {
        Vec::new()
    };
    let mut next_msg = next_message_counter(&history);

    match &session {
        Some(id) => eprintln!(
            "pawork chat  {} / {}  session {id}    Ctrl-C 取消当轮，/exit 退出",
            core.provider_id(),
            core.model()
        ),
        None => eprintln!(
            "pawork chat  {} / {}    Ctrl-C 取消当轮，/exit 退出",
            core.provider_id(),
            core.model()
        ),
    }

    let mut idle_interrupts = 0u8;
    let mut reader = BufReader::new(tokio::io::stdin());

    loop {
        print!("> ");
        io::stdout().flush()?;
        let mut line = String::new();
        tokio::select! {
            result = reader.read_line(&mut line) => {
                match result? {
                    0 => return Ok(()),
                    _ => {
                        idle_interrupts = 0;
                        let text = line.trim();
                        if text.is_empty() {
                            continue;
                        }
                        if text == "/exit" || text == "/quit" {
                            return Ok(());
                        }
                        if text == "/compact" {
                            let Some(id) = session.as_ref() else {
                                eprintln!("没有活动会话，先输入一条消息再压缩。");
                                continue;
                            };
                            let before = history.len();
                            match compact_now(core, id).await {
                                Ok(after) => {
                                    history = core.resume_messages(id).await?;
                                    next_msg = next_message_counter(&history);
                                    eprintln!("compacted: {before} → {after} messages");
                                }
                                Err(err) => eprintln!("compact failed: {err}"),
                            }
                            continue;
                        }
                        if session.is_none() {
                            let id = core
                                .create_session(session_title_from_text(text))
                                .await?;
                            eprintln!("session {id}");
                            session = Some(id);
                        }
                        let id = session.as_ref().expect("session created");
                        run_one_turn(core, id, &mut history, &mut next_msg, text, false, false).await?;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                idle_interrupts = idle_interrupts.saturating_add(1);
                eprintln!();
                if idle_interrupts >= 2 {
                    return Ok(());
                }
                eprintln!("再按一次 Ctrl-C 退出；输入 /exit 也可退出。");
            }
        }
    }
}

async fn open_or_create(
    core: &AppCore,
    resume: Option<&str>,
    first_prompt: &str,
) -> Result<(SessionId, Vec<Message>, u64), CliError> {
    if let Some(spec) = resume {
        let session = core.resolve_session(spec).await?;
        let history = core.resume_messages(&session).await?;
        let next_msg = next_message_counter(&history);
        Ok((session, history, next_msg))
    } else {
        let session = core
            .create_session(session_title_from_text(first_prompt))
            .await?;
        Ok((session, Vec::new(), 1))
    }
}

async fn run_one_turn(
    core: &AppCore,
    session: &SessionId,
    history: &mut Vec<Message>,
    next_msg: &mut u64,
    text: &str,
    json: bool,
    one_shot: bool,
) -> Result<(), CliError> {
    history.push(text_message(
        next_id(session, next_msg),
        MessageRole::User,
        text,
    ));
    let handle = CancelHandle::new(
        RunId::from(format!("cli-{session}")),
        std::sync::Arc::new(NoopProcessTreeCleaner),
    );
    let outcome = if json {
        drive_turn(core, session, history, &JsonlSink, handle).await
    } else {
        let sink = TextSink::default();
        let outcome = drive_turn(core, session, history, &sink, handle).await;
        println!();
        outcome
    };

    *history = core.resume_messages(session).await.unwrap_or_else(|_| history.clone());
    *next_msg = next_message_counter(history);

    if !json && outcome.is_ok() {
        print_usage_line(core, session).await;
    }

    match outcome {
        Ok(()) => Ok(()),
        Err(CliError::Cancelled) if !one_shot => Ok(()),
        other => other,
    }
}

/// 每轮尾部用量行：本轮 + 会话累计 token；费用按 registry 定价估算，
/// 无定价条目不显示（不编造）。
async fn print_usage_line(core: &AppCore, session: &SessionId) {
    let turn = core.last_run_usage(session).await.ok().flatten();
    let total = core.session_usage(session).await.ok();
    let (Some(turn), Some(total)) = (turn, total) else {
        return;
    };
    let cost = core
        .estimate_cost_for(core.model(), &total)
        .map(|cost| format!(" | ~{} {:.4}", cost.currency, cost.amount_micros as f64 / 1_000_000.0))
        .unwrap_or_default();
    eprintln!(
        "tokens: turn in {} out {} | session in {} out {} (cache read {} / write {}){cost}",
        turn.input_tokens,
        turn.output_tokens,
        total.input_tokens,
        total.output_tokens,
        total.cache_read_tokens,
        total.cache_write_tokens,
    );
}

/// 手动压缩：与自动链同一 engine 函数与事件序；TextSink 静默承载事件，
/// 结果行由调用方输出。
async fn compact_now(core: &AppCore, session: &SessionId) -> Result<usize, CliError> {
    let handle = CancelHandle::new(
        RunId::from(format!("cli-compact-{session}")),
        std::sync::Arc::new(NoopProcessTreeCleaner),
    );
    let sink = crate::render::TextSink::default();
    let rebuilt = core
        .compact_session(session, &sink, handle.token())
        .await?;
    Ok(rebuilt.len())
}

async fn drive_turn(
    core: &AppCore,
    session: &SessionId,
    history: &[Message],
    sink: &dyn AgentEventSink,
    handle: CancelHandle,
) -> Result<(), CliError> {
    let turn = core.chat_turn(session, history.to_vec(), sink, handle.token());
    tokio::pin!(turn);
    let result = tokio::select! {
        result = &mut turn => result,
        _ = tokio::signal::ctrl_c() => {
            handle.cancel(CancelReason::User);
            turn.await
        }
    };
    match result {
        Ok(_) => Ok(()),
        Err(error) => map_turn_error(error),
    }
}

fn map_turn_error(error: pawork_app::AppError) -> Result<(), CliError> {
    match error {
        pawork_app::AppError::Provider(err) if err.kind == ProviderErrorKind::Cancelled => {
            eprintln!("已取消");
            Err(CliError::Cancelled)
        }
        pawork_app::AppError::Engine(EngineError::Provider(err))
            if err.kind == ProviderErrorKind::Cancelled =>
        {
            eprintln!("已取消");
            Err(CliError::Cancelled)
        }
        pawork_app::AppError::Engine(EngineError::MaxToolRounds(n)) => Err(CliError::Turn(format!(
            "工具轮数已达上限 ({n})，已停止。"
        ))),
        pawork_app::AppError::Provider(err) => Err(CliError::Turn(format_provider_error(&err))),
        pawork_app::AppError::Engine(EngineError::Provider(err)) => {
            Err(CliError::Turn(format_provider_error(&err)))
        }
        other => Err(CliError::App(other)),
    }
}

fn next_message_counter(history: &[Message]) -> u64 {
    history.len() as u64 + 1
}

fn next_id(session: &SessionId, next_msg: &mut u64) -> MessageId {
    let id = *next_msg;
    *next_msg += 1;
    MessageId::from(format!("msg-{session}-{id}"))
}

fn text_message(id: MessageId, role: MessageRole, text: impl Into<String>) -> Message {
    Message {
        id,
        role,
        content: vec![ContentPart::Text(TextContent { text: text.into() })],
        metadata: Default::default(),
    }
}
