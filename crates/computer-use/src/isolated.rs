//! Explicit isolated-desktop surface.
//!
//! Discover, resolve, and liveness read guest metadata only. Titles and the
//! accessibility tree stay behind a granted window, and that tree is rooted at
//! the issued window. Capture reuses the single RFB client and refuses a
//! window that another window covers.

use std::sync::{Arc, Mutex, OnceLock};

#[cfg(test)]
use crate::guest::RejectedWindow;
use crate::guest::{
    ax_nodes, tree_revision, GuestAppInfo, GuestError, GuestLink, GuestMetadata, GuestWindow,
    PressQuery, TreeQuery, FIREFOX_BUNDLE_ID,
};
use crate::native_host::{AppSnapshot, CapturedFrame, WindowSnapshot};
use crate::target::{
    ActivityProof, AppFamily, AppIdentity, AxTreeRead, AxTreeTruncation, Environment,
    NativeActionKind, NativeProbe, ProcessInstance, SemanticAction, SemanticOutcome, TargetError,
    TargetedInput, TargetedOutcome, ValidatedElement, ValidatedWindow, ValidatedWindowDispatch,
    WindowIdentity, WindowMove,
};
use crate::{Computer, NativeHost, NativeSurface};

/// One guest connection shared by every clone of this surface.
pub struct GuestSurface {
    dial: Arc<dyn Fn() -> Result<GuestLink, GuestError> + Send + Sync>,
    link: Arc<Mutex<Option<GuestLink>>>,
}

impl Clone for GuestSurface {
    fn clone(&self) -> Self {
        Self {
            dial: Arc::clone(&self.dial),
            link: Arc::clone(&self.link),
        }
    }
}

impl GuestSurface {
    /// Production dial. The port is the fixed loopback guest, never a model input.
    pub fn endpoint() -> Self {
        Self::dial(GuestLink::connect)
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
        self.with_link(|link| link.metadata()).map_err(map_guest)
    }

    fn firefox<'a>(metadata: &'a GuestMetadata, bundle_id: &str) -> Option<&'a GuestAppInfo> {
        metadata
            .apps
            .iter()
            .find(|app| app.bundle_id.eq_ignore_ascii_case(bundle_id))
    }

    fn issued_window<'a>(
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

    fn issued_geometry(
        &self,
        validated: &ValidatedWindow,
    ) -> Result<(i32, i32, u32, u32, u64), TargetError> {
        let metadata = self.metadata()?;
        let Some(window) = Self::issued_window(&metadata, &validated.instance, &validated.window)
        else {
            return Err(TargetError::WindowReplaced);
        };
        if covered(&metadata, window) {
            return Err(TargetError::ProbeUnavailable(
                "isolated window is covered by another window".into(),
            ));
        }
        Ok((
            window.x,
            window.y,
            window.width,
            window.height,
            window.generation,
        ))
    }
}

fn map_guest(error: GuestError) -> TargetError {
    match error {
        GuestError::Unavailable(message) if identity_lost(&message) => TargetError::WindowReplaced,
        GuestError::Disconnected(message) => TargetError::ProbeUnavailable(message.into()),
        GuestError::Unavailable(message) => TargetError::ProbeUnavailable(message),
        GuestError::Identity(message) | GuestError::Protocol(message) => {
            TargetError::ProbeUnavailable(message.into())
        }
    }
}

fn identity_lost(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("generation")
        || message.contains("not the issued")
        || message.contains("tree revision")
}

fn rect(window: &GuestWindow) -> (i64, i64, i64, i64) {
    (
        i64::from(window.x),
        i64::from(window.y),
        i64::from(window.x) + i64::from(window.width),
        i64::from(window.y) + i64::from(window.height),
    )
}

fn intersects(left: &GuestWindow, right: &GuestWindow) -> bool {
    let (ax0, ay0, ax1, ay1) = rect(left);
    let (bx0, by0, bx1, by1) = rect(right);
    ax0 < bx1 && bx0 < ax1 && ay0 < by1 && by0 < ay1
}

