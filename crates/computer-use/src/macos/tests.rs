//! macOS backend bookkeeping and refusal tests.

use super::*;
use crate::approval::{DispatchPermit, GrantKind};
use crate::target::MAX_SEMANTIC_TEXT_CHARS;
use crate::target::{Environment, TargetObservation};
use crate::target::{TargetedKeyPress, MAX_TARGETED_TEXT_CHARS};

fn scope() -> Scope {
    Scope::new("ws", "run")
}

#[test]
fn window_generations_survive_and_reassign_after_vanish() {
    let mut generations = WindowGenerations::default();
    generations.sync(&HashSet::from([1, 2]));
    let first = generations.generation(1).unwrap();
    let second = generations.generation(2).unwrap();
    assert_ne!(first, second);
    // Surviving windows keep their generation.
    generations.sync(&HashSet::from([1, 2, 3]));
    assert_eq!(generations.generation(1), Some(first));
    // Window 1 vanished: forgotten.
    generations.sync(&HashSet::from([2, 3]));
    assert_eq!(generations.generation(1), None);
    assert_eq!(generations.generation(2), Some(second));
    // Identifier reused by a new window: a fresh generation, never the
    // one an old handle was bound to.
    generations.sync(&HashSet::from([1, 2, 3]));
    let reassigned = generations.generation(1).unwrap();
    assert_ne!(reassigned, first);
    assert_eq!(generations.generation(2), Some(second));
}

#[test]
fn classify_marks_browsers_chromium_and_appkit() {
    // Browsers are recognized by the web schemes they declare, whether
    // Chromium-based (Chrome), WebKit (Safari) or Gecko (Firefox).
    assert_eq!(classify(true, true), AppFamily::Browser);
    assert_eq!(classify(false, true), AppFamily::Browser);
    // Electron/CEF apps without web schemes are Chromium-embedded.
    assert_eq!(classify(true, false), AppFamily::Chromium);
    assert_eq!(classify(false, false), AppFamily::Appkit);
}

#[test]
fn launch_authorization_gates_the_system_seam() {
    // All counter assertions live in this one test: the counter is
    // process-global, and parallel tests must not interleave with it.
    let native = MacosNative::new();
    let mut authorizer = TargetAuthorizer::default();
    let app = AppIdentity {
        bundle_id: "com.apple.TextEdit".into(),
        family: AppFamily::Appkit,
    };
    let calls_before = LAUNCH_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    // No grant at all.
    assert!(matches!(
        native.launch_authorized(&authorizer, &scope(), &app),
        Err(LaunchError::Denied(TargetError::NotAuthorized))
    ));
    // Protected bundles are denied even at the launch seam.
    let protected = AppIdentity {
        bundle_id: "com.apple.systempreferences".into(),
        family: AppFamily::Appkit,
    };
    assert!(matches!(
        native.launch_authorized(&authorizer, &scope(), &protected),
        Err(LaunchError::Denied(TargetError::ForbiddenTarget))
    ));
    // Denials must be decided without touching LaunchServices.
    assert_eq!(
        LAUNCH_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        calls_before,
        "denied launches reached the system launch seam"
    );
    // A granted identity does reach the launch seam (which then reports
    // the bundle id as unresolvable).
    let unknown = AppIdentity {
        bundle_id: "dev.pawork.cu03.no-such-app".into(),
        family: AppFamily::Appkit,
    };
    authorizer
        .grant(
            &scope(),
            &TargetIdentity::application(unknown.clone()),
            GrantKind::ForRun,
        )
        .unwrap();
    assert!(matches!(
        native.launch_authorized(&authorizer, &scope(), &unknown),
        Err(LaunchError::NotFound(_))
    ));
    assert_eq!(
        LAUNCH_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        calls_before + 1
    );
}

fn cg_window(id: u32, pid: i32, frame: [f64; 4]) -> RawWindow {
    RawWindow {
        id,
        owner_pid: pid,
        layer: 0,
        bounds: WindowBounds {
            x: frame[0],
            y: frame[1],
            width: frame[2],
            height: frame[3],
        },
        title: None,
    }
}

#[test]
fn capture_scale_respects_pixel_ceilings() {
    // A normal window captures at 2x (Retina).
    assert_eq!(capture_scale(600.0, 400.0), Some(2.0));
    // A very long window is capped by the 4096 edge ceiling.
    assert_eq!(capture_scale(4096.0, 100.0), Some(1.0));
    assert_eq!(capture_scale(8192.0, 100.0), Some(0.5));
    // A huge window is capped by the 8M pixel ceiling.
    let scale = capture_scale(6000.0, 4000.0).unwrap();
    assert!(6000.0 * scale * 4000.0 * scale <= MAX_CAPTURE_PIXELS + 1.0);
    assert!(scale < 0.6);
    // Degenerate bounds never reach the capture seam.
    for (w, h) in [(0.0, 100.0), (100.0, -1.0), (f64::NAN, 100.0)] {
        assert_eq!(capture_scale(w, h), None);
    }
}

#[test]
fn capture_pixel_size_keeps_the_rounded_product_within_budget() {
    // Independent per-axis rounding-up would produce 2828x2829 =
    // 8,000,412 > 8,000,000 for a 2000x2001pt window at the pixel
    // ceiling; the floored integer size must hold both ceilings.
    let (width, height) = capture_pixel_size(2000.0, 2001.0).unwrap();
    assert!(width <= MAX_CAPTURE_EDGE as usize);
    assert!(height <= MAX_CAPTURE_EDGE as usize);
    assert!(width * height <= MAX_CAPTURE_PIXELS as usize);
    // A normal window captures at 2x.
    assert_eq!(capture_pixel_size(600.0, 400.0), Some((1200, 800)));
    // Degenerate bounds never reach the capture seam.
    assert_eq!(capture_pixel_size(0.0, 100.0), None);
}

#[test]
fn frame_layout_enforces_the_pixel_budget_before_copying() {
    use sck::checked_frame_layout as layout;
    assert!(layout(1280, 835, 1280 * 4).is_ok());
    assert!(layout(4096, 1953, 4096 * 4).is_ok());
    // Over-budget or degenerate delivered frames are rejected before
    // any pixel is copied out of the CGImage.
    assert!(layout(4097, 100, 4097 * 4).is_err());
    assert!(layout(2828, 2829, 2828 * 4).is_err());
    assert!(layout(0, 100, 400).is_err());
    assert!(layout(100, 100, 399).is_err());
}

#[test]
fn completion_releases_results_arriving_after_timeout() {
    // Counting disposer: every orphaned result is released exactly
    // once, and a normally delivered result never is.
    static RELEASED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    unsafe fn count_release(pointer: usize) {
        assert_ne!(pointer, 0);
        RELEASED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    // A result delivered before the wait completes is taken by the
    // waiter; the disposer does not run.
    let pair = sck::completion(count_release);
    sck::deliver(&pair, 42, None);
    let (result, error) = sck::wait_completion(&pair, Duration::from_millis(100)).unwrap();
    assert_eq!((result, error), (42, None));
    assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 0);
    // A result arriving after the wait timed out is released by the
    // handler side instead of leaking a retained frame.
    let pair = sck::completion(count_release);
    let err = sck::wait_completion(&pair, Duration::from_millis(1)).unwrap_err();
    assert!(matches!(err, sck::CaptureFailure::Unavailable(_)));
    sck::deliver(&pair, 43, None);
    assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 1);
    // A result arriving together with an error is released by the
    // handler side: the waiter receives the error and never an owned
    // pointer to leak on its error path.
    let pair = sck::completion(count_release);
    sck::deliver(&pair, 44, Some("boom".to_string()));
    let (result, error) = sck::wait_completion(&pair, Duration::from_millis(100)).unwrap();
    assert_eq!(result, 0);
    assert_eq!(error.as_deref(), Some("boom"));
    assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn bgra_pixels_convert_and_validate_layout() {
    // 2x1 BGRA with a padded stride converts to packed RGB.
    let bgra = [10, 20, 30, 255, 40, 50, 60, 255, 0, 0, 0, 0];
    assert_eq!(
        bgra_to_rgb(&bgra, 2, 1, 12),
        Some(vec![30, 20, 10, 60, 50, 40])
    );
    // Truncated or inconsistent layouts are rejected, never decoded
    // into a partial image.
    assert_eq!(bgra_to_rgb(&bgra, 4, 1, 12), None);
    assert_eq!(bgra_to_rgb(&bgra, 2, 2, 12), None);
    assert_eq!(bgra_to_rgb(&bgra, 2, 1, 7), None);
    assert_eq!(bgra_to_rgb(&bgra, 0, 1, 12), None);
}

