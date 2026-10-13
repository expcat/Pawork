//! Host target registry: validated handles, leases, and occupancy.

use super::*;

struct WindowRecord {
    target: TargetIdentity,
    instance: ProcessInstance,
    window: WindowIdentity,
    scope: Scope,
}

pub(super) struct ObservationRecord {
    window: WindowHandle,
    scope: Scope,
    observation: TargetObservation,
    tree_revision: Option<u64>,
    pub(super) created: Instant,
    consumed: bool,
    /// AX read this observation was issued from, kept so a later element
    /// lookup does not re-enter the backend and bind a newer tree.
    tree: Option<AxTreeRead>,
}

struct ElementRecord {
    observation: ObservationHandle,
    window: WindowHandle,
    scope: Scope,
    element_token: u64,
    /// Child-index path from the read that issued this handle. The model
    /// never sees it; dispatch re-resolves only this path.
    path: Vec<u32>,
    tree_revision: u64,
    consumed: bool,
}

/// A window binding that passed all liveness checks just now.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedWindow {
    pub target: TargetIdentity,
    pub instance: ProcessInstance,
    pub window: WindowIdentity,
    pub scope: Scope,
}

/// A consumed observation lease: exactly one action may dispatch from it.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedObservation {
    pub window: ValidatedWindow,
    pub observation: TargetObservation,
    pub tree_revision: Option<u64>,
}

/// A consumed screenshot lease for one seat input (CU-26). `point` is the
/// image point already mapped to window points; keys carry none.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedObservationDispatch {
    pub observation: ValidatedObservation,
    pub point: Option<WindowPoint>,
    pub dispatch_permit: DispatchPermit,
}

/// A consumed element handle: exactly one semantic action may dispatch.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedElement {
    pub window: ValidatedWindow,
    /// Host-side token the backend maps to its native element reference.
    pub element_token: u64,
    pub tree_revision: u64,
    /// Single-use credential for the one dispatch this consume released
    /// (CU-06): the backend's first dispatch attempt retires it, so a
    /// cloned validation result can never dispatch twice.
    pub dispatch_permit: DispatchPermit,
}

/// A consumed window-dispatch lease (CU-08): exactly one window-level
/// action (window move) may dispatch, bound to the single-use permit its
/// consume minted. The window binding itself stays live — it is not a
/// lease — so the same bound window can dispatch again after a fresh
/// consume.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedWindowDispatch {
    pub window: ValidatedWindow,
    pub dispatch_permit: DispatchPermit,
}

/// CU-17: one entry of the registry's target-state view. Recorded facts
/// only (binding / grant / occupancy / pause); liveness is validated on
/// use, so listing never probes the backend.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetState {
    pub target: TargetIdentity,
    pub scope: Scope,
    /// Outstanding grant; None for a bound-but-unauthorized target.
    pub grant: Option<GrantKind>,
    /// Host-issued handle of the bound window, if any.
    pub window: Option<WindowHandle>,
    /// Scope currently owning the application boundary; may differ from
    /// this entry's scope when another run holds the application.
    pub occupied_by: Option<Scope>,
    /// CU-09 user-takeover pause state of the application boundary.
    pub dispatch_paused: bool,
}

/// Host-side registry: the only issuer and validator of handles. Validation
/// order on every use is cancellation, existence, scope, staleness,
/// authorization (CU-16), process instance, window generation, tree
/// revision, window size; a one-shot grant is spent only after every other
/// check passed.
/// Authorization is checked before any probe call, so a denied, unknown or
/// revoked target never reaches the native backend.
pub struct TargetRegistry<P: NativeProbe> {
    pub(super) probe: Arc<P>,
    pub(super) authorizer: TargetAuthorizer,
    windows: HashMap<String, WindowRecord>,
    pub(super) observations: HashMap<String, ObservationRecord>,
    elements: HashMap<String, ElementRecord>,
    /// CU-09 occupancy: application anchor -> owning scope and last activity.
    /// Input is dispatched to an application process, so the app (or the
    /// browser hosting a website target) is the boundary two runs must not
    /// share. Owners idle past [LEASE_TTL] are treated as done.
    pub(super) occupied: HashMap<String, (Scope, Instant)>,
    /// Applications the user took over (CU-09 host signal): dispatch gates
    /// reject with [TargetError::UserActive] while paused.
    user_paused: HashSet<String>,
    /// CU-17 review: scopes currently registered as active runs. Grants are
    /// only accepted for these; [Self::close_scope] removes the entry, so
    /// approval can never slip between the terminal cleanup and the host
    /// registry removal, and no terminal-state record outlives the run.
    active: HashSet<Scope>,
    /// Targets a run asked the user to approve. Not a grant and not an
    /// occupancy record: the GUI list can show them before the user decides,
    /// and only [Self::grant] marks one authorized.
    requested: HashSet<(Scope, TargetIdentity)>,
}

