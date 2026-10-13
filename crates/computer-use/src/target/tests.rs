//! Target-contract rejection tests.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Mutex;

struct FakeProbe {
    alive: AtomicBool,
    generation: Mutex<Option<u64>>,
    tree: Mutex<Option<u64>>,
    origin: Mutex<Option<String>>,
    size: Mutex<Option<(f64, f64)>>,
    calls: AtomicUsize,
}

impl FakeProbe {
    fn new() -> Self {
        Self {
            alive: AtomicBool::new(true),
            generation: Mutex::new(Some(1)),
            tree: Mutex::new(Some(7)),
            origin: Mutex::new(None),
            // Matches geometry() below: the window is live at the size
            // observations record.
            size: Mutex::new(Some((2560.0, 1440.0))),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl NativeProbe for FakeProbe {
    fn process_instance_alive(&self, _: &ProcessInstance) -> bool {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.alive.load(Ordering::Relaxed)
    }
    fn window_generation(&self, _: &ProcessInstance, _: u64) -> Option<u64> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        *self.generation.lock().unwrap()
    }
    fn tree_revision(&self, _: &WindowIdentity) -> Option<u64> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        *self.tree.lock().unwrap()
    }
    fn current_origin(&self, _: &WindowIdentity) -> Option<String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.origin.lock().unwrap().clone()
    }
    fn window_size(&self, _: &ProcessInstance, _: u64) -> Option<(f64, f64)> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        *self.size.lock().unwrap()
    }
}

fn scope() -> Scope {
    Scope::new("ws-1", "run-1")
}

fn instance() -> ProcessInstance {
    ProcessInstance {
        pid: 4242,
        start_token: 99,
    }
}

fn app(family: AppFamily) -> AppIdentity {
    AppIdentity {
        bundle_id: "com.example.app".into(),
        family,
    }
}

fn target(family: AppFamily) -> TargetIdentity {
    TargetIdentity::application(app(family))
}

fn website_target() -> TargetIdentity {
    TargetIdentity::website(
        AppIdentity {
            bundle_id: "com.example.browser".into(),
            family: AppFamily::Browser,
        },
        "https://example.com",
    )
    .unwrap()
}

fn window_identity() -> WindowIdentity {
    WindowIdentity {
        window_id: 42,
        generation: 1,
    }
}

fn bound(family: AppFamily) -> (TargetRegistry<FakeProbe>, WindowHandle) {
    let mut registry = TargetRegistry::new(FakeProbe::new());
    let target = target(family);
    registry.open_scope(&scope());
    registry
        .grant(&scope(), &target, GrantKind::ForRun)
        .unwrap();
    let window = registry
        .bind_window(&scope(), target, instance(), window_identity())
        .unwrap();
    (registry, window)
}

fn forged_window() -> WindowHandle {
    serde_json::from_str("\"w-forged\"").unwrap()
}

fn geometry() -> ObservationGeometry {
    ObservationGeometry {
        image_width: 1280,
        image_height: 720,
        window_width: 2560.0,
        window_height: 1440.0,
    }
}

fn capture(registry: &mut TargetRegistry<FakeProbe>, window: &WindowHandle) -> TargetObservation {
    registry
        .begin_observation(
            window,
            &scope(),
            Environment::NativeBackground,
            ObservationKind::Capture,
            geometry(),
        )
        .unwrap()
}

fn ax_observation(
    registry: &mut TargetRegistry<FakeProbe>,
    window: &WindowHandle,
) -> TargetObservation {
    registry
        .begin_observation(
            window,
            &scope(),
            Environment::NativeBackground,
            ObservationKind::AxTree { tree_revision: 7 },
            geometry(),
        )
        .unwrap()
}

#[test]
fn capability_matrix_matches_cu01_measurements() {
    use AppFamily::*;
    use Capability::*;
    use NativeActionKind::*;
    for family in [Appkit, Chromium, Browser] {
        for minimized in [false, true] {
            assert_eq!(
                background_capability(family, CaptureWindow, minimized),
                Supported
            );
            assert_eq!(background_capability(family, AxRead, minimized), Supported);
            // Pointer has no measured background support anywhere.
            assert_eq!(
                background_capability(family, Pointer, minimized),
                Unsupported
            );
        }
        // Window move/resize was only measured in background/occluded
        // states; minimized is unverified everywhere.
        assert_eq!(
            background_capability(family, WindowResize, false),
            Supported
        );
        assert_eq!(
            background_capability(family, WindowResize, true),
            Unverified
        );
    }
    // AppKit targeted keys work in background/occluded, not minimized.
    assert_eq!(
        background_capability(Appkit, TargetedKeys, false),
        Supported
    );
    assert_eq!(
        background_capability(Appkit, TargetedKeys, true),
        Unsupported
    );
    assert_eq!(
        background_capability(Chromium, TargetedKeys, false),
        Unsupported
    );
    assert_eq!(
        background_capability(Browser, TargetedKeys, false),
        Unsupported
    );
    // Semantic write splits by family; unverified is not supported.
    assert_eq!(
        background_capability(Appkit, AxSemanticWrite, false),
        Supported
    );
    assert_eq!(
        background_capability(Chromium, AxSemanticWrite, false),
        Unsupported
    );
    assert_eq!(
        background_capability(Browser, AxSemanticWrite, false),
        Unverified
    );
    // CU-01 proved AXPress only on web pages; AppKit showed value writes,
    // not presses.
    assert_eq!(background_capability(Appkit, AxPress, false), Unverified);
    assert_eq!(background_capability(Chromium, AxPress, false), Unverified);
    assert_eq!(background_capability(Browser, AxPress, false), Supported);
    // Menu shortcuts do not route in background; menu-bar press is
    // unverified for Chromium/Browser.
    assert_eq!(
        background_capability(Appkit, MenuShortcut, false),
        Unsupported
    );
    assert_eq!(
        background_capability(Chromium, MenuShortcut, false),
        Unverified
    );
    assert_eq!(
        background_capability(Appkit, MenubarPress, false),
        Unsupported
    );
    // Chromium ignores AXPosition but honors AXSize; minimized move is
    // unverified even on AppKit.
    assert_eq!(background_capability(Appkit, WindowMove, false), Supported);
    assert_eq!(background_capability(Appkit, WindowMove, true), Unverified);
    assert_eq!(
        background_capability(Chromium, WindowMove, false),
        Unsupported
    );
    assert_eq!(
        background_capability(Browser, WindowMove, false),
        Unsupported
    );
}

#[test]
fn require_background_passes_only_supported_never_falling_back() {
    use AppFamily::*;
    use NativeActionKind::*;
    assert!(require_background(Appkit, TargetedKeys, false).is_ok());
    assert!(require_background(Browser, AxPress, false).is_ok());
    for (family, action, minimized) in [
        (Appkit, Pointer, false),
        (Appkit, TargetedKeys, true),
        (Appkit, AxPress, false),
        (Appkit, WindowMove, true),
        (Appkit, WindowResize, true),
        (Chromium, AxSemanticWrite, false),
        (Chromium, TargetedKeys, false),
        (Browser, AxSemanticWrite, false), // unverified still rejects
        (Browser, Pointer, false),
    ] {
        assert_eq!(
            require_background(family, action, minimized),
            Err(TargetError::BackgroundUnsupported)
        );
    }
}

