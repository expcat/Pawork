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
use core_graphics::window as cgwindow;
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, RgbImage};
use objc::rc::autoreleasepool;
use objc::runtime::{Object, BOOL, YES};
use objc::{class, msg_send, sel, sel_impl};

use crate::approval::{TargetAuthorizer, TargetIdentity};
use crate::target::{
    ax_tree_revision, require_background, AppFamily, AppIdentity, AxTreeNode, AxTreeRead,
    AxTreeTruncation, NativeProbe, ProcessInstance, Scope, SemanticAction, SemanticOutcome,
    TargetError, ValidatedElement, ValidatedWindow, WindowIdentity, AX_TREE_BOUNDS,
};
use crate::{MAX_IMAGE_BYTES, MAX_IMAGE_EDGE};

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
            if let std::collections::hash_map::Entry::Vacant(entry) =
                self.generations.entry(*id)
            {
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
                        title: if include_titles { w.title.clone() } else { None },
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
        #[cfg(test)]
        CAPTURE_SYSTEM_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let image = sck::capture(
            validated.instance.pid as i32,
            u32::try_from(validated.window.window_id)
                .map_err(|_| TargetError::Invalid("window id out of range"))?,
            proven.bounds.width,
            proven.bounds.height,
        )?;
        let rgb = bgra_to_rgb(&image.bgra, image.width, image.height, image.stride).ok_or_else(|| {
            TargetError::ProbeUnavailable("captured image has inconsistent pixels".to_string())
        })?;
        let raw = RgbImage::from_raw(image.width as u32, image.height as u32, rgb).ok_or_else(|| {
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

// ---------- process instances ----------

/// Start token of a live process: kern.proc.pid start time in microseconds
/// since the epoch. In kinfo_proc the leading extern_proc begins with a
/// union whose first alternative is p_starttime, so the start timeval sits
/// at offset 0 of the whole struct; reading only the head avoids depending
/// on the full (kernel-version-specific) layout. None once the pid is gone.
fn process_start_token(pid: i32) -> Option<u64> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    unsafe {
        let mut size: libc::size_t = 0;
        if libc::sysctl(
            mib.as_mut_ptr(),
            4,
            ptr::null_mut(),
            &mut size,
            ptr::null_mut(),
            0,
        ) != 0
            || size < 16
        {
            return None;
        }
        let mut buffer = vec![0u8; size];
        if libc::sysctl(
            mib.as_mut_ptr(),
            4,
            buffer.as_mut_ptr() as *mut c_void,
            &mut size,
            ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        // struct timeval on 64-bit macOS: time_t tv_sec (i64), suseconds_t
        // tv_usec (i32).
        let seconds = i64::from_ne_bytes(buffer[0..8].try_into().ok()?);
        let micros = i32::from_ne_bytes(buffer[8..12].try_into().ok()?) as i64;
        if seconds <= 0 || !(0..1_000_000).contains(&micros) {
            return None;
        }
        Some(seconds as u64 * 1_000_000 + micros as u64)
    }
}

/// The bundle id a pid actually belongs to, read back from the system:
/// the binding check that keeps a grant for one application from
/// unlocking reads of another process. None when the pid is not a
/// running application.
fn running_bundle_id(pid: i32) -> Option<String> {
    autoreleasepool(|| unsafe {
        let app: *mut Object = msg_send![
            class!(NSRunningApplication),
            runningApplicationWithProcessIdentifier: pid
        ];
        if app.is_null() {
            return None;
        }
        let bundle: *mut Object = msg_send![app, bundleIdentifier];
        if bundle.is_null() {
            return None;
        }
        Some(rs_str(bundle))
    })
}

/// The granted identity must own this process: the bundle id is read back
/// from the system so a grant for one application never unlocks another
/// process's windows. Listing and capture share this binding.
fn verify_instance_owns_bundle(bundle_id: &str, pid: i32) -> Result<(), TargetError> {
    if running_bundle_id(pid).as_deref() != Some(bundle_id) {
        return Err(TargetError::Invalid(
            "process instance does not belong to the authorized identity",
        ));
    }
    Ok(())
}

/// NSRunningApplication termination state, when the pid is an application:
/// Some(true) catches the zombie window between process exit and reaping.
fn ns_app_terminated(pid: i32) -> Option<bool> {
    autoreleasepool(|| unsafe {
        let app: *mut Object = msg_send![
            class!(NSRunningApplication),
            runningApplicationWithProcessIdentifier: pid
        ];
        if app.is_null() {
            return None;
        }
        // The property is named terminated; its getter is isTerminated.
        let terminated: BOOL = msg_send![app, isTerminated];
        Some(terminated == YES)
    })
}

// ---------- controlled launch ----------

/// Test-only proof that denied launches never reach LaunchServices.
#[cfg(test)]
static LAUNCH_SYSTEM_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Test-only proof that denied captures never reach ScreenCaptureKit.
#[cfg(test)]
static CAPTURE_SYSTEM_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

fn launch(identity: &AppIdentity) -> Result<ProcessInstance, LaunchError> {
    #[cfg(test)]
    LAUNCH_SYSTEM_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    autoreleasepool(|| unsafe {
        let ws: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let url: *mut Object =
            msg_send![ws, URLForApplicationWithBundleIdentifier: nsstr(&identity.bundle_id)];
        if url.is_null() {
            return Err(LaunchError::NotFound(identity.bundle_id.clone()));
        }
        let mut error: *mut Object = ptr::null_mut();
        let app: *mut Object = msg_send![
            ws,
            launchApplicationAtURL: url
            options: LAUNCH_WITHOUT_ACTIVATION
            configuration: ptr::null_mut::<Object>()
            error: &mut error
        ];
        if app.is_null() {
            let description: *mut Object = msg_send![error, localizedDescription];
            let reason = rs_str(description);
            return Err(LaunchError::Failed(if reason.is_empty() {
                "LaunchServices returned no application".to_string()
            } else {
                reason
            }));
        }
        let pid: i32 = msg_send![app, processIdentifier];
        if pid <= 0 {
            return Err(LaunchError::Failed(
                "LaunchServices returned no process".to_string(),
            ));
        }
        process_start_token(pid)
            .map(|start_token| ProcessInstance {
                pid: pid as u32,
                start_token,
            })
            .ok_or(LaunchError::ExitedEarly)
    })
}

// ---------- application family classification ----------

/// Capability family from bundle facts. Declaring the http/https URL scheme
/// marks a browser (covers WebKit/Gecko browsers too); a bundled Chromium
/// framework marks Electron/CEF apps; everything else is AppKit. Heuristic
/// feeding only the CU-01 background capability matrix — never a security
/// decision.
fn classify(chromium_embedded: bool, declares_web_scheme: bool) -> AppFamily {
    if declares_web_scheme {
        AppFamily::Browser
    } else if chromium_embedded {
        AppFamily::Chromium
    } else {
        AppFamily::Appkit
    }
}

fn classify_bundle(bundle_url: *mut Object) -> AppFamily {
    autoreleasepool(|| unsafe {
        if bundle_url.is_null() {
            return AppFamily::Appkit;
        }
        let bundle: *mut Object = msg_send![class!(NSBundle), bundleWithURL: bundle_url];
        if bundle.is_null() {
            return AppFamily::Appkit;
        }
        let types: *mut Object =
            msg_send![bundle, objectForInfoDictionaryKey: nsstr("CFBundleURLTypes")];
        let declares_web = declares_web_scheme(types);
        let path = rs_str(msg_send![bundle, bundlePath]);
        let chromium_embedded = [
            "Electron Framework.framework",
            "Chromium Embedded Framework.framework",
        ]
        .iter()
        .any(|framework| {
            Path::new(&path)
                .join("Contents/Frameworks")
                .join(framework)
                .exists()
        });
        classify(chromium_embedded, declares_web)
    })
}

/// CFBundleURLTypes → any entry whose CFBundleURLSchemes contains http or
/// https (case-insensitive).
fn declares_web_scheme(types: *mut Object) -> bool {
    unsafe {
        if types.is_null() {
            return false;
        }
        let count: usize = msg_send![types, count];
        for i in 0..count {
            let entry: *mut Object = msg_send![types, objectAtIndex: i];
            if entry.is_null() {
                continue;
            }
            let schemes: *mut Object = msg_send![entry, objectForKey: nsstr("CFBundleURLSchemes")];
            if schemes.is_null() {
                continue;
            }
            let scheme_count: usize = msg_send![schemes, count];
            for j in 0..scheme_count {
                let scheme: *mut Object = msg_send![schemes, objectAtIndex: j];
                let value = rs_str(scheme).to_ascii_lowercase();
                if value == "http" || value == "https" {
                    return true;
                }
            }
        }
        false
    }
}

// ---------- window enumeration ----------

/// titles_for_pid gates title copying: kCGWindowName (window content) is
/// copied out of a window's dictionary only when the window belongs to
/// that owner — callers pass the authorized target's pid, and no other
/// application's title is ever read into memory. None copies no titles.
fn enumerate_windows(titles_for_pid: Option<i32>) -> Vec<RawWindow> {
    // The kCGWindow* keys are immutable immutable framework constants;
    // reading them needs an unsafe block.
    unsafe fn keys() -> (
        CFStringRef,
        CFStringRef,
        CFStringRef,
        CFStringRef,
        CFStringRef,
    ) {
        (
            cgwindow::kCGWindowNumber,
            cgwindow::kCGWindowOwnerPID,
            cgwindow::kCGWindowLayer,
            cgwindow::kCGWindowName,
            cgwindow::kCGWindowBounds,
        )
    }
    let (k_number, k_owner, k_layer, k_name, k_bounds) = unsafe { keys() };
    let Some(array) = cgwindow::copy_window_info(
        cgwindow::kCGWindowListOptionAll | cgwindow::kCGWindowListExcludeDesktopElements,
        cgwindow::kCGNullWindowID,
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for pointer in array.get_all_values() {
        let info: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(pointer as CFDictionaryRef) };
        let Some(id) = number(&info, k_number) else {
            continue;
        };
        let Some(owner_pid) = number(&info, k_owner) else {
            continue;
        };
        let owner_pid = owner_pid as i32;
        out.push(RawWindow {
            id: id as u32,
            owner_pid,
            layer: number(&info, k_layer).unwrap_or(0.0) as i32,
            bounds: window_bounds(&info, k_bounds).unwrap_or(WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            }),
            title: if titles_for_pid == Some(owner_pid) {
                string(&info, k_name)
            } else {
                None
            },
        });
    }
    out
}

fn dict_value(
    dict: &CFDictionary<CFString, CFType>,
    key: CFStringRef,
) -> Option<ItemRef<'_, CFType>> {
    let key = unsafe { CFString::wrap_under_get_rule(key) };
    dict.find(&key)
}

fn number(dict: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<f64> {
    let value = dict_value(dict, key)?;
    if !value.instance_of::<CFNumber>() {
        return None;
    }
    let number =
        unsafe { CFNumber::wrap_under_get_rule(value.as_concrete_TypeRef() as CFNumberRef) };
    number.to_f64()
}

fn string(dict: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<String> {
    let value = dict_value(dict, key)?;
    if !value.instance_of::<CFString>() {
        return None;
    }
    let string =
        unsafe { CFString::wrap_under_get_rule(value.as_concrete_TypeRef() as CFStringRef) };
    Some(string.to_string())
}

fn window_bounds(dict: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<WindowBounds> {
    let value = dict_value(dict, key)?;
    let bounds: CFDictionary<CFString, CFType> = unsafe {
        CFDictionary::wrap_under_get_rule(value.as_concrete_TypeRef() as CFDictionaryRef)
    };
    let component = |name: &str| -> Option<f64> {
        let value = bounds.find(&CFString::new(name))?;
        if !value.instance_of::<CFNumber>() {
            return None;
        }
        let number =
            unsafe { CFNumber::wrap_under_get_rule(value.as_concrete_TypeRef() as CFNumberRef) };
        number.to_f64()
    };
    Some(WindowBounds {
        x: component("X")?,
        y: component("Y")?,
        width: component("Width")?,
        height: component("Height")?,
    })
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

// ---------- accessibility cross-validation ----------

/// AX entry points resolved at runtime: HIServices is a subframework outside
/// the default linker search path — the same approach the CU-01 probe
/// validated. None of these ever prompts.
struct AxFns {
    is_trusted: unsafe extern "C" fn() -> bool,
    create_application: unsafe extern "C" fn(i32) -> *mut c_void,
    copy_attribute: unsafe extern "C" fn(*mut c_void, CFStringRef, *mut *const c_void) -> i32,
    value_get_value: unsafe extern "C" fn(*const c_void, i32, *mut c_void) -> bool,
    /// _AXUIElementGetWindow: private export mapping an AX window to its
    /// window-server id (CGWindowID). Undocumented but stable for well
    /// over a decade (yabai, Hammerspoon and others rely on it); when it
    /// is missing the backend falls back to frame-group closure.
    get_window: Option<unsafe extern "C" fn(*const c_void, *mut u32) -> i32>,
    /// CU-05 tree walk: ranged children fetch, child count, action names
    /// and a per-call messaging timeout.
    copy_attribute_values:
        Option<unsafe extern "C" fn(*mut c_void, CFStringRef, i64, i64, *mut *const c_void) -> i32>,
    attribute_value_count: Option<unsafe extern "C" fn(*mut c_void, CFStringRef, *mut i64) -> i32>,
    copy_action_names: Option<unsafe extern "C" fn(*mut c_void, *mut *const c_void) -> i32>,
    set_messaging_timeout: Option<unsafe extern "C" fn(*mut c_void, f32) -> i32>,
    /// Chromium-family targets serialize their real tree only once a
    /// client sets AXManualAccessibility on the application element.
    set_attribute: Option<unsafe extern "C" fn(*mut c_void, CFStringRef, *const c_void) -> i32>,
    /// CU-06 semantic dispatch. Optional: reads work without them; the
    /// semantic action refuses when the one it needs is missing.
    perform_action: Option<unsafe extern "C" fn(*mut c_void, CFStringRef) -> i32>,
}

fn axf() -> Option<&'static AxFns> {
    static AXF: OnceLock<Option<AxFns>> = OnceLock::new();
    AXF.get_or_init(|| unsafe {
        let path =
            b"/System/Library/Frameworks/ApplicationServices.framework/Frameworks/HIServices.framework/HIServices\0";
        let handle = libc::dlopen(path.as_ptr() as *const c_char, libc::RTLD_LAZY);
        if handle.is_null() {
            return None;
        }
        unsafe fn load<T>(handle: *mut c_void, name: &str) -> Option<T> {
            let c = CString::new(name).unwrap();
            let symbol = libc::dlsym(handle, c.as_ptr());
            if symbol.is_null() {
                None
            } else {
                Some(std::mem::transmute_copy(&symbol))
            }
        }
        Some(AxFns {
            is_trusted: load(handle, "AXIsProcessTrusted")?,
            create_application: load(handle, "AXUIElementCreateApplication")?,
            copy_attribute: load(handle, "AXUIElementCopyAttributeValue")?,
            value_get_value: load(handle, "AXValueGetValue")?,
            // Optional: absence degrades identity to frame-group closure.
            get_window: load(handle, "_AXUIElementGetWindow"),
            // CU-05 tree walk. Optional: liveness (CU-03) works without
            // them; the tree read refuses when they are missing.
            copy_attribute_values: load(handle, "AXUIElementCopyAttributeValues"),
            attribute_value_count: load(handle, "AXUIElementGetAttributeValueCount"),
            copy_action_names: load(handle, "AXUIElementCopyActionNames"),
            set_messaging_timeout: load(handle, "AXUIElementSetMessagingTimeout"),
            set_attribute: load(handle, "AXUIElementSetAttributeValue"),
            perform_action: load(handle, "AXUIElementPerformAction"),
        })
    })
    .as_ref()
}

fn ax_is_process_trusted() -> bool {
    matches!(ax_preflight(), AxPreflight::Trusted)
}

/// Accessibility preflight. A symbol-load failure is NOT a permission
/// report: it propagates as a probe failure instead of PermissionMissing.
enum AxPreflight {
    Trusted,
    NotTrusted,
    Unavailable,
}

fn ax_preflight() -> AxPreflight {
    match axf() {
        Some(f) => {
            if unsafe { (f.is_trusted)() } {
                AxPreflight::Trusted
            } else {
                AxPreflight::NotTrusted
            }
        }
        None => AxPreflight::Unavailable,
    }
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFRelease(pointer: *const c_void);
    fn CFRetain(pointer: *const c_void) -> *const c_void;
    fn CFDataGetLength(data: *const c_void) -> isize;
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFBooleanGetValue(boolean: *const c_void) -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGImageGetWidth(image: *const c_void) -> usize;
    fn CGImageGetHeight(image: *const c_void) -> usize;
    fn CGImageGetBytesPerRow(image: *const c_void) -> usize;
    fn CGImageGetDataProvider(image: *const c_void) -> *mut c_void;
    fn CGDataProviderCopyData(provider: *mut c_void) -> *mut c_void;
}

const K_AX_VALUE_TYPE_CGPOINT: i32 = 1;
const K_AX_VALUE_TYPE_CGSIZE: i32 = 2;

/// One window from the app's own AXWindows: its window-server id when the
/// _AXUIElementGetWindow export resolved it (None when the export is
/// missing or failed for this window), and its global frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AxWindow {
    id: Option<u32>,
    frame: [f64; 4],
}

/// Why reading the app's AXWindows failed. Distinct from the permission
/// preflight: these are probe-call failures, and access is still refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AxReadError {
    /// HIServices symbols could not be resolved on this system.
    SymbolsUnavailable,
    /// AXUIElementCreateApplication returned nothing for the pid.
    CreateFailed,
    /// AXUIElementCopyAttributeValue(AXWindows) failed (raw AXError).
    ReadFailed(i32),
    /// The read outlived its wall-clock budget (CU-05 tree reads only).
    Overtime,
}

/// kAXErrorAPIDisabled (AXError.h): the process lacks the Accessibility
/// permission. CannotComplete (-25204) and every other code stay probe
/// failures.
const K_AX_ERROR_API_DISABLED: i32 = -25211;

/// Map an AX read failure to the honest contract error: the API-disabled
/// code is a permission report; everything else keeps its cause as a
/// probe failure. Both refuse access.
fn ax_read_target_error(error: AxReadError) -> TargetError {
    match error {
        AxReadError::SymbolsUnavailable => {
            TargetError::ProbeUnavailable("HIServices symbols unavailable".to_string())
        }
        AxReadError::CreateFailed => {
            TargetError::ProbeUnavailable("AXUIElementCreateApplication failed".to_string())
        }
        AxReadError::Overtime => TargetError::ProbeUnavailable(
            "accessibility tree read exceeded its time budget".to_string(),
        ),
        AxReadError::ReadFailed(code) if code == K_AX_ERROR_API_DISABLED => {
            TargetError::PermissionMissing("accessibility")
        }
        AxReadError::ReadFailed(code) => {
            TargetError::ProbeUnavailable(format!("AXWindows read failed: AXError {code}"))
        }
    }
}

/// One AX window with its element retained past the AXWindows array that
/// delivered it (CU-05 needs the element for the tree walk; CU-03 liveness
/// keeps only id + frame). Drop releases the retain.
struct AxWindowRef {
    element: *mut c_void,
    id: Option<u32>,
    frame: [f64; 4],
}

impl Drop for AxWindowRef {
    fn drop(&mut self) {
        unsafe { CFRelease(self.element as *const c_void) }
    }
}

/// Remaining slice of a read's wall-clock budget as messaging-timeout
/// seconds for the next AX call; None once the budget is blown, so the
/// caller fails instead of starting another unbounded call.
fn remaining_budget(deadline: Instant) -> Option<f32> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    Some(remaining.as_secs_f32().max(f32::EPSILON))
}

/// Per-call wall-clock budget of one bounded AX read. Messaging timeouts
/// are per element AND stale the moment they are set: every AX call is
/// armed with the budget remaining when THAT call starts (arm), and the
/// clock is re-checked when it returns (check), so the deadline bounds
/// the whole read — never just one phase — and no call ever starts
/// unbounded (a blown clock or a failed timeout-set refuses instead).
struct ReadBudget {
    set_timeout: unsafe extern "C" fn(*mut c_void, f32) -> i32,
    deadline: Instant,
}

/// Why the budget refused a read. Mapped per context: tree reads to
/// TargetError (budget_target_error), window-list reads to AxReadError
/// (budget_read_error).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BudgetError {
    /// The wall-clock budget is blown.
    Overtime,
    /// AXUIElementSetMessagingTimeout failed (raw AXError).
    SetFailed(i32),
}

