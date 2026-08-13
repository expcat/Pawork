//! 事务化资源宿主接口（P17-3）。
//!
//! Marketplace 在 install / update / uninstall 期间经宿主注入的 [ResourceHost]
//! 注册 / 注销六类子资源。Marketplace 本身**绝不执行**任何子资源：
//! skills / agents / hooks / lsp 由各 loader 加载，MCP stdio 由 sandboxed
//! spawner 托管，monitors 只进 task-manager 执行——本 crate 只提交声明并保证
//! 事务语义：
//!
//! - 注册按固定顺序 skills -> agents -> hooks -> mcp -> lsp -> monitors
//!   （经 plugin_package::install_package）；
//! - 任一失败反向补偿、整体回滚（[compensate]）；
//! - Monitor 卸载先 stop 再 unregister（[remove_plan]）。

use plugin_package::{
    AgentProfileDispatch, DispatchPlan, HookDispatch, LanguageServerDispatch, McpDispatch,
    MonitorDispatch, PackageDispatchSink, PackageError, SkillDispatch,
};
use serde_json::Value;

use crate::error::MarketplaceError;

/// 宿主注入的事务化资源接口（六类资源的注册 / 注销 + monitor 停止）。
pub trait ResourceHost {
    fn register_skill(&mut self, dispatch: &SkillDispatch) -> Result<(), MarketplaceError>;
    fn register_agent(&mut self, dispatch: &AgentProfileDispatch) -> Result<(), MarketplaceError>;
    fn register_hook(&mut self, dispatch: &HookDispatch) -> Result<(), MarketplaceError>;
    fn register_mcp(&mut self, dispatch: &McpDispatch) -> Result<(), MarketplaceError>;
    fn register_lsp(&mut self, dispatch: &LanguageServerDispatch) -> Result<(), MarketplaceError>;
    fn register_monitor(&mut self, dispatch: &MonitorDispatch) -> Result<(), MarketplaceError>;

    fn unregister_skill(&mut self, key: &str) -> Result<(), MarketplaceError>;
    fn unregister_agent(&mut self, key: &str) -> Result<(), MarketplaceError>;
    fn unregister_hook(&mut self, key: &str) -> Result<(), MarketplaceError>;
    fn unregister_mcp(&mut self, key: &str) -> Result<(), MarketplaceError>;
    fn unregister_lsp(&mut self, key: &str) -> Result<(), MarketplaceError>;
    /// 停止 package-owned monitor（task-manager）。必须先于 unregister_monitor 调用。
    fn stop_monitor(&mut self, monitor_id: &str) -> Result<(), MarketplaceError>;
    /// 注销 monitor。Marketplace 保证调用前已成功 stop。
    fn unregister_monitor(&mut self, monitor_id: &str) -> Result<(), MarketplaceError>;
}

/// 资源稳定键（注册 / 注销 / 冲突检测共用，须与宿主侧定位一致）。
pub fn skill_key(dispatch: &SkillDispatch) -> String {
    dispatch.path.to_posix_string()
}

pub fn agent_key(dispatch: &AgentProfileDispatch) -> String {
    dispatch
        .path
        .as_ref()
        .map(|path| path.to_posix_string())
        .or_else(|| {
            dispatch
                .inline
                .as_ref()
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .map(|name| format!("inline:{name}"))
        })
        .unwrap_or_else(|| "<agent>".into())
}

pub fn hook_key(dispatch: &HookDispatch) -> String {
    dispatch
        .path
        .as_ref()
        .map(|path| path.to_posix_string())
        .or_else(|| {
            dispatch
                .inline
                .as_ref()
                .and_then(|value| value.get("trigger"))
                .and_then(Value::as_str)
                .map(String::from)
        })
        .unwrap_or_else(|| "<hook>".into())
}

pub fn mcp_key(dispatch: &McpDispatch) -> String {
    dispatch.server.name.clone()
}

pub fn lsp_key(dispatch: &LanguageServerDispatch) -> String {
    dispatch
        .path
        .as_ref()
        .map(|path| path.to_posix_string())
        .or_else(|| {
            dispatch
                .inline
                .as_ref()
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .map(String::from)
        })
        .unwrap_or_else(|| "<lsp>".into())
}

pub fn monitor_key(dispatch: &MonitorDispatch) -> String {
    dispatch.declaration.monitor_id.as_str().to_string()
}

