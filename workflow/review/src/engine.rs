//! ReviewEngine：状态机、apply / 重放与命令入口。
//!
//! event-sourcing 三件套（进程内内存实现，持久化由 session-store 负责）：
//! 1. [`ReviewState`]（`Mutex` 包裹）保存聚合状态；
//! 2. [`ReviewState::apply`] 纯函数折叠 canonical 事件——重放 / 恢复的唯一入口；
//! 3. 命令方法校验状态机合法性，**先 apply 到 state 再返回**待持久化事件。
//!
//! 评审引擎对工作区只读：锚点解析 / re-anchor 只读文件，SuggestedPatch 只做
//! dry-run，不执行任何写。

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

use pawork_domain::{
    ReviewAnchor, ReviewEvent, ReviewFindingId, ReviewResolution, ReviewSessionId, ReviewSeverity,
    WorkspaceId,
};
use pawork_git::DiffFile;

use crate::{
    aggregate::{self, AggregateBy, FindingDraft},
    anchor::{AnchorResolver, ReanchorOutcome, StaleReason},
    error::ReviewError,
    forge::{ForgeAdapter, PrReference},
    model::{
        AggregateSnapshot, FindingSnapshot, GroupCount, PRComment, PRContext,
        PublishedCommentRecord, ReviewFinding, ReviewSession, ReviewSessionSnapshot,
        SuggestedPatch,
    },
    patch::PatchValidator,
};

const SESSION_PREFIX: &str = "session_";
const FINDING_PREFIX: &str = "finding_";

/// 聚合状态（重放后可直接查询）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewState {
    pub sessions: BTreeMap<ReviewSessionId, ReviewSession>,
}

impl ReviewState {
    /// 纯函数折叠一个 canonical 事件。这是重放 / 恢复的唯一入口：
    /// 崩溃后按序重放事件序列即可重建当前状态；非法事件返回错误。
    pub fn apply(&mut self, event: &ReviewEvent) -> Result<(), ReviewError> {
        match event {
            ReviewEvent::SessionCreated {
                session_id,
                workspace_id,
            } => {
                if self.sessions.contains_key(session_id) {
                    return Err(ReviewError::Duplicate {
                        kind: "session",
                        id: session_id.to_string(),
                    });
                }
                self.sessions.insert(
                    session_id.clone(),
                    ReviewSession {
                        session_id: session_id.clone(),
                        workspace_id: workspace_id.clone(),
                        findings: BTreeMap::new(),
                        published_comments: Vec::new(),
                    },
                );
            }
            ReviewEvent::FindingOpened {
                session_id,
                finding_id,
                anchor,
                severity,
                body,
                evidence,
                assignee,
                suggested_patch,
                fingerprint,
            } => {
                let session = self
                    .sessions
                    .get_mut(session_id)
                    .ok_or_else(|| ReviewError::UnknownSession(session_id.to_string()))?;
                if session.findings.contains_key(finding_id) {
                    return Err(ReviewError::Duplicate {
                        kind: "finding",
                        id: finding_id.to_string(),
                    });
                }
                session.findings.insert(
                    finding_id.clone(),
                    ReviewFinding {
                        finding_id: finding_id.clone(),
                        anchor: anchor.clone(),
                        severity: *severity,
                        body: body.clone(),
                        evidence: evidence.clone(),
                        assignee: assignee.clone(),
                        resolution: ReviewResolution::Open,
                        fix_ref: None,
                        suggested_patch: suggested_patch.clone(),
                        anchor_fingerprint: fingerprint.clone(),
                    },
                );
            }
            ReviewEvent::FindingResolved {
                finding_id,
                resolution,
                fix_ref,
            } => {
                let finding = self.find_mut(finding_id)?;
                validate_transition(finding.resolution, *resolution)?;
                finding.resolution = *resolution;
                finding.fix_ref = fix_ref.clone();
            }
            ReviewEvent::CommentPublished {
                session_id,
                finding_id,
                forge,
            } => {
                let session = self
                    .sessions
                    .get_mut(session_id)
                    .ok_or_else(|| ReviewError::UnknownSession(session_id.to_string()))?;
                if !session.findings.contains_key(finding_id) {
                    return Err(ReviewError::UnknownFinding(finding_id.to_string()));
                }
                session.published_comments.push(PublishedCommentRecord {
                    finding_id: finding_id.clone(),
                    forge: forge.clone(),
                });
            }
        }
        Ok(())
    }

