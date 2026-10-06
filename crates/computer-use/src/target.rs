//! Typed target, observation and background-capability contracts (CU-02).
//!
//! A target binds an application, a process instance (pid + start token), a
//! window generation and the issuing workspace/run [Scope]. Models only ever
//! hold opaque handles issued by the host [TargetRegistry]; they cannot
//! submit raw pids, absolute paths or network endpoints. Handles are validated
//! on every use and reject process restart, window destroy/reuse, tree changes,
//! stale or unknown handles, cross-run use and out-of-bounds coordinates.
//!
//! Background capability follows the CU-01 measured matrix per application
//! family (see background_capability). Actions without measured background
//! support are rejected by require_background; there is never a fallback to
//! global input, and pointer actions have no background support in any family.
//! User takeover of the same target has no OS-level arbitration (CU-01 T6b):
//! cancellation is the only stop mechanism. Unverified is not Supported:
//! actions not yet measured on a family are rejected until a real measurement
//! promotes them.
//!
//! This module is platform-free contract code. The host supplies live state
//! through [NativeProbe]; a downstream task implements the macOS probe and
//! the action dispatch behind these types. The existing isolated virtual
//! desktop ([Environment::IsolatedDesktop], RFB route) keeps its own
//! Computer/Backend path and is unaffected.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::approval::{normalize_origin, GrantKind, TargetAuthorizer, TargetIdentity};
use crate::MAX_IMAGE_EDGE;

/// One-shot lease lifetime, same as the isolated-desktop observation.
pub const LEASE_TTL: Duration = Duration::from_secs(60);

static TARGET_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Process-level startup namespace (pid + start instant in nanos). A handle
/// minted by a previous host instance can never collide with one minted after
/// a restart, even if the OS reuses the pid; the process-wide static sequence
/// keeps handles unique between registries inside one process.
fn host_instance() -> &'static str {
    static INSTANCE: OnceLock<String> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{}-{}", std::process::id(), nanos)
    })
}

fn next_handle(prefix: &str) -> String {
    format!(
        "{}-{}-{}",
        prefix,
        host_instance(),
        TARGET_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TargetError {
    /// Never issued by this host registry (covers forged model input and any
    /// handle minted before a host restart).
    #[error("unknown handle; use only handles issued by the host for this run")]
    UnknownHandle,
    /// Consumed by an earlier action, or past [LEASE_TTL]. Re-observe.
    #[error("handle expired or already consumed; observe the target again")]
    StaleHandle,
    /// Issued to a different workspace/run scope.
    #[error("handle belongs to a different workspace or run")]
    CrossRun,
    /// The pid died or was restarted; the instance token no longer matches.
    #[error("target process exited or restarted; bind a new target")]
    ProcessRestarted,
    /// Window destroyed, or its identifier reused by another window
    /// (generation mismatch).
    #[error("target window was destroyed or replaced; re-observe")]
    WindowReplaced,
    /// The accessibility tree advanced since the element handle was issued.
    #[error("window accessibility tree changed since the element was observed; re-read it")]
    TreeChanged,
    /// Coordinates outside the observed image bounds.
    #[error("coordinates outside the observed image bounds")]
    OutOfBounds,
    /// No measured background support for this application family (including
    /// Unverified); there is no fallback to global input.
    #[error("action has no verified background support for this application family; global input is never used")]
    BackgroundUnsupported,
    /// Cancelled before the handle was consumed or the action dispatched.
    #[error("computer action cancelled")]
    Cancelled,
    /// No active grant for this target in this workspace/run: never granted,
    /// granted to a different application or website, a spent one-shot
    /// grant, or revoked. Rejected before any probe or backend access.
    #[error("target is not authorized for this workspace/run; the user must approve this application or website")]
    NotAuthorized,
    /// The host forbids this target (its own UI); it can never be granted
    /// or bound, so the agent cannot approve itself through it.
    #[error("target is protected and can never be authorized")]
    ForbiddenTarget,
    /// The window no longer shows the origin the website target was
    /// authorized and bound for (same-window navigation), or the current
    /// origin cannot be determined; website targets fail closed.
    #[error("target window no longer shows the authorized origin; bind a new target")]
    SiteChanged,
    /// Host-side contract misuse (e.g. element handle from a screenshot).
    #[error("invalid target contract use: {0}")]
    Invalid(&'static str),
}

/// Which environment an observation or action belongs to. The isolated
/// desktop is the existing explicitly-started RFB container; native
/// background operates the user's own machine per target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    IsolatedDesktop,
    NativeBackground,
}

/// Application family per the CU-01 measured capability matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppFamily {
    /// Native AppKit apps (CU-01: TextEdit).
    Appkit,
    /// Electron/Chromium-embedded apps (CU-01: VS Code).
    Chromium,
    /// Browsers (CU-01: Chrome).
    Browser,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AppIdentity {
    pub bundle_id: String,
    pub family: AppFamily,
}

/// A process instance: the pid namespaced by a start token (e.g. the
/// platform's process start time/audit token) so pid reuse after an exit or
/// restart never validates. Host-side; models never submit one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInstance {
    pub pid: u32,
    pub start_token: u64,
}

/// A window inside a process instance. generation is host-assigned from a
/// creation signal so a destroyed window whose identifier gets reused by a
/// new window never validates against the old handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowIdentity {
    pub window_id: u64,
    pub generation: u64,
}