#[test]
fn semantic_actions_validate_and_map_to_capability_kinds() {
    assert_eq!(SemanticAction::Press.kind(), NativeActionKind::AxPress);
    assert_eq!(
        SemanticAction::SetValue("x".into()).kind(),
        NativeActionKind::AxSemanticWrite
    );
    assert_eq!(
        SemanticAction::InsertText("x".into()).kind(),
        NativeActionKind::AxSemanticWrite
    );
    assert!(!SemanticAction::Press.is_value_write());
    assert!(SemanticAction::SetValue(String::new()).is_value_write());
    // Empty text is a real payload (clearing a field).
    assert!(SemanticAction::SetValue(String::new()).validate().is_ok());
    assert!(SemanticAction::Press.validate().is_ok());
    // The bound counts chars, not bytes: 4096 three-byte chars pass.
    let at_bound: String = "漢".repeat(MAX_SEMANTIC_TEXT_CHARS);
    assert_eq!(at_bound.len(), MAX_SEMANTIC_TEXT_CHARS * 3);
    assert!(SemanticAction::InsertText(at_bound).validate().is_ok());
    let over_bound: String = "x".repeat(MAX_SEMANTIC_TEXT_CHARS + 1);
    assert_eq!(
        SemanticAction::SetValue(over_bound).validate(),
        Err(TargetError::Invalid(
            "semantic action text exceeds its bound"
        ))
    );
}

#[test]
fn semantic_capability_gate_is_the_matrix_and_minimized_invariant() {
    use AppFamily::*;
    // Every semantic action passes only the CU-01 measured cells:
    // AppKit value writes and Browser presses. Unverified rejects
    // exactly like Unsupported; there is no fallback to global input.
    let press = SemanticAction::Press;
    let set_value = SemanticAction::SetValue("x".into());
    let insert = SemanticAction::InsertText("x".into());
    for action in [&set_value, &insert] {
        assert!(require_background(Appkit, action.kind(), false).is_ok());
        assert_eq!(
            require_background(Chromium, action.kind(), false),
            Err(TargetError::BackgroundUnsupported)
        );
        assert_eq!(
            require_background(Browser, action.kind(), false),
            Err(TargetError::BackgroundUnsupported)
        );
    }
    assert_eq!(
        require_background(Appkit, press.kind(), false),
        Err(TargetError::BackgroundUnsupported)
    );
    assert_eq!(
        require_background(Chromium, press.kind(), false),
        Err(TargetError::BackgroundUnsupported)
    );
    assert!(require_background(Browser, press.kind(), false).is_ok());
    // CU-01 measured both semantic kinds in background AND minimized
    // states with identical support, so a dispatch gate does not consult
    // the window's minimized state for them. If a future measurement
    // splits a cell by minimized state, this pin breaks and the backend
    // must read the real state instead of passing false.
    for family in [Appkit, Chromium, Browser] {
        for kind in [NativeActionKind::AxPress, NativeActionKind::AxSemanticWrite] {
            assert_eq!(
                background_capability(family, kind, true),
                background_capability(family, kind, false)
            );
        }
    }
}

#[test]
fn targeted_inputs_validate_and_map_to_the_capability_kind() {
    let text = TargetedInput::InsertText("hello 漢😀".into());
    let key = TargetedInput::KeyPress(TargetedKeyPress {
        key: TargetedKey::Return,
        modifiers: KeyModifiers::none(),
    });
    assert_eq!(text.kind(), NativeActionKind::TargetedKeys);
    assert_eq!(key.kind(), NativeActionKind::TargetedKeys);
    assert!(text.validate().is_ok());
    assert!(key.validate().is_ok());
    // Empty text is not a payload here: unlike a semantic value write
    // (where empty clears a field), targeted text types, and typing
    // nothing is not an input.
    assert_eq!(
        TargetedInput::InsertText(String::new()).validate(),
        Err(TargetError::Invalid(
            "targeted text must be 1..4096 chars without control characters; use a key press for Return/Tab"
        ))
    );
    // Control characters are key presses, not text.
    assert_eq!(
        TargetedInput::InsertText("a\nb".into()).validate(),
        Err(TargetError::Invalid(
            "targeted text must be 1..4096 chars without control characters; use a key press for Return/Tab"
        ))
    );
    // The bound counts chars, not bytes.
    let at_bound: String = "漢".repeat(MAX_TARGETED_TEXT_CHARS);
    assert!(TargetedInput::InsertText(at_bound).validate().is_ok());
    assert_eq!(
        TargetedInput::InsertText("x".repeat(MAX_TARGETED_TEXT_CHARS + 1)).validate(),
        Err(TargetError::Invalid(
            "targeted text must be 1..4096 chars without control characters; use a key press for Return/Tab"
        ))
    );
    // The typed vocabulary admits no invalid values by construction;
    // every key and modifier combination validates.
    for key in [
        TargetedKey::Return,
        TargetedKey::Tab,
        TargetedKey::Space,
        TargetedKey::Backspace,
        TargetedKey::Escape,
        TargetedKey::Delete,
        TargetedKey::Home,
        TargetedKey::End,
        TargetedKey::PageUp,
        TargetedKey::PageDown,
        TargetedKey::Left,
        TargetedKey::Up,
        TargetedKey::Right,
        TargetedKey::Down,
    ] {
        assert!(TargetedInput::KeyPress(TargetedKeyPress {
            key,
            modifiers: KeyModifiers {
                shift: true,
                control: true,
                option: true,
            },
        })
        .validate()
        .is_ok());
    }
}

#[test]
fn targeted_capability_gate_is_the_measured_matrix() {
    use AppFamily::*;
    let kind = TargetedInput::InsertText("x".into()).kind();
    // The CU-01 measured cells: AppKit background yes (occluded or
    // plain background, with the window the app's key window), AppKit
    // minimized no (silently dropped), Chromium-family background no
    // (silently dropped even with AXMain restored). No family has a
    // fallback to global or foreground input.
    assert!(require_background(Appkit, kind, false).is_ok());
    assert_eq!(
        require_background(Appkit, kind, true),
        Err(TargetError::BackgroundUnsupported)
    );
    for family in [Chromium, Browser] {
        for minimized in [false, true] {
            assert_eq!(
                require_background(family, kind, minimized),
                Err(TargetError::BackgroundUnsupported)
            );
        }
    }
    // Menu shortcuts stay a separate, rejected kind: a Command combo is
    // not expressible as a targeted key press, and the matrix cell it
    // maps to was measured not to route in the background.
    assert_eq!(
        require_background(Appkit, NativeActionKind::MenuShortcut, false),
        Err(TargetError::BackgroundUnsupported)
    );
}

#[test]
fn pointer_actions_validate_against_observed_bounds() {
    let point = |x, y| WindowPoint { x, y };
    let click = PointerAction::Click {
        point: point(10.0, 20.0),
    };
    assert_eq!(click.kind(), NativeActionKind::Pointer);
    assert!(click.validate(2560.0, 1440.0).is_ok());
    // Points share image_to_window's edge-excluded bounds; NaN and
    // infinities are coordinate errors, not in-bounds values.
    for bad in [
        point(2560.0, 20.0),
        point(10.0, 1440.0),
        point(-0.1, 20.0),
        point(10.0, -1.0),
        point(f64::NAN, 20.0),
        point(10.0, f64::INFINITY),
    ] {
        assert_eq!(
            PointerAction::Click { point: bad }.validate(2560.0, 1440.0),
            Err(TargetError::OutOfBounds)
        );
    }
    // A drag needs both endpoints inside the observed window.
    let drag = PointerAction::Drag {
        from: point(0.0, 0.0),
        to: point(2559.9, 1439.9),
    };
    assert!(drag.validate(2560.0, 1440.0).is_ok());
    assert_eq!(
        PointerAction::Drag {
            from: point(0.0, 0.0),
            to: point(2560.0, 1439.9),
        }
        .validate(2560.0, 1440.0),
        Err(TargetError::OutOfBounds)
    );
    // Scroll deltas must be finite.
    let scroll = PointerAction::Scroll {
        point: point(10.0, 20.0),
        delta_x: -3.0,
        delta_y: 0.0,
    };
    assert!(scroll.validate(2560.0, 1440.0).is_ok());
    assert_eq!(
        PointerAction::Scroll {
            point: point(10.0, 20.0),
            delta_x: f64::NAN,
            delta_y: 0.0,
        }
        .validate(2560.0, 1440.0),
        Err(TargetError::Invalid("scroll deltas must be finite"))
    );
}