impl<P: NativeProbe> TargetRegistry<P> {
    pub fn new(probe: P) -> Self {
        Self::with_authorizer(probe, TargetAuthorizer::default())
    }

    pub fn with_authorizer(probe: P, authorizer: TargetAuthorizer) -> Self {
        Self {
            probe: Arc::new(probe),
            authorizer,
            windows: HashMap::new(),
            observations: HashMap::new(),
            elements: HashMap::new(),
            occupied: HashMap::new(),
            user_paused: HashSet::new(),
            active: HashSet::new(),
            requested: HashSet::new(),
        }
    }

    pub fn authorizer(&self) -> &TargetAuthorizer {
        &self.authorizer
    }

    pub fn authorizer_mut(&mut self) -> &mut TargetAuthorizer {
        &mut self.authorizer
    }

    /// CU-17 review: register a run scope as active (run start). Must happen
    /// before the host's run registry lists the run, so an approval that
    /// passes the run check always finds an active scope here.
    pub fn open_scope(&mut self, scope: &Scope) {
        self.active.insert(scope.clone());
    }

    /// Record an explicit user approval for a target in this scope. This is
    /// the host's only way to create a grant; models can never widen it.
    pub fn grant(
        &mut self,
        scope: &Scope,
        target: &TargetIdentity,
        kind: GrantKind,
    ) -> Result<(), TargetError> {
        if !self.active.contains(scope) {
            return Err(TargetError::ScopeClosed);
        }
        self.authorizer.grant(scope, target, kind)
    }

    /// Revoke a target's grant: every later gated operation on it is denied.
    pub fn revoke(&mut self, scope: &Scope, target: &TargetIdentity) {
        // CU-09: revocation keeps the CU-16 contract (in-flight leases are
        // blocked with NotAuthorized before any probe, consumed history stays
        // untouched) and additionally releases the scope's occupancy of the
        // application boundary so another run may take the target.
        if self
            .occupied
            .get(target.occupancy_anchor())
            .is_some_and(|(owner, _)| owner == scope)
        {
            self.occupied.remove(target.occupancy_anchor());
        }
        // CU-10 #5：撤销同时移除待批登记，等待 GUI 审批的 bind/launch
        // 立即失去等待谓词；随后的执行得到现有 NotAuthorized。
        self.requested.remove(&(scope.clone(), target.clone()));
        self.authorizer.revoke(scope, target);
    }

    /// Drop every grant of a run (run teardown).
    pub fn revoke_scope(&mut self, scope: &Scope) {
        self.authorizer.revoke_scope(scope);
        self.release_scope(scope);
    }

    /// CU-17 review: terminal close of a run scope. In addition to the
    /// [Self::revoke_scope] teardown the scope leaves the active registry,
    /// so a concurrent or late approval can never re-mint a grant for a run
    /// that already reached its terminal state. Idempotent; nothing is
    /// retained after the close.
    pub fn close_scope(&mut self, scope: &Scope) {
        self.active.remove(scope);
        self.requested.retain(|(held, _)| held != scope);
        self.revoke_scope(scope);
    }

    /// Record that this run wants the user to approve an application target.
    /// Does not grant, bind, or touch the backend. Website targets stay
    /// fail-closed: there is no current-origin signal to pin them to.
    /// Protected bundles are rejected here so they never appear as choices.
    pub fn request_target(
        &mut self,
        scope: &Scope,
        target: &TargetIdentity,
    ) -> Result<(), TargetError> {
        if !self.active.contains(scope) {
            return Err(TargetError::ScopeClosed);
        }
        target.validate()?;
        if matches!(target, TargetIdentity::Website { .. }) {
            return Err(TargetError::Invalid(
                "website targets have no current-origin signal and stay fail-closed",
            ));
        }
        match self.authorizer.check(scope, target) {
            Err(TargetError::ForbiddenTarget) => return Err(TargetError::ForbiddenTarget),
            Err(TargetError::NotAuthorized) | Ok(()) => {}
            Err(other) => return Err(other),
        }
        self.requested.insert((scope.clone(), target.clone()));
        Ok(())
    }

    pub fn scope_is_active(&self, scope: &Scope) -> bool {
        self.active.contains(scope)
    }

    /// CU-10 #5：某 scope 待批目标里 bundle id 全等匹配的应用目标。
    /// 大小写折叠、origin 或其它身份重建都不参与；返回值供等待方做
    /// 「恰好一条」判定。
    pub fn requested_by_bundle(&self, scope: &Scope, bundle_id: &str) -> Vec<TargetIdentity> {
        self.requested
            .iter()
            .filter(|(held, target)| {
                held == scope
                    && target.app().bundle_id.as_str() == bundle_id
                    && matches!(target, TargetIdentity::Application { .. })
            })
            .map(|(_, target)| target.clone())
            .collect()
    }

