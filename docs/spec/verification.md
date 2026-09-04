# 验证与证据规格

> 基线日期：2026-08-31。本文定义如何证明 Spec。旧 R/Wave 结论只作历史线索；当前工作区与 [AGENTS.md](../../AGENTS.md) 优先。

## 1. 四个独立结论

任何能力都必须分别回答：

1. **是否实现**：生产调用链和用户入口是否存在？
2. **是否自动验证**：定向测试、golden、fixture 或静态断言是否覆盖核心行为？
3. **是否真实验收**：真实 Provider、真实 OS、真窗口、真实客户端或用户签字是否完成？
4. **是否可发布**：License、安装/升级、供应链、三平台和发布/回滚门禁是否明确并通过？

前一项不能替代后一项。当前 Pawork 有大量“已实现 + 历史定向测试通过”的能力；Desktop 真实核心路径已完成本机 E3 验收，但完整视觉签字、跨平台与发布门禁均未完成。

## 2. 证据等级

| 等级 | 定义 | 可支持的表述 |
| --- | --- | --- |
| E0 | 只有 Spec/计划，无生产实现 | 候选、已确认未排期 |
| E1 | 源码/依赖/生成物证明生产路径存在 | 已实现（代码层） |
| E2 | 定向单测、契约测试、golden、升级 fixture 或静态断言通过 | 自动化验证通过（注明命令/时点） |
| E3 | 隔离实例真实冒烟、真实 Provider/客户端/OS 或真窗口取证通过 | 对应环境验收通过（注明环境） |
| E4 | 用户人工签字或发布任务定义的全量门禁通过 | 人工验收/发布门禁通过 |

E3/E4 证据必须包含日期、环境/版本、输入范围、实际结果和可追溯位置。`/tmp` 截图或日志若未检入，只能作为当次本机证据，不能假装成仓库可复现门禁。

### 2.1 功能测试用模型

日常需要真实 Provider 的功能验证（真窗口 Run、流式输出、工具/审批、live smoke）固定使用：

| Provider | Model | 选择方式 | 禁止 |
| --- | --- | --- | --- |
| `opencode-go` | `glm-5.3-flash` | 当次 Host/CLI `--provider` / `--model` 临时覆盖 | 写入 Global/Workspace 持久默认；连接失败时改用未指定模型或伪造成功终态 |

产品示例默认仍是 `glm-coding` / `glm-5.2`。该约定只约束日常功能验证的默认真实模型，不替代四通道矩阵或发布级 Provider 门禁。证据须写明实际使用的 provider/model；网络/凭证失败按真实终态记录。

## 3. 需求追踪矩阵

| 需求族 | 实现锚 | 自动化锚 | 真实/人工锚 | 当前结论 |
| --- | --- | --- | --- | --- |
| PRD-CORE-01 / ARC-01～04 | `apps/pawork`、app/domain/engine、Cargo 依赖 | domain-only、desktop deny-list、依赖断言 | `pawork`/Desktop 冒烟 | 已实现；发布级红线矩阵未执行。 |
| PRD-CHAT-01 / CAP-CHAT-01 | cli/app/engine/providers | engine mock、provider contract、CLI tests | 四通道真实 chat 矩阵 | 已实现；本轮单通道真实 Run 通过，四通道矩阵未执行。 |
| PRD-SESSION-01 / CAP-SESSION-01 | storage/app/cli | envelope、migration、export/import、projection golden | 真实 resume/fork/compact/import | 已实现；真实 fork/compact 仍有人工登记。 |
| PRD-TOOL-01 / CAP-TOOL-01 | tools/workspace/policy/exec | 八工具与路径/进程回归 | 真实仓库读写/命令冒烟 | 已实现；终局安全复跑未执行。 |
| PRD-SAFE-01 / SEC-* | policy/auth/exec/storage/app | 安全种子、Secret 扫描、Seatbelt golden、ledger 回归 | 平台探针、真实审批/PTY | 已实现；部分平台/人工项仍待验。 |
| PRD-PROVIDER-01 / CAP-PROVIDER-01 | providers/auth/app | adapter/negotiation/OAuth/脱敏测试 | ChatGPT/xAI/GLM/OpenCode 等真实请求 | 已实现；OAuth 自然临期 refresh 与真实 Anthropic/GLM Anthropic 端点待人工。 |
| PRD-SETTINGS-01 / CAP-SETTINGS-01 / SET-* | protocol/app/auth/providers/workspace/desktop（计划） | protocol golden/typegen、Secret 负断言、provider verify、config writer、Desktop/AX（计划） | 四家真实认证/目录、Host 重启、真窗口（计划） | SET-1/SET-2 已实现并通过定向测试（协议 golden/typegen、Secret 负断言、Host 门面主/失败路径）；Desktop UI、四家真实认证/目录与真窗口验收未开始。 |
| PRD-GIT-01 / CAP-GIT-01 | git/app/cli/Desktop Changes | git/checkpoint/diff 定向测试、Desktop projection | 真窗口 Changes、真实 rollback | Core 已实现；Desktop 写操作未实现，横滚人工项待验。 |
| PRD-RESOURCE-01 / CAP-RESOURCE-01 | workspace/tools/app/Desktop | resources/import/MCP contract | 外部配置、MCP stdio、真窗口 Resources | 主流程已实现；部分 GUI 出口为候选。 |
| PRD-CLIENT-01 / CAP-CLIENT-01 | protocol/app/client/cli | frame/headless/ACP golden、registry、probe | Desktop probe、Zed ACP、json-stdio | 已实现；发布级客户端矩阵未执行，probe 有已登记偶发超时。 |
| PRD-DESKTOP-01 / DESK-* | desktop/client/protocol | projection/controller、U0/U1、AX 模型/映射测试 | 真 Host/Desktop、三张阶段目标设计图、AX/IME、用户签字 | 正式构建、项目、消息/文件、Changes、Terminal 与 Session→Workspace 跨 Host 重启已完成本机真窗口验收；完整视觉与跨平台仍需专项取证（VoiceOver 验收已于 2026-09-04 按用户要求移出范围）。 |
| PRD-OPS-01 / CAP-OPS-01 | cli ops/service、app data_dir | 路径/状态/doctor 定向测试 | macOS/Linux/Windows service 与恢复演练 | 入口已实现；无发布级三平台/恢复门禁。 |

