# S2：Agent Loop 与只读工具

> 阶段 S2 · 工具循环 · 状态：🔵进行中 · 依赖：S1 · 规模：大

## 目标（本阶段结束时用户能做什么）

`pawork` 从「聊天工具」变成「最小 Agent」：模型可自主调用四个只读工具（read_file / list_directory / search_text / find_files）在真实仓库里多轮探索并回答问题（"X 功能在哪个文件实现？"）。同时引入 **Anthropic Messages 协议适配器**作为第二条真实通道（GLM Coding Plan 的 anthropic 端点 + OpenCode Go 的 `/messages` 模型），在工具调用这个最容易过拟合的地方验证 Provider 契约的协议中立性。

## 涉及包与 V1 资产

| V2 包（目录） | 本阶段动作 | V1 来源与方式 |
| --- | --- | --- |
| `pawork-api` | 增强：`tool` feature（波 A 已落地）。V1 `tool-api` 执行契约迁入：`AgentTool`（`descriptor`/`execute`）、`ToolEventSink`、`ToolRequest`/`ToolExecutionContext`（`workspace_id` + 相对 `working_directory`）、`ToolResult`/`ToolError`/`ToolStreamEvent`。`ToolDescriptor`（含 `requires_approval`/`read_only`/`allowed_in_untrusted_workspace`/`max_output_bytes` 等全部字段）已在 `pawork-domain`，不复制、不做 re-export 薄壳 | 直接迁移（descriptor 审批语义为 S3 铺路，本阶段只消费 `read_only = true` 工具） |
| `pawork-tools`（execution/tools） | 激活：V1 `builtin-tools` 的 `common` + `read_file`/`list_directory`/`search_text`/`find_files` 四模块迁移；V1 `tool-runtime` 的 scheduler 最小迁移（注册、并发上限、超时、输出截断） | 直接迁移（[archive/M1](archive/M1-execution-security.md) pawork-tools 节；写工具 S3、run_command S4、`tool_search` 冻结不迁） |
| `pawork-workspace`（workspace/core） | 激活（最小）：`WorkspaceId`、roots 管理、相对路径解析与校验入口（**文件工具输入一律 `workspace_id + relative_path`**，拒绝绝对路径与越 root 的 `..`；完整安全校验 S3 接 policy） | V1 `workspace-service` 最小子集迁移 |
| `pawork-engine` | 增强：多轮工具循环——收集 `ToolCallStarted/ArgumentsDelta/Completed` 组装 `PendingToolInvocation`，经 `LoopContext.execute_tools` 派发，`ToolCallResult` 回填下一轮 `CanonicalModelRequest`，直至非工具 `StopReason`；工具链路事件（`ToolExecutionStarted`/`ToolOutputDelta`/`ToolExecutionCompleted`）入事件流；防失控上限（每 run 最大工具轮数，超限事件化终止） | 语义对齐 V1 `provider_loop` 工具派发子集 + `SchedulerLoopContext` |
| `pawork-providers` | 增强：`anthropic` feature——V1 `provider-anthropic` 的 Messages 协议核心迁移（messages + SSE + `tool_use`/`tool_result` 映射到 canonical 事件）；完整能力（prompt cache、thinking 高级配置等）S6 补齐 | 部分迁移（拆迁边界记录在包内 TODO 与 S6 计划） |
| `pawork-testkit`（foundation/testkit） | 激活（最小）：`MockProvider`（脚本化流事件序列，可编排多轮 tool_use）+ 事件序列断言 helper | V1 `test-support` 最小子集迁移 |
| `pawork-cli` | 增强：工具活动渲染（"⚙ read_file src/lib.rs (1.2KB)"式单行进度）；`--json` 自然携带工具事件 | 新写 |

## 关键任务

1. **tool 契约迁移**（契约 owner 串行）：`pawork-api` tool feature；工具描述 JSON Schema 生成与两种协议（OpenAI tools / Anthropic tool_use）的映射在 adapter 侧完成，engine 不感知厂商差异（红线：无 Provider 名称特例分支）。
2. **四个只读工具迁移**：V1 行为测试随迁（编码、截断、路径处理、大文件、二进制探测）。
3. **workspace roots**：CLI 启动目录即默认 workspace root；相对路径解析入口统一。
4. **engine 工具循环**：MockProvider 驱动的确定性多轮测试先行；工具并发（`supports_concurrency`）与超时接 scheduler。
5. **anthropic-messages 适配器**：V1 契约 golden 随迁（最小 1–2 条）；OpenCode Go 仅走 `/messages` 的模型顺带覆盖（[../ROADMAP.md](../ROADMAP.md) §4 未决项在此关闭）。
6. **testkit MockProvider**：从本阶段起 engine/工具逻辑回归全部走 Mock，真实 API 只做冒烟。

