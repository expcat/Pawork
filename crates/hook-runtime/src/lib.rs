//! Pawork WASM 插件 lifecycle hook 派发器（P10-3）。
//!
//! 职责：
//! - 按 plugin id 确定性注册插件，重复 id 拒绝（冲突隔离）；
//! - 仅向 manifest 声明且具备 `LifecycleHook` capability 的事件派发；
//! - 启动/停止生命周期状态机，并在状态转换时派发 `Start` / `Stop` 事件；
//! - 单插件错误、取消与 panic 只落在该插件的 outcome 上，不中断其他插件，
//!   也不让 Core 崩溃；逐插件 outcome 可序列化、可持久化为 Core 事件。
//!
//! 本 crate 只做 lifecycle hook 派发；工具/命令注册与 WASM 宿主位于其他 crate，
//! 用户配置驱动的外部钩子（P17-1）与本 crate 互不调用。

use std::{
    collections::BTreeMap,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
    time::Instant,
};

use agent_domain::{CancellationToken, PluginId};
use plugin_api::{
    plugin_api_version, ManifestValidationError, Plugin, PluginCapability, PluginContext,
    PluginError, PluginErrorKind, PluginLifecycleEvent, PluginLifecycleEventKind, PluginManifest,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;

/// 判断插件是否订阅某事件：事件必须由 manifest 声明，且插件具备
/// `LifecycleHook` capability。注册时的 manifest 校验已强制二者配套，
/// 这里在派发边界再校验一次，防止变异后的 manifest 越权接收事件。
pub fn plugin_subscribes_to(manifest: &PluginManifest, event: PluginLifecycleEventKind) -> bool {
    manifest
        .capabilities
        .contains(&PluginCapability::LifecycleHook)
        && manifest.lifecycle_hooks.contains(&event)
}

/// hook 注册/派发期的确定性错误。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum HookRuntimeError {
    #[error("plugin manifest is invalid: {0}")]
    InvalidManifest(String),
    #[error(
        "plugin API requirement {plugin_requirement} does not include host version {host_version}"
    )]
    IncompatibleApi {
        plugin_requirement: String,
        host_version: String,
    },
    #[error("plugin {plugin_id} is already registered")]
    Conflict { plugin_id: PluginId },
    #[error("plugin {plugin_id} is not registered")]
    NotFound { plugin_id: PluginId },
    #[error("hook runtime has not been started")]
    NotStarted,
    #[error("hook runtime has already been started")]
    AlreadyStarted,
}

/// 单个插件针对单个事件的派发结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHookOutcome {
    pub plugin_id: PluginId,
    pub event: PluginLifecycleEventKind,
    /// 本次派发耗时毫秒（供审计，不参与确定性判定）。
    pub duration_ms: u64,
    pub status: PluginHookOutcomeStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PluginHookOutcomeStatus {
    Success,
    Error { error: PluginError },
    Cancelled { error: PluginError },
}

/// 一次事件派发的逐插件报告；可序列化为 Core 事件持久化。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookDispatchReport {
    pub event: PluginLifecycleEventKind,
    /// 派发级取消令牌触发时置为 true（未调用的订阅插件记为 Cancelled）。
    #[serde(default)]
    pub cancelled: bool,
    /// 实际被派发的订阅插件 outcome；未订阅插件不出现。
    #[serde(default)]
    pub outcomes: Vec<PluginHookOutcome>,
}

/// 进程内 WASM 插件 lifecycle hook 派发器。
///
/// 注册表由内部锁保护；派发按 plugin id 升序进行，与注册顺序无关。
/// 插件调用在独立 tokio task 中执行，panic、错误与取消都不会传播给调用方。
pub struct HookRuntime {
    inner: Arc<RwLock<Inner>>,
}

struct Inner {
    host_api_version: Version,
    started: bool,
    plugins: BTreeMap<PluginId, RegisteredPlugin>,
}

/// 注册时冻结 manifest，派发路径不再调用插件提供的同步 accessor。
/// 这既保证订阅判定稳定，也把异常 accessor 限制在可恢复的注册错误内。
#[derive(Clone)]
struct RegisteredPlugin {
    manifest: PluginManifest,
    plugin: Arc<dyn Plugin>,
}

impl HookRuntime {
    /// 使用当前宿主 API 版本创建运行时（初始为 stopped）。
    pub fn new() -> Self {
        Self::with_host_api_version(plugin_api_version())
    }

