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

use gpui::{App, Application, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};

use crate::controller::ControllerEvent;
use crate::projection::{DesktopProjection, ResumeApply, TimelineEntryKind};

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(usage) => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    };
    let socket = args.socket.unwrap_or_else(platform::default_socket_path);
    if args.probe_smoke {
        std::process::exit(run_probe_smoke(socket));
    }
    if args.probe {
        std::process::exit(run_probe(socket));
    }
    run_app(socket);
}

struct Args {
    socket: Option<PathBuf>,
    probe: bool,
    probe_smoke: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        socket: None,
        probe: false,
        probe_smoke: false,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--socket" => {
                let Some(path) = iter.next() else {
                    return Err(
                        "usage: pawork-desktop [--socket <path>] [--probe|--probe-smoke]".into(),
                    );
                };
                args.socket = Some(PathBuf::from(path));
            }
            "--probe" => args.probe = true,
            "--probe-smoke" => args.probe_smoke = true,
            other => {
                return Err(format!(
                    "unknown argument {other}; usage: pawork-desktop [--socket <path>] [--probe|--probe-smoke]"
                ))
            }
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
        let models = controller.fetch_models().await.unwrap_or_default();
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

/// 波 C 真实冒烟：同一条 controller 路径走流式回合、切模型、取消、审批。
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
    controller.create_session(workspace);
    let session_id = wait_for_session(&events, &mut projection).await?;
    projection.select_session(&session_id);

    let first = pick_model(&models, "glm-coding", "glm-4.7")
        .or_else(|| pick_model(&models, "deepseek", "deepseek-v4-flash"))
        .cloned()
        .ok_or_else(|| {
            "host ModelList is missing glm-4.7 and deepseek-v4-flash".to_string()
        })?;
    projection.set_pending_model(first.provider_id.clone(), first.id.clone());
    controller.send_message(
        session_id.clone(),
        "Reply with exactly one word: pong".into(),
        Some(first.id.clone()),
    );
    let first_turn = wait_for_turn(&events, &mut projection, Duration::from_secs(90)).await?;

    let second = pick_other_model(&models, &first).cloned();
    let second_id = second.as_ref().map(|model| model.id.clone());
    let second_turn = if let Some(model) = second {
        projection.set_pending_model(model.provider_id.clone(), model.id.clone());
        controller.send_message(
            session_id.clone(),
            "Reply with exactly one word: switched".into(),
            Some(model.id.clone()),
        );
        Some(wait_for_turn(&events, &mut projection, Duration::from_secs(90)).await?)
    } else {
        None
    };

    controller.send_message(
        session_id.clone(),
        "Write target/s7-wave-d-smoke.txt containing exactly hello-s7d. Use the write tool."
            .into(),
        Some(first.id.clone()),
    );
    let approval = wait_for_approval_or_turn(&events, &mut projection, Duration::from_secs(90)).await?;
    if approval {
        let pending = projection
            .pending_approval
            .as_ref()
            .ok_or_else(|| "approval card missing after ToolApprovalRequired".to_string())?;
        controller.approve(
            pending.run_id.clone(),
            pending.tool_call_id.clone(),
            "approve_once",
        );
        wait_for_turn(&events, &mut projection, Duration::from_secs(90)).await?;
    }

    controller.send_message(
        session_id.clone(),
        "Count slowly from 1 to 80 in digits only.".into(),
        Some(first.id.clone()),
    );
    let run_id = wait_for_run_id(&events, &mut projection, Duration::from_secs(30)).await?;
    controller.cancel_run(run_id);
    wait_for_cancel(&events, &mut projection, Duration::from_secs(30)).await?;

    let assistant_turns = projection
        .timeline
        .iter()
        .filter(|entry| matches!(entry.kind, TimelineEntryKind::AssistantMessage { .. }))
        .count();

    controller.disconnect().await;
    let connected = controller.connect(socket.clone()).await?;
    events = connected.events;
    apply_probe_resume(&mut projection, &connected, &models, &session_id, &controller);
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
        Some(first.id.clone()),
    );
    let live_run = wait_for_run_id(&events, &mut projection, Duration::from_secs(30)).await?;
    controller.disconnect().await;
    let connected = controller.connect(socket).await?;
    events = connected.events;
    apply_probe_resume(&mut projection, &connected, &models, &session_id, &controller);
    if projection.active_session_id.as_deref() != Some(session_id.as_str()) {
        projection.select_session(&session_id);
    }
    let disconnect_survive = if projection.active_run_id.as_deref() == Some(live_run.as_str())
        || projection
            .active_runs
            .iter()
            .any(|run| run.run_id == live_run)
    {
        controller.cancel_run(live_run);
        wait_for_cancel(&events, &mut projection, Duration::from_secs(30)).await?;
        "running"
    } else {
        controller.open_session(session_id.clone());
        wait_for_timeline(&events, &mut projection, Duration::from_secs(15)).await?;
        if projection.timeline.iter().any(|entry| {
            entry.run_id.as_deref() == Some(live_run.as_str())
                && matches!(&entry.kind, TimelineEntryKind::RunState(state) if state.contains("cancelled"))
        }) {
            return Err("disconnect cancelled the in-flight run".into());
        }
        "completed"
    };

    Ok(format!(
        "probe-smoke: session={session_id} models={} first={} first_turn={first_turn} second={} second_turn={} trusted={} approval={} assistant_turns={assistant_turns} cancelled=1 persisted={persisted} disconnect_survive={disconnect_survive}",
        models.len(),
        first.id,
        second_id.as_deref().unwrap_or("-"),
        second_turn.as_deref().unwrap_or("skipped"),
        match trusted {
            Some(true) => "1",
            Some(false) => "0",
            None => "unknown",
        },
        if approval { "approved" } else { "not_requested" },
    ))
}