impl ReadBudget {
    /// Arm one element's messaging timeout with the budget remaining
    /// right now. element must be a live AXUIElementRef.
    fn arm(&self, element: *mut c_void) -> Result<(), BudgetError> {
        let Some(remaining) = remaining_budget(self.deadline) else {
            return Err(BudgetError::Overtime);
        };
        // Safety: callers hand over live AXUIElementRefs; the timeout
        // bounds exactly the calls issued on this element below.
        let err = unsafe { (self.set_timeout)(element, remaining) };
        if err != 0 {
            return Err(BudgetError::SetFailed(err));
        }
        Ok(())
    }

    /// Fail once the wall-clock budget is blown.
    fn check(&self) -> Result<(), BudgetError> {
        if remaining_budget(self.deadline).is_none() {
            return Err(BudgetError::Overtime);
        }
        Ok(())
    }

    /// One armed AX call: arm with the current remaining budget, run,
    /// re-check the clock. Only for calls whose result owns no retained
    /// CoreFoundation pointer — a failed post-call check consumes the
    /// result, so a retained pointer would leak (copy-rule reads run the
    /// arm/call/check sequence by hand and release before propagating).
    fn call<T>(&self, element: *mut c_void, f: impl FnOnce() -> T) -> Result<T, BudgetError> {
        self.arm(element)?;
        let out = f();
        self.check()?;
        Ok(out)
    }
}

