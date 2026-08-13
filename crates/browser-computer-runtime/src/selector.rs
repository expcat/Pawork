//! 后端选择：探测 + 可观测回退 + 跨 trust boundary policy 闸门（P17-10）。
use std::sync::Arc;

use crate::backend::{BackendRoute, BrowserComputerBackend, ExecutionSite};
use crate::error::BrowserComputerError;
use crate::policy::BrowserComputerAudit;

/// 选择策略。
#[derive(Clone, Debug, Default)]
pub struct SelectionPolicy {
    pub allow_cross_trust_fallback: bool,
}

/// 单次探测尝试记录（可观测回退的依据）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeAttempt {
    pub descriptor_name: String,
    pub kind: String,
    pub site: String,
    pub trust: String,
    pub available: bool,
    pub reason: String,
}

/// 一次本地选择的结果。
#[derive(Clone, Debug)]
pub struct BackendSelection {
    pub route: BackendRoute,
    pub cross_trust_fallback: bool,
    pub attempted: Vec<ProbeAttempt>,
}

impl BackendSelection {
    pub fn audit(
        &self,
        action: &'static str,
        policy: &'static str,
        note: impl Into<String>,
    ) -> BrowserComputerAudit {
        BrowserComputerAudit {
            action: action.into(),
            backend: Some(self.route.kind.as_str().into()),
            site: Some(self.route.site.as_str().into()),
            trust: Some(self.route.trust.as_str().into()),
            cross_trust_fallback: self.cross_trust_fallback,
            policy: policy.into(),
            note: note.into(),
        }
    }
}

fn record(
    backend: &dyn BrowserComputerBackend,
    probe: crate::backend::BackendProbe,
) -> ProbeAttempt {
    ProbeAttempt {
        descriptor_name: backend.descriptor_name().to_string(),
        kind: backend.kind().as_str().to_string(),
        site: backend.execution_site().as_str().to_string(),
        trust: backend.trust_boundary().as_str().to_string(),
        available: probe.available,
        reason: probe.reason,
    }
}

/// 为本地（ClientFunction）执行选择后端。
///
/// 不按 Provider 名分支，只按 `execution_site()` 路由；`ProviderHosted` 从不被选中；
/// 回退可观测（`attempted` 记录全部尝试）。
pub fn select_for_local(
    backends: &[Arc<dyn BrowserComputerBackend>],
) -> Result<BackendSelection, Vec<ProbeAttempt>> {
    let mut attempted = Vec::with_capacity(backends.len());
    for backend in backends {
        // probe 可能触发系统探测；一次选择中每个 backend 只调用一次并复用结果。
        let probe = backend.probe();
        let available = probe.available;
        let is_candidate = backend.execution_site() == ExecutionSite::ClientFunction;
        attempted.push(record(backend.as_ref(), probe));
        if is_candidate && available {
            return Ok(BackendSelection {
                route: BackendRoute::from_backend(backend.as_ref()),
                cross_trust_fallback: false,
                attempted,
            });
        }
    }
    Err(attempted)
}

/// 探测是否存在可用的 provider-hosted 后端（跨 trust 降级目标）。
pub fn find_hosted(backends: &[Arc<dyn BrowserComputerBackend>]) -> Option<BackendRoute> {
    backends
        .iter()
        .find(|b| b.execution_site() == ExecutionSite::ProviderHosted && b.probe().available)
        .map(|b| BackendRoute::from_backend(b.as_ref()))
}

/// 本地无可用后端的显式错误。
///
/// 跨 trust 降级是否允许由调用方按 [`SelectionPolicy`] 判定；本函数绝不隐式跨 trust。
pub fn no_local_backend_error(_attempted: &[ProbeAttempt]) -> BrowserComputerError {
    BrowserComputerError::NoLocalBackend
}
