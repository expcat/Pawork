#![allow(unexpected_cfgs)] // objc 0.2 macros probe the historical cargo-clippy feature.

//! pawork-desktop：GPUI Agent 壳（S10 Replay / Fork / Terminal）。
//!
//! 四层结构：ui（GPUI 渲染与交互）/ projection（纯状态机）/
//! controller（只调 pawork-client）/ platform（socket 发现 + tokio 宿主）。

mod controller;
mod platform;
mod projection;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    prelude::*, px, size, App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};

use crate::controller::ControllerEvent;
use crate::projection::{DesktopProjection, ResumeApply, TimelineEntryKind};
use pawork_client::{AppEvent, EventStream, RunState};

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(usage) => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    };
    // R1 Wave B fixture barrier（可选）：空值视同未设置；None 则全程零开销，
    // 仅真实窗口路径使用（--probe / --probe-smoke 不发射 barrier）。
    let barrier_dir = std::env::var_os("PAWORK_UI_BARRIER_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let socket = args
        .socket
        .unwrap_or_else(|| platform::socket_path_for_instance(args.instance.as_deref()));
    if args.probe_smoke {
        std::process::exit(run_probe_smoke(socket));
    }
    if args.probe {
        std::process::exit(run_probe(socket));
    }
    run_app(socket, barrier_dir);
}

struct Args {
    socket: Option<PathBuf>,
    instance: Option<String>,
    probe: bool,
    probe_smoke: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        socket: None,
        instance: None,
        probe: false,
        probe_smoke: false,
    };
    let usage =
        "usage: pawork-desktop [--socket <path>] [--instance <name>] [--probe|--probe-smoke]";
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--socket" => {
                let Some(path) = iter.next() else {
                    return Err(usage.into());
                };
                args.socket = Some(PathBuf::from(path));
            }
            "--instance" => {
                let Some(name) = iter.next() else {
                    return Err(usage.into());
                };
                args.instance = Some(name);
            }
            "--probe" => args.probe = true,
            "--probe-smoke" => args.probe_smoke = true,
            other => return Err(format!("unknown argument {other}; {usage}")),
        }
    }
    Ok(args)
}

/// 连接冒烟模式：不开窗，connect + snapshot 后打印一行摘要退出
/// （供波 B 自动化连接验证）。
fn run_probe(socket: PathBuf) -> i32 {
    let platform = platform::Platform::new();
    let controller = controller::DesktopController::new(platform.handle());
    match platform.block_on(async {
        let connected = controller.connect(socket).await?;
        let models = controller.fetch_models().await?;
        Ok::<_, String>((connected.snapshot, models))
    }) {
        Ok((snapshot, models)) => {
            let projection = projection::DesktopProjection::from_snapshot(&snapshot);
            let catalog = models
                .iter()
                .map(|model| format!("{}/{}", model.provider_id, model.id))
                .collect::<Vec<_>>()
                .join(",");
            println!(
                "connected: instance={}, sessions={}, models={} catalog={}",
                snapshot.instance_id.as_str(),
                projection.sessions.len(),
                models.len(),
                catalog
            );
            0
        }
        Err(reason) => {
            eprintln!("pawork-desktop probe failed: {reason}");
            1
        }
    }
}

/// 真实功能冒烟：流式回合、审批、取消、两次断线重连。
/// 只选用一个授权精确对，签名行输出 `model_switch=uncovered`。
fn run_probe_smoke(socket: PathBuf) -> i32 {
    let platform = platform::Platform::new();
    let controller = controller::DesktopController::new(platform.handle());
    match platform.block_on(probe_smoke(&controller, socket)) {
        Ok(report) => {
            println!("{report}");
            0
        }
        Err(reason) => {
            eprintln!("pawork-desktop probe-smoke failed: {reason}");
            1
        }
    }
}

