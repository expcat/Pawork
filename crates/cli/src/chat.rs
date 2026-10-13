//! `pawork chat` / `pawork run`：落盘会话上的多轮或单次对话。

use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use pawork_client::{
    ClientAuthentication, ClientConfig, ClientError, ConnectOptions, GuiClient, GuiTransportClient,
    LocalTransport, TimelineItemKind, TimelinePage, TransportEndpoint,
};

use pawork_app::{session_title_from_text, AppCore, AppError, GuiApprovalHost};
use pawork_domain::ProviderErrorKind;
use pawork_domain::{
    ContentPart, ImageContent, ImageSource, Message, MessageId, MessageRole, RunId, SessionId,
    TextContent, WorkspaceId,
};
use pawork_engine::{AgentEventSink, CancelHandle, CancelReason, EngineError};
use pawork_gui_server::GuiHost;
use pawork_protocol::headless::translate::encode_protocol_response;
use pawork_protocol::headless::{HeadlessResponse, ProtocolErrorKind};
use pawork_protocol::{
    ActorIdentity, AppCommand, AppEvent, AppEventEnvelope, AppQuery, AppResponse, CommandSource,
    RunState,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::adapter::{adapter_from_locked, command_envelope, wrap_response};
use crate::error::format_provider_error;
use crate::render::TextSink;
use crate::{Cli, CliError, Command};

pub async fn run_chat(
    core: &mut AppCore,
    prompt: Option<String>,
    resume: Option<String>,
    branch: Option<String>,
    images: Vec<String>,
    video_urls: Vec<String>,
) -> Result<(), CliError> {
    switch_branch_if_requested(core, resume.as_deref(), branch.as_deref()).await?;
    if let Some(prompt) = prompt {
        return run_prompt(core, &prompt, resume, true, &images, &video_urls).await;
    }
    if !images.is_empty() || !video_urls.is_empty() {
        return Err(CliError::Usage(
            "--image / --video-url 需要 --prompt；交互 REPL 不接受附件参数".into(),
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
        return run_prompt(core, text, resume, true, &[], &[]).await;
    }
    run_repl(core, resume).await
}

pub async fn run_once(
    core: &AppCore,
    prompt: &str,
    images: Vec<String>,
    video_urls: Vec<String>,
) -> Result<(), CliError> {
    run_prompt(core, prompt, None, true, &images, &video_urls).await
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
        Some(crate::sessions::resolve_session(&core, Some(spec)).await?)
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
                )));
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
            effort: None,
            attachment_ids: Vec::new(),
            web_search: None,
            video_urls: Vec::new(),
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
            )));
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
    let session = crate::sessions::resolve_session(core, Some(spec)).await?;
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
    images: &[String],
    video_urls: &[String],
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
        images,
        video_urls,
    )
    .await
}

async fn run_repl(core: &mut AppCore, resume: Option<String>) -> Result<(), CliError> {
    let mut session = if let Some(spec) = resume.as_deref() {
        Some(crate::sessions::resolve_session(core, Some(spec)).await?)
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
                        run_one_turn(&*core, id, &mut history, &mut next_msg, text, false, &[], &[]).await?;
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
        let session = crate::sessions::resolve_session(core, Some(spec)).await?;
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
    images: &[String],
    video_urls: &[String],
) -> Result<(), CliError> {
    let mut content = core.expand_at_refs(Some(session), text).await?;
    if images.len() + video_urls.len() > 4 {
        return Err(CliError::Usage(
            "--image 与 --video-url 每轮合计最多 4 项".into(),
        ));
    }
    content.extend(local_image_parts(images)?);
    for url in video_urls {
        content.push(ContentPart::Video(video_content_from_url(url)?));
    }
    history.push(Message {
        id: next_id(session, next_msg),
        role: MessageRole::User,
        content,
        metadata: Default::default(),
    });
    let handle = CancelHandle::new(RunId::from(format!("cli-{session}")));
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
    let handle = CancelHandle::new(RunId::from(format!("cli-compact-{session}")));
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

/// MM-1：`--image` 读取调用方显式给出的本地文件，编码为 base64 Image part。
/// 与 GUI 附件同一交集：png / jpeg / gif / webp，每个不超过 8 MiB。
fn local_image_parts(paths: &[String]) -> Result<Vec<ContentPart>, CliError> {
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    const MAX_COUNT: usize = 4;
    if paths.len() > MAX_COUNT {
        return Err(CliError::Usage(format!(
            "--image 最多 {MAX_COUNT} 张，收到 {}",
            paths.len()
        )));
    }
    let mut parts = Vec::new();
    for raw in paths {
        let path = std::path::Path::new(raw);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| CliError::Usage(format!("--image 路径无效: {raw}")))?;
        let media_type = image_media_type(name).ok_or_else(|| {
            CliError::Usage(format!("--image 只接受 png、jpeg、gif、webp: {raw}"))
        })?;
        let meta = std::fs::metadata(path)
            .map_err(|err| CliError::Usage(format!("--image 无法读取 {raw}: {err}")))?;
        if !meta.is_file() {
            return Err(CliError::Usage(format!("--image 不是普通文件: {raw}")));
        }
        if meta.len() == 0 || meta.len() > MAX_BYTES {
            return Err(CliError::Usage(format!(
                "--image {raw} 必须在 1 字节到 8 MiB 之间"
            )));
        }
        let mut file = std::fs::File::open(path)
            .map_err(|err| CliError::Usage(format!("--image 无法读取 {raw}: {err}")))?;
        // 查已打开描述符的类型，不是再查路径。路径被换成可打开的非普通文件时在此拒绝；
        // 不消除普通文件之间的内容替换，也不避免 File::open 在特殊文件上阻塞。
        if !file.metadata().map(|m| m.is_file()).unwrap_or(false) {
            return Err(CliError::Usage(format!("--image 不是普通文件: {raw}")));
        }
        let mut bytes = Vec::new();
        std::io::Read::take(&mut file, MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| CliError::Usage(format!("--image 无法读取 {raw}: {err}")))?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_BYTES {
            return Err(CliError::Usage(format!(
                "--image {raw} 必须在 1 字节到 8 MiB 之间"
            )));
        }
        parts.push(ContentPart::Text(TextContent {
            text: format!(
                "[attached image: {name} ({media_type}, {} bytes)]",
                bytes.len()
            ),
        }));
        parts.push(ContentPart::Image(ImageContent {
            source: ImageSource::Base64(base64_encode(&bytes)),
            media_type: media_type.into(),
            alt_text: Some(name.to_string()),
        }));
    }
    Ok(parts)
}

fn image_media_type(name: &str) -> Option<&'static str> {
    let extension = name.rsplit('.').next()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn next_message_counter(history: &[Message]) -> u64 {
    history.len() as u64 + 1
}

fn next_id(session: &SessionId, next_msg: &mut u64) -> MessageId {
    let id = *next_msg;
    *next_msg += 1;
    MessageId::from(format!("msg-{session}-{id}"))
}

// ============================================================================
// CU-10 #5：--gui-host opt-in 客户端
//
// 显式 --gui-host 时本进程不加载 Executor Core：以 GuiClient 身份借用已经
// 持有 gui.lock 的 pawork gui serve，审批（含 computer 目标授权）全部发生在
// GUI 侧。socket/token 缺失、API 不足、workspace 不匹配一律失败关闭——不回
// 退本地 Core，不启动第二个 Run。
// ============================================================================

/// 断流后的恢复上限（间隔 1s；仍不可达则如实报告终态未知）。
const GUI_HOST_RECONNECT_ATTEMPTS: u8 = 3;
/// 空闲心跳间隔：服务端 30s 超时，任意入站帧刷新；沿用 Desktop 的 15s 口径。
const GUI_HOST_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// 重连与观测共享的连接参数（同一端点、同一实例认证、同一握手名）。
struct GuiHostConnection {
    client: GuiClient,
    /// 重连复用的传输工厂：保持与首连同一实现（测试注入内存传输）。
    transport: Arc<dyn GuiTransportClient>,
    endpoint: TransportEndpoint,
    options: ConnectOptions,
    authentication: ClientAuthentication,
    config: ClientConfig,
}

impl GuiHostConnection {
    fn source(&self) -> CommandSource {
        CommandSource::RemoteGui {
            client_id: self.client.client_id().clone(),
            connection_id: self.client.connection_id().clone(),
        }
    }

    fn identity() -> ActorIdentity {
        ActorIdentity::LocalUser {
            actor_id: "local-user".into(),
            display_name: None,
        }
    }

    /// 断线恢复：新 LocalTransport 重连 + 握手 + 按 last_acked Resume + 重新
    /// 全量订阅。失败由调用方按尝试次数收敛，不在此静默重试。
    async fn reconnect(&mut self) -> Result<Option<pawork_client::ResumeOutcome>, ClientError> {
        let (client, outcome) = GuiClient::connect_with_resume_config(
            Arc::clone(&self.transport),
            self.endpoint.clone(),
            self.options.clone(),
            Some(self.authentication.clone()),
            Some(self.client.last_acked_sequence()),
            self.config.clone(),
        )
        .await?;
        self.client = client;
        self.client.subscribe_all().await?;
        Ok(outcome)
    }
}

/// 直播事件阶段的退出原因。
enum GuiStreamOutcome {
    /// Run 到达 Completed / Cancelled / Failed 之一（唯一认可的终态）。
    Terminal(RunState),
    /// 事件流断开（传输错误 / 心跳失败）：进入恢复阶段。
    Broken,
    /// 协议返回了本命令相关的错误响应：如实失败。
    Fatal(CliError),
}

/// Ctrl-C 观察器：独立监听任务 + mpsc 缓冲 + 已请求标记。每轮 Run 安装新
/// 实例、Drop 即 abort 监听任务（空闲期信号不缓存到下一轮、不逐轮留悬挂
/// 任务）。信号经 channel 缓冲，直播/恢复任意阶段到达都不丢失；manual 供
/// 测试注入（无真实信号任务）。
struct CancelWatch {
    requested: Arc<AtomicBool>,
    receiver: Option<mpsc::Receiver<()>>,
    listener: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for CancelWatch {
    fn drop(&mut self) {
        if let Some(listener) = self.listener.take() {
            listener.abort();
        }
    }
}

impl CancelWatch {
    fn install() -> Self {
        let (sender, receiver) = mpsc::channel(1);
        let requested = Arc::new(AtomicBool::new(false));
        let listener = tokio::spawn(async move {
            loop {
                // 独立任务持有唯一 Signal 实例：观察窗口内到达的信号先进
                // channel 缓冲，不再因 select 分支轮换被丢弃。
                if tokio::signal::ctrl_c().await.is_err() {
                    return;
                }
                if sender.send(()).await.is_err() {
                    return;
                }
            }
        });
        Self {
            requested,
            receiver: Some(receiver),
            listener: Some(listener),
        }
    }

    /// 测试注入：不经真实 SIGINT，直接持有信号发送端。
    #[cfg(test)]
    fn manual() -> (Self, mpsc::Sender<()>) {
        let (sender, receiver) = mpsc::channel(1);
        (
            Self {
                requested: Arc::new(AtomicBool::new(false)),
                receiver: Some(receiver),
                listener: None,
            },
            sender,
        )
    }

    /// 消费一条取消信号并记录意图；guard 由调用方的 cancel_delivered 承担
    ///（拒绝后不再自动重发）。
    async fn recv(&mut self) {
        let Some(receiver) = self.receiver.as_mut() else {
            // 监听任务已结束（channel 关闭）：保持 pending，不让 biased
            // select 的取消臂永久就绪、饿死恢复 future。
            std::future::pending::<()>().await;
            return;
        };
        match receiver.recv().await {
            Some(()) => self.requested.store(true, Ordering::Release),
            None => {
                self.receiver = None;
                // 监听出错关闭 channel 不是用户取消。在本次调用内永久挂起，
                // 取消臂不得完成一次，否则 race_cancel 会把原 Run 误记为已请求。
                std::future::pending::<()>().await;
            }
        }
    }

    fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    /// 测试观察取消意图是否已被 recv 消费（任务外轮询，不另开信号通道）。
    #[cfg(test)]
    fn requested_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.requested)
    }
}

