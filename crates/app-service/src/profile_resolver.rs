//! P17-5 主 run profile 解析（RunStart 到 ProviderLoop 的权威配置来源）。
//!
//! 与 user-hook 的 EvalProfileResolver（P17-1，受限单轮判定落点）不同，本
//! 模块把可选 profile 名解析为 loader 已校验的完整不可变 AgentProfileV2
//! （prompt / canonical effort / tools / max_turns / background / isolation /
//! memory），作为该 run 的权威配置。复用 P17-1 ResourceLoader 加载结果，
//! 不重复 IO、不复制 run/task/sandbox 状态机。
//!
//! 解析失败（未知 / 跨 workspace / 引用不可用）一律 fail-closed，绝不静默
//! 回退默认模型或默认 profile。

use agent_domain::{AgentProfileV2, ModelId, ProfileIsolation, ProviderId, WorkspaceId};
use core_api::{ActorIdentity, CommandSource};

/// 已解析的主 run profile：loader 校验过的不可变 AgentProfileV2 + workspace
/// 绑定。重试与取消沿用同一不可变实例（retry 保持 profile）。
#[derive(Clone, Debug)]
pub struct ResolvedRunProfile {
    pub workspace_id: WorkspaceId,
    pub profile: AgentProfileV2,
}

/// 主 run profile 解析错误（结构化 fail-closed）。
#[derive(Debug, thiserror::Error)]
pub enum ProfileResolveError {
    #[error("profile `{name}` is not registered in workspace `{workspace}`")]
    Unknown {
        name: String,
        workspace: WorkspaceId,
    },
    #[error("profile name `{name}` is ambiguous in workspace `{workspace}`")]
    Ambiguous {
        name: String,
        workspace: WorkspaceId,
    },
    #[error("profile `{name}` is registered in workspace `{actual}`, not `{requested}`")]
    WrongWorkspace {
        name: String,
        actual: WorkspaceId,
        requested: WorkspaceId,
    },
    #[error("profile `{name}` has unresolved references and is unavailable")]
    ReferenceUnavailable { name: String },
}

/// 主 run profile 解析契约（P17-5）。
///
/// 宿主用生产 ResourceLoader 装配实现并注入；未注入时 RunStart 携带 profile
/// 名一律 fail-closed（无可用 profile 源）。实现只查表返回 loader 已校验的
/// 不可变 profile，不重复 IO、不做 Provider 名分支。
pub trait RunProfileResolver: Send + Sync {
    fn resolve(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<ResolvedRunProfile, ProfileResolveError>;
}

/// 隔离能力探测契约（P17-5）：决定 ProfileIsolation 在当前宿主能否被真实满足。
/// 绝不虚假可用——Container 在无真实硬 / 容器后端时必须报告不可用，RunStart
/// 随之 fail-closed，绝不静默降级。
pub trait IsolationCapability: Send + Sync {
    /// 本机受限沙箱（NativeRestricted 级软约束）是否可用。
    fn soft_isolation_available(&self) -> bool;
    /// 真实容器 / 硬隔离后端是否可用。
    fn hard_container_available(&self) -> bool;

    /// 该隔离等级在当前宿主是否可被真实满足（None 永远可满足）。
    fn satisfiable(&self, isolation: ProfileIsolation) -> bool {
        match isolation {
            ProfileIsolation::None => true,
            ProfileIsolation::Restricted => self.soft_isolation_available(),
            ProfileIsolation::Container => self.hard_container_available(),
        }
    }
}

/// 生产隔离能力探测（P17-5）：fail-closed——主 run 链当前**没有**真实隔离
/// 执行器接线（P17-5 工具执行为 P13-1 no-op runtime，`AppLoopContext`
/// 只把 isolation 作为约束上下文传播，不强制），因此 Restricted 与
/// Container 一律不可满足，RunStart 随之拒绝，绝不虚假可用 / 静默降级。
/// 真实执行器（sandbox-runtime 的 NativeRestricted / 平台硬隔离后端）接入
/// 工具执行路径后，只需在此按能力翻转对应开关。
#[derive(Clone, Copy, Debug, Default)]
pub struct SandboxIsolationCapability;

impl IsolationCapability for SandboxIsolationCapability {
    fn soft_isolation_available(&self) -> bool {
        false
    }
    fn hard_container_available(&self) -> bool {
        false
    }
}

// ===== P17-5 模型覆盖授权（ModelOverridePolicy） =====

/// 模型落点：provider + model 的 canonical 对（注册表解析后）。
///
/// 显式命令模型与 profile canonical 模型都以解析后的落点比较——同一模型经
/// 别名 / 大小写归一后落点相同即不构成 override（不误拒），落点不同才需要
/// 授权。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelLanding {
    pub provider_id: ProviderId,
    pub model_id: ModelId,
}

/// 模型覆盖授权请求：RunStart 显式模型与 profile canonical 落点不同时的
/// 完整判定上下文（source + identity + workspace + profile/from/to）。
#[derive(Clone, Debug)]
pub struct ModelOverrideRequest {
    pub source: CommandSource,
    pub identity: ActorIdentity,
    pub workspace_id: WorkspaceId,
    pub profile_name: String,
    /// profile canonical 模型落点（被覆盖的 from）。
    pub from: ModelLanding,
    /// 显式命令模型的落点（覆盖后的 to）。
    pub to: ModelLanding,
}

/// 模型覆盖授权判定（结构化；由注入策略自行决定）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelOverrideDecision {
    Allow,
    Deny,
}