/// A budget refusal inside a window-list read.
fn budget_read_error(err: BudgetError) -> AxReadError {
    match err {
        BudgetError::Overtime => AxReadError::Overtime,
        BudgetError::SetFailed(code) => AxReadError::ReadFailed(code),
    }
}

/// The app's own AXWindows with retained elements — the authoritative
/// answer to which windows actually exist, and the entry point of the
/// CU-05 tree walk.
/// deadline (CU-05 tree reads) bounds this read too: every AX call — the
/// AXWindows fetch and each window's id/position/size reads — is armed
/// with the messaging timeout of the budget remaining when it starts and
/// the clock is re-checked when it returns (ReadBudget). A blown clock,
/// a failed timeout-set or a missing timeout symbol fails the read — an
/// unbounded AX call is never issued. CU-03 liveness passes None and
/// keeps its unbounded (short, two-call) discipline.
fn ax_window_list(pid: i32, deadline: Option<Instant>) -> Result<Vec<AxWindowRef>, AxReadError> {
    let f = axf().ok_or(AxReadError::SymbolsUnavailable)?;
    autoreleasepool(|| unsafe {
        let app = (f.create_application)(pid);
        if app.is_null() {
            return Err(AxReadError::CreateFailed);
        }
        // A deadline-bounded read cannot run without the timeout symbol:
        // no call below could be time-bounded.
        let budget = match (deadline, f.set_messaging_timeout) {
            (Some(deadline), Some(set_timeout)) => Some(ReadBudget {
                set_timeout,
                deadline,
            }),
            (Some(_), None) => {
                CFRelease(app as *const c_void);
                return Err(AxReadError::SymbolsUnavailable);
            }
            (None, _) => None,
        };
        if let Some(budget) = &budget {
            if let Err(err) = budget.arm(app) {
                CFRelease(app as *const c_void);
                return Err(budget_read_error(err));
            }
        }
        let mut windows: *const c_void = ptr::null();
        let err = (f.copy_attribute)(app, nsstr("AXWindows") as CFStringRef, &mut windows);
        if err != 0 || windows.is_null() {
            CFRelease(app as *const c_void);
            return Err(AxReadError::ReadFailed(err));
        }
        if let Some(budget) = &budget {
            if let Err(err) = budget.check() {
                CFRelease(windows);
                CFRelease(app as *const c_void);
                return Err(budget_read_error(err));
            }
        }
        let count = CFArrayGetCount(windows);
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let window = CFArrayGetValueAtIndex(windows, i) as *mut c_void;
            if window.is_null() {
                continue;
            }
            // Messaging timeouts are per element and go stale: the id
            // read is armed with the budget remaining when it starts
            // (budget.call — the result owns no retained pointer), and
            // the frame's two attribute reads are armed one by one inside
            // ax_window_frame.
            let id = if let Some(get_window) = f.get_window {
                let mut id: u32 = 0;
                let read = match &budget {
                    Some(budget) => {
                        budget.call(window, || get_window(window as *const c_void, &mut id))
                    }
                    None => Ok(get_window(window as *const c_void, &mut id)),
                };
                match read {
                    Ok(err) => (err == 0).then_some(id),
                    Err(err) => {
                        CFRelease(windows);
                        CFRelease(app as *const c_void);
                        return Err(budget_read_error(err));
                    }
                }
            } else {
                None
            };
            let frame = match ax_window_frame(f, window, budget.as_ref()) {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(err) => {
                    CFRelease(windows);
                    CFRelease(app as *const c_void);
                    return Err(err);
                }
            };
            CFRetain(window as *const c_void);
            out.push(AxWindowRef {
                element: window,
                id,
                frame,
            });
        }
        CFRelease(windows);
        CFRelease(app as *const c_void);
        Ok(out)
    })
}

/// The app's own AXWindows as plain id + frame views (CU-03 liveness).
fn ax_windows(pid: i32) -> Result<Vec<AxWindow>, AxReadError> {
    Ok(ax_window_list(pid, None)?
        .into_iter()
        .map(|window| AxWindow {
            id: window.id,
            frame: window.frame,
        })
        .collect())
}

/// One AX window's frame (x, y, w, h in global points); None when the
/// position or size attributes cannot be read. With a read budget
/// (CU-05), each attribute read is armed and the clock re-checked after
/// it — a budget refusal is Err while an unreadable attribute stays
/// Ok(None) (CU-03 semantics). The retained position is released on
/// every path, including a budget refusal between the two reads.
fn ax_window_frame(
    f: &AxFns,
    window: *mut c_void,
    budget: Option<&ReadBudget>,
) -> Result<Option<[f64; 4]>, AxReadError> {
    unsafe {
        if let Some(budget) = budget {
            budget.arm(window).map_err(budget_read_error)?;
        }
        let position = ax_attr_raw(f, window, "AXPosition");
        if let Some(budget) = budget {
            if let Err(err) = budget.check() {
                if let Ok(Some(raw)) = position {
                    CFRelease(raw);
                }
                return Err(budget_read_error(err));
            }
        }
        let position = match position {
            Ok(position) => position,
            Err(_) => return Ok(None),
        };
        if let Some(budget) = budget {
            if let Err(err) = budget.arm(window) {
                if let Some(raw) = position {
                    CFRelease(raw);
                }
                return Err(budget_read_error(err));
            }
        }
        let size = ax_attr_raw(f, window, "AXSize");
        if let Some(budget) = budget {
            if let Err(err) = budget.check() {
                if let Some(raw) = position {
                    CFRelease(raw);
                }
                if let Ok(Some(raw)) = size {
                    CFRelease(raw);
                }
                return Err(budget_read_error(err));
            }
        }
        let size = match size {
            Ok(size) => size,
            Err(_) => {
                if let Some(raw) = position {
                    CFRelease(raw);
                }
                return Ok(None);
            }
        };
        let (Some(position), Some(size)) = (position, size) else {
            for raw in [position, size].into_iter().flatten() {
                CFRelease(raw);
            }
            return Ok(None);
        };
        let point = ax_value_tuple(f, position, K_AX_VALUE_TYPE_CGPOINT);
        let extent = ax_value_tuple(f, size, K_AX_VALUE_TYPE_CGSIZE);
        CFRelease(position);
        CFRelease(size);
        let (Some(point), Some(extent)) = (point, extent) else {
            return Ok(None);
        };
        Ok(Some([point.0, point.1, extent.0, extent.1]))
    }
}

fn bounds_frame(bounds: &WindowBounds) -> [f64; 4] {
    [bounds.x, bounds.y, bounds.width, bounds.height]
}

fn frame_eq(a: &[f64; 4], b: &[f64; 4]) -> bool {
    const EPSILON: f64 = 0.5;
    (0..4).all(|i| (a[i] - b[i]).abs() <= EPSILON)
}

/// A window-server window and an AX window sit at the same place when
/// their global frames agree within half a point (both are reported in
/// points, top-left origin; verified exact for TextEdit in CU-03).
fn frame_matches(bounds: &WindowBounds, frame: &[f64; 4]) -> bool {
    frame_eq(&bounds_frame(bounds), frame)
}

/// Whether one consistent snapshot proves window w (owned by pid) alive.
/// Exact mode — every AX window carries its window-server id: the AX set
/// must contain w.id; ghosts are never mapped, so a closed window dies
/// immediately even when a same-frame window survives, and the survivor
/// keeps validating. Fallback mode (the _AXUIElementGetWindow export is
/// unavailable or failed): the frame group must close — the app's
/// window-server layer-0 windows sharing the frame must number exactly
/// its AX windows with that frame. Ghosts inflate only the window-server
/// side, so equality proves every entry in the group live; any excess
/// either way is ambiguity and fails closed. Because epsilon matching is
/// not transitive, one AX window can also land inside two different CG
/// frame groups (e.g. a ghost at x=100, a live window at x=100.8, one AX
/// window at x=100.4 matches both): such overlap is equally ambiguous and
/// fails closed, so a single AX candidate can never prove two CG windows.
fn window_proven_alive(windows: &[RawWindow], ax: &[AxWindow], pid: i32, w: &RawWindow) -> bool {
    if ax.iter().all(|a| a.id.is_some()) {
        return ax.iter().any(|a| a.id == Some(w.id));
    }
    let frame = bounds_frame(&w.bounds);
    let cg_same = windows
        .iter()
        .filter(|x| x.owner_pid == pid && x.layer == 0 && frame_matches(&x.bounds, &frame))
        .count();
    let ax_same = ax.iter().filter(|a| frame_eq(&a.frame, &frame)).count();
    if cg_same == 0 || cg_same != ax_same {
        return false;
    }
    // No AX candidate of this group may match a CG window outside it.
    !ax.iter()
        .filter(|a| frame_eq(&a.frame, &frame))
        .any(|a| {
            windows.iter().any(|x| {
                x.owner_pid == pid
                    && x.layer == 0
                    && frame_eq(&bounds_frame(&x.bounds), &a.frame)
                    && !frame_matches(&x.bounds, &frame)
            })
        })
}

