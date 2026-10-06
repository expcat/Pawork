//! Target-scoped host approvals for native-background computer use (CU-16).
//!
//! A grant binds one [TargetIdentity] — a whole application, or one web
//! origin inside a browser — to the workspace/run [Scope] it was approved
//! in. The host creates grants only after an explicit user approval decision
//! through the existing approval channel: the once / for-run semantics map
//! onto the host's `ApprovedOnce` / `ApprovedForRun` decisions, and
//! denial or cancellation simply produces no grant. Automatic approval
//! resolvers cannot satisfy a policy AskUser, so they can never produce a
//! grant either; there is no separate approval framework here.
//!
//! One-shot grants ([GrantKind::Once]) cover exactly one observation or one
//! input dispatch; [GrantKind::ForRun] covers the rest of the current run
//! for that target only. Binding a window requires an active grant but does
//! not spend a one-shot grant. Revocation affects every later gated
//! operation; actions already consumed are history and stay recorded as-is.
//! Grants live in memory and die with their run scope or the host process:
//! persistent allow follows the existing adjudicated settings (approval
//! mode / workspace trust); a per-target persistent grant would need an ADR
//! before it could exist.
//!
//! Two anti-self-approval properties are structural:
//!
//! - Grant creation is host-only. Models hold opaque handles and can only
//!   ever trigger a check, never mint or widen a grant.
//! - [MINIMUM_PROTECTED_BUNDLES] are never authorizable: Pawork's own
//!   approval UI and the OS surfaces where permissions are granted. The set
//!   is built in (default construction is safe), case-insensitive, and the
//!   host can only widen it — so the agent cannot approve itself through
//!   Pawork's approval cards or through a system permission pane. No API
//!   here mutates OS Screen Recording / Accessibility / Automation
//!   permissions either — those are reported by the native probe (CU-03+)
//!   exactly as they stand, and a Pawork grant never implies them.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use crate::target::{AppFamily, AppIdentity, Scope, TargetError};

/// Bundle ids that can never be authorized, always enforced: Pawork's own
/// approval UI and the OS surfaces where approvals and permissions are
/// granted. This is the built-in minimum — the host may add more through
/// [TargetAuthorizer::new] but can never remove these. Matching is
/// case-insensitive. The list is fail-closed, not a proof of exhaustiveness:
/// the structural defenses remain that grants are host-issued only and that
/// no Pawork API mutates OS permission state.
pub const MINIMUM_PROTECTED_BUNDLES: &[&str] = &[
    // Pawork's own approval UI: the agent must never operate the surface
    // that creates grants.
    "dev.pawork.desktop",
    // macOS System Settings: the Screen Recording / Accessibility /
    // Automation panes where OS permissions are granted.
    "com.apple.systempreferences",
    // macOS consent and authorization surfaces (TCC alerts, security
    // prompts, notification-based consent).
    "com.apple.securityagent",
    "com.apple.usernotificationcenter",
    "com.apple.coreservicesuiagent",
];

/// What an authorization covers. A grant for one variant never covers the
/// other: approving an application does not approve any website inside it,
/// and approving one web origin does not approve the browser or other sites.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetIdentity {
    /// Every window of this application (bundle id + family).
    Application { app: AppIdentity },
    /// One web origin (`http(s)://host[:port]`) inside a browser app.
    Website { browser: AppIdentity, origin: String },
}

impl TargetIdentity {
    pub fn application(app: AppIdentity) -> Self {
        Self::Application { app }
    }

    /// The physical contention boundary CU-09 occupancy tracks: input is
    /// dispatched to an application process, so the app itself — or the
    /// browser hosting a website target — is the unit two runs must not
    /// share.
    pub fn occupancy_anchor(&self) -> &str {
        match self {
            Self::Application { app } => &app.bundle_id,
            Self::Website { browser, .. } => &browser.bundle_id,
        }
    }

    /// Website targets require a browser-family application; the origin is
    /// normalized (lowercase, no trailing slash) so casing variants of the
    /// same site share one grant.
    pub fn website(browser: AppIdentity, origin: &str) -> Result<Self, TargetError> {
        if browser.family != AppFamily::Browser {
            return Err(TargetError::Invalid(
                "website targets require a browser application",
            ));
        }
        Ok(Self::Website {
            browser,
            origin: normalize_origin(origin)?,
        })
    }

