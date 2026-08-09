//! WASM Component Model 插件宿主（P10-2 / P10-5）。
//!
//! 基于 wasmtime 27 的 **async** Component Model 运行时，固定顶层 ABI
//! `invoke(string) -> string`（JSON），实现：
//! - **加载/卸载**：`load` 验签 + 编译 + 实例化；`unload` 丢弃 Store。
//! - **崩溃隔离**：每个插件拥有独立 `Store`；任一插件 trap 仅返回
//!   [`PluginError`]，绝不影响其它插件或宿主进程（wasmtime trap 永不跨 Store）。
//! - **fuel / 内存 / 超时 / 取消**：单次 invoke 重新注入 fuel；`StoreLimits`
//!   限制内存/实例/表；epoch ticker + `epoch_deadline_async_yield_and_update`
//!   实现协作式 wall-clock 超时；`CancellationToken` 支持即时取消。
//! - **默认无 WASI/文件/网络/进程**：`Linker` 不注入任何 import，组件对宿主
//!   OS 资源零访问（ADR-012 / P10-5）。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_domain::{CancellationToken, PluginId};
use parking_lot::Mutex;
use plugin_api::{
    plugin_api_version, ManifestValidationError, Plugin, PluginCapability, PluginContext,
    PluginError, PluginErrorKind, PluginInvocation, PluginInvocationOutput, PluginLifecycleEvent,
    PluginOperation, PLUGIN_INVOKE_EXPORT,
};
use tokio::task::JoinHandle;
use wasmtime::component::{Component, Instance, Linker, TypedFunc};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap};

use crate::config::HostConfig;
use crate::registry::{ExternalCommandCaller, ExternalToolCaller};
use crate::state::{PluginStateError, PluginStateStore};
use crate::trust::{signature_error_to_plugin, TrustStore};

/// 每个 Store 的 host 侧资源限额。
pub(crate) struct PluginStoreData {
    limits: StoreLimits,
}

impl PluginStoreData {
    fn new(config: &HostConfig) -> Self {
        let limits = StoreLimitsBuilder::new()
            .memory_size(config.max_memory_bytes)
            .instances(config.max_instances)
            .tables(config.max_tables)
            .memories(config.max_memories)
            .table_elements(config.max_table_elements)
            // 内存/表增长越限时直接 trap（而非静默返回 -1），便于确定性检测。
            .trap_on_grow_failure(true)
            .build();
        Self { limits }
    }
}

/// 已加载的插件：独立 `Store` + 实例 + 类型化 `invoke`。
///
/// `inner` 用 `tokio::sync::Mutex` 保护，保证同一插件的 invoke 串行
/// （Store 不支持并发 wasm 调用）；不同插件各自独立、可并发。
pub struct LoadedPlugin {
    manifest: std::sync::Arc<plugin_api::PluginManifest>,
    state: std::sync::Arc<dyn PluginStateStore>,
    config: HostConfig,
    active: AtomicBool,
    /// 串行化 state snapshot → component invoke → mutation apply 整个事务。
    operation_lock: tokio::sync::Mutex<()>,
    inner: tokio::sync::Mutex<Option<LoadedPluginInner>>,
}

impl std::fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedPlugin").finish_non_exhaustive()
    }
}

struct LoadedPluginInner {
    store: Store<PluginStoreData>,
    _instance: Instance,
    invoke: TypedFunc<(String,), (String,)>,
}

