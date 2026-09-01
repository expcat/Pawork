//! 降级可观测契约(R4 波 C,T8)。
//!
//! 宿主各降级路径(HOME 回退 / 无凭证兜底 / 事件流 Lagged / tasks_finish 失败 /
//! 幂等冲突 / ACP 内部状态错误)统一事件化为 [`DegradeEvent`],经既有
//! `AgentEvent::Diagnostic`(持久化事件流)与 protocol `AppEvent::Diagnostic`
//!(实时帧,转换为 `From<&DegradeEvent>`,定义在 pawork-protocol)双通道外发。
//! 本契约**不改变**两通道的 serde 形状:26 帧 golden 与 events_golden 零 diff。
//!
//! 红线:`details` 永不携带 Secret(凭证 / Token / 明文 key);启动期与流受损
//! 等无会话上下文的接点走 [`DegradeSink::FrameStderr`],可重放接点走
//! [`DegradeSink::EventStream`]。code 字符串是 wire 契约,pin 测试锁定,禁止改名。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AgentEvent;

/// 降级类别。`code_suffix()` 即 wire 字符串(`degrade.<suffix>`),冻结。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradeKind {
    /// HOME 缺失 → 会话库落临时目录(启动期,原 data_dir 静默回退)。
    HomeDirFallback,
    /// 无凭证兜底 CatalogOnlyProvider(装配期;details 只含 provider_id)。
    MissingCredential,
    /// 广播 Lagged 断流(流本身受损,只发帧)。
    EventStreamLagged,
    /// tasks_finish / persist_tasks 失败(run 上下文,可落盘重放)。
    TasksFinishFailed,
    /// 幂等 record 失败 / 冲突(命令已执行但持久化失败)。
    IdempotencyConflict,
    /// ACP 内部状态错误(替毒锁 panic;cli 侧转 JSON-RPC error 或 tracing)。
    AcpState,
}

impl DegradeKind {
    /// wire code 后缀;返回值逐字冻结(见模块 pin 测试)。
    pub const fn code_suffix(self) -> &'static str {
        match self {
            Self::HomeDirFallback => "home_dir_fallback",
            Self::MissingCredential => "missing_credential",
            Self::EventStreamLagged => "event_stream_lagged",
            Self::TasksFinishFailed => "tasks_finish_failed",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::AcpState => "acp_state",
        }
    }

    /// 默认外发通道(敏感度分级):有 run 上下文且值得重放 → 事件流;
    /// 启动期 / 流受损 / 通道内部 → 实时帧 + stderr/tracing。
    pub const fn default_sink(self) -> DegradeSink {
        match self {
            Self::TasksFinishFailed => DegradeSink::EventStream,
            Self::HomeDirFallback
            | Self::MissingCredential
            | Self::EventStreamLagged
            | Self::IdempotencyConflict
            | Self::AcpState => DegradeSink::FrameStderr,
        }
    }
}

/// 严重度;与 protocol `DiagnosticLevel`(Info/Warning/Error)一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradeSeverity {
    Info,
    Warning,
    Error,
}

impl DegradeSeverity {
    /// serde 同形的 snake_case 串;与 protocol `DiagnosticLevel` 一一对应。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// 默认外发通道。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradeSink {
    /// 经 `AgentEvent::Diagnostic` + persist-first 落盘,可重放。
    EventStream,
    /// 只发 protocol `AppEvent::Diagnostic` 实时帧 + stderr/tracing,不落盘。
    FrameStderr,
}

/// 一条降级事件。类型化定义,出口形状复用既有 Diagnostic 通道。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DegradeEvent {
    pub kind: DegradeKind,
    pub severity: DegradeSeverity,
    /// 面向人的稳定描述(帧 message;不含 Secret)。
    pub message: String,
    /// 结构化上下文(JSON object;不含 Secret)。非 object 输入会被包进 `"context"` 键。
    pub details: Value,
}

impl DegradeEvent {
    pub fn new(
        kind: DegradeKind,
        severity: DegradeSeverity,
        message: impl Into<String>,
        details: Value,
    ) -> Self {
        Self {
            kind,
            severity,
            message: message.into(),
            details,
        }
    }

    /// wire code:`degrade.<suffix>`。逐字冻结。
    pub fn code(&self) -> String {
        format!("degrade.{}", self.kind.code_suffix())
    }

    /// 默认外发通道;接点可按上下文覆盖(例如 run 内 Lagged 可升级落盘)。
    pub fn default_sink(&self) -> DegradeSink {
        self.kind.default_sink()
    }

