//! 策略引擎：综合审批模式、能力、信任与命令风险给出裁决。

use serde_json::Value;
use tool_api::ToolCapability;

use crate::decision::{
    ApprovalPrompt, CommandRisk, ExecutionConstraints, PolicyDecision, RiskLevel,
};
use crate::mode::ApprovalMode;
use crate::shell::classify_command;

/// 进程执行允许时的默认超时（毫秒）。
const DEFAULT_PROCESS_TIMEOUT_MS: u64 = 60_000;
/// 进程执行允许时的默认输出上限（字节）。
const DEFAULT_PROCESS_MAX_OUTPUT_BYTES: u64 = 1_048_576; // 1 MiB

/// 一次策略裁决的输入。
#[derive(Clone, Debug)]
pub struct PolicyInput {
    pub capability: ToolCapability,
    pub input: Value,
    pub trusted: bool,
    pub approval_mode: ApprovalMode,
}

/// 策略引擎。按构造时的默认 [`ApprovalMode`] 运行；`decide` 以
/// [`PolicyInput::approval_mode`] 为准（支持按调用覆盖）。
#[derive(Clone, Debug)]
pub struct PolicyEngine {
    mode: ApprovalMode,
}

impl PolicyEngine {
    pub fn new(mode: ApprovalMode) -> Self {
        Self { mode }
    }

    /// 引擎构造时的默认审批模式。
    pub fn mode(&self) -> ApprovalMode {
        self.mode
    }

    pub fn decide(&self, input: &PolicyInput) -> PolicyDecision {
        let cap = &input.capability;
        let mode = input.approval_mode;

        // 硬性安全：未信任工作区禁止任何有副作用的写/进程/网络能力。
        if !input.trusted && requires_trust(cap) {
            return PolicyDecision::Deny {
                reason: format!("untrusted workspace forbids {:?} capability", cap),
            };
        }

        // 只读能力在通过信任检查后始终放行。
        if matches!(cap, ToolCapability::ReadOnly) {
            return PolicyDecision::Allow;
        }

        match mode {
            ApprovalMode::ReadOnly => PolicyDecision::Deny {
                reason: "read_only approval mode forbids non-read-only capabilities".into(),
            },
            ApprovalMode::NeverAsk | ApprovalMode::OnFailure => allow_or_constrained(cap),
            ApprovalMode::AlwaysAsk => ask(cap, input, effective_risk(cap, input)),
            ApprovalMode::AskForWrites => {
                if is_side_effecting(cap) {
                    ask(cap, input, effective_risk(cap, input))
                } else {
                    PolicyDecision::Allow
                }
            }
            ApprovalMode::AskForDangerous => {
                let risk = effective_risk(cap, input);
                if risk == RiskLevel::Dangerous {
                    ask(cap, input, RiskLevel::Dangerous)
                } else {
                    allow_or_constrained(cap)
                }
            }
        }
    }
}

/// 需要工作区受信任的能力（写/进程/网络）。
fn requires_trust(cap: &ToolCapability) -> bool {
    matches!(
        cap,
        ToolCapability::WorkspaceWrite
            | ToolCapability::GitWrite
            | ToolCapability::Process
            | ToolCapability::Network
    )
}

/// 是否产生副作用（除只读与用户交互外均为是）。
fn is_side_effecting(cap: &ToolCapability) -> bool {
    !matches!(
        cap,
        ToolCapability::ReadOnly | ToolCapability::UserInteraction
    )
}

/// 放行；对进程能力附加默认资源约束。
fn allow_or_constrained(cap: &ToolCapability) -> PolicyDecision {
    if matches!(cap, ToolCapability::Process) {
        PolicyDecision::AllowWithConstraints {
            constraints: ExecutionConstraints {
                timeout_ms: Some(DEFAULT_PROCESS_TIMEOUT_MS),
                max_output_bytes: Some(DEFAULT_PROCESS_MAX_OUTPUT_BYTES),
            },
        }
    } else {
        PolicyDecision::Allow
    }
}

/// 综合能力与命令内容得到风险等级。
fn effective_risk(cap: &ToolCapability, input: &PolicyInput) -> RiskLevel {
    match cap {
        ToolCapability::ReadOnly | ToolCapability::UserInteraction => RiskLevel::Safe,
        ToolCapability::Process => match command_risk(&input.input) {
            CommandRisk::Dangerous => RiskLevel::Dangerous,
            CommandRisk::Safe => RiskLevel::Moderate,
        },
        _ => RiskLevel::Moderate,
    }
}