impl LoadedPlugin {
    /// 插件 manifest（含 tools/commands/lifecycle_hooks/capabilities 声明）。
    pub fn manifest(&self) -> &plugin_api::PluginManifest {
        &self.manifest
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    async fn deactivate(&self) {
        // 先阻止新调用进入，再等待完整 operation 事务（snapshot → invoke → apply）
        // 结束。仅等待 `inner` 不足以覆盖 invoke 返回后、state apply 前的窗口，
        // 会让 unload 提前返回并允许旧实例在同 id 重载后继续写状态。
        self.active.store(false, Ordering::Release);
        let _operation_guard = self.operation_lock.lock().await;
        self.inner.lock().await.take();
    }

    fn ensure_active(&self) -> Result<(), PluginError> {
        if self.is_active() {
            Ok(())
        } else {
            Err(PluginError::new(
                PluginErrorKind::NotLoaded,
                format!("plugin not loaded: {}", self.manifest.id),
            ))
        }
    }

    async fn invoke_checked(
        &self,
        input: String,
        cancel: CancellationToken,
    ) -> Result<String, PluginError> {
        if input.len() > self.config.max_input_bytes {
            return Err(PluginError::new(
                PluginErrorKind::InvalidInvocation,
                format!(
                    "plugin input too large: {} > {}",
                    input.len(),
                    self.config.max_input_bytes
                ),
            ));
        }
        let output = self.invoke(input, &self.config, cancel).await?;
        if output.len() > self.config.max_output_bytes {
            return Err(PluginError::new(
                PluginErrorKind::InvalidInvocation,
                format!(
                    "plugin output too large: {} > {}",
                    output.len(),
                    self.config.max_output_bytes
                ),
            ));
        }
        Ok(output)
    }

    /// 统一 invoke 事务：operation_lock → ensure_active → 状态快照 →
    /// 序列化 → invoke_checked（已含 input 长度检查）→ 解析 → 原子应用状态变更。
    ///
    /// 这是 `on_lifecycle_event` 与 `invoke_operation` 两条路径的共同实现；
    /// 调用方按各自契约先做 no-op 判断（路径 A）或 capability/registration
    /// 校验（路径 B）。`operation_lock` 只在这里取一次（tokio Mutex 不可重入），
    /// `invoke_checked` 只取 `inner` 锁，无重入问题。
    async fn invoke_with_state(
        &self,
        operation: PluginOperation,
        scope: plugin_api::PluginStateScope,
        cancel: CancellationToken,
    ) -> Result<PluginInvocationOutput, PluginError> {
        let _operation_guard = self.operation_lock.lock().await;
        self.ensure_active()?;
        // PersistentState 闸门：未声明该能力的插件读取/写入状态被拒。
        // 这里仍允许调用（组件可能用 state snapshot 做无副作用只读决策），
        // 但 apply 阶段会再次校验；快照本身是只读复制，不构成越权。
        let snapshot = state_snapshot(
            &self.manifest,
            self.state.as_ref(),
            &self.manifest.id,
            &scope,
        )?;
        let invocation = PluginInvocation {
            api_version: plugin_api_version(),
            plugin_id: self.manifest.id.clone(),
            operation,
            state: snapshot.clone(),
        };
        let input = serde_json::to_string(&invocation)
            .map_err(|err| PluginError::new(PluginErrorKind::Internal, err.to_string()))?;
        let output_str = self.invoke_checked(input, cancel).await?;
        let output: PluginInvocationOutput = serde_json::from_str(&output_str).map_err(|err| {
            PluginError::new(
                PluginErrorKind::InvalidInvocation,
                format!("plugin returned invalid invocation output JSON: {err}"),
            )
        })?;
        if let PluginInvocationOutput::Success(response) = &output {
            apply_state_mutations(
                &self.manifest,
                self.state.as_ref(),
                &scope,
                &snapshot,
                &response.state_mutations,
                &self.config,
            )?;
        }
        Ok(output)
    }

    /// 执行一次 `invoke(input) -> output`，应用 fuel/内存/超时/取消。
    pub(crate) async fn invoke(
        &self,
        input: String,
        config: &HostConfig,
        cancel: CancellationToken,
    ) -> Result<String, PluginError> {
        self.ensure_active()?;
        if cancel.is_cancelled() {
            return Err(PluginError::cancelled(
                "plugin invocation cancelled before start",
            ));
        }

        let mut inner_guard = self.inner.lock().await;
        self.ensure_active()?;
        let inner = inner_guard.as_mut().ok_or_else(|| {
            PluginError::new(
                PluginErrorKind::NotLoaded,
                format!("plugin not loaded: {}", self.manifest.id),
            )
        })?;
        inner
            .store
            .set_fuel(config.fuel)
            .map_err(|err| PluginError::new(PluginErrorKind::Internal, err.to_string()))?;

        // TypedFunc 是 Copy；先取出引用，再 await（避免借用跨越 await 点）。
        let invoke = inner.invoke;
        let call = invoke.call_async(&mut inner.store, (input,));

        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => InvokeOutcome::Cancelled,
            _ = tokio::time::sleep(config.invoke_timeout) => InvokeOutcome::Timeout,
            result = call => InvokeOutcome::Done(result),
        };

        match outcome {
            InvokeOutcome::Cancelled => Err(PluginError::cancelled("plugin invocation cancelled")),
            InvokeOutcome::Timeout => Err(PluginError::new(
                PluginErrorKind::Timeout,
                format!("plugin invocation exceeded {:?}", config.invoke_timeout),
            )),
            InvokeOutcome::Done(result) => match result {
                Ok(output) => {
                    if let Err(err) = invoke.post_return_async(&mut inner.store).await {
                        return Err(map_wasm_error(err));
                    }
                    Ok(output.0)
                }
                Err(err) => Err(map_wasm_error(err)),
            },
        }
    }
}

