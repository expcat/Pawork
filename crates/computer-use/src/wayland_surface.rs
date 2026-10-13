//! Host side of the CU-26 Wayland guest.
//!
//! The seat lives in the guest container; this surface only speaks the
//! controlled guest frame protocol on the fixed loopback port. Discovery,
//! resolve and liveness read metadata only. Capture and seat input go
//! through NativeHost's approval, occupancy, permit and cancellation chain,
//! and every capture re-reads the window before and after the frame.

use std::sync::{Arc, Mutex, OnceLock};

use crate::guest::{
    decode_base64, CaptureQuery, GuestError, GuestLink, GuestMetadata, GuestWindow, InputQuery,
    SeatInput, WAYLAND_BUNDLE_PREFIX,
};
use crate::native_host::{
    AppSnapshot, CapturedFrame, ObservedAction, ObservedOutcome, WindowSnapshot,
};
use crate::target::{
    ActivityProof, AppFamily, AppIdentity, AxTreeRead, Environment, NativeActionKind, NativeProbe,
    ProcessInstance, SemanticAction, SemanticOutcome, TargetError, TargetedInput, TargetedKey,
    TargetedOutcome, ValidatedElement, ValidatedObservationDispatch, ValidatedWindow,
    ValidatedWindowDispatch, WindowIdentity, WindowMove,
};
use crate::{NativeHost, NativeSurface, MAX_IMAGE_BYTES, MAX_IMAGE_EDGE};

/// Stack value the guest reports for a window Sway says is visible. Any
/// other value is a window that does not paint on the output right now.
pub const VISIBLE_STACK: u32 = 1;

/// One Wayland guest connection shared by every clone of this surface.
pub struct WaylandSurface {
    dial: Arc<dyn Fn() -> Result<GuestLink, GuestError> + Send + Sync>,
    link: Arc<Mutex<Option<GuestLink>>>,
}

impl Clone for WaylandSurface {
    fn clone(&self) -> Self {
        Self {
            dial: Arc::clone(&self.dial),
            link: Arc::clone(&self.link),
        }
    }
}

impl WaylandSurface {
    /// Production dial: the fixed loopback Wayland guest, never a model input.
    pub fn endpoint() -> Self {
        Self::dial(GuestLink::connect_wayland)
    }

    /// Test dial. The closure may connect a local listener; production does not use it.
    pub fn test_dial(
        dial: impl Fn() -> Result<GuestLink, GuestError> + Send + Sync + 'static,
    ) -> Self {
        Self::dial(dial)
    }

    fn dial(dial: impl Fn() -> Result<GuestLink, GuestError> + Send + Sync + 'static) -> Self {
        Self {
            dial: Arc::new(dial),
            link: Arc::new(Mutex::new(None)),
        }
    }

    /// Drop the cached connection so the next call is a new guest epoch.
    #[cfg(test)]
    pub fn disconnect(&self) {
        let mut slot = self
            .link
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *slot = None;
    }

    fn with_link<T>(
        &self,
        op: impl FnOnce(&mut GuestLink) -> Result<T, GuestError>,
    ) -> Result<T, GuestError> {
        let mut slot = self
            .link
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if slot.is_none() {
            *slot = Some((self.dial)()?);
        }
        let Some(link) = slot.as_mut() else {
            return Err(GuestError::Disconnected(
                "guest connection was not installed",
            ));
        };
        let result = op(link);
        if matches!(
            &result,
            Err(GuestError::Disconnected(_) | GuestError::Protocol(_) | GuestError::Identity(_))
        ) {
            *slot = None;
        }
        result
    }

    fn metadata(&self) -> Result<GuestMetadata, TargetError> {
        self.with_link(|link| link.metadata()).map_err(map_wayland)
    }

    fn issued<'a>(
        metadata: &'a GuestMetadata,
        instance: &ProcessInstance,
        window: &WindowIdentity,
    ) -> Option<&'a GuestWindow> {
        metadata.apps.iter().find_map(|app| {
            if app.pid != instance.pid || app.start_token != instance.start_token {
                return None;
            }
            app.windows.iter().find(|item| {
                item.window_id == window.window_id && item.generation == window.generation
            })
        })
    }

    /// The issued window as it paints right now. Hidden or overlapped
    /// windows are refused: a crop or a click there could belong to
    /// another client.
    fn painted(&self, validated: &ValidatedWindow) -> Result<GuestWindow, TargetError> {
        let metadata = self.metadata()?;
        let Some(window) = Self::issued(&metadata, &validated.instance, &validated.window) else {
            return Err(TargetError::WindowReplaced);
        };
        if let Some(reason) = unpainted(&metadata, window) {
            return Err(TargetError::ProbeUnavailable(reason.into()));
        }
        Ok(window.clone())
    }
}