async fn probe_smoke(
    controller: &controller::DesktopController,
    socket: PathBuf,
) -> Result<String, String> {
    let connected = controller.connect(socket.clone()).await?;
    let snapshot = connected.snapshot;
    let mut events = connected.events;
    let trusted = workspace_trusted(&snapshot);
    let mut projection = DesktopProjection::from_snapshot(&snapshot);
    let models = controller.fetch_models().await?;
    projection.set_models(models.clone());
    let workspace = projection
        .workspace_id
        .clone()
        .unwrap_or_else(|| "ws-default".into());
    controller.create_session(Some(workspace));
    let session_id = wait_for_session(&events, &mut projection).await?;
    projection.select_session(&session_id);

    // 当次消息只携带目录里第一个启用且 text 的授权精确对。不写持久默认，
    // 不按 id 跨 provider 回退；请求失败后不改选下一对。
    let first = pick_probe_smoke_model(&models)?.clone();
    projection.set_pending_model(first.provider_id.clone(), first.id.clone());
    controller.send_message(
        session_id.clone(),
        "Reply with exactly one word: pong".into(),
        Some((first.provider_id.clone(), first.id.clone())),
        None,
        Default::default(),
    );
    let first_turn = wait_for_turn(&events, &mut projection, Duration::from_secs(90), None).await?;

    // 写文件只看审批卡和该 run 的终态。模型回复不算文件已落盘。
    controller.send_message(
        session_id.clone(),
        "Write target/s7-wave-d-smoke.txt containing exactly hello-s7d. Use the write tool.".into(),
        Some((first.provider_id.clone(), first.id.clone())),
        None,
        Default::default(),
    );
    let approval =
        wait_for_approval_or_turn(&events, &mut projection, Duration::from_secs(90)).await?;
    if approval {
        let pending = projection
            .pending_approval
            .as_ref()
            .ok_or_else(|| "approval card missing after ToolApprovalRequired".to_string())?;
        let approved_run = pending.run_id.clone();
        controller.approve(
            approved_run.clone(),
            pending.tool_call_id.clone(),
            "approve_once",
        );
        // approve 不再发 MessageSent。后续终态只认这张审批卡上的 run。
        wait_for_turn(
            &events,
            &mut projection,
            Duration::from_secs(90),
            Some(approved_run.as_str()),
        )
        .await?;
    }

    controller.send_message(
        session_id.clone(),
        "Count slowly from 1 to 80 in digits only.".into(),
        Some((first.provider_id.clone(), first.id.clone())),
        None,
        Default::default(),
    );
    let run_id = wait_for_run_id(&events, &mut projection, Duration::from_secs(30)).await?;
    controller.cancel_run(run_id.clone());
    wait_for_cancel(&events, &mut projection, Duration::from_secs(30), &run_id).await?;

    let assistant_turns = projection
        .timeline
        .iter()
        .filter(|entry| matches!(entry.kind, TimelineEntryKind::AssistantMessage { .. }))
        .count();

    controller.disconnect().await;
    let connected = controller.connect(socket.clone()).await?;
    apply_probe_resume(
        &mut projection,
        &connected.snapshot,
        connected.resume.as_ref(),
        &models,
        &session_id,
        controller,
    );
    events = connected.events;
    if !projection
        .sessions
        .iter()
        .any(|session| session.session_id == session_id)
        && connected.resume.as_ref().is_some_and(|outcome| {
            matches!(
                outcome.disposition,
                pawork_client::ResumeDisposition::SnapshotRequired { .. }
            )
        })
    {
        return Err("reconnect snapshot missing the smoke session".into());
    }
    let persisted = if projection.timeline.is_empty() {
        if projection.active_session_id.as_deref() != Some(session_id.as_str()) {
            projection.select_session(&session_id);
        }
        controller.open_session(session_id.clone());
        wait_for_timeline(&events, &mut projection, Duration::from_secs(15)).await?
    } else {
        projection.timeline.len()
    };
    if persisted == 0 {
        return Err("reconnect timeline is empty".into());
    }

    controller.send_message(
        session_id.clone(),
        "Count slowly from 1 to 80 in digits only.".into(),
        Some((first.provider_id.clone(), first.id.clone())),
        None,
        Default::default(),
    );
    let live_run = wait_for_run_id(&events, &mut projection, Duration::from_secs(30)).await?;
    controller.disconnect().await;
    let connected = controller.connect(socket).await?;
    apply_probe_resume(
        &mut projection,
        &connected.snapshot,
        connected.resume.as_ref(),
        &models,
        &session_id,
        controller,
    );
    events = connected.events;
    if projection.active_session_id.as_deref() != Some(session_id.as_str()) {
        projection.select_session(&session_id);
    }
    let disconnect_survive = if projection.active_run_id.as_deref() == Some(live_run.as_str())
        || projection
            .active_runs
            .iter()
            .any(|run| run.run_id == live_run)
    {
        controller.cancel_run(live_run.clone());
        wait_for_cancel(&events, &mut projection, Duration::from_secs(30), &live_run).await?;
        "running"
    } else {
        // 不在 active 列表只说明快照没把它当作进行中，不能当成 completed。
        if !has_terminal_run_label(&projection, &live_run) {
            if projection.active_session_id.as_deref() != Some(session_id.as_str()) {
                projection.select_session(&session_id);
            }
            controller.open_session(session_id.clone());
            wait_for_timeline(&events, &mut projection, Duration::from_secs(15)).await?;
        }
        classify_disconnect_run(&projection, &live_run)?
    };

    Ok(format!(
        "probe-smoke: session={session_id} models={} first={}/{} first_turn={first_turn} model_switch=uncovered trusted={} approval={} assistant_turns={assistant_turns} cancelled=1 persisted={persisted} disconnect_survive={disconnect_survive}",
        models.len(),
        first.provider_id,
        first.id,
        match trusted {
            Some(true) => "1",
            Some(false) => "0",
            None => "unknown",
        },
        if approval {
            "approved"
        } else {
            "not_requested"
        },
    ))
}

