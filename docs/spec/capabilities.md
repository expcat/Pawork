# Pawork 能力与入口矩阵

OPT-1 / [ADR-053](settings.md#adr-053opt-1-设置持久化2026-09-05)：Settings 审批模式保存为 Global 默认，信任选择按 canonical workspace 根路径保存；非 Global 高层禁止覆盖；命令与回执 JSON 形状保持不变。Appearance 保存用户目录 `desktop.json`。实现/自动验证/人工验收状态分别见 [ROADMAP](../ROADMAP.md)。

> 基线日期：2026-09-03。状态词汇见 [README.md](README.md#3-状态词汇)。本表记录生产可见面，不以“代码存在”替代当前阶段复验或发布证明。

## 1. 产品能力

| ID | 能力 | 用户入口 | 交付状态 | 关键边界 |
| --- | --- | --- | --- | --- |
| CAP-CHAT-01 | 流式多轮对话、单次 Run、取消、模型选择 | `chat`、`run`、`models` | 已实现 | Engine 只消费 canonical domain；真实通道可用性取决于凭证与运行期模型目录。 |
| CAP-SESSION-01 | 会话列表、查看、恢复、导出/导入、分支 | `sessions`、`--resume` | 已实现 | envelope v1、DB v14、export v3；损坏/Secret 导入 fail-closed。 |
| CAP-SESSION-02 | Desktop 会话生命周期：无项目直建（Unassigned）、行右侧改名/归档、命名模型自动标题 | Desktop TaskRail / New task | 已实现（ADR-054，API 1.11）；真窗口验收待 OPT-2 收尾 | 归档仅隐藏不删除，wire 保留反归档写口；自动标题须配置 Global `naming_provider`/`naming_model`，未配置不命名、不用启发式；无项目会话文件类工具 fail-closed。 |
| CAP-AGENT-01 | 多轮 Agent loop 与工具调用 | `chat`、`run`、GUI/headless/ACP | 已实现 | 轮数有界；Provider 特例只在 adapter，不进 Engine。 |
| CAP-TOOL-01 | read/list/search/find/write/edit/apply_patch/run_command 八工具 | Agent tool call | 已实现 | 文件输入为 workspace-relative；写/进程能力受 Policy。 |
| CAP-APPROVAL-01 | 工具审批、Run 内授权、拒绝、取消 | CLI approval、Desktop 审批卡 | 已实现 | 非 TTY/JSON deny-all；CLI resume seal Denied，GUI resume 保留 pending。 |
| CAP-EXEC-01 | 子进程、进程树回收、Sandbox、PTY | `run_command`、Desktop Terminal | 已实现 | Sandbox 可观测回退；PTY 创建的 AskUser 当前 fail-closed 为 Deny。 |
| CAP-PROVIDER-01 | 六条第一方通道、Anthropic 协议适配、OpenAI-compatible 端点 | `models`、全局 provider/model、配置 | 已实现 | 六通道：chatgpt/xai/glm-coding/opencode-go/qwen-token-plan/deepseek；未启用 feature/未知能力显式拒绝。 |
| CAP-AUTH-01 | API key、OAuth 登录/刷新、脱敏状态 | `auth list/set-key/login/logout` | 已实现；部分待人工验收 | ChatGPT/xAI 自然临期 refresh 仍需真实账号窗口；OS Keychain 不在当前实现。 |
| CAP-CONTEXT-01 | 上下文预算、compaction、用量/定价 | Run、`usage` | 已实现 | usage 幂等冲突与哨兵口径仍需专项复核。 |
| CAP-GIT-01 | diff、checkpoint、rollback、fork/worktree 支撑 | `diff`、`rollback`、Desktop Changes | 部分实现 | Core/CLI 已实现；Desktop Changes 只读，stage/unstage/hunk 是 ADR 候选。 |
| CAP-RESOURCE-01 | AGENTS.md、Skills、profiles、`@file`、配置导入 | chat/run、`import` | 已实现；部分 GUI 缺口 | Desktop 已消费 host `@` 展开和 MCP 列表；无 `@` 候选浮层/已加载规则 query。 |
| CAP-MCP-01 | MCP Client 配置、测试、工具/资源 | `mcp list/test`、Desktop Resources | 已实现 | MCP auth 与主 Provider auth 分域；Pawork 作为 MCP Server 未实现。 |
| CAP-GUI-01 | 本机 GUI server 与 GPUI Desktop | `gui serve`、`pawork-desktop` | 生产链路已实现；完整人工门禁未完成 | 项目/会话/Changes/Terminal 主路径已验收；完整视觉、AX/IME/跨平台仍需专项证据；断线不取消 Run。 |
| CAP-SETTINGS-01 | Desktop Settings：供应商连接、认证、模型目录/默认项、通用、权限、MCP、终端、外观、高级连接诊断与关于 | TaskRail `Local` 行 Settings | SET-1～SET-6h 已实现并通过各片定向门禁，本机真窗口验收通过（2026-09-05）；真实账号矩阵人工验收待后续 | 业务设置 Host-driven；Secret 只进 auth backend；外观字号仅当前 Desktop 会话；高级只读消费当前握手/endpoint/resume/ack，不显示 token 或伪造配置 instance。About 只在当前认证握手提供非空 Host 数据目录时启用，缺失/断线 fail-closed。见 [settings.md](settings.md)。 |
| CAP-CLIENT-01 | GUI typed client、headless JSON、ACP | `headless`、`acp serve`、`pawork-client` | 已实现 | GUI/headless/ACP 能力表同源；wire/JSON 受冻结契约约束。 |
| CAP-WORKFLOW-01 | plan、tasks 与演示型多 Agent 编排 | `plan`、`tasks`、`agents demo` | 已实现/受限 | `agents` 是 demo 入口；teams/goal/automation/monitor 的完整产品面已归档或候选。 |
| CAP-OPS-01 | 服务安装/启停、状态、观察、关闭、诊断 | `service`、`status`、`watch`、`shutdown`、`doctor` | 已实现；平台验收不完整 | service 默认 dry-run，显式 `--apply` 才改系统；Windows SCM 实机仍为候选验收。 |
| CAP-IMPORT-01 | 外部配置与本机会话导入 | `import`、`sessions import --from` | 已实现 | 配置扫描只读、不执行 hook/不启动 MCP；Claude/Codex 会话扫描有界且不跟 symlink。 |

## 2. CLI 顶级入口

当前 clap 顶级名称固定为 21 个；精确参数与帮助文本以 `pawork --help` 和 [CLI 源码](../../crates/cli/src/lib.rs) 为准。

| 命令 | 主要作用 | 状态/限制 |
| --- | --- | --- |
| `chat` | 交互式流式会话 | 已实现；TTY 才可交互审批。 |
| `sessions` | `list/show/export/import/fork` | 已实现；resume 通过全局/命令参数进入。 |
| `run` | 非交互单次任务 | 已实现；JSON/非 TTY deny-all approvals。 |
| `models` | 聚合静态与运行期模型目录 | 已实现。 |
| `auth` | `list/set-key/login/logout` | 已实现；auth file 为当前 Secret 后端。 |
| `gui` | `serve` 本机 GUI 连接服务 | 已实现；需要 token 认证。 |
| `diff` | 查看工作区变更 | 已实现。 |
| `rollback` | 回滚到 checkpoint | 已实现；属破坏性文件动作，受既有安全/确认语义约束。 |
| `mcp` | `list/test` | 已实现；MCP Client，不是 MCP Server。 |
| `import` | 外部配置扫描/导入 | 已实现；只读扫描、Secret fail-closed。 |
| `headless` | JSONL stdio 客户端协议 | 已实现；stdout 只输出 JSONL。 |
| `acp` | `serve` ACP 通道 | 已实现；裸 `acp` 是参数错误。 |
| `service` | `install/start/stop` | 已实现；默认 dry-run，`--apply` 才产生系统变更。 |
| `status` | 实例/GUI 服务状态 | 已实现；在加载 AppCore 前运行。 |
| `watch` | 观察服务/实例状态 | 已实现；在加载 AppCore 前运行。 |
| `shutdown` | 请求关闭服务 | 已实现；在加载 AppCore 前运行。 |
| `doctor` | 数据目录、socket、DB、握手诊断 | 已实现；支持结构化输出。 |
| `usage` | 用量查询 | 已实现；既有 usage 登记项仍需专项复核。 |
| `tasks` | 后台任务状态与控制面 | 已实现。 |
| `plan` | Plan 工作流 | 已实现。 |
| `agents` | 多 Agent demo | 仅演示入口，不代表完整 teams/automation 产品。 |

全局参数：`--provider`、`--model`、`--instance`、`--json`、`--approval-mode`。`--json`/`--json-stdio` 的 stdout 只能承载 JSONL，日志必须走 stderr。

## 3. Provider 与凭证能力

| Provider ID | 凭证形态 | 当前产品状态 |
| --- | --- | --- |
| `chatgpt` | OAuth | 已实现；自然临期 refresh 待真实账号人工验收。 |
| `xai` | OAuth / API key | 已实现；SET-4 起 API-key adapter 与 Desktop 写操作已接通，真实账号验收 pending。 |
| `glm-coding` | API key | 已实现。 |
| `opencode-go` | API key | 已实现。 |
| `qwen-token-plan` | API key | 已实现。 |
| `deepseek` | API key | 已实现。 |
| Kimi（待登记稳定 ID） | Kimi Platform API key / Kimi Code OAuth | Settings 活动线新增；当前未实现。 |

此外存在 feature 门控的 Anthropic Messages adapter 和可配置 OpenAI-compatible 入口。它们不应被误写成第七条 `CHANNEL_REGISTRY` 产品通道；实际启用能力以宿主 feature、配置和 `pawork models` 返回为准。

## 4. Desktop 可见面

| 面 | 当前能力 | 状态 |
| --- | --- | --- |
| TaskRail | 会话/任务选择、新建、长标题截断 | 生产入口已实现；Settings 将在 `Local` 行增加 gear。 |
| Timeline | 变高虚拟化、流式条目、审批、fork 边界、回底 | 生产入口已实现；完整视觉/长会话仍按风险专项复验。 |
| Composer | 输入、发送、`@` host 展开 | 部分实现；真实 IME/粘贴仍待人工，`@` 候选仅在 Host capability 存在时实现。 |
| Changes | Files/Summary/DiffView/ActivityPopover | 只读生产入口已实现；写操作仍是 ADR 候选。 |
| Terminal | 创建、输入、resize、Stop/Close、输出 | 生产入口已实现；真 PTY 主路径已验收。 |
| Resources | MCP server/tool 状态只读列表 | 生产入口已实现；无 host query 的分区不展示。 |
| Settings | 供应商连接、认证、模型目录/default、通用、权限/MCP/终端、外观、高级连接诊断与关于 | SET-1～SET-6g 已实现并通过定向门禁；SET-6h 供应商级代理开关按 ADR-052（API 1.10）实现，真窗口验收通过（2026-09-05）；About 按 ADR-051 动态启用，真实账号/完整真窗口/人工验收待后续。 |

## 5. 不可宣称为已交付

以下能力仅为已确认未排期、候选或归档：多账户池与缓存感知路由、远程 GUI、Web/Cloud、完整 teams/goal/automation/monitor、GUI stage/unstage/hunk、WASM 插件生态、第一方 IDE 扩展、MCP Server、自更新/安装器、企业 SSO、发布与三平台门禁。完整列表见 [backlog.md](backlog.md)。