/// Why a window cannot be cut from the output or clicked, if it cannot.
/// Sway tiles without overlap, so any overlap means a floating or
/// fullscreen window whose order is not proven here.
pub(crate) fn unpainted(metadata: &GuestMetadata, window: &GuestWindow) -> Option<&'static str> {
    if window.stack != VISIBLE_STACK {
        return Some("wayland window is not visible on the output");
    }
    if window.width == 0 || window.height == 0 {
        return Some("wayland window has no area");
    }
    let overlaps = |x: i32, y: i32, width: u32, height: u32| {
        let (ax0, ay0) = (i64::from(window.x), i64::from(window.y));
        let (ax1, ay1) = (
            ax0 + i64::from(window.width),
            ay0 + i64::from(window.height),
        );
        let (bx0, by0) = (i64::from(x), i64::from(y));
        let (bx1, by1) = (bx0 + i64::from(width), by0 + i64::from(height));
        ax0 < bx1 && bx0 < ax1 && ay0 < by1 && by0 < ay1
    };
    let other_app = metadata
        .apps
        .iter()
        .flat_map(|app| app.windows.iter())
        .filter(|item| item.window_id != window.window_id && item.stack == VISIBLE_STACK)
        .any(|item| overlaps(item.x, item.y, item.width, item.height));
    let rejected = metadata.rejected.iter().any(|item| {
        item.window_id != window.window_id
            && (!item.geometry_known || overlaps(item.x, item.y, item.width, item.height))
    });
    (other_app || rejected).then_some("wayland window overlaps another window")
}

fn map_wayland(error: GuestError) -> TargetError {
    match error {
        GuestError::Unavailable(message) if message.contains("not focused") => {
            TargetError::NotFocused
        }
        GuestError::Unavailable(message) if message.contains("generation") => {
            TargetError::WindowReplaced
        }
        GuestError::Unavailable(message) => TargetError::ProbeUnavailable(message),
        GuestError::Disconnected(message)
        | GuestError::Identity(message)
        | GuestError::Protocol(message) => TargetError::ProbeUnavailable(message.into()),
    }
}

fn input_outcome(error: GuestError) -> Result<ObservedOutcome, TargetError> {
    match error {
        GuestError::Disconnected("cancelled before the guest action") => {
            Err(TargetError::Cancelled)
        }
        GuestError::Unavailable(message)
            if message.contains("cancelled before the guest action") =>
        {
            Err(TargetError::Cancelled)
        }
        GuestError::Disconnected("input receipt lost") => Ok(ObservedOutcome::UnknownEffect),
        GuestError::Unavailable(message) if message.contains("receipt lost") => {
            Ok(ObservedOutcome::UnknownEffect)
        }
        other => Err(map_wayland(other)),
    }
}

fn seat_key_name(key: TargetedKey) -> &'static str {
    match key {
        TargetedKey::Return => "return",
        TargetedKey::Tab => "tab",
        TargetedKey::Space => "space",
        TargetedKey::Backspace => "backspace",
        TargetedKey::Escape => "escape",
        TargetedKey::Delete => "delete",
        TargetedKey::Home => "home",
        TargetedKey::End => "end",
        TargetedKey::PageUp => "pageup",
        TargetedKey::PageDown => "pagedown",
        TargetedKey::Left => "left",
        TargetedKey::Up => "up",
        TargetedKey::Right => "right",
        TargetedKey::Down => "down",
    }
}

/// The guest keymap covers printable ASCII only (CU-26 slice 1).
fn keymap_text(text: &str) -> bool {
    text.chars().all(|ch| ch == ' ' || ch.is_ascii_graphic())
}

impl NativeProbe for WaylandSurface {
    fn process_instance_alive(&self, instance: &ProcessInstance) -> bool {
        self.metadata().ok().is_some_and(|metadata| {
            metadata
                .apps
                .iter()
                .any(|app| app.pid == instance.pid && app.start_token == instance.start_token)
        })
    }

    fn window_generation(&self, instance: &ProcessInstance, window_id: u64) -> Option<u64> {
        let metadata = self.metadata().ok()?;
        metadata.apps.iter().find_map(|app| {
            if app.pid != instance.pid || app.start_token != instance.start_token {
                return None;
            }
            app.windows
                .iter()
                .find(|window| window.window_id == window_id)
                .map(|window| window.generation)
        })
    }

