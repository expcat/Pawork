# Pawork Spec 文档集

> 基线日期：2026-09-01。状态：**现行（Living）**。本目录描述 Pawork 当前产品范围、需求、可见能力、稳定契约、安全边界、Desktop、验证与运维约束，并承载 **21 个包的逐包 Spec** 与跨包链路速览；它是跨事实源的产品化索引与包内功能的文档化镜像，**不是源码、协议形状或阶段状态的新事实源**。

## 1. 文档范围

### 1.1 产品篇

| 文档 | 回答的问题 |
| --- | --- |
| [product.md](product.md) | Pawork 为谁解决什么问题，当前产品边界、用户流程与产品需求是什么？ |
| [capabilities.md](capabilities.md) | 当前有哪些用户可见能力、入口与限制？哪些仍是部分交付或待人工验收？ |
| [contracts.md](contracts.md) | 哪些 API、wire、磁盘格式与安全语义已经冻结，如何演进？ |
| [security.md](security.md) | 资产、信任边界、威胁、Policy、Sandbox、Secret 与路径要求是什么？ |
| [desktop.md](desktop.md) | Desktop 的信息架构、交互流程、状态、可访问性与验收边界是什么？ |
| [settings.md](settings.md) | 已立项的 Settings 如何管理供应商认证、模型发现、默认项与后续设置页？ |
| [verification.md](verification.md) | 需求如何映射到自动化、golden、真实冒烟和人工证据？当前缺口是什么？ |
| [operations.md](operations.md) | 如何启动、配置、诊断、备份与恢复本机实例？当前发布/运维边界是什么？ |
| [backlog.md](backlog.md) | 已确认扩展、未排期候选、排除项和候选转正闸门是什么？ |
| [feature-template.md](feature-template.md) | 大型候选转正时，Feature Spec 最少应包含哪些内容？ |

### 1.2 包级 Spec

每包一篇，位于 [crates/](crates/)，目标是**读文档即可了解该包全部功能与行为、尽量不读代码**。按写入集读取：进某包前读该包一篇，不要一次读完 21 份。

| Spec | 包 | 一句话职责 |
| --- | --- | --- |
| [crates/domain.md](crates/domain.md) | `pawork-domain` | canonical 领域类型 + provider/tool 契约 + 事件信封（无内部依赖，纯净红线） |
| [crates/protocol.md](crates/protocol.md) | `pawork-protocol` | GUI 帧 / headless JSON / core-api / typegen + 三通道 registry + 共享投影 reducer |
| [crates/testkit.md](crates/testkit.md) | `pawork-testkit` | dev-only MockProvider/MockTool 与契约断言 |
| [crates/policy.md](crates/policy.md) | `pawork-policy` | 安全内核：PolicyDecision/ApprovalMode、shell 风险分类、路径校验 |
| [crates/exec.md](crates/exec.md) | `pawork-exec` | 进程执行 / 沙箱（Seatbelt/Landlock/AppContainer）/ PTY；ADR-052 依赖 policy 路径 helper |
| [crates/tools.md](crates/tools.md) | `pawork-tools` | 八个内置工具 + ToolScheduler + MCP client |
| [crates/workspace.md](crates/workspace.md) | `pawork-workspace` | workspace 服务、file_index、resources、六层配置、五来源导入 |
| [crates/storage.md](crates/storage.md) | `pawork-storage` | SQLite Actor + session 事件存储（schema v14）+ PWB1 blob |
| [crates/providers.md](crates/providers.md) | `pawork-providers` | HTTP/SSE 传输 + registry/pricing/usage/negotiate/reasoning + 六通道 adapter |
| [crates/auth.md](crates/auth.md) | `pawork-auth` | Secret 后端、OAuth（PKCE/Device）、credential locator、脱敏 |
| [crates/git.md](crates/git.md) | `pawork-git` | Diff/Status/GitService/HunkStage/worktree/merge |
| [crates/engine.md](crates/engine.md) | `pawork-engine` | Agent tool loop、审批等待、取消、压缩注入点（生产依赖仅 domain） |
| [crates/workflow.md](crates/workflow.md) | `pawork-workflow` | plan/task 纯 reducer |
| [crates/orchestration.md](crates/orchestration.md) | `pawork-orchestration` | 多 Agent supervisor/budget/lifecycle/merge/task_graph |
| [crates/control-plane.md](crates/control-plane.md) | `pawork-control-plane` | 控制面：tenant/usage/audit + quota + credential lease/pool |
| [crates/transport.md](crates/transport.md) | `pawork-transport` | framed 字节传输：local（UDS/named pipe）+ memory |
| [crates/app.md](crates/app.md) | `pawork-app` | AppCore 装配宿主 + 领域服务 + gui_server/gui_host |
| [crates/cli.md](crates/cli.md) | `pawork-cli` | 21 子命令 + REPL + headless + ACP host |
| [crates/client.md](crates/client.md) | `pawork-client` | GuiClient framed 连接面 + headless SDK + probe |
| [crates/pawork.md](crates/pawork.md) | `pawork`（bin） | composition root + tracing 全链脱敏 |
| [crates/desktop.md](crates/desktop.md) | `pawork-desktop`（bin） | GPUI 四层桌面客户端（业务依赖仅 pawork-client） |

