//! Quota aggregation, caching, singleflight, and partial-failure semantics (P14-6).
//!
//! This module is the assembly point for [`QuotaAdapter`] implementations. It
//! owns a per-(scope, window, unit) cache, deduplicates concurrent fetches via
//! singleflight, and queries multiple windows concurrently — returning the best
//! snapshot by confidence (`Exact > Derived > Scraped`) plus any typed failures
//! observed from supporting adapters. Stale cached data is only ever returned
//! when a fresh fetch fails, and only with [`QuotaRead::served_stale`] raised
//! so the source/confidence/staleness chain stays visible.
//!
//! Stale fallback always rewrites `provenance.stale = true` on the served
//! snapshot so downstream readers (refresh scheduler, audit, UI) can branch on
//! staleness without consulting a separate flag.
//!
//! Credential injection: [`QuotaService::read_with_credential`] /
//! [`QuotaService::overview_with_credential`] thread an optional
//! [`ResolvedCredential`] through to adapters. The credential is borrowed only
//! for the duration of `fetch` and never stored, serialized, or logged. The
//! legacy [`QuotaService::read`] / [`QuotaService::overview`] pass `None` and
//! remain the convenience entry points for anonymous/local-ledger reads.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_domain::{CancellationToken, Timestamp};
use futures::future;
use provider_api::ResolvedCredential;
use tokio::sync::Notify;

use agent_domain::{ModelId, ProviderId, TenantId};

use crate::ledger::LedgerQuotaAdapter;
use crate::{
    AccountId, AdapterKind, Confidence, QuotaAdapter, QuotaError, QuotaScope, QuotaSnapshot,
};

// =========================================================================
// Clock
// =========================================================================

/// Wall-clock source used to stamp cache entries and freshness decisions.
///
/// Centralising time behind a trait keeps every freshness comparison
/// deterministic in tests (see [`MutableQuotaClock`]).
pub trait QuotaClock: Send + Sync {
    /// Current time as Unix milliseconds.
    fn now(&self) -> Timestamp;
}

/// Production clock backed by [`std::time::SystemTime`].
#[derive(Debug, Default)]
pub struct SystemQuotaClock;

impl QuotaClock for SystemQuotaClock {
    fn now(&self) -> Timestamp {
        crate::util::now_millis()
    }
}

/// Mutable test clock backed by an atomic counter; advance freely from tests.
#[derive(Debug, Default)]
pub struct MutableQuotaClock {
    ms: AtomicU64,
}

impl MutableQuotaClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn at(start_ms: u64) -> Self {
        let clock = Self::default();
        clock.ms.store(start_ms, Ordering::Release);
        clock
    }

    pub fn set(&self, ms: u64) {
        self.ms.store(ms, Ordering::Release);
    }

    pub fn advance(&self, by_ms: u64) {
        self.ms.fetch_add(by_ms, Ordering::AcqRel);
    }
}

impl QuotaClock for MutableQuotaClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(self.ms.load(Ordering::Acquire))
    }
}

// =========================================================================
// Scope matching
// =========================================================================

/// Cheap static predicate for routing a request to a registered adapter.
///
/// `None` fields are wildcards. An all-`None` [`ScopeMatch`] matches every
/// scope and is the default; this is the recommended registration when the
/// adapter has its own internal scoping. The adapter's [`QuotaAdapter::supports`]
/// is still consulted after the static match, so capability discovery remains
/// the source of truth for per-window/unit support.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScopeMatch {
    pub tenant_id: Option<TenantId>,
    pub account_id: Option<AccountId>,
    pub provider_id: Option<ProviderId>,
    pub credential_id: Option<String>,
    pub model_id: Option<ModelId>,
}

impl ScopeMatch {
    /// Matches every scope (wildcard on every dimension).
    pub fn any() -> Self {
        Self::default()
    }

    pub fn for_provider(provider_id: ProviderId) -> Self {
        Self {
            provider_id: Some(provider_id),
            ..Self::default()
        }
    }

    /// True iff every `Some` dimension equals the scope's value.
    pub fn matches(&self, scope: &QuotaScope) -> bool {
        if let Some(ref want) = self.tenant_id {
            if want != &scope.tenant_id {
                return false;
            }
        }
        if let Some(ref want) = self.account_id {
            if want != &scope.account_id {
                return false;
            }
        }
        if let Some(ref want) = self.provider_id {
            if want != &scope.provider_id {
                return false;
            }
        }
        if let Some(ref want) = self.credential_id {
            if scope.credential_id.as_deref() != Some(want.as_str()) {
                return false;
            }
        }
        if let Some(ref want) = self.model_id {
            if scope.model_id.as_ref() != Some(want) {
                return false;
            }
        }
        true
    }
}

// =========================================================================
// Read outcomes
// =========================================================================

/// A typed failure observed during an aggregated read.
///
/// `adapter_kind` is `Some` only when a real adapter produced the error.
/// Query-level failures — invalid scope, no candidate adapter, cancelled
/// singleflight, internal exhaustion — carry `None`: no adapter attribution
/// is fabricated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaFailure {
    pub adapter_kind: Option<AdapterKind>,
    pub error: QuotaError,
}

impl QuotaFailure {
    pub fn new(adapter_kind: AdapterKind, error: QuotaError) -> Self {
        Self {
            adapter_kind: Some(adapter_kind),
            error,
        }
    }

    /// Query-level failure with no adapter attribution (scope validation,
    /// no candidate adapter, cancellation, internal exhaustion).
    pub fn domain(error: QuotaError) -> Self {
        Self {
            adapter_kind: None,
            error,
        }
    }
}

/// The successful result of a single-window aggregated read.
///
/// `failures` carries typed errors from adapters that lost the source-priority
/// race (or never produced a usable snapshot) so callers can surface partial
/// degradation. `served_stale` is true when the snapshot came from a stale
/// cache entry because every fresh fetch attempt failed or was cancelled.
#[derive(Clone, Debug)]
pub struct QuotaRead {
    pub snapshot: QuotaSnapshot,
    pub failures: Vec<QuotaFailure>,
    pub served_stale: bool,
}

/// The outcome of one window inside an overview.
//
// Held as a `HashMap` value with at most one entry per `QuotaWindow` (<=4),
// so the `Ok(QuotaRead)` size difference is bounded; boxing would add a heap
// allocation on every overview read of the common path.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum WindowRead {
    /// At least one adapter produced a usable (possibly stale) snapshot.
    Ok(QuotaRead),
    /// Every candidate adapter failed and no stale cache fallback existed.
    /// Per-adapter failures that coexist with a served snapshot stay in
    /// [`QuotaRead::failures`].
    Failed { failures: Vec<QuotaFailure> },
}

/// Aggregated multi-window view for one `(scope, unit)` pair.
#[derive(Clone, Debug)]
pub struct QuotaOverview {
    pub scope: QuotaScope,
    pub windows: HashMap<crate::QuotaWindow, WindowRead>,
}

impl QuotaOverview {
    /// Number of windows that produced at least one snapshot (possibly stale).
    pub fn ok_count(&self) -> usize {
        self.windows
            .values()
            .filter(|r| matches!(r, WindowRead::Ok(_)))
            .count()
    }

    /// Aggregate every failure observed across windows.
    pub fn all_failures(&self) -> Vec<QuotaFailure> {
        let mut out = Vec::new();
        for read in self.windows.values() {
            match read {
                WindowRead::Ok(r) => out.extend(r.failures.clone()),
                WindowRead::Failed { failures } => out.extend(failures.clone()),
            }
        }
        out
    }
}

/// Outcome of a cache-only single-window query ([`QuotaService::read_cache_only`]).
///
/// Cache-only reads never invoke adapters, the network, or singleflight: they
/// consult only the in-process cache and return one of three structured
/// states. This is the read path P14-8's quota query API uses so that a mere
/// UI poll can never trigger a remote fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheRead {
    /// A fresh (within TTL) cached snapshot exists.
    Hit { snapshot: QuotaSnapshot },
    /// A cache entry exists but is older than TTL. Returned instead of serving
    /// potentially-stale data without an explicit refresh decision by the
    /// caller (the read path).
    Stale { snapshot: QuotaSnapshot },
    /// No cache entry exists for this key. Distinct from a fetch failure so
    /// callers can decide whether to trigger a refresh.
    NoData,
}

impl CacheRead {
    /// True if this window has a fresh cached hit.
    pub fn is_hit(&self) -> bool {
        matches!(self, CacheRead::Hit { .. })
    }
}

/// Aggregated cache-only multi-window view for one `(scope, unit)` pair.
///
/// Every window is populated from the cache alone; the absence of a window in
/// `windows` indicates the caller did not request it.
#[derive(Clone, Debug)]
pub struct CacheOverview {
    pub scope: QuotaScope,
    pub windows: HashMap<crate::QuotaWindow, CacheRead>,
}

impl CacheOverview {
    /// Number of windows with a fresh cached hit.
    pub fn hit_count(&self) -> usize {
        self.windows.values().filter(|r| r.is_hit()).count()
    }
}

// =========================================================================
// Internal outcome used by the singleflight cell
// =========================================================================

// Always stored behind `Arc<ReadOutcome>` in the singleflight cell, so the
// variant-size difference (driven by `QuotaSnapshot`) has no stack cost.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
enum ReadOutcome {
    Ok {
        snapshot: QuotaSnapshot,
        failures: Vec<QuotaFailure>,
    },
    AllFailed(Vec<QuotaFailure>),
}

impl ReadOutcome {
    fn snapshot_for_cache(&self) -> Option<&QuotaSnapshot> {
        match self {
            ReadOutcome::Ok { snapshot, .. } => Some(snapshot),
            ReadOutcome::AllFailed(_) => None,
        }
    }
}

// =========================================================================
// Cache
// =========================================================================

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    scope: QuotaScope,
    window: crate::QuotaWindow,
    unit: crate::QuotaUnit,
}

#[derive(Clone, Debug)]
struct CacheEntry {
    snapshot: QuotaSnapshot,
    stored_at: Timestamp,
}

impl CacheEntry {
    fn is_fresh(&self, now: Timestamp, ttl: Duration) -> bool {
        let age_ms = now
            .as_unix_millis()
            .saturating_sub(self.stored_at.as_unix_millis());
        Duration::from_millis(age_ms) < ttl
    }
}

#[derive(Default)]
struct QuotaCache {
    entries: Mutex<HashMap<CacheKey, CacheEntry>>,
}

impl QuotaCache {
    fn get(&self, key: &CacheKey) -> Option<CacheEntry> {
        self.entries
            .lock()
            .expect("quota cache poisoned")
            .get(key)
            .cloned()
    }

    fn put(&self, key: CacheKey, snapshot: QuotaSnapshot, stored_at: Timestamp) {
        self.entries.lock().expect("quota cache poisoned").insert(
            key,
            CacheEntry {
                snapshot,
                stored_at,
            },
        );
    }

