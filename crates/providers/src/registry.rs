//! 模型目录、别名解析、能力过滤、上下文校验、费用估算与三源能力证据。
//!
//! 迁自 V1 `model-registry` 整包机制：模型能力来自三处——(a) 目录静态声明
//! （`entries`）、(b) Provider 探测（`probe_provider` / `record_probe`，
//! 同一 provider 只发现一次，线程安全、不持锁跨 await）、(c) 配置覆盖
//! （`set_override`）。三源以 provider-neutral 的 `ModelCapabilities` 表达，
//! 合并取交集（覆盖只能收窄、不能放大）；「请求 × 支持」的最终交集由
//! [`crate::negotiate::CapabilityNegotiator`] 消费 [`CapabilityEvidence`]
//! 快照完成。

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

use pawork_domain::{Cost, ModelId, ProviderId, TokenUsage};
use pawork_domain::{ModelCapabilities, ModelDefinition, ModelProvider, ResolvedCredential};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::pricing::{estimate_cost, ModelPricing};
use crate::RegistryError;

/// 目录中的单个模型条目。比 `ModelDefinition` 多了 provider、定价与别名。
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
    /// 转换为 Provider 协议的 `ModelDefinition`（丢弃 provider/定价/别名）。
    pub fn to_definition(&self) -> ModelDefinition {
        ModelDefinition {
            id: self.id.clone(),
            display_name: self.display_name.clone(),
            context_window_tokens: self.context_window_tokens,
            max_output_tokens: self.max_output_tokens,
            capabilities: self.capabilities.clone(),
        }
    }
}

/// 能力证据来源。优先级：`Static < Probe < Override`。
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

/// 单个模型的三源能力证据快照（供 CapabilityNegotiator 消费）。
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
    /// 这是「证据层」合并，不等同于协商：与请求能力的最终交集由 negotiate 完成。
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

/// Narrow hook：Provider factory `builtin_models()` 目录经 `caps()` /
/// 协商证据进入 registry，Core 不做 Provider 名称匹配。
pub trait ProviderCapabilitySource: Send + Sync {
    /// `(provider_id, builtin_models)` 对。调用方提供 id；本 crate 绝不按
    /// Provider 名称字符串分支。
    fn provider_catalogs(&self) -> Vec<(ProviderId, Vec<ModelDefinition>)>;
}

impl ProviderCapabilitySource for Vec<(ProviderId, Vec<ModelDefinition>)> {
    fn provider_catalogs(&self) -> Vec<(ProviderId, Vec<ModelDefinition>)> {
        self.clone()
    }
}

