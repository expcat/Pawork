//! 审批模式 UI 文案（render / AX 同源）；不平行维护第二套枚举。

use crate::ui::i18n::t;
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
        ApprovalModeWire::AlwaysAsk => t("approval.mode.always_ask"),
        ApprovalModeWire::AskForWrites => t("approval.mode.ask_for_writes"),
        ApprovalModeWire::AskForDangerous => t("approval.mode.ask_for_dangerous"),
        ApprovalModeWire::NeverAsk => t("approval.mode.never_ask"),
        ApprovalModeWire::ReadOnly => t("approval.mode.read_only"),
    }
}

pub fn description(mode: ApprovalModeWire) -> &'static str {
    match mode {
        ApprovalModeWire::AlwaysAsk => t("approval.mode_desc.always_ask"),
        ApprovalModeWire::AskForWrites => t("approval.mode_desc.ask_for_writes"),
        ApprovalModeWire::AskForDangerous => t("approval.mode_desc.ask_for_dangerous"),
        ApprovalModeWire::NeverAsk => t("approval.mode_desc.never_ask"),
        ApprovalModeWire::ReadOnly => t("approval.mode_desc.read_only"),
    }
}
