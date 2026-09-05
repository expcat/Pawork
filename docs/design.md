# Pawork 设计

> 功能设计事实源：目标与原则、各能力域及其到参照项目的映射、明确排除的形态。包布局与冻结契约见 [architecture.md](architecture.md)；当前活动线见 [ROADMAP.md](ROADMAP.md)；包内实现见 [包级 Spec](spec/README.md)；Desktop 见 [gui-design.md](gui-design.md)；参照手册与调研附录见 [references.md](references.md)。未排期候选见 [产品候选](spec/backlog.md)。

---

## 1. 目标与原则

1. Pawork 是纯 Rust 的 CLI Coding Agent + 独立 GPUI Desktop：`pawork` 二进制内置 Core（引擎、工具、Provider、存储、策略），Desktop 经 GUI Connection Protocol 连接 CLI。
2. **纵向优先**：先保证内置工具真实接线、能在真实仓库完成编码任务，再在同一窗口增量加面。WASM 插件等扩展生态不在当前产品范围（候选见 [产品候选](spec/backlog.md)）。
3. **架构红线不变**：纯 Rust、CLI 与 Core 同进程同二进制、GUI 独立进程走协议、canonical domain 纯净、事件可持久化可重放、Secret 不落库不入日志、Engine 无 Provider 名称特例分支、禁止循环依赖（全文见 [architecture.md](architecture.md) §1）。
4. **无消费者不合入**：任何模块必须有真实装配点；零消费者代码归档（git tag `v2-final` 兜底）。
5. **少测试、无全量门禁**：验证纪律见 [AGENTS.md](../AGENTS.md)；三类关键测试（安全红线、持久化/重放 golden、协议 golden）不推迟。

---

## 2. 能力域与参照映射

「参照」列给出该能力在参照项目中的对应实现。项目背景见 [references.md](references.md)；反向分类见同文 §6。机制细节见其附录 A。

### 对话 CLI

