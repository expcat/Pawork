//! Isolated virtual-desktop computer use. Hosts must authorize each call.
//! All coordinates refer to the returned image, with the origin at its top left.

use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

mod rfb;

pub const MAX_IMAGE_BYTES: usize = 512 * 1024;
pub const MAX_IMAGE_EDGE: u32 = 1280;
static DESKTOP_LOCK: Mutex<()> = Mutex::new(());
static OBSERVATION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static DESKTOP_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("isolated desktop does not support this operation")]
    Unsupported,
    #[error("permission required: {0}")]
    Permission(&'static str),
    #[error("invalid computer action: {0}")]
    Invalid(&'static str),
    #[error("observation expired or desktop changed; take a new screenshot")]
    Stale,
    #[error("computer action cancelled")]
    Cancelled,
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

pub struct Computer {
    backend: Arc<dyn Backend>,
    latest: Mutex<Option<Lease>>,
}

impl Computer {
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            backend,
            latest: Mutex::new(None),
        }
    }

    /// Connect lazily to the dedicated desktop shipped in `desktop/compose.yaml`.
    /// No host display, input device, clipboard, or fallback backend is accessed.
    pub fn isolated() -> Self {
        Self::new(Arc::new(rfb::Rfb::new()))
    }

    /// `scope` identifies the host run. An observation authorizes at most one input
    /// in that run; it is invalidated before any virtual input, including failures.
    pub fn execute(
        &self,
        scope: &str,
        action: Action,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Output, Error> {
        let _global = DESKTOP_LOCK
            .lock()
            .map_err(|_| Error::Backend("desktop lock poisoned".into()))?;
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let permissions = self.backend.permissions()?;
        if matches!(action, Action::Status {}) {
            return Ok(Output {
                observation: None,
                permissions: Some(permissions),
                jpeg: None,
            });
        }
        let mut latest = self
            .latest
            .lock()
            .map_err(|_| Error::Backend("observation lock poisoned".into()))?;
        if !permissions.capture {
            return Err(Error::Permission("isolated desktop capture"));
        }
        if matches!(action, Action::Screenshot {}) {
            *latest = None;
            let generation = DESKTOP_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
            let capture = self.backend.capture()?;
            if cancelled() {
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
            if self.backend.desktop()? != capture.desktop {
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
            *latest = Some(Lease {
                generation,
                scope: scope.into(),
                observation: observation.clone(),
                created: Instant::now(),
            });
            return Ok(Output {
                observation: Some(observation),
                permissions: None,
                jpeg: Some(capture.jpeg),
            });
        }
        if !permissions.input {
            return Err(Error::Permission("isolated desktop input"));
        }
        let lease = latest.take().ok_or(Error::Stale)?;
        if lease.generation != DESKTOP_GENERATION.load(Ordering::Relaxed)
            || lease.scope != scope
            || lease.created.elapsed() > Duration::from_secs(60)
            || self.backend.desktop()? != lease.observation.desktop
        {
            return Err(Error::Stale);
        }
        let input = action.into_input(&lease.observation)?;
        DESKTOP_GENERATION.fetch_add(1, Ordering::Relaxed);
        if cancelled() {
            return Err(Error::Cancelled);
        }
        self.backend.input(input, cancelled)?;
        Ok(Output {
            observation: None,
            permissions: None,
            jpeg: None,
        })
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
    static TEST_DESKTOP: Mutex<()> = Mutex::new(());

    struct Fake {
        changed: AtomicBool,
        denied: AtomicBool,
        positions: Mutex<Vec<Point>>,
    }
    impl Fake {
        fn new() -> Self {
            Self {
                changed: AtomicBool::new(false),
                denied: AtomicBool::new(false),
                positions: Mutex::new(vec![]),
            }
        }
    }
    impl Backend for Fake {
        fn permissions(&self) -> Result<Permissions, Error> {
            Ok(Permissions {
                capture: !self.denied.load(Ordering::Relaxed),
                input: true,
            })
        }
        fn desktop(&self) -> Result<Desktop, Error> {
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
            Ok(Capture {
                desktop: self.desktop()?,
                width: 1280,
                height: 720,
                jpeg: vec![0xff, 0xd8, 0xff, 0xd9],
            })
        }
        fn input(&self, input: Input, _: &dyn Fn() -> bool) -> Result<(), Error> {
            if let Input::Click { at, .. } = input {
                self.positions.lock().unwrap().push(at);
            }
            Ok(())
        }
    }
    fn observe(c: &Computer) -> String {
        c.execute("run-1", Action::Screenshot {}, &|| false)
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
    fn observed_pixels_map_to_virtual_desktop_and_each_input_consumes_its_observation() {
        let _test = TEST_DESKTOP.lock().unwrap();
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
            Err(Error::Stale)
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
        let _test = TEST_DESKTOP.lock().unwrap();
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
        c.latest.lock().unwrap().as_mut().unwrap().created =
            Instant::now() - Duration::from_secs(61);
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
}