pub async fn run_on_gui_host(cli: &Cli, instance: &str, data_dir: &Path) -> Result<(), CliError> {
    reject_gui_host_conflicts(cli)?;
    let (socket, prompt, resume, video_urls) = match &cli.command {
        Command::Chat {
            gui_host,
            prompt,
            resume,
            video_urls,
            ..
        } => (
            gui_host.clone().flatten(),
            prompt.clone(),
            resume.clone(),
            video_urls.clone(),
        ),
        Command::Run {
            gui_host,
            prompt,
            video_urls,
            ..
        } => (
            gui_host.clone().flatten(),
            Some(prompt.clone()),
            None,
            video_urls.clone(),
        ),
        _ => unreachable!("run_on_gui_host 只从 Chat/Run 派发"),
    };
    if !io::stdin().is_terminal() {
        return Err(CliError::Usage(
            "--gui-host 需要交互终端（stdin 非 TTY）：首版保持交互式；目标审批与取消都在 GUI 侧处理".into(),
        ));
    }

    let socket_path = socket.unwrap_or_else(|| crate::ops::gui_socket_path(data_dir, instance));
    let endpoint = TransportEndpoint::Local {
        address: socket_path.to_string_lossy().into_owned(),
    };
    let unavailable = |error: String| {
        CliError::Usage(format!(
            "no GUI Host for --gui-host on {}: {error}; start one with pawork gui serve. This command does not fall back to a local Executor Core",
            socket_path.display()
        ))
    };
    let authentication = crate::ops::load_gui_client_authentication(data_dir, instance)
        .map_err(|error| unavailable(error.to_string()))?;
    let options = ConnectOptions {
        timeout_ms: 5_000,
        client_label: Some("pawork-cli-gui-host".into()),
        max_frame_bytes: 1024 * 1024,
    };
    let config = ClientConfig {
        // 精确握手名是 Host 侧 bind/launch 等待开关的唯一判据（全等比较）。
        client_name: "pawork-cli".into(),
        ..ClientConfig::default()
    };
    let transport: Arc<dyn GuiTransportClient> = Arc::new(LocalTransport::default());
    let client = GuiClient::connect_with_config(
        Arc::clone(&transport),
        endpoint.clone(),
        options.clone(),
        Some(authentication.clone()),
        config.clone(),
    )
    .await
    .map_err(|error| unavailable(error.to_string()))?;
    let mut connection = GuiHostConnection {
        client,
        transport,
        endpoint,
        options,
        authentication,
        config,
    };
    if connection.client.api_version() < pawork_protocol::V1_27 {
        let negotiated = connection.client.api_version();
        let _ = connection.client.disconnect().await;
        return Err(CliError::Usage(format!(
            "--gui-host requires Host API 1.27+ (target approval and observation queries); negotiated {negotiated:?}. Restart the Host after upgrading"
        )));
    }
    connection
        .client
        .subscribe_all()
        .await
        .map_err(|error| CliError::Usage(format!("gui host subscribe failed: {error}")))?;

    let workspace_id = resolve_gui_workspace(&connection.client).await?;
    let result = match prompt {
        Some(prompt) => {
            let videos = gui_video_parts(&video_urls)?;
            let session_id =
                open_gui_session(&mut connection, &workspace_id, resume.as_deref(), &prompt)
                    .await?;
            let mut cancel = CancelWatch::install();
            let (run_id, state) =
                run_gui_turn(&mut connection, &session_id, &prompt, videos, &mut cancel).await?;
            eprintln!("run {run_id} {state:?}");
            finish_gui_terminal(state, &run_id)
        }
        None => {
            let mut reader = BufReader::new(tokio::io::stdin());
            run_gui_repl(
                &mut connection,
                &workspace_id,
                resume.as_deref(),
                &mut reader,
                GUI_HOST_HEARTBEAT_INTERVAL,
            )
            .await
        }
    };
    let _ = connection.client.disconnect().await;
    result
}