#[test]
fn jpeg_stays_within_image_budget() {
    // A noisy (worst-case) 3000x2000 frame downscales to the 1280 edge
    // and fits the byte budget; the JPEG round-trips through the
    // decoder the isolated path and providers already use.
    let mut rgb = Vec::with_capacity(3000 * 2000 * 3);
    for i in 0..(3000 * 2000usize) {
        let v = (i as u32).wrapping_mul(2_654_435_761);
        rgb.extend_from_slice(&(v ^ (v >> 16)).to_le_bytes()[..3]);
    }
    let raw = RgbImage::from_raw(3000, 2000, rgb).unwrap();
    let (jpeg, width, height) = jpeg_within_budget(&raw).unwrap();
    assert!(width <= MAX_IMAGE_EDGE && height <= MAX_IMAGE_EDGE);
    assert_eq!(width, MAX_IMAGE_EDGE);
    assert!(jpeg.len() <= MAX_IMAGE_BYTES);
    let decoded = image::load_from_memory(&jpeg).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (width, height));
}

#[test]
fn capture_authorization_gates_precede_the_capture_seam() {
    // All counter assertions live in this one test: the counter is
    // process-global, and parallel tests must not interleave with it.
    let native = MacosNative::new();
    let mut authorizer = TargetAuthorizer::default();
    let app = AppIdentity {
        bundle_id: "com.apple.TextEdit".into(),
        family: AppFamily::Appkit,
    };
    let target = TargetIdentity::application(app);
    let calls_before = CAPTURE_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    let window = WindowIdentity {
        window_id: 999_999_999,
        generation: 1,
    };
    let dead = ProcessInstance {
        pid: 99_999_999,
        start_token: 1,
    };
    let validated = |instance: ProcessInstance| ValidatedWindow {
        target: target.clone(),
        instance,
        window,
        scope: scope(),
    };
    // No grant at all: rejected before any OS access.
    assert_eq!(
        native.capture_window(&authorizer, &validated(dead.clone())),
        Err(TargetError::NotAuthorized)
    );
    authorizer
        .grant(&scope(), &target, GrantKind::ForRun)
        .unwrap();
    // A dead process instance: rejected before liveness and capture.
    assert_eq!(
        native.capture_window(&authorizer, &validated(dead)),
        Err(TargetError::ProcessRestarted)
    );
    // A live process whose bundle id does not match the granted
    // identity: rejected before the AX read and the capture seam —
    // the test binary owns no bundle id, so it can never satisfy the
    // binding a TextEdit grant requires. The window-gone path after a
    // matching identity is covered by the capturegone acceptance.
    let own = ProcessInstance {
        pid: std::process::id(),
        start_token: process_start_token(std::process::id() as i32)
            .expect("own process start token"),
    };
    assert_eq!(
        native.capture_window(&authorizer, &validated(own)),
        Err(TargetError::Invalid(
            "process instance does not belong to the authorized identity"
        ))
    );
    assert_eq!(
        CAPTURE_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        calls_before,
        "denied captures reached the ScreenCaptureKit seam"
    );
}

#[test]
fn exact_ids_distinguish_same_frame_windows() {
    let frame = [100.0, 100.0, 600.0, 400.0];
    // Two live windows share a frame; one closes and lingers as a
    // ghost. The ghost id is never mapped by the app's AX tree, so the
    // closed window dies immediately while the survivor keeps
    // validating.
    let windows = vec![cg_window(10, 42, frame), cg_window(11, 42, frame)];
    let ax = vec![AxWindow {
        id: Some(11),
        frame,
    }];
    assert!(!window_proven_alive(&windows, &ax, 42, &windows[0]));
    assert!(window_proven_alive(&windows, &ax, 42, &windows[1]));
    // A brand-new window at the same frame proves itself by its own id;
    // the ghost never revives through it.
    let windows = vec![cg_window(10, 42, frame), cg_window(12, 42, frame)];
    let ax = vec![AxWindow {
        id: Some(12),
        frame,
    }];
    assert!(!window_proven_alive(&windows, &ax, 42, &windows[0]));
    assert!(window_proven_alive(&windows, &ax, 42, &windows[1]));
}

#[test]
fn frame_group_closure_fails_closed_on_ambiguity() {
    let frame = [100.0, 100.0, 600.0, 400.0];
    let other = [300.0, 300.0, 600.0, 400.0];
    // No id source (fallback mode). Single live window: counts close.
    let windows = vec![cg_window(1, 7, frame)];
    let ax = vec![AxWindow { id: None, frame }];
    assert!(window_proven_alive(&windows, &ax, 7, &windows[0]));
    // Ghost alone: window-server 1 > AX 0.
    assert!(!window_proven_alive(&windows, &[], 7, &windows[0]));
    // Two live windows at the same frame: 2 == 2, both alive.
    let windows = vec![cg_window(1, 7, frame), cg_window(2, 7, frame)];
    let ax = vec![AxWindow { id: None, frame }, AxWindow { id: None, frame }];
    assert!(window_proven_alive(&windows, &ax, 7, &windows[0]));
    assert!(window_proven_alive(&windows, &ax, 7, &windows[1]));
    // One of them closes: ghost + survivor (2 > 1) is ambiguous, so
    // both fail closed until the ghost is reaped.
    let ax = vec![AxWindow { id: None, frame }];
    assert!(!window_proven_alive(&windows, &ax, 7, &windows[0]));
    assert!(!window_proven_alive(&windows, &ax, 7, &windows[1]));
    // A new same-frame window while the ghost lingers stays ambiguous:
    // the old handle cannot revive through the new window.
    let windows = vec![cg_window(1, 7, frame), cg_window(3, 7, frame)];
    assert!(!window_proven_alive(&windows, &ax, 7, &windows[0]));
    assert!(!window_proven_alive(&windows, &ax, 7, &windows[1]));
    // Different frames never interfere.
    let windows = vec![cg_window(1, 7, frame), cg_window(2, 7, other)];
    let ax = vec![
        AxWindow { id: None, frame },
        AxWindow {
            id: None,
            frame: other,
        },
    ];
    assert!(window_proven_alive(&windows, &ax, 7, &windows[0]));
    assert!(window_proven_alive(&windows, &ax, 7, &windows[1]));
    // AX reporting more windows of a frame than the server lists
    // (stale) is equally ambiguous and fails closed.
    let ax = vec![
        AxWindow { id: None, frame },
        AxWindow { id: None, frame },
        AxWindow {
            id: None,
            frame: other,
        },
    ];
    assert!(!window_proven_alive(&windows, &ax, 7, &windows[0]));
    // Mixed id availability degrades the whole app to the fallback:
    // not every AX window carries an id, so exact matching is off and
    // the closed group (2 == 2) validates both entries.
    let windows = vec![cg_window(1, 7, frame), cg_window(2, 7, frame)];
    let ax = vec![
        AxWindow { id: Some(1), frame },
        AxWindow { id: None, frame },
    ];
    assert!(window_proven_alive(&windows, &ax, 7, &windows[0]));
    assert!(window_proven_alive(&windows, &ax, 7, &windows[1]));
}

