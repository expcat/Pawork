use super::*;
use crate::approval::{GrantKind, TargetIdentity};
use crate::guest::{
    encode_base64, serve_connection, GuestApp, GuestAppInfo, GuestCapture, GuestTree, InputReceipt,
    PressQuery, Pressed, TreeQuery,
};
use crate::target::{KeyModifiers, Scope, TargetedKeyPress};
use crate::NativeRequest;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

const TOKEN: &str = "0123456789abcdef";
const BUNDLE: &str = "guest.wayland.cu26target";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Receipt {
    Complete,
    Partial,
    Lost,
    NotFocused,
}

#[derive(Clone)]
struct State {
    metadata: Arc<AtomicUsize>,
    capture: Arc<AtomicUsize>,
    input: Arc<AtomicUsize>,
    epoch: Arc<AtomicU64>,
    overlap: Arc<AtomicBool>,
    shift_capture: Arc<AtomicBool>,
    receipt: Arc<Mutex<Receipt>>,
    sent: Arc<Mutex<Vec<SeatInput>>>,
}

impl State {
    fn new() -> Self {
        Self {
            metadata: Arc::new(AtomicUsize::new(0)),
            capture: Arc::new(AtomicUsize::new(0)),
            input: Arc::new(AtomicUsize::new(0)),
            epoch: Arc::new(AtomicU64::new(0)),
            overlap: Arc::new(AtomicBool::new(false)),
            shift_capture: Arc::new(AtomicBool::new(false)),
            receipt: Arc::new(Mutex::new(Receipt::Complete)),
            sent: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn generation(&self) -> u64 {
        1000 + self.epoch.load(Ordering::SeqCst)
    }

    fn window(&self) -> GuestWindow {
        GuestWindow {
            window_id: 7,
            generation: self.generation(),
            x: 10,
            y: 20,
            width: 200,
            height: 100,
            class: "cu26target".into(),
            instance: String::new(),
            stack: VISIBLE_STACK,
            parent: 0,
        }
    }

    fn inputs(&self) -> usize {
        self.input.load(Ordering::SeqCst)
    }

    fn captures(&self) -> usize {
        self.capture.load(Ordering::SeqCst)
    }
}

/// Half-scale JPEG of the 200x100 window, so image points map ×2.
fn jpeg() -> Vec<u8> {
    let image = image::RgbImage::from_pixel(100, 50, image::Rgb([30, 60, 90]));
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 75)
        .encode_image(&image)
        .unwrap();
    out
}

struct Fake {
    state: State,
}

impl GuestApp for Fake {
    fn begin_session(&mut self) {
        self.state.epoch.fetch_add(1, Ordering::SeqCst);
    }

    fn metadata(&mut self) -> Result<GuestMetadata, GuestError> {
        self.state.metadata.fetch_add(1, Ordering::SeqCst);
        let mut windows = vec![self.state.window()];
        if self.state.overlap.load(Ordering::SeqCst) {
            windows.push(GuestWindow {
                window_id: 8,
                x: 50,
                y: 50,
                width: 40,
                height: 40,
                ..self.state.window()
            });
        }
        Ok(GuestMetadata {
            apps: vec![GuestAppInfo {
                bundle_id: BUNDLE.into(),
                name: "cu26target".into(),
                pid: 40,
                start_token: 77,
                windows,
            }],
            rejected: Vec::new(),
            epoch: self.state.epoch.load(Ordering::SeqCst),
        })
    }

    fn tree(&mut self, _: &TreeQuery) -> Result<GuestTree, GuestError> {
        Err(GuestError::Unavailable("no tree".into()))
    }

    fn press(&mut self, _: &PressQuery, _: &dyn Fn() -> bool) -> Result<Pressed, GuestError> {
        Err(GuestError::Unavailable("no press".into()))
    }

    fn capture(
        &mut self,
        query: &CaptureQuery,
        _: &dyn Fn() -> bool,
    ) -> Result<GuestCapture, GuestError> {
        self.state.capture.fetch_add(1, Ordering::SeqCst);
        let window = self.state.window();
        if query.window_id != window.window_id || query.generation != window.generation {
            return Err(GuestError::Unavailable(
                "wayland window is not the issued generation".into(),
            ));
        }
        let shift = i32::from(self.state.shift_capture.load(Ordering::SeqCst));
        Ok(GuestCapture {
            window_id: window.window_id,
            generation: window.generation,
            x: window.x + shift,
            y: window.y,
            width: window.width,
            height: window.height,
            image_width: 100,
            image_height: 50,
            jpeg: encode_base64(&jpeg()),
        })
    }