    fn tree_revision(&self, _window: &WindowIdentity) -> Option<u64> {
        None
    }

    fn current_origin(&self, _window: &WindowIdentity) -> Option<String> {
        None
    }

    fn window_size(&self, instance: &ProcessInstance, window_id: u64) -> Option<(f64, f64)> {
        let metadata = self.metadata().ok()?;
        metadata.apps.iter().find_map(|app| {
            if app.pid != instance.pid || app.start_token != instance.start_token {
                return None;
            }
            app.windows
                .iter()
                .find(|window| window.window_id == window_id)
                .map(|window| (f64::from(window.width), f64::from(window.height)))
        })
    }
}

impl NativeSurface for WaylandSurface {
    /// The guest runs only the Pawork service and approved targets; there
    /// is no human seat in it to compete with.
    fn activity(&self, _bundle_id: &str) -> ActivityProof {
        ActivityProof::Idle
    }

    fn observation_environment(&self) -> Environment {
        Environment::IsolatedDesktop
    }

    fn admit_action(
        &self,
        _family: AppFamily,
        action: NativeActionKind,
    ) -> Result<(), TargetError> {
        match action {
            NativeActionKind::CaptureWindow
            | NativeActionKind::Pointer
            | NativeActionKind::TargetedKeys => Ok(()),
            _ => Err(TargetError::BackgroundUnsupported),
        }
    }

    fn admit_observed(&self, action: &ObservedAction) -> Result<(), TargetError> {
        match action {
            ObservedAction::Keys(TargetedInput::InsertText(text)) if !keymap_text(text) => Err(
                TargetError::Invalid("the wayland guest keymap covers printable ASCII only"),
            ),
            _ => Ok(()),
        }
    }

    fn discover(&self) -> Vec<AppSnapshot> {
        let Ok(metadata) = self.metadata() else {
            return Vec::new();
        };
        metadata
            .apps
            .into_iter()
            .filter(|app| app.bundle_id.starts_with(WAYLAND_BUNDLE_PREFIX))
            .map(|app| AppSnapshot {
                identity: AppIdentity {
                    bundle_id: app.bundle_id,
                    family: AppFamily::Browser,
                },
                instance: ProcessInstance {
                    pid: app.pid,
                    start_token: app.start_token,
                },
                localized_name: app.name,
                window_count: app.windows.len(),
            })
            .collect()
    }

    fn resolve(&self, bundle_id: &str) -> Option<AppIdentity> {
        if !bundle_id.starts_with(WAYLAND_BUNDLE_PREFIX) {
            return None;
        }
        let metadata = self.metadata().ok()?;
        metadata
            .apps
            .iter()
            .find(|app| app.bundle_id == bundle_id)
            .map(|app| AppIdentity {
                bundle_id: app.bundle_id.clone(),
                family: AppFamily::Browser,
            })
    }

    fn list_windows(
        &self,
        identity: &AppIdentity,
        instance: &ProcessInstance,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<WindowSnapshot>, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let metadata = self.metadata()?;
        let Some(app) = metadata.apps.iter().find(|app| {
            app.bundle_id == identity.bundle_id
                && app.pid == instance.pid
                && app.start_token == instance.start_token
        }) else {
            return Err(TargetError::ProcessRestarted);
        };
        Ok(app
            .windows
            .iter()
            .map(|window| WindowSnapshot {
                window: WindowIdentity {
                    window_id: window.window_id,
                    generation: window.generation,
                },
                width: f64::from(window.width),
                height: f64::from(window.height),
                title: None,
            })
            .collect())
    }

