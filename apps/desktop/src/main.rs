#![allow(unexpected_cfgs)] // objc 0.2 macros probe the historical cargo-clippy feature.

//! pawork-desktop：GPUI Agent 壳（S10 Replay / Fork / Terminal）。
//!
//! 四层结构：ui（GPUI 渲染与交互）/ projection（纯状态机）/
//! controller（只调 pawork-client）/ platform（socket 发现 + tokio 宿主）。

mod controller;
mod platform;
mod probe;
mod projection;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    prelude::*, px, size, App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};

use probe::{run_probe, run_probe_smoke};

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

const USAGE: &str =
    "usage: pawork-desktop [--socket <path>] [--instance <name>] [--probe|--probe-smoke]";

fn parse_args() -> Result<Args, String> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from<I, S>(args: I) -> Result<Args, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut parsed = Args {
        socket: None,
        instance: None,
        probe: false,
        probe_smoke: false,
    };
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_ref() {
            "--socket" => {
                let Some(path) = iter.next() else {
                    return Err(USAGE.into());
                };
                parsed.socket = Some(PathBuf::from(path.as_ref()));
            }
            "--instance" => {
                let Some(name) = iter.next() else {
                    return Err(USAGE.into());
                };
                let name = name.as_ref();
                if let Some(detail) = instance_name_error(name) {
                    return Err(format!("{detail}; {USAGE}"));
                }
                parsed.instance = Some(name.trim().to_string());
            }
            "--probe" => parsed.probe = true,
            "--probe-smoke" => parsed.probe_smoke = true,
            other => return Err(format!("unknown argument {other}; {USAGE}")),
        }
    }
    Ok(parsed)
}

/// 与 `pawork-app` 的 `normalize_instance` 同一允许集。desktop 不依赖 app。
fn instance_name_error(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("instance name must not be empty".into());
    }
    let allowed = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !allowed || name.contains("..") {
        return Some(format!("invalid instance name `{name}`"));
    }
    None
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
mod instance_args_tests {
    use super::{parse_args_from, USAGE};
    use crate::platform::{socket_path_for_instance, token_path_for_socket};

    #[test]
    fn instance_names_reject_path_escape_and_blank() {
        for name in ["../x", "../../../x", "a/b", "..", "", "   ", "a b", "a\\b"] {
            let Err(error) = parse_args_from(["--instance", name]) else {
                panic!("accepted {name}");
            };
            assert!(error.ends_with(&format!("; {USAGE}")), "{name}: {error}");
            if name.trim().is_empty() {
                assert!(
                    error.starts_with("instance name must not be empty"),
                    "{name}: {error}"
                );
            } else {
                assert!(
                    error.starts_with(&format!("invalid instance name `{}`", name.trim())),
                    "{name}: {error}"
                );
            }
        }
        let Err(missing) = parse_args_from(["--instance"]) else {
            panic!("missing instance value accepted");
        };
        assert_eq!(missing, USAGE);
        let Err(unknown) = parse_args_from(["--nope"]) else {
            panic!("unknown argument accepted");
        };
        assert!(
            unknown.starts_with("unknown argument --nope; "),
            "{unknown}"
        );
        assert!(unknown.ends_with(USAGE), "{unknown}");
    }

    #[test]
    fn instance_names_keep_socket_and_token_paths() {
        for name in ["default", "dev-1", "a.b_c"] {
            let args = parse_args_from(["--instance", name]).expect(name);
            assert_eq!(args.instance.as_deref(), Some(name));
            let socket = socket_path_for_instance(args.instance.as_deref());
            let expected_file = if name == "default" {
                "pawork-gui.sock".to_string()
            } else {
                format!("pawork-gui-{name}.sock")
            };
            let expected_token = if name == "default" {
                "gui.token".to_string()
            } else {
                format!("gui-{name}.token")
            };
            assert_eq!(
                socket,
                socket_path_for_instance(None).with_file_name(&expected_file),
                "{name}"
            );
            assert_eq!(
                token_path_for_socket(&socket),
                socket.parent().unwrap().join(expected_token),
                "{name}"
            );
        }
        assert_eq!(
            socket_path_for_instance(Some("default")),
            socket_path_for_instance(None)
        );
        let padded = parse_args_from(["--instance", " dev-1 "]).expect("padded name");
        assert_eq!(padded.instance.as_deref(), Some("dev-1"));
    }
}
