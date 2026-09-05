//! `pawork chat` / `pawork run`：落盘会话上的多轮或单次对话。

use std::io::{self, IsTerminal, Write};

use pawork_app::gui_server::GuiHost;
use pawork_app::{session_title_from_text, AppCore, AppError, GuiApprovalHost};
use pawork_domain::ProviderErrorKind;
use pawork_domain::{ContentPart, Message, MessageId, MessageRole, RunId, SessionId};
use pawork_engine::{
    AgentEventSink, CancelHandle, CancelReason, EngineError, NoopProcessTreeCleaner,
};
use pawork_protocol::headless::translate::encode_protocol_response;
use pawork_protocol::headless::{HeadlessResponse, ProtocolErrorKind};
use pawork_protocol::{AppCommand, AppEvent, AppResponse, RunState};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::broadcast::error::RecvError;

use crate::adapter::{adapter_from_locked, command_envelope, wrap_response};
use crate::error::format_provider_error;
use crate::render::TextSink;
use crate::CliError;

pub async fn run_chat(
    core: &mut AppCore,
    prompt: Option<String>,
    resume: Option<String>,
    branch: Option<String>,
) -> Result<(), CliError> {
    switch_branch_if_requested(core, resume.as_deref(), branch.as_deref()).await?;
    if let Some(prompt) = prompt {
        return run_prompt(core, &prompt, resume, true).await;
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
        return run_prompt(core, text, resume, true).await;
    }
    run_repl(core, resume).await
}

pub async fn run_once(core: &AppCore, prompt: &str) -> Result<(), CliError> {
    run_prompt(core, prompt, None, true).await
}

pub async fn run_json(
    core: AppCore,
    prompt: Option<String>,
    resume: Option<String>,
    branch: Option<String>,
) -> Result<(), CliError> {
    let prompt = prompt.ok_or_else(|| {
        CliError::Usage(
            "--json 需要 --prompt 或使用 `pawork run`（REPL 不会把 envelope 打到 stdout）".into(),
        )
    })?;
    switch_branch_if_requested(&core, resume.as_deref(), branch.as_deref()).await?;
    let workspace_id = core.workspace_id().clone();
    let resume_id = if let Some(spec) = resume.as_deref() {
        Some(core.resolve_session(spec).await?)
    } else {
        None
    };
    let text = flatten_parts(&core.expand_at_refs(resume_id.as_ref(), &prompt).await?);
    let adapter = adapter_from_locked(core, std::sync::Arc::new(GuiApprovalHost::new()));
    let mut events = adapter.subscribe_events();

    let session_id = if let Some(session_id) = resume_id {
        let _request_id = print_command(
            &adapter,
            AppCommand::SessionOpen {
                session_id: session_id.clone(),
            },
        )
        .await?;
        session_id
    } else {
        match dispatch_command(
            &adapter,
            AppCommand::SessionCreate {
                workspace_id: Some(workspace_id),
                title: Some(session_title_from_text(&prompt)),
            },
        )
        .await?
        {
            AppResponse::Data(data) => SessionId::from(
                data.get("session_id")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| CliError::Turn("SessionCreate 未返回 session_id".into()))?,
            ),
            other => {
                return Err(CliError::Turn(format!(
                    "SessionCreate 应返回 Data，got {other:?}"
                )))
            }
        }
    };

    let started = dispatch_command(
        &adapter,
        AppCommand::RunStart {
            session_id: session_id.clone(),
            user_message: text,
            model: None,
            provider: None,
            profile: None,
        },
    )
    .await?;
    let run_id = match started {
        AppResponse::Accepted {
            run_id: Some(run_id),
            ..
        } => run_id,
        AppResponse::Error(error) => {
            return Err(CliError::Turn(error.message));
        }
        other => {
            return Err(CliError::Turn(format!(
                "RunStart 应 Accepted 且携带 run id，got {other:?}"
            )))
        }
    };

    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(envelope) => {
                        let terminal = matches!(
                            &envelope.payload,
                            AppEvent::RunChanged { run_id: id, state }
                                if id == &run_id
                                    && matches!(
                                        state,
                                        RunState::Completed
                                            | RunState::Cancelled
                                            | RunState::Failed
                                            | RunState::Interrupted
                                    )
                        );
                        print_headless(&HeadlessResponse::Event { envelope })?;
                        if terminal {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(missed)) => {
                        print_headless(&HeadlessResponse::Error {
                            request_id: None,
                            kind: ProtocolErrorKind::Backpressure,
                            message: format!("event subscriber lagged; missed {missed}"),
                        })?;
                        return Err(CliError::Turn("event subscriber lagged".into()));
                    }
                    Err(RecvError::Closed) => break,
                }
            }
            _ = tokio::signal::ctrl_c() => {
                if let Err(error) = adapter
                    .command(&command_envelope(
                        AppCommand::RunCancel {
                            run_id: run_id.clone(),
                        },
                        "cli-json",
                    ))
                    .await
                {
                    tracing::warn!(%error, run_id = %run_id, "run cancel on ctrl-c failed");
                }
            }
        }
    }
    adapter.shutdown().await?;
    Ok(())
}

