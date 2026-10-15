//! CU-26 manual probe: drives `NativeHost::wayland()` against a running
//! `computer_guest --wayland` on 127.0.0.1:5907.
//!
//! Not a product path. The grant below stands in for the user's GUI
//! approval; the probe never passes a port, path or pid to the host.
//!
//! ```text
//! wayland_host_probe <bundle_id> <out_dir> <text>
//! wayland_host_probe <bundle_id> <out_dir> --boundaries
//! wayland_host_probe <bundle_id> <out_dir> --partial-receipt
//! ```
//!
//! `--boundaries` is the CU-26 revoke / cancel / disconnect / yield run.
//! `--partial-receipt` is the CU-26 partial / lost-receipt run. The guest
//! must be started with `PAWORK_WAYLAND_STEP_DELAY_MS` in 1..=1000 so a
//! prefix can reach the seat before the rest of the text. Partial
//! half-closes this process's write side after that prefix, then reads
//! the guest's incomplete receipt. Lost-receipt sets the host cancel flag
//! only after the prefix is in `events.bin`, so the request is already
//! written and the reply is abandoned.
//!
//! The target terminal must append raw input to `<out_dir>/events.bin`.
//! `PAWORK_ISOLATED_GUEST_TOKEN` must be set. There is no product takeover
//! control on this surface: the probe records occupancy and `close_scope`.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use pawork_computer_use::approval::{GrantKind, TargetIdentity};
use pawork_computer_use::guest::WAYLAND_GUEST_PORT;
use pawork_computer_use::target::{
    AppFamily, AppIdentity, KeyModifiers, Scope, TargetError, TargetedInput, TargetedKey,
    TargetedKeyPress,
};
use pawork_computer_use::{NativeHost, NativeOutput, NativeRequest};
use serde_json::{json, Value};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [bundle, out, flag] if flag == "--boundaries" => {
            std::process::exit(boundaries(bundle, out));
        }
        [bundle, out, flag] if flag == "--partial-receipt" => {
            std::process::exit(partial_receipt(bundle, out));
        }
        [bundle, out, text] => happy(bundle, out, text),
        _ => {
            eprintln!("usage: wayland_host_probe <bundle_id> <out_dir> <text>");
            eprintln!("       wayland_host_probe <bundle_id> <out_dir> --boundaries");
            eprintln!("       wayland_host_probe <bundle_id> <out_dir> --partial-receipt");
            std::process::exit(2);
        }
    }
}

fn happy(bundle: &str, out: &str, text: &str) {
    let host = NativeHost::wayland();
    let scope = Scope::new("cu26-probe", "slice2");
    host.open_scope(&scope);
    let report = |stage: &str, result: &Result<NativeOutput, _>| {
        let line = match result {
            Ok(output) => json!({
                "stage": stage,
                "ok": true,
                "detail": output.detail,
                "outcome": output.outcome,
                "input_dispatched": output.input_dispatched,
                "observation": output.observation_id,
                "observation_failure": output.observation_failure,
                "jpeg_bytes": output.jpeg.as_ref().map(Vec::len),
            }),
            Err(error) => json!({"stage": stage, "ok": false, "error": format!("{error:?}")}),
        };
        println!("{line}");
    };
    let save = |name: &str, output: &NativeOutput| {
        if let Some(jpeg) = &output.jpeg {
            std::fs::write(format!("{out}/{name}.jpg"), jpeg)
                .unwrap_or_else(|error| panic!("write {name}.jpg: {error}"));
        }
    };

    let bind = |label: &str| {
        let result = host.execute(
            &scope,
            NativeRequest::Bind {
                bundle_id: bundle.to_string(),
            },
            &|| false,
        );
        report(label, &result);
        result
    };
    let _ = host.execute(&scope, NativeRequest::Discover, &|| false);
    let _ = bind("bind_without_grant");
    let target = app_target(bundle);
    host.grant(&scope, &target, GrantKind::ForRun)
        .expect("probe grant");
    let Ok(bound) = bind("bind") else {
        std::process::exit(1);
    };
    let window = bound.window_id.expect("bind returns a window");

    let shot = host.execute(
        &scope,
        NativeRequest::Screenshot {
            window: window.clone(),
        },
        &|| false,
    );
    report("screenshot", &shot);
    let Ok(shot) = shot else {
        std::process::exit(1);
    };
    save("before", &shot);
    let first = shot.observation_id.clone().expect("lease");

    let (width, height) = image_size(shot.jpeg.as_deref().unwrap_or_default());
    let clicked = host.execute(
        &scope,
        NativeRequest::ObservedClick {
            observation: first.clone(),
            x: f64::from(width) / 2.0,
            y: f64::from(height) / 2.0,
        },
        &|| false,
    );
    report("click_center", &clicked);
    let Ok(clicked) = clicked else {
        std::process::exit(1);
    };
    save("after_click", &clicked);

    let stale = host.execute(
        &scope,
        NativeRequest::ObservedKeys {
            observation: first,
            input: TargetedInput::InsertText("stale".into()),
        },
        &|| false,
    );
    report("stale_lease", &stale);

    let typed = host.execute(
        &scope,
        NativeRequest::ObservedKeys {
            observation: clicked.observation_id.clone().expect("fresh lease"),
            input: TargetedInput::InsertText(text.to_string()),
        },
        &|| false,
    );
    report("type_text", &typed);
    let Ok(typed) = typed else {
        std::process::exit(1);
    };
    let entered = host.execute(
        &scope,
        NativeRequest::ObservedKeys {
            observation: typed.observation_id.clone().expect("fresh lease"),
            input: TargetedInput::KeyPress(TargetedKeyPress {
                key: TargetedKey::Return,
                modifiers: KeyModifiers::none(),
            }),
        },
        &|| false,
    );
    report("return", &entered);
    if let Ok(entered) = &entered {
        save("after_return", entered);
    }

    host.revoke(&scope, &target);
    let revoked = host.execute(&scope, NativeRequest::Screenshot { window }, &|| false);
    report("screenshot_after_revoke", &revoked);
    host.close_scope(&scope);
}