fn apply_probe_resume(
    projection: &mut DesktopProjection,
    connected: &controller::DesktopConnect,
    models: &[projection::ModelEntry],
    session_id: &str,
    controller: &controller::DesktopController,
) {
    projection.set_models(models.to_vec());
    match &connected.resume {
        None => {
            *projection = DesktopProjection::from_snapshot(&connected.snapshot);
            projection.set_models(models.to_vec());
            projection.select_session(session_id);
        }
        Some(outcome) => match projection.apply_resume_outcome(outcome, &connected.snapshot) {
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

fn pick_model<'a>(
    models: &'a [projection::ModelEntry],
    provider: &str,
    id: &str,
) -> Option<&'a projection::ModelEntry> {
    models
        .iter()
        .find(|model| model.provider_id == provider && model.id == id)
        .or_else(|| models.iter().find(|model| model.id == id))
}

fn pick_other_model<'a>(
    models: &'a [projection::ModelEntry],
    first: &projection::ModelEntry,
) -> Option<&'a projection::ModelEntry> {
    // 只走 ROADMAP §1.1 低消耗矩阵；目录没有 deepseek-v4-flash 就跳过，
    // 不擅自换 claude / grok / 同通道相邻档。
    pick_model(models, "deepseek", "deepseek-v4-flash")
        .or_else(|| pick_model(models, "opencode-go", "deepseek-v4-flash"))
        .filter(|model| model.id != first.id || model.provider_id != first.provider_id)
}

async fn wait_for_session(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
) -> Result<String, String> {
    wait_event(events, projection, Duration::from_secs(15), |event, projection| match event {
        ControllerEvent::Snapshot(snapshot) => {
            projection.merge_snapshot(snapshot);
            None
        }
        ControllerEvent::SessionCreated { session_id } => Some(session_id.clone()),
        ControllerEvent::OperationFailed { action, reason } => {
            Some(format!("FAIL {action}: {reason}"))
        }
        _ => None,
    })
    .await
    .and_then(|value| {
        if value.starts_with("FAIL ") {
            Err(value)
        } else {
            Ok(value)
        }
    })
}

async fn wait_for_run_id(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
) -> Result<String, String> {
    wait_event(events, projection, timeout, |event, projection| match event {
        ControllerEvent::MessageSent { run_id, .. } => {
            projection.active_run_id = Some(run_id.clone());
            Some(run_id.clone())
        }
        ControllerEvent::Event(envelope) => {
            projection.apply_event(envelope);
            projection.active_run_id.clone()
        }
        ControllerEvent::OperationFailed { action, reason } => {
            Some(format!("FAIL {action}: {reason}"))
        }
        _ => None,
    })
    .await
    .and_then(|value| {
        if value.starts_with("FAIL ") {
            Err(value)
        } else {
            Ok(value)
        }
    })
}

