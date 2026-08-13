//! 受控 peer messaging：worker↔worker 直接通信（非只能经 parent）。
//!
//! 与 mailbox 的区别：mailbox 是「持久投递 + 按需拉取」的异步通道；peer
//! messaging 是「即时 fan-out」语义，必须经 [`PeerPolicy`] 授权，避免
//! 无限制 fan-out（一个 worker 广播给全员、再被全员转发 → 指数级噪声）。
//!
//! policy 在本 crate 内可配；实际「执行」仍归 task-manager / 编排通道（teams
//! 只产出 [`crate::event::TeamEvent::PeerMessageRouted`] / [`crate::event::TeamEvent::FanOutDenied`]
//! 两个 canonical 事实）。

use std::collections::BTreeSet;

use agent_domain::AgentId;

use crate::error::TeamError;
use crate::event::Recipients;

/// peer messaging fan-out 策略。
///
/// 默认值收敛：禁止广播、单次直连上限 4、每成员并发 fan-out 上限 4，
/// 既允许协作又不放任 fan-out。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerPolicy {
    /// 是否允许广播（`Recipients::Broadcast`）。
    pub allow_broadcast: bool,
    /// 单次直连最大收件人数（`Recipients::Direct`）。
    pub max_direct_recipients: usize,
    /// 单成员同时持有的活跃 fan-out 上限（防嵌套转发风暴）。
    pub max_concurrent_outbound_per_member: usize,
}

impl Default for PeerPolicy {
    fn default() -> Self {
        Self {
            allow_broadcast: false,
            max_direct_recipients: 4,
            max_concurrent_outbound_per_member: 4,
        }
    }
}

impl PeerPolicy {
    /// 宽松策略（允许广播、上限 8）——仅在受信任 team 中由 supervisor 显式开启。
    pub fn permissive() -> Self {
        Self {
            allow_broadcast: true,
            max_direct_recipients: 8,
            max_concurrent_outbound_per_member: 8,
        }
    }

    /// 鉴权一次 peer fan-out。
    ///
    /// - 校验所有直连收件人都是 team 成员；
    /// - 校验广播开关；
    /// - 校验直连数量不超上限；
    /// - 校验发送者当前并发 fan-out 不超上限。
    ///
    /// 返回归一化后的收件人集合（广播展开为除发送者外的全部成员），失败返回
    /// [`TeamError::FanOutDenied`]。
    pub fn authorize(
        &self,
        sender: &AgentId,
        recipients: &Recipients,
        members: &BTreeSet<AgentId>,
        sender_active_fan_out: usize,
    ) -> Result<BTreeSet<AgentId>, TeamError> {
        if !members.contains(sender) {
            return Err(TeamError::FanOutDenied {
                reason: format!("sender {} is not a team member", sender),
            });
        }
        if sender_active_fan_out >= self.max_concurrent_outbound_per_member {
            return Err(TeamError::FanOutDenied {
                reason: format!(
                    "sender {} already has {sender_active_fan_out} active fan-outs (limit {})",
                    sender, self.max_concurrent_outbound_per_member
                ),
            });
        }
        match recipients {
            Recipients::Broadcast => {
                if !self.allow_broadcast {
                    return Err(TeamError::FanOutDenied {
                        reason: "broadcast fan-out is disabled by policy".into(),
                    });
                }
                Ok(members.iter().filter(|m| *m != sender).cloned().collect())
            }
            Recipients::Direct { members: list } => {
                let unique: BTreeSet<AgentId> = list.iter().cloned().collect();
                if unique.len() > self.max_direct_recipients {
                    return Err(TeamError::FanOutDenied {
                        reason: format!(
                            "direct fan-out {} exceeds limit {}",
                            unique.len(),
                            self.max_direct_recipients
                        ),
                    });
                }
                for m in &unique {
                    if !members.contains(m) {
                        return Err(TeamError::FanOutDenied {
                            reason: format!("recipient {m} is not a team member"),
                        });
                    }
                }
                Ok(unique)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn members() -> BTreeSet<AgentId> {
        ["a", "b", "c", "d", "e"]
            .into_iter()
            .map(AgentId::from)
            .collect()
    }

    #[test]
    fn direct_within_limit_is_authorized() {
        let policy = PeerPolicy::default();
        let recipients = Recipients::Direct {
            members: vec![AgentId::from("b"), AgentId::from("c")],
        };
        let auth = policy
            .authorize(&AgentId::from("a"), &recipients, &members(), 0)
            .expect("within limit");
        assert_eq!(auth.len(), 2);
    }

    #[test]
    fn direct_over_limit_is_denied() {
        let policy = PeerPolicy::default(); // max 4
        let recipients = Recipients::Direct {
            members: vec!["b", "c", "d", "e", "a"]
                .into_iter()
                .map(AgentId::from)
                .collect(),
        };
        let err = policy
            .authorize(&AgentId::from("a"), &recipients, &members(), 0)
            .unwrap_err();
        assert!(matches!(err, TeamError::FanOutDenied { .. }));
    }

    #[test]
    fn broadcast_denied_by_default_policy() {
        let policy = PeerPolicy::default();
        let err = policy
            .authorize(&AgentId::from("a"), &Recipients::Broadcast, &members(), 0)
            .unwrap_err();
        assert!(matches!(err, TeamError::FanOutDenied { .. }));
    }

    #[test]
    fn broadcast_authorized_under_permissive_policy() {
        let policy = PeerPolicy::permissive();
        let auth = policy
            .authorize(&AgentId::from("a"), &Recipients::Broadcast, &members(), 0)
            .expect("permissive broadcast");
        assert_eq!(auth.len(), 4); // 除发送者 a 外全部成员
        assert!(!auth.contains(&AgentId::from("a")));
    }

    #[test]
    fn non_member_recipient_is_denied() {
        let policy = PeerPolicy::default();
        let recipients = Recipients::Direct {
            members: vec![AgentId::from("z")],
        };
        let err = policy
            .authorize(&AgentId::from("a"), &recipients, &members(), 0)
            .unwrap_err();
        assert!(matches!(err, TeamError::FanOutDenied { .. }));
    }
}