fn apply_probe_resume(
    projection: &mut DesktopProjection,
    snapshot: &pawork_client::Snapshot,
    resume: Option<&pawork_client::ResumeOutcome>,
    models: &[projection::ModelEntry],
    session_id: &str,
    controller: &controller::DesktopController,
) {
    projection.set_models(models.to_vec());
    match resume {
        None => {
            *projection = DesktopProjection::from_snapshot(snapshot);
            projection.set_models(models.to_vec());
            projection.select_session(session_id);
        }
        Some(outcome) => match projection.apply_resume_outcome(outcome, snapshot) {
            ResumeApply::ReplaceBaseline | ResumeApply::Fresh => {
                if projection
                    .sessions
                    .iter()
                    .any(|session| session.session_id == session_id)
                {
                    projection.select_session(session_id);
                    controller.open_session(session_id.to_string());
                }
            }
            ResumeApply::Continued { .. } | ResumeApply::Unchanged => {}
        },
    }
}

fn workspace_trusted(snapshot: &pawork_client::Snapshot) -> Option<bool> {
    snapshot.sections.iter().find_map(|section| {
        let kind = serde_json::to_value(&section.kind).ok()?;
        if kind.as_str() != Some("workspaces") {
            return None;
        }
        section
            .data
            .as_ref()?
            .as_array()?
            .first()?
            .get("trusted")?
            .as_bool()
    })
}

/// 当次功能冒烟的授权精确对，按优先级。Desktop 不接受 provider/model 参数。
/// 可用性只看正式 ModelList 的同 provider、同 id、enabled 与 text，不看静态 preset。
const PROBE_SMOKE_CANDIDATES: &[(&str, &str)] = &[("glm-coding", "glm-5.3"), ("xai", "grok-4.7")];

fn pick_probe_smoke_model(
    models: &[projection::ModelEntry],
) -> Result<&projection::ModelEntry, String> {
    let mut rejected = Vec::with_capacity(PROBE_SMOKE_CANDIDATES.len());
    for &(provider, id) in PROBE_SMOKE_CANDIDATES {
        let Some(model) = models
            .iter()
            .find(|model| model.provider_id == provider && model.id == id)
        else {
            rejected.push(format!("{provider}/{id}=missing"));
            continue;
        };
        if !model.enabled {
            rejected.push(format!("{provider}/{id}=disabled"));
            continue;
        }
        if !model.text {
            rejected.push(format!("{provider}/{id}=text_false"));
            continue;
        }
        return Ok(model);
    }
    Err(format!(
        "host ModelList has no enabled text model among authorized pairs ({})",
        rejected.join(", ")
    ))
}

fn probe_run_state<'a>(
    event: &'a ControllerEvent,
    projection: &DesktopProjection,
    tracked: &str,
) -> Option<&'a RunState> {
    let ControllerEvent::Event(envelope) = event else {
        return None;
    };
    let EventStream::Session(session_id) = &envelope.stream else {
        return None;
    };
    if projection.active_session_id.as_deref() != Some(session_id.as_str()) {
        return None;
    }
    match &envelope.payload {
        AppEvent::RunChanged { run_id, state } if run_id.as_str() == tracked => Some(state),
        _ => None,
    }
}

fn probe_approval_run<'a>(
    event: &'a ControllerEvent,
    projection: &DesktopProjection,
) -> Option<&'a str> {
    let ControllerEvent::Event(envelope) = event else {
        return None;
    };
    let EventStream::Session(session_id) = &envelope.stream else {
        return None;
    };
    if projection.active_session_id.as_deref() != Some(session_id.as_str()) {
        return None;
    }
    match &envelope.payload {
        AppEvent::ToolApprovalRequired { run_id, .. } => Some(run_id.as_str()),
        _ => None,
    }
}

fn assistant_text_for_run(projection: &DesktopProjection, run_id: &str) -> Option<String> {
    projection.timeline.iter().rev().find_map(|entry| {
        if entry.run_id.as_deref() != Some(run_id) {
            return None;
        }
        match &entry.kind {
            TimelineEntryKind::AssistantMessage { text } if !text.is_empty() => Some(text.clone()),
            _ => None,
        }
    })
}

fn turn_outcome(
    state: &RunState,
    projection: &DesktopProjection,
    run_id: &str,
) -> Option<Result<String, String>> {
    match state {
        RunState::Completed => Some(Ok(
            assistant_text_for_run(projection, run_id).unwrap_or_else(|| "completed".into())
        )),
        RunState::Failed | RunState::Interrupted | RunState::Cancelled => {
            Some(Err(format!("run ended as {state:?}")))
        }
        _ => None,
    }
}

