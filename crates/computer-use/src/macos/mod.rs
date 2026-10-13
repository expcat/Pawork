//! macOS native target backend (CU-03): application and window discovery,
//! window-generation tracking and activation-free launching by bundle
//! identity.
//!
//! Discovery returns the minimal target description: the application
//! identity (bundle id + measured-capability family), the process instance
//! (pid + start token) and window-server metadata. Discovery reads only
//! window-server metadata — never titles (content) and never the AX tree
//! (target content access): both require an active grant for the exact
//! target and scope plus proof that the process instance actually belongs
//! to the authorized identity, all checked before any read in
//! [MacosNative::list_windows]; titles are copied only for the authorized
//! target's own windows. Without the OS Screen Recording permission
//! titles are simply absent. OS permissions are reported exactly as they
//! stand and are never requested.
//!
//! Launching accepts only a bundle identity that LaunchServices resolves —
//! never an executable path — and requires an active grant that is checked
//! before any system call; the app is launched with
//! NSWorkspaceLaunchWithoutActivation, so launching, selecting or
//! refreshing a target never activates it or steals focus.
//!
//! Window liveness needs more than the window-server list: closed windows
//! linger there as ghost entries whose attributes match live windows
//! (CU-03, TextEdit/Terminal), so list presence alone never proves a
//! window alive. This backend cross-validates against the app's own
//! AXWindows — the app's authoritative view of its real windows. The
//! correspondence is exact whenever the private but long-stable
//! _AXUIElementGetWindow export maps AX windows to window-server ids: a
//! ghost id is never mapped, so a closed window dies immediately even when
//! a same-frame window survives. Without that export the backend falls
//! back to frame-group closure (a window is live only when its frame
//! group's window-server count equals its AX count and no AX window of
//! the group matches a window-server window outside it); ambiguity then
//! fails closed and can collateral-invalidate a same-frame survivor until
//! the ghost is reaped — documented and still sound. The Accessibility
//! permission is required for either mode: when it is missing the
//! authorized window path reports PermissionMissing, AX call failures for
//! other reasons report ProbeUnavailable, and handle validation always
//! fails closed.
//!
//! Snapshot discipline: enumeration, generation-ledger sync and generation
//! extraction run in one critical section, so an older enumeration can
//! never overwrite newer ledger state and every reported generation comes
//! from the snapshot it was validated against.
//!
//! Bounded accessibility-tree reads (CU-05) sit on the same discipline:
//! read_ax_tree gates ledger authorization, process instance, identity
//! binding and the Accessibility preflight before any content read, then
//! walks the window's AX subtree bounded in depth, nodes, text and time.
//! The tree revision is the deterministic fingerprint of one bounded read
//! recomputed on demand, so an unchanged tree fingerprints identically and
//! any emitted change stales tree-bound handles; every read failure is
//! None / an error (fail-closed), never an empty tree disguised as an
//! application without elements. Element content (AXValue) is never read
//! and secure-input fields (identified by the AXSecureTextField subrole,
//! conservatively on identification failure) yield no name, so a tree
//! read cannot scoop up passwords, tokens or document text.
//!
//! Chromium-family targets serialize their real tree only once a client
//! sets AXManualAccessibility on the application element (the read-enable
//! VoiceOver performs — idempotent, no UI change); every tree read sets
//! it best-effort before walking. The renderer then materializes the tree
//! asynchronously (VS Code took on the order of a minute in CU-05
//! acceptance), so the first read of a fresh Chromium window may honestly
//! answer with window chrome only; later reads converge to the full tree.
//!
//! Fail-closed limits: this backend has no signal source for the current
//! web origin yet (CU-15 territory), so current_origin reports None. The
//! registry treats that as unavailable: website targets cannot validate
//! against this probe and are rejected (SiteChanged) before dispatch.
//! Application discovery, window binding and capture observations are
//! unaffected.
//!
//! Targeted background input (CU-07) rides CGEventPostToPid — key events
//! posted to the target pid only, never a session tap, never the user's
//! event stream. The dispatch rechecks everything the route depends on
//! right before the first event: the window is the app's key window
//! (AXMain), the resolved element is focused, a text insert target
//! exposes a selection range, and the window is not minimized (AppKit
//! drops background key events for minimized windows). The posting loop
//! checks cancellation and target liveness between balanced steps and
//! always releases pressed state on an early stop; a posted sequence is
//! dispatch proof only, never effect proof.
use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::ptr;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use block::ConcreteBlock;
use core_foundation::base::{CFType, ItemRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::{CFNumber, CFNumberRef};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::access::ScreenCaptureAccess;
use core_graphics::display::CGMainDisplayID;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventType, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use core_graphics::window as cgwindow;
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, RgbImage};
use objc::rc::autoreleasepool;
use objc::runtime::{Object, BOOL, YES};
use objc::{class, msg_send, sel, sel_impl};

use crate::approval::{TargetAuthorizer, TargetIdentity};
use crate::target::{
    ax_tree_revision, require_background, ActivityProof, AppFamily, AppIdentity, AxTreeNode,
    AxTreeRead, AxTreeTruncation, KeyModifiers, NativeProbe, PointerAction, ProcessInstance, Scope,
    SemanticAction, SemanticOutcome, TargetError, TargetedInput, TargetedKey, TargetedOutcome,
    ValidatedElement, ValidatedObservation, ValidatedWindow, ValidatedWindowDispatch,
    WindowIdentity, WindowMove, AX_TREE_BOUNDS,
};
use crate::{MAX_IMAGE_BYTES, MAX_IMAGE_EDGE};

mod actions;
mod ax;
mod ax_tree;
mod capture;
mod discovery;
mod input;

#[cfg(test)]
mod tests;

use actions::*;
use ax::*;
use ax_tree::*;
use capture::*;
use discovery::*;
use input::*;

/// NSWorkspaceLaunchWithoutActivation: launch in the background, never
/// activating the app or stealing focus.
const LAUNCH_WITHOUT_ACTIVATION: usize = 0x0000_0200;

// NSWorkspace / NSRunningApplication live in AppKit; force-linking it also
// loads their Objective-C classes into the runtime.
#[link(name = "AppKit", kind = "framework")]
extern "C" {}

/// Window generations: a window id first seen in an enumeration is assigned
/// a fresh generation; ids absent from an enumeration are forgotten, so a
/// destroyed window whose identifier the window server reuses never matches
/// the generation an old handle was bound to. Pure bookkeeping — the
/// platform layer only feeds it snapshots.
#[derive(Default)]
struct WindowGenerations {
    generations: HashMap<u32, u64>,
    next: u64,
}

impl WindowGenerations {
    fn sync(&mut self, current: &HashSet<u32>) {
        self.generations.retain(|id, _| current.contains(id));
        for id in current {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.generations.entry(*id) {
                self.next += 1;
                entry.insert(self.next);
            }
        }
    }