const PARTIAL_TEXT: &str = "INSEATGHOST";
const LOST_TEXT: &str = "LOSTSEATGHOST";

/// CU-26 partial receipt and lost receipt on a live seat.
///
/// Partial: once a prefix is in `events.bin`, half-close the host write
/// side. The guest stops between taps and returns an incomplete receipt,
/// which this process still reads. Lost receipt: the cancel flag flips
/// only after a prefix has landed, so the guest request is already
/// written and the host abandons the reply (`unknown_effect`).
fn partial_receipt(bundle: &str, out: &str) -> i32 {
    let host = NativeHost::wayland();
    let scope = Scope::new("cu26-partial", "owner");
    host.open_scope(&scope);
    let target = app_target(bundle);
    let mut log = Log::new(out);
    log.emit(json!({
        "stage": "note",
        "name": "partial-receipt",
        "detail": "Partial half-closes the host write side after a seat prefix and reads the incomplete receipt. Lost-receipt arms cancel only after a prefix is in events.bin.",
    }));

    if !wait_ready(out) {
        log.check("target_ready", false, "events log was not ready");
        return finish_one(&host, &scope, &mut log);
    }

    let discovered = host.execute(&scope, NativeRequest::Discover, &|| false);
    log.stage("discover", &discovered);
    let denied = bind(&host, &scope, bundle);
    log.stage("bind_without_grant", &denied);
    log.check(
        "bind_without_grant",
        err_is(&denied, "NotAuthorized"),
        err_text(&denied),
    );
    let granted = host.grant(&scope, &target, GrantKind::ForRun);
    log.check("grant", granted.is_ok(), format!("{granted:?}"));
    let bound = bind(&host, &scope, bundle);
    log.stage("bind", &bound);
    let mut window = window_of(&bound).unwrap_or_default();
    log.check(
        "bind",
        bound.is_ok() && !window.is_empty(),
        err_text(&bound),
    );
    if window.is_empty() {
        return finish_one(&host, &scope, &mut log);
    }

    let shot = screenshot(&host, &scope, &window);
    log.stage("screenshot", &shot);
    save_jpeg(out, "before", &shot);
    let mut obs = obs_of(&shot).unwrap_or_default();
    log.check(
        "screenshot",
        shot.is_ok() && !obs.is_empty(),
        err_text(&shot),
    );
    if obs.is_empty() {
        return finish_one(&host, &scope, &mut log);
    }

    let partial_before = events(out);
    let partial_closed = Arc::new(AtomicUsize::new(0));
    let partial_hits = watch_prefix(out, partial_before.len(), "I", {
        let partial_closed = Arc::clone(&partial_closed);
        move || {
            partial_closed.store(shut_write_guest_port(), Ordering::SeqCst);
        }
    });
    let partial = keys(&host, &scope, &obs, PARTIAL_TEXT);
    let _ = partial_hits.1.join();
    settle();
    let partial_after = events(out);
    let partial_delta = tail(&partial_before, &partial_after).to_vec();
    let _ = std::fs::write(format!("{out}/partial.delta"), &partial_delta);
    log.stage("partial_text", &partial);
    log.emit(json!({
        "stage": "partial_side",
        "watcher": partial_hits.0.load(Ordering::SeqCst),
        "shut_write": partial_closed.load(Ordering::SeqCst),
        "delta": lossy(&partial_delta),
        "delta_len": partial_delta.len(),
    }));
    let partial_outcome = outcome_of(&partial).unwrap_or("");
    let counts = parse_partial(partial_outcome);
    let dispatched = partial
        .as_ref()
        .ok()
        .is_some_and(|output| output.input_dispatched);
    log.check(
        "partial_outcome",
        counts.is_some_and(|(sent, total)| sent > 0 && sent < total)
            && dispatched
            && proper_prefix(&partial_delta, PARTIAL_TEXT)
            && !has(&partial_delta, "GHOST")
            && partial_hits.0.load(Ordering::SeqCst)
            && partial_closed.load(Ordering::SeqCst) >= 1,
        format!(
            "outcome={partial_outcome} counts={counts:?} dispatched={dispatched} delta={}",
            lossy(&partial_delta)
        ),
    );
    let stable = events(out);
    std::thread::sleep(Duration::from_millis(600));
    log.check(
        "partial_no_retry",
        events(out).len() == stable.len() && !has(&events(out), "GHOST"),
        format!(
            "len {}->{} delta={}",
            stable.len(),
            events(out).len(),
            lossy(&tail(&stable, &events(out)))
        ),
    );
    let retry = keys(&host, &scope, &obs, "RETRY1GHOST");
    log.stage("partial_stale_retry", &retry);
    settle();
    log.check(
        "partial_stale_retry",
        err_is(&retry, "StaleHandle") && !has(&events(out), "RETRY1GHOST"),
        err_text(&retry),
    );

    std::thread::sleep(Duration::from_millis(300));
    if let Some(id) = rebind_window(&host, &scope, bundle, &mut log, "rebind_after_partial") {
        window = id;
    }
    let resh = screenshot(&host, &scope, &window);
    log.stage("screenshot_after_partial", &resh);
    save_jpeg(out, "after_partial", &resh);
    obs = obs_of(&resh).unwrap_or_default();
    log.check(
        "screenshot_after_partial",
        resh.is_ok() && !obs.is_empty(),
        err_text(&resh),
    );
    if obs.is_empty() {
        return finish_one(&host, &scope, &mut log);
    }

    let lost_before = events(out);
    let lost_flag = Arc::new(AtomicBool::new(false));
    let lost_hits = watch_prefix(out, lost_before.len(), "L", {
        let lost_flag = Arc::clone(&lost_flag);
        move || {
            lost_flag.store(true, Ordering::SeqCst);
        }
    });
    let lost_cancel = {
        let lost_flag = Arc::clone(&lost_flag);
        move || lost_flag.load(Ordering::SeqCst)
    };
    let lost = keys_cancel(&host, &scope, &obs, LOST_TEXT, &lost_cancel);
    let _ = lost_hits.1.join();
    std::thread::sleep(Duration::from_millis(600));
    let lost_after = events(out);
    let lost_delta = tail(&lost_before, &lost_after).to_vec();
    let _ = std::fs::write(format!("{out}/lost.delta"), &lost_delta);
    log.stage("lost_receipt", &lost);
    log.emit(json!({
        "stage": "lost_side",
        "watcher": lost_hits.0.load(Ordering::SeqCst),
        "cancel_armed": lost_flag.load(Ordering::SeqCst),
        "delta": lossy(&lost_delta),
        "delta_len": lost_delta.len(),
    }));
    let lost_outcome = outcome_of(&lost).unwrap_or("");
    let lost_dispatched = lost
        .as_ref()
        .ok()
        .is_some_and(|output| output.input_dispatched);
    log.check(
        "lost_receipt",
        lost_outcome == "unknown_effect"
            && lost_dispatched
            && lost_hits.0.load(Ordering::SeqCst)
            && is_prefix(&lost_delta, LOST_TEXT),
        format!(
            "outcome={lost_outcome} dispatched={lost_dispatched} delta={}",
            lossy(&lost_delta)
        ),
    );
    let lost_stable = events(out);
    std::thread::sleep(Duration::from_millis(600));
    log.check(
        "lost_no_retry",
        events(out).len() == lost_stable.len(),
        format!(
            "len {}->{} delta={}",
            lost_stable.len(),
            events(out).len(),
            lossy(&tail(&lost_stable, &events(out)))
        ),
    );
    let lost_retry = keys(&host, &scope, &obs, "RETRY2GHOST");
    log.stage("lost_stale_retry", &lost_retry);
    settle();
    log.check(
        "lost_stale_retry",
        err_is(&lost_retry, "StaleHandle") && !has(&events(out), "RETRY2GHOST"),
        err_text(&lost_retry),
    );

    std::thread::sleep(Duration::from_millis(300));
    if let Some(id) = rebind_window(&host, &scope, bundle, &mut log, "rebind_after_lost") {
        window = id;
    }
    let again = screenshot(&host, &scope, &window);
    log.stage("screenshot_after_lost", &again);
    save_jpeg(out, "after_lost", &again);
    obs = obs_of(&again).unwrap_or_default();
    log.check(
        "screenshot_after_lost",
        again.is_ok() && !obs.is_empty(),
        err_text(&again),
    );
    if obs.is_empty() {
        return finish_one(&host, &scope, &mut log);
    }
    let after = keys(&host, &scope, &obs, "AFTEROK");
    log.stage("type_after_ok", &after);
    settle();
    log.check(
        "seat_still_works",
        is_dispatched(&after) && has(&events(out), "AFTEROK"),
        format!(
            "{} delta={}",
            err_text(&after),
            lossy(&tail(&lost_after, &events(out)))
        ),
    );
    save_jpeg(out, "after_ok", &after);

    finish_one(&host, &scope, &mut log)
}