enum ProjectedRun {
    Open,
    Completed,
    Rejected(String),
}

fn latest_projected_run_kind<'a>(
    projection: &'a DesktopProjection,
    run_id: &str,
) -> Option<&'a str> {
    projection.timeline.iter().rev().find_map(|entry| {
        if entry.run_id.as_deref() != Some(run_id) {
            return None;
        }
        let TimelineEntryKind::RunState(label) = &entry.kind else {
            return None;
        };
        let rest = label.strip_prefix("run ")?;
        Some(rest.split(" · ").next().unwrap_or(rest).trim())
    })
}

fn projected_run(projection: &DesktopProjection, run_id: &str) -> Option<ProjectedRun> {
    Some(match latest_projected_run_kind(projection, run_id)? {
        "completed" => ProjectedRun::Completed,
        "failed" => ProjectedRun::Rejected("run ended as Failed".into()),
        "interrupted" => ProjectedRun::Rejected("run ended as Interrupted".into()),
        "cancelled" => ProjectedRun::Rejected("run ended as Cancelled".into()),
        "started"
        | "preparing_context"
        | "waiting_for_provider"
        | "streaming_response"
        | "collecting_tool_calls"
        | "waiting_for_approval"
        | "executing_tools"
        | "appending_tool_results" => ProjectedRun::Open,
        other => ProjectedRun::Rejected(format!("run terminal state unknown: {other}")),
    })
}

fn projected_turn_outcome(
    projection: &DesktopProjection,
    run_id: &str,
) -> Option<Result<String, String>> {
    match projected_run(projection, run_id)? {
        ProjectedRun::Open => None,
        ProjectedRun::Completed => Some(Ok(
            assistant_text_for_run(projection, run_id).unwrap_or_else(|| "completed".into())
        )),
        ProjectedRun::Rejected(reason) => Some(Err(reason)),
    }
}

fn projected_approval_outcome(
    projection: &DesktopProjection,
    run_id: &str,
) -> Option<Result<String, String>> {
    match projected_run(projection, run_id)? {
        ProjectedRun::Open => None,
        ProjectedRun::Completed => Some(Ok("turn".into())),
        ProjectedRun::Rejected(reason) => Some(Err(reason)),
    }
}

fn bind_tracked_run(
    projection: &mut DesktopProjection,
    session_id: &str,
    run_id: &str,
    tracked: &mut Option<String>,
) -> bool {
    if tracked.is_some() || projection.active_session_id.as_deref() != Some(session_id) {
        return false;
    }
    projection.active_run_id = Some(run_id.to_string());
    *tracked = Some(run_id.to_string());
    true
}

fn cancel_outcome(state: &RunState) -> Option<Result<String, String>> {
    match state {
        RunState::Cancelled => Some(Ok("cancelled".into())),
        RunState::Completed | RunState::Failed | RunState::Interrupted => {
            Some(Err(format!("run ended as {state:?}")))
        }
        _ => None,
    }
}

fn latest_run_state_label<'a>(projection: &'a DesktopProjection, run_id: &str) -> Option<&'a str> {
    projection.timeline.iter().rev().find_map(|entry| {
        if entry.run_id.as_deref() != Some(run_id) {
            return None;
        }
        match &entry.kind {
            TimelineEntryKind::RunState(state) => Some(state.as_str()),
            _ => None,
        }
    })
}

fn run_label_kind(label: &str) -> &str {
    let body = label.strip_prefix("run ").unwrap_or(label);
    body.split(" · ").next().unwrap_or(body).trim()
}

fn has_terminal_run_label(projection: &DesktopProjection, run_id: &str) -> bool {
    latest_run_state_label(projection, run_id).is_some_and(|label| {
        matches!(
            run_label_kind(label),
            "completed" | "cancelled" | "failed" | "interrupted"
        )
    })
}

fn classify_disconnect_run(
    projection: &DesktopProjection,
    run_id: &str,
) -> Result<&'static str, String> {
    let Some(label) = latest_run_state_label(projection, run_id) else {
        return Err(format!("run {run_id} has no persisted run state"));
    };
    match run_label_kind(label) {
        "completed" => Ok("completed"),
        "cancelled" => Err("disconnect cancelled the in-flight run".into()),
        "failed" => Err(format!("disconnect run failed: {label}")),
        "interrupted" => Err(format!("disconnect run interrupted: {label}")),
        "started"
        | "preparing_context"
        | "waiting_for_provider"
        | "streaming_response"
        | "collecting_tool_calls"
        | "waiting_for_approval"
        | "executing_tools"
        | "appending_tool_results" => Err(format!("run {run_id} has no terminal state ({label})")),
        other => Err(format!("disconnect run terminal state unknown: {other}")),
    }
}