#[test]
fn window_moves_require_finite_origins() {
    let ok = WindowMove {
        x: -1300.0,
        y: 200.0,
    };
    assert_eq!(ok.kind(), NativeActionKind::WindowMove);
    assert!(ok.validate().is_ok());
    // Negative global coordinates are legal (a display left of the
    // primary); the origin is never clamped, only finiteness-checked.
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            WindowMove { x: bad, y: 0.0 }.validate(),
            Err(TargetError::Invalid("window origin must be finite"))
        );
        assert_eq!(
            WindowMove { x: 0.0, y: bad }.validate(),
            Err(TargetError::Invalid("window origin must be finite"))
        );
    }
}

#[test]
fn window_dispatch_mints_one_permit_per_consume() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    // Forged handles never reach the ledger.
    assert_eq!(
        registry.consume_window_dispatch(&forged_window(), &scope(), &|| false),
        Err(TargetError::UnknownHandle)
    );
    // Cross-run rejection does not mint anything.
    assert_eq!(
        registry.consume_window_dispatch(&window, &Scope::new("ws-1", "run-2"), &|| false),
        Err(TargetError::CrossRun)
    );
    // Cancellation precedes the ledger: nothing is spent.
    assert_eq!(
        registry.consume_window_dispatch(&window, &scope(), &|| true),
        Err(TargetError::Cancelled)
    );
    // A user takeover pauses the dispatch without consuming.
    registry.pause_dispatch(&target(AppFamily::Appkit));
    assert_eq!(
        registry.consume_window_dispatch(&window, &scope(), &|| false),
        Err(TargetError::UserActive)
    );
    assert!(registry.resume_dispatch(&target(AppFamily::Appkit)));
    // The window binding is not a lease: each consume mints an
    // independent single-use permit.
    let first = registry
        .consume_window_dispatch(&window, &scope(), &|| false)
        .unwrap();
    let second = registry
        .consume_window_dispatch(&window, &scope(), &|| false)
        .unwrap();
    assert_ne!(first.dispatch_permit, second.dispatch_permit);
    assert_eq!(first.window, second.window);
    // A one-shot grant is spent by the first consume only.
    let (mut once_registry, once_window) = {
        let mut registry = TargetRegistry::new(FakeProbe::new());
        registry.open_scope(&scope());
        registry
            .grant(&scope(), &target(AppFamily::Appkit), GrantKind::Once)
            .unwrap();
        let window = registry
            .bind_window(
                &scope(),
                target(AppFamily::Appkit),
                instance(),
                window_identity(),
            )
            .unwrap();
        (registry, window)
    };
    assert!(once_registry
        .consume_window_dispatch(&once_window, &scope(), &|| false)
        .is_ok());
    assert_eq!(
        once_registry.consume_window_dispatch(&once_window, &scope(), &|| false),
        Err(TargetError::NotAuthorized)
    );
    // Revocation after a consume drops the outstanding permit, so the
    // dispatch it authorized is blocked at the seam.
    registry.revoke(&scope(), &target(AppFamily::Appkit));
    assert_eq!(
        registry.authorizer.consume_dispatch_permit(
            &scope(),
            &target(AppFamily::Appkit),
            &first.dispatch_permit
        ),
        Err(TargetError::NotAuthorized)
    );
}

#[test]
fn observation_is_one_shot_and_scope_bound() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let observation = capture(&mut registry, &window);
    assert_eq!(observation.environment, Environment::NativeBackground);
    registry
        .consume_observation(&observation.observation_id, &scope(), &|| false)
        .unwrap();
    // Second use of the same lease.
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::StaleHandle)
    );
    // Fresh observation belongs to its issuing run only.
    let observation = capture(&mut registry, &window);
    for other in [Scope::new("ws-1", "run-2"), Scope::new("ws-2", "run-1")] {
        assert_eq!(
            registry.consume_observation(&observation.observation_id, &other, &|| false),
            Err(TargetError::CrossRun)
        );
    }
    // Cross-run rejection did not consume it.
    registry
        .consume_observation(&observation.observation_id, &scope(), &|| false)
        .unwrap();
}

#[test]
fn second_run_conflicts_on_the_same_application_until_released() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let other = Scope::new("ws-1", "run-2");
    registry.open_scope(&other);
    registry
        .grant(&other, &target(AppFamily::Appkit), GrantKind::ForRun)
        .unwrap();
    // Same application: a visible conflict, not shared observations.
    assert_eq!(
        registry.bind_window(
            &other,
            target(AppFamily::Appkit),
            instance(),
            window_identity()
        ),
        Err(TargetError::TargetOccupied)
    );
    // A different application is a different physical boundary.
    let notes = TargetIdentity::application(AppIdentity {
        bundle_id: "com.example.notes".into(),
        family: AppFamily::Appkit,
    });
    registry.open_scope(&other);
    registry.grant(&other, &notes, GrantKind::ForRun).unwrap();
    assert!(registry
        .bind_window(&other, notes, instance(), window_identity())
        .is_ok());
    // Websites contend on their browser process, even across origins.
    let browser = AppIdentity {
        bundle_id: "com.example.browser".into(),
        family: AppFamily::Browser,
    };
    let site_a = TargetIdentity::website(browser.clone(), "https://a.example").unwrap();
    let site_b = TargetIdentity::website(browser, "https://b.example").unwrap();
    registry.open_scope(&scope());
    registry
        .grant(&scope(), &site_a, GrantKind::ForRun)
        .unwrap();
    *registry.probe.origin.lock().unwrap() = Some("https://a.example".into());
    assert!(registry
        .bind_window(&scope(), site_a, instance(), window_identity())
        .is_ok());
    registry.grant(&other, &site_b, GrantKind::ForRun).unwrap();
    assert_eq!(
        registry.bind_window(&other, site_b, instance(), window_identity()),
        Err(TargetError::TargetOccupied)
    );
    // The owner keeps exclusive use: its observation still dispatches.
    let observation = capture(&mut registry, &window);
    registry
        .consume_observation(&observation.observation_id, &scope(), &|| false)
        .unwrap();
    // Run teardown releases occupancy and invalidates the owner's handles.
    registry.release_scope(&scope());
    assert_eq!(
        registry.validate_window(&window, &scope()),
        Err(TargetError::UnknownHandle)
    );
    assert!(registry
        .bind_window(
            &other,
            target(AppFamily::Appkit),
            instance(),
            window_identity()
        )
        .is_ok());
}

#[test]
fn idle_occupancy_lapses_for_the_next_run() {
    let (mut registry, _window) = bound(AppFamily::Appkit);
    let other = Scope::new("ws-1", "run-2");
    registry.open_scope(&other);
    registry
        .grant(&other, &target(AppFamily::Appkit), GrantKind::ForRun)
        .unwrap();
    // An owner idle past LEASE_TTL is treated as done: its freshest
    // observation is expired at the same moment.
    if let Some((_, active)) = registry.occupied.get_mut("com.example.app") {
        *active = Instant::now() - LEASE_TTL - Duration::from_secs(1);
    }
    assert!(registry
        .bind_window(
            &other,
            target(AppFamily::Appkit),
            instance(),
            window_identity()
        )
        .is_ok());
}