/// The workspace/run a handle is issued to. Cross-run use is rejected.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Scope {
    pub workspace_id: String,
    pub run_id: String,
}

impl Scope {
    pub fn new(workspace_id: impl Into<String>, run_id: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            run_id: run_id.into(),
        }
    }
}

macro_rules! opaque_handle {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        /// Opaque to the model: it round-trips the string but cannot mint a
        /// valid one; the registry rejects anything it did not issue.
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}
opaque_handle!(WindowHandle, "Host-issued handle for a bound target window.");
opaque_handle!(
    ObservationHandle,
    "Host-issued one-shot observation lease (60s TTL)."
);
opaque_handle!(
    ElementHandle,
    "Host-issued one-shot accessibility element handle bound to a tree revision."
);

/// What produced an observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationKind {
    /// Window image capture (CU-01: ScreenCaptureKit, safe when occluded,
    /// background or minimized for every family).
    Capture,
    /// Accessibility tree read at the given host revision; required before
    /// element handles can be issued.
    AxTree { tree_revision: u64 },
}

/// Geometry of a window observation: image pixels vs window points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObservationGeometry {
    pub image_width: u32,
    pub image_height: u32,
    pub window_width: f64,
    pub window_height: f64,
}

impl ObservationGeometry {
    fn validate(&self) -> Result<(), TargetError> {
        if self.image_width == 0
            || self.image_height == 0
            || self.image_width > MAX_IMAGE_EDGE
            || self.image_height > MAX_IMAGE_EDGE
            || !self.window_width.is_finite()
            || !self.window_height.is_finite()
            || self.window_width <= 0.0
            || self.window_height <= 0.0
        {
            return Err(TargetError::Invalid("observation geometry out of bounds"));
        }
        Ok(())
    }
}

/// Model-facing observation identity, mirroring the isolated-desktop
/// Observation: coordinates given back by the model are image pixels.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TargetObservation {
    pub observation_id: ObservationHandle,
    pub environment: Environment,
    pub window: WindowHandle,
    pub image_width: u32,
    pub image_height: u32,
    pub window_width: f64,
    pub window_height: f64,
}

/// Pixel point in an observation image, origin top-left (model input).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImagePoint {
    pub x: f64,
    pub y: f64,
}

/// Point in window-local coordinates (points, origin top-left of the window).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowPoint {
    pub x: f64,
    pub y: f64,
}

/// Map model image pixels to window points; rejects non-finite and
/// out-of-bounds coordinates before any dispatch.
pub fn image_to_window(
    point: ImagePoint,
    observation: &TargetObservation,
) -> Result<WindowPoint, TargetError> {
    let ImagePoint { x, y } = point;
    if !x.is_finite()
        || !y.is_finite()
        || x < 0.0
        || y < 0.0
        || x >= observation.image_width as f64
        || y >= observation.image_height as f64
    {
        return Err(TargetError::OutOfBounds);
    }
    Ok(WindowPoint {
        x: x * observation.window_width / observation.image_width as f64,
        y: y * observation.window_height / observation.image_height as f64,
    })
}