    fn generation(&self, id: u32) -> Option<u64> {
        self.generations.get(&id).copied()
    }
}

/// OS permission state, reported exactly as it stands (preflight only —
/// nothing here ever triggers a TCC prompt). A Pawork grant never implies
/// these; missing permissions surface as absent titles (Screen Recording)
/// or as the unavailable AX signals documented on this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativePermissions {
    /// Screen Recording (TCC): window titles and any capture need it.
    pub screen_recording: bool,
    /// Accessibility (TCC): AX reads and semantic actions need it.
    pub accessibility: bool,
}

/// A discovered application: the minimal description the host needs to let
/// the user pick and authorize a target.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredApp {
    /// Bundle identity plus the measured-capability family (heuristic, see
    /// classify — it only feeds the CU-01 background capability matrix).
    pub identity: AppIdentity,
    /// The process instance live at enumeration time.
    pub instance: ProcessInstance,
    /// Display name. Not window content.
    pub localized_name: String,
    /// Number of standard (layer-0) window-server entries. Pure metadata:
    /// may include closed-window ghosts (discovery performs no AX reads —
    /// those need an authorized target). Exact liveness is the authorized
    /// list_windows path.
    pub window_count: usize,
}

/// Window rectangle in global screen points (top-left origin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A discovered standard window of an application.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredWindow {
    /// Window id plus the generation this backend currently tracks for it.
    pub window: WindowIdentity,
    pub bounds: WindowBounds,
    /// Window title — content. Copied from the window server only when the
    /// caller passed include_titles (list_windows checks the target's grant
    /// before any read), and present only when the Screen Recording
    /// permission is granted; None otherwise.
    pub title: Option<String>,
}

/// A fresh window screenshot (CU-04): JPEG pixels plus the geometry the
/// host registers as the observation. window_* are the window's live size
/// in points from the same snapshot that proved it alive; image_* are the
/// encoded JPEG's pixel dimensions (longest edge <= [MAX_IMAGE_EDGE],
/// bytes <= [MAX_IMAGE_BYTES]). A capture is either complete or an error —
/// no partial frame, stale image or cached result is ever returned.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowCapture {
    /// The exact window (id + generation) that was captured.
    pub window: WindowIdentity,
    pub image_width: u32,
    pub image_height: u32,
    pub window_width: f64,
    pub window_height: f64,
    /// Baseline JPEG, the same ContentPart::Image-compatible form the
    /// isolated-desktop path already persists and sends to models.
    pub jpeg: Vec<u8>,
}

/// Controlled-launch failure. Authorization denials are [TargetError]s and
/// are checked before any system call.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    /// No active grant, revoked, cross-target or protected bundle: rejected
    /// before LaunchServices is touched.
    #[error(transparent)]
    Denied(#[from] TargetError),
    /// The bundle id does not resolve to an installed application. Launch
    /// accepts discovered bundle identities only, never executable paths.
    #[error("no installed application resolves bundle id {0:?}")]
    NotFound(String),
    #[error("application launch failed: {0}")]
    Failed(String),
    /// The process exited between launch and reading its instance token.
    #[error("launched process exited before its instance could be read")]
    ExitedEarly,
}

/// The macOS backend: implements [NativeProbe] for the contract registry
/// and adds discovery, window listing, permission reporting and controlled
/// launching. Clones share the generation ledger (one mutex serializes
/// enumeration, ledger sync and generation extraction); bind and validate
/// through clones of one instance (independently created instances assign
/// generations independently).
#[derive(Clone, Default)]
pub struct MacosNative {
    generations: Arc<Mutex<WindowGenerations>>,
}

struct RawWindow {
    id: u32,
    owner_pid: i32,
    layer: i32,
    bounds: WindowBounds,
    title: Option<String>,
}

/// A window proven alive in one consistent snapshot: its bounds and the
/// generation the ledger tracked for it in that same snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ProvenWindow {
    bounds: WindowBounds,
    generation: u64,
}

impl MacosNative {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current OS permission state. Preflight only — never prompts.
    pub fn permissions(&self) -> NativePermissions {
        NativePermissions {
            screen_recording: ScreenCaptureAccess.preflight(),
            accessibility: ax_is_process_trusted(),
        }
    }

    /// Frontmost-bundle proof only. A nil workspace or application is
    /// unprovable, not a claim that the user is active. A nil bundle id is
    /// idle. Matching is case-insensitive and nothing about keystrokes is kept.
    pub fn target_activity(&self, bundle_id: &str) -> ActivityProof {
        autoreleasepool(|| unsafe {
            let ws: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
            if ws.is_null() {
                return ActivityProof::Unprovable;
            }
            let app: *mut Object = msg_send![ws, frontmostApplication];
            if app.is_null() {
                return ActivityProof::Unprovable;
            }
            let bundle: *mut Object = msg_send![app, bundleIdentifier];
            if bundle.is_null() {
                return ActivityProof::Idle;
            }
            if rs_str(bundle).eq_ignore_ascii_case(bundle_id) {
                ActivityProof::Operating
            } else {
                ActivityProof::Idle
            }
        })
    }

    /// Resolve an installed application's identity (bundle id + classified
    /// family) without launching it. This is how the host names a launch
    /// target: identities come from discovery or this resolution, never
    /// from an executable path.
    pub fn resolve_identity(&self, bundle_id: &str) -> Option<AppIdentity> {
        autoreleasepool(|| unsafe {
            let ws: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
            let url: *mut Object =
                msg_send![ws, URLForApplicationWithBundleIdentifier: nsstr(bundle_id)];
            if url.is_null() {
                return None;
            }
            Some(AppIdentity {
                bundle_id: bundle_id.to_string(),
                family: classify_bundle(url),
            })
        })
    }

