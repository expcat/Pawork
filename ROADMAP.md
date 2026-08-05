# Pawork 开发路线图

本路线图是**目录式索引**：顶部是任务进度表与「下一个推荐任务」，下方按 Phase 列出每个任务的简短介绍与详情链接。每个任务的**最终目的、细分步骤、产出与验收标准**见 `plan/` 目录下对应文件。

> 范围说明、MVP 边界、横切门禁与风险监控见 [plan/README.md](plan/README.md)。

## 如何使用

1. 查看「进度总览」了解各 Phase 进度，查看「下一个推荐任务」获取当前应执行的任务。
2. 按任务 ID 顺序执行（`P0-1 → P0-2 → …`），尊重每个任务的「依赖」字段；前置未完成不开始。
3. 点开 `plan/<id>-<slug>.md` 获取该任务的细分步骤与最终目的。
4. 完成任务后：把对应行的状态改为 `🟢`、更新「进度总览」计数与「下一个推荐任务」。
5. 任务粒度：数小时内可独立完成、独立验收、写入集收敛到单一 crate 或一组紧相关文件。
6. 引入任何第三方依赖前先对照「依赖选型基线」一节；新增依赖必须同步回该节与对应 plan 任务。

状态符号：`🟡未开始` · `🔵进行中` · `🟢已完成` · `⚪已归档/推迟`。架构红线见 [AGENTS.md](AGENTS.md) §2 与各 [ADR](docs/adr/)。

## 进度总览

| Phase | 主题 | 任务数 | 已完成 | 状态 |
| --- | --- | --- | --- | --- |
| 0 | 架构与协议冻结 | 12 | 1 | 🔵进行中 |
| 1 | 基础设施 | 12 | 0 | 🟡未开始 |
| 2 | 首个真实 Provider | 11 | 0 | 🟡未开始 |
| 3 | Agent Loop | 10 | 0 | 🟡未开始 |
| 4 | 核心工具与权限 | 12 | 0 | 🟡未开始 |
| 5 | Session、Branch 与 Compaction | 9 | 0 | 🟡未开始 |
| 6 | 主要 Provider | 9 | 0 | 🟡未开始 |
| 7 | Git、Diff 与 Worktree | 8 | 0 | 🟡未开始 |
| 8 | Skills、Prompts 与 Instructions | 8 | 0 | 🟡未开始 |
| 9 | MCP | 7 | 0 | 🟡未开始 |
| 10 | WASM Plugin | 6 | 0 | 🟡未开始 |
| 11 | Sandbox 与跨平台强化 | 8 | 0 | 🟡未开始 |
| 12 | Multi-Agent | 6 | 0 | 🟡未开始 |
| 13 | CLI Host 与多 GUI 协议 | 10 | 0 | 🟡未开始 |
| **合计** | — | **128** | **1** | — |

> 计数口径：任务数与已完成数均包含 ⚪（归档/推迟）任务。

## 下一个推荐任务

> 🎯 **P0-1 仓库与 workspace 骨架** —— 关键路径的第一个动作，建立 workspace 根与目录骨架，使后续所有 crate 有落点。详情见 [plan/P0-1-workspace-skeleton.md](plan/P0-1-workspace-skeleton.md)。
>
> 开始方式：创建根 `Cargo.toml` 与 `crates/ apps/ schemas/ fixtures/ benches/` 目录，配置 `.gitignore` 与 CI 占位，确保 `cargo metadata` 与空构建通过。

## 关键路径

    Domain → Mock Provider → Event Store → OpenAI-compatible
          → Agent Loop → Built-in Tools → Policy
          → Sessions/Compaction → Git/Diff → Main Providers
          → MCP → WASM → Multi-Agent

在核心 Coding Agent 能可靠完成真实仓库任务前，不进入 Multi-Agent 与复杂插件开发。

> CLI Host 与多 GUI 协议（Phase 13）是 Core 的正式运行入口与 GUI 接入边界；其协议冻结部分（GUI Connection Protocol / Transport 抽象类型）随 [P0-8](plan/P0-8-core-api.md) 提前完成，运行时实现按依赖关系推进。

## 依赖选型基线