/// Action vocabulary for background-capability checks. Payloads (text, keys,
/// points) ride on the validated handles downstream; this enum names what may
/// run in native background at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeActionKind {
    /// Window image capture (ScreenCaptureKit).
    CaptureWindow,
    /// Read the window accessibility tree/values.
    AxRead,
    /// AXSetValue / selected-text insertion on an element.
    AxSemanticWrite,
    /// AXPress on an element.
    AxPress,
    /// Targeted text/key events (CGEventPostToPid).
    TargetedKeys,
    /// Targeted menu shortcut such as Cmd+S.
    MenuShortcut,
    /// Pressing menu-bar items via AX.
    MenubarPress,
    /// Pointer click/move/drag/scroll events.
    Pointer,
    /// Move the window (AXPosition).
    WindowMove,
    /// Resize the window (AXSize).
    WindowResize,
}

/// Measured background support. Unverified means CU-01 could not prove the
/// action on that family; it is rejected exactly like Unsupported until a
/// real measurement promotes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Supported,
    Unsupported,
    Unverified,
}

/// The CU-01 measured matrix (docs/plan/cu-01-background-feasibility.md).
/// minimized changes support in two measured ways: AppKit targeted keys are
/// silently dropped while minimized, and window move/resize was only measured
/// in background/occluded states.
pub fn background_capability(
    family: AppFamily,
    action: NativeActionKind,
    minimized: bool,
) -> Capability {
    use AppFamily::*;
    use Capability::*;
    use NativeActionKind::*;
    match action {
        CaptureWindow | AxRead => Supported,
        Pointer => Unsupported,
        AxSemanticWrite => match family {
            Appkit => Supported,
            Chromium => Unsupported,
            Browser => Unverified,
        },
        // CU-01 proved AXPress on web pages (JS/navigation). On AppKit only
        // AXSetValue / AXSelectedText writes were proven, and on Chromium the
        // press effect was undetermined.
        AxPress => match family {
            Appkit | Chromium => Unverified,
            Browser => Supported,
        },
        TargetedKeys => match family {
            Appkit => {
                if minimized {
                    Unsupported
                } else {
                    Supported
                }
            }
            Chromium | Browser => Unsupported,
        },
        MenuShortcut | MenubarPress => match family {
            Appkit => Unsupported,
            Chromium | Browser => Unverified,
        },
        // Minimized-state window move/resize was not measured in CU-01.
        WindowMove => match family {
            Appkit => {
                if minimized {
                    Unverified
                } else {
                    Supported
                }
            }
            Chromium | Browser => Unsupported,
        },
        WindowResize => {
            if minimized {
                Unverified
            } else {
                Supported
            }
        }
    }
}

/// Gate before any native-background dispatch. Only Supported passes; there
/// is no fallback to global or foreground input.
pub fn require_background(
    family: AppFamily,
    action: NativeActionKind,
    minimized: bool,
) -> Result<(), TargetError> {
    match background_capability(family, action, minimized) {
        Capability::Supported => Ok(()),
        Capability::Unsupported | Capability::Unverified => Err(TargetError::BackgroundUnsupported),
    }
}

/// Live native state supplied by the host; implemented by the macOS backend
/// (downstream task) and by test fakes. Contract code never calls platform
/// APIs directly.
pub trait NativeProbe: Send + Sync {
    /// True only while the exact process instance (pid + start token) lives.
    fn process_instance_alive(&self, instance: &ProcessInstance) -> bool;
    /// Current generation of a window; None once the window is gone.
    fn window_generation(&self, instance: &ProcessInstance, window_id: u64) -> Option<u64>;
    /// Current accessibility-tree revision of a window; None when the
    /// window or its tree is unavailable.
    fn tree_revision(&self, window: &WindowIdentity) -> Option<u64>;
    /// Current web origin shown in a window (website targets). None when
    /// the window is not showing web content or the origin cannot be
    /// determined — website validation fails closed in both cases.
    fn current_origin(&self, window: &WindowIdentity) -> Option<String>;
}

