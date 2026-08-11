//! 命名空间化的工具/命令注册（P10-3）。
//!
//! `tool-runtime::ToolRegistry` 用「最后写入覆盖同名」语义；插件工具必须：
//! - 以 `plugin_id::local_name` 形式命名，避免与内置/其它插件工具冲突；
//! - capability 固定为 `ExternalPlugin`（manifest 已禁用插件声明该 capability）；
//! - **不覆盖同名**：已存在则返回错误，保证多插件互不打架。
//!
//! 本模块提供 [`NamespacedToolRegistry`]：包装 `ToolRegistry`，新增
//! `register_external`（不覆盖同名）并保留 `clone-into-ToolRegistry` 的能力，
//! 供调度器消费。同时提供 [`PluginCommandRegistry`]：按命令名索引插件命令元数据。

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_domain::{CancellationToken, ContentPart, ErrorContext, TextContent};
use plugin_api::{PluginError, PluginErrorKind, PluginToolRegistration};
use tool_api::{
    AgentTool, ToolCapability, ToolDescriptor, ToolError, ToolEventSink, ToolExecutionContext,
    ToolRequest, ToolResult,
};
use tool_runtime::ToolRegistry;

/// 工具全名：`plugin_id::local_name`。
pub fn external_tool_name(plugin_id: &agent_domain::PluginId, local_name: &str) -> String {
    format!("{plugin_id}::{local_name}")
}

/// 命名空间化的工具注册表。
///
/// 内部维护 `name -> Arc<dyn AgentTool>` 与 `name -> owner plugin_id`，
/// 保证同名不覆盖。`Into<ToolRegistry>` 输出按名排序的描述符，供调度器使用。
#[derive(Default)]
pub struct NamespacedToolRegistry {
    tools: BTreeMap<String, Arc<dyn AgentTool>>,
    owners: BTreeMap<String, agent_domain::PluginId>,
}

impl NamespacedToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
            owners: BTreeMap::new(),
        }
    }

    /// 用一组描述符与回填工具句柄注册一个插件提供的全部工具。
    ///
    /// `factory` 把每个 `PluginToolRegistration` 翻译为一个 `AgentTool`，
    /// 通常由 `wasm-plugin-host` 提供「转发到 `invoke`」的适配器实现。
    /// 同名工具（跨插件）注册返回 [`PluginErrorKind::Conflict`]。
    pub fn register_external<F>(
        &mut self,
        plugin_id: &agent_domain::PluginId,
        registrations: &[PluginToolRegistration],
        factory: &F,
    ) -> Result<(), PluginError>
    where
        F: Fn(&agent_domain::PluginId, &PluginToolRegistration) -> Arc<dyn AgentTool>,
    {
        // 两阶段：先全部构造并预检同名，再提交，避免部分注册。
        let mut staged: Vec<(String, Arc<dyn AgentTool>)> = Vec::new();
        for reg in registrations {
            let name = external_tool_name(plugin_id, &reg.name);
            if self.tools.contains_key(&name) || staged.iter().any(|(n, _)| *n == name) {
                return Err(PluginError::new(
                    PluginErrorKind::Conflict,
                    format!("tool already registered: {name}"),
                ));
            }
            staged.push((name, factory(plugin_id, reg)));
        }
        for (name, tool) in staged {
            self.owners.insert(name.clone(), plugin_id.clone());
            self.tools.insert(name, tool);
        }
        Ok(())
    }

    /// 原子注销某插件拥有的全部工具，返回注销数量。
    pub fn unregister_plugin(&mut self, plugin_id: &agent_domain::PluginId) -> usize {
        let names = self
            .owners
            .iter()
            .filter(|(_, owner)| *owner == plugin_id)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for name in &names {
            self.owners.remove(name);
            self.tools.remove(name);
        }
        names.len()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools.get(name).cloned()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// 输出按名排序的描述符（与 ToolRegistry::descriptors 一致）。
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.values().map(|t| t.descriptor()).collect()
    }

    /// 克隆当前工具快照为 canonical `ToolRegistry`，供调度器重建时使用。
    pub fn to_tool_registry(&self) -> Result<ToolRegistry, tool_runtime::ToolRegistryError> {
        let mut registry = ToolRegistry::new();
        registry.extend(self.tools.values().cloned())?;
        Ok(registry)
    }
}

impl std::fmt::Debug for NamespacedToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NamespacedToolRegistry")
            .field("count", &self.tools.len())
            .finish_non_exhaustive()
    }
}

/// 把一个插件的工具注册翻译为 `AgentTool`：每个工具调用被路由到 host 的
/// `invoke`，Operation 为 `PluginOperation::Tool`。
pub struct ExternalPluginToolAdapter {
    descriptor: ToolDescriptor,
    host_caller: Arc<dyn ExternalToolCaller>,
    plugin_id: agent_domain::PluginId,
    local_name: String,
}