/// `LoadedPlugin` 实现 `plugin_api::Plugin`：让 hook-runtime 可把任何已加载的
/// WASM 插件当作统一 `Plugin` 句柄派发生命周期事件。
///
/// Lifecycle 事件被编码为 `PluginOperation::Lifecycle`，经固定 ABI 转发组件；
/// 组件返回的 state 变更按事件自带 `PluginContext.state_scope()` 落地，且仅当
/// manifest 声明 `PersistentState` 时才接受。未声明该事件 hook 的请求 no-op 成功。
#[async_trait::async_trait]
impl Plugin for LoadedPlugin {
    fn manifest(&self) -> &plugin_api::PluginManifest {
        &self.manifest
    }

    async fn on_lifecycle_event(
        &self,
        event: PluginLifecycleEvent,
        context: PluginContext,
        cancel: CancellationToken,
    ) -> Result<(), PluginError> {
        self.ensure_active()?;
        if !self.manifest.lifecycle_hooks.contains(&event.kind()) {
            // 插件未声明该事件 hook：no-op 成功返回（不是错误）。
            return Ok(());
        }
        // 先取 event.kind() 与 state_scope 再 move 进 operation。
        let scope = context.state_scope();
        let operation = PluginOperation::Lifecycle { event, context };
        let output = self.invoke_with_state(operation, scope, cancel).await?;
        match output {
            PluginInvocationOutput::Success(_) => Ok(()),
            PluginInvocationOutput::Error { error } => Err(error),
        }
    }
}

enum InvokeOutcome {
    Cancelled,
    Timeout,
    Done(anyhow::Result<(String,)>),
}

/// WASM Component Model 插件宿主。
pub struct WasmPluginHost {
    engine: Engine,
    config: HostConfig,
    trust: TrustStore,
    state: std::sync::Arc<dyn PluginStateStore>,
    plugins: Mutex<BTreeMap<PluginId, std::sync::Arc<LoadedPlugin>>>,
    /// load 全路径串行锁：消除 `contains → compile → insert` 竞态。
    /// load 是低频操作，全局串行不影响吞吐；invoke 仍按插件各自并发。
    load_lock: tokio::sync::Mutex<()>,
    ticker: Mutex<Option<JoinHandle<()>>>,
}

impl WasmPluginHost {
    /// 构造宿主。`trust` 与 `state` 由调用方注入，便于测试与未来替换为
    /// 持久化实现。
    pub fn new(
        config: HostConfig,
        trust: TrustStore,
        state: std::sync::Arc<dyn PluginStateStore>,
    ) -> Result<Self, PluginError> {
        config.validate().map_err(|error| {
            PluginError::new(
                PluginErrorKind::Internal,
                format!("invalid WASM plugin host config: {error}"),
            )
        })?;
        let mut wasm_config = Config::new();
        wasm_config.async_support(true);
        wasm_config.epoch_interruption(true);
        wasm_config.consume_fuel(true);
        wasm_config.wasm_backtrace(false);
        let engine = Engine::new(&wasm_config)
            .map_err(|err| PluginError::new(PluginErrorKind::Internal, err.to_string()))?;
        Ok(Self {
            engine,
            config,
            trust,
            state,
            plugins: Mutex::new(BTreeMap::new()),
            load_lock: tokio::sync::Mutex::new(()),
            ticker: Mutex::new(None),
        })
    }

