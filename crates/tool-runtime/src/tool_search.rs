//! 延迟加载工具索引与搜索激活（P15-6）。
//!
//! 面对大量可用工具（内置 + MCP + GUI + Provider extension），把全部 schema
//! 一次性塞进上下文代价过高。本模块维护「已声明但未激活」的工具 manifest 索引：
//! - [`LazyToolIndex::declare`] 登记 manifest（名称 / 描述 / capabilities /
//!   来源 / `requires_approval`），激活前不进入活跃 registry，因此也不会进入
//!   CanonicalModelRequest 的 tools 列表；
//! - [`LazyToolIndex::search_tools`] 按名称 / 描述 / capabilities 轻量匹配，
//!   只返回未激活且匹配的工具（自实现最小子集，不引入搜索引擎依赖）；
//! - [`LazyToolIndex::activate_tool`] 把延迟工具移入活跃 registry（可被
//!   [`ToolScheduler`] 路由），幂等；ProviderExtension 类来源先过审批闸门
//!   （未信任工作区默认拒绝，需显式审批，与 P4-9 / P15-1 §6 一致）；
//! - [`ToolTokenBudget`] 在激活时把 schema 计入 tools token 预算（计数口径与
//!   `context-engine` 的 `HeuristicEstimator::count_tool_schemas` 对齐），
//!   超限时优先淘汰「当前轮未使用」的工具，保留当前轮已用工具。
//!
//! 边界：`ToolKind::ProviderHosted`（server tool）由 P15-8 能力协商在请求侧
//! 声明，不参与本地搜索激活，`declare` 直接拒绝。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use agent_domain::ToolKind;
use tool_api::{AgentTool, ToolCapabilityTag, ToolDescriptor};

use crate::scheduler::{ToolRegistry, ToolRegistryError};

/// 每工具 schema 的结构开销（镜像 context-engine 的 `TOOL_FRAMING_TOKENS`）。
pub const TOOL_SCHEMA_FRAMING_TOKENS: u64 = 8;
/// 启发式估算的字符 / token 比率（镜像 context-engine 默认 `HeuristicEstimator`）。
pub const HEURISTIC_CHARS_PER_TOKEN: u32 = 4;

/// 工具来源（manifest 索引的一等字段；与执行位点一一对应）。
///
/// `ClientFunctionBuiltin` 对应 `ToolKind::ClientFunction`；其余三者均为
/// `ToolKind::ProviderExtension` 位点（内置扩展 / MCP / GUI / Provider 中介）。
/// `ToolKind::ProviderHosted` 不在本地索引内（边界）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolSource {
    /// 内置 ClientFunction（P4-*），Core 本地执行。
    ClientFunctionBuiltin,
    /// MCP server 工具（P9-3 能力发现后声明）。
    Mcp,
    /// GUI 工具。
    Gui,
    /// Provider 中介的外部扩展（P15-1）。
    ProviderExtension,
}

impl ToolSource {
    /// 来源声明的 canonical 执行位点；用于与 descriptor.kind 一致性校验。
    pub const fn tool_kind(self) -> ToolKind {
        match self {
            Self::ClientFunctionBuiltin => ToolKind::ClientFunction,
            Self::Mcp | Self::Gui | Self::ProviderExtension => ToolKind::ProviderExtension,
        }
    }
}

/// 延迟工具 manifest（搜索与激活的元数据视图）。
///
/// 字段全部复用 [`ToolDescriptor`]（名称 / 描述 / capabilities /
/// `requires_approval`），不引入独立 schema。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolManifest {
    pub name: String,
    pub description: String,
    pub capabilities: Vec<ToolCapabilityTag>,
    pub source: ToolSource,
    pub requires_approval: bool,
}

impl ToolManifest {
    /// 从 descriptor + 来源构造 manifest。
    pub fn from_descriptor(descriptor: &ToolDescriptor, source: ToolSource) -> Self {
        Self {
            name: descriptor.name.clone(),
            description: descriptor.description.clone(),
            capabilities: descriptor.capabilities.clone(),
            source,
            requires_approval: descriptor.requires_approval,
        }
    }
}

/// 一次搜索的命中结果。
///
/// 搜索只返回未激活且匹配的工具（已激活项不重复出现），`active` 恒为
/// `false`，显式保留以便调用方断言。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolMatch {
    pub manifest: ToolManifest,
    /// 匹配强度（仅排序用）：名称命中 2 分、描述 1 分、capabilities 1 分。
    pub score: u8,
    /// 恒为 `false`：搜索索引只含未激活工具。
    pub active: bool,
}

/// 工具 token 预算（P3-2 协同）。
///
/// 计数口径与 `context-engine::HeuristicEstimator::count_tool_schemas` 一致：
/// 把 `{name, description, input_schema}` 序列化为 JSON 后按 chars/token 估算
/// （CJK 1 字符/token，其余按 4 字符/token 向上取整），再加每工具 8 token 结构
/// 开销。`context-engine` 是唯一权威实现，本模块保持同口径以便两端数值互通；
/// 两者均为确定性启发式，不依赖精确 tokenizer。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolTokenBudget {
    /// `None` 表示不限制（默认）。
    limit: Option<u64>,
    used: u64,
}