async fn switch_branch_if_requested(
    core: &AppCore,
    resume: Option<&str>,
    branch: Option<&str>,
) -> Result<(), CliError> {
    let Some(branch) = branch else {
        return Ok(());
    };
    let spec = resume.ok_or_else(|| CliError::Usage("--branch 需要 --resume".into()))?;
    let session = core.resolve_session(spec).await?;
    core.store()?
        .switch_branch(&session, branch)
        .await
        .map_err(AppError::from)?;
    Ok(())
}

async fn dispatch_command(
    adapter: &pawork_app::GuiHostAdapter,
    command: AppCommand,
) -> Result<AppResponse, CliError> {
    let envelope = command_envelope(command, "cli-json");
    let request_id = envelope.command_id.as_str().to_string();
    let response = adapter
        .command(&envelope)
        .await
        .map_err(|error| CliError::Turn(error.to_string()))?;
    print_headless(&HeadlessResponse::Response {
        envelope: wrap_response(&request_id, response.clone()),
    })?;
    Ok(response)
}

async fn print_command(
    adapter: &pawork_app::GuiHostAdapter,
    command: AppCommand,
) -> Result<String, CliError> {
    let envelope = command_envelope(command, "cli-json");
    let request_id = envelope.command_id.as_str().to_string();
    let response = adapter
        .command(&envelope)
        .await
        .map_err(|error| CliError::Turn(error.to_string()))?;
    print_headless(&HeadlessResponse::Response {
        envelope: wrap_response(&request_id, response),
    })?;
    Ok(request_id)
}

fn print_headless(response: &HeadlessResponse) -> Result<(), CliError> {
    let line =
        encode_protocol_response(response).map_err(|error| CliError::Usage(error.to_string()))?;
    println!("{line}");
    io::stdout().flush()?;
    Ok(())
}

fn flatten_parts(parts: &[ContentPart]) -> String {
    parts
        .iter()
        .map(|part| match part {
            ContentPart::Text(text) => text.text.as_str(),
            _ => "",
        })
        .collect()
}

async fn run_prompt(
    core: &AppCore,
    prompt: &str,
    resume: Option<String>,
    one_shot: bool,
) -> Result<(), CliError> {
    let (session, mut history, mut next_msg) =
        open_or_create(core, resume.as_deref(), prompt).await?;
    eprintln!("session {session}");
    run_one_turn(
        core,
        &session,
        &mut history,
        &mut next_msg,
        prompt,
        one_shot,
    )
    .await
}

