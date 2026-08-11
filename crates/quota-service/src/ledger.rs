//! Usage Ledger-derived quota snapshots (P14-7).
//!
//! [`LedgerQuotaAdapter`] bridges the canonical Usage/Cost Ledger
//! ([`usage_ledger::UsageLedger`]) into the quota domain. It is the single
//! place that derives `used`/`limit`/`remaining` from local usage facts,
//! so there is never a second usage accumulator: the ledger remains the only
//! source of truth for usage and cost.
//!
//! Derivation rules:
//! - `used` = saturated sum of the ledger's usage totals within the window
//!   half-open time range, restricted to the request's currency (Cost) or
//!   token totals (Token). Records with a mismatched currency are excluded
//!   from cost aggregation, which keeps the projection currency-homogeneous
//!   ahead of a future `aggregate -> Result` change to the ledger trait.
//! - `limit` is an optional configured budget; when absent it stays
//!   [`QuotaMeasure::Infinite`] and only `used` is derived.
//! - `remaining` = `limit.saturating_sub(used)`, or `Infinite` when limit is
//!   `Infinite`.
//!
//! Confidence is always [`Confidence::Derived`]; provenance names the ledger
//! so the source/confidence/staleness chain stays visible and exact remote
//! reads still outrank this adapter via service source-priority ordering.
//!
//! Reconcile ([`LedgerQuotaAdapter::reconcile`]) overlays a provider baseline
//! with the ledger delta: records scoped to the baseline's full
//! tenant/account/credential/provider/model, window and currency, occurring
//! strictly after the baseline `fetched_at` and strictly before now. Zero
//! delta passes the baseline through untouched (an `Exact` remote stays
//! `Exact`); a positive delta advances `used`/`remaining` with checked
//! arithmetic (overflow → [`QuotaError::Parse`]), marks the result `Derived`,
//! preserves `reset`, and names both sources in provenance. Baselines that are
//! already ledger-derived or not strictly in the past are rejected so usage is
//! never overlaid twice.
//!
//! The adapter never accumulates state itself — every `fetch` re-queries the
//! ledger, so ledger replay/idempotency guarantees carry through unchanged.

use std::sync::Arc;

use agent_domain::{CancellationToken, Timestamp};
use async_trait::async_trait;
use usage_ledger::{UsageLedger, UsageQuery};

use crate::{
    AdapterKind, Confidence, QuotaAdapter, QuotaError, QuotaMeasure, QuotaProvenance, QuotaRequest,
    QuotaReset, QuotaSnapshot, QuotaUnit, QuotaValues,
};

/// Optional budget cap used to project `limit` / `remaining` for a scope.
///
/// Wrap in `Arc` if shared; otherwise clone cheaply per registration.
#[derive(Clone, Debug, Default)]
pub struct BudgetCap {
    /// Cap expressed in the same unit as the request. `None` = no cap.
    pub limit: Option<QuotaMeasure>,
}

impl BudgetCap {
    pub fn none() -> Self {
        Self { limit: None }
    }

    pub fn with_limit(limit: QuotaMeasure) -> Self {
        Self { limit: Some(limit) }
    }
}

/// Window length in milliseconds, used to compute the half-open time range
/// `[now - len, now)` for ledger queries. [`QuotaWindow::Overall`] uses
/// `[0, now)` (the full history).
fn window_length_ms(window: crate::QuotaWindow) -> Option<u64> {
    match window {
        crate::QuotaWindow::Overall => None,
        crate::QuotaWindow::Rolling5h => Some(5 * 60 * 60 * 1000),
        crate::QuotaWindow::Weekly => Some(7 * 24 * 60 * 60 * 1000),
        crate::QuotaWindow::Monthly => Some(30 * 24 * 60 * 60 * 1000),
    }
}

/// Wall-clock source used to anchor rolling window bounds. Re-exported from
/// the service so the refresh scheduler and the ledger adapter share one clock.
pub use crate::service::QuotaClock;

/// Adapter that derives quota from a Usage/Cost Ledger.
///
/// Clone-safe: it holds the ledger by `Arc<dyn UsageLedger>` and an immutable
/// budget cap. Multiple adapters with different caps can coexist by being
/// registered under different [`crate::service::ScopeMatch`] predicates.
pub struct LedgerQuotaAdapter {
    ledger: Arc<dyn UsageLedger>,
    clock: Arc<dyn QuotaClock>,
    cap: BudgetCap,
}

impl LedgerQuotaAdapter {
    pub fn new(ledger: Arc<dyn UsageLedger>, clock: Arc<dyn QuotaClock>) -> Self {
        Self {
            ledger,
            clock,
            cap: BudgetCap::none(),
        }
    }

    pub fn with_budget(
        ledger: Arc<dyn UsageLedger>,
        clock: Arc<dyn QuotaClock>,
        cap: BudgetCap,
    ) -> Self {
        Self { ledger, clock, cap }
    }