/// 三来源保守合并：present 来源逐字段取交集，来源缺失不约束。
///
/// 在 serde Value 层面做字段级合并，自动覆盖 pawork-domain/provider_api 同期新增的 v2
/// 能力字段（bool 取 AND、数组取元素交集、其它字段全部来源相等才保留，
/// 冲突即 fail-closed 移除该键取字段默认值），后续字段落地时无需改动本函数。
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
    // `hosted_tool_tags` 的空集合会被 serde skip 掉，不能把缺键误判为
    // 「未声明」。present 的每个证据来源都显式声明完整 ModelCapabilities，
    // 因此这里按真实集合再做一次交集，让空集合能够收窄为不支持。
    let hosted_tool_tags =
        sources[1..]
            .iter()
            .fold(sources[0].hosted_tool_tags.clone(), |current, source| {
                current
                    .intersection(&source.hosted_tool_tags)
                    .cloned()
                    .collect()
            });

    // 反序列化失败（如新字段未带 serde(default)）时整体降级为「全部不支持」
    // （fail-closed），不放大任何能力。
    let mut merged: ModelCapabilities =
        serde_json::from_value(Value::Object(acc)).unwrap_or_default();
    merged.hosted_tool_tags = hosted_tool_tags;
    merged
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
    /// 配置覆盖：model id -> 能力声明。线程安全读写。
    overrides: Mutex<BTreeMap<ModelId, ModelCapabilities>>,
    /// Provider 探测缓存：provider id -> 探测槽位。同一 provider 只探测一次；
    /// 并发调用共享槽位，不持锁跨 await。
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

    /// 创建带内置目录的注册表（S5 起为双开发通道条目，见 `builtin_entries`）。
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

    /// 注册并在别名 / 真实 model id 命名空间冲突时返回错误（预检后写入）。
    pub fn try_register(&mut self, entry: CatalogEntry) -> Result<(), RegistryError> {
        let normalized_id = normalized_model_id(&entry.id);
        if let Some(existing) = self.alias_to_id.get(normalized_id.as_str()) {
            if *existing != normalized_id {
                return Err(RegistryError::DuplicateAlias {
                    alias: entry.id.to_string(),
                    existing: existing.to_string(),
                });
            }
        }
        for alias in &entry.aliases {
            let normalized_alias = alias.to_ascii_lowercase();
            let alias_as_id = ModelId::new(&normalized_alias);
            if self.entries.contains_key(&alias_as_id) && alias_as_id != normalized_id {
                return Err(RegistryError::DuplicateAlias {
                    alias: alias.clone(),
                    existing: alias_as_id.to_string(),
                });
            }
            if let Some(existing) = self.alias_to_id.get(&normalized_alias) {
                if *existing != normalized_id {
                    return Err(RegistryError::DuplicateAlias {
                        alias: alias.clone(),
                        existing: existing.to_string(),
                    });
                }
            }
        }
        // 替换同 id 时，旧别名必须失效；否则 `aliases` 并非新条目的权威集合。
        self.alias_to_id
            .retain(|_, mapped_id| mapped_id != &normalized_id);
        for alias in &entry.aliases {
            self.alias_to_id
                .insert(alias.to_ascii_lowercase(), normalized_id.clone());
        }
        self.entries.insert(normalized_id, entry);
        Ok(())
    }

    /// 合并 provider 动态发现或用户自定义的模型；同 id 覆盖、别名以新条目为准。
    pub fn extend_with(&mut self, entries: Vec<CatalogEntry>) {
        for entry in entries {
            // 覆盖语义：同 id 直接替换；别名以新条目为准（覆盖旧映射）。
            let normalized_id = normalized_model_id(&entry.id);
            self.alias_to_id.retain(|alias, mapped_id| {
                mapped_id != &normalized_id && alias != normalized_id.as_str()
            });
            for alias in &entry.aliases {
                let normalized_alias = alias.to_ascii_lowercase();
                // 无 Result 的动态覆盖路径里，真实 model id 始终优先；与其它
                // 真实 id 冲突的别名直接忽略，避免静默劫持。
                let alias_as_id = ModelId::new(&normalized_alias);
                if !self.entries.contains_key(&alias_as_id) || alias_as_id == normalized_id {
                    self.alias_to_id
                        .insert(normalized_alias, normalized_id.clone());
                }
            }
            self.entries.insert(normalized_id, entry);
        }
    }

    /// 把 factory `builtin_models()` 目录合并进静态目录，从而进入
    /// [`CapabilityEvidence::static_declared`] 证据。
    ///
    /// 仅按 `ProviderId` 相等性查找——Core 不做 Provider 名称匹配/分支。
    /// 跨 provider 的 id 冲突跳过（fail-closed）。新模型追加，不带定价/别名
    /// （二者归目录所有）。
    pub fn merge_provider_source(&mut self, source: &dyn ProviderCapabilitySource) {
        for (provider, models) in source.provider_catalogs() {
            self.merge_provider_models(&provider, &models);
        }
    }

    /// 合并单个 provider 的 `builtin_models()` 进静态目录。
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
        // 真实 model id 优先，防止别名遮蔽另一个模型并导致错误定价 / 能力选择。
        if let Some(entry) = self.entries.get(&ModelId::new(&normalized)) {
            return Some(entry);
        }
        let id = self.alias_to_id.get(&normalized).cloned();
        if let Some(id) = id {
            return self.entries.get(&id);
        }
        None
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

    /// 按定价估算费用；模型未注册或无定价时返回 `None`（不编造费用）。
    pub fn estimate_cost(&self, id_or_alias: &str, usage: &TokenUsage) -> Option<Cost> {
        let entry = self.resolve(id_or_alias)?;
        let pricing = entry.pricing.as_ref()?;
        Some(estimate_cost(usage, pricing))
    }

    // ---------- 三源能力证据 ----------

    /// 配置覆盖写入（来源 Override）：model -> capabilities。
    ///
    /// 覆盖只能收窄、不能放大：`merged()` 取交集，覆盖声明的能力若静态/探测
    /// 未支持，最终合并结果仍为不支持。model id ASCII 大小写不敏感，合法
    /// 别名会先解析为真实 model id；未知 id 仍允许建立 override-only 证据。
    pub fn set_override(&self, model: impl AsRef<str>, capabilities: ModelCapabilities) {
        let model = self.canonical_model_id(model.as_ref());
        lock(&self.overrides).insert(model, capabilities);
    }

    /// 移除配置覆盖；返回是否确实存在该覆盖。
    pub fn remove_override(&self, model: impl AsRef<str>) -> bool {
        let model = self.canonical_model_id(model.as_ref());
        lock(&self.overrides).remove(&model).is_some()
    }

    /// 读取配置覆盖；无覆盖返回 `None`。
    pub fn override_for(&self, model: &str) -> Option<ModelCapabilities> {
        let model = self.canonical_model_id(model);
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
        // 别名解析后，probe / override 必须按真实 model id 查询；否则通过
        // `glm` 等别名只能看到静态证据，会悄然丢失另外两层来源。
        let evidence_key = normalized_model_id(&model_id);
        let static_declared = entry.map(|entry| entry.capabilities.clone());
        let (probe_provider, probe_declared) = match entry {
            // 静态锚定：只查询该 provider 的探测缓存。
            Some(entry) => (
                Some(entry.provider.clone()),
                self.probe_capabilities_for(&entry.provider, &evidence_key),
            ),
            // 无静态条目：按 provider id 排序扫描已缓存探测。
            None => self.probe_capabilities_any(&evidence_key),
        };
        let override_declared = self.override_for(evidence_key.as_str());
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

    fn canonical_model_id(&self, model: &str) -> ModelId {
        self.resolve(model)
            .map(|entry| normalized_model_id(&entry.id))
            .unwrap_or_else(|| normalized_model_id(&ModelId::new(model)))
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

    /// 强制固定结果（`record_probe` 路径，last-write-wins）：无论槽位处于
    /// `Idle` / `InFlight` / `Done` 都以新结果为准，原 `InFlight` 的等待者
    /// 被唤醒后读到新结果。
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

    /// Claim 完成（`probe_provider` owner 路径）：仅当槽位仍为自己的
    /// `InFlight` 时提交结果并唤醒等待者，返回 `true`；若已被强制固定为
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
    // v2：citations（required 为 true 时 have 必须声明）。
    if required.citations && !have.citations {
        return false;
    }
    // v2：transport——required 声明非默认（非 ChatCompletions）transport 时，
    // have 必须声明同一 transport。required 为默认 ChatCompletions 视为「不约束」。
    if required.transport != pawork_domain::ModelTransport::ChatCompletions
        && have.transport != required.transport
    {
        return false;
    }
    // v2：hosted tool 标签——required 中的每个标签 have 必须包含（子集）。
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
//
// pawork-domain/provider_api 同期新增 v2 能力字段时用 ..Default 兼容，避免构造点编译失败；
// v2 目录经 `ModelRegistry::merge_provider_models` / `ProviderCapabilitySource`
// 合并，而不是扩展本函数参数表。
#[allow(clippy::too_many_arguments)]
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
        ..Default::default()
    }
}