    fn find_mut(
        &mut self,
        finding_id: &ReviewFindingId,
    ) -> Result<&mut ReviewFinding, ReviewError> {
        for session in self.sessions.values_mut() {
            if let Some(finding) = session.findings.get_mut(finding_id) {
                return Ok(finding);
            }
        }
        Err(ReviewError::UnknownFinding(finding_id.to_string()))
    }

    pub fn session(&self, session_id: &ReviewSessionId) -> Result<&ReviewSession, ReviewError> {
        self.sessions
            .get(session_id)
            .ok_or_else(|| ReviewError::UnknownSession(session_id.to_string()))
    }

    pub fn finding(
        &self,
        session_id: &ReviewSessionId,
        finding_id: &ReviewFindingId,
    ) -> Result<&ReviewFinding, ReviewError> {
        let session = self.session(session_id)?;
        session
            .findings
            .get(finding_id)
            .ok_or_else(|| ReviewError::UnknownFinding(finding_id.to_string()))
    }

    pub fn finding_mut(
        &mut self,
        session_id: &ReviewSessionId,
        finding_id: &ReviewFindingId,
    ) -> Result<&mut ReviewFinding, ReviewError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| ReviewError::UnknownSession(session_id.to_string()))?;
        session
            .findings
            .get_mut(finding_id)
            .ok_or_else(|| ReviewError::UnknownFinding(finding_id.to_string()))
    }

    /// 确定性 ID 生成：取现有最大数值后缀 +1，重放后继续发命令不会冲突。
    pub fn next_session_id(&self) -> ReviewSessionId {
        ReviewSessionId::new(format!(
            "{SESSION_PREFIX}{}",
            Self::max_suffix(SESSION_PREFIX, self.sessions.keys().map(|k| k.as_str())) + 1
        ))
    }

    pub fn next_finding_id(&self) -> ReviewFindingId {
        let max = self
            .sessions
            .values()
            .flat_map(|s| s.findings.keys().map(|k| k.as_str()))
            .chain(std::iter::once(""))
            .map(|k| {
                k.strip_prefix(FINDING_PREFIX)
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0);
        ReviewFindingId::new(format!("{FINDING_PREFIX}{}", max + 1))
    }

    fn max_suffix<'a>(prefix: &str, keys: impl Iterator<Item = &'a str>) -> u64 {
        keys.filter_map(|k| k.strip_prefix(prefix).and_then(|s| s.parse::<u64>().ok()))
            .max()
            .unwrap_or(0)
    }
}

/// resolution 生命周期：`open → addressed → resolved | wontfix`。
fn validate_transition(from: ReviewResolution, to: ReviewResolution) -> Result<(), ReviewError> {
    let legal = matches!(
        (from, to),
        (ReviewResolution::Open, ReviewResolution::Addressed)
            | (ReviewResolution::Addressed, ReviewResolution::Resolved)
            | (ReviewResolution::Addressed, ReviewResolution::Wontfix)
    );
    if legal {
        Ok(())
    } else {
        Err(ReviewError::InvalidTransition { from, to })
    }
}

/// 打开 finding 的输入（富快照字段在命令时写入内存态）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenFindingInput {
    pub session_id: ReviewSessionId,
    pub anchor: ReviewAnchor,
    pub severity: ReviewSeverity,
    pub body: String,
    pub evidence: Vec<String>,
    pub assignee: Option<String>,
    pub suggested_patch: Option<SuggestedPatch>,
}

/// Review Engine 命令入口。
pub struct ReviewEngine {
    state: Mutex<ReviewState>,
    anchor: AnchorResolver,
}

