//! 模型目录、别名解析、能力过滤、上下文校验、费用估算与 P15-8 三源能力证据。
//!
//! P15-8 能力证据：模型能力来自三处——(a) 目录静态声明（本文件 `entries`）、
//! (b) Provider 探测（`probe_provider` / `record_probe`，同一 provider 只发现
//! 一次，线程安全、不持锁跨 await）、(c) 配置覆盖（`set_override`）。三源
//! 以 provider-neutral 的 [`ModelCapabilities`] 表达，合并取交集（覆盖只能
//! 收窄、不能放大）；「请求 × 支持」的最终交集由 provider-runtime 的
//! CapabilityNegotiator 消费 [`CapabilityEvidence`] 快照完成。

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

use agent_domain::{Cost, ModelId, ProviderId, TokenUsage};
use provider_api::{ModelCapabilities, ModelDefinition, ModelProvider, ResolvedCredential};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::RegistryError;
use crate::pricing::{estimate_cost, ModelPricing};

/// 目录中的单个模型条目。比 [`provider_api::ModelDefinition`] 多了 provider、定价与别名。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: ModelId,
    pub provider: ProviderId,
    pub display_name: String,
    pub context_window_tokens: u64,
    pub max_output_tokens: u64,
    pub capabilities: ModelCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<ModelPricing>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

impl CatalogEntry {
    /// 转换为 Provider 协议的 [`provider_api::ModelDefinition`]（丢弃 provider/定价/别名）。
    pub fn to_definition(&self) -> provider_api::ModelDefinition {
        provider_api::ModelDefinition {
            id: self.id.clone(),
            display_name: self.display_name.clone(),
            context_window_tokens: self.context_window_tokens,
            max_output_tokens: self.max_output_tokens,
            capabilities: self.capabilities.clone(),
        }
    }
}

/// 能力证据来源（P15-8）。优先级：`Static < Probe < Override`。
///
/// 优先级仅用于溯源与展示顺序；合并语义是「present 来源逐字段取交集」——
/// 覆盖不能放大静态/探测未支持的能力，只能收窄（fail-closed）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySource {
    /// 目录静态声明（`register` / `builtin` / `extend_with`）。
    Static,
    /// Provider 能力探测（运行时一次，缓存）。
    Probe,
    /// 配置 / 夹具覆盖（用户显式声明）。
    Override,
}

/// 单个模型的三源能力证据快照（供 P15-8 CapabilityNegotiator 消费）。
///
/// 三源各自保留原始声明（溯源），`merged()` 给出保守合并：仅当所有「有证据的
/// 来源」都声明某能力时才视为支持，来源缺失（`None`）不约束。provider 为
/// `None` 表示该模型只有覆盖证据、无静态 / 探测锚点。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapabilityEvidence {
    pub model: ModelId,
    pub provider: Option<ProviderId>,
    pub static_declared: Option<ModelCapabilities>,
    pub probe_declared: Option<ModelCapabilities>,
    pub override_declared: Option<ModelCapabilities>,
}

impl CapabilityEvidence {
    /// 按来源取原始声明。
    pub fn source(&self, source: CapabilitySource) -> Option<&ModelCapabilities> {
        match source {
            CapabilitySource::Static => self.static_declared.as_ref(),
            CapabilitySource::Probe => self.probe_declared.as_ref(),
            CapabilitySource::Override => self.override_declared.as_ref(),
        }
    }

    /// 保守合并：present 来源逐字段取交集；来源缺失不约束。
    ///
    /// 这是「证据层」合并，不等同于协商：与请求能力的最终交集由 runtime 完成。
    pub fn merged(&self) -> ModelCapabilities {
        let mut present: Vec<&ModelCapabilities> = Vec::new();
        if let Some(capabilities) = &self.static_declared {
            present.push(capabilities);
        }
        if let Some(capabilities) = &self.probe_declared {
            present.push(capabilities);
        }
        if let Some(capabilities) = &self.override_declared {
            present.push(capabilities);
        }
        merge_capabilities(&present)
    }
}

/// Provider 探测产物：一次 `list_models` 的结果，按 provider+model 消费。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderProbe {
    pub provider: ProviderId,
    /// 探测到的模型定义（保持 Provider 返回顺序）。
    pub definitions: Vec<ModelDefinition>,
}

impl ProviderProbe {
    /// 按 model id（ASCII 大小写不敏感）取能力声明。
    pub fn capabilities_for(&self, model: &str) -> Option<&ModelCapabilities> {
        let normalized = model.to_ascii_lowercase();
        self.definitions
            .iter()
            .find(|definition| definition.id.as_str().to_ascii_lowercase() == normalized)
            .map(|definition| &definition.capabilities)
    }

    /// 探测结果是否包含该 model id（ASCII 大小写不敏感）。
    pub fn contains(&self, model: &str) -> bool {
        self.capabilities_for(model).is_some()
    }
}

/// Provider 探测失败（网络 / 认证 / Provider 返回错误）。失败同样进入缓存，
/// 避免对同一 provider 反复探测。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeError(pub String);

impl ProbeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    pub fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ProbeError {}

/// Narrow hook: Provider factory `builtin_models()` catalogs feed `caps()` /
/// negotiation evidence without Provider-name match in Core.
pub trait ProviderCapabilitySource: Send + Sync {
    /// `(provider_id, builtin_models)` pairs. Caller supplies ids; this crate
    /// never branches on Provider name strings.
    fn provider_catalogs(&self) -> Vec<(ProviderId, Vec<ModelDefinition>)>;
}

impl ProviderCapabilitySource for Vec<(ProviderId, Vec<ModelDefinition>)> {
    fn provider_catalogs(&self) -> Vec<(ProviderId, Vec<ModelDefinition>)> {
        self.clone()
    }
}

/// 三来源保守合并：present 来源逐字段取交集，来源缺失不约束。
///
/// 在 serde Value 层面做字段级合并，自动覆盖 provider-api 同期新增的 v2
/// 能力字段（bool 取 AND、数组取元素交集、其它字段全部来源相等才保留，
/// 冲突即 fail-closed 移除该键取字段默认值），v2 落地时无需改动本函数。
pub fn merge_capabilities(sources: &[&ModelCapabilities]) -> ModelCapabilities {
    if sources.is_empty() {
        return ModelCapabilities::default();
    }
    if sources.len() == 1 {
        return sources[0].clone();
    }
    let Value::Object(mut acc) = (match serde_json::to_value(sources[0]) {
        Ok(value) => value,
        Err(_) => return ModelCapabilities::default(),
    }) else {
        return ModelCapabilities::default();
    };
    for source in &sources[1..] {
        let Value::Object(map) = serde_json::to_value(source).unwrap_or(Value::Null) else {
            return ModelCapabilities::default();
        };
        for (key, value) in acc.clone() {
            match map.get(&key) {
                // 该来源未声明此字段：不约束。
                None => {}
                Some(other) => match merge_value(value, other) {
                    Some(merged) => {
                        acc.insert(key, merged);
                    }
                    // 冲突的非布尔/数组字段：移除键，反序列化时取字段默认值。
                    None => {
                        acc.remove(&key);
                    }
                },
            }
        }
    }
    // 反序列化失败（如 v2 字段未带 serde(default)）时整体降级为「全部不支持」
    // （fail-closed），不放大任何能力。
    serde_json::from_value(Value::Object(acc)).unwrap_or_default()
}