/// 模型覆盖授权契约（P17-5）：显式模型与 profile canonical 模型落点不同时，
/// 由宿主注入的策略裁决，绝不直接信任 caller。缺省（未注入）为
/// [`DenyAllModelOverridePolicy`]，fail-closed。
pub trait ModelOverridePolicy: Send + Sync {
    fn allow(&self, request: &ModelOverrideRequest) -> ModelOverrideDecision;
}

/// 缺省策略：一律拒绝（fail-closed）。未注入策略时的唯一行为，保证
/// “only policy permitting” 的默认语义。
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAllModelOverridePolicy;

impl ModelOverridePolicy for DenyAllModelOverridePolicy {
    fn allow(&self, _request: &ModelOverrideRequest) -> ModelOverrideDecision {
        ModelOverrideDecision::Deny
    }
}

/// 生产策略（pawork 正式宿主显式注入）：最多允许本机交互来源
/// （LocalCli / LocalGui）+ LocalUser 身份覆盖 profile 锁定模型；Remote /
/// Automation / Plugin / MCP 一律拒绝。
///
/// System 默认拒绝：System 身份用于内部 / 无人值守服务动作，模型覆盖应走
/// 显式 profile / 配置而非隐式 caller 权威；确需放行时由宿主注入自定义
/// 策略显式授权。
#[derive(Clone, Copy, Debug, Default)]
pub struct ProductionModelOverridePolicy;

impl ModelOverridePolicy for ProductionModelOverridePolicy {
    fn allow(&self, request: &ModelOverrideRequest) -> ModelOverrideDecision {
        let local_source = matches!(
            request.source,
            CommandSource::LocalCli { .. } | CommandSource::LocalGui { .. }
        );
        let local_user = matches!(request.identity, ActorIdentity::LocalUser { .. });
        if local_source && local_user {
            ModelOverrideDecision::Allow
        } else {
            ModelOverrideDecision::Deny
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_capability_fails_closed_without_real_isolation_executor() {
        let cap = SandboxIsolationCapability;
        // 主 run 链无真实隔离执行器接线（P13-1 no-op runtime）：Restricted
        // 与 Container 都必须 fail-closed，绝不虚假可用。
        assert!(!cap.soft_isolation_available());
        assert!(!cap.hard_container_available());
        assert!(cap.satisfiable(ProfileIsolation::None));
        assert!(!cap.satisfiable(ProfileIsolation::Restricted));
        assert!(!cap.satisfiable(ProfileIsolation::Container));
    }

    fn landing(provider: &str, model: &str) -> ModelLanding {
        ModelLanding {
            provider_id: ProviderId::from(provider),
            model_id: ModelId::from(model),
        }
    }

    fn request(source: CommandSource, identity: ActorIdentity) -> ModelOverrideRequest {
        ModelOverrideRequest {
            source,
            identity,
            workspace_id: WorkspaceId::from("ws"),
            profile_name: "reviewer".into(),
            from: landing("openai", "gpt-4o"),
            to: landing("openai", "gpt-4o-mini"),
        }
    }

    fn local_user() -> ActorIdentity {
        ActorIdentity::LocalUser {
            actor_id: agent_domain::ActorId::from("tester"),
            display_name: None,
        }
    }

    #[test]
    fn default_policy_denies_every_override() {
        // 缺省策略必须 fail-closed：即使本机 LocalUser 也拒绝。
        let policy = DenyAllModelOverridePolicy;
        assert_eq!(
            policy.allow(&request(
                CommandSource::LocalCli {
                    terminal_session_id: None
                },
                local_user()
            )),
            ModelOverrideDecision::Deny
        );
    }

    #[test]
    fn production_policy_allows_only_local_source_and_local_user() {
        let policy = ProductionModelOverridePolicy;
        assert_eq!(
            policy.allow(&request(
                CommandSource::LocalCli {
                    terminal_session_id: None
                },
                local_user()
            )),
            ModelOverrideDecision::Allow
        );
        assert_eq!(
            policy.allow(&request(
                CommandSource::LocalGui {
                    client_id: agent_domain::GuiClientId::from("gui-1"),
                },
                local_user()
            )),
            ModelOverrideDecision::Allow
        );
    }

    #[test]
    fn production_policy_rejects_remote_automation_plugin_mcp_and_system() {
        let policy = ProductionModelOverridePolicy;
        let remote = request(
            CommandSource::RemoteGui {
                client_id: agent_domain::GuiClientId::from("remote-1"),
                connection_id: agent_domain::ConnectionId::from("conn-1"),
            },
            local_user(),
        );
        assert_eq!(policy.allow(&remote), ModelOverrideDecision::Deny);
        assert_eq!(
            policy.allow(&request(CommandSource::Automation, local_user())),
            ModelOverrideDecision::Deny
        );
        assert_eq!(
            policy.allow(&request(
                CommandSource::Plugin,
                ActorIdentity::Plugin {
                    plugin_id: agent_domain::PluginId::from("p")
                }
            )),
            ModelOverrideDecision::Deny
        );
        assert_eq!(
            policy.allow(&request(
                CommandSource::Mcp,
                ActorIdentity::McpServer {
                    server_id: "mcp".into()
                }
            )),
            ModelOverrideDecision::Deny
        );
        // System 默认拒绝（无明确理由不放行）。
        assert_eq!(
            policy.allow(&request(CommandSource::Automation, ActorIdentity::System)),
            ModelOverrideDecision::Deny
        );
    }
}