/// 首版产品边界的 fail-closed 拒绝（逐条给出原因，不静默忽略）。
fn reject_gui_host_conflicts(cli: &Cli) -> Result<(), CliError> {
    if cli.json {
        return Err(CliError::Usage(
            "--gui-host 不能与 --json 一起使用（本模式渲染文本到 stdout/stderr，不走 HeadlessResponse JSONL）".into(),
        ));
    }
    if cli.provider.is_some() || cli.model.is_some() {
        return Err(CliError::Usage(
            "--gui-host 不能与 -p/-m 一起使用：RunStart 的 provider/model 是共享 Host Core 上的持久写，不作为单轮覆盖转发".into(),
        ));
    }
    if cli.approval_mode.is_some() {
        return Err(CliError::Usage(
            "--gui-host 不能与 --approval-mode 一起使用：审批模式由 GUI Host 持有，本进程不覆盖"
                .into(),
        ));
    }
    if cli.trust_workspaces {
        return Err(CliError::Usage(
            "--gui-host 不能与 --trust-workspaces 一起使用：workspace 信任由 GUI Host 持有，本进程不覆盖".into(),
        ));
    }
    if let Command::Chat {
        branch: Some(_), ..
    } = &cli.command
    {
        return Err(CliError::Usage(
            "--gui-host 不接受 --branch：branch 切换是本进程 Core 的本地写，此模式不转发".into(),
        ));
    }
    match &cli.command {
        Command::Chat { images, .. } | Command::Run { images, .. } if !images.is_empty() => {
            return Err(CliError::Usage(
                "--gui-host 不接受 --image：附件上传通道不在本模式内（首版产品边界；--video-url 可用）".into(),
            ));
        }
        Command::Chat {
            resume: Some(spec), ..
        } if spec == "latest" => {
            return Err(CliError::Usage(
                "--gui-host 的 --resume 只接受完整 session id：latest/前缀解析需要本进程 Core，此模式不加载".into(),
            ));
        }
        Command::Chat {
            video_urls,
            prompt: None,
            ..
        } if !video_urls.is_empty() => {
            return Err(CliError::Usage(
                "--video-url 需要 --prompt；交互 REPL 不接受附件参数".into(),
            ));
        }
        _ => {}
    }
    Ok(())
}

/// WorkspaceList 唯一根匹配调用方的 canonical cwd；零个或多个都失败关闭。
async fn resolve_gui_workspace(client: &GuiClient) -> Result<WorkspaceId, CliError> {
    let cwd = std::env::current_dir()?;
    let canonical_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
    let envelope = client
        .query(
            AppQuery::WorkspaceList,
            CommandSource::RemoteGui {
                client_id: client.client_id().clone(),
                connection_id: client.connection_id().clone(),
            },
            GuiHostConnection::identity(),
        )
        .await
        .map_err(|error| CliError::Usage(format!("WorkspaceList query failed: {error}")))?;
    let entries = match envelope.response {
        AppResponse::Data(serde_json::Value::Array(entries)) => entries,
        AppResponse::Error(error) => {
            return Err(CliError::Usage(format!(
                "WorkspaceList failed: {}",
                error.message
            )))
        }
        other => {
            return Err(CliError::Turn(format!(
                "WorkspaceList should return Data, got {other:?}"
            )))
        }
    };
    let mut matched: Vec<WorkspaceId> = Vec::new();
    for entry in &entries {
        let Some(id) = entry.get("id").and_then(|value| value.as_str()) else {
            continue;
        };
        let Some(roots) = entry.get("roots").and_then(|value| value.as_array()) else {
            continue;
        };
        for root in roots {
            let Some(path) = root.get("path").and_then(|value| value.as_str()) else {
                continue;
            };
            let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
            if canonical == canonical_cwd {
                matched.push(WorkspaceId::from(id));
            }
        }
    }
    match matched.len() {
        1 => Ok(matched.remove(0)),
        0 => Err(CliError::Usage(format!(
            "no GUI Host workspace root matches cwd {}; register it on the Host first (pawork gui serve data dir)",
            cwd.display()
        ))),
        _ => Err(CliError::Usage(
            "cwd matches multiple GUI Host workspace roots; run from a deeper subdirectory or trim Host workspaces".into(),
        )),
    }
}

/// 建/开 session：resume 只认完整 id，且 SessionGet 的 workspace_id 必须与
/// 唯一匹配根一致（缺失也失败关闭）；新会话用首条提示生成标题。
async fn open_gui_session(
    connection: &mut GuiHostConnection,
    workspace_id: &WorkspaceId,
    resume: Option<&str>,
    first_prompt: &str,
) -> Result<SessionId, CliError> {
    if let Some(spec) = resume {
        let session_id = SessionId::from(spec);
        let envelope = connection
            .client
            .query(
                AppQuery::SessionGet {
                    session_id: session_id.clone(),
                    timeline_after_sequence: None,
                    timeline_limit: None,
                },
                connection.source(),
                GuiHostConnection::identity(),
            )
            .await
            .map_err(|error| CliError::Usage(format!("SessionGet failed: {error}")))?;
        let data = match envelope.response {
            AppResponse::Data(data) => data,
            AppResponse::Error(error) => {
                return Err(CliError::Usage(format!(
                    "SessionGet failed: {}",
                    error.message
                )))
            }
            other => {
                return Err(CliError::Turn(format!(
                    "SessionGet should return Data, got {other:?}"
                )))
            }
        };
        let session_workspace = data
            .get("workspace_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                CliError::Usage(
                    "SessionGet returned no workspace_id; this mode fails closed (cannot verify session ownership)".into(),
                )
            })?;
        if session_workspace != workspace_id.as_str() {
            return Err(CliError::Usage(format!(
                "session {session_id} belongs to workspace {session_workspace}, not the cwd-matched {}",
                workspace_id.as_str()
            )));
        }
        let envelope = connection
            .client
            .command(
                AppCommand::SessionOpen {
                    session_id: session_id.clone(),
                },
                connection.source(),
                GuiHostConnection::identity(),
            )
            .await
            .map_err(|error| CliError::Usage(format!("SessionOpen failed: {error}")))?;
        if let AppResponse::Error(error) = envelope.response {
            return Err(CliError::Usage(format!(
                "SessionOpen failed: {}",
                error.message
            )));
        }
        Ok(session_id)
    } else {
        let envelope = connection
            .client
            .command(
                AppCommand::SessionCreate {
                    workspace_id: Some(workspace_id.clone()),
                    title: Some(session_title_from_text(first_prompt)),
                },
                connection.source(),
                GuiHostConnection::identity(),
            )
            .await
            .map_err(|error| CliError::Usage(format!("SessionCreate failed: {error}")))?;
        let data = match envelope.response {
            AppResponse::Data(data) => data,
            AppResponse::Error(error) => {
                return Err(CliError::Usage(format!(
                    "SessionCreate failed: {}",
                    error.message
                )))
            }
            other => {
                return Err(CliError::Turn(format!(
                    "SessionCreate should return Data, got {other:?}"
                )))
            }
        };
        let session_id = data
            .get("session_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| CliError::Turn("SessionCreate 未返回 session_id".into()))?;
        Ok(SessionId::from(session_id))
    }
}