fn merge_value(acc: Value, other: &Value) -> Option<Value> {
    match (acc, other) {
        (Value::Bool(left), Value::Bool(right)) => Some(Value::Bool(left && *right)),
        (Value::Array(left), Value::Array(right)) => {
            let right_keys: BTreeSet<String> = right
                .iter()
                .filter_map(|value| serde_json::to_string(value).ok())
                .collect();
            Some(Value::Array(
                left.into_iter()
                    .filter(|value| {
                        serde_json::to_string(value)
                            .map(|key| right_keys.contains(&key))
                            .unwrap_or(false)
                    })
                    .collect(),
            ))
        }
        (left, right) if left == *right => Some(left),
        _ => None,
    }
}

/// 模型目录。内置模型 + provider 动态发现 + 用户自定义覆盖，统一按 id 索引。
#[derive(Debug, Default)]
pub struct ModelRegistry {
    entries: BTreeMap<ModelId, CatalogEntry>,
    alias_to_id: BTreeMap<String, ModelId>,
    /// 配置覆盖（P15-8）：model id -> 能力声明。线程安全读写。
    overrides: Mutex<BTreeMap<ModelId, ModelCapabilities>>,
    /// Provider 探测缓存（P15-8）：provider id -> 探测槽位。同一 provider 只
    /// 探测一次；并发调用共享槽位，不持锁跨 await。
    probes: Arc<Mutex<BTreeMap<ProviderId, Arc<ProbeSlot>>>>,
}

impl Clone for ModelRegistry {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            alias_to_id: self.alias_to_id.clone(),
            // 覆盖表深拷贝（用户配置随副本独立）；探测缓存整体共享（发现
            // 结果幂等，克隆体直接复用同一缓存，避免重复探测）。
            overrides: Mutex::new(lock(&self.overrides).clone()),
            probes: Arc::clone(&self.probes),
        }
    }
}

impl ModelRegistry {
    /// 创建空目录。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 创建带内置常用模型目录的注册表。
    pub fn builtin() -> Self {
        let mut registry = Self::empty();
        for entry in builtin_entries() {
            // 内置目录自身假定无冲突，直接写入（覆盖语义）。
            let normalized_id = normalized_model_id(&entry.id);
            for alias in &entry.aliases {
                registry
                    .alias_to_id
                    .insert(alias.to_ascii_lowercase(), normalized_id.clone());
            }
            registry.entries.insert(normalized_id, entry);
        }
        registry
    }

    /// 注册单个条目（含别名）。注册时若别名已被其它模型占用则返回错误。
    pub fn register(&mut self, entry: CatalogEntry) -> Result<(), RegistryError> {
        self.try_register(entry)
    }

    /// 注册并在别名冲突时返回错误（别名预检后写入）。
    pub fn try_register(&mut self, entry: CatalogEntry) -> Result<(), RegistryError> {
        let normalized_id = normalized_model_id(&entry.id);
        for alias in &entry.aliases {
            let normalized_alias = alias.to_ascii_lowercase();
            if let Some(existing) = self.alias_to_id.get(&normalized_alias) {
                if *existing != normalized_id {
                    return Err(RegistryError::DuplicateAlias {
                        alias: alias.clone(),
                        existing: existing.to_string(),
                    });
                }
            }
        }
        for alias in &entry.aliases {
            self.alias_to_id
                .insert(alias.to_ascii_lowercase(), normalized_id.clone());
        }
        self.entries.insert(normalized_id, entry);
        Ok(())
    }

    /// 合并 provider 动态发现或用户自定义的模型；同 id 覆盖、别名冲突时跳过旧映射。
    pub fn extend_with(&mut self, entries: Vec<CatalogEntry>) {
        for entry in entries {
            // 覆盖语义：同 id 直接替换；别名以新条目为准（覆盖旧映射）。
            let normalized_id = normalized_model_id(&entry.id);
            for alias in &entry.aliases {
                self.alias_to_id
                    .insert(alias.to_ascii_lowercase(), normalized_id.clone());
            }
            self.entries.insert(normalized_id, entry);
        }
    }

    /// Merge factory `builtin_models()` catalogs into the static directory and
    /// therefore into [`CapabilityEvidence::static_declared`] / `caps()` evidence.
    ///
    /// Lookup is by [`ProviderId`] equality only — no Provider-name match/case
    /// in Core. Cross-provider id collisions are skipped (fail-closed). New
    /// models are appended without pricing/aliases (those stay catalog-owned).
    pub fn merge_provider_source(&mut self, source: &dyn ProviderCapabilitySource) {
        for (provider, models) in source.provider_catalogs() {
            self.merge_provider_models(&provider, &models);
        }
    }

    /// Merge one provider's `builtin_models()` into the static catalog.
    pub fn merge_provider_models(&mut self, provider: &ProviderId, models: &[ModelDefinition]) {
        for definition in models {
            let id = normalized_model_id(&definition.id);
            if let Some(existing) = self.entries.get(&id) {
                if &existing.provider != provider {
                    continue;
                }
                let mut entry = existing.clone();
                entry.display_name = definition.display_name.clone();
                entry.context_window_tokens = definition.context_window_tokens;
                entry.max_output_tokens = definition.max_output_tokens;
                entry.capabilities = definition.capabilities.clone();
                self.entries.insert(id, entry);
            } else {
                self.extend_with(vec![CatalogEntry {
                    id: definition.id.clone(),
                    provider: provider.clone(),
                    display_name: definition.display_name.clone(),
                    context_window_tokens: definition.context_window_tokens,
                    max_output_tokens: definition.max_output_tokens,
                    capabilities: definition.capabilities.clone(),
                    pricing: None,
                    aliases: Vec::new(),
                }]);
            }
        }
    }

    /// 按 id 或别名解析条目。
    pub fn resolve(&self, id_or_alias: &str) -> Option<&CatalogEntry> {
        let normalized = id_or_alias.to_ascii_lowercase();
        let id = self.alias_to_id.get(&normalized).cloned();
        if let Some(id) = id {
            return self.entries.get(&id);
        }
        // 也允许直接用真实 model id；ASCII 大小写在入口统一归一。
        self.entries.get(&ModelId::new(normalized))
    }

    /// 列出全部条目（按 id 排序）。
    pub fn list(&self) -> Vec<&CatalogEntry> {
        self.entries.values().collect()
    }

    /// 按能力过滤：`required` 中为 `true` 的能力，候选条目必须同时满足。
    pub fn filter(&self, required: &ModelCapabilities) -> Vec<&CatalogEntry> {
        self.entries
            .values()
            .filter(|entry| caps_satisfied(&entry.capabilities, required))
            .collect()
    }

    /// 校验输入 token 数是否在模型的上下文窗口内。
    pub fn validate_context(&self, id_or_alias: &str, input_tokens: u64) -> bool {
        match self.resolve(id_or_alias) {
            Some(entry) => input_tokens <= entry.context_window_tokens,
            None => false,
        }
    }

    /// 按定价估算费用；模型未注册或无定价时返回 `None`。
    pub fn estimate_cost(&self, id_or_alias: &str, usage: &TokenUsage) -> Option<Cost> {
        let entry = self.resolve(id_or_alias)?;
        let pricing = entry.pricing.as_ref()?;
        Some(estimate_cost(usage, pricing))
    }

    // ---------- P15-8 三源能力证据 ----------

    /// 配置覆盖写入（来源 Override）：model -> capabilities。
    ///
    /// 覆盖只能收窄、不能放大：`merged()` 取交集，覆盖声明的能力若静态/探测
    /// 未支持，最终合并结果仍为不支持。model id ASCII 大小写不敏感。
    pub fn set_override(&self, model: impl AsRef<str>, capabilities: ModelCapabilities) {
        lock(&self.overrides).insert(
            normalized_model_id(&ModelId::new(model.as_ref())),
            capabilities,
        );
    }