fn finish_one(
    host: &NativeHost<pawork_computer_use::wayland_surface::WaylandSurface>,
    scope: &Scope,
    log: &mut Log,
) -> i32 {
    host.close_scope(scope);
    let failed = log.failed.clone();
    let code = if failed.is_empty() { 0 } else { 1 };
    log.emit(json!({"stage":"done","ok":code == 0,"failed":failed}));
    code
}

fn boundaries(bundle: &str, out: &str) -> i32 {
    let host = NativeHost::wayland();
    let owner = Scope::new("cu26-boundary", "owner");
    let waiter = Scope::new("cu26-boundary", "waiter");
    host.open_scope(&owner);
    host.open_scope(&waiter);
    let target = app_target(bundle);
    let mut log = Log::new(out);
    log.emit(json!({
        "stage": "note",
        "name": "takeover",
        "detail": "WaylandSurface::activity is always Idle, and this probe has no pause_dispatch entry. The closest yield is occupancy plus close_scope.",
    }));

    if !wait_ready(out) {
        log.check("target_ready", false, "events log was not ready");
        return finish(&host, &owner, &waiter, &mut log);
    }

    let discovered = host.execute(&owner, NativeRequest::Discover, &|| false);
    log.stage("discover", &discovered);
    let denied = bind(&host, &owner, bundle);
    log.stage("bind_without_grant", &denied);
    log.check(
        "bind_without_grant",
        err_is(&denied, "NotAuthorized"),
        err_text(&denied),
    );

    let granted = host.grant(&owner, &target, GrantKind::ForRun);
    log.check("grant", granted.is_ok(), format!("{granted:?}"));
    let bound = bind(&host, &owner, bundle);
    log.stage("bind", &bound);
    let mut window = window_of(&bound).unwrap_or_default();
    log.check(
        "bind",
        bound.is_ok() && !window.is_empty(),
        err_text(&bound),
    );
    if window.is_empty() {
        return finish(&host, &owner, &waiter, &mut log);
    }

    let shot = screenshot(&host, &owner, &window);
    log.stage("screenshot", &shot);
    save_jpeg(out, "before", &shot);
    let mut obs = obs_of(&shot).unwrap_or_default();
    log.check(
        "screenshot",
        shot.is_ok() && !obs.is_empty(),
        err_text(&shot),
    );
    if obs.is_empty() {
        return finish(&host, &owner, &waiter, &mut log);
    }

    let before_click = events(out);
    let (width, height) = image_size(
        shot.as_ref()
            .ok()
            .and_then(|o| o.jpeg.as_deref())
            .unwrap_or(&[]),
    );
    let clicked = host.execute(
        &owner,
        NativeRequest::ObservedClick {
            observation: obs.clone(),
            x: f64::from(width) / 2.0,
            y: f64::from(height) / 2.0,
        },
        &|| false,
    );
    log.stage("click_center", &clicked);
    settle();
    let after_click = events(out);
    let mice_before = mice(&before_click);
    let mice_after = mice(&after_click);
    log.check(
        "seat_click",
        clicked.is_ok() && mice_after > mice_before,
        format!(
            "mice {mice_before}->{mice_after} delta={}",
            lossy(&tail(&before_click, &after_click))
        ),
    );
    if let Some(id) = obs_of(&clicked) {
        obs = id;
    }
    let typed = keys(&host, &owner, &obs, "SEAT-OK");
    log.stage("type_seat_ok", &typed);
    settle();
    let after_type = events(out);
    log.check(
        "seat_text",
        typed.is_ok() && has(&after_type, "SEAT-OK"),
        lossy(&tail(&after_click, &after_type)),
    );
    if let Some(id) = obs_of(&typed) {
        obs = id;
    }

    let waiter_grant = host.grant(&waiter, &target, GrantKind::ForRun);
    log.check(
        "waiter_grant",
        waiter_grant.is_ok(),
        format!("{waiter_grant:?}"),
    );
    let occupied = bind(&host, &waiter, bundle);
    log.stage("waiter_bind_while_held", &occupied);
    log.check(
        "waiter_bind_occupied",
        err_is(&occupied, "TargetOccupied"),
        err_text(&occupied),
    );
    let states = state_line(&host);
    log.emit(json!({"stage":"target_states","when":"occupied","line":states}));
    let paused = host
        .target_states()
        .iter()
        .any(|state| state.dispatch_paused);
    log.check("no_user_pause_signal", !paused, states);
    let steal_before = events(out);
    let stolen = screenshot(&host, &waiter, &window);
    log.stage("waiter_screenshot_owner_window", &stolen);
    settle();
    log.check(
        "waiter_screenshot_rejected",
        stolen.is_err() && events(out).len() == steal_before.len(),
        err_text(&stolen),
    );

    let lease = screenshot(&host, &owner, &window);
    log.stage("lease_before_revoke", &lease);
    obs = obs_of(&lease).unwrap_or(obs);
    let mice_at_revoke = mice(&events(out));
    let revoke_before = events(out);
    host.revoke(&owner, &target);
    let revoked_shot = screenshot(&host, &owner, &window);
    log.stage("screenshot_after_revoke", &revoked_shot);
    let revoked_click = host.execute(
        &owner,
        NativeRequest::ObservedClick {
            observation: obs.clone(),
            x: 1.0,
            y: 1.0,
        },
        &|| false,
    );
    log.stage("click_after_revoke", &revoked_click);
    let revoked_keys = keys(&host, &owner, &obs, "REVOKE-GHOST");
    log.stage("keys_after_revoke", &revoked_keys);
    settle();
    let revoke_after = events(out);
    log.check(
        "revoke_screenshot",
        err_is(&revoked_shot, "NotAuthorized"),
        err_text(&revoked_shot),
    );
    log.check(
        "revoke_click",
        err_is(&revoked_click, "NotAuthorized"),
        err_text(&revoked_click),
    );
    log.check(
        "revoke_keys",
        err_is(&revoked_keys, "NotAuthorized"),
        err_text(&revoked_keys),
    );
    log.check(
        "revoke_no_ghost",
        !has(&revoke_after, "REVOKE-GHOST")
            && mice(&revoke_after) == mice_at_revoke
            && revoke_after.len() == revoke_before.len(),
        format!(
            "mice {mice_at_revoke}->{} delta={}",
            mice(&revoke_after),
            lossy(&tail(&revoke_before, &revoke_after))
        ),
    );

    let regrant = host.grant(&owner, &target, GrantKind::ForRun);
    log.check("regrant", regrant.is_ok(), format!("{regrant:?}"));
    let rebound = bind(&host, &owner, bundle);
    log.stage("rebind_after_revoke", &rebound);
    if let Some(id) = window_of(&rebound) {
        window = id;
    }
    let resh = screenshot(&host, &owner, &window);
    log.stage("screenshot_after_regrant", &resh);
    obs = obs_of(&resh).unwrap_or_default();
    log.check(
        "regrant_screenshot",
        resh.is_ok() && !obs.is_empty(),
        err_text(&resh),
    );
    if obs.is_empty() {
        return finish(&host, &owner, &waiter, &mut log);
    }

    let cancel_before_at = events(out);
    let mice_cancel = mice(&cancel_before_at);
    let cancelled = keys_cancel(&host, &owner, &obs, "CANCEL-GHOST", &|| true);
    log.stage("cancel_before_keys", &cancelled);
    settle();
    log.check(
        "cancel_before_keys",
        err_is(&cancelled, "Cancelled") && !has(&events(out), "CANCEL-GHOST"),
        format!(
            "{} delta={}",
            err_text(&cancelled),
            lossy(&tail(&cancel_before_at, &events(out)))
        ),
    );
    let continued = keys(&host, &owner, &obs, "AFTER-CANCEL-OK");
    log.stage("type_after_cancel_before", &continued);
    settle();
    log.check(
        "cancel_before_keeps_lease",
        continued.is_ok() && has(&events(out), "AFTER-CANCEL-OK"),
        err_text(&continued),
    );

    let click_lease = screenshot(&host, &owner, &window);
    log.stage("lease_for_cancel_click", &click_lease);
    let click_obs = obs_of(&click_lease).unwrap_or_default();
    let mice_before_cancel_click = mice(&events(out));
    let click_at = events(out);
    let cancelled_click = host.execute(
        &owner,
        NativeRequest::ObservedClick {
            observation: click_obs.clone(),
            x: 1.0,
            y: 1.0,
        },
        &|| true,
    );
    log.stage("cancel_before_click", &cancelled_click);
    settle();
    log.check(
        "cancel_before_click",
        err_is(&cancelled_click, "Cancelled")
            && mice(&events(out)) == mice_before_cancel_click
            && events(out).len() == click_at.len(),
        format!(
            "{} mice {mice_before_cancel_click}->{}",
            err_text(&cancelled_click),
            mice(&events(out))
        ),
    );

    let (spent, polls) = keys_after_polls(&host, &owner, &click_obs, "CANCEL2-GHOST", 2);
    log.stage("cancel_after_consume", &spent);
    log.emit(json!({"stage":"cancel_after_consume_polls","polls":polls}));
    settle();
    log.check(
        "cancel_after_consume",
        err_is(&spent, "Cancelled") && !has(&events(out), "CANCEL2-GHOST"),
        format!("{} polls={polls}", err_text(&spent)),
    );
    log.check(
        "cancel_mouse_unchanged",
        mice(&events(out)) == mice_cancel,
        format!("mice {mice_cancel}->{}", mice(&events(out))),
    );

    let inflight_lease = screenshot(&host, &owner, &window);
    log.stage("lease_for_inflight", &inflight_lease);
    let inflight_obs = obs_of(&inflight_lease).unwrap_or_default();
    let inflight_text = "INFLIGHT-GHOST-0123456789abcdef";
    let inflight_before = events(out);
    let (inflight, inflight_polls) =
        keys_after_polls(&host, &owner, &inflight_obs, inflight_text, 5);
    log.stage("cancel_after_write", &inflight);
    log.emit(json!({"stage":"cancel_after_write_polls","polls":inflight_polls}));
    settle();
    let inflight_after = events(out);
    let inflight_landed = has(&inflight_after, inflight_text);
    log.check(
        "cancel_after_write_not_clean",
        !is_dispatched(&inflight),
        format!(
            "{} polls={inflight_polls} full_text={inflight_landed} delta={}",
            err_text(&inflight),
            lossy(&tail(&inflight_before, &inflight_after))
        ),
    );

    if let Some(id) = rebind_window(&host, &owner, bundle, &mut log, "rebind_before_disconnect") {
        window = id;
    }
    let live = screenshot(&host, &owner, &window);
    log.stage("screenshot_before_disconnect", &live);
    obs = obs_of(&live).unwrap_or_default();
    let live_keys = keys(&host, &owner, &obs, "LIVE-AGAIN");
    log.stage("type_live_again", &live_keys);
    settle();
    log.check(
        "live_before_disconnect",
        live_keys.is_ok() && has(&events(out), "LIVE-AGAIN"),
        err_text(&live_keys),
    );
    // A fresh lease, not the one LIVE-AGAIN just spent. Severing must reject
    // this observation on its own; a spent lease would be StaleHandle anyway.
    let pending = screenshot(&host, &owner, &window);
    log.stage("lease_before_disconnect", &pending);
    obs = obs_of(&pending).unwrap_or_default();
    let stale_obs = obs.clone();
    let drop_before = events(out);
    let closed = sever_guest_port();
    log.emit(json!({"stage":"sever_guest","closed":closed,"port":WAYLAND_GUEST_PORT}));
    log.check("sever_guest", closed >= 1, format!("closed={closed}"));
    let dropped = keys(&host, &owner, &obs, "DROP-GHOST");
    log.stage("keys_after_disconnect", &dropped);
    settle();
    log.check(
        "disconnect_rejects",
        dropped.is_err() && !has(&events(out), "DROP-GHOST"),
        format!(
            "{} delta={}",
            err_text(&dropped),
            lossy(&tail(&drop_before, &events(out)))
        ),
    );
    let dead_shot = screenshot(&host, &owner, &window);
    log.stage("screenshot_after_disconnect", &dead_shot);
    log.check(
        "disconnect_screenshot_rejects",
        dead_shot.is_err(),
        err_text(&dead_shot),
    );

    if let Some(id) = rebind_window(&host, &owner, bundle, &mut log, "rebind_after_disconnect") {
        window = id;
    }
    let redial = screenshot(&host, &owner, &window);
    log.stage("screenshot_after_redial", &redial);
    save_jpeg(out, "after_redial", &redial);
    obs = obs_of(&redial).unwrap_or_default();
    let redial_keys = keys(&host, &owner, &obs, "REDIAL-OK");
    log.stage("type_redial_ok", &redial_keys);
    settle();
    log.check(
        "redial_ok",
        redial.is_ok() && redial_keys.is_ok() && has(&events(out), "REDIAL-OK"),
        err_text(&redial_keys),
    );
    let stale = keys(&host, &owner, &stale_obs, "STALE-GHOST");
    log.stage("keys_stale_after_redial", &stale);
    settle();
    log.check(
        "stale_after_redial",
        stale.is_err() && !has(&events(out), "STALE-GHOST"),
        err_text(&stale),
    );

    let close_before = events(out);
    host.close_scope(&owner);
    let closed_scope = keys(&host, &owner, &obs, "CLOSED-GHOST");
    log.stage("keys_after_close_scope", &closed_scope);
    settle();
    log.check(
        "close_scope_rejects",
        err_is(&closed_scope, "ScopeClosed")
            && !has(&events(out), "CLOSED-GHOST")
            && events(out).len() == close_before.len(),
        format!(
            "{} delta={}",
            err_text(&closed_scope),
            lossy(&tail(&close_before, &events(out)))
        ),
    );
    let yielded = bind(&host, &waiter, bundle);
    log.stage("waiter_bind_after_close", &yielded);
    let waiter_window = window_of(&yielded).unwrap_or_default();
    log.check(
        "waiter_bind_after_close",
        yielded.is_ok() && !waiter_window.is_empty(),
        err_text(&yielded),
    );
    log.emit(json!({
        "stage": "target_states",
        "when": "after_yield_bind",
        "line": state_line(&host),
    }));
    let waiter_shot = screenshot(&host, &waiter, &waiter_window);
    log.stage("waiter_screenshot", &waiter_shot);
    save_jpeg(out, "after_yield", &waiter_shot);
    let waiter_obs = obs_of(&waiter_shot).unwrap_or_default();
    let yield_keys = keys(&host, &waiter, &waiter_obs, "YIELD-OK");
    log.stage("type_yield_ok", &yield_keys);
    settle();
    log.check(
        "yield_ok",
        waiter_shot.is_ok() && yield_keys.is_ok() && has(&events(out), "YIELD-OK"),
        err_text(&yield_keys),
    );

    finish(&host, &owner, &waiter, &mut log)
}

