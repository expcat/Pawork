//! Accessibility cross-validation of window-server entries against AXWindows.

use super::*;

// ---------- accessibility cross-validation ----------

/// AX entry points resolved at runtime: HIServices is a subframework outside
/// the default linker search path — the same approach the CU-01 probe
/// validated. None of these ever prompts.
pub(super) struct AxFns {
    is_trusted: unsafe extern "C" fn() -> bool,
    pub(super) create_application: unsafe extern "C" fn(i32) -> *mut c_void,
    pub(super) copy_attribute:
        unsafe extern "C" fn(*mut c_void, CFStringRef, *mut *const c_void) -> i32,
    pub(super) value_get_value: unsafe extern "C" fn(*const c_void, i32, *mut c_void) -> bool,
    /// _AXUIElementGetWindow: private export mapping an AX window to its
    /// window-server id (CGWindowID). Undocumented but stable for well
    /// over a decade (yabai, Hammerspoon and others rely on it); when it
    /// is missing the backend falls back to frame-group closure.
    get_window: Option<unsafe extern "C" fn(*const c_void, *mut u32) -> i32>,
    /// CU-05 tree walk: ranged children fetch, child count, action names
    /// and a per-call messaging timeout.
    pub(super) copy_attribute_values:
        Option<unsafe extern "C" fn(*mut c_void, CFStringRef, i64, i64, *mut *const c_void) -> i32>,
    pub(super) attribute_value_count:
        Option<unsafe extern "C" fn(*mut c_void, CFStringRef, *mut i64) -> i32>,
    pub(super) copy_action_names:
        Option<unsafe extern "C" fn(*mut c_void, *mut *const c_void) -> i32>,
    pub(super) set_messaging_timeout: Option<unsafe extern "C" fn(*mut c_void, f32) -> i32>,
    /// Chromium-family targets serialize their real tree only once a
    /// client sets AXManualAccessibility on the application element.
    pub(super) set_attribute:
        Option<unsafe extern "C" fn(*mut c_void, CFStringRef, *const c_void) -> i32>,
    /// CU-06 semantic dispatch. Optional: reads work without them; the
    /// semantic action refuses when the one it needs is missing.
    pub(super) perform_action: Option<unsafe extern "C" fn(*mut c_void, CFStringRef) -> i32>,
    /// CU-08 window move: builds the CGPoint AXValue the AXPosition
    /// write carries. Optional: absent refuses the move instead of
    /// posting a malformed attribute.
    pub(super) value_create: Option<unsafe extern "C" fn(i32, *const c_void) -> *const c_void>,
}

pub(super) fn axf() -> Option<&'static AxFns> {
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
            value_create: load(handle, "AXValueCreate"),
        })
    })
    .as_ref()
}

pub(super) fn ax_is_process_trusted() -> bool {
    matches!(ax_preflight(), AxPreflight::Trusted)
}

/// Accessibility preflight. A symbol-load failure is NOT a permission
/// report: it propagates as a probe failure instead of PermissionMissing.
pub(super) enum AxPreflight {
    Trusted,
    NotTrusted,
    Unavailable,
}

pub(super) fn ax_preflight() -> AxPreflight {
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
    pub(super) fn CFArrayGetCount(array: *const c_void) -> isize;
    pub(super) fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    pub(super) fn CFRelease(pointer: *const c_void);
    pub(super) fn CFRetain(pointer: *const c_void) -> *const c_void;
    pub(super) fn CFDataGetLength(data: *const c_void) -> isize;
    pub(super) fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    pub(super) fn CFBooleanGetValue(boolean: *const c_void) -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub(super) fn CGImageGetWidth(image: *const c_void) -> usize;
    pub(super) fn CGImageGetHeight(image: *const c_void) -> usize;
    pub(super) fn CGImageGetBytesPerRow(image: *const c_void) -> usize;
    pub(super) fn CGImageGetDataProvider(image: *const c_void) -> *mut c_void;
    pub(super) fn CGDataProviderCopyData(provider: *mut c_void) -> *mut c_void;
}

pub(super) const K_AX_VALUE_TYPE_CGPOINT: i32 = 1;
pub(super) const K_AX_VALUE_TYPE_CGSIZE: i32 = 2;

