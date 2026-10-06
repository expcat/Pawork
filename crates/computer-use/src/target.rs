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
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::approval::{normalize_origin, DispatchPermit, GrantKind, TargetAuthorizer, TargetIdentity};
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

// ---------- bounded accessibility tree (CU-05) ----------

/// The bounds every accessibility-tree read runs with. The backend uses
/// exactly these on the initial read and on every revision re-check, so an
/// unchanged tree always yields the same revision; host-chosen bounds
/// would break that agreement, so they are fixed here instead of being a
/// per-call knob.
pub const AX_TREE_BOUNDS: AxTreeBounds = AxTreeBounds {
    max_depth: 24,
    max_nodes: 4096,
    max_text: 200,
    max_read_ms: 10_000,
};

/// Hard limits of one bounded tree read: depth, emitted nodes, per-string
/// text length and wall-clock read time. Depth, node and text truncation
/// are deterministic for an unchanged tree and are flagged on the read.
/// Exceeding the time budget fails the read instead, because a timed-out
/// partial tree can support neither a stable revision nor a provable
/// lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxTreeBounds {
    /// Deepest generation visited below the window root (root depth = 0).
    pub max_depth: u32,
    /// Most nodes emitted by one read, root included.
    pub max_nodes: u32,
    /// Longest kept name, in chars; longer names are cut and flagged.
    pub max_text: usize,
    /// Wall-clock budget of one read in milliseconds.
    pub max_read_ms: u64,
}

impl AxTreeBounds {
    /// Structural validity: every bound nonzero and within the hard
    /// ceilings, so a read is always finite.
    pub fn validate(&self) -> Result<(), TargetError> {
        const MAX_DEPTH: u32 = 64;
        const MAX_NODES: u32 = 8192;
        const MAX_TEXT: usize = 1024;
        const MAX_READ_MS: u64 = 30_000;
        if self.max_depth == 0
            || self.max_depth > MAX_DEPTH
            || self.max_nodes == 0
            || self.max_nodes > MAX_NODES
            || self.max_text == 0
            || self.max_text > MAX_TEXT
            || self.max_read_ms == 0
            || self.max_read_ms > MAX_READ_MS
        {
            return Err(TargetError::Invalid(
                "accessibility tree bounds out of range",
            ));
        }
        Ok(())
    }

    pub fn max_read(&self) -> Duration {
        Duration::from_millis(self.max_read_ms)
    }
}

/// One element of a bounded tree read. Names are readable labels only
/// (AXTitle, falling back to AXDescription); element content (AXValue) is
/// never read, so reading a tree never scoops up passwords, tokens or
/// document text. Secure-input roles are emitted with role, state, frame
/// and actions only; even their name is not read.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AxTreeNode {
    /// Child-index path from the window root within this read (root: []):
    /// host-side bookkeeping that lets a later dispatch re-resolve the
    /// element after the tree-revision check proved the tree unchanged.
    /// Never a cross-observation identity.
    pub path: Vec<u32>,
    pub depth: u32,
    pub role: String,
    /// Readable name, truncated to [AxTreeBounds::max_text]; None when the
    /// element has no usable name or is a secure-input role.
    pub name: Option<String>,
    pub name_truncated: bool,
    /// The name read itself failed (neither AXTitle nor AXDescription
    /// answered): None here is then NOT evidence of absence, and a
    /// name-based lookup over this read is unprovable (see
    /// [lookup_element]). Also flags the read via
    /// [AxTreeTruncation::names].
    pub name_unreadable: bool,
    /// Readable state; None when the element does not expose it or the
    /// attribute could not be read (never a claim of false).
    pub enabled: Option<bool>,
    pub focused: Option<bool>,
    /// Window-local frame [x, y, w, h] in points; None when the element
    /// exposes no position/size. Window-local, so a pure window move
    /// changes neither the emitted nodes nor the revision.
    pub frame: Option<[f64; 4]>,
    /// Actions the element reports (AXPress, …).
    pub actions: Vec<String>,
    /// Children the application reported, whether or not they were
    /// visited within the bounds.
    pub children: u32,
    /// Children exist but were not visited because of the depth bound.
    pub depth_limited: bool,
}

