//! `pawork chat`：内存多轮 REPL，或 `--prompt` 单次模式。

use std::io::{self, IsTerminal, Write};

use pawork_api::ProviderErrorKind;
use pawork_app::AppCore;
use pawork_domain::{
    CancellationToken, ContentPart, Message, MessageId, MessageRole, TextContent,
};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::error::format_provider_error;
use crate::render::StdoutSink;
use crate::CliError;

pub async fn run_chat(core: &AppCore, prompt: Option<String>) -> Result<(), CliError> {
    if let Some(prompt) = prompt {
        return run_one_turn(core, &mut Vec::new(), &mut 1, &prompt, true).await;
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
        return run_one_turn(core, &mut Vec::new(), &mut 1, text, true).await;
    }
    run_repl(core).await
}

async fn run_repl(core: &AppCore) -> Result<(), CliError> {
    let mut history = Vec::new();
    let mut next_msg = 1u64;
    let mut idle_interrupts = 0u8;
    let mut reader = BufReader::new(tokio::io::stdin());

    eprintln!(
        "pawork chat  {} / {}    Ctrl-C 取消当轮，/exit 退出",
        core.provider_id(),
        core.model()
    );

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
                        run_one_turn(core, &mut history, &mut next_msg, text, false).await?;
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

async fn run_one_turn(
    core: &AppCore,
    history: &mut Vec<Message>,
    next_msg: &mut u64,
    text: &str,
    one_shot: bool,
) -> Result<(), CliError> {
    history.push(text_message(
        next_id(next_msg),
        MessageRole::User,
        text,
    ));
    let cancel = CancellationToken::new();
    let sink = StdoutSink::default();
    let turn = core.chat_turn(history.clone(), &sink, cancel.clone());
    tokio::pin!(turn);

    let result = tokio::select! {
        result = &mut turn => result,
        _ = tokio::signal::ctrl_c() => {
            cancel.cancel();
            turn.await
        }
    };

    println!();

    match result {
        Ok(_) => {
            let reply = sink.collected_text();
            if !reply.is_empty() {
                history.push(text_message(
                    next_id(next_msg),
                    MessageRole::Assistant,
                    reply,
                ));
            }
            Ok(())
        }
        Err(err) if err.kind == ProviderErrorKind::Cancelled => {
            eprintln!("已取消");
            let reply = sink.collected_text();
            if !reply.is_empty() {
                let mut message = text_message(next_id(next_msg), MessageRole::Assistant, reply);
                message.metadata.incomplete = true;
                history.push(message);
            }
            if one_shot {
                Err(CliError::Cancelled)
            } else {
                Ok(())
            }
        }
        Err(err) => {
            let formatted = format_provider_error(&err);
            if one_shot {
                Err(CliError::Turn(formatted))
            } else {
                eprintln!("{formatted}");
                Ok(())
            }
        }
    }
}

fn next_id(next_msg: &mut u64) -> MessageId {
    let id = *next_msg;
    *next_msg += 1;
    MessageId::from(format!("msg-{id}"))
}

fn text_message(id: MessageId, role: MessageRole, text: impl Into<String>) -> Message {
    Message {
        id,
        role,
        content: vec![ContentPart::Text(TextContent { text: text.into() })],
        metadata: Default::default(),
    }
}
