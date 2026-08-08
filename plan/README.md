# Pawork 任务计划（plan/）

本目录存放 ROADMAP 中每个任务的**具体细节**。`ROADMAP.md` 是目录索引（进度表 + 任务简介 + 链接），本目录是每个任务的展开实现说明。

## 如何使用

1. 打开 [ROADMAP.md](../ROADMAP.md)，查看顶部的「进度总览」与「下一个推荐任务」。
2. 按任务 ID 顺序执行（`P0-1 → P0-2 → …`），尊重每个任务的「依赖」字段；前置未完成不开始。
3. 点开对应 `plan/<id>-<slug>.md` 获取该任务的最终目的、细分步骤、产出与验收标准。
4. 完成任务后：勾选该文件内的验收项，并把 `ROADMAP.md` 中对应行的状态改为 `🟢`、更新进度总览与「下一个推荐任务」。
5. 任务粒度目标：数小时内可独立完成、独立验收、写入集收敛到单一 crate 或一组紧相关文件。

## 命名约定

- 文件名：`<任务ID>-<英文短名>.md`，例如 `P0-1-workspace-skeleton.md`。
- 任务 ID 沿用 ROADMAP 的 `P{n}-{seq}`：`P{n}` 是 Phase 序号，`{seq}` 是 Phase 内顺序；与优先级记号 P0–P2（见术语表）无关。
- 状态符号：`🟡未开始` · `🔵进行中` · `🟢已完成` · `⚪已归档/推迟`。

## 依赖选型

引入任何第三方包前，先对照 [ROADMAP「依赖选型基线」](../ROADMAP.md#依赖选型基线)：列为「直接采用」的按表使用；列为「参考 + 自实现」的只实现所需最小子集；新增依赖必须同步回基线一节。任务文件带有「依赖建议」小节时，以任务文件为准。

## 单个任务文件结构

每个 `plan/<id>-<slug>.md` 包含以下小节（务必齐全）：

- 元信息头：`> Phase {n} · {Phase 名} · 状态 · 依赖`。
- **最终目的**：一段话，说明这一步完成后解锁什么能力、为什么它处于关键路径上。
- **涉及范围**：涉及的 crate / 目录。
- **细分步骤**：编号列表，每一步写清「做什么」与「目的」，最后由「最终目的」串起整步交付意义。
- **主要产出物**：代码 / 文档 / 配置 / 测试的具体清单。
- **验收标准**：可勾选、可复核的条件。
- **相关文档**：仓库内相对路径链接。

> 「细分步骤」中每一步都要写清**任务**与**目的**，避免只罗列动作而不交代它服务哪个目标。

---

## 关键路径

    Domain → Mock Provider → Event Store → OpenAI-compatible
          → Agent Loop → Built-in Tools → Policy
          → Sessions/Compaction → Git/Diff → Main Providers
          → MCP → WASM → Multi-Agent

在核心 Coding Agent 能可靠完成真实仓库任务前，不进入 Multi-Agent 与复杂插件开发。

> CLI Host 与多 GUI 协议（Phase 13）是 Core 的正式运行入口与 GUI 接入边界；协议冻结部分（GUI Connection Protocol / Transport 抽象类型）随 [P0-8](P0-8-core-api.md) 提前完成，Remote Transport 真实内网穿透库可推迟。

---

## MVP 范围

首个可发布 Core 的能力边界。详细依据见 [性能目标](../docs/quality/performance-targets.md) 与各 Phase 退出标准。

**必须具备**：纯 Rust 无 Node/Bun；OpenAI-compatible / OpenAI（GPT）/ Anthropic（Claude）/ xAI Grok / 智谱 GLM / 阿里 Qwen / Moonshot Kimi；API Key + OAuth 订阅；Agent Loop + Streaming；Text/Image/Tool Call；read/write/edit/apply_patch；shell/search/find/list；Policy 与 Approval；Workspace Trust；SQLite Session；Fork/Resume/Branch；Compaction；Git status/diff/stage/unstage；Worktree；Checkpoint 与 Rollback；Skills 与 `AGENTS.md`；Pi JSONL 导入；CLI Host（`pawork`，CLI=Core 宿主，可脱离 GUI 运行）；GUI Connection Protocol 与多 GUI 连接（含本地 Transport、Snapshot/重放、Remote 占位接口）；macOS/Windows/Linux 基础支持。

**可推迟**：Bedrock、Mistral、Vertex、Google Gemini（已实现但降级次要）；MCP OAuth；WASM Provider；高级 Sandbox；Multi-Agent；Hunk/Line Stage；真实 Remote Transport（内网穿透库）；远程开发；云同步；长期语义记忆。

## MVP 验收清单

1. Core 启动不加载任何 JavaScript Runtime
2. 可从空工作区创建 Session
3. 可流式调用至少三个主要 Provider（初始集合：OpenAI / Anthropic / xAI / 智谱 / 阿里 / Moonshot）
4. 可接收多个 Tool Call
5. 可取消 Provider 和 Tool
6. 可安全读取和修改文件
7. 可运行测试命令并流式返回输出
8. 所有文件修改可查看 Diff
9. 所有 Agent 修改可回滚
10. 可从任意消息 Fork
11. 可自动和手动 Compaction
12. Core 崩溃后 Session 可恢复
13. Secret 不写入数据库和日志
14. 未信任 Workspace 默认限制写入和命令
15. 可导入 Pi JSONL Session
16. 大型 Tool Output 使用 Artifact 引用
17. 100,000 行 Diff 不需要一次复制到 GUI
18. GUI Connection Protocol 有版本和 Contract Tests
19. CLI 与 Core 编译为同一二进制，CLI 可脱离 GUI 运行
20. 一个 CLI/Core 实例可同时连接多个本地/远程 GUI，GUI 断线不取消任务
21. 三个平台的路径和子进程测试通过
22. 性能指标达到 Core 目标

## 横切门禁

每个 Phase 结束须对照：

- [性能目标](../docs/quality/performance-targets.md)：区分 Rust Core / Git 子进程 / Provider 网络 / 模型生成 / 外部命令 / GUI 渲染。
- [安全验收](../docs/quality/security-acceptance.md)：发布前 15 项必过。
- [测试体系](../docs/quality/testing.md)：单元 / contract / mock / golden / fuzz / chaos / 差分。

## 风险监控

| 风险 | 影响 | 对策 | 相关 |
| --- | --- | --- | --- |
| Provider 适配工作量 | 高 | Provider 只依赖 canonical domain；保存 raw metadata；Contract Tests；禁止在 Agent Engine 判断 Provider 名 | [ADR-002](../docs/adr/ADR-002-agent-engine-provider-decoupled.md)、[ADR-015](../docs/adr/ADR-015-provider-contract-tests.md) |
| Compaction 品质 | 高 | 保留结构化状态/目标/未完成任务/修改文件；Golden Sessions；可检查摘要；可恢复压缩前 branch | [context](../docs/features/context.md) |
| Shell 跨平台 | 中 | Process Runtime 独立 crate；平台特定实现；三平台 CI | [process](../docs/features/process.md) |
| 插件生态重建 | 中 | Skills 优先；MCP 优先于 WASM；WASM API 小而稳定；不过早支持 native plugin | [ADR-011](../docs/adr/ADR-011-mcp-first-extension.md)、[ADR-012](../docs/adr/ADR-012-wasm-first-plugin.md) |
| 范围过大 | 高 | 严格按关键路径推进；Coding Agent 可靠完成真实任务前不进入 Multi-Agent/复杂插件 | [ROADMAP 关键路径](../ROADMAP.md) |