/// 补偿步骤：撤销一次已注册的新资源，或恢复一次已注销的旧资源。
#[derive(Clone, Debug)]
pub(crate) enum UndoStep {
    RegisteredSkill(String),
    RegisteredAgent(String),
    RegisteredHook(String),
    RegisteredMcp(String),
    RegisteredLsp(String),
    RegisteredMonitor(String),
    RemovedSkill(SkillDispatch),
    RemovedAgent(AgentProfileDispatch),
    RemovedHook(HookDispatch),
    RemovedMcp(McpDispatch),
    RemovedLsp(LanguageServerDispatch),
    RemovedMonitor(MonitorDispatch),
}

impl UndoStep {
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::RegisteredSkill(key) => format!("registered skill {key}"),
            Self::RegisteredAgent(key) => format!("registered agent {key}"),
            Self::RegisteredHook(key) => format!("registered hook {key}"),
            Self::RegisteredMcp(key) => format!("registered mcp {key}"),
            Self::RegisteredLsp(key) => format!("registered lsp {key}"),
            Self::RegisteredMonitor(key) => format!("registered monitor {key}"),
            Self::RemovedSkill(dispatch) => format!("removed skill {}", skill_key(dispatch)),
            Self::RemovedAgent(dispatch) => format!("removed agent {}", agent_key(dispatch)),
            Self::RemovedHook(dispatch) => format!("removed hook {}", hook_key(dispatch)),
            Self::RemovedMcp(dispatch) => format!("removed mcp {}", mcp_key(dispatch)),
            Self::RemovedLsp(dispatch) => format!("removed lsp {}", lsp_key(dispatch)),
            Self::RemovedMonitor(dispatch) => {
                format!("removed monitor {}", monitor_key(dispatch))
            }
        }
    }
}

/// 把宿主接口适配为 plugin_package 的分发 sink，并为每个成功注册记录补偿步骤。
pub(crate) struct RegisteringSink<'a> {
    host: &'a mut dyn ResourceHost,
    undo: Vec<UndoStep>,
}

impl<'a> RegisteringSink<'a> {
    pub(crate) fn new(host: &'a mut dyn ResourceHost, undo: Vec<UndoStep>) -> Self {
        Self { host, undo }
    }

    pub(crate) fn into_parts(self) -> (&'a mut dyn ResourceHost, Vec<UndoStep>) {
        (self.host, self.undo)
    }
}

fn to_package_error(error: MarketplaceError) -> PackageError {
    PackageError::Conflict(error.to_string())
}

impl PackageDispatchSink for RegisteringSink<'_> {
    fn install_skill(&mut self, dispatch: &SkillDispatch) -> Result<(), PackageError> {
        self.host
            .register_skill(dispatch)
            .map_err(to_package_error)?;
        self.undo
            .push(UndoStep::RegisteredSkill(skill_key(dispatch)));
        Ok(())
    }

    fn install_agent_profile(
        &mut self,
        dispatch: &AgentProfileDispatch,
    ) -> Result<(), PackageError> {
        self.host
            .register_agent(dispatch)
            .map_err(to_package_error)?;
        self.undo
            .push(UndoStep::RegisteredAgent(agent_key(dispatch)));
        Ok(())
    }

    fn install_hook(&mut self, dispatch: &HookDispatch) -> Result<(), PackageError> {
        self.host
            .register_hook(dispatch)
            .map_err(to_package_error)?;
        self.undo.push(UndoStep::RegisteredHook(hook_key(dispatch)));
        Ok(())
    }

    fn install_mcp_server(&mut self, dispatch: &McpDispatch) -> Result<(), PackageError> {
        self.host.register_mcp(dispatch).map_err(to_package_error)?;
        self.undo.push(UndoStep::RegisteredMcp(mcp_key(dispatch)));
        Ok(())
    }

    fn install_language_server(
        &mut self,
        dispatch: &LanguageServerDispatch,
    ) -> Result<(), PackageError> {
        self.host.register_lsp(dispatch).map_err(to_package_error)?;
        self.undo.push(UndoStep::RegisteredLsp(lsp_key(dispatch)));
        Ok(())
    }

    fn install_monitor(&mut self, dispatch: &MonitorDispatch) -> Result<(), PackageError> {
        self.host
            .register_monitor(dispatch)
            .map_err(to_package_error)?;
        self.undo
            .push(UndoStep::RegisteredMonitor(monitor_key(dispatch)));
        Ok(())
    }
}

