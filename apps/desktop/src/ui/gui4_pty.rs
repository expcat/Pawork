//! GUI4：AppView 输入订阅经 Desktop controller 到隔离 Host 的真实 PTY。
//! 不是系统 IME，也不是真窗口。

use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{ClipboardItem, Entity, EntityInputHandler, Focusable};

use super::inspector::InspectorTab;
use super::{AppView, InspectorFocusTarget};
use crate::projection::ConnectionState;

struct HostProcess {
    child: Child,
    log: PathBuf,
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pawork_binary() -> PathBuf {
    match std::env::var("PAWORK_BIN") {
        Ok(path) => PathBuf::from(path),
        Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/pawork"),
    }
}

fn wait_until(timeout: Duration, mut ready: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(
            start.elapsed() < timeout,
            "timed out before host socket was ready"
        );
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn spawn_host(data_dir: &Path, zdotdir: &Path) -> HostProcess {
    let binary = pawork_binary();
    assert!(
        binary.is_file(),
        "build target/debug/pawork before this test ({})",
        binary.display()
    );
    let instance = format!("g4{}", std::process::id());
    let log_dir = PathBuf::from("/tmp/pawork-gui4-status-empty-terminal/evidence");
    std::fs::create_dir_all(&log_dir).expect("evidence dir");
    let log = log_dir.join(format!("host-gui4-pty-{}.log", std::process::id()));
    let stderr = std::fs::File::create(&log).expect("host log");
    let child = Command::new(&binary)
        .arg("--instance")
        .arg(&instance)
        .arg("--approval-mode")
        .arg("ask-for-dangerous")
        .arg("--trust-workspaces")
        .arg("gui")
        .arg("serve")
        .current_dir(data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .env("PAWORK_DATA_DIR", data_dir)
        .env("ZDOTDIR", zdotdir)
        .env("SHELL", "/bin/zsh")
        .spawn()
        .expect("spawn pawork gui serve");
    let mut host = HostProcess { child, log };
    let socket = data_dir.join(format!("pawork-gui-{instance}.sock"));
    let token = data_dir.join(format!("gui-{instance}.token"));
    let log = host.log.clone();
    let child = &mut host.child;
    wait_until(Duration::from_secs(30), || {
        if let Some(status) = child.try_wait().expect("host status") {
            panic!("host exited {status}\n{}", log_tail(&log));
        }
        std::fs::metadata(&socket)
            .map(|meta| meta.file_type().is_socket())
            .unwrap_or(false)
            && std::fs::metadata(&token)
                .map(|meta| meta.len() > 0)
                .unwrap_or(false)
    });
    host
}

fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<&str> = text.lines().rev().take(30).collect();
    lines.reverse();
    lines.join("\n")
}

fn snapshot(view: &Entity<AppView>, cx: &mut gpui::VisualTestContext) -> String {
    view.read_with(cx, |view, _| {
        let terminals = view
            .projection
            .terminals
            .iter()
            .map(|terminal| {
                format!(
                    "{:?}@{:?}:{:?}",
                    terminal.session_id, terminal.workspace_id, terminal.last_error
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "connection={:?} tab={:?} open={} session={:?} cols={} rows={} fitted={:?} confirmed={} cwd={} pending_create={:?} pending_resize={:?} pending_close={:?} pending_write={} error={:?} output_len={} scope={:?} active_session={:?} active_ws={:?} terminals=[{}]",
            view.projection.connection,
            view.inspector_tab,
            view.inspector_open,
            view.projection.terminal.session_id,
            view.projection.terminal.columns,
            view.projection.terminal.rows,
            view.terminal_fitted_size,
            view.projection.terminal.resize_confirmed,
            view.projection.terminal.cwd,
            view.terminal_pending_create_workspace,
            view.terminal_pending_resize,
            view.terminal_pending_close,
            view.terminal_pending_write.len(),
            view.projection.terminal.last_error,
            view.projection.terminal.output.len(),
            view.scope_workspace_id,
            view.projection.active_session_id,
            view.projection.active_workspace_id(),
            terminals,
        )
    })
}

fn fail(
    view: &Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    host: &HostProcess,
    what: &str,
) -> ! {
    panic!("{what}\n{}\n{}", snapshot(view, cx), log_tail(&host.log));
}

fn wait_view(
    view: &Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    host: &HostProcess,
    timeout: Duration,
    what: &str,
    mut ready: impl FnMut(&AppView) -> bool,
) {
    let start = Instant::now();
    loop {
        let draw = view.read_with(cx, |view, _| view.pending_inspector_focus.is_some());
        if draw {
            cx.refresh().expect("draw");
        }
        cx.run_until_parked();
        if view.read_with(cx, |view, _| ready(view)) {
            return;
        }
        if start.elapsed() > timeout {
            fail(view, cx, host, what);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn draw(cx: &mut gpui::VisualTestContext) {
    cx.refresh().expect("draw");
    cx.run_until_parked();
}

fn terminal_focused(view: &Entity<AppView>, cx: &mut gpui::VisualTestContext) -> bool {
    cx.update(|window, app| {
        view.read(app)
            .terminal_input
            .read(app)
            .focus_handle(app)
            .is_focused(window)
    })
}

fn ensure_terminal_focus(
    view: &Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    host: &HostProcess,
) {
    if terminal_focused(view, cx) {
        return;
    }
    cx.update(|_window, app| {
        view.update(app, |view, cx| {
            view.pending_inspector_focus = Some(InspectorFocusTarget::SelectedTab);
            cx.notify();
        });
    });
    draw(cx);
    if !terminal_focused(view, cx) {
        fail(view, cx, host, "terminal input did not take focus");
    }
}

fn commit_text(view: &Entity<AppView>, cx: &mut gpui::VisualTestContext, text: &str) {
    cx.update(|window, app| {
        view.update(app, |view, cx| {
            view.terminal_input.update(cx, |input, cx| {
                input.replace_text_in_range(None, text, window, cx);
            });
        });
    });
}

fn press(view: &Entity<AppView>, cx: &mut gpui::VisualTestContext, host: &HostProcess, key: &str) {
    ensure_terminal_focus(view, cx, host);
    cx.simulate_keystrokes(key);
}

fn submit_line(
    view: &Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    host: &HostProcess,
    text: &str,
) {
    ensure_terminal_focus(view, cx, host);
    commit_text(view, cx, text);
    let pending = view.read_with(cx, |view, _| {
        view.projection
            .terminal
            .session_id
            .as_ref()
            .is_some_and(|id| view.terminal_pending_write.contains(id))
    });
    assert!(pending, "write pending missing\n{}", snapshot(view, cx));
    press(view, cx, host, "enter");
}

fn plain(view: &Entity<AppView>, cx: &mut gpui::VisualTestContext) -> String {
    view.read_with(cx, |view, _| {
        pawork_terminal::plain_output(&view.projection.terminal.output)
    })
}

fn raw_output(view: &Entity<AppView>, cx: &mut gpui::VisualTestContext) -> String {
    view.read_with(cx, |view, _| view.projection.terminal.output.clone())
}

fn wait_plain(
    view: &Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    host: &HostProcess,
    what: &str,
    mut ready: impl FnMut(&str) -> bool,
) {
    wait_view(view, cx, host, Duration::from_secs(15), what, |view| {
        ready(&pawork_terminal::plain_output(
            &view.projection.terminal.output,
        ))
    });
}

fn parse_pid(output: &str, marker: &str) -> Option<u32> {
    let mut found = None;
    let mut rest = output;
    while let Some(index) = rest.find(marker) {
        let after = &rest[index + marker.len()..];
        let digits: String = after.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        if let Ok(pid) = digits.parse::<u32>() {
            if pid > 1 {
                found = Some(pid);
            }
        }
        rest = &rest[index + marker.len()..];
    }
    found
}

fn process_alive(pid: u32) -> bool {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output();
    match output {
        Ok(output) => !String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        Err(_) => false,
    }
}

fn child_matching(parent: u32, name: &str) -> Option<u32> {
    let output = Command::new("/bin/ps")
        .args(["-ax", "-o", "pid=,ppid=,ucomm="])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let pid = parts.next()?.parse::<u32>().ok()?;
        let ppid = parts.next()?.parse::<u32>().ok()?;
        let command = parts.next()?;
        if ppid == parent && command == name && pid > 1 {
            return Some(pid);
        }
    }
    None
}

fn wait_child(
    parent: u32,
    name: &str,
    view: &Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    host: &HostProcess,
) -> u32 {
    let start = Instant::now();
    loop {
        cx.run_until_parked();
        if let Some(pid) = child_matching(parent, name) {
            return pid;
        }
        if start.elapsed() > Duration::from_secs(8) {
            fail(
                view,
                cx,
                host,
                &format!("{name} did not start under shell {parent}"),
            );
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn wait_repo_file(
    view: &Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    host: &HostProcess,
    path: &Path,
    expected: &str,
    what: &str,
) {
    let start = Instant::now();
    loop {
        cx.run_until_parked();
        if std::fs::read_to_string(path).ok().as_deref() == Some(expected) {
            return;
        }
        if start.elapsed() > Duration::from_secs(15) {
            fail(view, cx, host, what);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn wait_dead(
    pid: u32,
    view: &Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    host: &HostProcess,
    what: &str,
) {
    let start = Instant::now();
    while process_alive(pid) {
        if start.elapsed() > Duration::from_secs(8) {
            fail(view, cx, host, what);
        }
        std::thread::sleep(Duration::from_millis(40));
        cx.run_until_parked();
    }
}

fn settled_measured_size(view: &AppView) -> Option<(u16, u16)> {
    let size = (
        view.projection.terminal.columns,
        view.projection.terminal.rows,
    );
    (view.projection.terminal.session_id.is_some()
        && view.projection.terminal.resize_confirmed
        && view.terminal_pending_resize.is_none()
        && view.terminal_fitted_size == Some(size))
    .then_some(size)
}

fn pump_resize_debounce(cx: &mut gpui::VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(80));
    let _ = cx.executor().tick();
}

fn wait_measured_terminal(
    view: &Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    host: &HostProcess,
    what: &str,
) -> (u16, u16) {
    let start = Instant::now();
    loop {
        cx.run_until_parked();
        pump_resize_debounce(cx);
        if let Some(size) = view.read_with(cx, |view, _| settled_measured_size(view)) {
            return size;
        }
        if start.elapsed() > Duration::from_secs(15) {
            fail(view, cx, host, what);
        }
        std::thread::sleep(Duration::from_millis(40));
        draw(cx);
    }
}

/// 窗口尺寸走 simulate_resize，由正式 canvas 测量。并排宽度按壁钟动画，
/// 先等过 180ms 再去抖，确认值等于实测列乘行。
fn resize_window_to_measured(
    view: &Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    host: &HostProcess,
    width: f32,
    height: f32,
) -> (u16, u16) {
    cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
    draw(cx);
    std::thread::sleep(Duration::from_millis(200));
    draw(cx);
    cx.run_until_parked();
    wait_measured_terminal(
        view,
        cx,
        host,
        "terminal size did not follow the resized window",
    )
}

/// stty size 先打印行再打印列。标记在命令里，行列数字只在 PTY 输出里。
fn expect_pty_reports(
    view: &Entity<AppView>,
    cx: &mut gpui::VisualTestContext,
    host: &HostProcess,
    token: &str,
    columns: u16,
    rows: u16,
) {
    submit_line(
        view,
        cx,
        host,
        &format!("stty size | awk '{{print \"{token}\", $1, $2}}'"),
    );
    let needle = format!("{token} {rows} {columns}");
    wait_plain(
        view,
        cx,
        host,
        &format!("stty did not report {needle}"),
        |text| text.contains(&needle),
    );
}

/// 从产品输入路径驱动隔离 Host 上的真实 PTY。标记文本不是系统 IME。
#[gpui::test]
fn app_view_drives_real_pty_through_host(cx: &mut gpui::TestAppContext) {
    let data = tempfile::tempdir().expect("data dir");
    let zdotdir = tempfile::tempdir().expect("zdotdir");
    let repo = tempfile::tempdir().expect("repo");
    let git = Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(repo.path())
        .status()
        .expect("git init");
    assert!(git.success(), "git init failed");
    let repo_path = repo.path().canonicalize().expect("repo path");
    let host = spawn_host(data.path(), zdotdir.path());
    let socket = data
        .path()
        .join(format!("pawork-gui-g4{}.sock", std::process::id()));

    cx.executor().allow_parking();
    let platform = Arc::new(crate::platform::Platform::new());
    let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));

    wait_view(
        &view,
        cx,
        &host,
        Duration::from_secs(20),
        "desktop did not connect",
        |view| {
            matches!(
                view.projection.connection,
                ConnectionState::Connected { .. }
            )
        },
    );
    cx.update(|_window, app| {
        view.update(app, |view, _cx| {
            view.controller.open_workspace(repo_path.clone(), false);
        });
    });
    wait_view(
        &view,
        cx,
        &host,
        Duration::from_secs(20),
        "workspace did not open",
        |view| view.scope_workspace_id.is_some(),
    );

    cx.update(|window, app| {
        view.update(app, |view, cx| view.on_toggle_inspector(window, cx));
    });
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.inspector_open));

    for tab in [InspectorTab::Changes, InspectorTab::Resources] {
        cx.update(|_window, app| {
            view.update(app, |view, cx| view.select_inspector_tab(tab, cx));
        });
        draw(cx);
        assert_eq!(view.read_with(cx, |view, _| view.inspector_tab), tab);
    }

    cx.update(|_window, app| {
        view.update(app, |view, cx| {
            view.select_inspector_tab(InspectorTab::Terminal, cx);
            assert!(
                view.terminal_pending_create_workspace.is_some()
                    || view.projection.terminal.session_id.is_some(),
                "terminal create was not requested"
            );
        });
    });
    draw(cx);
    wait_view(
        &view,
        cx,
        &host,
        Duration::from_secs(20),
        "terminal was not created at the measured size",
        |view| settled_measured_size(view).is_some() && !view.projection.terminal.output.is_empty(),
    );
    let initial = view
        .read_with(cx, |view, _| settled_measured_size(view))
        .expect("measured size");
    let session = view
        .read_with(cx, |view, _| view.projection.terminal.session_id.clone())
        .expect("session");
    ensure_terminal_focus(&view, cx, &host);

    expect_pty_reports(&view, cx, &host, "GUI4SIZEA", initial.0, initial.1);

    let before = raw_output(&view, cx);
    cx.update(|window, app| {
        view.update(app, |view, cx| {
            view.terminal_input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "标记未发送", None, window, cx);
            });
        });
    });
    cx.run_until_parked();
    let (preedit, pending) = view.read_with(cx, |view, cx| {
        (
            view.terminal_input.read(cx).preedit().to_string(),
            view.terminal_pending_write.contains(&session),
        )
    });
    assert_eq!(preedit, "标记未发送");
    assert!(!pending, "marked text must not write to the pty");
    assert_eq!(raw_output(&view, cx), before);
    view.update(cx, |view, cx| {
        view.terminal_input.update(cx, |input, cx| input.clear(cx));
    });
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |view, cx| view
            .terminal_input
            .read(cx)
            .preedit()
            .is_empty()),
        "clear only removes preedit"
    );
    assert_eq!(raw_output(&view, cx), before, "clear must not write");

    submit_line(
        &view,
        cx,
        &host,
        "printf 'GUI4FILE\\n' | rev > gui4-pty-mark.txt",
    );
    let mark = repo_path.join("gui4-pty-mark.txt");
    wait_repo_file(
        &view,
        cx,
        &host,
        &mark,
        "ELIF4IUG\n",
        "repo file was not written by the pty",
    );
    submit_line(&view, cx, &host, "echo GUI4PID1:$$");
    wait_plain(&view, cx, &host, "missing shell pid", |text| {
        parse_pid(text, "GUI4PID1:").is_some()
    });
    let pid_a = parse_pid(&plain(&view, cx), "GUI4PID1:").expect("pid");
    assert!(process_alive(pid_a), "shell pid was not alive");

    commit_text(&view, cx, "cat gui4-pty-m");
    press(&view, cx, &host, "tab");
    press(&view, cx, &host, "enter");
    wait_plain(
        &view,
        cx,
        &host,
        "tab completion did not yield the file contents",
        |text| text.contains("ELIF4IUG"),
    );

    commit_text(&view, cx, "echo ARROW");
    press(&view, cx, &host, "left");
    press(&view, cx, &host, "left");
    commit_text(&view, cx, "X");
    press(&view, cx, &host, "enter");
    wait_plain(
        &view,
        cx,
        &host,
        "arrow keys did not edit the line",
        |text| text.contains("ARRXOW"),
    );

    submit_line(&view, cx, &host, "sleep 30");
    let sleep_pid = wait_child(pid_a, "sleep", &view, cx, &host);
    assert!(process_alive(sleep_pid), "sleep exited before ctrl-c");
    press(&view, cx, &host, "ctrl-c");
    submit_line(
        &view,
        cx,
        &host,
        "printf 'GUI4BACK\\n' | rev > gui4-after-int.txt",
    );
    wait_repo_file(
        &view,
        cx,
        &host,
        &repo_path.join("gui4-after-int.txt"),
        "KCAB4IUG\n",
        "ctrl-c did not return to the shell",
    );
    assert!(process_alive(pid_a), "ctrl-c killed the shell");
    wait_dead(
        sleep_pid,
        &view,
        cx,
        &host,
        "sleep still alive after ctrl-c",
    );

    cx.write_to_clipboard(ClipboardItem::new_string("中文粘贴".into()));
    commit_text(&view, cx, "printf '%s\n' '");
    press(&view, cx, &host, "cmd-v");
    commit_text(&view, cx, "'");
    press(&view, cx, &host, "enter");
    wait_plain(
        &view,
        cx,
        &host,
        "chinese paste did not reach the pty",
        |text| text.contains("中文粘贴"),
    );

    let narrow = resize_window_to_measured(&view, cx, &host, 960.0, 640.0);
    assert_ne!(narrow, initial, "narrow window did not change the pty size");
    expect_pty_reports(&view, cx, &host, "GUI4SIZEB", narrow.0, narrow.1);
    let wide = resize_window_to_measured(&view, cx, &host, 1600.0, 720.0);
    assert_ne!(wide, narrow, "wide window did not change the pty size");
    expect_pty_reports(&view, cx, &host, "GUI4SIZEC", wide.0, wide.1);

    submit_line(&view, cx, &host, "echo PRECLEAR_MARKER");
    wait_plain(&view, cx, &host, "pre-clear marker missing", |text| {
        text.contains("PRECLEAR_MARKER")
    });
    press(&view, cx, &host, "ctrl-l");
    wait_view(
        &view,
        cx,
        &host,
        Duration::from_secs(10),
        "ctrl-l did not clear the parsed screen",
        |view| {
            let raw = &view.projection.terminal.output;
            raw.contains("PRECLEAR_MARKER")
                && !pawork_terminal::plain_output(raw).contains("PRECLEAR_MARKER")
        },
    );

    cx.update(|_window, app| {
        view.update(app, |view, cx| {
            view.on_new_terminal_tab(cx);
            assert!(view.terminal_pending_create_workspace.is_some());
        });
    });
    wait_view(
        &view,
        cx,
        &host,
        Duration::from_secs(15),
        "second terminal was not created",
        |view| {
            view.projection.terminal.session_id.as_deref() != Some(session.as_str())
                && view.projection.terminal.session_id.is_some()
                && view.terminal_pending_resize.is_none()
        },
    );
    let session_b = view
        .read_with(cx, |view, _| view.projection.terminal.session_id.clone())
        .expect("second session");
    ensure_terminal_focus(&view, cx, &host);
    submit_line(&view, cx, &host, "echo GUI4PID2:$$");
    wait_plain(&view, cx, &host, "missing second shell pid", |text| {
        parse_pid(text, "GUI4PID2:").is_some()
    });
    let pid_b = parse_pid(&plain(&view, cx), "GUI4PID2:").expect("pid 2");
    assert_ne!(pid_a, pid_b);
    assert!(process_alive(pid_b));
    assert!(
        !raw_output(&view, cx).contains("PRECLEAR_MARKER"),
        "second tab reused the first raw output"
    );

    cx.update(|window, app| {
        view.update(app, |view, cx| {
            view.on_close_terminal(window, cx);
            assert_eq!(
                view.terminal_pending_close.as_deref(),
                Some(session_b.as_str())
            );
        });
    });
    wait_view(
        &view,
        cx,
        &host,
        Duration::from_secs(10),
        "closing the second tab did not return to the first",
        |view| {
            view.terminal_pending_close.is_none()
                && view.projection.terminal.session_id.as_deref() == Some(session.as_str())
        },
    );
    wait_dead(
        pid_b,
        &view,
        cx,
        &host,
        "second shell still alive after close",
    );
    assert!(
        raw_output(&view, cx).contains("PRECLEAR_MARKER"),
        "first tab lost its raw output"
    );
    assert!(
        !raw_output(&view, cx).contains(&format!("GUI4PID2:{pid_b}")),
        "first tab raw output contains the second shell"
    );

    cx.update(|window, app| {
        view.update(app, |view, cx| {
            view.on_close_terminal(window, cx);
            assert_eq!(
                view.terminal_pending_close.as_deref(),
                Some(session.as_str())
            );
        });
    });
    wait_view(
        &view,
        cx,
        &host,
        Duration::from_secs(10),
        "closing the first tab left a session",
        |view| {
            view.terminal_pending_close.is_none() && view.projection.terminal.session_id.is_none()
        },
    );
    wait_dead(
        pid_a,
        &view,
        cx,
        &host,
        "first shell still alive after close",
    );
}
