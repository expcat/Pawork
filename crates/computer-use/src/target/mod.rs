//! Typed target, observation and background-capability contracts (CU-02).
//!
//! A target binds an application, a process instance (pid + start token), a
//! window generation and the issuing workspace/run [Scope]. Models only ever
//! hold opaque handles issued by the host [TargetRegistry]; they cannot
//! submit raw pids, absolute paths or network endpoints. Handles are validated
//! on every use and reject process restart, window destroy/reuse, tree changes,
//! window resize since the observation was taken, stale or unknown handles,
//! cross-run use and out-of-bounds coordinates.
//!
//! Background capability follows the CU-01 measured matrix per application
//! family (see background_capability). Actions without measured background
//! support are rejected by require_background; there is never a fallback to
//! global input, and pointer actions have no background support in any family.
//! User takeover of the same target has no OS-level arbitration (CU-01 T6b),
//! so CU-09 makes it a host-signal: while the host reports the user active on
//! a target application, dispatch gates pause (yielding, without consuming
//! leases) instead of competing. Unverified is not Supported:
//! actions not yet measured on a family are rejected until a real measurement
//! promotes them.
//!
//! This module is platform-free contract code. The host supplies live state
//! through [NativeProbe]; a downstream task implements the macOS probe and
//! the action dispatch behind these types. The existing isolated virtual
//! desktop ([Environment::IsolatedDesktop], RFB route) keeps its own
//! Computer/Backend path and is unaffected.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::approval::{
    normalize_origin, DispatchPermit, GrantKind, TargetAuthorizer, TargetIdentity,
};
use crate::MAX_IMAGE_EDGE;

mod actions;
mod ax_tree;
mod registry;

#[cfg(test)]
mod tests;

pub use actions::*;
pub use ax_tree::*;
pub use registry::*;

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
    /// Consumed by an earlier action, past [LEASE_TTL], or the window no
    /// longer has the size the observation recorded (resized since the
    /// capture/read). Re-observe.
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
    /// An OS permission the native backend needs (Screen Recording,
    /// Accessibility, …) is missing. Reported as-is by discovery and
    /// pre-flight paths; Pawork can never grant it, and validation paths
    /// that cannot verify a target without it fail closed.
    #[error("native OS permission missing: {0}")]
    PermissionMissing(&'static str),
    /// The native probe could not answer although the OS permission is
    /// present (AX call failed, app unresponsive, symbols unavailable).
    /// Access is refused all the same; the cause is kept for diagnosis
    /// instead of being misreported as a missing permission.
    #[error("native probe unavailable: {0}")]
    ProbeUnavailable(String),
    /// The target's application is occupied by another workspace/run (CU-09):
    /// binds, observations and dispatch are exclusive per application so two
    /// runs never interleave on one physical input boundary.
    #[error("target application is occupied by another workspace/run; it must finish or release it first")]
    TargetOccupied,
    /// The user is operating the target (CU-09 host signal); the agent yields
    /// and dispatch stays paused until the host reports the user gave the
    /// target back. Nothing is consumed while paused.
    #[error("user is operating the target; dispatch paused until the user yields it back")]
    UserActive,
    /// The element itself affirmatively refused the action
    /// (kAXErrorActionUnsupported / kAXErrorAttributeUnsupported): a clear
    /// rejection — the AX action call ran and the element declined it, so
    /// nothing was dispatched. Distinct from
    /// [TargetError::BackgroundUnsupported], which rejects the family ×
    /// action combination before any element contact.
    #[error("element does not support this action; nothing was dispatched")]
    ElementUnsupported,
    /// Host-side contract misuse (e.g. element handle from a screenshot).
    #[error("invalid target contract use: {0}")]
    Invalid(&'static str),
    /// The keyboard-target recheck failed (CU-07): the bound window is not
    /// the application's key window, the resolved element is not focused, or
    /// a text-insert target exposes no selection range. Background key
    /// events route to the app's key window and land in its focused view, so
    /// dispatching anyway could not be attributed to the bound target
    /// element. Nothing was dispatched.
    #[error("target window or element does not hold keyboard focus; input would not land in it")]
    NotFocused,
    /// The scope already reached its terminal close (CU-17 review: the run
    /// finished, was cancelled, failed, or the host tore it down). Grants
    /// can never be (re-)minted for it, so a late approval that still sees
    /// the run in a host registry is rejected instead of leaking past the
    /// cleanup.
    #[error("scope is closed; the run already reached a terminal state")]
    ScopeClosed,
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

            /// Wraps a model-supplied string. This does not mean the registry
            /// issued it; unknown strings still fail on use.
            pub(crate) fn from_model(value: impl Into<String>) -> Self {
                Self(value.into())
            }
        }
    };
}
opaque_handle!(
    WindowHandle,
    "Host-issued handle for a bound target window."
);
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
    /// Current size (points) of a live window. None when the window cannot
    /// be proven alive — the same fail-closed discipline as
    /// window_generation. Observations dispatch only while the window keeps
    /// the size they recorded; a resize stales them (StaleHandle).
    fn window_size(&self, instance: &ProcessInstance, window_id: u64) -> Option<(f64, f64)>;
}

/// Fail-closed probe for hosts without a native backend: every liveness
/// check denies, so the registry still records grants and bindings but no
/// gated operation can pass. (CU-17: the GUI path never probes; this only
/// keeps non-macOS hosts constructible.)
#[derive(Clone, Copy, Debug, Default)]
pub struct UnsupportedProbe;

impl NativeProbe for UnsupportedProbe {
    fn process_instance_alive(&self, _instance: &ProcessInstance) -> bool {
        false
    }
    fn window_generation(&self, _instance: &ProcessInstance, _window_id: u64) -> Option<u64> {
        None
    }
    fn tree_revision(&self, _window: &WindowIdentity) -> Option<u64> {
        None
    }
    fn current_origin(&self, _window: &WindowIdentity) -> Option<String> {
        None
    }
    fn window_size(&self, _instance: &ProcessInstance, _window_id: u64) -> Option<(f64, f64)> {
        None
    }
}
