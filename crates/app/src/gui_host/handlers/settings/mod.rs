//! SET-2 Host Settings 门面（ADR-046）、SET-6a 通用设置（ADR-047）与
//! SET-6b 权限与审批设置（ADR-048）。
//! Secret 红线：api_key 明文只在 handler 栈上与验证请求的 Authorization
//! 头中短暂停留，绝不进入 tracing / 事件 / ledger。proxy URL 可能内嵌
//! user:pass，loopback_aware_proxy 错误串含原文，禁止送进 GUI Error / tracing。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pawork_domain::CancellationToken;
use pawork_policy::ApprovalMode;
use pawork_protocol::{AppResponse, ApprovalModeWire};
use serde::Serialize;

mod auth;
mod catalog;
mod general;
mod permissions;
mod terminal;

pub(crate) use auth::{auth_cancel, auth_remove, auth_set_api_key, auth_start};
pub(crate) use catalog::{
    provider_auth_status, set_default_model, set_default_role_model, set_model_enabled,
    set_provider_models_enabled, set_provider_use_proxy,
};
pub(crate) use general::{general_settings, set_proxy_url};
pub(crate) use permissions::{permissions_settings, set_approval_mode, workspace_trust};
pub(crate) use terminal::{set_terminal_settings, terminal_settings};

/// 一次进行中的认证 flight：取消令牌 + 种类标记。
///
/// D3：api_key 验证是单次同步请求（不可取消），OAuth 授权等待可取消；
/// auth_cancel 按活跃 flight 的种类放行，不按通道声明推断。
pub(crate) struct AuthFlight {
    token: Arc<CancellationToken>,
    /// true = OAuth 授权等待（可取消）；false = api_key 验证（拒绝取消）。
    oauth_wait: bool,
}

/// 认证单飞注册表（按 provider_id；Arc 身份用于安全移除自己的 flight）。
pub(crate) type AuthFlights = Arc<Mutex<HashMap<String, Arc<AuthFlight>>>>;

pub(super) fn settings_data<T: Serialize>(value: T) -> AppResponse {
    AppResponse::Data(serde_json::to_value(value).expect("settings data serializes"))
}

pub(super) fn policy_approval_mode(mode: ApprovalModeWire) -> ApprovalMode {
    match mode {
        ApprovalModeWire::AlwaysAsk => ApprovalMode::AlwaysAsk,
        ApprovalModeWire::AskForWrites => ApprovalMode::AskForWrites,
        ApprovalModeWire::AskForDangerous => ApprovalMode::AskForDangerous,
        ApprovalModeWire::NeverAsk => ApprovalMode::NeverAsk,
        ApprovalModeWire::ReadOnly => ApprovalMode::ReadOnly,
    }
}

pub(super) fn wire_approval_mode(mode: ApprovalMode) -> ApprovalModeWire {
    match mode {
        ApprovalMode::AlwaysAsk => ApprovalModeWire::AlwaysAsk,
        ApprovalMode::AskForWrites => ApprovalModeWire::AskForWrites,
        ApprovalMode::AskForDangerous => ApprovalModeWire::AskForDangerous,
        ApprovalMode::NeverAsk => ApprovalModeWire::NeverAsk,
        ApprovalMode::ReadOnly => ApprovalModeWire::ReadOnly,
    }
}

fn flight_begin(
    flights: &AuthFlights,
    provider: &str,
    oauth_wait: bool,
) -> Result<Arc<AuthFlight>, crate::gui_server::GuiHostError> {
    let mut flights = flights.lock().expect("auth flights poisoned");
    if flights.contains_key(provider) {
        return Err(crate::gui_host::GuiHostAdapter::host_error(
            "busy",
            format!("an auth operation for provider {provider} is already in progress"),
        ));
    }
    let flight = Arc::new(AuthFlight {
        token: Arc::new(CancellationToken::new()),
        oauth_wait,
    });
    flights.insert(provider.to_string(), Arc::clone(&flight));
    Ok(flight)
}

/// 仅当注册表中仍是同一 flight 时移除，避免误删后来者。
fn flight_end(flights: &AuthFlights, provider: &str, flight: &Arc<AuthFlight>) {
    let mut flights = flights.lock().expect("auth flights poisoned");
    if flights
        .get(provider)
        .is_some_and(|current| Arc::ptr_eq(current, flight))
    {
        flights.remove(provider);
    }
}

/// 判定并取消可取消的 flight（单锁内完成种类判定与移除）。
/// None = 无活跃 flight（幂等）；Some(false) = 活跃的是 api_key 验证
/// flight（拒绝取消，登记保留）；Some(true) = OAuth 等待已移除并取消。
fn cancel_oauth_flight_if_present(flights: &AuthFlights, provider: &str) -> Option<bool> {
    let mut flights = flights.lock().expect("auth flights poisoned");
    let Some(flight) = flights.get(provider) else {
        return None;
    };
    if !flight.oauth_wait {
        return Some(false);
    }
    let cancelled = flights.remove(provider).expect("flight present under lock");
    cancelled.token.cancel();
    Some(true)
}

fn flight_active(flights: &AuthFlights, provider: &str) -> bool {
    flights
        .lock()
        .expect("auth flights poisoned")
        .contains_key(provider)
}

/// unix 毫秒 → UTC ISO-8601（无时区依赖；仅供 wire 展示字段使用）。
fn iso8601_utc(millis: u64) -> String {
    let secs = millis / 1000;
    let days = (secs / 86_400) as i64;
    let time = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    )
}

/// Howard Hinnant civil_from_days：epoch 天数 → (年, 月, 日)。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn now_millis() -> u64 {
    pawork_engine::now_timestamp().as_unix_millis()
}
