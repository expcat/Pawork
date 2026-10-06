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
//! Fail-closed limits: this backend has no signal source for the
//! accessibility-tree revision or the current web origin yet (CU-05 / CU-15
//! territory), so it reports None for both. The registry treats that as
//! unavailable: AX tree-bound observations and website targets cannot
//! validate against this probe and are rejected (WindowReplaced /
//! SiteChanged) before dispatch. Application discovery, window binding and
//! capture observations are unaffected.
use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::ptr;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use block::ConcreteBlock;
use core_foundation::base::{CFType, ItemRef, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::{CFNumber, CFNumberRef};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::access::ScreenCaptureAccess;
use core_graphics::display::CGMainDisplayID;
use core_graphics::geometry::{CGPoint, CGSize};
use core_graphics::window as cgwindow;
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, RgbImage};
use objc::rc::autoreleasepool;
use objc::runtime::{Object, BOOL, YES};
use objc::{class, msg_send, sel, sel_impl};

use crate::approval::{TargetAuthorizer, TargetIdentity};
use crate::target::{
    AppFamily, AppIdentity, NativeProbe, ProcessInstance, Scope, TargetError, ValidatedWindow,
    WindowIdentity,
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
        let id = u32::try_from(window_id).ok()?;
        let ax = ax_windows(instance.pid as i32).ok()?;
        let pid = instance.pid as i32;
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

    /// No AX tree-revision signal source yet (CU-05 territory): reported as
    /// unavailable, so AX tree-bound handles fail closed (WindowReplaced)
    /// instead of silently trusting a tree that may have advanced.
    fn tree_revision(&self, _window: &WindowIdentity) -> Option<u64> {
        None
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
        AxReadError::ReadFailed(code) if code == K_AX_ERROR_API_DISABLED => {
            TargetError::PermissionMissing("accessibility")
        }
        AxReadError::ReadFailed(code) => {
            TargetError::ProbeUnavailable(format!("AXWindows read failed: AXError {code}"))
        }
    }
}

/// The app's own AXWindows — the authoritative answer to which windows
/// actually exist.
fn ax_windows(pid: i32) -> Result<Vec<AxWindow>, AxReadError> {
    let f = axf().ok_or(AxReadError::SymbolsUnavailable)?;
    autoreleasepool(|| unsafe {
        let app = (f.create_application)(pid);
        if app.is_null() {
            return Err(AxReadError::CreateFailed);
        }
        let mut windows: *const c_void = ptr::null();
        let err = (f.copy_attribute)(app, nsstr("AXWindows") as CFStringRef, &mut windows);
        if err != 0 || windows.is_null() {
            CFRelease(app as *const c_void);
            return Err(AxReadError::ReadFailed(err));
        }
        let count = CFArrayGetCount(windows);
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let window = CFArrayGetValueAtIndex(windows, i) as *mut c_void;
            if window.is_null() {
                continue;
            }
            let id = f.get_window.and_then(|get_window| {
                let mut id: u32 = 0;
                (get_window(window as *const c_void, &mut id) == 0).then_some(id)
            });
            let Some(frame) = ax_window_frame(f, window) else {
                continue;
            };
            out.push(AxWindow { id, frame });
        }
        CFRelease(windows);
        CFRelease(app as *const c_void);
        Ok(out)
    })
}

/// One AX window's frame (x, y, w, h in global points); None when the
/// position or size attributes cannot be read.
fn ax_window_frame(f: &AxFns, window: *mut c_void) -> Option<[f64; 4]> {
    unsafe {
        let mut position = CGPoint::new(0.0, 0.0);
        let mut size = CGSize::new(0.0, 0.0);
        let mut raw: *const c_void = ptr::null();
        if (f.copy_attribute)(window, nsstr("AXPosition") as CFStringRef, &mut raw) != 0
            || raw.is_null()
            || !(f.value_get_value)(
                raw,
                K_AX_VALUE_TYPE_CGPOINT,
                &mut position as *mut CGPoint as *mut c_void,
            )
        {
            if !raw.is_null() {
                CFRelease(raw);
            }
            return None;
        }
        CFRelease(raw);
        let mut raw: *const c_void = ptr::null();
        if (f.copy_attribute)(window, nsstr("AXSize") as CFStringRef, &mut raw) != 0
            || raw.is_null()
            || !(f.value_get_value)(
                raw,
                K_AX_VALUE_TYPE_CGSIZE,
                &mut size as *mut CGSize as *mut c_void,
            )
        {
            if !raw.is_null() {
                CFRelease(raw);
            }
            return None;
        }
        CFRelease(raw);
        Some([position.x, position.y, size.width, size.height])
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
                    b"/System/Library/Frameworks/ScreenCaptureKit.framework/ScreenCaptureKit ";
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
    use crate::approval::GrantKind;

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
}