    pub fn app(&self) -> &AppIdentity {
        match self {
            Self::Application { app } => app,
            Self::Website { browser, .. } => browser,
        }
    }

    /// Structural validity required at every authorization entry. The enum
    /// and its serde shape are public, so a manually built or deserialized
    /// value could bypass the checked constructors; grants and checks
    /// re-validate instead of trusting the value.
    pub fn validate(&self) -> Result<(), TargetError> {
        if self.app().bundle_id.trim().is_empty() {
            return Err(TargetError::Invalid("application bundle id must not be empty"));
        }
        match self {
            Self::Application { .. } => Ok(()),
            Self::Website { browser, origin } => {
                if browser.family != AppFamily::Browser {
                    return Err(TargetError::Invalid(
                        "website targets require a browser application",
                    ));
                }
                match normalize_origin(origin) {
                    Ok(canonical) if canonical == *origin => Ok(()),
                    _ => Err(TargetError::Invalid(
                        "website origin is not in canonical form",
                    )),
                }
            }
        }
    }
}

/// Normalize and validate an origin to `http(s)://host[:port]`: lowercase,
/// no trailing slash, no path / query / fragment / userinfo, a non-empty
/// host of domain characters with at least one alphanumeric, and an
/// optional numeric port in 1..=65535 (leading zeros canonicalized). Not a
/// full URL parser — bracketed IPv6 literals are currently rejected
/// (fail-closed) rather than half-parsed.
pub(crate) fn normalize_origin(raw: &str) -> Result<String, TargetError> {
    const INVALID: TargetError = TargetError::Invalid(
        "website origin must be http(s)://host[:port] without path, query or fragment",
    );
    let lower = raw.trim().to_ascii_lowercase();
    let (scheme, rest) = lower
        .strip_prefix("https://")
        .map(|rest| ("https", rest))
        .or_else(|| lower.strip_prefix("http://").map(|rest| ("http", rest)))
        .ok_or(TargetError::Invalid(
            "website origin must start with http:// or https://",
        ))?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.contains(&['/', '?', '#', '@', '[', ']'][..])
        || rest.chars().any(char::is_whitespace)
    {
        return Err(INVALID);
    }
    let (host, port) = match rest.rfind(':') {
        None => (rest, None),
        Some(at) => {
            let (host, port) = (&rest[..at], &rest[at + 1..]);
            if host.contains(':') {
                return Err(INVALID);
            }
            let port: u16 = port.parse().map_err(|_| INVALID)?;
            if port == 0 {
                return Err(INVALID);
            }
            (host, Some(port))
        }
    };
    if host.is_empty()
        || !host.chars().any(|c| c.is_ascii_alphanumeric())
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(INVALID);
    }
    Ok(match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Lifetime of a grant, mirroring the existing approval decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantKind {
    /// One observation or one input dispatch for this target.
    Once,
    /// The rest of the current run for this target.
    ForRun,
}

static PERMIT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Process-level namespace for permit tokens (pid + start instant in
/// nanos): a token minted by a previous host instance can never collide
/// with one minted after a restart, and the process-wide sequence keeps
/// tokens unique between authorizers inside one process.
fn next_permit_token() -> String {
    static INSTANCE: OnceLock<String> = OnceLock::new();
    let instance = INSTANCE.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{}-{}", std::process::id(), nanos)
    });
    format!("dp-{}-{}", instance, PERMIT_SEQUENCE.fetch_add(1, Ordering::Relaxed))
}

/// Single-use dispatch credential minted by
/// [TargetAuthorizer::spend_for_dispatch] and retired by
/// [TargetAuthorizer::consume_dispatch_permit]. It binds one consuming
/// handle use (element handle since CU-06, window dispatch since CU-08)
/// to the one dispatch that consume released: the permit travels inside
/// [crate::target::ValidatedElement] / [crate::target::ValidatedWindowDispatch],
/// and the first dispatch attempt — whatever its later outcome — retires
/// it, so a cloned validation result can never dispatch twice. The token
/// is crate-private: outside code can carry a permit but can never mint
/// or forge one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchPermit {
    pub(crate) token: String,
}