    /// Regular (dock-visible) applications with their process instances.
    /// Minimal metadata only: no titles (content) and no AX reads — those
    /// require a grant for the specific target, checked in list_windows.
    /// window_count counts layer-0 window-server entries and may include
    /// closed-window ghosts. Read-only: no activation, no focus change.
    pub fn list_applications(&self) -> Vec<DiscoveredApp> {
        self.with_window_snapshot(None, |windows, _| {
            autoreleasepool(|| unsafe {
                let ws: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
                let apps: *mut Object = msg_send![ws, runningApplications];
                if apps.is_null() {
                    return Vec::new();
                }
                let count: usize = msg_send![apps, count];
                let mut out = Vec::new();
                for i in 0..count {
                    let app: *mut Object = msg_send![apps, objectAtIndex: i];
                    if app.is_null() {
                        continue;
                    }
                    // NSApplicationActivationPolicyRegular == 0: skip agents and
                    // menu-bar/accessory processes.
                    let policy: i64 = msg_send![app, activationPolicy];
                    if policy != 0 {
                        continue;
                    }
                    let bundle: *mut Object = msg_send![app, bundleIdentifier];
                    if bundle.is_null() {
                        continue;
                    }
                    let bundle_id = rs_str(bundle);
                    let pid: i32 = msg_send![app, processIdentifier];
                    let Some(start_token) = process_start_token(pid) else {
                        continue; // exited mid-enumeration
                    };
                    let url: *mut Object = msg_send![app, bundleURL];
                    let window_count = windows
                        .iter()
                        .filter(|w| w.owner_pid == pid && w.layer == 0)
                        .count();
                    out.push(DiscoveredApp {
                        identity: AppIdentity {
                            bundle_id,
                            family: classify_bundle(url),
                        },
                        instance: ProcessInstance {
                            pid: pid as u32,
                            start_token,
                        },
                        localized_name: rs_str(msg_send![app, localizedName]),
                        window_count,
                    });
                }
                out
            })
        })
    }

    /// Standard (layer-0) windows of a live process instance. The grant for
    /// this exact target and scope is checked before anything is read:
    /// without it no AX call is made and no title is copied out of the
    /// window server. The process instance must actually belong to the
    /// authorized identity (its bundle id is read back from the system) —
    /// a grant for one application never unlocks reads of another process.
    /// The AX tree then decides which window-server entries are real
    /// (ghost filtering — see the module docs): exact id correspondence
    /// when _AXUIElementGetWindow is available, frame-group closure
    /// otherwise. The Accessibility permission is required: missing it
    /// reports PermissionMissing, while AX call failures for other reasons
    /// report ProbeUnavailable. Pass include_titles only when the caller
    /// may see window content for this target; titles are copied only for
    /// this target's own windows and are also absent without the Screen
    /// Recording permission.
    pub fn list_windows(
        &self,
        authorizer: &TargetAuthorizer,
        scope: &Scope,
        identity: &AppIdentity,
        instance: &ProcessInstance,
        include_titles: bool,
    ) -> Result<Vec<DiscoveredWindow>, TargetError> {
        // Authorization precedes every AX read and every title read.
        authorizer.check(scope, &TargetIdentity::application(identity.clone()))?;
        self.list_windows_detached(identity, instance, include_titles, &|| false)
    }

    /// Window list after the caller already checked the grant. The registry
    /// lock must not be held across this call: AX window proof can stall,
    /// and a concurrent revoke has to land before the result is adopted.
    pub(crate) fn list_windows_detached(
        &self,
        identity: &AppIdentity,
        instance: &ProcessInstance,
        include_titles: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<DiscoveredWindow>, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        if !self.process_instance_alive(instance) {
            return Err(TargetError::ProcessRestarted);
        }
        let pid = instance.pid as i32;
        // The granted identity must own this process: a TextEdit grant
        // paired with a Chrome instance reads nothing.
        verify_instance_owns_bundle(&identity.bundle_id, pid)?;
        match ax_preflight() {
            AxPreflight::Trusted => {}
            AxPreflight::NotTrusted => {
                return Err(TargetError::PermissionMissing("accessibility"));
            }
            AxPreflight::Unavailable => {
                return Err(TargetError::ProbeUnavailable(
                    "HIServices symbols unavailable".to_string(),
                ));
            }
        }
        let ax = ax_windows(pid).map_err(ax_read_target_error)?;
        self.with_window_snapshot(include_titles.then_some(pid), |windows, ledger| {
            Ok(windows
                .iter()
                .filter(|w| w.owner_pid == pid && w.layer == 0)
                .filter(|w| window_proven_alive(windows, &ax, pid, w))
                .filter_map(|w| {
                    Some(DiscoveredWindow {
                        window: WindowIdentity {
                            window_id: w.id as u64,
                            generation: ledger.generation(w.id)?,
                        },
                        bounds: w.bounds,
                        title: if include_titles {
                            w.title.clone()
                        } else {
                            None
                        },
                    })
                })
                .collect::<Vec<DiscoveredWindow>>())
        })
    }

    /// Launch an application by bundle identity without activating it. The
    /// grant for the application target is checked before any system call:
    /// denied, revoked, cross-target or protected identities are rejected
    /// before LaunchServices is touched, and the model can never submit an
    /// executable path — only a bundle identity that resolves to an
    /// installed application.
    pub fn launch_authorized(
        &self,
        authorizer: &TargetAuthorizer,
        scope: &Scope,
        identity: &AppIdentity,
    ) -> Result<ProcessInstance, LaunchError> {
        authorizer.check(scope, &TargetIdentity::application(identity.clone()))?;
        self.launch_detached(identity, &|| false)
    }

