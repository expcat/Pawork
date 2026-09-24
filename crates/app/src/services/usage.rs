//! Usage 领域服务：usage 对账（overview / session / last run）与费用估算。

use pawork_domain::{
    AgentEvent, Cost, ModelId, ProviderId, RequestId, RunId, SessionId, TokenUsage,
};

use crate::control::{self, ControlPlaneRuntime, UsageOverview};
use crate::{AppCore, AppError};

pub(crate) struct UsageService {
    pub(crate) control: ControlPlaneRuntime,
}

impl UsageService {
    pub(crate) fn in_memory() -> Self {
        Self {
            control: ControlPlaneRuntime::in_memory(),
        }
    }

    pub(crate) async fn projected_run_usage(
        &self,
        core: &AppCore,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> Option<TokenUsage> {
        let runs = core
            .store()
            .ok()?
            .projection_snapshot(session_id)
            .await
            .ok()?
            .runs;
        runs.iter()
            .find(|run| run.run_id == *run_id)
            .and_then(|run| usage_from_run_json(&run.data))
    }

    pub(crate) async fn record_completed_usage(
        &self,
        core: &AppCore,
        session_id: &SessionId,
        run_id: &RunId,
        request_id: &RequestId,
        usage: &TokenUsage,
    ) -> Result<(), AppError> {
        self.record_attributed_usage(
            core,
            session_id,
            run_id,
            request_id,
            &core.provider_id,
            &core.model,
            usage,
            pawork_engine::now_timestamp().as_unix_millis(),
        )
        .await
    }

    /// 带显式归属的入账：正常路径用 core 当前 provider/model；启动对账
    /// （R-22）用持久事件恢复的归属，不拿当前配置顶替历史事实。
    /// R-13：账本成功写入后失效该 scope 的本地派生 quota 窗口缓存，
    /// 读→写→立即读不再拿到入账前旧窗口；远端权威额度缓存口径不变。
    async fn record_attributed_usage(
        &self,
        core: &AppCore,
        session_id: &SessionId,
        run_id: &RunId,
        request_id: &RequestId,
        provider_id: &ProviderId,
        model_id: &ModelId,
        usage: &TokenUsage,
        occurred_at_ms: u64,
    ) -> Result<(), AppError> {
        let cost = self.estimate_cost_for(core, model_id, usage);
        let mut record = control::usage_record(
            session_id,
            run_id,
            request_id,
            provider_id,
            model_id,
            usage,
            cost.as_ref().map(|item| item.amount_micros).unwrap_or(0),
            cost.as_ref()
                .map(|item| item.currency.as_str())
                .unwrap_or(""),
        );
        record.occurred_at_ms = occurred_at_ms;
        self.control
            .ledger
            .record(record)
            .await
            .map_err(|error| AppError::ControlPlane(error.to_string()))?;
        self.control
            .quota
            .invalidate_local_scope(&control::quota_scope(provider_id));
        Ok(())
    }

    /// R-22 启动对账：持久终态已知用量的 run 若账本缺记录则幂等补账。
    /// - 归属（request_id / provider / model）只取自该 run 首个
    ///   `ProviderRequestStarted` 持久事件——缺归属不猜，跳过并告警；
    /// - 幂等：先按 (tenant, account, run_id) 查账本，命中即跳过；补账
    ///   与正常入账共用 `usage_record` 构造（record_id 确定性），跨
    ///   进程撞键由账本 (tenant, account, request_id, attempt) 去重兜底；
    /// - 只写账本，不改 Run 终态、不追加事件；
    /// - 单 session 失败只 warn，不阻断启动。
    pub(crate) async fn reconcile_ledger_from_terminal_runs(&self, core: &AppCore) {
        let sessions = match core.store() {
            Ok(store) => match store.list_sessions_including_archived().await {
                Ok(sessions) => sessions,
                Err(error) => {
                    tracing::warn!(error = %error, "usage reconcile skipped: list sessions failed");
                    return;
                }
            },
            Err(error) => {
                tracing::warn!(error = %error, "usage reconcile skipped: store not open");
                return;
            }
        };
        for record in sessions {
            let session_id = SessionId::from(record.session_id.as_str());
            if let Err(error) = self.reconcile_session_ledger(core, &session_id).await {
                tracing::warn!(
                    session_id = session_id.as_str(),
                    error = %error,
                    "usage reconcile failed for session; continuing startup"
                );
            }
        }
    }

    async fn reconcile_session_ledger(
        &self,
        core: &AppCore,
        session_id: &SessionId,
    ) -> Result<(), AppError> {
        let snapshot = core.store()?.projection_snapshot(session_id).await?;
        let terminal: Vec<(RunId, TokenUsage)> = snapshot
            .runs
            .iter()
            .filter(|run| matches!(run.state.as_str(), "completed" | "failed" | "cancelled"))
            .filter_map(|run| {
                usage_from_run_json(&run.data)
                    .filter(|usage| !usage.is_zero())
                    .map(|usage| (run.run_id.clone(), usage))
            })
            .collect();
        if terminal.is_empty() {
            return Ok(());
        }
        // 每个 run 的首个 ProviderRequestStarted 归属（事件按序重放，
        // 首个 request_id 即正常入账使用的那个）。
        let events = core
            .store()?
            .replay_events(session_id, 0, usize::MAX)
            .await?;
        let mut attribution: std::collections::HashMap<RunId, (RequestId, ProviderId, String)> =
            std::collections::HashMap::new();
        let mut usage_times = std::collections::HashMap::new();
        for envelope in &events {
            match &envelope.payload {
                AgentEvent::UsageUpdated { .. } => {
                    usage_times
                        .insert(envelope.run_id.clone(), envelope.timestamp.as_unix_millis());
                }
                AgentEvent::RunCompleted { .. }
                | AgentEvent::RunFailed { .. }
                | AgentEvent::RunCancelled { .. } => {
                    usage_times
                        .entry(envelope.run_id.clone())
                        .or_insert(envelope.timestamp.as_unix_millis());
                }
                _ => {}
            }
            if let AgentEvent::ProviderRequestStarted {
                request_id,
                provider_id,
                model,
            } = &envelope.payload
            {
                attribution
                    .entry(envelope.run_id.clone())
                    .or_insert_with(|| (request_id.clone(), provider_id.clone(), model.clone()));
            }
        }
        for (run_id, usage) in terminal {
            let Some(&occurred_at_ms) = usage_times.get(&run_id) else {
                continue;
            };
            let Some((request_id, provider_id, model)) = attribution.get(&run_id) else {
                tracing::warn!(
                    run_id = run_id.as_str(),
                    "usage reconcile skipped: no persisted ProviderRequestStarted attribution"
                );
                continue;
            };
            match control::ledger_has_run(self.control.ledger.as_ref(), &run_id).await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(
                        run_id = run_id.as_str(),
                        error = %error,
                        "usage reconcile ledger query failed"
                    );
                    continue;
                }
            }
            if let Err(error) = self
                .record_attributed_usage(
                    core,
                    session_id,
                    &run_id,
                    request_id,
                    provider_id,
                    &ModelId::from(model.as_str()),
                    &usage,
                    occurred_at_ms,
                )
                .await
            {
                tracing::warn!(
                    run_id = run_id.as_str(),
                    error = %error,
                    "usage reconcile record failed"
                );
            }
        }
        Ok(())
    }

    pub async fn usage_overview(
        &self,
        core: &AppCore,
        provider_id: Option<&str>,
        session: Option<&SessionId>,
    ) -> Result<UsageOverview, AppError> {
        let provider = match provider_id {
            Some(id) if !id.trim().is_empty() => ProviderId::from(id),
            _ => core.provider_id.clone(),
        };
        if provider.as_str() == "catalog" || provider.as_str().is_empty() {
            return Err(AppError::ControlPlane(
                "pawork usage 需要 --provider（或已配置的 default_provider）".into(),
            ));
        }
        let session_line = if let Some(session_id) = session {
            let usage = self.session_usage(core, session_id).await?;
            Some(control::SessionUsageLine {
                session_id: session_id.as_str().to_string(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_write_tokens: usage.cache_write_tokens,
            })
        } else {
            None
        };
        let totals =
            control::ledger_totals(self.control.ledger.as_ref(), &provider, session).await?;
        let windows = control::quota_windows(&self.control.quota, &provider).await?;
        Ok(UsageOverview {
            provider_id: provider.as_str().to_string(),
            session: session_line,
            ledger: totals.into(),
            windows,
        })
    }

    pub async fn session_usage(
        &self,
        core: &AppCore,
        session_id: &SessionId,
    ) -> Result<TokenUsage, AppError> {
        Ok(self.session_usage_inner(core, session_id).await?.0)
    }

    pub async fn last_run_usage(
        &self,
        core: &AppCore,
        session_id: &SessionId,
    ) -> Result<Option<TokenUsage>, AppError> {
        Ok(self.session_usage_inner(core, session_id).await?.1)
    }

    async fn session_usage_inner(
        &self,
        core: &AppCore,
        session_id: &SessionId,
    ) -> Result<(TokenUsage, Option<TokenUsage>), AppError> {
        let runs = core.store()?.projection_snapshot(session_id).await?.runs;
        let mut total = TokenUsage::default();
        let mut last = None;
        for run in runs
            .iter()
            .filter(|run| matches!(run.state.as_str(), "completed" | "failed" | "cancelled"))
            .rev()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            if let Some(usage) = usage_from_run_json(&run.data) {
                // 按时间正序遍历，持续覆盖：最终拿到的是最新 completed run
                // 的 usage（get_or_insert 会冻结在最早一轮，REPL 每轮用量行
                // 因此显示过期数据，S5 波 C 冒烟实测发现）。
                last = Some(usage.clone());
                total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
                total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
                total.cache_read_tokens = total
                    .cache_read_tokens
                    .saturating_add(usage.cache_read_tokens);
                total.cache_write_tokens = total
                    .cache_write_tokens
                    .saturating_add(usage.cache_write_tokens);
            }
        }
        Ok((total, last))
    }

    /// 按 registry 定价估算费用；无定价条目返回 None（不编造）。
    pub fn estimate_cost_for(
        &self,
        core: &AppCore,
        model: &ModelId,
        usage: &TokenUsage,
    ) -> Option<Cost> {
        let entry = core.registry.resolve(model.as_str())?;
        let pricing = entry.pricing.as_ref()?;
        Some(pawork_providers::estimate_cost(usage, pricing))
    }
}

