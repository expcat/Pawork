//! P2 预留：外部触发器信封（External Trigger Adapter）。
//!
//! 具体平台 adapter（Webhook / GitHub / GitLab / MCP）在 Core 边界完成认证、
//! 签名校验、限速与重放防护，再调用 [`canonical_event_from_external`] 把已认证
//! 的外部事件转换为 canonical 载荷字符串。
//!
//! **automation-service core 只消费 canonical 载荷字符串**（经
//! [`crate::AutomationEngine::match_event`] 做正则匹配），从不直接分支判断平台
//! 名称——平台差异封装在 adapter 内。集成测试用断言验证此不变量。

use serde::{Deserialize, Serialize};

/// 外部触发器信封：标注来源平台 + 已提取的 canonical 载荷。
///
/// 真实 adapter 在构造此结构前必须完成认证 / 签名 / 限速 / 重放防护；
/// automation-service 信任传入的 `payload` 已是认证后的 canonical 内容。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ExternalTrigger {
    /// 通用 Webhook（adapter 自行验证签名 / token）。
    Webhook { id: String, payload: String },
    /// 受控 HTTP API 回调。
    HttpApi { id: String, payload: String },
    /// GitHub 事件（adapter 验证 X-Hub-Signature-256 等）。
    GitHubEvent { id: String, payload: String },
    /// GitLab 事件（adapter 验证 X-Gitlab-Token 等）。
    GitLabEvent { id: String, payload: String },
    /// 外部 MCP 服务器事件（adapter 验证会话 / 能力）。
    ExternalMcpEvent { id: String, payload: String },
}

impl ExternalTrigger {
    /// 已认证后的 canonical 载荷（automation-service 唯一消费的字符串）。
    pub fn payload(&self) -> &str {
        match self {
            ExternalTrigger::Webhook { payload, .. }
            | ExternalTrigger::HttpApi { payload, .. }
            | ExternalTrigger::GitHubEvent { payload, .. }
            | ExternalTrigger::GitLabEvent { payload, .. }
            | ExternalTrigger::ExternalMcpEvent { payload, .. } => payload,
        }
    }

    /// adapter 侧的诊断 ID（不参与 core 匹配）。
    pub fn id(&self) -> &str {
        match self {
            ExternalTrigger::Webhook { id, .. }
            | ExternalTrigger::HttpApi { id, .. }
            | ExternalTrigger::GitHubEvent { id, .. }
            | ExternalTrigger::GitLabEvent { id, .. }
            | ExternalTrigger::ExternalMcpEvent { id, .. } => id,
        }
    }
}

/// 把已认证的外部触发器转换为 canonical 载荷字符串。
///
/// 这是 adapter → automation-service 的唯一桥梁：core 只看到返回的字符串，
/// 不感知具体平台。adapter 实现负责在调用前完成全部安全校验。
pub fn canonical_event_from_external(trigger: &ExternalTrigger) -> &str {
    trigger.payload()
}
