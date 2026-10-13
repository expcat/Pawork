//! Guest service for the dedicated Wayland seat (CU-26 slice 2).
//!
//! `computer_guest --wayland` serves the controlled guest frame protocol on
//! 0.0.0.0:5907 inside the container; the Host dials only the published
//! loopback port. Every operation re-checks that the seat is still the
//! dedicated one, resolves the window from compositor data, and refuses a
//! hidden or overlapped window before a frame is cut or an event is sent.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, TcpListener};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::sway::{CompositorInstance, SwayIpc, SwayRect, SwayWindow, WindowLedger};
use super::{
    InputOutcome, PointerButton, SeatError, SeatFrame, SeatKey, SeatModifiers, WaylandSeat,
    DEVICE_SETTLE,
};
use crate::guest::{
    encode_base64, serve_connection, token_from_env, CaptureQuery, GuestApp, GuestAppInfo,
    GuestCapture, GuestError, GuestMetadata, GuestTree, GuestWindow, InputQuery, InputReceipt,
    PressQuery, Pressed, RejectedWindow, SeatInput, TreeQuery, WAYLAND_BUNDLE_PREFIX,
    WAYLAND_GUEST_PORT,
};
use crate::wayland_surface::VISIBLE_STACK;

const IPC_TIMEOUT: Duration = Duration::from_secs(5);
const GET_OUTPUTS: u32 = 3;
const HIDDEN_STACK: u32 = 0;
const NOT_ISSUED: &str = "wayland window is not the issued generation";
const CHANGED: &str = "wayland window generation or geometry changed during the operation";

pub fn run() -> Result<(), String> {
    let token = token_from_env().map_err(|error| error.to_string())?;
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, WAYLAND_GUEST_PORT))
        .map_err(|error| format!("bind 0.0.0.0:{WAYLAND_GUEST_PORT}: {error}"))?;
    eprintln!("computer_guest (wayland) listening on 0.0.0.0:{WAYLAND_GUEST_PORT}");
    let mut guest = WaylandGuest::new(process_nonce());
    loop {
        let (stream, _) = listener
            .accept()
            .map_err(|error| format!("accept: {error}"))?;
        if let Err(error) = serve_connection(stream, &token, &mut guest) {
            eprintln!("computer_guest (wayland) connection closed: {error}");
        }
    }
}

fn process_nonce() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ u64::from(std::process::id()).rotate_left(32)
}

struct Session {
    seat: WaylandSeat,
    ipc: SwayIpc,
}

/// One resolved toplevel: compositor data plus the issued generation and
/// its rect in output-local pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub window: SwayWindow,
    pub start_token: u64,
    pub generation: u64,
    pub local: SwayRect,
}

pub struct WaylandGuest {
    session: Option<Session>,
    ledger: WindowLedger,
    epoch: u64,
    nonce: u64,
}

impl WaylandGuest {
    pub fn new(nonce: u64) -> Self {
        Self {
            session: None,
            ledger: WindowLedger::default(),
            epoch: 0,
            nonce,
        }
    }

    fn session(&mut self) -> Result<&mut Session, GuestError> {
        if self.session.is_none() {
            let seat = WaylandSeat::connect_from_env(DEVICE_SETTLE).map_err(unavailable)?;
            let ipc = SwayIpc::connect_from_env(IPC_TIMEOUT).map_err(unavailable)?;
            self.session = Some(Session { seat, ipc });
        }
        self.session
            .as_mut()
            .ok_or_else(|| GuestError::Unavailable("wayland session was not installed".into()))
    }

    /// Drop a session whose sockets may be broken so the next call reconnects.
    fn fail(&mut self, error: SeatError) -> GuestError {
        if !matches!(
            error,
            SeatError::Input(_) | SeatError::Cancelled | SeatError::NotDedicated(_)
        ) {
            self.session = None;
        }
        unavailable(error)
    }

