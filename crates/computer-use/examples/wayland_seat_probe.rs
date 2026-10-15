//! CU-26 manual acceptance probe for the dedicated Sway / wlroots seat.
//! Not a product path. Needs WAYLAND_DISPLAY / XDG_RUNTIME_DIR / SWAYSOCK
//! of a headless sway that Pawork owns.
//!
//!   wayland_seat_probe windows
//!   wayland_seat_probe smoke <out_dir> <text>
//!
//! `smoke` checks the seat is dedicated, captures, clicks the centre of the
//! first toplevel, types <text> + Return, captures again and prints JSON.
//! Effects in the target must be confirmed from facts outside this probe.

#[cfg(target_os = "linux")]
fn main() {
    use pawork_computer_use::wayland::{
        sway::{SwayIpc, WindowLedger},
        InputOutcome, PointerButton, SeatKey, SeatModifiers, WaylandSeat, DEVICE_SETTLE,
    };
    use serde_json::json;
    use std::time::Duration;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let fail = |stage: &str, error: &dyn std::fmt::Display| -> ! {
        println!(
            "{}",
            json!({"ok": false, "stage": stage, "error": error.to_string()})
        );
        std::process::exit(1)
    };
    let mut ipc =
        SwayIpc::connect_from_env(Duration::from_secs(8)).unwrap_or_else(|e| fail("ipc", &e));
    let mut ledger = WindowLedger::default();
    let windows = ipc.windows().unwrap_or_else(|e| fail("windows", &e));
    let entries = ledger.sync(ipc.compositor(), &windows);
    let listed: Vec<_> = entries
        .iter()
        .map(|(window, process, generation)| {
            json!({
                "con_id": window.con_id, "generation": generation, "app_id": window.app_id,
                "pid": process.pid, "start_token": process.start_token,
                "rect": [window.rect.x, window.rect.y, window.rect.width, window.rect.height],
                "focused": window.focused,
            })
        })
        .collect();
    let compositor = ipc.compositor();
    match args.first().map(String::as_str) {
        Some("windows") => {
            println!(
                "{}",
                json!({"ok": true, "compositor": [compositor.pid, compositor.start_token], "windows": listed})
            );
        }
        Some("smoke") if args.len() == 3 => {
            let dir = std::path::Path::new(&args[1]);
            let never = || false;
            let mut seat = WaylandSeat::connect_from_env(DEVICE_SETTLE)
                .unwrap_or_else(|e| fail("connect", &e));
            let inputs = ipc
                .require_dedicated()
                .unwrap_or_else(|e| fail("dedicated", &e));
            let before = seat
                .capture()
                .unwrap_or_else(|e| fail("capture_before", &e));
            let (jpeg, w, h) = before.jpeg().unwrap_or_else(|e| fail("jpeg", &e));
            std::fs::write(dir.join("before.jpg"), jpeg).unwrap_or_else(|e| fail("write", &e));
            let Some((target, _, _)) = entries.first() else {
                fail("target", &"no toplevel on the seat")
            };
            let (x, y) = (
                (target.rect.x + target.rect.width / 2) as u32,
                (target.rect.y + target.rect.height / 2) as u32,
            );
            let click = seat
                .click(x, y, PointerButton::Left, &never)
                .unwrap_or_else(|e| fail("click", &e));
            std::thread::sleep(Duration::from_millis(300));
            let typed = seat
                .type_text(&args[2], &never)
                .unwrap_or_else(|e| fail("type", &e));
            let enter = seat
                .key(SeatKey::Return, SeatModifiers::default(), &never)
                .unwrap_or_else(|e| fail("key", &e));
            std::thread::sleep(Duration::from_millis(500));
            let after = seat.capture().unwrap_or_else(|e| fail("capture_after", &e));
            std::fs::write(
                dir.join("after.jpg"),
                after.jpeg().unwrap_or_else(|e| fail("jpeg", &e)).0,
            )
            .unwrap_or_else(|e| fail("write", &e));
            let changed = before
                .rgb
                .chunks_exact(3)
                .zip(after.rgb.chunks_exact(3))
                .filter(|(a, b)| a != b)
                .count();
            let outcome = |o: InputOutcome| format!("{o:?}");
            println!(
                "{}",
                json!({
                    "ok": true,
                    "compositor": [compositor.pid, compositor.start_token],
                    "windows": listed,
                    "dedicated_inputs": inputs,
                    "output": seat.output_size(),
                    "frame": [before.width, before.height], "jpeg": [w, h],
                    "click": {"at": [x, y], "outcome": outcome(click)},
                    "type": outcome(typed), "return": outcome(enter),
                    "changed_pixels": changed,
                })
            );
        }
        _ => fail("usage", &"windows | smoke <out_dir> <text>"),
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("wayland_seat_probe runs only inside the Linux guest");
    std::process::exit(2);
}