> 2026-08 文档 review 结论。选型准则：**高采用率、文档好、能最小子集使用**；包功能太杂乱时只参考其中需要的部分自己实现。下表是 `[workspace.dependencies]` 的基线（落地见 [P0-1](plan/P0-1-workspace-skeleton.md)）；新增依赖必须先更新本节。

**判断准则**

- 采用条件（须同时满足）：采用率高；活跃维护（近 12 个月有发布）；docs.rs 文档质量好；只需最小子集即可用；属于自实现正确性风险高的领域（加密、编码、OS 绑定、协议编解码）。
- 自实现条件（满足其一）：生态碎片化、无明确赢家；与 canonical domain / 架构红线冲突；安全关键路径、需完整 fuzz 与审计；集成成本高于自实现成本。
- 中间态：参考其设计与字段清单，并用差分测试对照参考实现行为（见「参考 + 自实现」表）。

### 直接采用

| 类别 | 包 | 关联任务 | 理由与使用范围 |
| --- | --- | --- | --- |
| 异步运行时 | tokio | P0-1 | 事实标准；按需启用 feature |
| 序列化 | serde / serde_json | P0-1 | 生态统一 |
| 错误 | thiserror（库）+ anyhow（应用层） | P0-7 | Rust 惯用分工 |
| 标识 / 哈希 / 版本 | uuid、blake3、semver | P0-2、P1-6 | blake3 用于 Blob 内容寻址 |
| 配置解析 | toml | P1-1 | 与 serde 配合 |
| CLI | clap | P1-12 | derive 宏最小化胶水 |
| 结构化日志 | tracing + tracing-subscriber + tracing-appender | P1-9 | 脱敏（redaction）规则仍自实现 |
| SQLite 绑定 | rusqlite | P1-2 | 契合「SQLite Actor 单连接」设计；sqlx 亦活跃，但其异步池 + 编译期 SQL 检查与该设计不匹配，集成成本更高 |
| HTTP 客户端 | reqwest（rustls + stream） | P2-1、P9-2 | Provider 与 MCP Streamable HTTP 所需 |
| OS Keychain | keyring（v3） | P2-6 | Secret 不落库不入日志 |
| OAuth 基础 | oauth2 | P6-4 | 只用 PKCE + Device Flow 子集 |
| MCP SDK | rmcp | P9-1、P9-2 | 官方 SDK、跟进 MCP 2026-07-28 规范；只用 transport + codec 层；锁定小版本（2.x→3.0 有 breaking） |
| WASM 宿主 | wasmtime + wit-bindgen | P10-2、P10-5 | Component Model 成熟；fuel / 内存上限对应 ADR-012 |
| 文件遍历 | ignore + globset | P1-8、P4-6、P4-7 | ripgrep 同源，性能经过验证 |
| 正则 | regex | P4-6 | 线性时间匹配、无 ReDoS 风险 |
| 文件监听 | notify + notify-debouncer-full | P1-8、P8-8 | 跨平台统一抽象 |
| 路径规范化 | dunce | P11-8 | Windows 短路径 / UNC |
| 编码检测 | content-inspector + chardetng + encoding_rs | P4-1 | Mozilla 系 |
| Token 计数 | tiktoken-rs | P3-2 | 仅对 OpenAI 系精确；其它 Provider 用启发式估算 |
| TS 类型导出 | ts-rs | P0-10、P13-7 | GUI Contract 类型生成，比 typeshare / specta 轻 |
| 系统目录 | directories | P1-12 | 配置 / 数据目录标准路径 |
| Linux 沙箱 | landlock | P11-1 | 基于 LSM，活跃维护 |
| Windows 绑定 | windows-rs（+ windows-service） | P11-4、P1-12 | 官方绑定 |
| Diff 生成 | similar | P7-3 | word-level diff，纯 Rust |
| PTY 基础 | portable-pty（或维护 fork） | P11-6 | 上游迭代慢，开工前先评估 fork |
| 重试 | backon | P2-10 | 退避策略完整 |
| 签名 | ed25519-dalek | P10-1 | 插件 manifest 签名 |
| 测试与基准 | criterion、proptest、cargo-fuzz + arbitrary、wiremock、insta、assert_cmd | P0-12、P2-11 等 | 基准 / 属性 / fuzz / HTTP mock / 快照 / CLI e2e |

### 参考 + 自实现（第三方包仅作设计参照，只实现所需最小子集）