impl ToolTokenBudget {
    pub fn new(limit: Option<u64>) -> Self {
        Self { limit, used: 0 }
    }

    /// 估算单个工具 schema 的 token 占用（与 context-engine 同口径）。
    pub fn count_schema(descriptor: &ToolDescriptor) -> u64 {
        let schema = serde_json::json!({
            "name": descriptor.name.clone(),
            "description": descriptor.description.clone(),
            "input_schema": descriptor.input_schema,
        });
        let json = serde_json::to_string(&schema).unwrap_or_default();
        heuristic_count_text(&json) + TOOL_SCHEMA_FRAMING_TOKENS
    }

    pub const fn limit(&self) -> Option<u64> {
        self.limit
    }

    pub const fn used(&self) -> u64 {
        self.used
    }

    /// 剩余可用 token；无限制时返回 `None`。
    pub fn remaining(&self) -> Option<u64> {
        self.limit.map(|limit| limit.saturating_sub(self.used))
    }

    fn add(&mut self, tokens: u64) {
        self.used = self.used.saturating_add(tokens);
    }

    fn remove(&mut self, tokens: u64) {
        self.used = self.used.saturating_sub(tokens);
    }
}

/// 镜像 context-engine `HeuristicEstimator` 的 CJK 感知计数。
fn heuristic_count_text(text: &str) -> u64 {
    let (cjk_chars, other_chars) = text.chars().fold((0u64, 0u64), |(cjk, other), ch| {
        if is_cjk_like(ch) {
            (cjk + 1, other)
        } else {
            (cjk, other + 1)
        }
    });
    cjk_chars + other_chars.div_ceil(HEURISTIC_CHARS_PER_TOKEN as u64)
}

/// 镜像 context-engine `HeuristicEstimator::is_cjk_like` 的字符区间。
fn is_cjk_like(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x11FF // Hangul Jamo
            | 0x2E80..=0x2FFF // CJK radicals
            | 0x3000..=0x303F // CJK symbols and punctuation
            | 0x3040..=0x30FF // Hiragana and Katakana
            | 0x3100..=0x312F // Bopomofo
            | 0x3130..=0x318F // Hangul compatibility Jamo
            | 0x31A0..=0x31BF // Bopomofo extended
            | 0x31F0..=0x31FF // Katakana phonetic extensions
            | 0x3400..=0x4DBF // CJK unified ideographs extension A
            | 0x4E00..=0x9FFF // CJK unified ideographs
            | 0xA960..=0xA97F // Hangul Jamo extended-A
            | 0xAC00..=0xD7AF // Hangul syllables
            | 0xD7B0..=0xD7FF // Hangul Jamo extended-B
            | 0xF900..=0xFAFF // CJK compatibility ideographs
            | 0x20000..=0x2FA1F // CJK extensions and compatibility supplement
    )
}

/// 激活审批拒绝（与 P15-1 §6 审计语义一致的 fail-closed 描述）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationDenied {
    pub name: String,
    pub reason: String,
}

/// ProviderExtension 激活审批闸门（trait 抽象，便于宿主注入）。
///
/// 索引只在激活 Extension 类来源（Mcp / Gui / ProviderExtension）时调用；
/// 默认实现 [`PolicyActivationGate`] 复用 P4-9 语义：未信任工作区默认拒绝，
/// 信任工作区仍需真实、可审计的显式审批（自动放行器不能满足）。
#[async_trait::async_trait]
pub trait ToolActivationGate: Send + Sync {
    async fn authorize_activation(&self, manifest: &ToolManifest) -> Result<(), ActivationDenied>;
}

/// 显式审批通道（镜像 scheduler 的 `ApprovalResolver`，但不绑定 ToolRequest）。
#[async_trait::async_trait]
pub trait ActivationApprovalResolver: Send + Sync {
    /// 是否代表一次真实、可审计的用户审批通道。
    ///
    /// 自动放行器必须返回 `false`，防止其满足激活审批要求。
    fn can_resolve_policy_prompt(&self) -> bool {
        true
    }

    /// 返回 `true` 表示该次激活被批准。
    async fn resolve(&self, manifest: &ToolManifest) -> bool;
}

/// 仅用于无审批路径的占位；不能满足显式审批要求。
#[derive(Debug, Default, Clone)]
pub struct AutoActivationApproval;

#[async_trait::async_trait]
impl ActivationApprovalResolver for AutoActivationApproval {
    fn can_resolve_policy_prompt(&self) -> bool {
        false
    }

    async fn resolve(&self, _manifest: &ToolManifest) -> bool {
        true
    }
}

/// 默认激活审批闸门（P4-9 / P15-1 §6 语义）。
///
/// - 未信任工作区：无条件拒绝（不允许 descriptor 自降级）；
/// - 信任工作区：要求真实审批通道显式批准，自动放行器 fail closed。
#[derive(Clone)]
pub struct PolicyActivationGate {
    workspace_trusted: bool,
    approval: Arc<dyn ActivationApprovalResolver>,
}

