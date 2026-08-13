//! Mailbox 命令校验（pure）：成员校验、收件人解析、拉取投递。
//!
//! mailbox 与 run loop 解耦：消息持久化（[`crate::event::TeamEvent::MailboxPosted`]
//! 即事实），worker 按需 [`pull`] 拉取，拉取产生
//! [`crate::event::TeamEvent::MailboxDelivered`]，标记已读产生
//! [`crate::event::TeamEvent::MailboxRead`]。广播消息对除发送者外的全员可见。

use std::collections::BTreeSet;

use agent_domain::AgentId;

use crate::error::TeamError;
use crate::event::Recipients;
use crate::ids::MailboxMessageId;
use crate::state::{MailboxEntry, TeamAggregate};

/// 校验发件与构造收件人集合（直连成员必须全部在 team）。
pub fn resolve_recipients(
    state: &TeamAggregate,
    sender: &AgentId,
    recipients: &Recipients,
) -> Result<BTreeSet<AgentId>, TeamError> {
    if !state.members.contains_key(sender) {
        return Err(TeamError::NotMember {
            team_id: state.team_id.clone().unwrap_or_default(),
            agent_id: (*sender).clone(),
        });
    }
    match recipients {
        Recipients::Broadcast => Ok(state
            .member_set()
            .into_iter()
            .filter(|m| m != sender)
            .collect()),
        Recipients::Direct { members } => {
            for m in members {
                if !state.members.contains_key(m) {
                    return Err(TeamError::NotMember {
                        team_id: state.team_id.clone().unwrap_or_default(),
                        agent_id: (*m).clone(),
                    });
                }
            }
            Ok(members.iter().cloned().collect())
        }
    }
}

/// 拉取指定成员尚未投递的消息（按 message_id 升序）。
pub fn pull(state: &TeamAggregate, agent: &AgentId) -> Vec<MailboxMessageId> {
    let mut ids: Vec<MailboxMessageId> = state
        .mailbox
        .values()
        .filter(|entry| state.is_recipient(entry, agent) && !entry.delivered_to.contains(agent))
        .map(|entry| entry.message_id.clone())
        .collect();
    ids.sort();
    ids
}

/// 校验 `mark_read`：消息存在、agent 是收件人。
pub fn validate_read<'a>(
    state: &'a TeamAggregate,
    message_id: &MailboxMessageId,
    by: &AgentId,
) -> Result<&'a MailboxEntry, TeamError> {
    let entry = state
        .mailbox
        .get(message_id)
        .ok_or(TeamError::MailboxMessageNotFound)?;
    if !state.is_recipient(entry, by) {
        return Err(TeamError::NotRecipient {
            agent_id: (*by).clone(),
        });
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TeamEvent;
    use crate::ids::{MemberRole, TeamId};
    use crate::state::apply;
    use agent_domain::TenantId;

    fn agg() -> TeamAggregate {
        let mut s = TeamAggregate::default();
        apply(
            &mut s,
            TeamEvent::TeamCreated {
                team_id: TeamId::from("t1"),
                tenant_id: TenantId::from("ten"),
                supervisor: AgentId::from("sup"),
                name: "T".into(),
            },
        );
        apply(
            &mut s,
            TeamEvent::MemberAdded {
                team_id: TeamId::from("t1"),
                agent_id: AgentId::from("w1"),
                role: MemberRole::Worker,
            },
        );
        s
    }

    #[test]
    fn pull_returns_direct_message_for_recipient_only() {
        let mut s = agg();
        apply(
            &mut s,
            TeamEvent::MailboxPosted {
                team_id: TeamId::from("t1"),
                message_id: MailboxMessageId::from("m1"),
                sender: AgentId::from("sup"),
                recipients: Recipients::Direct {
                    members: vec![AgentId::from("w1")],
                },
                body: "hi".into(),
            },
        );
        assert_eq!(
            pull(&s, &AgentId::from("w1")),
            vec![MailboxMessageId::from("m1")]
        );
        assert!(pull(&s, &AgentId::from("sup")).is_empty());
    }

    #[test]
    fn resolve_recipients_rejects_non_member() {
        let s = agg();
        let err = resolve_recipients(
            &s,
            &AgentId::from("sup"),
            &Recipients::Direct {
                members: vec![AgentId::from("ghost")],
            },
        )
        .unwrap_err();
        assert!(matches!(err, TeamError::NotMember { .. }));
    }
}