    fn clear(&self) {
        self.entries.lock().expect("quota cache poisoned").clear();
    }

    fn len(&self) -> usize {
        self.entries.lock().expect("quota cache poisoned").len()
    }

    /// Every cached snapshot whose full scope matches `scope`, across all
    /// windows and units. Full-scope equality (tenant + account + provider +
    /// model + credential) keeps sibling credentials and accounts isolated.
    /// Both fresh and stale entries are returned; callers decide freshness.
    fn snapshots_for_scope(&self, scope: &QuotaScope) -> Vec<QuotaSnapshot> {
        self.entries
            .lock()
            .expect("quota cache poisoned")
            .iter()
            .filter(|(key, _)| &key.scope == scope)
            .map(|(_, entry)| entry.snapshot.clone())
            .collect()
    }
}

// =========================================================================
// Singleflight
// =========================================================================

/// Deduplicates concurrent fetches for the same key.
///
/// Abort-safe deduplication of concurrent fetches for the same key.
///
/// Semantics required by P14:
/// - **Leader abort is recoverable.** If the leader's future is dropped
///   (caller cancelled its `read` task, panicked, etc.), a [`LeaderGuard`]
///   marks the flight leaderless and wakes followers. The next follower to
///   observe the dead leader promotes itself and re-runs the work, bounded by
///   [`SINGLEFLIGHT_MAX_PROMOTIONS`] to prevent unbounded retry loops.
/// - **Per-caller cancel is isolated.** A *follower* that cancels only bows
///   out of its own wait; the leader (a different caller) keeps running, so one
///   caller's cancel never aborts shared in-flight work for the others. The
///   *leader* that cancels aborts only its own in-flight work, drops its
///   [`LeaderGuard`] (waking followers), and returns `Cancelled`; a follower
///   then promotes and re-runs so the shared result still lands.
/// - **No Mutex guard crosses an await.** Every `.lock()` is released before
///   any `.await`, so a `std::sync::Mutex` cannot deadlock or poison across
///   suspension points.
#[derive(Default)]
struct Singleflight {
    in_flight: Mutex<HashMap<CacheKey, Arc<Flight>>>,
}

/// Upper bound on leader re-promotions for one `run` call. Beyond this, the
/// caller receives [`SingleflightResult::Exhausted`] rather than looping
/// forever when leaders keep dying.
const SINGLEFLIGHT_MAX_PROMOTIONS: u32 = 8;

struct Flight {
    result: Mutex<Option<Arc<ReadOutcome>>>,
    /// `true` while a leader is actively running `work`. Cleared by the
    /// leader's [`LeaderGuard`] on drop (success, error, or abort) so
    /// followers can observe the death and promote.
    leader_alive: AtomicBool,
    notify: Notify,
}

/// Outcome of a singleflight `run` from a caller's perspective.
enum SingleflightResult {
    /// A result (Ok or AllFailed) was published by some leader.
    Outcome(Arc<ReadOutcome>),
    /// The caller's own token fired while it was a follower. Shared work is
    /// untouched.
    CallerCancelled,
    /// Leaders died repeatedly past the promotion bound.
    Exhausted,
}

impl Flight {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            leader_alive: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    /// Atomically claim leadership. Returns `true` if this caller is now the
    /// leader. On a brand-new flight the flag starts `false`, so the first
    /// caller wins; on a leaderless flight (aborted leader, no published
    /// result) a follower steals leadership.
    ///
    /// A flight that already published a result is never claimable. The
    /// completing leader flips `leader_alive` back to `false` *before* it
    /// retires the flight from the map, so the flag alone would make the
    /// flight look claimable during that release→remove window; granting
    /// leadership there would re-run already-finished work and then get the
    /// new leader's flight deleted by the retiring leader's map removal. The
    /// result check runs under the same mutex `publish` stores through, and
    /// the flag CAS stays inside that guard, so "result absent then claim
    /// succeeds" is impossible: a successful CAS implies the previous leader
    /// already published (release strictly follows publish).
    fn try_claim_leader(&self) -> bool {
        let result = self.result.lock().expect("singleflight poisoned");
        if result.is_some() {
            return false;
        }
        self.leader_alive
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Mark the flight leaderless and wake any waiting followers. Called by
    /// [`LeaderGuard::drop`] and on the success path.
    fn release_leader(&self) {
        self.leader_alive.store(false, Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Current published result, if any.
    fn snapshot(&self) -> Option<Arc<ReadOutcome>> {
        self.result.lock().expect("singleflight poisoned").clone()
    }

    /// Publish the result and wake every follower waiting on it.
    fn publish(&self, result: Arc<ReadOutcome>) {
        *self.result.lock().expect("singleflight poisoned") = Some(result);
        self.notify.notify_waiters();
    }

    /// Follower wait: returns when the leader publishes a result, when the
    /// leader dies (so this follower can promote), or when the caller's own
    /// token fires.
    ///
    /// The `Notified` future is registered BEFORE every state check to honor
    /// the `tokio::sync::Notify` contract and avoid lost wakeups. The borrow
    /// of `&self` is contained inside this method (the pinned `Notified`
    /// future borrows `&self` only for the duration of the call), which keeps
    /// the borrow checker from entangling the future's lifetime with the
    /// caller's `Arc<Flight>` handle.
    async fn wait(&self, caller_cancel: &CancellationToken) -> FollowerEvent {
        loop {
            // Register interest BEFORE checking state to avoid lost wakeups.
            let notified = self.notify.notified();
            tokio::pin!(notified);

            if let Some(result) = self.snapshot() {
                return FollowerEvent::Published(result);
            }
            if !self.leader_alive.load(Ordering::Acquire) {
                return FollowerEvent::LeaderDied;
            }

            // Wait for either a notification (publish / leader drop) or the
            // caller's own cancellation. The caller token never reaches the
            // leader's work, so cancelling here cannot abort shared work.
            tokio::select! {
                biased;
                _ = caller_cancel.cancelled() => return FollowerEvent::CallerCancelled,
                _ = &mut notified => {}
            }

            if let Some(result) = self.snapshot() {
                return FollowerEvent::Published(result);
            }
            if !self.leader_alive.load(Ordering::Acquire) {
                return FollowerEvent::LeaderDied;
            }
            // Spurious wakeup or follower-only churn; loop and re-register.
        }
    }
}

/// What a follower observed when its wait resolved.
enum FollowerEvent {
    /// The leader published a result.
    Published(Arc<ReadOutcome>),
    /// The leader died (dropped/aborted); the caller may promote.
    LeaderDied,
    /// The caller's own cancellation token fired.
    CallerCancelled,
}

/// RAII leadership marker. On drop (success, error, or future abort) it
/// releases leadership and wakes followers so they can promote instead of
/// waiting forever on a dead leader.
///
/// This is the crux of abort-safety: if the leader's `work` future is dropped
/// mid-flight (caller task aborted, panic unwind, etc.), the guard's `Drop`
/// still runs, flipping `leader_alive` to `false` and notifying waiters.
struct LeaderGuard {
    flight: Arc<Flight>,
}

impl Drop for LeaderGuard {
    fn drop(&mut self) {
        self.flight.release_leader();
    }
}

impl Singleflight {
    /// Deduplicate `build_work` across concurrent callers for `key`.
    ///
    /// `build_work` is a factory invoked once per leader promotion; it returns
    /// the future that actually performs the (cached) fetch. The leader races
    /// that future against its own `caller_cancel`: if the leader's caller
    /// cancels, the in-flight work future is dropped (fetch aborted), the
    /// [`LeaderGuard`] is dropped (releasing leadership and waking followers),
    /// and this caller returns `CallerCancelled` — a follower then promotes
    /// and re-runs. A *follower* that cancels only exits its own wait; the
    /// leader (a different caller) is untouched. Promotions are bounded so
    /// repeated leader deaths return [`SingleflightResult::Exhausted`] instead
    /// of looping forever.
    async fn run<W, Fut>(
        &self,
        key: CacheKey,
        build_work: W,
        caller_cancel: &CancellationToken,
    ) -> SingleflightResult
    where
        W: Fn() -> Fut + Send,
        Fut: std::future::Future<Output = Arc<ReadOutcome>> + Send,
    {
        let mut promotions = 0u32;
        loop {
            // Acquire the flight under a brief lock; release before any await.
            let flight = {
                let mut state = self.in_flight.lock().expect("singleflight poisoned");
                if let Some(flight) = state.get(&key) {
                    flight.clone()
                } else {
                    let flight = Arc::new(Flight::new());
                    state.insert(key.clone(), flight.clone());
                    flight
                }
            };

            // Claim leadership atomically. A fresh flight starts with
            // leader_alive == false so the first caller wins; a leaderless
            // flight lets a follower steal leadership.
            let is_leader = flight.try_claim_leader();

            if is_leader {
                // Guard releases leadership on drop — including when the
                // leader future is aborted mid-`build_work`. It must outlive
                // the await so an abort still wakes followers to promote.
                let guard = LeaderGuard {
                    flight: flight.clone(),
                };
                // Race the shared work against the leader's own caller token.
                // If the leader's caller cancels, the in-flight `build_work`
                // future is dropped (aborting the fetch) and, after `select!`
                // resolves, we drop `guard` to release leadership and wake
                // followers — one of which promotes and re-runs `build_work`.
                let result = tokio::select! {
                    biased;
                    _ = caller_cancel.cancelled() => None,
                    work = build_work() => Some(work),
                };
                match result {
                    Some(result) => {
                        // Publish BEFORE releasing leadership: waiting
                        // followers observe the result first and never see a
                        // leaderless-but-completed flight (they check
                        // `snapshot()` before `leader_alive` on every wake).
                        // Cache is populated inside `build_work`.
                        flight.publish(result.clone());
                        drop(guard);
                        // Ownership-compare removal: retire only the flight we
                        // led. `try_claim_leader` refuses result-published
                        // flights, so any caller entering between the release
                        // above and this removal becomes a follower of this
                        // same flight (never a new leader), and the map still
                        // holds it; the comparison guarantees a stale leader
                        // can never delete a different, newer flight.
                        let mut state = self.in_flight.lock().expect("singleflight poisoned");
                        if let Some(registered) = state.get(&key) {
                            if Arc::ptr_eq(registered, &flight) {
                                state.remove(&key);
                            }
                        }
                        return SingleflightResult::Outcome(result);
                    }
                    None => {
                        // Leader cancelled. `build_work`'s future was already
                        // dropped by `select!` (fetch aborted). Release
                        // leadership and wake followers so one promotes; do NOT
                        // remove the flight, so the promoted follower finds it.
                        drop(guard);
                        return SingleflightResult::CallerCancelled;
                    }
                }
            }

            // Follower. Wait for a published result, leader death, or our own
            // cancellation. None of these abort the leader's shared work.
            match flight.wait(caller_cancel).await {
                FollowerEvent::Published(result) => return SingleflightResult::Outcome(result),
                FollowerEvent::CallerCancelled => return SingleflightResult::CallerCancelled,
                FollowerEvent::LeaderDied => {
                    promotions += 1;
                    if promotions > SINGLEFLIGHT_MAX_PROMOTIONS {
                        return SingleflightResult::Exhausted;
                    }
                    // Loop: re-acquire and try to claim leadership on the
                    // (still-registered) leaderless flight.
                    continue;
                }
            }
        }
    }
}

/// Validate a request scope: tenant / account / provider must be non-empty.
/// Returns a plain domain error — scope validation is not an adapter failure,
/// so no `AdapterKind` is fabricated.
fn validate_scope(scope: &QuotaScope) -> Result<(), QuotaError> {
    if scope.tenant_id.as_str().trim().is_empty() {
        return Err(QuotaError::unsupported(
            "quota scope tenant_id must not be empty",
        ));
    }
    if scope.account_id.as_str().trim().is_empty() {
        return Err(QuotaError::unsupported(
            "quota scope account_id must not be empty",
        ));
    }
    if scope.provider_id.as_str().trim().is_empty() {
        return Err(QuotaError::unsupported(
            "quota scope provider_id must not be empty",
        ));
    }
    Ok(())
}

// =========================================================================
// QuotaService
// =========================================================================

/// Aggregating quota service: registry + cache + singleflight.
pub struct QuotaService {
    inner: Arc<Inner>,
}

struct Inner {
    registry: Mutex<Vec<(ScopeMatch, Arc<dyn QuotaAdapter>)>>,
    cache: QuotaCache,
    singleflight: Singleflight,
    cache_ttl: Duration,
    clock: Arc<dyn QuotaClock>,
    /// Optional local-ledger reconciler. When set, `fetch_fresh` overlays the
    /// ledger's strict increment on every fresh Exact remote baseline.
    ledger_reconciler: Mutex<Option<Arc<LedgerQuotaAdapter>>>,
}

impl QuotaService {
    /// Build a service with the default 30 s cache TTL.
    pub fn new(clock: Arc<dyn QuotaClock>) -> Self {
        Self::with_ttl(clock, Duration::from_secs(30))
    }

    /// Build a service with a custom cache TTL. Zero TTL means cached entries
    /// are never fresh; every read attempts a fresh fetch (still deduplicated
    /// by singleflight), and the most recent cache entry is kept as a stale
    /// fallback when a fresh fetch fails.
    pub fn with_ttl(clock: Arc<dyn QuotaClock>, cache_ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                registry: Mutex::new(Vec::new()),
                cache: QuotaCache::default(),
                singleflight: Singleflight::default(),
                cache_ttl,
                clock,
                ledger_reconciler: Mutex::new(None),
            }),
        }
    }