    /// 使用显式宿主 API 版本创建运行时（P10-6 兼容矩阵测试用）。
    pub fn with_host_api_version(host_api_version: Version) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                host_api_version,
                started: false,
                plugins: BTreeMap::new(),
            })),
        }
    }

    /// 注册插件：校验 manifest 与宿主 API 兼容性；同 id 冲突拒绝。
    pub async fn register(&self, plugin: Arc<dyn Plugin>) -> Result<(), HookRuntimeError> {
        let manifest =
            catch_unwind(AssertUnwindSafe(|| plugin.manifest().clone())).map_err(|_| {
                HookRuntimeError::InvalidManifest("plugin manifest accessor panicked".into())
            })?;
        manifest
            .validate()
            .map_err(|error| HookRuntimeError::InvalidManifest(error.to_string()))?;
        check_api_compatible(&manifest, &self.host_api_version().await)?;
        let plugin_id = manifest.id.clone();
        let mut inner = self.inner.write().await;
        if inner.plugins.contains_key(&plugin_id) {
            return Err(HookRuntimeError::Conflict { plugin_id });
        }
        inner
            .plugins
            .insert(plugin_id, RegisteredPlugin { manifest, plugin });
        Ok(())
    }

    /// 注销插件；未注册的 id 返回 NotFound。
    pub async fn unregister(&self, plugin_id: &PluginId) -> Result<(), HookRuntimeError> {
        let mut inner = self.inner.write().await;
        if inner.plugins.remove(plugin_id).is_none() {
            return Err(HookRuntimeError::NotFound {
                plugin_id: plugin_id.clone(),
            });
        }
        Ok(())
    }

    /// 当前宿主 API 版本。
    pub async fn host_api_version(&self) -> Version {
        self.inner.read().await.host_api_version.clone()
    }

    /// 是否已启动（Start 已派发、尚未 Stop）。
    pub async fn is_started(&self) -> bool {
        self.inner.read().await.started
    }

    /// 当前已注册插件 id，按 plugin id 升序（确定性）。
    pub async fn registered(&self) -> Vec<PluginId> {
        self.inner.read().await.plugins.keys().cloned().collect()
    }

    /// 启动生命周期：状态置为 started，并向订阅 `Start` 的插件派发。
    pub async fn start(
        &self,
        context: PluginContext,
    ) -> Result<HookDispatchReport, HookRuntimeError> {
        let plugins = self
            .transition(true, HookRuntimeError::AlreadyStarted)
            .await?;
        Ok(Self::dispatch_to(
            &plugins,
            &PluginLifecycleEvent::Start,
            &context,
            CancellationToken::new(),
        )
        .await)
    }

    /// 停止生命周期：状态置为 stopped，并向订阅 `Stop` 的插件派发。
    pub async fn stop(
        &self,
        context: PluginContext,
    ) -> Result<HookDispatchReport, HookRuntimeError> {
        let plugins = self.transition(false, HookRuntimeError::NotStarted).await?;
        Ok(Self::dispatch_to(
            &plugins,
            &PluginLifecycleEvent::Stop,
            &context,
            CancellationToken::new(),
        )
        .await)
    }

    /// 派发事件：仅向已启动运行时中 manifest 声明且具备 `LifecycleHook`
    /// capability 的插件派发。单个插件的错误/取消/panic 被隔离为对应 outcome；
    /// 整体取消令牌触发后，尚未派发的订阅插件标记为 Cancelled，不再调用。
    pub async fn dispatch(
        &self,
        event: PluginLifecycleEvent,
        context: PluginContext,
        cancel: CancellationToken,
    ) -> Result<HookDispatchReport, HookRuntimeError> {
        let plugins = {
            let inner = self.inner.read().await;
            if !inner.started {
                return Err(HookRuntimeError::NotStarted);
            }
            inner.plugins.values().cloned().collect::<Vec<_>>()
        };
        Ok(Self::dispatch_to(&plugins, &event, &context, cancel).await)
    }

    async fn transition(
        &self,
        target_started: bool,
        conflict_error: HookRuntimeError,
    ) -> Result<Vec<RegisteredPlugin>, HookRuntimeError> {
        let mut inner = self.inner.write().await;
        if inner.started == target_started {
            return Err(conflict_error);
        }
        inner.started = target_started;
        Ok(inner.plugins.values().cloned().collect())
    }

    async fn dispatch_to(
        plugins: &[RegisteredPlugin],
        event: &PluginLifecycleEvent,
        context: &PluginContext,
        cancel: CancellationToken,
    ) -> HookDispatchReport {
        let event_kind = event.kind();
        let mut outcomes = Vec::new();
        let mut cancelled = false;

        for registered in plugins {
            if !plugin_subscribes_to(&registered.manifest, event_kind) {
                continue;
            }
            let plugin_id = registered.manifest.id.clone();
            if cancel.is_cancelled() {
                cancelled = true;
                outcomes.push(PluginHookOutcome {
                    plugin_id,
                    event: event_kind,
                    duration_ms: 0,
                    status: PluginHookOutcomeStatus::Cancelled {
                        error: PluginError::cancelled(
                            "dispatch cancelled before plugin invocation",
                        ),
                    },
                });
                continue;
            }

            let started_at = Instant::now();
            let status = invoke_plugin(
                registered.plugin.clone(),
                event.clone(),
                context.clone(),
                cancel.clone(),
            )
            .await;
            outcomes.push(PluginHookOutcome {
                plugin_id,
                event: event_kind,
                duration_ms: started_at.elapsed().as_millis() as u64,
                status,
            });
        }

        HookDispatchReport {
            event: event_kind,
            cancelled: cancelled || cancel.is_cancelled(),
            outcomes,
        }
    }
}