    fn snapshot(&mut self) -> Result<(Vec<Resolved>, (u32, u32)), GuestError> {
        let session = self.session()?;
        let checked = session
            .ipc
            .require_dedicated()
            .and_then(|_| session.ipc.windows())
            .and_then(|windows| {
                let outputs = session.ipc.request(GET_OUTPUTS, "")?;
                Ok((windows, output_rect(&outputs)?))
            });
        let compositor = session.ipc.compositor();
        let seat_size = session.seat.output_size();
        let (windows, output) = checked.map_err(|error| self.fail(error))?;
        if (output.width, output.height) != (i64::from(seat_size.0), i64::from(seat_size.1)) {
            return Err(GuestError::Unavailable(
                "sway output size does not match the screencopy output".into(),
            ));
        }
        let resolved = self
            .ledger
            .sync(compositor, &windows)
            .into_iter()
            .map(|(window, instance, ledger)| Resolved {
                local: SwayRect {
                    x: window.rect.x - output.x,
                    y: window.rect.y - output.y,
                    width: window.rect.width,
                    height: window.rect.height,
                },
                generation: issue_generation(self.nonce, self.epoch, compositor, ledger),
                start_token: instance.start_token,
                window,
            })
            .collect();
        Ok((resolved, seat_size))
    }

    fn resolve(
        &mut self,
        window_id: u64,
        generation: u64,
    ) -> Result<(Vec<Resolved>, Resolved, (u32, u32)), GuestError> {
        let (all, size) = self.snapshot()?;
        let target = all
            .iter()
            .find(|item| item.window.con_id == window_id && item.generation == generation)
            .cloned()
            .ok_or_else(|| GuestError::Unavailable(NOT_ISSUED.into()))?;
        if let Some(reason) = unsafe_target(&all, &target, size) {
            return Err(GuestError::Unavailable(reason.into()));
        }
        Ok((all, target, size))
    }
}

fn unavailable(error: SeatError) -> GuestError {
    GuestError::Unavailable(error.to_string())
}

/// The single output's layout rect. `require_dedicated` already proved
/// there is exactly one headless output.
pub(crate) fn output_rect(outputs: &Value) -> Result<SwayRect, SeatError> {
    let active: Vec<&Value> = outputs
        .as_array()
        .ok_or_else(|| SeatError::Ipc("get_outputs is not a list".into()))?
        .iter()
        .filter(|output| output.get("active").and_then(Value::as_bool) != Some(false))
        .collect();
    let [output] = active.as_slice() else {
        return Err(SeatError::NotDedicated(format!(
            "{} active outputs",
            active.len()
        )));
    };
    let rect = output
        .get("rect")
        .ok_or_else(|| SeatError::Ipc("output without a rect".into()))?;
    let coord = |name: &str| {
        rect.get(name)
            .and_then(Value::as_i64)
            .ok_or_else(|| SeatError::Ipc(format!("output rect without {name}")))
    };
    Ok(SwayRect {
        x: coord("x")?,
        y: coord("y")?,
        width: coord("width")?,
        height: coord("height")?,
    })
}

/// Generation the Host sees: the ledger's per-con generation mixed with the
/// compositor instance, the connection epoch and a process nonce, so a new
/// compositor, a reconnect or a guest restart can never reissue an old value.
pub(crate) fn issue_generation(
    nonce: u64,
    epoch: u64,
    compositor: CompositorInstance,
    ledger: u64,
) -> u64 {
    let mixed = nonce
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(epoch.wrapping_mul(0x6C62_272E_07BB_0142))
        .wrapping_add(u64::from(compositor.pid).wrapping_mul(0xD6E8_FEB8_6659_FD93))
        .wrapping_add(compositor.start_token.rotate_left(17))
        .wrapping_add(ledger);
    if mixed == 0 {
        1
    } else {
        mixed
    }
}

fn overlaps(left: &SwayRect, right: &SwayRect) -> bool {
    left.x < right.x + right.width
        && right.x < left.x + left.width
        && left.y < right.y + right.height
        && right.y < left.y + left.height
}

