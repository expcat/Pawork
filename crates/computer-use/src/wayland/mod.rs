//! Dedicated Wayland seat client for the controlled guest (CU-26, Linux).
//!
//! Binds only the compositor protocols CU-23 proved on Sway / wlroots:
//! `zwlr_screencopy_manager_v1` for whole-output frames,
//! `zwlr_virtual_pointer_manager_v1` and `zwp_virtual_keyboard_manager_v1`
//! for input. Their authority is socket reachability and covers the whole
//! seat — there is no per-application grant or portal consent — so the
//! seat must belong to Pawork alone: [sway::SwayIpc::require_dedicated]
//! refuses physical outputs, other seats and non-virtual input devices, and
//! [select_globals] refuses more than one seat or output. Nothing here
//! touches the host's own seat, falls back to XTEST, or goes through a
//! portal. Host approval, occupancy, cancellation and post-action
//! observation stay with the existing NativeHost chain.

pub mod guest;
mod keymap;
pub mod sway;
mod wire;

#[cfg(test)]
mod tests;

pub use keymap::{SeatKey, SeatModifiers};

use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, RgbImage};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use wire::{Connection, Event, Message, DISPLAY_ID};

const IO_BUDGET: Duration = Duration::from_secs(8);
/// A seat that just gained a virtual device announces the capability
/// asynchronously; clients bind wl_pointer / wl_keyboard only after that.
/// CU-23 measured events before this settle as lost.
pub const DEVICE_SETTLE: Duration = Duration::from_secs(1);
const MAX_EDGE: u32 = 4096;
const MAX_PIXELS: u64 = 8_000_000;
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const KEYMAP_XKB_V1: u32 = 1;

const SHM_ARGB8888: u32 = 0;
const SHM_XRGB8888: u32 = 1;
const SHM_ABGR8888: u32 = 0x3432_4241;
const SHM_XBGR8888: u32 = 0x3432_4258;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SeatError {
    #[error("no Wayland display: {0}")]
    NoDisplay(String),
    #[error("Wayland I/O failed: {0}")]
    Io(String),
    #[error("Wayland protocol violation: {0}")]
    Protocol(String),
    #[error("compositor error on object {object} code {code}: {message}")]
    Compositor {
        object: u32,
        code: u32,
        message: String,
    },
    #[error("required Wayland global missing or ambiguous: {0}")]
    Globals(String),
    #[error("seat is not dedicated to Pawork: {0}")]
    NotDedicated(String),
    #[error("screencopy failed: {0}")]
    Capture(String),
    #[error("input rejected: {0}")]
    Input(String),
    #[error("sway IPC failed: {0}")]
    Ipc(String),
    #[error("operation cancelled")]
    Cancelled,
}

/// Socket path from `WAYLAND_DISPLAY` / `XDG_RUNTIME_DIR`, libwayland's
/// rules minus the `wayland-0` default: an unset display is a refusal.
pub fn display_socket(
    display: Option<&str>,
    runtime_dir: Option<&str>,
) -> Result<PathBuf, SeatError> {
    let display = display
        .filter(|value| !value.is_empty())
        .ok_or_else(|| SeatError::NoDisplay("WAYLAND_DISPLAY is not set".into()))?;
    if display.starts_with('/') {
        return Ok(PathBuf::from(display));
    }
    let runtime = runtime_dir
        .filter(|value| value.starts_with('/'))
        .ok_or_else(|| SeatError::NoDisplay("XDG_RUNTIME_DIR is not an absolute path".into()))?;
    Ok(Path::new(runtime).join(display))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Global {
    pub(crate) name: u32,
    pub(crate) interface: String,
    pub(crate) version: u32,
}

/// The globals one dedicated seat binds, with negotiated versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SeatGlobals {
    pub(crate) shm: (u32, u32),
    pub(crate) seat: (u32, u32),
    pub(crate) output: (u32, u32),
    pub(crate) screencopy: (u32, u32),
    pub(crate) pointer: (u32, u32),
    pub(crate) keyboard: (u32, u32),
}