    /// 当前 host API 版本（与 plugin-api 一致）。
    pub fn api_version(&self) -> semver::Version {
        plugin_api_version()
    }

    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    pub fn trust_store(&self) -> &TrustStore {
        &self.trust
    }

    pub fn state_store(&self) -> &std::sync::Arc<dyn PluginStateStore> {
        &self.state
    }

    pub fn is_loaded(&self, plugin_id: &PluginId) -> bool {
        self.plugins.lock().contains_key(plugin_id)
    }

    /// 已加载插件 ID 列表（稳定排序）。
    pub fn loaded_plugins(&self) -> Vec<PluginId> {
        self.plugins.lock().keys().cloned().collect()
    }

    /// 加载并验证一个签名插件组件。
    ///
    /// 顺序：组件字节上限 → API 版本兼容 → manifest 校验 → Ed25519 验签
    /// （绑定 manifest + 组件字节）→ 编译 → 实例化（独立 Store）→ 取出
    /// `invoke: (string) -> string` 导出。
    pub async fn load(
        &self,
        signed: &plugin_api::SignedPluginManifest,
        component_bytes: &[u8],
    ) -> Result<std::sync::Arc<LoadedPlugin>, PluginError> {
        if component_bytes.len() > self.config.max_component_bytes {
            return Err(PluginError::new(
                PluginErrorKind::InvalidManifest,
                format!(
                    "component too large: {} > {}",
                    component_bytes.len(),
                    self.config.max_component_bytes
                ),
            ));
        }

        let manifest = &signed.manifest;
        manifest
            .ensure_api_compatible(&plugin_api_version())
            .map_err(manifest_error_to_plugin)?;
        manifest.validate().map_err(manifest_error_to_plugin)?;

        self.trust
            .verify_signature(&signed.signature, manifest, component_bytes)
            .map_err(signature_error_to_plugin)?;

        // 持锁完成「检测重复 → 编译 → 实例化 → 登记」全路径，
        // 杜绝同 id 并发 load 造成的双重编译/登记竞态。
        let _load_guard = self.load_lock.lock().await;
        if self.plugins.lock().contains_key(&manifest.id) {
            return Err(PluginError::new(
                PluginErrorKind::Conflict,
                format!("plugin already loaded: {}", manifest.id),
            ));
        }
        self.ensure_ticker();

        let component = Component::new(&self.engine, component_bytes).map_err(|err| {
            PluginError::new(
                PluginErrorKind::InvalidManifest,
                format!("failed to compile component: {err}"),
            )
        })?;

        let mut store = Store::new(&self.engine, PluginStoreData::new(&self.config));
        // 内存/实例/表限额。
        store.limiter(|data| &mut data.limits);
        // 实例化前注入 fuel：组件可能在 start function 中无限循环，
        // 没有 fuel 的话 instantiate_async 自身就会失控。
        store
            .set_fuel(self.config.fuel)
            .map_err(|err| PluginError::new(PluginErrorKind::Internal, err.to_string()))?;
        // epoch 协作式中断：deadline 到达时 yield，resume 后 deadline = current + 1，
        // 由后台 ticker 驱动 increment_epoch，配合 tokio timeout 实现可取消的超时。
        store.set_epoch_deadline(1);
        store.epoch_deadline_async_yield_and_update(1);

        // 不注入任何 WASI/host import：组件对 OS 文件/网络/进程零访问。
        let linker: Linker<PluginStoreData> = Linker::new(&self.engine);
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .map_err(map_wasm_error)?;

        let invoke = instance
            .get_typed_func::<(String,), (String,)>(&mut store, PLUGIN_INVOKE_EXPORT)
            .map_err(|err| {
                PluginError::new(
                    PluginErrorKind::InvalidManifest,
                    format!(
                        "component must export `invoke(string) -> string` ({PLUGIN_INVOKE_EXPORT}): {err}"
                    ),
                )
            })?;

        let loaded = std::sync::Arc::new(LoadedPlugin {
            manifest: std::sync::Arc::new(manifest.clone()),
            state: self.state.clone(),
            config: self.config.clone(),
            active: AtomicBool::new(true),
            operation_lock: tokio::sync::Mutex::new(()),
            inner: tokio::sync::Mutex::new(Some(LoadedPluginInner {
                store,
                _instance: instance,
                invoke,
            })),
        });
        self.plugins
            .lock()
            .insert(manifest.id.clone(), loaded.clone());
        Ok(loaded)
    }

