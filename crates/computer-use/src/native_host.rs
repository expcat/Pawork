//! Shared native computer-use host.
//!
//! One registry is shared by the GUI, the model tool, and child cores.
//! Ledger checks happen under the registry lock. AX walks, captures, launches
//! and input run only after that lock is dropped, then the result is adopted
//! only if the scope is still open and a for-run grant was not revoked.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::approval::{DispatchPermit, GrantKind, TargetIdentity};
use crate::target::{
    lookup_element, require_background, ActivityProof, AppFamily, AppIdentity, AxTreeRead,
    ElementHandle, ElementQuery, Environment, ImagePoint, NativeActionKind, NativeProbe,
    ObservationGeometry, ObservationHandle, ObservationKind, ProcessInstance, Scope,
    SemanticAction, SemanticOutcome, TargetError, TargetObservation, TargetRegistry, TargetState,
    TargetedInput, TargetedOutcome, ValidatedElement, ValidatedObservationDispatch,
    ValidatedWindow, ValidatedWindowDispatch, WindowHandle, WindowIdentity, WindowMove,
};
use crate::MAX_IMAGE_EDGE;

#[cfg(not(target_os = "macos"))]
use crate::target::UnsupportedProbe;

/// Application metadata the host keeps. Process identity stays here so the
/// model-facing detail string never has to carry a pid.
#[derive(Clone, Debug, PartialEq)]
pub struct AppSnapshot {
    pub identity: AppIdentity,
    pub instance: ProcessInstance,
    pub localized_name: String,
    pub window_count: usize,
}

/// One standard window. `window` is the backend identity, not a model handle.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowSnapshot {
    pub window: WindowIdentity,
    pub width: f64,
    pub height: f64,
    pub title: Option<String>,
}

/// A fresh frame. Returned to the model only when a new observation lease
/// was actually issued.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturedFrame {
    pub image_width: u32,
    pub image_height: u32,
    pub window_width: f64,
    pub window_height: f64,
    pub jpeg: Vec<u8>,
}