impl ReviewEngine {
    pub fn new(workspace_root: Option<PathBuf>) -> Self {
        Self {
            state: Mutex::new(ReviewState::default()),
            anchor: AnchorResolver::new(workspace_root),
        }
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, ReviewState>, ReviewError> {
        self.state.lock().map_err(|_| ReviewError::StatePoisoned)
    }

    /// 追加一个 canonical 事件（调用方持久化后的回放入口）。
    pub fn apply(&self, event: &ReviewEvent) -> Result<(), ReviewError> {
        self.lock_state()?.apply(event)
    }

    /// 用一串事件重建状态（重放 / 恢复）。
    pub fn replay<I>(&self, events: I) -> Result<(), ReviewError>
    where
        I: IntoIterator<Item = ReviewEvent>,
    {
        let mut state = ReviewState::default();
        for event in events {
            state.apply(&event)?;
        }
        *self.lock_state()? = state;
        Ok(())
    }

    /// 创建评审会话。
    pub fn create_session(
        &self,
        workspace_id: Option<WorkspaceId>,
    ) -> Result<ReviewEvent, ReviewError> {
        let mut state = self.lock_state()?;
        let session_id = state.next_session_id();
        let event = ReviewEvent::SessionCreated {
            session_id,
            workspace_id,
        };
        state.apply(&event)?;
        Ok(event)
    }

    /// 打开一条 finding：校验锚点（只读解析 + 指纹）、补丁 dry-run，
    /// 先 apply 事件再返回。
    pub fn open_finding(&self, input: OpenFindingInput) -> Result<ReviewEvent, ReviewError> {
        let mut state = self.lock_state()?;
        state.session(&input.session_id)?;
        let resolved = self.anchor.resolve_optional(&input.anchor)?;
        if let Some(patch) = &input.suggested_patch {
            PatchValidator::dry_run(patch)?;
        }
        let finding_id = state.next_finding_id();
        let event = ReviewEvent::FindingOpened {
            session_id: input.session_id.clone(),
            finding_id: finding_id.clone(),
            anchor: input.anchor.clone(),
            severity: input.severity,
            body: input.body.clone(),
            evidence: input.evidence,
            assignee: input.assignee,
            suggested_patch: input.suggested_patch,
            fingerprint: resolved.fingerprint,
        };
        state.apply(&event)?;
        Ok(event)
    }

    /// resolution 转移（`open → addressed → resolved | wontfix`），可关联修复引用。
    pub fn resolve_finding(
        &self,
        session_id: &ReviewSessionId,
        finding_id: &ReviewFindingId,
        resolution: ReviewResolution,
        fix_ref: Option<String>,
    ) -> Result<ReviewEvent, ReviewError> {
        let mut state = self.lock_state()?;
        let finding = state.finding(session_id, finding_id)?;
        validate_transition(finding.resolution, resolution)?;
        let event = ReviewEvent::FindingResolved {
            finding_id: finding_id.clone(),
            resolution,
            fix_ref: fix_ref.clone(),
        };
        state.apply(&event)?;
        Ok(event)
    }

    /// 导入 diff 范围：为每条变更（Addition）行生成 finding，并使其进入
    /// 待发布评论集合（`export_comments` 生成评论，不自动发布）。
    pub fn import_diff(
        &self,
        session_id: &ReviewSessionId,
        diff: &DiffFile,
    ) -> Result<Vec<ReviewEvent>, ReviewError> {
        let mut state = self.lock_state()?;
        state.session(session_id)?;
        let drafts: Vec<FindingDraft> = aggregate::drafts_from_diff(diff);
        let mut events = Vec::with_capacity(drafts.len());
        for draft in drafts {
            let resolved = self.anchor.resolve_optional(&draft.anchor)?;
            let finding_id = state.next_finding_id();
            let event = ReviewEvent::FindingOpened {
                session_id: session_id.clone(),
                finding_id: finding_id.clone(),
                anchor: draft.anchor,
                severity: draft.severity,
                body: draft.body,
                evidence: draft.evidence,
                assignee: None,
                suggested_patch: None,
                fingerprint: resolved.fingerprint,
            };
            state.apply(&event)?;
            events.push(event);
        }
        Ok(events)
    }

    /// 对单个 finding 做 re-anchor（只读查询）。
    pub fn reanchor(
        &self,
        session_id: &ReviewSessionId,
        finding_id: &ReviewFindingId,
    ) -> Result<ReanchorOutcome, ReviewError> {
        let state = self.lock_state()?;
        let finding = state.finding(session_id, finding_id)?;
        self.anchor
            .reanchor(&finding.anchor, finding.anchor_fingerprint.as_deref())
    }

    /// 会话快照：含 re-anchor 派生结果（编辑后自动重新定位；漂移标 stale）、
    /// 待发布评论与按 file / severity / status 的聚合。
    pub fn snapshot(
        &self,
        session_id: &ReviewSessionId,
    ) -> Result<ReviewSessionSnapshot, ReviewError> {
        let state = self.lock_state()?;
        let session = state.session(session_id)?;
        let mut findings = Vec::with_capacity(session.findings.len());
        for finding in session.findings.values() {
            let outcome = self
                .anchor
                .reanchor(&finding.anchor, finding.anchor_fingerprint.as_deref())
                .unwrap_or(ReanchorOutcome {
                    anchor: finding.anchor.clone(),
                    stale: true,
                    reason: StaleReason::FileUnavailable,
                });
            findings.push(FindingSnapshot {
                finding_id: finding.finding_id.clone(),
                anchor: outcome.anchor,
                stale: outcome.stale,
                stale_reason: outcome.stale.then(|| outcome.reason.label().to_string()),
                severity: finding.severity,
                body: finding.body.clone(),
                evidence: finding.evidence.clone(),
                assignee: finding.assignee.clone(),
                resolution: finding.resolution,
                fix_ref: finding.fix_ref.clone(),
                suggested_patch: finding.suggested_patch.clone(),
            });
        }
        let aggregate = AggregateSnapshot {
            by_file: aggregate::aggregate(session.findings.values(), AggregateBy::File),
            by_severity: aggregate::aggregate(session.findings.values(), AggregateBy::Severity),
            by_status: aggregate::aggregate(session.findings.values(), AggregateBy::Status),
        };
        Ok(ReviewSessionSnapshot {
            session_id: session.session_id.clone(),
            workspace_id: session.workspace_id.clone(),
            findings,
            published_comments: session.published_comments.clone(),
            pending_comments: aggregate::pending_comments(session.findings.values()),
            aggregate,
        })
    }

    /// 按维度聚合（file / severity / status）。
    pub fn aggregate(
        &self,
        session_id: &ReviewSessionId,
        by: AggregateBy,
    ) -> Result<Vec<GroupCount>, ReviewError> {
        let state = self.lock_state()?;
        let session = state.session(session_id)?;
        Ok(aggregate::aggregate(session.findings.values(), by))
    }

    /// 生成待发布评论（映射为平台无关 [`PRComment`]），**不发布**。
    pub fn export_comments(
        &self,
        session_id: &ReviewSessionId,
        adapter: &dyn ForgeAdapter,
        pr: &PrReference,
    ) -> Result<(PRContext, Vec<PRComment>), ReviewError> {
        let state = self.lock_state()?;
        let session = state.session(session_id)?;
        let context = adapter.fetch_pr_context(pr)?;
        let comments = aggregate::pending_comments(session.findings.values())
            .into_iter()
            .map(|pending| {
                let finding = session
                    .findings
                    .get(&pending.finding_id)
                    .expect("pending comment 一定对应已存在 finding");
                adapter.map_comment(&context, finding)
            })
            .collect();
        Ok((context, comments))
    }

    /// 显式发布评论：这是调用 `adapter.publish_comment`（外部副作用）的唯一路径，
    /// 仅在用户显式调用时执行；成功后才 emit `CommentPublished`。
    pub fn publish_comment(
        &self,
        session_id: &ReviewSessionId,
        finding_id: &ReviewFindingId,
        adapter: &dyn ForgeAdapter,
        pr: &PrReference,
    ) -> Result<ReviewEvent, ReviewError> {
        let finding = {
            let state = self.lock_state()?;
            state.finding(session_id, finding_id)?.clone()
        };
        // 外部副作用不放锁内执行。
        let context = adapter.fetch_pr_context(pr)?;
        let comment = adapter.map_comment(&context, &finding);
        let published = adapter.publish_comment(&context, &comment)?;
        if !published.published {
            return Err(ReviewError::Forge(
                "adapter 返回的评论未标记为已发布".to_string(),
            ));
        }
        let mut state = self.lock_state()?;
        let event = ReviewEvent::CommentPublished {
            session_id: session_id.clone(),
            finding_id: finding_id.clone(),
            forge: adapter.kind().as_str().to_string(),
        };
        state.apply(&event)?;
        Ok(event)
    }
}