// ---------- bounded accessibility tree read (CU-05) ----------

/// Test-only proof that denied tree reads never reach the AX walk seam.
#[cfg(test)]
static AX_WALK_SYSTEM_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// kAXErrorNoValue / kAXErrorAttributeUnsupported (AXError.h): the element
/// affirmatively has no such attribute — an answer, not a failure.
const K_AX_ERROR_NO_VALUE: i32 = -25212;
const K_AX_ERROR_ATTRIBUTE_UNSUPPORTED: i32 = -25205;

fn ax_attr_absent(err: i32) -> bool {
    err == K_AX_ERROR_NO_VALUE || err == K_AX_ERROR_ATTRIBUTE_UNSUPPORTED
}

/// An AX failure during the tree walk keeps its cause: the API-disabled
/// code is a permission report, everything else a probe failure. Both
/// refuse access.
fn ax_tree_read_error(err: i32) -> TargetError {
    if err == K_AX_ERROR_API_DISABLED {
        TargetError::PermissionMissing("accessibility")
    } else {
        TargetError::ProbeUnavailable(format!("accessibility tree read failed: AXError {err}"))
    }
}

/// A budget refusal inside a tree read: overtime is a probe failure, a
/// failed timeout-set keeps its raw AXError cause.
fn budget_target_error(err: BudgetError) -> TargetError {
    match err {
        BudgetError::Overtime => TargetError::ProbeUnavailable(
            "accessibility tree read exceeded its time budget".to_string(),
        ),
        BudgetError::SetFailed(code) => ax_tree_read_error(code),
    }
}

/// Copy-rule attribute read: Ok(Some) carries a retained pointer the
/// caller releases; Ok(None) is an affirmative absent; Err keeps the raw
/// AXError.
unsafe fn ax_attr_raw(
    f: &AxFns,
    element: *mut c_void,
    attribute: &str,
) -> Result<Option<*const c_void>, i32> {
    let mut raw: *const c_void = ptr::null();
    let err = (f.copy_attribute)(element, nsstr(attribute) as CFStringRef, &mut raw);
    if err == 0 && !raw.is_null() {
        return Ok(Some(raw));
    }
    if !raw.is_null() {
        CFRelease(raw);
    }
    if err == 0 || ax_attr_absent(err) {
        return Ok(None);
    }
    Err(err)
}

/// CGPoint and CGSize share one layout (two f64); extract either from a
/// retained AXValue pointer.
unsafe fn ax_value_tuple(f: &AxFns, raw: *const c_void, value_type: i32) -> Option<(f64, f64)> {
    let mut pair = [0.0f64; 2];
    let ok = (f.value_get_value)(raw, value_type, pair.as_mut_ptr() as *mut c_void);
    ok.then_some((pair[0], pair[1]))
}

/// Cut on a char boundary; the flag records every cut.
fn truncate_chars(text: &str, max_text: usize) -> (String, bool) {
    if text.chars().count() <= max_text {
        return (text.to_string(), false);
    }
    (text.chars().take(max_text).collect(), true)
}

/// A string attribute cut to max_text chars (flag set when cut).
/// Non-string values are absent — never a read of the wrong type.
unsafe fn ax_attr_string(
    f: &AxFns,
    element: *mut c_void,
    attribute: &str,
    max_text: usize,
) -> Result<Option<(String, bool)>, i32> {
    let Some(raw) = ax_attr_raw(f, element, attribute)? else {
        return Ok(None);
    };
    let value = CFType::wrap_under_get_rule(raw);
    let text = if value.instance_of::<CFString>() {
        let string = CFString::wrap_under_get_rule(raw as CFStringRef).to_string();
        Some(truncate_chars(&string, max_text))
    } else {
        None
    };
    CFRelease(raw);
    Ok(text)
}

/// A boolean attribute; None is "not exposed", never a claim of false.
unsafe fn ax_attr_bool(
    f: &AxFns,
    element: *mut c_void,
    attribute: &str,
) -> Result<Option<bool>, i32> {
    let Some(raw) = ax_attr_raw(f, element, attribute)? else {
        return Ok(None);
    };
    let value = CFType::wrap_under_get_rule(raw);
    let state = if value.instance_of::<CFBoolean>() {
        Some(CFBooleanGetValue(raw) != 0)
    } else {
        None
    };
    CFRelease(raw);
    Ok(state)
}

/// Secure-input classification. The SDK defines AXSecureTextField as a
/// SUBROLE (kAXSecureTextFieldSubrole, AXRoleConstants.h): a standard
/// NSSecureTextField reads as role AXTextField with subrole
/// AXSecureTextField, so matching the role alone never identifies it.
/// The subrole is read for AXTextField elements (the only role the SDK
/// assigns this subrole to); the legacy role match is kept for
/// nonstandard exposers. A failed subrole read (subrole_read_failed) is
/// conservative: treated as secure, so no name is ever read from an
/// element whose secure state could not be determined. Secure elements
/// are emitted with role, state, frame and actions only — not even a
/// name, and never a value.
fn ax_secure(role: &str, subrole: Option<&str>, subrole_read_failed: bool) -> bool {
    if role == "AXSecureTextField" {
        return true;
    }
    if role != "AXTextField" {
        return false;
    }
    if subrole_read_failed {
        return true;
    }
    subrole == Some("AXSecureTextField")
}

/// Ask the target to serialize its full accessibility tree: Chromium
/// family apps (Electron, Chrome, Edge) answer reads with only the
/// window chrome until a client sets AXManualAccessibility on the
/// application element — the same read-enable VoiceOver performs, no UI
/// change, idempotent. AppKit targets already serialize and simply
/// ignore the attribute. Best-effort and never a read failure: an
/// unsupported attribute leaves the target as found, and the walk reads
/// whatever the target actually exposes (a window without exposed
/// elements stays an honest root-only tree, not an error).
/// The app element's own calls join the read budget: the messaging
/// timeout is set to the remaining budget first (a set failure refuses
/// the read — an unbounded AX call is never issued) and the clock is
/// re-checked afterwards.
fn ax_enable_full_tree(pid: i32, deadline: Instant) -> Result<(), TargetError> {
    let f = axf()
        .ok_or_else(|| TargetError::ProbeUnavailable("HIServices symbols unavailable".to_string()))?;
    let (Some(set_attribute), Some(set_timeout)) = (f.set_attribute, f.set_messaging_timeout)
    else {
        // No way to ask for the full tree: Chromium reads stay chrome-only
        // but honest. set_messaging_timeout's absence is refused earlier
        // by tree_symbols, so this branch is the set_attribute case.
        return Ok(());
    };
    autoreleasepool(|| unsafe {
        let app = (f.create_application)(pid);
        if app.is_null() {
            return Ok(());
        }
        let Some(remaining) = remaining_budget(deadline) else {
            CFRelease(app as *const c_void);
            return Err(TargetError::ProbeUnavailable(
                "accessibility tree read exceeded its time budget".to_string(),
            ));
        };
        let timeout_err = (set_timeout)(app, remaining);
        if timeout_err != 0 {
            CFRelease(app as *const c_void);
            return Err(ax_tree_read_error(timeout_err));
        }
        let _ = (set_attribute)(
            app,
            nsstr("AXManualAccessibility") as CFStringRef,
            CFBoolean::from(true).as_CFTypeRef() as *const CFType as *const c_void,
        );
        CFRelease(app as *const c_void);
        if remaining_budget(deadline).is_none() {
            return Err(TargetError::ProbeUnavailable(
                "accessibility tree read exceeded its time budget".to_string(),
            ));
        }
        Ok(())
    })
}

/// Resolve a window to its AX window element for the tree walk. Exact id
/// correspondence when _AXUIElementGetWindow covers every AX window (a
/// ghost id is never mapped, so a closed window is WindowReplaced here);
/// otherwise the frame-group closure discipline of window_proven_alive
/// decides, and only a single-candidate closed group yields an element —
/// with several same-frame windows the element's identity is unknowable
/// and fails closed.
fn resolve_window_element(
    native: &MacosNative,
    pid: i32,
    window_id: u32,
    deadline: Instant,
) -> Result<AxWindowRef, TargetError> {
    ax_enable_full_tree(pid, deadline)?;
    let mut list = ax_window_list(pid, Some(deadline)).map_err(ax_read_target_error)?;
    if list.iter().all(|window| window.id.is_some()) {
        let index = list
            .iter()
            .position(|window| window.id == Some(window_id))
            .ok_or(TargetError::WindowReplaced)?;
        return Ok(list.swap_remove(index));
    }
    native.with_window_snapshot(None, |windows, _| {
        let ax: Vec<AxWindow> = list
            .iter()
            .map(|window| AxWindow {
                id: window.id,
                frame: window.frame,
            })
            .collect();
        let raw = windows
            .iter()
            .find(|w| w.id == window_id && w.owner_pid == pid && w.layer == 0)
            .ok_or(TargetError::WindowReplaced)?;
        if !window_proven_alive(windows, &ax, pid, raw) {
            return Err(TargetError::WindowReplaced);
        }
        let frame = bounds_frame(&raw.bounds);
        let matching: Vec<usize> = list
            .iter()
            .enumerate()
            .filter(|(_, window)| frame_eq(&window.frame, &frame))
            .map(|(index, _)| index)
            .collect();
        if matching.len() != 1 {
            return Err(TargetError::ProbeUnavailable(
                "window identity is ambiguous without _AXUIElementGetWindow".to_string(),
            ));
        }
        Ok(list.swap_remove(matching[0]))
    })
}

/// One bounded preorder walk of a window's AX subtree. Emission stops at
/// the depth and node bounds (flagged); the time budget fails the read
/// (see AX_TREE_BOUNDS). Optional attributes that fail degrade to None;
/// the role, the child list and the action list fail the read, so an
/// emitted tree is never a partial truth.
struct TreeWalk<'a> {
    f: &'a AxFns,
    copy_values: unsafe extern "C" fn(*mut c_void, CFStringRef, i64, i64, *mut *const c_void) -> i32,
    value_count: unsafe extern "C" fn(*mut c_void, CFStringRef, *mut i64) -> i32,
    action_names: unsafe extern "C" fn(*mut c_void, *mut *const c_void) -> i32,
    budget: ReadBudget,
    origin: (f64, f64),
    nodes: Vec<AxTreeNode>,
    truncation: AxTreeTruncation,
}