#[test]
fn frame_group_overlap_fails_closed() {
    // Epsilon matching is not transitive: a ghost at x=100 and a live
    // window at x=100.8 form separate groups, but one AX window at
    // x=100.4 matches both. Counts close (1 == 1) for each, yet the
    // single AX candidate belongs to two groups — ambiguous, both
    // fail closed (the ghost must never borrow the live window's
    // proof).
    let ghost = [100.0, 100.0, 600.0, 400.0];
    let live = [100.8, 100.0, 600.0, 400.0];
    let shared = [100.4, 100.0, 600.0, 400.0];
    let windows = vec![cg_window(1, 7, ghost), cg_window(2, 7, live)];
    let ax = vec![AxWindow {
        id: None,
        frame: shared,
    }];
    assert!(!window_proven_alive(&windows, &ax, 7, &windows[0]));
    assert!(!window_proven_alive(&windows, &ax, 7, &windows[1]));
    // Without overlap each group closes on its own and both validate.
    let ax = vec![
        AxWindow {
            id: None,
            frame: ghost,
        },
        AxWindow {
            id: None,
            frame: live,
        },
    ];
    assert!(window_proven_alive(&windows, &ax, 7, &windows[0]));
    assert!(window_proven_alive(&windows, &ax, 7, &windows[1]));
}

#[test]
fn list_windows_rejects_identity_process_mismatch() {
    let native = MacosNative::new();
    let app = AppIdentity {
        bundle_id: "com.apple.TextEdit".into(),
        family: AppFamily::Appkit,
    };
    let mut authorizer = TargetAuthorizer::default();
    authorizer
        .grant(
            &scope(),
            &TargetIdentity::application(app.clone()),
            GrantKind::ForRun,
        )
        .unwrap();
    // Our own test process is alive but is not TextEdit: the TextEdit
    // grant must not unlock reads of an unrelated process.
    let pid = std::process::id();
    let token = process_start_token(pid as i32).expect("own process start token");
    let instance = ProcessInstance {
        pid,
        start_token: token,
    };
    assert!(matches!(
        native.list_windows(&authorizer, &scope(), &app, &instance, false),
        Err(TargetError::Invalid(_))
    ));
}

#[test]
fn ax_read_failures_keep_their_cause() {
    assert_eq!(
        ax_read_target_error(AxReadError::ReadFailed(K_AX_ERROR_API_DISABLED)),
        TargetError::PermissionMissing("accessibility")
    );
    // CannotComplete (-25204) is an ordinary probe failure, not a
    // permission report.
    assert!(matches!(
        ax_read_target_error(AxReadError::ReadFailed(-25204)),
        TargetError::ProbeUnavailable(_)
    ));
    assert!(matches!(
        ax_read_target_error(AxReadError::SymbolsUnavailable),
        TargetError::ProbeUnavailable(_)
    ));
    assert!(matches!(
        ax_read_target_error(AxReadError::CreateFailed),
        TargetError::ProbeUnavailable(_)
    ));
}

#[test]
fn frame_match_uses_global_points_with_epsilon() {
    let bounds = WindowBounds {
        x: 441.0,
        y: 103.0,
        width: 673.0,
        height: 439.0,
    };
    assert!(frame_matches(&bounds, &[441.0, 103.0, 673.0, 439.0]));
    assert!(frame_matches(&bounds, &[441.4, 102.6, 673.0, 439.0]));
    assert!(!frame_matches(&bounds, &[442.0, 103.0, 673.0, 439.0]));
    assert!(!frame_matches(&bounds, &[441.0, 103.0, 673.0, 440.0]));
}

#[test]
fn process_instance_liveness_uses_the_start_token() {
    let native = MacosNative::new();
    let pid = std::process::id();
    let token = process_start_token(pid as i32).expect("own process start token");
    assert!(native.process_instance_alive(&ProcessInstance {
        pid,
        start_token: token,
    }));
    // Same pid, different token: a previous incarnation never validates.
    assert!(!native.process_instance_alive(&ProcessInstance {
        pid,
        start_token: token + 1,
    }));
    // A pid that does not exist.
    assert!(!native.process_instance_alive(&ProcessInstance {
        pid: 99_999_999,
        start_token: 1,
    }));
}

#[test]
fn tree_read_authorization_gates_precede_the_walk_seam() {
    // All counter assertions live in this one test: the counter is
    // process-global, and parallel tests must not interleave with it.
    let native = MacosNative::new();
    let mut authorizer = TargetAuthorizer::default();
    let app = AppIdentity {
        bundle_id: "com.apple.TextEdit".into(),
        family: AppFamily::Appkit,
    };
    let target = TargetIdentity::application(app);
    let calls_before = AX_WALK_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    let window = WindowIdentity {
        window_id: 999_999_999,
        generation: 1,
    };
    let dead = ProcessInstance {
        pid: 99_999_999,
        start_token: 1,
    };
    let validated = |instance: ProcessInstance| ValidatedWindow {
        target: target.clone(),
        instance,
        window,
        scope: scope(),
    };
    // No grant at all: rejected before any OS access.
    assert_eq!(
        native.read_ax_tree(&authorizer, &validated(dead.clone())),
        Err(TargetError::NotAuthorized)
    );
    authorizer
        .grant(&scope(), &target, GrantKind::ForRun)
        .unwrap();
    // A dead process instance: rejected before liveness and the walk.
    assert_eq!(
        native.read_ax_tree(&authorizer, &validated(dead)),
        Err(TargetError::ProcessRestarted)
    );
    // A live process whose bundle id does not match the granted
    // identity: rejected before the preflight and the walk — the test
    // binary owns no bundle id, so a TextEdit grant never unlocks it.
    let own = ProcessInstance {
        pid: std::process::id(),
        start_token: process_start_token(std::process::id() as i32)
            .expect("own process start token"),
    };
    assert_eq!(
        native.read_ax_tree(&authorizer, &validated(own)),
        Err(TargetError::Invalid(
            "process instance does not belong to the authorized identity"
        ))
    );
    assert_eq!(
        AX_WALK_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        calls_before,
        "denied tree reads reached the AX walk seam"
    );
}

