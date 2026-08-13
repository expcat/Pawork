//! 远程控制受限协议线帧（P17-12）。
//!
//! 帧格式：单个 JSON 文档（一帧一文档）；承载为任意
//! transport_api::GuiConnection 实现（transport-remote 承载集成证据见
//! tests/transport_remote_carrier.rs）。本 crate 不消费 GUI Connection
//! Protocol 帧，不取代 GUI 通道。
//!
//! ## 受限集
//!
//! - 查询：SessionGet / RunStatus / PlanStatus；
//! - 命令：RunStart / RunCancel / ToolApprove。
//!
//! 受限集之外的任何操作（文件写、RunTool、Provider 直连、终端、会话/工作区
//! 变更、批量内容读取等）经 Full 变体进入 crate::gate 分类 → 显式拒绝
//! + 审计，绝不放行到 Core。

use agent_domain::{ModelId, RunId, SessionId, ToolCallId};
use core_api::{AppCommand, AppQuery, AppResponse, ApprovalDecision};
use serde::{Deserialize, Serialize};

use crate::notify::{Notification, NotificationPayload};

/// 客户端 → 服务端帧。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientFrame {
    /// 配对第 1 步：请求配对挑战（配对码由宿主呈现给用户）。
    Pair {
        request_id: String,
        device_label: String,
    },
    /// 配对第 2 步：兑换配对码，获得 device_id 与一次性设备凭证。
    Activate {
        request_id: String,
        pairing_code: String,
    },
    /// 后续连接：device_id + 凭证认证。
    Authenticate {
        request_id: String,
        device_id: String,
        credential: String,
    },
    /// 受限命令（全权命令经 RemoteCommand::Full 进门禁）。
    Command {
        request_id: String,
        command: RemoteCommand,
    },
    /// 受限查询（全权查询经 RemoteQuery::Full 进门禁）。
    Query {
        request_id: String,
        query: RemoteQuery,
    },
    /// 从指定 seq 起按序重放通知。
    Replay { request_id: String, from_seq: u64 },
}

/// 远程受限命令集。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RemoteCommand {
    RunStart {
        session_id: SessionId,
        user_message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<ModelId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
    },
    RunCancel {
        run_id: RunId,
    },
    ToolApprove {
        run_id: RunId,
        tool_call_id: ToolCallId,
        decision: ApprovalDecision,
    },
    /// 全权命令透传位：一律经门禁分类 → 显式拒绝 + 审计（绝不放行）。
    Full {
        command: AppCommand,
    },
}

impl RemoteCommand {
    /// 转为 canonical AppCommand（门禁分类与转发统一使用 canonical 类型）。
    pub fn into_app_command(self) -> AppCommand {
        match self {
            RemoteCommand::RunStart {
                session_id,
                user_message,
                model,
                profile,
            } => AppCommand::RunStart {
                session_id,
                user_message,
                model,
                profile,
            },
            RemoteCommand::RunCancel { run_id } => AppCommand::RunCancel { run_id },
            RemoteCommand::ToolApprove {
                run_id,
                tool_call_id,
                decision,
            } => AppCommand::ToolApprove {
                run_id,
                tool_call_id,
                decision,
            },
            RemoteCommand::Full { command } => command,
        }
    }

    /// 审计/拒绝帧使用的稳定操作名。
    pub fn operation(&self) -> &'static str {
        match self {
            RemoteCommand::RunStart { .. } => "run_start",
            RemoteCommand::RunCancel { .. } => "run_cancel",
            RemoteCommand::ToolApprove { .. } => "tool_approve",
            RemoteCommand::Full { command } => crate::gate::command_operation(command),
        }
    }
}

/// 远程受限查询集。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RemoteQuery {
    SessionGet {
        session_id: SessionId,
    },
    RunStatus {
        run_id: RunId,
    },
    /// 计划状态查询：Core 暂未暴露专用 plan 查询；服务层经 SessionGet
    /// 代理并返回显式可用性标记（plan: null + plan_availability），
    /// 绝不伪造计划状态（Core 单一事实源）。
    PlanStatus {
        session_id: SessionId,
    },
    /// 全权查询透传位：一律经门禁分类 → 显式拒绝 + 审计（绝不放行）。
    Full {
        query: AppQuery,
    },
}

impl RemoteQuery {
    /// 审计/拒绝帧使用的稳定操作名。
    pub fn operation(&self) -> &'static str {
        match self {
            RemoteQuery::SessionGet { .. } => "session_get",
            RemoteQuery::RunStatus { .. } => "run_status",
            RemoteQuery::PlanStatus { .. } => "plan_status",
            RemoteQuery::Full { query } => crate::gate::query_operation(query),
        }
    }

    /// 映射为 canonical 查询；PlanStatus 返回 None（服务层经
    /// SessionGet 代理，不直接映射）。
    pub fn as_app_query(&self) -> Option<AppQuery> {
        match self {
            RemoteQuery::SessionGet { session_id } => Some(AppQuery::SessionGet {
                session_id: session_id.clone(),
            }),
            RemoteQuery::RunStatus { run_id } => Some(AppQuery::RunStatus {
                run_id: run_id.clone(),
            }),
            RemoteQuery::PlanStatus { .. } => None,
            RemoteQuery::Full { query } => Some(query.clone()),
        }
    }
}