| 功能 | 参考对象 | 关联任务 | 不直接采用的原因 |
| --- | --- | --- | --- |
| SSE 解析 | eventsource-stream 状态机；TS eventsource-parser（Pi / Vercel 使用） | P2-2 | eventsource-stream 约 1.5k dependents 但维护停滞；SSE 规范小，fuzz 与畸形输入要求明确，自实现更可控 |
| Provider 适配 | async-openai 类型定义（字段清单）；rig-core 分层；Pi 行为 | P2-5、P6-1~3 | 整套 SDK 与 canonical domain 冲突（ADR-002）；Anthropic / Gemini 的 Rust 生态碎片化；以类型定义做字段清单 + 差分测试 |
| Partial JSON 拼接 | llmx、tool-parser、Vercel partial-json | P2-4 | 现有 crate 采用率低、测试弱；流式 Tool Call 需要确定性修复语义 |
| unified diff 解析 | unidiff 的 PatchSet 模型 | P7-3 | 优先 git 结构化输出（--raw -z --numstat）；仅参考数据模型 |
| apply_patch / edit_file | patch-apply-rs、mpatch | P4-3、P4-4 | 安全关键路径，需完整 fuzz 与审计；精确匹配与上下文校验语义必须完全可控 |
| 配置合并 | config-rs | P1-1 | 全局 / 工作区 / session / CLI 合并要求确定性优先级语义，比通用解析更重要 |
| Metrics 采集 | metrics crate 命名与标签约定 | P1-10 | 采集留在 SQLite Actor 内自实现，只参考命名 |
| OAuth Device Flow | oauth-device-flows；RFC 8628 | P6-4 | RFC 本身很小，可在 oauth2 之上实现 |
| 本地 Transport | tokio 原生 UDS / Named Pipe；interprocess（可选） | P13-4 | 无需引入整套框架 |
| OAuth 回调服务器 | tiny_http / hyper | P6-4 | 一次性本地临时监听，不引入 axum |

### 完全自实现（架构红线或安全关键）

Agent Loop / 状态机 / Tool Scheduler / 预算 / 消息队列（P3-*）；Event Store 与 Projection 语义（P1-4、P1-5，rusqlite 只是绑定层）；Policy Engine / Workspace Trust / 路径与 shell 安全（P4-9、P4-10）；Checkpoint / 回滚编排（P4-11）；Compaction 引擎（P5-5、P5-6）；JSONL 流式解析（P2-3、P5-9，serde_json 逐行即可）；沙箱编排：macOS sandbox-exec、bwrap、Windows AppContainer / Job Object 与进程树清理（P11-1~4、P11-7）；PTY 会话层：重连 / 有界缓冲 / 归属（P11-6，在 portable-pty 之上）；GUI Connection Protocol 编解码 / 快照 / 订阅 / 慢客户端隔离（P13-3、P13-5）；日志 redaction 规则（P1-9）。

### 行为参照（不作为依赖）

Pi（TS，差分测试对象，P5-9）；goose（Block → Linux Foundation，MCP-first 的 Rust 参照实现）；rig-core（Provider 中立抽象设计参照）；ripgrep / wezterm（性能与 PTY 设计参照）。

### 重点风险

- rmcp 是唯一「官方协议 SDK」级依赖：锁定小版本、跟进官方迁移指南，在 mcp-client 内封装以隔离 breaking change。
- portable-pty 上游缓慢：P11-6 开工前评估维护中的 fork（如 xpy/portable-pty-psmux）或 vendor 兜底。
- tiktoken-rs 仅对 OpenAI 精确：其它 Provider 统一启发式估算 + 容差，不依赖精确 token 数。

## 遗留待决项（2026-08 review）

| 事项 | 说明 | 解决时点 |
| --- | --- | --- |
| P1-1 crate 归属 | 独立 config crate 还是并入 context-engine；倾向独立 config crate（P1-1 早于 context-engine 使用点、合并语义独立），选定后按 [workspace-layout §7](docs/architecture/workspace-layout.md) 登记 | P1-1 开工前 |
| agent-api 职责边界 | 评估与 core-api / app-service 的重叠；workspace-layout §6 依赖图仅画主干链（完整清单以其 §2 为准，含 agent-api / app-database / transport-memory / hook-runtime） | Phase 13 前 |
| provider-bedrock / provider-mistral | 已在 workspace-layout 登记但 ROADMAP 无对应任务（MVP 可推迟） | 启动时补任务 |
| 缺失功能文档 | audit-log、client-auth 尚无独立 `docs/features/` 文档 | 对应 crate 实现时 |