## 真实测试与评估（冒烟清单）

三通道各跑：GLM(OpenAI 端点)、GLM(Anthropic 端点 `https://open.bigmodel.cn/api/anthropic`)、OpenCode Go：

- [ ] 在本仓库运行：「`Pawork_v2/ROADMAP.md` 里 S4 阶段的真实验收要点是什么？」——Agent 应 read_file 后据实回答。
- [ ] 「这个仓库里所有 `.gitkeep` 文件在哪些目录？」——应走 find_files/list_directory，答案可核对。
- [ ] 「search 一下哪里定义了 `CURRENT_SCHEMA_VERSION`」——应走 search_text 并给出文件路径。
- [ ] 诱导测试：要求读取 `C:\Windows\system32\...` 或 `..\..\` 外部路径 → 工具层拒绝、Agent 得到错误后正常向用户解释。
- [ ] 工具轮数上限：构造会无限探索的提问，验证上限触发、run 以可读方式终止。
- [ ] **协议对比评估记录**：三通道的 tool-calling 可靠性（是否产生合法 tool_use、参数 JSON 是否有效、多轮是否收敛、参数幻觉率）、每任务轮数与耗时——为后续阶段选默认评测模型定标。

## 定向自动化测试

- `cargo test -p pawork-engine`：MockProvider 多轮工具循环（单工具、并行多工具、工具失败回填、轮数上限）；事件序列 golden（`--json` 全链完整性）。
- `cargo test -p pawork-tools`：四工具 V1 行为测试随迁全绿；输出截断与 `max_output_bytes`。
- `cargo test -p pawork-workspace`：相对路径解析（拒绝绝对路径 / 越 root `..` / 保留字设备名）。
- `cargo test -p pawork-providers --features anthropic`：Messages 请求组装与流解析 golden；tool_use 映射。
- env 门控真实 API（`--ignored`）：单工具往返（模型被要求调用 read_file 读固定 fixture 并复述内容）。

## 退出标准

- [ ] 冒烟清单全项通过（三通道），协议对比评估留档。
- [ ] engine 工具循环在 MockProvider 下确定性回归全绿；无 Provider 名称特例分支（代码审查断言）。
- [ ] tool 契约整组迁移零裁剪；文件工具输入均为 `workspace_id + relative_path`。
- [ ] anthropic 适配器 golden 通过；同一任务在 OpenAI/Anthropic 双协议下事件流形状一致（仅 provider metadata 不同）。
- [ ] 只读边界成立：本阶段 Agent 无任何写盘/执行能力（descriptor `read_only` 全真断言）。

## 为后续阶段预留 / 明确不做

- 预留：`ToolDescriptor.requires_approval`/`allowed_in_untrusted_workspace` 字段已随契约在位（S3 消费）；scheduler 的审批挂点（`ApprovalResolver` 注入位）预留为 trait 参数但 S2 传 None。
- 不做：写工具与审批（S3）、run_command（S4）、`@file` 引用与 file-index（S8）、工具输出进 context 预算（S5）。

## 并行拆分建议

- 波 A（契约 owner 串行）：`pawork-api` tool feature。✅
- 波 B（并行 ×3）：`pawork-tools`（四工具 + scheduler）、`pawork-workspace`、`pawork-providers`（anthropic）。✅
- 波 C（并行 ×2）：`pawork-engine` 工具循环（依赖波 A/B 接口）、`pawork-testkit`。✅
- 波 D（串行收口）：cli 渲染 + 装配 + 三通道冒烟。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [../docs/design.md](../docs/design.md) §3.2（工具契约、引擎语义行）
- [archive/M1-execution-security.md](archive/M1-execution-security.md)（pawork-tools/pawork-workspace 迁移细则）
- [archive/M2-providers.md](archive/M2-providers.md)（provider-anthropic 归属与 feature 组织）
- [archive/M4-engine-closed-loop.md](archive/M4-engine-closed-loop.md)（provider_loop 拆分目标形态——S2 起逐步对齐）
