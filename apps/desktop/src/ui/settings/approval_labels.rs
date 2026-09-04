//! 审批模式 UI 文案（render / AX 同源）；不平行维护第二套枚举。

use pawork_client::ApprovalModeWire;

pub const ALL: [ApprovalModeWire; 5] = [
    ApprovalModeWire::AlwaysAsk,
    ApprovalModeWire::AskForWrites,
    ApprovalModeWire::AskForDangerous,
    ApprovalModeWire::NeverAsk,
    ApprovalModeWire::ReadOnly,
];

pub fn label(mode: ApprovalModeWire) -> &'static str {
    match mode {
        ApprovalModeWire::AlwaysAsk => "Always ask",
        ApprovalModeWire::AskForWrites => "Ask for writes",
        ApprovalModeWire::AskForDangerous => "Ask for dangerous actions",
        ApprovalModeWire::NeverAsk => "Never ask",
        ApprovalModeWire::ReadOnly => "Read only",
    }
}

pub fn description(mode: ApprovalModeWire) -> &'static str {
    match mode {
        ApprovalModeWire::AlwaysAsk => "Require approval for every tool call",
        ApprovalModeWire::AskForWrites => "Allow reads; require approval for writes",
        ApprovalModeWire::AskForDangerous => "Allow routine actions; ask before dangerous actions",
        ApprovalModeWire::NeverAsk => {
            "Run automatically; the Host still blocks catastrophic commands"
        }
        ApprovalModeWire::ReadOnly => "Allow read-only actions and block all writes",
    }
}