    /// 转持久化事件:`AgentEvent::Diagnostic`。details 在调用方传入值
    /// 之上合并 `kind` / `severity` / `message` 三键,键冲突时以本契约为准。
    pub fn to_agent_event(&self) -> AgentEvent {
        let mut details = match &self.details {
            Value::Object(map) => map.clone(),
            other => {
                let mut map = serde_json::Map::new();
                map.insert("context".to_string(), other.clone());
                map
            }
        };
        details.insert(
            "kind".to_string(),
            Value::String(self.kind.code_suffix().to_string()),
        );
        details.insert(
            "severity".to_string(),
            Value::String(self.severity.as_str().to_string()),
        );
        details.insert("message".to_string(), Value::String(self.message.clone()));
        AgentEvent::Diagnostic {
            code: self.code(),
            details: Value::Object(details),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// wire code 逐字冻结:改名即破坏既有消费者(desktop/projection 按 code 特判)。
    #[test]
    fn degrade_codes_are_frozen() {
        let cases = [
            (DegradeKind::HomeDirFallback, "degrade.home_dir_fallback"),
            (DegradeKind::MissingCredential, "degrade.missing_credential"),
            (
                DegradeKind::EventStreamLagged,
                "degrade.event_stream_lagged",
            ),
            (
                DegradeKind::TasksFinishFailed,
                "degrade.tasks_finish_failed",
            ),
            (
                DegradeKind::IdempotencyConflict,
                "degrade.idempotency_conflict",
            ),
            (DegradeKind::AcpState, "degrade.acp_state"),
        ];
        for (kind, expected) in cases {
            let event = DegradeEvent::new(kind, DegradeSeverity::Warning, "m", json!({}));
            assert_eq!(event.code(), expected);
        }
    }

    /// serde 形状 pin:kind/severity/sink 全 snake_case。
    #[test]
    fn degrade_enums_serialize_snake_case() {
        assert_eq!(
            serde_json::to_value(DegradeKind::EventStreamLagged).unwrap(),
            json!("event_stream_lagged")
        );
        assert_eq!(
            serde_json::to_value(DegradeSeverity::Warning).unwrap(),
            json!("warning")
        );
        assert_eq!(
            serde_json::to_value(DegradeSink::EventStream).unwrap(),
            json!("event_stream")
        );
    }

    /// 默认 sink 分级 pin:TasksFinishFailed 落盘,其余帧/stderr。
    #[test]
    fn default_sink_table_is_pinned() {
        assert_eq!(
            DegradeKind::TasksFinishFailed.default_sink(),
            DegradeSink::EventStream
        );
        for kind in [
            DegradeKind::HomeDirFallback,
            DegradeKind::MissingCredential,
            DegradeKind::EventStreamLagged,
            DegradeKind::IdempotencyConflict,
            DegradeKind::AcpState,
        ] {
            assert_eq!(kind.default_sink(), DegradeSink::FrameStderr);
        }
    }

    /// 持久化出口形状 pin:code + details 合并三键,不改变 Diagnostic 变体形状。
    #[test]
    fn to_agent_event_merges_contract_keys() {
        let event = DegradeEvent::new(
            DegradeKind::TasksFinishFailed,
            DegradeSeverity::Error,
            "tasks_finish failed",
            json!({"task_id": "t1", "kind": "caller-value"}),
        );
        let AgentEvent::Diagnostic { code, details } = event.to_agent_event() else {
            panic!("degrade must map to AgentEvent::Diagnostic");
        };
        assert_eq!(code, "degrade.tasks_finish_failed");
        // 键冲突以契约为准。
        assert_eq!(
            details,
            json!({
                "task_id": "t1",
                "kind": "tasks_finish_failed",
                "severity": "error",
                "message": "tasks_finish failed",
            })
        );
    }

    /// 非 object details 包裹进 "context" 键,出口仍为 object。
    #[test]
    fn to_agent_event_wraps_non_object_details() {
        let event = DegradeEvent::new(
            DegradeKind::AcpState,
            DegradeSeverity::Error,
            "m",
            json!("raw"),
        );
        let AgentEvent::Diagnostic { details, .. } = event.to_agent_event() else {
            panic!("degrade must map to AgentEvent::Diagnostic");
        };
        assert_eq!(details["context"], json!("raw"));
        assert!(details.is_object());
    }
}