    /// Overlay the local ledger delta on top of a remote baseline snapshot
    /// (P14-7 step 2: 远端 / 本地对照).
    ///
    /// The remote snapshot is the baseline as observed at
    /// `remote.provenance.fetched_at`. Records are pulled from the ledger
    /// scoped to the remote's full tenant/account/credential/provider/model,
    /// bounded by the remote's window as of now, restricted to the requested
    /// currency (Cost), and occurring strictly after `fetched_at` and strictly
    /// before now — records at the boundary timestamps are never overlaid, so
    /// repeating a reconcile with the same baseline cannot double-count.
    ///
    /// - A baseline that is already [`AdapterKind::LocalLedger`], or whose
    ///   `fetched_at` is not strictly before now, is rejected with
    ///   [`QuotaError::Other`]: overlaying it would double-count usage the
    ///   ledger already contributed or apply an empty/invalid delta window.
    /// - Zero delta returns the remote snapshot unchanged — an `Exact` remote
    ///   stays `Exact`.
    /// - Positive delta updates `used` (Count: record count; Token/Cost:
    ///   checked sums, overflow → [`QuotaError::Parse`]) and `remaining`
    ///   (saturating at zero once exhausted). `Unknown` baselines are never
    ///   guessed; `limit` and `reset` are preserved; confidence becomes
    ///   [`Confidence::Derived`] and provenance names both the remote source
    ///   and the ledger without carrying secrets.
    pub async fn reconcile(
        &self,
        remote: &QuotaSnapshot,
        cancel: &CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError> {
        if cancel.is_cancelled() {
            return Err(QuotaError::Cancelled);
        }
        if remote.provenance.adapter_kind == AdapterKind::LocalLedger {
            return Err(QuotaError::other(
                "reconcile baseline is already ledger-derived; refusing to overlay twice",
            ));
        }
        let now = self.clock.now();
        if remote.provenance.fetched_at.as_unix_millis() >= now.as_unix_millis() {
            return Err(QuotaError::other(
                "reconcile baseline fetched_at is not strictly before now",
            ));
        }

        let delta = self.delta_since_fetch(remote, now.as_unix_millis()).await?;

        if cancel.is_cancelled() {
            return Err(QuotaError::Cancelled);
        }

        if delta == 0 {
            return Ok(remote.clone());
        }

        let values = overlay_values(&remote.values, delta)?;
        let provenance = QuotaProvenance {
            adapter_kind: AdapterKind::LocalLedger,
            source: format!("remote:{} + usage-ledger", remote.provenance.source),
            endpoint: remote.provenance.endpoint.clone(),
            fetched_at: now,
            observed_at: Some(remote.provenance.fetched_at),
            selector_version: None,
            stale: remote.provenance.stale,
        };

        Ok(QuotaSnapshot {
            scope: remote.scope.clone(),
            window: remote.window,
            unit: remote.unit.clone(),
            values,
            reset: remote.reset,
            confidence: Confidence::Derived,
            provenance,
        })
    }

    /// Local usage strictly after the remote baseline `fetched_at` and
    /// strictly before `now_ms`, bounded by the remote's window as of
    /// `now_ms`.
    async fn delta_since_fetch(
        &self,
        remote: &QuotaSnapshot,
        now_ms: u64,
    ) -> Result<u64, QuotaError> {
        let fetched_at = remote.provenance.fetched_at.as_unix_millis();
        // Half-open ledger range [start, now_ms): `fetched_at + 1` is the
        // first millisecond strictly after the baseline, and the end bound
        // excludes records at exactly `now`. The window bound keeps a stale
        // baseline from pulling in usage outside the current window.
        let start_ms = window_start_ms(remote.window, now_ms).max(fetched_at.saturating_add(1));
        let query = UsageQuery {
            tenant_id: Some(remote.scope.tenant_id.clone()),
            account_id: Some(remote.scope.account_id.as_str().to_string()),
            credential_id: remote.scope.credential_id.clone(),
            provider_id: Some(remote.scope.provider_id.clone()),
            model_id: remote.scope.model_id.clone(),
            occurred_at_start_ms: Some(start_ms),
            occurred_at_end_ms: Some(now_ms),
            ..UsageQuery::default()
        };
        let records = self.ledger.query(&query).await;

        match &remote.unit {
            QuotaUnit::Count => Ok(records.len() as u64),
            QuotaUnit::Token => {
                let mut total = 0u64;
                for record in &records {
                    let record_tokens = [
                        record.input_tokens,
                        record.output_tokens,
                        record.cache_read_tokens,
                        record.cache_write_tokens,
                    ]
                    .into_iter()
                    .try_fold(0u64, |acc, value| {
                        acc.checked_add(value).ok_or_else(|| {
                            QuotaError::parse(
                                "reconcile token overflow: record token sum exceeds u64",
                            )
                        })
                    })?;
                    total = total.checked_add(record_tokens).ok_or_else(|| {
                        QuotaError::parse("reconcile token overflow: delta exceeds u64")
                    })?;
                }
                Ok(total)
            }
            QuotaUnit::Cost { currency } => {
                let mut total = 0u64;
                for record in records
                    .iter()
                    .filter(|record| record.currency.eq_ignore_ascii_case(currency))
                {
                    total = total.checked_add(record.cost_micros).ok_or_else(|| {
                        QuotaError::parse("reconcile cost overflow: delta exceeds u64")
                    })?;
                }
                Ok(total)
            }
        }
    }
}

#[async_trait]
impl QuotaAdapter for LedgerQuotaAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::LocalLedger
    }

    /// Supports every (scope, window, unit) combination. The aggregator still
    /// ranks Exact remote reads above this Derived adapter, so an unsupported
    /// remote capability naturally surfaces as Derived here when no exact
    /// source exists.
    fn supports(&self, _request: &QuotaRequest) -> bool {
        true
    }

    async fn fetch(
        &self,
        request: &QuotaRequest,
        _credential: Option<&provider_api::ResolvedCredential>,
        cancel: &CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError> {
        if cancel.is_cancelled() {
            return Err(QuotaError::Cancelled);
        }

        let now = self.clock.now();
        let now_ms = now.as_unix_millis();
        let window_start = window_start_ms(request.window, now_ms);
        // Half-open query end `[window_start, now_ms + 1)`: a record observed
        // at exactly `now` is part of this local snapshot, while one at
        // `now + 1` is not. At u64::MAX there is no representable `now + 1`,
        // so the end bound degrades to unbounded — every u64 timestamp is
        // strictly below MAX + 1, so the snapshot stays complete.
        let query = build_query(request, window_start, now_ms.checked_add(1));

        // Pull the raw records once and derive used/limit/remaining directly.
        // We deliberately bypass `aggregate`: it now rejects mixed-currency
        // result sets (returning `MixedCurrencies`) regardless of unit, which
        // would break Token/Count derivation for accounts that legitimately
        // hold records in more than one currency. Token totals are
        // currency-agnostic; cost totals are restricted to the requested
        // currency by post-filtering, keeping every projection
        // currency-homogeneous and robust to any future `aggregate` change.
        let records = self.ledger.query(&query).await;

        if cancel.is_cancelled() {
            return Err(QuotaError::Cancelled);
        }

        let used = match request.unit {
            QuotaUnit::Count => {
                QuotaMeasure::exact(u64::try_from(records.len()).map_err(|_| {
                    QuotaError::parse("ledger record count exceeds canonical u64 range")
                })?)
            }
            QuotaUnit::Token => {
                let tokens = records
                    .iter()
                    .map(|r| {
                        r.input_tokens
                            .saturating_add(r.output_tokens)
                            .saturating_add(r.cache_read_tokens)
                            .saturating_add(r.cache_write_tokens)
                    })
                    .fold(0u64, |acc, v| acc.saturating_add(v));
                QuotaMeasure::exact(tokens)
            }
            QuotaUnit::Cost { ref currency } => {
                let cost_micros = records
                    .iter()
                    .filter(|r| r.currency.eq_ignore_ascii_case(currency))
                    .map(|r| r.cost_micros)
                    .fold(0u64, |acc, v| acc.saturating_add(v));
                QuotaMeasure::exact(cost_micros)
            }
        };

        let limit = self.cap.limit.unwrap_or(QuotaMeasure::Infinite);
        let remaining = match (limit, used) {
            (QuotaMeasure::Infinite, _) => QuotaMeasure::Infinite,
            (QuotaMeasure::Unknown, _) => QuotaMeasure::Unknown,
            (QuotaMeasure::Exact(limit_v), QuotaMeasure::Exact(used_v)) => {
                QuotaMeasure::exact(limit_v.saturating_sub(used_v))
            }
            // Non-finite used against a finite limit yields Unknown rather
            // than a fabricated value.
            (QuotaMeasure::Exact(_), QuotaMeasure::Infinite | QuotaMeasure::Unknown) => {
                QuotaMeasure::Unknown
            }
        };

        let reset = window_reset(request.window, now);
        let provenance = QuotaProvenance::new(AdapterKind::LocalLedger, "usage-ledger", now);

        Ok(QuotaSnapshot {
            scope: request.scope.clone(),
            window: request.window,
            unit: request.unit.clone(),
            values: QuotaValues {
                used,
                limit,
                remaining,
            },
            reset,
            confidence: Confidence::Derived,
            provenance,
        })
    }
}