    pub fn active_scopes(&self) -> Vec<Scope> {
        let mut scopes: Vec<Scope> = self.active.iter().cloned().collect();
        scopes.sort_by(|left, right| {
            (&left.workspace_id, &left.run_id).cmp(&(&right.workspace_id, &right.run_id))
        });
        scopes
    }

    pub fn dispatch_is_paused(&self, target: &TargetIdentity) -> bool {
        self.user_paused.contains(target.occupancy_anchor())
    }

    pub fn grant_kind(&self, scope: &Scope, target: &TargetIdentity) -> Option<GrantKind> {
        self.authorizer
            .grants()
            .find_map(|((held_scope, held_target), kind)| {
                (held_scope == scope && held_target == target).then_some(*kind)
            })
    }

    /// Bound target identity without probing. Unknown and cross-scope handles
    /// fail before any backend access.
    pub fn window_target(
        &self,
        window: &WindowHandle,
        scope: &Scope,
    ) -> Result<TargetIdentity, TargetError> {
        let record = self
            .windows
            .get(window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        Ok(record.target.clone())
    }

    /// CU-09 host signal: the user is operating this target's application.
    /// The agent yields — dispatch gates pause without consuming leases until
    /// [Self::resume_dispatch] reports the user gave the target back. Hosts
    /// that cannot determine the user's intent keep the pause asserted
    /// (fail-closed) instead of competing for the target.
    pub fn pause_dispatch(&mut self, target: &TargetIdentity) {
        self.user_paused
            .insert(target.occupancy_anchor().to_string());
    }

    /// CU-09 host signal: the user yielded the target back; agent dispatch may
    /// resume. Returns whether a pause was actually cleared.
    pub fn resume_dispatch(&mut self, target: &TargetIdentity) -> bool {
        self.user_paused.remove(target.occupancy_anchor())
    }

    /// CU-09 run end (stop, cancel, timeout, host disconnect): every handle
    /// issued to the scope becomes invalid and its target occupancy is
    /// released. Authorizations are untouched; use [Self::revoke_scope] to
    /// drop them as well.
    pub fn release_scope(&mut self, scope: &Scope) {
        self.observations.retain(|_, record| record.scope != *scope);
        self.elements.retain(|_, record| record.scope != *scope);
        self.windows.retain(|_, record| record.scope != *scope);
        self.occupied.retain(|_, (owner, _)| owner != scope);
    }

    /// CU-17: aggregate view over grants ∪ bound windows for the GUI.
    /// Grants without bindings and bindings without grants both appear;
    /// occupancy and pause attach by application anchor. Deterministic
    /// order (scope, anchor, variant) so repeated calls are stable.
    pub fn target_states(&self) -> Vec<TargetState> {
        let mut states: Vec<TargetState> = self
            .authorizer
            .grants()
            .map(|((scope, target), grant)| TargetState {
                target: target.clone(),
                scope: scope.clone(),
                grant: Some(*grant),
                window: None,
                occupied_by: None,
                dispatch_paused: false,
            })
            .collect();
        // One GUI row still carries one handle. Multiple windows of the same
        // application keep the lowest issued handle so the row does not follow
        // HashMap order; the tool result lists every handle.
        let mut bound: Vec<_> = self.windows.iter().collect();
        bound.sort_by(|left, right| left.0.cmp(right.0));
        for (handle, record) in bound {
            let entry = states
                .iter_mut()
                .find(|state| state.scope == record.scope && state.target == record.target);
            match entry {
                Some(state) => {
                    if state.window.is_none() {
                        state.window = Some(WindowHandle(handle.clone()));
                    }
                }
                None => states.push(TargetState {
                    target: record.target.clone(),
                    scope: record.scope.clone(),
                    grant: None,
                    window: Some(WindowHandle(handle.clone())),
                    occupied_by: None,
                    dispatch_paused: false,
                }),
            }
        }
        for state in &mut states {
            let anchor = state.target.occupancy_anchor();
            if let Some((owner, _)) = self.occupied.get(anchor) {
                state.occupied_by = Some(owner.clone());
            }
            state.dispatch_paused = self.user_paused.contains(anchor);
        }
        for (scope, target) in &self.requested {
            if states
                .iter()
                .any(|state| &state.scope == scope && &state.target == target)
            {
                continue;
            }
            let anchor = target.occupancy_anchor();
            states.push(TargetState {
                target: target.clone(),
                scope: scope.clone(),
                grant: None,
                window: None,
                occupied_by: self.occupied.get(anchor).map(|(owner, _)| owner.clone()),
                dispatch_paused: self.user_paused.contains(anchor),
            });
        }
        states.sort_by(|a, b| {
            (
                a.scope.workspace_id.clone(),
                a.scope.run_id.clone(),
                a.target.occupancy_anchor().to_string(),
            )
                .cmp(&(
                    b.scope.workspace_id.clone(),
                    b.scope.run_id.clone(),
                    b.target.occupancy_anchor().to_string(),
                ))
        });
        states
    }

    /// Pre-flight gate the host must pass before connecting to, launching,
    /// capturing or reading a bound target. Non-consuming: each gated
    /// registry operation re-checks and spends one-shot grants itself.
    /// Ledger denial (NotAuthorized / ForbiddenTarget) precedes any probe
    /// call, and so does the occupancy gate: a window handle whose run lost
    /// the application to another run never reaches an origin probe. Like
    /// [Self::validate_window], the pre-flight never refreshes occupancy;
    /// activity is refreshed only by binding and successful gated use. A
    /// website binding then re-pins the window's current origin, so
    /// a same-window navigation (or an undeterminable origin) fails closed
    /// before the host captures. Liveness re-validation stays in the gated
    /// operations.
    pub fn require_authorized(
        &self,
        window: &WindowHandle,
        scope: &Scope,
    ) -> Result<(), TargetError> {
        let record = self
            .windows
            .get(window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        self.authorizer.check(scope, &record.target)?;
        self.occupancy_conflict(&record.target, scope)?;
        if let TargetIdentity::Website { origin, .. } = &record.target {
            if !origin_matches(self.probe.as_ref(), &record.window, origin) {
                return Err(TargetError::SiteChanged);
            }
        }
        Ok(())
    }

    /// Handle already stored for this target, scope, process and generation.
    /// Map lookup only: no probe and no second ledger.
    pub(crate) fn issued_window(
        &self,
        scope: &Scope,
        target: &TargetIdentity,
        instance: &ProcessInstance,
        window: &WindowIdentity,
    ) -> Option<WindowHandle> {
        self.windows.iter().find_map(|(handle, record)| {
            (record.scope == *scope
                && record.target == *target
                && record.instance == *instance
                && record.window == *window)
                .then(|| WindowHandle(handle.clone()))
        })
    }

    /// Bind a window the host just enumerated for scope. Requires an active
    /// grant for the target and rejects a process that is already gone;
    /// the authorization check runs before any probe access. Binding alone
    /// does not spend a one-shot grant, and occupancy is claimed only after
    /// liveness and origin checks pass — a failed binding never strands
    /// occupancy another run would have to wait out.
    pub fn bind_window(
        &mut self,
        scope: &Scope,
        target: TargetIdentity,
        instance: ProcessInstance,
        window: WindowIdentity,
    ) -> Result<WindowHandle, TargetError> {
        let probe = Arc::clone(&self.probe);
        self.bind_window_with(scope, target, instance, window, probe.as_ref())
    }

    pub fn bind_window_with<Q: NativeProbe>(
        &mut self,
        scope: &Scope,
        target: TargetIdentity,
        instance: ProcessInstance,
        window: WindowIdentity,
        probe: &Q,
    ) -> Result<WindowHandle, TargetError> {
        self.authorizer.check(scope, &target)?;
        self.occupancy_conflict(&target, scope)?;
        if !probe.process_instance_alive(&instance) {
            return Err(TargetError::ProcessRestarted);
        }
        // A website binding pins the origin the window actually shows:
        // claiming a different origin than the current page is rejected
        // before the handle exists.
        if let TargetIdentity::Website { origin, .. } = &target {
            if !origin_matches(probe, &window, origin) {
                return Err(TargetError::SiteChanged);
            }
        }
        self.touch_occupancy(&target, scope);
        // Reuse only after the grant and occupancy gates above.
        if let Some(handle) = self.issued_window(scope, &target, &instance, &window) {
            return Ok(handle);
        }
        let handle = WindowHandle(next_handle("w"));
        self.windows.insert(
            handle.0.clone(),
            WindowRecord {
                target,
                instance,
                window,
                scope: scope.clone(),
            },
        );
        Ok(handle)
    }

    /// Issue a one-shot observation lease for a live bound window. The host
    /// captures/reads first, then registers the geometry it produced.
    pub fn begin_observation(
        &mut self,
        window: &WindowHandle,
        scope: &Scope,
        environment: Environment,
        kind: ObservationKind,
        geometry: ObservationGeometry,
    ) -> Result<TargetObservation, TargetError> {
        let probe = Arc::clone(&self.probe);
        self.begin_observation_with(window, scope, environment, kind, geometry, probe.as_ref())
    }

    pub fn begin_observation_with<Q: NativeProbe>(
        &mut self,
        window: &WindowHandle,
        scope: &Scope,
        environment: Environment,
        kind: ObservationKind,
        geometry: ObservationGeometry,
        probe: &Q,
    ) -> Result<TargetObservation, TargetError> {
        let record = self
            .windows
            .get(window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        geometry.validate()?;
        let bound_target = record.target.clone();
        // Authorization precedes any probe call; the one-shot grant is spent
        // only after the window passed liveness, so a stale target does not
        // burn the user's single approval.
        self.authorizer.check(scope, &record.target)?;
        self.occupancy_conflict(&record.target, scope)?;
        self.live_with(record, probe)?;
        // The registered geometry must still be the window's live size:
        // a capture taken before a resize is born stale.
        self.size_current(
            probe,
            &record.instance,
            &record.window,
            (geometry.window_width, geometry.window_height),
        )?;
        self.authorizer.spend(scope, &record.target)?;
        self.touch_occupancy(&bound_target, scope);
        let observation = TargetObservation {
            observation_id: ObservationHandle(next_handle("o")),
            environment,
            window: window.clone(),
            image_width: geometry.image_width,
            image_height: geometry.image_height,
            window_width: geometry.window_width,
            window_height: geometry.window_height,
        };
        let tree_revision = match kind {
            ObservationKind::Capture => None,
            ObservationKind::AxTree { tree_revision } => Some(tree_revision),
        };
        self.observations.insert(
            observation.observation_id.0.clone(),
            ObservationRecord {
                window: window.clone(),
                scope: scope.clone(),
                observation: observation.clone(),
                tree_revision,
                created: Instant::now(),
                consumed: false,
                tree: None,
            },
        );
        Ok(observation)
    }

    /// Issue an element handle from an unconsumed accessibility-tree
    /// observation. element_token is host-side bookkeeping the backend maps
    /// to its native element reference.
    pub fn issue_element(
        &mut self,
        observation: &ObservationHandle,
        scope: &Scope,
        element_token: u64,
    ) -> Result<ElementHandle, TargetError> {
        self.issue_element_with_path(observation, scope, element_token, Vec::new())
    }

    /// Issue an element handle and remember the child-index path the backend
    /// re-resolves. The path stays host-side.
    pub fn issue_element_with_path(
        &mut self,
        observation: &ObservationHandle,
        scope: &Scope,
        element_token: u64,
        path: Vec<u32>,
    ) -> Result<ElementHandle, TargetError> {
        let record = self
            .observations
            .get(observation.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        if record.consumed || record.created.elapsed() > LEASE_TTL {
            return Err(TargetError::StaleHandle);
        }
        let tree_revision = record.tree_revision.ok_or(TargetError::Invalid(
            "element handles require an accessibility tree observation",
        ))?;
        let handle = ElementHandle(next_handle("e"));
        self.elements.insert(
            handle.0.clone(),
            ElementRecord {
                observation: observation.clone(),
                window: record.window.clone(),
                scope: scope.clone(),
                element_token,
                path,
                tree_revision,
                consumed: false,
            },
        );
        Ok(handle)
    }

    pub fn note_tree(
        &mut self,
        observation: &ObservationHandle,
        scope: &Scope,
        tree: AxTreeRead,
    ) -> Result<(), TargetError> {
        let record = self
            .observations
            .get_mut(observation.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        if record.tree_revision != Some(tree.tree_revision) {
            return Err(TargetError::TreeChanged);
        }
        record.tree = Some(tree);
        Ok(())
    }

    pub fn stored_tree(
        &self,
        observation: &ObservationHandle,
        scope: &Scope,
    ) -> Result<AxTreeRead, TargetError> {
        let record = self
            .observations
            .get(observation.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        record.tree.clone().ok_or(TargetError::Invalid(
            "element lookup needs the accessibility tree stored with this observation",
        ))
    }

    /// Window this observation was issued for. Does not probe or consume.
    pub fn observation_window(
        &self,
        observation: &ObservationHandle,
        scope: &Scope,
    ) -> Result<WindowHandle, TargetError> {
        let record = self
            .observations
            .get(observation.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        Ok(record.window.clone())
    }

    pub fn element_path(
        &self,
        element: &ElementHandle,
        scope: &Scope,
    ) -> Result<Vec<u32>, TargetError> {
        let record = self
            .elements
            .get(element.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        Ok(record.path.clone())
    }

    /// Target of an element handle without consuming it or probing.
    pub(crate) fn element_target(
        &self,
        element: &ElementHandle,
        scope: &Scope,
    ) -> Result<TargetIdentity, TargetError> {
        let record = self
            .elements
            .get(element.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        let window = record.window.clone();
        self.window_target(&window, scope)
    }

    /// Liveness check without consuming anything (status, re-capture, AX
    /// re-read). Requires an active grant, checked before the probe.
    pub fn validate_window(
        &self,
        window: &WindowHandle,
        scope: &Scope,
    ) -> Result<ValidatedWindow, TargetError> {
        self.validate_window_with(window, scope, self.probe.as_ref())
    }

    /// Same ledger checks as [Self::validate_window], using facts read
    /// while the host was not holding the registry lock.
    pub fn validate_window_with<Q: NativeProbe>(
        &self,
        window: &WindowHandle,
        scope: &Scope,
        probe: &Q,
    ) -> Result<ValidatedWindow, TargetError> {
        let record = self
            .windows
            .get(window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        self.authorizer.check(scope, &record.target)?;
        self.occupancy_conflict(&record.target, scope)?;
        self.live_with(record, probe)
    }

    /// Bound identity to re-validate. Authorization is checked first.
    /// Nothing is probed or consumed.
    pub fn window_probe_inputs(
        &self,
        window: &WindowHandle,
        scope: &Scope,
    ) -> Result<(ProcessInstance, WindowIdentity), TargetError> {
        let record = self
            .windows
            .get(window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        self.authorizer.check(scope, &record.target)?;
        self.occupancy_conflict(&record.target, scope)?;
        Ok((record.instance.clone(), record.window))
    }

    /// Consume a one-shot observation lease before dispatching one action
    /// from it. Successful validation consumes the lease even if the later
    /// dispatch fails, and invalidates element handles issued from it.
    pub fn consume_observation(
        &mut self,
        id: &ObservationHandle,
        scope: &Scope,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ValidatedObservation, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let record = self
            .observations
            .get(id.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        if record.consumed || record.created.elapsed() > LEASE_TTL {
            return Err(TargetError::StaleHandle);
        }
        let observation = record.observation.clone();
        let tree_revision = record.tree_revision;
        let window_record = self
            .windows
            .get(record.window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        // Revocation between observation and dispatch blocks the action
        // before any probe access; the grant is spent only when every other
        // check passed and the action is actually released for dispatch.
        self.authorizer.check(scope, &window_record.target)?;
        let dispatch_target = window_record.target.clone();
        self.occupancy_conflict(&dispatch_target, scope)?;
        self.dispatch_paused(&dispatch_target)?;
        let window = self.live(window_record)?;
        // An AX observation is bound to the tree revision it read: if the
        // tree advanced before the lease is consumed, the observation is as
        // stale as its element handles.
        if let Some(issued) = record.tree_revision {
            match self.probe.tree_revision(&window_record.window) {
                None => return Err(TargetError::WindowReplaced),
                Some(revision) if revision != issued => return Err(TargetError::TreeChanged),
                Some(_) => {}
            }
        }
        // A window resized since the observation stales it: coordinates
        // would no longer map onto the observed image.
        self.size_current(
            self.probe.as_ref(),
            &window_record.instance,
            &window_record.window,
            (
                record.observation.window_width,
                record.observation.window_height,
            ),
        )?;
        self.authorizer.spend(scope, &window_record.target)?;
        self.touch_occupancy(&dispatch_target, scope);
        let validated = ValidatedObservation {
            window,
            observation,
            tree_revision,
        };
        self.observations.get_mut(id.as_str()).unwrap().consumed = true;
        Ok(validated)
    }

    /// Consume a screenshot lease for one seat input made against that image
    /// (CU-26): a click at a point of it, or keys while it is current. Same
    /// gate order as [Self::consume_observation] with facts read outside the
    /// lock, and a single-use permit like an element consume. The point is
    /// checked against the image before anything is spent, so a bad
    /// coordinate leaves the lease and the grant untouched.
    pub fn consume_observation_dispatch_with<Q: NativeProbe>(
        &mut self,
        id: &ObservationHandle,
        scope: &Scope,
        point: Option<ImagePoint>,
        cancelled: &dyn Fn() -> bool,
        probe: &Q,
    ) -> Result<ValidatedObservationDispatch, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let record = self
            .observations
            .get(id.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        if record.consumed || record.created.elapsed() > LEASE_TTL {
            return Err(TargetError::StaleHandle);
        }
        if record.tree_revision.is_some() {
            return Err(TargetError::Invalid(
                "seat input needs a screenshot observation, not a tree read",
            ));
        }
        let point = point
            .map(|point| image_to_window(point, &record.observation))
            .transpose()?;
        let observation = record.observation.clone();
        let window_record = self
            .windows
            .get(record.window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        self.authorizer.check(scope, &window_record.target)?;
        let dispatch_target = window_record.target.clone();
        self.occupancy_conflict(&dispatch_target, scope)?;
        self.dispatch_paused(&dispatch_target)?;
        let window = self.live_with(window_record, probe)?;
        self.size_current(
            probe,
            &window_record.instance,
            &window_record.window,
            (observation.window_width, observation.window_height),
        )?;
        let dispatch_permit = self
            .authorizer
            .spend_for_dispatch(scope, &window_record.target)?;
        self.touch_occupancy(&dispatch_target, scope);
        self.observations.get_mut(id.as_str()).unwrap().consumed = true;
        Ok(ValidatedObservationDispatch {
            observation: ValidatedObservation {
                window,
                observation,
                tree_revision: None,
            },
            point,
            dispatch_permit,
        })
    }

    /// Ledger checks that precede an element probe. Does not probe or consume.
    pub fn element_probe_inputs(
        &self,
        id: &ElementHandle,
        scope: &Scope,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(ProcessInstance, WindowIdentity), TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let record = self
            .elements
            .get(id.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        let parent = self
            .observations
            .get(record.observation.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if record.consumed || parent.consumed || parent.created.elapsed() > LEASE_TTL {
            return Err(TargetError::StaleHandle);
        }
        let window_record = self
            .windows
            .get(record.window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        self.authorizer.check(scope, &window_record.target)?;
        self.occupancy_conflict(&window_record.target, scope)?;
        self.dispatch_paused(&window_record.target)?;
        Ok((window_record.instance.clone(), window_record.window))
    }

    pub fn consume_element(
        &mut self,
        id: &ElementHandle,
        scope: &Scope,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ValidatedElement, TargetError> {
        let probe = Arc::clone(&self.probe);
        self.consume_element_with(id, scope, cancelled, probe.as_ref())
    }

    pub fn consume_element_with<Q: NativeProbe>(
        &mut self,
        id: &ElementHandle,
        scope: &Scope,
        cancelled: &dyn Fn() -> bool,
        probe: &Q,
    ) -> Result<ValidatedElement, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let record = self
            .elements
            .get(id.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        let parent = self
            .observations
            .get(record.observation.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if record.consumed || parent.consumed || parent.created.elapsed() > LEASE_TTL {
            return Err(TargetError::StaleHandle);
        }
        let element_token = record.element_token;
        let element_tree_revision = record.tree_revision;
        let parent_key = record.observation.clone();
        let window_record = self
            .windows
            .get(record.window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        self.authorizer.check(scope, &window_record.target)?;
        let dispatch_target = window_record.target.clone();
        self.occupancy_conflict(&dispatch_target, scope)?;
        self.dispatch_paused(&dispatch_target)?;
        let window = self.live_with(window_record, probe)?;
        match probe.tree_revision(&window_record.window) {
            None => return Err(TargetError::WindowReplaced),
            Some(revision) if revision != record.tree_revision => {
                return Err(TargetError::TreeChanged)
            }
            Some(_) => {}
        }
        // Same staleness rule as the parent observation: a resized window
        // invalidates the geometry every element handle was read against.
        self.size_current(
            probe,
            &window_record.instance,
            &window_record.window,
            (
                parent.observation.window_width,
                parent.observation.window_height,
            ),
        )?;
        let dispatch_permit = self
            .authorizer
            .spend_for_dispatch(scope, &window_record.target)?;
        self.touch_occupancy(&dispatch_target, scope);
        let validated = ValidatedElement {
            window,
            element_token,
            tree_revision: element_tree_revision,
            dispatch_permit,
        };
        self.elements.get_mut(id.as_str()).unwrap().consumed = true;
        self.observations
            .get_mut(parent_key.as_str())
            .unwrap()
            .consumed = true;
        Ok(validated)
    }

    /// Consume one window-level dispatch on a live bound window (CU-08:
    /// window move). The window binding stays live — unlike observation
    /// and element leases it is not consumed — but every dispatch needs
    /// its own consume, which re-checks authorization, occupancy and
    /// user takeover, revalidates liveness (process instance, window
    /// generation, website origin), spends the grant and mints the
    /// single-use [DispatchPermit] that authorizes exactly one dispatch.
    pub fn consume_window_dispatch(
        &mut self,
        window: &WindowHandle,
        scope: &Scope,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ValidatedWindowDispatch, TargetError> {
        let probe = Arc::clone(&self.probe);
        self.consume_window_dispatch_with(window, scope, cancelled, probe.as_ref())
    }

    pub fn consume_window_dispatch_with<Q: NativeProbe>(
        &mut self,
        window: &WindowHandle,
        scope: &Scope,
        cancelled: &dyn Fn() -> bool,
        probe: &Q,
    ) -> Result<ValidatedWindowDispatch, TargetError> {
        if cancelled() {
            return Err(TargetError::Cancelled);
        }
        let record = self
            .windows
            .get(window.as_str())
            .ok_or(TargetError::UnknownHandle)?;
        if &record.scope != scope {
            return Err(TargetError::CrossRun);
        }
        self.authorizer.check(scope, &record.target)?;
        let dispatch_target = record.target.clone();
        self.occupancy_conflict(&dispatch_target, scope)?;
        self.dispatch_paused(&dispatch_target)?;
        let window = self.live_with(record, probe)?;
        let dispatch_permit = self.authorizer.spend_for_dispatch(scope, &record.target)?;
        self.touch_occupancy(&dispatch_target, scope);
        Ok(ValidatedWindowDispatch {
            window,
            dispatch_permit,
        })
    }

    fn live(&self, record: &WindowRecord) -> Result<ValidatedWindow, TargetError> {
        self.live_with(record, self.probe.as_ref())
    }

    fn live_with<Q: NativeProbe>(
        &self,
        record: &WindowRecord,
        probe: &Q,
    ) -> Result<ValidatedWindow, TargetError> {
        if !probe.process_instance_alive(&record.instance) {
            return Err(TargetError::ProcessRestarted);
        }
        match probe.window_generation(&record.instance, record.window.window_id) {
            None => return Err(TargetError::WindowReplaced),
            Some(generation) if generation != record.window.generation => {
                return Err(TargetError::WindowReplaced)
            }
            Some(_) => {}
        }
        if let TargetIdentity::Website { origin, .. } = &record.target {
            if !origin_matches(probe, &record.window, origin) {
                return Err(TargetError::SiteChanged);
            }
        }
        Ok(ValidatedWindow {
            target: record.target.clone(),
            instance: record.instance.clone(),
            window: record.window,
            scope: record.scope.clone(),
        })
    }

    /// Check-only half of the occupancy gate, usable while record borrows are
    /// alive: a live foreign owner rejects with [TargetError::TargetOccupied].
    fn occupancy_conflict(
        &self,
        target: &TargetIdentity,
        scope: &Scope,
    ) -> Result<(), TargetError> {
        if let Some((owner, active)) = self.occupied.get(target.occupancy_anchor()) {
            if owner != scope && active.elapsed() <= LEASE_TTL {
                return Err(TargetError::TargetOccupied);
            }
        }
        Ok(())
    }

    /// Refresh or take occupancy for a scope that just passed
    /// [Self::occupancy_conflict]; also drops owners idle past [LEASE_TTL].
    fn touch_occupancy(&mut self, target: &TargetIdentity, scope: &Scope) {
        let anchor = target.occupancy_anchor();
        match self.occupied.get_mut(anchor) {
            Some((owner, active)) if owner == scope => *active = Instant::now(),
            Some((_, active)) if active.elapsed() <= LEASE_TTL => {}
            _ => {
                self.occupied
                    .retain(|_, (_, active)| active.elapsed() <= LEASE_TTL);
                self.occupied
                    .insert(anchor.to_string(), (scope.clone(), Instant::now()));
            }
        }
    }

    /// CU-09 user takeover: dispatch gates reject while the host reports the
    /// user operating the target's application. Nothing is consumed.
    fn dispatch_paused(&self, target: &TargetIdentity) -> Result<(), TargetError> {
        if self.user_paused.contains(target.occupancy_anchor()) {
            Err(TargetError::UserActive)
        } else {
            Ok(())
        }
    }

    /// The observation's recorded window size must still be the live size.
    /// A window the probe cannot prove alive is replaced (fail-closed, same
    /// as a generation miss); a live window of a different size stales the
    /// observation — image coordinates would map onto the wrong points.
    fn size_current(
        &self,
        probe: &impl NativeProbe,
        instance: &ProcessInstance,
        window: &WindowIdentity,
        recorded: (f64, f64),
    ) -> Result<(), TargetError> {
        match probe.window_size(instance, window.window_id) {
            None => Err(TargetError::WindowReplaced),
            Some(current) if !size_matches(recorded, current) => Err(TargetError::StaleHandle),
            Some(_) => Ok(()),
        }
    }
}

/// Two window sizes agree within half a point (the backend's frame
/// tolerance). A sub-half-point resize shifts image mapping by at most one
/// image pixel at the 2x capture scale; anything larger stales the
/// observation.
fn size_matches(recorded: (f64, f64), current: (f64, f64)) -> bool {
    const EPSILON: f64 = 0.5;
    (recorded.0 - current.0).abs() <= EPSILON && (recorded.1 - current.1).abs() <= EPSILON
}

/// A website target is valid only while the window shows the origin it was
/// authorized and bound for, compared in canonical form so casing or port
/// spelling cannot dodge the check.
fn origin_matches<P: NativeProbe>(probe: &P, window: &WindowIdentity, origin: &str) -> bool {
    probe
        .current_origin(window)
        .as_deref()
        .and_then(|value| normalize_origin(value).ok())
        .as_deref()
        == Some(origin)
}
