//! Isolated virtual-desktop computer use. Hosts must authorize each call.
//! All coordinates refer to the returned image, with the origin at its top left.

use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};

pub mod approval;
mod rfb;
pub mod target;

/// macOS native backend (CU-03): application/window discovery, window
/// generation tracking and activation-free launching by bundle identity.
#[cfg(target_os = "macos")]
pub mod macos;

pub const MAX_IMAGE_BYTES: usize = 512 * 1024;
pub const MAX_IMAGE_EDGE: u32 = 1280;
/// How long a run holds the desktop after its last call. Matches the
/// observation lease so ownership never outlives a usable observation: a
/// run that stopped calling loses the desktop at the same moment its
/// freshest screenshot expires.
pub const OWNERSHIP_TTL: Duration = Duration::from_secs(60);
static OBSERVATION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("permission required: {0}")]
    Permission(&'static str),
    #[error("invalid computer action: {0}")]
    Invalid(&'static str),
    #[error("observation expired or desktop changed; take a new screenshot")]
    Stale,
    #[error("computer action cancelled")]
    Cancelled,
    #[error("isolated desktop is owned by another run ({0}); it must finish or release it")]
    Conflict(String),
    #[error("computer backend failed: {0}")]
    Backend(String),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Status {},
    Screenshot {},
    Click {
        observation_id: String,
        x: f64,
        y: f64,
        button: Button,
        clicks: u8,
    },
    Move {
        observation_id: String,
        x: f64,
        y: f64,
    },
    Drag {
        observation_id: String,
        from: Point,
        to: Point,
    },
    Scroll {
        observation_id: String,
        x: f64,
        y: f64,
        delta_x: i32,
        delta_y: i32,
    },
    TypeText {
        observation_id: String,
        text: String,
    },
    Key {
        observation_id: String,
        key: String,
        #[serde(default)]
        modifiers: Vec<String>,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Desktop {
    pub session_id: u64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Permissions {
    pub capture: bool,
    pub input: bool,
}

pub struct Capture {
    pub desktop: Desktop,
    pub width: u32,
    pub height: u32,
    pub jpeg: Vec<u8>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Observation {
    pub observation_id: String,
    pub desktop: Desktop,
    pub image_width: u32,
    pub image_height: u32,
}

pub struct Output {
    pub observation: Option<Observation>,
    pub permissions: Option<Permissions>,
    pub jpeg: Option<Vec<u8>>,
    /// CU-10: true when input events were dispatched into the backend. This
    /// is a dispatch fact, not proof the UI accepted the action; the actual
    /// state is checked against the fresh observation of the same call.
    pub input_dispatched: bool,
    /// CU-10: why the post-input observation failed. The dispatch that
    /// already happened is kept as fact (input_dispatched stays true), the
    /// observation is absent and never retried within this call.
    pub observation_failure: Option<String>,
}

/// Input already validated and converted to virtual display pixels.
pub enum Input {
    Click {
        at: Point,
        button: Button,
        clicks: u8,
    },
    Move(Point),
    Drag {
        from: Point,
        to: Point,
    },
    Scroll {
        at: Point,
        delta_x: i32,
        delta_y: i32,
    },
    TypeText(String),
    Key {
        key: String,
        modifiers: Vec<String>,
    },
}

/// A synchronous seam for isolated backends and deterministic host tests.
/// Implementations must balance every key/button down with an up even on cancellation.
pub trait Backend: Send + Sync {
    fn permissions(&self) -> Result<Permissions, Error>;
    fn desktop(&self) -> Result<Desktop, Error>;
    fn capture(&self) -> Result<Capture, Error>;
    fn input(&self, input: Input, cancelled: &dyn Fn() -> bool) -> Result<(), Error>;
}

struct Lease {
    generation: u64,
    scope: String,
    observation: Observation,
    created: Instant,
}

struct DesktopState {
    /// The single run that owns this desktop. CU-09: the desktop is an
    /// exclusive shared boundary — a second run gets a visible conflict,
    /// never silent serialization or shared observations.
    owner: Option<(String, Instant)>,
    generation: u64,
    latest: Option<Lease>,
}

/// One physical desktop: whole operations are serialized by `operation`,
/// while `state` (owner, lease, generation) uses short critical sections so
/// a conflicting run is rejected immediately instead of queueing behind an
/// in-flight operation. All `Computer::isolated()` instances share one
/// session because there is exactly one desktop per deployment; hosts that
/// construct custom backends get a private session whose boundary they own.
struct DesktopSession {
    operation: Mutex<()>,
    state: Mutex<DesktopState>,
}

impl DesktopSession {
    fn new() -> Self {
        Self {
            operation: Mutex::new(()),
            state: Mutex::new(DesktopState {
                owner: None,
                generation: 0,
                latest: None,
            }),
        }
    }
}

pub struct Computer {
    backend: Arc<dyn Backend>,
    session: Arc<DesktopSession>,
}

impl Computer {
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            backend,
            session: Arc::new(DesktopSession::new()),
        }
    }

    /// Connect lazily to the dedicated desktop shipped in `desktop/compose.yaml`.
    /// No host display, input device, clipboard, or fallback backend is accessed.
    pub fn isolated() -> Self {
        static ISOLATED_SESSION: OnceLock<Arc<DesktopSession>> = OnceLock::new();
        Self {
            backend: Arc::new(rfb::Rfb::new()),
            session: ISOLATED_SESSION
                .get_or_init(|| Arc::new(DesktopSession::new()))
                .clone(),
        }
    }

    /// Host hook for run end (stop, cancel, disconnect): the owner's
    /// occupancy and its pending observation are released, and the
    /// generation bump makes every in-flight handle from that run fail as
    /// cancelled instead of re-claiming the freed desktop. Releasing a
    /// scope that does not own the desktop changes nothing.
    pub fn release(&self, scope: &str) {
        if let Ok(mut state) = self.session.state.lock() {
            abandon(&mut state, scope);
        }
    }

    /// `scope` identifies the host run. An observation authorizes at most one input
    /// in that run; it is invalidated before any virtual input, including failures.
    pub fn execute(
        &self,
        scope: &str,
        action: Action,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Output, Error> {
        // Ownership gate before any lock waiting or backend access: a second
        // run gets a visible conflict immediately, never a silent queue.
        claim(&self.session, scope)?;
        let _operation = self
            .session
            .operation
            .lock()
            .map_err(|_| Error::Backend("desktop operation lock poisoned".into()))?;
        // Re-check after waiting for an in-flight operation: another run may
        // have claimed the desktop while this call was queued.
        claim(&self.session, scope)?;
        if cancelled() {
            self.release(scope);
            return Err(Error::Cancelled);
        }
        let permissions = self.run_backend(scope, cancelled, || self.backend.permissions())?;
        if matches!(action, Action::Status {}) {
            return Ok(Output {
                observation: None,
                permissions: Some(permissions),
                jpeg: None,
                input_dispatched: false,
                observation_failure: None,
            });
        }
        if !permissions.capture {
            return Err(Error::Permission("isolated desktop capture"));
        }
        if matches!(action, Action::Screenshot {}) {
            let (observation, jpeg) = self.capture_observation(scope, cancelled, None)?;
            return Ok(Output {
                observation: Some(observation),
                permissions: None,
                jpeg: Some(jpeg),
                input_dispatched: false,
                observation_failure: None,
            });
        }
        if !permissions.input {
            return Err(Error::Permission("isolated desktop input"));
        }
        let (lease, generation) = {
            let mut state = self
                .session
                .state
                .lock()
                .map_err(|_| Error::Backend("desktop state lock poisoned".into()))?;
            let lease = state.latest.take().ok_or(Error::Stale)?;
            if lease.generation != state.generation
                || lease.scope != scope
                || lease.created.elapsed() > Duration::from_secs(60)
            {
                return Err(Error::Stale);
            }
            (lease, state.generation)
        };
        if self.run_backend(scope, cancelled, || self.backend.desktop())?
            != lease.observation.desktop
        {
            return Err(Error::Stale);
        }
        let input = action.into_input(&lease.observation)?;
        let dispatch_generation = {
            let mut state = self
                .session
                .state
                .lock()
                .map_err(|_| Error::Backend("desktop state lock poisoned".into()))?;
            verify_owned(&mut state, scope)?;
            if generation != state.generation {
                return Err(Error::Stale);
            }
            state.generation += 1;
            state.generation
        };
        if cancelled() {
            self.release(scope);
            return Err(Error::Cancelled);
        }
        // Ownership revocation is part of the backend's cancellation
        // predicate: an input that already entered the backend stops sending
        // its remaining events the moment its run is released (or the
        // desktop is taken over); the backend balances key/button state on
        // cancellation.
        let session = self.session.clone();
        let dispatch_scope = scope.to_string();
        let revoked = move || match session.state.lock() {
            Ok(state) => {
                !state
                    .owner
                    .as_ref()
                    .is_some_and(|(owner, _)| owner.as_str() == dispatch_scope)
                    || state.generation != dispatch_generation
            }
            Err(_) => true,
        };
        self.run_backend(scope, cancelled, || {
            self.backend.input(input, &|| cancelled() || revoked())
        })?;
        // CU-10: the dispatched input is a fact, not an effect proof. The
        // same call captures a fresh observation so the next decision sees
        // the actual state; when that capture fails the dispatch already
        // happened and is reported as fact with the failure — never erased
        // by an error and never retried here.
        let (observation, jpeg, observation_failure) = match self.capture_observation(
            scope,
            cancelled,
            Some(dispatch_generation),
        ) {
            Ok((observation, jpeg)) => (Some(observation), Some(jpeg), None),
            Err(error) => (None, None, Some(error.to_string())),
        };
        Ok(Output {
            observation,
            permissions: None,
            jpeg,
            input_dispatched: true,
            observation_failure,
        })
    }

    /// Capture and register one fresh observation lease for the owning
    /// scope. Shared by the screenshot action and the CU-10 post-input
    /// observation: the generation bump invalidates any pending lease, the
    /// frame must be current (desktop identity re-checked) and the lease is
    /// stored only while this run still owns the desktop at the generation
    /// the capture started from. Post-input captures anchor on
    /// `dispatch_generation`: the anchor check and the bump share one
    /// critical section, so a release (or same-scope reclaim) between the
    /// last input event and this capture cannot deliver an observation
    /// across the CU-09 invalidation — the dispatch fact survives, the
    /// observation fails instead.
    fn capture_observation(
        &self,
        scope: &str,
        cancelled: &dyn Fn() -> bool,
        dispatch_generation: Option<u64>,
    ) -> Result<(Observation, Vec<u8>), Error> {
        let generation = {
            let mut state = self
                .session
                .state
                .lock()
                .map_err(|_| Error::Backend("desktop state lock poisoned".into()))?;
            if let Some(expected) = dispatch_generation {
                verify_owned(&mut state, scope)?;
                if state.generation != expected {
                    return Err(Error::Stale);
                }
            }
            state.latest = None;
            state.generation += 1;
            state.generation
        };
        let capture = self.run_backend(scope, cancelled, || self.backend.capture())?;
        if cancelled() {
            self.release(scope);
            return Err(Error::Cancelled);
        }
        if capture.width == 0
            || capture.height == 0
            || capture.width > MAX_IMAGE_EDGE
            || capture.height > MAX_IMAGE_EDGE
            || capture.jpeg.is_empty()
            || capture.jpeg.len() > MAX_IMAGE_BYTES
        {
            return Err(Error::Backend(
                "screenshot exceeds dimensions or byte budget".into(),
            ));
        }
        if self.run_backend(scope, cancelled, || self.backend.desktop())? != capture.desktop {
            return Err(Error::Stale);
        }
        let observation = Observation {
            observation_id: format!(
                "{}-{}",
                std::process::id(),
                OBSERVATION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ),
            desktop: capture.desktop,
            image_width: capture.width,
            image_height: capture.height,
        };
        {
            let mut state = self
                .session
                .state
                .lock()
                .map_err(|_| Error::Backend("desktop state lock poisoned".into()))?;
            // Store the lease only while this run still owns the desktop:
            // a concurrent host release must not resurrect a handle.
            verify_owned(&mut state, scope)?;
            // The screenshot's generation must still be current: a
            // release that let the same scope re-claim (a queued call)
            // must not deliver an observation that is stale on arrival.
            if generation != state.generation {
                return Err(Error::Stale);
            }
            state.latest = Some(Lease {
                generation,
                scope: scope.into(),
                observation: observation.clone(),
                created: Instant::now(),
            });
        }
        Ok((observation, capture.jpeg))
    }

    /// Run one backend call of a claimed operation. When the host cancelled
    /// while the backend was failing, ownership is released so an error path
    /// cannot strand occupancy; the original error is returned unchanged.
    fn run_backend<T>(
        &self,
        scope: &str,
        cancelled: &dyn Fn() -> bool,
        call: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        match call() {
            Err(error) => {
                if matches!(error, Error::Cancelled) || cancelled() {
                    self.release(scope);
                }
                Err(error)
            }
            Ok(value) => Ok(value),
        }
    }
}

/// Claim or refresh this scope's ownership of the desktop. A different
/// live owner is a visible conflict; an owner idle past [OWNERSHIP_TTL] is
/// treated as gone (its observations are expired anyway).
fn claim_state(state: &mut DesktopState, scope: &str) -> Result<(), Error> {
    match &state.owner {
        Some((owner, active)) if owner.as_str() != scope && active.elapsed() < OWNERSHIP_TTL => {
            Err(Error::Conflict(owner.clone()))
        }
        _ => {
            state.owner = Some((scope.into(), Instant::now()));
            Ok(())
        }
    }
}

fn claim(session: &DesktopSession, scope: &str) -> Result<(), Error> {
    let mut state = session
        .state
        .lock()
        .map_err(|_| Error::Backend("desktop state lock poisoned".into()))?;
    claim_state(&mut state, scope)
}

/// Verify that `scope` still owns the desktop without ever claiming it.
/// In-flight operations must not resurrect a slot the host released: a
/// freed desktop reads as cancelled, a new owner as a visible conflict.
fn verify_owned(state: &mut DesktopState, scope: &str) -> Result<(), Error> {
    match &state.owner {
        Some((owner, _)) if owner.as_str() == scope => {
            state.owner = Some((scope.into(), Instant::now()));
            Ok(())
        }
        Some((owner, _)) => Err(Error::Conflict(owner.clone())),
        None => Err(Error::Cancelled),
    }
}

/// Release the scope's occupancy and invalidate its pending observation.
/// The generation bump keeps leases taken before the release unusable even
/// if the same scope re-claims the desktop right after.
fn abandon(state: &mut DesktopState, scope: &str) {
    if state
        .owner
        .as_ref()
        .is_some_and(|(owner, _)| owner.as_str() == scope)
    {
        state.owner = None;
        state.latest = None;
        state.generation += 1;
    }
}

impl Action {
    fn into_input(self, o: &Observation) -> Result<Input, Error> {
        let point = |x: f64, y: f64| -> Result<Point, Error> {
            if !x.is_finite()
                || !y.is_finite()
                || x < 0.0
                || y < 0.0
                || x >= o.image_width as f64
                || y >= o.image_height as f64
            {
                return Err(Error::Invalid("coordinates outside the observed image"));
            }
            Ok(Point {
                x: x * o.desktop.width / o.image_width as f64,
                y: y * o.desktop.height / o.image_height as f64,
            })
        };
        let check_id = |id: &str| {
            if id == o.observation_id {
                Ok(())
            } else {
                Err(Error::Stale)
            }
        };
        Ok(match self {
            Self::Click {
                observation_id,
                x,
                y,
                button,
                clicks,
            } => {
                check_id(&observation_id)?;
                if !(1..=2).contains(&clicks) {
                    return Err(Error::Invalid("clicks must be 1 or 2"));
                }
                Input::Click {
                    at: point(x, y)?,
                    button,
                    clicks,
                }
            }
            Self::Move {
                observation_id,
                x,
                y,
            } => {
                check_id(&observation_id)?;
                Input::Move(point(x, y)?)
            }
            Self::Drag {
                observation_id,
                from,
                to,
            } => {
                check_id(&observation_id)?;
                Input::Drag {
                    from: point(from.x, from.y)?,
                    to: point(to.x, to.y)?,
                }
            }
            Self::Scroll {
                observation_id,
                x,
                y,
                delta_x,
                delta_y,
            } => {
                check_id(&observation_id)?;
                if delta_x.unsigned_abs() > 2000
                    || delta_y.unsigned_abs() > 2000
                    || (delta_x == 0 && delta_y == 0)
                {
                    return Err(Error::Invalid(
                        "scroll delta must be nonzero and within 2000 pixels",
                    ));
                }
                Input::Scroll {
                    at: point(x, y)?,
                    delta_x,
                    delta_y,
                }
            }
            Self::TypeText {
                observation_id,
                text,
            } => {
                check_id(&observation_id)?;
                if text.is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
                    return Err(Error::Invalid("text must be 1..4096 bytes without control characters; use key for Return/Tab"));
                }
                Input::TypeText(text)
            }
            Self::Key {
                observation_id,
                key,
                modifiers,
            } => {
                check_id(&observation_id)?;
                if keysym(&key).is_none()
                    || modifiers.len() > 4
                    || modifiers
                        .iter()
                        .any(|m| !matches!(m.as_str(), "command" | "shift" | "control" | "option"))
                {
                    return Err(Error::Invalid("unsupported key or modifier"));
                }
                Input::Key { key, modifiers }
            }
            _ => return Err(Error::Invalid("expected input action")),
        })
    }
}

fn keysym(key: &str) -> Option<u32> {
    if key.len() == 1
        && key
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return Some(key.as_bytes()[0] as u32);
    }
    Some(match key {
        "return" => 0xff0d,
        "tab" => 0xff09,
        "space" => 0x20,
        "backspace" => 0xff08,
        "escape" => 0xff1b,
        "delete" => 0xffff,
        "home" => 0xff50,
        "end" => 0xff57,
        "page_up" => 0xff55,
        "page_down" => 0xff56,
        "left" => 0xff51,
        "up" => 0xff52,
        "right" => 0xff53,
        "down" => 0xff54,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    struct Fake {
        changed: AtomicBool,
        denied: AtomicBool,
        fail: AtomicBool,
        calls: std::sync::atomic::AtomicUsize,
        positions: Mutex<Vec<Point>>,
        typed: Mutex<Vec<char>>,
        hook: Mutex<Option<Box<dyn Fn(&'static str) + Send>>>,
    }
    impl Fake {
        fn new() -> Self {
            Self {
                changed: AtomicBool::new(false),
                denied: AtomicBool::new(false),
                fail: AtomicBool::new(false),
                calls: std::sync::atomic::AtomicUsize::new(0),
                positions: Mutex::new(vec![]),
                typed: Mutex::new(vec![]),
                hook: Mutex::new(None),
            }
        }
        fn fire(&self, method: &'static str) {
            if let Some(hook) = self.hook.lock().unwrap().as_ref() {
                hook(method);
            }
        }
    }
    impl Backend for Fake {
        fn permissions(&self) -> Result<Permissions, Error> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.fire("permissions");
            if self.fail.load(Ordering::Relaxed) {
                return Err(Error::Backend("injected permissions failure".into()));
            }
            Ok(Permissions {
                capture: !self.denied.load(Ordering::Relaxed),
                input: true,
            })
        }
        fn desktop(&self) -> Result<Desktop, Error> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.fire("desktop");
            if self.fail.load(Ordering::Relaxed) {
                return Err(Error::Backend("injected desktop failure".into()));
            }
            Ok(Desktop {
                width: 2560.0,
                height: 1440.0,
                session_id: if self.changed.load(Ordering::Relaxed) {
                    20
                } else {
                    10
                },
            })
        }
        fn capture(&self) -> Result<Capture, Error> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.fire("capture");
            if self.fail.load(Ordering::Relaxed) {
                return Err(Error::Backend("injected capture failure".into()));
            }
            Ok(Capture {
                desktop: Desktop {
                    width: 2560.0,
                    height: 1440.0,
                    session_id: if self.changed.load(Ordering::Relaxed) {
                        20
                    } else {
                        10
                    },
                },
                width: 1280,
                height: 720,
                jpeg: vec![0xff, 0xd8, 0xff, 0xd9],
            })
        }
        fn input(&self, input: Input, cancelled: &dyn Fn() -> bool) -> Result<(), Error> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.fire("input");
            if self.fail.load(Ordering::Relaxed) {
                return Err(Error::Backend("injected input failure".into()));
            }
            match input {
                Input::Click { at, .. } => {
                    if cancelled() {
                        return Err(Error::Cancelled);
                    }
                    self.positions.lock().unwrap().push(at);
                    self.fire("input_done");
                }
                Input::TypeText(text) => {
                    for ch in text.chars() {
                        self.typed.lock().unwrap().push(ch);
                        self.fire("input_char");
                        if cancelled() {
                            return Err(Error::Cancelled);
                        }
                    }
                }
                _ => {}
            }
            Ok(())
        }
    }
    fn observe(c: &Computer) -> String {
        observe_as(c, "run-1")
    }
    fn observe_as(c: &Computer, scope: &str) -> String {
        c.execute(scope, Action::Screenshot {}, &|| false)
            .unwrap()
            .observation
            .unwrap()
            .observation_id
    }
    fn click(id: String, x: f64) -> Action {
        Action::Click {
            observation_id: id,
            x,
            y: 360.0,
            button: Button::Left,
            clicks: 1,
        }
    }

    #[test]
    fn input_returns_its_fresh_observation_in_the_same_call() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let first = observe(&c);
        let output = c
            .execute("run-1", click(first.clone(), 640.0), &|| false)
            .unwrap();
        assert!(output.input_dispatched);
        assert!(output.observation_failure.is_none());
        let observation = output.observation.expect("post-input observation");
        assert_ne!(observation.observation_id, first);
        assert!(output.jpeg.is_some());
        // The fresh observation is the only dispatchable lease and works
        // exactly once: a consume attempt burns it even when the id is stale.
        let second = observation.observation_id.clone();
        assert!(c
            .execute("run-1", click(second.clone(), 640.0), &|| false)
            .is_ok());
        assert!(matches!(
            c.execute("run-1", click(second, 640.0), &|| false),
            Err(Error::Stale)
        ));
        assert!(matches!(
            c.execute("run-1", click(first, 640.0), &|| false),
            Err(Error::Stale)
        ));
        assert_eq!(backend.positions.lock().unwrap().len(), 2);
    }

    #[test]
    fn capture_failure_after_dispatch_keeps_the_input_fact() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let first = observe(&c);
        // Fail the capture that runs after an input has really dispatched
        // (positions non-empty); the standalone first screenshot succeeds.
        let fail = Arc::downgrade(&backend);
        *backend.hook.lock().unwrap() = Some(Box::new(move |method| {
            if method == "capture" {
                let Some(backend) = fail.upgrade() else {
                    return;
                };
                if !backend.positions.lock().unwrap().is_empty() {
                    backend.fail.store(true, Ordering::Relaxed);
                }
            }
        }));
        let output = c.execute("run-1", click(first, 640.0), &|| false).unwrap();
        // The input happened and the result keeps that fact; the failed
        // observation is recorded, not retried inside the same call.
        assert!(output.input_dispatched);
        assert_eq!(backend.positions.lock().unwrap().len(), 1);
        assert!(output.observation.is_none());
        assert!(output.jpeg.is_none());
        assert!(output
            .observation_failure
            .expect("capture failure recorded")
            .contains("injected"));
        // observe(3) + permissions/desktop/input(3) + one failed capture: no
        // silent second attempt.
        assert_eq!(backend.calls.load(Ordering::Relaxed), 7);
    }

    #[test]
    fn release_between_dispatch_and_observation_never_mints_a_lease() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let id = observe(&c);
        // The last input event has landed; before the post-input capture
        // starts, the host releases the run and a queued call from the same
        // scope re-claims the desktop (the CU-09 interleaving). The old
        // call must not adopt the new generation and mint a lease across
        // the release.
        let session = c.session.clone();
        *backend.hook.lock().unwrap() = Some(Box::new(move |method| {
            if method == "input_done" {
                let mut state = session.state.lock().unwrap();
                abandon(&mut state, "run-1");
                claim_state(&mut state, "run-1").unwrap();
            }
        }));
        let output = c
            .execute("run-1", click(id, 640.0), &|| false)
            .unwrap();
        // The click was dispatched; the observation failed honestly and
        // no lease was stored for the superseded generation.
        assert_eq!(backend.positions.lock().unwrap().len(), 1);
        assert!(output.input_dispatched);
        assert!(output.observation.is_none());
        assert!(output.jpeg.is_none());
        assert!(output.observation_failure.is_some());
        {
            let state = c.session.state.lock().unwrap();
            assert!(state.latest.is_none());
        }
    }

    #[test]
    fn observed_pixels_map_to_virtual_desktop_and_each_input_consumes_its_observation() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let id = observe(&c);
        c.execute("run-1", click(id.clone(), 640.0), &|| false)
            .unwrap();
        assert_eq!(
            *backend.positions.lock().unwrap(),
            vec![Point {
                x: 1280.0,
                y: 720.0
            }]
        );
        assert!(matches!(
            c.execute("run-1", click(id, 640.0), &|| false),
            Err(Error::Stale)
        ));
        let id = observe(&c);
        assert!(matches!(
            c.execute("run-2", click(id, 640.0), &|| false),
            Err(Error::Conflict(_))
        ));
        let id = observe(&c);
        backend.changed.store(true, Ordering::Relaxed);
        assert!(matches!(
            c.execute("run-1", click(id, 640.0), &|| false),
            Err(Error::Stale)
        ));
        assert_eq!(backend.positions.lock().unwrap().len(), 1);
    }

    #[test]
    fn rejects_permissions_cancellation_stale_and_malformed_inputs_before_virtual_dispatch() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        backend.denied.store(true, Ordering::Relaxed);
        assert!(matches!(
            c.execute("run-1", Action::Screenshot {}, &|| false),
            Err(Error::Permission(_))
        ));
        backend.denied.store(false, Ordering::Relaxed);
        assert!(matches!(
            c.execute("run-1", Action::Screenshot {}, &|| true),
            Err(Error::Cancelled)
        ));
        for x in [-1.0, 1280.0, f64::NAN, f64::INFINITY] {
            let id = observe(&c);
            assert!(matches!(
                c.execute("run-1", click(id, x), &|| false),
                Err(Error::Invalid(_))
            ));
        }
        let id = observe(&c);
        c.session
            .state
            .lock()
            .unwrap()
            .latest
            .as_mut()
            .unwrap()
            .created = Instant::now() - Duration::from_secs(61);
        assert!(matches!(
            c.execute("run-1", click(id, 0.0), &|| false),
            Err(Error::Stale)
        ));
        for input in [
            r#"{"action":"screenshot","command":"ignored?"}"#,
            r#"{"action":"key","observation_id":"1","key":"return","unexpected":true}"#,
        ] {
            assert!(serde_json::from_str::<Action>(input).is_err());
        }
        let id = observe(&c);
        assert!(matches!(
            c.execute(
                "run-1",
                Action::TypeText {
                    observation_id: id,
                    text: "secret\n".into()
                },
                &|| false
            ),
            Err(Error::Invalid(_))
        ));
        assert!(backend.positions.lock().unwrap().is_empty());
    }

    #[test]
    fn second_run_gets_a_visible_conflict_without_touching_the_backend() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let id = observe(&c);
        let calls_before = backend.calls.load(Ordering::Relaxed);
        match c.execute("run-2", Action::Screenshot {}, &|| false) {
            Err(Error::Conflict(owner)) => assert!(owner.contains("run-1")),
            Ok(_) => panic!("expected a visible conflict, got a successful output"),
            Err(other) => panic!("expected a visible conflict, got {other}"),
        }
        // The rejected run never reaches the backend: the conflict is a
        // registry decision, not a serialized queue that touches the desktop.
        assert_eq!(backend.calls.load(Ordering::Relaxed), calls_before);
        // The owner is unaffected and its observation still dispatches.
        c.execute("run-1", click(id, 640.0), &|| false).unwrap();
        assert_eq!(backend.positions.lock().unwrap().len(), 1);
    }

    #[test]
    fn cancellation_releases_ownership_and_invalidates_the_observation() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let id = observe(&c);
        assert!(matches!(
            c.execute("run-1", click(id.clone(), 640.0), &|| true),
            Err(Error::Cancelled)
        ));
        assert!(backend.positions.lock().unwrap().is_empty());
        // Cancellation left no residual occupancy and no dispatchable lease.
        {
            let state = c.session.state.lock().unwrap();
            assert!(state.owner.is_none());
            assert!(state.latest.is_none());
        }
        // Another run takes the desktop immediately.
        observe_as(&c, "run-2");
        assert!(matches!(
            c.execute("run-1", Action::Status {}, &|| false),
            Err(Error::Conflict(_))
        ));
    }

    #[test]
    fn release_and_idle_expiry_free_the_desktop_for_the_next_run() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let _ = observe(&c);
        // A foreign scope cannot steal the desktop by releasing it.
        c.release("run-2");
        assert!(matches!(
            c.execute("run-2", Action::Screenshot {}, &|| false),
            Err(Error::Conflict(_))
        ));
        // The owner's release frees the desktop for the next run.
        c.release("run-1");
        let orphaned = observe_as(&c, "run-2");
        assert!(!orphaned.is_empty());
        // An owner idle past OWNERSHIP_TTL is treated as gone: its freshest
        // observation is expired at the same moment, so ownership never
        // outlives a usable lease.
        {
            let mut state = c.session.state.lock().unwrap();
            let (_, active) = state.owner.as_mut().unwrap();
            *active = Instant::now() - OWNERSHIP_TTL - Duration::from_secs(1);
        }
        observe_as(&c, "run-3");
        // The lapsed run's pending observation is orphaned with it.
        assert!(matches!(
            c.execute("run-2", click(orphaned, 640.0), &|| false),
            Err(Error::Conflict(_))
        ));
    }

    #[test]
    fn release_during_screenshot_prevents_reclaiming_the_desktop() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let session = c.session.clone();
        *backend.hook.lock().unwrap() = Some(Box::new(move |method| {
            if method == "capture" {
                let mut state = session.state.lock().unwrap();
                abandon(&mut state, "run-1");
            }
        }));
        assert!(matches!(
            c.execute("run-1", Action::Screenshot {}, &|| false),
            Err(Error::Cancelled)
        ));
        // The released run leaves neither ownership nor a dispatchable lease.
        {
            let state = c.session.state.lock().unwrap();
            assert!(state.owner.is_none());
            assert!(state.latest.is_none());
        }
        // The freed desktop is claimable by the next run without waiting.
        observe_as(&c, "run-2");
    }

    #[test]
    fn release_after_the_lease_is_taken_prevents_input_dispatch() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let id = observe(&c);
        let session = c.session.clone();
        *backend.hook.lock().unwrap() = Some(Box::new(move |method| {
            if method == "desktop" {
                let mut state = session.state.lock().unwrap();
                abandon(&mut state, "run-1");
            }
        }));
        assert!(matches!(
            c.execute("run-1", click(id, 640.0), &|| false),
            Err(Error::Cancelled)
        ));
        assert!(backend.positions.lock().unwrap().is_empty());
        {
            let state = c.session.state.lock().unwrap();
            assert!(state.owner.is_none());
            assert!(state.latest.is_none());
        }
    }

    #[test]
    fn backend_error_during_cancellation_releases_ownership() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let cancelled = Arc::new(AtomicBool::new(false));
        let trip = cancelled.clone();
        *backend.hook.lock().unwrap() = Some(Box::new(move |method| {
            if method == "permissions" {
                trip.store(true, Ordering::Relaxed);
            }
        }));
        backend.fail.store(true, Ordering::Relaxed);
        let flag = cancelled.clone();
        match c.execute("run-1", Action::Screenshot {}, &move || {
            flag.load(Ordering::Relaxed)
        }) {
            Err(Error::Backend(message)) => assert!(message.contains("injected")),
            Err(other) => panic!("expected the injected backend error, got {other}"),
            Ok(_) => panic!("expected the injected backend error, got a successful output"),
        }
        // The error path did not strand ownership on a cancelled run.
        {
            let state = c.session.state.lock().unwrap();
            assert!(state.owner.is_none());
            assert!(state.latest.is_none());
        }
    }

    #[test]
    fn release_during_input_stops_the_remaining_events() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let id = observe(&c);
        let session = c.session.clone();
        *backend.hook.lock().unwrap() = Some(Box::new(move |method| {
            if method == "input_char" {
                let mut state = session.state.lock().unwrap();
                abandon(&mut state, "run-1");
            }
        }));
        assert!(matches!(
            c.execute(
                "run-1",
                Action::TypeText {
                    observation_id: id,
                    text: "ab".into()
                },
                &|| false
            ),
            Err(Error::Cancelled)
        ));
        // The first character was already dispatched; the revocation stopped
        // the rest and left neither ownership nor a lease behind.
        assert_eq!(*backend.typed.lock().unwrap(), vec!['a']);
        {
            let state = c.session.state.lock().unwrap();
            assert!(state.owner.is_none());
            assert!(state.latest.is_none());
        }
    }

    #[test]
    fn screenshot_after_release_and_same_scope_reclaim_delivers_no_stale_observation() {
        let backend = Arc::new(Fake::new());
        let c = Computer::new(backend.clone());
        let session = c.session.clone();
        *backend.hook.lock().unwrap() = Some(Box::new(move |method| {
            if method == "capture" {
                let mut state = session.state.lock().unwrap();
                abandon(&mut state, "run-1");
                // A queued call from the same run re-claims the freed slot
                // while the in-flight screenshot is still capturing.
                claim_state(&mut state, "run-1").unwrap();
            }
        }));
        assert!(matches!(
            c.execute("run-1", Action::Screenshot {}, &|| false),
            Err(Error::Stale)
        ));
        // No lease was stored for the superseded generation.
        {
            let state = c.session.state.lock().unwrap();
            assert!(state.latest.is_none());
        }
    }
}