    /// Register an adapter under a static scope predicate. Multiple adapters
    /// may match one scope; they are run concurrently and ranked by confidence.
    pub fn register(&self, scope_match: ScopeMatch, adapter: Arc<dyn QuotaAdapter>) {
        self.inner
            .registry
            .lock()
            .expect("registry poisoned")
            .push((scope_match, adapter));
    }

    /// Borrow the shared clock (the refresh scheduler shares it).
    pub fn clock(&self) -> Arc<dyn QuotaClock> {
        self.inner.clock.clone()
    }

    /// Drop all cached snapshots. Tests and explicit "force refresh" paths use
    /// this; normal reads rely on TTL.
    pub fn invalidate(&self) {
        self.inner.cache.clear();
    }

    /// Number of cached entries (test/diagnostic helper).
    pub fn cache_size(&self) -> usize {
        self.inner.cache.len()
    }

    /// Attach a local-ledger reconciler. Once attached, every fresh Exact
    /// remote baseline selected by `fetch_fresh` is overlaid with the ledger's
    /// strict increment: delta = 0 keeps the remote `Exact`, delta > 0 becomes
    /// `Derived` (provenance names both the remote source and the ledger). A
    /// reconcile failure never discards the remote — it is surfaced as an
    /// advisory `LocalLedger` [`QuotaFailure`] alongside the still-served
    /// remote snapshot. LocalLedger / already-Derived / Scraped baselines are
    /// never re-overlaid.
    pub fn set_ledger_reconciler(&self, reconciler: Arc<LedgerQuotaAdapter>) {
        *self
            .inner
            .ledger_reconciler
            .lock()
            .expect("ledger reconciler poisoned") = Some(reconciler);
    }

    /// Publish a locally-derived snapshot directly into the cache so cache-only
    /// reads and budget projection can surface it without a remote fetch. Only
    /// `LocalLedger` / `Derived` snapshots are accepted — a raw remote `Exact`
    /// or `Scraped` snapshot must not be republished as ledger-derived. The
    /// scope is validated first and the entry is stored under the full scope +
    /// window + unit key.
    pub fn publish_local_snapshot(&self, snapshot: QuotaSnapshot) -> Result<(), QuotaError> {
        validate_scope(&snapshot.scope)?;
        let is_local = snapshot.provenance.adapter_kind == AdapterKind::LocalLedger
            || snapshot.confidence == Confidence::Derived;
        if !is_local {
            return Err(QuotaError::unsupported(
                "publish_local_snapshot only accepts LocalLedger/Derived snapshots",
            ));
        }
        let key = CacheKey {
            scope: snapshot.scope.clone(),
            window: snapshot.window,
            unit: snapshot.unit.clone(),
        };
        self.inner.cache.put(key, snapshot, self.inner.clock.now());
        Ok(())
    }

    /// Every cached snapshot whose full scope matches `scope`, across all
    /// windows and units. Matching is full-scope equality (tenant + account +
    /// provider + model + credential), so sibling credentials or accounts never
    /// leak into each other's views. Both fresh and stale entries are returned;
    /// callers decide freshness.
    pub fn cached_snapshots_for_scope(&self, scope: &QuotaScope) -> Vec<QuotaSnapshot> {
        self.inner.cache.snapshots_for_scope(scope)
    }

    /// Single-window aggregated read.
    ///
    /// Returns [`QuotaRead`] when any candidate (or stale cache fallback)
    /// produced a snapshot. Returns the typed [`QuotaFailure`]s when every
    /// candidate failed AND no stale cache entry was available. Adapter
    /// failures carry their [`AdapterKind`]; query-level failures (scope,
    /// cancellation, exhaustion) carry no attribution.
    ///
    /// Anonymous convenience entry point: passes `None` to adapters. Use
    /// [`QuotaService::read_with_credential`] to inject a credential.
    pub async fn read(
        &self,
        request: &crate::QuotaRequest,
        cancel: &CancellationToken,
    ) -> Result<QuotaRead, Vec<QuotaFailure>> {
        read_impl(self.inner.clone(), request, None, cancel).await
    }

    /// Single-window aggregated read with an optional credential.
    ///
    /// The credential is borrowed only for the duration of the underlying
    /// `fetch`: it is never stored on the service, never serialized into a
    /// cached [`QuotaSnapshot`], and never logged. The singleflight leader
    /// re-injects it on every promotion so followers receive identical
    /// results regardless of which caller happened to lead.
    pub async fn read_with_credential(
        &self,
        request: &crate::QuotaRequest,
        credential: Option<&ResolvedCredential>,
        cancel: &CancellationToken,
    ) -> Result<QuotaRead, Vec<QuotaFailure>> {
        read_impl(self.inner.clone(), request, credential, cancel).await
    }

    /// Multi-window concurrent read. Each window is fetched independently;
    /// one window's failure never aborts the others (partial, not
    /// all-or-nothing).
    ///
    /// Anonymous convenience entry point: passes `None` to adapters. Use
    /// [`QuotaService::overview_with_credential`] to inject a credential.
    pub async fn overview(
        &self,
        scope: &QuotaScope,
        windows: &[crate::QuotaWindow],
        unit: &crate::QuotaUnit,
        cancel: &CancellationToken,
    ) -> QuotaOverview {
        self.overview_with_credential(scope, windows, unit, None, cancel)
            .await
    }

    /// Multi-window concurrent read with an optional credential. The
    /// credential is threaded into every window's `fetch` and never retained.
    pub async fn overview_with_credential(
        &self,
        scope: &QuotaScope,
        windows: &[crate::QuotaWindow],
        unit: &crate::QuotaUnit,
        credential: Option<&ResolvedCredential>,
        cancel: &CancellationToken,
    ) -> QuotaOverview {
        let futs = windows.iter().map(|window| {
            let request = crate::QuotaRequest {
                scope: scope.clone(),
                window: *window,
                unit: unit.clone(),
            };
            let cancel = cancel.clone();
            let inner = self.inner.clone();
            async move {
                let result = read_impl(inner, &request, credential, &cancel).await;
                (*window, result)
            }
        });
        let results = future::join_all(futs).await;
        let mut map = HashMap::with_capacity(results.len());
        for (window, result) in results {
            let entry = match result {
                Ok(read) => WindowRead::Ok(read),
                Err(failures) => WindowRead::Failed { failures },
            };
            map.insert(window, entry);
        }
        QuotaOverview {
            scope: scope.clone(),
            windows: map,
        }
    }

    /// Cache-only single-window query.
    ///
    /// Consults only the in-process cache. It **never** calls any adapter,
    /// the network, or singleflight — so a UI poll that only needs to render
    /// the last-known quota cannot accidentally trigger a remote fetch (the
    /// bug P14-8 fixes: `AppService` calling [`QuotaService::overview`] on
    /// every poll was triggering scrapes/API calls).
    ///
    /// Returns [`CacheRead::Hit`] when a fresh (within TTL) entry exists,
    /// [`CacheRead::Stale`] when an entry exists but is older than TTL, and
    /// [`CacheRead::NoData`] when there is no cached entry. `scope` is still
    /// validated (`tenant_id` / `account_id` / `provider_id` non-empty); an
    /// invalid scope surfaces as a plain [`QuotaError`] via [`Result::Err`].
    pub fn read_cache_only(&self, request: &crate::QuotaRequest) -> Result<CacheRead, QuotaError> {
        validate_scope(&request.scope)?;
        let key = CacheKey {
            scope: request.scope.clone(),
            window: request.window,
            unit: request.unit.clone(),
        };
        let now = self.inner.clock.now();
        Ok(match self.inner.cache.get(&key) {
            Some(entry) if entry.is_fresh(now, self.inner.cache_ttl) => CacheRead::Hit {
                snapshot: entry.snapshot,
            },
            Some(entry) => CacheRead::Stale {
                snapshot: entry.snapshot,
            },
            None => CacheRead::NoData,
        })
    }