/// Exactly one of each required global, bound at min(advertised, known).
/// A second seat or output is refused rather than guessed between.
pub(crate) fn select_globals(globals: &[Global]) -> Result<SeatGlobals, SeatError> {
    let one = |interface: &'static str, max: u32| -> Result<(u32, u32), SeatError> {
        let mut found = globals
            .iter()
            .filter(|global| global.interface == interface);
        let first = found
            .next()
            .ok_or_else(|| SeatError::Globals(format!("{interface} absent")))?;
        if found.next().is_some() {
            return Err(SeatError::Globals(format!("more than one {interface}")));
        }
        Ok((first.name, first.version.min(max)))
    };
    Ok(SeatGlobals {
        shm: one("wl_shm", 1)?,
        seat: one("wl_seat", 1)?,
        output: one("wl_output", 2)?,
        screencopy: one("zwlr_screencopy_manager_v1", 3)?,
        pointer: one("zwlr_virtual_pointer_manager_v1", 1)?,
        keyboard: one("zwp_virtual_keyboard_manager_v1", 1)?,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}

impl PointerButton {
    fn code(self) -> u32 {
        match self {
            Self::Left => BTN_LEFT,
            Self::Right => BTN_RIGHT,
            Self::Middle => BTN_MIDDLE,
        }
    }
}

/// Whether every planned input event reached the compositor. A sync
/// round-trip after the last event is the receipt; it proves delivery to
/// the seat, not an effect in any client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOutcome {
    Dispatched,
    /// Cancelled between balanced steps: earlier steps may have taken
    /// effect and nothing is left pressed. Never a clean failure.
    Partial {
        steps_sent: usize,
    },
}

/// One whole-output frame, RGB, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatFrame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl SeatFrame {
    /// Baseline JPEG within the shared image budget (longest edge 1280,
    /// 512 KiB, quality ladder 75/50/30).
    pub fn jpeg(&self) -> Result<(Vec<u8>, u32, u32), SeatError> {
        let raw = RgbImage::from_raw(self.width, self.height, self.rgb.clone())
            .ok_or_else(|| SeatError::Capture("frame buffer size mismatch".into()))?;
        let scale = (crate::MAX_IMAGE_EDGE as f64 / self.width.max(self.height) as f64).min(1.0);
        let scaled = image::imageops::resize(
            &raw,
            (self.width as f64 * scale).round().max(1.0) as u32,
            (self.height as f64 * scale).round().max(1.0) as u32,
            FilterType::Triangle,
        );
        for quality in [75, 50, 30] {
            let mut jpeg = Vec::new();
            JpegEncoder::new_with_quality(&mut jpeg, quality)
                .encode_image(&scaled)
                .map_err(|_| SeatError::Capture("JPEG encoding failed".into()))?;
            if jpeg.len() <= crate::MAX_IMAGE_BYTES {
                return Ok((jpeg, scaled.width(), scaled.height()));
            }
        }
        Err(SeatError::Capture(
            "frame exceeds the JPEG byte budget".into(),
        ))
    }
}

/// Layout check for a screencopy buffer offer, before any memory exists.
pub(crate) fn checked_buffer(width: u32, height: u32, stride: u32) -> Result<usize, SeatError> {
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
        return Err(SeatError::Capture(format!(
            "buffer {width}x{height} outside 1..=4096"
        )));
    }
    if width as u64 * height as u64 > MAX_PIXELS {
        return Err(SeatError::Capture("buffer exceeds 8M pixels".into()));
    }
    if (stride as u64) < width as u64 * 4 || stride as u64 > MAX_EDGE as u64 * 4 * 2 {
        return Err(SeatError::Capture(format!(
            "stride {stride} invalid for width {width}"
        )));
    }
    Ok(stride as usize * height as usize)
}

/// Converts a 32-bit shm frame to RGB. Only byte orders the converter knows
/// are accepted; anything else refuses instead of guessing channels.
pub(crate) fn shm_to_rgb(
    format: u32,
    width: u32,
    height: u32,
    stride: u32,
    y_invert: bool,
    data: &[u8],
) -> Result<Vec<u8>, SeatError> {
    let (r, g, b) = match format {
        SHM_ARGB8888 | SHM_XRGB8888 => (2, 1, 0),
        SHM_ABGR8888 | SHM_XBGR8888 => (0, 1, 2),
        other => {
            return Err(SeatError::Capture(format!(
                "unsupported shm format {other:#x}"
            )))
        }
    };
    let needed = checked_buffer(width, height, stride)?;
    if data.len() < needed {
        return Err(SeatError::Capture("frame data truncated".into()));
    }
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for row in 0..height as usize {
        let source = if y_invert {
            height as usize - 1 - row
        } else {
            row
        };
        let line = &data[source * stride as usize..][..width as usize * 4];
        for pixel in line.chunks_exact(4) {
            rgb.extend_from_slice(&[pixel[r], pixel[g], pixel[b]]);
        }
    }
    Ok(rgb)
}