impl PolicyActivationGate {
    pub fn new(workspace_trusted: bool, approval: Arc<dyn ActivationApprovalResolver>) -> Self {
        Self {
            workspace_trusted,
            approval,
        }
    }
}

#[async_trait::async_trait]
impl ToolActivationGate for PolicyActivationGate {
    async fn authorize_activation(&self, manifest: &ToolManifest) -> Result<(), ActivationDenied> {
        if !self.workspace_trusted {
            return Err(ActivationDenied {
                name: manifest.name.clone(),
                reason: "ProviderExtension activation is denied in an untrusted workspace; \
                         trust the workspace first"
                    .into(),
            });
        }
        if !self.approval.can_resolve_policy_prompt() {
            return Err(ActivationDenied {
                name: manifest.name.clone(),
                reason: "ProviderExtension activation requires explicit user approval; \
                         automatic approval is forbidden"
                    .into(),
            });
        }
        if self.approval.resolve(manifest).await {
            Ok(())
        } else {
            Err(ActivationDenied {
                name: manifest.name.clone(),
                reason: "ProviderExtension activation was not approved".into(),
            })
        }
    }
}

/// 索引配置。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolIndexConfig {
    /// tools token 预算上限；`None` 表示不限制。
    pub tools_token_limit: Option<u64>,
}

/// 一次激活的结果（含预算联动信息）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolActivation {
    /// `false` 表示该工具本就处于激活状态（幂等 no-op）。
    pub activated: bool,
    /// 为容纳新工具被预算淘汰的工具名（按淘汰顺序）。
    pub evicted: Vec<String>,
    /// 新工具 schema 占用的 token。
    pub schema_tokens: u64,
    /// 激活后活跃工具的 schema token 总量。
    pub used_tokens: u64,
    /// 预算上限；`None` 表示不限制。
    pub limit: Option<u64>,
}

/// 索引错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ToolIndexError {
    #[error("unknown tool: {name}")]
    UnknownTool { name: String },
    #[error("tool `{name}` is already declared")]
    AlreadyDeclared { name: String },
    #[error("tool `{name}` is already active")]
    AlreadyActive { name: String },
    #[error(
        "ProviderHosted tool `{name}` is not part of the local lazy index; \
             hosted tools are declared at request time via capability negotiation"
    )]
    HostedNotIndexed { name: String },
    #[error("tool `{name}` has source {tool_source:?} which does not match kind {kind:?}")]
    SourceKindMismatch {
        name: String,
        tool_source: ToolSource,
        kind: ToolKind,
    },
    #[error("ClientFunction `{name}` requires a local executor")]
    MissingExecutor { name: String },
    #[error("activation of `{name}` denied: {reason}")]
    ActivationDenied { name: String, reason: String },
    #[error(
        "tools token budget exceeded: activating `{name}` needs {tokens} tokens, \
             limit {limit}, already used {used}"
    )]
    BudgetExceeded {
        name: String,
        tokens: u64,
        limit: u64,
        used: u64,
    },
    #[error(transparent)]
    Registry(#[from] ToolRegistryError),
}

struct DeclaredTool {
    manifest: ToolManifest,
    descriptor: ToolDescriptor,
    /// 仅 `ClientFunctionBuiltin` 持有本地 executor。
    executor: Option<Arc<dyn AgentTool>>,
}

/// 延迟加载工具索引：已声明未激活的工具清单 + 活跃 registry + token 预算。
#[derive(Default)]
pub struct LazyToolIndex {
    declared: HashMap<String, DeclaredTool>,
    active: HashMap<String, DeclaredTool>,
    /// 激活顺序（预算淘汰的 LRU 依据；淘汰即移除）。
    activation_order: Vec<String>,
    /// 当前轮已使用的活跃工具（预算超限时优先保留）。
    used_this_round: HashSet<String>,
    registry: ToolRegistry,
    budget: ToolTokenBudget,
}

impl LazyToolIndex {
    pub fn new(config: ToolIndexConfig) -> Self {
        let budget = ToolTokenBudget::new(config.tools_token_limit);
        Self {
            budget,
            ..Self::default()
        }
    }

