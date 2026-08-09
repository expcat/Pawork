//! Phase 10 插件子系统组合层。
//!
//! 把 component load/unload、工具/命令注册和 lifecycle hook 注册收敛到同一
//! mutation lock，避免正式组合层重复手工拼装并遗留部分注册。`pawork` / Core 的
//! 进程级装配仍由 Phase 13 负责。

use std::sync::Arc;

use agent_domain::{CancellationToken, PluginId};
use hook_runtime::{HookDispatchReport, HookRuntime, HookRuntimeError};
use plugin_api::{
    PluginContext, PluginError, PluginErrorKind, PluginLifecycleEvent, SignedPluginManifest,
};
use tokio::sync::{Mutex, RwLock};
use tool_runtime::ToolRegistry;

use crate::{
    ExternalPluginToolAdapter, ExternalToolCaller, LoadedPlugin, NamespacedToolRegistry,
    PluginCommandRegistry, WasmPluginHost,
};

/// WASM 插件的统一加载、注册、派发与卸载入口。
///
/// 注册表只能在 lifecycle runtime 停止时变更；这样 Start/Stop 事件与插件集合
/// 始终对应同一快照。热安装策略可在未来组合层显式扩展，不在 P10 中猜测语义。
pub struct PluginRuntime {
    host: Arc<WasmPluginHost>,
    hooks: HookRuntime,
    tools: RwLock<NamespacedToolRegistry>,
    commands: RwLock<PluginCommandRegistry>,
    mutation_lock: Mutex<()>,
}

impl PluginRuntime {
    /// 从空的 WASM host 构造协调层。已有裸 host 注册无法安全推导工具/命令/hook
    /// 状态，因此 fail closed，要求调用方从本入口统一加载。
    pub fn new(host: WasmPluginHost) -> Result<Self, PluginError> {
        if !host.loaded_plugins().is_empty() {
            return Err(PluginError::new(
                PluginErrorKind::Conflict,
                "plugin runtime requires an empty WASM host",
            ));
        }
        Ok(Self {
            host: Arc::new(host),
            hooks: HookRuntime::new(),
            tools: RwLock::new(NamespacedToolRegistry::new()),
            commands: RwLock::new(PluginCommandRegistry::new()),
            mutation_lock: Mutex::new(()),
        })
    }

    pub fn is_loaded(&self, plugin_id: &PluginId) -> bool {
        self.host.is_loaded(plugin_id)
    }

    pub fn loaded_plugins(&self) -> Vec<PluginId> {
        self.host.loaded_plugins()
    }

    /// 验签并加载组件，然后原子发布其工具、命令和 hook 注册。
    /// 任一步失败都会撤销之前的注册并卸载 component。
    pub async fn load(
        &self,
        signed: &SignedPluginManifest,
        component_bytes: &[u8],
    ) -> Result<Arc<LoadedPlugin>, PluginError> {
        let _mutation_guard = self.mutation_lock.lock().await;
        self.ensure_registry_mutable().await?;

        let loaded = self.host.load(signed, component_bytes).await?;
        let manifest = loaded.manifest().clone();
        let caller: Arc<dyn ExternalToolCaller> = self.host.clone();

        let tool_result = self.tools.write().await.register_external(
            &manifest.id,
            &manifest.tools,
            &|plugin_id, registration| {
                Arc::new(ExternalPluginToolAdapter::new(
                    plugin_id,
                    registration,
                    caller.clone(),
                ))
            },
        );
        if let Err(error) = tool_result {
            return Err(self.rollback_load(&manifest.id, error).await);
        }

        let command_result = self
            .commands
            .write()
            .await
            .register(&manifest.id, &manifest.commands);
        if let Err(error) = command_result {
            return Err(self.rollback_load(&manifest.id, error).await);
        }

        if let Err(error) = self.hooks.register(loaded.clone()).await {
            let error = hook_error_to_plugin(error);
            return Err(self.rollback_load(&manifest.id, error).await);
        }

        Ok(loaded)
    }