struct WindowRecord {
    target: TargetIdentity,
    instance: ProcessInstance,
    window: WindowIdentity,
    scope: Scope,
}

struct ObservationRecord {
    window: WindowHandle,
    scope: Scope,
    observation: TargetObservation,
    tree_revision: Option<u64>,
    created: Instant,
    consumed: bool,
}

struct ElementRecord {
    observation: ObservationHandle,
    window: WindowHandle,
    scope: Scope,
    element_token: u64,
    tree_revision: u64,
    consumed: bool,
}

/// A window binding that passed all liveness checks just now.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedWindow {
    pub target: TargetIdentity,
    pub instance: ProcessInstance,
    pub window: WindowIdentity,
    pub scope: Scope,
}

/// A consumed observation lease: exactly one action may dispatch from it.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedObservation {
    pub window: ValidatedWindow,
    pub observation: TargetObservation,
    pub tree_revision: Option<u64>,
}

/// A consumed element handle: exactly one semantic action may dispatch.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedElement {
    pub window: ValidatedWindow,
    /// Host-side token the backend maps to its native element reference.
    pub element_token: u64,
    pub tree_revision: u64,
}

/// Host-side registry: the only issuer and validator of handles. Validation
/// order on every use is cancellation, existence, scope, staleness,
/// authorization (CU-16), process instance, window generation, tree
/// revision; a one-shot grant is spent only after every other check passed.
/// Authorization is checked before any probe call, so a denied, unknown or
/// revoked target never reaches the native backend.
pub struct TargetRegistry<P: NativeProbe> {
    probe: P,
    authorizer: TargetAuthorizer,
    windows: HashMap<String, WindowRecord>,
    observations: HashMap<String, ObservationRecord>,
    elements: HashMap<String, ElementRecord>,
}

impl<P: NativeProbe> TargetRegistry<P> {
    pub fn new(probe: P) -> Self {
        Self::with_authorizer(probe, TargetAuthorizer::default())
    }

    pub fn with_authorizer(probe: P, authorizer: TargetAuthorizer) -> Self {
        Self {
            probe,
            authorizer,
            windows: HashMap::new(),
            observations: HashMap::new(),
            elements: HashMap::new(),
        }
    }

    pub fn authorizer(&self) -> &TargetAuthorizer {
        &self.authorizer
    }

    /// Record an explicit user approval for a target in this scope. This is
    /// the host's only way to create a grant; models can never widen it.
    pub fn grant(
        &mut self,
        scope: &Scope,
        target: &TargetIdentity,
        kind: GrantKind,
    ) -> Result<(), TargetError> {
        self.authorizer.grant(scope, target, kind)
    }

    /// Revoke a target's grant: every later gated operation on it is denied.
    pub fn revoke(&mut self, scope: &Scope, target: &TargetIdentity) {
        self.authorizer.revoke(scope, target);
    }

    /// Drop every grant of a run (run teardown).
    pub fn revoke_scope(&mut self, scope: &Scope) {
        self.authorizer.revoke_scope(scope);
    }