/// One window from the app's own AXWindows: its window-server id when the
/// _AXUIElementGetWindow export resolved it (None when the export is
/// missing or failed for this window), and its global frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct AxWindow {
    pub(super) id: Option<u32>,
    pub(super) frame: [f64; 4],
}

/// Why reading the app's AXWindows failed. Distinct from the permission
/// preflight: these are probe-call failures, and access is still refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AxReadError {
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
pub(super) const K_AX_ERROR_API_DISABLED: i32 = -25211;

/// Map an AX read failure to the honest contract error: the API-disabled
/// code is a permission report; everything else keeps its cause as a
/// probe failure. Both refuse access.
pub(super) fn ax_read_target_error(error: AxReadError) -> TargetError {
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
pub(super) struct AxWindowRef {
    pub(super) element: *mut c_void,
    pub(super) id: Option<u32>,
    pub(super) frame: [f64; 4],
}

impl Drop for AxWindowRef {
    fn drop(&mut self) {
        unsafe { CFRelease(self.element as *const c_void) }
    }
}

/// Remaining slice of a read's wall-clock budget as messaging-timeout
/// seconds for the next AX call; None once the budget is blown, so the
/// caller fails instead of starting another unbounded call.
pub(super) fn remaining_budget(deadline: Instant) -> Option<f32> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    Some(remaining.as_secs_f32().max(f32::EPSILON))
}

/// Per-call wall-clock budget of one bounded AX read. Messaging timeouts
/// are per element AND stale the moment they are set: every AX call is
/// armed with the budget remaining when THAT call starts (arm), and the
/// clock is re-checked when it returns (check), so the deadline bounds
/// the whole read — never just one phase — and no call ever starts
/// unbounded (a blown clock or a failed timeout-set refuses instead).
pub(super) struct ReadBudget {
    pub(super) set_timeout: unsafe extern "C" fn(*mut c_void, f32) -> i32,
    pub(super) deadline: Instant,
}

/// Why the budget refused a read. Mapped per context: tree reads to
/// TargetError (budget_target_error), window-list reads to AxReadError
/// (budget_read_error).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BudgetError {
    /// The wall-clock budget is blown.
    Overtime,
    /// AXUIElementSetMessagingTimeout failed (raw AXError).
    SetFailed(i32),
}

impl ReadBudget {
    /// Arm one element's messaging timeout with the budget remaining
    /// right now. element must be a live AXUIElementRef.
    pub(super) fn arm(&self, element: *mut c_void) -> Result<(), BudgetError> {
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
    pub(super) fn check(&self) -> Result<(), BudgetError> {
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
    pub(super) fn call<T>(
        &self,
        element: *mut c_void,
        f: impl FnOnce() -> T,
    ) -> Result<T, BudgetError> {
        self.arm(element)?;
        let out = f();
        self.check()?;
        Ok(out)
    }
}

/// A budget refusal inside a window-list read.
pub(super) fn budget_read_error(err: BudgetError) -> AxReadError {
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
pub(super) fn ax_window_list(
    pid: i32,
    deadline: Option<Instant>,
) -> Result<Vec<AxWindowRef>, AxReadError> {
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
pub(super) fn ax_windows(pid: i32) -> Result<Vec<AxWindow>, AxReadError> {
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

pub(super) fn bounds_frame(bounds: &WindowBounds) -> [f64; 4] {
    [bounds.x, bounds.y, bounds.width, bounds.height]
}

pub(super) fn frame_eq(a: &[f64; 4], b: &[f64; 4]) -> bool {
    const EPSILON: f64 = 0.5;
    (0..4).all(|i| (a[i] - b[i]).abs() <= EPSILON)
}

/// A window-server window and an AX window sit at the same place when
/// their global frames agree within half a point (both are reported in
/// points, top-left origin; verified exact for TextEdit in CU-03).
pub(super) fn frame_matches(bounds: &WindowBounds, frame: &[f64; 4]) -> bool {
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
pub(super) fn window_proven_alive(
    windows: &[RawWindow],
    ax: &[AxWindow],
    pid: i32,
    w: &RawWindow,
) -> bool {
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
    !ax.iter().filter(|a| frame_eq(&a.frame, &frame)).any(|a| {
        windows.iter().any(|x| {
            x.owner_pid == pid
                && x.layer == 0
                && frame_eq(&bounds_frame(&x.bounds), &a.frame)
                && !frame_matches(&x.bounds, &frame)
        })
    })
}