/// Host-side ledger of target grants. The only issuer is the host after an
/// explicit user decision; checks run inside [crate::target::TargetRegistry]
/// before any probe or backend access. Protection always includes
/// [MINIMUM_PROTECTED_BUNDLES], so default construction is safe.
/// The ledger is deliberately not Clone: a copy would deep-copy the
/// outstanding dispatch permits, so one consume could dispatch once per
/// copy and a revocation on the original would miss the copy's permits.
/// (The compile-time assertion below pins this.)
#[derive(Debug)]
pub struct TargetAuthorizer {
    /// Bundle ids that can never be authorized, lowercase.
    protected: HashSet<String>,
    grants: HashMap<(Scope, TargetIdentity), GrantKind>,
    /// Outstanding single-use dispatch permits, keyed by the scope and
    /// target their consume was authorized for plus the permit token.
    /// `spend_for_dispatch` mints one permit per consuming handle use;
    /// `consume_dispatch_permit` removes it on the first dispatch
    /// attempt, so one consume releases exactly one dispatch.
    /// `revoke` / `revoke_scope` drop outstanding permits, so a
    /// revocation between consume and dispatch still blocks the
    /// dispatch.
    dispatch_permits: HashSet<(Scope, TargetIdentity, String)>,
}

// Compile-time regression for the ledger-duplication finding: if Clone is
// ever re-derived, both impls below apply to TargetAuthorizer and the
// ambiguous trait reference stops the build (E0283, verified against a
// Clone fixture). A cloned ledger would deep-copy outstanding dispatch
// permits — one consume could then dispatch once per copy, and revoking
// the original would miss the copy.
const _: () = {
    trait AmbiguousIfClone<A> {
        fn some_item() {}
    }
    impl<T> AmbiguousIfClone<()> for T {}
    #[allow(dead_code)]
    struct Invalid;
    impl<T: Clone> AmbiguousIfClone<Invalid> for T {}
    let _ = <TargetAuthorizer as AmbiguousIfClone<_>>::some_item;
};

impl Default for TargetAuthorizer {
    fn default() -> Self {
        Self::new(std::iter::empty())
    }
}

impl TargetAuthorizer {
    /// `protected_bundle_ids` adds host-specific bundles on top of the
    /// always-enforced [MINIMUM_PROTECTED_BUNDLES].
    pub fn new(protected_bundle_ids: impl IntoIterator<Item = String>) -> Self {
        Self {
            protected: MINIMUM_PROTECTED_BUNDLES
                .iter()
                .map(|bundle| bundle.to_string())
                .chain(
                    protected_bundle_ids
                        .into_iter()
                        .map(|bundle| bundle.to_ascii_lowercase()),
                )
                .collect(),
            grants: HashMap::new(),
            dispatch_permits: HashSet::new(),
        }
    }

    pub fn is_protected(&self, bundle_id: &str) -> bool {
        self.protected.contains(bundle_id.to_ascii_lowercase().as_str())
    }

    /// Record an explicit user approval. Re-granting replaces the previous
    /// kind (e.g. a spent one-shot grant, or an upgrade to for-run).
    pub fn grant(
        &mut self,
        scope: &Scope,
        target: &TargetIdentity,
        kind: GrantKind,
    ) -> Result<(), TargetError> {
        target.validate()?;
        if self.is_protected(target.app().bundle_id.as_str()) {
            return Err(TargetError::ForbiddenTarget);
        }
        self.grants.insert((scope.clone(), target.clone()), kind);
        Ok(())
    }

    /// Revoke one target's grant; later gated operations on it are
    /// denied, including the dispatch a spent one-shot had released.
    pub fn revoke(&mut self, scope: &Scope, target: &TargetIdentity) {
        self.grants.remove(&(scope.clone(), target.clone()));
        self.dispatch_permits
            .retain(|(held_scope, held_target, _)| held_scope != scope || held_target != target);
    }

    /// Drop every grant of a run (run teardown). Other runs keep theirs.
    pub fn revoke_scope(&mut self, scope: &Scope) {
        self.grants.retain(|(held, _), _| held != scope);
        self.dispatch_permits.retain(|(held, _, _)| held != scope);
    }