    /// 注销 hook、停用 component，并撤销该插件拥有的全部命令与工具。
    /// retained tool/LoadedPlugin handle 会由 host active gate 拒绝继续执行。
    pub async fn unload(&self, plugin_id: &PluginId) -> Result<(), PluginError> {
        let _mutation_guard = self.mutation_lock.lock().await;
        self.ensure_registry_mutable().await?;
        let loaded = self.host.get(plugin_id)?;

        self.hooks
            .unregister(plugin_id)
            .await
            .map_err(hook_error_to_plugin)?;
        if let Err(error) = self.host.unload(plugin_id).await {
            // host 卸载失败时恢复 hook 注册；工具/命令尚未撤销。
            let _ = self.hooks.register(loaded).await;
            return Err(error);
        }
        self.commands.write().await.unregister_plugin(plugin_id);
        self.tools.write().await.unregister_plugin(plugin_id);
        Ok(())
    }

    /// 启动并派发 Start；注册集合在本次转换期间不可变。
    pub async fn start(&self, context: PluginContext) -> Result<HookDispatchReport, PluginError> {
        let _mutation_guard = self.mutation_lock.lock().await;
        self.hooks
            .start(context)
            .await
            .map_err(hook_error_to_plugin)
    }

    /// 派发 Stop 后允许再次加载/卸载插件。
    pub async fn stop(&self, context: PluginContext) -> Result<HookDispatchReport, PluginError> {
        let _mutation_guard = self.mutation_lock.lock().await;
        self.hooks.stop(context).await.map_err(hook_error_to_plugin)
    }

    pub async fn dispatch(
        &self,
        event: PluginLifecycleEvent,
        context: PluginContext,
        cancel: CancellationToken,
    ) -> Result<HookDispatchReport, PluginError> {
        let _mutation_guard = self.mutation_lock.lock().await;
        self.hooks
            .dispatch(event, context, cancel)
            .await
            .map_err(hook_error_to_plugin)
    }

    /// 当前插件工具的 canonical registry 快照。组合层在插件集合变更后据此重建
    /// ToolScheduler；旧快照中的 adapter 会在卸载后由 host active gate 拒绝。
    pub async fn tool_registry(&self) -> ToolRegistry {
        self.tools.read().await.to_tool_registry()
    }

    pub async fn command_names(&self) -> Vec<String> {
        self.commands.read().await.names()
    }

    pub async fn invoke_command(
        &self,
        name: &str,
        input: serde_json::Value,
        context: PluginContext,
        cancel: CancellationToken,
    ) -> Result<serde_json::Value, PluginError> {
        self.commands
            .read()
            .await
            .invoke(name, input, context, self.host.as_ref(), cancel)
            .await
    }

    async fn ensure_registry_mutable(&self) -> Result<(), PluginError> {
        if self.hooks.is_started().await {
            Err(PluginError::new(
                PluginErrorKind::Conflict,
                "plugin registry cannot change while lifecycle runtime is started",
            ))
        } else {
            Ok(())
        }
    }

    async fn rollback_load(&self, plugin_id: &PluginId, mut error: PluginError) -> PluginError {
        self.commands.write().await.unregister_plugin(plugin_id);
        self.tools.write().await.unregister_plugin(plugin_id);
        if let Err(rollback) = self.host.unload(plugin_id).await {
            error.message = format!("{}; component rollback failed: {rollback}", error.message);
        }
        error
    }
}

impl std::fmt::Debug for PluginRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginRuntime")
            .field("loaded_plugins", &self.loaded_plugins())
            .finish_non_exhaustive()
    }
}

fn hook_error_to_plugin(error: HookRuntimeError) -> PluginError {
    let kind = match error {
        HookRuntimeError::InvalidManifest(_) => PluginErrorKind::InvalidManifest,
        HookRuntimeError::IncompatibleApi { .. } => PluginErrorKind::IncompatibleApi,
        HookRuntimeError::Conflict { .. }
        | HookRuntimeError::AlreadyStarted
        | HookRuntimeError::NotStarted => PluginErrorKind::Conflict,
        HookRuntimeError::NotFound { .. } => PluginErrorKind::NotLoaded,
    };
    PluginError::new(kind, error.to_string())
}