fn finish(
    host: &NativeHost<pawork_computer_use::wayland_surface::WaylandSurface>,
    owner: &Scope,
    waiter: &Scope,
    log: &mut Log,
) -> i32 {
    host.close_scope(owner);
    host.close_scope(waiter);
    let failed = log.failed.clone();
    let code = if failed.is_empty() { 0 } else { 1 };
    log.emit(json!({"stage":"done","ok":code == 0,"failed":failed}));
    code
}

struct Log {
    out: String,
    lines: Vec<Value>,
    failed: Vec<String>,
}

impl Log {
    fn new(out: &str) -> Self {
        Self {
            out: out.to_string(),
            lines: Vec::new(),
            failed: Vec::new(),
        }
    }

    fn emit(&mut self, value: Value) {
        println!("{value}");
        let _ = std::io::stdout().flush();
        self.lines.push(value);
        let _ = std::fs::write(
            format!("{}/summary.json", self.out),
            serde_json::to_string_pretty(&self.lines).unwrap_or_else(|_| "[]".into()),
        );
    }

    fn stage(&mut self, name: &str, result: &Result<NativeOutput, TargetError>) {
        let value = match result {
            Ok(output) => json!({
                "stage": name,
                "ok": true,
                "detail": output.detail,
                "outcome": output.outcome,
                "input_dispatched": output.input_dispatched,
                "observation": output.observation_id,
                "window": output.window_id,
                "observation_failure": output.observation_failure,
                "jpeg_bytes": output.jpeg.as_ref().map(Vec::len),
            }),
            Err(error) => json!({"stage": name, "ok": false, "error": format!("{error:?}")}),
        };
        self.emit(value);
    }