    /// Cache-only multi-window query.
    ///
    /// Like [`QuotaService::read_cache_only`] but spans multiple windows; each
    /// window is resolved independently from the cache, with no adapter or
    /// singleflight involvement. `scope` validation is shared across windows.
    pub fn overview_cache_only(
        &self,
        scope: &QuotaScope,
        windows: &[crate::QuotaWindow],
        unit: &crate::QuotaUnit,
    ) -> Result<CacheOverview, QuotaError> {
        validate_scope(scope)?;
        let now = self.inner.clock.now();
        let mut map = HashMap::with_capacity(windows.len());
        for window in windows {
            let key = CacheKey {
                scope: scope.clone(),
                window: *window,
                unit: unit.clone(),
            };
            let entry = match self.inner.cache.get(&key) {
                Some(entry) if entry.is_fresh(now, self.inner.cache_ttl) => CacheRead::Hit {
                    snapshot: entry.snapshot,
                },
                Some(entry) => CacheRead::Stale {
                    snapshot: entry.snapshot,
                },
                None => CacheRead::NoData,
            };
            map.insert(*window, entry);
        }
        Ok(CacheOverview {
            scope: scope.clone(),
            windows: map,
        })
    }
}

async fn read_impl(
    inner: Arc<Inner>,
    request: &crate::QuotaRequest,
    credential: Option<&ResolvedCredential>,
    cancel: &CancellationToken,
) -> Result<QuotaRead, Vec<QuotaFailure>> {
    // Scope validation: tenant / account / provider are mandatory isolation
    // keys and must be non-empty. Reject before touching cache or adapters.
    if let Err(error) = validate_scope(&request.scope) {
        return Err(vec![QuotaFailure::domain(error)]);
    }

    let key = CacheKey {
        scope: request.scope.clone(),
        window: request.window,
        unit: request.unit.clone(),
    };
    let now = inner.clock.now();

    // Fresh cache hit short-circuits before any candidate work.
    if let Some(entry) = inner.cache.get(&key) {
        if entry.is_fresh(now, inner.cache_ttl) {
            return Ok(QuotaRead {
                snapshot: entry.snapshot,
                failures: Vec::new(),
                served_stale: false,
            });
        }
    }
    // Stale fallback retained for recovery on fresh-fetch failure.
    let stale_fallback = inner.cache.get(&key);

    // Singleflight around the fresh fetch. The work factory owns an
    // `Arc<Inner>` so it can write through to the shared cache. A follower that
    // cancels only bows out of its own wait; the leader (a different caller)
    // keeps running. A leader that cancels aborts only its own in-flight fetch,
    // hands leadership to a follower, and reports `Cancelled`; the follower
    // re-runs and populates the cache. See [`Singleflight::run`].
    let work_inner = inner.clone();
    let work_request = request.clone();
    let work_key = key.clone();
    // The credential is `Copy` as `Option<&ResolvedCredential>`; the factory
    // re-injects it on every leader promotion without cloning the secret.
    let outcome = inner
        .singleflight
        .run(
            key,
            move || {
                let inner = work_inner.clone();
                let key = work_key.clone();
                let request = work_request.clone();
                let credential = credential;
                async move {
                    // The fetch is driven by an internal token that is never
                    // cooperatively cancelled, so a *follower's* cancel never
                    // reaches the in-flight fetch (it only exits the follower
                    // wait). A *leader's* own caller cancel aborts this future
                    // by drop (see [`Singleflight::run`]), after which a
                    // promoted follower re-invokes this factory fresh.
                    let leader_cancel = CancellationToken::new();
                    let outcome = fetch_fresh(&inner, &request, credential, &leader_cancel).await;
                    if let Some(snap) = outcome.snapshot_for_cache() {
                        inner.cache.put(key, snap.clone(), inner.clock.now());
                    }
                    Arc::new(outcome)
                }
            },
            cancel,
        )
        .await;

    let outcome = match outcome {
        SingleflightResult::Outcome(o) => o,
        SingleflightResult::CallerCancelled => {
            // This caller's token fired — either a follower bowing out of its
            // wait, or a leader that aborted its own in-flight fetch. In both
            // cases shared work continues under a (possibly new) leader.
            // Report a domain-level Cancelled failure without touching the
            // cache. No adapter produced this error, so no attribution.
            return Err(vec![QuotaFailure::domain(QuotaError::Cancelled)]);
        }
        SingleflightResult::Exhausted => {
            return Err(vec![QuotaFailure::domain(QuotaError::other(
                "singleflight leader promotions exhausted",
            ))]);
        }
    };

    match &*outcome {
        ReadOutcome::Ok { snapshot, failures } => Ok(QuotaRead {
            snapshot: snapshot.clone(),
            failures: failures.clone(),
            served_stale: false,
        }),
        ReadOutcome::AllFailed(failures) => {
            if let Some(entry) = stale_fallback {
                // Rewrite provenance.stale = true so downstream readers can
                // branch on staleness from the snapshot alone. Clone first so
                // the cached entry is untouched.
                let mut snapshot = entry.snapshot;
                snapshot.provenance.stale = true;
                Ok(QuotaRead {
                    snapshot,
                    failures: failures.clone(),
                    served_stale: true,
                })
            } else {
                Err(failures.clone())
            }
        }
    }
}

fn candidates_for(inner: &Inner, request: &crate::QuotaRequest) -> Vec<Arc<dyn QuotaAdapter>> {
    inner
        .registry
        .lock()
        .expect("registry poisoned")
        .iter()
        .filter(|(scope_match, _)| scope_match.matches(&request.scope))
        .filter(|(_, adapter)| adapter.supports(request))
        .map(|(_, adapter)| adapter.clone())
        .collect()
}

async fn fetch_fresh(
    inner: &Inner,
    request: &crate::QuotaRequest,
    credential: Option<&ResolvedCredential>,
    cancel: &CancellationToken,
) -> ReadOutcome {
    // The credential is borrowed only here; it never outlives this fetch and
    // is never written into a snapshot, cache entry, or log.
    let candidates = candidates_for(inner, request);
    if candidates.is_empty() {
        // No candidate adapter matched: a query-level failure, not an adapter
        // error, so no `AdapterKind` is fabricated.
        return ReadOutcome::AllFailed(vec![QuotaFailure::domain(QuotaError::unsupported(
            "no adapter supports this scope/window/unit",
        ))]);
    }

    // Run every candidate concurrently, racing each against cancellation.
    // Adapters that finish before cancel contribute their result; the rest
    // report Cancelled so partial successes remain usable.
    let futs = candidates.iter().map(|adapter| {
        let adapter = adapter.clone();
        let request = request.clone();
        let cancel = cancel.clone();
        async move {
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(QuotaError::Cancelled),
                outcome = adapter.fetch(&request, credential, &cancel) => outcome,
            };
            (adapter.kind(), result)
        }
    });
    let results = future::join_all(futs).await;

    let mut successes: Vec<QuotaSnapshot> = Vec::new();
    let mut failures = Vec::new();
    for (kind, result) in results {
        match result {
            Ok(snapshot) => successes.push(snapshot),
            Err(error) => failures.push(QuotaFailure::new(kind, error)),
        }
    }

    if successes.is_empty() {
        return ReadOutcome::AllFailed(failures);
    }

    // Source priority: Exact > Derived > Scraped. Ties broken by freshness
    // (most recent fetched_at wins). Scraped never silently overwrites Exact
    // because its confidence priority is strictly lower.
    successes.sort_by(|a, b| {
        b.confidence
            .priority()
            .cmp(&a.confidence.priority())
            .then_with(|| b.provenance.fetched_at.cmp(&a.provenance.fetched_at))
    });
    let best = successes.into_iter().next().expect("non-empty");

    // P14-7 step 2: overlay the local ledger's strict increment on the chosen
    // fresh Exact remote baseline. LocalLedger / already-Derived / Scraped
    // baselines are returned untouched (see `overlay_ledger_delta`). A
    // reconcile failure does not discard the remote — it is reported as an
    // advisory LocalLedger failure alongside the still-served remote snapshot.
    let (snapshot, failures) = overlay_ledger_delta(inner, best, failures, cancel).await;

    ReadOutcome::Ok { snapshot, failures }
}