    /// 登记一个延迟工具。`ProviderHosted` 一律拒绝（边界断言）。
    ///
    /// `ClientFunctionBuiltin` 必须携带本地 executor；其余来源只登记
    /// descriptor。同名重复声明（含已激活名）返回错误。
    pub fn declare(
        &mut self,
        descriptor: ToolDescriptor,
        source: ToolSource,
        executor: Option<Arc<dyn AgentTool>>,
    ) -> Result<(), ToolIndexError> {
        if descriptor.kind == ToolKind::ProviderHosted {
            return Err(ToolIndexError::HostedNotIndexed {
                name: descriptor.name.clone(),
            });
        }
        if !descriptor.has_consistent_hosting() {
            return Err(ToolRegistryError::KindHostingMismatch {
                name: descriptor.name.clone(),
                kind: descriptor.kind,
                hosting_kind: descriptor.hosting.tool_kind(),
            }
            .into());
        }
        if source.tool_kind() != descriptor.kind {
            return Err(ToolIndexError::SourceKindMismatch {
                name: descriptor.name.clone(),
                tool_source: source,
                kind: descriptor.kind,
            });
        }
        if descriptor.kind == ToolKind::ClientFunction && executor.is_none() {
            return Err(ToolIndexError::MissingExecutor {
                name: descriptor.name.clone(),
            });
        }
        let name = descriptor.name.clone();
        if self.declared.contains_key(&name) {
            return Err(ToolIndexError::AlreadyDeclared { name });
        }
        if self.active.contains_key(&name) {
            return Err(ToolIndexError::AlreadyActive { name });
        }
        self.declared.insert(
            name,
            DeclaredTool {
                manifest: ToolManifest::from_descriptor(&descriptor, source),
                descriptor,
                executor,
            },
        );
        Ok(())
    }

    /// 搜索未激活工具：query 按非字母数字切分为词元，每个词元需命中
    /// 名称 / 描述 / capabilities 拼接的归一化文本（大小写不敏感）。
    ///
    /// 空 query 不返回任何结果。结果按匹配强度降序、名称升序排序。
    pub fn search_tools(&self, query: &str) -> Vec<ToolMatch> {
        let terms = tokenize_query(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let mut matches: Vec<ToolMatch> = self
            .declared
            .values()
            .filter_map(|entry| {
                match_score(&entry.manifest, &terms).map(|score| ToolMatch {
                    manifest: entry.manifest.clone(),
                    score,
                    active: false,
                })
            })
            .collect();
        matches.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.manifest.name.cmp(&b.manifest.name))
        });
        matches
    }

    /// 激活一个延迟工具：移入活跃 registry（可被 [`ToolScheduler`] 路由）。
    ///
    /// - 幂等：已激活时返回 `activated: false` 的 no-op，不重复过闸门；
    /// - Extension 类来源先过 `gate` 审批（未信任工作区默认拒绝）；
    /// - 激活后 schema 计入 token 预算；超限时先淘汰「当前轮未使用」的工具
    ///   （按激活先后），仍超限（当前轮已用工具占满预算）则拒绝激活。
    pub async fn activate_tool(
        &mut self,
        id: &str,
        gate: &dyn ToolActivationGate,
    ) -> Result<ToolActivation, ToolIndexError> {
        if self.active.contains_key(id) {
            return Ok(ToolActivation {
                activated: false,
                evicted: Vec::new(),
                schema_tokens: 0,
                used_tokens: self.budget.used(),
                limit: self.budget.limit(),
            });
        }
        let Some(entry) = self.declared.get(id) else {
            return Err(ToolIndexError::UnknownTool {
                name: id.to_string(),
            });
        };

        // 审批闸门：只约束 Extension 类来源。
        if entry.manifest.source != ToolSource::ClientFunctionBuiltin {
            gate.authorize_activation(&entry.manifest)
                .await
                .map_err(|denied| ToolIndexError::ActivationDenied {
                    name: denied.name,
                    reason: denied.reason,
                })?;
        }

        let tokens = ToolTokenBudget::count_schema(&entry.descriptor);
        let mut evicted = Vec::new();
        if let Some(limit) = self.budget.limit() {
            if tokens > limit {
                return Err(ToolIndexError::BudgetExceeded {
                    name: id.to_string(),
                    tokens,
                    limit,
                    used: self.budget.used(),
                });
            }
            while self.budget.used().saturating_add(tokens) > limit {
                // 优先淘汰当前轮未使用、最早激活的工具；当前轮已用工具保留。
                let candidate = self
                    .activation_order
                    .iter()
                    .find(|name| {
                        !self.used_this_round.contains(*name) && self.active.contains_key(*name)
                    })
                    .cloned();
                let Some(candidate) = candidate else {
                    return Err(ToolIndexError::BudgetExceeded {
                        name: id.to_string(),
                        tokens,
                        limit,
                        used: self.budget.used(),
                    });
                };
                self.evict(&candidate);
                evicted.push(candidate);
            }
        }

        let entry = self
            .declared
            .remove(id)
            .expect("entry presence checked above");
        match entry.manifest.source {
            ToolSource::ClientFunctionBuiltin => {
                let executor = entry
                    .executor
                    .clone()
                    .expect("ClientFunction executor required by declare");
                self.registry.register(executor)?;
            }
            _ => self
                .registry
                .register_descriptor(entry.descriptor.clone())?,
        }
        self.active.insert(id.to_string(), entry);
        self.activation_order.push(id.to_string());
        self.budget.add(tokens);

        Ok(ToolActivation {
            activated: true,
            evicted,
            schema_tokens: tokens,
            used_tokens: self.budget.used(),
            limit: self.budget.limit(),
        })
    }

    /// 标记某活跃工具在当前轮已被调度使用（预算淘汰时优先保留）。
    pub fn mark_used(&mut self, id: &str) {
        if self.active.contains_key(id) {
            self.used_this_round.insert(id.to_string());
        }
    }

    /// 开启新一轮：清空「当前轮已用」标记。
    pub fn start_round(&mut self) {
        self.used_this_round.clear();
    }

    /// 活跃工具的 registry 快照（Arc 共享，构建 [`ToolScheduler`] 用）。
    pub fn active_registry(&self) -> ToolRegistry {
        self.registry.clone()
    }

    /// 活跃工具的全部 descriptor（即进入 CanonicalModelRequest.tools 的集合；
    /// 未激活工具不在此列）。
    pub fn active_descriptors(&self) -> Vec<ToolDescriptor> {
        self.registry.descriptors()
    }

    /// 活跃工具的全部 manifest（按名称排序）。
    pub fn active_manifests(&self) -> Vec<ToolManifest> {
        let mut manifests: Vec<_> = self
            .active
            .values()
            .map(|entry| entry.manifest.clone())
            .collect();
        manifests.sort_by(|a, b| a.name.cmp(&b.name));
        manifests
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.active.contains_key(id)
    }

    pub fn declared_count(&self) -> usize {
        self.declared.len()
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    pub fn budget(&self) -> &ToolTokenBudget {
        &self.budget
    }

    /// 预算淘汰：从活跃集合移回已声明集合（可再次搜索 / 激活）。
    fn evict(&mut self, name: &str) {
        if let Some(entry) = self.active.remove(name) {
            self.registry.remove(name);
            let tokens = ToolTokenBudget::count_schema(&entry.descriptor);
            self.budget.remove(tokens);
            self.activation_order.retain(|n| n != name);
            self.declared.insert(name.to_string(), entry);
        }
    }
}