    /// Launch after the caller already checked the grant. Do not hold the
    /// registry lock across LaunchServices.
    pub(crate) fn launch_detached(
        &self,
        identity: &AppIdentity,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ProcessInstance, LaunchError> {
        if cancelled() {
            return Err(LaunchError::Failed("cancelled before launch".to_string()));
        }
        launch(identity)
    }

    /// Capture a fresh screenshot of a validated bound window (CU-04) via
    /// ScreenCaptureKit — never by activating the target, so the user's
    /// frontmost app, focus and pointer stay untouched, and an occluded,
    /// background or minimized window still yields its own content.
    ///
    /// Gate order mirrors the contract: ledger authorization (a denied,
    /// revoked or cross-target window never reaches any OS call), process
    /// instance, window liveness plus generation (the shot is of the exact
    /// window the handle names; closed windows lingering as window-server
    /// ghosts fail here), then the OS Screen Recording preflight — a
    /// missing permission is reported before any capture attempt, and
    /// Pawork never prompts for it. Only then is the ScreenCaptureKit seam
    /// touched (counted in tests). A window that vanishes between liveness
    /// and the capture request reports WindowReplaced; every other
    /// capture-side failure keeps its OS cause as ProbeUnavailable. The
    /// result is a complete fresh frame within the image budget or an
    /// error — never a partial or stale frame.
    pub fn capture_window(
        &self,
        authorizer: &TargetAuthorizer,
        validated: &ValidatedWindow,
    ) -> Result<WindowCapture, TargetError> {
        authorizer.check(&validated.scope, &validated.target)?;
        self.capture_window_detached(validated, &|| false)
    }

    /// Screenshot after the caller already checked the grant. The registry
    /// lock must not be held: ScreenCaptureKit waits up to 10 seconds, and
    /// revoke or takeover has to update the ledger while this runs.
    /// [cancelled] is checked before the capture seam and does not invent a
    /// frame when it fires.
    pub(crate) fn capture_window_detached(
        &self,
        validated: &ValidatedWindow,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowCapture, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        if !self.process_instance_alive(&validated.instance) {
            return Err(TargetError::ProcessRestarted);
        }
        // The granted identity must own this process before any AX read:
        // a grant for one application paired with another app's live
        // instance and window captures nothing (the same binding
        // list_windows enforces).
        verify_instance_owns_bundle(
            &validated.target.app().bundle_id,
            validated.instance.pid as i32,
        )?;
        let proven = self
            .proven_window(&validated.instance, validated.window.window_id)
            .ok_or(TargetError::WindowReplaced)?;
        if proven.generation != validated.window.generation {
            return Err(TargetError::WindowReplaced);
        }
        // The OS permission is reported before the capture seam is touched;
        // this backend never requests it.
        if !ScreenCaptureAccess.preflight() {
            return Err(TargetError::PermissionMissing("screen recording"));
        }
        // A revoke during the liveness proof must not reach the capture seam.
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        #[cfg(test)]
        CAPTURE_SYSTEM_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let image = sck::capture(
            validated.instance.pid as i32,
            u32::try_from(validated.window.window_id)
                .map_err(|_| TargetError::Invalid("window id out of range"))?,
            proven.bounds.width,
            proven.bounds.height,
        )?;
        let rgb =
            bgra_to_rgb(&image.bgra, image.width, image.height, image.stride).ok_or_else(|| {
                TargetError::ProbeUnavailable("captured image has inconsistent pixels".to_string())
            })?;
        let raw =
            RgbImage::from_raw(image.width as u32, image.height as u32, rgb).ok_or_else(|| {
                TargetError::ProbeUnavailable("captured image size mismatch".to_string())
            })?;
        let (jpeg, image_width, image_height) = jpeg_within_budget(&raw)?;
        Ok(WindowCapture {
            window: validated.window,
            image_width,
            image_height,
            window_width: proven.bounds.width,
            window_height: proven.bounds.height,
            jpeg,
        })
    }

    /// Read the bounded accessibility tree of a validated bound window
    /// (CU-05). Gate order: ledger authorization (a denied, revoked or
    /// cross-target window never reaches any OS call), process instance,
    /// identity binding, then the Accessibility preflight — this read IS
    /// AX content access, so a missing permission is PermissionMissing
    /// before the liveness AX reads, never a disguised empty tree — then
    /// window liveness + generation, and only then the bounded walk seam
    /// (counted in tests). The walk is bounded in depth, nodes, text and
    /// time (AX_TREE_BOUNDS): one deadline spans window resolution, the
    /// Chromium read-enable and the walk; every AX call is armed with
    /// the messaging timeout of the budget remaining when it starts and
    /// the clock is re-checked when it returns, a read that cannot set
    /// the timeout refuses, and the clock gets one final check before
    /// delivery — so the budget bounds the WHOLE read, never just one
    /// phase. Deterministic truncation is flagged on the read, a
    /// blown budget fails it. Structural inconsistency (a child list that
    /// contradicts the reported child count) fails the read rather than
    /// posing as a complete branch; a failed name read flags the node and
    /// the read (names) so name-based lookups become explicitly
    /// unprovable instead of treating the name as absent. The window root
    /// is always
    /// nodes[0], so a successful read of a window without exposed elements
    /// returns exactly the root — "no elements" only ever answers a real
    /// question, read failures are errors. The returned revision is the
    /// read's fingerprint; the host registers it as the AxTree
    /// observation's tree_revision.
    /// Before the walk, the read enables the target's full tree
    /// (AXManualAccessibility on the application element, best-effort and
    /// idempotent — see ax_enable_full_tree): Chromium-family apps answer
    /// with window chrome only until it is set and then materialize
    /// asynchronously, so a fresh Chromium window can honestly read as
    /// chrome-only on the first read and converge on later reads.
    pub fn read_ax_tree(
        &self,
        authorizer: &TargetAuthorizer,
        validated: &ValidatedWindow,
    ) -> Result<AxTreeRead, TargetError> {
        authorizer.check(&validated.scope, &validated.target)?;
        self.read_ax_tree_detached(validated, &|| false)
    }

    /// Tree read after the caller already checked the grant. Do not hold the
    /// registry lock across this call.
    pub(crate) fn read_ax_tree_detached(
        &self,
        validated: &ValidatedWindow,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<AxTreeRead, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        if !self.process_instance_alive(&validated.instance) {
            return Err(TargetError::ProcessRestarted);
        }
        // The granted identity must own this process before any AX read:
        // the same binding list_windows and capture_window enforce.
        verify_instance_owns_bundle(
            &validated.target.app().bundle_id,
            validated.instance.pid as i32,
        )?;
        match ax_preflight() {
            AxPreflight::Trusted => {}
            AxPreflight::NotTrusted => {
                return Err(TargetError::PermissionMissing("accessibility"));
            }
            AxPreflight::Unavailable => {
                return Err(TargetError::ProbeUnavailable(
                    "HIServices symbols unavailable".to_string(),
                ));
            }
        }
        let proven = self
            .proven_window(&validated.instance, validated.window.window_id)
            .ok_or(TargetError::WindowReplaced)?;
        if proven.generation != validated.window.generation {
            return Err(TargetError::WindowReplaced);
        }
        // One deadline spans window resolution, the Chromium read-enable
        // and the walk — the budget covers the whole read, not one phase.
        tree_symbols()?;
        let deadline = Instant::now() + AX_TREE_BOUNDS.max_read();
        #[cfg(test)]
        AX_WALK_SYSTEM_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let window_id = u32::try_from(validated.window.window_id)
            .map_err(|_| TargetError::Invalid("window id out of range"))?;
        let element =
            resolve_window_element(self, validated.instance.pid as i32, window_id, deadline)?;
        let (nodes, truncation) = walk_ax_tree(&element, deadline)?;
        Ok(AxTreeRead {
            window: validated.window,
            tree_revision: ax_tree_revision(&nodes, truncation),
            nodes,
            truncation,
        })
    }

    /// Dispatch one semantic action on a validated element handle (CU-06).
    /// Gate order: cancellation, dispatch-side ledger authorization and
    /// payload validity (all pure, before any OS call), process instance,
    /// identity binding, then the CU-01 capability gate — family × action
    /// must be Supported, so a Chromium value write (measured silently
    /// ineffective) is rejected here without a single AX call to the
    /// target, and there is never a fallback to global or foreground
    /// input — then Accessibility preflight, window liveness + generation
    /// (bounded by the same deadline as the tree contact below), and only
    /// then tree contact: the window's subtree is re-walked with the
    /// fixed bounds and must fingerprint to the revision the element
    /// handle was validated against (TreeChanged otherwise), the handle's
    /// child-index path is re-resolved against the live tree, and the
    /// single AX action call runs armed with the remaining read budget.
    /// Authorization retires the single-use [crate::approval::DispatchPermit]
    /// the registry minted when the handle was consumed: the first
    /// dispatch attempt consumes it whatever its later outcome, so a
    /// cloned validation result can never dispatch twice, and a
    /// revocation between consume and dispatch dropped the permit and
    /// blocks here. Cancellation is honored at entry and
    /// once more right before the action call; once the call is issued,
    /// its returned code alone decides the outcome.
    /// The capability gate does not consult the minimized state: both
    /// semantic kinds are minimized-invariant in the CU-01 matrix (pinned
    /// by semantic_capability_gate_is_the_matrix_and_minimized_invariant).
    /// Value writes to secure-input elements are refused (Invalid): the
    /// read side never reads secure content, and the write side never
    /// silently enters it. Terminal states stay honest: clear rejections
    /// are [TargetError]s decided before the action call (or the element's
    /// own affirmative refusal, [TargetError::ElementUnsupported]); the
    /// action call's success is [SemanticOutcome::Dispatched] — not proof
    /// of effect, verify with a fresh observation or a target-side fact —
    /// and a messaging timeout is [SemanticOutcome::UnknownEffect]: the
    /// action may already have happened, so it is never reported as a
    /// clean failure that would invite a blind retry. The action call is
    /// the point of no return: once issued, its returned code alone
    /// decides the outcome.
    pub fn semantic_action(
        &self,
        authorizer: &mut TargetAuthorizer,
        validated: &ValidatedElement,
        path: &[u32],
        action: &SemanticAction,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SemanticOutcome, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        // Dispatch-side authorization: atomically retires the permit the
        // element-handle consume minted. The first dispatch attempt
        // consumes it whatever happens later, so a cloned validation
        // result can never dispatch twice, and a revocation between
        // consume and dispatch dropped the permit and blocks here.
        authorizer.consume_dispatch_permit(
            &validated.window.scope,
            &validated.window.target,
            &validated.dispatch_permit,
        )?;
        self.semantic_action_detached(validated, path, action, cancelled)
    }

    /// Semantic dispatch after the caller already retired the permit.
    /// Do not hold the registry lock across this call.
    pub(crate) fn semantic_action_detached(
        &self,
        validated: &ValidatedElement,
        path: &[u32],
        action: &SemanticAction,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SemanticOutcome, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        action.validate()?;
        if !self.process_instance_alive(&validated.window.instance) {
            return Err(TargetError::ProcessRestarted);
        }
        // The granted identity must own this process before any AX contact:
        // the same binding list_windows, capture_window and read_ax_tree
        // enforce.
        verify_instance_owns_bundle(
            &validated.window.target.app().bundle_id,
            validated.window.instance.pid as i32,
        )?;
        require_background(validated.window.target.app().family, action.kind(), false)?;
        match ax_preflight() {
            AxPreflight::Trusted => {}
            AxPreflight::NotTrusted => {
                return Err(TargetError::PermissionMissing("accessibility"));
            }
            AxPreflight::Unavailable => {
                return Err(TargetError::ProbeUnavailable(
                    "HIServices symbols unavailable".to_string(),
                ));
            }
        }
        // One deadline spans the liveness proof, window resolution, the
        // revision re-walk, the path resolution and the armed action
        // call; a liveness read that outlives it keeps the overtime
        // cause instead of collapsing into WindowReplaced.
        let deadline = Instant::now() + AX_TREE_BOUNDS.max_read();
        let proven = self.proven_window_cause(
            &validated.window.instance,
            validated.window.window.window_id,
            Some(deadline),
        )?;
        if proven.generation != validated.window.window.generation {
            return Err(TargetError::WindowReplaced);
        }
        // The dispatch walks and resolves with the same fixed bounds the
        // read used; the action symbols for the requested kind must exist.
        let f = tree_symbols()?;
        let (Some(set_timeout), Some(copy_values)) =
            (f.set_messaging_timeout, f.copy_attribute_values)
        else {
            return Err(TargetError::ProbeUnavailable(
                "HIServices tree symbols unavailable".to_string(),
            ));
        };
        let perform = match action {
            SemanticAction::Press => Some(f.perform_action.ok_or_else(|| {
                TargetError::ProbeUnavailable("HIServices action symbols unavailable".to_string())
            })?),
            _ => None,
        };
        let set_attribute = if action.is_value_write() {
            Some(f.set_attribute.ok_or_else(|| {
                TargetError::ProbeUnavailable("HIServices action symbols unavailable".to_string())
            })?)
        } else {
            None
        };
        let window_id = u32::try_from(validated.window.window.window_id)
            .map_err(|_| TargetError::Invalid("window id out of range"))?;
        let window_element = resolve_window_element(
            self,
            validated.window.instance.pid as i32,
            window_id,
            deadline,
        )?;
        let (nodes, truncation) = walk_ax_tree(&window_element, deadline)?;
        if ax_tree_revision(&nodes, truncation) != validated.tree_revision {
            return Err(TargetError::TreeChanged);
        }
        // The revision-identical walk contains the issued node; a path
        // that does not resolve in it is host contract misuse (a foreign
        // path paired with this handle's revision), not a tree change.
        if !nodes.iter().any(|node| node.path.as_slice() == path) {
            return Err(TargetError::Invalid(
                "element path does not resolve in the tree the handle was validated against",
            ));
        }
        let budget = ReadBudget {
            set_timeout,
            deadline,
        };
        let element = resolve_element_by_path(copy_values, &window_element, path, &budget)?;
        if action.is_value_write() {
            refuse_secure_write(f, &element, &budget)?;
        }
        // The action call is the point of no return (see the doc comment):
        // it runs armed with the remaining budget, and its returned code
        // alone decides the outcome. Cancellation is honored up to this
        // point; once the call is issued, Dispatched / UnknownEffect stay
        // the honest terminal states.
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        budget.arm(element.0).map_err(budget_target_error)?;
        #[cfg(test)]
        AX_ACTION_SYSTEM_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let err = unsafe {
            match action {
                SemanticAction::Press => {
                    (perform.unwrap())(element.0, nsstr("AXPress") as CFStringRef)
                }
                SemanticAction::SetValue(text) => {
                    let value = CFString::new(text);
                    (set_attribute.unwrap())(
                        element.0,
                        nsstr("AXValue") as CFStringRef,
                        value.as_concrete_TypeRef() as *const c_void,
                    )
                }
                SemanticAction::InsertText(text) => {
                    let value = CFString::new(text);
                    (set_attribute.unwrap())(
                        element.0,
                        nsstr("AXSelectedText") as CFStringRef,
                        value.as_concrete_TypeRef() as *const c_void,
                    )
                }
            }
        };
        semantic_action_result(err)
    }

    /// Dispatch one targeted input (Unicode text or a key press with a
    /// modifier combo) through CGEventPostToPid on a validated element
    /// handle (CU-07). Gate order: cancellation, dispatch-side ledger
    /// authorization (the single-use [crate::approval::DispatchPermit]
    /// retires on the first attempt whatever happens later), payload
    /// validity — all pure, before any OS call — then process instance,
    /// identity binding, the pure family half of the CU-01 capability gate
    /// (a Chromium-family target silently drops background key events, so
    /// it is rejected without a single AX call; there is never a fallback
    /// to session taps or global input), Accessibility preflight, window
    /// liveness + generation (bounded by the same deadline as everything
    /// below), the minimized half of the capability gate (read live off
    /// the window element: AppKit drops background key events for
    /// minimized windows), the tree re-walk fingerprinting to the
    /// revision the handle was validated against, path re-resolution, the
    /// secure-input refusal, and the keyboard-target recheck: the window
    /// must be the app's key window (AXMain) and the resolved element
    /// focused, and a text insert target must expose a selection range —
    /// background key events land in the key window's focused view, so
    /// without these the input could not be attributed to the bound
    /// target element ([TargetError::NotFocused], nothing dispatched).
    /// Cancellation is honored at entry and once more right before the
    /// first event; the posting loop re-checks cancellation, target
    /// liveness and the binding itself between balanced steps (window
    /// generation actively proven, the window still not minimized, the
    /// bound element still focused — each recheck independently bounded
    /// and failing closed), stops the input on any failure and always
    /// releases pressed state on an early stop. Every posted sequence is
    /// [TargetedOutcome::Dispatched] — dispatch proof only, never effect
    /// proof; a partial sequence is [TargetedOutcome::Partial] and is
    /// never retried as if it were clean.
    pub fn targeted_input(
        &self,
        authorizer: &mut TargetAuthorizer,
        validated: &ValidatedElement,
        path: &[u32],
        input: &TargetedInput,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<TargetedOutcome, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        authorizer.consume_dispatch_permit(
            &validated.window.scope,
            &validated.window.target,
            &validated.dispatch_permit,
        )?;
        self.targeted_input_detached(validated, path, input, cancelled)
    }

    /// Targeted input after the caller already retired the permit.
    /// Do not hold the registry lock across this call.
    pub(crate) fn targeted_input_detached(
        &self,
        validated: &ValidatedElement,
        path: &[u32],
        input: &TargetedInput,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<TargetedOutcome, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        input.validate()?;
        if !self.process_instance_alive(&validated.window.instance) {
            return Err(TargetError::ProcessRestarted);
        }
        verify_instance_owns_bundle(
            &validated.window.target.app().bundle_id,
            validated.window.instance.pid as i32,
        )?;
        // Pure family gate: a Chromium-family target never reaches a
        // single AX call or a single posted event. The minimized half of
        // the same cell needs the live window element and runs below,
        // before any event is planned.
        require_background(validated.window.target.app().family, input.kind(), false)?;
        match ax_preflight() {
            AxPreflight::Trusted => {}
            AxPreflight::NotTrusted => {
                return Err(TargetError::PermissionMissing("accessibility"));
            }
            AxPreflight::Unavailable => {
                return Err(TargetError::ProbeUnavailable(
                    "HIServices symbols unavailable".to_string(),
                ));
            }
        }
        // One deadline spans the liveness proof, window resolution, the
        // minimized/key-window reads, the revision re-walk, the path
        // resolution and the focus/selection rechecks.
        let deadline = Instant::now() + AX_TREE_BOUNDS.max_read();
        let proven = self.proven_window_cause(
            &validated.window.instance,
            validated.window.window.window_id,
            Some(deadline),
        )?;
        if proven.generation != validated.window.window.generation {
            return Err(TargetError::WindowReplaced);
        }
        let f = tree_symbols()?;
        let (Some(set_timeout), Some(copy_values)) =
            (f.set_messaging_timeout, f.copy_attribute_values)
        else {
            return Err(TargetError::ProbeUnavailable(
                "HIServices tree symbols unavailable".to_string(),
            ));
        };
        let budget = ReadBudget {
            set_timeout,
            deadline,
        };
        let window_id = u32::try_from(validated.window.window.window_id)
            .map_err(|_| TargetError::Invalid("window id out of range"))?;
        let window_element = resolve_window_element(
            self,
            validated.window.instance.pid as i32,
            window_id,
            deadline,
        )?;
        // Minimized half of the capability gate, read off the live window
        // element: an affirmatively absent attribute means the window
        // cannot be minimized (panels), which is not minimized.
        let minimized =
            dispatch_bool_attr(f, window_element.element, "AXMinimized", &budget)?.unwrap_or(false);
        require_background(
            validated.window.target.app().family,
            input.kind(),
            minimized,
        )?;
        let (nodes, truncation) = walk_ax_tree(&window_element, deadline)?;
        if ax_tree_revision(&nodes, truncation) != validated.tree_revision {
            return Err(TargetError::TreeChanged);
        }
        if !nodes.iter().any(|node| node.path.as_slice() == path) {
            return Err(TargetError::Invalid(
                "element path does not resolve in the tree the handle was validated against",
            ));
        }
        let element = resolve_element_by_path(copy_values, &window_element, path, &budget)?;
        refuse_secure_targeted(f, &element, &budget)?;
        // Keyboard-target recheck: background key events route to the
        // app's key window and land in its focused view, so both must
        // hold for the input to be attributable to the bound element.
        let key_window =
            dispatch_bool_attr(f, window_element.element, "AXMain", &budget)?.unwrap_or(false);
        let focused = dispatch_bool_attr(f, element.0, "AXFocused", &budget)?.unwrap_or(false);
        if !key_window || !focused {
            return Err(TargetError::NotFocused);
        }
        if matches!(input, TargetedInput::InsertText(_))
            && !selection_present(f, &element, &budget)?
        {
            return Err(TargetError::NotFocused);
        }
        // Planning, the event source, and the first group's events still
        // happen after this check. Every group, including the first, checks
        // cancellation again after it is built and before it posts. A
        // pressed sequence still releases.
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let plan = targeted_event_plan(input);
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| {
            TargetError::ProbeUnavailable("CGEventSource create failed".to_string())
        })?;
        // Between-step binding recheck for the posting loop: a live
        // process is not a live binding — the window may close or
        // minimize, or the element lose focus, while later chunks would
        // keep typing into the same pid. Any doubt stops the input.
        let recheck = || -> Option<TargetError> {
            let deadline = Instant::now() + TARGETED_RECHECK_BUDGET;
            match self.proven_window_cause(
                &validated.window.instance,
                validated.window.window.window_id,
                Some(deadline),
            ) {
                Ok(proven) if proven.generation == validated.window.window.generation => {}
                Ok(_) => return Some(TargetError::WindowReplaced),
                Err(cause) => return Some(cause),
            }
            let budget = ReadBudget {
                set_timeout,
                deadline: Instant::now() + TARGETED_RECHECK_BUDGET,
            };
            match dispatch_bool_attr(f, window_element.element, "AXMinimized", &budget) {
                Ok(Some(true)) => {
                    return require_background(
                        validated.window.target.app().family,
                        input.kind(),
                        true,
                    )
                    .err();
                }
                Ok(_) => {}
                Err(cause) => return Some(cause),
            }
            match dispatch_bool_attr(f, element.0, "AXFocused", &budget) {
                Ok(Some(true)) => None,
                Ok(_) => Some(TargetError::NotFocused),
                Err(cause) => Some(cause),
            }
        };
        post_targeted_plan(
            &source,
            validated.window.instance.pid as libc::pid_t,
            &plan,
            &|| self.process_instance_alive(&validated.window.instance),
            cancelled,
            &recheck,
        )
    }

