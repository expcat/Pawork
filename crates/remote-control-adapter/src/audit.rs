//! 审计日志：远程控制通道的安全相关事件（有界环形缓冲）。
//!
//! 记录配对/认证/吊销、门禁拒绝与放行转发、重放与推送缺口、协议违规等
//! 事件；每条记录带单调 seq（首条为 1）与 Unix 毫秒时间戳。
//!
//! **Secret 卫生**：审计事件只携带稳定标识（pairing_id / device_id /
//! 操作名 / 拒绝码），绝不携带配对码、设备凭证或任何 Token 明文；
//! PairingRegistry 的 Debug 同样做了脱敏，明文仅在签发帧中出现一次。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

/// 默认审计环形缓冲容量。
pub const DEFAULT_AUDIT_CAPACITY: usize = 4096;

/// 审计事件（稳定、可序列化；不含任何 Secret）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEvent {
    /// 配对挑战已签发（配对码本身不入审计）。
    PairingCodeIssued {
        pairing_id: String,
        device_label: String,
    },
    /// 配对码兑换成功，设备获得 device_id 与一次性凭证。
    DevicePaired {
        device_id: String,
        device_label: String,
    },
    /// 设备认证通过（配对激活或凭证认证）。
    DeviceAuthenticated { device_id: String },
    /// 认证失败（原因来自配对/凭证校验的结构化错误）。
    AuthenticationFailed { reason: String },
    /// 宿主吊销设备凭证。
    DeviceRevoked {
        device_id: String,
        remaining_active: usize,
    },
    /// 门禁显式拒绝（附稳定拒绝码与操作名）。
    OperationDenied { code: String, operation: String },
    /// 未认证连接尝试执行受限操作。
    AuthenticationRequired { operation: String },
    /// 允许集命令经 canonical 信封转发到 Core。
    CommandDispatched {
        command_id: String,
        operation: String,
    },
    /// 允许集查询经 canonical 信封转发到 Core。
    QueryDispatched {
        request_id: String,
        operation: String,
    },
    /// 通知重放成功（按序）。
    ReplayServed { from_seq: u64, count: usize },
    /// 通知重放缺口：请求起点已被环形缓冲淘汰（显式告知最早可用 seq）。
    ReplayGapServed {
        requested_from: u64,
        earliest_available: u64,
    },
    /// 推送背压缺口：出站队列溢出导致 [from_seq, to_seq] 未实时推送。
    PushGap {
        from_seq: u64,
        to_seq: u64,
        reason: String,
    },
    /// 事件 Hub 订阅滞后：错过的 canonical 事件无法映射为通知。
    HubLagged { missed: u64 },
    /// 协议违规（帧超限 / 无法解码等）。
    ProtocolViolation { detail: String },
    /// 连接关闭（收尾记录）。
    ConnectionClosed { reason: String },
}

/// 一条审计记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    /// 本日志分配的单调序列号（首条为 1）。
    pub seq: u64,
    /// 记录时刻（Unix 毫秒）。
    pub timestamp_ms: u64,
    /// 行为主体：device_id / "anonymous" / "host" / "system"。
    pub actor: String,
    pub event: AuditEvent,
}

struct Inner {
    ring: VecDeque<AuditRecord>,
    next_seq: u64,
}

/// 审计日志（克隆廉价，内部共享同一状态；容量有界，最旧记录先淘汰）。
#[derive(Clone)]
pub struct AuditLog {
    capacity: usize,
    inner: Arc<Mutex<Inner>>,
}

impl AuditLog {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_AUDIT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            inner: Arc::new(Mutex::new(Inner {
                ring: VecDeque::with_capacity(capacity.max(1)),
                next_seq: 1,
            })),
        }
    }

    /// 追加一条审计记录并返回（seq 单调递增，环形缓冲满时淘汰最旧）。
    pub fn record(&self, actor: impl Into<String>, event: AuditEvent) -> AuditRecord {
        let mut inner = lock(&self.inner);
        let record = AuditRecord {
            seq: inner.next_seq,
            timestamp_ms: crate::now_unix_ms(),
            actor: actor.into(),
            event,
        };
        inner.next_seq += 1;
        if inner.ring.len() == self.capacity {
            inner.ring.pop_front();
        }
        inner.ring.push_back(record.clone());
        record
    }

    /// 现存记录（按 seq 升序）。
    pub fn entries(&self) -> Vec<AuditRecord> {
        lock(&self.inner).ring.iter().cloned().collect()
    }

    /// 最新已分配序列号；尚无记录时为 None。
    pub fn latest_seq(&self) -> Option<u64> {
        let inner = lock(&self.inner);
        inner.next_seq.checked_sub(1)
    }

    pub fn len(&self) -> usize {
        lock(&self.inner).ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

fn lock(inner: &Arc<Mutex<Inner>>) -> MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_assigns_monotonic_seq_and_captures_actor() {
        let log = AuditLog::new();
        let first = log.record(
            "host",
            AuditEvent::DeviceRevoked {
                device_id: "device-1".into(),
                remaining_active: 0,
            },
        );
        let second = log.record(
            "device-1",
            AuditEvent::OperationDenied {
                code: DENY_TOOL_EXECUTION.into(),
                operation: "run_tool".into(),
            },
        );
        assert_eq!(first.seq, 1);
        assert_eq!(second.seq, 2);
        assert_eq!(log.latest_seq(), Some(2));
        assert_eq!(log.len(), 2);
        let entries = log.entries();
        assert_eq!(entries[0].actor, "host");
        assert_eq!(entries[1].actor, "device-1");
        assert!(entries[0].timestamp_ms <= entries[1].timestamp_ms);
    }

    const DENY_TOOL_EXECUTION: &str = "tool_execution_denied";

    #[test]
    fn ring_is_bounded_and_evicts_oldest() {
        let log = AuditLog::with_capacity(3);
        for index in 1..=5 {
            log.record("system", AuditEvent::HubLagged { missed: index });
        }
        assert_eq!(log.len(), 3);
        assert_eq!(log.latest_seq(), Some(5));
        let entries = log.entries();
        let seqs: Vec<u64> = entries.iter().map(|record| record.seq).collect();
        assert_eq!(seqs, vec![3, 4, 5], "最旧记录必须先被淘汰");
    }

    #[test]
    fn events_serialize_with_stable_type_tags_and_no_secrets() {
        let record = AuditLog::new().record(
            "anonymous",
            AuditEvent::PairingCodeIssued {
                pairing_id: "pairing-1".into(),
                device_label: "phone".into(),
            },
        );
        let json = serde_json::to_string(&record).expect("serialize");
        assert!(json.contains("\"type\":\"pairing_code_issued\""));
        // 审计记录中不存在任何 secret 字段名。
        assert!(!json.contains("pairing_code\":"));
        assert!(!json.contains("credential\":"));
    }
}
