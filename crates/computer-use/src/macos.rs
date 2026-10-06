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
use std::sync::{Arc, Mutex, OnceLock};

use core_foundation::base::{CFType, ItemRef, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::{CFNumber, CFNumberRef};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::access::ScreenCaptureAccess;
use core_graphics::geometry::{CGPoint, CGSize};
use core_graphics::window as cgwindow;
use objc::rc::autoreleasepool;
use objc::runtime::{Object, BOOL, YES};
use objc::{class, msg_send, sel, sel_impl};

use crate::approval::{TargetAuthorizer, TargetIdentity};
use crate::target::{
    AppFamily, AppIdentity, NativeProbe, ProcessInstance, Scope, TargetError, WindowIdentity,
};

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
        if running_bundle_id(pid).as_deref() != Some(identity.bundle_id.as_str()) {
            return Err(TargetError::Invalid(
                "process instance does not belong to the authorized identity",
            ));
        }
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
}

impl NativeProbe for MacosNative {
    fn process_instance_alive(&self, instance: &ProcessInstance) -> bool {
        if ns_app_terminated(instance.pid as i32) == Some(true) {
            return false;
        }
        process_start_token(instance.pid as i32) == Some(instance.start_token)
    }

    fn window_generation(&self, instance: &ProcessInstance, window_id: u64) -> Option<u64> {
        let id = u32::try_from(window_id).ok()?;
        // Any AX failure (permission missing, app unresponsive, symbols
        // unavailable) fails closed: the window cannot be proven alive.
        let ax = ax_windows(instance.pid as i32).ok()?;
        let pid = instance.pid as i32;
        self.with_window_snapshot(None, |windows, ledger| {
            let window = windows.iter().find(|w| w.id == id && w.owner_pid == pid)?;
            if !window_proven_alive(windows, &ax, pid, window) {
                return None;
            }
            ledger.generation(id)
        })
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