    /// 移除配置覆盖；返回是否确实存在该覆盖。
    pub fn remove_override(&self, model: impl AsRef<str>) -> bool {
        let model = normalized_model_id(&ModelId::new(model.as_ref()));
        lock(&self.overrides).remove(&model).is_some()
    }

    /// 读取配置覆盖；无覆盖返回 `None`。
    pub fn override_for(&self, model: &str) -> Option<ModelCapabilities> {
        let model = normalized_model_id(&ModelId::new(model));
        lock(&self.overrides).get(&model).cloned()
    }

    /// 全部覆盖（按 model id 排序）。
    pub fn overrides(&self) -> Vec<(ModelId, ModelCapabilities)> {
        lock(&self.overrides)
            .iter()
            .map(|(model, capabilities)| (model.clone(), capabilities.clone()))
            .collect()
    }

    /// 写入探测结果（来源 Probe）。调用方自行执行 `list_models` 时使用；
    /// 与 [`Self::probe_provider`] 共用同一缓存。last-write-wins：无论槽位
    /// 处于 `Idle` / `InFlight` / `Done`，都以新结果为准——立即固定并唤醒
    /// 等待者；若此时有探测进行中，其 owner 稍后完成时不会覆盖新结果。
    pub fn record_probe(
        &self,
        provider: &ProviderId,
        definitions: Vec<ModelDefinition>,
    ) -> Arc<ProviderProbe> {
        let probe = Arc::new(ProviderProbe {
            provider: provider.clone(),
            definitions,
        });
        self.probe_slot(provider).complete(Ok(probe.clone()));
        probe
    }

    /// 清除探测缓存；返回是否确实存在缓存。正在探测时清除会使后续调用
    /// 重新探测（旧槽位结果仍会完成，仅不再被引用）。
    pub fn clear_probe(&self, provider: &ProviderId) -> bool {
        lock(&self.probes).remove(provider).is_some()
    }

    /// 异步探测：同一 provider 只执行一次 `list_models`，结果与失败均按
    /// provider 缓存；并发调用共享同一次探测（线程安全，不持锁跨 await）。
    /// 失败可经 [`Self::clear_probe`] 清除后重试。探测进行中若并发
    /// [`Self::record_probe`] 固定了新结果，本调用以固定结果为准。
    pub async fn probe_provider(
        &self,
        provider: &dyn ModelProvider,
        credential: Option<&ResolvedCredential>,
    ) -> Result<Arc<ProviderProbe>, ProbeError> {
        let provider_id = provider.id();
        let slot = self.probe_slot(&provider_id);
        loop {
            if let Some(cached) = slot.cached() {
                return cached;
            }
            match slot.try_claim() {
                ClaimOutcome::Won => {
                    let result = match provider.list_models(credential).await {
                        Ok(definitions) => Ok(Arc::new(ProviderProbe {
                            provider: provider_id.clone(),
                            definitions,
                        })),
                        Err(error) => Err(ProbeError::new(error.to_string())),
                    };
                    if slot.complete_claimed(result.clone()) {
                        return result;
                    }
                    // 探测期间被并发 `record_probe` 固定了新结果：以固定结果为准。
                    continue;
                }
                ClaimOutcome::Wait => WaitForProbe { slot: &slot }.await,
                ClaimOutcome::Done => continue,
            }
        }
    }

    /// 单模型三源证据；无任何来源证据时返回 `None`。model id 或别名均可，
    /// 别名解析复用目录。
    pub fn capability_evidence(&self, model: &str) -> Option<CapabilityEvidence> {
        let entry = self.resolve(model);
        let normalized = ModelId::new(model.to_ascii_lowercase());
        let model_id = entry
            .map(|entry| entry.id.clone())
            .unwrap_or_else(|| normalized.clone());
        let static_declared = entry.map(|entry| entry.capabilities.clone());
        let (probe_provider, probe_declared) = match entry {
            // 静态锚定：只查询该 provider 的探测缓存。
            Some(entry) => (
                Some(entry.provider.clone()),
                self.probe_capabilities_for(&entry.provider, &normalized),
            ),
            // 无静态条目：按 provider id 排序扫描已缓存探测。
            None => self.probe_capabilities_any(&normalized),
        };
        let override_declared = self.override_for(normalized.as_str());
        if static_declared.is_none() && probe_declared.is_none() && override_declared.is_none() {
            return None;
        }
        Some(CapabilityEvidence {
            model: model_id,
            provider: probe_provider,
            static_declared,
            probe_declared,
            override_declared,
        })
    }

    /// 全量三源证据快照（按 model id 排序），供 negotiator 初始化。
    ///
    /// 模型键并集 = 静态条目 ∪ 探测定义 ∪ 覆盖键；仅含探测/覆盖证据的模型
    /// 也会出现在快照中（provider 分别为探测锚点或 `None`）。
    pub fn capability_snapshot(&self) -> Vec<CapabilityEvidence> {
        let mut models: BTreeSet<ModelId> = self.entries.keys().cloned().collect();
        for slot in lock(&self.probes).values() {
            let Some(result) = slot.cached() else {
                continue;
            };
            let Ok(probe) = result else {
                continue;
            };
            for definition in &probe.definitions {
                models.insert(normalized_model_id(&definition.id));
            }
        }
        for model in lock(&self.overrides).keys() {
            models.insert(model.clone());
        }
        models
            .into_iter()
            .filter_map(|model| self.capability_evidence(model.as_str()))
            .collect()
    }

    fn probe_slot(&self, provider: &ProviderId) -> Arc<ProbeSlot> {
        lock(&self.probes)
            .entry(provider.clone())
            .or_insert_with(|| Arc::new(ProbeSlot::default()))
            .clone()
    }

    fn probe_capabilities_for(
        &self,
        provider: &ProviderId,
        model: &ModelId,
    ) -> Option<ModelCapabilities> {
        let probes = lock(&self.probes);
        let probe = probes.get(provider)?.cached()?.ok()?;
        probe.capabilities_for(model.as_str()).cloned()
    }

    fn probe_capabilities_any(
        &self,
        model: &ModelId,
    ) -> (Option<ProviderId>, Option<ModelCapabilities>) {
        for (provider, slot) in lock(&self.probes).iter() {
            let Some(result) = slot.cached() else {
                continue;
            };
            let Ok(probe) = result else {
                continue;
            };
            if let Some(capabilities) = probe.capabilities_for(model.as_str()) {
                return (Some(provider.clone()), Some(capabilities.clone()));
            }
        }
        (None, None)
    }
}

fn normalized_model_id(id: &ModelId) -> ModelId {
    ModelId::new(id.as_str().to_ascii_lowercase())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 单个 provider 的探测槽位。状态机：`Idle -> InFlight -> Done`；`Done` 同时
/// 缓存成功结果与失败原因。所有锁仅在同步段内持有，不跨 await。
///
/// 写入分两种：[`ProbeSlot::complete`] 为强制固定（`record_probe` 路径，
/// last-write-wins，可从任意状态进入 `Done`）；[`ProbeSlot::complete_claimed`]
/// 仅当槽位仍为自己的 `InFlight` 时提交，防止迟到的探测结果覆盖已固定结果。
#[derive(Default)]
struct ProbeSlot {
    state: Mutex<ProbeState>,
}

impl std::fmt::Debug for ProbeSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ProbeSlot").finish_non_exhaustive()
    }
}