/// Requests in the order the plan sends them; validated in full before the
/// first event so an unsupported character costs zero events.
pub(crate) fn text_plan(text: &str) -> Result<Vec<u32>, SeatError> {
    if text.is_empty() || text.chars().count() > crate::target::MAX_TARGETED_TEXT_CHARS {
        return Err(SeatError::Input("text must be 1..=4096 characters".into()));
    }
    text.chars()
        .map(|c| {
            keymap::char_keycode(c).ok_or_else(|| {
                SeatError::Input(format!("character {c:?} is outside the seat keymap"))
            })
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct OutputState {
    width: u32,
    height: u32,
    scale: i32,
    transform: i32,
}

/// A connection to the dedicated seat with its virtual pointer and keyboard
/// created once (so the device-settle cost is paid once per connection).
pub struct WaylandSeat {
    conn: Connection,
    globals: SeatGlobals,
    shm: u32,
    output: u32,
    screencopy: u32,
    pointer: u32,
    keyboard: u32,
    output_state: OutputState,
    shm_formats: Vec<u32>,
    epoch: Instant,
}

impl WaylandSeat {
    pub fn connect_from_env(settle: Duration) -> Result<Self, SeatError> {
        let display = std::env::var("WAYLAND_DISPLAY").ok();
        let runtime = std::env::var("XDG_RUNTIME_DIR").ok();
        Self::connect(
            &display_socket(display.as_deref(), runtime.as_deref())?,
            settle,
        )
    }

    pub fn connect(socket: &Path, settle: Duration) -> Result<Self, SeatError> {
        let mut conn = Connection::open(socket, IO_BUDGET)?;
        let registry = conn.allocate()?;
        conn.send(&Message::new(DISPLAY_ID, 1).uint(registry))?;
        let mut globals = Vec::new();
        roundtrip(&mut conn, |event| {
            if event.object == registry && event.opcode == 0 {
                let mut args = event.args();
                globals.push(Global {
                    name: args.uint()?,
                    interface: args.string()?,
                    version: args.uint()?,
                });
            }
            Ok(())
        })?;
        let selected = select_globals(&globals)?;
        let mut bind = |(name, version): (u32, u32), interface: &str| -> Result<u32, SeatError> {
            let id = conn.allocate()?;
            conn.send(
                &Message::new(registry, 0)
                    .uint(name)
                    .string(interface)
                    .uint(version)
                    .uint(id),
            )?;
            Ok(id)
        };
        let shm = bind(selected.shm, "wl_shm")?;
        let seat = bind(selected.seat, "wl_seat")?;
        let output = bind(selected.output, "wl_output")?;
        let screencopy = bind(selected.screencopy, "zwlr_screencopy_manager_v1")?;
        let pointer_manager = bind(selected.pointer, "zwlr_virtual_pointer_manager_v1")?;
        let keyboard_manager = bind(selected.keyboard, "zwp_virtual_keyboard_manager_v1")?;
        let mut this = Self {
            conn,
            globals: selected,
            shm,
            output,
            screencopy,
            pointer: 0,
            keyboard: 0,
            output_state: OutputState {
                width: 0,
                height: 0,
                scale: 1,
                transform: 0,
            },
            shm_formats: Vec::new(),
            epoch: Instant::now(),
        };
        this.sync()?;
        this.require_plain_output()?;

        this.pointer = this.conn.allocate()?;
        this.conn.send(
            &Message::new(pointer_manager, 0)
                .uint(seat)
                .uint(this.pointer),
        )?;
        this.keyboard = this.conn.allocate()?;
        this.conn.send(
            &Message::new(keyboard_manager, 0)
                .uint(seat)
                .uint(this.keyboard),
        )?;
        this.upload_keymap()?;
        this.sync()?;
        std::thread::sleep(settle);
        Ok(this)
    }

    /// Output size in pixels; pointer coordinates use this space.
    pub fn output_size(&self) -> (u32, u32) {
        (self.output_state.width, self.output_state.height)
    }

    fn require_plain_output(&self) -> Result<(), SeatError> {
        let state = self.output_state;
        if state.width == 0 || state.height == 0 {
            return Err(SeatError::Globals("output reported no current mode".into()));
        }
        if state.scale != 1 || state.transform != 0 {
            return Err(SeatError::Globals(format!(
                "output scale {} transform {} — only unscaled, untransformed outputs map 1:1",
                state.scale, state.transform
            )));
        }
        Ok(())
    }

    fn upload_keymap(&mut self) -> Result<(), SeatError> {
        let mut text = keymap::keymap_text().into_bytes();
        text.push(0);
        let file = memfd("pawork-keymap", text.len())?;
        file.write_all_at(&text, 0)
            .map_err(|error| SeatError::Io(error.to_string()))?;
        self.conn.send(
            &Message::new(self.keyboard, 0)
                .uint(KEYMAP_XKB_V1)
                .fd(file.as_raw_fd())
                .uint(text.len() as u32),
        )
    }

    fn timestamp(&self) -> u32 {
        self.epoch.elapsed().as_millis() as u32
    }

    /// Sync round-trip that also tracks output and shm state.
    fn sync(&mut self) -> Result<(), SeatError> {
        let (shm, output) = (self.shm, self.output);
        let mut state = self.output_state;
        let mut formats = std::mem::take(&mut self.shm_formats);
        let result = roundtrip(&mut self.conn, |event| {
            track(event, shm, output, &mut state, &mut formats)
        });
        self.output_state = state;
        self.shm_formats = formats;
        result
    }

    /// Captures the whole output into fresh shared memory. Every frame is a
    /// new copy that completed with `ready`; `failed` or any short buffer is
    /// an error — there is no stale-frame path.
    pub fn capture(&mut self) -> Result<SeatFrame, SeatError> {
        let frame = self.conn.allocate()?;
        self.conn.send(
            &Message::new(self.screencopy, 0)
                .uint(frame)
                .int(0)
                .uint(self.output),
        )?;
        let version = self.globals.screencopy.1;
        let deadline = Instant::now() + IO_BUDGET;
        let mut offer = None;
        loop {
            let event = self.next_event(deadline)?;
            if event.object != frame {
                self.track_event(&event)?;
                continue;
            }
            match event.opcode {
                0 => {
                    let mut args = event.args();
                    let candidate = (args.uint()?, args.uint()?, args.uint()?, args.uint()?);
                    if offer.is_none() && shm_to_rgb_supported(candidate.0) {
                        offer = Some(candidate);
                    }
                    if version < 3 {
                        break;
                    }
                }
                3 => return Err(SeatError::Capture("compositor reported failed".into())),
                6 => break,
                _ => {}
            }
        }
        let (format, width, height, stride) = offer
            .ok_or_else(|| SeatError::Capture("no shm buffer format the converter knows".into()))?;
        let size = checked_buffer(width, height, stride)?;
        let file = memfd("pawork-screencopy", size)?;
        let pool = self.conn.allocate()?;
        self.conn.send(
            &Message::new(self.shm, 0)
                .uint(pool)
                .fd(file.as_raw_fd())
                .int(size as i32),
        )?;
        let buffer = self.conn.allocate()?;
        self.conn.send(
            &Message::new(pool, 0)
                .uint(buffer)
                .int(0)
                .int(width as i32)
                .int(height as i32)
                .int(stride as i32)
                .uint(format),
        )?;
        self.conn.send(&Message::new(frame, 0).uint(buffer))?;
        let mut y_invert = false;
        let outcome = loop {
            let event = self.next_event(deadline)?;
            if event.object != frame {
                self.track_event(&event)?;
                continue;
            }
            match event.opcode {
                1 => y_invert = event.args().uint()? & 1 != 0,
                2 => break Ok(()),
                3 => break Err(SeatError::Capture("compositor reported failed".into())),
                _ => {}
            }
        };
        self.conn.send(&Message::new(buffer, 0))?;
        self.conn.send(&Message::new(pool, 1))?;
        self.conn.send(&Message::new(frame, 1))?;
        outcome?;
        let mut data = vec![0u8; size];
        file.read_exact_at(&mut data, 0)
            .map_err(|error| SeatError::Capture(error.to_string()))?;
        let rgb = shm_to_rgb(format, width, height, stride, y_invert, &data)?;
        Ok(SeatFrame { width, height, rgb })
    }

    /// Absolute motion to an output pixel, then a balanced press / release.
    /// Cancellation after the motion and before the press returns
    /// [`InputOutcome::Partial`] with one step and does not send the button.
    /// Cancellation after the press still sends the release.
    ///
    /// When `PAWORK_WAYLAND_STEP_DELAY_MS` is 1..=1000, the motion is
    /// round-tripped and that delay elapses before the button decision, so a
    /// live compositor can report the move and a probe can drop the client
    /// first. Unset, the check stays immediate and no extra round-trip is added.
    pub fn click(
        &mut self,
        x: u32,
        y: u32,
        button: PointerButton,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<InputOutcome, SeatError> {
        let (width, height) = self.output_size();
        if x >= width || y >= height {
            return Err(SeatError::Input(format!(
                "point {x},{y} outside {width}x{height}"
            )));
        }
        if cancelled() {
            return Err(SeatError::Cancelled);
        }
        let pointer = self.pointer;
        let time = self.timestamp();
        self.conn.send(
            &Message::new(pointer, 1)
                .uint(time)
                .uint(x)
                .uint(y)
                .uint(width)
                .uint(height),
        )?;
        self.conn.send(&Message::new(pointer, 4))?;
        let pause = !configured_step_delay().is_zero();
        if pause {
            self.sync()?;
            probe_step_delay();
        }
        if cancelled() {
            if !pause {
                self.sync()?;
            }
            return Ok(InputOutcome::Partial { steps_sent: 1 });
        }
        for state in [1, 0] {
            let time = self.timestamp();
            self.conn.send(
                &Message::new(pointer, 2)
                    .uint(time)
                    .uint(button.code())
                    .uint(state),
            )?;
            self.conn.send(&Message::new(pointer, 4))?;
        }
        self.sync()?;
        Ok(InputOutcome::Dispatched)
    }

    /// One key with modifiers: modifier state, press, release, cleared
    /// modifier state — always balanced once the press is sent.
    pub fn key(
        &mut self,
        key: SeatKey,
        modifiers: SeatModifiers,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<InputOutcome, SeatError> {
        if cancelled() {
            return Err(SeatError::Cancelled);
        }
        let mask = modifiers.mask();
        if mask != 0 {
            self.send_modifiers(mask)?;
        }
        self.tap(keymap::key_keycode(key))?;
        if mask != 0 {
            self.send_modifiers(0)?;
        }
        self.sync()?;
        Ok(InputOutcome::Dispatched)
    }

    /// Printable ASCII text, one balanced tap per character, checking
    /// cancellation between taps. The whole text is validated first.
    pub fn type_text(
        &mut self,
        text: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<InputOutcome, SeatError> {
        let plan = text_plan(text)?;
        if cancelled() {
            return Err(SeatError::Cancelled);
        }
        for (sent, keycode) in plan.iter().enumerate() {
            if sent > 0 && cancelled() {
                self.sync()?;
                return Ok(InputOutcome::Partial { steps_sent: sent });
            }
            self.tap(*keycode)?;
            // After a tap is on the socket, wait before the next cancel
            // check. Unset, this is zero. The CU-26 probe sets it so a
            // real compositor can show a prefix before the rest is sent.
            probe_step_delay();
        }
        self.sync()?;
        Ok(InputOutcome::Dispatched)
    }

    fn tap(&mut self, keycode: u32) -> Result<(), SeatError> {
        for state in [1, 0] {
            let time = self.timestamp();
            self.conn.send(
                &Message::new(self.keyboard, 1)
                    .uint(time)
                    .uint(keycode)
                    .uint(state),
            )?;
        }
        Ok(())
    }

    fn send_modifiers(&mut self, depressed: u32) -> Result<(), SeatError> {
        self.conn.send(
            &Message::new(self.keyboard, 2)
                .uint(depressed)
                .uint(0)
                .uint(0)
                .uint(0),
        )
    }

    fn next_event(&mut self, deadline: Instant) -> Result<Event, SeatError> {
        if Instant::now() >= deadline {
            return Err(SeatError::Io(
                "compositor did not answer within the I/O budget".into(),
            ));
        }
        let event = self.conn.next_event()?;
        if event.object == DISPLAY_ID && event.opcode == 0 {
            return Err(display_error(&event));
        }
        Ok(event)
    }

    fn track_event(&mut self, event: &Event) -> Result<(), SeatError> {
        track(
            event,
            self.shm,
            self.output,
            &mut self.output_state,
            &mut self.shm_formats,
        )
    }
}

/// Pause between seat steps when `PAWORK_WAYLAND_STEP_DELAY_MS` is
/// 1..=1000. Any other value, including unset, waits nothing. The delay
/// does not add or drop pointer or key events. `type_text` waits after
/// each tap. `click` round-trips absolute motion and then waits, so the
/// button decision can observe a dropped client.
fn probe_step_delay() {
    std::thread::sleep(configured_step_delay());
}

fn configured_step_delay() -> Duration {
    step_delay(
        std::env::var("PAWORK_WAYLAND_STEP_DELAY_MS")
            .ok()
            .as_deref(),
    )
}

fn step_delay(raw: Option<&str>) -> Duration {
    match raw.and_then(|text| text.parse::<u64>().ok()) {
        Some(ms) if (1..=1000).contains(&ms) => Duration::from_millis(ms),
        _ => Duration::ZERO,
    }
}

fn shm_to_rgb_supported(format: u32) -> bool {
    matches!(
        format,
        SHM_ARGB8888 | SHM_XRGB8888 | SHM_ABGR8888 | SHM_XBGR8888
    )
}

fn track(
    event: &Event,
    shm: u32,
    output: u32,
    state: &mut OutputState,
    formats: &mut Vec<u32>,
) -> Result<(), SeatError> {
    if event.object == shm && event.opcode == 0 {
        formats.push(event.args().uint()?);
    } else if event.object == output {
        let mut args = event.args();
        match event.opcode {
            0 => {
                for _ in 0..5 {
                    args.int()?;
                }
                args.string()?;
                args.string()?;
                state.transform = args.int()?;
            }
            1 => {
                let flags = args.uint()?;
                let (width, height) = (args.int()?, args.int()?);
                if flags & 1 != 0 && width > 0 && height > 0 {
                    state.width = width as u32;
                    state.height = height as u32;
                }
            }
            3 => state.scale = args.int()?,
            _ => {}
        }
    }
    Ok(())
}

fn display_error(event: &Event) -> SeatError {
    let mut args = event.args();
    match (args.uint(), args.uint(), args.string()) {
        (Ok(object), Ok(code), Ok(message)) => SeatError::Compositor {
            object,
            code,
            message,
        },
        _ => SeatError::Protocol("malformed wl_display.error".into()),
    }
}

/// wl_display.sync, dispatching every event to `handle` until the callback
/// fires; wl_display.error aborts with the compositor's reason.
fn roundtrip(
    conn: &mut Connection,
    mut handle: impl FnMut(&Event) -> Result<(), SeatError>,
) -> Result<(), SeatError> {
    let callback = conn.allocate()?;
    conn.send(&Message::new(DISPLAY_ID, 0).uint(callback))?;
    let deadline = Instant::now() + IO_BUDGET;
    loop {
        if Instant::now() >= deadline {
            return Err(SeatError::Io(
                "compositor did not answer within the I/O budget".into(),
            ));
        }
        let event = conn.next_event()?;
        if event.object == DISPLAY_ID && event.opcode == 0 {
            return Err(display_error(&event));
        }
        if event.object == callback && event.opcode == 0 {
            return Ok(());
        }
        handle(&event)?;
    }
}

fn memfd(name: &str, size: usize) -> Result<std::fs::File, SeatError> {
    let name = std::ffi::CString::new(name).expect("static name");
    // SAFETY: name is a valid C string; the returned descriptor is owned.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(SeatError::Io(std::io::Error::last_os_error().to_string()));
    }
    // SAFETY: fd is a fresh descriptor this function owns.
    let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    file.set_len(size as u64)
        .map_err(|error| SeatError::Io(error.to_string()))?;
    Ok(file)
}
