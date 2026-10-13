//! Targeted background input (CU-07).

use super::*;

// ---------- targeted background input (CU-07) ----------

/// Test-only proof that denied targeted inputs never reach the event
/// posting seam.
#[cfg(test)]
pub(super) static TARGETED_POST_SYSTEM_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
/// Test-only record of the flags every posted event carried, so the
/// posting-discipline test can assert the actual modifier masks of the
/// pre-built release (counts alone cannot see wrong flags).
#[cfg(test)]
pub(super) static TARGETED_POSTED_FLAGS: std::sync::Mutex<Vec<CGEventFlags>> =
    std::sync::Mutex::new(Vec::new());

/// A boolean attribute read on the dispatch path: armed with the shared
/// deadline, keeping its cause. None is an affirmative absence, never a
/// claim of false.
pub(super) fn dispatch_bool_attr(
    f: &AxFns,
    element: *mut c_void,
    attribute: &str,
    budget: &ReadBudget,
) -> Result<Option<bool>, TargetError> {
    budget
        .call(element, || unsafe { ax_attr_bool(f, element, attribute) })
        .map_err(budget_target_error)?
        .map_err(ax_dispatch_read_error)
}

/// Whether the element exposes a selection range (AXSelectedTextRange):
/// the proof that a text insert has a live insertion point to land in.
/// Only presence is read — never the range contents.
pub(super) fn selection_present(
    f: &AxFns,
    element: &RetainedElement,
    budget: &ReadBudget,
) -> Result<bool, TargetError> {
    budget.arm(element.0).map_err(budget_target_error)?;
    let mut raw: *const c_void = ptr::null();
    let err = unsafe {
        (f.copy_attribute)(
            element.0,
            nsstr("AXSelectedTextRange") as CFStringRef,
            &mut raw,
        )
    };
    let clock = budget.check();
    if err != 0 || raw.is_null() {
        if !raw.is_null() {
            unsafe { CFRelease(raw) };
        }
        if let Err(overtime) = clock {
            return Err(budget_target_error(overtime));
        }
        if ax_attr_absent(err) {
            return Ok(false);
        }
        return Err(ax_dispatch_read_error(err));
    }
    if let Err(overtime) = clock {
        unsafe { CFRelease(raw) };
        return Err(budget_target_error(overtime));
    }
    unsafe { CFRelease(raw) };
    Ok(true)
}

/// Max UTF-16 units one Unicode key event carries (the CU-01 probe shape).
pub(super) const MAX_EVENT_UTF16_UNITS: usize = 10;

/// Pacing between planned steps, the rhythm the CU-01 measurement used.
const TARGETED_STEP_PACING: Duration = Duration::from_millis(6);

/// Ceiling for one between-step binding recheck in the posting loop:
/// the generation proof plus two attribute reads must stay quick — the
/// recheck guards every step, so it cannot inherit the full read budget.
pub(super) const TARGETED_RECHECK_BUDGET: Duration = Duration::from_millis(250);

/// kVK_ keycodes (HIToolbox/Events.h) of the contract's key vocabulary.
fn targeted_keycode(key: TargetedKey) -> CGKeyCode {
    match key {
        TargetedKey::Return => 36,
        TargetedKey::Tab => 48,
        TargetedKey::Space => 49,
        TargetedKey::Backspace => 51,
        TargetedKey::Escape => 53,
        TargetedKey::Delete => 117,
        TargetedKey::Home => 115,
        TargetedKey::End => 119,
        TargetedKey::PageUp => 116,
        TargetedKey::PageDown => 121,
        TargetedKey::Left => 123,
        TargetedKey::Up => 126,
        TargetedKey::Right => 124,
        TargetedKey::Down => 125,
    }
}

/// Active modifiers as (keycode, flags) in the fixed press order.
fn active_modifiers(modifiers: &KeyModifiers) -> Vec<(CGKeyCode, CGEventFlags)> {
    let mut active = Vec::new();
    if modifiers.shift {
        active.push((56, CGEventFlags::CGEventFlagShift));
    }
    if modifiers.control {
        active.push((59, CGEventFlags::CGEventFlagControl));
    }
    if modifiers.option {
        active.push((58, CGEventFlags::CGEventFlagAlternate));
    }
    active
}