impl TreeWalk<'_> {
    fn visit(
        &mut self,
        element: *mut c_void,
        depth: u32,
        path: &mut Vec<u32>,
    ) -> Result<(), TargetError> {
        if self.nodes.len() >= AX_TREE_BOUNDS.max_nodes as usize {
            self.truncation.nodes = true;
            return Ok(());
        }
        // Every AX call is armed with the messaging timeout of the budget
        // remaining when THAT call starts, and the clock is re-checked
        // when it returns (ReadBudget::call; copy-rule reads whose result
        // owns a retained pointer run the same arm/call/check sequence by
        // hand, releasing before an error propagates). A failed
        // timeout-set refuses the read — an unbounded AX call is never
        // issued.
        // The role is structural: without it the element cannot be
        // classified, so its absence or failure fails the read.
        let role = self
            .budget
            .call(element, || unsafe { ax_attr_string(self.f, element, "AXRole", 64) })
            .map_err(budget_target_error)?
            .map_err(ax_tree_read_error)?
            .map(|(role, _)| role)
            .ok_or_else(|| TargetError::ProbeUnavailable("element without a role".to_string()))?;
        // Secure-input identification (see ax_secure): the SDK defines
        // AXSecureTextField as a subrole, so AXTextField elements get an
        // AXSubrole read; a failed identification stays conservative and
        // no name is read.
        let secure = if role == "AXTextField" {
            match self
                .budget
                .call(element, || unsafe { ax_attr_string(self.f, element, "AXSubrole", 64) })
            {
                Err(budget) => return Err(budget_target_error(budget)),
                Ok(Ok(subrole)) => {
                    ax_secure(&role, subrole.as_ref().map(|(text, _)| text.as_str()), false)
                }
                Ok(Err(_)) => ax_secure(&role, None, true),
            }
        } else {
            ax_secure(&role, None, false)
        };
        // Names are readable labels (AXTitle, else AXDescription); element
        // content (AXValue) is never read, and secure-input roles not even
        // a name. A name-read FAILURE (not absence: real apps answer
        // kAXErrorFailure for AXDescription on text areas) marks the node
        // name_unreadable and the read's names flag — a name-based lookup
        // over the read is then explicitly unprovable instead of treating
        // the unread name as a non-match.
        let (name, name_truncated, name_failed) = if secure {
            (None, false, false)
        } else {
            match self.budget.call(element, || unsafe {
                ax_attr_string(self.f, element, "AXTitle", AX_TREE_BOUNDS.max_text)
            }) {
                Err(budget) => return Err(budget_target_error(budget)),
                Ok(Ok(Some(named))) => (Some(named.0), named.1, false),
                Ok(Ok(None)) => match self.budget.call(element, || unsafe {
                    ax_attr_string(self.f, element, "AXDescription", AX_TREE_BOUNDS.max_text)
                }) {
                    Err(budget) => return Err(budget_target_error(budget)),
                    Ok(Ok(Some(named))) => (Some(named.0), named.1, false),
                    Ok(Ok(None)) => (None, false, false),
                    Ok(Err(_)) => (None, false, true),
                },
                Ok(Err(_)) => (None, false, true),
            }
        };
        if name_truncated {
            self.truncation.text = true;
        }
        if name_failed {
            self.truncation.names = true;
        }
        // Optional state degrades to None on an AX failure (never a claim
        // of false or a fabricated frame); a budget refusal still fails
        // the read.
        let enabled = self
            .budget
            .call(element, || unsafe { ax_attr_bool(self.f, element, "AXEnabled") })
            .map_err(budget_target_error)?
            .unwrap_or(None);
        let focused = self
            .budget
            .call(element, || unsafe { ax_attr_bool(self.f, element, "AXFocused") })
            .map_err(budget_target_error)?
            .unwrap_or(None);
        let frame = self.read_frame(element)?;
        let actions = unsafe { self.actions(element) }?;
        let children = unsafe { self.children_count(element) }?;
        let depth_limited = children > 0 && depth >= AX_TREE_BOUNDS.max_depth;
        if depth_limited {
            self.truncation.depth = true;
        }
        self.nodes.push(AxTreeNode {
            path: path.clone(),
            depth,
            role,
            name,
            name_truncated,
            name_unreadable: name_failed,
            enabled,
            focused,
            frame,
            actions,
            children,
            depth_limited,
        });
        if children == 0 || depth_limited {
            return Ok(());
        }
        let remaining = AX_TREE_BOUNDS.max_nodes as usize - self.nodes.len();
        let take = (children as usize).min(remaining);
        if take < children as usize {
            self.truncation.nodes = true;
        }
        if take == 0 {
            return Ok(());
        }
        self.visit_children(element, depth, path, take)
    }

    /// The element's frame in window-local points (the walk's origin is
    /// the window's own AX position): two independently armed attribute
    /// reads. A budget refusal fails the read; an AX failure, a missing
    /// attribute or a non-AXValue payload degrades to None — never a
    /// fabricated frame. The retained position is released on every
    /// path, including a budget refusal between the two reads.
    fn read_frame(&self, element: *mut c_void) -> Result<Option<[f64; 4]>, TargetError> {
        unsafe {
            self.budget.arm(element).map_err(budget_target_error)?;
            let position = ax_attr_raw(self.f, element, "AXPosition");
            if let Err(err) = self.budget.check() {
                if let Ok(Some(raw)) = position {
                    CFRelease(raw);
                }
                return Err(budget_target_error(err));
            }
            let position = match position {
                Ok(position) => position,
                Err(_) => return Ok(None),
            };
            if let Err(err) = self.budget.arm(element) {
                if let Some(raw) = position {
                    CFRelease(raw);
                }
                return Err(budget_target_error(err));
            }
            let size = ax_attr_raw(self.f, element, "AXSize");
            if let Err(err) = self.budget.check() {
                if let Some(raw) = position {
                    CFRelease(raw);
                }
                if let Ok(Some(raw)) = size {
                    CFRelease(raw);
                }
                return Err(budget_target_error(err));
            }
            let size = match size {
                Ok(size) => size,
                Err(_) => {
                    if let Some(raw) = position {
                        CFRelease(raw);
                    }
                    return Ok(None);
                }
            };
            let (Some(position), Some(size)) = (position, size) else {
                for raw in [position, size].into_iter().flatten() {
                    CFRelease(raw);
                }
                return Ok(None);
            };
            let point = ax_value_tuple(self.f, position, K_AX_VALUE_TYPE_CGPOINT);
            let extent = ax_value_tuple(self.f, size, K_AX_VALUE_TYPE_CGSIZE);
            CFRelease(position);
            CFRelease(size);
            let (Some(point), Some(extent)) = (point, extent) else {
                return Ok(None);
            };
            Ok(Some([
                point.0 - self.origin.0,
                point.1 - self.origin.1,
                extent.0,
                extent.1,
            ]))
        }
    }

    /// The affirmative child count: 0 when the attribute is absent (a
    /// leaf), never when the read fails.
    unsafe fn children_count(&self, element: *mut c_void) -> Result<u32, TargetError> {
        let mut count: i64 = 0;
        let err = self
            .budget
            .call(element, || {
                (self.value_count)(element, nsstr("AXChildren") as CFStringRef, &mut count)
            })
            .map_err(budget_target_error)?;
        if err == 0 {
            return Ok(count.clamp(0, i64::from(u32::MAX)) as u32);
        }
        if ax_attr_absent(err) {
            return Ok(0);
        }
        Err(ax_tree_read_error(err))
    }

    /// The element's action names. An unreadable or pathologically long
    /// list fails the read — an emitted `actions` is always the true list.
    unsafe fn actions(&self, element: *mut c_void) -> Result<Vec<String>, TargetError> {
        self.budget.arm(element).map_err(budget_target_error)?;
        let mut raw: *const c_void = ptr::null();
        let err = (self.action_names)(element, &mut raw);
        // The clock is re-checked after the call; the retained array is
        // released on every path before either error propagates.
        let clock = self.budget.check();
        if err != 0 || raw.is_null() {
            if !raw.is_null() {
                CFRelease(raw);
            }
            if let Err(err) = clock {
                return Err(budget_target_error(err));
            }
            if err == 0 || ax_attr_absent(err) {
                return Ok(Vec::new());
            }
            return Err(ax_tree_read_error(err));
        }
        let count = CFArrayGetCount(raw);
        if count > 64 {
            CFRelease(raw);
            clock.map_err(budget_target_error)?;
            return Err(TargetError::ProbeUnavailable(
                "element reports an implausible action list".to_string(),
            ));
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let item = CFArrayGetValueAtIndex(raw, i);
            if item.is_null() {
                continue;
            }
            let value = CFType::wrap_under_get_rule(item);
            if value.instance_of::<CFString>() {
                let name = CFString::wrap_under_get_rule(item as CFStringRef).to_string();
                out.push(truncate_chars(&name, 64).0);
            }
        }
        CFRelease(raw);
        clock.map_err(budget_target_error)?;
        Ok(out)
    }

    /// Visit up to `take` children (a bounded slice fetch — the array the
    /// app reports is never copied whole). The element already reported
    /// `take` reachable children, so the fetch must agree: an absent,
    /// failed, short or null-holed list is a structural inconsistency and
    /// fails the read — silently treating it as a complete branch would
    /// let a lookup crown a unique match the read never proved. The
    /// fetched array is released on every path, including a child visit
    /// that failed and a clock check that fired after the fetch — the
    /// clock error propagates only once the retained array is released.
    fn visit_children(
        &mut self,
        element: *mut c_void,
        depth: u32,
        path: &mut Vec<u32>,
        take: usize,
    ) -> Result<(), TargetError> {
        self.budget.arm(element).map_err(budget_target_error)?;
        let mut raw: *const c_void = ptr::null();
        let err = unsafe {
            (self.copy_values)(
                element,
                nsstr("AXChildren") as CFStringRef,
                0,
                take as i64,
                &mut raw,
            )
        };
        let clock = self.budget.check();
        if err != 0 || raw.is_null() {
            if !raw.is_null() {
                unsafe { CFRelease(raw) };
            }
            if let Err(err) = clock {
                return Err(budget_target_error(err));
            }
            return Err(TargetError::ProbeUnavailable(
                "child list inconsistent with the reported child count".to_string(),
            ));
        }
        let fetched = unsafe { CFArrayGetCount(raw) };
        if fetched < take as isize {
            unsafe { CFRelease(raw) };
            if let Err(err) = clock {
                return Err(budget_target_error(err));
            }
            return Err(TargetError::ProbeUnavailable(
                "child list inconsistent with the reported child count".to_string(),
            ));
        }
        let mut outcome = Ok(());
        for i in 0..fetched {
            let child = unsafe { CFArrayGetValueAtIndex(raw, i) as *mut c_void };
            if child.is_null() {
                outcome = Err(TargetError::ProbeUnavailable(
                    "child list inconsistent with the reported child count".to_string(),
                ));
                break;
            }
            path.push(i as u32);
            outcome = self.visit(child, depth + 1, path);
            path.pop();
            if outcome.is_err() {
                break;
            }
        }
        unsafe { CFRelease(raw) };
        outcome?;
        clock.map_err(budget_target_error)?;
        Ok(())
    }
}