impl Default for HookRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for HookRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HookRuntime")
            .finish_non_exhaustive()
    }
}

/// 在独立 task 中调用插件，把错误、取消与 panic 统一映射为 outcome。
async fn invoke_plugin(
    plugin: Arc<dyn Plugin>,
    event: PluginLifecycleEvent,
    context: PluginContext,
    cancel: CancellationToken,
) -> PluginHookOutcomeStatus {
    let task =
        tokio::task::spawn(async move { plugin.on_lifecycle_event(event, context, cancel).await });
    match task.await {
        Ok(Ok(())) => PluginHookOutcomeStatus::Success,
        Ok(Err(error)) if error.kind == PluginErrorKind::Cancelled => {
            PluginHookOutcomeStatus::Cancelled { error }
        }
        Ok(Err(error)) => PluginHookOutcomeStatus::Error { error },
        Err(_) => PluginHookOutcomeStatus::Error {
            error: PluginError::new(PluginErrorKind::Internal, "plugin hook task panicked"),
        },
    }
}

fn check_api_compatible(
    manifest: &PluginManifest,
    host_api_version: &Version,
) -> Result<(), HookRuntimeError> {
    manifest
        .ensure_api_compatible(host_api_version)
        .map_err(|error| match error {
            ManifestValidationError::IncompatibleApi {
                plugin_requirement,
                host_version,
            } => HookRuntimeError::IncompatibleApi {
                plugin_requirement,
                host_version,
            },
            other => HookRuntimeError::InvalidManifest(other.to_string()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_api::PluginPermissions;
    use semver::VersionReq;

    #[test]
    fn subscription_requires_declared_hook_and_lifecycle_hook_capability() {
        let manifest = PluginManifest {
            id: PluginId::from("p"),
            name: "p".into(),
            version: Version::new(1, 0, 0),
            api_version: VersionReq::parse("^1").expect("valid requirement"),
            description: None,
            permissions: PluginPermissions::default(),
            capabilities: vec![PluginCapability::LifecycleHook],
            tool_capabilities: Vec::new(),
            tools: Vec::new(),
            commands: Vec::new(),
            lifecycle_hooks: vec![PluginLifecycleEventKind::RunStart],
        };

        assert!(plugin_subscribes_to(
            &manifest,
            PluginLifecycleEventKind::RunStart
        ));
        assert!(!plugin_subscribes_to(
            &manifest,
            PluginLifecycleEventKind::Stop
        ));

        let mut no_capability = manifest.clone();
        no_capability.capabilities.clear();
        assert!(!plugin_subscribes_to(
            &no_capability,
            PluginLifecycleEventKind::RunStart
        ));

        let mut no_hook = manifest.clone();
        no_hook.lifecycle_hooks.clear();
        assert!(!plugin_subscribes_to(
            &no_hook,
            PluginLifecycleEventKind::RunStart
        ));
    }
}