async fn run_repl(core: &mut AppCore, resume: Option<String>) -> Result<(), CliError> {
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
            "pawork chat  {} / {}    Ctrl-C 取消当轮，/exit 退出；@file 引用工作区文件",
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
                            match compact_now(&*core, id).await {
                                Ok(after) => {
                                    history = core.resume_messages(id).await?;
                                    next_msg = next_message_counter(&history);
                                    eprintln!("compacted: {before} → {after} messages");
                                }
                                Err(err) => eprintln!("compact failed: {err}"),
                            }
                            continue;
                        }
                        if text == "/model" || text.starts_with("/model ") {
                            handle_model_command(core, session.as_ref(), &text["/model".len()..]).await;
                            continue;
                        }
                        if text == "/provider" || text.starts_with("/provider ") {
                            handle_provider_command(core, session.as_ref(), &text["/provider".len()..]).await;
                            continue;
                        }
                        if text == "/plan" || text.starts_with("/plan ") {
                            handle_plan_command(core, &mut session, &text["/plan".len()..]).await;
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
                        run_one_turn(&*core, id, &mut history, &mut next_msg, text, false).await?;
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

/// /model：无参列当前 provider 的静态目录；有参切换并落 model.switched 事件。
async fn handle_model_command(core: &mut AppCore, session: Option<&SessionId>, args: &str) {
    let name = args.trim();
    if name.is_empty() {
        let entries: Vec<_> = core
            .model_catalog()
            .await
            .into_iter()
            .filter(|entry| entry.provider == *core.provider_id())
            .collect();
        if entries.is_empty() {
            eprintln!("当前 provider 无静态目录条目。");
            return;
        }
        eprintln!("model: {}（当前）", core.model());
        for entry in entries {
            eprintln!("  {}", entry.id.as_str());
        }
        return;
    }
    match core.switch_model(session, name).await {
        Ok(()) => eprintln!("model -> {}", core.model()),
        Err(err) => eprintln!("switch failed: {err}"),
    }
}

/// /provider <id> [model]：切换 provider（可选同时切模型），事件流记录变更。
async fn handle_provider_command(core: &mut AppCore, session: Option<&SessionId>, args: &str) {
    let mut parts = args.trim().split_whitespace();
    let Some(provider) = parts.next() else {
        eprintln!("用法：/provider <id> [model]；当前 {}", core.provider_id());
        return;
    };
    let model = parts.next();
    match core.switch_provider(session, provider, model).await {
        Ok(()) => eprintln!("provider -> {} / {}", core.provider_id(), core.model()),
        Err(err) => eprintln!("switch failed: {err}"),
    }
}

async fn handle_plan_command(
    core: &mut AppCore,
    session: &mut Option<pawork_domain::SessionId>,
    args: &str,
) {
    let text = args.trim();
    let (verb, rest) = match text.split_once(' ') {
        Some((verb, rest)) => (verb, rest.trim()),
        None => (text, ""),
    };
    let ensure_session = async {
        if let Some(id) = session.clone() {
            return Ok(id);
        }
        let id = core.create_session("plan").await?;
        *session = Some(id.clone());
        Ok::<_, crate::CliError>(id)
    };
    let result = match verb {
        "" | "show" => {
            let Ok(id) = ensure_session.await else {
                eprintln!("plan: 没有活动会话");
                return;
            };
            core.plan_snapshot(&id)
                .await
                .map(|snapshot| match snapshot {
                    Some(plan) => format!(
                        "{}@{} {} {}",
                        plan.plan_id.as_str(),
                        plan.version.as_str(),
                        plan.title,
                        pawork_app::review_status_label(plan.review_status)
                    ),
                    None => "no plan".into(),
                })
        }
        "create" | "replace" => {
            let Some((title, steps)) = rest.split_once('|') else {
                eprintln!("用法：/plan {verb} Title | step1 | step2");
                return;
            };
            let steps: Vec<String> = steps
                .split('|')
                .map(|step| step.trim().to_string())
                .filter(|step| !step.is_empty())
                .collect();
            let Ok(id) = ensure_session.await else {
                eprintln!("plan: 无法创建会话");
                return;
            };
            let outcome = if verb == "create" {
                core.plan_create(&id, title.trim(), steps).await
            } else {
                core.plan_replace(&id, title.trim(), steps).await
            };
            outcome.map(|plan| {
                format!(
                    "{}@{} {}",
                    plan.plan_id.as_str(),
                    plan.version.as_str(),
                    pawork_app::review_status_label(plan.review_status)
                )
            })
        }
        "submit" | "approve" => {
            let Ok(id) = ensure_session.await else {
                eprintln!("plan: 没有活动会话");
                return;
            };
            let outcome = if verb == "submit" {
                core.plan_submit(&id).await
            } else {
                core.plan_approve(&id).await
            };
            outcome.map(|plan| pawork_app::review_status_label(plan.review_status).to_string())
        }
        "reject" => {
            if rest.is_empty() {
                eprintln!("用法：/plan reject <reason>");
                return;
            }
            let Ok(id) = ensure_session.await else {
                eprintln!("plan: 没有活动会话");
                return;
            };
            core.plan_reject(&id, rest)
                .await
                .map(|plan| pawork_app::review_status_label(plan.review_status).to_string())
        }
        _ => {
            eprintln!("用法：/plan show|create|replace|submit|approve|reject");
            return;
        }
    };
    match result {
        Ok(line) => eprintln!("{line}"),
        Err(err) => eprintln!("plan failed: {err}"),
    }
}

async fn run_one_turn(
    core: &AppCore,
    session: &SessionId,
    history: &mut Vec<Message>,
    next_msg: &mut u64,
    text: &str,
    one_shot: bool,
) -> Result<(), CliError> {
    let content = core.expand_at_refs(Some(session), text).await?;
    history.push(Message {
        id: next_id(session, next_msg),
        role: MessageRole::User,
        content,
        metadata: Default::default(),
    });
    let handle = CancelHandle::new(
        RunId::from(format!("cli-{session}")),
        std::sync::Arc::new(NoopProcessTreeCleaner),
    );
    let sink = TextSink::default();
    let outcome = drive_turn(core, session, history, &sink, handle).await;
    println!();

    *history = core
        .resume_messages(session)
        .await
        .unwrap_or_else(|_| history.clone());
    *next_msg = next_message_counter(history);

    if outcome.is_ok() {
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
        .map(|cost| {
            format!(
                " | ~{} {:.4}",
                cost.currency,
                cost.amount_micros as f64 / 1_000_000.0
            )
        })
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
    let rebuilt = core.compact_session(session, &sink, handle.token()).await?;
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
        pawork_app::AppError::Engine(EngineError::MaxToolRounds(n)) => {
            Err(CliError::Turn(format!("工具轮数已达上限 ({n})，已停止。")))
        }
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