#[test]
fn semantic_action_authorization_gates_precede_the_action_seam() {
    // All counter assertions live in this one test: the counter is
    // process-global, and parallel tests must not interleave with it.
    let native = MacosNative::new();
    let mut authorizer = TargetAuthorizer::default();
    let app = AppIdentity {
        bundle_id: "com.apple.TextEdit".into(),
        family: AppFamily::Appkit,
    };
    let target = TargetIdentity::application(app);
    let calls_before = AX_ACTION_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    let window = WindowIdentity {
        window_id: 999_999_999,
        generation: 1,
    };
    let dead = ProcessInstance {
        pid: 99_999_999,
        start_token: 1,
    };
    let validated = |instance: ProcessInstance, permit: DispatchPermit| ValidatedElement {
        window: ValidatedWindow {
            target: target.clone(),
            instance,
            window,
            scope: scope(),
        },
        element_token: 0,
        tree_revision: 0,
        dispatch_permit: permit,
    };
    let press = SemanticAction::Press;
    // A forged permit is rejected before any OS access, even though
    // no grant was ever spent for it.
    let forged = DispatchPermit {
        token: "dp-forged".into(),
    };
    assert_eq!(
        native.semantic_action(
            &mut authorizer,
            &validated(dead.clone(), forged),
            &[],
            &press,
            &|| false
        ),
        Err(TargetError::NotAuthorized)
    );
    authorizer
        .grant(&scope(), &target, GrantKind::ForRun)
        .unwrap();
    // Payload validity precedes even the process check: an over-long
    // text on a dead process is Invalid, not ProcessRestarted. Every
    // attempt retires its permit, so each case mints a fresh one.
    let over_bound = SemanticAction::SetValue("x".repeat(MAX_SEMANTIC_TEXT_CHARS + 1));
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    assert_eq!(
        native.semantic_action(
            &mut authorizer,
            &validated(dead.clone(), permit),
            &[],
            &over_bound,
            &|| false
        ),
        Err(TargetError::Invalid(
            "semantic action text exceeds its bound"
        ))
    );
    // A dead process instance: rejected before identity and the seam.
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    assert_eq!(
        native.semantic_action(
            &mut authorizer,
            &validated(dead, permit),
            &[],
            &press,
            &|| false
        ),
        Err(TargetError::ProcessRestarted)
    );
    // A live process whose bundle id does not match the granted
    // identity: rejected before the capability gate and the seam — the
    // test binary owns no bundle id, so a TextEdit grant never unlocks
    // it. The capability gate itself (a Chromium value write reaching
    // BackgroundUnsupported without any AX call) is pinned by the
    // contract tests and the VS Code acceptance evidence.
    let own = ProcessInstance {
        pid: std::process::id(),
        start_token: process_start_token(std::process::id() as i32)
            .expect("own process start token"),
    };
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    assert_eq!(
        native.semantic_action(
            &mut authorizer,
            &validated(own, permit),
            &[],
            &press,
            &|| false
        ),
        Err(TargetError::Invalid(
            "process instance does not belong to the authorized identity"
        ))
    );
    // A one-shot grant spent by the consume authorizes the dispatch
    // it released through the minted permit: the dead process is what
    // rejects now. The first attempt retired the permit, so the same
    // (cloned) validation result can never dispatch twice; revoking
    // after the consume drops the outstanding permit and blocks the
    // dispatch; cancellation precedes even the ledger check.
    authorizer
        .grant(&scope(), &target, GrantKind::Once)
        .unwrap();
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    assert_eq!(
        authorizer.check(&scope(), &target),
        Err(TargetError::NotAuthorized)
    );
    let dead_again = ProcessInstance {
        pid: 99_999_999,
        start_token: 1,
    };
    let consumed_once = validated(dead_again.clone(), permit);
    assert_eq!(
        native.semantic_action(&mut authorizer, &consumed_once, &[], &press, &|| false),
        Err(TargetError::ProcessRestarted)
    );
    assert_eq!(
        native.semantic_action(&mut authorizer, &consumed_once, &[], &press, &|| false),
        Err(TargetError::NotAuthorized)
    );
    authorizer
        .grant(&scope(), &target, GrantKind::Once)
        .unwrap();
    let revoked = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    authorizer.revoke(&scope(), &target);
    assert_eq!(
        native.semantic_action(
            &mut authorizer,
            &validated(dead_again.clone(), revoked),
            &[],
            &press,
            &|| false,
        ),
        Err(TargetError::NotAuthorized)
    );
    let forged_again = DispatchPermit {
        token: "dp-forged".into(),
    };
    assert_eq!(
        native.semantic_action(
            &mut authorizer,
            &validated(dead_again, forged_again),
            &[],
            &press,
            &|| true,
        ),
        Err(TargetError::Cancelled)
    );
    assert_eq!(
        AX_ACTION_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        calls_before,
        "denied semantic actions reached the AX action seam"
    );
}

#[test]
fn semantic_action_result_maps_ax_codes_to_terminal_states() {
    assert_eq!(semantic_action_result(0), Ok(SemanticOutcome::Dispatched));
    // A messaging timeout may already have performed the action:
    // unknown effect, never a clean failure inviting a blind retry.
    assert_eq!(
        semantic_action_result(K_AX_ERROR_CANNOT_COMPLETE),
        Ok(SemanticOutcome::UnknownEffect)
    );
    // The element's own refusals are clear rejections.
    assert_eq!(
        semantic_action_result(K_AX_ERROR_ACTION_UNSUPPORTED),
        Err(TargetError::ElementUnsupported)
    );
    assert_eq!(
        semantic_action_result(K_AX_ERROR_ATTRIBUTE_UNSUPPORTED),
        Err(TargetError::ElementUnsupported)
    );
    // A dead element is a tree change; API-disabled is the permission
    // report; everything else keeps its cause as a probe failure.
    assert_eq!(
        semantic_action_result(K_AX_ERROR_INVALID_UI_ELEMENT),
        Err(TargetError::TreeChanged)
    );
    assert_eq!(
        semantic_action_result(K_AX_ERROR_API_DISABLED),
        Err(TargetError::PermissionMissing("accessibility"))
    );
    assert!(matches!(
        semantic_action_result(-25200),
        Err(TargetError::ProbeUnavailable(_))
    ));
    assert!(matches!(
        semantic_action_result(K_AX_ERROR_NO_VALUE),
        Err(TargetError::ProbeUnavailable(_))
    ));
}

#[test]
fn targeted_event_plans_chunk_on_char_boundaries_and_stay_balanced() {
    // Text chunks never split a character: every chunk decodes back,
    // and the chunk sizes respect the per-event UTF-16 bound.
    let text: String = "a😀汉".repeat(4); // 1+2+1 units per group
    let plan = targeted_event_plan(&TargetedInput::InsertText(text.clone()));
    let chunks: Vec<&Vec<u16>> = plan
        .iter()
        .map(|step| match step {
            PlannedStep::TextChunk(units) => units,
            _ => panic!("text plan must only contain chunks"),
        })
        .collect();
    assert!(chunks
        .iter()
        .all(|units| units.len() <= MAX_EVENT_UTF16_UNITS));
    assert!(chunks.iter().all(|units| String::from_utf16(units).is_ok()));
    let reassembled: String = chunks
        .iter()
        .map(|units| String::from_utf16(units).unwrap())
        .collect::<Vec<_>>()
        .concat();
    assert_eq!(reassembled, text);
    // Plain ASCII chunks at the bound: 25 chars -> 10/10/5.
    let plan = targeted_event_plan(&TargetedInput::InsertText("x".repeat(25)));
    let sizes: Vec<usize> = plan
        .iter()
        .map(|step| match step {
            PlannedStep::TextChunk(units) => units.len(),
            _ => panic!("text plan must only contain chunks"),
        })
        .collect();
    assert_eq!(sizes, vec![10, 10, 5]);
    assert_eq!(plan.iter().map(PlannedStep::event_count).sum::<usize>(), 6);

    // A bare key is one balanced tap with no modifier steps.
    let plan = targeted_event_plan(&TargetedInput::KeyPress(TargetedKeyPress {
        key: TargetedKey::Return,
        modifiers: KeyModifiers::none(),
    }));
    assert_eq!(
        plan,
        vec![PlannedStep::KeyTap {
            keycode: 36,
            flags: CGEventFlags::empty(),
        }]
    );

    // A combo presses modifiers with accumulating masks, taps the key
    // with the combined mask, and releases with shrinking masks ending
    // clean — pressed state always exists to release.
    let shift_ctrl = KeyModifiers {
        shift: true,
        control: true,
        option: false,
    };
    let plan = targeted_event_plan(&TargetedInput::KeyPress(TargetedKeyPress {
        key: TargetedKey::Left,
        modifiers: shift_ctrl,
    }));
    let shift = CGEventFlags::CGEventFlagShift;
    let control = CGEventFlags::CGEventFlagControl;
    assert_eq!(
        plan,
        vec![
            PlannedStep::ModsDown(vec![(56, shift), (59, shift | control)]),
            PlannedStep::KeyTap {
                keycode: 123,
                flags: shift | control,
            },
            PlannedStep::ModsUp(vec![(59, shift), (56, CGEventFlags::empty())]),
        ]
    );
    assert_eq!(plan.iter().map(PlannedStep::event_count).sum::<usize>(), 6);
}