    fn check(&mut self, name: &str, pass: bool, detail: impl AsRef<str>) {
        let detail = detail.as_ref().to_string();
        self.emit(json!({"stage":"check","name":name,"ok":pass,"detail":detail}));
        if !pass {
            self.failed.push(name.to_string());
        }
    }
}

fn app_target(bundle: &str) -> TargetIdentity {
    TargetIdentity::application(AppIdentity {
        bundle_id: bundle.to_string(),
        family: AppFamily::Browser,
    })
}

fn bind(
    host: &NativeHost<pawork_computer_use::wayland_surface::WaylandSurface>,
    scope: &Scope,
    bundle: &str,
) -> Result<NativeOutput, TargetError> {
    host.execute(
        scope,
        NativeRequest::Bind {
            bundle_id: bundle.to_string(),
        },
        &|| false,
    )
}

fn screenshot(
    host: &NativeHost<pawork_computer_use::wayland_surface::WaylandSurface>,
    scope: &Scope,
    window: &str,
) -> Result<NativeOutput, TargetError> {
    host.execute(
        scope,
        NativeRequest::Screenshot {
            window: window.to_string(),
        },
        &|| false,
    )
}

fn keys(
    host: &NativeHost<pawork_computer_use::wayland_surface::WaylandSurface>,
    scope: &Scope,
    observation: &str,
    text: &str,
) -> Result<NativeOutput, TargetError> {
    keys_cancel(host, scope, observation, text, &|| false)
}

