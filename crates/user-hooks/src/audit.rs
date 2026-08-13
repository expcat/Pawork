//! 审计记录与派发结果（P17-1 步骤 5、8）。
//!
//! 所有审计记录在落库前已对 secret 明文 redaction；PromptTransform 额外记录
//! before/after 摘要与 diff，供审计与重放。

use crate::config::HookScope;
use crate::error::HookStatus;
use crate::exec::JudgeDecision;
use crate::trigger::TriggerPoint;
use agent_domain::{EventId, Timestamp};
use serde::{Deserialize, Serialize};

/// 一次 PromptTransform 的可审计 diff。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptTransformDiff {
    pub target: String,
    /// 改写前后文本的 blake3 摘要前 16 字符 hex（不泄露明文）。
    pub before_digest: String,
    pub after_digest: String,
    /// 改写前后前 N 字符摘要（已 redaction）。
    pub before_excerpt: String,
    pub after_excerpt: String,
    /// 是否改写了 system prompt（用于审计高亮）。
    pub touched_system: bool,
}

impl PromptTransformDiff {
    const EXCERPT_LEN: usize = 80;

    pub fn new(target: &str, before: &str, after: &str, touched_system: bool) -> Self {
        Self {
            target: target.to_string(),
            before_digest: digest_hex(before),
            after_digest: digest_hex(after),
            before_excerpt: excerpt(before, Self::EXCERPT_LEN),
            after_excerpt: excerpt(after, Self::EXCERPT_LEN),
            touched_system,
        }
    }
}

fn digest_hex(text: &str) -> String {
    // FNV-1a 64 位：避免引入 blake3 依赖到本 crate；审计摘要无需密码学强度，
    // 只需稳定唯一标识改写前后文本。app-service 落库时可再叠加 canonical hash。
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in text.as_bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn excerpt(text: &str, max: usize) -> String {
    let clean: String = text.chars().take(max).collect();
    clean.replace('\n', "\\n")
}

/// 单条 hook 派发的审计记录。
///
/// **canonical / replay**：[`UserHookEvent`] 是持久化到 Event Store 的最小信封，
/// 带 [`USER_HOOK_EVENT_SCHEMA_VERSION`]，可 JSON 往返无损重建（见
/// `tests::user_hook_event_round_trips`）。`AuditSink` 接收该 canonical event。
pub const USER_HOOK_EVENT_SCHEMA_VERSION: u32 = 1;

/// 一条 user hook 派发的 canonical、versioned 审计事件。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserHookEvent {
    pub schema_version: u32,
    pub event_id: EventId,
    pub timestamp: Timestamp,
    pub hook_id: String,
    pub trigger: TriggerPoint,
    pub scope: HookScope,
    pub capability: String,
    pub lifecycle: String,
    pub payload: UserHookEventPayload,
}

impl UserHookEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_id: EventId,
        timestamp: Timestamp,
        hook_id: String,
        trigger: TriggerPoint,
        scope: HookScope,
        capability: String,
        lifecycle: String,
        payload: UserHookEventPayload,
    ) -> Self {
        Self {
            schema_version: USER_HOOK_EVENT_SCHEMA_VERSION,
            event_id,
            timestamp,
            hook_id,
            trigger,
            scope,
            capability,
            lifecycle,
            payload,
        }
    }

    /// 若是 Dispatch 载荷，返回其 status；Transform 返回 `None`。
    pub fn dispatch_status(&self) -> Option<&HookStatus> {
        match &self.payload {
            UserHookEventPayload::Dispatch { status, .. } => Some(status),
            UserHookEventPayload::Transform { .. } => None,
        }
    }
}

/// User hook canonical 事件载荷。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserHookEventPayload {
    /// 一次派发的执行结果（Command/Http/Eval/McpTool/拒绝/超时/async 投递与终态）。
    Dispatch {
        status: HookStatus,
        /// 派发耗时毫秒（async queued 记录为 0）。
        #[serde(default)]
        duration_ms: u64,
        /// 已 redaction 的执行摘要（命令、URL、退出码、判定 reason 等）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// PromptTransform 改写记录（diff + 耗时），供审计与重放。
    Transform {
        diff: PromptTransformDiff,
        #[serde(default)]
        duration_ms: u64,
    },
}

/// 单个同步 handler 的回灌结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookEffect {
    /// 无副作用回灌（Command/Http async、通知类）。
    None,
    /// PromptTransform 改写结果。
    Transform {
        target: String,
        new_prompt: String,
        diff: PromptTransformDiff,
    },
    /// PromptEval / AgentEval / McpTool 的判定。
    Decision(JudgeDecision),
}

/// 一次 dispatch 的聚合结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DispatchOutcome {
    /// 同步 handler 的回灌效果（按 handler id）。
    pub effects: Vec<(String, HookEffect)>,
    /// 已投递的 async handler id（不阻塞 run loop）。
    pub fired_async: Vec<String>,
    /// 被策略拒绝的 handler id 与原因。
    pub denied: Vec<(String, String)>,
    /// 产出的审计记录（已 redaction）。
    pub audit: Vec<UserHookEvent>,
}

impl DispatchOutcome {
    pub fn push_effect(&mut self, hook_id: impl Into<String>, effect: HookEffect) {
        self.effects.push((hook_id.into(), effect));
    }

    /// 合并另一次派发结果（多 workspace / 多触发点聚合）。
    ///
    /// 回灌效果、async 投递、策略拒绝与审计记录全部按序拼接；`is_denied`
    /// 与 `transformed_prompt` 的语义在合并后保持不变（任一源拒绝即拒绝，
    /// 同 target 改写按合并顺序取最后一条已基于前序结果计算的终值）。
    pub fn merge(&mut self, other: DispatchOutcome) {
        self.effects.extend(other.effects);
        self.fired_async.extend(other.fired_async);
        self.denied.extend(other.denied);
        self.audit.extend(other.audit);
    }

    /// 读取 target 的最终改写。dispatcher 会让每条 transform 基于该 target
    /// 当前值计算，因此这里只取最后一个终值，绝不能再次 prefix 造成双写。
    pub fn transformed_prompt(&self, target: &str, original: &str) -> String {
        let mut out = original.to_string();
        for (_id, effect) in &self.effects {
            if let HookEffect::Transform {
                target: t,
                new_prompt,
                ..
            } = effect
            {
                if t == target {
                    out.clone_from(new_prompt);
                }
            }
        }
        out
    }

    /// 是否任一 Eval/AgentEval/McpTool handler 阻断。
    pub fn is_denied(&self) -> bool {
        !self.denied.is_empty()
            || self
                .effects
                .iter()
                .any(|(_, e)| matches!(e, HookEffect::Decision(JudgeDecision::Deny { .. })))
    }
}