    /// Dispatch one targeted pointer action (click / drag / scroll) on a
    /// consumed observation (CU-08). Gate order: cancellation, ledger
    /// recheck (a revocation since the consume blocks here), payload
    /// validity against the observed window size, process instance,
    /// identity binding — all before the capability gate, which is where
    /// this route ends on every family: CU-01 measured CGEventPostToPid
    /// pointer events as ineffective in the background everywhere (the
    /// caret does not move, buttons do not fire), so
    /// [crate::target::background_capability] holds Pointer Unsupported
    /// across the board and this entry point rejects with
    /// [TargetError::BackgroundUnsupported] — zero events posted, zero AX
    /// calls, no auto-retry, and never a fallback to moving the physical
    /// pointer or activating the target.
    pub fn pointer_action(
        &self,
        authorizer: &mut TargetAuthorizer,
        validated: &ValidatedObservation,
        action: &PointerAction,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        // An observation consume authorized via plain spend and minted no
        // permit; the ledger recheck still blocks a revocation that
        // arrived between the consume and this dispatch.
        authorizer.check(&validated.window.scope, &validated.window.target)?;
        action.validate(
            validated.observation.window_width,
            validated.observation.window_height,
        )?;
        if !self.process_instance_alive(&validated.window.instance) {
            return Err(TargetError::ProcessRestarted);
        }
        verify_instance_owns_bundle(
            &validated.window.target.app().bundle_id,
            validated.window.instance.pid as i32,
        )?;
        // The terminal rejection: no pointer action is proven for any
        // family, minimized or not, so the gate needs no live window
        // read — nothing is ever dispatched past this point.
        require_background(validated.window.target.app().family, action.kind(), false)
    }