fn keys_cancel(
    host: &NativeHost<pawork_computer_use::wayland_surface::WaylandSurface>,
    scope: &Scope,
    observation: &str,
    text: &str,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<NativeOutput, TargetError> {
    host.execute(
        scope,
        NativeRequest::ObservedKeys {
            observation: observation.to_string(),
            input: TargetedInput::InsertText(text.to_string()),
        },
        cancel,
    )
}

fn keys_after_polls(
    host: &NativeHost<pawork_computer_use::wayland_surface::WaylandSurface>,
    scope: &Scope,
    observation: &str,
    text: &str,
    allow: usize,
) -> (Result<NativeOutput, TargetError>, usize) {
    let calls = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&calls);
    let cancel = move || flag.fetch_add(1, Ordering::SeqCst) >= allow;
    let result = keys_cancel(host, scope, observation, text, &cancel);
    (result, calls.load(Ordering::SeqCst))
}

fn rebind_window(
    host: &NativeHost<pawork_computer_use::wayland_surface::WaylandSurface>,
    scope: &Scope,
    bundle: &str,
    log: &mut Log,
    stage: &str,
) -> Option<String> {
    let bound = bind(host, scope, bundle);
    log.stage(stage, &bound);
    window_of(&bound)
}

fn window_of(result: &Result<NativeOutput, TargetError>) -> Option<String> {
    result
        .as_ref()
        .ok()
        .and_then(|output| output.window_id.clone())
}