#[derive(Default)]
enum ProbeState {
    #[default]
    Idle,
    InFlight {
        wakers: Vec<Waker>,
    },
    Done(Result<Arc<ProviderProbe>, ProbeError>),
}

enum ClaimOutcome {
    /// 本调用获得探测权，负责执行 `list_models`。
    Won,
    /// 已有探测进行中：登记 waker 等待完成。
    Wait,
    /// 已完成的瞬间观察到：调用方重新读缓存即可。
    Done,
}

impl ProbeSlot {
    fn cached(&self) -> Option<Result<Arc<ProviderProbe>, ProbeError>> {
        match &*lock(&self.state) {
            ProbeState::Done(result) => Some(result.clone()),
            _ => None,
        }
    }

    fn try_claim(&self) -> ClaimOutcome {
        let mut state = lock(&self.state);
        match &mut *state {
            ProbeState::Idle => {
                *state = ProbeState::InFlight { wakers: Vec::new() };
                ClaimOutcome::Won
            }
            ProbeState::InFlight { .. } => ClaimOutcome::Wait,
            ProbeState::Done(_) => ClaimOutcome::Done,
        }
    }

    /// 强制固定结果（[`Self::record_probe`] 路径，last-write-wins）：无论
    /// 槽位处于 `Idle` / `InFlight` / `Done` 都以新结果为准，原 `InFlight`
    /// 的等待者被唤醒后读到新结果。
    fn complete(&self, result: Result<Arc<ProviderProbe>, ProbeError>) {
        let wakers = {
            let mut state = lock(&self.state);
            let wakers = match &mut *state {
                ProbeState::InFlight { wakers } => std::mem::take(wakers),
                _ => Vec::new(),
            };
            *state = ProbeState::Done(result);
            wakers
        };
        for waker in wakers {
            waker.wake();
        }
    }

    /// Claim 完成（[`Self::probe_provider`] owner 路径）：仅当槽位仍为自己
    /// 的 `InFlight` 时提交结果并唤醒等待者，返回 `true`；若已被强制固定为
    /// `Done`（或槽位处于 `Idle`），丢弃本次结果并返回 `false`，调用方应
    /// 重新读取缓存。
    fn complete_claimed(&self, result: Result<Arc<ProviderProbe>, ProbeError>) -> bool {
        let wakers = {
            let mut state = lock(&self.state);
            let ProbeState::InFlight { wakers } = &mut *state else {
                return false;
            };
            let wakers = std::mem::take(wakers);
            *state = ProbeState::Done(result);
            wakers
        };
        for waker in wakers {
            waker.wake();
        }
        true
    }
}

/// 等待进行中的探测完成：仅在 poll 内持有锁并登记 waker，锁不跨 await。
/// 同一 waker 重复 poll 时用 `will_wake` 去重，避免等待者列表累积。
struct WaitForProbe<'a> {
    slot: &'a ProbeSlot,
}

impl Future for WaitForProbe<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        match &mut *lock(&self.slot.state) {
            // 无人探测（槽位被重置）或已完成：返回让调用方重新读缓存/认领。
            ProbeState::Idle | ProbeState::Done(_) => Poll::Ready(()),
            ProbeState::InFlight { wakers } => {
                let dedup = wakers
                    .iter()
                    .any(|registered| registered.will_wake(cx.waker()));
                eprintln!(
                    "DEBUG poll dedup={dedup} existing={} self={}",
                    wakers.len(),
                    cx.waker().will_wake(cx.waker())
                );
                if !dedup {
                    wakers.push(cx.waker().clone());
                }
                Poll::Pending
            }
        }
    }
}

fn caps_satisfied(have: &ModelCapabilities, required: &ModelCapabilities) -> bool {
    // v1 布尔能力：required 为 true 时 have 必须满足。
    let v1 = (!required.text || have.text)
        && (!required.image_input || have.image_input)
        && (!required.tool_calls || have.tool_calls)
        && (!required.parallel_tool_calls || have.parallel_tool_calls)
        && (!required.thinking || have.thinking)
        && (!required.structured_output || have.structured_output)
        && (!required.prompt_cache || have.prompt_cache);
    if !v1 {
        return false;
    }
    // P15-8 v2：citations（required 为 true 时 have 必须声明）。
    if required.citations && !have.citations {
        return false;
    }
    // P15-8 v2：transport——required 声明非默认（非 ChatCompletions）transport 时，
    // have 必须声明同一 transport（要求 Responses 时只接受 Responses）。
    // required 为默认 ChatCompletions 视为「不约束」。
    if required.transport != provider_api::ModelTransport::ChatCompletions
        && have.transport != required.transport
    {
        return false;
    }
    // P15-8 v2：hosted tool 标签——required 中的每个标签 have 必须包含（子集）。
    if !required
        .hosted_tool_tags
        .iter()
        .all(|tag| have.hosted_tool_tags.contains(tag))
    {
        return false;
    }
    true
}

/// 构造能力集合的便捷函数。
// `..Default::default()` 目前无实际效果（v1 字段已全部显式给出），但 provider-api
// 同期新增 v2 字段后它就是构造点的兼容性保障，因此保留并允许该 lint。
//
// v2 capability catalogs from Provider factory `builtin_models()` are merged
// via [`ModelRegistry::merge_provider_models`] / [`ProviderCapabilitySource`],
// not by expanding this helper's argument list.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::needless_update)]
pub fn caps(
    text: bool,
    image_input: bool,
    tool_calls: bool,
    parallel_tool_calls: bool,
    thinking: bool,
    structured_output: bool,
    prompt_cache: bool,
) -> ModelCapabilities {
    ModelCapabilities {
        text,
        image_input,
        tool_calls,
        parallel_tool_calls,
        thinking,
        structured_output,
        prompt_cache,
        // provider-api 同期新增 v2 能力字段：用 ..Default 兼容，避免构造点
        // 在 v2 落地后编译失败；合并由 merge_capabilities 在 Value 层面自动覆盖。
        ..Default::default()
    }
}