### 1.3 跨包链路

[flows.md](flows.md)：Agent loop、GUI 连接、事件持久化与重放、凭证与脱敏四条跨包链路的速览与红线；进单包前的全局定位用。

不在本目录重复维护：布局与依赖边（[architecture.md](../architecture.md)）、逐符号 API（源码/rustdoc/golden）、任务状态（[ROADMAP](../../ROADMAP.md)）、视觉 token 明细（[GUI 视觉基准](../../design/README.md)）。

## 2. 事实源优先级

出现冲突时按下列顺序处理，并回写较低层文档：

1. 当前分支源码、检入 schema/golden、工作区差异、实际运行日志与真实远程状态；
2. 已 Accepted 的 ADR 与 [docs/architecture.md](../architecture.md) 中的布局/冻结契约；
3. [ROADMAP.md](../../ROADMAP.md) 中的活动状态；
4. 本 Spec 文档集（产品篇与包级 Spec）。

Spec 中的能力状态不替代验证结论。某项“已实现”只说明生产路径存在；是否完成当前阶段复验、真实环境验收或发布门禁，必须再看 [verification.md](verification.md)。

## 3. 状态词汇

| 状态 | 判定 |
| --- | --- |
| **已实现（Implemented）** | 当前生产路径和用户入口存在；至少有对应源码/契约证据。它不自动等于本轮已复验、已人工签字或已发布。 |
| **部分实现（Partial）** | 主流程可用，但 Spec 所列的重要子能力、渲染面或协议出口仍缺失。 |
| **待人工验收（Pending manual acceptance）** | 自动化或取证已完成，但真实 UI、真实 Provider、特定 OS/账号等仍需人工签字。 |
| **已确认未排期（Confirmed, unscheduled）** | 用户已确认需求方向，但尚未立项和实现；不得写成已交付。 |
| **候选（Candidate）** | 仅进入候选池，尚未确认版本、优先级或实现方案。 |
| **归档（Archived）** | 曾存在于历史实现或任务书，当前主干不承载生产实现。 |
| **排除（Excluded）** | 与架构红线冲突或已明确不做。 |

状态采用两条独立轴：**交付状态**（是否有生产路径）与**证据状态**（自动化/真实冒烟/人工/发布是否完成）。禁止用一个绿色标记同时代替两者。

## 4. 编号谱系

“Spec 编程到 P 几”与当前开发线不是同一编号体系：

| 世代 | 编号 | 结论 |
| --- | --- | --- |
| V1 | P0–P19 | 历史任务书到 **P19**；共 224 个编号任务。P19-1～P19-16 为 Designed/未开始。2026-08-17 随 V1 归档。 |
| V2 | S0–S13 | 已于 2026-08-18 收官；交付摘要见 [history.md](../history.md)。 |
| V3 结构线 | R0–R9 | 已归档；旧编号和过程只在 [history](../history.md) / git 历史中检索。 |
| 真实 Desktop 线 | E0–E2 / P1–P4 | 旧阶段已经停止承载活动计划；完成事实见 [history](../history.md)。 |
| Settings 线 | SET-0～SET-7 | SET-0～SET-6g 已实现（过程见 [history](../history.md)）；SET-7 真窗口/人工签字暂停，缺口见 [plan/settings.md](../../plan/settings.md)。当前活动线是 CLN，指针只看 [ROADMAP](../../ROADMAP.md)。 |

因此不会创建 P20 作为当前阶段。本目录使用领域化 Spec 名称；下一产品线的版本名和阶段编号只在用户选择真实产品目标并立项后确定。

## 5. 维护规则

- 用户可见能力、产品边界或状态变化：同批更新 `product.md` / `capabilities.md` 及 ROADMAP。
- wire、schema、配置层级、Secret/Policy 语义变化：先完成 ADR（若需）与 golden，再更新 `contracts.md` / `security.md`。
- Desktop 流程或可访问性变化：以 [gui-design.md](../gui-design.md) 和 [design/README.md](../../design/README.md) 为视觉/交互事实源，同批更新 `desktop.md`。
- **包级 Spec**：写入集改了模块树、对外 API、`pawork-*` 依赖边、feature 门、红线相关行为或测试资产时，同批更新该包 `crates/<pkg>.md`；冲突以源码为准并回写。固定八节结构：职责与边界 · 模块树 · 对外 API 面 · 关键行为与语义 · 依赖与 feature · 红线与不变量 · 测试与 golden 资产 · 相关文档。
- 验证只记录实际执行的命令与证据；缺凭证、缺平台或未签字必须如实标为 pending/fail-closed。
- 小功能直接在 ROADMAP 当前切片写清需求；像 Settings 这样跨协议、Secret、Provider 与 Desktop 的功能才从 [feature-template.md](feature-template.md) 建独立 Feature Spec。
- 发布、License、安装器、三平台与供应链门禁未经用户明确授权，不得从候选改写成已交付。
