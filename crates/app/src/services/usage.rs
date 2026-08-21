//! Usage 领域服务：usage 对账（overview / session / last run）与费用估算。

use pawork_domain::{Cost, ModelId, ProviderId, RequestId, RunId, SessionId, TokenUsage};

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
        let cost = self.estimate_cost_for(core, &core.model, usage);
        let record = control::usage_record(
            session_id,
            run_id,
            request_id,
            &core.provider_id,
            &core.model,
            usage,
            cost.as_ref().map(|item| item.amount_micros).unwrap_or(0),
            cost.as_ref().map(|item| item.currency.as_str()).unwrap_or(""),
        );
        self.control
            .ledger
            .record(record)
            .await
            .map_err(|error| AppError::ControlPlane(error.to_string()))
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
        let runs = core
            .store()?
            .projection_snapshot(session_id)
            .await?
            .runs;
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use pawork_domain::{
        CancellationToken, CanonicalModelRequest, ModelDefinition, ModelId, ModelProvider,
        ModelResponseSummary, ProviderError, ProviderId, ProviderStreamEvent, ResolvedCredential,
        StopReason, TokenUsage,
    };
    use pawork_providers::ModelRegistry;

    use crate::testsupport::{
        RecordingEvents, core_with_registry, mock_core_with_usage, user_hello,
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
                _request: CanonicalModelRequest,
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
        core.chat_turn(&session, vec![user_hello()], &sink, CancellationToken::new())
            .await
            .expect("turn 1");
        core.chat_turn(&session, vec![user_hello()], &sink, CancellationToken::new())
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