/// 服务端 → 客户端帧。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerFrame {
    /// 配对挑战：配对码明文（一次性展示，注册表仅存摘要）。
    PairChallenge {
        request_id: String,
        pairing_id: String,
        pairing_code: String,
        expires_in_ms: u64,
    },
    /// 配对成功：device_id + 一次性设备凭证。
    Activated {
        request_id: String,
        device_id: String,
        credential: String,
    },
    /// 凭证认证成功。
    Authenticated {
        request_id: String,
        device_id: String,
    },
    /// canonical 响应透传（来自 AppService）。
    Response {
        request_id: String,
        response: AppResponse,
    },
    /// 门禁显式拒绝（附稳定拒绝码；必然伴随审计记录）。
    Denied {
        request_id: String,
        code: String,
        reason: String,
        operation: String,
    },
    /// 通知推送（有界、去重、按 seq 有序）。
    Notification {
        seq: u64,
        event_id: String,
        occurred_at_ms: u64,
        payload: NotificationPayload,
    },
    /// 重放结果（按序）。
    Replayed {
        request_id: String,
        notifications: Vec<Notification>,
    },
    /// 重放缺口：请求起点已被环形缓冲淘汰，显式给出最早可重放 seq。
    ReplayGap {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        requested_from: u64,
        earliest_available: u64,
    },
    /// 推送背压缺口：[from_seq, to_seq] 区间未及时推送（有界队列溢出）；
    /// 通知仍在日志中，客户端可用 Replay 补齐。
    PushGap {
        from_seq: u64,
        to_seq: u64,
        reason: String,
    },
    /// 设备凭证已被宿主吊销：连接即将关闭。
    Revoked { device_id: String, reason: String },
    /// 协议/认证错误。
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        code: String,
        message: String,
    },
}

/// 编码服务端帧为帧字节。
pub fn encode_server_frame(frame: &ServerFrame) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(frame)
}

/// 解码客户端帧（生产解析路径）。
pub fn decode_client_frame(bytes: &[u8]) -> Result<ClientFrame, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn client_frames_round_trip() {
        let frames = vec![
            ClientFrame::Pair {
                request_id: "1".into(),
                device_label: "phone".into(),
            },
            ClientFrame::Activate {
                request_id: "2".into(),
                pairing_code: "abcd2345".into(),
            },
            ClientFrame::Authenticate {
                request_id: "3".into(),
                device_id: "device-0001".into(),
                credential: "secret".into(),
            },
            ClientFrame::Command {
                request_id: "4".into(),
                command: RemoteCommand::RunStart {
                    session_id: SessionId::from("s"),
                    user_message: "hello".into(),
                    model: None,
                    profile: None,
                },
            },
            ClientFrame::Query {
                request_id: "5".into(),
                query: RemoteQuery::PlanStatus {
                    session_id: SessionId::from("s"),
                },
            },
            ClientFrame::Replay {
                request_id: "6".into(),
                from_seq: 7,
            },
        ];
        for frame in frames {
            let bytes = serde_json::to_vec(&frame).expect("encode");
            let decoded: ClientFrame = serde_json::from_slice(&bytes).expect("decode");
            assert_eq!(decoded, frame);
        }
    }

    #[test]
    fn full_command_variant_carries_canonical_command() {
        let frame = ClientFrame::Command {
            request_id: "x".into(),
            command: RemoteCommand::Full {
                command: AppCommand::RunTool {
                    run_id: RunId::from("r"),
                    tool_name: "shell".into(),
                    input: json!({"cmd": "ls"}),
                },
            },
        };
        let bytes = serde_json::to_vec(&frame).expect("encode");
        let decoded: ClientFrame = serde_json::from_slice(&bytes).expect("decode");
        assert_eq!(decoded, frame);
        let ClientFrame::Command { command, .. } = decoded else {
            panic!("expected command frame");
        };
        assert_eq!(command.operation(), "run_tool");
    }

    #[test]
    fn unknown_or_malformed_frames_are_rejected() {
        let unknown = json!({"kind": "shell_exec", "request_id": "1"});
        assert!(decode_client_frame(&serde_json::to_vec(&unknown).unwrap()).is_err());
        let garbage = b"{not json";
        assert!(decode_client_frame(garbage).is_err());
    }

    #[test]
    fn server_frames_round_trip_including_denied_and_gap_markers() {
        let frames = vec![
            ServerFrame::Denied {
                request_id: "1".into(),
                code: crate::DENY_TOOL_EXECUTION.into(),
                reason: "no".into(),
                operation: "run_tool".into(),
            },
            ServerFrame::ReplayGap {
                request_id: None,
                requested_from: 1,
                earliest_available: 40,
            },
            ServerFrame::PushGap {
                from_seq: 3,
                to_seq: 9,
                reason: "outbound_backlog".into(),
            },
            ServerFrame::Revoked {
                device_id: "device-0001".into(),
                reason: "revoked".into(),
            },
        ];
        for frame in frames {
            let bytes = encode_server_frame(&frame).expect("encode");
            let decoded: ServerFrame = serde_json::from_slice(&bytes).expect("decode");
            assert_eq!(decoded, frame);
        }
    }
}