#[test]
fn pre_flight_rejects_a_taken_over_application_before_any_probe_call() {
    let mut registry = TargetRegistry::new(FakeProbe::new());
    let browser = AppIdentity {
        bundle_id: "com.example.browser".into(),
        family: AppFamily::Browser,
    };
    let site = TargetIdentity::website(browser.clone(), "https://example.com").unwrap();
    registry.open_scope(&scope());
    registry.grant(&scope(), &site, GrantKind::ForRun).unwrap();
    *registry.probe.origin.lock().unwrap() = Some("https://example.com".into());
    let window = registry
        .bind_window(&scope(), site, instance(), window_identity())
        .unwrap();
    let other = Scope::new("ws-1", "run-2");
    let site_other = TargetIdentity::website(browser, "https://other.example").unwrap();
    registry.open_scope(&other);
    registry
        .grant(&other, &site_other, GrantKind::ForRun)
        .unwrap();
    *registry.probe.origin.lock().unwrap() = Some("https://other.example".into());
    // The owner goes idle past LEASE_TTL and the next run takes over
    // the shared browser boundary.
    if let Some((_, active)) = registry.occupied.get_mut("com.example.browser") {
        *active = Instant::now() - LEASE_TTL - Duration::from_secs(1);
    }
    registry
        .bind_window(&other, site_other, instance(), window_identity())
        .unwrap();
    // The lapsed run's pre-flight hits the occupancy gate before the
    // origin probe: the host must not read content another run owns.
    let calls = registry.probe.calls();
    assert_eq!(
        registry.require_authorized(&window, &scope()),
        Err(TargetError::TargetOccupied)
    );
    assert_eq!(registry.probe.calls(), calls);
}

#[test]
fn failed_binding_leaves_no_occupancy_behind() {
    let mut registry = TargetRegistry::new(FakeProbe::new());
    let editor = target(AppFamily::Appkit);
    registry.open_scope(&scope());
    registry
        .grant(&scope(), &editor, GrantKind::ForRun)
        .unwrap();
    // The process is already gone: the rejected binding must not strand
    // occupancy another run would have to wait out.
    registry.probe.alive.store(false, Ordering::Relaxed);
    assert_eq!(
        registry.bind_window(&scope(), editor.clone(), instance(), window_identity()),
        Err(TargetError::ProcessRestarted)
    );
    assert!(registry.occupied.get("com.example.app").is_none());
    let other = Scope::new("ws-1", "run-2");
    registry.open_scope(&other);
    registry.grant(&other, &editor, GrantKind::ForRun).unwrap();
    registry.probe.alive.store(true, Ordering::Relaxed);
    assert!(registry
        .bind_window(&other, editor, instance(), window_identity())
        .is_ok());
}

#[test]
fn user_takeover_pauses_dispatch_without_consuming() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let ax = ax_observation(&mut registry, &window);
    let element = registry
        .issue_element(&ax.observation_id, &scope(), 1)
        .unwrap();
    let still = capture(&mut registry, &window);
    registry.pause_dispatch(&target(AppFamily::Appkit));
    // Dispatch gates yield to the user; neither lease is consumed.
    assert_eq!(
        registry.consume_observation(&ax.observation_id, &scope(), &|| false),
        Err(TargetError::UserActive)
    );
    assert_eq!(
        registry.consume_element(&element, &scope(), &|| false),
        Err(TargetError::UserActive)
    );
    // Observing stays available: the recovery flow re-observes after the
    // user acted instead of dispatching against a stale view.
    assert!(registry
        .begin_observation(
            &window,
            &scope(),
            Environment::NativeBackground,
            ObservationKind::Capture,
            geometry(),
        )
        .is_ok());
    // The user yields back: paused leases dispatch again.
    assert!(registry.resume_dispatch(&target(AppFamily::Appkit)));
    assert!(!registry.resume_dispatch(&target(AppFamily::Appkit)));
    registry
        .consume_element(&element, &scope(), &|| false)
        .unwrap();
    registry
        .consume_observation(&still.observation_id, &scope(), &|| false)
        .unwrap();
}

#[test]
fn revocation_releases_occupancy_while_leases_block() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let observation = capture(&mut registry, &window);
    registry.revoke(&scope(), &target(AppFamily::Appkit));
    // CU-16 semantics stay: the lease is blocked by authorization first.
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::NotAuthorized)
    );
    // CU-09: the application boundary is free for the next run.
    let other = Scope::new("ws-1", "run-2");
    registry.open_scope(&other);
    registry
        .grant(&other, &target(AppFamily::Appkit), GrantKind::ForRun)
        .unwrap();
    assert!(registry
        .bind_window(
            &other,
            target(AppFamily::Appkit),
            instance(),
            window_identity()
        )
        .is_ok());
}

#[test]
fn forged_and_unknown_handles_are_rejected() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    // The model path: arbitrary strings decode as handles but the registry
    // only accepts what it issued.
    let forged_window: WindowHandle = serde_json::from_str("\"w-1-1\"").unwrap();
    let forged_observation: ObservationHandle = serde_json::from_str("\"o-1-2\"").unwrap();
    let forged_element: ElementHandle = serde_json::from_str("\"e-1-3\"").unwrap();
    assert_eq!(
        registry.validate_window(&forged_window, &scope()),
        Err(TargetError::UnknownHandle)
    );
    assert_eq!(
        registry.consume_observation(&forged_observation, &scope(), &|| false),
        Err(TargetError::UnknownHandle)
    );
    assert_eq!(
        registry.consume_element(&forged_element, &scope(), &|| false),
        Err(TargetError::UnknownHandle)
    );
    // Handles round-trip through JSON as opaque strings.
    let encoded = serde_json::to_string(&window).unwrap();
    assert_eq!(
        serde_json::from_str::<WindowHandle>(&encoded).unwrap(),
        window
    );
}

#[test]
fn locked_use_environment_is_rejected_by_the_contract() {
    // CU-24 boundary pin: the wire contract cannot express a locked-screen
    // environment. Contract simulation only, not macOS authorization or
    // real locked-screen validation (docs/plan/cu-24-macos-locked-use.md).
    assert_eq!(
        serde_json::to_value(Environment::IsolatedDesktop).unwrap(),
        serde_json::json!("isolated_desktop")
    );
    assert_eq!(
        serde_json::to_value(Environment::NativeBackground).unwrap(),
        serde_json::json!("native_background")
    );

    let observation = serde_json::json!({
        "observation_id": "o-1-1",
        "environment": "native_background",
        "window": "w-1-1",
        "image_width": 1280,
        "image_height": 720,
        "window_width": 2560.0,
        "window_height": 1440.0,
    });
    let decoded: TargetObservation = serde_json::from_value(observation.clone()).unwrap();
    assert_eq!(decoded.environment, Environment::NativeBackground);

    let mut locked = observation;
    locked["environment"] = serde_json::json!("locked_use");
    assert!(serde_json::from_value::<TargetObservation>(locked).is_err());
}

#[test]
fn handles_from_a_previous_host_instance_are_unknown() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    // Handles embed a process-level startup namespace (pid + nanos), so a
    // value minted before a host restart can never collide with a newly
    // issued one, even under pid reuse.
    let parts: Vec<&str> = window.as_str().split('-').collect();
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[0], "w");
    assert_eq!(parts[1], std::process::id().to_string());
    assert!(parts[2].parse::<u128>().is_ok());
    assert!(parts[3].parse::<u64>().is_ok());
    // A same-pid handle from an earlier instance (different namespace,
    // same scope) must not hit this instance's records.
    let previous: WindowHandle =
        serde_json::from_str(&format!("\"w-{}-1-1\"", std::process::id())).unwrap();
    assert_eq!(
        registry.validate_window(&previous, &scope()),
        Err(TargetError::UnknownHandle)
    );
    let previous_observation: ObservationHandle =
        serde_json::from_str(&format!("\"o-{}-1-1\"", std::process::id())).unwrap();
    assert_eq!(
        registry.consume_observation(&previous_observation, &scope(), &|| false),
        Err(TargetError::UnknownHandle)
    );
}

