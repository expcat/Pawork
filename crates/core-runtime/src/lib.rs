//! CoreRuntime（P13-2）：完整 Core 的进程内装配。
//!
//! 装配 [`AppService`]（含 [`CommandRouter`]）与 [`EventHub`]，并运行
//! `EventPump` 后台任务：以固定间隔（默认 10ms）轮询
//! `router.drain_events()` / `supervisor.drain_events()`（Run 监督器的限流合并
//! 输出），发布到 Event Hub——CLI 渲染器与未来 GUI 订阅到同一份全局连续序列
//! 的事件流（连续性由 Hub 的强制重写保证）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use app_service::AppService;
use app_service::UserHookHost;
use provider_api::ModelProvider;
use subscription_hub::{EventHub, DEFAULT_HUB_CAPACITY};
use tokio::sync::watch;
use tracing::debug;

/// CoreRuntime 配置。
#[derive(Clone, Debug)]
pub struct CoreRuntimeConfig {
    /// Core 实例名（默认 `default`；命名实例拥有独立 Endpoint 与状态）。
    pub instance: String,
    /// EventPump 轮询间隔（默认 10ms）。
    pub pump_interval: Duration,
    /// Event Hub ring buffer / 广播容量（默认 4096）。
    pub hub_capacity: usize,
    /// Team canonical event store。`None` 仅用于测试/嵌入；正式 `pawork`
    /// 必须传入实例目录中的持久路径。
    pub team_db_path: Option<PathBuf>,
    /// P17-1 User Hooks 宿主（正式宿主装配后注入；`None` 时 run 不回调 hooks）。
    pub user_hooks: Option<Arc<UserHookHost>>,
}

impl Default for CoreRuntimeConfig {
    fn default() -> Self {
        Self {
            instance: "default".into(),
            pump_interval: Duration::from_millis(10),
            hub_capacity: DEFAULT_HUB_CAPACITY,
            team_db_path: None,
            user_hooks: None,
        }
    }
}

/// 完整 Core 运行时：AppService + EventHub + EventPump。
///
/// [`CoreRuntime::shutdown`] 停止 EventPump（幂等）；Run 任务本身由
/// `RunCancel` / 终态自行收敛，不随 pump 终止而取消（[ADR-026]）。
///
/// [ADR-026]: ../../docs/adr/ADR-026-gui-disconnect-safe.md
pub struct CoreRuntime {
    service: Arc<AppService>,
    hub: Arc<EventHub>,
    pump: tokio::task::JoinHandle<()>,
    shutdown: watch::Sender<bool>,
}

impl CoreRuntime {
    /// 以默认配置装配（实例名 + 10ms pump + 4096 Hub 容量 + 生产 Quota
    /// 运行时）。
    pub fn new(instance: impl Into<String>) -> Self {
        Self::with_config(CoreRuntimeConfig {
            instance: instance.into(),
            ..CoreRuntimeConfig::default()
        })
    }

    /// 以指定配置装配。默认携带生产 Quota 运行时（共享
    /// [`app_service::QuotaRuntime`]：内存账本 + 系统时钟，唯一本地 ledger
    /// 适配器，构造与空查询不触发网络）；`from_parts` 注入的既有
    /// `AppService` 原样保留，不覆盖其 Quota 注入状态。
    pub fn with_config(config: CoreRuntimeConfig) -> Self {
        let service = match config.team_db_path.as_ref() {
            Some(path) => AppService::with_runtime_components(
                config.instance.clone(),
                None,
                app_service::QuotaRuntime::production(),
                path,
            )
            .expect("configured durable Team store must open and replay"),
            None => AppService::with_quota_runtime(
                config.instance.clone(),
                None,
                app_service::QuotaRuntime::production(),
            ),
        };
        let service = Arc::new(service);
        if let Some(user_hooks) = config.user_hooks.as_ref() {
            service.set_user_hooks(Arc::clone(user_hooks));
        }
        Self::from_parts(service, config)
    }