/// Overlay the local ledger's strict increment onto a fresh Exact remote
/// baseline, when a ledger reconciler has been attached via
/// [`QuotaService::set_ledger_reconciler`]. Baselines that are already
/// local-ledger-sourced, derived, or scraped are returned untouched, so the
/// ledger is never layered on itself or on a lower-confidence source. A
/// reconcile error keeps the remote baseline and appends an advisory
/// `LocalLedger` failure rather than discarding the remote read.
async fn overlay_ledger_delta(
    inner: &Inner,
    best: QuotaSnapshot,
    mut failures: Vec<QuotaFailure>,
    cancel: &CancellationToken,
) -> (QuotaSnapshot, Vec<QuotaFailure>) {
    let reconciler = inner
        .ledger_reconciler
        .lock()
        .expect("ledger reconciler poisoned")
        .clone();
    let Some(reconciler) = reconciler else {
        return (best, failures);
    };
    // Only a fresh, remote Exact baseline is eligible. `adapter_kind ==
    // LocalLedger` covers both raw local-ledger snapshots and snapshots already
    // produced by a previous reconcile pass (which stamps adapter_kind =
    // LocalLedger); `confidence != Exact` excludes Scraped and Derived.
    if best.confidence != Confidence::Exact
        || best.provenance.adapter_kind == AdapterKind::LocalLedger
    {
        return (best, failures);
    }
    match reconciler.reconcile(&best, cancel).await {
        Ok(overlay) => (overlay, failures),
        Err(error) => {
            // Keep the remote baseline; surface the ledger miss as advisory.
            failures.push(QuotaFailure::new(AdapterKind::LocalLedger, error));
            (best, failures)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use async_trait::async_trait;
    use provider_api::{CredentialKind, ResolvedCredential};
    use usage_ledger::{InMemoryUsageLedger, UsageLedger, UsageRecord};

    use super::*;
    use crate::{
        AccountId, AdapterKind, Confidence, QuotaMeasure, QuotaProvenance, QuotaRequest,
        QuotaReset, QuotaScope, QuotaUnit, QuotaValues, QuotaWindow,
    };

    fn scope(provider: &str) -> QuotaScope {
        QuotaScope::new(
            TenantId::new("tenant-a"),
            AccountId::new("account-1"),
            ProviderId::new(provider),
            Some(ModelId::new("model-x")),
        )
    }

    fn snapshot(
        scope: &QuotaScope,
        window: QuotaWindow,
        unit: QuotaUnit,
        confidence: Confidence,
        adapter_kind: AdapterKind,
        fetched_at_ms: u64,
    ) -> QuotaSnapshot {
        QuotaSnapshot {
            scope: scope.clone(),
            window,
            unit,
            values: QuotaValues::new(
                QuotaMeasure::exact(25),
                QuotaMeasure::exact(100),
                QuotaMeasure::exact(75),
            ),
            reset: QuotaReset::Unknown,
            confidence,
            provenance: QuotaProvenance::new(
                adapter_kind,
                "test",
                Timestamp::from_unix_millis(fetched_at_ms),
            ),
        }
    }

    struct MockAdapter {
        kind: AdapterKind,
        confidence: Confidence,
        fetched_at_ms: u64,
        delay_ms: u64,
        error: Option<QuotaError>,
        calls: Arc<AtomicU64>,
        supports_flag: bool,
        /// Last credential kind observed by `fetch`; `None` when no credential
        /// was supplied. Asserted by credential-injection tests.
        seen_credential: Arc<Mutex<Option<CredentialKind>>>,
    }

    impl MockAdapter {
        fn exact(fetched_at_ms: u64) -> Self {
            Self {
                kind: AdapterKind::ApiKeyApi,
                confidence: Confidence::Exact,
                fetched_at_ms,
                delay_ms: 0,
                error: None,
                calls: Arc::new(AtomicU64::new(0)),
                supports_flag: true,
                seen_credential: Arc::new(Mutex::new(None)),
            }
        }
    }

    #[async_trait]
    impl QuotaAdapter for MockAdapter {
        fn kind(&self) -> AdapterKind {
            self.kind
        }

        fn supports(&self, _: &QuotaRequest) -> bool {
            self.supports_flag
        }

        async fn fetch(
            &self,
            request: &QuotaRequest,
            credential: Option<&ResolvedCredential>,
            cancel: &CancellationToken,
        ) -> Result<QuotaSnapshot, QuotaError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.seen_credential.lock().expect("seen_credential") = credential.map(|c| c.kind());
            if self.delay_ms > 0 {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(QuotaError::Cancelled),
                    _ = tokio::time::sleep(Duration::from_millis(self.delay_ms)) => {}
                }
            }
            if let Some(err) = &self.error {
                return Err(err.clone());
            }
            Ok(snapshot(
                &request.scope,
                request.window,
                request.unit.clone(),
                self.confidence,
                self.kind,
                self.fetched_at_ms,
            ))
        }
    }

    fn request(window: QuotaWindow) -> QuotaRequest {
        QuotaRequest {
            scope: scope("anthropic"),
            window,
            unit: QuotaUnit::Token,
        }
    }

    #[tokio::test]
    async fn cache_hit_skips_adapters() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock.clone(), Duration::from_secs(60));
        let adapter = Arc::new(MockAdapter::exact(1_000));
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), adapter);

        let cancel = CancellationToken::new();
        let req = request(QuotaWindow::Monthly);
        let first = svc.read(&req, &cancel).await.expect("first ok");
        assert!(!first.served_stale);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        clock.advance(5_000);
        let second = svc.read(&req, &cancel).await.expect("second ok");
        assert!(!second.served_stale);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(svc.cache_size(), 1);
    }

    #[tokio::test]
    async fn singleflight_dedups_concurrent_reads() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = Arc::new(QuotaService::with_ttl(clock, Duration::from_secs(60)));
        let mut adapter = MockAdapter::exact(1_000);
        adapter.delay_ms = 50;
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), Arc::new(adapter));

        let cancel = CancellationToken::new();
        let req = request(QuotaWindow::Monthly);
        let mut handles = Vec::new();
        for _ in 0..8 {
            let svc = svc.clone();
            let req = req.clone();
            let cancel = cancel.clone();
            handles.push(tokio::spawn(async move { svc.read(&req, &cancel).await }));
        }
        for h in handles {
            h.await.expect("join").expect("ok");
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "singleflight must collapse concurrent reads into one fetch"
        );
    }

    #[tokio::test]
    async fn exact_beats_derived_beats_scraped() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));

        let mut scraped = MockAdapter::exact(2_000);
        scraped.kind = AdapterKind::WebScrape;
        scraped.confidence = Confidence::Scraped;

        let mut derived = MockAdapter::exact(1_500);
        derived.kind = AdapterKind::LocalLedger;
        derived.confidence = Confidence::Derived;

        let exact = MockAdapter::exact(1_000);

        svc.register(ScopeMatch::any(), Arc::new(scraped));
        svc.register(ScopeMatch::any(), Arc::new(derived));
        svc.register(ScopeMatch::any(), Arc::new(exact));

        let cancel = CancellationToken::new();
        let read = svc
            .read(&request(QuotaWindow::Monthly), &cancel)
            .await
            .expect("ok");
        assert_eq!(read.snapshot.confidence, Confidence::Exact);
        assert_eq!(
            read.snapshot.provenance.adapter_kind,
            AdapterKind::ApiKeyApi
        );
        assert!(read.failures.is_empty());
    }

    #[tokio::test]
    async fn scraped_never_overrides_exact_even_when_fresher() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));

        let mut exact = MockAdapter::exact(900);
        exact.confidence = Confidence::Exact;
        let mut scraped = MockAdapter::exact(999);
        scraped.kind = AdapterKind::WebScrape;
        scraped.confidence = Confidence::Scraped;

        svc.register(ScopeMatch::any(), Arc::new(exact));
        svc.register(ScopeMatch::any(), Arc::new(scraped));

        let cancel = CancellationToken::new();
        let read = svc
            .read(&request(QuotaWindow::Monthly), &cancel)
            .await
            .expect("ok");
        assert_eq!(read.snapshot.confidence, Confidence::Exact);
        assert_eq!(
            read.snapshot.provenance.adapter_kind,
            AdapterKind::ApiKeyApi
        );
    }

    #[tokio::test]
    async fn partial_failure_still_returns_snapshot_and_lists_failures() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));

        let exact = Arc::new(MockAdapter::exact(1_000));
        let mut broken = MockAdapter::exact(1_000);
        broken.kind = AdapterKind::OAuthApi;
        broken.error = Some(QuotaError::forbidden("console denies"));

        svc.register(ScopeMatch::any(), exact);
        svc.register(ScopeMatch::any(), Arc::new(broken));

        let cancel = CancellationToken::new();
        let read = svc
            .read(&request(QuotaWindow::Monthly), &cancel)
            .await
            .expect("ok");
        assert_eq!(read.snapshot.confidence, Confidence::Exact);
        assert_eq!(read.failures.len(), 1);
        assert_eq!(
            read.failures[0].adapter_kind,
            Some(AdapterKind::OAuthApi),
            "real adapter failures must keep their attribution"
        );
        assert!(matches!(
            read.failures[0].error,
            QuotaError::Forbidden { .. }
        ));
    }

    #[tokio::test]
    async fn all_failures_without_cache_return_err() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));

        let mut a = MockAdapter::exact(1_000);
        a.error = Some(QuotaError::forbidden("nope"));
        let mut b = MockAdapter::exact(1_000);
        b.kind = AdapterKind::OAuthApi;
        b.error = Some(QuotaError::rate_limited("slow", Some(123)));

        svc.register(ScopeMatch::any(), Arc::new(a));
        svc.register(ScopeMatch::any(), Arc::new(b));

        let cancel = CancellationToken::new();
        let err = svc
            .read(&request(QuotaWindow::Monthly), &cancel)
            .await
            .expect_err("must error");
        assert_eq!(err.len(), 2);
        assert!(err.iter().any(|f| matches!(
            f.error,
            QuotaError::RateLimited {
                retry_after_ms: Some(123),
                ..
            }
        )));
    }

    #[tokio::test]
    async fn stale_cache_served_when_fresh_fetch_fails() {
        // ttl=0 means cached entries are never fresh; every read attempts a
        // fresh fetch. The most recent cached snapshot is kept as a stale
        // fallback when a fresh fetch genuinely fails (here: adapter error),
        // and the served snapshot's provenance.stale must be rewritten to true.
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::ZERO);

        // A flappable adapter: succeeds the first fetch (caches a snapshot),
        // then returns a forbidden error on the second fetch, forcing a
        // genuine failure (not a cancellation) on the second read.
        let adapter = Arc::new(FlappableAdapter::default());
        svc.register(ScopeMatch::any(), adapter);
        let cancel = CancellationToken::new();
        let req = request(QuotaWindow::Monthly);
        let first = svc.read(&req, &cancel).await.expect("first ok");
        assert!(!first.served_stale);
        assert!(!first.snapshot.provenance.stale);

        // Second read: adapter now errors. With no fresh data, the cached
        // snapshot is served as stale and its provenance.stale is set true.
        let second = svc.read(&req, &cancel).await.expect("stale ok");
        assert!(second.served_stale);
        assert!(
            second.snapshot.provenance.stale,
            "stale fallback must rewrite provenance.stale = true"
        );
        // No secret ever leaks into the served snapshot.
        let json = serde_json::to_string(&second.snapshot).expect("serialize");
        assert!(!json.contains("sk-") && !json.contains("secret"));
        let _ = CredentialKind::ApiKey;
    }

    #[tokio::test]
    async fn overview_partial_failure_for_one_window() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));

        let monthly_only = Arc::new(WindowRestrictedAdapter {
            inner: MockAdapter::exact(1_000),
            supported: vec![QuotaWindow::Monthly],
        });
        svc.register(ScopeMatch::any(), monthly_only);

        let cancel = CancellationToken::new();
        let overview = svc
            .overview(
                &scope("openai"),
                &[QuotaWindow::Monthly, QuotaWindow::Rolling5h],
                &QuotaUnit::Token,
                &cancel,
            )
            .await;
        assert!(matches!(
            overview.windows.get(&QuotaWindow::Monthly),
            Some(WindowRead::Ok(_))
        ));
        assert!(matches!(
            overview.windows.get(&QuotaWindow::Rolling5h),
            Some(WindowRead::Failed { .. })
        ));
    }

    struct WindowRestrictedAdapter {
        inner: MockAdapter,
        supported: Vec<QuotaWindow>,
    }

    #[async_trait]
    impl QuotaAdapter for WindowRestrictedAdapter {
        fn kind(&self) -> AdapterKind {
            self.inner.kind
        }
        fn supports(&self, request: &QuotaRequest) -> bool {
            self.supported.contains(&request.window)
        }
        async fn fetch(
            &self,
            request: &QuotaRequest,
            credential: Option<&ResolvedCredential>,
            cancel: &CancellationToken,
        ) -> Result<QuotaSnapshot, QuotaError> {
            self.inner.fetch(request, credential, cancel).await
        }
    }

    #[tokio::test]
    async fn single_caller_cancel_does_not_abort_shared_work() {
        // A slow leader fetch is in flight. A follower caller cancels its own
        // token: it must receive a Cancelled failure promptly, while the
        // leader (and any other follower) still completes the shared fetch.
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = Arc::new(QuotaService::with_ttl(clock, Duration::from_secs(60)));
        let mut adapter = MockAdapter::exact(1_000);
        adapter.delay_ms = 120;
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), Arc::new(adapter));

        let req = request(QuotaWindow::Monthly);
        // Leader: a long-lived cancel so it drives the fetch to completion.
        let leader_cancel = CancellationToken::new();
        let leader_svc = svc.clone();
        let leader_req = req.clone();
        let leader_cancel_clone = leader_cancel.clone();
        let leader = tokio::spawn(async move {
            leader_svc
                .read(&leader_req, &leader_cancel_clone)
                .await
                .expect("leader completes shared fetch")
        });

        // Give the leader a chance to claim leadership before the follower.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Follower cancels immediately. It must return Cancelled WITHOUT
        // aborting the in-flight leader fetch.
        let follower_cancel = CancellationToken::new();
        follower_cancel.cancel();
        let follower_svc = svc.clone();
        let follower_req = req.clone();
        let follower =
            tokio::spawn(async move { follower_svc.read(&follower_req, &follower_cancel).await });
        let follower_err = follower
            .await
            .expect("join")
            .expect_err("follower cancelled");
        assert!(follower_err
            .iter()
            .any(|f| matches!(f.error, QuotaError::Cancelled)));

        // The leader still completes exactly one fetch (shared work survived
        // the follower's cancellation).
        let leader_read = leader.await.expect("leader join");
        assert!(!leader_read.served_stale);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn scope_match_filters_by_provider() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));
        let openai_calls = Arc::new(AtomicU64::new(0));
        let anthropic_calls = Arc::new(AtomicU64::new(0));
        svc.register(
            ScopeMatch::for_provider(ProviderId::new("openai")),
            Arc::new(CountingAdapter {
                kind: AdapterKind::ApiKeyApi,
                calls: openai_calls.clone(),
            }),
        );
        svc.register(
            ScopeMatch::for_provider(ProviderId::new("anthropic")),
            Arc::new(CountingAdapter {
                kind: AdapterKind::ApiKeyApi,
                calls: anthropic_calls.clone(),
            }),
        );

        let cancel = CancellationToken::new();
        let req = QuotaRequest {
            scope: scope("openai"),
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Token,
        };
        svc.read(&req, &cancel).await.expect("ok");
        assert_eq!(openai_calls.load(Ordering::SeqCst), 1);
        assert_eq!(anthropic_calls.load(Ordering::SeqCst), 0);
    }

    struct CountingAdapter {
        kind: AdapterKind,
        calls: Arc<AtomicU64>,
    }

    /// Adapter that succeeds the first fetch and fails every subsequent one.
    /// Used to exercise the stale-cache fallback on a genuine fetch failure
    /// (not a cancellation).
    #[derive(Default)]
    struct FlappableAdapter {
        calls: Mutex<u32>,
    }

    #[async_trait]
    impl QuotaAdapter for FlappableAdapter {
        fn kind(&self) -> AdapterKind {
            AdapterKind::ApiKeyApi
        }
        fn supports(&self, _: &QuotaRequest) -> bool {
            true
        }
        async fn fetch(
            &self,
            request: &QuotaRequest,
            _: Option<&ResolvedCredential>,
            _: &CancellationToken,
        ) -> Result<QuotaSnapshot, QuotaError> {
            let mut n = self.calls.lock().expect("flappable");
            *n += 1;
            if *n == 1 {
                return Ok(snapshot(
                    &request.scope,
                    request.window,
                    request.unit.clone(),
                    Confidence::Exact,
                    AdapterKind::ApiKeyApi,
                    1_000,
                ));
            }
            Err(QuotaError::forbidden("flapped"))
        }
    }

    #[tokio::test]
    async fn read_with_credential_threads_credential_to_adapter() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));
        let adapter = Arc::new(MockAdapter::exact(1_000));
        let seen = adapter.seen_credential.clone();
        svc.register(ScopeMatch::any(), adapter);

        let cancel = CancellationToken::new();
        let req = request(QuotaWindow::Monthly);
        let cred = ResolvedCredential::new(CredentialKind::ApiKey, "sk-test-secret-value");
        let read = svc
            .read_with_credential(&req, Some(&cred), &cancel)
            .await
            .expect("ok");
        assert_eq!(read.snapshot.confidence, Confidence::Exact);
        assert_eq!(
            *seen.lock().expect("seen"),
            Some(CredentialKind::ApiKey),
            "credential must be threaded into adapter.fetch"
        );
        // The secret never reaches the served snapshot or its serialization.
        let json = serde_json::to_string(&read.snapshot).expect("serialize");
        assert!(!json.contains("sk-test-secret-value"));
    }

    #[tokio::test]
    async fn read_passes_none_credential_on_legacy_path() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));
        let adapter = Arc::new(MockAdapter::exact(1_000));
        let seen = adapter.seen_credential.clone();
        svc.register(ScopeMatch::any(), adapter);

        let cancel = CancellationToken::new();
        let req = request(QuotaWindow::Monthly);
        svc.read(&req, &cancel).await.expect("ok");
        assert_eq!(*seen.lock().expect("seen"), None);
    }

    #[tokio::test]
    async fn overview_with_credential_threads_credential_across_windows() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));
        let adapter = Arc::new(MockAdapter::exact(1_000));
        let seen = adapter.seen_credential.clone();
        svc.register(ScopeMatch::any(), adapter);

        let cancel = CancellationToken::new();
        let cred = ResolvedCredential::new(CredentialKind::OAuthBearer, "oauth-bearer-secret");
        let overview = svc
            .overview_with_credential(
                &scope("anthropic"),
                &[QuotaWindow::Monthly, QuotaWindow::Weekly],
                &QuotaUnit::Token,
                Some(&cred),
                &cancel,
            )
            .await;
        assert_eq!(overview.ok_count(), 2);
        assert_eq!(
            *seen.lock().expect("seen"),
            Some(CredentialKind::OAuthBearer)
        );
    }

    #[tokio::test]
    async fn empty_scope_tenant_account_provider_are_rejected() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));
        // Register an adapter so rejection must come from scope validation,
        // not "no adapter".
        svc.register(ScopeMatch::any(), Arc::new(MockAdapter::exact(1_000)));
        let cancel = CancellationToken::new();

        for (label, scope) in [
            (
                "empty tenant",
                QuotaScope::new(
                    TenantId::new(""),
                    AccountId::new("acc"),
                    ProviderId::new("anthropic"),
                    None,
                ),
            ),
            (
                "whitespace tenant",
                QuotaScope::new(
                    TenantId::new("   "),
                    AccountId::new("acc"),
                    ProviderId::new("anthropic"),
                    None,
                ),
            ),
            (
                "empty account",
                QuotaScope::new(
                    TenantId::new("t"),
                    AccountId::new(""),
                    ProviderId::new("anthropic"),
                    None,
                ),
            ),
            (
                "empty provider",
                QuotaScope::new(
                    TenantId::new("t"),
                    AccountId::new("acc"),
                    ProviderId::new(""),
                    None,
                ),
            ),
        ] {
            let req = QuotaRequest {
                scope,
                window: QuotaWindow::Monthly,
                unit: QuotaUnit::Token,
            };
            let err = svc.read(&req, &cancel).await.expect_err(label);
            assert_eq!(err.len(), 1, "{label}: one failure");
            assert!(
                matches!(err[0].error, QuotaError::Unsupported { .. }),
                "{label}: must be Unsupported, got {:?}",
                err[0].error
            );
            assert_eq!(
                err[0].adapter_kind, None,
                "{label}: scope/no-candidate failures carry no adapter attribution"
            );
        }
    }

    #[tokio::test]
    async fn overview_rejects_empty_scope_without_calling_adapters() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));
        let adapter = Arc::new(MockAdapter::exact(1_000));
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), adapter);
        let cancel = CancellationToken::new();
        let bad_scope = QuotaScope::new(
            TenantId::new(""),
            AccountId::new("acc"),
            ProviderId::new("anthropic"),
            None,
        );
        let overview = svc
            .overview(
                &bad_scope,
                &[QuotaWindow::Monthly],
                &QuotaUnit::Token,
                &cancel,
            )
            .await;
        // Every window is Failed with an Unsupported scope error; no adapter call.
        assert_eq!(overview.ok_count(), 0);
        let read = overview
            .windows
            .get(&QuotaWindow::Monthly)
            .expect("present");
        assert!(matches!(read, WindowRead::Failed { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn completion_boundary_does_not_refetch_or_steal_leadership() {
        // A finishing leader publishes its result, releases leadership
        // (`leader_alive = false`), and only then retires the flight from the
        // map. A caller entering that release→remove window must not claim the
        // leaderless-but-completed flight and re-run finished work: it must
        // become a follower and receive the already-published result. This
        // reproduces the window state deterministically instead of racing the
        // two adjacent statements.
        let sf = Singleflight::default();
        let key = CacheKey {
            scope: scope("anthropic"),
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Token,
        };

        // Register a flight in the exact state left by a completing leader
        // between `release_leader` and map removal: still registered,
        // leaderless, result already published.
        let flight = {
            let mut state = sf.in_flight.lock().expect("singleflight poisoned");
            let flight = Arc::new(Flight::new());
            state.insert(key.clone(), flight.clone());
            flight
        };
        let outcome = Arc::new(ReadOutcome::Ok {
            snapshot: snapshot(
                &scope("anthropic"),
                QuotaWindow::Monthly,
                QuotaUnit::Token,
                Confidence::Exact,
                AdapterKind::ApiKeyApi,
                1_000,
            ),
            failures: vec![],
        });
        flight.publish(outcome.clone());
        flight.release_leader();

        // A caller arriving in that window must observe the published result
        // without invoking the work factory even once.
        let fetches = Arc::new(AtomicU64::new(0));
        let fetches_for_work = fetches.clone();
        let cancel = CancellationToken::new();
        let served = match sf
            .run(
                key,
                move || {
                    fetches_for_work.fetch_add(1, Ordering::SeqCst);
                    async {
                        Arc::new(ReadOutcome::Ok {
                            snapshot: snapshot(
                                &scope("anthropic"),
                                QuotaWindow::Monthly,
                                QuotaUnit::Token,
                                Confidence::Exact,
                                AdapterKind::ApiKeyApi,
                                2_000,
                            ),
                            failures: vec![],
                        })
                    }
                },
                &cancel,
            )
            .await
        {
            SingleflightResult::Outcome(served) => served,
            _ => panic!("completion-window caller must get Outcome, not promote/cancel"),
        };
        assert!(
            Arc::ptr_eq(&served, &outcome),
            "completion-window caller must receive the published result, not a fresh one"
        );
        assert_eq!(
            fetches.load(Ordering::SeqCst),
            0,
            "completion boundary must never re-run finished work"
        );
    }

    #[tokio::test]
    async fn leader_abort_follower_recovers_without_deadlock() {
        // A leader that aborts mid-fetch (its task is dropped) must not strand
        // followers: the next follower promotes and runs the fetch itself.
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = Arc::new(QuotaService::with_ttl(clock, Duration::from_secs(60)));
        let mut adapter = MockAdapter::exact(1_000);
        adapter.delay_ms = 80;
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), Arc::new(adapter));

        let req = request(QuotaWindow::Monthly);
        let svc1 = svc.clone();
        let req1 = req.clone();

        // Spawn the first caller as a leader and abort it while it is mid-fetch.
        let leader = tokio::spawn(async move {
            // Never-cancelled token: this caller leads and is then aborted by
            // dropping the JoinHandle.
            let cancel = CancellationToken::new();
            svc1.read(&req1, &cancel).await
        });
        // Let the leader claim leadership and enter the fetch.
        tokio::time::sleep(Duration::from_millis(20)).await;
        leader.abort();
        // A drop + await to observe the abort resolves the JoinHandle.
        let _ = leader.await;

        // A follower (now effectively the next leader) must complete the fetch
        // rather than wait forever on the aborted leader.
        let cancel = CancellationToken::new();
        let read = svc
            .read(&req, &cancel)
            .await
            .expect("follower promotes and completes");
        assert!(!read.served_stale);
        // The fetch ran at least once (the aborted leader may or may not have
        // completed before abort; the follower definitely ran it).
        assert!(calls.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn leader_abort_does_not_corrupt_concurrent_followers() {
        // Multiple followers wait on a leader that aborts; they must all
        // eventually observe a published result via promotion, none deadlocking.
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = Arc::new(QuotaService::with_ttl(clock, Duration::from_secs(60)));
        let mut adapter = MockAdapter::exact(1_000);
        adapter.delay_ms = 60;
        svc.register(ScopeMatch::any(), Arc::new(adapter));

        let req = request(QuotaWindow::Monthly);

        // Leader that we will abort mid-fetch.
        let svc_l = svc.clone();
        let req_l = req.clone();
        let leader = tokio::spawn(async move {
            let cancel = CancellationToken::new();
            svc_l.read(&req_l, &cancel).await
        });
        tokio::time::sleep(Duration::from_millis(15)).await;

        // Followers waiting on the leader.
        let mut followers = Vec::new();
        for _ in 0..4 {
            let svc_f = svc.clone();
            let req_f = req.clone();
            followers.push(tokio::spawn(async move {
                let cancel = CancellationToken::new();
                svc_f.read(&req_f, &cancel).await
            }));
        }

        // Abort the leader; followers must recover via promotion.
        leader.abort();
        let _ = leader.await;

        for f in followers {
            let read = f.await.expect("join").expect("follower recovers");
            assert!(!read.served_stale);
        }
    }

    #[tokio::test]
    async fn leader_caller_cancel_returns_promptly_instead_of_blocking() {
        // The LEADER's own caller cancels mid-fetch. It must return Cancelled
        // promptly — not stay blocked until the slow fetch finishes.
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = Arc::new(QuotaService::with_ttl(clock, Duration::from_secs(60)));
        let mut adapter = MockAdapter::exact(1_000);
        adapter.delay_ms = 400;
        svc.register(ScopeMatch::any(), Arc::new(adapter));

        let req = request(QuotaWindow::Monthly);
        let cancel = CancellationToken::new();

        let svc_l = svc.clone();
        let req_l = req.clone();
        let cancel_l = cancel.clone();
        let leader = tokio::spawn(async move { svc_l.read(&req_l, &cancel_l).await });

        // Let the leader claim leadership and enter the slow fetch.
        tokio::time::sleep(Duration::from_millis(30)).await;
        cancel.cancel();

        // Must resolve well before the 400 ms fetch would complete; if the
        // leader still blocked on `build_work` this timeout would fire.
        let err = tokio::time::timeout(Duration::from_millis(200), leader)
            .await
            .expect("leader returns Cancelled without blocking on the fetch")
            .expect("leader join")
            .expect_err("leader cancelled");
        assert!(err.iter().any(|f| matches!(f.error, QuotaError::Cancelled)));
    }

    #[tokio::test]
    async fn follower_promotes_after_leader_caller_cancel() {
        // Leader's caller cancels mid-fetch. A follower waiting on that flight
        // must promote, re-run the fetch, and publish — never hanging on the
        // cancelled leader.
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = Arc::new(QuotaService::with_ttl(clock, Duration::from_secs(60)));
        let mut adapter = MockAdapter::exact(1_000);
        adapter.delay_ms = 120;
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), Arc::new(adapter));

        let req = request(QuotaWindow::Monthly);
        let leader_cancel = CancellationToken::new();

        let svc_l = svc.clone();
        let req_l = req.clone();
        let leader_cancel_clone = leader_cancel.clone();
        let leader = tokio::spawn(async move { svc_l.read(&req_l, &leader_cancel_clone).await });
        // Leader claims leadership and enters the fetch.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Follower (separate, never-cancelled token) waits on the leader's
        // flight.
        let svc_f = svc.clone();
        let req_f = req.clone();
        let follower = tokio::spawn(async move {
            let cancel = CancellationToken::new();
            svc_f.read(&req_f, &cancel).await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        leader_cancel.cancel();
        let leader_err = leader
            .await
            .expect("leader join")
            .expect_err("leader cancelled");
        assert!(leader_err
            .iter()
            .any(|f| matches!(f.error, QuotaError::Cancelled)));

        // Follower promotes and completes — no hang.
        let read = tokio::time::timeout(Duration::from_secs(2), follower)
            .await
            .expect("follower promotes and completes without hanging")
            .expect("follower join")
            .expect("follower ok");
        assert!(!read.served_stale);
        assert!(calls.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn cache_populated_after_leader_cancel_then_follower_promotes() {
        // After the leader cancels and a follower promotes and completes, the
        // cache must hold a fresh entry: a subsequent read is a hit with no
        // extra fetch.
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = Arc::new(QuotaService::with_ttl(clock, Duration::from_secs(60)));
        let mut adapter = MockAdapter::exact(1_000);
        adapter.delay_ms = 80;
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), Arc::new(adapter));

        let req = request(QuotaWindow::Monthly);
        let leader_cancel = CancellationToken::new();

        let svc_l = svc.clone();
        let req_l = req.clone();
        let lc = leader_cancel.clone();
        let leader = tokio::spawn(async move { svc_l.read(&req_l, &lc).await });
        tokio::time::sleep(Duration::from_millis(20)).await;

        let svc_f = svc.clone();
        let req_f = req.clone();
        let follower = tokio::spawn(async move {
            let cancel = CancellationToken::new();
            svc_f.read(&req_f, &cancel).await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        leader_cancel.cancel();
        leader
            .await
            .expect("leader join")
            .expect_err("leader cancelled");
        tokio::time::timeout(Duration::from_secs(2), follower)
            .await
            .expect("follower completes")
            .expect("follower join")
            .expect("follower ok");

        let fetches = calls.load(Ordering::SeqCst);
        assert!(fetches >= 1);

        // Fresh cache hit: no additional fetch, not served stale.
        let cancel = CancellationToken::new();
        let read = svc.read(&req, &cancel).await.expect("cache hit read");
        assert!(!read.served_stale);
        assert_eq!(calls.load(Ordering::SeqCst), fetches);
    }

    #[async_trait]
    impl QuotaAdapter for CountingAdapter {
        fn kind(&self) -> AdapterKind {
            self.kind
        }
        fn supports(&self, _: &QuotaRequest) -> bool {
            true
        }
        async fn fetch(
            &self,
            request: &QuotaRequest,
            _: Option<&ResolvedCredential>,
            _: &CancellationToken,
        ) -> Result<QuotaSnapshot, QuotaError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(snapshot(
                &request.scope,
                request.window,
                request.unit.clone(),
                Confidence::Exact,
                self.kind,
                1_000,
            ))
        }
    }

    #[test]
    fn cache_key_is_full_scope_plus_window_and_unit() {
        let s1 = QuotaScope::new(
            TenantId::new("t"),
            AccountId::new("a"),
            ProviderId::new("p"),
            Some(ModelId::new("m1")),
        );
        let s2 = QuotaScope::new(
            TenantId::new("t"),
            AccountId::new("a"),
            ProviderId::new("p"),
            Some(ModelId::new("m2")),
        );
        let k1 = CacheKey {
            scope: s1,
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Token,
        };
        let k2 = CacheKey {
            scope: s2.clone(),
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Token,
        };
        assert_ne!(k1, k2);
        let k3 = CacheKey {
            scope: s2.clone(),
            window: QuotaWindow::Weekly,
            unit: QuotaUnit::Token,
        };
        let k4 = CacheKey {
            scope: s2.clone(),
            window: QuotaWindow::Weekly,
            unit: QuotaUnit::Cost {
                currency: "USD".into(),
            },
        };
        assert_ne!(k2, k3);
        assert_ne!(k3, k4);
        drop(s2);
    }

    #[test]
    fn scope_match_predicate_is_correct() {
        let m = ScopeMatch {
            tenant_id: Some(TenantId::new("t1")),
            provider_id: Some(ProviderId::new("p1")),
            ..ScopeMatch::default()
        };
        let yes = QuotaScope::new(
            TenantId::new("t1"),
            AccountId::new("a1"),
            ProviderId::new("p1"),
            None,
        );
        let no_tenant = QuotaScope::new(
            TenantId::new("t2"),
            AccountId::new("a1"),
            ProviderId::new("p1"),
            None,
        );
        let no_provider = QuotaScope::new(
            TenantId::new("t1"),
            AccountId::new("a1"),
            ProviderId::new("p2"),
            None,
        );
        assert!(m.matches(&yes));
        assert!(!m.matches(&no_tenant));
        assert!(!m.matches(&no_provider));
        assert!(ScopeMatch::any().matches(&yes));
    }

    // ----- cache-only reads (no adapter / no network / no singleflight) -----

    #[tokio::test]
    async fn read_cache_only_returns_no_data_when_cache_empty_and_never_calls_adapter() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));
        let adapter = Arc::new(MockAdapter::exact(1_000));
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), adapter);

        let req = request(QuotaWindow::Monthly);
        let read = svc.read_cache_only(&req).expect("empty cache ok");
        assert!(
            matches!(read, CacheRead::NoData),
            "empty cache must be NoData, got {read:?}"
        );
        // No adapter, network, or singleflight involvement.
        assert_eq!(calls.load(Ordering::SeqCst), 0, "must not call any adapter");
        assert_eq!(svc.cache_size(), 0);
    }

    #[tokio::test]
    async fn read_cache_only_returns_no_data_for_stale_entry_and_never_calls_adapter() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock.clone(), Duration::from_millis(500));
        let adapter = Arc::new(MockAdapter::exact(1_000));
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), adapter);

        // Seed the cache with one real read, then age it past TTL.
        let req = request(QuotaWindow::Monthly);
        let cancel = CancellationToken::new();
        let seeded = svc.read(&req, &cancel).await.expect("seed ok");
        assert!(!seeded.served_stale);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let seed_calls = calls.load(Ordering::SeqCst);

        clock.advance(5_000); // well past the 500ms TTL
        let read = svc.read_cache_only(&req).expect("stale cache ok");
        // A stale entry surfaces as Stale (carrying the snapshot) — distinct
        // from an empty cache (NoData) — but it MUST NOT trigger a refetch.
        match read {
            CacheRead::Stale { .. } => {}
            other => panic!("expected Stale for aged entry, got {other:?}"),
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            seed_calls,
            "cache-only read must never call the adapter, even when stale"
        );
    }

    #[tokio::test]
    async fn read_cache_only_returns_from_cache_hit_without_calling_adapter() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));
        let adapter = Arc::new(MockAdapter::exact(1_000));
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), adapter);

        // Seed the cache.
        let req = request(QuotaWindow::Monthly);
        let cancel = CancellationToken::new();
        svc.read(&req, &cancel).await.expect("seed ok");
        let seed_calls = calls.load(Ordering::SeqCst);

        // Subsequent cache-only reads are pure cache hits.
        for _ in 0..5 {
            let read = svc.read_cache_only(&req).expect("hit ok");
            match read {
                CacheRead::Hit { snapshot } => {
                    assert_eq!(snapshot.window, QuotaWindow::Monthly);
                    assert!(!snapshot.provenance.stale);
                }
                other => panic!("expected Hit, got {other:?}"),
            }
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            seed_calls,
            "cache-only hits must never call the adapter"
        );
    }

    #[tokio::test]
    async fn overview_cache_only_does_not_fetch_and_reports_per_window_state() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock.clone(), Duration::from_secs(60));
        let adapter = Arc::new(MockAdapter::exact(1_000));
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), adapter);

        let cancel = CancellationToken::new();
        // Seed only the Monthly window via a normal read.
        let monthly = QuotaRequest {
            scope: scope("anthropic"),
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Token,
        };
        svc.read(&monthly, &cancel).await.expect("seed monthly");
        let seed_calls = calls.load(Ordering::SeqCst);

        let overview = svc
            .overview_cache_only(
                &scope("anthropic"),
                &[QuotaWindow::Monthly, QuotaWindow::Weekly],
                &QuotaUnit::Token,
            )
            .expect("overview ok");
        assert_eq!(overview.hit_count(), 1);
        assert!(matches!(
            overview.windows.get(&QuotaWindow::Monthly),
            Some(CacheRead::Hit { .. })
        ));
        assert!(matches!(
            overview.windows.get(&QuotaWindow::Weekly),
            Some(CacheRead::NoData)
        ));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            seed_calls,
            "overview_cache_only must never fetch"
        );
    }

    #[tokio::test]
    async fn read_cache_only_validates_scope_and_does_not_touch_cache() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));
        let adapter = Arc::new(MockAdapter::exact(1_000));
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), adapter);

        let req = QuotaRequest {
            scope: QuotaScope::new(
                TenantId::new(""),
                AccountId::new("acc"),
                ProviderId::new("anthropic"),
                None,
            ),
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Token,
        };
        let err = svc.read_cache_only(&req).expect_err("rejected");
        assert!(matches!(err, QuotaError::Unsupported { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(svc.cache_size(), 0);
    }

    // =====================================================================
    // Ledger reconcile integration (P14-7 step 2): remote Exact baseline +
    // strict local-ledger increment, plus local snapshot publishing.
    // =====================================================================

    /// Seed a token usage record matching the service test scope
    /// (tenant-a / account-1 / anthropic / model-x) at `occurred_at_ms`.
    async fn seed_usage(ledger: &Arc<dyn UsageLedger>, occurred_at_ms: u64, input_tokens: u64) {
        ledger
            .record(UsageRecord {
                tenant_id: TenantId::new("tenant-a"),
                account_id: "account-1".to_string(),
                provider_id: ProviderId::new("anthropic"),
                model_id: ModelId::new("model-x"),
                input_tokens,
                occurred_at_ms,
                currency: "USD".to_string(),
                ..Default::default()
            })
            .await
            .expect("record ok");
    }

    #[tokio::test]
    async fn reconcile_overlays_ledger_delta_on_exact_remote() {
        // A fresh Exact remote baseline + a non-zero ledger increment: the
        // served snapshot becomes Derived, credited to the ledger overlay.
        let clock = Arc::new(MutableQuotaClock::at(10_000));
        let svc = Arc::new(QuotaService::with_ttl(
            clock.clone(),
            Duration::from_secs(60),
        ));
        // fetched_at (1_000) is strictly before now (10_000), so reconcile runs.
        let adapter = MockAdapter::exact(1_000);
        let calls = adapter.calls.clone();
        svc.register(ScopeMatch::any(), Arc::new(adapter));

        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        // Record strictly inside (fetched_at, now): delta = 100 tokens.
        seed_usage(&ledger, 5_000, 100).await;
        svc.set_ledger_reconciler(Arc::new(LedgerQuotaAdapter::new(ledger, clock)));

        let req = request(QuotaWindow::Monthly);
        let cancel = CancellationToken::new();
        let read = svc.read(&req, &cancel).await.expect("ok");
        assert_eq!(read.snapshot.confidence, Confidence::Derived);
        assert_eq!(
            read.snapshot.provenance.adapter_kind,
            AdapterKind::LocalLedger
        );
        assert_eq!(read.snapshot.values.used, QuotaMeasure::exact(125)); // 25 + 100
        assert_eq!(read.snapshot.values.remaining, QuotaMeasure::exact(0)); // 75 saturating 100
        assert_eq!(read.snapshot.values.limit, QuotaMeasure::exact(100));
        assert!(
            read.failures.is_empty(),
            "successful overlay adds no advisory failure"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reconcile_zero_delta_keeps_remote_exact() {
        // delta == 0: the Exact remote baseline passes through unchanged.
        let clock = Arc::new(MutableQuotaClock::at(10_000));
        let svc = QuotaService::with_ttl(clock.clone(), Duration::from_secs(60));
        svc.register(ScopeMatch::any(), Arc::new(MockAdapter::exact(1_000)));

        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        svc.set_ledger_reconciler(Arc::new(LedgerQuotaAdapter::new(ledger, clock)));

        let req = request(QuotaWindow::Monthly);
        let cancel = CancellationToken::new();
        let read = svc.read(&req, &cancel).await.expect("ok");
        assert_eq!(read.snapshot.confidence, Confidence::Exact);
        assert_eq!(
            read.snapshot.provenance.adapter_kind,
            AdapterKind::ApiKeyApi
        );
        assert_eq!(read.snapshot.values.used, QuotaMeasure::exact(25));
        assert!(read.failures.is_empty());
    }

    #[tokio::test]
    async fn reconcile_failure_keeps_remote_and_appends_local_failure() {
        // fetched_at (1_000) == now (1_000): reconcile rejects the baseline.
        // The remote Exact read must still succeed, with an advisory
        // LocalLedger failure appended rather than discarding the remote.
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock.clone(), Duration::from_secs(60));
        svc.register(ScopeMatch::any(), Arc::new(MockAdapter::exact(1_000)));

        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        svc.set_ledger_reconciler(Arc::new(LedgerQuotaAdapter::new(ledger, clock)));

        let req = request(QuotaWindow::Monthly);
        let cancel = CancellationToken::new();
        let read = svc.read(&req, &cancel).await.expect("remote still served");
        assert_eq!(read.snapshot.confidence, Confidence::Exact);
        assert_eq!(
            read.snapshot.provenance.adapter_kind,
            AdapterKind::ApiKeyApi
        );
        assert_eq!(read.failures.len(), 1);
        assert_eq!(
            read.failures[0].adapter_kind,
            Some(AdapterKind::LocalLedger)
        );
    }

    #[tokio::test]
    async fn reconcile_does_not_overlay_scraped_baseline() {
        // Scraped (and likewise Derived / LocalLedger) baselines are not
        // re-overlaid: the reconciler is not invoked, so no advisory failure.
        let clock = Arc::new(MutableQuotaClock::at(10_000));
        let svc = QuotaService::with_ttl(clock.clone(), Duration::from_secs(60));
        let mut adapter = MockAdapter::exact(1_000);
        adapter.confidence = Confidence::Scraped;
        svc.register(ScopeMatch::any(), Arc::new(adapter));

        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        seed_usage(&ledger, 5_000, 100).await;
        svc.set_ledger_reconciler(Arc::new(LedgerQuotaAdapter::new(ledger, clock)));

        let req = request(QuotaWindow::Monthly);
        let cancel = CancellationToken::new();
        let read = svc.read(&req, &cancel).await.expect("ok");
        assert_eq!(
            read.snapshot.confidence,
            Confidence::Scraped,
            "scraped baseline is not overlaid"
        );
        assert_eq!(
            read.snapshot.provenance.adapter_kind,
            AdapterKind::ApiKeyApi
        );
        assert!(
            read.failures.is_empty(),
            "reconcile must not be attempted on a scraped baseline"
        );
    }

    #[tokio::test]
    async fn publish_local_snapshot_rejects_raw_remote() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));
        let snap = snapshot(
            &scope("anthropic"),
            QuotaWindow::Monthly,
            QuotaUnit::Token,
            Confidence::Exact,
            AdapterKind::ApiKeyApi,
            1_000,
        );
        let err = svc
            .publish_local_snapshot(snap)
            .expect_err("raw remote rejected");
        assert!(matches!(err, QuotaError::Unsupported { .. }));
        assert_eq!(svc.cache_size(), 0);
    }

    #[tokio::test]
    async fn publish_local_snapshot_accepts_derived_and_isolates_scope() {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = QuotaService::with_ttl(clock, Duration::from_secs(60));

        // Full-scope isolation keys on credential_id: same tenant/account/
        // provider/model, different credentials.
        let scope_a = scope("anthropic").with_credential_id("cred-a");
        let scope_b = scope("anthropic").with_credential_id("cred-b");

        let snap_a = snapshot(
            &scope_a,
            QuotaWindow::Monthly,
            QuotaUnit::Token,
            Confidence::Derived,
            AdapterKind::LocalLedger,
            1_000,
        );
        let snap_b = snapshot(
            &scope_b,
            QuotaWindow::Overall,
            QuotaUnit::Token,
            Confidence::Derived,
            AdapterKind::LocalLedger,
            1_000,
        );
        svc.publish_local_snapshot(snap_a.clone())
            .expect("derived accepted");
        svc.publish_local_snapshot(snap_b)
            .expect("derived accepted");
        assert_eq!(svc.cache_size(), 2);

        // Each full scope sees only its own snapshots, across all windows/units.
        let a = svc.cached_snapshots_for_scope(&scope_a);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0], snap_a);
        let b = svc.cached_snapshots_for_scope(&scope_b);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].window, QuotaWindow::Overall);

        // A sibling credential sees nothing.
        let scope_c = scope("anthropic").with_credential_id("cred-c");
        assert!(svc.cached_snapshots_for_scope(&scope_c).is_empty());
    }
}