#[test]
fn targeted_input_authorization_gates_precede_the_post_seam() {
    // All counter assertions and all real posting live in this one
    // test: the counter is process-global and parallel tests must not
    // interleave with it.
    let native = MacosNative::new();
    let mut authorizer = TargetAuthorizer::default();
    let app = AppIdentity {
        bundle_id: "com.apple.TextEdit".into(),
        family: AppFamily::Appkit,
    };
    let target = TargetIdentity::application(app);
    let calls_before = TARGETED_POST_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    let window = WindowIdentity {
        window_id: 999_999_999,
        generation: 1,
    };
    let dead = ProcessInstance {
        pid: 99_999_999,
        start_token: 1,
    };
    let validated = |instance: ProcessInstance, permit: DispatchPermit| ValidatedElement {
        window: ValidatedWindow {
            target: target.clone(),
            instance,
            window,
            scope: scope(),
        },
        element_token: 0,
        tree_revision: 0,
        dispatch_permit: permit,
    };
    let text = TargetedInput::InsertText("hello".into());
    // A forged permit is rejected before any OS access, even though no
    // grant was ever spent for it.
    let forged = DispatchPermit {
        token: "dp-forged".into(),
    };
    assert_eq!(
        native.targeted_input(
            &mut authorizer,
            &validated(dead.clone(), forged),
            &[],
            &text,
            &|| false
        ),
        Err(TargetError::NotAuthorized)
    );
    authorizer
        .grant(&scope(), &target, GrantKind::ForRun)
        .unwrap();
    // Payload validity precedes even the process check: an over-long
    // text on a dead process is Invalid, not ProcessRestarted. Every
    // attempt retires its permit, so each case mints a fresh one.
    let over_bound = TargetedInput::InsertText("x".repeat(MAX_TARGETED_TEXT_CHARS + 1));
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    assert_eq!(
        native.targeted_input(
            &mut authorizer,
            &validated(dead.clone(), permit),
            &[],
            &over_bound,
            &|| false
        ),
        Err(TargetError::Invalid(
            "targeted text must be 1..4096 chars without control characters; use a key press for Return/Tab"
        ))
    );
    // A dead process instance: rejected before identity and the seam.
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    assert_eq!(
        native.targeted_input(
            &mut authorizer,
            &validated(dead.clone(), permit),
            &[],
            &text,
            &|| false
        ),
        Err(TargetError::ProcessRestarted)
    );
    // A live process whose bundle id does not match the granted
    // identity: rejected before the capability gate and the seam —
    // the test binary owns no bundle id, so a TextEdit grant never
    // unlocks it. The family capability gate (a Chromium target
    // reaching BackgroundUnsupported without any AX call or event) is
    // pinned by the contract tests and the acceptance evidence.
    let own = ProcessInstance {
        pid: std::process::id(),
        start_token: process_start_token(std::process::id() as i32)
            .expect("own process start token"),
    };
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    assert_eq!(
        native.targeted_input(
            &mut authorizer,
            &validated(own, permit),
            &[],
            &text,
            &|| false
        ),
        Err(TargetError::Invalid(
            "process instance does not belong to the authorized identity"
        ))
    );
    // A one-shot grant spent by the consume authorizes the dispatch it
    // released through the minted permit; the first attempt retired
    // the permit, so the same (cloned) validation result can never
    // dispatch twice; revoking after the consume drops the outstanding
    // permit and blocks the dispatch; cancellation precedes even the
    // ledger check.
    authorizer
        .grant(&scope(), &target, GrantKind::Once)
        .unwrap();
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    assert_eq!(
        authorizer.check(&scope(), &target),
        Err(TargetError::NotAuthorized)
    );
    let dead_again = ProcessInstance {
        pid: 99_999_999,
        start_token: 1,
    };
    let consumed_once = validated(dead_again.clone(), permit);
    assert_eq!(
        native.targeted_input(&mut authorizer, &consumed_once, &[], &text, &|| false),
        Err(TargetError::ProcessRestarted)
    );
    assert_eq!(
        native.targeted_input(&mut authorizer, &consumed_once, &[], &text, &|| false),
        Err(TargetError::NotAuthorized)
    );
    authorizer
        .grant(&scope(), &target, GrantKind::Once)
        .unwrap();
    let revoked = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    authorizer.revoke(&scope(), &target);
    assert_eq!(
        native.targeted_input(
            &mut authorizer,
            &validated(dead_again.clone(), revoked),
            &[],
            &text,
            &|| false,
        ),
        Err(TargetError::NotAuthorized)
    );
    let forged_again = DispatchPermit {
        token: "dp-forged".into(),
    };
    assert_eq!(
        native.targeted_input(
            &mut authorizer,
            &validated(dead_again, forged_again),
            &[],
            &text,
            &|| true,
        ),
        Err(TargetError::Cancelled)
    );
    assert_eq!(
        TARGETED_POST_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        calls_before,
        "denied targeted inputs reached the event posting seam"
    );

    // Posting-path discipline (posted to this test process only — a
    // CLI without an event loop never consumes them). A combo aborted
    // between the modifier press and the key tap releases the pressed
    // modifiers: 1 posted + 1 release event, Partial, never clean.
    // (A shift-only combo is 4 events: 1 flags press + tap pair + 1
    // flags release.)
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).expect("event source");
    let own_pid = std::process::id() as libc::pid_t;
    let combo = targeted_event_plan(&TargetedInput::KeyPress(TargetedKeyPress {
        key: TargetedKey::Left,
        modifiers: KeyModifiers {
            shift: true,
            control: false,
            option: false,
        },
    }));
    assert_eq!(
        post_targeted_plan(&source, own_pid, &combo, &|| true, &|| false, &|| None),
        Ok(TargetedOutcome::Dispatched {
            events_posted: 4,
            events_total: 4,
        })
    );
    // A two-modifier combo must release with the plan's own
    // shrinking masks: control lifts while shift is still held
    // (flags = S), then shift lifts clean. Asserted on the flags the
    // pre-built release actually posted — counts alone cannot see a
    // wrong mask.
    let shift = CGEventFlags::CGEventFlagShift;
    let control = CGEventFlags::CGEventFlagControl;
    let shift_ctrl = targeted_event_plan(&TargetedInput::KeyPress(TargetedKeyPress {
        key: TargetedKey::Left,
        modifiers: KeyModifiers {
            shift: true,
            control: true,
            option: false,
        },
    }));
    let flags_before = TARGETED_POSTED_FLAGS.lock().expect("posted-flags").len();
    assert_eq!(
        post_targeted_plan(&source, own_pid, &shift_ctrl, &|| true, &|| false, &|| None),
        Ok(TargetedOutcome::Dispatched {
            events_posted: 6,
            events_total: 6,
        })
    );
    let posted_flags = TARGETED_POSTED_FLAGS.lock().expect("posted-flags");
    // The tap-up carries the HID source's live keyboard state (fn /
    // numpad, or whatever the person running the test is holding) —
    // the CU-01 measured shape — so nothing is asserted on it; the
    // presses, the tap-down and the release pair are exact.
    assert_eq!(posted_flags[flags_before], shift);
    assert_eq!(posted_flags[flags_before + 1], shift | control);
    assert_eq!(posted_flags[flags_before + 2], shift | control);
    assert_eq!(
        posted_flags[flags_before + 4],
        shift,
        "control must lift while shift is still held"
    );
    assert_eq!(
        posted_flags[flags_before + 5],
        CGEventFlags::empty(),
        "shift must lift clean"
    );
    drop(posted_flags);
    // Production plans put ModsDown first. A stop that is already
    // true after that group is built must not post the press, and
    // must not emit the release of a press that never went out.
    // A missed gate would post the press and then the later release.
    assert_eq!(
        post_targeted_plan(&source, own_pid, &combo, &|| true, &|| true, &|| None),
        Err(TargetError::Cancelled)
    );
    // A dead target between steps reports ProcessRestarted with the
    // same release discipline.
    assert_eq!(
        post_targeted_plan(&source, own_pid, &combo, &|| false, &|| false, &|| None),
        Ok(TargetedOutcome::Partial {
            events_posted: 2,
            events_total: 4,
            cause: TargetError::ProcessRestarted,
        })
    );
    // A failed between-step binding recheck (window replaced,
    // minimized, or the element unfocused) stops the input the same
    // way: release posted, Partial, never clean.
    assert_eq!(
        post_targeted_plan(&source, own_pid, &combo, &|| true, &|| false, &|| {
            Some(TargetError::NotFocused)
        }),
        Ok(TargetedOutcome::Partial {
            events_posted: 2,
            events_total: 4,
            cause: TargetError::NotFocused,
        })
    );
    let text_plan = targeted_event_plan(&TargetedInput::InsertText("x".repeat(25)));
    // The first chunk is built after the caller's entry check. A stop
    // already true must not post it. A missed gate would post that
    // chunk and then stop.
    assert_eq!(
        post_targeted_plan(&source, own_pid, &text_plan, &|| true, &|| true, &|| None),
        Err(TargetError::Cancelled)
    );
    // One production text chunk and one bare key are a single group.
    // Skipping the pre-post check returns Dispatched, not Partial.
    let one_chunk = targeted_event_plan(&TargetedInput::InsertText("x".into()));
    assert_eq!(one_chunk.len(), 1);
    assert_eq!(
        post_targeted_plan(&source, own_pid, &one_chunk, &|| true, &|| true, &|| None),
        Err(TargetError::Cancelled)
    );
    let bare_key = targeted_event_plan(&TargetedInput::KeyPress(TargetedKeyPress {
        key: TargetedKey::Left,
        modifiers: KeyModifiers::none(),
    }));
    assert_eq!(bare_key.len(), 1);
    assert!(matches!(bare_key[0], PlannedStep::KeyTap { .. }));
    assert_eq!(
        post_targeted_plan(&source, own_pid, &bare_key, &|| true, &|| true, &|| None),
        Err(TargetError::Cancelled)
    );
    // Creation-failure discipline (injected at the shared creation
    // seam; the combo's creation order is press #1, held release #2,
    // tap-down #3, tap-up #4). A failure before anything posted and
    // anything pressed is a plain error with zero events; once a
    // press has posted, the pre-built release still goes out and the
    // result is an honest Partial.
    let create_failed =
        || TargetError::ProbeUnavailable("CGEventCreateKeyboardEvent failed".to_string());
    TARGETED_CREATE_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    TARGETED_CREATE_FAIL_AT.store(1, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        post_targeted_plan(&source, own_pid, &combo, &|| true, &|| false, &|| None),
        Err(create_failed())
    );
    TARGETED_CREATE_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    TARGETED_CREATE_FAIL_AT.store(2, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        post_targeted_plan(&source, own_pid, &combo, &|| true, &|| false, &|| None),
        Err(create_failed())
    );
    TARGETED_CREATE_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    TARGETED_CREATE_FAIL_AT.store(3, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        post_targeted_plan(&source, own_pid, &combo, &|| true, &|| false, &|| None),
        Ok(TargetedOutcome::Partial {
            events_posted: 2,
            events_total: 4,
            cause: create_failed(),
        })
    );
    // Text: a first-chunk creation failure is a plain error; a later
    // chunk failing keeps the posted prefix and reports Partial with
    // nothing to release.
    TARGETED_CREATE_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    TARGETED_CREATE_FAIL_AT.store(1, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        post_targeted_plan(&source, own_pid, &text_plan, &|| true, &|| false, &|| None),
        Err(TargetError::ProbeUnavailable(
            "CGEventCreateKeyboardEvent failed".to_string(),
        ))
    );
    TARGETED_CREATE_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    TARGETED_CREATE_FAIL_AT.store(3, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        post_targeted_plan(&source, own_pid, &text_plan, &|| true, &|| false, &|| None),
        Ok(TargetedOutcome::Partial {
            events_posted: 2,
            events_total: 6,
            cause: TargetError::ProbeUnavailable("CGEventCreateKeyboardEvent failed".to_string(),),
        })
    );
    TARGETED_CREATE_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    TARGETED_CREATE_FAIL_AT.store(0, std::sync::atomic::Ordering::SeqCst);
    // A stop that flips inside the binding recheck, which itself
    // returns clean, must not post the group just built. Index 0
    // still posts. A missed gate would post the next text chunk
    // (4), the key tap (then the later release, 4), or the modifier
    // press. An empty first group posts nothing, so the same stop
    // is a plain error and does not emit a held release.
    let post_with_stop_in_recheck = |plan: &[PlannedStep]| {
        let stop = std::cell::Cell::new(false);
        let rechecks = std::cell::Cell::new(0usize);
        let outcome = post_targeted_plan(&source, own_pid, plan, &|| true, &|| stop.get(), &|| {
            rechecks.set(rechecks.get() + 1);
            stop.set(true);
            None
        });
        assert_eq!(rechecks.get(), 1, "binding recheck did not run once");
        outcome
    };
    assert_eq!(
        post_with_stop_in_recheck(&text_plan),
        Ok(TargetedOutcome::Partial {
            events_posted: 2,
            events_total: 6,
            cause: TargetError::Cancelled,
        })
    );
    assert_eq!(
        post_with_stop_in_recheck(&combo),
        Ok(TargetedOutcome::Partial {
            events_posted: 2,
            events_total: 4,
            cause: TargetError::Cancelled,
        })
    );
    let shift_down = [(56, shift)];
    let text_then_mod = vec![
        PlannedStep::TextChunk(vec![120]),
        PlannedStep::ModsDown(shift_down.to_vec()),
        PlannedStep::ModsUp(release_specs(&shift_down)),
    ];
    assert_eq!(
        post_with_stop_in_recheck(&text_then_mod),
        Ok(TargetedOutcome::Partial {
            events_posted: 2,
            events_total: 4,
            cause: TargetError::Cancelled,
        })
    );
    let empty_mod_then_text = vec![
        PlannedStep::ModsDown(vec![]),
        PlannedStep::TextChunk(vec![120]),
        PlannedStep::ModsUp(vec![]),
    ];
    assert_eq!(
        post_with_stop_in_recheck(&empty_mod_then_text),
        Err(TargetError::Cancelled)
    );
    // Call order: full combo 4, shift+ctrl 6, cancelled combo 0,
    // dead target 2, NotFocused 2, cancelled 25-char text 0,
    // cancelled one-chunk text 0, cancelled bare key 0, create
    // failures 0/0/2, text create failures 0/2, recheck 2/2/2/0.
    assert_eq!(
        TARGETED_POST_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        calls_before + 4 + 6 + 0 + 2 + 2 + 0 + 0 + 0 + 0 + 0 + 2 + 0 + 2 + 2 + 2 + 2 + 0,
        "the posting path posted an unexpected number of events"
    );
}