fn covered(metadata: &GuestMetadata, window: &GuestWindow) -> bool {
    let own = own_structure(metadata, window);
    let mut others = Vec::new();
    for app in &metadata.apps {
        for item in &app.windows {
            if !own.contains(&item.window_id) {
                others.push(item.clone());
            }
        }
    }
    for rejected in &metadata.rejected {
        if own.contains(&rejected.window_id) {
            continue;
        }
        if !rejected.geometry_known {
            return true;
        }
        others.push(GuestWindow {
            window_id: rejected.window_id,
            generation: 0,
            x: rejected.x,
            y: rejected.y,
            width: rejected.width,
            height: rejected.height,
            class: rejected.class.clone(),
            instance: rejected.instance.clone(),
            stack: rejected.stack,
            parent: rejected.parent,
        });
    }
    others.iter().any(|other| {
        intersects(window, other)
            && (window.stack == u32::MAX || other.stack == u32::MAX || other.stack > window.stack)
    })
}

/// Reported ancestors of the target, plus chrome and content under that chain.
/// The screen root is not a reported window, so the walk stops there. Another
/// reported client is not chrome merely because it shares an ancestor.
fn own_structure(metadata: &GuestMetadata, window: &GuestWindow) -> std::collections::HashSet<u64> {
    let mut parent_of = std::collections::HashMap::<u64, u64>::new();
    let mut children_of = std::collections::HashMap::<u64, Vec<u64>>::new();
    let mut listed = std::collections::HashSet::new();
    let mut clients = std::collections::HashSet::new();
    let mut link = |id: u64, parent: u64| {
        if parent == 0 || parent == id {
            return;
        }
        parent_of.insert(id, parent);
        children_of.entry(parent).or_default().push(id);
    };
    for app in &metadata.apps {
        for item in &app.windows {
            listed.insert(item.window_id);
            clients.insert(item.window_id);
            link(item.window_id, item.parent);
        }
    }
    for rejected in &metadata.rejected {
        listed.insert(rejected.window_id);
        if rejected.stack != u32::MAX {
            clients.insert(rejected.window_id);
        }
        link(rejected.window_id, rejected.parent);
    }
    let mut own = std::collections::HashSet::new();
    let mut current = window.window_id;
    loop {
        if !own.insert(current) {
            break;
        }
        let Some(parent) = parent_of.get(&current).copied() else {
            break;
        };
        if !listed.contains(&parent) {
            break;
        }
        current = parent;
    }
    let mut queue: Vec<u64> = own.iter().copied().collect();
    while let Some(id) = queue.pop() {
        for child in children_of.get(&id).into_iter().flatten().copied() {
            if child != window.window_id && clients.contains(&child) {
                continue;
            }
            if own.insert(child) {
                queue.push(child);
            }
        }
    }
    own
}

impl NativeProbe for GuestSurface {
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