/// 单轮：RunStart（不动 Host 的 provider/model 选择）→ 观测到终态。
async fn run_gui_turn(
    connection: &mut GuiHostConnection,
    session_id: &SessionId,
    prompt: &str,
    video_urls: Vec<pawork_domain::VideoContent>,
    cancel: &mut CancelWatch,
) -> Result<(RunId, RunState), CliError> {
    let envelope = connection
        .client
        .command(
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: prompt.to_string(),
                model: None,
                provider: None,
                profile: None,
                effort: None,
                attachment_ids: Vec::new(),
                web_search: None,
                video_urls,
            },
            connection.source(),
            GuiHostConnection::identity(),
        )
        .await
        .map_err(|error| CliError::Usage(format!("RunStart failed: {error}")))?;
    let run_id = match envelope.response {
        AppResponse::Accepted {
            run_id: Some(run_id),
            ..
        } => run_id,
        AppResponse::Error(error) => {
            return Err(CliError::Usage(format!(
                "RunStart failed: {}",
                error.message
            )))
        }
        other => {
            return Err(CliError::Turn(format!(
                "RunStart should return Accepted with run_id, got {other:?}"
            )))
        }
    };
    let state = observe_gui_run(connection, session_id, &run_id, cancel).await?;
    Ok((run_id, state))
}

/// 在等待 future 期间持续消费取消信号（只记录意图）：断连恢复窗口内的
/// Ctrl-C 不丢失，连接恢复后由 RunCancel 重试块统一送达。
async fn race_cancel<F: std::future::Future>(
    future: F,
    cancel: &mut CancelWatch,
    run_id: &RunId,
    cancel_requested: &mut bool,
) -> F::Output {
    let future = future;
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            _ = cancel.recv() => {
                *cancel_requested = true;
                eprintln!("已记录取消请求，连接恢复后向 run {run_id} 发送 RunCancel");
            }
            output = &mut future => return output,
        }
    }
}

/// 发送一次 RunCancel。明确拒绝只打印一次并停止自动重发：响应体里的
/// AppResponse::Error，以及 ServerFrame::Error 转成的 ClientError::Protocol
///（已终态 Run 的 not_found 走这条，不是传输失败）。传输失败、超时和断连
/// 保持未送达，断线恢复后重试。
async fn send_run_cancel(
    connection: &mut GuiHostConnection,
    run_id: &RunId,
    cancel_delivered: &mut bool,
) {
    match connection
        .client
        .command(
            AppCommand::RunCancel {
                run_id: run_id.clone(),
            },
            connection.source(),
            GuiHostConnection::identity(),
        )
        .await
    {
        Ok(envelope) => {
            if let AppResponse::Error(error) = envelope.response {
                eprintln!("RunCancel 被拒：{}", error.message);
            }
            *cancel_delivered = true;
        }
        Err(ClientError::Protocol(error)) => {
            eprintln!("RunCancel 被拒：{} ({:?})", error.message, error.code);
            *cancel_delivered = true;
        }
        Err(error) => eprintln!("RunCancel 发送失败（{error}）；断线恢复后会重试"),
    }
}

/// 观测一个 run 到终态：直播事件 → 断流恢复（Resume + Timeline 兜底）→
/// 回到直播。Ctrl-C 只对本次 run_id 发 RunCancel（未送达则在恢复后重试），
/// 之后仍等到终态；断开本身不发送 RunCancel。
async fn observe_gui_run(
    connection: &mut GuiHostConnection,
    session_id: &SessionId,
    run_id: &RunId,
    cancel: &mut CancelWatch,
) -> Result<RunState, CliError> {
    let mut cancel_requested = false;
    let mut cancel_delivered = false;
    loop {
        match live_gui_events(
            connection,
            run_id,
            &mut cancel_requested,
            &mut cancel_delivered,
            cancel,
        )
        .await
        {
            GuiStreamOutcome::Terminal(state) => return Ok(state),
            GuiStreamOutcome::Fatal(error) => return Err(error),
            GuiStreamOutcome::Broken => {}
        }
        let mut resumed_live = false;
        for attempt in 0..GUI_HOST_RECONNECT_ATTEMPTS {
            if attempt > 0 {
                race_cancel(
                    tokio::time::sleep(Duration::from_secs(1)),
                    cancel,
                    run_id,
                    &mut cancel_requested,
                )
                .await;
            }
            let outcome = match race_cancel(
                connection.reconnect(),
                cancel,
                run_id,
                &mut cancel_requested,
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(_) => continue,
            };
            if cancel_requested && !cancel_delivered {
                send_run_cancel(connection, run_id, &mut cancel_delivered).await;
            }
            if let Some(outcome) = outcome {
                for envelope in outcome.replayed {
                    let sequence = envelope.global_sequence;
                    let _ = connection.client.ack(sequence).await;
                    if let Some(state) = render_gui_event(&envelope, run_id) {
                        return Ok(state);
                    }
                }
            }
            match race_cancel(
                timeline_gui_terminal(connection, session_id, run_id),
                cancel,
                run_id,
                &mut cancel_requested,
            )
            .await
            {
                Ok(Some(state)) => return Ok(state),
                Ok(None) => {}
                Err(_) => continue,
            }
            resumed_live = true;
            break;
        }
        if !resumed_live {
            return Err(CliError::Turn(format!(
                "run {run_id} 终态未知：GUI Host 不可达（已按上限重连失败）。不新建 Run、不自动取消；请在 Host 恢复后核对"
            )));
        }
    }
}

/// 直播阶段：10s 事件窗 + 15s 心跳 + Ctrl-C（RunCancel 一次，等终态）。
async fn live_gui_events(
    connection: &mut GuiHostConnection,
    run_id: &RunId,
    cancel_requested: &mut bool,
    cancel_delivered: &mut bool,
    cancel: &mut CancelWatch,
) -> GuiStreamOutcome {
    let mut last_heartbeat = tokio::time::Instant::now();
    loop {
        // 恢复窗口内记录的取消意图：信号已被消费、直播阶段等不到新的
        // Ctrl-C，进入直播前按标记补送达，否则该意图会静默丢失。
        if cancel.is_requested() && !*cancel_delivered {
            *cancel_requested = true;
            eprintln!("正在取消 run {run_id}（等待 GUI Host 终态）…");
            send_run_cancel(connection, run_id, cancel_delivered).await;
        }
        tokio::select! {
            biased;
            _ = cancel.recv(), if !*cancel_delivered => {
                *cancel_requested = true;
                eprintln!("正在取消 run {run_id}（等待 GUI Host 终态）…");
                send_run_cancel(connection, run_id, cancel_delivered).await;
            }
            event = connection.client.next_event_timeout(Duration::from_secs(10)) => {
                match event {
                    Ok(envelope) => {
                        let sequence = envelope.global_sequence;
                        if connection.client.ack(sequence).await.is_err() {
                            return GuiStreamOutcome::Broken;
                        }
                        if let Some(state) = render_gui_event(&envelope, run_id) {
                            return GuiStreamOutcome::Terminal(state);
                        }
                    }
                    Err(ClientError::Timeout { .. }) => {
                        if last_heartbeat.elapsed() >= GUI_HOST_HEARTBEAT_INTERVAL {
                            match connection.client.heartbeat().await {
                                Ok(_) => last_heartbeat = tokio::time::Instant::now(),
                                Err(_) => return GuiStreamOutcome::Broken,
                            }
                        }
                    }
                    // 传输断开/超时之外的错误（协议、编解码等）不是连接抖动：
                    // 如实失败，不进入无限重连循环。
                    Err(ClientError::Transport(_)) | Err(ClientError::Disconnected) => {
                        return GuiStreamOutcome::Broken;
                    }
                    Err(error) => {
                        return GuiStreamOutcome::Fatal(CliError::Turn(format!(
                            "gui event stream failed: {error}"
                        )));
                    }
                }
            }
        }
    }
}