/// Where one read is less than the full truth. The depth, node and text
/// bounds cut a read short deterministically (the time budget never
/// truncates; it fails the read); `names` records that at least one
/// element's name could not be read — not a bound, but the same class of
/// incompleteness: a name-based lookup over the read is unprovable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AxTreeTruncation {
    pub depth: bool,
    pub nodes: bool,
    pub text: bool,
    /// At least one element's name read failed; the emitted None names
    /// are not all proven absences.
    pub names: bool,
}

impl AxTreeTruncation {
    pub fn any(&self) -> bool {
        self.depth || self.nodes || self.text || self.names
    }
}

/// One bounded read of a window's accessibility tree. nodes is preorder
/// and nodes[0] is always the window root, so a successful read is never
/// empty; an application exposing no elements beyond the window frame
/// yields exactly the root node. Read failures (missing permission,
/// unresponsive application, dead window) are [TargetError]s, never an
/// empty or partial read, so "no elements" is always an affirmative
/// answer rather than a disguised failure.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AxTreeRead {
    pub window: WindowIdentity,
    /// Revision of this read, from [ax_tree_revision]. The host registers
    /// it as the [ObservationKind::AxTree] revision; element handles
    /// issued from the observation stay valid only while the backend's
    /// current read fingerprints identically.
    pub tree_revision: u64,
    pub nodes: Vec<AxTreeNode>,
    pub truncation: AxTreeTruncation,
}

/// Deterministic revision of one bounded read: FNV-1a over the canonical
/// node stream plus the truncation flags. Same tree and same bounds yield
/// the same revision; an emitted difference (role, name, state, frame,
/// actions, child counts, truncation) changes the input stream. The
/// fingerprint is a 64-bit non-cryptographic hash: it is a cheap change
/// signal, not a proof — a colliding edit would go unnoticed, which is an
/// accepted limitation of the fixed-size digest. Frames are
/// window-local, so a pure window move does not advance the revision
/// (CU-04 move tolerance) while a resize or layout change does.
pub fn ax_tree_revision(nodes: &[AxTreeNode], truncation: AxTreeTruncation) -> u64 {
    /// FNV-1a with explicit tags and length prefixes, so optional and
    /// variable-length fields stay unambiguous.
    struct Hash(u64);
    impl Hash {
        fn bytes(&mut self, bytes: &[u8]) {
            for &byte in bytes {
                self.0 ^= u64::from(byte);
                self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
            }
        }
        fn number(&mut self, value: u64) {
            self.bytes(&value.to_le_bytes());
        }
        fn text(&mut self, text: &str) {
            self.number(text.len() as u64);
            self.bytes(text.as_bytes());
        }
        fn flag(&mut self, value: bool) {
            self.bytes(&[u8::from(value)]);
        }
    }
    let mut hash = Hash(0xcbf2_9ce4_8422_2325);
    for node in nodes {
        hash.number(u64::from(node.depth));
        hash.text(&node.role);
        hash.flag(node.name.is_some());
        if let Some(name) = &node.name {
            hash.text(name);
        }
        hash.flag(node.name_truncated);
        hash.flag(node.name_unreadable);
        for state in [node.enabled, node.focused] {
            hash.bytes(&[match state {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            }]);
        }
        hash.flag(node.frame.is_some());
        if let Some(frame) = node.frame {
            for component in frame {
                hash.number(component.to_bits());
            }
        }
        hash.number(node.actions.len() as u64);
        for action in &node.actions {
            hash.text(action);
        }
        hash.number(u64::from(node.children));
        hash.flag(node.depth_limited);
    }
    hash.number(nodes.len() as u64);
    hash.flag(truncation.depth);
    hash.flag(truncation.nodes);
    hash.flag(truncation.text);
    hash.flag(truncation.names);
    hash.0
}

/// Element lookup criteria: exact role and/or case-insensitive name
/// substring. At least one is required.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementQuery {
    pub role: Option<String>,
    pub name: Option<String>,
}