fn obs_of(result: &Result<NativeOutput, TargetError>) -> Option<String> {
    result
        .as_ref()
        .ok()
        .and_then(|output| output.observation_id.clone())
}

fn err_is(result: &Result<NativeOutput, TargetError>, name: &str) -> bool {
    match result {
        Err(error) => format!("{error:?}").contains(name),
        Ok(_) => false,
    }
}

fn err_text(result: &Result<NativeOutput, TargetError>) -> String {
    match result {
        Ok(output) => format!(
            "ok outcome={:?} failure={:?} dispatched={}",
            output.outcome, output.observation_failure, output.input_dispatched
        ),
        Err(error) => format!("{error:?}"),
    }
}

fn is_dispatched(result: &Result<NativeOutput, TargetError>) -> bool {
    matches!(result, Ok(output) if output.outcome.as_deref() == Some("dispatched"))
}

fn outcome_of(result: &Result<NativeOutput, TargetError>) -> Option<&str> {
    result
        .as_ref()
        .ok()
        .and_then(|output| output.outcome.as_deref())
}

fn parse_partial(outcome: &str) -> Option<(u32, u32)> {
    let rest = outcome.strip_prefix("partial ")?;
    let (counts, _) = rest.split_once(':')?;
    let (sent, total) = counts.trim().split_once('/')?;
    Some((sent.parse().ok()?, total.parse().ok()?))
}