/// Half-open window start in Unix milliseconds. `Overall` starts at 0.
fn window_start_ms(window: crate::QuotaWindow, now_ms: u64) -> u64 {
    match window_length_ms(window) {
        Some(len) => now_ms.saturating_sub(len),
        None => 0,
    }
}

/// Build a half-open ledger query `[start_ms, end_ms)` restricted to the
/// request's scope. `end_ms == None` leaves the end unbounded, the exact
/// equivalent of `[start_ms, u64::MAX + 1)` over u64 timestamps.
fn build_query(request: &QuotaRequest, start_ms: u64, end_ms: Option<u64>) -> UsageQuery {
    UsageQuery {
        tenant_id: Some(request.scope.tenant_id.clone()),
        account_id: Some(request.scope.account_id.as_str().to_string()),
        credential_id: request.scope.credential_id.clone(),
        provider_id: Some(request.scope.provider_id.clone()),
        model_id: request.scope.model_id.clone(),
        occurred_at_start_ms: Some(start_ms),
        occurred_at_end_ms: end_ms,
        ..UsageQuery::default()
    }
}

/// Add a positive ledger delta to a remote baseline's values.
///
/// `used` only advances when the baseline is `Exact` — `Unknown`/`Infinite`
/// baselines are never guessed. `remaining` subtracts the delta with
/// saturating semantics, so an exhausted quota zeroes out instead of
/// underflowing; `limit` is preserved untouched.
fn overlay_values(remote: &QuotaValues, delta: u64) -> Result<QuotaValues, QuotaError> {
    let used = match remote.used {
        QuotaMeasure::Exact(value) => QuotaMeasure::Exact(
            value
                .checked_add(delta)
                .ok_or_else(|| QuotaError::parse("reconcile overflow: used exceeds u64"))?,
        ),
        other => other,
    };
    let remaining = match remote.remaining {
        QuotaMeasure::Exact(value) => QuotaMeasure::Exact(value.saturating_sub(delta)),
        other => other,
    };
    Ok(QuotaValues {
        used,
        limit: remote.limit,
        remaining,
    })
}

/// Rolling windows reset at a deterministic relative offset; `Overall` never
/// resets (Unknown). All rolling resets are marked `uncertain=true` because
/// the boundary is an approximation (e.g. a 30-day "monthly" window is not a
/// calendar month).
fn window_reset(window: crate::QuotaWindow, now: Timestamp) -> QuotaReset {
    match window_length_ms(window) {
        Some(len) => QuotaReset::Relative {
            after_secs: len / 1000,
            observed_at: now,
            uncertain: true,
        },
        None => QuotaReset::Unknown,
    }
}

// =========================================================================
// Exhaustion prediction (P14-7 step 3)
// =========================================================================

/// Predicted time-to-exhaustion for a derived quota.
///
/// `None` means no prediction is possible (infinite/unknown limit or zero
/// observed rate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExhaustionPrediction {
    /// Seconds until `used` reaches `limit` at the observed rate.
    pub seconds_until_exhausted: u64,
    /// Confidence is always `Derived` because the rate is extrapolated.
    pub uncertain: bool,
}