async fn wait_for_session(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
) -> Result<String, String> {
    wait_event(
        events,
        projection,
        Duration::from_secs(15),
        |event, projection| match event {
            ControllerEvent::Snapshot(snapshot) => {
                projection.merge_snapshot(snapshot);
                None
            }
            ControllerEvent::SessionCreated { session_id } => Some(Ok(session_id.clone())),
            ControllerEvent::OperationFailed { action, reason } => {
                Some(Err(format!("{action}: {reason}")))
            }
            ControllerEvent::SessionOpenFailed { reason, .. } => {
                Some(Err(format!("open session: {reason}")))
            }
            _ => None,
        },
    )
    .await
}

async fn wait_for_run_id(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
) -> Result<String, String> {
    wait_event(
        events,
        projection,
        timeout,
        |event, projection| match event {
            ControllerEvent::MessageSent { run_id, .. } => {
                projection.active_run_id = Some(run_id.clone());
                Some(Ok(run_id.clone()))
            }
            ControllerEvent::Event(envelope) => {
                projection.apply_event(envelope);
                None
            }
            ControllerEvent::OperationFailed { action, reason } => {
                Some(Err(format!("{action}: {reason}")))
            }
            _ => None,
        },
    )
    .await
}

async fn wait_for_turn(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
    known_run: Option<&str>,
) -> Result<String, String> {
    let mut tracked = known_run.map(str::to_string);
    if let Some(run_id) = tracked.clone() {
        if let Some(outcome) = projected_turn_outcome(projection, &run_id) {
            return outcome;
        }
    }
    wait_event(
        events,
        projection,
        timeout,
        |event, projection| match event {
            ControllerEvent::MessageSent {
                session_id, run_id, ..
            } => {
                if !bind_tracked_run(projection, session_id, run_id, &mut tracked) {
                    return None;
                }
                projected_turn_outcome(projection, run_id)
            }
            ControllerEvent::Event(envelope) => {
                projection.apply_event(envelope);
                let run_id = tracked.as_deref()?;
                let state = probe_run_state(event, projection, run_id)?;
                turn_outcome(state, projection, run_id)
            }
            ControllerEvent::OperationFailed { action, reason } => {
                Some(Err(format!("{action}: {reason}")))
            }
            _ => None,
        },
    )
    .await
}

async fn wait_for_approval_or_turn(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
) -> Result<bool, String> {
    let mut tracked = None;
    let mut early_approval = None;
    wait_event(
        events,
        projection,
        timeout,
        |event, projection| match event {
            ControllerEvent::MessageSent {
                session_id, run_id, ..
            } => {
                if !bind_tracked_run(projection, session_id, run_id, &mut tracked) {
                    return None;
                }
                if let Some(outcome) = projected_approval_outcome(projection, run_id) {
                    return Some(outcome);
                }
                if early_approval.as_deref() == Some(run_id.as_str()) {
                    return Some(Ok("approval".into()));
                }
                None
            }
            ControllerEvent::Event(envelope) => {
                projection.apply_event(envelope);
                if let Some(approval_run) = probe_approval_run(event, projection) {
                    if tracked.as_deref() == Some(approval_run) {
                        return Some(Ok("approval".into()));
                    }
                    if tracked.is_none() {
                        early_approval = Some(approval_run.to_string());
                    }
                }
                let run_id = tracked.as_deref()?;
                probe_run_state(event, projection, run_id).and_then(|state| match state {
                    RunState::Completed => Some(Ok("turn".into())),
                    RunState::Failed | RunState::Interrupted | RunState::Cancelled => {
                        Some(Err(format!("run ended as {state:?}")))
                    }
                    _ => None,
                })
            }
            ControllerEvent::OperationFailed { action, reason } => {
                Some(Err(format!("{action}: {reason}")))
            }
            _ => None,
        },
    )
    .await
    .map(|value| value == "approval")
}

async fn wait_for_timeline(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
) -> Result<usize, String> {
    wait_event(
        events,
        projection,
        timeout,
        |event, projection| match event {
            ControllerEvent::TimelineLoaded { page, .. } => {
                projection.apply_timeline_page(page);
                if page.complete {
                    Some(Ok(projection.timeline.len().to_string()))
                } else {
                    None
                }
            }
            ControllerEvent::Event(envelope) => {
                projection.apply_event(envelope);
                None
            }
            ControllerEvent::OperationFailed { action, reason } => {
                Some(Err(format!("{action}: {reason}")))
            }
            ControllerEvent::SessionOpenFailed { reason, .. } => {
                Some(Err(format!("open session: {reason}")))
            }
            _ => None,
        },
    )
    .await
    .and_then(|value| {
        value
            .parse()
            .map_err(|_| format!("bad timeline count {value}"))
    })
}