    /// 正式宿主使用的可失败装配。持久 Team store 无法打开或重放时返回错误，
    /// 调用方不得降级到内存空状态。
    pub fn try_with_config(config: CoreRuntimeConfig) -> Result<Self, app_service::TeamError> {
        let service = match config.team_db_path.as_ref() {
            Some(path) => AppService::with_runtime_components(
                config.instance.clone(),
                None,
                app_service::QuotaRuntime::production(),
                path,
            )?,
            None => AppService::with_quota_runtime(
                config.instance.clone(),
                None,
                app_service::QuotaRuntime::production(),
            ),
        };
        let service = Arc::new(service);
        if let Some(user_hooks) = config.user_hooks.as_ref() {
            service.set_user_hooks(Arc::clone(user_hooks));
        }
        Ok(Self::from_parts(service, config))
    }

    /// 注入 builder：以既有 AppService 装配（测试 / 嵌入场景复用，保持
    /// `AppService` 现有 API 与测试不变）。
    pub fn from_parts(service: Arc<AppService>, config: CoreRuntimeConfig) -> Self {
        let hub = Arc::new(EventHub::with_capacity(config.hub_capacity));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let pump = spawn_event_pump(
            Arc::clone(&service),
            Arc::clone(&hub),
            config.pump_interval,
            shutdown_rx,
        );
        Self {
            service,
            hub,
            pump,
            shutdown: shutdown_tx,
        }
    }

    pub fn service(&self) -> &Arc<AppService> {
        &self.service
    }

    pub fn hub(&self) -> &Arc<EventHub> {
        &self.hub
    }

    /// Provider 注册透传（正式宿主后续由 provider-runtime / auth-service 注入）。
    pub fn register_provider(&self, provider: Arc<dyn ModelProvider>) -> agent_domain::ProviderId {
        self.service.register_provider(provider)
    }

    /// 停止 EventPump（幂等）。
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    /// pump 任务是否已结束（shutdown 后为 true；测试用）。
    pub fn pump_finished(&self) -> bool {
        self.pump.is_finished()
    }
}