    /// 卸载插件：从注册表移除，丢弃 Store（Arc 引用归零时释放）。
    pub async fn unload(&self, plugin_id: &PluginId) -> Result<(), PluginError> {
        // 与 load 串行：同 id 的新实例不能在旧实例完整 operation 事务结束前注册。
        let _load_guard = self.load_lock.lock().await;
        let loaded = self.plugins.lock().remove(plugin_id).ok_or_else(|| {
            PluginError::new(
                PluginErrorKind::NotLoaded,
                format!("plugin not loaded: {plugin_id}"),
            )
        })?;
        loaded.deactivate().await;
        Ok(())
    }

    /// 取已加载插件句柄（生命周期随 host）。
    pub fn get(&self, plugin_id: &PluginId) -> Result<std::sync::Arc<LoadedPlugin>, PluginError> {
        self.plugins.lock().get(plugin_id).cloned().ok_or_else(|| {
            PluginError::new(
                PluginErrorKind::NotLoaded,
                format!("plugin not loaded: {plugin_id}"),
            )
        })
    }

    /// 低层 invoke：直接发送/接收字符串，不构造 `PluginInvocation`、不解析输出。
    /// 用于直接验证 ABI 与资源限额路径（trap/fuel/内存/超时/取消）。
    pub async fn invoke_raw(
        &self,
        plugin_id: &PluginId,
        input: String,
        cancel: CancellationToken,
    ) -> Result<String, PluginError> {
        let plugin = self.get(plugin_id)?;
        plugin.invoke_checked(input, cancel).await
    }

    /// 高层 invoke：按 `PluginOperation` 构造 `PluginInvocation`（携带状态快照），
    /// 调用插件，解析 `PluginInvocationOutput` 并原子应用状态变更。
    pub async fn invoke_operation(
        &self,
        plugin_id: &PluginId,
        operation: PluginOperation,
        cancel: CancellationToken,
    ) -> Result<PluginInvocationOutput, PluginError> {
        let plugin = self.get(plugin_id)?;
        plugin.ensure_active()?;
        // Capability 闸门：按 operation 类别校验 manifest 声明的能力。
        // manifest.validate() 已保证 tools/commands/lifecycle_hooks 非空时
        // 对应 capability 存在；这里针对「调用入口」再做一次显式校验，
        // 防止绕过 manifest 注册直接 invoke 一个未声明能力的操作。
        enforce_operation_capability(&plugin.manifest, &operation)?;
        enforce_operation_registration(&plugin.manifest, &operation)?;
        let scope = operation.state_scope();
        let output = plugin.invoke_with_state(operation, scope, cancel).await?;
        Ok(output)
    }

    /// 懒启动全局 epoch ticker（每个 host 一个）。已在运行则跳过。
    /// ticker 周期推进 engine epoch，配合各 store 的 yield-and-update
    /// 实现协作式时间片与可取消的超时。
    fn ensure_ticker(&self) {
        let mut guard = self.ticker.lock();
        if guard.is_none() {
            let engine = self.engine.clone();
            let tick = self.config.epoch_tick;
            let handle = tokio::spawn(async move {
                let mut interval = tokio::time::interval(tick);
                // 跳过第一个立即触发，避免在 invoke 前无故推进 epoch。
                interval.tick().await;
                loop {
                    interval.tick().await;
                    engine.increment_epoch();
                }
            });
            *guard = Some(handle);
        }
    }
}