    /// Move a bound window to a new global origin through the AX
    /// AXPosition write (CU-08) — the one window move CU-01 measured as
    /// working on an occluded AppKit window (Chromium-family windows
    /// ignore the write). Gate order: cancellation, dispatch-side ledger
    /// authorization (the single-use [crate::approval::DispatchPermit]
    /// from [crate::target::TargetRegistry::consume_window_dispatch]
    /// retires on the first attempt whatever happens later), payload
    /// validity (finite origin, no clamping — negative global
    /// coordinates are a legal multi-display state), process instance,
    /// identity binding, the pure family half of the capability gate
    /// (a Chromium-family target never reaches a single AX call),
    /// Accessibility preflight, window liveness + generation (bounded by
    /// the same deadline as everything below), window resolution, the
    /// minimized half of the capability gate (CU-01 never measured move
    /// on a minimized window — Unverified rejects exactly like
    /// Unsupported), then the armed AXPosition write: the point of no
    /// return, whose returned code alone decides the outcome. Success is
    /// [crate::target::SemanticOutcome::Dispatched] — dispatch proof
    /// only, verify with a fresh observation; a messaging timeout is
    /// [crate::target::SemanticOutcome::UnknownEffect]: the move may
    /// already have happened and is never reported as a clean failure.
    /// The window element needs no tree walk: a move does not change the
    /// tree's window-local frames, so no revision recheck applies.
    pub fn move_window(
        &self,
        authorizer: &mut TargetAuthorizer,
        validated: &ValidatedWindowDispatch,
        origin: &WindowMove,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SemanticOutcome, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        authorizer.consume_dispatch_permit(
            &validated.window.scope,
            &validated.window.target,
            &validated.dispatch_permit,
        )?;
        self.move_window_detached(validated, origin, cancelled)
    }