#[test]
fn process_exit_or_restart_invalidates_everything() {
    let mut registry = TargetRegistry::new(FakeProbe::new());
    let target = target(AppFamily::Appkit);
    registry.open_scope(&scope());
    registry
        .grant(&scope(), &target, GrantKind::ForRun)
        .unwrap();
    // A dead process cannot be bound at all.
    registry.probe.alive.store(false, Ordering::Relaxed);
    assert_eq!(
        registry.bind_window(&scope(), target.clone(), instance(), window_identity()),
        Err(TargetError::ProcessRestarted)
    );
    registry.probe.alive.store(true, Ordering::Relaxed);
    let window = registry
        .bind_window(&scope(), target, instance(), window_identity())
        .unwrap();
    let observation = capture(&mut registry, &window);
    // Restart (or exit): the instance token no longer matches.
    registry.probe.alive.store(false, Ordering::Relaxed);
    assert_eq!(
        registry.validate_window(&window, &scope()),
        Err(TargetError::ProcessRestarted)
    );
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::ProcessRestarted)
    );
}

#[test]
fn window_destroyed_or_identifier_reused_invalidates() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let observation = capture(&mut registry, &window);
    // Destroyed.
    *registry.probe.generation.lock().unwrap() = None;
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::WindowReplaced)
    );
    assert_eq!(
        registry.begin_observation(
            &window,
            &scope(),
            Environment::NativeBackground,
            ObservationKind::Capture,
            geometry(),
        ),
        Err(TargetError::WindowReplaced)
    );
    // Identifier reused by a new window: the generation moved on.
    *registry.probe.generation.lock().unwrap() = Some(2);
    assert_eq!(
        registry.validate_window(&window, &scope()),
        Err(TargetError::WindowReplaced)
    );
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::WindowReplaced)
    );
}

#[test]
fn tree_change_invalidates_element_handles() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let observation = ax_observation(&mut registry, &window);
    let element = registry
        .issue_element(&observation.observation_id, &scope(), 11)
        .unwrap();
    *registry.probe.tree.lock().unwrap() = Some(8);
    // The AX observation itself is bound to the tree revision it read.
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::TreeChanged)
    );
    assert_eq!(
        registry.consume_element(&element, &scope(), &|| false),
        Err(TargetError::TreeChanged)
    );
    // A fresh element from a fresh read at the new revision works.
    let observation = registry
        .begin_observation(
            &window,
            &scope(),
            Environment::NativeBackground,
            ObservationKind::AxTree { tree_revision: 8 },
            geometry(),
        )
        .unwrap();
    let element = registry
        .issue_element(&observation.observation_id, &scope(), 12)
        .unwrap();
    let validated = registry
        .consume_element(&element, &scope(), &|| false)
        .unwrap();
    assert_eq!(validated.element_token, 12);
    assert_eq!(validated.tree_revision, 8);
    // The tree read disappearing entirely is a window-level failure.
    let observation = ax_observation(&mut registry, &window);
    let element = registry
        .issue_element(&observation.observation_id, &scope(), 13)
        .unwrap();
    *registry.probe.tree.lock().unwrap() = None;
    assert_eq!(
        registry.consume_element(&element, &scope(), &|| false),
        Err(TargetError::WindowReplaced)
    );
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::WindowReplaced)
    );
    // A screenshot observation carries no tree binding and is unaffected
    // by tree changes.
    let observation = capture(&mut registry, &window);
    registry
        .consume_observation(&observation.observation_id, &scope(), &|| false)
        .unwrap();
}

#[test]
fn consuming_any_handle_invalidates_its_siblings() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let observation = ax_observation(&mut registry, &window);
    let first = registry
        .issue_element(&observation.observation_id, &scope(), 21)
        .unwrap();
    let second = registry
        .issue_element(&observation.observation_id, &scope(), 22)
        .unwrap();
    let validated = registry
        .consume_element(&first, &scope(), &|| false)
        .unwrap();
    // The consume minted a single-use dispatch permit bound to this
    // scope and target: the first dispatch attempt retires it, so a
    // cloned validation result can never dispatch twice.
    let permit = validated.dispatch_permit.clone();
    let authorizer = registry.authorizer_mut();
    assert!(authorizer
        .consume_dispatch_permit(&scope(), &target(AppFamily::Appkit), &permit)
        .is_ok());
    assert_eq!(
        authorizer.consume_dispatch_permit(&scope(), &target(AppFamily::Appkit), &permit),
        Err(TargetError::NotAuthorized)
    );
    // The sibling element and the parent observation are both spent.
    assert_eq!(
        registry.consume_element(&second, &scope(), &|| false),
        Err(TargetError::StaleHandle)
    );
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::StaleHandle)
    );
    // And vice versa: consuming the observation spends its elements.
    let observation = ax_observation(&mut registry, &window);
    let element = registry
        .issue_element(&observation.observation_id, &scope(), 23)
        .unwrap();
    registry
        .consume_observation(&observation.observation_id, &scope(), &|| false)
        .unwrap();
    assert_eq!(
        registry.consume_element(&element, &scope(), &|| false),
        Err(TargetError::StaleHandle)
    );
    // Element handles require a tree observation, not a screenshot.
    let observation = capture(&mut registry, &window);
    assert_eq!(
        registry.issue_element(&observation.observation_id, &scope(), 24),
        Err(TargetError::Invalid(
            "element handles require an accessibility tree observation"
        ))
    );
}

#[test]
fn window_resize_stales_observations_and_element_handles() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    // A capture registered against a window that no longer has the
    // captured size is born stale.
    *registry.probe.size.lock().unwrap() = Some((1920.0, 1080.0));
    assert_eq!(
        registry.begin_observation(
            &window,
            &scope(),
            Environment::NativeBackground,
            ObservationKind::Capture,
            geometry(),
        ),
        Err(TargetError::StaleHandle)
    );
    // A rejected registration consumed nothing: restoring the size lets
    // the same window observe again.
    *registry.probe.size.lock().unwrap() = Some((2560.0, 1440.0));
    let observation = capture(&mut registry, &window);
    // Resizing after the observation stales it; dispatch is rejected
    // before the lease is spent.
    *registry.probe.size.lock().unwrap() = Some((2561.0, 1440.0));
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::StaleHandle)
    );
    // Restoring the recorded size revalidates the still-unconsumed
    // lease: coordinates map onto the observed image again.
    *registry.probe.size.lock().unwrap() = Some((2560.0, 1440.0));
    registry
        .consume_observation(&observation.observation_id, &scope(), &|| false)
        .unwrap();
    // Element handles follow the same rule through their parent
    // observation's geometry.
    let observation = ax_observation(&mut registry, &window);
    let element = registry
        .issue_element(&observation.observation_id, &scope(), 51)
        .unwrap();
    *registry.probe.size.lock().unwrap() = Some((1280.0, 720.0));
    assert_eq!(
        registry.consume_element(&element, &scope(), &|| false),
        Err(TargetError::StaleHandle)
    );
    // A window whose size can no longer be proven fails closed like a
    // generation miss.
    *registry.probe.size.lock().unwrap() = None;
    assert_eq!(
        registry.consume_element(&element, &scope(), &|| false),
        Err(TargetError::WindowReplaced)
    );
    assert_eq!(
        registry.begin_observation(
            &window,
            &scope(),
            Environment::NativeBackground,
            ObservationKind::Capture,
            geometry(),
        ),
        Err(TargetError::WindowReplaced)
    );
}

#[test]
fn expired_leases_are_stale() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let observation = ax_observation(&mut registry, &window);
    let element = registry
        .issue_element(&observation.observation_id, &scope(), 31)
        .unwrap();
    let key = observation.observation_id.as_str().to_owned();
    registry.observations.get_mut(&key).unwrap().created =
        Instant::now() - LEASE_TTL - Duration::from_secs(1);
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::StaleHandle)
    );
    assert_eq!(
        registry.consume_element(&element, &scope(), &|| false),
        Err(TargetError::StaleHandle)
    );
    assert_eq!(
        registry.issue_element(&observation.observation_id, &scope(), 32),
        Err(TargetError::StaleHandle)
    );
}