impl Drop for WasmPluginHost {
    fn drop(&mut self) {
        for plugin in self.plugins.get_mut().values() {
            plugin.active.store(false, Ordering::Release);
        }
        if let Some(handle) = self.ticker.lock().take() {
            handle.abort();
        }
    }
}

/// 把 wasmtime 的 `anyhow::Error` 映射为面向调用方的 [`PluginError`]。
/// 优先识别确定性 trap（fuel/内存/栈/不可达/中断），其余归 Internal。
pub(crate) fn map_wasm_error(error: anyhow::Error) -> PluginError {
    if let Some(trap) = error.downcast_ref::<Trap>() {
        return match trap {
            Trap::OutOfFuel => PluginError::new(
                PluginErrorKind::FuelExhausted,
                "plugin exhausted its fuel budget",
            ),
            Trap::MemoryOutOfBounds | Trap::HeapMisaligned | Trap::TableOutOfBounds => {
                PluginError::new(
                    PluginErrorKind::MemoryLimit,
                    "plugin exceeded its memory or table limit",
                )
            }
            Trap::StackOverflow => PluginError::new(PluginErrorKind::Trap, "plugin stack overflow"),
            Trap::UnreachableCodeReached => PluginError::new(
                PluginErrorKind::Trap,
                "plugin reached unreachable instruction (explicit trap)",
            ),
            Trap::Interrupt => {
                PluginError::new(PluginErrorKind::Trap, "plugin interrupted by host")
            }
            other => PluginError::new(PluginErrorKind::Trap, format!("plugin trap: {other:?}")),
        };
    }
    // `StoreLimits::trap_on_grow_failure(true)` 的拒绝不是 `Trap` 枚举，
    // 而是 host 侧 "forcing trap when growing memory/table to N bytes" 文案。
    // 统一映射为 MemoryLimit。
    let chain: String = format!("{error:#}");
    if chain.contains("forcing trap when growing memory")
        || chain.contains("forcing trap when growing table")
    {
        return PluginError::new(
            PluginErrorKind::MemoryLimit,
            "plugin exceeded its memory or table limit",
        );
    }
    PluginError::new(PluginErrorKind::Internal, error.to_string())
}

fn manifest_error_to_plugin(error: ManifestValidationError) -> PluginError {
    let kind = match &error {
        ManifestValidationError::IncompatibleApi { .. } => PluginErrorKind::IncompatibleApi,
        _ => PluginErrorKind::InvalidManifest,
    };
    PluginError::new(kind, error.to_string())
}

pub(crate) fn state_error_to_plugin(error: PluginStateError) -> PluginError {
    PluginError::new(PluginErrorKind::State, error.to_string())
}

/// 按 operation 类别校验 manifest 声明的能力。
///
/// - `Tool`：要求声明 `RegisterTool`；
/// - `Command`：要求声明 `RegisterCommand`；
/// - `Lifecycle`：要求声明 `LifecycleHook`。
///
/// 状态读写由调用返回后再过 `PersistentState` 闸门（见 `invoke_operation`）。
/// `manifest.validate()` 已保证 tools/commands/lifecycle_hooks 非空时对应
/// capability 存在；这里是面向「调用入口」的二次校验，覆盖直接构造 operation
/// 调用却未在 manifest 声明对应入口的场景。
pub(crate) fn enforce_operation_capability(
    manifest: &plugin_api::PluginManifest,
    operation: &PluginOperation,
) -> Result<(), PluginError> {
    let (required, label) = match operation {
        PluginOperation::Tool { .. } => (PluginCapability::RegisterTool, "tool"),
        PluginOperation::Command(_) => (PluginCapability::RegisterCommand, "command"),
        PluginOperation::Lifecycle { .. } => (PluginCapability::LifecycleHook, "lifecycle"),
    };
    if !manifest.capabilities.contains(&required) {
        return Err(PluginError::new(
            PluginErrorKind::PermissionDenied,
            format!("plugin lacks {label} capability for this operation"),
        ));
    }
    Ok(())
}

