//! IDE Host Adapter feature（`ide`）。
//!
//! V1 `ide-host-adapter` 的 contract / adapter / diagnostics / lsp_output
//! 缠住 `lsp-runtime`（`DocumentDiagnostic`、`LspQueryKind` 等）。LSP 已移出
//! S0–S12，本波不迁 adapter 实现、不依赖 `lsp-runtime`、不迁 `lsp_output`。
//!
//! 本模块仅作 feature 门控占位，避免默默拉入 LSP。默认构建不含本模块。
//! V1 `client-adapter-api` 已在 [`pawork_protocol::adapter`]。