#[test]
fn cancellation_precedes_consumption() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let observation = ax_observation(&mut registry, &window);
    let element = registry
        .issue_element(&observation.observation_id, &scope(), 41)
        .unwrap();
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| true),
        Err(TargetError::Cancelled)
    );
    assert_eq!(
        registry.consume_element(&element, &scope(), &|| true),
        Err(TargetError::Cancelled)
    );
    // A cancelled attempt consumed nothing.
    registry
        .consume_element(&element, &scope(), &|| false)
        .unwrap();
    let observation = capture(&mut registry, &window);
    registry
        .consume_observation(&observation.observation_id, &scope(), &|| false)
        .unwrap();
}

#[test]
fn image_coordinates_map_into_window_bounds_only() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let observation = capture(&mut registry, &window);
    let point = image_to_window(ImagePoint { x: 640.0, y: 360.0 }, &observation).unwrap();
    assert_eq!(
        point,
        WindowPoint {
            x: 1280.0,
            y: 720.0
        }
    );
    for (x, y) in [
        (-0.1, 0.0),
        (0.0, -0.1),
        (1280.0, 0.0), // the image edge itself is out of bounds
        (0.0, 720.0),
        (f64::NAN, 0.0),
        (0.0, f64::INFINITY),
    ] {
        assert_eq!(
            image_to_window(ImagePoint { x, y }, &observation),
            Err(TargetError::OutOfBounds)
        );
    }
    // Host-side geometry is validated as well.
    let mut bad = geometry();
    bad.image_width = 0;
    assert!(matches!(
        registry.begin_observation(
            &window,
            &scope(),
            Environment::NativeBackground,
            ObservationKind::Capture,
            bad,
        ),
        Err(TargetError::Invalid(_))
    ));
}

#[test]
fn unauthorized_and_cross_target_rejections_precede_any_probe_call() {
    let mut registry = TargetRegistry::new(FakeProbe::new());
    let editor = target(AppFamily::Appkit);
    // No grant at all: rejected before the probe.
    assert_eq!(
        registry.bind_window(&scope(), editor.clone(), instance(), window_identity()),
        Err(TargetError::NotAuthorized)
    );
    registry.open_scope(&scope());
    registry
        .grant(&scope(), &editor, GrantKind::ForRun)
        .unwrap();
    // A grant covers only the approved application: another app, and any
    // website (even inside the same bundle), stay unauthorized.
    let other_app = TargetIdentity::application(AppIdentity {
        bundle_id: "com.other.app".into(),
        family: AppFamily::Chromium,
    });
    let website = TargetIdentity::website(
        AppIdentity {
            bundle_id: "com.example.app".into(),
            family: AppFamily::Browser,
        },
        "https://example.com",
    )
    .unwrap();
    for target in [other_app, website] {
        assert_eq!(
            registry.bind_window(&scope(), target, instance(), window_identity()),
            Err(TargetError::NotAuthorized)
        );
    }
    // A grant is bound to its workspace/run scope.
    for other_scope in [Scope::new("ws-1", "run-2"), Scope::new("ws-2", "run-1")] {
        assert_eq!(
            registry.bind_window(&other_scope, editor.clone(), instance(), window_identity()),
            Err(TargetError::NotAuthorized)
        );
    }
    assert_eq!(registry.probe.calls(), 0);
    // The approved target binds, observes and dispatches.
    let window = registry
        .bind_window(&scope(), editor, instance(), window_identity())
        .unwrap();
    let observation = capture(&mut registry, &window);
    registry
        .consume_observation(&observation.observation_id, &scope(), &|| false)
        .unwrap();
}

#[test]
fn one_shot_grant_covers_one_action_and_is_not_spent_early() {
    let mut registry = TargetRegistry::new(FakeProbe::new());
    let editor = target(AppFamily::Appkit);
    registry.open_scope(&scope());
    registry.grant(&scope(), &editor, GrantKind::Once).unwrap();
    // Binding requires the grant but does not spend it.
    let window = registry
        .bind_window(&scope(), editor.clone(), instance(), window_identity())
        .unwrap();
    // A stale target does not burn the single approval: the spend
    // happens only after liveness passed.
    registry.probe.alive.store(false, Ordering::Relaxed);
    assert_eq!(
        registry.begin_observation(
            &window,
            &scope(),
            Environment::NativeBackground,
            ObservationKind::Capture,
            geometry(),
        ),
        Err(TargetError::ProcessRestarted)
    );
    registry.probe.alive.store(true, Ordering::Relaxed);
    // The first observation spends the grant.
    let observation = capture(&mut registry, &window);
    // The follow-up input is a new action and needs its own approval.
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::NotAuthorized)
    );
    // The rejected attempt consumed neither grant nor lease.
    registry.grant(&scope(), &editor, GrantKind::Once).unwrap();
    registry
        .consume_observation(&observation.observation_id, &scope(), &|| false)
        .unwrap();
}

#[test]
fn revocation_blocks_dispatch_before_backend_access_and_history_stays() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let editor = target(AppFamily::Appkit);
    let observation = capture(&mut registry, &window);
    registry.revoke(&scope(), &editor);
    let calls_before = registry.probe.calls();
    // The outstanding lease can no longer be dispatched, and the
    // rejection happens before any probe call.
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::NotAuthorized)
    );
    assert_eq!(registry.probe.calls(), calls_before);
    // Fresh observations and liveness checks are blocked the same way.
    assert_eq!(
        registry.begin_observation(
            &window,
            &scope(),
            Environment::NativeBackground,
            ObservationKind::Capture,
            geometry(),
        ),
        Err(TargetError::NotAuthorized)
    );
    assert_eq!(
        registry.validate_window(&window, &scope()),
        Err(TargetError::NotAuthorized)
    );
    // Re-approval revives the still-valid lease.
    registry.open_scope(&scope());
    registry
        .grant(&scope(), &editor, GrantKind::ForRun)
        .unwrap();
    registry
        .consume_observation(&observation.observation_id, &scope(), &|| false)
        .unwrap();
    // Already-consumed history is not rewritten: the spent lease stays spent.
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::StaleHandle)
    );
}

#[test]
fn protected_targets_are_rejected_before_any_probe_call() {
    let authorizer = TargetAuthorizer::new(["dev.pawork.desktop".to_string()]);
    let mut registry = TargetRegistry::with_authorizer(FakeProbe::new(), authorizer);
    let own_ui = TargetIdentity::application(AppIdentity {
        bundle_id: "dev.pawork.desktop".into(),
        family: AppFamily::Appkit,
    });
    registry.open_scope(&scope());
    // Granting the host's own UI is refused, so it can never be bound:
    // the agent cannot approve itself through the approval surface.
    assert_eq!(
        registry.grant(&scope(), &own_ui, GrantKind::ForRun),
        Err(TargetError::ForbiddenTarget)
    );
    assert_eq!(
        registry.bind_window(&scope(), own_ui, instance(), window_identity()),
        Err(TargetError::ForbiddenTarget)
    );
    assert_eq!(registry.probe.calls(), 0);
}

#[test]
fn default_registry_protects_pawork_ui_and_permission_surfaces() {
    // Default construction is safe: the built-in minimum protected set
    // covers Pawork's own approval UI and the OS permission surfaces,
    // without any host-supplied list.
    let mut registry = TargetRegistry::new(FakeProbe::new());
    for bundle in ["dev.pawork.desktop", "com.apple.systempreferences"] {
        registry.open_scope(&scope());
        let protected = TargetIdentity::application(AppIdentity {
            bundle_id: bundle.into(),
            family: AppFamily::Appkit,
        });
        assert_eq!(
            registry.grant(&scope(), &protected, GrantKind::ForRun),
            Err(TargetError::ForbiddenTarget)
        );
        assert_eq!(
            registry.bind_window(&scope(), protected, instance(), window_identity()),
            Err(TargetError::ForbiddenTarget)
        );
    }
    assert_eq!(registry.probe.calls(), 0);
}