/// 最小渲染：AssistantDelta → stdout；工具/审批行 → stderr。返回本 run 的
/// 终态（Completed/Cancelled/Failed 之外一律继续观测，不臆造结果）。
fn render_gui_event(envelope: &AppEventEnvelope, run_id: &RunId) -> Option<RunState> {
    match &envelope.payload {
        AppEvent::RunChanged {
            run_id: event_run,
            state,
        } if event_run == run_id => match state {
            RunState::Completed | RunState::Cancelled | RunState::Failed => Some(state.clone()),
            _ => None,
        },
        AppEvent::AssistantDelta {
            run_id: event_run,
            delta,
            ..
        } if event_run == run_id => {
            print!("{delta}");
            let _ = io::stdout().flush();
            None
        }
        AppEvent::ToolStarted {
            run_id: event_run,
            name,
            ..
        } if event_run == run_id => {
            eprintln!("tool: {name}");
            None
        }
        AppEvent::ToolApprovalRequired {
            run_id: event_run,
            reason,
            ..
        } if event_run == run_id => {
            eprintln!("需要审批：{reason}（在 GUI 窗口中处理）");
            None
        }
        _ => None,
    }
}

/// 断流后按持久化 Timeline 兜底找终态：只认 run_id 匹配的 RunCompleted /
/// RunCancelled / RunFailed。page.complete 只表示走到 head，不是 Run 终态。
/// 游标是 SessionStore 的会话持久 sequence（与 GUI global_sequence 不同数
/// 域）：不能用 ack 序列播种；断线期间没有已维护的会话游标，从 0 起分页。
/// next_sequence 是服务端已消费的最后一条持久事件序列，下一页 after 直接
/// 等于它（服务端 from = after+1），并防游标未前进造成死循环。
async fn timeline_gui_terminal(
    connection: &GuiHostConnection,
    session_id: &SessionId,
    run_id: &RunId,
) -> Result<Option<RunState>, CliError> {
    let mut after: Option<u64> = None;
    loop {
        let envelope = connection
            .client
            .query(
                AppQuery::SessionGet {
                    session_id: session_id.clone(),
                    timeline_after_sequence: after,
                    timeline_limit: Some(200),
                },
                connection.source(),
                GuiHostConnection::identity(),
            )
            .await
            .map_err(|error| CliError::Usage(format!("SessionGet timeline failed: {error}")))?;
        let data = match envelope.response {
            AppResponse::Data(data) => data,
            AppResponse::Error(error) => {
                return Err(CliError::Usage(format!(
                    "SessionGet timeline failed: {}",
                    error.message
                )))
            }
            other => {
                return Err(CliError::Turn(format!(
                    "SessionGet should return Data, got {other:?}"
                )))
            }
        };
        let page_value = data
            .get("timeline_page")
            .cloned()
            .ok_or_else(|| CliError::Turn("SessionGet 未返回 timeline_page".into()))?;
        let page: TimelinePage = serde_json::from_value(page_value)
            .map_err(|error| CliError::Turn(format!("timeline_page 解析失败：{error}")))?;
        for item in page.items {
            if item.run_id.as_deref() == Some(run_id.as_str()) {
                match item.kind {
                    TimelineItemKind::RunCompleted => return Ok(Some(RunState::Completed)),
                    TimelineItemKind::RunCancelled => return Ok(Some(RunState::Cancelled)),
                    TimelineItemKind::RunFailed => return Ok(Some(RunState::Failed)),
                    _ => {}
                }
            }
        }
        match page.next_sequence {
            Some(next) => {
                if after.is_some_and(|current| next <= current) {
                    return Ok(None);
                }
                after = Some(next);
            }
            None => return Ok(None),
        }
    }
}

/// REPL：同一 session 顺序多轮，每轮等到终态再读下一行；/exit 只关闭本
/// 客户端。等输入期间维持心跳（Host 30s 无入站帧即断连）；心跳失败标记
/// 断线并在下一轮输入前重连恢复。会写本地 Core 或 Host 未开放的命令在此
/// 失败关闭。
async fn run_gui_repl<R: tokio::io::AsyncBufRead + Unpin>(
    connection: &mut GuiHostConnection,
    workspace_id: &WorkspaceId,
    resume: Option<&str>,
    reader: &mut R,
    heartbeat_interval: Duration,
) -> Result<(), CliError> {
    let mut session = if let Some(spec) = resume {
        Some(open_gui_session(connection, workspace_id, Some(spec), spec).await?)
    } else {
        None
    };
    eprintln!(
        "pawork chat --gui-host    Ctrl-C 取消当轮（经 Host RunCancel），/exit 退出；/compact /model /provider /plan 此模式不可用"
    );
    // interval_at 避免首 tick 立即触发；Delay 让错过的心跳合并为一次。
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + heartbeat_interval,
        heartbeat_interval,
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut host_alive = true;
    loop {
        print!("> ");
        io::stdout().flush()?;
        let mut line = String::new();
        // read_line 不是 cancellation-safe：future 全程 pinned 复用，心跳
        // 分支只轮询 &mut 引用，读到一半的字节留在 future 里不丢输入草稿。
        let read = reader.read_line(&mut line);
        tokio::pin!(read);
        let n = loop {
            tokio::select! {
                biased;
                _ = heartbeat.tick(), if host_alive => {
                    if connection.client.heartbeat().await.is_err() {
                        host_alive = false;
                        eprintln!("（GUI Host 连接已断，下一轮输入前自动重连）");
                    }
                }
                n = &mut read => break n,
            }
        };
        if n? == 0 {
            return Ok(());
        }
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        if text == "/exit" || text == "/quit" {
            return Ok(());
        }
        if text == "/compact" {
            eprintln!("/compact 在 --gui-host 模式不可用（Host 通道未开放该命令；用本地 CLI 或 GUI 压缩）");
            continue;
        }
        if text == "/model" || text.starts_with("/model ") {
            eprintln!("/model 在 --gui-host 模式不可用：模型选择在共享 Host Core 上持久生效，此模式不做单轮覆盖");
            continue;
        }
        if text == "/provider" || text.starts_with("/provider ") {
            eprintln!("/provider 在 --gui-host 模式不可用：provider 选择在共享 Host Core 上持久生效，此模式不做单轮覆盖");
            continue;
        }
        if text == "/plan" || text.starts_with("/plan ") {
            eprintln!(
                "/plan 在 --gui-host 模式不可用（plan 命令是本进程 Core 的写，此模式不加载 Core）"
            );
            continue;
        }
        if !host_alive {
            let outcome = connection.reconnect().await.map_err(|error| {
                CliError::Usage(format!(
                    "GUI Host 重连失败（{error}）：不新建 Run、不回退本地 Core；Host 恢复后重新启动 CLI"
                ))
            })?;
            if let Some(outcome) = outcome {
                for envelope in outcome.replayed {
                    let _ = connection.client.ack(envelope.global_sequence).await;
                }
            }
            host_alive = true;
        }
        let session_id = if let Some(id) = &session {
            id.clone()
        } else {
            let id = open_gui_session(connection, workspace_id, None, text).await?;
            eprintln!("session {id}");
            session = Some(id.clone());
            id
        };
        println!();
        let (run_id, state) = {
            let mut cancel = CancelWatch::install();
            run_gui_turn(connection, &session_id, text, Vec::new(), &mut cancel).await?
        };
        eprintln!("\nrun {run_id} {state:?}");
    }
}

fn finish_gui_terminal(state: RunState, run_id: &RunId) -> Result<(), CliError> {
    match state {
        RunState::Completed => Ok(()),
        RunState::Cancelled => Err(CliError::Cancelled),
        RunState::Failed => Err(CliError::Turn(format!(
            "run {run_id} failed on the GUI Host (see the GUI session timeline for details)"
        ))),
        _ => unreachable!("finish_gui_terminal 只处理终态"),
    }
}

fn gui_video_parts(urls: &[String]) -> Result<Vec<pawork_domain::VideoContent>, CliError> {
    if urls.len() > 4 {
        return Err(CliError::Usage("--video-url 每轮最多 4 项".into()));
    }
    urls.iter().map(|url| video_content_from_url(url)).collect()
}

