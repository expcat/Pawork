<!-- 代码 Review 文档总索引 -->
# Pawork 代码 Review 文档

> 本目录是对整个仓库（21 成员：19 库 + 2 应用，约 21.9 万行 Rust）的完整代码级 Review 产出，面向后续分析、重构与新人上手。
>
> 与既有文档的分工：[docs/architecture.md](../architecture.md) 是架构事实源（红线 / 布局 / 冻结契约），[docs/spec/crates/](../spec/README.md) 是每包功能 Spec；**本目录是代码实况的镜像**——逐 crate、逐文件的类型与方法功能清单，全部以当前工作区源码为准生成。冲突时以源码为准。

## 阅读顺序

1. [architecture.md](architecture.md)：全景架构——分层模型、全量 crate 依赖图（mermaid）、四条跨包热路径、冻结契约速查。
2. 按关注面选读 [backend/](backend/)（19 个库 + CLI 宿主）或 [frontend/](frontend/)（GPUI Desktop，按状态层 / 视图层拆分）。
3. 每篇 crate 文档固定七节：职责与边界 → 依赖关系 → 文件清单 → **类型与方法功能列表（核心）** → 关键行为与契约 → 测试资产 → 协作关系图。

## 后端（crates/ + apps/pawork）

按依赖分层组织；层内按字母序。行数为生成时统计，含 tests。

### L0 基础层（无内部依赖）

| 包 | 文档 | 规模 | 一句话 |
| --- | --- | --- | --- |
| pawork-domain | [backend/domain.md](backend/domain.md) | 18 文件 / 5.2k 行 | canonical 领域类型 + Provider/Tool 契约 + 事件信封 v1（纯净红线） |
| pawork-transport | [backend/transport.md](backend/transport.md) | 6 文件 / 1.7k 行 | framed 字节传输：local（UDS / named pipe）+ memory |

### L1 契约层

| 包 | 文档 | 规模 | 一句话 |
| --- | --- | --- | --- |
| pawork-protocol | [backend/protocol.md](backend/protocol.md) | 37 文件 / 14.0k 行 | GUI 帧 / headless-json / core-api / typegen + 三通道 registry + 共享投影 reducer |
| pawork-testkit | [backend/testkit.md](backend/testkit.md) | 2 文件 / 0.9k 行 | dev-only：MockProvider / MockTool / 契约断言 |

### L2 平台服务层

| 包 | 文档 | 规模 | 一句话 |
| --- | --- | --- | --- |
| pawork-policy | [backend/policy.md](backend/policy.md) | 6 文件 / 2.6k 行 | 安全内核：PolicyDecision / ApprovalMode / path 内核 / shell 风险分类 |
| pawork-auth | [backend/auth.md](backend/auth.md) | 11 文件 / 4.6k 行 | Secret 后端 / OAuth / 脱敏解析链（locator 单一事实源） |
| pawork-providers | [backend/providers.md](backend/providers.md) | 34 文件 / 13.0k 行 | net/（http/sse/retry）+ registry/pricing/usage/negotiate/reasoning + 六通道 channels/ |
| pawork-storage | [backend/storage.md](backend/storage.md) | 31 文件 / 20.0k 行 | SQLite Actor + session DDL/迁移/export + blob（PWB1 / protected / checkpoint） |
| pawork-workflow | [backend/workflow.md](backend/workflow.md) | 12 文件 / 2.6k 行 | plan / task 纯 reducer |
| pawork-control-plane | [backend/control-plane.md](backend/control-plane.md) | 16 文件 / 13.8k 行 | 控制面 core + quota/ + credential/（lease/pool） |

### L3 执行层

| 包 | 文档 | 规模 | 一句话 |
| --- | --- | --- | --- |
| pawork-exec | [backend/exec.md](backend/exec.md) | 11 文件 / 7.0k 行 | process / sandbox（Seatbelt/Landlock）/ PTY |
| pawork-git | [backend/git.md](backend/git.md) | 13 文件 / 3.7k 行 | Diff / Status / GitService / HunkStage / worktree |
| pawork-workspace | [backend/workspace.md](backend/workspace.md) | 35 文件 / 15.3k 行 | workspace service + file_index + resources + config 六层矩阵 + 五来源 import |
| pawork-engine | [backend/engine.md](backend/engine.md) | 18 文件 / 6.5k 行 | Agent Engine：tool_loop / session_turn / context / cancel（生产依赖仅 domain） |

### L4 组合层

| 包 | 文档 | 规模 | 一句话 |
| --- | --- | --- | --- |
| pawork-tools | [backend/tools.md](backend/tools.md) | 20 文件 / 10.0k 行 | 八工具 + ToolScheduler + mcp/ |
| pawork-orchestration | [backend/orchestration.md](backend/orchestration.md) | 13 文件 / 7.3k 行 | supervisor / budget / lifecycle / merge / task_graph / worktree / identity |

### L5 装配层

| 包 | 文档 | 规模 | 一句话 |
| --- | --- | --- | --- |
| pawork-app | [backend/app.md](backend/app.md) | 65 文件 / 31.5k 行 | 装配宿主 AppCore + gui_server/ + gui_host/ 分发表（最大库） |

### L6 接口层与宿主

| 包 | 文档 | 规模 | 一句话 |
| --- | --- | --- | --- |
| pawork-cli | [backend/cli.md](backend/cli.md) | 30 文件 / 12.1k 行 | 21 子命令 + channels/acp/（AcpHost） |
| pawork-client | [backend/client.md](backend/client.md) | 15 文件 / 6.4k 行 | framed 连接面 + headless/ SDK |
| pawork（bin） | [backend/pawork-host.md](backend/pawork-host.md) | 2 文件 / 0.3k 行 | composition root + redact.rs（RedactingFmtLayer） |

## 前端（apps/desktop，GPUI 独立进程）

Desktop 共 60 文件 / 40.2k 行，按四层职责拆为四篇：

| 文档 | 覆盖 | 内容 |
| --- | --- | --- |
| [frontend/overview.md](frontend/overview.md) | main.rs + 全进程 | 进程模型、与 CLI Host 的连接链路、四层职责与依赖纪律 |
| [frontend/state.md](frontend/state.md) | platform/ + controller/ + projection/ | socket/token 平台侧、会话/设置/终端控制器、无 gpui 的投影 reducer |
| [frontend/views.md](frontend/views.md) | ui/ 顶层模块 | shell_layout / timeline / input_area / inspector / changes / approval_card / task_rail / theme / i18n 等主界面 |
| [frontend/components-settings.md](frontend/components-settings.md) | ui/components/ + ui/settings/ + ui/accessibility/ | 通用组件库、Settings 八个页面、macOS AX 桥 |

## 使用建议（重构场景）

- **评估改动爆炸半径**：先查 architecture.md 的依赖图找下游，再读目标 crate 文档 §2「被哪些包依赖」。
- **定位功能实现**：各 crate 文档 §4 方法功能列表按模块组织，配合 rg 精确定位。
- **契约 / 红线核查**：architecture.md §4 冻结契约速查表 + 各 crate 文档 §5；改契约前先看对应 golden 位置。
- **跨包链路**：architecture.md §3 四条热路径（Agent loop / GUI 连接 / 事件持久化 / 凭证脱敏）给出调用链与「不要做的」清单。