async fn wait_for_cancel(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
    run_id: &str,
) -> Result<(), String> {
    wait_event(
        events,
        projection,
        timeout,
        |event, projection| match event {
            ControllerEvent::Event(envelope) => {
                projection.apply_event(envelope);
                let state = probe_run_state(event, projection, run_id)?;
                cancel_outcome(state)
            }
            ControllerEvent::OperationFailed { action, reason } => {
                Some(Err(format!("{action}: {reason}")))
            }
            _ => None,
        },
    )
    .await
    .map(|_| ())
}

async fn wait_event(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
    mut on_event: impl FnMut(&ControllerEvent, &mut DesktopProjection) -> Option<Result<String, String>>,
) -> Result<String, String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err("timed out waiting for host event".into());
        }
        let event = match tokio::time::timeout(remaining, events.recv()).await {
            Ok(Ok(event)) => event,
            Ok(Err(_)) => return Err("event channel closed".into()),
            Err(_) => return Err("timed out waiting for host event".into()),
        };
        if let Some(value) = on_event(&event, projection) {
            return value;
        }
    }
}

/// R2 Wave B：窗口最小尺寸钉 1080×720——设计响应式底线（design/README
/// §2：1080 宽下 rail 收敛 240 + Inspector 折叠后 Workspace ≥560）；再窄
/// 将击穿三栏合同，故由窗口系统直接限制。
const WINDOW_MIN_SIZE: gpui::Size<gpui::Pixels> = size(px(1080.), px(720.));

fn run_app(socket: PathBuf, barrier_dir: Option<PathBuf>) {
    let platform = Arc::new(platform::Platform::new());
    Application::new()
        .with_assets(ui::Assets)
        .run(move |cx: &mut App| {
            ui::install_keybindings(cx);
            ui::theme::install(cx);
            let bounds = Bounds::centered(None, size(px(1440.), px(1024.)), cx);
            let view_platform = Arc::clone(&platform);
            let view_socket = socket.clone();
            let view_barrier_dir = barrier_dir.clone();
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        window_min_size: Some(WINDOW_MIN_SIZE),
                        // F-01：macOS 透明 titlebar。gpui 0.2.2 在 appears_transparent
                        // 时启用 NSFullSizeContentView + 隐藏原生标题（platform/mac/
                        // window.rs），内容视口贯通全窗、无白带；traffic lights 保持
                        // 默认位置悬浮于 TaskRail 顶部，由 rail 内 ≥36px 安全区让位。
                        titlebar: Some(TitlebarOptions {
                            title: Some("Pawork".into()),
                            appears_transparent: true,
                            traffic_light_position: None,
                        }),
                        ..Default::default()
                    },
                    move |window, cx| {
                        cx.new(|cx| {
                            let mut view =
                                ui::AppView::new(view_platform, view_socket, view_barrier_dir, cx);
                            view.restore_appearance(window, cx);
                            view
                        })
                    },
                )
                .expect("open pawork-desktop window");
            let accessibility_app = cx.to_async();
            let accessibility_window = window;
            window
                .update(cx, move |view, window, cx| {
                    let callback_app = accessibility_app.clone();
                    view.install_files_close_guard(window, cx);
                    if let Err(reason) = view.install_accessibility(
                        window,
                        move |request| {
                            let mut app = callback_app.clone();
                            let executor = app.foreground_executor().clone();
                            executor
                                .spawn(async move {
                                    let _ = accessibility_window.update(
                                        &mut app,
                                        |view, window, cx| {
                                            view.handle_accessibility_request(request, window, cx);
                                        },
                                    );
                                })
                                .detach();
                        },
                        cx,
                    ) {
                        eprintln!("pawork-desktop accessibility install failed: {reason}");
                    }
                    window.focus(&view.composer_focus_handle(cx));
                    cx.activate(true);
                })
                .expect("focus composer");
        });
}

#[cfg(test)]
mod probe_run_terminal_tests {
    use super::{wait_for_approval_or_turn, wait_for_cancel, wait_for_turn, DesktopProjection};
    use crate::controller::ControllerEvent;
    use pawork_client::AppEventEnvelope;
    use std::time::Duration;

    fn fresh_projection() -> DesktopProjection {
        let mut projection = DesktopProjection::default();
        projection.select_session("session-1");
        projection
    }

    fn envelope(seq: u64, payload: serde_json::Value) -> ControllerEvent {
        envelope_in("session-1", seq, payload)
    }

    fn envelope_in(session: &str, seq: u64, payload: serde_json::Value) -> ControllerEvent {
        let raw = serde_json::json!({
            "api_version": {"major": 1, "minor": 0},
            "instance_id": "instance-1",
            "event_id": format!("event-{seq}"),
            "global_sequence": seq,
            "stream": {"type": "session", "id": session},
            "stream_sequence": seq,
            "timestamp": 1_700_000_000_000u64,
            "source": {"type": "core"},
            "payload": payload,
        });
        let envelope: AppEventEnvelope = serde_json::from_value(raw).expect("event envelope");
        ControllerEvent::Event(envelope)
    }