fn ask(cap: &ToolCapability, input: &PolicyInput, risk: RiskLevel) -> PolicyDecision {
    PolicyDecision::AskUser {
        prompt: ApprovalPrompt {
            message: ask_message(cap, input),
            risk,
        },
    }
}

fn ask_message(cap: &ToolCapability, input: &PolicyInput) -> String {
    match cap {
        ToolCapability::Process => {
            let (prog, args) = extract_command(&input.input).unwrap_or_default();
            format!("Approve command: {} {}", prog, args.join(" "))
        }
        ToolCapability::WorkspaceWrite => "Approve workspace file write".into(),
        ToolCapability::GitWrite => "Approve git write".into(),
        ToolCapability::Network => "Approve network access".into(),
        ToolCapability::ExternalPlugin => "Approve external plugin execution".into(),
        ToolCapability::UserInteraction => "Approve user interaction".into(),
        ToolCapability::ReadOnly => "Approve read".into(),
    }
}

fn command_risk(input: &Value) -> CommandRisk {
    match extract_command(input) {
        Some((prog, args)) => classify_command(&prog, &args),
        None => CommandRisk::Safe,
    }
}

/// 从工具入参中提取命令（支持 `program`/`command`/`cmd` + `args` 多种形状）。
fn extract_command(input: &Value) -> Option<(String, Vec<String>)> {
    if let Some(prog) = input.get("program").and_then(|v| v.as_str()) {
        return Some((prog.to_string(), read_args(input)));
    }
    if let Some(cmd) = input
        .get("command")
        .and_then(|v| v.as_str())
        .or_else(|| input.get("cmd").and_then(|v| v.as_str()))
    {
        return Some((cmd.to_string(), read_args(input)));
    }
    None
}

