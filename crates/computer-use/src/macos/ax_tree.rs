//! Bounded accessibility-tree reads (CU-05).

use super::*;

// ---------- bounded accessibility tree read (CU-05) ----------

/// Test-only proof that denied tree reads never reach the AX walk seam.
#[cfg(test)]
pub(super) static AX_WALK_SYSTEM_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// kAXErrorNoValue / kAXErrorAttributeUnsupported (AXError.h): the element
/// affirmatively has no such attribute — an answer, not a failure.
pub(super) const K_AX_ERROR_NO_VALUE: i32 = -25212;
pub(super) const K_AX_ERROR_ATTRIBUTE_UNSUPPORTED: i32 = -25205;

pub(super) fn ax_attr_absent(err: i32) -> bool {
    err == K_AX_ERROR_NO_VALUE || err == K_AX_ERROR_ATTRIBUTE_UNSUPPORTED
}

/// An AX failure during the tree walk keeps its cause: the API-disabled
/// code is a permission report, everything else a probe failure. Both
/// refuse access.
pub(super) fn ax_tree_read_error(err: i32) -> TargetError {
    if err == K_AX_ERROR_API_DISABLED {
        TargetError::PermissionMissing("accessibility")
    } else {
        TargetError::ProbeUnavailable(format!("accessibility tree read failed: AXError {err}"))
    }
}

/// A budget refusal inside a tree read: overtime is a probe failure, a
/// failed timeout-set keeps its raw AXError cause.
pub(super) fn budget_target_error(err: BudgetError) -> TargetError {
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
pub(super) unsafe fn ax_attr_raw(
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
pub(super) unsafe fn ax_value_tuple(
    f: &AxFns,
    raw: *const c_void,
    value_type: i32,
) -> Option<(f64, f64)> {
    let mut pair = [0.0f64; 2];
    let ok = (f.value_get_value)(raw, value_type, pair.as_mut_ptr() as *mut c_void);
    ok.then_some((pair[0], pair[1]))
}

/// Cut on a char boundary; the flag records every cut.
pub(super) fn truncate_chars(text: &str, max_text: usize) -> (String, bool) {
    if text.chars().count() <= max_text {
        return (text.to_string(), false);
    }
    (text.chars().take(max_text).collect(), true)
}

/// A string attribute cut to max_text chars (flag set when cut).
/// Non-string values are absent — never a read of the wrong type.
pub(super) unsafe fn ax_attr_string(
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
pub(super) unsafe fn ax_attr_bool(
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
pub(super) fn ax_secure(role: &str, subrole: Option<&str>, subrole_read_failed: bool) -> bool {
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
    let f = axf().ok_or_else(|| {
        TargetError::ProbeUnavailable("HIServices symbols unavailable".to_string())
    })?;
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
pub(super) fn resolve_window_element(
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
    copy_values:
        unsafe extern "C" fn(*mut c_void, CFStringRef, i64, i64, *mut *const c_void) -> i32,
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
            .call(element, || unsafe {
                ax_attr_string(self.f, element, "AXRole", 64)
            })
            .map_err(budget_target_error)?
            .map_err(ax_tree_read_error)?
            .map(|(role, _)| role)
            .ok_or_else(|| TargetError::ProbeUnavailable("element without a role".to_string()))?;
        // Secure-input identification (see ax_secure): the SDK defines
        // AXSecureTextField as a subrole, so AXTextField elements get an
        // AXSubrole read; a failed identification stays conservative and
        // no name is read.
        let secure = if role == "AXTextField" {
            match self.budget.call(element, || unsafe {
                ax_attr_string(self.f, element, "AXSubrole", 64)
            }) {
                Err(budget) => return Err(budget_target_error(budget)),
                Ok(Ok(subrole)) => ax_secure(
                    &role,
                    subrole.as_ref().map(|(text, _)| text.as_str()),
                    false,
                ),
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
            .call(element, || unsafe {
                ax_attr_bool(self.f, element, "AXEnabled")
            })
            .map_err(budget_target_error)?
            .unwrap_or(None);
        let focused = self
            .budget
            .call(element, || unsafe {
                ax_attr_bool(self.f, element, "AXFocused")
            })
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
pub(super) fn tree_symbols() -> Result<&'static AxFns, TargetError> {
    let f = axf().ok_or_else(|| {
        TargetError::ProbeUnavailable("HIServices symbols unavailable".to_string())
    })?;
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
pub(super) fn walk_ax_tree(
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