/// 把工具调用转发到 `WasmPluginHost::invoke_operation` 的回调抽象。
/// 使用 trait 对象避免 `WasmPluginHost` 出现在 registry 模块（保持单向依赖）。
#[async_trait::async_trait]
pub trait ExternalToolCaller: Send + Sync {
    async fn call(
        &self,
        plugin_id: &agent_domain::PluginId,
        local_name: &str,
        request: ToolRequest,
        context: ToolExecutionContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, PluginError>;
}

impl ExternalPluginToolAdapter {
    pub fn new(
        plugin_id: &agent_domain::PluginId,
        registration: &PluginToolRegistration,
        host_caller: Arc<dyn ExternalToolCaller>,
    ) -> Self {
        let descriptor = ToolDescriptor {
            name: external_tool_name(plugin_id, &registration.name),
            description: registration.description.clone(),
            input_schema: registration.input_schema.clone(),
            // 插件工具统一为 ExternalPlugin：调度器据此串行 + 审批门控，
            // 且不允许插件自行声明该 capability（manifest 已禁止）。
            capability: ToolCapability::ExternalPlugin,
            kind: tool_api::ToolKind::ClientFunction,
            hosting: tool_api::ToolHosting::Local,
            capabilities: Vec::new(),
            requires_approval: false,
            // 插件工具的副作用边界未知，保守视为可写。
            read_only: false,
            supports_concurrency: false,
            default_timeout_ms: registration.default_timeout_ms,
            max_output_bytes: registration.max_output_bytes,
            // 插件始终视为不可信工作区默认不可用，由 policy 进一步审批。
            allowed_in_untrusted_workspace: false,
        };
        Self {
            descriptor,
            host_caller,
            plugin_id: plugin_id.clone(),
            local_name: registration.name.clone(),
        }
    }
}

#[async_trait::async_trait]
impl AgentTool for ExternalPluginToolAdapter {
    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        _sink: &dyn ToolEventSink,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        match self
            .host_caller
            .call(&self.plugin_id, &self.local_name, request, context, cancel)
            .await
        {
            Ok(result) => Ok(result),
            Err(plugin_error) => Ok(ToolResult::failure(ErrorContext::from(plugin_error))),
        }
    }
}

/// 把任意 `serde_json::Value` 包装成 ToolResult 成功内容（单 Text 段）。
pub fn tool_result_from_value(value: serde_json::Value) -> ToolResult {
    if let Ok(result) = serde_json::from_value::<ToolResult>(value.clone()) {
        return result;
    }
    let text = match value {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    };
    ToolResult::success(vec![ContentPart::Text(TextContent { text })])
}

/// 插件命令注册表：`plugin_id::local_name -> PluginCommandRegistration`。
#[derive(Clone, Default)]
pub struct PluginCommandRegistry {
    commands: BTreeMap<String, agent_domain::PluginId>,
    descriptors: BTreeMap<String, plugin_api::PluginCommandRegistration>,
}

impl PluginCommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册插件命令。同名（跨插件）返回 [`PluginErrorKind::Conflict`]。
    pub fn register(
        &mut self,
        plugin_id: &agent_domain::PluginId,
        commands: &[plugin_api::PluginCommandRegistration],
    ) -> Result<(), PluginError> {
        // 两阶段：先预检，再提交。
        let mut staged: Vec<(String, plugin_api::PluginCommandRegistration)> = Vec::new();
        for reg in commands {
            let name = external_tool_name(plugin_id, &reg.name);
            if self.commands.contains_key(&name) || staged.iter().any(|(n, _)| *n == name) {
                return Err(PluginError::new(
                    PluginErrorKind::Conflict,
                    format!("command already registered: {name}"),
                ));
            }
            staged.push((name, reg.clone()));
        }
        for (name, reg) in staged {
            self.commands.insert(name.clone(), plugin_id.clone());
            self.descriptors.insert(name, reg);
        }
        Ok(())
    }

    /// 原子注销某插件拥有的全部命令，返回注销数量。
    pub fn unregister_plugin(&mut self, plugin_id: &agent_domain::PluginId) -> usize {
        let names = self
            .commands
            .iter()
            .filter(|(_, owner)| *owner == plugin_id)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for name in &names {
            self.commands.remove(name);
            self.descriptors.remove(name);
        }
        names.len()
    }

    pub fn get(&self, name: &str) -> Option<&plugin_api::PluginCommandRegistration> {
        self.descriptors.get(name)
    }

    pub fn owner(&self, name: &str) -> Option<&agent_domain::PluginId> {
        self.commands.get(name)
    }

    /// 全部命令名（稳定排序）。
    pub fn names(&self) -> Vec<String> {
        self.descriptors.keys().cloned().collect()
    }

    /// 解析已注册命令并通过宿主 caller 执行，未知命令 fail closed。
    pub async fn invoke(
        &self,
        name: &str,
        input: serde_json::Value,
        context: plugin_api::PluginContext,
        caller: &dyn ExternalCommandCaller,
        cancel: CancellationToken,
    ) -> Result<serde_json::Value, PluginError> {
        let plugin_id = self.commands.get(name).ok_or_else(|| {
            PluginError::new(
                PluginErrorKind::InvalidInvocation,
                format!("plugin command is not registered: {name}"),
            )
        })?;
        let descriptor = self.descriptors.get(name).ok_or_else(|| {
            PluginError::new(
                PluginErrorKind::Internal,
                format!("plugin command descriptor is missing: {name}"),
            )
        })?;
        caller
            .call_command(plugin_id, &descriptor.name, input, context, cancel)
            .await
    }
}