fn usage_from_run_json(data: &serde_json::Value) -> Option<TokenUsage> {
    data.get("data")
        .and_then(|inner| inner.get("usage"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use pawork_domain::{
        AgentEvent, CancellationToken, CanonicalModelRequest, MessageId, ModelDefinition, ModelId,
        ModelProvider, ModelResponseSummary, ProviderError, ProviderId, ProviderStreamEvent,
        RequestId, ResolvedCredential, RunId, SessionId, StopReason, TokenUsage,
    };
    use pawork_providers::ModelRegistry;

    use crate::testsupport::{
        core_with_registry, mock_core_with_usage, user_hello, RecordingEvents,
    };

    #[tokio::test]
    async fn session_usage_accumulates_completed_runs() {
        let usage = TokenUsage {
            input_tokens: 120,
            output_tokens: 45,
            cache_read_tokens: 10,
            cache_write_tokens: 5,
        };
        let (core, _dir) = mock_core_with_usage(
            vec![
                ProviderStreamEvent::TextDelta("ok".into()),
                ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
            ],
            usage.clone(),
        )
        .await;
        let session = core.create_session("usage").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn 1");
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn 2");

        let total = core.session_usage(&session).await.expect("total");
        assert_eq!(total.input_tokens, 240);
        assert_eq!(total.output_tokens, 90);
        assert_eq!(total.cache_read_tokens, 20);
        assert_eq!(total.cache_write_tokens, 10);
        let last = core
            .last_run_usage(&session)
            .await
            .expect("last")
            .expect("at least one completed run");
        assert_eq!(last, usage);
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn reconcile_backfills_missing_ledger_record_once() {
        // R-22：终态已知用量但账本缺记录（模拟终态后入账失败）——启动对账
        // 幂等补账恰好一次，重复对账不重复，成功 Run 终态与事件流不变。
        use pawork_auth::{MemoryBackend, SecretBackend};
        use pawork_workspace::config::PaworkConfig;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.db");
        let session = SessionId::from("ses-reconcile");
        let run = RunId::from("run-reconcile");
        let occurred_at = pawork_domain::Timestamp::from_unix_millis(1_700_000_000_000);
        let usage = TokenUsage {
            input_tokens: 200,
            output_tokens: 80,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        {
            let (store, _) = pawork_storage::session::SessionStore::open(&path)
                .await
                .expect("store");
            store
                .create_session(&session, "reconcile", pawork_engine::now_timestamp())
                .await
                .expect("session");
            let payloads = [
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("msg-reconcile"),
                },
                AgentEvent::ProviderRequestStarted {
                    request_id: RequestId::from("req-reconcile-1"),
                    provider_id: ProviderId::from("mock"),
                    model: "glm-5.2".into(),
                },
                AgentEvent::RunCompleted {
                    stop_reason: StopReason::Completed,
                    usage: usage.clone(),
                },
            ];
            for (index, payload) in payloads.into_iter().enumerate() {
                store
                    .append_event(
                        pawork_storage::session::DEFAULT_BRANCH_ID,
                        pawork_domain::AgentEventEnvelope::new(
                            pawork_domain::EventId::from(format!("evt-reconcile-{index}")),
                            session.clone(),
                            run.clone(),
                            pawork_domain::EventSequence::new((index + 1) as u64),
                            occurred_at,
                            payload,
                        ),
                    )
                    .await
                    .expect("append seed event");
            }
            store.shutdown().await.expect("seed shutdown");
        }

        let backend: std::sync::Arc<dyn SecretBackend> = std::sync::Arc::new(MemoryBackend::new());
        let mut core =
            crate::AppCore::from_config_inner(PaworkConfig::default(), None, None, backend, true)
                .await
                .expect("core");
        core.attach_workspace(dir.path()).expect("attach workspace");
        core.open_store(&path).await.expect("open store");
        core.open_control_plane(dir.path()).expect("control");

        async fn records_for_run(
            core: &crate::AppCore,
            run: RunId,
        ) -> Vec<pawork_control_plane::UsageRecord> {
            core.usage
                .control
                .ledger
                .query(&pawork_control_plane::UsageQuery {
                    run_id: Some(run),
                    ..Default::default()
                })
                .await
                .expect("query")
        }

        core.reconcile_usage_ledger().await;
        let records = records_for_run(&core, run.clone()).await;
        assert_eq!(records.len(), 1, "缺记录必须补恰好一条");
        assert_eq!(records[0].input_tokens, 200);
        assert_eq!(records[0].output_tokens, 80);
        assert_eq!(
            records[0].occurred_at_ms,
            occurred_at.as_unix_millis(),
            "补账必须保留历史时间，不能移入本次启动的用量窗口"
        );
        assert_eq!(
            records[0].request_id.as_ref().map(|id| id.as_str()),
            Some("req-reconcile-1"),
            "归属必须取自持久 ProviderRequestStarted"
        );
        assert_eq!(records[0].provider_id.as_str(), "mock");

        let snapshot = core
            .store()
            .expect("store")
            .projection_snapshot(&session)
            .await
            .expect("snapshot");
        assert_eq!(snapshot.runs[0].state, "completed", "成功 Run 终态不变");
        let events = core
            .store()
            .expect("store")
            .replay_events(&session, 0, usize::MAX)
            .await
            .expect("replay");
        assert_eq!(events.len(), 3, "对账只写账本，不追加事件");

        core.reconcile_usage_ledger().await;
        let records = records_for_run(&core, run.clone()).await;
        assert_eq!(records.len(), 1, "重复对账不得重复计账");
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn record_usage_invalidates_local_quota_cache() {
        // R-13：账本成功入账后，同 scope 的本地派生 quota 窗口缓存失效——
        // 读→写→立即读不再拿到入账前的旧窗口。
        let (core, _dir) = mock_core_with_usage(
            vec![
                ProviderStreamEvent::TextDelta("ok".into()),
                ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
            ],
            TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
        )
        .await;
        let scope = crate::control::quota_scope(&ProviderId::from("mock"));
        let snapshot = pawork_control_plane::quota::QuotaSnapshot {
            scope: scope.clone(),
            window: pawork_control_plane::quota::QuotaWindow::Overall,
            unit: pawork_control_plane::quota::QuotaUnit::Token,
            values: pawork_control_plane::quota::QuotaValues::new(
                pawork_control_plane::quota::QuotaMeasure::exact(0),
                pawork_control_plane::quota::QuotaMeasure::exact(100),
                pawork_control_plane::quota::QuotaMeasure::exact(100),
            ),
            reset: pawork_control_plane::quota::QuotaReset::Unknown,
            confidence: pawork_control_plane::quota::Confidence::Derived,
            provenance: pawork_control_plane::quota::QuotaProvenance::new(
                pawork_control_plane::quota::AdapterKind::LocalLedger,
                "test",
                pawork_domain::Timestamp::from_unix_millis(1),
            ),
        };
        core.usage
            .control
            .quota
            .publish_local_snapshot(snapshot)
            .expect("publish");
        assert_eq!(
            core.usage
                .control
                .quota
                .cached_snapshots_for_scope(&scope)
                .len(),
            1
        );

        let session = core.create_session("quota").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        assert!(
            core.usage
                .control
                .quota
                .cached_snapshots_for_scope(&scope)
                .is_empty(),
            "入账后本地派生窗口缓存必须失效"
        );
        core.shutdown().await.expect("shutdown");
    }

    /// 回归（S5 波 C 冒烟发现）：每轮用量行必须取「最新 completed run」的
    /// usage，而不是最早一轮——按次递变 usage 验证 last_run_usage 跟随第 2 轮。
    #[tokio::test]
    async fn last_run_usage_returns_latest_completed_run() {
        struct SteppedUsageProvider {
            usages: Vec<TokenUsage>,
            calls: AtomicUsize,
        }

        #[async_trait]
        impl ModelProvider for SteppedUsageProvider {
            fn id(&self) -> ProviderId {
                ProviderId::from("mock")
            }

            async fn list_models(
                &self,
                _credential: Option<&ResolvedCredential>,
            ) -> Result<Vec<ModelDefinition>, ProviderError> {
                Ok(Vec::new())
            }

            async fn stream(
                &self,
                _request: &CanonicalModelRequest,
                sink: &dyn pawork_domain::ProviderEventSink,
                _cancel: CancellationToken,
            ) -> Result<ModelResponseSummary, ProviderError> {
                let index = self.calls.fetch_add(1, Ordering::SeqCst);
                let usage = self.usages[index.min(self.usages.len() - 1)].clone();
                sink.emit(ProviderStreamEvent::TextDelta("ok".into()))
                    .await?;
                Ok(ModelResponseSummary {
                    stop_reason: StopReason::Completed,
                    usage,
                    response_id: Some("resp-stepped".into()),
                    provider_metadata: Default::default(),
                })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.db");
        let (store, _) = pawork_storage::session::SessionStore::open(&path)
            .await
            .expect("store");
        let core = crate::AppCore::from_parts(
            Arc::new(SteppedUsageProvider {
                usages: vec![
                    TokenUsage {
                        input_tokens: 100,
                        output_tokens: 10,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    },
                    TokenUsage {
                        input_tokens: 222,
                        output_tokens: 22,
                        cache_read_tokens: 4,
                        cache_write_tokens: 0,
                    },
                ],
                calls: AtomicUsize::new(0),
            }),
            None,
            ModelId::from("glm-5.2"),
            ProviderId::from("mock"),
            Some(store),
        );
        let session = core.create_session("stepped").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn 1");
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn 2");

        let last = core
            .last_run_usage(&session)
            .await
            .expect("last")
            .expect("at least one completed run");
        assert_eq!(last.input_tokens, 222);
        assert_eq!(last.output_tokens, 22);
        assert_eq!(last.cache_read_tokens, 4);
        let total = core.session_usage(&session).await.expect("total");
        assert_eq!(total.input_tokens, 322);
        core.shutdown().await.expect("shutdown");
    }

    #[test]
    fn estimate_cost_uses_registry_pricing_and_hides_unpriced() {
        let core = core_with_registry(ModelRegistry::builtin(), "glm-5.2");
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..TokenUsage::default()
        };
        let cost = core
            .estimate_cost_for(&ModelId::from("deepseek-v4-pro"), &usage)
            .expect("deepseek-v4-pro is priced");
        assert_eq!(cost.currency, "USD");
        assert_eq!(cost.amount_micros, 435_000 + 870_000);
        // 订阅制无公开费率、未知条目：不编造费用。
        assert!(core
            .estimate_cost_for(&ModelId::from("glm-5.2"), &usage)
            .is_none());
        assert!(core
            .estimate_cost_for(&ModelId::from("mystery"), &usage)
            .is_none());
    }

    #[tokio::test]
    async fn usage_ledger_matches_session_usage() {
        let usage = TokenUsage {
            input_tokens: 11,
            output_tokens: 7,
            cache_read_tokens: 2,
            cache_write_tokens: 1,
        };
        let (core, _dir) = mock_core_with_usage(
            vec![
                ProviderStreamEvent::TextDelta("hi".into()),
                ProviderStreamEvent::UsageUpdated(usage.clone()),
                ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
            ],
            usage.clone(),
        )
        .await;
        let session = core.create_session("usage").await.expect("create");
        core.chat_turn(
            &session,
            vec![user_hello()],
            &RecordingEvents::default(),
            CancellationToken::new(),
        )
        .await
        .expect("turn");
        let session_usage = core.session_usage(&session).await.expect("session usage");
        assert_eq!(session_usage.input_tokens, 11);
        assert_eq!(session_usage.output_tokens, 7);
        let overview = core
            .usage_overview(Some("mock"), Some(&session))
            .await
            .expect("overview");
        assert_eq!(overview.ledger.input_tokens, session_usage.input_tokens);
        assert_eq!(overview.ledger.output_tokens, session_usage.output_tokens);
        assert_eq!(
            overview.session.map(|line| line.input_tokens),
            Some(session_usage.input_tokens)
        );
        core.shutdown().await.expect("shutdown");
    }
}
