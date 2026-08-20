//! 策略引擎：综合审批模式、能力、信任与命令风险给出裁决。

use serde_json::Value;
use pawork_domain::ToolCapability;

use crate::decision::{
    ApprovalPrompt, CommandRisk, ExecutionConstraints, PolicyDecision, RiskLevel,
};
use crate::mode::ApprovalMode;
use crate::shell::{classify_command, hits_danger_floor};

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
    pub allowed_in_untrusted_workspace: bool,
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

        // Descriptor 是未信任工作区的第一道硬门：未显式声明可用则一律拒绝。
        if !input.trusted && !input.allowed_in_untrusted_workspace {
            return PolicyDecision::Deny {
                reason: "tool is not allowed in an untrusted workspace".into(),
            };
        }

        // 灾难命令地板：即使 trusted + NeverAsk 也不得静默执行。
        if matches!(cap, ToolCapability::Process) && command_hits_danger_floor(&input.input) {
            return match mode {
                ApprovalMode::NeverAsk | ApprovalMode::ReadOnly => {
                    PolicyDecision::Deny {
                        reason: "catastrophic command cannot run without explicit pre-approval"
                            .into(),
                    }
                }
                ApprovalMode::AlwaysAsk
                | ApprovalMode::AskForWrites
                | ApprovalMode::AskForDangerous => ask(cap, input, RiskLevel::Dangerous),
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
            ApprovalMode::NeverAsk => allow_or_constrained(cap),
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

fn command_hits_danger_floor(input: &Value) -> bool {
    extract_command(input).is_some_and(|(program, args)| hits_danger_floor(&program, &args))
}

/// 从工具入参中提取命令。
///
/// 优先认非空 `argv`（`argv[0]` 为 program、其余为 args），与 `run_command`
/// 实际执行形状一致；否则保留 `program` / `command` / `cmd` + `args`。
fn extract_command(input: &Value) -> Option<(String, Vec<String>)> {
    if let Some(argv) = input.get("argv").and_then(|v| v.as_array()) {
        if !argv.is_empty() {
            if let Some(program) = argv[0].as_str() {
                return Some((program.to_string(), read_args(input)));
            }
        }
    }
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
    if let Some(argv) = input.get("argv").and_then(|v| v.as_array()) {
        if !argv.is_empty() {
            return argv
                .iter()
                .skip(1)
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
        }
    }
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
    use pawork_domain::ToolCapability;

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
            allowed_in_untrusted_workspace: trusted,
            approval_mode: mode,
        }
    }

    fn untrusted_allowed_input(
        cap: ToolCapability,
        mode: ApprovalMode,
        value: serde_json::Value,
    ) -> PolicyInput {
        PolicyInput {
            capability: cap,
            input: value,
            trusted: false,
            allowed_in_untrusted_workspace: true,
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
        let dec = eng.decide(&untrusted_allowed_input(
            ToolCapability::ReadOnly,
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
    fn untrusted_descriptor_permission_continues_through_policy() {
        let eng = PolicyEngine::new(ApprovalMode::NeverAsk);
        let dec = eng.decide(&untrusted_allowed_input(
            ToolCapability::WorkspaceWrite,
            ApprovalMode::NeverAsk,
            json!({}),
        ));
        assert_eq!(dec, PolicyDecision::Allow);
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
    fn never_ask_denies_catastrophic_commands() {
        let eng = PolicyEngine::new(ApprovalMode::NeverAsk);
        for command in [
            json!({"command": "rm", "args": ["-rf", "/"]}),
            json!({"command": "mkfs", "args": ["/dev/sda1"]}),
            json!({"command": "dd", "args": ["if=image", "of=/dev/sda"]}),
        ] {
            let dec = eng.decide(&input(
                ToolCapability::Process,
                true,
                ApprovalMode::NeverAsk,
                command,
            ));
            assert!(
                matches!(dec, PolicyDecision::Deny { .. }),
                "unexpected decision: {dec:?}"
            );
        }
    }

    #[test]
    fn argv_catastrophic_is_denied_in_never_ask_trusted_process() {
        let eng = PolicyEngine::new(ApprovalMode::NeverAsk);
        let dec = eng.decide(&input(
            ToolCapability::Process,
            true,
            ApprovalMode::NeverAsk,
            json!({"argv": ["rm", "-rf", "/"]}),
        ));
        assert!(
            matches!(dec, PolicyDecision::Deny { .. }),
            "unexpected decision: {dec:?}"
        );
    }

    #[test]
    fn argv_force_push_asks_user_as_dangerous() {
        let eng = PolicyEngine::new(ApprovalMode::AskForDangerous);
        let dec = eng.decide(&input(
            ToolCapability::Process,
            true,
            ApprovalMode::AskForDangerous,
            json!({"argv": ["git", "push", "--force"]}),
        ));
        match dec {
            PolicyDecision::AskUser { prompt } => assert_eq!(prompt.risk, RiskLevel::Dangerous),
            other => panic!("expected AskUser, got {other:?}"),
        }
    }

    #[test]
    fn argv_safe_process_allows_with_constraints() {
        let eng = PolicyEngine::new(ApprovalMode::AskForDangerous);
        let dec = eng.decide(&input(
            ToolCapability::Process,
            true,
            ApprovalMode::AskForDangerous,
            json!({"argv": ["ls"]}),
        ));
        match dec {
            PolicyDecision::AllowWithConstraints { constraints } => {
                assert!(constraints.timeout_ms.is_some());
            }
            other => panic!("expected AllowWithConstraints, got {other:?}"),
        }
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
        assert_eq!(
            super::command_risk(&json!({"argv": ["git", "push", "--force"]})),
            CommandRisk::Dangerous
        );
    }

    #[test]
    fn extract_command_prefers_argv_over_command_and_args() {
        assert_eq!(
            super::extract_command(&json!({
                "command": "echo",
                "args": ["hi"],
                "argv": ["rm", "-rf", "/"]
            })),
            Some(("rm".into(), vec!["-rf".into(), "/".into()]))
        );
        assert_eq!(
            super::extract_command(&json!({"command": "rm", "args": ["-rf", "/"]})),
            Some(("rm".into(), vec!["-rf".into(), "/".into()]))
        );
        assert_eq!(
            super::extract_command(&json!({"program": "ls", "args": ["-l"]})),
            Some(("ls".into(), vec!["-l".into()]))
        );
    }

    #[test]
    fn engine_mode_getter_returns_default() {
        let eng = PolicyEngine::new(ApprovalMode::AskForDangerous);
        assert_eq!(eng.mode(), ApprovalMode::AskForDangerous);
    }

}