/// Why the target cannot be cut from the output or receive input, if it
/// cannot: hidden, partly off the output, or overlapping another visible
/// window (tiled Sway windows never overlap, so overlap means an order this
/// guest has not proven).
pub(crate) fn unsafe_target(
    all: &[Resolved],
    target: &Resolved,
    output: (u32, u32),
) -> Option<&'static str> {
    if !target.window.visible {
        return Some("wayland window is not visible on the output");
    }
    let local = target.local;
    if local.width <= 0
        || local.height <= 0
        || local.x < 0
        || local.y < 0
        || local.x + local.width > i64::from(output.0)
        || local.y + local.height > i64::from(output.1)
    {
        return Some("wayland window extends beyond the output");
    }
    let covered = all.iter().any(|other| {
        other.window.con_id != target.window.con_id
            && other.window.visible
            && overlaps(&other.local, &local)
    });
    covered.then_some("wayland window overlaps another window")
}

/// Cut one window rect out of a whole-output frame.
pub(crate) fn crop(frame: &SeatFrame, rect: SwayRect) -> Result<SeatFrame, GuestError> {
    let inside = rect.x >= 0
        && rect.y >= 0
        && rect.width > 0
        && rect.height > 0
        && rect.x + rect.width <= i64::from(frame.width)
        && rect.y + rect.height <= i64::from(frame.height);
    if !inside || frame.rgb.len() != frame.width as usize * frame.height as usize * 3 {
        return Err(GuestError::Unavailable(
            "wayland window extends beyond the captured frame".into(),
        ));
    }
    let (x, y, width, height) = (
        rect.x as usize,
        rect.y as usize,
        rect.width as usize,
        rect.height as usize,
    );
    let stride = frame.width as usize * 3;
    let mut rgb = Vec::with_capacity(width * height * 3);
    for row in y..y + height {
        let start = row * stride + x * 3;
        rgb.extend_from_slice(&frame.rgb[start..start + width * 3]);
    }
    Ok(SeatFrame {
        width: width as u32,
        height: height as u32,
        rgb,
    })
}

fn bundle_id(app_id: &str) -> Option<String> {
    let valid = !app_id.is_empty()
        && app_id.len() <= 128
        && app_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'));
    valid.then(|| format!("{WAYLAND_BUNDLE_PREFIX}{}", app_id.to_ascii_lowercase()))
}

pub(crate) fn metadata_from(all: &[Resolved], epoch: u64) -> GuestMetadata {
    let mut apps: BTreeMap<(u32, u64, String), GuestAppInfo> = BTreeMap::new();
    let mut rejected = Vec::new();
    for item in all {
        let window = &item.window;
        let geometry = (
            i32::try_from(item.local.x),
            i32::try_from(item.local.y),
            u32::try_from(item.local.width),
            u32::try_from(item.local.height),
        );
        let (Ok(x), Ok(y), Ok(width), Ok(height)) = geometry else {
            rejected.push(RejectedWindow {
                window_id: window.con_id,
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                class: window.app_id.clone(),
                instance: String::new(),
                stack: u32::MAX,
                parent: 0,
                geometry_known: false,
                reason: "wayland window geometry is out of range".into(),
            });
            continue;
        };
        let stack = if window.visible {
            VISIBLE_STACK
        } else {
            HIDDEN_STACK
        };
        let Some(bundle) = bundle_id(&window.app_id) else {
            rejected.push(RejectedWindow {
                window_id: window.con_id,
                x,
                y,
                width,
                height,
                class: String::new(),
                instance: String::new(),
                stack,
                parent: 0,
                geometry_known: true,
                reason: "wayland app_id is not a plain identifier".into(),
            });
            continue;
        };
        apps.entry((window.pid, item.start_token, bundle.clone()))
            .or_insert_with(|| GuestAppInfo {
                bundle_id: bundle,
                name: window.app_id.clone(),
                pid: window.pid,
                start_token: item.start_token,
                windows: Vec::new(),
            })
            .windows
            .push(GuestWindow {
                window_id: window.con_id,
                generation: item.generation,
                x,
                y,
                width,
                height,
                class: window.app_id.clone(),
                instance: String::new(),
                stack,
                parent: 0,
            });
    }
    GuestMetadata {
        apps: apps.into_values().collect(),
        rejected,
        epoch,
    }
}