fn enforce_operation_registration(
    manifest: &plugin_api::PluginManifest,
    operation: &PluginOperation,
) -> Result<(), PluginError> {
    let declared = match operation {
        PluginOperation::Tool { name, .. } => manifest.tools.iter().any(|tool| tool.name == *name),
        PluginOperation::Command(command) => manifest
            .commands
            .iter()
            .any(|registered| registered.name == command.name),
        PluginOperation::Lifecycle { event, .. } => {
            manifest.lifecycle_hooks.contains(&event.kind())
        }
    };
    if declared {
        Ok(())
    } else {
        Err(PluginError::new(
            PluginErrorKind::PermissionDenied,
            "plugin operation was not declared in the signed manifest",
        ))
    }
}

fn state_snapshot(
    manifest: &plugin_api::PluginManifest,
    state: &dyn PluginStateStore,
    plugin_id: &PluginId,
    scope: &plugin_api::PluginStateScope,
) -> Result<plugin_api::PluginStateSnapshot, PluginError> {
    if manifest
        .capabilities
        .contains(&PluginCapability::PersistentState)
    {
        state
            .snapshot(plugin_id, scope)
            .map_err(state_error_to_plugin)
    } else {
        Ok(plugin_api::PluginStateSnapshot::default())
    }
}

fn apply_state_mutations(
    manifest: &plugin_api::PluginManifest,
    state: &dyn PluginStateStore,
    scope: &plugin_api::PluginStateScope,
    snapshot: &plugin_api::PluginStateSnapshot,
    mutations: &[plugin_api::PluginStateMutation],
    config: &HostConfig,
) -> Result<(), PluginError> {
    if mutations.is_empty() {
        return Ok(());
    }
    if !manifest
        .capabilities
        .contains(&PluginCapability::PersistentState)
    {
        return Err(PluginError::new(
            PluginErrorKind::PermissionDenied,
            "plugin attempted state mutation without PersistentState capability",
        ));
    }
    state
        .apply(&manifest.id, scope, mutations, snapshot.revision, config)
        .map_err(state_error_to_plugin)?;
    Ok(())
}

/// `WasmPluginHost` 实现 `ExternalToolCaller`：把工具调用编码为
/// `PluginOperation::Tool`，经固定 ABI 转发给组件，并把组件返回的
/// `ToolResult` 解码回 `tool-api` 类型。这让 `NamespacedToolRegistry` 注册的
/// `ExternalPluginToolAdapter` 直接调用真实 host，无需 mock。
#[async_trait::async_trait]
impl ExternalToolCaller for WasmPluginHost {
    async fn call(
        &self,
        plugin_id: &PluginId,
        local_name: &str,
        request: tool_api::ToolRequest,
        context: tool_api::ToolExecutionContext,
        cancel: CancellationToken,
    ) -> Result<tool_api::ToolResult, PluginError> {
        let operation = PluginOperation::Tool {
            name: local_name.to_string(),
            request,
            context,
        };
        match self.invoke_operation(plugin_id, operation, cancel).await? {
            PluginInvocationOutput::Success(response) => {
                Ok(crate::registry::tool_result_from_value(response.result))
            }
            PluginInvocationOutput::Error { error } => {
                // 组件自身返回错误：转为 failure 结果，保留 category/retryable。
                Ok(tool_api::ToolResult::failure(
                    agent_domain::ErrorContext::from(error),
                ))
            }
        }
    }
}

#[async_trait::async_trait]
impl ExternalCommandCaller for WasmPluginHost {
    async fn call_command(
        &self,
        plugin_id: &PluginId,
        local_name: &str,
        input: serde_json::Value,
        context: PluginContext,
        cancel: CancellationToken,
    ) -> Result<serde_json::Value, PluginError> {
        let operation = PluginOperation::Command(plugin_api::PluginCommandInvocation {
            name: local_name.to_string(),
            input,
            context,
        });
        match self.invoke_operation(plugin_id, operation, cancel).await? {
            PluginInvocationOutput::Success(response) => Ok(response.result),
            PluginInvocationOutput::Error { error } => Err(error),
        }
    }
}