#[test]
fn website_target_rejects_same_window_navigation_before_dispatch() {
    let mut registry = TargetRegistry::new(FakeProbe::new());
    let authorized = TargetIdentity::website(
        AppIdentity {
            bundle_id: "com.example.browser".into(),
            family: AppFamily::Browser,
        },
        "https://example.com",
    )
    .unwrap();
    registry.open_scope(&scope());
    registry
        .grant(&scope(), &authorized, GrantKind::ForRun)
        .unwrap();
    // Binding pins the origin the window actually shows.
    *registry.probe.origin.lock().unwrap() = Some("https://other.example.com".into());
    assert_eq!(
        registry.bind_window(&scope(), authorized.clone(), instance(), window_identity()),
        Err(TargetError::SiteChanged)
    );
    *registry.probe.origin.lock().unwrap() = Some("https://example.com".into());
    let window = registry
        .bind_window(&scope(), authorized.clone(), instance(), window_identity())
        .unwrap();
    // The capture pre-flight passes while the window shows the
    // authorized origin.
    assert!(registry.require_authorized(&window, &scope()).is_ok());
    let observation = capture(&mut registry, &window);
    // Same-window navigation to an unauthorized origin blocks dispatch,
    // observation and liveness checks, with window and grant unchanged.
    *registry.probe.origin.lock().unwrap() = Some("https://other.example.com".into());
    // The capture pre-flight rejects just as early: the host must not
    // read an unauthorized origin's screen.
    assert_eq!(
        registry.require_authorized(&window, &scope()),
        Err(TargetError::SiteChanged)
    );
    assert_eq!(
        registry.consume_observation(&observation.observation_id, &scope(), &|| false),
        Err(TargetError::SiteChanged)
    );
    assert_eq!(
        registry.begin_observation(
            &window,
            &scope(),
            Environment::NativeBackground,
            ObservationKind::Capture,
            geometry(),
        ),
        Err(TargetError::SiteChanged)
    );
    assert_eq!(
        registry.validate_window(&window, &scope()),
        Err(TargetError::SiteChanged)
    );
    // An undeterminable origin fails closed the same way.
    *registry.probe.origin.lock().unwrap() = None;
    assert_eq!(
        registry.require_authorized(&window, &scope()),
        Err(TargetError::SiteChanged)
    );
    assert_eq!(
        registry.validate_window(&window, &scope()),
        Err(TargetError::SiteChanged)
    );
    // Navigating back restores the still-valid lease; origin spelling
    // differences (trailing slash, casing) cannot dodge or break it.
    *registry.probe.origin.lock().unwrap() = Some("HTTPS://Example.COM/".into());
    assert!(registry.require_authorized(&window, &scope()).is_ok());
    registry
        .consume_observation(&observation.observation_id, &scope(), &|| false)
        .unwrap();
}

#[test]
fn pre_flight_require_authorized_matches_the_gated_operations() {
    let (mut registry, window) = bound(AppFamily::Appkit);
    let editor = target(AppFamily::Appkit);
    assert!(registry.require_authorized(&window, &scope()).is_ok());
    assert_eq!(
        registry.require_authorized(&window, &Scope::new("ws-1", "run-2")),
        Err(TargetError::CrossRun)
    );
    registry.revoke(&scope(), &editor);
    assert_eq!(
        registry.require_authorized(&window, &scope()),
        Err(TargetError::NotAuthorized)
    );
    let forged: WindowHandle = serde_json::from_str("\"w-1-1\"").unwrap();
    assert_eq!(
        registry.require_authorized(&forged, &scope()),
        Err(TargetError::UnknownHandle)
    );
}

// ---------- CU-05 bounded accessibility tree ----------

fn node(path: &[u32], role: &str, name: Option<&str>) -> AxTreeNode {
    AxTreeNode {
        path: path.to_vec(),
        depth: path.len() as u32,
        role: role.to_string(),
        name: name.map(str::to_string),
        name_truncated: false,
        name_unreadable: false,
        enabled: Some(true),
        focused: Some(false),
        frame: Some([0.0, 0.0, 100.0, 40.0]),
        actions: vec!["AXPress".to_string()],
        children: 0,
        depth_limited: false,
    }
}

fn read(nodes: Vec<AxTreeNode>, truncation: AxTreeTruncation) -> AxTreeRead {
    let tree_revision = ax_tree_revision(&nodes, truncation);
    AxTreeRead {
        window: window_identity(),
        tree_revision,
        nodes,
        truncation,
    }
}

#[test]
fn ax_tree_bounds_are_finite_and_capped() {
    AX_TREE_BOUNDS.validate().unwrap();
    let valid = AxTreeBounds {
        max_depth: 4,
        max_nodes: 16,
        max_text: 32,
        max_read_ms: 500,
    };
    valid.validate().unwrap();
    assert_eq!(valid.max_read(), Duration::from_millis(500));
    for bounds in [
        AxTreeBounds {
            max_depth: 0,
            ..valid
        },
        AxTreeBounds {
            max_depth: 65,
            ..valid
        },
        AxTreeBounds {
            max_nodes: 0,
            ..valid
        },
        AxTreeBounds {
            max_nodes: 8193,
            ..valid
        },
        AxTreeBounds {
            max_text: 0,
            ..valid
        },
        AxTreeBounds {
            max_text: 1025,
            ..valid
        },
        AxTreeBounds {
            max_read_ms: 0,
            ..valid
        },
        AxTreeBounds {
            max_read_ms: 30_001,
            ..valid
        },
    ] {
        assert_eq!(
            bounds.validate(),
            Err(TargetError::Invalid(
                "accessibility tree bounds out of range"
            ))
        );
    }
}

#[test]
fn ax_tree_revision_is_deterministic_and_sensitive() {
    let nodes = vec![
        node(&[], "AXWindow", Some("Document")),
        node(&[0], "AXTextArea", None),
    ];
    let revision = ax_tree_revision(&nodes, AxTreeTruncation::default());
    assert_eq!(
        revision,
        ax_tree_revision(&nodes, AxTreeTruncation::default())
    );
    // Each of these emitted differences changes the fingerprint (the
    // 64-bit FNV-1a digest is a cheap change signal, not a
    // collision-proof — see the fn docs).
    let mut renamed = nodes.clone();
    renamed[0].name = Some("Other".to_string());
    assert_ne!(
        revision,
        ax_tree_revision(&renamed, AxTreeTruncation::default())
    );
    let mut unreadable = nodes.clone();
    unreadable[1].name_unreadable = true;
    assert_ne!(
        revision,
        ax_tree_revision(&unreadable, AxTreeTruncation::default())
    );
    let mut moved = nodes.clone();
    moved[1].frame = Some([0.0, 8.0, 100.0, 40.0]);
    assert_ne!(
        revision,
        ax_tree_revision(&moved, AxTreeTruncation::default())
    );
    let mut focused = nodes.clone();
    focused[1].focused = Some(true);
    assert_ne!(
        revision,
        ax_tree_revision(&focused, AxTreeTruncation::default())
    );
    let mut actions = nodes.clone();
    actions[1].actions.push("AXShowMenu".to_string());
    assert_ne!(
        revision,
        ax_tree_revision(&actions, AxTreeTruncation::default())
    );
    let mut counts = nodes.clone();
    counts[0].children = 2;
    assert_ne!(
        revision,
        ax_tree_revision(&counts, AxTreeTruncation::default())
    );
    assert_ne!(
        revision,
        ax_tree_revision(
            &nodes,
            AxTreeTruncation {
                nodes: true,
                ..AxTreeTruncation::default()
            }
        )
    );
    let mut extra = nodes.clone();
    extra.push(node(&[1], "AXButton", Some("OK")));
    assert_ne!(
        revision,
        ax_tree_revision(&extra, AxTreeTruncation::default())
    );
}

