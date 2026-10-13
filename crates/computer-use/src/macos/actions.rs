//! Semantic element actions (CU-06).

use super::*;

// ---------- semantic element actions (CU-06) ----------

/// Test-only proof that denied semantic actions never reach the AX action
/// seam.
#[cfg(test)]
pub(super) static AX_ACTION_SYSTEM_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// kAXErrorCannotComplete (AXError.h): the call did not answer within its
/// messaging timeout. For an action dispatch the effect is unknown — the
/// element may have performed it — so it is never reported as a clean
/// failure.
pub(super) const K_AX_ERROR_CANNOT_COMPLETE: i32 = -25204;
/// kAXErrorInvalidUIElement: the element is gone — the live tree moved
/// past the revision the dispatch was validated against.
pub(super) const K_AX_ERROR_INVALID_UI_ELEMENT: i32 = -25202;
/// kAXErrorActionUnsupported: the element affirmatively refuses the
/// action.
pub(super) const K_AX_ERROR_ACTION_UNSUPPORTED: i32 = -25206;

/// The AX action call's return code mapped to the honest terminal state:
/// success is Dispatched (not proof of effect); CannotComplete is
/// UnknownEffect (a timed-out action may already have happened — never
/// blindly retried); the element's own refusal codes are
/// ElementUnsupported (a clear rejection — the call ran and the element
/// declined, nothing was dispatched); a dead element means the tree moved
/// past the validated revision; API-disabled is the permission report;
/// everything else keeps its cause as a probe failure.
pub(super) fn semantic_action_result(err: i32) -> Result<SemanticOutcome, TargetError> {
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

/// The minimized half of the window-move capability gate (CU-08). Only an
/// affirmative Some(false) proves the unminimized state CU-01 measured;
/// Some(true) passes the value on to require_background (the minimized
/// move cell is Unverified and rejects there), and an unknown state —
/// AXMinimized absent or non-boolean — rejects outright: a dispatch must
/// be provably inside the measured matrix, never defaulted into it.
pub(super) fn move_minimized_gate(state: Option<bool>) -> Result<bool, TargetError> {
    match state {
        Some(minimized) => Ok(minimized),
        None => Err(TargetError::ProbeUnavailable(
            "AXMinimized unknown; cannot prove the window unminimized".to_string(),
        )),
    }
}

/// A retained CF object released exactly once on drop (create-rule
/// discipline): every exit path after acquisition — budget failure,
/// cancellation, the action call — releases the object exactly once
/// without hand-written CFRelease pairs per branch.
pub(super) struct RetainedCfValue(pub(super) *const c_void);

impl Drop for RetainedCfValue {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

/// An AX read failure while resolving or classifying the dispatch element
/// keeps its cause; a dead element is a tree change here.
pub(super) fn ax_dispatch_read_error(err: i32) -> TargetError {
    if err == K_AX_ERROR_INVALID_UI_ELEMENT {
        TargetError::TreeChanged
    } else {
        ax_tree_read_error(err)
    }
}

/// A retained AX element released on drop: copy-rule discipline for the
/// dispatch-side path resolution.
pub(super) struct RetainedElement(pub(super) *mut c_void);

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
pub(super) fn resolve_element_by_path(
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
pub(super) fn resolve_path_child(
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

/// Secure-input classification of a resolved dispatch element: the role
/// (and subrole, for AXTextField) is re-read with the same conservative
/// classification as the walk; a failed read classifies as secure.
fn element_is_secure(
    f: &AxFns,
    element: &RetainedElement,
    budget: &ReadBudget,
) -> Result<bool, TargetError> {
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
    Ok(secure)
}

/// Value writes to secure-input elements are refused: the read side never
/// reads secure content (CU-05), and the write side never silently enters
/// it — entering credentials is the user's keyboard, not a background AX
/// write. Presses are unaffected.
pub(super) fn refuse_secure_write(
    f: &AxFns,
    element: &RetainedElement,
    budget: &ReadBudget,
) -> Result<(), TargetError> {
    if element_is_secure(f, element, budget)? {
        return Err(TargetError::Invalid(
            "semantic value writes to secure-input elements are refused",
        ));
    }
    Ok(())
}

/// Targeted input to secure-input elements is refused with the same rule:
/// entering credentials is the user's keyboard, not a background agent's
/// posted key events.
pub(super) fn refuse_secure_targeted(
    f: &AxFns,
    element: &RetainedElement,
    budget: &ReadBudget,
) -> Result<(), TargetError> {
    if element_is_secure(f, element, budget)? {
        return Err(TargetError::Invalid(
            "targeted input to secure-input elements is refused",
        ));
    }
    Ok(())
}
