# P17-4：LSP Runtime（语言服务运行时）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P4-12、P1-9、P8-1、P11-7

**最终目的**：实现 LSP（Language Server Protocol）运行时，让 Pawork 作为 LSP server 暴露代码智能：diagnostics、hover、definition、references、document symbol、call hierarchy、rename。提供标准 JSON-RPC over stdio 传输、文档管理、能力协商，并与具体语言分析器解耦（analysis adapter），使编辑器可直接连接获得 Pawork 提供的智能。本任务交付**协议与运行时骨架**及契约测试，不在本任务实现完整语言语义。

**涉及范围**：新增 `lsp-runtime`；复用 `jsonl-parser` / `partial-json`（消息解析）、`logging`、`resource-loader`

## 细分步骤

1. **传输与会话** —— 目的：JSON-RPC over stdio 的帧解析（复用 [P2-2](P2-2-sse-parser.md) / [P2-3](P2-3-jsonl-parser.md) 解析能力）、initialize / shutdown 握手、能力协商（capabilities advertise）。
2. **文档管理** —— 目的：`textDocument/didOpen|didChange|didClose`，维护文档版本与增量同步（text document manager），作为所有请求的源数据。
3. **diagnostics** —— 目的：`textDocument/publishDiagnostics` 推送，对接 analysis adapter 输出错误 / 警告。
4. **hover / definition / references** —— 目的：实现 hover（类型 / 文档提示）、go-to definition、find references，基于 adapter 的符号解析。
5. **document symbol / call hierarchy** —— 目的：document symbol（大纲）与 call hierarchy（prepare / incoming / outgoing 调用链）。
6. **rename** —— 目的：`textDocument/rename` + `prepareRename`，跨文件 WorkspaceEdit，写操作经 `policy-engine` / sandbox 约束（走既有 edit-file 能力）。
7. **analysis adapter** —— 目的：定义统一分析适配接口，具体语言分析器（内置或外部）按接口接入；默认提供最小 / 契约实现。
8. **定向 / Mock 测试** —— 目的：LSP 契约测试（用 mock client 覆盖各方法）、能力协商、文档增量同步、rename 经策略约束。仅定向 + Mock。

## 主要产出物

- `lsp-runtime`：传输 + 文档管理 + 七项能力 + analysis adapter 接口
- LSP 契约测试

## 验收标准

- [ ] 支持诊断 / 悬停 / 定义 / 引用 / 符号 / 调用层级 / 重命名七项 LSP 能力
- [ ] JSON-RPC over stdio 与能力协商符合 LSP 契约
- [ ] 文档增量同步正确，rename 等写操作经策略约束
- [ ] analysis adapter 解耦，可接入内置或外部分析器

**相关文档**：[workspace-index](../docs/features/workspace-index.md) · [tools](../docs/features/tools.md) · [process](../docs/features/process.md) · [policy](../docs/features/policy.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：JSON-RPC / LSP 帧优先用 [P2-2](P2-2-sse-parser.md) / [P2-3](P2-3-jsonl-parser.md) 解析能力自实现，不新增第三方依赖；仅在用户明确确认后才评估引入 `tower-lsp` 之类。复用 `logging` / `resource-loader`。新 crate `lsp-runtime` 依赖方向：`agent-domain → lsp-runtime → app-service`（作为可选服务暴露）。
