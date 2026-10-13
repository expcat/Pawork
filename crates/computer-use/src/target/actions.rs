//! Semantic actions, targeted input, and pointer-action contracts (CU-06, CU-07, CU-08).

use super::*;

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

// ---------- targeted background input (CU-07) ----------

/// Longest text one targeted input carries, in chars (same bound as a
/// semantic value write).
pub const MAX_TARGETED_TEXT_CHARS: usize = 4096;

/// Non-printing keys a targeted key press can carry. Printable characters
/// travel as [TargetedInput::InsertText] text instead: the Unicode key-event
/// route is the measured background path, and a keycode for a character
/// depends on the target's keyboard layout, which the contract cannot see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetedKey {
    Return,
    Tab,
    Space,
    Backspace,
    Escape,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Up,
    Right,
    Down,
}

/// Modifier combo riding a targeted key press. Command is deliberately not
/// representable: a Command combo is a menu shortcut, and CU-01 measured
/// that menu shortcuts do not route in the background even on AppKit
/// (the menu bar belongs to the foreground). Use the semantic action route
/// or a foreground flow instead — there is no background fallback here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub option: bool,
}

impl KeyModifiers {
    pub fn none() -> Self {
        Self::default()
    }
}

/// One key press: a named key with an optional modifier combo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetedKeyPress {
    pub key: TargetedKey,
    pub modifiers: KeyModifiers,
}

/// Targeted input dispatched through the CGEventPostToPid route on a
/// validated element handle (CU-07): the complement to CU-06's semantic
/// writes for what only real key events reach. Every input passes the same
/// [require_background] gate ([NativeActionKind::TargetedKeys]): AppKit
/// background only — Chromium-family apps silently drop background key
/// events (CU-01), so they are rejected without a single event, and an
/// AppKit window that is minimized drops them just as silently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetedInput {
    /// Insert Unicode text at the target element's current insertion point
    /// (replacing a non-empty selection). This types; it never replaces the
    /// whole value — that is CU-06's [SemanticAction::SetValue].
    InsertText(String),
    /// One key press (keyDown + keyUp) with an optional modifier combo.
    KeyPress(TargetedKeyPress),
}

impl TargetedInput {
    /// The capability-matrix kind this input is gated by.
    pub fn kind(&self) -> NativeActionKind {
        NativeActionKind::TargetedKeys
    }

    /// Structural validity, mirroring the isolated-desktop text rule: text
    /// is 1..=[MAX_TARGETED_TEXT_CHARS] chars without control characters
    /// (control characters are key presses, not text), and the typed key
    /// vocabulary admits no invalid values by construction.
    pub fn validate(&self) -> Result<(), TargetError> {
        let TargetedInput::InsertText(text) = self else {
            return Ok(());
        };
        if text.is_empty()
            || text.chars().count() > MAX_TARGETED_TEXT_CHARS
            || text.chars().any(char::is_control)
        {
            return Err(TargetError::Invalid(
                "targeted text must be 1..4096 chars without control characters; use a key press for Return/Tab",
            ));
        }
        Ok(())
    }
}

/// The honest terminal state of one targeted input dispatch (CU-07). Clear
/// rejections are [TargetError]s decided before any event was posted; both
/// outcomes mean events reached the posting seam.
#[derive(Debug, PartialEq, Eq)]
pub enum TargetedOutcome {
    /// Every planned event was posted to the target pid. CGEventPostToPid
    /// acknowledges nothing, so this is dispatch proof only, never effect
    /// proof — verify with a fresh observation or a target-side fact.
    Dispatched {
        events_posted: usize,
        events_total: usize,
    },
    /// The sequence stopped early (cancellation or a dead target between
    /// events). Pressed state was released — every posted keyDown got its
    /// keyUp and every pressed modifier its release event — but events
    /// already posted may have taken effect, so the partial input is never
    /// silently retried as if it were a clean failure.
    Partial {
        events_posted: usize,
        events_total: usize,
        cause: TargetError,
    },
}

// ---------- targeted pointer actions (CU-08) ----------

/// Background pointer-action vocabulary (CU-08): click, drag and scroll
/// against a window, addressed in window points converted from an
/// observation (CU-04 image_to_window). CU-01 measured CGEventPostToPid
/// pointer events as ineffective in the background on every family
/// (caret does not move, buttons do not fire), so the capability matrix
/// holds Pointer Unsupported across the board: this entry point exists
/// so the contract rejects these actions explicitly — zero dispatch, no
/// auto-retry of anything that may have posted — and there is never a
/// fallback to moving the physical pointer. An in-target move is not
/// represented at all: it was never proven non-intrusive, and an
/// unproven action is not exposed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerAction {
    /// One left-button click at a window point.
    Click { point: WindowPoint },
    /// Left-button press at `from`, move, release at `to`.
    Drag { from: WindowPoint, to: WindowPoint },
    /// Scroll at a window point by finite deltas.
    Scroll {
        point: WindowPoint,
        delta_x: f64,
        delta_y: f64,
    },
}

impl PointerAction {
    /// The capability-matrix kind this action is gated by.
    pub fn kind(&self) -> NativeActionKind {
        NativeActionKind::Pointer
    }

    /// Structural validity against the observed window size: points are
    /// finite and inside `[0,width) × [0,height)` — the same
    /// edge-excluded bounds image_to_window enforces — and scroll deltas
    /// are finite. Checked before the capability gate, so a malformed
    /// payload reports its own rejection instead of the family's.
    pub fn validate(&self, window_width: f64, window_height: f64) -> Result<(), TargetError> {
        let in_bounds = |p: &WindowPoint| {
            p.x.is_finite()
                && p.y.is_finite()
                && p.x >= 0.0
                && p.y >= 0.0
                && p.x < window_width
                && p.y < window_height
        };
        match self {
            PointerAction::Click { point } | PointerAction::Scroll { point, .. } => {
                if !in_bounds(point) {
                    return Err(TargetError::OutOfBounds);
                }
            }
            PointerAction::Drag { from, to } => {
                if !in_bounds(from) || !in_bounds(to) {
                    return Err(TargetError::OutOfBounds);
                }
            }
        }
        if let PointerAction::Scroll {
            delta_x, delta_y, ..
        } = self
        {
            if !delta_x.is_finite() || !delta_y.is_finite() {
                return Err(TargetError::Invalid("scroll deltas must be finite"));
            }
        }
        Ok(())
    }
}

/// Move a bound window to a new top-left origin (CU-08): global screen
/// coordinates, written through the AX AXPosition route — the one window
/// move CU-01 measured as working on an occluded AppKit window. The
/// origin is not clamped to screen bounds: a display left of the primary
/// has legitimately negative global coordinates, and a partially
/// off-screen window is legal user state the contract will not silently
/// "correct".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowMove {
    pub x: f64,
    pub y: f64,
}

impl WindowMove {
    /// The capability-matrix kind this action is gated by.
    pub fn kind(&self) -> NativeActionKind {
        NativeActionKind::WindowMove
    }

    /// Structural validity: a finite origin.
    pub fn validate(&self) -> Result<(), TargetError> {
        if !self.x.is_finite() || !self.y.is_finite() {
            return Err(TargetError::Invalid("window origin must be finite"));
        }
        Ok(())
    }
}

/// What a bounded frontmost-bundle query can prove. Unprovable is not
/// evidence that the user is at the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityProof {
    /// The queried bundle is the frontmost application.
    Operating,
    /// Some other bundle is frontmost, or the frontmost app has no bundle id.
    Idle,
    /// The workspace or frontmost application could not be read.
    Unprovable,
}
