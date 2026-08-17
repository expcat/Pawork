//! Team 协作层的稳定标识与成员角色。
//!
//! 复用 pawork_domain::AgentId 作为成员标识（一个 team 成员就是 P12 中被
//! 监督的 worker / parent）；本模块只补充 team 域独有的 TeamId /
//! MailboxMessageId / FanOutId 与 MemberRole。

/// Team 协作 ID（团队唯一标识）。复用 SessionId 作为 opaque 字符串 ID。
pub type TeamId = pawork_domain::SessionId;

/// Mailbox 消息 ID（团队内单调投递标识）。
pub type MailboxMessageId = pawork_domain::MessageId;

/// 一次受控 peer messaging fan-out 的追踪 ID（用于 presence / policy 审计）。
pub type FanOutId = pawork_domain::CommandId;

/// 成员在 team 中的角色。
///
/// `Supervisor` 即 P12 根 parent（默认可审批 plan、变更成员）；`Worker` 是
/// 普通成员（受 [`crate::teams::peer::PeerPolicy`] 约束的 peer messaging 参与者）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberRole {
    /// 团队根（P12 parent）：可审批 plan、增删成员、解散 team。
    Supervisor,
    /// 普通成员（P12 worker）：可认领 task、收发 mailbox、发起受控 peer 消息。
    Worker,
}

impl MemberRole {
    /// 是否持有审批 / 管理权。
    pub const fn is_supervisor(self) -> bool {
        matches!(self, Self::Supervisor)
    }
}