---

## 任务目录

### Phase 0：架构与协议冻结

冻结所有协议与领域类型，用 Mock Provider 跑通最小链路，确保无 Tauri 依赖进入 Agent Core。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P0-1 | 🟡 | 仓库与 workspace 骨架 | 建立 workspace 根与目录骨架、CI 占位 | [详情](plan/P0-1-workspace-skeleton.md) |
| P0-2 | 🟡 | 领域类型基线 | 冻结消息/角色/内容块/元数据/ID 领域类型 | [详情](plan/P0-2-domain-types.md) |
| P0-3 | 🟡 | 事件模型 | 可持久化、可重放的事件与 schema version | [详情](plan/P0-3-event-model.md) |
| P0-4 | 🟡 | Provider 协议 | canonical 请求/流式事件/错误统一契约 | [详情](plan/P0-4-provider-api.md) |
| P0-5 | 🟡 | Tool 协议 | AgentTool/描述/结果/capability/取消 | [详情](plan/P0-5-tool-api.md) |
| P0-6 | 🟡 | 插件协议骨架 | manifest/生命周期事件接口（不实现宿主） | [详情](plan/P0-6-plugin-api.md) |
| P0-7 | 🟡 | 错误与取消模型 | 跨 crate 统一错误类别与取消语义 | [详情](plan/P0-7-error-cancel.md) |
| P0-8 | 🟡 | Core Command/Event 协议 | 面向 GUI/CLI 的稳定 Core API | [详情](plan/P0-8-core-api.md) |
| P0-9 | 🟡 | Mock Provider / Mock Tool | 可编程 mock，跑通最小链路 | [详情](plan/P0-9-mock-provider-tool.md) |
| P0-10 | 🟡 | TS 类型生成脚手架 | Rust→TS 生成管线占位 | [详情](plan/P0-10-ts-typegen.md) |
| P0-11 | 🟢 | ADR 与文档基线 | ADR-001~030 定稿与链接校验（含 CLI Host 架构修正） | [详情](plan/P0-11-adr-docs.md) |
| P0-12 | 🟡 | 基准框架骨架 | benches 目录与计时口径 | [详情](plan/P0-12-bench-skeleton.md) |

### Phase 1：基础设施

奠定存储、工作区、文件索引与可观测性，支撑后续 Session 与 Agent Loop。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P1-1 | 🟡 | 配置系统 | 确定性配置层级与优先级合并 | [详情](plan/P1-1-config.md) |
| P1-2 | 🟡 | SQLite Actor | 串行化 DB 访问、WAL | [详情](plan/P1-2-sqlite-actor.md) |
| P1-3 | 🟡 | 数据库 schema 与迁移 | 核心表与向前迁移框架 | [详情](plan/P1-3-db-schema-migration.md) |
| P1-4 | 🟡 | Event Store | 事件 append 与按 sequence 重放 | [详情](plan/P1-4-event-store.md) |
| P1-5 | 🟡 | Projection | 可重建投影 | [详情](plan/P1-5-projection.md) |
| P1-6 | 🟡 | Blob Store | BLAKE3 寻址+引用计数+GC | [详情](plan/P1-6-blob-store.md) |
| P1-7 | 🟡 | Workspace 服务 | 增删改/多 root/Git 检测 | [详情](plan/P1-7-workspace-service.md) |
| P1-8 | 🟡 | 文件索引 | 异步扫描+ignore+去抖 | [详情](plan/P1-8-file-index.md) |
| P1-9 | 🟡 | 结构化日志 | 规范字段+自动脱敏 | [详情](plan/P1-9-structured-logging.md) |
| P1-10 | 🟡 | Metrics | 关键指标采集 | [详情](plan/P1-10-metrics.md) |
| P1-11 | 🟡 | 诊断包导出 | 脱敏可分享诊断包 | [详情](plan/P1-11-diagnostics-export.md) |
| P1-12 | 🟡 | CLI Host 骨架（pawork） | serve/run/shell/watch 子命令骨架（CLI=Core 宿主） | [详情](plan/P1-12-cli-skeleton.md) |