/// What the seat will be asked to do, in output-local pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    Click { x: u32, y: u32 },
    Text(String),
    Key(SeatKey, SeatModifiers),
}

impl Plan {
    fn steps(&self) -> u32 {
        match self {
            Self::Click { .. } => 2,
            Self::Text(text) => text.chars().count() as u32,
            Self::Key(..) => 1,
        }
    }
}

/// Keys go to the compositor's keyboard focus, so text and key presses
/// need the target to hold it; the guest never moves focus itself.
pub(crate) fn plan(target: &Resolved, input: &SeatInput) -> Result<Plan, GuestError> {
    match input {
        SeatInput::Click { x, y } => {
            if i64::from(*x) >= target.local.width || i64::from(*y) >= target.local.height {
                return Err(GuestError::Unavailable(
                    "click point is outside the wayland window".into(),
                ));
            }
            Ok(Plan::Click {
                x: (target.local.x + i64::from(*x)) as u32,
                y: (target.local.y + i64::from(*y)) as u32,
            })
        }
        SeatInput::Text { .. } | SeatInput::Key { .. } if !target.window.focused => Err(
            GuestError::Unavailable("target wayland window is not focused".into()),
        ),
        SeatInput::Text { text } => Ok(Plan::Text(text.clone())),
        SeatInput::Key {
            key,
            shift,
            control,
            alt,
        } => {
            let key = SeatKey::from_name(key)
                .ok_or_else(|| GuestError::Unavailable("unknown seat key name".into()))?;
            Ok(Plan::Key(
                key,
                SeatModifiers {
                    shift: *shift,
                    control: *control,
                    alt: *alt,
                    super_key: false,
                },
            ))
        }
    }
}

impl GuestApp for WaylandGuest {
    fn begin_session(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
    }

    fn metadata(&mut self) -> Result<GuestMetadata, GuestError> {
        let (all, _) = self.snapshot()?;
        Ok(metadata_from(&all, self.epoch))
    }

    fn tree(&mut self, _query: &TreeQuery) -> Result<GuestTree, GuestError> {
        Err(GuestError::Unavailable(
            "the wayland guest exposes no accessibility tree".into(),
        ))
    }

    fn press(
        &mut self,
        _query: &PressQuery,
        _stop: &dyn Fn() -> bool,
    ) -> Result<Pressed, GuestError> {
        Err(GuestError::Unavailable(
            "the wayland guest exposes no accessibility actions".into(),
        ))
    }

    fn capture(
        &mut self,
        query: &CaptureQuery,
        stop: &dyn Fn() -> bool,
    ) -> Result<GuestCapture, GuestError> {
        let (_, before, _) = self.resolve(query.window_id, query.generation)?;
        if stop() {
            return Err(GuestError::Unavailable(
                "cancelled before the guest capture".into(),
            ));
        }
        let frame = self.session()?.seat.capture();
        let frame = frame.map_err(|error| self.fail(error))?;
        let (_, after, _) = self
            .resolve(query.window_id, query.generation)
            .map_err(|_| GuestError::Unavailable(CHANGED.into()))?;
        if after.local != before.local {
            return Err(GuestError::Unavailable(CHANGED.into()));
        }
        let window = crop(&frame, after.local)?;
        let (jpeg, image_width, image_height) = window.jpeg().map_err(unavailable)?;
        Ok(GuestCapture {
            window_id: query.window_id,
            generation: query.generation,
            x: after.local.x as i32,
            y: after.local.y as i32,
            width: after.local.width as u32,
            height: after.local.height as u32,
            image_width,
            image_height,
            jpeg: encode_base64(&jpeg),
        })
    }

