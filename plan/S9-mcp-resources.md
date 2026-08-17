# S9：MCP、资源与兼容导入

> 阶段 S9 · 扩展输入面 · 状态：🔵进行中（波 A–B ✅）· 依赖：S2（工具注册面）、S6（config 凭证链）· 规模：大

## 目标（本阶段结束时用户能做什么）

`pawork` 接入外部生态的输入面：配置真实 MCP server 后其工具与内置工具在同一注册表共存可调；AGENTS.md / Skills / profiles 被加载并注入上下文、对 Agent 行为可观测生效；`@file` 引用（file-index 支持的模糊补全语义）把文件内容注入对话；一键导入本机 Claude / Codex / Grok / Cursor / Pi 的现有配置（MCP 声明、指令文件等，只读、不产生写副作用）；配置系统补齐完整六层合并与 Profile。

## 涉及包与 V1 资产

| V2 包（目录） | 本阶段动作 | V1 来源与方式 |
| --- | --- | --- |
| `pawork-mcp`（extensions/mcp） | 激活：V1 `mcp-client` 迁移 + **rmcp 类型收口**（canonical `McpPeer`，rmcp 锁进内部 `codec` 模块，`=2.2.0` 锁定只在本包）；MCP 工具注册进 S2 的工具注册面，与内置工具共存 | 直接迁移（[archive/M6](archive/README.md) pawork-mcp 节全文适用） |
| `pawork-resources`（workspace/resources） | 激活：V1 `resource-loader` 迁移（loader 基础设施层 / profiles+skills 格式契约层分层）；AGENTS.md / Skills 加载注入主循环 context（修复 V1 未接主循环缺口） | 直接迁移（[archive/M6](archive/README.md) pawork-resources 节） |
| `pawork-compat`（clients/compat） | 激活：V1 `compat-loader` 迁移——Claude / Codex / Grok / Cursor / Pi 配置只读导入；MCP 声明经 `pawork-mcp` 薄类型（不拖 rmcp）；导入结果作为外部配置源，不进默认信任域 | 直接迁移（[archive/M6](archive/README.md) pawork-compat 节） |
| `pawork-workspace` | 增强：V1 `file-index` 并入（索引构建/查询），消费者 `@file` 引用同批落地（修复 V1 零消费者缺口） | 直接迁移 + 接线 |
| `pawork-config` | 增强：完整六层合并（补 Profile/Session/Run 层）+ `ProfileConfig` 生效（Profile 层插入 Global 与 Workspace 之间）；`trust_workspaces` 层级限制回归 | 迁移 V1 层级合并引擎完整版（替换 S0 三层实现，公共 API 不变） |
| `pawork-session` | 增强：`import::formats` 导入器——V1 `compat_import`（四来源）/`export_import`（`EXPORT_SCHEMA_VERSION = 3` 往返）/`pi_import`（保留 unknown fields）收敛为纯函数解析器模块；导入产物只生成 canonical event、单事务失败整批回滚；高置信凭证前缀整批拒绝 | 直接迁移（[archive/M3](archive/README.md) 关键动作 4） |
| `pawork-cli` | 增强：`pawork mcp list/test`、`pawork import <tool>`（compat 配置导入向导）、`pawork sessions import/export`、REPL `@` 引用补全 | 新写 |

## 关键任务

1. **rmcp 收口**：`McpPeer` 公开签名无 `rmcp::model::*`（API 边界断言）；MCP 工具与内置工具共存注册。
2. **资源注入链**：AGENTS.md（workspace 层级向上发现）+ Skills 目录 + profiles → 系统提示组装位（engine turn 组装的固定注入点，S5 的 context 预算把注入内容计入）。
3. **`@file` 引用**：REPL 输入 `@path` 前缀触发 file-index 补全，选中文件内容作为消息附件注入（大文件按 S5 预算截断）。
4. **compat 导入**：五来源最小样本 golden；本机真实配置导入冒烟（用户机器上有 Claude/Codex/Cursor 真实配置——天然测试材料）。
5. **会话导入导出**：Pawork 导出→导入往返一致（sequence/parent/branch）；Claude/Codex 会话导入映射为 canonical event 不污染既有事件。
6. **config 完整化**：S0 三层实现替换为 V1 完整引擎（外部 API 不变——计划内替换）；Profile 切换冒烟。