### Phase 2：首个真实 Provider

先实现 OpenAI-compatible，可同时覆盖云端兼容接口与多数本地服务。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P2-1 | 🟡 | HTTP 运行时 | 超时/代理/cancel/trace | [详情](plan/P2-1-http-runtime.md) |
| P2-2 | 🟡 | SSE 解析器 | 跨 chunk/Unicode/fuzz | [详情](plan/P2-2-sse-parser.md) |
| P2-3 | 🟡 | JSON Lines 解析器 | 提前断开/错误事件/fuzz | [详情](plan/P2-3-jsonl-parser.md) |
| P2-4 | 🟡 | Partial JSON 拼接 | 跨 chunk tool arguments | [详情](plan/P2-4-partial-json.md) |
| P2-5 | 🟡 | OpenAI-compatible 适配 | canonical 转换+流式组装 | [详情](plan/P2-5-openai-compatible.md) |
| P2-6 | 🟡 | API Key 认证 | OS Keychain 存取不落库 | [详情](plan/P2-6-apikey-auth.md) |
| P2-7 | 🟡 | Model Registry | 目录/别名/能力/费用 | [详情](plan/P2-7-model-registry.md) |
| P2-8 | 🟡 | 流式组装 | 事件→领域消息 | [详情](plan/P2-8-stream-assembly.md) |
| P2-9 | 🟡 | Usage 与 stop reason | token/费用/完成原因归一 | [详情](plan/P2-9-usage-stopreason.md) |
| P2-10 | 🟡 | 重试与错误归一化 | 可重试判定/退避 | [详情](plan/P2-10-retry-error.md) |
| P2-11 | 🟡 | Provider Contract Tests | 统一测试套件 | [详情](plan/P2-11-contract-tests.md) |

### Phase 3：Agent Loop

跑通完整 Agent 循环（含多轮工具、预算、取消、中断恢复）。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P3-1 | 🟡 | Run 状态机 | 全状态转换+事件化 | [详情](plan/P3-1-run-state-machine.md) |
| P3-2 | 🟡 | 上下文构建与预算 | 来源优先级+token 预算 | [详情](plan/P3-2-context-budget.md) |
| P3-3 | 🟡 | Provider Loop | 流式提交/解析 tool call/多轮 | [详情](plan/P3-3-provider-loop.md) |
| P3-4 | 🟡 | Tool Scheduler | 并发/串行/审批暂停 | [详情](plan/P3-4-tool-scheduler.md) |
| P3-5 | 🟡 | 消息队列 | 排队/replace queued | [详情](plan/P3-5-message-queue.md) |
| P3-6 | 🟡 | 预算控制 | 多维预算+事件不静默停 | [详情](plan/P3-6-budget-control.md) |
| P3-7 | 🟡 | 重试 | 断流重试/retry last call/run | [详情](plan/P3-7-retry.md) |
| P3-8 | 🟡 | 取消 | 取消 provider/tool+进程清理 | [详情](plan/P3-8-cancel.md) |
| P3-9 | 🟡 | 事件流式分发 | 广播+背压+<2ms | [详情](plan/P3-9-event-broadcast.md) |
| P3-10 | 🟡 | Interrupted Run 恢复 | 崩溃后 <1s 恢复 | [详情](plan/P3-10-interrupted-run-recovery.md) |

### Phase 4：核心工具与权限