/// 反向补偿：已注册的新资源注销（monitor 先 stop），已注销的旧资源重新注册。
/// 返回补偿失败清单（空 = 回滚干净）。补偿失败不吞错：由调用方升级为
/// RollbackFailed 上报。
pub(crate) fn compensate(host: &mut dyn ResourceHost, undo: &[UndoStep]) -> Vec<String> {
    let mut failures = Vec::new();
    for step in undo.iter().rev() {
        let result = match step {
            UndoStep::RegisteredSkill(key) => host.unregister_skill(key),
            UndoStep::RegisteredAgent(key) => host.unregister_agent(key),
            UndoStep::RegisteredHook(key) => host.unregister_hook(key),
            UndoStep::RegisteredMcp(key) => host.unregister_mcp(key),
            UndoStep::RegisteredLsp(key) => host.unregister_lsp(key),
            UndoStep::RegisteredMonitor(key) => host
                .stop_monitor(key)
                .and_then(|()| host.unregister_monitor(key)),
            UndoStep::RemovedSkill(dispatch) => host.register_skill(dispatch),
            UndoStep::RemovedAgent(dispatch) => host.register_agent(dispatch),
            UndoStep::RemovedHook(dispatch) => host.register_hook(dispatch),
            UndoStep::RemovedMcp(dispatch) => host.register_mcp(dispatch),
            UndoStep::RemovedLsp(dispatch) => host.register_lsp(dispatch),
            UndoStep::RemovedMonitor(dispatch) => host.register_monitor(dispatch),
        };
        if let Err(error) = result {
            failures.push(format!("{}: {error}", step.describe()));
        }
    }
    failures
}

/// 注销一组旧资源（update / uninstall 共用）：monitors 先 stop 再 unregister，
/// 随后 lsp、mcp、hooks、agents、skills（与安装顺序相反）。每个成功步骤写入
/// undo 以供失败时补偿。
pub(crate) fn remove_plan(
    host: &mut dyn ResourceHost,
    plan: &DispatchPlan,
    undo: &mut Vec<UndoStep>,
) -> Result<(), MarketplaceError> {
    for dispatch in plan.monitors.iter().rev() {
        let key = monitor_key(dispatch);
        host.stop_monitor(&key)?;
        // stop 成功即记录：若 unregister 失败，补偿可重新注册该 monitor。
        undo.push(UndoStep::RemovedMonitor(dispatch.clone()));
        host.unregister_monitor(&key)?;
    }
    for dispatch in plan.lsp.iter().rev() {
        host.unregister_lsp(&lsp_key(dispatch))?;
        undo.push(UndoStep::RemovedLsp(dispatch.clone()));
    }
    for dispatch in plan.mcp.iter().rev() {
        host.unregister_mcp(&mcp_key(dispatch))?;
        undo.push(UndoStep::RemovedMcp(dispatch.clone()));
    }
    for dispatch in plan.hooks.iter().rev() {
        host.unregister_hook(&hook_key(dispatch))?;
        undo.push(UndoStep::RemovedHook(dispatch.clone()));
    }
    for dispatch in plan.agents.iter().rev() {
        host.unregister_agent(&agent_key(dispatch))?;
        undo.push(UndoStep::RemovedAgent(dispatch.clone()));
    }
    for dispatch in plan.skills.iter().rev() {
        host.unregister_skill(&skill_key(dispatch))?;
        undo.push(UndoStep::RemovedSkill(dispatch.clone()));
    }
    Ok(())
}

/// 记录型 mock 宿主：记录 canonical 操作序列与注册态，支持注入一次性失败，
/// 用于事务语义（顺序 / 回滚 / monitor stop）的定向测试。
///
/// canonical 操作形如 register <kind> <key> / unregister <kind> <key> /
/// stop monitor <key>；kind 取 skill、agent、hook、mcp、lsp、monitor。
#[derive(Default)]
pub struct RecordingHost {
    /// 成功执行的 canonical 操作序列（按调用顺序）。
    pub ops: Vec<String>,
    registered: std::collections::BTreeSet<String>,
    fail_target: Option<String>,
}