/// One balanced group of posted events. A group, once started, always
/// completes — so cancellation between groups never leaves a keyDown
/// without its keyUp.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PlannedStep {
    /// One Unicode chunk: keyDown carrying the string plus its keyUp.
    TextChunk(Vec<u16>),
    /// Modifier presses before a key tap: one flags-changed event per
    /// active modifier, each carrying the accumulated mask.
    ModsDown(Vec<(CGKeyCode, CGEventFlags)>),
    /// One key tap: keyDown with the modifier flags, keyUp clean.
    KeyTap {
        keycode: CGKeyCode,
        flags: CGEventFlags,
    },
    /// Modifier releases after a key tap: the reverse presses, each
    /// carrying the shrinking mask, ending clean.
    ModsUp(Vec<(CGKeyCode, CGEventFlags)>),
}

impl PlannedStep {
    pub(super) fn event_count(&self) -> usize {
        match self {
            PlannedStep::TextChunk(_) | PlannedStep::KeyTap { .. } => 2,
            PlannedStep::ModsDown(specs) | PlannedStep::ModsUp(specs) => specs.len(),
        }
    }
}

/// The event plan of one targeted input: chunked on char boundaries (a
/// surrogate pair never splits across events), modifier combos expressed
/// as explicit flags-changed press/release steps around one key tap, so
/// pressed state always exists to release. A bare key is one tap; text is
/// chunk pairs.
pub(super) fn targeted_event_plan(input: &TargetedInput) -> Vec<PlannedStep> {
    match input {
        TargetedInput::InsertText(text) => {
            let mut chunks = Vec::new();
            let mut current: Vec<u16> = Vec::new();
            for character in text.chars() {
                let mut buffer = [0u16; 2];
                let units = character.encode_utf16(&mut buffer);
                if current.len() + units.len() > MAX_EVENT_UTF16_UNITS {
                    chunks.push(std::mem::take(&mut current));
                }
                current.extend_from_slice(units);
            }
            if !current.is_empty() {
                chunks.push(current);
            }
            chunks.into_iter().map(PlannedStep::TextChunk).collect()
        }
        TargetedInput::KeyPress(press) => {
            let active = active_modifiers(&press.modifiers);
            let combined = active
                .iter()
                .fold(CGEventFlags::empty(), |flags, (_, more)| flags | *more);
            let mut plan = Vec::new();
            if !active.is_empty() {
                let mut accumulated = CGEventFlags::empty();
                plan.push(PlannedStep::ModsDown(
                    active
                        .iter()
                        .map(|(keycode, flags)| {
                            accumulated |= *flags;
                            (*keycode, accumulated)
                        })
                        .collect(),
                ));
            }
            plan.push(PlannedStep::KeyTap {
                keycode: targeted_keycode(press.key),
                flags: combined,
            });
            if !active.is_empty() {
                plan.push(PlannedStep::ModsUp(release_specs(&active)));
            }
            plan
        }
    }
}

/// Release specs matching a press group: the reverse presses, each
/// carrying the mask remaining after that modifier lifts; the last one
/// ends clean.
pub(super) fn release_specs(
    active: &[(CGKeyCode, CGEventFlags)],
) -> Vec<(CGKeyCode, CGEventFlags)> {
    let combined = active
        .iter()
        .fold(CGEventFlags::empty(), |flags, (_, more)| flags | *more);
    let mut remaining = combined;
    let mut specs = Vec::with_capacity(active.len());
    for (keycode, flags) in active.iter().rev() {
        remaining -= *flags;
        specs.push((*keycode, remaining));
    }
    specs
}

fn event_create_failed() -> TargetError {
    TargetError::ProbeUnavailable("CGEventCreateKeyboardEvent failed".to_string())
}