#[test]
fn lookup_never_picks_first_and_flags_unprovable_uniqueness() {
    let nodes = vec![
        node(&[], "AXWindow", Some("Document")),
        node(&[0], "AXButton", Some("Save")),
        node(&[1], "AXButton", Some("Cancel")),
        node(&[2], "AXTextArea", None),
    ];
    let complete = read(nodes.clone(), AxTreeTruncation::default());
    // Exactly one match in a complete tree issues.
    assert_eq!(
        lookup_element(
            &complete,
            &ElementQuery::new(Some("AXTextArea"), None).unwrap()
        ),
        ElementLookup::Unique { index: 3 }
    );
    // Several matches never resolve to the first.
    assert_eq!(
        lookup_element(
            &complete,
            &ElementQuery::new(Some("AXButton"), None).unwrap()
        ),
        ElementLookup::Ambiguous {
            matches: 2,
            tree_truncated: false
        }
    );
    // Name matching is a case-insensitive substring over readable names.
    assert_eq!(
        lookup_element(&complete, &ElementQuery::new(None, Some("SAVE")).unwrap()),
        ElementLookup::Unique { index: 1 }
    );
    // Zero matches in a complete tree are a genuine absence.
    assert_eq!(
        lookup_element(
            &complete,
            &ElementQuery::new(Some("AXSlider"), None).unwrap()
        ),
        ElementLookup::NoMatch {
            tree_truncated: false
        }
    );
    // The same unique match under a truncated tree is unprovable.
    let truncated = read(
        nodes.clone(),
        AxTreeTruncation {
            nodes: true,
            ..AxTreeTruncation::default()
        },
    );
    assert_eq!(
        lookup_element(
            &truncated,
            &ElementQuery::new(Some("AXTextArea"), None).unwrap()
        ),
        ElementLookup::UnprovenUnique { index: 3 }
    );
    assert_eq!(
        lookup_element(
            &truncated,
            &ElementQuery::new(Some("AXSlider"), None).unwrap()
        ),
        ElementLookup::NoMatch {
            tree_truncated: true
        }
    );
    // Unreadable names undermine NAME queries only: a role-only query
    // over a structurally complete read still issues, while the same
    // read refuses to crown a name-based unique match.
    let names_lost = read(
        nodes.clone(),
        AxTreeTruncation {
            names: true,
            ..AxTreeTruncation::default()
        },
    );
    assert_eq!(
        lookup_element(
            &names_lost,
            &ElementQuery::new(Some("AXTextArea"), None).unwrap()
        ),
        ElementLookup::Unique { index: 3 }
    );
    assert_eq!(
        lookup_element(&names_lost, &ElementQuery::new(None, Some("SAVE")).unwrap()),
        ElementLookup::UnprovenUnique { index: 1 }
    );
    assert_eq!(
        lookup_element(&names_lost, &ElementQuery::new(None, Some("nope")).unwrap()),
        ElementLookup::NoMatch {
            tree_truncated: true
        }
    );
    // A query without criteria is rejected.
    assert!(matches!(
        ElementQuery::new(None, Some("  ")),
        Err(TargetError::Invalid(_))
    ));
    // An element without a name never matches a name query.
    assert_eq!(
        lookup_element(
            &complete,
            &ElementQuery::new(Some("AXTextArea"), Some("x")).unwrap()
        ),
        ElementLookup::NoMatch {
            tree_truncated: false
        }
    );
}

#[test]
fn target_states_aggregate_grants_bindings_occupancy_and_pause() {
    let mut registry = TargetRegistry::new(FakeProbe::new());
    let other = Scope::new("ws-1", "run-2");
    let site = website_target();
    registry.open_scope(&scope());
    registry.open_scope(&other);
    // A grant without a binding still lists the target.
    registry.grant(&scope(), &site, GrantKind::ForRun).unwrap();
    // A binding without a grant lists the target with grant None.
    let notes = TargetIdentity::application(AppIdentity {
        bundle_id: "com.example.notes".into(),
        family: AppFamily::Appkit,
    });
    registry.grant(&other, &notes, GrantKind::ForRun).unwrap();
    registry
        .bind_window(&other, notes.clone(), instance(), window_identity())
        .unwrap();
    // The browser anchor is occupied by another scope and paused.
    registry.occupied.insert(
        "com.example.browser".into(),
        (other.clone(), Instant::now()),
    );
    registry.pause_dispatch(&site);

    let states = registry.target_states();
    assert_eq!(states.len(), 2, "one entry per (scope, target)");
    let site_state = states
        .iter()
        .find(|state| state.target == site)
        .expect("website grant listed");
    assert_eq!(site_state.scope, scope());
    assert_eq!(site_state.grant, Some(GrantKind::ForRun));
    assert_eq!(site_state.window, None);
    assert_eq!(site_state.occupied_by, Some(other.clone()));
    assert!(site_state.dispatch_paused);
    let notes_state = states
        .iter()
        .find(|state| state.target == notes)
        .expect("bound target listed");
    assert_eq!(notes_state.scope, other);
    assert_eq!(notes_state.grant, Some(GrantKind::ForRun));
    assert!(notes_state.window.is_some());
    // Binding refreshes occupancy, so the binder is its own owner.
    assert_eq!(notes_state.occupied_by, Some(other.clone()));
    assert!(!notes_state.dispatch_paused);

    // Grant plus binding merge into one entry; revocation keeps the
    // binding listed with grant None; release_scope clears the rest.
    registry.occupied.remove("com.example.browser");
    registry
        .probe
        .origin
        .lock()
        .unwrap()
        .replace("https://example.com".into());
    let window = registry
        .bind_window(&scope(), site.clone(), instance(), window_identity())
        .unwrap();
    let states = registry.target_states();
    let site_state = states
        .iter()
        .find(|state| state.target == site)
        .expect("merged entry");
    assert_eq!(site_state.window, Some(window));
    registry.revoke(&scope(), &site);
    let states = registry.target_states();
    let site_state = states.iter().find(|state| state.target == site).unwrap();
    assert_eq!(site_state.grant, None);
    assert!(site_state.window.is_some());
    registry.release_scope(&scope());
    assert!(registry
        .target_states()
        .iter()
        .all(|state| state.scope != scope()));
}

#[test]
fn closed_scopes_reject_late_grants_and_clear_the_ledger() {
    let mut registry = TargetRegistry::new(FakeProbe::new());
    let other = Scope::new("ws-1", "run-2");
    let site = website_target();
    let notes = TargetIdentity::application(AppIdentity {
        bundle_id: "com.example.notes".into(),
        family: AppFamily::Appkit,
    });
    registry.open_scope(&scope());
    registry.grant(&scope(), &site, GrantKind::ForRun).unwrap();
    registry.open_scope(&other);
    registry.grant(&other, &notes, GrantKind::ForRun).unwrap();

    // close_scope clears the ledger and the active registration for the
    // scope, so every later grant from it is rejected without leaving a
    // terminal-state record behind (nothing accumulates after the close).
    registry.close_scope(&scope());
    assert!(registry
        .target_states()
        .iter()
        .all(|state| state.scope != scope()));
    assert!(matches!(
        registry.grant(&scope(), &site, GrantKind::ForRun),
        Err(TargetError::ScopeClosed)
    ));
    // Closing again stays idempotent and leaves other scopes alone.
    registry.close_scope(&scope());
    let states = registry.target_states();
    assert_eq!(states.len(), 1, "only the other scope's grant remains");
    assert_eq!(states[0].scope, other);
    // A scope that was never opened is closed by definition: the only
    // grant path (host approval) always follows run registration.
    let unopened = Scope::new("ws-1", "run-3");
    assert!(matches!(
        registry.grant(&unopened, &notes, GrantKind::ForRun),
        Err(TargetError::ScopeClosed)
    ));
}

#[test]
fn unsupported_probe_fails_closed() {
    let probe = UnsupportedProbe;
    assert!(!probe.process_instance_alive(&instance()));
    assert_eq!(probe.window_generation(&instance(), 42), None);
    assert_eq!(probe.tree_revision(&window_identity()), None);
    assert_eq!(probe.current_origin(&window_identity()), None);
    assert_eq!(probe.window_size(&instance(), 42), None);
}