impl ElementQuery {
    pub fn new(role: Option<&str>, name: Option<&str>) -> Result<Self, TargetError> {
        let role = role
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if role.is_none() && name.is_none() {
            return Err(TargetError::Invalid("element query needs a role or a name"));
        }
        Ok(Self { role, name })
    }

    fn matches(&self, node: &AxTreeNode) -> bool {
        if let Some(role) = &self.role {
            if &node.role != role {
                return false;
            }
        }
        if let Some(name) = &self.name {
            let Some(candidate) = &node.name else {
                return false;
            };
            if !candidate.to_lowercase().contains(&name.to_lowercase()) {
                return false;
            }
        }
        true
    }
}

/// The honest outcome of one lookup over a bounded read. A lookup never
/// silently picks the first of several matches, and never issues
/// uniqueness it cannot prove.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementLookup {
    /// Exactly one match in a complete (untruncated) tree: the host may
    /// issue an element handle for nodes[index].
    Unique { index: usize },
    /// Zero matches. tree_truncated marks whether the unread part of the
    /// tree (or an unreadable name, for a name query) could have hidden a
    /// match, so "not found" is never confused with "not read".
    NoMatch { tree_truncated: bool },
    /// More than one match; matches counts them within the read (a lower
    /// bound when the tree was truncated). Refine the query.
    Ambiguous {
        matches: usize,
        tree_truncated: bool,
    },
    /// Exactly one match but an unprovable read: an untraversed subtree
    /// (or an unreadable name, for a name query) could hold another
    /// match, so uniqueness is unproven and the host must not issue a
    /// handle from it.
    UnprovenUnique { index: usize },
}

/// Locate elements of one bounded read by role and/or name. Provability
/// is exact: depth/node truncation hides whole subtrees from every
/// query, while text truncation and unreadable names only undermine
/// queries that read names — a role-only query over a structurally
/// complete read stays provable even when some names were cut or
/// unreadable.
pub fn lookup_element(read: &AxTreeRead, query: &ElementQuery) -> ElementLookup {
    let truncated = read.truncation.any();
    let provable = !(read.truncation.depth || read.truncation.nodes)
        && (query.name.is_none() || !(read.truncation.text || read.truncation.names));
    let matches: Vec<usize> = read
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| query.matches(node))
        .map(|(index, _)| index)
        .collect();
    match matches.len() {
        0 => ElementLookup::NoMatch {
            tree_truncated: truncated,
        },
        1 if !provable => ElementLookup::UnprovenUnique { index: matches[0] },
        1 => ElementLookup::Unique { index: matches[0] },
        n => ElementLookup::Ambiguous {
            matches: n,
            tree_truncated: truncated,
        },
    }
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

// ---------- semantic element actions (CU-06) ----------

/// Longest text one semantic value write carries, in chars.
pub const MAX_SEMANTIC_TEXT_CHARS: usize = 4096;

/// Semantic actions dispatched on a validated element handle (CU-06). Only
/// what CU-01 measured as non-interfering background actions exists here:
/// value writes (AppKit) and presses (web pages). CU-01 measured no
/// standalone selection action, so none exists — [SemanticAction::InsertText]
/// is the measured "select" primitive (it replaces the current selection).
/// Every action maps to a [NativeActionKind] and passes the same
/// [require_background] gate, so an Unverified or Unsupported family
/// combination is rejected exactly like any other unproven action — there
/// is never a fallback to global or foreground input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SemanticAction {
    /// AXPress on the element (CU-01: web pages, background and minimized).
    Press,
    /// Replace the element's whole value (AXValue write; CU-01: AppKit).
    SetValue(String),
    /// Replace the element's current selection (AXSelectedText write;
    /// CU-01: AppKit): insertion at a collapsed cursor, type-over of a
    /// non-empty selection.
    InsertText(String),
}

impl SemanticAction {
    /// The capability-matrix kind this action is gated by.
    pub fn kind(&self) -> NativeActionKind {
        match self {
            SemanticAction::Press => NativeActionKind::AxPress,
            SemanticAction::SetValue(_) | SemanticAction::InsertText(_) => {
                NativeActionKind::AxSemanticWrite
            }
        }
    }