/// Test injection for the creation seam: while FAIL_AT is non-zero, the
/// Nth creation call (1-based, counted process-wide by CALLS) fails.
/// Only this module's posting tests create keyboard events, so the
/// counters stay race-free inside the single posting-discipline test.
#[cfg(test)]
pub(super) static TARGETED_CREATE_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub(super) static TARGETED_CREATE_FAIL_AT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Create one keyboard event at the shared injection seam.
fn create_keyboard_event(
    source: &CGEventSource,
    keycode: CGKeyCode,
    key_down: bool,
) -> Result<CGEvent, TargetError> {
    #[cfg(test)]
    {
        use std::sync::atomic::Ordering;
        let nth = TARGETED_CREATE_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        if TARGETED_CREATE_FAIL_AT.load(Ordering::SeqCst) == nth {
            return Err(event_create_failed());
        }
    }
    CGEvent::new_keyboard_event(source.clone(), keycode, key_down)
        .map_err(|_| event_create_failed())
}

/// One flags-changed event per spec, carrying each spec's mask.
fn create_flags_changed_events(
    source: &CGEventSource,
    specs: &[(CGKeyCode, CGEventFlags)],
) -> Result<Vec<CGEvent>, TargetError> {
    specs
        .iter()
        .map(|(keycode, flags)| {
            let event = create_keyboard_event(source, *keycode, true)?;
            event.set_type(CGEventType::FlagsChanged);
            event.set_flags(*flags);
            Ok(event)
        })
        .collect()
}

/// One Unicode chunk: keyDown carrying the string plus its keyUp.
fn create_text_chunk_events(
    source: &CGEventSource,
    units: &[u16],
) -> Result<Vec<CGEvent>, TargetError> {
    let down = create_keyboard_event(source, 0, true)?;
    down.set_string_from_utf16_unchecked(units);
    let up = create_keyboard_event(source, 0, false)?;
    Ok(vec![down, up])
}

/// One key tap: keyDown with the modifier flags, keyUp clean.
fn create_key_tap_events(
    source: &CGEventSource,
    keycode: CGKeyCode,
    flags: CGEventFlags,
) -> Result<Vec<CGEvent>, TargetError> {
    let down = create_keyboard_event(source, keycode, true)?;
    down.set_flags(flags);
    let up = create_keyboard_event(source, keycode, false)?;
    Ok(vec![down, up])
}

/// Post one event to the target pid only. This is the CU-07 seam counted
/// in tests: every gate must reject before it.
fn post_key_event(pid: libc::pid_t, event: &CGEvent) {
    #[cfg(test)]
    {
        TARGETED_POST_SYSTEM_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        TARGETED_POSTED_FLAGS
            .lock()
            .expect("posted-flags record")
            .push(event.get_flags());
    }
    event.post_to_pid(pid);
}

/// Post a pre-built event group and return how many went out.
fn post_events(pid: libc::pid_t, events: &[CGEvent]) -> usize {
    for event in events {
        post_key_event(pid, event);
    }
    events.len()
}