async fn wait_for_turn(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
) -> Result<String, String> {
    wait_event(events, projection, timeout, |event, projection| match event {
        ControllerEvent::MessageSent { run_id, .. } => {
            projection.active_run_id = Some(run_id.clone());
            None
        }
        ControllerEvent::Event(envelope) => {
            projection.apply_event(envelope);
            if projection.active_run_id.is_none() {
                let text = projection.timeline.iter().rev().find_map(|entry| match &entry.kind {
                    TimelineEntryKind::AssistantMessage { text } => Some(text.clone()),
                    TimelineEntryKind::Error(message) => Some(format!("error:{message}")),
                    _ => None,
                });
                Some(text.unwrap_or_else(|| "completed".into()))
            } else {
                None
            }
        }
        ControllerEvent::OperationFailed { action, reason } => {
            Some(format!("FAIL {action}: {reason}"))
        }
        _ => None,
    })
    .await
    .and_then(|value| {
        if value.starts_with("FAIL ") {
            Err(value)
        } else {
            Ok(value)
        }
    })
}

async fn wait_for_approval_or_turn(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
) -> Result<bool, String> {
    wait_event(events, projection, timeout, |event, projection| match event {
        ControllerEvent::MessageSent { run_id, .. } => {
            projection.active_run_id = Some(run_id.clone());
            None
        }
        ControllerEvent::Event(envelope) => {
            projection.apply_event(envelope);
            if projection.pending_approval.is_some() {
                Some("approval".into())
            } else if projection.active_run_id.is_none() {
                Some("turn".into())
            } else {
                None
            }
        }
        ControllerEvent::OperationFailed { action, reason } => {
            Some(format!("FAIL {action}: {reason}"))
        }
        _ => None,
    })
    .await
    .and_then(|value| match value.as_str() {
        "approval" => Ok(true),
        "turn" => Ok(false),
        other if other.starts_with("FAIL ") => Err(other.to_string()),
        other => Err(other.to_string()),
    })
}

async fn wait_for_timeline(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
) -> Result<usize, String> {
    wait_event(events, projection, timeout, |event, projection| match event {
        ControllerEvent::TimelineLoaded { page, .. } => {
            projection.apply_timeline_page(page);
            if page.complete {
                Some(projection.timeline.len().to_string())
            } else {
                None
            }
        }
        ControllerEvent::Event(envelope) => {
            projection.apply_event(envelope);
            None
        }
        ControllerEvent::OperationFailed { action, reason } => {
            Some(format!("FAIL {action}: {reason}"))
        }
        _ => None,
    })
    .await
    .and_then(|value| {
        if value.starts_with("FAIL ") {
            Err(value)
        } else {
            value
                .parse()
                .map_err(|_| format!("bad timeline count {value}"))
        }
    })
}

async fn wait_for_cancel(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
) -> Result<(), String> {
    wait_event(events, projection, timeout, |event, projection| match event {
        ControllerEvent::Event(envelope) => {
            projection.apply_event(envelope);
            if projection.active_run_id.is_none() {
                Some("cancelled".into())
            } else {
                None
            }
        }
        ControllerEvent::OperationFailed { action, reason } => {
            Some(format!("FAIL {action}: {reason}"))
        }
        _ => None,
    })
    .await
    .and_then(|value| {
        if value.starts_with("FAIL ") {
            Err(value)
        } else {
            Ok(())
        }
    })
}

async fn wait_event(
    events: &smol::channel::Receiver<ControllerEvent>,
    projection: &mut DesktopProjection,
    timeout: Duration,
    mut on_event: impl FnMut(&ControllerEvent, &mut DesktopProjection) -> Option<String>,
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
            return Ok(value);
        }
    }
}

fn run_app(socket: PathBuf) {
    let platform = Arc::new(platform::Platform::new());
    Application::new().run(move |cx: &mut App| {
        ui::install_keybindings(cx);
        let bounds = Bounds::centered(None, size(px(1440.), px(1024.)), cx);
        let view_platform = Arc::clone(&platform);
        let view_socket = socket.clone();
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |_, cx| cx.new(|cx| ui::AppView::new(view_platform, view_socket, cx)),
            )
            .expect("open pawork-desktop window");
        window
            .update(cx, |view, window, cx| {
                window.focus(&view.composer_focus_handle(cx));
                cx.activate(true);
            })
            .expect("focus composer");
    });
}