    fn tree_revision(&self, window: &WindowIdentity) -> Option<u64> {
        let metadata = self.metadata().ok()?;
        let live = metadata
            .apps
            .iter()
            .flat_map(|app| app.windows.iter())
            .any(|item| item.window_id == window.window_id && item.generation == window.generation);
        if !live {
            return None;
        }
        let tree = self
            .with_link(|link| {
                link.tree(&TreeQuery {
                    window_id: window.window_id,
                    generation: window.generation,
                })
            })
            .ok()?;
        (tree.window_id == window.window_id).then(|| tree_revision(&tree))
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

impl NativeSurface for GuestSurface {
    fn activity(&self, _bundle_id: &str) -> ActivityProof {
        ActivityProof::Idle
    }

    fn observation_environment(&self) -> Environment {
        Environment::IsolatedDesktop
    }

    fn admit_action(&self, family: AppFamily, action: NativeActionKind) -> Result<(), TargetError> {
        let proved = family == AppFamily::Browser
            && matches!(
                action,
                NativeActionKind::CaptureWindow
                    | NativeActionKind::AxRead
                    | NativeActionKind::AxPress
            );
        if proved {
            Ok(())
        } else {
            Err(TargetError::BackgroundUnsupported)
        }
    }

    fn discover(&self) -> Vec<AppSnapshot> {
        let Ok(metadata) = self.metadata() else {
            return Vec::new();
        };
        metadata
            .apps
            .into_iter()
            .filter(|app| app.bundle_id == FIREFOX_BUNDLE_ID)
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
        let metadata = self.metadata().ok()?;
        Self::firefox(&metadata, bundle_id).map(|app| AppIdentity {
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
            "the isolated firefox guest is started with the desktop, not by the host",
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
        let issued = self.issued_geometry(validated)?;
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let frame = Computer::capture_frame(issued.0, issued.1, issued.2, issued.3)
            .map_err(|error| TargetError::ProbeUnavailable(error.to_string()))?;
        let again = self.issued_geometry(validated)?;
        if again != issued {
            return Err(TargetError::WindowReplaced);
        }
        Ok(CapturedFrame {
            image_width: frame.width,
            image_height: frame.height,
            window_width: f64::from(again.2),
            window_height: f64::from(again.3),
            jpeg: frame.jpeg,
        })
    }

    fn read_tree(
        &self,
        validated: &ValidatedWindow,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<AxTreeRead, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let metadata = self.metadata()?;
        if Self::issued_window(&metadata, &validated.instance, &validated.window).is_none() {
            return Err(TargetError::WindowReplaced);
        }
        let tree = self
            .with_link(|link| {
                link.tree(&TreeQuery {
                    window_id: validated.window.window_id,
                    generation: validated.window.generation,
                })
            })
            .map_err(map_guest)?;
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let rooted = tree
            .nodes
            .first()
            .is_some_and(|root| root.path.is_empty() && root.depth == 0);
        if tree.window_id != validated.window.window_id || !rooted {
            return Err(TargetError::ProbeUnavailable(
                "guest tree is not rooted at the issued window".into(),
            ));
        }
        Ok(AxTreeRead {
            window: validated.window,
            tree_revision: tree_revision(&tree),
            nodes: ax_nodes(&tree.nodes),
            truncation: AxTreeTruncation {
                depth: tree.truncation.depth,
                nodes: tree.truncation.nodes,
                text: tree.truncation.text,
                names: tree.truncation.names,
            },
        })
    }

    fn semantic(
        &self,
        validated: &ValidatedElement,
        path: &[u32],
        action: &SemanticAction,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SemanticOutcome, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        if !matches!(action, SemanticAction::Press) {
            return Err(TargetError::BackgroundUnsupported);
        }
        let metadata = self.metadata()?;
        if Self::issued_window(
            &metadata,
            &validated.window.instance,
            &validated.window.window,
        )
        .is_none()
        {
            return Err(TargetError::WindowReplaced);
        }
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let query = PressQuery {
            window_id: validated.window.window.window_id,
            generation: validated.window.window.generation,
            tree_revision: validated.tree_revision,
            path: path.to_vec(),
        };
        let pressed = self.with_link(|link| {
            if cancelled() {
                return Err(GuestError::Disconnected(
                    "cancelled before the guest action",
                ));
            }
            link.press_stopping(&query, cancelled)
        });
        match pressed {
            Err(error) => press_outcome(error),
            Ok(pressed) => {
                if pressed.window_id != query.window_id
                    || pressed.generation != query.generation
                    || pressed.tree_revision != query.tree_revision
                    || pressed.path != query.path
                {
                    Ok(SemanticOutcome::UnknownEffect)
                } else {
                    Ok(SemanticOutcome::Dispatched)
                }
            }
        }
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
}

fn press_outcome(error: GuestError) -> Result<SemanticOutcome, TargetError> {
    match error {
        GuestError::Disconnected("cancelled before the guest action") => {
            Err(TargetError::Cancelled)
        }
        GuestError::Unavailable(message)
            if message.contains("cancelled before the guest action") =>
        {
            Err(TargetError::Cancelled)
        }
        GuestError::Disconnected("press receipt lost") => Ok(SemanticOutcome::UnknownEffect),
        GuestError::Unavailable(message) if message.contains("action receipt lost") => {
            Ok(SemanticOutcome::UnknownEffect)
        }
        GuestError::Unavailable(message) if message.contains("timed out") => {
            Ok(SemanticOutcome::UnknownEffect)
        }
        other => Err(map_guest(other)),
    }
}

/// The isolated registry. Separate from [NativeHost::host].
pub fn shared_host() -> Arc<NativeHost<GuestSurface>> {
    static HOST: OnceLock<Arc<NativeHost<GuestSurface>>> = OnceLock::new();
    HOST.get_or_init(|| Arc::new(NativeHost::isolated()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{GrantKind, TargetIdentity};
    use crate::guest::{serve_connection, GuestApp, GuestTree, Pressed};
    use crate::target::Scope;
    use crate::NativeRequest;
    use std::io::ErrorKind;
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    const TOKEN: &str = "0123456789abcdef";

    #[derive(Clone)]
    struct Counts {
        metadata: Arc<AtomicUsize>,
        tree: Arc<AtomicUsize>,
        press: Arc<AtomicUsize>,
        epoch: Arc<AtomicU64>,
        nonce: Arc<AtomicU64>,
    }

    impl Counts {
        fn new() -> Self {
            Self {
                metadata: Arc::new(AtomicUsize::new(0)),
                tree: Arc::new(AtomicUsize::new(0)),
                press: Arc::new(AtomicUsize::new(0)),
                epoch: Arc::new(AtomicU64::new(0)),
                nonce: Arc::new(AtomicU64::new(0x51CE_0001)),
            }
        }

        fn generation(&self) -> u64 {
            let epoch = self.epoch.load(Ordering::SeqCst).max(1);
            let nonce = self.nonce.load(Ordering::SeqCst);
            let mixed = nonce
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(epoch);
            if mixed == 0 {
                1
            } else {
                mixed
            }
        }
    }

    struct Fake {
        counts: Counts,
    }

    impl GuestApp for Fake {
        fn begin_session(&mut self) {
            self.counts.epoch.fetch_add(1, Ordering::SeqCst);
        }

        fn metadata(&mut self) -> Result<GuestMetadata, GuestError> {
            self.counts.metadata.fetch_add(1, Ordering::SeqCst);
            let generation = self.counts.generation();
            Ok(GuestMetadata {
                apps: vec![GuestAppInfo {
                    bundle_id: FIREFOX_BUNDLE_ID.into(),
                    name: "Firefox".into(),
                    pid: 25,
                    start_token: 99,
                    windows: vec![GuestWindow {
                        window_id: 7,
                        generation,
                        x: 10,
                        y: 20,
                        width: 200,
                        height: 150,
                        class: "firefox-esr".into(),
                        instance: "Navigator".into(),
                        stack: 1,
                        parent: 1,
                    }],
                }],
                rejected: Vec::new(),
                epoch: generation,
            })
        }

        fn tree(&mut self, _: &TreeQuery) -> Result<GuestTree, GuestError> {
            self.counts.tree.fetch_add(1, Ordering::SeqCst);
            Err(GuestError::Unavailable("tree was not authorized".into()))
        }

        fn press(
            &mut self,
            _: &crate::guest::PressQuery,
            _: &dyn Fn() -> bool,
        ) -> Result<Pressed, GuestError> {
            self.counts.press.fetch_add(1, Ordering::SeqCst);
            Err(GuestError::Unavailable("press was not authorized".into()))
        }
    }

    struct GuestDouble {
        surface: GuestSurface,
        counts: Counts,
        stop: Arc<AtomicBool>,
        server: Option<thread::JoinHandle<()>>,
    }

    impl GuestDouble {
        fn start() -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let counts = Counts::new();
            let stop = Arc::new(AtomicBool::new(false));
            let server_counts = counts.clone();
            let server_stop = Arc::clone(&stop);
            let server = thread::spawn(move || {
                while !server_stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                            let mut fake = Fake {
                                counts: server_counts.clone(),
                            };
                            let _ = serve_connection(stream, TOKEN, &mut fake);
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            });
            let token = TOKEN.to_string();
            let surface = GuestSurface::test_dial(move || {
                let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
                    .map_err(|_| GuestError::Disconnected("test guest dial failed"))?;
                GuestLink::handshake(stream, &token)
            });
            Self {
                surface,
                counts,
                stop,
                server: Some(server),
            }
        }
    }

    impl Drop for GuestDouble {
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

    fn firefox_target() -> TargetIdentity {
        TargetIdentity::application(AppIdentity {
            bundle_id: FIREFOX_BUNDLE_ID.into(),
            family: AppFamily::Browser,
        })
    }

    #[test]
    fn unauthorized_discovery_reads_metadata_only() {
        let double = GuestDouble::start();
        let host = NativeHost::new(double.surface.clone());
        let scope = scope();
        host.open_scope(&scope);
        host.execute(&scope, NativeRequest::Discover, &|| false)
            .unwrap();
        host.execute(
            &scope,
            NativeRequest::Request {
                bundle_id: FIREFOX_BUNDLE_ID.into(),
                origin: None,
            },
            &|| false,
        )
        .unwrap();
        let denied = host
            .execute(
                &scope,
                NativeRequest::Bind {
                    bundle_id: FIREFOX_BUNDLE_ID.into(),
                },
                &|| false,
            )
            .unwrap_err();
        assert_eq!(denied, TargetError::NotAuthorized);
        assert!(double.counts.metadata.load(Ordering::SeqCst) > 0);
        assert_eq!(double.counts.tree.load(Ordering::SeqCst), 0);
        assert_eq!(double.counts.press.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn old_window_after_guest_reconnect_is_rejected_without_tree_or_press() {
        let double = GuestDouble::start();
        let host = NativeHost::new(double.surface.clone());
        let scope = scope();
        host.open_scope(&scope);
        host.grant(&scope, &firefox_target(), GrantKind::ForRun)
            .unwrap();
        let bound = host
            .execute(
                &scope,
                NativeRequest::Bind {
                    bundle_id: FIREFOX_BUNDLE_ID.into(),
                },
                &|| false,
            )
            .unwrap();
        let window = bound.window_id.expect("bind returns a window handle");
        double.counts.epoch.store(0, Ordering::SeqCst);
        double.counts.nonce.fetch_add(1, Ordering::SeqCst);
        double.surface.disconnect();
        let error = host
            .execute(&scope, NativeRequest::ReadTree { window }, &|| false)
            .unwrap_err();
        assert!(matches!(
            error,
            TargetError::WindowReplaced | TargetError::ProcessRestarted
        ));
        assert_eq!(double.counts.tree.load(Ordering::SeqCst), 0);
        assert_eq!(double.counts.press.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unconfirmed_overlap_and_lost_action_receipt_are_not_clean_failures() {
        let target = GuestWindow {
            window_id: 7,
            generation: 1,
            x: 0,
            y: 0,
            width: 200,
            height: 150,
            class: "firefox-esr".into(),
            instance: "Navigator".into(),
            stack: 4,
            parent: 1,
        };
        let below = GuestWindow {
            window_id: 8,
            generation: 0,
            x: 0,
            y: 0,
            width: 20,
            height: 20,
            class: String::new(),
            instance: String::new(),
            stack: 1,
            parent: 2,
        };
        let unknown = GuestWindow {
            window_id: 9,
            generation: 0,
            x: 10,
            y: 10,
            width: 30,
            height: 30,
            class: String::new(),
            instance: String::new(),
            stack: u32::MAX,
            parent: 2,
        };
        let metadata = GuestMetadata {
            apps: vec![GuestAppInfo {
                bundle_id: FIREFOX_BUNDLE_ID.into(),
                name: "Firefox".into(),
                pid: 25,
                start_token: 99,
                windows: vec![target.clone()],
            }],
            rejected: vec![
                RejectedWindow {
                    window_id: below.window_id,
                    x: below.x,
                    y: below.y,
                    width: below.width,
                    height: below.height,
                    class: below.class.clone(),
                    instance: below.instance.clone(),
                    stack: below.stack,
                    parent: below.parent,
                    geometry_known: true,
                    reason: "not firefox".into(),
                },
                RejectedWindow {
                    window_id: unknown.window_id,
                    x: unknown.x,
                    y: unknown.y,
                    width: unknown.width,
                    height: unknown.height,
                    class: unknown.class,
                    instance: unknown.instance,
                    stack: unknown.stack,
                    parent: unknown.parent,
                    geometry_known: true,
                    reason: "stack order is unknown".into(),
                },
            ],
            epoch: 1,
        };
        assert!(
            covered(&metadata, &target),
            "an overlapping window with an unknown stack must refuse capture"
        );
        let metadata = GuestMetadata {
            rejected: metadata.rejected.into_iter().take(1).collect(),
            ..metadata
        };
        assert!(
            !covered(&metadata, &target),
            "a smaller window proven below the target does not cover it"
        );
        let unranked = GuestWindow {
            stack: u32::MAX,
            ..target.clone()
        };
        assert!(
            covered(&metadata, &unranked),
            "an unranked target overlapping a ranked window must refuse capture"
        );
        let chrome = GuestMetadata {
            rejected: vec![RejectedWindow {
                window_id: target.parent,
                x: 0,
                y: 0,
                width: 220,
                height: 180,
                class: String::new(),
                instance: String::new(),
                stack: u32::MAX,
                parent: 0,
                geometry_known: false,
                reason: "target frame".into(),
            }],
            ..metadata
        };
        assert!(
            !covered(&chrome, &target),
            "the target's own parent frame is not another window"
        );
        let ancestor = GuestMetadata {
            rejected: vec![
                chrome.rejected[0].clone(),
                RejectedWindow {
                    window_id: 12,
                    x: 0,
                    y: 0,
                    width: 240,
                    height: 200,
                    class: String::new(),
                    instance: String::new(),
                    stack: u32::MAX,
                    parent: 0,
                    geometry_known: false,
                    reason: "ancestor frame".into(),
                },
            ],
            ..chrome
        };
        let ancestor = GuestMetadata {
            rejected: {
                let mut rejected = ancestor.rejected;
                rejected[0].parent = 12;
                rejected
            },
            ..ancestor
        };
        assert!(
            !covered(&ancestor, &target),
            "a parent edge from the direct frame to its ancestor keeps both in the target structure"
        );
        let nested = GuestMetadata {
            rejected: {
                let mut rejected = ancestor.rejected;
                rejected.push(RejectedWindow {
                    window_id: 11,
                    x: 4,
                    y: 4,
                    width: 40,
                    height: 20,
                    class: String::new(),
                    instance: String::new(),
                    stack: u32::MAX,
                    parent: target.window_id,
                    geometry_known: true,
                    reason: "child of the target content".into(),
                });
                rejected
            },
            ..ancestor
        };
        assert!(
            !covered(&nested, &target),
            "a real child edge is part of the target structure"
        );
        let framed = GuestMetadata {
            rejected: {
                let mut rejected = nested.rejected;
                rejected.push(RejectedWindow {
                    window_id: 16,
                    x: 0,
                    y: 0,
                    width: 200,
                    height: 150,
                    class: String::new(),
                    instance: String::new(),
                    stack: u32::MAX,
                    parent: target.parent,
                    geometry_known: true,
                    reason: "plate under the same frame".into(),
                });
                rejected
            },
            ..nested
        };
        assert!(
            !covered(&framed, &target),
            "a sibling under the target frame is part of that frame, not another window"
        );
        let foreign = GuestMetadata {
            rejected: {
                let mut rejected = framed.rejected;
                rejected.push(RejectedWindow {
                    window_id: 17,
                    x: 20,
                    y: 20,
                    width: 40,
                    height: 40,
                    class: String::new(),
                    instance: String::new(),
                    stack: u32::MAX,
                    parent: 0,
                    geometry_known: true,
                    reason: "another top-level window".into(),
                });
                rejected
            },
            ..framed
        };
        assert!(
            covered(&foreign, &target),
            "an overlapping window outside the target parent chain still covers it"
        );
        let shared = GuestMetadata {
            apps: vec![GuestAppInfo {
                bundle_id: FIREFOX_BUNDLE_ID.into(),
                name: "Firefox".into(),
                pid: 25,
                start_token: 99,
                windows: vec![
                    target.clone(),
                    GuestWindow {
                        window_id: 18,
                        generation: 1,
                        x: 0,
                        y: 0,
                        width: 200,
                        height: 150,
                        class: "firefox-esr".into(),
                        instance: "Navigator".into(),
                        stack: 9,
                        parent: target.parent,
                    },
                ],
            }],
            ..foreign
        };
        assert!(
            covered(&shared, &target),
            "another client under the same frame is not the target's chrome"
        );
        let ranked_rejected = GuestMetadata {
            apps: vec![GuestAppInfo {
                bundle_id: FIREFOX_BUNDLE_ID.into(),
                name: "Firefox".into(),
                pid: 25,
                start_token: 99,
                windows: vec![target.clone()],
            }],
            rejected: vec![
                RejectedWindow {
                    window_id: target.parent,
                    x: 0,
                    y: 0,
                    width: 220,
                    height: 180,
                    class: String::new(),
                    instance: String::new(),
                    stack: 0,
                    parent: 0,
                    geometry_known: true,
                    reason: "target frame".into(),
                },
                RejectedWindow {
                    window_id: 19,
                    x: 0,
                    y: 0,
                    width: 200,
                    height: 150,
                    class: "other-app".into(),
                    instance: "Other".into(),
                    stack: 9,
                    parent: target.parent,
                    geometry_known: true,
                    reason: "ranked client under the same frame".into(),
                },
            ],
            epoch: 1,
        };
        assert!(
            covered(&ranked_rejected, &target),
            "a ranked rejected client under the same frame is not the target's chrome"
        );

        let lost = press_outcome(GuestError::Unavailable("action receipt lost".into())).unwrap();
        assert_eq!(lost, SemanticOutcome::UnknownEffect);
        let declined = press_outcome(GuestError::Unavailable(
            "at-spi action was declined: UnknownMethod".into(),
        ))
        .unwrap_err();
        assert!(
            matches!(declined, TargetError::ProbeUnavailable(message) if message.contains("declined"))
        );
    }
}