impl RecordingHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// 下一个包含 target 的操作失败一次（随后清除）。
    pub fn fail_next(&mut self, target: impl Into<String>) {
        self.fail_target = Some(target.into());
    }

    pub fn is_registered(&self, kind: &str, key: &str) -> bool {
        self.registered.contains(&slot(kind, key))
    }

    pub fn registered_count(&self) -> usize {
        self.registered.len()
    }

    fn check_failure(&mut self, op: &str) -> Result<(), MarketplaceError> {
        if let Some(target) = self.fail_target.take() {
            if op.contains(&target) {
                return Err(MarketplaceError::Host {
                    op: "injected",
                    resource: op.to_string(),
                    message: "injected failure".into(),
                });
            }
            self.fail_target = Some(target);
        }
        Ok(())
    }

    fn register(&mut self, kind: &'static str, key: &str) -> Result<(), MarketplaceError> {
        let slot = slot(kind, key);
        if self.registered.contains(&slot) {
            return Err(MarketplaceError::ResourceConflict {
                kind: kind.into(),
                key: key.into(),
                package: "<host>".into(),
            });
        }
        let op = format!("register {slot}");
        self.check_failure(&op)?;
        self.ops.push(op);
        self.registered.insert(slot);
        Ok(())
    }

    fn unregister(&mut self, kind: &'static str, key: &str) -> Result<(), MarketplaceError> {
        let slot = slot(kind, key);
        if !self.registered.contains(&slot) {
            return Err(MarketplaceError::Host {
                op: "unregister",
                resource: slot,
                message: "resource is not registered".into(),
            });
        }
        let op = format!("unregister {slot}");
        self.check_failure(&op)?;
        self.ops.push(op);
        self.registered.remove(&slot);
        Ok(())
    }
}

fn slot(kind: &str, key: &str) -> String {
    format!("{kind} {key}")
}

impl ResourceHost for RecordingHost {
    fn register_skill(&mut self, dispatch: &SkillDispatch) -> Result<(), MarketplaceError> {
        self.register("skill", &skill_key(dispatch))
    }

    fn register_agent(&mut self, dispatch: &AgentProfileDispatch) -> Result<(), MarketplaceError> {
        self.register("agent", &agent_key(dispatch))
    }

    fn register_hook(&mut self, dispatch: &HookDispatch) -> Result<(), MarketplaceError> {
        self.register("hook", &hook_key(dispatch))
    }

    fn register_mcp(&mut self, dispatch: &McpDispatch) -> Result<(), MarketplaceError> {
        self.register("mcp", &mcp_key(dispatch))
    }

    fn register_lsp(&mut self, dispatch: &LanguageServerDispatch) -> Result<(), MarketplaceError> {
        self.register("lsp", &lsp_key(dispatch))
    }

    fn register_monitor(&mut self, dispatch: &MonitorDispatch) -> Result<(), MarketplaceError> {
        self.register("monitor", &monitor_key(dispatch))
    }

    fn unregister_skill(&mut self, key: &str) -> Result<(), MarketplaceError> {
        self.unregister("skill", key)
    }

    fn unregister_agent(&mut self, key: &str) -> Result<(), MarketplaceError> {
        self.unregister("agent", key)
    }

    fn unregister_hook(&mut self, key: &str) -> Result<(), MarketplaceError> {
        self.unregister("hook", key)
    }

    fn unregister_mcp(&mut self, key: &str) -> Result<(), MarketplaceError> {
        self.unregister("mcp", key)
    }

    fn unregister_lsp(&mut self, key: &str) -> Result<(), MarketplaceError> {
        self.unregister("lsp", key)
    }

    fn stop_monitor(&mut self, monitor_id: &str) -> Result<(), MarketplaceError> {
        let slot = slot("monitor", monitor_id);
        if !self.registered.contains(&slot) {
            return Err(MarketplaceError::Host {
                op: "stop",
                resource: slot,
                message: "monitor is not registered".into(),
            });
        }
        let op = format!("stop monitor {monitor_id}");
        self.check_failure(&op)?;
        self.ops.push(op);
        Ok(())
    }

    fn unregister_monitor(&mut self, monitor_id: &str) -> Result<(), MarketplaceError> {
        self.unregister("monitor", monitor_id)
    }
}