    /// Pre-flight gate the host must pass before connecting to, launching,
    /// capturing or reading a bound target. Non-consuming: each gated
    /// registry operation re-checks and spends one-shot grants itself.
    /// Ledger denial (NotAuthorized / ForbiddenTarget) precedes any probe
    /// call; a website binding then re-pins the window's current origin, so
    /// a same-window navigation (or an undeterminable origin) fails closed
    /// before the host captures. Liveness re-validation stays in the gated
    /// operations.
    pub fn require_authorized(
        &self,
        window: &WindowHandle,
        scope: &Scope,
    ) -> Result<(), TargetError> {
        let record = self
            .windows
            .get(window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        self.authorizer.check(scope, &record.target)?;
        if let TargetIdentity::Website { origin, .. } = &record.target {
            if !origin_matches(&self.probe, &record.window, origin) {
                return Err(TargetError::SiteChanged);
            }
        }
        Ok(())
    }

    /// Bind a window the host just enumerated for scope. Requires an active
    /// grant for the target and rejects a process that is already gone;
    /// the authorization check runs before any probe access. Binding alone
    /// does not spend a one-shot grant.
    pub fn bind_window(
        &mut self,
        scope: &Scope,
        target: TargetIdentity,
        instance: ProcessInstance,
        window: WindowIdentity,
    ) -> Result<WindowHandle, TargetError> {
        self.authorizer.check(scope, &target)?;
        if !self.probe.process_instance_alive(&instance) {
            return Err(TargetError::ProcessRestarted);
        }
        // A website binding pins the origin the window actually shows:
        // claiming a different origin than the current page is rejected
        // before the handle exists.
        if let TargetIdentity::Website { origin, .. } = &target {
            if !origin_matches(&self.probe, &window, origin) {
                return Err(TargetError::SiteChanged);
            }
        }
        let handle = WindowHandle(next_handle("w"));
        self.windows.insert(
            handle.0.clone(),
            WindowRecord {
                target,
                instance,
                window,
                scope: scope.clone(),
            },
        );
        Ok(handle)
    }

    /// Issue a one-shot observation lease for a live bound window. The host
    /// captures/reads first, then registers the geometry it produced.
    pub fn begin_observation(
        &mut self,
        window: &WindowHandle,
        scope: &Scope,
        environment: Environment,
        kind: ObservationKind,
        geometry: ObservationGeometry,
    ) -> Result<TargetObservation, TargetError> {
        let record = self
            .windows
            .get(window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        geometry.validate()?;
        // Authorization precedes any probe call; the one-shot grant is spent
        // only after the window passed liveness, so a stale target does not
        // burn the user's single approval.
        self.authorizer.check(scope, &record.target)?;
        self.live(record)?;
        self.authorizer.spend(scope, &record.target)?;
        let observation = TargetObservation {
            observation_id: ObservationHandle(next_handle("o")),
            environment,
            window: window.clone(),
            image_width: geometry.image_width,
            image_height: geometry.image_height,
            window_width: geometry.window_width,
            window_height: geometry.window_height,
        };
        let tree_revision = match kind {
            ObservationKind::Capture => None,
            ObservationKind::AxTree { tree_revision } => Some(tree_revision),
        };
        self.observations.insert(
            observation.observation_id.0.clone(),
            ObservationRecord {
                window: window.clone(),
                scope: scope.clone(),
                observation: observation.clone(),
                tree_revision,
                created: Instant::now(),
                consumed: false,
            },
        );
        Ok(observation)
    }

    /// Issue an element handle from an unconsumed accessibility-tree
    /// observation. element_token is host-side bookkeeping the backend maps
    /// to its native element reference.
    pub fn issue_element(
        &mut self,
        observation: &ObservationHandle,
        scope: &Scope,
        element_token: u64,
    ) -> Result<ElementHandle, TargetError> {
        let record = self
            .observations
            .get(observation.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        if record.consumed || record.created.elapsed() > LEASE_TTL {
            return Err(TargetError::StaleHandle);
        }
        let tree_revision = record.tree_revision.ok_or(TargetError::Invalid(
            "element handles require an accessibility tree observation",
        ))?;
        let handle = ElementHandle(next_handle("e"));
        self.elements.insert(
            handle.0.clone(),
            ElementRecord {
                observation: observation.clone(),
                window: record.window.clone(),
                scope: scope.clone(),
                element_token,
                tree_revision,
                consumed: false,
            },
        );
        Ok(handle)
    }

    /// Liveness check without consuming anything (status, re-capture, AX
    /// re-read). Requires an active grant, checked before the probe.
    pub fn validate_window(
        &self,
        window: &WindowHandle,
        scope: &Scope,
    ) -> Result<ValidatedWindow, TargetError> {
        let record = self
            .windows
            .get(window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        self.authorizer.check(scope, &record.target)?;
        self.live(record)
    }

    /// Consume a one-shot observation lease before dispatching one action
    /// from it. Successful validation consumes the lease even if the later
    /// dispatch fails, and invalidates element handles issued from it.
    pub fn consume_observation(
        &mut self,
        id: &ObservationHandle,
        scope: &Scope,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ValidatedObservation, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let record = self
            .observations
            .get(id.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        if record.consumed || record.created.elapsed() > LEASE_TTL {
            return Err(TargetError::StaleHandle);
        }
        let window_record = self
            .windows
            .get(record.window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        // Revocation between observation and dispatch blocks the action
        // before any probe access; the grant is spent only when every other
        // check passed and the action is actually released for dispatch.
        self.authorizer.check(scope, &window_record.target)?;
        let window = self.live(window_record)?;
        // An AX observation is bound to the tree revision it read: if the
        // tree advanced before the lease is consumed, the observation is as
        // stale as its element handles.
        if let Some(issued) = record.tree_revision {
            match self.probe.tree_revision(&window_record.window) {
                None => return Err(TargetError::WindowReplaced),
                Some(revision) if revision != issued => {
                    return Err(TargetError::TreeChanged)
                }
                Some(_) => {}
            }
        }
        self.authorizer.spend(scope, &window_record.target)?;
        let validated = ValidatedObservation {
            window,
            observation: record.observation.clone(),
            tree_revision: record.tree_revision,
        };
        self.observations.get_mut(id.as_str()).unwrap().consumed = true;
        Ok(validated)
    }

    /// Consume a one-shot element handle before dispatching one semantic
    /// action. Consumes the parent observation as well, invalidating sibling
    /// handles.
    pub fn consume_element(
        &mut self,
        id: &ElementHandle,
        scope: &Scope,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ValidatedElement, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let record = self
            .elements
            .get(id.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        let parent = self
            .observations
            .get(record.observation.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if record.consumed || parent.consumed || parent.created.elapsed() > LEASE_TTL {
            return Err(TargetError::StaleHandle);
        }
        let window_record = self
            .windows
            .get(record.window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        self.authorizer.check(scope, &window_record.target)?;
        let window = self.live(window_record)?;
        match self.probe.tree_revision(&window_record.window) {
            None => return Err(TargetError::WindowReplaced),
            Some(revision) if revision != record.tree_revision => {
                return Err(TargetError::TreeChanged)
            }
            Some(_) => {}
        }
        self.authorizer.spend(scope, &window_record.target)?;
        let validated = ValidatedElement {
            window,
            element_token: record.element_token,
            tree_revision: record.tree_revision,
        };
        let parent_key = record.observation.clone();
        self.elements.get_mut(id.as_str()).unwrap().consumed = true;
        self.observations
            .get_mut(parent_key.as_str())
            .unwrap()
            .consumed = true;
        Ok(validated)
    }

    fn live(&self, record: &WindowRecord) -> Result<ValidatedWindow, TargetError> {
        if !self.probe.process_instance_alive(&record.instance) {
            return Err(TargetError::ProcessRestarted);
        }
        match self
            .probe
            .window_generation(&record.instance, record.window.window_id)
        {
            None => return Err(TargetError::WindowReplaced),
            Some(generation) if generation != record.window.generation => {
                return Err(TargetError::WindowReplaced)
            }
            Some(_) => {}
        }
        // A website target stays valid only while the window keeps showing
        // the authorized origin: same-window navigation away (or an
        // undeterminable origin) rejects before observation or dispatch.
        if let TargetIdentity::Website { origin, .. } = &record.target {
            if !origin_matches(&self.probe, &record.window, origin) {
                return Err(TargetError::SiteChanged);
            }
        }
        Ok(ValidatedWindow {
            target: record.target.clone(),
            instance: record.instance.clone(),
            window: record.window,
            scope: record.scope.clone(),
        })
    }
}

/// A website target is valid only while the window shows the origin it was
/// authorized and bound for, compared in canonical form so casing or port
/// spelling cannot dodge the check.
fn origin_matches<P: NativeProbe>(probe: &P, window: &WindowIdentity, origin: &str) -> bool {
    probe
        .current_origin(window)
        .as_deref()
        .and_then(|value| normalize_origin(value).ok())
        .as_deref()
        == Some(origin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Mutex;

    struct FakeProbe {
        alive: AtomicBool,
        generation: Mutex<Option<u64>>,
        tree: Mutex<Option<u64>>,
        origin: Mutex<Option<String>>,
        calls: AtomicUsize,
    }

    impl FakeProbe {
        fn new() -> Self {
            Self {
                alive: AtomicBool::new(true),
                generation: Mutex::new(Some(1)),
                tree: Mutex::new(Some(7)),
                origin: Mutex::new(None),
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

    fn window_identity() -> WindowIdentity {
        WindowIdentity {
            window_id: 42,
            generation: 1,
        }
    }

    fn bound(family: AppFamily) -> (TargetRegistry<FakeProbe>, WindowHandle) {
        let mut registry = TargetRegistry::new(FakeProbe::new());
        let target = target(family);
        registry
            .grant(&scope(), &target, GrantKind::ForRun)
            .unwrap();
        let window = registry
            .bind_window(&scope(), target, instance(), window_identity())
            .unwrap();
        (registry, window)
    }

    fn geometry() -> ObservationGeometry {
        ObservationGeometry {
            image_width: 1280,
            image_height: 720,
            window_width: 2560.0,
            window_height: 1440.0,
        }
    }

    fn capture(
        registry: &mut TargetRegistry<FakeProbe>,
        window: &WindowHandle,
    ) -> TargetObservation {
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
                assert_eq!(background_capability(family, Pointer, minimized), Unsupported);
            }
            // Window move/resize was only measured in background/occluded
            // states; minimized is unverified everywhere.
            assert_eq!(background_capability(family, WindowResize, false), Supported);
            assert_eq!(background_capability(family, WindowResize, true), Unverified);
        }
        // AppKit targeted keys work in background/occluded, not minimized.
        assert_eq!(background_capability(Appkit, TargetedKeys, false), Supported);
        assert_eq!(background_capability(Appkit, TargetedKeys, true), Unsupported);
        assert_eq!(background_capability(Chromium, TargetedKeys, false), Unsupported);
        assert_eq!(background_capability(Browser, TargetedKeys, false), Unsupported);
        // Semantic write splits by family; unverified is not supported.
        assert_eq!(background_capability(Appkit, AxSemanticWrite, false), Supported);
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
        assert_eq!(background_capability(Appkit, MenuShortcut, false), Unsupported);
        assert_eq!(background_capability(Chromium, MenuShortcut, false), Unverified);
        assert_eq!(background_capability(Appkit, MenubarPress, false), Unsupported);
        // Chromium ignores AXPosition but honors AXSize; minimized move is
        // unverified even on AppKit.
        assert_eq!(background_capability(Appkit, WindowMove, false), Supported);
        assert_eq!(background_capability(Appkit, WindowMove, true), Unverified);
        assert_eq!(background_capability(Chromium, WindowMove, false), Unsupported);
        assert_eq!(background_capability(Browser, WindowMove, false), Unsupported);
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
        let previous: WindowHandle = serde_json::from_str(&format!(
            "\"w-{}-1-1\"",
            std::process::id()
        ))
        .unwrap();
        assert_eq!(
            registry.validate_window(&previous, &scope()),
            Err(TargetError::UnknownHandle)
        );
        let previous_observation: ObservationHandle = serde_json::from_str(&format!(
            "\"o-{}-1-1\"",
            std::process::id()
        ))
        .unwrap();
        assert_eq!(
            registry.consume_observation(&previous_observation, &scope(), &|| false),
            Err(TargetError::UnknownHandle)
        );
    }

    #[test]
    fn process_exit_or_restart_invalidates_everything() {
        let mut registry = TargetRegistry::new(FakeProbe::new());
        let target = target(AppFamily::Appkit);
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
        registry.consume_element(&first, &scope(), &|| false).unwrap();
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
        registry.consume_element(&element, &scope(), &|| false).unwrap();
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
}