/// EventPump 任务：固定间隔轮询 app-service 的事件队列并发布到 Hub。
fn spawn_event_pump(
    service: Arc<AppService>,
    hub: Arc<EventHub>,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = ticker.tick() => {}
            }
            let drained = service.drain_events();
            if !drained.is_empty() {
                debug!(
                    count = drained.len(),
                    "event pump publishing drained events"
                );
            }
            for event in drained {
                hub.publish(event);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{CommandId, CoreInstanceId, SessionId, Timestamp, WorkspaceId};
    use core_api::{
        ActorIdentity, AppCommand, AppCommandEnvelope, AppResponse, CommandSource, RunState,
        API_VERSION,
    };
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use subscription_hub::HubError;
    use test_support::{MockProvider, MockScript};

    static NEXT_COMMAND_ID: AtomicU64 = AtomicU64::new(0);

    fn now_timestamp() -> Timestamp {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        Timestamp::from_unix_millis(millis)
    }

    fn command(instance: &CoreInstanceId, command: AppCommand) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from(format!(
                "{}-{}",
                instance,
                NEXT_COMMAND_ID.fetch_add(1, AtomicOrdering::SeqCst) + 1
            )),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: "core-runtime-test".into(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: now_timestamp(),
            command,
        }
    }

    fn is_terminal(state: &RunState) -> bool {
        matches!(
            state,
            RunState::Completed | RunState::Cancelled | RunState::Failed | RunState::Interrupted
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pump_publishes_run_events_with_contiguous_global_sequence() {
        let runtime = CoreRuntime::new("pump-test");
        runtime.register_provider(Arc::new(MockProvider::new(
            MockScript::new().text("hello from mock").complete(),
        )));
        let service = runtime.service();
        let instance = CoreInstanceId::from("pump-test");

        // 订阅必须在 RunStart 之前建立，避免错过事件。
        let mut subscription = runtime.hub().subscribe();

        // 打开默认 workspace（SessionCreate 要求 workspace 已登记）。
        let workspace_add = service.dispatch_envelope(command(
            &instance,
            AppCommand::WorkspaceAdd {
                root_path: std::env::current_dir()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| ".".into()),
            },
        ));
        let workspace_id = match workspace_add.response {
            AppResponse::Data(value) => WorkspaceId::from(
                value
                    .get("id")
                    .and_then(|id| id.as_str())
                    .expect("workspace id"),
            ),
            other => panic!("workspace add failed: {other:?}"),
        };
        let session_response = service.dispatch_envelope(command(
            &instance,
            AppCommand::SessionCreate {
                workspace_id: workspace_id.clone(),
                title: Some("core-runtime pump test".into()),
            },
        ));
        let session_id = match session_response.response {
            AppResponse::Data(value) => SessionId::from(
                value
                    .get("session_id")
                    .and_then(|id| id.as_str())
                    .expect("session id"),
            ),
            other => panic!("session create failed: {other:?}"),
        };

        let run_response = service.dispatch_envelope(command(
            &instance,
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: "run a mock task".into(),
                model: None,
                profile: None,
            },
        ));
        assert!(
            matches!(run_response.response, AppResponse::Accepted { .. }),
            "run start failed: {:?}",
            run_response.response
        );

        // 等待 RunChanged 终态，期间校验全局序列连续。
        let mut events: Vec<core_api::AppEventEnvelope> = Vec::new();
        let mut observed_run_id = None;
        let mut saw_terminal = false;
        for _ in 0..10_000 {
            match tokio::time::timeout(Duration::from_secs(5), subscription.recv()).await {
                Ok(Ok(event)) => {
                    if let Some(previous) = events.last() {
                        assert!(
                            event
                                .global_sequence
                                .is_immediately_after(previous.global_sequence),
                            "hub global sequence must stay contiguous"
                        );
                    }
                    if let core_api::AppEvent::RunChanged { run_id, state, .. } = &event.payload {
                        observed_run_id.get_or_insert_with(|| run_id.clone());
                        if is_terminal(state) {
                            saw_terminal = true;
                            break;
                        }
                    }
                    events.push(event);
                }
                Ok(Err(
                    HubError::Lagged { .. } | HubError::Empty | HubError::ReplayUnavailable { .. },
                )) => continue,
                Ok(Err(HubError::Closed)) | Err(_) => break,
            }
        }
        assert!(saw_terminal, "run never reached a terminal state");
        assert!(
            observed_run_id.is_some(),
            "run id must be observable from the event stream"
        );

        // Hub replay 与订阅一致：全局序列从 1 连续到 current。
        let current = runtime.hub().current();
        let replayed = runtime
            .hub()
            .replay(core_api::GlobalSequence(1), Some(current))
            .expect("replay");
        assert_eq!(replayed.len(), current.0 as usize);
        for pair in replayed.windows(2) {
            assert!(
                pair[1]
                    .global_sequence
                    .is_immediately_after(pair[0].global_sequence),
                "hub global sequence must be contiguous in replay"
            );
        }

        // run 已终态：supervisor 无活跃任务。
        assert_eq!(service.router().supervisor().stats().active, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_stops_the_event_pump() {
        let runtime = CoreRuntime::new("shutdown-test");
        assert!(!runtime.pump_finished());
        runtime.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(runtime.pump_finished());
    }

    #[tokio::test]
    async fn from_parts_keeps_app_service_api_unchanged() {
        let service = Arc::new(AppService::new("parts-test"));
        let runtime = CoreRuntime::from_parts(
            Arc::clone(&service),
            CoreRuntimeConfig {
                instance: "parts-test".into(),
                ..CoreRuntimeConfig::default()
            },
        );
        assert!(Arc::ptr_eq(runtime.service(), &service));
        assert_eq!(runtime.hub().capacity(), DEFAULT_HUB_CAPACITY);
        assert!(
            runtime.service().quota_runtime().is_none(),
            "from_parts must keep the injected AppService exactly as given"
        );
    }

    #[tokio::test]
    async fn with_config_defaults_to_production_quota_runtime() {
        let runtime = CoreRuntime::new("quota-wiring-test");
        assert!(
            runtime.service().quota_runtime().is_some(),
            "default CoreRuntime must carry a production QuotaRuntime"
        );
        runtime.shutdown();
    }

    // —— P17-1 回归：CoreRuntimeConfig.user_hooks 注入可达 ——

    struct NoProviders;
    impl app_service::ProviderResolver for NoProviders {
        fn resolve(
            &self,
            _id: &agent_domain::ProviderId,
        ) -> Option<Arc<dyn provider_api::ModelProvider>> {
            None
        }
    }

    #[derive(Clone)]
    struct DefaultProfiles(app_service::EvalProfile);
    impl app_service::EvalProfileResolver for DefaultProfiles {
        fn resolve(
            &self,
            _workspace_id: Option<&agent_domain::WorkspaceId>,
            profile: &str,
        ) -> Option<app_service::EvalProfile> {
            if profile.is_empty() || profile == "default" {
                Some(self.0.clone())
            } else {
                None
            }
        }
    }

    #[derive(Default)]
    struct MemSecret(std::sync::Mutex<std::collections::HashMap<String, String>>);
    impl auth_service::SecretBackend for MemSecret {
        fn store(
            &self,
            service: &str,
            account: &str,
            secret: &str,
        ) -> Result<(), auth_service::AuthError> {
            self.0
                .lock()
                .expect("mem secret")
                .insert(format!("{service}/{account}"), secret.to_string());
            Ok(())
        }
        fn get(&self, service: &str, account: &str) -> Result<String, auth_service::AuthError> {
            self.0
                .lock()
                .expect("mem secret")
                .get(&format!("{service}/{account}"))
                .cloned()
                .ok_or(auth_service::AuthError::NotFound)
        }
        fn delete(&self, service: &str, account: &str) -> Result<(), auth_service::AuthError> {
            let mut map = self.0.lock().expect("mem secret");
            if map.remove(&format!("{service}/{account}")).is_some() {
                Ok(())
            } else {
                Err(auth_service::AuthError::NotFound)
            }
        }
    }

    #[tokio::test]
    async fn user_hooks_config_injection_reaches_service() {
        // 默认配置不注入 hooks。
        let plain = CoreRuntime::new("hooks-none-test");
        assert!(
            !plain.service().user_hooks_active(),
            "default config must not install user hooks"
        );
        plain.shutdown();

        // with_config 注入的宿主必须到达 AppService（run loop 权威位点可调用）。
        let default_eval = app_service::EvalProfile {
            provider_id: agent_domain::ProviderId::from("default"),
            model: agent_domain::ModelId::from("default"),
            system_prompt: None,
            reasoning_effort: None,
            budget: None,
            tool_rules: agent_domain::ProfileToolRules::default(),
            isolation: agent_domain::ProfileIsolation::None,
        };
        let host = app_service::UserHookHost::new(app_service::UserHookHostOptions::new(
            Vec::new(),
            Arc::new(NoProviders),
            default_eval.clone(),
            Arc::new(DefaultProfiles(default_eval)),
            Arc::new(MemSecret::default()),
        ))
        .expect("host must construct");
        let runtime = CoreRuntime::with_config(CoreRuntimeConfig {
            user_hooks: Some(Arc::new(host)),
            ..CoreRuntimeConfig::default()
        });
        assert!(
            runtime.service().user_hooks_active(),
            "CoreRuntimeConfig.user_hooks must reach the AppService"
        );
        runtime.shutdown();
    }
}