    /// Non-consuming presence check: protected targets are always rejected;
    /// a missing, spent or revoked grant is [TargetError::NotAuthorized].
    pub fn check(&self, scope: &Scope, target: &TargetIdentity) -> Result<(), TargetError> {
        target.validate()?;
        if self.is_protected(target.app().bundle_id.as_str()) {
            return Err(TargetError::ForbiddenTarget);
        }
        if self.grants.contains_key(&(scope.clone(), target.clone())) {
            Ok(())
        } else {
            Err(TargetError::NotAuthorized)
        }
    }

    /// Re-check and consume a one-shot grant; for-run grants remain. Called
    /// once per gated action after all other validation passed.
    pub fn spend(&mut self, scope: &Scope, target: &TargetIdentity) -> Result<(), TargetError> {
        self.check(scope, target)?;
        let key = (scope.clone(), target.clone());
        if matches!(self.grants.get(&key), Some(GrantKind::Once)) {
            self.grants.remove(&key);
        }
        Ok(())
    }

    /// Consume-side authorization for one dispatching handle use (an
    /// element handle, or a window dispatch since CU-08): spends the
    /// grant like [Self::spend], then mints the single-use
    /// [DispatchPermit] that authorizes the one dispatch this consume
    /// released. Observation consumes use plain `spend` and never mint
    /// a permit, so an observation can never stand in for a dispatch.
    pub fn spend_for_dispatch(
        &mut self,
        scope: &Scope,
        target: &TargetIdentity,
    ) -> Result<DispatchPermit, TargetError> {
        self.spend(scope, target)?;
        let permit = DispatchPermit {
            token: next_permit_token(),
        };
        self.dispatch_permits
            .insert((scope.clone(), target.clone(), permit.token.clone()));
        Ok(permit)
    }