    fn launch(
        &self,
        _identity: &AppIdentity,
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<ProcessInstance, TargetError> {
        Err(TargetError::Invalid(
            "wayland guest applications start with the guest, not by the host",
        ))
    }

    fn capture(
        &self,
        validated: &ValidatedWindow,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<CapturedFrame, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let before = self.painted(validated)?;
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let capture = self
            .with_link(|link| {
                link.capture(&CaptureQuery {
                    window_id: validated.window.window_id,
                    generation: validated.window.generation,
                })
            })
            .map_err(map_wayland)?;
        let reported = (
            capture.window_id,
            capture.generation,
            capture.x,
            capture.y,
            capture.width,
            capture.height,
        );
        let expected = |window: &GuestWindow| {
            (
                window.window_id,
                window.generation,
                window.x,
                window.y,
                window.width,
                window.height,
            )
        };
        if reported != expected(&before) {
            return Err(TargetError::WindowReplaced);
        }
        let after = self.painted(validated)?;
        if expected(&after) != expected(&before) {
            return Err(TargetError::WindowReplaced);
        }
        let jpeg = decode_base64(&capture.jpeg)
            .ok_or_else(|| TargetError::ProbeUnavailable("guest capture is not base64".into()))?;
        let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
            .map_err(|_| TargetError::ProbeUnavailable("guest capture is not a JPEG".into()))?;
        if jpeg.len() > MAX_IMAGE_BYTES
            || decoded.width() != capture.image_width
            || decoded.height() != capture.image_height
            || capture.image_width.max(capture.image_height) > MAX_IMAGE_EDGE
        {
            return Err(TargetError::ProbeUnavailable(
                "guest capture is outside the image budget or its reported size".into(),
            ));
        }
        Ok(CapturedFrame {
            image_width: capture.image_width,
            image_height: capture.image_height,
            window_width: f64::from(after.width),
            window_height: f64::from(after.height),
            jpeg,
        })
    }

    fn read_tree(
        &self,
        _validated: &ValidatedWindow,
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<AxTreeRead, TargetError> {
        Err(TargetError::BackgroundUnsupported)
    }

    fn semantic(
        &self,
        _validated: &ValidatedElement,
        _path: &[u32],
        _action: &SemanticAction,
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<SemanticOutcome, TargetError> {
        Err(TargetError::BackgroundUnsupported)
    }

    fn targeted(
        &self,
        _validated: &ValidatedElement,
        _path: &[u32],
        _input: &TargetedInput,
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<TargetedOutcome, TargetError> {
        Err(TargetError::BackgroundUnsupported)
    }

    fn move_window(
        &self,
        _validated: &ValidatedWindowDispatch,
        _origin: &WindowMove,
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<SemanticOutcome, TargetError> {
        Err(TargetError::BackgroundUnsupported)
    }

    fn observed_input(
        &self,
        validated: &ValidatedObservationDispatch,
        action: &ObservedAction,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ObservedOutcome, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let window = self.painted(&validated.observation.window)?;
        let input = match action {
            ObservedAction::Click => {
                let point = validated
                    .point
                    .ok_or(TargetError::Invalid("a click needs an image point"))?;
                let (x, y) = (point.x.floor(), point.y.floor());
                if x < 0.0
                    || y < 0.0
                    || x >= f64::from(window.width)
                    || y >= f64::from(window.height)
                {
                    return Err(TargetError::OutOfBounds);
                }
                SeatInput::Click {
                    x: x as u32,
                    y: y as u32,
                }
            }
            ObservedAction::Keys(TargetedInput::InsertText(text)) => {
                if !keymap_text(text) {
                    return Err(TargetError::Invalid(
                        "the wayland guest keymap covers printable ASCII only",
                    ));
                }
                SeatInput::Text { text: text.clone() }
            }
            ObservedAction::Keys(TargetedInput::KeyPress(press)) => SeatInput::Key {
                key: seat_key_name(press.key).into(),
                shift: press.modifiers.shift,
                control: press.modifiers.control,
                alt: press.modifiers.option,
            },
        };
        let query = InputQuery {
            window_id: window.window_id,
            generation: window.generation,
            input,
        };
        let receipt = self.with_link(|link| {
            if cancelled() {
                return Err(GuestError::Disconnected(
                    "cancelled before the guest action",
                ));
            }
            link.input_stopping(&query, cancelled)
        });
        match receipt {
            Err(error) => input_outcome(error),
            Ok(receipt)
                if receipt.window_id != query.window_id
                    || receipt.generation != query.generation =>
            {
                Ok(ObservedOutcome::UnknownEffect)
            }
            Ok(receipt) if receipt.complete => Ok(ObservedOutcome::Dispatched {
                steps: receipt.steps_sent,
            }),
            Ok(receipt) => Ok(ObservedOutcome::Partial {
                steps_sent: receipt.steps_sent,
                steps_total: receipt.steps_total,
                cause: TargetError::Cancelled,
            }),
        }
    }
}

/// The Wayland guest registry. Separate from [NativeHost::host] and the
/// X11 isolated host.
pub fn shared_wayland_host() -> Arc<NativeHost<WaylandSurface>> {
    static HOST: OnceLock<Arc<NativeHost<WaylandSurface>>> = OnceLock::new();
    HOST.get_or_init(|| Arc::new(NativeHost::wayland())).clone()
}

#[cfg(test)]
mod tests;
