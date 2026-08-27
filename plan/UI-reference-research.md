# Agent UI 参照调研

> 快照日期：2026-08-25
> 用途：为 R1–R8 提供可验证的交互与测试方法；Pawork 的视觉外观仍只以 [design](../design/README.md) 和 [gui-design](../docs/gui-design.md) 为准。本文不授权复制竞品品牌、未接入能力或布局。

## 1. 研究原则

- 只采用当前官方产品文档、官方源码/测试文档或平台文档；营销截图只作为发现线索。
- 明确区分“官方已说明的行为”和“对 Pawork 的推断”。
- 竞品共同模式可进入任务书；单一产品专属能力需证明适合 Pawork 架构与 capability。
- Cloud/Remote/Worktree/IDE 能力不存在时不展示假入口；不使用不可验证完成百分比。

## 2. Codex 当前模式

| 官方行为 | Pawork 可吸收方式 | 资料 |
| --- | --- | --- |
| project sidebar + thread list + review pane；pin、rename、archive、search、unread、next needs attention | TaskRail 直接表达 Running/Needs input/Ready/Blocked/Unread，Inspector 承载 Changes/Terminal | [Desktop app](https://learn.chatgpt.com/docs/app) · [Projects](https://learn.chatgpt.com/docs/projects) · [Commands](https://learn.chatgpt.com/docs/reference/commands) |
| Composer 以单一主输入承载 `@` 文件、`/` 命令、model、附件、上一 prompt 与 queued follow-up | context/model/workspace 作为可见 controls/chips；只有 Host 支持才开放 steer/queue | [Modes](https://learn.chatgpt.com/docs/environments/modes) · [IDE](https://learn.chatgpt.com/docs/codex/ide) · [Changelog](https://learn.chatgpt.com/docs/changelog) |
| 审批有动作、风险、Approved/Denied/Aborted/Timed out 等状态，支持键盘接受/拒绝 | approval 是可持久化 typed event，同时把 task 提升为 Needs input | [Permissions](https://learn.chatgpt.com/docs/permission-modes) · [Approvals](https://learn.chatgpt.com/docs/agent-approvals-security) |
| chat-scoped terminal；review pane 区分 working tree scopes，并提供 file/hunk 操作 | Inspector 的 Changes/Terminal 需明确 workspace/session/scope；协议未实现时隐藏 stage/hunk | [Terminal](https://learn.chatgpt.com/docs/integrated-terminal) · [Code review](https://learn.chatgpt.com/docs/code-review) |
| Goal row、Activity、notifications 使用离散状态；运行中可 pause/resume/follow-up | 显示当前步骤和真实状态，不造百分比；Activity 汇总跨 task 注意事项 | [Long-running work](https://learn.chatgpt.com/docs/long-running-work) · [Notifications](https://learn.chatgpt.com/docs/notifications) |
| command menu、panel toggles、task cycling、next attention、find、font zoom 和 approval keys 可搜索/重映射 | 复制“可发现、焦点相关”的机制，不照搬某组 macOS 按键 | [Commands](https://learn.chatgpt.com/docs/reference/commands) · [IDE commands](https://learn.chatgpt.com/docs/developer-commands?surface=ide) |
| Local/Worktree/Cloud/Handoff 明确区分运行位置与持续执行 | Pawork 分开表达 persisted、connected、executing、blocked；没有基础设施就不展示后台承诺 | [Worktrees](https://learn.chatgpt.com/docs/environments/git-worktrees) · [Remote](https://learn.chatgpt.com/docs/remote) |

Codex 官方资料未给出桌面 screen-reader announcement、AX role、焦点恢复、high contrast、reduced motion 或 WCAG 完整契约，因此 Pawork 必须自行建立 R7/R8 Accessibility 门禁。

## 3. Zed、Cursor、Claude 与 VS Code Agent

| 共同模式 | Pawork 采用 | 官方资料 |
| --- | --- | --- |
| sessions / chat / Changes-Files 或项目-Git 三域分工 | 保留 `TaskRail → Timeline + Composer → Inspector`，不引入可任意拖拽的 IDE pane 系统 | [VS Code Agents Window](https://code.visualstudio.com/docs/agents/run/agents-window) · [Claude Desktop](https://code.claude.com/docs/en/desktop) · [Zed parallel agents](https://zed.dev/docs/ai/parallel-agents) |
| 会话按 workspace/project 组织，并在列表直接显示状态与变更摘要 | Timeline/Projects 都以 project-scoped task 为导航单位，状态只来自 Host projection | [VS Code sessions](https://code.visualstudio.com/docs/agents/run/sessions/manage-sessions) · [Zed parallel agents](https://zed.dev/docs/ai/parallel-agents) |
| 对话保留消息、tool/checkpoint 与运行进度，细节按需展开 | Timeline 默认显示 tool 名、状态、短摘要；参数/输出按需展开并保持事件顺序与脱敏 | [Zed Agent Panel](https://zed.dev/docs/ai/agent-panel) · [Cursor Agent](https://prod.cursor.com/docs/agent/overview) |
| Diff/Terminal 跟随 active session，作为证据面而非独立产品 | Inspector 明示 workspace/session/working tree/last turn scope；不在 Desktop 直连 Git/PTY | [VS Code tools](https://code.visualstudio.com/docs/agents/run/tools) · [Zed terminal threads](https://zed.dev/docs/ai/terminal-threads) |
| 工具可用性、执行审批、事后审查分层 | Resources/capability、ApprovalCard、Changes 分别表达，不提供全局 bypass/auto-approve | [Zed profiles](https://zed.dev/docs/ai/agent-profiles) · [VS Code approvals](https://code.visualstudio.com/docs/agents/run/approvals) |
| reconnect/replay、fork、checkpoint/rollback 是不同恢复语义 | 入口明确影响范围，不把 checkpoint 当 Git 或完整“时光机” | [Claude checkpointing](https://code.claude.com/docs/en/checkpointing) · [VS Code sessions](https://code.visualstudio.com/docs/agents/run/sessions/manage-sessions) |
| 后台/并行工作需要可扫读状态、中断与安全的中途反馈 | 当前只展示 Host 已有状态；queue/steer/subagent control 等到协议有因果与持久化语义后再开放 | [Cursor background agents](https://docs.cursor.com/background-agent) · [VS Code Agent Host](https://code.visualstudio.com/docs/agents/concepts/agent-host) |

产品差异也很明确：Zed 偏 IDE thread/terminal，Cursor 强调云端 background agent，Claude 偏可重排本地工作台与 checkpoint，VS Code 以 Agent Host 和多客户端 session 为中心。Pawork 只吸收交互层次和状态表达，不移入它们的运行时、云执行、Git/worktree 或权限模型。

## 4. UI 测试方法调研

推荐混合金字塔，不押注单一工具：

| 层 | 候选工具与证据 | 关键限制 |
| --- | --- | --- |
| U0 | Rust projection/controller 测试、协议 golden | 不证明像素、窗口命中或系统输入 |
| U1 | GPUI `TestAppContext`，覆盖 action/focus/key/mouse/scroll/resize/layout | 当前 `gpui = 0.2.2` 尚未证明具有 Zed main 的 AccessKit/完整视觉捕获能力 |
| U2 | 外部 macOS XCUITest/XCTest 或等价 AX 驱动，启动真 Host/.app | 必须使用稳定 AX identifier 和状态等待；外部测试驱动不得进入生产构建 |
| U3 | 真窗口截图 + ImageMagick 指标、AX audit/VoiceOver、真实 IME、性能和用户签字 | 分区 SSIM 不能替代结构门禁或人工 overlay |

R1 的硬闸门是验证 role/label/value/action/identifier 的真实 macOS 映射。若 AX 树只见 Window/traffic lights，就必须评估精确 GPUI revision、有限 backport 或等价 AX bridge；不能用脆弱坐标点击代替后宣称“全功能”。Zed 当前源码可作为能力方向参考，但不能把 main 上的 `VisualTestAppContext`/AccessKit 写成 Pawork 0.2.2 已具备的事实。[GPUI TestAppContext](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/app/test_context.rs) · [GPUI VisualTestContext](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/app/visual_test_context.rs) · [Apple XCUIElement](https://developer.apple.com/documentation/xcuiautomation/xcuielement) · [AppKit Accessibility](https://developer.apple.com/documentation/appkit/accessibility-for-appkit) · [ImageMagick compare](https://imagemagick.org/compare/)

## 5. 跨产品可复用结论

- **稳定导航 + 当前工作 + 证据面**：左侧组织 task/attention，中间保持 transcript/composer 主焦点，右侧承载 Changes/Terminal/资源证据。
- **状态贯穿**：同一 Run/approval/connection 状态在 TaskRail、Header、Timeline 与 Activity 使用同一语义，不能各面自己推断。
- **主输入克制**：一个 Composer 配少量 context controls；不把模式、权限和附件拆成抢焦点的大表单。
- **工具与审批留在时间线**：动作、目标、结果与风险可回放；需要关注时再在全局导航升级状态。
- **证据 scope 明示**：working tree、last turn、workspace、session 和 terminal 归属都必须可见。
- **操作可恢复**：task/panel/menu/connection 切换后保留合理 selection、scroll、draft 与 focus。
- **离散进度**：使用 running/waiting/needs input/ready/failed 和当前步骤，不显示无法证明的百分比。
- **能力诚实**：外部产品拥有但 Pawork 未接入的 Remote、Cloud、Handoff、stage/hunk、插件市场等不成为装饰性入口。

## 6. 对 R1–R8 的方法映射

| 阶段 | 采用的方法 |
| --- | --- |
| R1 | 用三栏组件树、状态 manifest、确定性 fixture 与语义 UI driver 冻结合同 |
| R2 | 先固定窗口 chrome/三栏几何/全局 token，再做局部 polish |
| R3 | 让 task 状态、attention、grouping、selection 与重连恢复先可观察 |
| R4 | transcript 中统一消息、tool、approval、error 与 completion typed events |
| R5 | 单一 Composer + context controls；输入、send/cancel 与能力禁用都可测试 |
| R6 | Changes/Terminal/Resources 明确 scope；Activity 只做权威摘要与恢复入口 |
| R7 | command/focus/keyboard/AX/响应式作为跨组件合同，不靠后期人工补点 |
| R8 | 以组件 × 状态 × 输入方式矩阵覆盖完整操作，并保留失败证据包与三图视觉门禁 |

## 7. 明确不照搬

- Chat/Work/Codex 产品切换、模型营销、用量图表、Pets、voice、browser 与 plugin discovery。
- 没有运行基础设施支撑的 Cloud、Remote、Handoff、后台继续和虚假通知。
- Full access / Always approve 的低风险普通 toggle 表达。
- 非 Git workspace 仍强制展示完整 Git review pane。
- 某个平台的具体快捷键、不可验证完成百分比、竞品文案和品牌视觉。

## 8. 待实现阶段验证的假设

- GPUI 当前锁定版本能否提供稳定语义 identifier、完整 AX tree 与真实输入/截图能力，需要 R1 spike；调研已确认不能从 Zed main 的能力反推 0.2.2。
- macOS 主门禁可证明当前设计还原，但不能自然推出 Linux/Windows 的窗口行为；跨平台证据留到 R10。发布级三平台矩阵仍属 ROADMAP §5 候选。
- 外部 Agent 产品的公开资料不能证明 Pawork 的 VoiceOver、IME、重连或后台生命周期正确，必须由自身 suite 证明。