具备最小可用 Coding Agent 能力（读写编辑搜索命令 + 权限审批 + 可回滚）。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P4-1 | 🟡 | read_file | offset/limit/编码/二进制/路径安全 | [详情](plan/P4-1-read-file.md) |
| P4-2 | 🟡 | write_file | 原子写/审批/checkpoint | [详情](plan/P4-2-write-file.md) |
| P4-3 | 🟡 | edit_file | 精确替换/unified patch/模糊匹配 | [详情](plan/P4-3-edit-file.md) |
| P4-4 | 🟡 | apply_patch | 多文件/dry run/原子/回滚 | [详情](plan/P4-4-apply-patch.md) |
| P4-5 | 🟡 | run_command | 流式/cwd/env/timeout/cancel | [详情](plan/P4-5-run-command.md) |
| P4-6 | 🟡 | search_text | 正则/ignore/上下文行 | [详情](plan/P4-6-search-text.md) |
| P4-7 | 🟡 | find_files | glob/ignore/排序 | [详情](plan/P4-7-find-files.md) |
| P4-8 | 🟡 | list_directory | 类型/symlink/分页 | [详情](plan/P4-8-list-directory.md) |
| P4-9 | 🟡 | Policy Engine | 审批/路径安全/Shell 风险 | [详情](plan/P4-9-policy-engine.md) |
| P4-10 | 🟡 | Workspace Trust | 默认受限/信任放宽 | [详情](plan/P4-10-workspace-trust.md) |
| P4-11 | 🟡 | Checkpoint 与回滚 | 单次/整 run 回滚+冲突检测 | [详情](plan/P4-11-checkpoint-rollback.md) |
| P4-12 | 🟡 | Process Runtime | 进程组/Job/无死锁 IO/cancel | [详情](plan/P4-12-process-runtime.md) |

### Phase 5：Session、Branch 与 Compaction

完善会话树、分支、压缩与 Pi 导入。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P5-1 | 🟡 | Session Tree / Fork | 从任意事件分叉 | [详情](plan/P5-1-session-fork.md) |
| P5-2 | 🟡 | Branch 切换 | 切换+并发写保护 | [详情](plan/P5-2-branch-switch.md) |
| P5-3 | 🟡 | Resume/归档/删除/重命名 | lease+损坏检测 | [详情](plan/P5-3-session-lifecycle.md) |
| P5-4 | 🟡 | 搜索 / 标签 | session 搜索与标签 | [详情](plan/P5-4-session-search.md) |
| P5-5 | 🟡 | Compaction 引擎 | 自动/手动压缩+快照 | [详情](plan/P5-5-compaction-engine.md) |
| P5-6 | 🟡 | 压缩保留策略 | 保留约束/任务/待处理 | [详情](plan/P5-6-compaction-retention.md) |
| P5-7 | 🟡 | Tool Result 裁剪 | 分级裁剪+artifact 引用 | [详情](plan/P5-7-toolresult-trim.md) |
| P5-8 | 🟡 | Export / Import | 稳定 schema 往返 | [详情](plan/P5-8-session-export-import.md) |
| P5-9 | 🟡 | Pi JSONL Importer | 解析/未知字段/不改原文件 | [详情](plan/P5-9-pi-jsonl-import.md) |

### Phase 6：主要 Provider

覆盖三大主 Provider 与高级能力，Agent Core 不含 Provider 特例。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P6-1 | 🟡 | OpenAI 适配 | 适配+contract tests | [详情](plan/P6-1-openai.md) |
| P6-2 | 🟡 | Anthropic 适配 | 适配+contract tests | [详情](plan/P6-2-anthropic.md) |
| P6-3 | 🟡 | Google Gemini 适配 | 适配+contract tests | [详情](plan/P6-3-gemini.md) |
| P6-4 | 🟡 | OAuth | PKCE/Device/refresh/callback | [详情](plan/P6-4-oauth.md) |
| P6-5 | 🟡 | Thinking / Reasoning | level+stream delta | [详情](plan/P6-5-thinking.md) |
| P6-6 | 🟡 | 图片输入 | image content part | [详情](plan/P6-6-image-input.md) |
| P6-7 | 🟡 | Prompt Cache | 缓存控制+命中 | [详情](plan/P6-7-prompt-cache.md) |
| P6-8 | 🟡 | 结构化输出 | JSON/structured | [详情](plan/P6-8-structured-output.md) |
| P6-9 | 🟡 | Provider-specific options | 透传+raw metadata | [详情](plan/P6-9-provider-options.md) |

### Phase 7：Git、Diff 与 Worktree

