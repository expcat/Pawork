//! pawork-desktop：GPUI 最小 Agent 壳（S7 波 C）。
//!
//! 四层结构：ui（GPUI 渲染与交互）/ projection（纯状态机）/
//! controller（只调 pawork-client）/ platform（socket 发现 + tokio 宿主）。
//! 范围：Sessions + Timeline + Composer 发送/取消、时间线审批、模型切换。

mod controller;
mod platform;
mod projection;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Application, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(usage) => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    };
    let socket = args.socket.unwrap_or_else(platform::default_socket_path);
    if args.probe {
        std::process::exit(run_probe(socket));
    }
    run_app(socket);
}

struct Args {
    socket: Option<PathBuf>,
    probe: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        socket: None,
        probe: false,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--socket" => {
                let Some(path) = iter.next() else {
                    return Err("usage: pawork-desktop [--socket <path>] [--probe]".into());
                };
                args.socket = Some(PathBuf::from(path));
            }
            "--probe" => args.probe = true,
            other => {
                return Err(format!(
                    "unknown argument {other}; usage: pawork-desktop [--socket <path>] [--probe]"
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
    match platform.block_on(controller.connect(socket)) {
        Ok((snapshot, _events)) => {
            let projection = projection::DesktopProjection::from_snapshot(&snapshot);
            println!(
                "connected: instance={}, sessions={}",
                snapshot.instance_id.as_str(),
                projection.sessions.len()
            );
            0
        }
        Err(reason) => {
            eprintln!("pawork-desktop probe failed: {reason}");
            1
        }
    }
}

fn run_app(socket: PathBuf) {
    let platform = Arc::new(platform::Platform::new());
    Application::new().run(move |cx: &mut App| {
        ui::install_keybindings(cx);
        let bounds = Bounds::centered(None, size(px(1080.), px(720.)), cx);
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