    /// Window move after the caller already retired the permit.
    /// Do not hold the registry lock across this call.
    pub(crate) fn move_window_detached(
        &self,
        validated: &ValidatedWindowDispatch,
        origin: &WindowMove,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SemanticOutcome, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        origin.validate()?;
        if !self.process_instance_alive(&validated.window.instance) {
            return Err(TargetError::ProcessRestarted);
        }
        verify_instance_owns_bundle(
            &validated.window.target.app().bundle_id,
            validated.window.instance.pid as i32,
        )?;
        require_background(validated.window.target.app().family, origin.kind(), false)?;
        match ax_preflight() {
            AxPreflight::Trusted => {}
            AxPreflight::NotTrusted => {
                return Err(TargetError::PermissionMissing("accessibility"));
            }
            AxPreflight::Unavailable => {
                return Err(TargetError::ProbeUnavailable(
                    "HIServices symbols unavailable".to_string(),
                ));
            }
        }
        let deadline = Instant::now() + AX_TREE_BOUNDS.max_read();
        let proven = self.proven_window_cause(
            &validated.window.instance,
            validated.window.window.window_id,
            Some(deadline),
        )?;
        if proven.generation != validated.window.window.generation {
            return Err(TargetError::WindowReplaced);
        }
        // The move needs no tree walk, so only its own symbols are
        // required: messaging timeout, attribute write and AXValue
        // creation.
        let f = axf().ok_or_else(|| {
            TargetError::ProbeUnavailable("HIServices symbols unavailable".to_string())
        })?;
        let (Some(set_timeout), Some(set_attribute), Some(value_create)) =
            (f.set_messaging_timeout, f.set_attribute, f.value_create)
        else {
            return Err(TargetError::ProbeUnavailable(
                "HIServices action symbols unavailable".to_string(),
            ));
        };
        let window_id = u32::try_from(validated.window.window.window_id)
            .map_err(|_| TargetError::Invalid("window id out of range"))?;
        let window_element = resolve_window_element(
            self,
            validated.window.instance.pid as i32,
            window_id,
            deadline,
        )?;
        let budget = ReadBudget {
            set_timeout,
            deadline,
        };
        // Minimized half of the capability gate, read off the live window
        // element: CU-01 never measured a move on a minimized window, so
        // that cell stays Unverified — and an unknown state (attribute
        // absent or non-boolean) proves nothing either way, so it rejects
        // instead of defaulting the dispatch into the verified matrix.
        let minimized = move_minimized_gate(dispatch_bool_attr(
            f,
            window_element.element,
            "AXMinimized",
            &budget,
        )?)?;
        require_background(
            validated.window.target.app().family,
            origin.kind(),
            minimized,
        )?;
        // The AXPosition write is the point of no return (see the doc
        // comment); cancellation is honored up to here.
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        autoreleasepool(|| unsafe {
            let point = CGPoint {
                x: origin.x,
                y: origin.y,
            };
            let value = value_create(
                K_AX_VALUE_TYPE_CGPOINT,
                &point as *const CGPoint as *const c_void,
            );
            if value.is_null() {
                return Err(TargetError::ProbeUnavailable(
                    "AXValueCreate returned null".to_string(),
                ));
            }
            // Create-rule ownership: the wrapper releases exactly once on
            // every path, including the budget failure below (the write
            // has not run yet, so zero writes and one release).
            let value = RetainedCfValue(value);
            budget
                .arm(window_element.element)
                .map_err(budget_target_error)?;
            #[cfg(test)]
            AX_ACTION_SYSTEM_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let err = set_attribute(
                window_element.element,
                nsstr("AXPosition") as CFStringRef,
                value.0,
            );
            semantic_action_result(err)
        })
    }