#[test]
fn pointer_actions_reject_without_dispatching() {
    // All counter assertions live in this one test: the counters are
    // process-global, and parallel tests must not interleave with
    // them. CU-01 measured pointer events as ineffective in the
    // background on every family, so the route's terminal state is
    // the capability gate — and no denied call may touch AX or the
    // event seam on the way there.
    let native = MacosNative::new();
    let mut authorizer = TargetAuthorizer::default();
    let app = AppIdentity {
        bundle_id: "com.apple.TextEdit".into(),
        family: AppFamily::Appkit,
    };
    let target = TargetIdentity::application(app);
    let calls_before = AX_ACTION_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    let walks_before = AX_WALK_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    let dead = ProcessInstance {
        pid: 99_999_999,
        start_token: 1,
    };
    let validated = ValidatedObservation {
        window: ValidatedWindow {
            target: target.clone(),
            instance: dead,
            window: WindowIdentity {
                window_id: 999_999_999,
                generation: 1,
            },
            scope: scope(),
        },
        observation: TargetObservation {
            observation_id: serde_json::from_str("\"o-forged\"").unwrap(),
            environment: Environment::NativeBackground,
            window: serde_json::from_str("\"w-forged\"").unwrap(),
            image_width: 800,
            image_height: 600,
            window_width: 1600.0,
            window_height: 1200.0,
        },
        tree_revision: None,
    };
    let click = PointerAction::Click {
        point: crate::target::WindowPoint { x: 10.0, y: 10.0 },
    };
    // Cancellation precedes even the ledger recheck.
    assert_eq!(
        native.pointer_action(&mut authorizer, &validated, &click, &|| true),
        Err(TargetError::Cancelled)
    );
    // A revocation since the consume blocks at the ledger recheck.
    assert_eq!(
        native.pointer_action(&mut authorizer, &validated, &click, &|| false),
        Err(TargetError::NotAuthorized)
    );
    authorizer
        .grant(&scope(), &target, GrantKind::ForRun)
        .unwrap();
    // Payload validity precedes the process check: an out-of-bounds
    // point on a dead process is OutOfBounds, not ProcessRestarted.
    let out_of_bounds = PointerAction::Click {
        point: crate::target::WindowPoint { x: 1600.0, y: 10.0 },
    };
    assert_eq!(
        native.pointer_action(&mut authorizer, &validated, &out_of_bounds, &|| false),
        Err(TargetError::OutOfBounds)
    );
    assert_eq!(
        native.pointer_action(&mut authorizer, &validated, &click, &|| false),
        Err(TargetError::ProcessRestarted)
    );
    // A live process whose bundle id does not match the granted
    // identity is rejected before the capability gate: the test
    // binary owns no bundle id, so a TextEdit grant never unlocks it.
    let own = ProcessInstance {
        pid: std::process::id(),
        start_token: process_start_token(std::process::id() as i32)
            .expect("own process start token"),
    };
    let own_validated = ValidatedObservation {
        window: ValidatedWindow {
            target: target.clone(),
            instance: own,
            window: WindowIdentity {
                window_id: 999_999_999,
                generation: 1,
            },
            scope: scope(),
        },
        observation: validated.observation.clone(),
        tree_revision: None,
    };
    assert_eq!(
        native.pointer_action(&mut authorizer, &own_validated, &click, &|| false),
        Err(TargetError::Invalid(
            "process instance does not belong to the authorized identity"
        ))
    );
    // The capability gate itself (every family rejecting Pointer with
    // BackgroundUnsupported, zero AX / event contact) is pinned by the
    // contract tests and the CU-01 matrix.
    assert_eq!(
        AX_ACTION_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        calls_before,
        "denied pointer actions reached the AX action seam"
    );
    assert_eq!(
        AX_WALK_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        walks_before,
        "denied pointer actions reached the AX walk seam"
    );
}