fn read_args(input: &Value) -> Vec<String> {
    input
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{PolicyEngine, PolicyInput};
    use crate::decision::{CommandRisk, PolicyDecision, RiskLevel};
    use crate::mode::ApprovalMode;
    use serde_json::json;
    use tool_api::ToolCapability;

    fn input(
        cap: ToolCapability,
        trusted: bool,
        mode: ApprovalMode,
        value: serde_json::Value,
    ) -> PolicyInput {
        PolicyInput {
            capability: cap,
            input: value,
            trusted,
            approval_mode: mode,
        }
    }

    #[test]
    fn read_only_mode_denies_writes() {
        let eng = PolicyEngine::new(ApprovalMode::ReadOnly);
        let dec = eng.decide(&input(
            ToolCapability::WorkspaceWrite,
            true,
            ApprovalMode::ReadOnly,
            json!({}),
        ));
        assert!(matches!(dec, PolicyDecision::Deny { .. }), "{dec:?}");
    }

    #[test]
    fn read_only_mode_allows_reads() {
        let eng = PolicyEngine::new(ApprovalMode::ReadOnly);
        let dec = eng.decide(&input(
            ToolCapability::ReadOnly,
            true,
            ApprovalMode::ReadOnly,
            json!({}),
        ));
        assert_eq!(dec, PolicyDecision::Allow);
    }

    #[test]
    fn untrusted_denies_writes_even_in_never_ask() {
        let eng = PolicyEngine::new(ApprovalMode::NeverAsk);
        let dec = eng.decide(&input(
            ToolCapability::WorkspaceWrite,
            false,
            ApprovalMode::NeverAsk,
            json!({}),
        ));
        assert!(matches!(dec, PolicyDecision::Deny { .. }), "{dec:?}");
    }

    #[test]
    fn untrusted_allows_reads() {
        let eng = PolicyEngine::new(ApprovalMode::ReadOnly);
        let dec = eng.decide(&input(
            ToolCapability::ReadOnly,
            false,
            ApprovalMode::ReadOnly,
            json!({}),
        ));
        assert_eq!(dec, PolicyDecision::Allow);
    }

    #[test]
    fn trusted_relaxes_write_capability() {
        // P4-10：信任后写能力不再被硬性拒绝（按审批模式走 AskUser/Allow）。
        let eng = PolicyEngine::new(ApprovalMode::NeverAsk);
        let dec = eng.decide(&input(
            ToolCapability::WorkspaceWrite,
            true,
            ApprovalMode::NeverAsk,
            json!({}),
        ));
        assert_eq!(dec, PolicyDecision::Allow, "trusted+NeverAsk 应放行写");
    }

    #[test]
    fn trusted_relaxes_process_capability_with_constraints() {
        // P4-10：信任后进程能力在 NeverAsk 下带约束放行。
        let eng = PolicyEngine::new(ApprovalMode::NeverAsk);
        let dec = eng.decide(&input(
            ToolCapability::Process,
            true,
            ApprovalMode::NeverAsk,
            json!({"command": "echo hi"}),
        ));
        match dec {
            PolicyDecision::AllowWithConstraints { .. } | PolicyDecision::Allow => {}
            other => panic!("trusted process should be allowed, got {other:?}"),
        }
    }

    #[test]
    fn ask_for_writes_prompts_on_write() {
        let eng = PolicyEngine::new(ApprovalMode::AskForWrites);
        let dec = eng.decide(&input(
            ToolCapability::WorkspaceWrite,
            true,
            ApprovalMode::AskForWrites,
            json!({}),
        ));
        match dec {
            PolicyDecision::AskUser { prompt } => assert_eq!(prompt.risk, RiskLevel::Moderate),
            other => panic!("expected AskUser, got {other:?}"),
        }
    }

    #[test]
    fn ask_for_writes_allows_read() {
        let eng = PolicyEngine::new(ApprovalMode::AskForWrites);
        let dec = eng.decide(&input(
            ToolCapability::ReadOnly,
            true,
            ApprovalMode::AskForWrites,
            json!({}),
        ));
        assert_eq!(dec, PolicyDecision::Allow);
    }

    #[test]
    fn ask_for_dangerous_flags_dangerous_command() {
        let eng = PolicyEngine::new(ApprovalMode::AskForDangerous);
        let dec = eng.decide(&input(
            ToolCapability::Process,
            true,
            ApprovalMode::AskForDangerous,
            json!({"command": "rm", "args": ["-rf", "/"]}),
        ));
        match dec {
            PolicyDecision::AskUser { prompt } => assert_eq!(prompt.risk, RiskLevel::Dangerous),
            other => panic!("expected AskUser, got {other:?}"),
        }
    }

    #[test]
    fn ask_for_dangerous_allows_safe_process_with_constraints() {
        let eng = PolicyEngine::new(ApprovalMode::AskForDangerous);
        let dec = eng.decide(&input(
            ToolCapability::Process,
            true,
            ApprovalMode::AskForDangerous,
            json!({"command": "ls"}),
        ));
        match dec {
            PolicyDecision::AllowWithConstraints { constraints } => {
                assert!(constraints.timeout_ms.is_some());
            }
            other => panic!("expected AllowWithConstraints, got {other:?}"),
        }
    }

    #[test]
    fn never_ask_allows_process_with_constraints() {
        let eng = PolicyEngine::new(ApprovalMode::NeverAsk);
        let dec = eng.decide(&input(
            ToolCapability::Process,
            true,
            ApprovalMode::NeverAsk,
            json!({"command": "echo hi"}),
        ));
        assert!(
            matches!(dec, PolicyDecision::AllowWithConstraints { .. }),
            "{dec:?}"
        );
    }

    #[test]
    fn always_ask_prompts() {
        let eng = PolicyEngine::new(ApprovalMode::AlwaysAsk);
        let dec = eng.decide(&input(
            ToolCapability::Network,
            true,
            ApprovalMode::AlwaysAsk,
            json!({}),
        ));
        assert!(matches!(dec, PolicyDecision::AskUser { .. }), "{dec:?}");
    }

    #[test]
    fn input_mode_overrides_engine_mode() {
        // 引擎构造为 NeverAsk，但输入指定 AlwaysAsk → 应询问。
        let eng = PolicyEngine::new(ApprovalMode::NeverAsk);
        let dec = eng.decide(&input(
            ToolCapability::WorkspaceWrite,
            true,
            ApprovalMode::AlwaysAsk,
            json!({}),
        ));
        assert!(matches!(dec, PolicyDecision::AskUser { .. }), "{dec:?}");
    }

    #[test]
    fn command_risk_elevates_to_dangerous() {
        assert_eq!(
            super::command_risk(&json!({"command": "sudo", "args": ["ls"]})),
            CommandRisk::Dangerous
        );
        assert_eq!(
            super::command_risk(&json!({"program": "git", "args": ["push", "--force"]})),
            CommandRisk::Dangerous
        );
        assert_eq!(
            super::command_risk(&json!({"command": "ls"})),
            CommandRisk::Safe
        );
    }

    #[test]
    fn engine_mode_getter_returns_default() {
        let eng = PolicyEngine::new(ApprovalMode::AskForDangerous);
        assert_eq!(eng.mode(), ApprovalMode::AskForDangerous);
    }
}