/// The symbols a tree read cannot run without, including the messaging
/// timeout the per-call budget relies on: when they are missing the read
/// refuses instead of walking unbounded.
fn tree_symbols() -> Result<&'static AxFns, TargetError> {
    let f = axf()
        .ok_or_else(|| TargetError::ProbeUnavailable("HIServices symbols unavailable".to_string()))?;
    if f.copy_attribute_values.is_none()
        || f.attribute_value_count.is_none()
        || f.copy_action_names.is_none()
        || f.set_messaging_timeout.is_none()
    {
        return Err(TargetError::ProbeUnavailable(
            "HIServices tree symbols unavailable".to_string(),
        ));
    }
    Ok(f)
}

/// One bounded read of the window element's subtree; the root is always
/// emitted as nodes[0]. The deadline is the caller's: it spans window
/// resolution, the Chromium read-enable and the walk, so the whole read
/// — not just the walk — is bounded by AX_TREE_BOUNDS.max_read.
fn walk_ax_tree(
    root: &AxWindowRef,
    deadline: Instant,
) -> Result<(Vec<AxTreeNode>, AxTreeTruncation), TargetError> {
    let f = tree_symbols()?;
    let (Some(copy_values), Some(value_count), Some(action_names), Some(set_timeout)) = (
        f.copy_attribute_values,
        f.attribute_value_count,
        f.copy_action_names,
        f.set_messaging_timeout,
    ) else {
        return Err(TargetError::ProbeUnavailable(
            "HIServices tree symbols unavailable".to_string(),
        ));
    };
    autoreleasepool(|| {
        let mut walk = TreeWalk {
            f,
            copy_values,
            value_count,
            action_names,
            budget: ReadBudget {
                set_timeout,
                deadline,
            },
            origin: (root.frame[0], root.frame[1]),
            nodes: Vec::new(),
            truncation: AxTreeTruncation::default(),
        };
        let mut path = Vec::new();
        walk.visit(root.element, 0, &mut path)?;
        // Final clock check before delivery: a walk whose last AX call
        // returned inside the budget but whose assembly crossed the
        // deadline still fails rather than delivering late data.
        walk.budget.check().map_err(budget_target_error)?;
        Ok((walk.nodes, walk.truncation))
    })
}

// ---------- semantic element actions (CU-06) ----------

/// Test-only proof that denied semantic actions never reach the AX action
/// seam.
#[cfg(test)]
static AX_ACTION_SYSTEM_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// kAXErrorCannotComplete (AXError.h): the call did not answer within its
/// messaging timeout. For an action dispatch the effect is unknown — the
/// element may have performed it — so it is never reported as a clean
/// failure.
const K_AX_ERROR_CANNOT_COMPLETE: i32 = -25204;
/// kAXErrorInvalidUIElement: the element is gone — the live tree moved
/// past the revision the dispatch was validated against.
const K_AX_ERROR_INVALID_UI_ELEMENT: i32 = -25202;
/// kAXErrorActionUnsupported: the element affirmatively refuses the
/// action.
const K_AX_ERROR_ACTION_UNSUPPORTED: i32 = -25206;

/// The AX action call's return code mapped to the honest terminal state:
/// success is Dispatched (not proof of effect); CannotComplete is
/// UnknownEffect (a timed-out action may already have happened — never
/// blindly retried); the element's own refusal codes are
/// ElementUnsupported (a clear rejection — the call ran and the element
/// declined, nothing was dispatched); a dead element means the tree moved
/// past the validated revision; API-disabled is the permission report;
/// everything else keeps its cause as a probe failure.
fn semantic_action_result(err: i32) -> Result<SemanticOutcome, TargetError> {
    match err {
        0 => Ok(SemanticOutcome::Dispatched),
        K_AX_ERROR_CANNOT_COMPLETE => Ok(SemanticOutcome::UnknownEffect),
        K_AX_ERROR_ACTION_UNSUPPORTED | K_AX_ERROR_ATTRIBUTE_UNSUPPORTED => {
            Err(TargetError::ElementUnsupported)
        }
        K_AX_ERROR_INVALID_UI_ELEMENT => Err(TargetError::TreeChanged),
        K_AX_ERROR_API_DISABLED => Err(TargetError::PermissionMissing("accessibility")),
        code => Err(TargetError::ProbeUnavailable(format!(
            "semantic action failed: AXError {code}"
        ))),
    }
}

/// An AX read failure while resolving or classifying the dispatch element
/// keeps its cause; a dead element is a tree change here.
fn ax_dispatch_read_error(err: i32) -> TargetError {
    if err == K_AX_ERROR_INVALID_UI_ELEMENT {
        TargetError::TreeChanged
    } else {
        ax_tree_read_error(err)
    }
}

/// A retained AX element released on drop: copy-rule discipline for the
/// dispatch-side path resolution.
struct RetainedElement(*mut c_void);

impl Drop for RetainedElement {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0 as *const c_void) }
    }
}

/// Re-resolve a validated element's child-index path against the live
/// tree, starting at the window element (an empty path is the window
/// itself). Path shape is validated against the fixed tree bounds first —
/// a forged path is host contract misuse (Invalid), never an unbounded
/// fetch. Every step is an armed bounded slice fetch of the child list
/// (never a whole-array copy); a list that no longer covers the recorded
/// index means the live tree moved past the revision the handle was
/// validated against (TreeChanged), while a list contradicting itself
/// (null hole) stays a structural inconsistency (ProbeUnavailable). The
/// returned element carries its own retain.
fn resolve_element_by_path(
    copy_values: unsafe extern "C" fn(
        *mut c_void,
        CFStringRef,
        i64,
        i64,
        *mut *const c_void,
    ) -> i32,
    root: &AxWindowRef,
    path: &[u32],
    budget: &ReadBudget,
) -> Result<RetainedElement, TargetError> {
    if path.len() > AX_TREE_BOUNDS.max_depth as usize {
        return Err(TargetError::Invalid(
            "element path deeper than the tree depth bound",
        ));
    }
    if path.iter().any(|&index| index >= AX_TREE_BOUNDS.max_nodes) {
        return Err(TargetError::Invalid("element path index out of bounds"));
    }
    unsafe { CFRetain(root.element as *const c_void) };
    let mut current = root.element;
    for &index in path {
        match resolve_path_child(copy_values, current, index, budget) {
            Ok(child) => {
                unsafe { CFRelease(current as *const c_void) };
                current = child;
            }
            Err(err) => {
                unsafe { CFRelease(current as *const c_void) };
                return Err(err);
            }
        }
    }
    Ok(RetainedElement(current))
}

/// One descent step: the child at "index" of "element", retained for the
/// caller. The slice fetch runs armed with the remaining budget and the
/// clock is re-checked when it returns — before the child is retained, so
/// a blown clock never leaks it. "element" stays the caller's own retain.
fn resolve_path_child(
    copy_values: unsafe extern "C" fn(
        *mut c_void,
        CFStringRef,
        i64,
        i64,
        *mut *const c_void,
    ) -> i32,
    element: *mut c_void,
    index: u32,
    budget: &ReadBudget,
) -> Result<*mut c_void, TargetError> {
    unsafe {
        budget.arm(element).map_err(budget_target_error)?;
        let mut raw: *const c_void = ptr::null();
        let err = copy_values(
            element,
            nsstr("AXChildren") as CFStringRef,
            0,
            i64::from(index) + 1,
            &mut raw,
        );
        // The clock is re-checked after the call; the fetched array is
        // released on every path before either error propagates.
        let clock = budget.check();
        if err != 0 || raw.is_null() {
            if !raw.is_null() {
                CFRelease(raw);
            }
            if let Err(err) = clock {
                return Err(budget_target_error(err));
            }
            // The revision-identical walk just saw this child list; an
            // absent or failed fetch now means the live tree moved.
            if err == 0 || ax_attr_absent(err) {
                return Err(TargetError::TreeChanged);
            }
            return Err(ax_dispatch_read_error(err));
        }
        let fetched = CFArrayGetCount(raw);
        if fetched <= index as isize {
            CFRelease(raw);
            clock.map_err(budget_target_error)?;
            return Err(TargetError::TreeChanged);
        }
        let child = CFArrayGetValueAtIndex(raw, index as isize) as *mut c_void;
        if child.is_null() {
            CFRelease(raw);
            clock.map_err(budget_target_error)?;
            return Err(TargetError::ProbeUnavailable(
                "child list inconsistent with the reported child count".to_string(),
            ));
        }
        // The clock verdict precedes the retain: a blown budget discards
        // the step instead of leaking the child — and the fetched array
        // is released before the error propagates.
        if let Err(err) = clock {
            CFRelease(raw);
            return Err(budget_target_error(err));
        }
        CFRetain(child as *const c_void);
        CFRelease(raw);
        Ok(child)
    }
}