结构化 Git/Diff，支持 worktree 与大规模 diff。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P7-1 | 🟡 | Repo 检测 / branch / HEAD | 系统 Git 封装 | [详情](plan/P7-1-git-repo.md) |
| P7-2 | 🟡 | status / changed files | staged/unstaged/untracked | [详情](plan/P7-2-git-status.md) |
| P7-3 | 🟡 | 结构化 Diff | DiffFile/Hunk/分页/100k 行 | [详情](plan/P7-3-structured-diff.md) |
| P7-4 | 🟡 | stage / unstage / discard | 暂存操作 | [详情](plan/P7-4-git-stage.md) |
| P7-5 | 🟡 | Worktree | 创建/删除/不删用户数据 | [详情](plan/P7-5-worktree.md) |
| P7-6 | 🟡 | Git 缓存 / watcher | status 缓存+切换<50ms | [详情](plan/P7-6-git-cache.md) |
| P7-7 | ⚪ | Hunk / Line stage（优先级 P1） | 块/行暂存 | [详情](plan/P7-7-hunk-stage.md) |
| P7-8 | ⚪ | commit / branch / ...（优先级 P1） | P1 Git 操作 | [详情](plan/P7-8-git-operations.md) |

### Phase 8：Skills、Prompts 与 Instructions

确定性上下文来源，资源加载错误不崩溃。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P8-1 | 🟡 | Resource Loader | 加载 AGENTS.md/Skills/Prompt | [详情](plan/P8-1-resource-loader.md) |
| P8-2 | 🟡 | AGENTS.md 层级 | 根+路径层级聚合 | [详情](plan/P8-2-agents-md.md) |
| P8-3 | 🟡 | Skills | manifest/激活/冲突/热重载 | [详情](plan/P8-3-skills.md) |
| P8-4 | 🟡 | Prompt Templates | 参数/默认配置/覆盖 | [详情](plan/P8-4-prompt-templates.md) |
| P8-5 | 🟡 | Profiles / Agent Profile | 运行期 instructions | [详情](plan/P8-5-profiles.md) |
| P8-6 | 🟡 | 配置优先级（确定性） | 确定性合并 | [详情](plan/P8-6-config-priority.md) |
| P8-7 | 🟡 | Resource Diagnostics | 显示生效来源 | [详情](plan/P8-7-resource-diagnostics.md) |
| P8-8 | 🟡 | Hot Reload | 变更去抖重载 | [详情](plan/P8-8-hot-reload.md) |

### Phase 9：MCP

MCP 作为第一外部扩展机制，server 故障隔离。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P9-1 | 🟡 | stdio Transport | 本地进程接入 | [详情](plan/P9-1-mcp-stdio.md) |
| P9-2 | 🟡 | Streamable HTTP Transport | 远程接入+timeout/restart | [详情](plan/P9-2-mcp-http.md) |
| P9-3 | 🟡 | Tools / Resources / Prompts | 能力发现+注册 | [详情](plan/P9-3-mcp-capabilities.md) |
| P9-4 | 🟡 | Health / restart / cancel / logging | 故障隔离 | [详情](plan/P9-4-mcp-health.md) |
| P9-5 | 🟡 | Approval / 输出限制 / Secret 注入 | 每 server 独立权限 | [详情](plan/P9-5-mcp-approval.md) |
| P9-6 | 🟡 | MCP Config | workspace/global | [详情](plan/P9-6-mcp-config.md) |
| P9-7 | ⚪ | OAuth（优先级 P1） | 保护型 server 鉴权 | [详情](plan/P9-7-mcp-oauth.md) |

### Phase 10：WASM Plugin

WASM 作为第一代码插件机制，能力受控。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P10-1 | 🟡 | Plugin Manifest + signature | 元数据+签名校验 | [详情](plan/P10-1-plugin-manifest.md) |
| P10-2 | 🟡 | WASM Host | component 宿主+加载卸载 | [详情](plan/P10-2-wasm-host.md) |
| P10-3 | 🟡 | Tool / command + hooks | 注册+生命周期 hook | [详情](plan/P10-3-plugin-registration.md) |
| P10-4 | 🟡 | Plugin state | 状态保存 | [详情](plan/P10-4-plugin-state.md) |
| P10-5 | 🟡 | Capability / fuel / 内存 / 时间 | 默认无文件/网络/进程 | [详情](plan/P10-5-plugin-capability.md) |
| P10-6 | 🟡 | API version 兼容测试 | 版本兼容套件 | [详情](plan/P10-6-plugin-apiversion.md) |

### Phase 11：Sandbox 与跨平台强化