#[test]
fn move_window_gates_precede_the_position_seam() {
    // All counter assertions live in this one test: the counter is
    // process-global, and parallel tests must not interleave with it.
    let native = MacosNative::new();
    let mut authorizer = TargetAuthorizer::default();
    let app = AppIdentity {
        bundle_id: "com.apple.TextEdit".into(),
        family: AppFamily::Appkit,
    };
    let target = TargetIdentity::application(app);
    let calls_before = AX_ACTION_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    let window = WindowIdentity {
        window_id: 999_999_999,
        generation: 1,
    };
    let dead = ProcessInstance {
        pid: 99_999_999,
        start_token: 1,
    };
    let validated = |instance: ProcessInstance, permit: DispatchPermit| ValidatedWindowDispatch {
        window: ValidatedWindow {
            target: target.clone(),
            instance,
            window,
            scope: scope(),
        },
        dispatch_permit: permit,
    };
    let move_to = WindowMove { x: 100.0, y: 100.0 };
    // A forged permit is rejected before any OS access, even though
    // no grant was ever spent for it.
    let forged = DispatchPermit {
        token: "dp-forged".into(),
    };
    assert_eq!(
        native.move_window(
            &mut authorizer,
            &validated(dead.clone(), forged),
            &move_to,
            &|| false
        ),
        Err(TargetError::NotAuthorized)
    );
    authorizer
        .grant(&scope(), &target, GrantKind::ForRun)
        .unwrap();
    // Payload validity precedes even the process check: a NaN origin
    // on a dead process is Invalid, not ProcessRestarted. Every
    // attempt retires its permit, so each case mints a fresh one.
    let nan_origin = WindowMove {
        x: f64::NAN,
        y: 0.0,
    };
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    assert_eq!(
        native.move_window(
            &mut authorizer,
            &validated(dead.clone(), permit),
            &nan_origin,
            &|| false
        ),
        Err(TargetError::Invalid("window origin must be finite"))
    );
    // A dead process instance: rejected before identity and the seam.
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    assert_eq!(
        native.move_window(
            &mut authorizer,
            &validated(dead.clone(), permit),
            &move_to,
            &|| false
        ),
        Err(TargetError::ProcessRestarted)
    );
    // A live process whose bundle id does not match the granted
    // identity: rejected before the capability gate and the seam. The
    // family half of the gate (a Chromium-family target reaching
    // BackgroundUnsupported without any AX call) is pinned by the
    // contract tests and the CU-01 matrix.
    let own = ProcessInstance {
        pid: std::process::id(),
        start_token: process_start_token(std::process::id() as i32)
            .expect("own process start token"),
    };
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    assert_eq!(
        native.move_window(&mut authorizer, &validated(own, permit), &move_to, &|| {
            false
        }),
        Err(TargetError::Invalid(
            "process instance does not belong to the authorized identity"
        ))
    );
    // The single-permit discipline: the first attempt retired the
    // permit, so the same (cloned) validation result can never
    // dispatch twice; revoking after the consume drops the
    // outstanding permit and blocks the dispatch; cancellation
    // precedes even the ledger check.
    let permit = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    let consumed_once = validated(dead.clone(), permit);
    assert_eq!(
        native.move_window(&mut authorizer, &consumed_once, &move_to, &|| false),
        Err(TargetError::ProcessRestarted)
    );
    assert_eq!(
        native.move_window(&mut authorizer, &consumed_once, &move_to, &|| false),
        Err(TargetError::NotAuthorized)
    );
    let revoked = authorizer.spend_for_dispatch(&scope(), &target).unwrap();
    authorizer.revoke(&scope(), &target);
    assert_eq!(
        native.move_window(
            &mut authorizer,
            &validated(dead.clone(), revoked),
            &move_to,
            &|| false
        ),
        Err(TargetError::NotAuthorized)
    );
    let forged_again = DispatchPermit {
        token: "dp-forged".into(),
    };
    assert_eq!(
        native.move_window(
            &mut authorizer,
            &validated(dead, forged_again),
            &move_to,
            &|| true,
        ),
        Err(TargetError::Cancelled)
    );
    assert_eq!(
        AX_ACTION_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        calls_before,
        "denied window moves reached the AX action seam"
    );
}

#[test]
fn move_minimized_gate_fails_closed_on_unknown_state() {
    // Only an affirmative answer proves anything: Some(false) is the
    // unminimized proof, Some(true) is passed on for require_background
    // to reject, and an unknown AXMinimized rejects outright instead of
    // defaulting the dispatch into the measured matrix. The gate is
    // pure — the action-seam counter must not move.
    let calls_before = AX_ACTION_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(move_minimized_gate(Some(false)), Ok(false));
    assert_eq!(move_minimized_gate(Some(true)), Ok(true));
    assert_eq!(
        move_minimized_gate(None),
        Err(TargetError::ProbeUnavailable(
            "AXMinimized unknown; cannot prove the window unminimized".to_string()
        ))
    );
    assert_eq!(
        AX_ACTION_SYSTEM_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        calls_before,
        "the minimized gate touched the AX action seam"
    );
}

#[test]
fn retained_cf_value_releases_exactly_once_on_every_path() {
    extern "C" {
        fn CFGetRetainCount(cf: *const c_void) -> isize;
    }
    // One extra retain makes the wrapper's single release observable
    // without freeing the string: 2 after the retain, 1 after the
    // wrapper drops. Dropping the CFString afterwards balances its own
    // reference — exactly one release per path, never two.
    let string = CFString::new("cu08-retained-cf-value");
    let raw = string.as_concrete_TypeRef() as *const c_void;
    unsafe { CFRetain(raw) };
    assert_eq!(unsafe { CFGetRetainCount(raw) }, 2);
    drop(RetainedCfValue(raw));
    assert_eq!(
        unsafe { CFGetRetainCount(raw) },
        1,
        "RetainedCfValue did not release exactly once on drop"
    );
    drop(string);
}