| 能力 | 参照 |
| --- | --- |
| `pawork chat` 流式多轮 REPL、Ctrl-C 取消当轮 | Codex CLI；OpenCode/Pi 的终端交互语义（只对标行为——Pawork 无 TUI，见 §4） |
| `pawork models` 模型目录 | OpenCode 外置 [models.dev](https://models.dev) vs Pi 自维护内置目录——Pawork 走 registry + config 覆盖 |
| TOML 配置 + env key（配置**无 api_key 字段**） | OpenCode `opencode.json` 与 `auth.json` 分离；Pi `auth.json`（0600）与 `!command`/`$ENV` 插值 |
| openai-compatible 适配器（可配 `base_url`） | GLM Coding Plan / OpenCode Go / 自建网关（opencodex、Codex Router 等） |
| 可读错误呈现（401/429/超时/断网） | OpenCode ≤5 次重试、遵循 Retry-After；Pi agent 层退避 |

### 会话持久化与恢复

| 能力 | 参照 |
| --- | --- |
| 事件流落盘（`AgentEventEnvelope` + append-only 存储） | 冻结契约（[architecture.md](architecture.md) §3.2）；最接近的外部同形：DeepSeek Harness 仅追加 `SessionEvent` 日志；相邻：Pi JSONL 树形 session、OpenCode 消息级 SQLite |
| `pawork sessions list/show`、`--resume` 续聊 | Codex sessions/resume；OpenCode/Pi；DeepSeek Harness 从同一事件流 resume |
| `pawork run`（非交互单次）+ `--json` JSONL | Codex exec / headless；DeepSeek Harness `dsh-headless`；映射见 [spec/contracts.md](spec/contracts.md) |

### Agent Loop 与工具

| 能力 | 参照 |
| --- | --- |
| 只读四工具 read_file/list_directory/search_text/find_files | OpenCode 内置工具族；Codex 工具面 |
| 引擎多轮工具循环（每 run 轮数上限防失控） | OpenCode agent `steps` 上限；工具映射在 adapter 侧完成，engine 零厂商分支 |
| OpenAI tools / Anthropic tool_use 双协议 | 官方 API；Pi `anthropic-messages` |
| workspace roots + `workspace_id + relative_path` 输入红线 | tool-api 类型化路径；OpenCode permission 边界 |
| write_file/edit_file/apply_patch | OpenCode edit/write/patch；Codex apply_patch |
| 终端审批（一次/本运行/拒绝）+ `--approval-mode` 五档（默认 ReadOnly；旧 `on-failure` 仅兼容读入并映射 NeverAsk） | Codex approval modes；OpenCode `permission`；DeepSeek Harness 把 sandbox 与 approval 做成独立 knob |
| 未信任 workspace 强制询问 | Pi Project Trust |
| 路径越界/symlink/TOCTOU + 提示注入回归 | policy 整包红线 |

### 命令执行与沙箱

| 能力 | 参照 |
| --- | --- |
| run_command + 沙箱（AppContainer/Landlock/Seatbelt）+ 可观测回退（不是拒跑；CLI/GUI 必须展示 fallback） | Codex sandbox；DeepSeek Harness `ctx.sandbox`；Windows Job Object + AppContainer |
| shell 风险分类 → 审批（Dangerous 必询） | policy `shell` 分类；OpenCode `permission.bash` |
| 取消 = 清理整棵进程树 | Job Object / 进程组 |
| 输出截断 + 完整输出落工件 | 上下文预算纪律；对照 [references.md](references.md) 附录 A §5.3 前缀稳定技巧 |

### 上下文预算与用量

| 能力 | 参照 |
| --- | --- |
| 软限压缩 / 硬限截断 + `/compact` | OpenCode 自动 compaction；compaction=重写前缀=缓存全失效 |
| token 与费用统计（micros 定价、无定价不编造） | OpenCode 消息级 cost/tokens；Pi footer 命中率；LiteLLM 缓存差价 |
| 模型 registry（context window / 定价 / 别名） | models.dev；Pi `models-store.json` |

### 多 Provider 与认证

| 能力 | 参照 |
| --- | --- |
| 六条首发通道：ChatGPT OAuth、xAI Grok OAuth、Z.AI GLM Coding Plan、OpenCode Go、Qwen Token Plan、DeepSeek | 各厂商官方 API；端点/凭证形态对照 Codex Router |
| ChatGPT/xAI 共用 Responses transport；按模型 capability 选 Chat/Responses | canonical 保持 provider-neutral |
| `auth.json` 文件凭证 + `pawork auth` | 形态对齐 Codex CLI；额外锁定 0600、跨进程写锁、原子写、损坏 fail-closed、掩码展示与全链日志脱敏。env 仅作 headless/CI fallback |
| ChatGPT/xAI OAuth（PKCE/Device/refresh/callback） | Codex Sign in with ChatGPT；OAuth client secret 不进入 adapter/仓库 |
| REPL `/model` `/provider` 切换（事件流记录变更） | OpenCode `/models` + transform 归一化历史；Pi 跨厂商 handoff |
| Z.AI GLM Coding Plan 端点预设 | 国际站 `https://api.z.ai/api/coding/paas/v4` |

### Desktop GUI

见 [gui-design.md](gui-design.md)。吸收 Codex/OpenCode/Zed 的主对话壳，不复制完整 IDE；独立进程 + GUI Connection Protocol；GUI 只消费对话与工作台所需要子集。

### Git、Diff 与 Checkpoint

| 能力 | 参照 |
| --- | --- |
| `pawork diff` 结构化 diff（分页、CRLF/中文文件名） | unified diff 状态机 parser |
| 写前 checkpoint + `pawork rollback` | OpenCode `/undo` `/redo` 是 turn 级经 Git，粒度不同于 Run/Tool 级快照 |
| git 状态感知 + 注入防护 | `validate_position_arg` 等防御 |
| 审批 UX 升级为 diff 预览 | 写工具审批的可见面 |

### MCP、资源与兼容导入

| 能力 | 参照 |
| --- | --- |
| MCP client（rmcp）+ 与内置工具共存注册 | [MCP 官方](https://modelcontextprotocol.io)；「Pawork 作为 MCP server」为候选反向形态 |
| AGENTS.md / Skills / profiles 加载注入 | [AGENTS.md 约定](https://agents.md)；OpenCode rules、Codex AGENTS.md；DeepSeek Harness `tool-skill` |
| `@file` 引用 + file-index 模糊补全 | 各家 `@` 语义 |
| 一键导入本机 Claude/Codex/Grok/Cursor/Pi 配置（只读） | 各工具本机配置布局；账户/端点导入源见附录（cc-switch、CLIProxyAPI、opencodex、Codex Router） |
| config 完整六层 + Profile | 层级合并引擎 |

### 服务化与客户端

| 能力 | 参照 |
| --- | --- |
| `pawork headless --json-stdio` + SDK | Codex TS/Python SDK；OpenCode SDK/serve；Pi `createAgentSession()`；DeepSeek Harness headless |
| `gui serve` 多客户端 + 断线 Replay + 慢客户端隔离 | Desktop 增量见 [gui-design.md](gui-design.md) |
| `pawork acp serve` | [Agent Client Protocol](https://github.com/zed-industries/agent-client-protocol) |
| 会话分支 / `pawork session fork`（仅闭合 turn 后稳定事件） | Pi session tree；OpenCode 子 session；DeepSeek Harness `ctx.sessions.fork` |
| `pawork service install/start/stop` + 运维子命令 | 六运行模式（外部无直接对标） |
| PTY 交互式命令 + GUI Terminal | DeepSeek Harness `tool-terminal` + 持久 bash |

### 工作流、多 Agent 与控制面

| 能力 | 参照 |
| --- | --- |
| Plan 审批 gate（未批准整版拦截 turn；无 plan 放行） | 相邻：OpenCode question/todowrite、DeepSeek Harness planning |
| 多 Agent 编排（spawn/registry/cancel-tree/recovery/budget-gate） | OpenCode `task` 子代理 + 权限派生；Pi「核心不内置子代理」；DeepSeek Harness `tool-subagent`。CCR in-band 标签为**明确不采纳**的反例 |
| 子 Agent 声明式 provider/model/账户绑定 + 预算分配 | opencode `agent.model`；方案见 [references.md](references.md) 附录 B（F4-A+B） |
| 多账户池 / 租约 / 路由 / 会话-账户亲和 | opencodex 账户池；CLIProxyAPI RR/加权/fill-first；claude-relay-service sticky；Codex Router 仅额度耗尽换**模型** |
| 额度感知与预算 gate + `pawork usage` | opencodex 主动配额窗口；LiteLLM 层级预算 |
| audit / tenant 控制面 | LiteLLM org/team/user/key；`dedup_key`/audit JSONL 冻结契约 |

---

## 3. 已确认扩展功能族（G1–G7）

调研结论与方案全文见 [references.md](references.md) 附录 A–C。决策原则：**减少实现复杂度、优先缓存命中**。未排期转正规则见 [产品候选](spec/backlog.md)。

| ID | 功能 | 说明 |
| --- | --- | --- |
| G1 | 同 Provider 多账户池与订阅 plan 凭证 | 激活账户层 + 订阅 plan OAuth kind + `auth.json` 多账户命名（0600、原子写、损坏 fail-closed） |
| G2 | 额度窗口跟踪与预算 gate | LocalLedger 派生 + 响应头/错误体被动配额信号归一为 QuotaSnapshot；远端适配器保持冻结 |
| G3 | 缓存感知的会话-账户亲和路由 | SessionBinding 亲和默认开 + 新会话再平衡 + 「配额余量优先」；请求级轮换不作默认 |
| G4 | 子 Agent 声明式 provider/model/账户绑定 | Profile/spawn 声明 → RouteContext；默认继承父绑定、显式覆盖；预算经 budget-gate |
| G5 | canonical 输入缓存策略控制 | 无厂商字段的 cache 注解 + registry 能力表 + adapter 映射 + 用量入账；附加式契约，golden 先行 |
| G6 | 账户/端点配置导入 | 只读导入源，secret 直接转存 Pawork auth 文件 |
| G7 | 对外账户池网关模式 | **不内建**；以 openai-compatible 上游接外部网关 |

明确不做：身份伪装（Claude Code UA、`identity-confuse`）、in-band 子代理标签、请求级默认轮换、响应/语义缓存、独立网关 app、订阅转售。

配套约定：执行期凭证 fail-closed；少测试无全量门禁；缓存命中率目标 95 / 97 / 99（口径见附录 C）。

---

## 4. 架构红线排除项（不实现）

| 功能 | 来源 | 排除理由 |
| --- | --- | --- |
| 交互式全屏 TUI | OpenCode / Pi | 以 CLI 交互模式 + GPUI Desktop 为用户界面 |
| JS/TS 插件运行时（Bun/Node、hot-reload、JS hooks） | OpenCode / Pi / DeepSeek Harness Cordis | 纯 Rust 红线；若未来做代码插件，只评估 WASM + in-process hooks |
| npm 生态传输 | OpenCode / Pi / DeepSeek Harness | 同上 |

未排期的其它候选（自定义命令、webfetch、IDE 扩展等）只登记在 [产品候选](spec/backlog.md)，落地时须遵守冻结契约先行。
