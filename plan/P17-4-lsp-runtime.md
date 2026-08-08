# P17-4：LSP Client Runtime（语言服务客户端运行时）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P4-12、P1-9、P8-1、P11-7（协调 P17-9）

**最终目的**：实现 LSP（Language Server Protocol）客户端运行时——Pawork **作为 LSP Client**，负责启动、管理、调用现有 Language Server（rust-analyzer、pyright、typescript-language-server、gopls、clangd 等），把它们的代码智能（diagnostics / hover / definition / references / document_symbols / workspace_symbols / call_hierarchy / rename / code_actions）收敛为 Agent 可经统一接口消费的 canonical 能力。本任务交付**进程托管 + 协议骨架 + 统一消费接口**及契约测试，不在本任务实现完整语言语义，也不自行实现语言分析器。

> 定位变更（2026-08 收敛）：本任务不再把 Pawork 设计成 LSP Server。「Pawork 对 IDE 暴露 LSP Server」属于可选输出能力，移到 [P17-9 IDE Host Adapter](P17-9-ide-host-adapter.md) 作为可选 server-side 输出，不作为 P17-4 的主要职责。P17-4 只解决「Agent 调用语言服务」，P17-9 才解决「IDE 接入 Pawork」。

**涉及范围**：新增 `lsp-runtime`；在 `process-runtime` 字节流之上实现专用 LSP `Content-Length` framing（不复用 SSE/JSONL/partial JSON 解析器）、复用 `logging`、`resource-loader`（语言服务配置加载）、`process-runtime`（语言服务子进程托管）；经 `policy-engine` / `sandbox-runtime`（P11）约束子进程。

## 细分步骤

1. **LanguageServerDescriptor 模型** —— 目的：在 `agent-domain` / `lsp-runtime` 定义语言服务描述符，统一 `command` / `args` / `transport` / `env` / `language | extension mapping` / `initialization_options` / `settings` / `workspace_folder` / `startup_timeout` / `shutdown_timeout` / `restart_on_crash` / `max_restarts`；首版 `transport=stdio`，为未来显式配置 socket 预留枚举，描述符纯领域类型，不绑定具体实现。
2. **LSP framing 与客户端进程托管** —— 目的：实现 JSON-RPC over stdio 的 `Content-Length: N\r\n...\r\n\r\n<body>` 增量解帧，正确处理 header/body 跨 chunk、多个连续 frame、大小上限、重复/缺失/非法 header 与 EOF 半帧；随后完成 `initialize` / `initialized` / `shutdown` / `exit` 握手与能力协商，崩溃按策略重启。子进程经 `sandbox-runtime::SandboxBackend::spawn` 执行，不绕过沙箱。
3. **文档同步** —— 目的：以 LSP Client 身份向语言服务发送 `textDocument/didOpen | didChange | didClose`，维护文档版本与增量同步，作为所有请求的源数据；Agent 侧文件变更驱动文档同步。
4. **统一消费接口** —— 目的：定义 Agent 统一消费接口，把语言服务能力归一为 canonical 结果——`diagnostics`、`hover`、`definition`、`references`、`document_symbols`、`workspace_symbols`、`call_hierarchy`、`rename`、`code_actions`；接口屏蔽具体语言服务差异，Agent 不感知后端。
5. **配置发现与接入** —— 目的：支持内置预设（rust-analyzer / pyright / typescript-language-server / gopls / clangd）与用户配置接入其他 LSP Server；配置经 `resource-loader` 加载，工作区作用域与既有资源一致。
6. **rename / code_action 写操作约束** —— 目的：`rename` 产生 WorkspaceEdit、`code_action` 产生应用变更时，写操作落回既有 edit-file / apply-patch 能力，经 `policy-engine` 审批与 checkpoint（P4-9/P4-11），不允许语言服务直接写盘。
7. **结果归一与 artifact 引用** —— 目的：大体积诊断 / 符号表 / 大范围 WorkspaceEdit 经 artifact-store 引用（ADR-018），结果归一为统一结构；调用可取消、可审计。
8. **定向 / Mock 测试** —— 目的：用 mock language server 覆盖 fragmented header/body、连续 frame、malformed/oversized frame、客户端托管、能力协商、文档增量同步、九项消费接口、崩溃重启、rename 经策略约束。仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- `lsp-runtime`：`LanguageServerDescriptor` + LSP Client 进程托管 + 文档同步 + 九项统一消费接口 + 配置接入
- 写操作经 policy/checkpoint 约束 + 结果归一/artifact 引用
- 定向测试（客户端托管 / 文档同步 / 九项接口 / 崩溃重启 / rename 约束）

## 验收标准

- [ ] Pawork 作为 LSP Client 启动并托管语言服务子进程，崩溃按策略重启，子进程经 sandbox 执行
- [ ] LSP `Content-Length` framing 可跨任意 chunk 边界解析，并对非法 / 超大 / EOF 半帧给出有界错误
- [ ] 九项能力（diagnostics / hover / definition / references / document/workspace symbols / call_hierarchy / rename / code_actions）经统一接口可消费
- [ ] 文档增量同步正确；rename / code_action 等写操作经 policy + checkpoint 约束，不直接写盘
- [ ] 支持内置预设与用户配置接入其他 LSP Server；配置作用域与 resource-loader 一致
- [ ] 本任务不实现「Pawork 对 IDE 暴露 LSP Server」（属 P17-9 可选输出）
- [ ] 定向 / Mock smoke 通过，不要求 workspace 全量门禁

**相关文档**：[P17-9 IDE Host Adapter](P17-9-ide-host-adapter.md) · [workspace-index](../docs/features/workspace-index.md) · [tools](../docs/features/tools.md) · [process](../docs/features/process.md) · [policy](../docs/features/policy.md) · [sandbox](../docs/features/sandbox.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：LSP framing 按协议的 `Content-Length` header/body 状态机自实现，不复用 [P2-2](P2-2-sse-parser.md) SSE、[P2-3](P2-3-jsonl-parser.md) JSONL 或 partial-json；不新增第三方依赖，仅在用户明确确认后才评估 `tower-lsp` 之类。复用 `logging` / `resource-loader` / `process-runtime` / `sandbox-runtime`。新 crate `lsp-runtime` 依赖方向：`agent-domain → lsp-runtime → app-service`（作为可选服务暴露）。