    /// Value writes carry the text payloads; presses do not.
    pub fn is_value_write(&self) -> bool {
        !matches!(self, SemanticAction::Press)
    }

    /// Structural validity: text payloads stay within the fixed bound
    /// (empty text is a real payload — clearing a field).
    pub fn validate(&self) -> Result<(), TargetError> {
        let text = match self {
            SemanticAction::Press => None,
            SemanticAction::SetValue(text) | SemanticAction::InsertText(text) => Some(text),
        };
        if let Some(text) = text {
            if text.chars().count() > MAX_SEMANTIC_TEXT_CHARS {
                return Err(TargetError::Invalid(
                    "semantic action text exceeds its bound",
                ));
            }
        }
        Ok(())
    }
}

/// The honest terminal state of a dispatched semantic action (CU-06).
/// Clear rejections are [TargetError]s decided before the element's AX
/// action call (plus the element's own affirmative refusal); both Ok
/// outcomes mean the action call was issued, so neither may be blindly
/// retried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticOutcome {
    /// The AX action call returned success: dispatched — not proof of
    /// effect (a target can accept a call and ignore it; CU-01 measured a
    /// Chromium editor doing exactly that, which is why that family ×
    /// action cell is Unsupported). Verify with a fresh observation or a
    /// target-side fact.
    Dispatched,
    /// The action call did not answer within its messaging timeout
    /// (kAXErrorCannotComplete): the element may or may not have performed
    /// the action. Reconcile from a fresh observation before any retry.
    UnknownEffect,
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
    /// Single-use credential for the one dispatch this consume released
    /// (CU-06): the backend's first dispatch attempt retires it, so a
    /// cloned validation result can never dispatch twice.
    pub dispatch_permit: DispatchPermit,
}