/// Platform operations the host calls without holding the registry lock.
pub trait NativeSurface: NativeProbe + Clone + Send + Sync + 'static {
    fn activity(&self, bundle_id: &str) -> ActivityProof;
    fn discover(&self) -> Vec<AppSnapshot>;
    /// `Ok(None)` means this surface answered and the bundle is not installed.
    /// `Err` means it could not answer. Callers must not report that as
    /// "not installed". Target authorization is a later check.
    fn resolve(&self, bundle_id: &str) -> Result<Option<AppIdentity>, TargetError>;
    fn list_windows(
        &self,
        identity: &AppIdentity,
        instance: &ProcessInstance,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<WindowSnapshot>, TargetError>;
    fn launch(
        &self,
        identity: &AppIdentity,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ProcessInstance, TargetError>;
    fn capture(
        &self,
        validated: &ValidatedWindow,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<CapturedFrame, TargetError>;
    fn read_tree(
        &self,
        validated: &ValidatedWindow,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<AxTreeRead, TargetError>;
    fn semantic(
        &self,
        validated: &ValidatedElement,
        path: &[u32],
        action: &SemanticAction,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SemanticOutcome, TargetError>;
    fn targeted(
        &self,
        validated: &ValidatedElement,
        path: &[u32],
        input: &TargetedInput,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<TargetedOutcome, TargetError>;
    fn move_window(
        &self,
        validated: &ValidatedWindowDispatch,
        origin: &WindowMove,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SemanticOutcome, TargetError>;

    /// Where observations from this surface are recorded. The local host stays
    /// on the native background. An explicit isolated host overrides this.
    fn observation_environment(&self) -> Environment {
        Environment::NativeBackground
    }

    /// Gate before a native action. The local host uses the measured background
    /// matrix. The isolated host does not: it only admits actions it has proved.
    fn admit_action(&self, family: AppFamily, action: NativeActionKind) -> Result<(), TargetError> {
        require_background(family, action, false)
    }

    /// Payload gate for seat input bound to a screenshot (CU-26), checked
    /// before the lease or grant is spent. Only a surface that owns a
    /// dedicated seat admits anything.
    fn admit_observed(&self, _action: &ObservedAction) -> Result<(), TargetError> {
        Err(TargetError::BackgroundUnsupported)
    }

    fn observed_input(
        &self,
        _validated: &ValidatedObservationDispatch,
        _action: &ObservedAction,
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<ObservedOutcome, TargetError> {
        Err(TargetError::BackgroundUnsupported)
    }
}

/// Seat input made against one screenshot observation (CU-26). A click
/// lands at the image point carried in the request; keys go to whatever
/// the guest proves is the focused target window.
#[derive(Clone, Debug, PartialEq)]
pub enum ObservedAction {
    Click,
    Keys(TargetedInput),
}

impl ObservedAction {
    pub fn kind(&self) -> NativeActionKind {
        match self {
            Self::Click => NativeActionKind::Pointer,
            Self::Keys(input) => input.kind(),
        }
    }
}

/// `Dispatched` proves the seat received every step, not the target's
/// reaction. `Partial` left nothing pressed; `UnknownEffect` lost the
/// receipt after the request went out. Neither is retried.
#[derive(Debug, PartialEq)]
pub enum ObservedOutcome {
    Dispatched {
        steps: u32,
    },
    Partial {
        steps_sent: u32,
        steps_total: u32,
        cause: TargetError,
    },
    UnknownEffect,
}

/// The surface this process can actually drive.
///
/// Public so NativeHost::host can name it. The platform value is private:
/// this Rust rejects visibility on enum variants, and a public Macos variant
/// could be built with MacosNative::new outside the ledger.
#[derive(Clone)]
pub struct HostSurface {
    kind: HostSurfaceKind,
}

#[derive(Clone)]
enum HostSurfaceKind {
    #[cfg(target_os = "macos")]
    Macos(crate::macos::MacosNative),
    #[cfg(not(target_os = "macos"))]
    Unsupported(UnsupportedProbe),
}

impl HostSurface {
    pub(crate) fn local() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                kind: HostSurfaceKind::Macos(crate::macos::MacosNative::new()),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self {
                kind: HostSurfaceKind::Unsupported(UnsupportedProbe),
            }
        }
    }

    fn probe(&self) -> &dyn NativeProbe {
        match &self.kind {
            #[cfg(target_os = "macos")]
            HostSurfaceKind::Macos(native) => native,
            #[cfg(not(target_os = "macos"))]
            HostSurfaceKind::Unsupported(probe) => probe,
        }
    }
}

impl NativeProbe for HostSurface {
    fn process_instance_alive(&self, instance: &ProcessInstance) -> bool {
        self.probe().process_instance_alive(instance)
    }
    fn window_generation(&self, instance: &ProcessInstance, window_id: u64) -> Option<u64> {
        self.probe().window_generation(instance, window_id)
    }
    fn tree_revision(&self, window: &WindowIdentity) -> Option<u64> {
        self.probe().tree_revision(window)
    }
    fn current_origin(&self, window: &WindowIdentity) -> Option<String> {
        self.probe().current_origin(window)
    }
    fn window_size(&self, instance: &ProcessInstance, window_id: u64) -> Option<(f64, f64)> {
        self.probe().window_size(instance, window_id)
    }
}

impl NativeSurface for HostSurface {
    fn activity(&self, bundle_id: &str) -> ActivityProof {
        match &self.kind {
            #[cfg(target_os = "macos")]
            HostSurfaceKind::Macos(native) => native.target_activity(bundle_id),
            #[cfg(not(target_os = "macos"))]
            HostSurfaceKind::Unsupported(_) => ActivityProof::Unprovable,
        }
    }

    fn discover(&self) -> Vec<AppSnapshot> {
        match &self.kind {
            #[cfg(target_os = "macos")]
            HostSurfaceKind::Macos(native) => native
                .list_applications()
                .into_iter()
                .map(|app| AppSnapshot {
                    identity: app.identity,
                    instance: app.instance,
                    localized_name: app.localized_name,
                    window_count: app.window_count,
                })
                .collect(),
            #[cfg(not(target_os = "macos"))]
            HostSurfaceKind::Unsupported(_) => Vec::new(),
        }
    }

    fn resolve(&self, bundle_id: &str) -> Result<Option<AppIdentity>, TargetError> {
        match &self.kind {
            #[cfg(target_os = "macos")]
            HostSurfaceKind::Macos(native) => Ok(native.resolve_identity(bundle_id)),
            #[cfg(not(target_os = "macos"))]
            HostSurfaceKind::Unsupported(_) => Ok(None),
        }
    }

    fn list_windows(
        &self,
        identity: &AppIdentity,
        instance: &ProcessInstance,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<WindowSnapshot>, TargetError> {
        match &self.kind {
            #[cfg(target_os = "macos")]
            HostSurfaceKind::Macos(native) => Ok(native
                .list_windows_detached(identity, instance, false, cancelled)?
                .into_iter()
                .map(|window| WindowSnapshot {
                    window: window.window,
                    width: window.bounds.width,
                    height: window.bounds.height,
                    title: window.title,
                })
                .collect()),
            #[cfg(not(target_os = "macos"))]
            HostSurfaceKind::Unsupported(_) => Err(unavailable()),
        }
    }

    fn launch(
        &self,
        identity: &AppIdentity,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ProcessInstance, TargetError> {
        match &self.kind {
            #[cfg(target_os = "macos")]
            HostSurfaceKind::Macos(native) => match native.launch_detached(identity, cancelled) {
                Ok(instance) => Ok(instance),
                Err(crate::macos::LaunchError::Denied(error)) => Err(error),
                Err(crate::macos::LaunchError::NotFound(_)) => Err(TargetError::Invalid(
                    "no installed application resolves this bundle id",
                )),
                Err(crate::macos::LaunchError::Failed(message)) => {
                    Err(TargetError::ProbeUnavailable(message))
                }
                Err(crate::macos::LaunchError::ExitedEarly) => Err(TargetError::ProcessRestarted),
            },
            #[cfg(not(target_os = "macos"))]
            HostSurfaceKind::Unsupported(_) => Err(unavailable()),
        }
    }

    fn capture(
        &self,
        validated: &ValidatedWindow,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<CapturedFrame, TargetError> {
        match &self.kind {
            #[cfg(target_os = "macos")]
            HostSurfaceKind::Macos(native) => {
                let capture = native.capture_window_detached(validated, cancelled)?;
                Ok(CapturedFrame {
                    image_width: capture.image_width,
                    image_height: capture.image_height,
                    window_width: capture.window_width,
                    window_height: capture.window_height,
                    jpeg: capture.jpeg,
                })
            }
            #[cfg(not(target_os = "macos"))]
            HostSurfaceKind::Unsupported(_) => Err(unavailable()),
        }
    }

    fn read_tree(
        &self,
        validated: &ValidatedWindow,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<AxTreeRead, TargetError> {
        match &self.kind {
            #[cfg(target_os = "macos")]
            HostSurfaceKind::Macos(native) => native.read_ax_tree_detached(validated, cancelled),
            #[cfg(not(target_os = "macos"))]
            HostSurfaceKind::Unsupported(_) => Err(unavailable()),
        }
    }

    fn semantic(
        &self,
        validated: &ValidatedElement,
        path: &[u32],
        action: &SemanticAction,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SemanticOutcome, TargetError> {
        match &self.kind {
            #[cfg(target_os = "macos")]
            HostSurfaceKind::Macos(native) => {
                native.semantic_action_detached(validated, path, action, cancelled)
            }
            #[cfg(not(target_os = "macos"))]
            HostSurfaceKind::Unsupported(_) => Err(unavailable()),
        }
    }

    fn targeted(
        &self,
        validated: &ValidatedElement,
        path: &[u32],
        input: &TargetedInput,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<TargetedOutcome, TargetError> {
        match &self.kind {
            #[cfg(target_os = "macos")]
            HostSurfaceKind::Macos(native) => {
                native.targeted_input_detached(validated, path, input, cancelled)
            }
            #[cfg(not(target_os = "macos"))]
            HostSurfaceKind::Unsupported(_) => Err(unavailable()),
        }
    }

    fn move_window(
        &self,
        validated: &ValidatedWindowDispatch,
        origin: &WindowMove,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SemanticOutcome, TargetError> {
        match &self.kind {
            #[cfg(target_os = "macos")]
            HostSurfaceKind::Macos(native) => {
                native.move_window_detached(validated, origin, cancelled)
            }
            #[cfg(not(target_os = "macos"))]
            HostSurfaceKind::Unsupported(_) => Err(unavailable()),
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn unavailable() -> TargetError {
    TargetError::ProbeUnavailable("native computer use is not available on this platform".into())
}

/// One model-visible native operation. Strings only: JSON is assembled by the tool.
#[derive(Clone, Debug)]
pub enum NativeRequest {
    Discover,
    Request {
        bundle_id: String,
        origin: Option<String>,
    },
    Bind {
        bundle_id: String,
    },
    Launch {
        bundle_id: String,
    },
    Screenshot {
        window: String,
    },
    ReadTree {
        window: String,
    },
    FindElement {
        observation: String,
        role: Option<String>,
        name: Option<String>,
    },
    Semantic {
        element: String,
        action: SemanticAction,
    },
    Targeted {
        element: String,
        input: TargetedInput,
    },
    MoveWindow {
        window: String,
        x: f64,
        y: f64,
    },
    Pointer {
        window: String,
    },
    /// CU-26: left click at an image point of a screenshot observation.
    ObservedClick {
        observation: String,
        x: f64,
        y: f64,
    },
    /// CU-26: text or one key press while a screenshot observation is current.
    ObservedKeys {
        observation: String,
        input: TargetedInput,
    },
}

/// Typed result. `jpeg` is set only when this call issued a new observation lease.
#[derive(Clone, Debug)]
pub struct NativeOutput {
    pub detail: String,
    pub input_dispatched: bool,
    pub observation_id: Option<String>,
    pub window_id: Option<String>,
    pub element_id: Option<String>,
    pub observation_failure: Option<String>,
    pub outcome: Option<String>,
    pub jpeg: Option<Vec<u8>>,
}

/// Object-safe entry the tool holds. The concrete host stays shared behind `Arc`.
pub trait NativePerform: Send + Sync {
    fn environment_name(&self) -> &'static str {
        "native_background"
    }
    fn background_matrix(&self) -> &'static str {
        "applied"
    }
    fn isolated_desktop(&self) -> bool {
        false
    }
    fn execute(
        &self,
        scope: &Scope,
        request: NativeRequest,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<NativeOutput, TargetError>;
}

/// Facts sampled while the registry lock is not held. Commit paths pass this
/// value into the `*_with` methods, so the lock never walks the platform and
/// a newer generation is never written onto an old handle.
#[derive(Clone, Debug)]
pub struct ProbeSnapshot {
    instance: ProcessInstance,
    window: WindowIdentity,
    alive: bool,
    generation: Option<u64>,
    tree: Option<u64>,
    origin: Option<String>,
    size: Option<(f64, f64)>,
    read_tree: bool,
}

impl ProbeSnapshot {
    /// `read_tree` is only for element consumption. Validate, bind, begin,
    /// and window dispatch pass `false` and do not call `tree_revision`.
    pub fn capture(
        probe: &dyn NativeProbe,
        instance: &ProcessInstance,
        window: &WindowIdentity,
        read_tree: bool,
    ) -> Self {
        let alive = probe.process_instance_alive(instance);
        let generation = probe.window_generation(instance, window.window_id);
        let tree = if read_tree {
            probe.tree_revision(window)
        } else {
            None
        };
        Self {
            instance: instance.clone(),
            window: *window,
            alive,
            generation,
            tree,
            origin: probe.current_origin(window),
            size: probe.window_size(instance, window.window_id),
            read_tree,
        }
    }
}

impl NativeProbe for ProbeSnapshot {
    fn process_instance_alive(&self, instance: &ProcessInstance) -> bool {
        self.alive && instance == &self.instance
    }

    fn window_generation(&self, instance: &ProcessInstance, window_id: u64) -> Option<u64> {
        (instance == &self.instance && window_id == self.window.window_id)
            .then_some(self.generation)
            .flatten()
    }

    fn tree_revision(&self, window: &WindowIdentity) -> Option<u64> {
        if !self.read_tree || window != &self.window {
            None
        } else {
            self.tree
        }
    }

    fn current_origin(&self, window: &WindowIdentity) -> Option<String> {
        (window == &self.window)
            .then(|| self.origin.clone())
            .flatten()
    }

    fn window_size(&self, instance: &ProcessInstance, window_id: u64) -> Option<(f64, f64)> {
        (instance == &self.instance && window_id == self.window.window_id)
            .then_some(self.size)
            .flatten()
    }
}

/// Object-safe ledger control. The same host is the tool executor: `Arc<dyn
/// NativeControl>` upcasts to `Arc<dyn NativePerform>`.
pub trait NativeControl: NativePerform {
    fn open_scope(&self, scope: &Scope);
    fn close_scope(&self, scope: &Scope);
    fn grant(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
        kind: GrantKind,
    ) -> Result<(), TargetError>;
    fn revoke(&self, scope: &Scope, target: &TargetIdentity);
    fn grant_kind(&self, scope: &Scope, target: &TargetIdentity) -> Option<GrantKind>;
    fn target_states(&self) -> Vec<TargetState>;
    /// Record an application the run wants approved. Does not resolve a
    /// bundle, launch a process, bind a window, or call the probe.
    /// Website targets stay fail-closed inside the registry.
    fn note_requested_application(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
    ) -> Result<(), TargetError>;
    fn active_scopes(&self) -> Vec<Scope>;
    /// CU-10 #5：同步等待 GUI 目标审批（同锁谓词 + Condvar 唤醒）。
    /// 语义见 NativeHost::wait_for_requested_grant。
    fn wait_for_requested_grant(
        &self,
        scope: &Scope,
        bundle_id: &str,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    );
}

/// Shared registry state: the ledger mutex plus the condvar that wakes
/// approval waiters after grant / revoke / close_scope mutations.
struct RegistryState<S: NativeProbe> {
    inner: Mutex<TargetRegistry<S>>,
    changed: Condvar,
}

impl<S: NativeProbe> RegistryState<S> {
    fn lock(&self) -> MutexGuard<'_, TargetRegistry<S>> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn try_lock(&self) -> Option<MutexGuard<'_, TargetRegistry<S>>> {
        match self.inner.try_lock() {
            Ok(registry) => Some(registry),
            Err(std::sync::TryLockError::Poisoned(poison)) => Some(poison.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }
}

/// Registry plus the surface clone that performs detached calls.
///
/// The surface stored in the registry and `surface` are clones of one value.
/// On macOS those clones share the window-generation ledger.
pub struct NativeHost<S: NativeSurface> {
    registry: Arc<RegistryState<S>>,
    surface: S,
}

impl NativeHost<HostSurface> {
    pub fn host() -> Self {
        Self::new(HostSurface::local())
    }
}

impl NativeHost<crate::isolated::GuestSurface> {
    /// Explicit isolated desktop. This does not replace [Self::host].
    pub fn isolated() -> Self {
        Self::new(crate::isolated::GuestSurface::endpoint())
    }
}

impl NativeHost<crate::wayland_surface::WaylandSurface> {
    /// Explicit CU-26 Wayland guest. This does not replace [Self::host].
    pub fn wayland() -> Self {
        Self::new(crate::wayland_surface::WaylandSurface::endpoint())
    }
}

impl<S: NativeSurface> NativeHost<S> {
    pub fn new(surface: S) -> Self {
        Self {
            registry: Arc::new(RegistryState {
                inner: Mutex::new(TargetRegistry::new(surface.clone())),
                changed: Condvar::new(),
            }),
            surface,
        }
    }

    pub fn open_scope(&self, scope: &Scope) {
        self.lock().open_scope(scope);
    }

    pub fn close_scope(&self, scope: &Scope) {
        let mut registry = self.lock();
        registry.close_scope(scope);
        self.registry.changed.notify_all();
    }

    pub fn grant(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
        kind: GrantKind,
    ) -> Result<(), TargetError> {
        let mut registry = self.lock();
        let outcome = registry.grant(scope, target, kind);
        self.registry.changed.notify_all();
        outcome
    }

    pub fn revoke(&self, scope: &Scope, target: &TargetIdentity) {
        let mut registry = self.lock();
        registry.revoke(scope, target);
        self.registry.changed.notify_all();
    }

    pub fn grant_kind(&self, scope: &Scope, target: &TargetIdentity) -> Option<GrantKind> {
        self.lock().grant_kind(scope, target)
    }

    pub fn target_states(&self) -> Vec<TargetState> {
        self.lock().target_states()
    }

    /// Record an application target without resolving or launching it.
    /// Production tool dispatch still resolves installed apps through
    /// [Self::request_target]. This entry exists so a fixture can place a
    /// simulated identity into the same registry the GUI later lists.
    pub fn note_requested_application(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
    ) -> Result<(), TargetError> {
        self.lock().request_target(scope, target)
    }

    pub fn active_scopes(&self) -> Vec<Scope> {
        self.lock().active_scopes()
    }

    pub fn execute(
        &self,
        scope: &Scope,
        request: NativeRequest,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<NativeOutput, TargetError> {
        match request {
            NativeRequest::Discover => self.discover(scope),
            NativeRequest::Request { bundle_id, origin } => {
                self.request_target(scope, &bundle_id, origin.as_deref())
            }
            NativeRequest::Bind { bundle_id } => {
                self.bind_or_launch(scope, &bundle_id, false, cancelled)
            }
            NativeRequest::Launch { bundle_id } => {
                self.bind_or_launch(scope, &bundle_id, true, cancelled)
            }
            NativeRequest::Screenshot { window } => self.screenshot(scope, &window, cancelled),
            NativeRequest::ReadTree { window } => self.read_tree(scope, &window, cancelled),
            NativeRequest::FindElement {
                observation,
                role,
                name,
            } => self.find_element(scope, &observation, role.as_deref(), name.as_deref()),
            NativeRequest::Semantic { element, action } => {
                self.semantic(scope, &element, &action, cancelled)
            }
            NativeRequest::Targeted { element, input } => {
                self.targeted(scope, &element, &input, cancelled)
            }
            NativeRequest::MoveWindow { window, x, y } => {
                self.move_window(scope, &window, WindowMove { x, y }, cancelled)
            }
            NativeRequest::Pointer { window } => self.reject_pointer(scope, &window),
            NativeRequest::ObservedClick { observation, x, y } => self.observed(
                scope,
                &observation,
                Some(ImagePoint { x, y }),
                ObservedAction::Click,
                cancelled,
            ),
            NativeRequest::ObservedKeys { observation, input } => self.observed(
                scope,
                &observation,
                None,
                ObservedAction::Keys(input),
                cancelled,
            ),
        }
    }

    fn lock(&self) -> MutexGuard<'_, TargetRegistry<S>> {
        self.registry.lock()
    }

    /// CU-10 #5：等待 GUI 目标审批。只在真实 requested pair（同 scope 且
    /// bundle id 全等）恰好一条、授权检查仍为 NotAuthorized 时等待；grant /
    /// revoke / close_scope 在同一把锁内 notify_all 唤醒。取消由调用方闭包
    /// 提供，经 250ms 短超时轮询，不占工具执行预算。等待结束（无论原因）
    /// 后返回，由随后的 bind/launch 执行自行解析终态。
    pub fn wait_for_requested_grant(
        &self,
        scope: &Scope,
        bundle_id: &str,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) {
        const WAIT_SLICE: std::time::Duration = std::time::Duration::from_millis(250);
        let state = &*self.registry;
        let mut registry = state.lock();
        loop {
            if cancelled() {
                return;
            }
            if !registry.scope_is_active(scope) {
                return;
            }
            let matches = registry.requested_by_bundle(scope, bundle_id);
            if matches.len() != 1 {
                return;
            }
            let target = matches[0].clone();
            match registry.authorizer().check(scope, &target) {
                Ok(()) => return,
                Err(TargetError::NotAuthorized) => {}
                Err(_) => return,
            }
            let (guard, outcome) = state
                .changed
                .wait_timeout(registry, WAIT_SLICE)
                .unwrap_or_else(|poison| poison.into_inner());
            registry = guard;
            let _ = outcome;
        }
    }

    fn discover(&self, scope: &Scope) -> Result<NativeOutput, TargetError> {
        self.require_scope(scope)?;
        let apps = self.surface.discover();
        let detail = if apps.is_empty() {
            "no regular applications".to_string()
        } else {
            apps.iter()
                .map(|app| {
                    format!(
                        "{} {} {} windows={}",
                        app.localized_name,
                        app.identity.bundle_id,
                        family_name(app.identity.family),
                        app.window_count
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(info(detail))
    }

    /// `Ok(None)` from the surface is "not installed". A probe or connection
    /// error stays that error.
    fn resolve_installed(&self, bundle_id: &str) -> Result<AppIdentity, TargetError> {
        self.surface.resolve(bundle_id)?.ok_or(TargetError::Invalid(
            "no installed application resolves this bundle id",
        ))
    }

    fn request_target(
        &self,
        scope: &Scope,
        bundle_id: &str,
        origin: Option<&str>,
    ) -> Result<NativeOutput, TargetError> {
        self.require_scope(scope)?;
        if origin.is_some() {
            return Err(TargetError::Invalid(
                "website targets have no current-origin signal and stay fail-closed",
            ));
        }
        model_bundle(bundle_id)?;
        let identity = self.resolve_installed(bundle_id)?;
        let target = TargetIdentity::application(identity);
        self.lock().request_target(scope, &target)?;
        Ok(info(format!("requested {}", target.app().bundle_id)))
    }

    fn bind_or_launch(
        &self,
        scope: &Scope,
        bundle_id: &str,
        launch: bool,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<NativeOutput, TargetError> {
        self.require_scope(scope)?;
        model_bundle(bundle_id)?;
        let resolved = self.resolve_installed(bundle_id)?;
        let target = TargetIdentity::application(resolved.clone());
        {
            let registry = self.lock();
            self.require_scope_locked(&registry, scope)?;
            registry.authorizer().check(scope, &target)?;
        }
        let watch = self.watches_for_run(scope, &target);
        let matches: Vec<AppSnapshot> = self
            .surface
            .discover()
            .into_iter()
            .filter(|app| app.identity.bundle_id.eq_ignore_ascii_case(bundle_id))
            .collect();
        if matches.len() > 1 {
            return Err(TargetError::Invalid(
                "multiple live instances of this application; refusing to guess",
            ));
        }
        let instance = if launch {
            if !matches.is_empty() {
                return Err(TargetError::Invalid(
                    "application already has a live instance",
                ));
            }
            self.surface.launch(&resolved, &|| {
                self.should_cancel(scope, &target, watch, cancelled)
            })?
        } else {
            let Some(found) = matches.into_iter().next() else {
                return Err(TargetError::Invalid("no live instance of this application"));
            };
            found.instance
        };
        if self.should_cancel(scope, &target, watch, cancelled) {
            return Err(TargetError::Cancelled);
        }
        let windows = self.surface.list_windows(&resolved, &instance, &|| {
            self.should_cancel(scope, &target, watch, cancelled)
        })?;
        if windows.is_empty() {
            return Err(TargetError::Invalid(
                "the application does not have a live window",
            ));
        }
        if windows.len() > 1 {
            return self.bind_listed(scope, &target, &instance, &windows, watch, cancelled);
        }
        let window = windows[0].window;
        if self.should_cancel(scope, &target, watch, cancelled) {
            return Err(TargetError::Cancelled);
        }
        let snapshot = self.snapshot(&instance, &window, false);
        let mut registry = self.lock();
        if self.revoked(&registry, scope, &target, watch) || cancelled() {
            return Err(TargetError::Cancelled);
        }
        let handle = registry.bind_window_with(scope, target, instance, window, &snapshot)?;
        let mut output = info(format!("bound {}", handle.as_str()));
        output.window_id = Some(handle.as_str().to_string());
        Ok(output)
    }

    /// One host handle per live window, issued in the same registry. The
    /// result does not pick a window. The model passes one handle back.
    /// Lines are `handle widthxheight` only: no title, pid, or window id.
    fn bind_listed(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
        instance: &ProcessInstance,
        windows: &[WindowSnapshot],
        watch: bool,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<NativeOutput, TargetError> {
        if self.should_cancel(scope, target, watch, cancelled) {
            return Err(TargetError::Cancelled);
        }
        let snapshot = self.snapshot(instance, &windows[0].window, false);
        if self.should_cancel(scope, target, watch, cancelled) {
            return Err(TargetError::Cancelled);
        }
        let mut registry = self.lock();
        if self.revoked(&registry, scope, target, watch) || cancelled() {
            return Err(TargetError::Cancelled);
        }
        let mut lines = Vec::with_capacity(windows.len());
        for window in windows {
            let handle = registry.bind_window_with(
                scope,
                target.clone(),
                instance.clone(),
                window.window,
                &snapshot,
            )?;
            lines.push(format!(
                "{} {}x{}",
                handle.as_str(),
                window.width,
                window.height
            ));
        }
        Ok(info(lines.join("\n")))
    }

    fn screenshot(
        &self,
        scope: &Scope,
        window: &str,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<NativeOutput, TargetError> {
        let handle = WindowHandle::from_model(window);
        let (instance, identity) = {
            let registry = self.lock();
            self.require_scope_locked(&registry, scope)?;
            registry.window_probe_inputs(&handle, scope)?
        };
        let snapshot = self.snapshot(&instance, &identity, false);
        let validated = {
            let registry = self.lock();
            registry.validate_window_with(&handle, scope, &snapshot)?
        };
        let target = validated.target.clone();
        let watch = self.watches_for_run(scope, &target);
        let frame = self.surface.capture(&validated, &|| {
            self.should_cancel(scope, &target, watch, cancelled)
        })?;
        self.adopt_capture(scope, &handle, &target, watch, cancelled, frame, None)
    }

    fn read_tree(
        &self,
        scope: &Scope,
        window: &str,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<NativeOutput, TargetError> {
        let handle = WindowHandle::from_model(window);
        let (instance, identity) = {
            let registry = self.lock();
            self.require_scope_locked(&registry, scope)?;
            registry.window_probe_inputs(&handle, scope)?
        };
        let snapshot = self.snapshot(&instance, &identity, false);
        let validated = {
            let registry = self.lock();
            registry.validate_window_with(&handle, scope, &snapshot)?
        };
        let target = validated.target.clone();
        let watch = self.watches_for_run(scope, &target);
        let tree = self.surface.read_tree(&validated, &|| {
            self.should_cancel(scope, &target, watch, cancelled)
        })?;
        if self.should_cancel(scope, &target, watch, cancelled) {
            return Err(TargetError::Cancelled);
        }
        let commit = self.snapshot(&validated.instance, &validated.window, false);
        let (width, height) = commit
            .window_size(&validated.instance, validated.window.window_id)
            .ok_or(TargetError::WindowReplaced)?;
        let mut registry = self.lock();
        if self.revoked(&registry, scope, &target, watch) || cancelled() {
            return Err(TargetError::Cancelled);
        }
        let geometry = geometry_for(1, 1, width, height)?;
        let observation = registry.begin_observation_with(
            &handle,
            scope,
            self.surface.observation_environment(),
            ObservationKind::AxTree {
                tree_revision: tree.tree_revision,
            },
            geometry,
            &commit,
        )?;
        let detail = tree_listing(&tree);
        registry.note_tree(&observation.observation_id, scope, tree)?;
        let mut output = observation_output(&observation, None);
        output.detail = detail;
        Ok(output)
    }

    fn find_element(
        &self,
        scope: &Scope,
        observation: &str,
        role: Option<&str>,
        name: Option<&str>,
    ) -> Result<NativeOutput, TargetError> {
        let id = ObservationHandle::from_model(observation);
        let query = ElementQuery::new(role, name)?;
        let mut registry = self.lock();
        self.require_scope_locked(&registry, scope)?;
        let tree = registry.stored_tree(&id, scope)?;
        let index = match lookup_element(&tree, &query) {
            crate::target::ElementLookup::Unique { index } => index,
            crate::target::ElementLookup::NoMatch { .. } => {
                return Err(TargetError::Invalid("no element matched the query"));
            }
            crate::target::ElementLookup::Ambiguous { .. } => {
                return Err(TargetError::Invalid(
                    "more than one element matched the query",
                ));
            }
            crate::target::ElementLookup::UnprovenUnique { .. } => {
                return Err(TargetError::Invalid(
                    "the matching element is not unique in a provable tree",
                ));
            }
        };
        let path = tree
            .nodes
            .get(index)
            .ok_or(TargetError::Invalid(
                "element lookup index was outside the tree",
            ))?
            .path
            .clone();
        let handle = registry.issue_element_with_path(&id, scope, index as u64, path)?;
        let mut output = info(format!(
            "element {} role {} name {}",
            handle.as_str(),
            tree.nodes[index].role,
            tree.nodes[index].name.as_deref().unwrap_or("")
        ));
        output.element_id = Some(handle.as_str().to_string());
        let window = registry.observation_window(&id, scope)?;
        output.window_id = Some(window.as_str().to_string());
        Ok(output)
    }

    fn semantic(
        &self,
        scope: &Scope,
        element: &str,
        action: &SemanticAction,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<NativeOutput, TargetError> {
        let handle = ElementHandle::from_model(element);
        let target = self.prepared_element(scope, &handle, action.kind())?;
        self.yield_if_user(scope, &target)?;
        let watch = self.watches_for_run(scope, &target);
        let (instance, identity) = {
            let registry = self.lock();
            self.ready_to_dispatch(&registry, scope, &target, cancelled)?;
            registry.element_probe_inputs(&handle, scope, &|| false)?
        };
        let snapshot = self.snapshot(&instance, &identity, true);
        self.yield_if_user(scope, &target)?;
        let (validated, path) = self.commit_element(scope, &handle, &snapshot, cancelled)?;
        let permit = validated.dispatch_permit.clone();
        let ticket = self.arm_watch(scope, &target, permit.clone());
        let surface = self.surface.clone();
        let dispatched = self.platform_dispatch(scope, &target, &permit, cancelled, |stop| {
            surface.semantic(&validated, &path, action, stop)
        });
        drop(ticket);
        let (outcome, _) = dispatched?;
        let label = match outcome {
            SemanticOutcome::Dispatched => "dispatched",
            SemanticOutcome::UnknownEffect => "unknown_effect",
        };
        Ok(self.observe_after(scope, &validated.window, &target, watch, label, cancelled))
    }

    fn targeted(
        &self,
        scope: &Scope,
        element: &str,
        input: &TargetedInput,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<NativeOutput, TargetError> {
        let handle = ElementHandle::from_model(element);
        let target = self.prepared_element(scope, &handle, input.kind())?;
        self.yield_if_user(scope, &target)?;
        let watch = self.watches_for_run(scope, &target);
        let (instance, identity) = {
            let registry = self.lock();
            self.ready_to_dispatch(&registry, scope, &target, cancelled)?;
            registry.element_probe_inputs(&handle, scope, &|| false)?
        };
        let snapshot = self.snapshot(&instance, &identity, true);
        self.yield_if_user(scope, &target)?;
        let (validated, path) = self.commit_element(scope, &handle, &snapshot, cancelled)?;
        let permit = validated.dispatch_permit.clone();
        let ticket = self.arm_watch(scope, &target, permit.clone());
        let surface = self.surface.clone();
        let dispatched = self.platform_dispatch(scope, &target, &permit, cancelled, |stop| {
            surface.targeted(&validated, &path, input, stop)
        });
        drop(ticket);
        let (outcome, stop) = dispatched?;
        let outcome = match outcome {
            TargetedOutcome::Partial {
                events_posted,
                events_total,
                cause: TargetError::Cancelled,
            } => TargetedOutcome::Partial {
                events_posted,
                events_total,
                cause: stop
                    .map(DispatchStop::error)
                    .unwrap_or(TargetError::Cancelled),
            },
            other => other,
        };
        let label = match outcome {
            TargetedOutcome::Dispatched { .. } => "dispatched".to_string(),
            TargetedOutcome::Partial {
                events_posted,
                events_total,
                cause,
            } => format!("partial {events_posted}/{events_total}: {cause}"),
        };
        Ok(self.observe_after(scope, &validated.window, &target, watch, &label, cancelled))
    }

    fn move_window(
        &self,
        scope: &Scope,
        window: &str,
        origin: WindowMove,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<NativeOutput, TargetError> {
        let handle = WindowHandle::from_model(window);
        let target = {
            let registry = self.lock();
            self.require_scope_locked(&registry, scope)?;
            let target = registry.window_target(&handle, scope)?;
            registry.authorizer().check(scope, &target)?;
            self.surface
                .admit_action(target.app().family, origin.kind())?;
            target
        };
        self.yield_if_user(scope, &target)?;
        let watch = self.watches_for_run(scope, &target);
        let (instance, identity) = {
            let registry = self.lock();
            self.ready_to_dispatch(&registry, scope, &target, cancelled)?;
            registry.window_probe_inputs(&handle, scope)?
        };
        let snapshot = self.snapshot(&instance, &identity, false);
        self.yield_if_user(scope, &target)?;
        let validated = {
            let mut registry = self.lock();
            if cancelled() {
                return Err(TargetError::Cancelled);
            }
            let validated =
                registry.consume_window_dispatch_with(&handle, scope, &|| false, &snapshot)?;
            registry.authorizer_mut().consume_dispatch_permit(
                &validated.window.scope,
                &validated.window.target,
                &validated.dispatch_permit,
            )?;
            validated
        };
        let permit = validated.dispatch_permit.clone();
        let ticket = self.arm_watch(scope, &target, permit.clone());
        let surface = self.surface.clone();
        let dispatched = self.platform_dispatch(scope, &target, &permit, cancelled, |stop| {
            surface.move_window(&validated, &origin, stop)
        });
        drop(ticket);
        let (outcome, _) = dispatched?;
        let label = match outcome {
            SemanticOutcome::Dispatched => "dispatched",
            SemanticOutcome::UnknownEffect => "unknown_effect",
        };
        Ok(self.observe_after(scope, &validated.window, &target, watch, label, cancelled))
    }

    /// Same chain as a window move: ledger and gates before any probe, facts
    /// read outside the lock, one armed permit per consumed screenshot, and
    /// a fresh observation after the seat reports delivery.
    fn observed(
        &self,
        scope: &Scope,
        observation: &str,
        point: Option<ImagePoint>,
        action: ObservedAction,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<NativeOutput, TargetError> {
        if let ObservedAction::Keys(input) = &action {
            input.validate()?;
        }
        let id = ObservationHandle::from_model(observation);
        let (target, window) = {
            let registry = self.lock();
            self.require_scope_locked(&registry, scope)?;
            let window = registry.observation_window(&id, scope)?;
            let target = registry.window_target(&window, scope)?;
            registry.authorizer().check(scope, &target)?;
            self.surface
                .admit_action(target.app().family, action.kind())?;
            self.surface.admit_observed(&action)?;
            (target, window)
        };
        self.yield_if_user(scope, &target)?;
        let watch = self.watches_for_run(scope, &target);
        let (instance, identity) = {
            let registry = self.lock();
            self.ready_to_dispatch(&registry, scope, &target, cancelled)?;
            registry.window_probe_inputs(&window, scope)?
        };
        let snapshot = self.snapshot(&instance, &identity, false);
        self.yield_if_user(scope, &target)?;
        let validated = {
            let mut registry = self.lock();
            if cancelled() {
                return Err(TargetError::Cancelled);
            }
            let validated = registry.consume_observation_dispatch_with(
                &id,
                scope,
                point,
                &|| false,
                &snapshot,
            )?;
            registry.authorizer_mut().consume_dispatch_permit(
                &validated.observation.window.scope,
                &validated.observation.window.target,
                &validated.dispatch_permit,
            )?;
            validated
        };
        let permit = validated.dispatch_permit.clone();
        let ticket = self.arm_watch(scope, &target, permit.clone());
        let surface = self.surface.clone();
        let dispatched = self.platform_dispatch(scope, &target, &permit, cancelled, |stop| {
            surface.observed_input(&validated, &action, stop)
        });
        drop(ticket);
        let (outcome, stop) = dispatched?;
        let label = match outcome {
            ObservedOutcome::Dispatched { .. } => "dispatched".to_string(),
            ObservedOutcome::Partial {
                steps_sent,
                steps_total,
                cause,
            } => {
                let cause = match cause {
                    TargetError::Cancelled => stop
                        .map(DispatchStop::error)
                        .unwrap_or(TargetError::Cancelled),
                    other => other,
                };
                format!("partial {steps_sent}/{steps_total}: {cause}")
            }
            ObservedOutcome::UnknownEffect => "unknown_effect".to_string(),
        };
        Ok(self.observe_after(
            scope,
            &validated.observation.window,
            &target,
            watch,
            &label,
            cancelled,
        ))
    }

    fn reject_pointer(&self, scope: &Scope, window: &str) -> Result<NativeOutput, TargetError> {
        let handle = WindowHandle::from_model(window);
        let registry = self.lock();
        self.require_scope_locked(&registry, scope)?;
        let target = registry.window_target(&handle, scope)?;
        self.surface
            .admit_action(target.app().family, NativeActionKind::Pointer)?;
        Err(TargetError::BackgroundUnsupported)
    }

    fn prepared_element(
        &self,
        scope: &Scope,
        element: &ElementHandle,
        kind: NativeActionKind,
    ) -> Result<TargetIdentity, TargetError> {
        let registry = self.lock();
        self.require_scope_locked(&registry, scope)?;
        let target = registry.element_target(element, scope)?;
        registry.authorizer().check(scope, &target)?;
        self.surface.admit_action(target.app().family, kind)?;
        Ok(target)
    }

    fn yield_if_user(&self, scope: &Scope, target: &TargetIdentity) -> Result<(), TargetError> {
        let proof = self.surface.activity(&target.app().bundle_id);
        let mut registry = self.lock();
        self.require_scope_locked(&registry, scope)?;
        match proof {
            ActivityProof::Operating => {
                registry.pause_dispatch(target);
                Err(TargetError::UserActive)
            }
            ActivityProof::Unprovable => {
                registry.pause_dispatch(target);
                Err(TargetError::ProbeUnavailable(
                    "user activity could not be proven; dispatch paused".into(),
                ))
            }
            ActivityProof::Idle => {
                registry.resume_dispatch(target);
                Ok(())
            }
        }
    }

    fn observe_after(
        &self,
        scope: &Scope,
        validated: &ValidatedWindow,
        target: &TargetIdentity,
        watch_for_run: bool,
        outcome: &str,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> NativeOutput {
        self.observe_bound(scope, validated, target, watch_for_run, outcome, cancelled)
    }

    fn observe_bound(
        &self,
        scope: &Scope,
        validated: &ValidatedWindow,
        target: &TargetIdentity,
        watch_for_run: bool,
        outcome: &str,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> NativeOutput {
        if self.should_cancel(scope, target, watch_for_run, cancelled) {
            return dispatched_failure(outcome, "cancelled before the post-action observation");
        }
        if self.lock().authorizer().check(scope, target).is_err() {
            return dispatched_failure(outcome, &TargetError::NotAuthorized.to_string());
        }
        let handle = match self.handle_for(scope, validated) {
            Ok(handle) => handle,
            Err(error) => return dispatched_failure(outcome, &error.to_string()),
        };
        let inputs = {
            let registry = self.lock();
            registry.window_probe_inputs(&handle, scope)
        };
        let inputs = match inputs {
            Ok(inputs) => inputs,
            Err(error) => return dispatched_failure(outcome, &error.to_string()),
        };
        let snapshot = self.snapshot(&inputs.0, &inputs.1, false);
        if let Err(error) = self.lock().validate_window_with(&handle, scope, &snapshot) {
            return dispatched_failure(outcome, &error.to_string());
        }
        let frame = match self.surface.capture(validated, &|| {
            self.should_cancel(scope, target, watch_for_run, cancelled)
        }) {
            Ok(frame) => frame,
            Err(error) => return dispatched_failure(outcome, &error.to_string()),
        };
        match self.adopt_capture(
            scope,
            &handle,
            target,
            watch_for_run,
            cancelled,
            frame,
            Some(outcome),
        ) {
            Ok(mut output) => {
                output.input_dispatched = true;
                output.outcome = Some(outcome.to_string());
                output
            }
            Err(error) => dispatched_failure(outcome, &error.to_string()),
        }
    }

    fn adopt_capture(
        &self,
        scope: &Scope,
        handle: &WindowHandle,
        target: &TargetIdentity,
        watch_for_run: bool,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
        frame: CapturedFrame,
        outcome: Option<&str>,
    ) -> Result<NativeOutput, TargetError> {
        let (instance, identity) = {
            let registry = self.lock();
            if cancelled() || self.revoked(&registry, scope, target, watch_for_run) {
                return Err(TargetError::Cancelled);
            }
            registry.window_probe_inputs(handle, scope)?
        };
        let snapshot = self.snapshot(&instance, &identity, false);
        let mut registry = self.lock();
        if cancelled() || self.revoked(&registry, scope, target, watch_for_run) {
            return Err(TargetError::Cancelled);
        }
        let geometry = geometry_for(
            frame.image_width,
            frame.image_height,
            frame.window_width,
            frame.window_height,
        )?;
        let observation = registry.begin_observation_with(
            handle,
            scope,
            self.surface.observation_environment(),
            ObservationKind::Capture,
            geometry,
            &snapshot,
        )?;
        let mut output = observation_output(&observation, Some(frame.jpeg));
        output.outcome = outcome.map(str::to_string);
        Ok(output)
    }

    fn snapshot(
        &self,
        instance: &ProcessInstance,
        window: &WindowIdentity,
        read_tree: bool,
    ) -> ProbeSnapshot {
        ProbeSnapshot::capture(&self.surface, instance, window, read_tree)
    }

    /// Re-check the ledger after the element probe. A close removes the
    /// handle (`UnknownHandle`). A revoke fails in `consume_element_with`
    /// before `spend_for_dispatch`, so the semantic backend is not called.
    fn commit_element(
        &self,
        scope: &Scope,
        handle: &ElementHandle,
        snapshot: &ProbeSnapshot,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<(ValidatedElement, Vec<u32>), TargetError> {
        let mut registry = self.lock();
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let validated = registry.consume_element_with(handle, scope, &|| false, snapshot)?;
        let path = registry.element_path(handle, scope)?;
        registry.authorizer_mut().consume_dispatch_permit(
            &validated.window.scope,
            &validated.window.target,
            &validated.dispatch_permit,
        )?;
        Ok((validated, path))
    }

    fn handle_for(
        &self,
        scope: &Scope,
        validated: &ValidatedWindow,
    ) -> Result<WindowHandle, TargetError> {
        self.lock()
            .issued_window(
                scope,
                &validated.target,
                &validated.instance,
                &validated.window,
            )
            .ok_or(TargetError::UnknownHandle)
    }

    fn ready_to_dispatch(
        &self,
        registry: &TargetRegistry<S>,
        scope: &Scope,
        target: &TargetIdentity,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<(), TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        self.require_scope_locked(registry, scope)?;
        if registry.dispatch_is_paused(target) {
            return Err(TargetError::UserActive);
        }
        Ok(())
    }

    fn require_scope(&self, scope: &Scope) -> Result<(), TargetError> {
        self.require_scope_locked(&self.lock(), scope)
    }

    fn require_scope_locked(
        &self,
        registry: &TargetRegistry<S>,
        scope: &Scope,
    ) -> Result<(), TargetError> {
        if registry.scope_is_active(scope) {
            Ok(())
        } else {
            Err(TargetError::ScopeClosed)
        }
    }

    fn watches_for_run(&self, scope: &Scope, target: &TargetIdentity) -> bool {
        self.lock().grant_kind(scope, target) == Some(GrantKind::ForRun)
    }

    fn should_cancel(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
        watch_for_run: bool,
        external: &(dyn Fn() -> bool + Send + Sync),
    ) -> bool {
        if external() {
            return true;
        }
        let Some(registry) = self.try_registry() else {
            return true;
        };
        self.revoked(&registry, scope, target, watch_for_run)
    }

    fn revoked(
        &self,
        registry: &TargetRegistry<S>,
        scope: &Scope,
        target: &TargetIdentity,
        watch_for_run: bool,
    ) -> bool {
        if !registry.scope_is_active(scope) || registry.dispatch_is_paused(target) {
            return true;
        }
        watch_for_run && registry.grant_kind(scope, target) != Some(GrantKind::ForRun)
    }

    fn try_registry(&self) -> Option<MutexGuard<'_, TargetRegistry<S>>> {
        self.registry.try_lock()
    }

    fn arm_watch(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
        permit: DispatchPermit,
    ) -> DispatchWatch<S> {
        DispatchWatch {
            registry: Arc::clone(&self.registry),
            scope: scope.clone(),
            target: target.clone(),
            permit,
        }
    }

    fn dispatch_stop(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
        permit: &DispatchPermit,
        external: &(dyn Fn() -> bool + Send + Sync),
    ) -> Option<DispatchStop> {
        if external() {
            return Some(DispatchStop::External);
        }
        match self.surface.activity(&target.app().bundle_id) {
            ActivityProof::Operating => {
                if let Some(mut registry) = self.try_registry() {
                    registry.pause_dispatch(target);
                }
                Some(DispatchStop::UserActive)
            }
            ActivityProof::Unprovable => {
                if let Some(mut registry) = self.try_registry() {
                    registry.pause_dispatch(target);
                }
                Some(DispatchStop::Unprovable)
            }
            ActivityProof::Idle => {
                let Some(registry) = self.try_registry() else {
                    return Some(DispatchStop::AuthUnavailable);
                };
                if !registry.scope_is_active(scope) {
                    return Some(DispatchStop::ScopeClosed);
                }
                if registry.dispatch_is_paused(target) {
                    return Some(DispatchStop::UserActive);
                }
                if registry
                    .authorizer()
                    .inflight_current(scope, target, permit)
                    .is_err()
                {
                    return Some(DispatchStop::NotAuthorized);
                }
                None
            }
        }
    }

    fn platform_dispatch<T>(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
        permit: &DispatchPermit,
        external: &(dyn Fn() -> bool + Send + Sync),
        invoke: impl FnOnce(&dyn Fn() -> bool) -> Result<T, TargetError>,
    ) -> Result<(T, Option<DispatchStop>), TargetError> {
        let reason = std::sync::Arc::new(Mutex::new(None));
        enum Raw<T> {
            Stopped,
            Done(Result<T, TargetError>),
        }
        let raw = {
            let reason_stop = std::sync::Arc::clone(&reason);
            let stop = || -> bool {
                let Some(found) = self.dispatch_stop(scope, target, permit, external) else {
                    return false;
                };
                let mut slot = reason_stop
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                if slot.is_none() {
                    *slot = Some(found);
                }
                true
            };
            if stop() {
                Raw::Stopped
            } else {
                Raw::Done(invoke(&stop))
            }
        };
        let recorded = *reason.lock().unwrap_or_else(|poison| poison.into_inner());
        match raw {
            Raw::Stopped => Err(recorded.unwrap_or(DispatchStop::AuthUnavailable).error()),
            Raw::Done(Ok(value)) => Ok((value, recorded)),
            Raw::Done(Err(TargetError::Cancelled)) => {
                Err(recorded.unwrap_or(DispatchStop::External).error())
            }
            Raw::Done(Err(error)) => Err(error),
        }
    }
}

impl<S: NativeSurface> NativePerform for NativeHost<S> {
    fn environment_name(&self) -> &'static str {
        match self.surface.observation_environment() {
            Environment::IsolatedDesktop => "isolated_desktop",
            Environment::NativeBackground => "native_background",
        }
    }

    fn background_matrix(&self) -> &'static str {
        match self.surface.observation_environment() {
            Environment::IsolatedDesktop => "not_applied",
            Environment::NativeBackground => "applied",
        }
    }

    fn isolated_desktop(&self) -> bool {
        self.surface.observation_environment() == Environment::IsolatedDesktop
    }

    fn execute(
        &self,
        scope: &Scope,
        request: NativeRequest,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<NativeOutput, TargetError> {
        NativeHost::execute(self, scope, request, cancelled)
    }
}

impl<S: NativeSurface> NativeControl for NativeHost<S> {
    fn open_scope(&self, scope: &Scope) {
        NativeHost::open_scope(self, scope);
    }

    fn close_scope(&self, scope: &Scope) {
        NativeHost::close_scope(self, scope);
    }

    fn grant(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
        kind: GrantKind,
    ) -> Result<(), TargetError> {
        NativeHost::grant(self, scope, target, kind)
    }

    fn revoke(&self, scope: &Scope, target: &TargetIdentity) {
        NativeHost::revoke(self, scope, target);
    }

    fn grant_kind(&self, scope: &Scope, target: &TargetIdentity) -> Option<GrantKind> {
        NativeHost::grant_kind(self, scope, target)
    }

    fn target_states(&self) -> Vec<TargetState> {
        NativeHost::target_states(self)
    }

    fn note_requested_application(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
    ) -> Result<(), TargetError> {
        NativeHost::note_requested_application(self, scope, target)
    }

    fn active_scopes(&self) -> Vec<Scope> {
        NativeHost::active_scopes(self)
    }

    fn wait_for_requested_grant(
        &self,
        scope: &Scope,
        bundle_id: &str,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) {
        NativeHost::wait_for_requested_grant(self, scope, bundle_id, cancelled);
    }
}

#[derive(Clone, Copy)]
enum DispatchStop {
    External,
    UserActive,
    Unprovable,
    NotAuthorized,
    ScopeClosed,
    AuthUnavailable,
}

impl DispatchStop {
    fn error(self) -> TargetError {
        match self {
            Self::External => TargetError::Cancelled,
            Self::UserActive => TargetError::UserActive,
            Self::Unprovable => TargetError::ProbeUnavailable(
                "user activity could not be proven; dispatch paused".into(),
            ),
            Self::NotAuthorized => TargetError::NotAuthorized,
            Self::ScopeClosed => TargetError::ScopeClosed,
            Self::AuthUnavailable => {
                TargetError::ProbeUnavailable("native authorization state is unavailable".into())
            }
        }
    }
}

struct DispatchWatch<S: NativeSurface> {
    registry: Arc<RegistryState<S>>,
    scope: Scope,
    target: TargetIdentity,
    permit: DispatchPermit,
}

impl<S: NativeSurface> Drop for DispatchWatch<S> {
    fn drop(&mut self) {
        let mut registry = self.registry.lock();
        registry
            .authorizer_mut()
            .retire_inflight(&self.scope, &self.target, &self.permit);
    }
}

const TREE_LISTING_LIMIT: usize = 512 * 1024;

fn tree_listing(tree: &AxTreeRead) -> String {
    let mut listing = format!(
        "truncation depth={} nodes={} text={} names={}\n",
        tree.truncation.depth, tree.truncation.nodes, tree.truncation.text, tree.truncation.names
    );
    for node in &tree.nodes {
        let line = format!(
            "path={} role={} name={} children={} name_truncated={} name_unreadable={} depth_limited={}\n",
            path_text(&node.path),
            node.role,
            node.name.as_deref().unwrap_or(""),
            node.children,
            node.name_truncated,
            node.name_unreadable,
            node.depth_limited
        );
        if listing.len().saturating_add(line.len()) > TREE_LISTING_LIMIT {
            listing.push_str("listing truncated\n");
            break;
        }
        listing.push_str(&line);
    }
    listing
}

fn path_text(path: &[u32]) -> String {
    let mut text = String::from("[");
    for (index, part) in path.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        text.push_str(&part.to_string());
    }
    text.push(']');
    text
}

fn info(detail: impl Into<String>) -> NativeOutput {
    NativeOutput {
        detail: detail.into(),
        input_dispatched: false,
        observation_id: None,
        window_id: None,
        element_id: None,
        observation_failure: None,
        outcome: None,
        jpeg: None,
    }
}

fn observation_output(observation: &TargetObservation, jpeg: Option<Vec<u8>>) -> NativeOutput {
    NativeOutput {
        detail: format!("observation {}", observation.observation_id.as_str()),
        input_dispatched: false,
        observation_id: Some(observation.observation_id.as_str().to_string()),
        window_id: Some(observation.window.as_str().to_string()),
        element_id: None,
        observation_failure: None,
        outcome: None,
        jpeg,
    }
}

fn dispatched_failure(outcome: &str, failure: &str) -> NativeOutput {
    NativeOutput {
        detail: failure.to_string(),
        input_dispatched: true,
        observation_id: None,
        window_id: None,
        element_id: None,
        observation_failure: Some(failure.to_string()),
        outcome: Some(outcome.to_string()),
        jpeg: None,
    }
}

fn geometry_for(
    image_width: u32,
    image_height: u32,
    window_width: f64,
    window_height: f64,
) -> Result<ObservationGeometry, TargetError> {
    if image_width == 0
        || image_height == 0
        || image_width > MAX_IMAGE_EDGE
        || image_height > MAX_IMAGE_EDGE
    {
        return Err(TargetError::Invalid(
            "observation image is outside its budget",
        ));
    }
    Ok(ObservationGeometry {
        image_width,
        image_height,
        window_width,
        window_height,
    })
}

fn family_name(family: AppFamily) -> &'static str {
    match family {
        AppFamily::Appkit => "appkit",
        AppFamily::Chromium => "chromium",
        AppFamily::Browser => "browser",
    }
}

fn model_bundle(bundle_id: &str) -> Result<(), TargetError> {
    if bundle_id.trim().is_empty()
        || bundle_id.starts_with('/')
        || bundle_id.contains('/')
        || bundle_id.contains('\\')
        || bundle_id.chars().all(|ch| ch.is_ascii_digit())
    {
        return Err(TargetError::Invalid(
            "model input must be a bundle id or a host-issued handle, not a pid or path",
        ));
    }
    Ok(())
}