## 4. 三类不可推迟的回归

| 类别 | 最低覆盖 |
| --- | --- |
| 安全红线 | 路径越界、symlink、`.git` 写、审批 deny、灾难地板、Sandbox 探测/fallback、Secret 脱敏与外部输入 Secret 拒绝 |
| 持久化与重放 | envelope、SQLite 迁移、append-only、branch lineage、PWB1、checkpoint、export/import、projection、CommandLedger 崩溃/重试 |
| 协议与解析 | GUI frame、版本协商、registry fail-closed、headless JSON、ACP、MCP、配置六层、usage dedup、外部格式解析 |

普通任务只跑写入集的定向命令；触及上述面时，对应关键回归必须同批更新，不能推迟到全量门禁。当前默认命令和单 Cargo 进程纪律见 [AGENTS.md](../../AGENTS.md) §6–§7。

## 5. 当前验收缺口

| 缺口 | 状态 | 完成条件 |
| --- | --- | --- |
| Desktop 真实核心路径 | 已验证（2026-08-31） | 正式脚本构建/启动；真实项目、Provider Run、审批写文件、Git Changes 与 PTY Terminal 均有窗口与外部事实双证据。 |
| Session→Workspace 重启归属 | ✅ P1 片 1 | 同一 Session 在 Host 重启后仍恢复到原 Workspace，Task/Timeline/Changes 一致；Terminal 进程诚实不恢复，workspace/cwd 恢复后新 PTY 仍在同一仓库。 |
| 配置根闭环 | 未执行 | git 根/子目录/非 git 三态与六层配置文档一致，偏差已修或登记。 |
| ChatGPT/xAI 自然临期 OAuth refresh | 待真实账号/临期窗口 | refresh → retry → success 与 `invalid_grant` 清理均有真实证据。 |
| 三类关键回归发布矩阵 | 未立项 | 发布任务明确命令和环境后执行；普通改动仍同批跑受影响的定向种子。 |
| 真实客户端/Provider 矩阵 | 未执行 | 四通道 chat、GUI/Desktop、Zed ACP、headless json-stdio、doctor 实际通过或明确 fail-closed。 |
| 真实 Anthropic、fork/compact、PTY/审批恢复等历史人工项 | 待后续任务 | 实际执行，或由用户明确接受延期并在 ROADMAP/收口摘要登记。 |
| Settings 模型与供应商 | SET-1/SET-2 已实现（定向测试通过） | [settings.md](settings.md) 的 SET-001～011 获得 E1/E2；四家真实认证/目录、Host 重启和真窗口取得 E3；人工视觉/键盘单独签字。 |
| 完整视觉与 Accessibility 签字 | 待专项任务/Settings 收口 | 主工作台与 Settings 状态矩阵、100/125/150%、键盘/AX 由当前真窗口重新取证并人工签字。 |
| 发布级验证 | 未立项 | 发布不在当前 ROADMAP；用户另行授权后先定 License，再定义三平台、供应链、安装/升级/回滚门禁。 |

## 6. 证据记录格式

每个任务收尾至少记录：

```text
Implemented: <生产路径/用户入口，或 none>
Validated: <实际执行命令、测试数/关键结果，或 none + 原因>
Targeted regressions: <覆盖的安全/持久化/协议种子，或 none>
Real-world evidence: <Provider/OS/客户端/窗口/人工签字，或 pending + 原因>
Known gaps: <未覆盖、flake、环境/凭证阻塞与登记位置>
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

禁止只写“tests passed”而不列实际命令；禁止把缺凭证后的 mock 结果写成真实冒烟；禁止把重跑即绿的 flake 隐去。

## 7. Spec 文档自身验证

纯文档任务不运行 Cargo。最低验证为：

- Markdown 相对链接解析到真实文件/目录/anchor（anchor 可按渲染器规则抽查）；
- 文档集导航无孤儿文档；状态词汇一致；
- 候选数量、版本号、命令列表和手工验收项从当前事实源复核；
- `git diff --check` 通过，diff 仅包含授权文档范围；
- 不出现真实 Secret、虚构日志或未执行的“通过”结论。