/// Post the whole plan, checking cancellation, target liveness and the
/// binding recheck between balanced steps. Every group, including the
/// first, is built and then checked
/// again before it posts. The caller's entry check is not the last one:
/// planning and event creation sit between it and the post, so a stop
/// that flips there must not reach the seam. Nothing posted yet is a
/// plain error and does not emit a held release; anything already posted
/// still releases and returns Partial. The plan's closing ModsUp stays
/// outside this check. The release promise is structural: a modifier
/// press group
/// posts only after its matching release events are created and held, so
/// whatever stops the input later — cancellation, a dead process, a
/// failed binding recheck, a creation failure on a later step — can
/// still post the held release (posting never creates anything, so
/// cleanup is never blocked), and posts exactly the pressed state this
/// dispatch created: with nothing pressed, nothing is released. A
/// creation failure before anything posted and anything pressed is a
/// plain error — there is no partial effect to report; otherwise an
/// early stop is [TargetedOutcome::Partial] with the events actually
/// posted: a partial sequence may already have taken effect and is never
/// retried as if it were clean. All steps posted is
/// [TargetedOutcome::Dispatched]: CGEventPostToPid acknowledges nothing,
/// so dispatch is the proof, not the effect.
pub(super) fn post_targeted_plan(
    source: &CGEventSource,
    pid: libc::pid_t,
    plan: &[PlannedStep],
    target_alive: &dyn Fn() -> bool,
    cancelled: &dyn Fn() -> bool,
    recheck: &dyn Fn() -> Option<TargetError>,
) -> Result<TargetedOutcome, TargetError> {
    let total: usize = plan.iter().map(PlannedStep::event_count).sum();
    // While this is Some, pressed state exists and its release events
    // are already created: the balance promise cannot be broken by any
    // later failure.
    let mut held_release: Option<Vec<CGEvent>> = None;
    let release_held = |posted: &mut usize, held: &mut Option<Vec<CGEvent>>| {
        if let Some(events) = held.take() {
            *posted += post_events(pid, &events);
        }
    };
    let partial = |posted: usize, cause: TargetError| TargetedOutcome::Partial {
        events_posted: posted,
        events_total: total,
        cause,
    };
    // After this group's events exist and before they post, including
    // index 0. ModsUp never reaches it. Nothing posted yet does not
    // emit a held release.
    let cancelled_before_post = |posted: &mut usize, held: &mut Option<Vec<CGEvent>>| {
        if !cancelled() {
            return None;
        }
        if *posted == 0 {
            held.take();
            return Some(Err(TargetError::Cancelled));
        }
        release_held(posted, held);
        Some(Ok(partial(*posted, TargetError::Cancelled)))
    };
    let mut posted = 0usize;
    for (index, step) in plan.iter().enumerate() {
        if index > 0 {
            let cause = if cancelled() {
                Some(TargetError::Cancelled)
            } else if !target_alive() {
                Some(TargetError::ProcessRestarted)
            } else {
                recheck()
            };
            if let Some(cause) = cause {
                release_held(&mut posted, &mut held_release);
                return Ok(partial(posted, cause));
            }
        }
        let built = match step {
            PlannedStep::TextChunk(units) => create_text_chunk_events(source, units),
            PlannedStep::KeyTap { keycode, flags } => {
                create_key_tap_events(source, *keycode, *flags)
            }
            PlannedStep::ModsDown(specs) => {
                // The release specs come from the plan's own ModsUp
                // step directly: its shrinking masks are derived from
                // the independent modifier masks, while this step's
                // specs carry accumulated ones — re-deriving here would
                // compute wrong flags for multi-modifier combos. The
                // release is created before any press posts: if it
                // cannot exist, nothing presses and the release promise
                // cannot be broken later.
                let up_specs = plan
                    .iter()
                    .skip(index + 1)
                    .find_map(|step| match step {
                        PlannedStep::ModsUp(specs) => Some(specs.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        TargetError::ProbeUnavailable(
                            "unbalanced plan: press group without a release step".to_string(),
                        )
                    })?;
                match (
                    create_flags_changed_events(source, specs),
                    create_flags_changed_events(source, &up_specs),
                ) {
                    (Ok(down), Ok(up)) => {
                        if let Some(stopped) = cancelled_before_post(&mut posted, &mut held_release)
                        {
                            return stopped;
                        }
                        posted += post_events(pid, &down);
                        held_release = Some(up);
                        if index + 1 < plan.len() {
                            std::thread::sleep(TARGETED_STEP_PACING);
                        }
                        continue;
                    }
                    (Err(cause), _) | (_, Err(cause)) => Err(cause),
                }
            }
            PlannedStep::ModsUp(_) => {
                // The plan's own closing release: post the held pre-built
                // copy (identical to this step's specs by construction —
                // both come from release_specs on the same press group).
                match held_release.take() {
                    Some(events) => {
                        posted += post_events(pid, &events);
                        if index + 1 < plan.len() {
                            std::thread::sleep(TARGETED_STEP_PACING);
                        }
                        continue;
                    }
                    None => Err(TargetError::ProbeUnavailable(
                        "unbalanced plan: release step without a held release".to_string(),
                    )),
                }
            }
        };
        match built {
            Ok(events) => {
                if let Some(stopped) = cancelled_before_post(&mut posted, &mut held_release) {
                    return stopped;
                }
                posted += post_events(pid, &events);
            }
            Err(cause) => {
                // Nothing posted and nothing pressed: a plain error,
                // because there is no partial effect to report.
                if posted == 0 && held_release.is_none() {
                    return Err(cause);
                }
                release_held(&mut posted, &mut held_release);
                return Ok(partial(posted, cause));
            }
        }
        if index + 1 < plan.len() {
            std::thread::sleep(TARGETED_STEP_PACING);
        }
    }
    Ok(TargetedOutcome::Dispatched {
        events_posted: posted,
        events_total: total,
    })
}