/// Value writes to secure-input elements are refused: the read side never
/// reads secure content (CU-05), and the write side never silently enters
/// it — entering credentials is the user's keyboard, not a background AX
/// write. The role (and subrole, for AXTextField) is re-read on the
/// resolved element with the same conservative classification as the walk;
/// a failed read classifies as secure. Presses are unaffected.
fn refuse_secure_write(
    f: &AxFns,
    element: &RetainedElement,
    budget: &ReadBudget,
) -> Result<(), TargetError> {
    let role = budget
        .call(element.0, || unsafe {
            ax_attr_string(f, element.0, "AXRole", 64)
        })
        .map_err(budget_target_error)?
        .map_err(ax_dispatch_read_error)?;
    let Some((role, _)) = role else {
        // The revision-identical walk just emitted a role for this node;
        // an affirmatively absent role now means the element moved.
        return Err(TargetError::TreeChanged);
    };
    let secure = if role == "AXTextField" {
        match budget.call(element.0, || unsafe {
            ax_attr_string(f, element.0, "AXSubrole", 64)
        }) {
            Err(budget) => return Err(budget_target_error(budget)),
            Ok(Ok(subrole)) => ax_secure(
                &role,
                subrole.as_ref().map(|(text, _)| text.as_str()),
                false,
            ),
            Ok(Err(err)) if err == K_AX_ERROR_INVALID_UI_ELEMENT => {
                return Err(TargetError::TreeChanged)
            }
            // A failed subrole identification is conservative: secure.
            Ok(Err(_)) => ax_secure(&role, None, true),
        }
    } else {
        ax_secure(&role, None, false)
    };
    if secure {
        return Err(TargetError::Invalid(
            "semantic value writes to secure-input elements are refused",
        ));
    }
    Ok(())
}

// ---------- ScreenCaptureKit window capture (CU-04) ----------

/// Raw-pixel ceilings for one capture, mirroring the isolated desktop's
/// framebuffer limits: at most 4096 on one edge and 8M pixels overall.
const MAX_CAPTURE_EDGE: f64 = 4096.0;
const MAX_CAPTURE_PIXELS: f64 = 8_000_000.0;

/// Capture pixel scale for a window of the given points: up to 2x (Retina
/// text stays legible after the downscale to the image budget), bounded by
/// the raw-pixel ceilings. None for degenerate bounds.
fn capture_scale(width_pt: f64, height_pt: f64) -> Option<f64> {
    if !width_pt.is_finite() || !height_pt.is_finite() || width_pt <= 0.0 || height_pt <= 0.0 {
        return None;
    }
    let by_edge = MAX_CAPTURE_EDGE / width_pt.max(height_pt);
    let by_pixels = (MAX_CAPTURE_PIXELS / (width_pt * height_pt)).sqrt();
    Some(2.0f64.min(by_edge).min(by_pixels))
}

/// Final integer output size for one capture. The scaled size is floored
/// per axis — independent rounding-up can push the pixel product past the
/// budget (2000x2001pt rounded to 2828x2829 = 8,000,412 > 8,000,000) —
/// and both ceilings are re-checked on the integer result. None when the
/// window has degenerate bounds.
fn capture_pixel_size(width_pt: f64, height_pt: f64) -> Option<(usize, usize)> {
    let scale = capture_scale(width_pt, height_pt)?;
    let width = (width_pt * scale).floor().clamp(1.0, MAX_CAPTURE_EDGE) as usize;
    let height = (height_pt * scale).floor().clamp(1.0, MAX_CAPTURE_EDGE) as usize;
    if width.checked_mul(height)? > MAX_CAPTURE_PIXELS as usize {
        return None;
    }
    Some((width, height))
}

/// BGRA frame → tightly packed RGB for the JPEG encoder; stride-aware.
/// None when the buffer cannot contain the announced layout.
fn bgra_to_rgb(bgra: &[u8], width: usize, height: usize, stride: usize) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || stride < width * 4 {
        return None;
    }
    let needed = stride.checked_mul(height - 1)?.checked_add(width * 4)?;
    if bgra.len() < needed {
        return None;
    }
    let mut rgb = Vec::with_capacity(width * height * 3);
    for row in 0..height {
        let base = row * stride;
        for column in 0..width {
            let pixel = base + column * 4;
            rgb.extend_from_slice(&[bgra[pixel + 2], bgra[pixel + 1], bgra[pixel]]);
        }
    }
    Some(rgb)
}

/// Downscale to the image budget and encode as baseline JPEG, stepping the
/// quality down until the byte budget holds (same ladder as the isolated
/// desktop). An image that cannot fit is a capture failure, not a smaller
/// truth: no caller ever receives an out-of-budget frame.
fn jpeg_within_budget(raw: &RgbImage) -> Result<(Vec<u8>, u32, u32), TargetError> {
    let scale = (MAX_IMAGE_EDGE as f64 / raw.width().max(raw.height()) as f64).min(1.0);
    let scaled = image::imageops::resize(
        raw,
        (raw.width() as f64 * scale).round().max(1.0) as u32,
        (raw.height() as f64 * scale).round().max(1.0) as u32,
        FilterType::Triangle,
    );
    for quality in [75, 50, 30] {
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, quality)
            .encode_image(&scaled)
            .map_err(|_| TargetError::ProbeUnavailable("JPEG encoding failed".to_string()))?;
        if jpeg.len() <= MAX_IMAGE_BYTES {
            return Ok((jpeg, scaled.width(), scaled.height()));
        }
    }
    Err(TargetError::ProbeUnavailable(
        "screenshot exceeds JPEG byte budget".to_string(),
    ))
}

/// ScreenCaptureKit single-frame window capture, the route CU-01 measured
/// for occluded, background and minimized windows of every application
/// family (no main-thread requirement). The framework is dlopen-loaded
/// like HIServices above and driven through Objective-C message sends;
/// completion handlers are awaited with a bounded timeout.
mod sck {
    use super::*;

    /// One captured frame: BGRA pixels with the image-reported row stride
    /// (may exceed width * 4).
    pub struct SckImage {
        pub width: usize,
        pub height: usize,
        pub stride: usize,
        pub bgra: Vec<u8>,
    }

    /// Why a capture failed. WindowGone maps to WindowReplaced (the window
    /// left the shareable set between the liveness proof and the capture);
    /// everything else keeps its OS cause for diagnosis.
    #[derive(Debug)]
    pub enum CaptureFailure {
        WindowGone,
        Unavailable(String),
    }

    impl From<CaptureFailure> for TargetError {
        fn from(failure: CaptureFailure) -> Self {
            match failure {
                CaptureFailure::WindowGone => TargetError::WindowReplaced,
                CaptureFailure::Unavailable(reason) => TargetError::ProbeUnavailable(reason),
            }
        }
    }

    /// Bound for one completion handler (shareable content or the capture
    /// itself); a single frame is normally sub-second.
    const COMPLETION_TIMEOUT: Duration = Duration::from_secs(10);

    /// Load the framework and warm the WindowServer connection once.
    /// SCScreenshotManager asserts on an uninitialized CGS connection, so a
    /// CoreGraphics call runs first (CU-01 probe finding).
    fn ensure_sck() -> Result<(), CaptureFailure> {
        static READY: OnceLock<Result<(), String>> = OnceLock::new();
        READY
            .get_or_init(|| unsafe {
                CGMainDisplayID();
                let path =
                    b"/System/Library/Frameworks/ScreenCaptureKit.framework/ScreenCaptureKit\x00";
                let handle = libc::dlopen(path.as_ptr() as *const c_char, libc::RTLD_LAZY);
                if handle.is_null() {
                    return Err("dlopen ScreenCaptureKit failed".to_string());
                }
                for name in [
                    "SCShareableContent",
                    "SCContentFilter",
                    "SCStreamConfiguration",
                    "SCScreenshotManager",
                ] {
                    if objc::runtime::Class::get(name).is_none() {
                        return Err(format!(
                            "ScreenCaptureKit class {name} unavailable (needs macOS 14+)"
                        ));
                    }
                }
                Ok(())
            })
            .clone()
            .map_err(CaptureFailure::Unavailable)
    }

    /// Shared state between a completion handler and the waiting caller,
    /// with explicit ownership of the retained result pointer. The
    /// handler retains a successful object and stringifies any error
    /// before its autoreleasepool drains. Exactly one side releases the
    /// retained pointer: the waiter after a successful wait, the handler
    /// itself when the wait was already abandoned (timeout), or the
    /// waiter when the result landed between the timeout and the
    /// abandonment lock. A late frame therefore can never leak, and a
    /// timed-out wait never leaves filter/config or images behind.
    pub struct Completion {
        state: Mutex<CompletionState>,
        ready: Condvar,
        dispose: unsafe fn(usize),
    }

    #[derive(Default)]
    struct CompletionState {
        result: usize,
        error: Option<String>,
        done: bool,
        abandoned: bool,
    }

    pub fn completion(dispose: unsafe fn(usize)) -> Arc<Completion> {
        Arc::new(Completion {
            state: Mutex::new(CompletionState::default()),
            ready: Condvar::new(),
            dispose,
        })
    }

    /// Handler side, invoked exactly once per block call: stores the
    /// (already retained) result for the waiter, or releases it
    /// immediately when the waiter has given up. A result that arrives
    /// together with an error is released here instead of being handed
    /// out, so an error delivery never carries an owned pointer and the
    /// waiter's error path never has anything to release.
    pub fn deliver(pair: &Arc<Completion>, result: usize, error: Option<String>) {
        let release_now = {
            let mut state = pair.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.abandoned {
                result != 0
            } else if error.is_some() {
                state.result = 0;
                state.error = error;
                state.done = true;
                result != 0
            } else {
                state.result = result;
                state.done = true;
                false
            }
        };
        pair.ready.notify_one();
        if release_now {
            unsafe { (pair.dispose)(result) }
        }
    }

    /// Waiter side: on timeout the wait is abandoned; a result that
    /// landed between the deadline and the abandonment lock is released
    /// here before the error is returned.
    pub fn wait_completion(
        pair: &Arc<Completion>,
        timeout: Duration,
    ) -> Result<(usize, Option<String>), CaptureFailure> {
        let guard = pair.state.lock().unwrap_or_else(|e| e.into_inner());
        let (mut guard, elapsed) = pair
            .ready
            .wait_timeout_while(guard, timeout, |state| !state.done)
            .map_err(|_| CaptureFailure::Unavailable("completion lock poisoned".to_string()))?;
        if elapsed.timed_out() {
            guard.abandoned = true;
            let late = std::mem::take(&mut guard.result);
            drop(guard);
            if late != 0 {
                unsafe { (pair.dispose)(late) }
            }
            return Err(CaptureFailure::Unavailable(format!(
                "completion handler timed out after {}s",
                timeout.as_secs()
            )));
        }
        Ok((guard.result, guard.error.clone()))
    }