    /// One consistent window snapshot: enumeration, generation-ledger sync
    /// and the caller's generation extraction run in a single critical
    /// section. An older enumeration can therefore never overwrite newer
    /// ledger state, and every generation handed out comes from the
    /// snapshot it was validated against. titles_for_pid gates title
    /// copying to one owner's windows only (content of an authorized
    /// target); None copies no titles at all.
    fn with_window_snapshot<T>(
        &self,
        titles_for_pid: Option<i32>,
        f: impl FnOnce(&[RawWindow], &WindowGenerations) -> T,
    ) -> T {
        let mut ledger = self.generations.lock().unwrap();
        let windows = enumerate_windows(titles_for_pid);
        let ids: HashSet<u32> = windows.iter().map(|w| w.id).collect();
        ledger.sync(&ids);
        f(&windows, &ledger)
    }

    /// One consistent liveness proof of a window: present in the window
    /// server, proven by the app's own AXWindows (see the module docs),
    /// with bounds and generation extracted from the same snapshot. Any AX
    /// failure (permission missing, app unresponsive, symbols unavailable)
    /// fails closed: the window cannot be proven alive.
    fn proven_window(&self, instance: &ProcessInstance, window_id: u64) -> Option<ProvenWindow> {
        // CU-03 callers keep the unbounded discipline and flatten every
        // failure to None (fail-closed).
        self.proven_window_cause(instance, window_id, None).ok()
    }

    /// Liveness proof that keeps its failure cause (CU-06 dispatch):
    /// with a deadline, the AXWindows read runs armed with the remaining
    /// budget of the same deadline that bounds the tree re-walk and the
    /// action call, and a blown clock surfaces as the overtime cause
    /// instead of collapsing into WindowReplaced.
    fn proven_window_cause(
        &self,
        instance: &ProcessInstance,
        window_id: u64,
        deadline: Option<Instant>,
    ) -> Result<ProvenWindow, TargetError> {
        let id =
            u32::try_from(window_id).map_err(|_| TargetError::Invalid("window id out of range"))?;
        let pid = instance.pid as i32;
        let ax: Vec<AxWindow> = ax_window_list(pid, deadline)
            .map_err(ax_read_target_error)?
            .iter()
            .map(|window| AxWindow {
                id: window.id,
                frame: window.frame,
            })
            .collect();
        self.with_window_snapshot(None, |windows, ledger| {
            let window = windows.iter().find(|w| w.id == id && w.owner_pid == pid)?;
            if !window_proven_alive(windows, &ax, pid, window) {
                return None;
            }
            Some(ProvenWindow {
                bounds: window.bounds,
                generation: ledger.generation(id)?,
            })
        })
        .ok_or(TargetError::WindowReplaced)
    }
}

impl NativeProbe for MacosNative {
    fn process_instance_alive(&self, instance: &ProcessInstance) -> bool {
        if ns_app_terminated(instance.pid as i32) == Some(true) {
            return false;
        }
        process_start_token(instance.pid as i32) == Some(instance.start_token)
    }

    fn window_generation(&self, instance: &ProcessInstance, window_id: u64) -> Option<u64> {
        self.proven_window(instance, window_id)
            .map(|proven| proven.generation)
    }

    /// CU-05: the revision is the deterministic fingerprint
    /// (ax_tree_revision) of one bounded read of the window's AX subtree,
    /// recomputed on each call with the same fixed bounds. An unchanged
    /// tree fingerprints identically; an emitted change advances it
    /// (64-bit FNV-1a — a cheap change signal, not collision-proof).
    /// Every failure (dead window, ghost id, AX error, missing permission)
    /// is None — fail-closed, so tree-bound handles reject instead of
    /// trusting a tree that could not be re-read.
    fn tree_revision(&self, window: &WindowIdentity) -> Option<u64> {
        let id = u32::try_from(window.window_id).ok()?;
        if !ax_is_process_trusted() {
            return None;
        }
        tree_symbols().ok()?;
        let deadline = Instant::now() + AX_TREE_BOUNDS.max_read();
        let pid = self.with_window_snapshot(None, |windows, _| {
            windows
                .iter()
                .find(|w| w.id == id && w.layer == 0)
                .map(|w| w.owner_pid)
        })?;
        let element = resolve_window_element(self, pid, id, deadline).ok()?;
        let (nodes, truncation) = walk_ax_tree(&element, deadline).ok()?;
        Some(ax_tree_revision(&nodes, truncation))
    }

    /// No page-identity signal source yet (CU-05 / CU-15 territory):
    /// reported as unavailable, so website targets fail closed
    /// (SiteChanged) at binding instead of trusting an unchecked origin.
    fn current_origin(&self, _window: &WindowIdentity) -> Option<String> {
        None
    }

    /// Live window size from the same snapshot discipline as
    /// window_generation: any proof failure fails closed (None).
    fn window_size(&self, instance: &ProcessInstance, window_id: u64) -> Option<(f64, f64)> {
        self.proven_window(instance, window_id)
            .map(|proven| (proven.bounds.width, proven.bounds.height))
    }
}

// ---------- shared Objective-C helpers ----------

fn nsstr(s: &str) -> *mut Object {
    let c = CString::new(s).unwrap();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

fn rs_str(ns: *mut Object) -> String {
    if ns.is_null() {
        return String::new();
    }
    unsafe {
        let p: *const c_char = msg_send![ns, UTF8String];
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}