#[async_trait::async_trait]
pub trait ExternalCommandCaller: Send + Sync {
    async fn call_command(
        &self,
        plugin_id: &agent_domain::PluginId,
        local_name: &str,
        input: serde_json::Value,
        context: plugin_api::PluginContext,
        cancel: CancellationToken,
    ) -> Result<serde_json::Value, PluginError>;
}

impl std::fmt::Debug for PluginCommandRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginCommandRegistry")
            .field("count", &self.descriptors.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_api::PluginToolRegistration;

    fn tool_reg(name: &str) -> PluginToolRegistration {
        PluginToolRegistration {
            name: name.into(),
            description: "desc".into(),
            input_schema: serde_json::json!({"type": "object"}),
            default_timeout_ms: None,
            max_output_bytes: 1024,
        }
    }

    struct DummyCaller;

    #[async_trait::async_trait]
    impl ExternalToolCaller for DummyCaller {
        async fn call(
            &self,
            _plugin_id: &agent_domain::PluginId,
            _local_name: &str,
            _request: ToolRequest,
            _context: ToolExecutionContext,
            _cancel: CancellationToken,
        ) -> Result<ToolResult, PluginError> {
            Ok(tool_result_from_value(serde_json::json!({"ok": true})))
        }
    }

    #[test]
    fn register_external_namespaces_and_refuses_duplicates() {
        let mut registry = NamespacedToolRegistry::new();
        let a = agent_domain::PluginId::from("a.plugin");
        let b = agent_domain::PluginId::from("b.plugin");
        let caller: Arc<dyn ExternalToolCaller> = Arc::new(DummyCaller);

        registry
            .register_external(&a, &[tool_reg("echo")], &|pid, reg| {
                Arc::new(ExternalPluginToolAdapter::new(pid, reg, caller.clone()))
            })
            .unwrap();
        // 不同插件同名 local name 不冲突（namespace 不同）。
        registry
            .register_external(&b, &[tool_reg("echo")], &|pid, reg| {
                Arc::new(ExternalPluginToolAdapter::new(pid, reg, caller.clone()))
            })
            .unwrap();
        assert!(registry.contains("a.plugin::echo"));
        assert!(registry.contains("b.plugin::echo"));

        // 同 namespace 同名直接冲突。
        let err = registry
            .register_external(&a, &[tool_reg("echo")], &|pid, reg| {
                Arc::new(ExternalPluginToolAdapter::new(pid, reg, caller.clone()))
            })
            .unwrap_err();
        assert_eq!(err.kind, PluginErrorKind::Conflict);
    }

    #[test]
    fn command_registry_namespaces_and_refuses_duplicates() {
        let mut registry = PluginCommandRegistry::new();
        let a = agent_domain::PluginId::from("a.plugin");
        let b = agent_domain::PluginId::from("b.plugin");

        registry
            .register(
                &a,
                &[plugin_api::PluginCommandRegistration {
                    name: "run".into(),
                    description: "run".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                }],
            )
            .unwrap();
        registry
            .register(
                &b,
                &[plugin_api::PluginCommandRegistration {
                    name: "run".into(),
                    description: "run".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                }],
            )
            .unwrap();

        let err = registry
            .register(
                &a,
                &[plugin_api::PluginCommandRegistration {
                    name: "run".into(),
                    description: "run".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                }],
            )
            .unwrap_err();
        assert_eq!(err.kind, PluginErrorKind::Conflict);

        assert_eq!(registry.owner("a.plugin::run"), Some(&a));
        assert_eq!(registry.names().len(), 2);
    }

    #[test]
    fn canonical_tool_result_is_preserved_instead_of_stringified() {
        let expected =
            ToolResult::success(vec![ContentPart::Text(TextContent { text: "ok".into() })]);
        let encoded = serde_json::to_value(&expected).expect("serialize tool result");
        assert_eq!(tool_result_from_value(encoded), expected);
    }
}