三平台核心可用，沙箱可控，进程树可清理。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P11-1 | 🟡 | NativeRestricted backend | 路径/env/资源限制 | [详情](plan/P11-1-sandbox-native-restricted.md) |
| P11-2 | 🟡 | macOS Sandbox profile | 系统 sandbox | [详情](plan/P11-2-sandbox-macos.md) |
| P11-3 | 🟡 | Linux Bubblewrap | bwrap 隔离 | [详情](plan/P11-3-sandbox-linux.md) |
| P11-4 | 🟡 | Windows AppContainer / Job | 进程级隔离 | [详情](plan/P11-4-sandbox-windows.md) |
| P11-5 | ⚪ | Docker / Podman（优先级 P1） | 容器沙箱 | [详情](plan/P11-5-sandbox-docker.md) |
| P11-6 | 🟡 | PTY Service | 终端/重连/自动清理 | [详情](plan/P11-6-pty-service.md) |
| P11-7 | 🟡 | 进程树清理 | 三平台取消清理 | [详情](plan/P11-7-process-tree-cleanup.md) |
| P11-8 | 🟡 | 跨平台路径 | 规范化/symlink/junction | [详情](plan/P11-8-cross-platform-path.md) |

### Phase 12：Multi-Agent

Parent/Worker 编排，写入隔离，取消传播。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P12-1 | 🟡 | Supervisor / Worker | parent/worker 抽象 | [详情](plan/P12-1-supervisor-worker.md) |
| P12-2 | 🟡 | 任务分解 / 任务图 | 依赖图调度 | [详情](plan/P12-2-task-graph.md) |
| P12-3 | 🟡 | 子 session / 独立 worktree | 写入隔离 | [详情](plan/P12-3-worker-worktree.md) |
| P12-4 | 🟡 | Worker 预算 / 模型 / 并发 | 预算可控 | [详情](plan/P12-4-worker-budget.md) |
| P12-5 | 🟡 | 结果聚合 / patch merge | 合并+冲突检测 | [详情](plan/P12-5-result-merge.md) |
| P12-6 | 🟡 | 取消树 | parent 取消联动 workers | [详情](plan/P12-6-cancel-tree.md) |

### Phase 13：CLI Host 与多 GUI 协议

完成 CLI/Core 一体化装配与多 GUI 连接协议：CLI 既能独立运行，也能同时服务多个本地与远程 GUI；GUI 经 GUI Connection Protocol 连接 CLI，不嵌入 Core。不开发真实 GUI，用协议测试客户端验证全流程。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P13-1 | 🟡 | app-service 完整化 + 统一 Command Source | CLI/GUI 共享入口与来源记录 | [详情](plan/P13-1-app-service.md) |
| P13-2 | 🟡 | CLI Host 装配与运行模式 | core-runtime + cli-host + pawork + 运行模式 + Event Hub | [详情](plan/P13-2-cli-host.md) |
| P13-3 | 🟡 | GUI Connection Protocol | 协议契约（Command/Query/Event/Snapshot） | [详情](plan/P13-3-gui-protocol.md) |
| P13-4 | 🟡 | GUI Server 与 Local Transport | CLI 内部协议服务器 + Unix Socket/Named Pipe | [详情](plan/P13-4-gui-server-local-transport.md) |
| P13-5 | 🟡 | 多 GUI 运行时 | Connection Manager + Subscription Hub + Snapshot/Event Replay + 慢客户端隔离 | [详情](plan/P13-5-multi-gui-runtime.md) |
| P13-6 | 🟡 | Remote Transport 占位与可替换 Adapter | 占位接口 + Mock 端到端 | [详情](plan/P13-6-remote-transport.md) |
| P13-7 | 🟡 | TS 类型生成落地 | 自动生成一致 | [详情](plan/P13-7-ts-typegen-final.md) |
| P13-8 | 🟡 | 大型 payload Artifact API | 按 ID 传递 | [详情](plan/P13-8-artifact-api.md) |
| P13-9 | 🟡 | 测试 GUI Client 与 API Contract Tests | 协议测试端 + 契约套件 | [详情](plan/P13-9-gui-client-contract-tests.md) |
| P13-10 | 🟡 | GUI Protocol schema 版本化 | 版本 + 兼容策略 | [详情](plan/P13-10-protocol-schema-version.md) |

---

**范围、MVP、横切门禁与风险监控**：见 [plan/README.md](plan/README.md)。