    fn message(session: &str, run: &str) -> ControllerEvent {
        ControllerEvent::MessageSent {
            session_id: session.into(),
            run_id: run.into(),
            text: "hi".into(),
        }
    }

    fn approval(seq: u64, run_id: &str) -> ControllerEvent {
        envelope(
            seq,
            serde_json::json!({
                "type": "tool_approval_required",
                "data": {
                    "run_id": run_id,
                    "tool_call_id": "call-1",
                    "reason": "write file",
                }
            }),
        )
    }

    fn assistant(seq: u64, run_id: &str, delta: &str) -> ControllerEvent {
        envelope(
            seq,
            serde_json::json!({
                "type": "assistant_delta",
                "data": {
                    "run_id": run_id,
                    "message_id": format!("msg-{run_id}"),
                    "delta": delta,
                }
            }),
        )
    }

    fn run_changed(seq: u64, run_id: &str, state: &str) -> ControllerEvent {
        envelope(
            seq,
            serde_json::json!({
                "type": "run_changed",
                "data": {"run_id": run_id, "state": state}
            }),
        )
    }

    fn seed_old_reply(projection: &mut DesktopProjection) {
        for event in [
            assistant(1, "run-old", "old-reply"),
            run_changed(2, "run-old", "completed"),
        ] {
            if let ControllerEvent::Event(envelope) = event {
                projection.apply_event(&envelope);
            }
        }
    }

    #[tokio::test]
    async fn probe_run_terminal_completed_turn_uses_current_text() {
        let (sender, events) = smol::channel::unbounded();
        let mut projection = fresh_projection();
        seed_old_reply(&mut projection);
        sender
            .try_send(ControllerEvent::MessageSent {
                session_id: "session-1".into(),
                run_id: "run-new".into(),
                text: "hi".into(),
            })
            .expect("queue message");
        sender
            .try_send(assistant(3, "run-new", "pong"))
            .expect("queue delta");
        sender
            .try_send(run_changed(4, "run-new", "completed"))
            .expect("queue completed");
        let text = wait_for_turn(&events, &mut projection, Duration::from_secs(2), None)
            .await
            .expect("current run completed");
        assert_eq!(text, "pong");

        let (sender, events) = smol::channel::unbounded();
        let mut projection = fresh_projection();
        seed_old_reply(&mut projection);
        sender
            .try_send(message("session-1", "run-new"))
            .expect("queue message");
        sender
            .try_send(approval(3, "run-new"))
            .expect("queue approval");
        assert!(
            wait_for_approval_or_turn(&events, &mut projection, Duration::from_secs(2))
                .await
                .expect("approval card")
        );
        let approved_run = projection
            .pending_approval
            .as_ref()
            .expect("approval card run")
            .run_id
            .clone();
        assert_eq!(approved_run, "run-new");
        sender
            .try_send(assistant(4, "run-new", "pong"))
            .expect("queue delta");
        sender
            .try_send(run_changed(5, "run-new", "completed"))
            .expect("queue completed");
        let text = wait_for_turn(
            &events,
            &mut projection,
            Duration::from_secs(2),
            Some(approved_run.as_str()),
        )
        .await
        .expect("approved run completed without a second receipt");
        assert_eq!(text, "pong");

        let (sender, events) = smol::channel::unbounded();
        let mut projection = fresh_projection();
        seed_old_reply(&mut projection);
        sender
            .try_send(assistant(3, "run-new", "pong"))
            .expect("queue early delta");
        sender
            .try_send(run_changed(4, "run-new", "completed"))
            .expect("queue early completed");
        sender
            .try_send(message("session-1", "run-new"))
            .expect("queue late receipt");
        let text = wait_for_turn(&events, &mut projection, Duration::from_secs(2), None)
            .await
            .expect("terminal state already projected");
        assert_eq!(text, "pong");
    }

