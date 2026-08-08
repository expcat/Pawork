# P17-13：Cross-Agent Compatibility Loader（配置与资源兼容加载）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P8-1～P8-6、P9-6、P17-1、P17-5；与 P16-9 协调

**最终目的**：以输入侧 Adapter 读取 Claude Code、Codex、Grok Build、Cursor 与 Pi 的项目配置和扩展资源，将可表达内容映射为 Pawork canonical Instructions、Skills、MCP、Agent Profile、Hooks 与 Permission rules，降低迁移成本；外部配置永远不是运行时事实源，加载过程不得直接执行脚本、连接 MCP 或放宽权限。

**涉及范围**：新增 `compat-loader` 或在 `resource-loader` 下建立隔离 adapter；复用 `resource-loader`、`mcp-client` 配置模型、`hook-runtime`、`policy-engine` 与 P17-5 Agent Profile v2；Session 历史导入不在本任务，统一由 P16-9 负责。

## 细分步骤

1. **来源探测与只读解析** —— 目的：识别 `AGENTS.md`、`CLAUDE.md`、`.claude/rules` 及 Claude/Codex/Grok/Cursor/Pi 的已知配置目录；只读取 workspace 内相对路径与用户明确启用的全局来源，未知版本返回诊断，不猜测执行。
2. **Instructions / Skills 映射** —— 目的：把层级 instructions、skill manifest 与 prompt 资源映射到 P8 canonical resource，并保留来源、优先级和不可映射字段摘要，禁止外部优先级覆盖 Pawork 安全策略。
3. **MCP / Agent Profile 映射** —— 目的：把可兼容的 MCP server 描述、custom agent/subagent 配置映射到 P9/P17-5；Secret 只保留 credential reference，不复制明文 token。
4. **Hooks / Permission rules 映射** —— 目的：把可表达的 hook trigger 与 permission rule 转为 P17-1/P4-9 canonical 规则；Shell/HTTP/Prompt handler 仅形成待审配置，导入阶段不执行，无法安全映射的规则默认禁用并解释原因。
5. **冲突、版本与来源诊断** —— 目的：同一能力多来源冲突时遵循 P8-6 确定性优先级，输出 `Imported` / `Disabled` / `Unsupported` / `Conflict` 诊断和来源追踪，不静默覆盖。
6. **预览与显式应用** —— 目的：提供 dry-run 预览和按项选择，只有用户或受信策略确认后才写入 Pawork 配置；原文件只读、不改写，重复导入幂等。
7. **Fixture smoke** —— 目的：为五类来源各准备最小 fixture，定向验证 instructions/skills/MCP/agent/hook/permission 映射、Secret 不落盘、冲突可诊断；不运行 workspace 全量门禁。

## 主要产出物

- Compatibility Loader 来源探测与只读 parser
- 外部资源到 Pawork canonical resource/profile/policy 的映射层
- dry-run 预览、来源诊断与幂等应用
- 五类来源的定向 fixture smoke

## 验收标准

- [ ] 可读取 Claude/Codex/Grok/Cursor/Pi 的已知 Instructions、Skills、MCP、Agents、Hooks 与 Permission 配置，并映射可支持子集
- [ ] `CLAUDE.md` / `.claude/rules` / `AGENTS.md` 层级与来源可追踪，冲突按 P8-6 确定性处理
- [ ] 外部 hooks、MCP 与脚本在导入/预览阶段绝不执行；Secret 只保留 credential reference
- [ ] 不可映射内容显式标为 `Unsupported` 或 `Disabled`，不静默放宽权限或写入非 canonical 类型
- [ ] 原配置不被改写，重复导入幂等；Session 历史由 P16-9 单独处理
- [ ] 仅运行定向 fixture smoke，不要求 workspace 全量门禁

**相关文档**：[skills](../docs/features/skills.md) · [mcp](../docs/features/mcp.md) · [policy](../docs/features/policy.md) · [P8-1 Resource Loader](P8-1-resource-loader.md) · [P17-1 User Hooks](P17-1-user-hooks.md) · [P17-5 Agent Profile v2](P17-5-agent-profile-v2.md) · [P16-9 Session Import](P16-9-session-compat-import.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：优先复用 serde/TOML/JSON 与现有 resource loader，不引入外部 Agent SDK；每种来源独立 adapter，版本变化只影响输入层，不改变 Pawork canonical domain。