/// URL → VideoContent（与本地路径同一套语法边界校验与媒体类型推断）。
fn video_content_from_url(url: &str) -> Result<pawork_domain::VideoContent, CliError> {
    let extension = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('.')
        .next()
        .unwrap_or("");
    let video = pawork_domain::VideoContent {
        url: url.to_string(),
        media_type: match extension.to_ascii_lowercase().as_str() {
            "webm" => "video/webm",
            "mov" => "video/quicktime",
            "mpeg" | "mpg" => "video/mpeg",
            "avi" => "video/x-msvideo",
            _ => "video/mp4",
        }
        .into(),
    };
    video
        .validate()
        .map_err(|error| CliError::Usage(error.into()))?;
    Ok(video)
}

#[cfg(test)]
mod tests {
    use super::{image_media_type, local_image_parts, reject_gui_host_conflicts};
    use crate::{Cli, CliError, Command};
    use pawork_domain::{ContentPart, ImageSource};

    #[test]
    fn local_image_becomes_base64_part() {
        let dir = std::env::temp_dir().join(format!("pawork-cli-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("shot.png");
        std::fs::write(&path, b"png-bytes").expect("write");

        let parts = local_image_parts(&[path.display().to_string()]).expect("image part");
        assert!(matches!(&parts[0], ContentPart::Text(text) if text.text.contains("shot.png")));
        match &parts[1] {
            ContentPart::Image(image) => {
                assert_eq!(image.media_type, "image/png");
                assert!(
                    matches!(&image.source, ImageSource::Base64(data) if data == "cG5nLWJ5dGVz")
                );
            }
            other => panic!("expected image, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);

        let error = local_image_parts(&["notes.txt".into()]).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("png"), "{message}");
        assert_eq!(image_media_type("photo.JPEG"), Some("image/jpeg"));
    }

    fn gui_host_cli(mutate: impl FnOnce(&mut Cli)) -> Cli {
        let mut cli = Cli {
            provider: None,
            model: None,
            instance: "default".into(),
            json: false,
            approval_mode: None,
            trust_workspaces: false,
            computer_native: "host".into(),
            command: Command::Chat {
                prompt: Some("hi".into()),
                resume: None,
                branch: None,
                images: Vec::new(),
                video_urls: Vec::new(),
                gui_host: Some(None),
            },
        };
        mutate(&mut cli);
        cli
    }

    /// CU-10 #5：会写本地 Core、需要本地解析或走 HeadlessResponse 的标志，
    /// 在 --gui-host 模式一律 fail-closed（Usage），不静默忽略。
    #[test]
    fn gui_host_rejects_local_core_writes_and_local_only_features() {
        let cases: Vec<(Cli, &str)> = vec![
            (gui_host_cli(|c| c.json = true), "--json"),
            (gui_host_cli(|c| c.provider = Some("p1".into())), "-p"),
            (gui_host_cli(|c| c.model = Some("m1".into())), "-m"),
            (
                gui_host_cli(|c| c.approval_mode = Some("read-only".into())),
                "--approval-mode",
            ),
            (
                gui_host_cli(|c| c.trust_workspaces = true),
                "--trust-workspaces",
            ),
            (
                gui_host_cli(|c| {
                    if let Command::Chat { branch, .. } = &mut c.command {
                        *branch = Some("b1".into());
                    }
                }),
                "--branch",
            ),
            (
                gui_host_cli(|c| {
                    if let Command::Chat { images, .. } = &mut c.command {
                        images.push("a.png".into());
                    }
                }),
                "--image",
            ),
            (
                gui_host_cli(|c| {
                    if let Command::Chat { resume, .. } = &mut c.command {
                        *resume = Some("latest".into());
                    }
                }),
                "resume latest",
            ),
            (
                gui_host_cli(|c| {
                    if let Command::Chat {
                        prompt, video_urls, ..
                    } = &mut c.command
                    {
                        *prompt = None;
                        video_urls.push("https://example.test/a.mp4".into());
                    }
                }),
                "--video-url without --prompt",
            ),
        ];
        for (cli, label) in cases {
            let error = reject_gui_host_conflicts(&cli).expect_err(label);
            assert!(
                matches!(error, CliError::Usage(_)),
                "{label} 应为 Usage 拒绝，got {error:?}"
            );
        }
    }

    #[test]
    fn gui_host_accepts_prompt_turn_and_full_session_resume() {
        let cli = gui_host_cli(|_| {});
        assert!(reject_gui_host_conflicts(&cli).is_ok());

        // 完整 session id + --video-url（伴随 --prompt）是首版允许面。
        let cli = gui_host_cli(|c| {
            if let Command::Chat {
                resume, video_urls, ..
            } = &mut c.command
            {
                *resume = Some("ses-12345678".into());
                video_urls.push("https://example.test/a.mp4".into());
            }
        });
        assert!(reject_gui_host_conflicts(&cli).is_ok());
    }

    // =====================================================================
    // CU-10 #5 P2：--gui-host REPL 空闲心跳与重连期取消（内存 Transport 全
    // 链路；外部边界替换：stdin=duplex 注入、连接=MemoryTransport、
    // Provider=Mock）。心跳间隔是测试注入值（80ms），不改生产 15s/30s 契约。
    // =====================================================================

    use super::{
        observe_gui_run, open_gui_session, run_gui_repl, run_gui_turn, CancelWatch,
        GuiHostConnection,
    };
    use pawork_app::{AppCore, GuiHostAdapter};
    use pawork_client::{
        ClientAuthentication, ClientConfig, ConnectOptions, GuiClient, TimelineItemKind,
    };
    use pawork_domain::WorkspaceId;
    use pawork_protocol::RunState;
    use pawork_testkit::{MockProvider, MockScript};
    use pawork_transport::{
        ConnectionInfo, GuiConnection, GuiListener, GuiTransportClient, MemoryTransport,
        TransportEndpoint, TransportError, TransportFrame,
    };
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    static WIRE_CHANNEL_NEXT: AtomicUsize = AtomicUsize::new(1);

    /// client→server 帧计数连接：空闲心跳以真实发出的 Heartbeat 帧为证。
    struct CountingConnection {
        inner: Box<dyn GuiConnection>,
        sent: Arc<AtomicUsize>,
        run_cancels: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl GuiConnection for CountingConnection {
        async fn send(&self, frame: TransportFrame) -> Result<(), TransportError> {
            self.sent.fetch_add(1, AtomicOrdering::Relaxed);
            if frame
                .as_bytes()
                .windows(b"run_cancel".len())
                .any(|window| window == b"run_cancel")
            {
                self.run_cancels.fetch_add(1, AtomicOrdering::Relaxed);
            }
            self.inner.send(frame).await
        }

        async fn receive(&self) -> Result<TransportFrame, TransportError> {
            self.inner.receive().await
        }

        async fn close(&self) -> Result<(), TransportError> {
            self.inner.close().await
        }

        fn info(&self) -> ConnectionInfo {
            self.inner.info()
        }
    }

    struct CountingTransport {
        inner: Arc<MemoryTransport>,
        sent: Arc<AtomicUsize>,
        run_cancels: Arc<AtomicUsize>,
    }

    /// 恢复阶段的 connect 在真正拨号前挂起：测试先看到「已进入重连」，
    /// 再注入取消，最后放行。首连（attempt 0）直接通过。
    struct RecoveryHoldTransport {
        inner: Arc<MemoryTransport>,
        connects: AtomicUsize,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl GuiTransportClient for RecoveryHoldTransport {
        async fn connect(
            &self,
            endpoint: TransportEndpoint,
            options: ConnectOptions,
        ) -> Result<Box<dyn GuiConnection>, TransportError> {
            let attempt = self.connects.fetch_add(1, AtomicOrdering::SeqCst);
            if attempt > 0 {
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.inner.connect(endpoint, options).await
        }
    }

    /// 记录每次 Provider stream 的最后一条 User 文本，断言半行补齐后的完整输入。
    struct CapturingProvider {
        inner: Arc<MockProvider>,
        user_texts: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl pawork_domain::ModelProvider for CapturingProvider {
        fn id(&self) -> pawork_domain::ProviderId {
            self.inner.id()
        }

        async fn list_models(
            &self,
            credential: Option<&pawork_domain::ResolvedCredential>,
        ) -> Result<Vec<pawork_domain::ModelDefinition>, pawork_domain::ProviderError> {
            self.inner.list_models(credential).await
        }

        async fn stream(
            &self,
            request: &pawork_domain::CanonicalModelRequest,
            sink: &dyn pawork_domain::ProviderEventSink,
            cancel: pawork_domain::CancellationToken,
        ) -> Result<pawork_domain::ModelResponseSummary, pawork_domain::ProviderError> {
            let text = request
                .messages
                .iter()
                .rev()
                .find(|message| message.role == pawork_domain::MessageRole::User)
                .map(|message| {
                    message
                        .content
                        .iter()
                        .filter_map(|part| match part {
                            ContentPart::Text(text) => Some(text.text.as_str()),
                            _ => None,
                        })
                        .collect::<String>()
                })
                .unwrap_or_default();
            self.user_texts.lock().expect("user texts").push(text);
            self.inner.stream(request, sink, cancel).await
        }
    }

    #[async_trait::async_trait]
    impl GuiTransportClient for CountingTransport {
        async fn connect(
            &self,
            endpoint: TransportEndpoint,
            options: ConnectOptions,
        ) -> Result<Box<dyn GuiConnection>, TransportError> {
            let inner = self.inner.connect(endpoint, options).await?;
            Ok(Box::new(CountingConnection {
                inner,
                sent: Arc::clone(&self.sent),
                run_cancels: Arc::clone(&self.run_cancels),
            }))
        }
    }

    /// Mock Core + GuiHostAdapter + GuiServer（内存 Transport）：CLI 生产
    /// 控制循环（REPL / run_gui_turn / observe_gui_run）跑在真实 GuiClient
    /// wire 上；服务端断连/重绑通过 accepted 连接与 listener 控制。
    struct GuiWireHarness {
        adapter: GuiHostAdapter,
        provider: Arc<MockProvider>,
        transport: Arc<MemoryTransport>,
        connections: Arc<pawork_gui_server::ConnectionManager>,
        listener: Option<Arc<dyn GuiListener>>,
        channel: String,
        accepted: Arc<tokio::sync::Mutex<Vec<Box<dyn GuiConnection>>>>,
        workspace: WorkspaceId,
        user_texts: Arc<std::sync::Mutex<Vec<String>>>,
        _dir: tempfile::TempDir,
    }

    async fn gui_wire_harness(scripts: Vec<MockScript>) -> GuiWireHarness {
        let provider =
            MockProvider::sequence(scripts).with_id(pawork_domain::ProviderId::from("mock"));
        let user_texts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let capturing = CapturingProvider {
            inner: Arc::new(provider.clone()),
            user_texts: Arc::clone(&user_texts),
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let mut core = AppCore::from_parts(
            Arc::new(capturing),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        core.attach_workspace(dir.path()).expect("attach workspace");
        let workspace = core.workspace_id().clone();
        let core = Arc::new(tokio::sync::RwLock::new(core));
        let connections = Arc::new(pawork_gui_server::ConnectionManager::new());
        let adapter = GuiHostAdapter::from_locked(
            Arc::clone(&core),
            Arc::new(pawork_app::GuiApprovalHost::new()),
        )
        .with_connections(Arc::clone(&connections));
        let channel = format!(
            "cli-repl-wire-{}",
            WIRE_CHANNEL_NEXT.fetch_add(1, AtomicOrdering::Relaxed)
        );
        let transport = Arc::new(MemoryTransport::new());
        let listener = bind_wire_listener(&transport, &channel, &adapter, &connections).await;
        let accepted: Arc<tokio::sync::Mutex<Vec<Box<dyn GuiConnection>>>> = Arc::default();
        spawn_wire_accept_loop(&listener, &accepted);
        GuiWireHarness {
            adapter,
            provider: Arc::new(provider),
            transport,
            connections,
            listener: Some(listener),
            channel,
            accepted,
            workspace,
            user_texts,
            _dir: dir,
        }
    }

    impl GuiWireHarness {
        fn endpoint(&self) -> TransportEndpoint {
            TransportEndpoint::Memory {
                channel: self.channel.clone(),
            }
        }

        /// 关闭当前 listener（channel 注销，connect 失败）并保留句柄供重绑。
        async fn close_listener(&mut self) {
            if let Some(listener) = self.listener.take() {
                let _ = listener.close().await;
            }
        }

        /// 重新绑定同一 channel：Host 恢复可连。
        async fn rebind(&mut self) {
            let listener = bind_wire_listener(
                &self.transport,
                &self.channel,
                &self.adapter,
                &self.connections,
            )
            .await;
            spawn_wire_accept_loop(&listener, &self.accepted);
            self.listener = Some(listener);
        }
    }

    async fn bind_wire_listener(
        transport: &Arc<MemoryTransport>,
        channel: &str,
        adapter: &GuiHostAdapter,
        connections: &Arc<pawork_gui_server::ConnectionManager>,
    ) -> Arc<dyn GuiListener> {
        let server = pawork_gui_server::GuiServer::new(pawork_gui_server::GuiServerConfig {
            host: Arc::new(adapter.clone()),
            handshake: pawork_protocol::HandshakeService::new(
                pawork_gui_server::GuiHost::instance_id(adapter),
                pawork_protocol::SUPPORTED_API_VERSIONS.to_vec(),
                vec![pawork_protocol::GuiCapability::Events],
            ),
            transport: transport.clone(),
            connections: Some(Arc::clone(connections)),
        });
        let listener = server
            .bind(TransportEndpoint::Memory {
                channel: channel.to_string(),
            })
            .await
            .expect("bind memory transport");
        drop(server);
        Arc::from(listener)
    }

    /// 后台 accept：CLI 重连产生的新连接也必须被接住，否则握手挂起。
    fn spawn_wire_accept_loop(
        listener: &Arc<dyn GuiListener>,
        accepted: &Arc<tokio::sync::Mutex<Vec<Box<dyn GuiConnection>>>>,
    ) {
        let listener = Arc::clone(listener);
        let accepted = Arc::clone(accepted);
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok(connection) => accepted.lock().await.push(connection),
                    Err(_) => return,
                }
            }
        });
    }

    /// 构造与生产同构的 GuiHostConnection（握手名 pawork-cli、同一 transport
    /// 复用于重连；往返超时缩短到 500ms 让静默断连可观测）。
    async fn connect_cli_wire(
        transport: Arc<dyn GuiTransportClient>,
        endpoint: TransportEndpoint,
    ) -> GuiHostConnection {
        let options = ConnectOptions {
            timeout_ms: 5_000,
            client_label: Some("pawork-cli-gui-host-test".into()),
            max_frame_bytes: 1024 * 1024,
        };
        let config = ClientConfig {
            client_name: "pawork-cli".into(),
            timeout: Duration::from_millis(500),
            ..ClientConfig::default()
        };
        let authentication = ClientAuthentication {
            scheme: "test".into(),
            proof: "test".into(),
        };
        let client = GuiClient::connect_with_config(
            Arc::clone(&transport),
            endpoint.clone(),
            options.clone(),
            Some(authentication.clone()),
            config.clone(),
        )
        .await
        .expect("wire connect + handshake");
        client
            .subscribe_all()
            .await
            .expect("subscribe all (mirrors run_on_gui_host)");
        GuiHostConnection {
            client,
            transport,
            endpoint,
            options,
            authentication,
            config,
        }
    }

    async fn wait_for(condition: impl Fn() -> bool) {
        for _ in 0..2000 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition not met within 20s");
    }

    /// P2-1：等输入期间维持心跳；跨空闲期两轮输入各恰好一个 Run；空闲期
    /// 服务端断连后，下一轮输入前自动重连（不新建多余 Run、不丢输入行）。
    #[tokio::test]
    async fn gui_host_repl_heartbeats_across_idle_and_recovers_before_next_turn() {
        let harness = gui_wire_harness(vec![
            MockScript::new().text("ok1").complete(),
            MockScript::new().text("ok2").complete(),
        ])
        .await;
        let counting = Arc::new(CountingTransport {
            inner: Arc::clone(&harness.transport),
            sent: Arc::new(AtomicUsize::new(0)),
            run_cancels: Arc::new(AtomicUsize::new(0)),
        });
        let sent = Arc::clone(&counting.sent);
        let mut connection = connect_cli_wire(counting, harness.endpoint()).await;

        let (mut writer, reader) = tokio::io::duplex(64);
        let mut reader = tokio::io::BufReader::new(reader);
        let workspace = harness.workspace.clone();
        let provider = Arc::clone(&harness.provider);
        let repl = tokio::spawn(async move {
            run_gui_repl(
                &mut connection,
                &workspace,
                None,
                &mut reader,
                Duration::from_millis(80),
            )
            .await
        });

        // 轮 1：半行跨过至少一次心跳再补齐换行。read_line future 被 pin 复用，
        // Provider 收到的 User 文本必须是完整行，不能是 "fir"。
        writer.write_all(b"fir").await.expect("write partial");
        let partial_baseline = sent.load(AtomicOrdering::SeqCst);
        wait_for(|| sent.load(AtomicOrdering::SeqCst) > partial_baseline).await;
        assert!(
            provider.calls().is_empty(),
            "partial line must not start a run before the newline"
        );
        writer
            .write_all(b"st\n")
            .await
            .expect("complete first line");
        wait_for(|| provider.calls().len() == 1).await;
        wait_for(|| harness.adapter.runs().active().is_empty()).await;
        assert_eq!(
            harness.user_texts.lock().expect("user texts").as_slice(),
            ["first"],
            "heartbeat between partial bytes must not truncate the RunStart text"
        );

        // 空闲期心跳真实发出：80ms 间隔 × 300ms ≥ 2 帧。
        let baseline = sent.load(AtomicOrdering::SeqCst);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let after_idle = sent.load(AtomicOrdering::SeqCst);
        assert!(
            after_idle >= baseline + 2,
            "idle heartbeats must keep the host connection alive: {baseline} -> {after_idle}"
        );

        // 服务端断连：CLI 由下一次心跳往返超时发现（≤ 80ms + 500ms），
        // 不清空已等待的输入行。
        let first = harness.accepted.lock().await.remove(0);
        first.close().await.expect("close server side");
        drop(first);
        // 等服务端会话注销，再给 CLI 一次心跳往返超时的窗口发现断连
        //（tick ≤ 80ms + Pong 超时 500ms）；期间到达的输入行仍保留。
        wait_for(|| harness.connections.count() == 0).await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        writer.write_all(b"second\n").await.expect("write second");

        // 下一轮输入前重连恢复：第二轮恰好一个 provider stream（零重执行）。
        wait_for(|| provider.calls().len() == 2).await;
        wait_for(|| harness.adapter.runs().active().is_empty()).await;
        drop(writer);
        repl.await
            .expect("repl task join")
            .expect("repl returns Ok");
        assert!(
            !harness.accepted.lock().await.is_empty(),
            "REPL 必须在第二轮前重连（关闭后被接受到新服务端连接）"
        );
        assert_eq!(harness.provider.calls().len(), 2, "每轮恰好一个 Run");
        assert_eq!(
            harness.user_texts.lock().expect("user texts").as_slice(),
            ["first", "second"],
            "断连恢复后的第二轮仍是完整输入，且没有第三次 Run"
        );
    }

    /// P2-2：断流恢复窗口内的 Ctrl-C 不丢失——重连成功后向原 run_id 送达
    /// RunCancel、收到真实 Cancelled 终态、零 RunStart 重执行。
    #[tokio::test]
    async fn gui_host_cancel_during_reconnect_window_reaches_run_cancel_and_terminal() {
        let mut harness = gui_wire_harness(vec![MockScript::new().wait_for_cancellation()]).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let transport = Arc::new(RecoveryHoldTransport {
            inner: Arc::clone(&harness.transport),
            connects: AtomicUsize::new(0),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let mut connection = connect_cli_wire(transport, harness.endpoint()).await;
        let session =
            open_gui_session(&mut connection, &harness.workspace, None, "cancel me").await;
        assert!(session.is_ok(), "session create: {session:?}");
        let session = session.unwrap();

        let (mut cancel, trigger) = CancelWatch::manual();
        let requested = cancel.requested_flag();
        let turn_session = session.clone();
        let turn = tokio::spawn(async move {
            run_gui_turn(
                &mut connection,
                &turn_session,
                "cancel me",
                Vec::new(),
                &mut cancel,
            )
            .await
        });

        // run 已在 provider stream 上阻塞（等待取消）。turn 在独立任务里持续 poll。
        wait_for(|| harness.provider.calls().len() == 1).await;
        assert_eq!(harness.provider.calls().len(), 1, "run must be in flight");

        // 断流：注销 channel 并关掉服务端连接。恢复 connect 会停在 Hold 上，
        // 直到测试确认取消已被这次 connect 的 race 消费后再放行。
        harness.close_listener().await;
        let first = harness.accepted.lock().await.remove(0);
        first.close().await.expect("close server side");
        drop(first);

        tokio::time::timeout(Duration::from_secs(20), entered.notified())
            .await
            .expect("recovery connect must start before the cancel is injected");
        trigger.send(()).await.expect("trigger cancel");
        wait_for(|| requested.load(AtomicOrdering::Acquire)).await;
        harness.rebind().await;
        release.notify_one();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let (run_id, state) = tokio::time::timeout_at(deadline, turn)
            .await
            .expect("turn must finish after host recovery")
            .expect("turn task join")
            .expect("turn ok");
        assert_eq!(
            state,
            RunState::Cancelled,
            "恢复后送达 RunCancel 得真实终态"
        );
        assert_eq!(
            harness.provider.calls().len(),
            1,
            "取消路径零 RunStart 重执行"
        );

        // 终态落进持久化 Timeline（RunCancelled，与 CLI 观察一致）。
        let page =
            pawork_gui_server::GuiHost::timeline(&harness.adapter, &session, None, Some(200))
                .await
                .expect("timeline");
        assert!(
            page.items.iter().any(|item| {
                item.run_id.as_deref() == Some(run_id.as_str())
                    && item.kind == TimelineItemKind::RunCancelled
            }),
            "timeline must record RunCancelled for {run_id}: {:?}",
            page.items
        );

        // 原 Run 已终态：真实 Host 对再次 RunCancel 回 not_found，经
        // ServerFrame::Error 变成 ClientError::Protocol。只允许这一次发送。
        wait_for(|| harness.adapter.runs().active().is_empty()).await;
        let run_cancels = Arc::new(AtomicUsize::new(0));
        let counting = Arc::new(CountingTransport {
            inner: Arc::clone(&harness.transport),
            sent: Arc::new(AtomicUsize::new(0)),
            run_cancels: Arc::clone(&run_cancels),
        });
        let mut reject_connection = connect_cli_wire(counting, harness.endpoint()).await;
        let (mut reject_cancel, reject_trigger) = CancelWatch::manual();
        let reject_session = session.clone();
        let reject_run = run_id.clone();
        let reject_observe = tokio::spawn(async move {
            observe_gui_run(
                &mut reject_connection,
                &reject_session,
                &reject_run,
                &mut reject_cancel,
            )
            .await
        });
        reject_trigger
            .send(())
            .await
            .expect("trigger cancel against the finished run");
        wait_for(|| run_cancels.load(AtomicOrdering::SeqCst) >= 1).await;
        let mut seen = run_cancels.load(AtomicOrdering::SeqCst);
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let now = run_cancels.load(AtomicOrdering::SeqCst);
            assert_eq!(
                now, seen,
                "Host not_found must be printed once and must not resend RunCancel"
            );
            seen = now;
        }
        assert_eq!(seen, 1, "finished run rejects RunCancel exactly once");
        reject_observe.abort();
    }
}