    /// Dispatch-side authorization: atomically retires the permit the
    /// consuming handle minted for this scope and target. The
    /// first dispatch attempt consumes the permit whether the dispatch
    /// later succeeds or not — a repeated attempt with the same
    /// (cloned) validation result is [TargetError::NotAuthorized].
    /// Protected targets are always rejected, and a revocation since
    /// the consume removed the permit, so a revoked grant blocks its
    /// in-flight dispatch here.
    pub fn consume_dispatch_permit(
        &mut self,
        scope: &Scope,
        target: &TargetIdentity,
        permit: &DispatchPermit,
    ) -> Result<(), TargetError> {
        target.validate()?;
        if self.is_protected(target.app().bundle_id.as_str()) {
            return Err(TargetError::ForbiddenTarget);
        }
        if self
            .dispatch_permits
            .remove(&(scope.clone(), target.clone(), permit.token.clone()))
        {
            Ok(())
        } else {
            Err(TargetError::NotAuthorized)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> Scope {
        Scope::new("ws-1", "run-1")
    }

    fn app(bundle: &str, family: AppFamily) -> AppIdentity {
        AppIdentity {
            bundle_id: bundle.into(),
            family,
        }
    }

    fn editor() -> TargetIdentity {
        TargetIdentity::application(app("com.example.editor", AppFamily::Appkit))
    }

    fn site(origin: &str) -> TargetIdentity {
        TargetIdentity::website(app("com.example.browser", AppFamily::Browser), origin).unwrap()
    }

    #[test]
    fn website_origins_normalize_and_reject_non_origins() {
        let normalized = TargetIdentity::website(
            app("com.example.browser", AppFamily::Browser),
            "HTTPS://Example.COM/",
        )
        .unwrap();
        assert_eq!(normalized, site("https://example.com"));
        for bad in [
            "",
            "example.com",
            "https://",
            "https://example.com/path",
            "https://example.com?q=1",
            "https://example.com#frag",
            "https://user@example.com",
            "https://exa mple.com",
            "https://:abc",
            "https://example.com:abc",
            "https://example.com:0",
            "https://example.com:65536",
            "https://example.com:",
            "https://exa_mple.com",
            "https://[::1]",
            "https://..",
        ] {
            assert!(
                TargetIdentity::website(app("com.example.browser", AppFamily::Browser), bad)
                    .is_err(),
                "origin {bad:?} must be rejected"
            );
        }
        for good in [
            "http://localhost:8080",
            "https://example.com:443",
            "http://127.0.0.1:3000",
            "https://sub.example-site.com",
        ] {
            assert!(
                TargetIdentity::website(app("com.example.browser", AppFamily::Browser), good)
                    .is_ok(),
                "origin {good:?} must be accepted"
            );
        }
        // Port spellings canonicalize to one key.
        let with_leading_zero = TargetIdentity::website(
            app("com.example.browser", AppFamily::Browser),
            "https://example.com:0443",
        )
        .unwrap();
        assert_eq!(with_leading_zero, site("https://example.com:443"));
        assert_eq!(
            TargetIdentity::website(app("com.example.editor", AppFamily::Appkit), "https://x.com"),
            Err(TargetError::Invalid(
                "website targets require a browser application"
            ))
        );
    }

    #[test]
    fn manually_built_identities_are_validated_at_authorization_entry() {
        let mut authorizer = TargetAuthorizer::default();
        // Public fields and derived serde can bypass the checked
        // constructors; grant and check still reject non-canonical or
        // structurally invalid values.
        let not_canonical = TargetIdentity::Website {
            browser: app("com.example.browser", AppFamily::Browser),
            origin: "HTTPS://Example.COM".into(),
        };
        assert!(matches!(
            authorizer.grant(&scope(), &not_canonical, GrantKind::ForRun),
            Err(TargetError::Invalid(_))
        ));
        assert!(matches!(
            authorizer.check(&scope(), &not_canonical),
            Err(TargetError::Invalid(_))
        ));
        let serde_bypass: TargetIdentity = serde_json::from_str(
            r#"{"kind":"website","browser":{"bundle_id":"com.example.browser","family":"browser"},"origin":"https://:bad"}"#,
        )
        .unwrap();
        assert!(matches!(
            authorizer.grant(&scope(), &serde_bypass, GrantKind::Once),
            Err(TargetError::Invalid(_))
        ));
        let wrong_family = TargetIdentity::Website {
            browser: app("com.example.editor", AppFamily::Appkit),
            origin: "https://example.com".into(),
        };
        assert!(matches!(
            authorizer.grant(&scope(), &wrong_family, GrantKind::ForRun),
            Err(TargetError::Invalid(_))
        ));
        let empty_bundle = TargetIdentity::Application {
            app: app("  ", AppFamily::Appkit),
        };
        assert!(matches!(
            authorizer.grant(&scope(), &empty_bundle, GrantKind::ForRun),
            Err(TargetError::Invalid(_))
        ));
        // A structurally valid manual value is accepted.
        let manual = TargetIdentity::Website {
            browser: app("com.example.browser", AppFamily::Browser),
            origin: "https://example.com".into(),
        };
        assert!(authorizer.grant(&scope(), &manual, GrantKind::ForRun).is_ok());
    }

    #[test]
    fn default_authorizer_protects_pawork_ui_and_os_permission_surfaces() {
        let mut authorizer = TargetAuthorizer::default();
        for bundle in [
            "dev.pawork.desktop",
            "com.apple.systempreferences",
            "com.apple.securityagent",
            "com.apple.usernotificationcenter",
            "com.apple.coreservicesuiagent",
        ] {
            let target = TargetIdentity::application(app(bundle, AppFamily::Appkit));
            assert_eq!(
                authorizer.grant(&scope(), &target, GrantKind::ForRun),
                Err(TargetError::ForbiddenTarget),
                "{bundle} must stay protected"
            );
            assert!(authorizer.is_protected(bundle));
        }
        // Casing does not dodge the protection.
        let dodged = TargetIdentity::application(app("DEV.PAWORK.DESKTOP", AppFamily::Appkit));
        assert_eq!(
            authorizer.check(&scope(), &dodged),
            Err(TargetError::ForbiddenTarget)
        );
        // Ordinary applications remain grantable.
        assert!(authorizer.grant(&scope(), &editor(), GrantKind::ForRun).is_ok());
    }

    #[test]
    fn target_identity_serde_round_trips() {
        for identity in [editor(), site("https://example.com")] {
            let json = serde_json::to_string(&identity).unwrap();
            assert_eq!(
                serde_json::from_str::<TargetIdentity>(&json).unwrap(),
                identity
            );
        }
        let json = serde_json::to_string(&site("https://example.com")).unwrap();
        assert!(json.contains("\"kind\":\"website\""));
    }

    #[test]
    fn grants_cover_only_the_approved_target_in_the_approved_scope() {
        let mut authorizer = TargetAuthorizer::default();
        authorizer.grant(&scope(), &editor(), GrantKind::ForRun).unwrap();
        // Same application in another run or workspace is not covered.
        for other in [Scope::new("ws-1", "run-2"), Scope::new("ws-2", "run-1")] {
            assert_eq!(
                authorizer.check(&other, &editor()),
                Err(TargetError::NotAuthorized)
            );
        }
        // Another application is not covered.
        let other_app = TargetIdentity::application(app("com.other.app", AppFamily::Chromium));
        assert_eq!(
            authorizer.check(&scope(), &other_app),
            Err(TargetError::NotAuthorized)
        );
        // An application grant does not cover websites, and a website grant
        // does not cover its browser or a sibling origin.
        let browser_app = TargetIdentity::application(app("com.example.browser", AppFamily::Browser));
        assert_eq!(
            authorizer.check(&scope(), &browser_app),
            Err(TargetError::NotAuthorized)
        );
        authorizer
            .grant(&scope(), &site("https://example.com"), GrantKind::ForRun)
            .unwrap();
        assert_eq!(
            authorizer.check(&scope(), &browser_app),
            Err(TargetError::NotAuthorized)
        );
        assert_eq!(
            authorizer.check(&scope(), &site("https://other.example.com")),
            Err(TargetError::NotAuthorized)
        );
        assert!(authorizer.check(&scope(), &site("https://example.com")).is_ok());
    }

    #[test]
    fn one_shot_grants_spend_exactly_once_and_checks_do_not_spend() {
        let mut authorizer = TargetAuthorizer::default();
        authorizer.grant(&scope(), &editor(), GrantKind::Once).unwrap();
        // Presence checks never spend.
        assert!(authorizer.check(&scope(), &editor()).is_ok());
        assert!(authorizer.check(&scope(), &editor()).is_ok());
        assert!(authorizer.spend(&scope(), &editor()).is_ok());
        assert_eq!(
            authorizer.check(&scope(), &editor()),
            Err(TargetError::NotAuthorized)
        );
        assert_eq!(
            authorizer.spend(&scope(), &editor()),
            Err(TargetError::NotAuthorized)
        );
        // For-run grants are never spent.
        authorizer
            .grant(&scope(), &editor(), GrantKind::ForRun)
            .unwrap();
        for _ in 0..3 {
            assert!(authorizer.spend(&scope(), &editor()).is_ok());
        }
    }

    #[test]
    fn dispatch_permit_is_bound_to_one_consume_and_one_attempt() {
        let mut authorizer = TargetAuthorizer::default();
        authorizer
            .grant(&scope(), &editor(), GrantKind::Once)
            .unwrap();
        // A forged permit never passes, even while a grant exists.
        let forged = DispatchPermit {
            token: "dp-forged".into(),
        };
        assert_eq!(
            authorizer.consume_dispatch_permit(&scope(), &editor(), &forged),
            Err(TargetError::NotAuthorized)
        );
        // The element consume spends the one-shot grant and mints the
        // permit for the dispatch it released: presence checks and new
        // consumes deny from here on.
        let permit = authorizer
            .spend_for_dispatch(&scope(), &editor())
            .unwrap();
        assert_eq!(
            authorizer.check(&scope(), &editor()),
            Err(TargetError::NotAuthorized)
        );
        assert_eq!(
            authorizer.spend(&scope(), &editor()),
            Err(TargetError::NotAuthorized)
        );
        // The permit is bound to its scope and target: under any other
        // key the attempt fails and does not retire it.
        assert_eq!(
            authorizer.consume_dispatch_permit(&Scope::new("ws-1", "run-2"), &editor(), &permit),
            Err(TargetError::NotAuthorized)
        );
        assert_eq!(
            authorizer.consume_dispatch_permit(&scope(), &site("https://example.com"), &permit),
            Err(TargetError::NotAuthorized)
        );
        // The first real attempt retires the permit: a repeated dispatch
        // from a cloned validation result is denied.
        assert!(
            authorizer
                .consume_dispatch_permit(&scope(), &editor(), &permit)
                .is_ok()
        );
        assert_eq!(
            authorizer.consume_dispatch_permit(&scope(), &editor(), &permit),
            Err(TargetError::NotAuthorized)
        );
        // A plain observation spend mints no permit: an observation
        // consume can never stand in for an element dispatch.
        authorizer
            .grant(&scope(), &editor(), GrantKind::Once)
            .unwrap();
        authorizer.spend(&scope(), &editor()).unwrap();
        assert_eq!(
            authorizer.consume_dispatch_permit(&scope(), &editor(), &permit),
            Err(TargetError::NotAuthorized)
        );
        // Revocation between consume and dispatch drops the outstanding
        // permit and blocks the dispatch.
        authorizer
            .grant(&scope(), &editor(), GrantKind::Once)
            .unwrap();
        let revoked = authorizer
            .spend_for_dispatch(&scope(), &editor())
            .unwrap();
        authorizer.revoke(&scope(), &editor());
        assert_eq!(
            authorizer.consume_dispatch_permit(&scope(), &editor(), &revoked),
            Err(TargetError::NotAuthorized)
        );
        // Run teardown drops outstanding permits too.
        authorizer
            .grant(&scope(), &editor(), GrantKind::Once)
            .unwrap();
        let torn_down = authorizer
            .spend_for_dispatch(&scope(), &editor())
            .unwrap();
        authorizer.revoke_scope(&scope());
        assert_eq!(
            authorizer.consume_dispatch_permit(&scope(), &editor(), &torn_down),
            Err(TargetError::NotAuthorized)
        );
        // For-run grants stay: every consume mints an independent
        // single-use permit.
        authorizer
            .grant(&scope(), &editor(), GrantKind::ForRun)
            .unwrap();
        for _ in 0..2 {
            let permit = authorizer
                .spend_for_dispatch(&scope(), &editor())
                .unwrap();
            assert!(
                authorizer
                    .consume_dispatch_permit(&scope(), &editor(), &permit)
                    .is_ok()
            );
        }
        assert!(authorizer.check(&scope(), &editor()).is_ok());
    }

    #[test]
    fn revocation_affects_only_the_revoked() {
        let mut authorizer = TargetAuthorizer::default();
        authorizer.grant(&scope(), &editor(), GrantKind::ForRun).unwrap();
        authorizer
            .grant(&scope(), &site("https://example.com"), GrantKind::ForRun)
            .unwrap();
        authorizer.revoke(&scope(), &editor());
        assert_eq!(
            authorizer.check(&scope(), &editor()),
            Err(TargetError::NotAuthorized)
        );
        assert!(authorizer.check(&scope(), &site("https://example.com")).is_ok());
        // Re-granting after revocation works.
        authorizer.grant(&scope(), &editor(), GrantKind::Once).unwrap();
        assert!(authorizer.check(&scope(), &editor()).is_ok());
        // Run teardown drops the whole scope, other scopes are untouched.
        let other_run = Scope::new("ws-1", "run-2");
        authorizer
            .grant(&other_run, &editor(), GrantKind::ForRun)
            .unwrap();
        authorizer.revoke_scope(&scope());
        assert_eq!(
            authorizer.check(&scope(), &site("https://example.com")),
            Err(TargetError::NotAuthorized)
        );
        assert!(authorizer.check(&other_run, &editor()).is_ok());
    }

    #[test]
    fn protected_targets_can_never_be_granted() {
        let mut authorizer = TargetAuthorizer::new(["dev.pawork.desktop".to_string()]);
        let own_ui = TargetIdentity::application(app("dev.pawork.desktop", AppFamily::Appkit));
        assert_eq!(
            authorizer.grant(&scope(), &own_ui, GrantKind::ForRun),
            Err(TargetError::ForbiddenTarget)
        );
        assert_eq!(
            authorizer.check(&scope(), &own_ui),
            Err(TargetError::ForbiddenTarget)
        );
        // A protected browser cannot be authorized per-website either.
        let own_site = TargetIdentity::website(
            app("dev.pawork.desktop", AppFamily::Browser),
            "https://example.com",
        )
        .unwrap();
        assert_eq!(
            authorizer.grant(&scope(), &own_site, GrantKind::Once),
            Err(TargetError::ForbiddenTarget)
        );
    }
}