    #[tokio::test]
    async fn probe_run_terminal_rejects_false_success() {
        let (sender, events) = smol::channel::unbounded();
        let mut projection = fresh_projection();
        seed_old_reply(&mut projection);
        sender
            .try_send(ControllerEvent::MessageSent {
                session_id: "session-1".into(),
                run_id: "run-new".into(),
                text: "hi".into(),
            })
            .expect("queue message");
        sender
            .try_send(run_changed(3, "run-new", "failed"))
            .expect("queue failed");
        let error = wait_for_turn(&events, &mut projection, Duration::from_secs(2), None)
            .await
            .expect_err("failed run is not complete");
        assert!(error.contains("Failed"), "{error}");
        assert!(!error.contains("old-reply"), "{error}");

        let (sender, events) = smol::channel::unbounded();
        let mut projection = fresh_projection();
        projection.active_run_id = Some("run-new".into());
        sender
            .try_send(run_changed(1, "run-new", "streaming_response"))
            .expect("queue streaming");
        sender
            .try_send(run_changed(2, "run-new", "completed"))
            .expect("queue completed");
        let error = wait_for_cancel(&events, &mut projection, Duration::from_secs(2), "run-new")
            .await
            .expect_err("completed run is not cancelled");
        assert!(error.contains("Completed"), "{error}");

        let (sender, events) = smol::channel::unbounded();
        let mut projection = fresh_projection();
        sender
            .try_send(ControllerEvent::MessageSent {
                session_id: "session-1".into(),
                run_id: "run-new".into(),
                text: "write".into(),
            })
            .expect("queue message");
        sender
            .try_send(run_changed(1, "run-new", "failed"))
            .expect("queue failed");
        let error = wait_for_approval_or_turn(&events, &mut projection, Duration::from_secs(2))
            .await
            .expect_err("failed run is not a turn without approval");
        assert!(error.contains("Failed"), "{error}");

        let (sender, events) = smol::channel::unbounded();
        let mut projection = fresh_projection();
        seed_old_reply(&mut projection);
        sender
            .try_send(run_changed(3, "run-new", "failed"))
            .expect("queue early failed");
        sender
            .try_send(message("session-1", "run-new"))
            .expect("queue late receipt");
        let error = wait_for_turn(&events, &mut projection, Duration::from_secs(2), None)
            .await
            .expect_err("early failure is not success");
        assert!(error.contains("Failed"), "{error}");
        assert!(!error.contains("old-reply"), "{error}");

        let (sender, events) = smol::channel::unbounded();
        let mut projection = fresh_projection();
        seed_old_reply(&mut projection);
        sender
            .try_send(envelope_in(
                "session-other",
                3,
                serde_json::json!({
                    "type": "run_changed",
                    "data": {"run_id": "run-new", "state": "completed"}
                }),
            ))
            .expect("queue foreign completed");
        sender
            .try_send(message("session-other", "run-new"))
            .expect("queue foreign receipt");
        sender
            .try_send(message("session-1", "run-new"))
            .expect("queue current receipt");
        sender
            .try_send(run_changed(4, "run-other", "completed"))
            .expect("queue other run");
        sender
            .try_send(run_changed(5, "run-new", "failed"))
            .expect("queue current failure");
        let error = wait_for_turn(&events, &mut projection, Duration::from_secs(2), None)
            .await
            .expect_err("other session or run is not this turn");
        assert!(error.contains("Failed"), "{error}");
        assert!(!error.contains("old-reply"), "{error}");

        let (sender, events) = smol::channel::unbounded();
        let mut projection = fresh_projection();
        sender
            .try_send(run_changed(1, "run-new", "cancelled"))
            .expect("queue early cancelled");
        sender
            .try_send(message("session-1", "run-new"))
            .expect("queue late receipt");
        let error = wait_for_approval_or_turn(&events, &mut projection, Duration::from_secs(2))
            .await
            .expect_err("early cancellation is not approval");
        assert!(error.contains("Cancelled"), "{error}");
    }
}

#[cfg(test)]
mod probe_smoke_model_tests {
    use super::pick_probe_smoke_model;
    use crate::projection::ModelEntry;

    fn entry(provider: &str, id: &str, enabled: bool, text: bool) -> ModelEntry {
        ModelEntry {
            provider_id: provider.into(),
            id: id.into(),
            display_name: id.into(),
            enabled,
            text,
            ..ModelEntry::default()
        }
    }

    #[test]
    fn probe_smoke_model_picks_first_enabled_text_pair() {
        let models = vec![
            entry("opencode-go", "glm-5.3-flash", true, true),
            entry("opencode-go", "glm-5.3", true, true),
            entry("glm-coding", "glm-5.3", true, true),
            entry("xai", "grok-4.7", true, true),
        ];
        let picked = pick_probe_smoke_model(&models).expect("enabled text glm-5.3");
        assert_eq!(picked.provider_id, "glm-coding");
        assert_eq!(picked.id, "glm-5.3");
    }

    #[test]
    fn probe_smoke_model_rejects_disabled_non_text_and_other_ids() {
        let models = vec![
            entry("glm-coding", "glm-5.3", false, true),
            entry("xai", "grok-4.7", true, false),
            entry("xai", "grok-4.6", true, true),
            entry("glm-coding", "glm-5.3-flash", true, true),
        ];
        let error = pick_probe_smoke_model(&models).expect_err("no authorized pair");
        assert!(error.contains("glm-coding/glm-5.3=disabled"), "{error}");
        assert!(error.contains("xai/grok-4.7=text_false"), "{error}");
    }
}