    /// Release hooks for the two retain conventions a handler can hold.
    unsafe fn release_objc_object(pointer: usize) {
        let _: () = msg_send![pointer as *mut Object, release];
    }

    unsafe fn release_cf_object(pointer: usize) {
        CFRelease(pointer as *const c_void);
    }

    fn error_description(error: *mut Object) -> String {
        if error.is_null() {
            return "unknown nil-context error".to_string();
        }
        unsafe { rs_str(msg_send![error, localizedDescription]) }
    }

    /// The current shareable content: every window of every app, including
    /// off-screen, occluded and minimized ones (onScreenWindowsOnly: false).
    fn shareable_content() -> Result<*mut Object, CaptureFailure> {
        ensure_sck()?;
        unsafe {
            let class = objc::runtime::Class::get("SCShareableContent").unwrap();
            let pair = completion(release_objc_object);
            let handler = pair.clone();
            let block = ConcreteBlock::new(move |content: *mut Object, error: *mut Object| {
                if !content.is_null() {
                    let _: *mut Object = msg_send![content, retain];
                }
                let description = if error.is_null() {
                    None
                } else {
                    Some(error_description(error))
                };
                deliver(&handler, content as usize, description);
            });
            let block = block.copy();
            let modern: BOOL = msg_send![
                class,
                respondsToSelector: sel!(getShareableContentExcludingDesktopWindows:onScreenWindowsOnly:completionHandler:)
            ];
            if modern == YES {
                let _: () = msg_send![class,
                    getShareableContentExcludingDesktopWindows: false
                    onScreenWindowsOnly: false
                    completionHandler: &*block];
            } else {
                let _: () = msg_send![class, getShareableContentWithCompletionHandler: &*block];
            }
            let (content, error) = wait_completion(&pair, COMPLETION_TIMEOUT)?;
            // deliver guarantees an error never carries an owned pointer.
            if let Some(description) = error {
                return Err(CaptureFailure::Unavailable(format!(
                    "SCShareableContent: {description}"
                )));
            }
            if content == 0 {
                return Err(CaptureFailure::Unavailable(
                    "SCShareableContent returned nil".to_string(),
                ));
            }
            Ok(content as *mut Object)
        }
    }

    /// The one SCWindow naming (window_id, pid): identity comes from both
    /// the window-server id and the owning process, so a stale id another
    /// process reused can never be captured for the authorized target.
    fn find_window(content: *mut Object, pid: i32, window_id: u32) -> Option<*mut Object> {
        unsafe {
            let windows: *mut Object = msg_send![content, windows];
            if windows.is_null() {
                return None;
            }
            let count: usize = msg_send![windows, count];
            for i in 0..count {
                let window: *mut Object = msg_send![windows, objectAtIndex: i];
                if window.is_null() {
                    continue;
                }
                let id: u32 = msg_send![window, windowID];
                let app: *mut Object = msg_send![window, owningApplication];
                let owner: i32 = if app.is_null() {
                    0
                } else {
                    msg_send![app, processID]
                };
                if id == window_id && owner == pid {
                    let _: *mut Object = msg_send![window, retain];
                    return Some(window);
                }
            }
            None
        }
    }

    /// Layout ceilings re-checked against the actually delivered frame
    /// before any pixel is copied: the capture is only trusted within the
    /// raw-pixel budget, whatever size was configured.
    pub fn checked_frame_layout(
        width: usize,
        height: usize,
        stride: usize,
    ) -> Result<(), CaptureFailure> {
        if width == 0 || height == 0 || stride < width.saturating_mul(4) {
            return Err(CaptureFailure::Unavailable(format!(
                "captured image has degenerate layout ({width}x{height} stride {stride})"
            )));
        }
        if width > MAX_CAPTURE_EDGE as usize
            || height > MAX_CAPTURE_EDGE as usize
            || width.saturating_mul(height) > MAX_CAPTURE_PIXELS as usize
        {
            return Err(CaptureFailure::Unavailable(format!(
                "captured image exceeds the pixel budget ({width}x{height})"
            )));
        }
        Ok(())
    }

    /// Copy the frame's BGRA bytes out of the CGImage. A frame with no
    /// pixels or a truncated provider is a capture failure, never a
    /// partial image.
    fn extract_pixels(image: *const c_void) -> Result<SckImage, CaptureFailure> {
        unsafe {
            let width = CGImageGetWidth(image);
            let height = CGImageGetHeight(image);
            let stride = CGImageGetBytesPerRow(image);
            checked_frame_layout(width, height, stride)?;
            let provider = CGImageGetDataProvider(image);
            if provider.is_null() {
                return Err(CaptureFailure::Unavailable(
                    "captured image has no data provider".to_string(),
                ));
            }
            let data = CGDataProviderCopyData(provider);
            if data.is_null() {
                return Err(CaptureFailure::Unavailable(
                    "captured image bytes unavailable".to_string(),
                ));
            }
            let length = CFDataGetLength(data);
            let needed = stride * (height - 1) + width * 4;
            let bytes = if length < 0 || (length as usize) < needed {
                None
            } else {
                let pointer = CFDataGetBytePtr(data);
                if pointer.is_null() {
                    None
                } else {
                    Some(std::slice::from_raw_parts(pointer, length as usize).to_vec())
                }
            };
            CFRelease(data);
            let bgra = bytes.ok_or_else(|| {
                CaptureFailure::Unavailable(format!(
                    "captured image truncated ({length} bytes for {width}x{height} stride {stride})"
                ))
            })?;
            Ok(SckImage {
                width,
                height,
                stride,
                bgra,
            })
        }
    }

    /// Capture one complete frame of the window. Errors never carry a
    /// partial image: the frame is either fully delivered or absent.
    pub fn capture(
        pid: i32,
        window_id: u32,
        width_pt: f64,
        height_pt: f64,
    ) -> Result<SckImage, CaptureFailure> {
        let (width_px, height_px) = capture_pixel_size(width_pt, height_pt)
            .ok_or_else(|| CaptureFailure::Unavailable("window has degenerate bounds".to_string()))?;
        autoreleasepool(|| unsafe {
            let content = shareable_content()?;
            let window = find_window(content, pid, window_id);
            let _: () = msg_send![content, release];
            let Some(window) = window else {
                return Err(CaptureFailure::WindowGone);
            };
            let filter: *mut Object = msg_send![class!(SCContentFilter), alloc];
            let filter: *mut Object = msg_send![filter, initWithDesktopIndependentWindow: window];
            let _: () = msg_send![window, release];
            if filter.is_null() {
                return Err(CaptureFailure::Unavailable(
                    "SCContentFilter init returned nil".to_string(),
                ));
            }
            let config: *mut Object = msg_send![class!(SCStreamConfiguration), alloc];
            let config: *mut Object = msg_send![config, init];
            let _: () = msg_send![config, setWidth: width_px];
            let _: () = msg_send![config, setHeight: height_px];
            // The window content must fill the whole configured frame:
            // without scalesToFit SCK draws it at its native size into the
            // top-left corner and leaves the rest black, which silently
            // breaks image->window coordinate mapping.
            let _: () = msg_send![config, setScalesToFit: true];
            let _: () = msg_send![config, setShowsCursor: false];
            // 'BGRA' little-endian fourcc.
            let _: () = msg_send![config, setPixelFormat: 0x4247_5241u32];
            let has_resolution: BOOL =
                msg_send![config, respondsToSelector: sel!(setCaptureResolution:)];
            if has_resolution == YES {
                // SCCaptureResolutionBest: honor the explicit pixel size.
                let _: () = msg_send![config, setCaptureResolution: 1i64];
            }
            let pair = completion(release_cf_object);
            let handler = pair.clone();
            let block = ConcreteBlock::new(move |image: *mut c_void, error: *mut Object| {
                let description = if error.is_null() {
                    None
                } else {
                    Some(error_description(error))
                };
                if !image.is_null() {
                    CFRetain(image);
                }
                deliver(&handler, image as usize, description);
            });
            let block = block.copy();
            let _: () = msg_send![class!(SCScreenshotManager),
                captureImageWithFilter: filter
                configuration: config
                completionHandler: &*block];
            // Filter and config are released on every path, including a
            // timed-out wait.
            let waited = wait_completion(&pair, COMPLETION_TIMEOUT);
            let _: () = msg_send![filter, release];
            let _: () = msg_send![config, release];
            let (image, error) = waited?;
            // deliver guarantees an error never carries an owned pointer.
            if let Some(description) = error {
                return Err(CaptureFailure::Unavailable(format!("capture: {description}")));
            }
            if image == 0 {
                return Err(CaptureFailure::Unavailable(
                    "capture returned a nil image".to_string(),
                ));
            }
            let image = image as *const c_void;
            let extracted = extract_pixels(image);
            CFRelease(image);
            extracted
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{DispatchPermit, GrantKind};
    use crate::target::MAX_SEMANTIC_TEXT_CHARS;

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
        static RELEASED: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
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
        let ax = vec![AxWindow { id: Some(1), frame }, AxWindow { id: None, frame }];
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
        let ax = vec![AxWindow { id: None, frame: ghost }, AxWindow { id: None, frame: live }];
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
        let permit = authorizer
            .spend_for_element_dispatch(&scope(), &target)
            .unwrap();
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
        let permit = authorizer
            .spend_for_element_dispatch(&scope(), &target)
            .unwrap();
        assert_eq!(
            native.semantic_action(&mut authorizer, &validated(dead, permit), &[], &press, &|| false),
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
        let permit = authorizer
            .spend_for_element_dispatch(&scope(), &target)
            .unwrap();
        assert_eq!(
            native.semantic_action(&mut authorizer, &validated(own, permit), &[], &press, &|| false),
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
        let permit = authorizer
            .spend_for_element_dispatch(&scope(), &target)
            .unwrap();
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
        let revoked = authorizer
            .spend_for_element_dispatch(&scope(), &target)
            .unwrap();
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
        assert!(!ax_secure("AXTextField", Some("AXStandardTextField"), false));
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
        assert!(recorded.iter().all(|seconds| *seconds > 0.0 && *seconds <= 30.0));
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
}