/// query 归一化：小写并按非字母数字切分为词元。
fn tokenize_query(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_string)
        .collect()
}

/// 轻量匹配：所有词元必须命中归一化 haystack；返回匹配强度。
fn match_score(manifest: &ToolManifest, terms: &[String]) -> Option<u8> {
    let name = manifest.name.to_lowercase();
    let description = manifest.description.to_lowercase();
    let capabilities: Vec<String> = manifest
        .capabilities
        .iter()
        .filter_map(|tag| serde_json::to_string(tag).ok())
        .collect();
    let haystack = format!("{name} {description} {}", capabilities.join(" "));
    if !terms.iter().all(|term| haystack.contains(term)) {
        return None;
    }

    let mut score = 0u8;
    if terms.iter().all(|term| name.contains(term)) {
        score += 2;
    }
    if terms.iter().all(|term| description.contains(term)) {
        score += 1;
    }
    if terms
        .iter()
        .all(|term| capabilities.iter().any(|cap| cap.contains(term)))
    {
        score += 1;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use agent_domain::{CancellationToken, RunId, ToolCallId, WorkspaceId};
    use async_trait::async_trait;
    use serde_json::json;
    use tool_api::{
        AgentTool, ToolCapability, ToolError, ToolEventSink, ToolExecutionContext, ToolHosting,
        ToolKind, ToolRequest, ToolResult,
    };

    use crate::{
        ApprovalMode, ApprovalOutcome, ApprovalResolver, AutoApproveResolver, NoopToolEventSink,
        ToolScheduler, ToolSchedulerConfig,
    };

    use super::*;

    /// 计数探针：验证激活后的 ClientFunction 可被 scheduler 路由。
    struct CountingProbe {
        name: &'static str,
        calls: Arc<AtomicU64>,
    }

    #[async_trait]
    impl AgentTool for CountingProbe {
        fn descriptor(&self) -> ToolDescriptor {
            client_descriptor(
                self.name,
                "probe tool",
                Vec::new(),
                ToolCapabilityTag::ToolSearch,
                false,
            )
        }

        async fn execute(
            &self,
            _request: ToolRequest,
            _context: ToolExecutionContext,
            _sink: &dyn ToolEventSink,
            _cancel: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult::success(Vec::new()))
        }
    }

    fn client_descriptor(
        name: &str,
        description: &str,
        mut capabilities: Vec<ToolCapabilityTag>,
        tag: ToolCapabilityTag,
        requires_approval: bool,
    ) -> ToolDescriptor {
        capabilities.push(tag);
        ToolDescriptor {
            name: name.into(),
            description: description.into(),
            input_schema: json!({"type": "object"}),
            capability: ToolCapability::ReadOnly,
            kind: ToolKind::ClientFunction,
            hosting: ToolHosting::Local,
            capabilities,
            requires_approval,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: None,
            max_output_bytes: 1024,
            allowed_in_untrusted_workspace: true,
        }
    }

    fn extension_descriptor(
        name: &str,
        description: &str,
        reference: &str,
        requires_approval: bool,
    ) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            description: description.into(),
            input_schema: json!({"type": "object"}),
            capability: ToolCapability::ExternalPlugin,
            kind: ToolKind::ProviderExtension,
            hosting: ToolHosting::ProviderExtension {
                reference: reference.into(),
            },
            capabilities: vec![ToolCapabilityTag::ServerSideMcp],
            requires_approval,
            read_only: true,
            supports_concurrency: false,
            default_timeout_ms: None,
            max_output_bytes: 1024,
            allowed_in_untrusted_workspace: false,
        }
    }

    fn hosted_descriptor(name: &str) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            description: "provider-hosted".into(),
            input_schema: json!({"type": "object"}),
            capability: ToolCapability::Network,
            kind: ToolKind::ProviderHosted,
            hosting: ToolHosting::ProviderHosted {
                hosted_name: name.into(),
                kind: ToolCapabilityTag::WebSearch,
            },
            capabilities: vec![ToolCapabilityTag::WebSearch],
            requires_approval: false,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: None,
            max_output_bytes: 1024,
            allowed_in_untrusted_workspace: true,
        }
    }

    fn request(name: &str) -> ToolRequest {
        ToolRequest {
            tool_call_id: ToolCallId::from(name),
            input: json!({}),
        }
    }

    fn execution_context() -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_id: WorkspaceId::from("workspace-smoke"),
            run_id: RunId::from("run-smoke"),
            working_directory: Some("project".into()),
        }
    }

    /// 显式审批通道（满足 PolicyActivationGate 的显式审批语义）。
    struct ExplicitActivationApproval;

    #[async_trait]
    impl ActivationApprovalResolver for ExplicitActivationApproval {
        async fn resolve(&self, _manifest: &ToolManifest) -> bool {
            true
        }
    }

    struct DenyActivationApproval;

    #[async_trait]
    impl ActivationApprovalResolver for DenyActivationApproval {
        async fn resolve(&self, _manifest: &ToolManifest) -> bool {
            false
        }
    }

    fn gate(trusted: bool) -> PolicyActivationGate {
        PolicyActivationGate::new(trusted, Arc::new(AutoActivationApproval))
    }

    fn explicit_gate(trusted: bool) -> PolicyActivationGate {
        PolicyActivationGate::new(trusted, Arc::new(ExplicitActivationApproval))
    }

    #[tokio::test]
    async fn search_returns_only_inactive_matching_tools() {
        let mut index = LazyToolIndex::default();
        index
            .declare(
                client_descriptor(
                    "read_file",
                    "read a workspace-relative file",
                    Vec::new(),
                    ToolCapabilityTag::FileOrCollectionSearch,
                    false,
                ),
                ToolSource::ClientFunctionBuiltin,
                Some(Arc::new(CountingProbe {
                    name: "read_file",
                    calls: Arc::new(AtomicU64::new(0)),
                })),
            )
            .unwrap();
        index
            .declare(
                extension_descriptor(
                    "mcp_web_search",
                    "search the web through an mcp server",
                    "mcp://search",
                    true,
                ),
                ToolSource::Mcp,
                None,
            )
            .unwrap();
        index
            .declare(
                extension_descriptor(
                    "gui_artifact_browser",
                    "browse generated artifacts in the gui",
                    "gui://artifacts",
                    true,
                ),
                ToolSource::Gui,
                None,
            )
            .unwrap();
        index
            .declare(
                extension_descriptor(
                    "provider_remote_shell",
                    "run a shell through a provider extension",
                    "connector:shell",
                    true,
                ),
                ToolSource::ProviderExtension,
                None,
            )
            .unwrap();
        assert_eq!(index.active_count(), 0);
        assert!(index.active_descriptors().is_empty());

        // 激活 mcp_web_search 后，搜索不再返回它。
        let activation = index
            .activate_tool("mcp_web_search", &explicit_gate(true))
            .await
            .expect("explicit approval activates extension");
        assert!(activation.activated);
        assert!(index.is_active("mcp_web_search"));

        // 唯一匹配项已被激活：搜索不再重复返回。
        assert!(index.search_tools("web search").is_empty());
        // 其余未激活工具按名称 / 描述 / capabilities 命中。
        let by_capability: Vec<String> = index
            .search_tools("search")
            .into_iter()
            .map(|m| m.manifest.name)
            .collect();
        assert_eq!(by_capability, vec!["read_file"]);
        let by_name: Vec<String> = index
            .search_tools("GUI")
            .into_iter()
            .map(|m| m.manifest.name)
            .collect();
        assert_eq!(by_name, vec!["gui_artifact_browser"]);
        let by_description: Vec<String> = index
            .search_tools("shell extension")
            .into_iter()
            .map(|m| m.manifest.name)
            .collect();
        assert_eq!(by_description, vec!["provider_remote_shell"]);

        // 命中结果携带完整 manifest 且标注未激活。
        let hit = &index.search_tools("shell extension")[0];
        assert!(!hit.active);
        assert_eq!(hit.manifest.source, ToolSource::ProviderExtension);
        assert!(hit.manifest.requires_approval);

        assert!(index.search_tools("").is_empty(), "空 query 不返回结果");
        assert!(index.search_tools("no_such_tool").is_empty());
    }

    #[tokio::test]
    async fn activated_client_function_is_routed_by_scheduler() {
        let mut index = LazyToolIndex::default();
        let calls = Arc::new(AtomicU64::new(0));
        index
            .declare(
                client_descriptor(
                    "read_file",
                    "read a workspace-relative file",
                    Vec::new(),
                    ToolCapabilityTag::FileOrCollectionSearch,
                    false,
                ),
                ToolSource::ClientFunctionBuiltin,
                Some(Arc::new(CountingProbe {
                    name: "read_file",
                    calls: calls.clone(),
                })),
            )
            .unwrap();

        // 激活前：不可路由、不进入请求工具列表。
        assert!(index.active_descriptors().is_empty());
        assert_eq!(index.active_count(), 0);

        let activation = index
            .activate_tool("read_file", &gate(false))
            .await
            .expect("builtin client function activates without approval");
        assert!(activation.activated);
        assert!(activation.evicted.is_empty());
        assert_eq!(index.active_descriptors().len(), 1);

        let scheduler = ToolScheduler::new(
            index.active_registry(),
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::NeverAsk,
                workspace_trusted: true,
            },
        );
        let result = scheduler
            .execute_named(
                "read_file",
                request("read_file"),
                execution_context(),
                CancellationToken::new(),
                &AutoApproveResolver,
                &NoopToolEventSink,
            )
            .await
            .expect("activated tool is routable");
        assert!(!result.is_error());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 幂等：重复激活是 no-op，不重过闸门、不重复入 registry。
        let again = index
            .activate_tool("read_file", &gate(false))
            .await
            .expect("idempotent activation");
        assert!(!again.activated);
        assert_eq!(index.active_count(), 1);
        assert_eq!(index.active_descriptors().len(), 1);
    }

    #[tokio::test]
    async fn provider_extension_activation_respects_approval_gate() {
        let mut index = LazyToolIndex::default();
        index
            .declare(
                extension_descriptor("remote_mcp", "server-side mcp tool", "mcp://remote", true),
                ToolSource::Mcp,
                None,
            )
            .unwrap();

        // 未信任工作区：默认拒绝（即使审批通道愿意放行）。
        let denied = index
            .activate_tool("remote_mcp", &explicit_gate(false))
            .await
            .expect_err("untrusted workspace must deny extension activation");
        assert!(matches!(
            denied,
            ToolIndexError::ActivationDenied { ref name, .. } if name == "remote_mcp"
        ));
        assert_eq!(index.active_count(), 0);
        assert_eq!(index.declared_count(), 1);

        // 信任工作区 + 自动放行器：显式审批要求 fail closed。
        let auto_denied = index
            .activate_tool("remote_mcp", &gate(true))
            .await
            .expect_err("auto approval must not satisfy extension activation");
        assert!(matches!(
            auto_denied,
            ToolIndexError::ActivationDenied { ref name, .. } if name == "remote_mcp"
        ));

        // 信任工作区 + 审批被拒：不激活。
        let rejected = index
            .activate_tool(
                "remote_mcp",
                &PolicyActivationGate::new(true, Arc::new(DenyActivationApproval)),
            )
            .await
            .expect_err("denied approval must not activate");
        assert!(matches!(
            rejected,
            ToolIndexError::ActivationDenied { ref name, .. } if name == "remote_mcp"
        ));

        // 信任工作区 + 显式审批：激活成功，可被路由为 provider dispatch。
        let activation = index
            .activate_tool("remote_mcp", &explicit_gate(true))
            .await
            .expect("explicit approval activates extension");
        assert!(activation.activated);
        assert!(index.is_active("remote_mcp"));
        assert_eq!(
            index.active_descriptors()[0].kind,
            ToolKind::ProviderExtension
        );

        // Extension 激活后只能走 provider dispatch，不进入本地执行。
        let scheduler = ToolScheduler::new(
            index.active_registry(),
            ToolSchedulerConfig {
                max_concurrent: 4,
                approval_mode: ApprovalMode::NeverAsk,
                workspace_trusted: true,
            },
        );
        let dispatch = scheduler
            .authorize_provider_call(
                "remote_mcp",
                request("remote_mcp"),
                CancellationToken::new(),
                &ExplicitApprove,
            )
            .await
            .expect("activated extension is dispatchable");
        assert_eq!(dispatch.descriptor().kind, ToolKind::ProviderExtension);
    }

    struct ExplicitApprove;

    #[async_trait]
    impl ApprovalResolver for ExplicitApprove {
        async fn resolve(&self, _requests: &[ToolRequest]) -> Vec<ApprovalOutcome> {
            vec![ApprovalOutcome::Approved]
        }
    }

    #[test]
    fn provider_hosted_is_rejected_by_the_lazy_index() {
        let mut index = LazyToolIndex::default();
        let error = index
            .declare(hosted_descriptor("web_search"), ToolSource::Mcp, None)
            .expect_err("ProviderHosted must not enter the local lazy index");
        assert!(matches!(
            error,
            ToolIndexError::HostedNotIndexed { ref name } if name == "web_search"
        ));
        assert_eq!(index.declared_count(), 0);

        // 来源与执行位点不一致同样拒绝。
        let mismatch = index
            .declare(
                client_descriptor(
                    "builtin_as_mcp",
                    "mismatch",
                    Vec::new(),
                    ToolCapabilityTag::ToolSearch,
                    false,
                ),
                ToolSource::Mcp,
                Some(Arc::new(CountingProbe {
                    name: "builtin_as_mcp",
                    calls: Arc::new(AtomicU64::new(0)),
                })),
            )
            .expect_err("source must match execution site");
        assert!(matches!(
            mismatch,
            ToolIndexError::SourceKindMismatch { .. }
        ));
    }

    #[tokio::test]
    async fn budget_eviction_keeps_current_round_tools() {
        // alpha/bravo 各 29 tokens、charlie 30 tokens（名称长度影响 JSON 长度）。
        let mut index = LazyToolIndex::new(ToolIndexConfig {
            tools_token_limit: Some(59),
        });
        for name in ["alpha", "bravo", "charlie"] {
            index
                .declare(
                    client_descriptor(
                        name,
                        "budget probe tool",
                        Vec::new(),
                        ToolCapabilityTag::ToolSearch,
                        false,
                    ),
                    ToolSource::ClientFunctionBuiltin,
                    Some(Arc::new(CountingProbe {
                        name,
                        calls: Arc::new(AtomicU64::new(0)),
                    })),
                )
                .unwrap();
        }
        index.activate_tool("alpha", &gate(false)).await.unwrap();
        index.activate_tool("bravo", &gate(false)).await.unwrap();
        assert_eq!(index.active_count(), 2);
        assert_eq!(index.budget().used(), 58);

        // 本轮已使用 alpha、bravo：激活 charlie 时不得淘汰它们 → 预算拒绝。
        index.mark_used("alpha");
        index.mark_used("bravo");
        let activation = index
            .activate_tool("charlie", &gate(false))
            .await
            .expect_err("round-used tools must not be evicted");
        assert!(matches!(
            activation,
            ToolIndexError::BudgetExceeded { ref name, .. } if name == "charlie"
        ));
        assert!(index.is_active("alpha"));
        assert!(index.is_active("bravo"));
        assert!(!index.is_active("charlie"));

        // 新一轮只使用 alpha：激活 charlie 时淘汰未使用的 bravo。
        index.start_round();
        index.mark_used("alpha");
        let activation = index
            .activate_tool("charlie", &gate(false))
            .await
            .expect("eviction makes room for charlie");
        assert!(activation.activated);
        assert_eq!(activation.evicted, vec!["bravo".to_string()]);
        assert_eq!(activation.used_tokens, 59);
        assert!(index.is_active("alpha"));
        assert!(index.is_active("charlie"));
        assert!(!index.is_active("bravo"));

        // 被淘汰的工具回到已声明集合，可再次搜索到并重新激活。
        let names: Vec<String> = index
            .search_tools("bravo")
            .into_iter()
            .map(|m| m.manifest.name)
            .collect();
        assert_eq!(names, vec!["bravo"]);
        let reactivation = index
            .activate_tool("bravo", &gate(false))
            .await
            .expect("evicted tool can be re-activated after evicting charlie");
        assert!(reactivation.activated);
        assert_eq!(reactivation.evicted, vec!["charlie".to_string()]);
        assert_eq!(reactivation.used_tokens, 58);
        assert!(index.is_active("alpha"));
        assert!(index.is_active("bravo"));
        assert!(!index.is_active("charlie"));
    }

    #[test]
    fn budget_counts_schema_with_context_engine_convention() {
        // 与 context-engine HeuristicEstimator::count_tool_schemas 同口径：
        // JSON 序列化（name/description/input_schema）+ 每工具 8 token。
        let descriptor = client_descriptor(
            "read_file",
            "read a file",
            Vec::new(),
            ToolCapabilityTag::FileOrCollectionSearch,
            false,
        );
        let tokens = ToolTokenBudget::count_schema(&descriptor);
        let json = serde_json::to_string(&json!({
            "name": "read_file",
            "description": "read a file",
            "input_schema": {"type": "object"},
        }))
        .unwrap();
        assert_eq!(
            tokens,
            heuristic_count_text(&json) + TOOL_SCHEMA_FRAMING_TOKENS
        );
        assert_eq!(heuristic_count_text("abcdefgh"), 2); // 8 chars / 4
        assert_eq!(heuristic_count_text("你好世界"), 4); // CJK 1 字符/token

        // CJK 描述按字符计 token，不会被 4 chars/token 低估。
        let latin_tokens = ToolTokenBudget::count_schema(&descriptor);
        let mut cjk = client_descriptor(
            "read_file",
            "读取文件内容",
            Vec::new(),
            ToolCapabilityTag::FileOrCollectionSearch,
            false,
        );
        let cjk_tokens = ToolTokenBudget::count_schema(&cjk);
        cjk.description = "读取文件内容并返回".into();
        let longer_cjk = ToolTokenBudget::count_schema(&cjk);
        assert!(latin_tokens < cjk_tokens);
        assert!(cjk_tokens < longer_cjk);
    }
}