/// Predict when `used` will reach `limit`, given an observed usage rate.
///
/// `used_per_second` must be non-negative; zero yields `None`. Limit must be a
/// finite `Exact` measure; `Infinite`/`Unknown` yield `None`.
pub fn predict_exhaustion(
    used: QuotaMeasure,
    limit: QuotaMeasure,
    used_per_second: u64,
) -> Option<ExhaustionPrediction> {
    let limit_v = match limit {
        QuotaMeasure::Exact(v) => v,
        QuotaMeasure::Infinite | QuotaMeasure::Unknown => return None,
    };
    if used_per_second == 0 {
        return None;
    }
    let used_v = match used {
        QuotaMeasure::Exact(v) => v,
        // Infinite used is already exhausted; Unknown used blocks prediction.
        QuotaMeasure::Infinite | QuotaMeasure::Unknown => return None,
    };
    let remaining = limit_v.checked_sub(used_v)?;
    let seconds = remaining.checked_div(used_per_second)?;
    Some(ExhaustionPrediction {
        seconds_until_exhausted: seconds,
        uncertain: true,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_domain::{CancellationToken, ModelId, ProviderId, TenantId, Timestamp};
    use usage_ledger::{InMemoryUsageLedger, UsageRecord};

    use super::*;
    use crate::{AccountId, QuotaScope, QuotaWindow};

    fn clock_at(ms: u64) -> Arc<dyn QuotaClock> {
        Arc::new(crate::service::MutableQuotaClock::at(ms))
    }

    fn scope() -> QuotaScope {
        QuotaScope::new(
            TenantId::new("tenant-a"),
            AccountId::new("account-1"),
            ProviderId::new("anthropic"),
            Some(ModelId::new("claude-opus")),
        )
    }

    async fn seed_record(
        ledger: &Arc<dyn UsageLedger>,
        occurred_at_ms: u64,
        input_tokens: u64,
        output_tokens: u64,
        cost_micros: u64,
        currency: &str,
    ) {
        let record = UsageRecord {
            tenant_id: TenantId::new("tenant-a"),
            account_id: "account-1".to_string(),
            provider_id: ProviderId::new("anthropic"),
            model_id: ModelId::new("claude-opus"),
            input_tokens,
            output_tokens,
            cost_micros,
            currency: currency.to_string(),
            occurred_at_ms,
            ..Default::default()
        };
        ledger.record(record).await.expect("record ok");
    }

    /// Seed a record with an optional credential scope (used by reconcile
    /// isolation tests, where the baseline may pin a credential).
    async fn seed_record_full(
        ledger: &Arc<dyn UsageLedger>,
        occurred_at_ms: u64,
        credential_id: Option<&str>,
        input_tokens: u64,
        cost_micros: u64,
        currency: &str,
    ) {
        let record = UsageRecord {
            tenant_id: TenantId::new("tenant-a"),
            account_id: "account-1".to_string(),
            credential_id: credential_id.map(str::to_string),
            provider_id: ProviderId::new("anthropic"),
            model_id: ModelId::new("claude-opus"),
            input_tokens,
            cost_micros,
            currency: currency.to_string(),
            occurred_at_ms,
            ..Default::default()
        };
        ledger.record(record).await.expect("record ok");
    }

    fn remote_snapshot(unit: QuotaUnit) -> QuotaSnapshot {
        QuotaSnapshot {
            scope: scope(),
            window: QuotaWindow::Overall,
            unit,
            values: QuotaValues::new(
                QuotaMeasure::exact(1_000),
                QuotaMeasure::exact(10_000),
                QuotaMeasure::exact(9_000),
            ),
            reset: QuotaReset::Absolute {
                at: Timestamp::from_unix_millis(100),
                uncertain: false,
            },
            confidence: Confidence::Exact,
            provenance: QuotaProvenance::new(
                AdapterKind::ApiKeyApi,
                "anthropic.admin",
                Timestamp::from_unix_millis(5_000),
            ),
        }
    }

    async fn reconcile(
        remote: &QuotaSnapshot,
        ledger: Arc<dyn UsageLedger>,
        now_ms: u64,
    ) -> Result<QuotaSnapshot, QuotaError> {
        let adapter = LedgerQuotaAdapter::new(ledger, clock_at(now_ms));
        adapter.reconcile(remote, &CancellationToken::new()).await
    }

    #[tokio::test]
    async fn token_derivation_sums_window_and_marks_derived() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let clock = clock_at(10_000_000);
        // Two records inside [0, now), different token fields.
        seed_record(&ledger, 1_000, 100, 50, 0, "USD").await;
        seed_record(&ledger, 5_000, 200, 10, 0, "USD").await;

        let adapter = LedgerQuotaAdapter::new(ledger, clock);
        let request = QuotaRequest {
            scope: scope(),
            window: QuotaWindow::Overall,
            unit: QuotaUnit::Token,
        };
        let snap = adapter
            .fetch(&request, None, &CancellationToken::new())
            .await
            .expect("ok");
        assert_eq!(snap.confidence, Confidence::Derived);
        assert_eq!(snap.provenance.adapter_kind, AdapterKind::LocalLedger);
        assert_eq!(snap.values.used, QuotaMeasure::exact(360)); // 100+50+200+10
        assert_eq!(snap.values.limit, QuotaMeasure::Infinite);
        assert_eq!(snap.values.remaining, QuotaMeasure::Infinite);
        assert_eq!(snap.reset, QuotaReset::Unknown);
    }

    #[tokio::test]
    async fn count_derivation_counts_only_records_in_scope() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        seed_record(&ledger, 1_000, 1, 0, 0, "USD").await;
        seed_record(&ledger, 2_000, 1, 0, 0, "USD").await;
        ledger
            .record(UsageRecord {
                tenant_id: TenantId::new("tenant-b"),
                account_id: "account-1".to_string(),
                provider_id: ProviderId::new("anthropic"),
                model_id: ModelId::new("claude-opus"),
                input_tokens: 1,
                currency: "USD".to_string(),
                occurred_at_ms: 3_000,
                ..Default::default()
            })
            .await
            .expect("sibling tenant record");

        let adapter = LedgerQuotaAdapter::new(ledger, clock_at(10_000));
        let snapshot = adapter
            .fetch(
                &QuotaRequest {
                    scope: scope(),
                    window: QuotaWindow::Overall,
                    unit: QuotaUnit::Count,
                },
                None,
                &CancellationToken::new(),
            )
            .await
            .expect("count projection");

        assert_eq!(snapshot.values.used, QuotaMeasure::exact(2));
    }

    #[tokio::test]
    async fn cost_derivation_filters_by_currency() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let clock = clock_at(10_000_000);
        seed_record(&ledger, 1_000, 10, 5, 1_000, "USD").await;
        // Foreign-currency record must be excluded from USD aggregation.
        seed_record(&ledger, 2_000, 10, 5, 9_999, "EUR").await;
        // A second foreign currency must also be excluded from USD aggregation.
        seed_record(&ledger, 3_000, 10, 5, 9_999, "JPY").await;

        let adapter = LedgerQuotaAdapter::new(ledger, clock);
        let request = QuotaRequest {
            scope: scope(),
            window: QuotaWindow::Overall,
            unit: QuotaUnit::Cost {
                currency: "USD".into(),
            },
        };
        let snap = adapter
            .fetch(&request, None, &CancellationToken::new())
            .await
            .expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::exact(1_000));
    }

    #[tokio::test]
    async fn fetch_includes_record_observed_exactly_at_now() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let now_ms = 10_000_000;
        // `occurred_at_ms == clock.now()` is part of the local snapshot: the
        // half-open query end is `now + 1`, not `now`.
        seed_record(&ledger, now_ms, 1, 0, 0, "USD").await;

        let adapter = LedgerQuotaAdapter::new(ledger, clock_at(now_ms));
        let snap = adapter
            .fetch(
                &QuotaRequest {
                    scope: scope(),
                    window: QuotaWindow::Overall,
                    unit: QuotaUnit::Count,
                },
                None,
                &CancellationToken::new(),
            )
            .await
            .expect("ok");

        assert_eq!(snap.values.used, QuotaMeasure::exact(1));
    }

    #[tokio::test]
    async fn fetch_excludes_record_observed_at_now_plus_one() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let now_ms = 10_000_000;
        // `occurred_at_ms == now + 1` must stay outside the snapshot.
        seed_record(&ledger, now_ms + 1, 1, 0, 0, "USD").await;

        let adapter = LedgerQuotaAdapter::new(ledger, clock_at(now_ms));
        let snap = adapter
            .fetch(
                &QuotaRequest {
                    scope: scope(),
                    window: QuotaWindow::Overall,
                    unit: QuotaUnit::Count,
                },
                None,
                &CancellationToken::new(),
            )
            .await
            .expect("ok");

        assert_eq!(snap.values.used, QuotaMeasure::exact(0));
    }

    #[tokio::test]
    async fn fetch_at_u64_max_includes_max_timestamp_record() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        // Clock at u64::MAX has no representable `now + 1`; the query end
        // degrades to unbounded so the record at exactly u64::MAX still
        // counts (and no record time is fabricated).
        seed_record(&ledger, u64::MAX - 1, 2, 0, 0, "USD").await;
        seed_record(&ledger, u64::MAX, 1, 0, 0, "USD").await;

        let adapter = LedgerQuotaAdapter::new(ledger, clock_at(u64::MAX));
        let snap = adapter
            .fetch(
                &QuotaRequest {
                    scope: scope(),
                    window: QuotaWindow::Overall,
                    unit: QuotaUnit::Count,
                },
                None,
                &CancellationToken::new(),
            )
            .await
            .expect("ok");

        assert_eq!(snap.values.used, QuotaMeasure::exact(2));
    }

    #[tokio::test]
    async fn budget_cap_projects_remaining() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let clock = clock_at(10_000_000);
        seed_record(&ledger, 1_000, 100, 0, 0, "USD").await;

        let adapter = LedgerQuotaAdapter::with_budget(
            ledger,
            clock,
            BudgetCap::with_limit(QuotaMeasure::exact(1_000)),
        );
        let request = QuotaRequest {
            scope: scope(),
            window: QuotaWindow::Overall,
            unit: QuotaUnit::Token,
        };
        let snap = adapter
            .fetch(&request, None, &CancellationToken::new())
            .await
            .expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::exact(100));
        assert_eq!(snap.values.limit, QuotaMeasure::exact(1_000));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(900));
    }

    #[tokio::test]
    async fn rolling_window_excludes_out_of_range_records() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        // Now is 10,000,000,000 ms. Rolling5h length = 18,000,000 ms, so the
        // window is [9,982,000,000, 10,000,000,000).
        let clock = clock_at(10_000_000_000);
        // Inside window.
        seed_record(&ledger, 9_990_000_000, 100, 0, 0, "USD").await;
        // Outside window (before start).
        seed_record(&ledger, 1_000, 999, 0, 0, "USD").await;

        let adapter = LedgerQuotaAdapter::new(ledger, clock);
        let request = QuotaRequest {
            scope: scope(),
            window: QuotaWindow::Rolling5h,
            unit: QuotaUnit::Token,
        };
        let snap = adapter
            .fetch(&request, None, &CancellationToken::new())
            .await
            .expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::exact(100));
        match snap.reset {
            QuotaReset::Relative {
                after_secs,
                uncertain,
                ..
            } => {
                assert!(uncertain, "rolling reset is uncertain");
                assert_eq!(after_secs, 5 * 60 * 60);
            }
            other => panic!("expected Relative reset, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancellation_short_circuits() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let adapter = LedgerQuotaAdapter::new(ledger, clock_at(1_000));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let request = QuotaRequest {
            scope: scope(),
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Token,
        };
        let err = adapter
            .fetch(&request, None, &cancel)
            .await
            .expect_err("must cancel");
        assert!(matches!(err, QuotaError::Cancelled));
    }

    #[tokio::test]
    async fn snapshot_carries_no_secret() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let adapter = LedgerQuotaAdapter::new(ledger, clock_at(1_000));
        let request = QuotaRequest {
            scope: scope(),
            window: QuotaWindow::Overall,
            unit: QuotaUnit::Token,
        };
        let snap = adapter
            .fetch(&request, None, &CancellationToken::new())
            .await
            .expect("ok");
        let json = serde_json::to_string(&snap).expect("serialize");
        assert!(!json.contains("sk-"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn predict_exhaustion_basic() {
        // used=25, limit=100, rate=1/s → 75s.
        let p =
            predict_exhaustion(QuotaMeasure::exact(25), QuotaMeasure::exact(100), 1).expect("some");
        assert_eq!(p.seconds_until_exhausted, 75);
        assert!(p.uncertain);
    }

    #[test]
    fn predict_exhaustion_returns_none_for_infinite_or_zero_rate() {
        assert!(predict_exhaustion(QuotaMeasure::exact(25), QuotaMeasure::Infinite, 1).is_none());
        assert!(predict_exhaustion(QuotaMeasure::exact(25), QuotaMeasure::exact(100), 0).is_none());
        // Already at or over limit.
        assert!(
            predict_exhaustion(QuotaMeasure::exact(150), QuotaMeasure::exact(100), 1).is_none()
        );
    }

    #[test]
    fn window_length_matches_canonical_windows() {
        assert!(window_length_ms(crate::QuotaWindow::Overall).is_none());
        assert_eq!(
            window_length_ms(crate::QuotaWindow::Rolling5h),
            Some(5 * 60 * 60 * 1000)
        );
        assert_eq!(
            window_length_ms(crate::QuotaWindow::Weekly),
            Some(7 * 24 * 60 * 60 * 1000)
        );
        assert_eq!(
            window_length_ms(crate::QuotaWindow::Monthly),
            Some(30 * 24 * 60 * 60 * 1000)
        );
    }

    #[tokio::test]
    async fn replay_in_ledger_does_not_double_count() {
        // Demonstrates ledger idempotency carries through to derivation: the
        // same record id+content replayed does not inflate `used`.
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let clock = clock_at(10_000_000);

        let record = UsageRecord {
            record_id: "rec-1".into(),
            tenant_id: TenantId::new("tenant-a"),
            account_id: "account-1".into(),
            provider_id: ProviderId::new("anthropic"),
            model_id: ModelId::new("claude-opus"),
            input_tokens: 100,
            occurred_at_ms: 1_000,
            currency: "USD".into(),
            ..Default::default()
        };

        ledger.record(record.clone()).await.expect("first ok");
        ledger.record(record).await.expect("replay ok"); // idempotent

        let adapter = LedgerQuotaAdapter::new(ledger, clock);
        let request = QuotaRequest {
            scope: scope(),
            window: QuotaWindow::Overall,
            unit: QuotaUnit::Token,
        };
        let snap = adapter
            .fetch(&request, None, &CancellationToken::new())
            .await
            .expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::exact(100));
    }

    #[tokio::test]
    async fn credential_scope_excludes_sibling_credentials() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let clock = clock_at(10_000_000);

        for (record_id, credential_id, input_tokens) in
            [("rec-a", "cred-a", 100), ("rec-b", "cred-b", 900)]
        {
            ledger
                .record(UsageRecord {
                    record_id: record_id.into(),
                    tenant_id: TenantId::new("tenant-a"),
                    account_id: "account-1".into(),
                    credential_id: Some(credential_id.into()),
                    provider_id: ProviderId::new("anthropic"),
                    model_id: ModelId::new("claude-opus"),
                    input_tokens,
                    occurred_at_ms: 1_000,
                    currency: "USD".into(),
                    ..Default::default()
                })
                .await
                .expect("record ok");
        }

        let adapter = LedgerQuotaAdapter::new(ledger, clock);
        let request = QuotaRequest {
            scope: scope().with_credential_id("cred-a"),
            window: QuotaWindow::Overall,
            unit: QuotaUnit::Token,
        };
        let snap = adapter
            .fetch(&request, None, &CancellationToken::new())
            .await
            .expect("ok");

        assert_eq!(snap.values.used, QuotaMeasure::exact(100));
    }

    // =====================================================================
    // Reconcile (P14-7 step 2): remote baseline + ledger delta overlay
    // =====================================================================

    #[tokio::test]
    async fn reconcile_zero_delta_keeps_remote_exact() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        // Records exactly at the boundary timestamps must not be overlaid:
        // the delta window is strictly after fetched_at (5_000) and strictly
        // before now (10_000_000).
        seed_record(&ledger, 5_000, 100, 0, 0, "USD").await;
        seed_record(&ledger, 10_000_000, 200, 0, 0, "USD").await;

        let remote = remote_snapshot(QuotaUnit::Token);
        let snap = reconcile(&remote, ledger, 10_000_000).await.expect("ok");
        assert_eq!(
            snap, remote,
            "zero delta must pass the exact remote through"
        );
        assert_eq!(snap.confidence, Confidence::Exact);
        assert_eq!(snap.provenance.adapter_kind, AdapterKind::ApiKeyApi);
        assert_eq!(
            snap.provenance.fetched_at,
            Timestamp::from_unix_millis(5_000)
        );
    }

    #[tokio::test]
    async fn reconcile_token_delta_updates_values_and_marks_derived() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        // After fetched_at (5_000), before now (10_000_000): 100+50 and
        // 200+10+5+5 -> delta 370.
        seed_record(&ledger, 5_001, 100, 50, 0, "USD").await;
        ledger
            .record(UsageRecord {
                tenant_id: TenantId::new("tenant-a"),
                account_id: "account-1".into(),
                provider_id: ProviderId::new("anthropic"),
                model_id: ModelId::new("claude-opus"),
                input_tokens: 200,
                output_tokens: 10,
                cache_read_tokens: 5,
                cache_write_tokens: 5,
                occurred_at_ms: 9_000_000,
                currency: "USD".into(),
                ..Default::default()
            })
            .await
            .expect("record ok");

        let remote = remote_snapshot(QuotaUnit::Token);
        let snap = reconcile(&remote, ledger, 10_000_000).await.expect("ok");

        assert_eq!(snap.values.used, QuotaMeasure::exact(1_370));
        assert_eq!(snap.values.limit, QuotaMeasure::exact(10_000));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(8_630));
        assert_eq!(snap.confidence, Confidence::Derived);
        assert_eq!(snap.reset, remote.reset, "reset is preserved");
        assert_eq!(snap.provenance.adapter_kind, AdapterKind::LocalLedger);
        assert_eq!(
            snap.provenance.source,
            "remote:anthropic.admin + usage-ledger"
        );
        assert_eq!(
            snap.provenance.fetched_at,
            Timestamp::from_unix_millis(10_000_000)
        );
        assert_eq!(
            snap.provenance.observed_at,
            Some(Timestamp::from_unix_millis(5_000))
        );
    }

    #[tokio::test]
    async fn reconcile_count_uses_record_count_as_delta() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        seed_record(&ledger, 5_001, 10, 0, 0, "USD").await;
        seed_record(&ledger, 6_000, 20, 0, 0, "USD").await;
        seed_record(&ledger, 5_000, 999, 0, 0, "USD").await; // boundary: excluded

        let remote = remote_snapshot(QuotaUnit::Count);
        let snap = reconcile(&remote, ledger, 10_000_000).await.expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::exact(1_002));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(8_998));
        assert_eq!(snap.confidence, Confidence::Derived);
    }

    #[tokio::test]
    async fn reconcile_cost_filters_currency_and_isolates_scope() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        // In-scope USD delta: 400 + 600 = 1_000.
        seed_record_full(&ledger, 6_000, Some("cred-a"), 0, 400, "USD").await;
        seed_record_full(&ledger, 7_000, Some("cred-a"), 0, 600, "USD").await;
        // Wrong currency: excluded from the USD delta.
        seed_record_full(&ledger, 8_000, Some("cred-a"), 0, 9_999, "EUR").await;
        // Exactly at fetched_at: excluded (strictly after).
        seed_record_full(&ledger, 5_000, Some("cred-a"), 0, 5_000, "USD").await;
        // Sibling credential: excluded by the credential dimension.
        seed_record_full(&ledger, 6_000, Some("cred-other"), 0, 9_999, "USD").await;
        // No credential on the record: excluded when the baseline pins one.
        seed_record_full(&ledger, 6_000, None, 0, 9_999, "USD").await;
        // Sibling tenant / account / provider / model: all isolated.
        for record in [
            UsageRecord {
                tenant_id: TenantId::new("tenant-b"),
                account_id: "account-1".into(),
                credential_id: Some("cred-a".into()),
                provider_id: ProviderId::new("anthropic"),
                model_id: ModelId::new("claude-opus"),
                cost_micros: 9_999,
                occurred_at_ms: 6_000,
                currency: "USD".into(),
                ..Default::default()
            },
            UsageRecord {
                tenant_id: TenantId::new("tenant-a"),
                account_id: "account-2".into(),
                credential_id: Some("cred-a".into()),
                provider_id: ProviderId::new("anthropic"),
                model_id: ModelId::new("claude-opus"),
                cost_micros: 9_999,
                occurred_at_ms: 6_000,
                currency: "USD".into(),
                ..Default::default()
            },
            UsageRecord {
                tenant_id: TenantId::new("tenant-a"),
                account_id: "account-1".into(),
                credential_id: Some("cred-a".into()),
                provider_id: ProviderId::new("openai"),
                model_id: ModelId::new("gpt-4o"),
                cost_micros: 9_999,
                occurred_at_ms: 6_000,
                currency: "USD".into(),
                ..Default::default()
            },
        ] {
            ledger.record(record).await.expect("record ok");
        }

        let mut remote = remote_snapshot(QuotaUnit::Cost {
            currency: "USD".into(),
        });
        remote.scope = remote.scope.with_credential_id("cred-a");
        let snap = reconcile(&remote, ledger, 10_000_000).await.expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::exact(2_000));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(8_000));
    }

    #[tokio::test]
    async fn reconcile_bounds_delta_to_the_window() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        // Rolling5h window as of now (10_000_000_000) is
        // [9_982_000_000, 10_000_000_000). fetched_at = 1_000 predates the
        // window start: records after it but outside the current window must
        // not be overlaid.
        seed_record(&ledger, 5_000, 100, 0, 0, "USD").await; // before window start
        seed_record(&ledger, 9_981_999_999, 200, 0, 0, "USD").await; // boundary
        seed_record(&ledger, 9_990_000_000, 300, 0, 0, "USD").await; // counts

        let mut remote = remote_snapshot(QuotaUnit::Token);
        remote.window = QuotaWindow::Rolling5h;
        let snap = reconcile(&remote, ledger, 10_000_000_000)
            .await
            .expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::exact(1_300));
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(8_700));
    }

    #[tokio::test]
    async fn reconcile_zeroes_remaining_on_exhaustion() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        seed_record(&ledger, 6_000, 600, 0, 0, "USD").await;

        let mut remote = remote_snapshot(QuotaUnit::Token);
        remote.values = QuotaValues::new(
            QuotaMeasure::exact(9_500),
            QuotaMeasure::exact(10_000),
            QuotaMeasure::exact(500),
        );
        let snap = reconcile(&remote, ledger, 10_000_000).await.expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::exact(10_100));
        assert_eq!(
            snap.values.remaining,
            QuotaMeasure::exact(0),
            "remaining saturates at zero once exhausted"
        );
    }

    #[tokio::test]
    async fn reconcile_never_guesses_unknown_baselines() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        seed_record(&ledger, 6_000, 100, 0, 0, "USD").await;

        // Unknown used/limit: both stay Unknown; an Exact remaining still
        // advances by exact arithmetic.
        let mut remote = remote_snapshot(QuotaUnit::Token);
        remote.values = QuotaValues::new(
            QuotaMeasure::Unknown,
            QuotaMeasure::Unknown,
            QuotaMeasure::exact(9_000),
        );
        let snap = reconcile(&remote, ledger.clone(), 10_000_000)
            .await
            .expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::Unknown);
        assert_eq!(snap.values.limit, QuotaMeasure::Unknown);
        assert_eq!(snap.values.remaining, QuotaMeasure::exact(8_900));

        // Unknown remaining stays Unknown.
        let mut remote = remote_snapshot(QuotaUnit::Token);
        remote.values = QuotaValues::new(
            QuotaMeasure::exact(1_000),
            QuotaMeasure::Unknown,
            QuotaMeasure::Unknown,
        );
        let snap = reconcile(&remote, ledger, 10_000_000).await.expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::exact(1_100));
        assert_eq!(snap.values.remaining, QuotaMeasure::Unknown);
    }

    #[tokio::test]
    async fn reconcile_overflow_reports_parse() {
        // Per-record token sum overflows u64.
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        ledger
            .record(UsageRecord {
                tenant_id: TenantId::new("tenant-a"),
                account_id: "account-1".into(),
                provider_id: ProviderId::new("anthropic"),
                model_id: ModelId::new("claude-opus"),
                input_tokens: u64::MAX,
                output_tokens: u64::MAX,
                occurred_at_ms: 6_000,
                currency: "USD".into(),
                ..Default::default()
            })
            .await
            .expect("record ok");
        let remote = remote_snapshot(QuotaUnit::Token);
        let err = reconcile(&remote, ledger, 10_000_000)
            .await
            .expect_err("must fail");
        assert!(matches!(err, QuotaError::Parse { .. }), "got {err:?}");

        // Delta accumulation overflows u64.
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        seed_record(&ledger, 6_000, u64::MAX, 0, 0, "USD").await;
        seed_record(&ledger, 7_000, u64::MAX, 0, 0, "USD").await;
        let remote = remote_snapshot(QuotaUnit::Token);
        let err = reconcile(&remote, ledger, 10_000_000)
            .await
            .expect_err("must fail");
        assert!(matches!(err, QuotaError::Parse { .. }), "got {err:?}");

        // used + delta overflows u64.
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        seed_record(&ledger, 6_000, 1, 0, 0, "USD").await;
        let mut remote = remote_snapshot(QuotaUnit::Token);
        remote.values = QuotaValues::new(
            QuotaMeasure::exact(u64::MAX),
            QuotaMeasure::exact(u64::MAX),
            QuotaMeasure::exact(0),
        );
        let err = reconcile(&remote, ledger, 10_000_000)
            .await
            .expect_err("must fail");
        assert!(matches!(err, QuotaError::Parse { .. }), "got {err:?}");

        // Cost delta accumulation overflows u64.
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        seed_record(&ledger, 6_000, 0, 0, u64::MAX, "USD").await;
        seed_record(&ledger, 7_000, 0, 0, u64::MAX, "USD").await;
        let remote = remote_snapshot(QuotaUnit::Cost {
            currency: "USD".into(),
        });
        let err = reconcile(&remote, ledger, 10_000_000)
            .await
            .expect_err("must fail");
        assert!(matches!(err, QuotaError::Parse { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn reconcile_rejects_ledger_derived_baseline() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        seed_record(&ledger, 6_000, 100, 0, 0, "USD").await;

        let mut remote = remote_snapshot(QuotaUnit::Token);
        remote.provenance = QuotaProvenance::new(
            AdapterKind::LocalLedger,
            "usage-ledger",
            Timestamp::from_unix_millis(5_000),
        );
        let err = reconcile(&remote, ledger, 10_000_000)
            .await
            .expect_err("must reject");
        assert!(matches!(err, QuotaError::Other { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn reconcile_rejects_invalid_baseline_time() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let remote = remote_snapshot(QuotaUnit::Token);
        // fetched_at == now: nothing can be strictly after the baseline.
        let err = reconcile(&remote, ledger.clone(), 5_000)
            .await
            .expect_err("must reject");
        assert!(matches!(err, QuotaError::Other { .. }), "got {err:?}");
        // fetched_at after now: invalid baseline, no overlay.
        let err = reconcile(&remote, ledger, 1_000)
            .await
            .expect_err("must reject");
        assert!(matches!(err, QuotaError::Other { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn reconcile_replay_does_not_double_count() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let record = UsageRecord {
            record_id: "rec-reconcile-1".into(),
            tenant_id: TenantId::new("tenant-a"),
            account_id: "account-1".into(),
            provider_id: ProviderId::new("anthropic"),
            model_id: ModelId::new("claude-opus"),
            input_tokens: 100,
            occurred_at_ms: 6_000,
            currency: "USD".into(),
            ..Default::default()
        };
        ledger.record(record.clone()).await.expect("first ok");
        ledger.record(record).await.expect("replay ok"); // idempotent

        let remote = remote_snapshot(QuotaUnit::Token);
        let snap = reconcile(&remote, ledger, 10_000_000).await.expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::exact(1_100));
    }

    #[tokio::test]
    async fn reconcile_cancellation_short_circuits() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let remote = remote_snapshot(QuotaUnit::Token);
        let adapter = LedgerQuotaAdapter::new(ledger, clock_at(10_000_000));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = adapter
            .reconcile(&remote, &cancel)
            .await
            .expect_err("must cancel");
        assert!(matches!(err, QuotaError::Cancelled));
    }

    #[tokio::test]
    async fn reconcile_provenance_carries_no_secret() {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        seed_record_full(&ledger, 6_000, Some("cred-1"), 100, 0, "USD").await;

        let mut remote = remote_snapshot(QuotaUnit::Token);
        remote.scope = remote.scope.with_credential_id("cred-1");
        remote.provenance = QuotaProvenance::new(
            AdapterKind::ApiKeyApi,
            "anthropic.admin",
            Timestamp::from_unix_millis(5_000),
        )
        .with_endpoint("https://api.example.com/v1/usage?api_key=sk-secret#top");
        let snap = reconcile(&remote, ledger, 10_000_000).await.expect("ok");

        assert_eq!(snap.provenance.adapter_kind, AdapterKind::LocalLedger);
        assert_eq!(
            snap.provenance.endpoint.as_deref(),
            Some("https://api.example.com/v1/usage")
        );
        let json = serde_json::to_string(&snap).expect("serialize");
        assert!(!json.contains("sk-"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("?"), "endpoint must stay canonicalized");
        assert!(!json.contains("#"), "fragment must stay stripped");
    }
}