/// Host-side registry: the only issuer and validator of handles. Validation
/// order on every use is cancellation, existence, scope, staleness,
/// authorization (CU-16), process instance, window generation, tree
/// revision, window size; a one-shot grant is spent only after every other
/// check passed.
/// Authorization is checked before any probe call, so a denied, unknown or
/// revoked target never reaches the native backend.
pub struct TargetRegistry<P: NativeProbe> {
    probe: P,
    authorizer: TargetAuthorizer,
    windows: HashMap<String, WindowRecord>,
    observations: HashMap<String, ObservationRecord>,
    elements: HashMap<String, ElementRecord>,
    /// CU-09 occupancy: application anchor -> owning scope and last activity.
    /// Input is dispatched to an application process, so the app (or the
    /// browser hosting a website target) is the boundary two runs must not
    /// share. Owners idle past [LEASE_TTL] are treated as done.
    occupied: HashMap<String, (Scope, Instant)>,
    /// Applications the user took over (CU-09 host signal): dispatch gates
    /// reject with [TargetError::UserActive] while paused.
    user_paused: HashSet<String>,
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
            occupied: HashMap::new(),
            user_paused: HashSet::new(),
        }
    }

    pub fn authorizer(&self) -> &TargetAuthorizer {
        &self.authorizer
    }

    pub fn authorizer_mut(&mut self) -> &mut TargetAuthorizer {
        &mut self.authorizer
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
        // CU-09: revocation keeps the CU-16 contract (in-flight leases are
        // blocked with NotAuthorized before any probe, consumed history stays
        // untouched) and additionally releases the scope's occupancy of the
        // application boundary so another run may take the target.
        if self
            .occupied
            .get(target.occupancy_anchor())
            .is_some_and(|(owner, _)| owner == scope)
        {
            self.occupied.remove(target.occupancy_anchor());
        }
        self.authorizer.revoke(scope, target);
    }

    /// Drop every grant of a run (run teardown).
    pub fn revoke_scope(&mut self, scope: &Scope) {
        self.authorizer.revoke_scope(scope);
        self.release_scope(scope);
    }

    /// CU-09 host signal: the user is operating this target's application.
    /// The agent yields — dispatch gates pause without consuming leases until
    /// [Self::resume_dispatch] reports the user gave the target back. Hosts
    /// that cannot determine the user's intent keep the pause asserted
    /// (fail-closed) instead of competing for the target.
    pub fn pause_dispatch(&mut self, target: &TargetIdentity) {
        self.user_paused.insert(target.occupancy_anchor().to_string());
    }

    /// CU-09 host signal: the user yielded the target back; agent dispatch may
    /// resume. Returns whether a pause was actually cleared.
    pub fn resume_dispatch(&mut self, target: &TargetIdentity) -> bool {
        self.user_paused.remove(target.occupancy_anchor())
    }

    /// CU-09 run end (stop, cancel, timeout, host disconnect): every handle
    /// issued to the scope becomes invalid and its target occupancy is
    /// released. Authorizations are untouched; use [Self::revoke_scope] to
    /// drop them as well.
    pub fn release_scope(&mut self, scope: &Scope) {
        self.observations.retain(|_, record| record.scope != *scope);
        self.elements.retain(|_, record| record.scope != *scope);
        self.windows.retain(|_, record| record.scope != *scope);
        self.occupied.retain(|_, (owner, _)| owner != scope);
    }

    /// Pre-flight gate the host must pass before connecting to, launching,
    /// capturing or reading a bound target. Non-consuming: each gated
    /// registry operation re-checks and spends one-shot grants itself.
    /// Ledger denial (NotAuthorized / ForbiddenTarget) precedes any probe
    /// call, and so does the occupancy gate: a window handle whose run lost
    /// the application to another run never reaches an origin probe. Like
    /// [Self::validate_window], the pre-flight never refreshes occupancy;
    /// activity is refreshed only by binding and successful gated use. A
    /// website binding then re-pins the window's current origin, so
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
        self.occupancy_conflict(&record.target, scope)?;
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
    /// does not spend a one-shot grant, and occupancy is claimed only after
    /// liveness and origin checks pass — a failed binding never strands
    /// occupancy another run would have to wait out.
    pub fn bind_window(
        &mut self,
        scope: &Scope,
        target: TargetIdentity,
        instance: ProcessInstance,
        window: WindowIdentity,
    ) -> Result<WindowHandle, TargetError> {
        self.authorizer.check(scope, &target)?;
        self.occupancy_conflict(&target, scope)?;
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
        self.touch_occupancy(&target, scope);
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
        let bound_target = record.target.clone();
        // Authorization precedes any probe call; the one-shot grant is spent
        // only after the window passed liveness, so a stale target does not
        // burn the user's single approval.
        self.authorizer.check(scope, &record.target)?;
        self.occupancy_conflict(&record.target, scope)?;
        self.live(record)?;
        // The registered geometry must still be the window's live size:
        // a capture taken before a resize is born stale.
        self.size_current(
            &record.instance,
            &record.window,
            (geometry.window_width, geometry.window_height),
        )?;
        self.authorizer.spend(scope, &record.target)?;
        self.touch_occupancy(&bound_target, scope);
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
        self.occupancy_conflict(&record.target, scope)?;
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
        let observation = record.observation.clone();
        let tree_revision = record.tree_revision;
        let window_record = self
            .windows
            .get(record.window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        // Revocation between observation and dispatch blocks the action
        // before any probe access; the grant is spent only when every other
        // check passed and the action is actually released for dispatch.
        self.authorizer.check(scope, &window_record.target)?;
        let dispatch_target = window_record.target.clone();
        self.occupancy_conflict(&dispatch_target, scope)?;
        self.dispatch_paused(&dispatch_target)?;
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
        // A window resized since the observation stales it: coordinates
        // would no longer map onto the observed image.
        self.size_current(
            &window_record.instance,
            &window_record.window,
            (
                record.observation.window_width,
                record.observation.window_height,
            ),
        )?;
        self.authorizer.spend(scope, &window_record.target)?;
        self.touch_occupancy(&dispatch_target, scope);
        let validated = ValidatedObservation {
            window,
            observation,
            tree_revision,
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
        let element_token = record.element_token;
        let element_tree_revision = record.tree_revision;
        let parent_key = record.observation.clone();
        let window_record = self
            .windows
            .get(record.window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        self.authorizer.check(scope, &window_record.target)?;
        let dispatch_target = window_record.target.clone();
        self.occupancy_conflict(&dispatch_target, scope)?;
        self.dispatch_paused(&dispatch_target)?;
        let window = self.live(window_record)?;
        match self.probe.tree_revision(&window_record.window) {
            None => return Err(TargetError::WindowReplaced),
            Some(revision) if revision != record.tree_revision => {
                return Err(TargetError::TreeChanged)
            }
            Some(_) => {}
        }
        // Same staleness rule as the parent observation: a resized window
        // invalidates the geometry every element handle was read against.
        self.size_current(
            &window_record.instance,
            &window_record.window,
            (
                parent.observation.window_width,
                parent.observation.window_height,
            ),
        )?;
        let dispatch_permit = self
            .authorizer
            .spend_for_element_dispatch(scope, &window_record.target)?;
        self.touch_occupancy(&dispatch_target, scope);
        let validated = ValidatedElement {
            window,
            element_token,
            tree_revision: element_tree_revision,
            dispatch_permit,
        };
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

    /// Check-only half of the occupancy gate, usable while record borrows are
    /// alive: a live foreign owner rejects with [TargetError::TargetOccupied].
    fn occupancy_conflict(&self, target: &TargetIdentity, scope: &Scope) -> Result<(), TargetError> {
        if let Some((owner, active)) = self.occupied.get(target.occupancy_anchor()) {
            if owner != scope && active.elapsed() <= LEASE_TTL {
                return Err(TargetError::TargetOccupied);
            }
        }
        Ok(())
    }

    /// Refresh or take occupancy for a scope that just passed
    /// [Self::occupancy_conflict]; also drops owners idle past [LEASE_TTL].
    fn touch_occupancy(&mut self, target: &TargetIdentity, scope: &Scope) {
        let anchor = target.occupancy_anchor();
        match self.occupied.get_mut(anchor) {
            Some((owner, active)) if owner == scope => *active = Instant::now(),
            Some((_, active)) if active.elapsed() <= LEASE_TTL => {}
            _ => {
                self.occupied
                    .retain(|_, (_, active)| active.elapsed() <= LEASE_TTL);
                self.occupied
                    .insert(anchor.to_string(), (scope.clone(), Instant::now()));
            }
        }
    }

    /// CU-09 user takeover: dispatch gates reject while the host reports the
    /// user operating the target's application. Nothing is consumed.
    fn dispatch_paused(&self, target: &TargetIdentity) -> Result<(), TargetError> {
        if self.user_paused.contains(target.occupancy_anchor()) {
            Err(TargetError::UserActive)
        } else {
            Ok(())
        }
    }

    /// The observation's recorded window size must still be the live size.
    /// A window the probe cannot prove alive is replaced (fail-closed, same
    /// as a generation miss); a live window of a different size stales the
    /// observation — image coordinates would map onto the wrong points.
    fn size_current(
        &self,
        instance: &ProcessInstance,
        window: &WindowIdentity,
        recorded: (f64, f64),
    ) -> Result<(), TargetError> {
        match self.probe.window_size(instance, window.window_id) {
            None => Err(TargetError::WindowReplaced),
            Some(current) if !size_matches(recorded, current) => Err(TargetError::StaleHandle),
            Some(_) => Ok(()),
        }
    }
}

/// Two window sizes agree within half a point (the backend's frame
/// tolerance). A sub-half-point resize shifts image mapping by at most one
/// image pixel at the 2x capture scale; anything larger stales the
/// observation.
fn size_matches(recorded: (f64, f64), current: (f64, f64)) -> bool {
    const EPSILON: f64 = 0.5;
    (recorded.0 - current.0).abs() <= EPSILON && (recorded.1 - current.1).abs() <= EPSILON
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
        registry.grant(&scope(), &site_a, GrantKind::ForRun).unwrap();
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
            .bind_window(&other, target(AppFamily::Appkit), instance(), window_identity())
            .is_ok());
    }

    #[test]
    fn idle_occupancy_lapses_for_the_next_run() {
        let (mut registry, _window) = bound(AppFamily::Appkit);
        let other = Scope::new("ws-1", "run-2");
        registry
            .grant(&other, &target(AppFamily::Appkit), GrantKind::ForRun)
            .unwrap();
        // An owner idle past LEASE_TTL is treated as done: its freshest
        // observation is expired at the same moment.
        if let Some((_, active)) = registry.occupied.get_mut("com.example.app") {
            *active = Instant::now() - LEASE_TTL - Duration::from_secs(1);
        }
        assert!(registry
            .bind_window(&other, target(AppFamily::Appkit), instance(), window_identity())
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
        registry.grant(&scope(), &site, GrantKind::ForRun).unwrap();
        *registry.probe.origin.lock().unwrap() = Some("https://example.com".into());
        let window = registry
            .bind_window(&scope(), site, instance(), window_identity())
            .unwrap();
        let other = Scope::new("ws-1", "run-2");
        let site_other = TargetIdentity::website(browser, "https://other.example").unwrap();
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
        registry
            .grant(&other, &target(AppFamily::Appkit), GrantKind::ForRun)
            .unwrap();
        assert!(registry
            .bind_window(&other, target(AppFamily::Appkit), instance(), window_identity())
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
        let validated = registry.consume_element(&first, &scope(), &|| false).unwrap();
        // The consume minted a single-use dispatch permit bound to this
        // scope and target: the first dispatch attempt retires it, so a
        // cloned validation result can never dispatch twice.
        let permit = validated.dispatch_permit.clone();
        let authorizer = registry.authorizer_mut();
        assert!(
            authorizer
                .consume_dispatch_permit(&scope(), &target(AppFamily::Appkit), &permit)
                .is_ok()
        );
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
        assert_eq!(revision, ax_tree_revision(&nodes, AxTreeTruncation::default()));
        // Each of these emitted differences changes the fingerprint (the
        // 64-bit FNV-1a digest is a cheap change signal, not a
        // collision-proof — see the fn docs).
        let mut renamed = nodes.clone();
        renamed[0].name = Some("Other".to_string());
        assert_ne!(revision, ax_tree_revision(&renamed, AxTreeTruncation::default()));
        let mut unreadable = nodes.clone();
        unreadable[1].name_unreadable = true;
        assert_ne!(
            revision,
            ax_tree_revision(&unreadable, AxTreeTruncation::default())
        );
        let mut moved = nodes.clone();
        moved[1].frame = Some([0.0, 8.0, 100.0, 40.0]);
        assert_ne!(revision, ax_tree_revision(&moved, AxTreeTruncation::default()));
        let mut focused = nodes.clone();
        focused[1].focused = Some(true);
        assert_ne!(revision, ax_tree_revision(&focused, AxTreeTruncation::default()));
        let mut actions = nodes.clone();
        actions[1].actions.push("AXShowMenu".to_string());
        assert_ne!(revision, ax_tree_revision(&actions, AxTreeTruncation::default()));
        let mut counts = nodes.clone();
        counts[0].children = 2;
        assert_ne!(revision, ax_tree_revision(&counts, AxTreeTruncation::default()));
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
        assert_ne!(revision, ax_tree_revision(&extra, AxTreeTruncation::default()));
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
            lookup_element(&complete, &ElementQuery::new(Some("AXTextArea"), None).unwrap()),
            ElementLookup::Unique { index: 3 }
        );
        // Several matches never resolve to the first.
        assert_eq!(
            lookup_element(&complete, &ElementQuery::new(Some("AXButton"), None).unwrap()),
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
            lookup_element(&complete, &ElementQuery::new(Some("AXSlider"), None).unwrap()),
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
            lookup_element(&truncated, &ElementQuery::new(Some("AXTextArea"), None).unwrap()),
            ElementLookup::UnprovenUnique { index: 3 }
        );
        assert_eq!(
            lookup_element(&truncated, &ElementQuery::new(Some("AXSlider"), None).unwrap()),
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
            lookup_element(&names_lost, &ElementQuery::new(Some("AXTextArea"), None).unwrap()),
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
            lookup_element(&complete, &ElementQuery::new(Some("AXTextArea"), Some("x")).unwrap()),
            ElementLookup::NoMatch {
                tree_truncated: false
            }
        );
    }
}
