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
        ApprovalModeWire::AlwaysAsk => "每次询问",
        ApprovalModeWire::AskForWrites => "写操作询问",
        ApprovalModeWire::AskForDangerous => "危险操作询问",
        ApprovalModeWire::NeverAsk => "从不询问",
        ApprovalModeWire::ReadOnly => "只读",
    }
}

pub fn description(mode: ApprovalModeWire) -> &'static str {
    match mode {
        ApprovalModeWire::AlwaysAsk => "所有工具调用都需要人工批准",
        ApprovalModeWire::AskForWrites => "只读放行，写操作需要批准",
        ApprovalModeWire::AskForDangerous => "常规操作放行，危险操作需要批准",
        ApprovalModeWire::NeverAsk => "全部自动执行；灾难命令仍被 Host 拒绝",
        ApprovalModeWire::ReadOnly => "只放行只读操作，不执行任何写操作",
    }
}