    fn input(
        &mut self,
        query: &InputQuery,
        _: &dyn Fn() -> bool,
    ) -> Result<InputReceipt, GuestError> {
        self.state.input.fetch_add(1, Ordering::SeqCst);
        self.state.sent.lock().unwrap().push(query.input.clone());
        let receipt = |steps_sent, complete| InputReceipt {
            window_id: query.window_id,
            generation: query.generation,
            steps_sent,
            steps_total: 2,
            complete,
        };
        match *self.state.receipt.lock().unwrap() {
            Receipt::Complete => Ok(receipt(2, true)),
            Receipt::Partial => Ok(receipt(1, false)),
            Receipt::Lost => Err(GuestError::Unavailable(
                "action receipt lost: Wayland I/O failed".into(),
            )),
            Receipt::NotFocused => Err(GuestError::Unavailable(
                "target wayland window is not focused".into(),
            )),
        }
    }
}

struct Double {
    surface: WaylandSurface,
    state: State,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl Double {
    fn start() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let state = State::new();
        let stop = Arc::new(AtomicBool::new(false));
        let server_state = state.clone();
        let server_stop = Arc::clone(&stop);
        let server = thread::spawn(move || {
            while !server_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let mut fake = Fake {
                            state: server_state.clone(),
                        };
                        let _ = serve_connection(stream, TOKEN, &mut fake);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        let surface = WaylandSurface::test_dial(move || {
            let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
                .map_err(|_| GuestError::Disconnected("test guest dial failed"))?;
            GuestLink::handshake(stream, TOKEN)
        });
        Self {
            surface,
            state,
            stop,
            server: Some(server),
        }
    }

    fn host(&self) -> NativeHost<WaylandSurface> {
        NativeHost::new(self.surface.clone())
    }
}

impl Drop for Double {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.surface.disconnect();
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn scope() -> Scope {
    Scope::new("workspace", "run")
}

fn target() -> TargetIdentity {
    TargetIdentity::application(AppIdentity {
        bundle_id: BUNDLE.into(),
        family: AppFamily::Browser,
    })
}

fn bind(host: &NativeHost<WaylandSurface>, scope: &Scope) -> String {
    host.execute(
        scope,
        NativeRequest::Bind {
            bundle_id: BUNDLE.into(),
        },
        &|| false,
    )
    .unwrap()
    .window_id
    .expect("bind returns a window handle")
}

fn screenshot(host: &NativeHost<WaylandSurface>, scope: &Scope, window: &str) -> String {
    let output = host
        .execute(
            scope,
            NativeRequest::Screenshot {
                window: window.into(),
            },
            &|| false,
        )
        .unwrap();
    assert!(output.jpeg.is_some());
    output.observation_id.expect("screenshot issues a lease")
}

fn click(observation: &str, x: f64, y: f64) -> NativeRequest {
    NativeRequest::ObservedClick {
        observation: observation.into(),
        x,
        y,
    }
}

#[test]
fn click_and_keys_go_through_approval_one_lease_and_a_fresh_observation() {
    let double = Double::start();
    let host = double.host();
    let scope = scope();
    host.open_scope(&scope);
    let denied = host
        .execute(
            &scope,
            NativeRequest::Bind {
                bundle_id: BUNDLE.into(),
            },
            &|| false,
        )
        .unwrap_err();
    assert_eq!(denied, TargetError::NotAuthorized);
    assert_eq!((double.state.captures(), double.state.inputs()), (0, 0));

    host.grant(&scope, &target(), GrantKind::ForRun).unwrap();
    let window = bind(&host, &scope);
    let first = screenshot(&host, &scope, &window);
    assert_eq!(double.state.captures(), 1);

    let output = host
        .execute(&scope, click(&first, 25.0, 15.0), &|| false)
        .unwrap();
    assert!(output.input_dispatched);
    assert_eq!(output.outcome.as_deref(), Some("dispatched"));
    assert!(
        output.jpeg.is_some(),
        "the click returns its fresh observation"
    );
    let fresh = output.observation_id.expect("fresh lease");
    assert_ne!(fresh, first);
    assert_eq!(double.state.captures(), 2);
    assert_eq!(
        double.state.sent.lock().unwrap().as_slice(),
        &[SeatInput::Click { x: 50, y: 30 }],
        "image point (25,15) on a half-scale image is window pixel (50,30)"
    );

    let reused = host
        .execute(&scope, click(&first, 25.0, 15.0), &|| false)
        .unwrap_err();
    assert_eq!(reused, TargetError::StaleHandle);
    let outside = host
        .execute(&scope, click(&fresh, 100.0, 0.0), &|| false)
        .unwrap_err();
    assert_eq!(outside, TargetError::OutOfBounds);
    let accented = host
        .execute(
            &scope,
            NativeRequest::ObservedKeys {
                observation: fresh.clone(),
                input: TargetedInput::InsertText("héllo".into()),
            },
            &|| false,
        )
        .unwrap_err();
    assert!(matches!(accented, TargetError::Invalid(_)));
    assert_eq!(double.state.inputs(), 1, "refusals send nothing");

    let keyed = host
        .execute(
            &scope,
            NativeRequest::ObservedKeys {
                observation: fresh.clone(),
                input: TargetedInput::KeyPress(TargetedKeyPress {
                    key: TargetedKey::Return,
                    modifiers: KeyModifiers {
                        shift: true,
                        control: false,
                        option: true,
                    },
                }),
            },
            &|| false,
        )
        .unwrap();
    assert_eq!(keyed.outcome.as_deref(), Some("dispatched"));
    assert_eq!(
        double.state.sent.lock().unwrap().last(),
        Some(&SeatInput::Key {
            key: "return".into(),
            shift: true,
            control: false,
            alt: true,
        }),
        "a bad point or text left the lease usable for this key"
    );
}

#[test]
fn revoked_cancelled_or_reconnected_input_never_reaches_the_seat() {
    let double = Double::start();
    let host = double.host();
    let scope = scope();
    host.open_scope(&scope);
    host.grant(&scope, &target(), GrantKind::ForRun).unwrap();
    let window = bind(&host, &scope);

    let observation = screenshot(&host, &scope, &window);
    host.revoke(&scope, &target());
    let revoked = host
        .execute(&scope, click(&observation, 1.0, 1.0), &|| false)
        .unwrap_err();
    assert_eq!(revoked, TargetError::NotAuthorized);

    host.grant(&scope, &target(), GrantKind::ForRun).unwrap();
    let observation = screenshot(&host, &scope, &window);
    let cancelled = host
        .execute(&scope, click(&observation, 1.0, 1.0), &|| true)
        .unwrap_err();
    assert_eq!(cancelled, TargetError::Cancelled);

    let observation = screenshot(&host, &scope, &window);
    double.surface.disconnect();
    let reconnected = host
        .execute(&scope, click(&observation, 1.0, 1.0), &|| false)
        .unwrap_err();
    assert!(
        matches!(
            reconnected,
            TargetError::WindowReplaced | TargetError::ProcessRestarted
        ),
        "{reconnected:?}"
    );
    assert_eq!(double.state.inputs(), 0);

    host.close_scope(&scope);
    let closed = host
        .execute(&scope, click(&observation, 1.0, 1.0), &|| false)
        .unwrap_err();
    assert_eq!(closed, TargetError::ScopeClosed);
    assert_eq!(double.state.inputs(), 0);
}

#[test]
fn overlap_moved_frames_and_lost_receipts_are_never_clean_results() {
    let double = Double::start();
    let host = double.host();
    let scope = scope();
    host.open_scope(&scope);
    host.grant(&scope, &target(), GrantKind::ForRun).unwrap();
    let window = bind(&host, &scope);

    double.state.overlap.store(true, Ordering::SeqCst);
    let covered = host
        .execute(
            &scope,
            NativeRequest::Screenshot {
                window: window.clone(),
            },
            &|| false,
        )
        .unwrap_err();
    assert!(
        matches!(&covered, TargetError::ProbeUnavailable(message) if message.contains("overlaps")),
        "{covered:?}"
    );
    assert_eq!(double.state.captures(), 0, "refused before the frame");
    double.state.overlap.store(false, Ordering::SeqCst);

    double.state.shift_capture.store(true, Ordering::SeqCst);
    let moved = host
        .execute(
            &scope,
            NativeRequest::Screenshot {
                window: window.clone(),
            },
            &|| false,
        )
        .unwrap_err();
    assert_eq!(moved, TargetError::WindowReplaced);
    double.state.shift_capture.store(false, Ordering::SeqCst);

    *double.state.receipt.lock().unwrap() = Receipt::Lost;
    let observation = screenshot(&host, &scope, &window);
    let lost = host
        .execute(&scope, click(&observation, 1.0, 1.0), &|| false)
        .unwrap();
    assert!(lost.input_dispatched);
    assert_eq!(lost.outcome.as_deref(), Some("unknown_effect"));

    *double.state.receipt.lock().unwrap() = Receipt::Partial;
    let observation = screenshot(&host, &scope, &window);
    let partial = host
        .execute(&scope, click(&observation, 1.0, 1.0), &|| false)
        .unwrap();
    assert!(partial
        .outcome
        .as_deref()
        .is_some_and(|outcome| outcome.starts_with("partial 1/2")));

    *double.state.receipt.lock().unwrap() = Receipt::NotFocused;
    let observation = screenshot(&host, &scope, &window);
    let unfocused = host
        .execute(
            &scope,
            NativeRequest::ObservedKeys {
                observation,
                input: TargetedInput::InsertText("hi".into()),
            },
            &|| false,
        )
        .unwrap_err();
    assert_eq!(unfocused, TargetError::NotFocused);
    assert_eq!(double.state.inputs(), 3);
}