## 真实测试与评估（冒烟清单）

- [ ] 配置一个真实 MCP server（如 filesystem 或 everything 参考实现）：`pawork mcp list` 可见其工具 → 对话中 Agent 混用 MCP 工具与内置工具完成任务。
- [ ] 在 workspace 写 AGENTS.md 规则（如「所有回答以『收到』开头」）→ 行为立即可观测生效；删除后失效。
- [ ] 放置一个 Skill（含 SKILL.md）→ 相关任务中被采用（事件流可见资源注入）。
- [ ] `@ROADMAP` 补全出 `ROADMAP.md` → 引用后就内容提问回答正确。
- [ ] `pawork import claude`（本机真实 Claude 配置）：MCP 声明与指令文件被识别、列出、确认后并入（只读源文件未被修改——校验 mtime/内容）。
- [ ] 导出→新机器（或新目录）导入→resume 续聊连贯。
- [ ] Profile 切换：`profile = "work"` 覆盖 default_model 生效、层级优先序正确。
- [ ] **评估记录**：AGENTS.md 指令遵循度（两模型对注入规则的服从率）。

## 定向自动化测试

- `cargo test -p pawork-mcp`：rmcp 收口 API 断言、MCP contract golden（握手 + tool call）、共存注册回归。
- `cargo test -p pawork-resources`：AGENTS.md/Skills/profiles 解析与分层加载；注入内容进预算断言。
- `cargo test -p pawork-compat`：五来源 golden；无 rmcp 依赖边（`cargo tree` 断言）；只读无写副作用。
- `cargo test -p pawork-session`：导入器往返/映射回归、凭证前缀拒绝、事务回滚。
- `cargo test -p pawork-config`：六层完整合并矩阵、`trust_workspaces` 层级限制、Profile 派生插层。
- `cargo test -p pawork-workspace`：file-index 构建/查询。

## 退出标准

- [ ] 冒烟全项通过（含本机真实 Claude/Codex 配置导入）。
- [ ] rmcp 收口完成（公开签名断言 + compat 无 rmcp 边）。
- [ ] AGENTS.md/Skills 注入在主循环真实生效且计入预算；file-index 有真实消费者（`@file`）。
- [ ] 导入产物只读、canonical、可回滚；Secret 拒绝策略回归通过。
- [ ] config 六层完整、S0 实现替换后外部 API 未变。

## GUI 增量

按 [gui-design.md](../docs/gui-design.md) §5：Composer `@file` 补全；Resources 只读展示 MCP 列表与已加载 AGENTS.md/Skills。不做插件市场。

## 为后续阶段预留 / 明确不做

- 预留：MCP server 进程管理走 `pawork-exec`（沙箱内启动）；resources loader 抽象保留，供日后 LSP 注入（LSP 本身待设计，见 ROADMAP §4）。
- 不做：MCP server 市场/发现、用户 Hooks、WASM 插件（整族移出计划，见 ROADMAP §4）；远程 MCP 鉴权 UI（按需）。

## 并行拆分建议

- 波 A（并行 ×3）✅（2026-08-17）：`pawork-mcp`（含 rmcp 收口）；`pawork-resources`；`pawork-config` 完整化。
- 波 B（并行 ×3）✅（2026-08-17）：`pawork-compat`（五来源只读；MCP/Hook 本地薄类型，无 rmcp 边）；`pawork-session` 导入器（`import::formats` + 单事务写入）；`pawork-workspace` file-index（构建/查询）。
- 波 C（串行）：engine 注入点 + cli（mcp/import/@引用）+ 冒烟。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [archive/M6-extensions.md](archive/README.md)（mcp/resources/compat 迁移细则——本阶段主文档）
- [archive/M3-storage-session.md](archive/README.md)（导入器细则）
- [archive/M0-skeleton-foundation.md](archive/README.md)（config 完整层级细则）