/// 内置常用模型目录。定价为近似基线（仅用于估算演示，真实定价以 provider 为准），
/// 上下文窗口取公开文档的保守值。本地兼容服务（Ollama/vLLM/LM Studio）的模型
/// 在连接后通过 `extend_with` 动态补充。
fn builtin_entries() -> Vec<CatalogEntry> {
    let text_vision_tools = caps(true, true, true, true, false, true, true);
    let tools_only = caps(true, false, true, true, false, true, true);
    let text_only = caps(true, false, false, false, false, false, false);

    vec![
        // OpenAI 系
        CatalogEntry {
            id: ModelId::new("gpt-4o"),
            provider: ProviderId::new("openai"),
            display_name: "GPT-4o".into(),
            context_window_tokens: 128_000,
            max_output_tokens: 16_384,
            capabilities: text_vision_tools.clone(),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 2_500_000,
                output_per_mtoken_micros: 10_000_000,
                cache_read_per_mtoken_micros: 1_250_000,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["gpt4o".into(), "4o".into()],
        },
        CatalogEntry {
            id: ModelId::new("gpt-4o-mini"),
            provider: ProviderId::new("openai"),
            display_name: "GPT-4o mini".into(),
            context_window_tokens: 128_000,
            max_output_tokens: 16_384,
            capabilities: text_vision_tools.clone(),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 150_000,
                output_per_mtoken_micros: 600_000,
                cache_read_per_mtoken_micros: 75_000,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["mini".into()],
        },
        // Anthropic 系
        CatalogEntry {
            id: ModelId::new("claude-3-5-sonnet"),
            provider: ProviderId::new("anthropic"),
            display_name: "Claude 3.5 Sonnet".into(),
            context_window_tokens: 200_000,
            max_output_tokens: 8_192,
            capabilities: caps(true, true, true, true, true, true, true),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 3_000_000,
                output_per_mtoken_micros: 15_000_000,
                cache_read_per_mtoken_micros: 300_000,
                cache_write_per_mtoken_micros: 3_750_000,
                currency: "USD".into(),
            }),
            aliases: vec!["sonnet".into(), "claude".into()],
        },
        // Google 系
        CatalogEntry {
            id: ModelId::new("gemini-1.5-pro"),
            provider: ProviderId::new("google"),
            display_name: "Gemini 1.5 Pro".into(),
            context_window_tokens: 1_000_000,
            max_output_tokens: 8_192,
            capabilities: caps(true, true, true, true, false, true, false),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 1_250_000,
                output_per_mtoken_micros: 5_000_000,
                cache_read_per_mtoken_micros: 0,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["gemini".into(), "gemini-pro".into()],
        },
        // 本地兼容服务占位（实际由 provider 动态发现覆盖）
        CatalogEntry {
            id: ModelId::new("llama-3.1-8b"),
            provider: ProviderId::new("openai-compatible"),
            display_name: "Llama 3.1 8B (local)".into(),
            context_window_tokens: 128_000,
            max_output_tokens: 4_096,
            capabilities: tools_only,
            pricing: None,
            aliases: vec!["llama".into()],
        },
        // 纯文本小模型示例（无工具/无视觉）
        CatalogEntry {
            id: ModelId::new("gpt-3.5-turbo"),
            provider: ProviderId::new("openai"),
            display_name: "GPT-3.5 Turbo".into(),
            context_window_tokens: 16_385,
            max_output_tokens: 4_096,
            capabilities: text_only,
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 500_000,
                output_per_mtoken_micros: 1_500_000,
                cache_read_per_mtoken_micros: 0,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["3.5".into()],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::ModelPricing;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use agent_domain::CancellationToken;
    use async_trait::async_trait;
    use provider_api::{
        CanonicalModelRequest, ModelResponseSummary, ProviderError, ProviderErrorKind,
        ProviderEventSink,
    };

    fn test_entry(id: &str, provider: &str, capabilities: ModelCapabilities) -> CatalogEntry {
        CatalogEntry {
            id: ModelId::new(id),
            provider: ProviderId::new(provider),
            display_name: id.into(),
            context_window_tokens: 32_000,
            max_output_tokens: 4_096,
            capabilities,
            pricing: None,
            aliases: Vec::new(),
        }
    }

    fn mock_definition(id: &str, capabilities: ModelCapabilities) -> ModelDefinition {
        ModelDefinition {
            id: ModelId::new(id),
            display_name: id.into(),
            context_window_tokens: 32_000,
            max_output_tokens: 4_096,
            capabilities,
        }
    }

    struct MockProvider {
        id: ProviderId,
        calls: Arc<AtomicUsize>,
        result: Result<Vec<ModelDefinition>, ProviderError>,
        yield_before: bool,
    }

    #[async_trait]
    impl ModelProvider for MockProvider {
        fn id(&self) -> ProviderId {
            self.id.clone()
        }

        async fn list_models(
            &self,
            _credential: Option<&ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            if self.yield_before {
                // 让并发测试中的其它任务有机会到达等待点。
                tokio::task::yield_now().await;
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }

        async fn stream(
            &self,
            _request: CanonicalModelRequest,
            _sink: &dyn ProviderEventSink,
            _cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            Err(ProviderError::new(
                ProviderErrorKind::Unknown,
                "mock stream",
            ))
        }
    }

    /// 可确定性放行的探测 Provider：进入 `list_models` 时置位 `started`，
    /// 在 `release` 置位前反复 yield（不阻塞运行时线程）。
    struct BlockingProvider {
        id: ProviderId,
        started: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
        result: Result<Vec<ModelDefinition>, ProviderError>,
    }

    #[async_trait]
    impl ModelProvider for BlockingProvider {
        fn id(&self) -> ProviderId {
            self.id.clone()
        }

        async fn list_models(
            &self,
            _credential: Option<&ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            self.started.store(true, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            self.result.clone()
        }

        async fn stream(
            &self,
            _request: CanonicalModelRequest,
            _sink: &dyn ProviderEventSink,
            _cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            Err(ProviderError::new(
                ProviderErrorKind::Unknown,
                "mock stream",
            ))
        }
    }

    #[test]
    fn builtin_catalog_resolves_real_ids_and_aliases() {
        let registry = ModelRegistry::builtin();
        assert!(registry.resolve("gpt-4o").is_some());
        assert_eq!(registry.resolve("GPT-4O"), registry.resolve("gpt-4o"));
        assert!(registry.resolve("gpt4o").is_some(), "别名须可解析");
        assert_eq!(registry.resolve("GPT4O"), registry.resolve("gpt4o"));
        assert!(registry.resolve("sonnet").is_some());
        assert!(registry.resolve("gemini-pro").is_some());
        assert!(registry.resolve("nonexistent").is_none());
        assert!(!registry.list().is_empty());
    }

    #[test]
    fn alias_conflict_is_reported() {
        let mut registry = ModelRegistry::empty();
        let first = CatalogEntry {
            id: ModelId::new("a"),
            provider: ProviderId::new("p"),
            display_name: "A".into(),
            context_window_tokens: 1000,
            max_output_tokens: 100,
            capabilities: caps(true, false, false, false, false, false, false),
            pricing: None,
            aliases: vec!["shared".into()],
        };
        registry.try_register(first).expect("首次注册成功");

        let conflicting = CatalogEntry {
            id: ModelId::new("b"),
            provider: ProviderId::new("p"),
            display_name: "B".into(),
            context_window_tokens: 1000,
            max_output_tokens: 100,
            capabilities: caps(true, false, false, false, false, false, false),
            pricing: None,
            aliases: vec!["shared".into()],
        };
        let err = registry
            .try_register(conflicting)
            .expect_err("重复别名必须报错");
        assert!(matches!(err, RegistryError::DuplicateAlias { .. }));
    }

    #[test]
    fn alias_conflict_is_ascii_case_insensitive() {
        let mut registry = ModelRegistry::empty();
        let first = CatalogEntry {
            id: ModelId::new("a"),
            provider: ProviderId::new("p"),
            display_name: "A".into(),
            context_window_tokens: 1000,
            max_output_tokens: 100,
            capabilities: caps(true, false, false, false, false, false, false),
            pricing: None,
            aliases: vec!["Shared".into()],
        };
        registry.try_register(first).expect("首次注册成功");

        let conflicting = CatalogEntry {
            id: ModelId::new("b"),
            provider: ProviderId::new("p"),
            display_name: "B".into(),
            context_window_tokens: 1000,
            max_output_tokens: 100,
            capabilities: caps(true, false, false, false, false, false, false),
            pricing: None,
            aliases: vec!["SHARED".into()],
        };

        assert!(matches!(
            registry.try_register(conflicting),
            Err(RegistryError::DuplicateAlias { .. })
        ));
        assert_eq!(
            registry.resolve("shared").map(|entry| &entry.id),
            Some(&ModelId::new("a"))
        );
    }

    #[test]
    fn direct_model_ids_are_ascii_case_insensitive() {
        let mut registry = ModelRegistry::empty();
        registry
            .try_register(CatalogEntry {
                id: ModelId::new("Custom-Model"),
                provider: ProviderId::new("p"),
                display_name: "Custom".into(),
                context_window_tokens: 1000,
                max_output_tokens: 100,
                capabilities: caps(true, false, false, false, false, false, false),
                pricing: None,
                aliases: Vec::new(),
            })
            .expect("register mixed-case id");

        assert_eq!(
            registry.resolve("CUSTOM-MODEL").map(|entry| &entry.id),
            Some(&ModelId::new("Custom-Model"))
        );
    }

    #[test]
    fn capability_filter_selects_only_matching_models() {
        let registry = ModelRegistry::builtin();
        let with_tools = caps(true, false, true, false, false, false, false);
        let filtered: Vec<ModelId> = registry
            .filter(&with_tools)
            .into_iter()
            .map(|entry| entry.id.clone())
            .collect();
        assert!(filtered.contains(&ModelId::new("gpt-4o")));
        assert!(filtered.contains(&ModelId::new("claude-3-5-sonnet")));
        // gpt-3.5-turbo 无工具能力，应被排除
        assert!(!filtered.contains(&ModelId::new("gpt-3.5-turbo")));
    }

    #[test]
    fn context_validation_respects_window() {
        let registry = ModelRegistry::builtin();
        assert!(registry.validate_context("gpt-3.5-turbo", 16_000));
        assert!(!registry.validate_context("gpt-3.5-turbo", 100_000));
        assert!(!registry.validate_context("unknown-model", 10));
    }

    #[test]
    fn cost_estimate_matches_manual_integer_math() {
        let registry = ModelRegistry::builtin();
        let usage = TokenUsage {
            input_tokens: 2_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let cost = registry
            .estimate_cost("gpt-4o", &usage)
            .expect("有定价的模型可估算");
        assert_eq!(cost.currency, "USD");
        // 2M input * $2.5/M + 1M output * $10/M = 5M + 10M = 15_000_000 micros = $15.00
        assert_eq!(cost.amount_micros, 15_000_000);
    }

    #[test]
    fn extend_with_overrides_same_id() {
        let mut registry = ModelRegistry::builtin();
        let discovered = vec![CatalogEntry {
            id: ModelId::new("llama-3.1-8b"),
            provider: ProviderId::new("ollama"),
            display_name: "Llama 3.1 8B (discovered)".into(),
            context_window_tokens: 32_000,
            max_output_tokens: 2_048,
            capabilities: caps(true, false, true, false, false, false, false),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 0,
                output_per_mtoken_micros: 0,
                cache_read_per_mtoken_micros: 0,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["llama".into()],
        }];
        registry.extend_with(discovered);

        let entry = registry.resolve("llama-3.1-8b").expect("覆盖后仍可解析");
        assert_eq!(entry.provider, ProviderId::new("ollama"));
        assert_eq!(entry.context_window_tokens, 32_000);
        assert!(entry.pricing.is_some(), "动态发现的定价覆盖了无定价占位");
        assert_eq!(
            registry.resolve("llama").map(|entry| entry.id.clone()),
            Some(ModelId::new("llama-3.1-8b"))
        );
    }

    #[test]
    fn models_without_pricing_cannot_estimate_cost() {
        let registry = ModelRegistry::builtin();
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 10,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        // llama 内置占位无定价
        assert!(registry.estimate_cost("llama", &usage).is_none());
    }

    // ---------- P15-8 三源能力证据 ----------

    #[test]
    fn capability_source_priority_is_static_then_probe_then_override() {
        assert!(CapabilitySource::Static < CapabilitySource::Probe);
        assert!(CapabilitySource::Probe < CapabilitySource::Override);
    }

    #[test]
    fn caps_satisfied_v2_citations_and_transport_and_tools() {
        use provider_api::{ModelCapabilities, ModelTransport};

        let full = ModelCapabilities {
            citations: true,
            transport: ModelTransport::Responses,
            hosted_tool_tags: [agent_domain::ToolCapabilityTag::WebSearch]
                .into_iter()
                .collect(),
            ..caps(true, true, true, true, true, true, true)
        };

        // 要求 v2 citations：声明即满足。
        let req = ModelCapabilities {
            citations: true,
            ..ModelCapabilities::default()
        };
        assert!(caps_satisfied(&full, &req), "citations 声明即满足");

        // 要求 Responses transport：声明即满足；要求 Messages 不满足。
        let req_responses = ModelCapabilities {
            transport: ModelTransport::Responses,
            ..ModelCapabilities::default()
        };
        assert!(caps_satisfied(&full, &req_responses));
        let req_messages = ModelCapabilities {
            transport: ModelTransport::Messages,
            ..ModelCapabilities::default()
        };
        assert!(
            !caps_satisfied(&full, &req_messages),
            "要求 Messages 但模型只声明 Responses → 不满足"
        );

        // 要求 hosted tool WebSearch：包含即满足；要求 CodeExecution 不满足。
        let req_tool = ModelCapabilities {
            hosted_tool_tags: [agent_domain::ToolCapabilityTag::WebSearch]
                .into_iter()
                .collect(),
            ..ModelCapabilities::default()
        };
        assert!(caps_satisfied(&full, &req_tool));
        let req_tool_missing = ModelCapabilities {
            hosted_tool_tags: [agent_domain::ToolCapabilityTag::CodeExecution]
                .into_iter()
                .collect(),
            ..ModelCapabilities::default()
        };
        assert!(
            !caps_satisfied(&full, &req_tool_missing),
            "未声明的 hosted tool → 不满足（fail-closed）"
        );

        // 默认 ChatCompletions required 不约束 transport。
        let baseline = caps(true, false, false, false, false, false, false);
        let req_default = ModelCapabilities::default();
        assert!(caps_satisfied(&baseline, &req_default));
    }

    #[test]
    fn merge_is_field_wise_intersection() {
        let a = caps(true, false, true, false, false, false, false);
        let b = caps(true, false, true, true, false, false, false);
        let merged = merge_capabilities(&[&a, &b]);
        assert!(merged.text);
        assert!(merged.tool_calls);
        assert!(!merged.parallel_tool_calls, "任一来源不支持则整体不支持");
    }

    #[test]
    fn merge_ignores_absent_sources() {
        let a = caps(true, false, true, false, false, false, false);
        assert_eq!(merge_capabilities(&[&a]), a, "单一来源原样返回");
        assert_eq!(
            merge_capabilities(&[]),
            ModelCapabilities::default(),
            "无来源全部不支持"
        );
    }

    #[test]
    fn merge_value_intersects_bools_arrays_and_drops_conflicts() {
        assert_eq!(
            merge_value(Value::Bool(true), &Value::Bool(false)),
            Some(Value::Bool(false))
        );
        assert_eq!(
            merge_value(Value::Bool(true), &Value::Bool(true)),
            Some(Value::Bool(true))
        );
        let merged = merge_value(
            serde_json::json!(["a", "b"]),
            &serde_json::json!(["b", "c"]),
        )
        .expect("数组取交集");
        assert_eq!(merged, serde_json::json!(["b"]));
        assert_eq!(
            merge_value(Value::String("a".into()), &Value::String("a".into())),
            Some(Value::String("a".into()))
        );
        assert_eq!(
            merge_value(Value::String("a".into()), &Value::String("b".into())),
            None,
            "冲突的非布尔/数组字段 fail-closed 移除"
        );
    }

    #[test]
    fn override_cannot_amplify_static_capabilities() {
        let mut registry = ModelRegistry::empty();
        registry
            .try_register(test_entry(
                "m1",
                "p",
                caps(true, false, false, false, false, false, false),
            ))
            .expect("register");
        // 试图放大：静态未支持 tool_calls / thinking / structured_output。
        registry.set_override("m1", caps(true, false, true, true, true, true, true));

        let evidence = registry.capability_evidence("m1").expect("有静态证据");
        let merged = evidence.merged();
        assert!(merged.text);
        assert!(!merged.tool_calls, "覆盖不能放大静态未支持的能力");
        assert!(!merged.thinking);
        assert!(!merged.structured_output);
        assert!(
            evidence.override_declared.as_ref().unwrap().tool_calls,
            "快照保留覆盖原始声明（溯源）"
        );
    }

    #[test]
    fn override_can_narrow_static_capabilities() {
        let mut registry = ModelRegistry::empty();
        registry
            .try_register(test_entry(
                "m1",
                "p",
                caps(true, false, true, false, false, false, false),
            ))
            .expect("register");
        registry.set_override("m1", caps(true, false, false, false, false, false, false));

        let merged = registry.capability_evidence("m1").unwrap().merged();
        assert!(merged.text);
        assert!(!merged.tool_calls, "覆盖可收窄静态声明");
    }

    #[test]
    fn override_read_write_roundtrip_is_case_insensitive() {
        let registry = ModelRegistry::empty();
        let full = caps(true, false, true, false, false, false, false);
        registry.set_override("MiXeD", full.clone());
        assert_eq!(registry.override_for("mixed"), Some(full.clone()));
        assert_eq!(registry.overrides().len(), 1);
        assert!(registry.remove_override("MIXED"));
        assert_eq!(registry.override_for("mixed"), None);
        assert!(!registry.remove_override("mixed"));
    }

    #[test]
    fn probe_merges_with_static_and_cannot_amplify() {
        let registry = ModelRegistry::builtin();
        // gpt-4o 静态声明 thinking=false；探测声明 thinking=true。
        registry.record_probe(
            &ProviderId::new("openai"),
            vec![mock_definition(
                "gpt-4o",
                caps(true, true, true, true, true, true, true),
            )],
        );
        let evidence = registry.capability_evidence("gpt-4o").unwrap();
        assert!(!evidence.merged().thinking, "探测不能放大静态未支持的能力");
        assert!(evidence.merged().tool_calls);
        assert!(
            evidence.probe_declared.as_ref().unwrap().thinking,
            "快照保留探测原始声明"
        );
    }

    #[test]
    fn evidence_resolves_via_alias() {
        let registry = ModelRegistry::builtin();
        let evidence = registry.capability_evidence("gpt4o").expect("别名可解析");
        assert_eq!(evidence.model.as_str(), "gpt-4o");
        assert_eq!(
            evidence.provider.as_ref().map(ProviderId::as_str),
            Some("openai")
        );
        assert!(evidence.merged().tool_calls);
    }

    #[test]
    fn evidence_for_unknown_model_is_none() {
        let registry = ModelRegistry::builtin();
        assert!(registry.capability_evidence("nonexistent").is_none());
        assert!(registry
            .capability_snapshot()
            .iter()
            .all(|evidence| evidence.model.as_str() != "nonexistent"));
    }

    #[test]
    fn capability_snapshot_covers_all_three_sources() {
        let registry = ModelRegistry::builtin();
        registry.record_probe(
            &ProviderId::new("openai"),
            vec![
                mock_definition("gpt-4o", caps(true, true, true, true, false, true, true)),
                mock_definition(
                    "probe-only",
                    caps(true, false, false, false, false, false, false),
                ),
            ],
        );
        registry.set_override("gpt-4o", caps(true, true, true, true, false, true, true));
        registry.set_override(
            "override-only",
            caps(true, false, false, false, false, false, false),
        );

        let snapshot = registry.capability_snapshot();
        assert!(
            snapshot
                .windows(2)
                .all(|pair| pair[0].model <= pair[1].model),
            "快照按 model id 排序"
        );

        let gpt4o = snapshot
            .iter()
            .find(|evidence| evidence.model.as_str() == "gpt-4o")
            .expect("三源齐全");
        assert!(gpt4o.static_declared.is_some());
        assert!(gpt4o.probe_declared.is_some());
        assert!(gpt4o.override_declared.is_some());
        assert_eq!(
            gpt4o.provider.as_ref().map(ProviderId::as_str),
            Some("openai")
        );

        let probe_only = snapshot
            .iter()
            .find(|evidence| evidence.model.as_str() == "probe-only")
            .expect("仅探测证据");
        assert!(probe_only.static_declared.is_none());
        assert!(probe_only.probe_declared.is_some());
        assert_eq!(
            probe_only.provider.as_ref().map(ProviderId::as_str),
            Some("openai"),
            "探测锚定 provider"
        );

        let override_only = snapshot
            .iter()
            .find(|evidence| evidence.model.as_str() == "override-only")
            .expect("仅覆盖证据");
        assert!(override_only.static_declared.is_none());
        assert!(override_only.probe_declared.is_none());
        assert!(override_only.override_declared.is_some());
        assert!(override_only.provider.is_none(), "无静态/探测锚点");
    }

    #[test]
    fn cloned_registry_has_independent_override_but_shared_probe_cache() {
        let registry = ModelRegistry::builtin();
        let clone = registry.clone();
        registry.set_override(
            "gpt-4o",
            caps(false, false, false, false, false, false, false),
        );
        assert!(clone.override_for("gpt-4o").is_none(), "覆盖表深拷贝");
        registry.record_probe(
            &ProviderId::new("openai"),
            vec![mock_definition(
                "probe-only",
                caps(true, false, false, false, false, false, false),
            )],
        );
        assert!(
            clone
                .capability_evidence("probe-only")
                .unwrap()
                .probe_declared
                .is_some(),
            "探测缓存跨克隆共享（结果幂等）"
        );
    }

    #[tokio::test]
    async fn probe_cached_per_provider_once() {
        let registry = ModelRegistry::empty();
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = MockProvider {
            id: ProviderId::new("mock"),
            calls: calls.clone(),
            result: Ok(vec![mock_definition(
                "mock-model",
                caps(true, false, false, false, false, false, false),
            )]),
            yield_before: false,
        };
        let first = registry
            .probe_provider(&provider, None)
            .await
            .expect("首次探测成功");
        let second = registry
            .probe_provider(&provider, None)
            .await
            .expect("缓存命中");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "同一 provider 只发现一次");
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn distinct_providers_are_discovered_separately() {
        let registry = ModelRegistry::empty();
        let calls_a = Arc::new(AtomicUsize::new(0));
        let calls_b = Arc::new(AtomicUsize::new(0));
        let provider_a = MockProvider {
            id: ProviderId::new("mock-a"),
            calls: calls_a.clone(),
            result: Ok(vec![mock_definition(
                "model-a",
                caps(true, false, false, false, false, false, false),
            )]),
            yield_before: false,
        };
        let provider_b = MockProvider {
            id: ProviderId::new("mock-b"),
            calls: calls_b.clone(),
            result: Ok(vec![mock_definition(
                "model-b",
                caps(true, false, false, false, false, false, false),
            )]),
            yield_before: false,
        };
        registry.probe_provider(&provider_a, None).await.unwrap();
        registry.probe_provider(&provider_b, None).await.unwrap();
        assert_eq!(calls_a.load(Ordering::SeqCst), 1);
        assert_eq!(calls_b.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_probes_discover_once_per_provider() {
        let registry = Arc::new(ModelRegistry::empty());
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(MockProvider {
            id: ProviderId::new("mock"),
            calls: calls.clone(),
            result: Ok(vec![mock_definition(
                "mock-model",
                caps(true, false, false, false, false, false, false),
            )]),
            yield_before: true,
        });
        let mut handles = Vec::new();
        for _ in 0..8 {
            let registry = Arc::clone(&registry);
            let provider = Arc::clone(&provider);
            handles.push(tokio::spawn(async move {
                registry.probe_provider(provider.as_ref(), None).await
            }));
        }
        for handle in handles {
            let probe = handle.await.expect("任务未 panic").expect("探测成功");
            assert!(probe.contains("mock-model"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "并发探测仍只发现一次");
    }

    #[tokio::test]
    async fn failed_probe_is_cached_without_retry() {
        let registry = ModelRegistry::empty();
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = MockProvider {
            id: ProviderId::new("mock"),
            calls: calls.clone(),
            result: Err(ProviderError::new(ProviderErrorKind::Network, "boom")),
            yield_before: false,
        };
        let first = registry
            .probe_provider(&provider, None)
            .await
            .expect_err("探测失败");
        let second = registry
            .probe_provider(&provider, None)
            .await
            .expect_err("失败同样缓存");
        assert_eq!(first, second);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "失败不反复重试");
        assert!(registry.clear_probe(&ProviderId::new("mock")));
    }

    #[tokio::test]
    async fn record_probe_and_clear_probe_roundtrip() {
        let registry = ModelRegistry::empty();
        registry.record_probe(
            &ProviderId::new("mock"),
            vec![mock_definition(
                "m",
                caps(true, false, false, false, false, false, false),
            )],
        );
        assert!(registry
            .capability_evidence("m")
            .unwrap()
            .probe_declared
            .is_some());
        assert!(registry.clear_probe(&ProviderId::new("mock")));
        assert!(!registry.clear_probe(&ProviderId::new("mock")));
        assert!(registry.capability_evidence("m").is_none());

        // 清除后重新探测。
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = MockProvider {
            id: ProviderId::new("mock"),
            calls: calls.clone(),
            result: Ok(vec![mock_definition(
                "m",
                caps(true, false, false, false, false, false, false),
            )]),
            yield_before: false,
        };
        registry.probe_provider(&provider, None).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn in_flight_record_probe_wins_and_callers_share_pinned_cache() {
        let registry = Arc::new(ModelRegistry::empty());
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let provider = Arc::new(BlockingProvider {
            id: ProviderId::new("mock"),
            started: started.clone(),
            release: release.clone(),
            result: Ok(vec![mock_definition(
                "stale",
                caps(true, false, false, false, false, false, false),
            )]),
        });

        // owner 认领探测并进入 list_models（槽位 InFlight）。
        let owner = tokio::spawn({
            let registry = Arc::clone(&registry);
            let provider = Arc::clone(&provider);
            async move { registry.probe_provider(provider.as_ref(), None).await }
        });
        for _ in 0..1_000_000 {
            if started.load(Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(started.load(Ordering::SeqCst), "owner 未进入 list_models");

        // 并发 waiter：探测进行中等待，随后应读到固定结果。
        let waiter = tokio::spawn({
            let registry = Arc::clone(&registry);
            let provider = Arc::clone(&provider);
            async move { registry.probe_provider(provider.as_ref(), None).await }
        });
        tokio::task::yield_now().await;

        // 探测进行中强制记录新结果（last-write-wins：固定 + 唤醒等待者）。
        let pinned = registry.record_probe(
            &ProviderId::new("mock"),
            vec![mock_definition(
                "fresh",
                caps(true, true, true, true, false, true, true),
            )],
        );
        // 放行 owner；其迟到结果不得覆盖已固定结果。
        release.store(true, Ordering::SeqCst);

        let owner_probe = owner.await.expect("owner 任务未 panic").expect("探测成功");
        let waiter_probe = waiter
            .await
            .expect("waiter 任务未 panic")
            .expect("探测成功");
        assert!(
            Arc::ptr_eq(&pinned, &owner_probe),
            "in-flight record_probe 胜出：owner 与调用者共享同一固定缓存"
        );
        assert!(
            Arc::ptr_eq(&pinned, &waiter_probe),
            "等待者被唤醒后同样读到固定结果"
        );
        assert!(owner_probe.contains("fresh"));
        assert!(registry.capability_evidence("fresh").is_some());
        assert!(
            registry.capability_evidence("stale").is_none(),
            "owner 的迟到结果不得写入缓存"
        );
    }

    #[test]
    fn wait_for_probe_dedups_repeated_poll_of_same_waker() {
        let slot = ProbeSlot::default();
        assert!(matches!(slot.try_claim(), ClaimOutcome::Won));

        let mut wait = WaitForProbe { slot: &slot };
        let waker = std::task::Waker::noop();
        eprintln!(
            "DEBUG test clone_will_wake={} data={:p} clone_data={:p}",
            waker.will_wake(&waker.clone()),
            waker.data(),
            waker.clone().data()
        );
        let mut cx = Context::from_waker(&waker);
        for _ in 0..3 {
            assert!(Pin::new(&mut wait).poll(&mut cx).is_pending());
        }
        let registered = match &*lock(&slot.state) {
            ProbeState::InFlight { wakers } => wakers.len(),
            _ => panic!("槽位应处于 InFlight"),
        };
        assert_eq!(registered, 1, "同一 waker 重复 poll 不得累积登记");
    }

    #[test]
    fn merge_provider_catalog_feeds_v2_caps_into_evidence_without_name_match() {
        let mut registry = ModelRegistry::builtin();
        let v2 = ModelCapabilities {
            text: true,
            image_input: true,
            tool_calls: true,
            parallel_tool_calls: true,
            thinking: true,
            structured_output: true,
            prompt_cache: true,
            transport: provider_api::ModelTransport::Responses,
            citations: true,
            ..ModelCapabilities::default()
        };
        let source: Vec<(ProviderId, Vec<ModelDefinition>)> = vec![(
            ProviderId::new("openai"),
            vec![mock_definition("gpt-4o", v2.clone())],
        )];
        registry.merge_provider_source(&source);
        let evidence = registry.capability_evidence("gpt-4o").unwrap();
        let static_caps = evidence.static_declared.expect("static after merge");
        assert_eq!(
            static_caps.transport,
            provider_api::ModelTransport::Responses
        );
        assert!(static_caps.citations);
        assert_eq!(
            evidence.provider.as_ref().map(ProviderId::as_str),
            Some("openai")
        );

        // Cross-provider id collision is skipped (no steal, no name match).
        let other: Vec<(ProviderId, Vec<ModelDefinition>)> = vec![(
            ProviderId::new("other"),
            vec![mock_definition(
                "gpt-4o",
                ModelCapabilities {
                    text: true,
                    citations: false,
                    ..ModelCapabilities::default()
                },
            )],
        )];
        registry.merge_provider_source(&other);
        let after = registry
            .capability_evidence("gpt-4o")
            .unwrap()
            .static_declared
            .unwrap();
        assert!(after.citations, "foreign provider must not overwrite");

        // Unknown model from a factory is appended.
        let extra: Vec<(ProviderId, Vec<ModelDefinition>)> = vec![(
            ProviderId::new("factory-a"),
            vec![mock_definition("factory-only-model", v2)],
        )];
        registry.merge_provider_source(&extra);
        let added = registry.capability_evidence("factory-only-model").unwrap();
        assert_eq!(
            added.provider.as_ref().map(ProviderId::as_str),
            Some("factory-a")
        );
        assert_eq!(
            added.static_declared.unwrap().transport,
            provider_api::ModelTransport::Responses
        );
    }
}