fn is_prefix(delta: &[u8], text: &str) -> bool {
    let text = text.as_bytes();
    !delta.is_empty() && delta.len() <= text.len() && text.starts_with(delta)
}

fn proper_prefix(delta: &[u8], text: &str) -> bool {
    is_prefix(delta, text) && delta.len() < text.len()
}

fn watch_prefix(
    out: &str,
    baseline: usize,
    prefix: &str,
    action: impl Fn() + Send + 'static,
) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
    let hit = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&hit);
    let out = out.to_string();
    let needle = prefix.as_bytes().to_vec();
    let handle = thread::spawn(move || {
        for _ in 0..800 {
            let buf = events(&out);
            let delta = if buf.len() >= baseline {
                &buf[baseline..]
            } else {
                &buf[..]
            };
            if delta.len() >= needle.len()
                && delta.windows(needle.len()).any(|window| window == needle)
            {
                action();
                flag.store(true, Ordering::SeqCst);
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
    (hit, handle)
}

fn state_line(host: &NativeHost<pawork_computer_use::wayland_surface::WaylandSurface>) -> String {
    host.target_states()
        .iter()
        .map(|state| {
            let occupied = state
                .occupied_by
                .as_ref()
                .map(|scope| format!("{}/{}", scope.workspace_id, scope.run_id))
                .unwrap_or_else(|| "-".into());
            format!(
                "{}/{} grant={:?} occupied={occupied} paused={}",
                state.scope.workspace_id, state.scope.run_id, state.grant, state.dispatch_paused
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn save_jpeg(out: &str, name: &str, result: &Result<NativeOutput, TargetError>) {
    let Ok(output) = result else {
        return;
    };
    if let Some(jpeg) = &output.jpeg {
        let _ = std::fs::write(format!("{out}/{name}.jpg"), jpeg);
    }
}

fn events(out: &str) -> Vec<u8> {
    std::fs::read(format!("{out}/events.bin")).unwrap_or_default()
}

fn wait_ready(out: &str) -> bool {
    let path = format!("{out}/ready");
    for _ in 0..40 {
        if std::path::Path::new(&path).exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

fn settle() {
    std::thread::sleep(Duration::from_millis(250));
}

fn has(buf: &[u8], needle: &str) -> bool {
    let needle = needle.as_bytes();
    !needle.is_empty() && buf.windows(needle.len()).any(|window| window == needle)
}

fn mice(buf: &[u8]) -> usize {
    buf.windows(3)
        .filter(|window| *window == *b"\x1b[M")
        .count()
}

fn tail<'a>(before: &[u8], after: &'a [u8]) -> &'a [u8] {
    if after.len() >= before.len() && after.starts_with(before) {
        &after[before.len()..]
    } else {
        after
    }
}

fn lossy(buf: &[u8]) -> String {
    let end = buf.len().min(80);
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

fn image_size(jpeg: &[u8]) -> (u32, u32) {
    image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .map(|image| (image.width(), image.height()))
        .unwrap_or((1, 1))
}

/// Shut down this process's TCP sockets whose peer port is the Wayland guest.
/// The listener lives in another process, so this only drops the cached link.
#[cfg(target_os = "linux")]
fn sever_guest_port() -> usize {
    let mut closed = 0usize;
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(fd) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if fd < 3 {
            continue;
        }
        let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let rc =
            unsafe { libc::getpeername(fd, &mut addr as *mut _ as *mut libc::sockaddr, &mut len) };
        if rc != 0 {
            continue;
        }
        let port = unsafe {
            if i32::from(addr.ss_family) != libc::AF_INET {
                continue;
            }
            let v4 = &*(&addr as *const _ as *const libc::sockaddr_in);
            u16::from_be(v4.sin_port)
        };
        if port == WAYLAND_GUEST_PORT && unsafe { libc::shutdown(fd, libc::SHUT_RDWR) } == 0 {
            closed += 1;
        }
    }
    closed
}

/// Half-close the write side only. The guest sees EOF between taps and
/// can still write the incomplete receipt for this process to read.
#[cfg(target_os = "linux")]
fn shut_write_guest_port() -> usize {
    let mut closed = 0usize;
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(fd) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if fd < 3 {
            continue;
        }
        let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let rc =
            unsafe { libc::getpeername(fd, &mut addr as *mut _ as *mut libc::sockaddr, &mut len) };
        if rc != 0 {
            continue;
        }
        let port = unsafe {
            if i32::from(addr.ss_family) != libc::AF_INET {
                continue;
            }
            let v4 = &*(&addr as *const _ as *const libc::sockaddr_in);
            u16::from_be(v4.sin_port)
        };
        if port == WAYLAND_GUEST_PORT && unsafe { libc::shutdown(fd, libc::SHUT_WR) } == 0 {
            closed += 1;
        }
    }
    closed
}

#[cfg(not(target_os = "linux"))]
fn shut_write_guest_port() -> usize {
    0
}

#[cfg(not(target_os = "linux"))]
fn sever_guest_port() -> usize {
    0
}