/// 内置目录（S5 起为两条开发通道；S6 波 C 增补 qwen/deepseek 聚合条目）：
///
/// - `glm-5.2`（GLM Coding Plan）：订阅制通道，无公开 per-token 费率——
///   不伪造定价（pricing = None，费用显示为「无定价」）。
/// - `deepseek-v4-pro`（OpenCode Go）：公开费率 input $0.435/M、
///   output $0.87/M、cache read $0.003625/M；cache write 未单列，按 0 计。
/// - `qwen3.8-max`（Qwen Token Plan 当前目录）与 `deepseek-chat` /
///   `deepseek-reasoner`（DeepSeek API key 通道）：窗口/输出取公开文档
///   保守值；费率未核对到 micros 前不编造（pricing = None）。
/// - ChatGPT / xAI 为 OAuth 通道：ChatGPT 目录登录后经 /models 探测，不
///   维护静态条目；xai 静态目录由 adapter 的 builtin_models 在装配期合并。
///
/// 能力声明取保守基线（text + tool_calls）；其余维度由 Provider 探测与
/// 配置覆盖收窄。本地兼容服务的模型在连接后经 `extend_with` 动态补充。
fn builtin_entries() -> Vec<CatalogEntry> {
    let text_tools = caps(true, false, true, false, false, false, false);

    vec![
        CatalogEntry {
            id: ModelId::new("glm-5.2"),
            provider: ProviderId::new("glm-coding"),
            display_name: "GLM 5.2".into(),
            context_window_tokens: 1_000_000,
            max_output_tokens: 131_072,
            capabilities: text_tools.clone(),
            pricing: None,
            aliases: vec!["glm".into()],
        },
        CatalogEntry {
            id: ModelId::new("deepseek-v4-pro"),
            provider: ProviderId::new("opencode-go"),
            display_name: "DeepSeek V4 Pro".into(),
            context_window_tokens: 1_000_000,
            max_output_tokens: 393_216,
            capabilities: text_tools.clone(),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 435_000,
                output_per_mtoken_micros: 870_000,
                cache_read_per_mtoken_micros: 3_625,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["deepseek".into()],
        },
        CatalogEntry {
            id: ModelId::new("qwen3.8-max"),
            provider: ProviderId::new("qwen-token-plan"),
            display_name: "Qwen3.8 Max".into(),
            context_window_tokens: 0,
            max_output_tokens: 0,
            capabilities: text_tools.clone(),
            pricing: None,
            aliases: Vec::new(),
        },
        CatalogEntry {
            id: ModelId::new("deepseek-chat"),
            provider: ProviderId::new("deepseek"),
            display_name: "DeepSeek Chat".into(),
            context_window_tokens: 128_000,
            max_output_tokens: 8_192,
            capabilities: text_tools.clone(),
            pricing: None,
            aliases: Vec::new(),
        },
        CatalogEntry {
            id: ModelId::new("deepseek-reasoner"),
            provider: ProviderId::new("deepseek"),
            display_name: "DeepSeek Reasoner".into(),
            context_window_tokens: 128_000,
            max_output_tokens: 64_000,
            capabilities: text_tools.clone(),
            pricing: None,
            aliases: Vec::new(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use async_trait::async_trait;
    use pawork_domain::CancellationToken;
    use pawork_domain::{
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
        assert!(registry.resolve("glm-5.2").is_some());
        assert_eq!(registry.resolve("GLM-5.2"), registry.resolve("glm-5.2"));
        assert!(registry.resolve("glm").is_some(), "别名须可解析");
        assert_eq!(registry.resolve("GLM"), registry.resolve("glm"));
        assert!(registry.resolve("deepseek").is_some());
        assert!(registry.resolve("nonexistent").is_none());
        assert_eq!(
            registry.list().len(),
            5,
            "S6 内置目录覆盖五条 API-key 通道静态条目"
        );
    }

    #[test]
    fn builtin_catalog_carries_channel_windows_and_pricing() {
        let registry = ModelRegistry::builtin();

        let glm = registry.resolve("glm-5.2").expect("glm-5.2 in builtin");
        assert_eq!(glm.context_window_tokens, 1_000_000);
        assert_eq!(glm.max_output_tokens, 131_072);
        assert!(
            glm.pricing.is_none(),
            "GLM Coding Plan 订阅制无公开 per-token 费率，不伪造定价"
        );
        assert_eq!(glm.provider, ProviderId::new("glm-coding"));

        let deepseek = registry
            .resolve("deepseek-v4-pro")
            .expect("deepseek-v4-pro in builtin");
        assert_eq!(deepseek.context_window_tokens, 1_000_000);
        assert_eq!(deepseek.max_output_tokens, 393_216);
        assert_eq!(deepseek.provider, ProviderId::new("opencode-go"));
        let pricing = deepseek.pricing.as_ref().expect("公开费率条目");
        assert_eq!(pricing.input_per_mtoken_micros, 435_000);
        assert_eq!(pricing.output_per_mtoken_micros, 870_000);
        assert_eq!(pricing.cache_read_per_mtoken_micros, 3_625);
        assert_eq!(pricing.cache_write_per_mtoken_micros, 0);
        assert_eq!(pricing.currency, "USD");
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
    fn aliases_cannot_shadow_real_model_ids_in_either_direction() {
        let mut registry = ModelRegistry::empty();
        let mut alpha = test_entry(
            "alpha",
            "p",
            caps(true, false, false, false, false, false, false),
        );
        alpha.aliases = vec!["shared".into()];
        registry.try_register(alpha).expect("register alpha");

        assert!(matches!(
            registry.try_register(test_entry(
                "shared",
                "p",
                caps(true, false, false, false, false, false, false),
            )),
            Err(RegistryError::DuplicateAlias { .. })
        ));

        let mut beta = test_entry(
            "beta",
            "p",
            caps(true, false, false, false, false, false, false),
        );
        beta.aliases = vec!["ALPHA".into()];
        assert!(matches!(
            registry.try_register(beta),
            Err(RegistryError::DuplicateAlias { .. })
        ));
        assert_eq!(
            registry.resolve("alpha").map(|entry| entry.id.as_str()),
            Some("alpha")
        );
        assert_eq!(
            registry.resolve("shared").map(|entry| entry.id.as_str()),
            Some("alpha")
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
        let mut registry = ModelRegistry::builtin();
        registry.extend_with(vec![test_entry(
            "tiny-text",
            "local",
            caps(true, false, false, false, false, false, false),
        )]);
        let with_tools = caps(true, false, true, false, false, false, false);
        let filtered: Vec<ModelId> = registry
            .filter(&with_tools)
            .into_iter()
            .map(|entry| entry.id.clone())
            .collect();
        assert!(filtered.contains(&ModelId::new("glm-5.2")));
        assert!(filtered.contains(&ModelId::new("deepseek-v4-pro")));
        // tiny-text 无工具能力，应被排除。
        assert!(!filtered.contains(&ModelId::new("tiny-text")));
    }

    #[test]
    fn context_validation_respects_window() {
        let registry = ModelRegistry::builtin();
        assert!(registry.validate_context("glm-5.2", 1_000_000));
        assert!(!registry.validate_context("glm-5.2", 1_000_001));
        assert!(!registry.validate_context("unknown-model", 10));
    }

    #[test]
    fn cost_estimate_matches_manual_integer_math() {
        let registry = ModelRegistry::builtin();
        let usage = TokenUsage {
            input_tokens: 2_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 1_000_000,
            cache_write_tokens: 0,
        };
        let cost = registry
            .estimate_cost("deepseek-v4-pro", &usage)
            .expect("有定价的模型可估算");
        assert_eq!(cost.currency, "USD");
        // 2M input + 1M output + 1M cache read = $0.87 + $0.87 + $0.003625。
        assert_eq!(cost.amount_micros, 1_743_625);
    }

    #[test]
    fn extend_with_overrides_same_id() {
        let mut registry = ModelRegistry::builtin();
        let discovered = vec![CatalogEntry {
            id: ModelId::new("deepseek-v4-pro"),
            provider: ProviderId::new("opencode-go"),
            display_name: "DeepSeek V4 Pro (discovered)".into(),
            context_window_tokens: 512_000,
            max_output_tokens: 65_536,
            capabilities: caps(true, false, true, false, false, false, false),
            pricing: Some(ModelPricing {
                input_per_mtoken_micros: 400_000,
                output_per_mtoken_micros: 800_000,
                cache_read_per_mtoken_micros: 0,
                cache_write_per_mtoken_micros: 0,
                currency: "USD".into(),
            }),
            aliases: vec!["deepseek".into()],
        }];
        registry.extend_with(discovered);

        let entry = registry.resolve("deepseek-v4-pro").expect("覆盖后仍可解析");
        assert_eq!(entry.provider, ProviderId::new("opencode-go"));
        assert_eq!(entry.context_window_tokens, 512_000, "动态发现覆盖窗口");
        assert!(entry.pricing.is_some(), "动态发现的定价覆盖内置定价");
        assert_eq!(
            registry.resolve("deepseek").map(|entry| entry.id.clone()),
            Some(ModelId::new("deepseek-v4-pro"))
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
        // GLM Coding Plan 订阅制无公开费率：不编造费用。
        assert!(registry.estimate_cost("glm-5.2", &usage).is_none());
    }

    // ---------- 三源能力证据 ----------

    #[test]
    fn capability_source_priority_is_static_then_probe_then_override() {
        assert!(CapabilitySource::Static < CapabilitySource::Probe);
        assert!(CapabilitySource::Probe < CapabilitySource::Override);
    }

    #[test]
    fn caps_satisfied_v2_citations_and_transport_and_tools() {
        let full = ModelCapabilities {
            citations: true,
            transport: pawork_domain::ModelTransport::Responses,
            hosted_tool_tags: [pawork_domain::ToolCapabilityTag::WebSearch]
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
            transport: pawork_domain::ModelTransport::Responses,
            ..ModelCapabilities::default()
        };
        assert!(caps_satisfied(&full, &req_responses));
        let req_messages = ModelCapabilities {
            transport: pawork_domain::ModelTransport::Messages,
            ..ModelCapabilities::default()
        };
        assert!(
            !caps_satisfied(&full, &req_messages),
            "要求 Messages 但模型只声明 Responses → 不满足"
        );

        // 要求 hosted tool WebSearch：包含即满足；要求 CodeExecution 不满足。
        let req_tool = ModelCapabilities {
            hosted_tool_tags: [pawork_domain::ToolCapabilityTag::WebSearch]
                .into_iter()
                .collect(),
            ..ModelCapabilities::default()
        };
        assert!(caps_satisfied(&full, &req_tool));
        let req_tool_missing = ModelCapabilities {
            hosted_tool_tags: [pawork_domain::ToolCapabilityTag::CodeExecution]
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
    fn merge_allows_empty_tool_tags_to_narrow_capabilities() {
        let declared = ModelCapabilities {
            hosted_tool_tags: [pawork_domain::ToolCapabilityTag::WebSearch]
                .into_iter()
                .collect(),
            ..ModelCapabilities::default()
        };
        let narrowed = ModelCapabilities::default();

        let merged = merge_capabilities(&[&declared, &narrowed]);
        assert!(
            merged.hosted_tool_tags.is_empty(),
            "空集合证据必须把 hosted tool 能力收窄为空"
        );
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
    fn override_alias_resolves_to_the_canonical_model_id() {
        let registry = ModelRegistry::builtin();
        let narrowed = caps(true, false, false, false, false, false, false);

        registry.set_override("GLM", narrowed.clone());
        assert_eq!(registry.override_for("glm-5.2"), Some(narrowed));
        assert!(registry.remove_override("glm"));
        assert_eq!(registry.override_for("glm-5.2"), None);
        assert!(registry
            .overrides()
            .iter()
            .all(|(model, _)| model.as_str() != "glm"));
    }

    #[test]
    fn probe_merges_with_static_and_cannot_amplify() {
        let registry = ModelRegistry::builtin();
        // glm-5.2 静态声明 thinking=false；探测声明 thinking=true。
        registry.record_probe(
            &ProviderId::new("glm-coding"),
            vec![mock_definition(
                "glm-5.2",
                caps(true, true, true, true, true, true, true),
            )],
        );
        let evidence = registry.capability_evidence("glm-5.2").unwrap();
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
        registry.record_probe(
            &ProviderId::new("glm-coding"),
            vec![mock_definition(
                "glm-5.2",
                caps(true, false, true, false, false, false, false),
            )],
        );
        registry.set_override(
            "glm-5.2",
            caps(true, false, true, false, false, false, false),
        );
        let evidence = registry.capability_evidence("glm").expect("别名可解析");
        assert_eq!(evidence.model.as_str(), "glm-5.2");
        assert_eq!(
            evidence.provider.as_ref().map(ProviderId::as_str),
            Some("glm-coding")
        );
        assert!(evidence.probe_declared.is_some(), "别名查询保留 probe 证据");
        assert!(
            evidence.override_declared.is_some(),
            "别名查询保留 override 证据"
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
            &ProviderId::new("glm-coding"),
            vec![
                mock_definition("glm-5.2", caps(true, true, true, true, false, true, true)),
                mock_definition(
                    "probe-only",
                    caps(true, false, false, false, false, false, false),
                ),
            ],
        );
        registry.set_override("glm-5.2", caps(true, true, true, true, false, true, true));
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

        let glm = snapshot
            .iter()
            .find(|evidence| evidence.model.as_str() == "glm-5.2")
            .expect("三源齐全");
        assert!(glm.static_declared.is_some());
        assert!(glm.probe_declared.is_some());
        assert!(glm.override_declared.is_some());
        assert_eq!(
            glm.provider.as_ref().map(ProviderId::as_str),
            Some("glm-coding")
        );

        let probe_only = snapshot
            .iter()
            .find(|evidence| evidence.model.as_str() == "probe-only")
            .expect("仅探测证据");
        assert!(probe_only.static_declared.is_none());
        assert!(probe_only.probe_declared.is_some());
        assert_eq!(
            probe_only.provider.as_ref().map(ProviderId::as_str),
            Some("glm-coding"),
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
            "glm-5.2",
            caps(false, false, false, false, false, false, false),
        );
        assert!(clone.override_for("glm-5.2").is_none(), "覆盖表深拷贝");
        registry.record_probe(
            &ProviderId::new("glm-coding"),
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
        let waker = deterministic_waker();
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

    /// 构造确定性 waker：clone 保持相同 data 指针与 vtable，
    /// `will_wake` 对同一来源的 waker 必返回 true（`Waker::noop()`
    /// 不提供该保证，会导致 dedup 测试跨平台不稳定）。
    fn deterministic_waker() -> Waker {
        use std::task::RawWaker;

        fn no_op(_: *const ()) {}
        fn clone(pointer: *const ()) -> RawWaker {
            RawWaker::new(pointer, &VTABLE)
        }
        static VTABLE: std::task::RawWakerVTable =
            std::task::RawWakerVTable::new(clone, no_op, no_op, no_op);
        // SAFETY：data 指针从不解引用，全部回调为 no-op。
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
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
            transport: pawork_domain::ModelTransport::Responses,
            citations: true,
            ..ModelCapabilities::default()
        };
        let source: Vec<(ProviderId, Vec<ModelDefinition>)> = vec![(
            ProviderId::new("glm-coding"),
            vec![mock_definition("glm-5.2", v2.clone())],
        )];
        registry.merge_provider_source(&source);
        let evidence = registry.capability_evidence("glm-5.2").unwrap();
        let static_caps = evidence.static_declared.expect("static after merge");
        assert_eq!(
            static_caps.transport,
            pawork_domain::ModelTransport::Responses
        );
        assert!(static_caps.citations);
        assert_eq!(
            evidence.provider.as_ref().map(ProviderId::as_str),
            Some("glm-coding")
        );

        // 跨 provider id 冲突跳过（不抢占、不做名称匹配）。
        let other: Vec<(ProviderId, Vec<ModelDefinition>)> = vec![(
            ProviderId::new("other"),
            vec![mock_definition(
                "glm-5.2",
                ModelCapabilities {
                    text: true,
                    citations: false,
                    ..ModelCapabilities::default()
                },
            )],
        )];
        registry.merge_provider_source(&other);
        let after = registry
            .capability_evidence("glm-5.2")
            .unwrap()
            .static_declared
            .unwrap();
        assert!(after.citations, "foreign provider must not overwrite");

        // factory 的未知模型被追加。
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
            pawork_domain::ModelTransport::Responses
        );
    }
}
