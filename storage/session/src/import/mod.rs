//! Session 导入器：外部会话解析（`formats`）与 store 侧事务写入。
//!
//! 解析纯函数在 [`formats`]；`SessionStore` 导入 / 导出入口在本目录 persist 模块。

pub mod formats;

mod persist_compat;
mod persist_export;
mod persist_pi;

pub use formats::{
    parse_pi_line, CompatImportHistoryEntry, CompatImportHistoryPage, CompatImportReport,
    ExportedBranch, ExportedEvent, ExternalRecord, ExternalSource, ParsedExternalSession,
    PiEntryKind, PiImportReport, PiParsedEntry, PiPayload, SessionExport, EXPORT_SCHEMA_VERSION,
};