    fn input(
        &mut self,
        query: &InputQuery,
        stop: &dyn Fn() -> bool,
    ) -> Result<InputReceipt, GuestError> {
        let (_, target, _) = self.resolve(query.window_id, query.generation)?;
        let plan = plan(&target, &query.input)?;
        if stop() {
            return Err(GuestError::Unavailable(
                "cancelled before the guest action".into(),
            ));
        }
        let total = plan.steps();
        let seat = &mut self.session()?.seat;
        let sent = match &plan {
            Plan::Click { x, y } => seat.click(*x, *y, PointerButton::Left, stop),
            Plan::Text(text) => seat.type_text(text, stop),
            Plan::Key(key, modifiers) => seat.key(*key, *modifiers, stop),
        };
        let outcome = match sent {
            Ok(outcome) => outcome,
            Err(SeatError::Cancelled) => {
                return Err(GuestError::Unavailable(
                    "cancelled before the guest action".into(),
                ))
            }
            Err(SeatError::Input(message)) => {
                return Err(GuestError::Unavailable(format!(
                    "seat input refused: {message}"
                )))
            }
            Err(error) => {
                let error = self.fail(error);
                return Err(GuestError::Unavailable(format!(
                    "action receipt lost: {error}"
                )));
            }
        };
        let (steps_sent, complete) = match outcome {
            InputOutcome::Dispatched => (total, true),
            InputOutcome::Partial { steps_sent } => (steps_sent as u32, false),
        };
        Ok(InputReceipt {
            window_id: query.window_id,
            generation: query.generation,
            steps_sent,
            steps_total: total,
            complete,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wayland_surface::unpainted;

    fn resolved(con_id: u64, rect: SwayRect, visible: bool, focused: bool) -> Resolved {
        Resolved {
            window: SwayWindow {
                con_id,
                pid: 40,
                app_id: "cu26target".into(),
                rect,
                visible,
                focused,
            },
            start_token: 7,
            generation: 1000 + con_id,
            local: rect,
        }
    }

    fn rect(x: i64, y: i64, width: i64, height: i64) -> SwayRect {
        SwayRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn crop_cuts_exactly_the_window_and_refuses_anything_off_frame() {
        let width = 4u32;
        let height = 3u32;
        let rgb: Vec<u8> = (0..width * height)
            .flat_map(|index| [index as u8, 0, 0])
            .collect();
        let frame = SeatFrame { width, height, rgb };
        let cut = crop(&frame, rect(1, 1, 2, 2)).unwrap();
        assert_eq!((cut.width, cut.height), (2, 2));
        let reds: Vec<u8> = cut.rgb.chunks(3).map(|pixel| pixel[0]).collect();
        assert_eq!(reds, vec![5, 6, 9, 10]);
        for bad in [
            rect(3, 0, 2, 1),
            rect(-1, 0, 1, 1),
            rect(0, 0, 0, 1),
            rect(0, 2, 1, 2),
        ] {
            assert!(crop(&frame, bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn hidden_overlapped_or_off_output_targets_are_refused() {
        let target = resolved(1, rect(0, 0, 320, 240), true, true);
        let beside = resolved(2, rect(320, 0, 320, 240), true, false);
        assert_eq!(
            unsafe_target(&[target.clone(), beside.clone()], &target, (640, 480)),
            None
        );
        let floating = resolved(3, rect(300, 200, 100, 100), true, false);
        assert_eq!(
            unsafe_target(&[target.clone(), floating], &target, (640, 480)),
            Some("wayland window overlaps another window")
        );
        let hidden_other = resolved(4, rect(0, 0, 100, 100), false, false);
        assert_eq!(
            unsafe_target(&[target.clone(), hidden_other], &target, (640, 480)),
            None
        );
        let hidden = resolved(5, rect(0, 0, 320, 240), false, false);
        assert!(unsafe_target(&[hidden.clone()], &hidden, (640, 480)).is_some());
        let off = resolved(6, rect(500, 0, 320, 240), true, true);
        assert_eq!(
            unsafe_target(&[off.clone()], &off, (640, 480)),
            Some("wayland window extends beyond the output")
        );

        let metadata = metadata_from(
            &[
                target.clone(),
                beside,
                resolved(7, rect(10, 10, 5, 5), true, false),
            ],
            1,
        );
        let window = &metadata.apps[0].windows[0];
        assert_eq!(
            unpainted(&metadata, window),
            Some("wayland window overlaps another window"),
            "the host repeats the overlap check on guest metadata"
        );
    }

    #[test]
    fn plans_offset_clicks_and_require_focus_for_keys() {
        let mut target = resolved(1, rect(100, 50, 200, 100), true, false);
        assert_eq!(
            plan(&target, &SeatInput::Click { x: 10, y: 20 }).unwrap(),
            Plan::Click { x: 110, y: 70 }
        );
        assert!(plan(&target, &SeatInput::Click { x: 200, y: 0 }).is_err());
        let text = SeatInput::Text { text: "hi".into() };
        let refused = plan(&target, &text).unwrap_err();
        assert!(
            matches!(refused, GuestError::Unavailable(message) if message.contains("not focused"))
        );
        target.window.focused = true;
        assert_eq!(plan(&target, &text).unwrap(), Plan::Text("hi".into()));
        let key = SeatInput::Key {
            key: "return".into(),
            shift: true,
            control: false,
            alt: true,
        };
        assert_eq!(
            plan(&target, &key).unwrap(),
            Plan::Key(
                SeatKey::Return,
                SeatModifiers {
                    shift: true,
                    control: false,
                    alt: true,
                    super_key: false
                }
            )
        );
        let unknown = SeatInput::Key {
            key: "f13".into(),
            shift: false,
            control: false,
            alt: false,
        };
        assert!(plan(&target, &unknown).is_err());
    }

    #[test]
    fn generations_change_with_epoch_compositor_and_ledger() {
        let compositor = CompositorInstance {
            pid: 10,
            start_token: 99,
        };
        let base = issue_generation(5, 1, compositor, 3);
        assert_eq!(issue_generation(5, 1, compositor, 3), base);
        assert_ne!(issue_generation(5, 2, compositor, 3), base);
        assert_ne!(issue_generation(5, 1, compositor, 4), base);
        assert_ne!(issue_generation(6, 1, compositor, 3), base);
        let restarted = CompositorInstance {
            pid: 10,
            start_token: 100,
        };
        assert_ne!(issue_generation(5, 1, restarted, 3), base);
    }

    #[test]
    fn output_rect_and_app_ids_are_parsed_strictly() {
        let outputs = serde_json::json!([
            {"name":"HEADLESS-1","active":true,"rect":{"x":0,"y":0,"width":640,"height":480}}
        ]);
        assert_eq!(output_rect(&outputs).unwrap(), rect(0, 0, 640, 480));
        let two = serde_json::json!([
            {"active":true,"rect":{"x":0,"y":0,"width":640,"height":480}},
            {"active":true,"rect":{"x":640,"y":0,"width":640,"height":480}}
        ]);
        assert!(output_rect(&two).is_err());

        let mut odd = resolved(1, rect(0, 0, 10, 10), true, false);
        odd.window.app_id = "evil/../app".into();
        let metadata = metadata_from(&[odd, resolved(2, rect(20, 0, 10, 10), true, false)], 3);
        assert_eq!(metadata.rejected.len(), 1);
        assert_eq!(metadata.apps.len(), 1);
        assert_eq!(metadata.apps[0].bundle_id, "guest.wayland.cu26target");
        assert_eq!(metadata.epoch, 3);
    }
}
