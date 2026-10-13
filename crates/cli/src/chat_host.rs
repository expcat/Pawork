use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use pawork_app::session_title_from_text;
use pawork_client::{
    ClientAuthentication, ClientConfig, ClientError, ConnectOptions, GuiClient, GuiTransportClient,
    LocalTransport, TimelineItemKind, TimelinePage, TransportEndpoint,
};
use pawork_domain::{RunId, SessionId, WorkspaceId};
use pawork_protocol::{
    ActorIdentity, AppCommand, AppEvent, AppEventEnvelope, AppQuery, AppResponse, CommandSource,
    RunState,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::chat::video_content_from_url;
use crate::{Cli, CliError, Command};

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

#[cfg(test)]
mod tests {
    use super::reject_gui_host_conflicts;
    use crate::{Cli, CliError, Command};
    use pawork_domain::ContentPart;

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