#[test]
fn path_resolution_releases_the_child_list_when_the_clock_blows_after_the_fetch() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    extern "C" {
        fn CFArrayCreate(
            allocator: *const c_void,
            values: *const *const c_void,
            count: isize,
            callbacks: *const c_void,
        ) -> *const c_void;
        fn CFGetRetainCount(cf: *const c_void) -> isize;
    }
    // A real CFArray stands in for the AXChildren fetch so the
    // release is observable: the test holds one retain, the fake
    // fetch hands out a second (copy rule), and the blown-clock
    // branch must release it before the budget error propagates.
    static HANDOUT: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn fake_copy_values(
        _element: *mut c_void,
        _attribute: CFStringRef,
        _start: i64,
        _stop: i64,
        out: *mut *const c_void,
    ) -> i32 {
        std::thread::sleep(Duration::from_millis(200));
        let array = HANDOUT.load(Ordering::SeqCst) as *const c_void;
        unsafe {
            CFRetain(array);
            *out = array;
        }
        0
    }
    unsafe extern "C" fn fake_set_timeout(_element: *mut c_void, _seconds: f32) -> i32 {
        0
    }
    autoreleasepool(|| unsafe {
        let child = nsstr("cu06-path-resolution-leak-probe") as *const c_void;
        let values = [child];
        let array = CFArrayCreate(
            ptr::null(),
            values.as_ptr(),
            values.len() as isize,
            ptr::null(),
        );
        assert!(!array.is_null());
        HANDOUT.store(array as usize, Ordering::SeqCst);
        let budget = ReadBudget {
            set_timeout: fake_set_timeout,
            deadline: Instant::now() + Duration::from_millis(50),
        };
        let err = resolve_path_child(fake_copy_values, child as *mut c_void, 0, &budget)
            .expect_err("a blown clock after the fetch must fail the step");
        assert_eq!(
            err,
            TargetError::ProbeUnavailable(
                "accessibility tree read exceeded its time budget".to_string()
            )
        );
        assert_eq!(
            CFGetRetainCount(array),
            1,
            "the blown-clock branch leaked the fetched child list"
        );
        // The dispatch-side liveness read maps the same blown clock
        // to the overtime cause instead of collapsing into
        // WindowReplaced.
        assert_eq!(
            ax_read_target_error(AxReadError::Overtime),
            TargetError::ProbeUnavailable(
                "accessibility tree read exceeded its time budget".to_string()
            )
        );
        CFRelease(array);
    });
}

#[test]
fn ax_absent_attribute_codes_are_answers_not_failures() {
    assert!(ax_attr_absent(K_AX_ERROR_NO_VALUE));
    assert!(ax_attr_absent(K_AX_ERROR_ATTRIBUTE_UNSUPPORTED));
    assert!(!ax_attr_absent(K_AX_ERROR_API_DISABLED));
    // CannotComplete (-25204) and everything else stay failures.
    assert!(!ax_attr_absent(-25204));
    assert!(!ax_attr_absent(0));
}

#[test]
fn tree_read_failures_keep_their_cause() {
    assert_eq!(
        ax_tree_read_error(K_AX_ERROR_API_DISABLED),
        TargetError::PermissionMissing("accessibility")
    );
    assert!(matches!(
        ax_tree_read_error(-25204),
        TargetError::ProbeUnavailable(_)
    ));
}

#[test]
fn secure_fields_yield_no_content_reads() {
    // Legacy nonstandard role spelling.
    assert!(ax_secure("AXSecureTextField", None, false));
    // The SDK-standard shape: role AXTextField + secure subrole.
    assert!(ax_secure("AXTextField", Some("AXSecureTextField"), false));
    // Ordinary text field (no subrole, or a non-secure one).
    assert!(!ax_secure("AXTextField", None, false));
    assert!(!ax_secure(
        "AXTextField",
        Some("AXStandardTextField"),
        false
    ));
    // A failed subrole identification is conservative: secure.
    assert!(ax_secure("AXTextField", None, true));
    assert!(ax_secure("AXTextField", Some("AXSecureTextField"), true));
    // Other roles never get the secure-by-subrole reading.
    assert!(!ax_secure("AXButton", None, false));
    assert!(!ax_secure("AXButton", Some("AXSecureTextField"), false));
    assert!(!ax_secure("AXButton", None, true));
}

#[test]
fn remaining_budget_reports_only_unexpired_time() {
    let future = Instant::now() + Duration::from_secs(5);
    let remaining = remaining_budget(future).expect("unexpired deadline");
    assert!(remaining > 0.0 && remaining <= 5.0);
    let past = Instant::now() - Duration::from_secs(1);
    assert_eq!(remaining_budget(past), None);
}

#[test]
fn read_budget_arms_each_call_with_the_fresh_remaining_budget() {
    use std::sync::Mutex;
    static RECORDED: Mutex<Vec<f32>> = Mutex::new(Vec::new());
    unsafe extern "C" fn recording_set_timeout(_element: *mut c_void, seconds: f32) -> i32 {
        RECORDED.lock().unwrap().push(seconds);
        0
    }
    let budget = ReadBudget {
        set_timeout: recording_set_timeout,
        deadline: Instant::now() + Duration::from_secs(30),
    };
    // Two armed calls through the real call path: each hands the
    // budget remaining when it starts to the timeout entry point.
    assert_eq!(budget.call(ptr::null_mut(), || "first"), Ok("first"));
    assert_eq!(budget.call(ptr::null_mut(), || "second"), Ok("second"));
    let recorded = RECORDED.lock().unwrap();
    assert_eq!(recorded.len(), 2, "recorded {recorded:?}");
    assert!(recorded
        .iter()
        .all(|seconds| *seconds > 0.0 && *seconds <= 30.0));
    // A later call never inherits a stale (larger) budget.
    assert!(recorded[0] >= recorded[1]);
}

#[test]
fn read_budget_refuses_to_start_a_call_after_the_deadline() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLED: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn forbidden_set_timeout(_element: *mut c_void, _seconds: f32) -> i32 {
        CALLED.fetch_add(1, Ordering::SeqCst);
        0
    }
    let budget = ReadBudget {
        set_timeout: forbidden_set_timeout,
        deadline: Instant::now() - Duration::from_secs(1),
    };
    assert_eq!(budget.arm(ptr::null_mut()), Err(BudgetError::Overtime));
    assert_eq!(
        budget.call(ptr::null_mut(), || ()),
        Err(BudgetError::Overtime)
    );
    assert_eq!(budget.check(), Err(BudgetError::Overtime));
    // A blown clock refuses BEFORE arming: the timeout entry point
    // never ran, so no unbounded AX call was issued.
    assert_eq!(CALLED.load(Ordering::SeqCst), 0);
}

#[test]
fn read_budget_set_failure_refuses_the_read_with_its_cause() {
    unsafe extern "C" fn failing_set_timeout(_element: *mut c_void, _seconds: f32) -> i32 {
        K_AX_ERROR_API_DISABLED
    }
    let budget = ReadBudget {
        set_timeout: failing_set_timeout,
        deadline: Instant::now() + Duration::from_secs(30),
    };
    assert_eq!(
        budget.arm(ptr::null_mut()),
        Err(BudgetError::SetFailed(K_AX_ERROR_API_DISABLED))
    );
    // The tree-read mapping keeps the permission cause; the
    // window-list mapping keeps the raw code.
    assert!(matches!(
        budget_target_error(BudgetError::SetFailed(K_AX_ERROR_API_DISABLED)),
        TargetError::PermissionMissing("accessibility")
    ));
    assert_eq!(
        budget_read_error(BudgetError::SetFailed(K_AX_ERROR_API_DISABLED)),
        AxReadError::ReadFailed(K_AX_ERROR_API_DISABLED)
    );
    // Every other set failure is a probe failure, not a permission
    // report (kAXErrorFailure, -25200).
    assert!(matches!(
        budget_target_error(BudgetError::SetFailed(-25200)),
        TargetError::ProbeUnavailable(_)
    ));
    assert_eq!(
        budget_read_error(BudgetError::Overtime),
        AxReadError::Overtime
    );
}

#[test]
fn text_truncation_cuts_on_char_boundaries() {
    assert_eq!(truncate_chars("short", 10), ("short".to_string(), false));
    assert_eq!(
        truncate_chars("abcdefghij", 10),
        ("abcdefghij".to_string(), false)
    );
    assert_eq!(
        truncate_chars("abcdefghijklm", 5),
        ("abcde".to_string(), true)
    );
    // Multibyte characters are never split.
    assert_eq!(
        truncate_chars("中文字符串", 3),
        ("中文字".to_string(), true)
    );
}
