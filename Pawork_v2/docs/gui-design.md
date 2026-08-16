# Pawork V2 GUI 设计（最小 Agent 界面先行）

> 本文是 V2 Desktop GUI 的**设计事实源**。S7 波 0 已于 2026-08-16 锁定；在此之后才可写 `apps/desktop`。后续阶段只按 §5 增量图给已有壳加面，不另起一套信息架构。
>
> 关联：[../ROADMAP.md](../ROADMAP.md) · [S7 任务书](../plan/S7-gui-agent.md) · [references.md](references.md) · 根仓 [Desktop GUI](../../docs/features/desktop-gui.md) · [GUI 连接](../../docs/features/gui-connection.md) · [ADR-035](../../docs/adr/ADR-035-gpui-desktop.md)

---

## 1. 目标与非目标

**目标**：先交付一个能真实驱动 `pawork` 的最小 Agent 窗口——选会话、看时间线、发消息、取消当轮、切换已配置模型。界面从最简模型长出，而不是按 V1 Phase 19 一次性铺满 Settings / Diff / Terminal / Workflow。

**非目标（本设计明确不做）**：

- 不嵌入 Core，不直连 Provider / SQLite / 工具 / Keychain。
- 不做 TUI，不做 WebView / JS 壳。
- 不实现插件市场、Hooks 管理、WASM 安装器（整族待设计，见 [ROADMAP §4](../ROADMAP.md)）。
- 不在 S7 做完整多窗口远程桌面、签名安装器、主题生态。

架构红线沿用根仓：独立 GPUI 进程，只经 GUI Connection Protocol 连接 CLI；关闭窗口不取消已进入 Core 的 Run。

---

## 2. 参照与取舍

对照现有 Agent GUI，只吸收可验证的「主对话壳」行为，不复制完整 IDE，也不做像素级克隆。下表按 2026-08-16 的官方公开资料核对：

| 参照 | 吸收 | 不吸收 |
| --- | --- | --- |
| [Codex app](https://openai.com/index/introducing-the-codex-app/) | 项目内组织 thread、thread 内持续查看 Agent 进度与结果，桌面与 CLI 会话连续 | 多 Agent command center、Worktree 编排、Skills / Automations、Cloud / Remote 全家桶 |
| [OpenCode](https://opencode.ai/)（[models](https://opencode.ai/v2/docs/models) / [tools](https://dev.opencode.ai/docs/tools/)） | 会话继续、当前会话模型切换、工具详情与 permission 状态可见 | TUI 键位、WebView/JS 插件面板、并行多会话工作站 |
| [Cursor Agent / MCP](https://docs.cursor.com/context/model-context-protocol) | 工具请求、参数与结果在对话内可展开；需要时就地审批 | 编辑器分屏、代码导航、IDE Settings 与 MCP 管理面 |
| V1 [desktop-gui.md](../../docs/features/desktop-gui.md) | `ui / projection / controller / platform` 四层；Snapshot + `global_sequence` Replay | P19-1～P19-16 一次铺开 11 个 Surface |

S7 的产品形状：**一个本地 Coding Agent 聊天窗**，不是工作站。Git / MCP / 多客户端 / Plan 等能力随后续阶段长到同一壳上。

---

## 3. 最小信息架构（S7 只做这些）

```text
┌────────────┬──────────────────────────────────────┐
│ Sessions   │  Timeline                            │
│  · 当前    │   user / assistant / tool / error    │
│  · 历史    │                                      │
│  · 新建    │                                      │
│            ├──────────────────────────────────────┤
│ Workspace  │  Composer                            │
│  · 路径    │   输入 · 发送 · 取消 · 模型          │
│  · 连接    │                                      │
└────────────┴──────────────────────────────────────┘
```

| Surface | S7 范围 | 明确延后 |
| --- | --- | --- |
| Connection / Shell | 发现或拉起本机 `pawork gui serve`、连接状态、断线提示 | 多 instance、远程 Host、updater |
| Sessions | 列表 / 新建 / 打开 / resume | Fork / 分支树 |
| Timeline | user、assistant 流式、tool 调用起止、错误 | citation、Artifact 分页、thinking 精细折叠（有事件就只读展示，不做专门产品页） |
| Composer | 纯文本发送、取消当轮、下拉已配置 model/provider | `@file`、附件、profile（S9 再长） |
| Approval | 时间线内嵌仅本次允许 / 本轮运行允许 / 拒绝（复用 S3 语义） | 完整 Policy 说明页、信任向导 |
| Changes / Terminal / Settings / Resources / Workflow | 占位或隐藏 | 分别随 S8–S11 增量 |

空态：无会话时主区只有一句提示和 Composer。不以假卡片冒充未实现能力。

### 3.1 主路径与状态

S7 的唯一主路径是：启动 Desktop → 连接本机 Host → 恢复 Snapshot 与当前会话历史 → 新建或打开会话 → 发送一轮 → 查看流式文本 / 工具活动 → 必要时审批或取消 → 在终态继续下一轮。一个窗口同一时刻只操作一个 active session；其它会话只在侧栏显示摘要与运行状态。

| 状态 | 必须显示 | 可用动作 |
| --- | --- | --- |
| 首次连接 / 无可用 Host | 正在连接；失败后显示可重试的原因 | 重试或退出；不显示业务假数据 |
| 已连接、当前会话空闲 | 权威 Timeline、当前 model/provider | 发送、新建/打开会话、切换模型 |
| 当前 Run 运行中 | assistant 流式片段；tool `pending/running/succeeded/failed/cancelled`；明确的运行中状态 | 取消当轮；发送与模型切换禁用 |
| 等待审批 | 内嵌工具名、目标、风险与短摘要 | `仅本次允许` / `本轮运行允许` / `拒绝`；仍可取消当轮；无默认允许 |
| Run 已完成 / 失败 / 取消 | 终态留在 Timeline，不删除已产生内容 | 继续下一轮；失败信息可复制 |
| 重连中 | 保留内存 projection 但整体标为 stale / 只读 | 禁用发送、模型切换与审批；不得把本地 pending 当权威结果 |
| 协议不兼容 | 明确显示客户端与 Host 版本不兼容 | 只允许退出/重试；不得降级走 `--json` |

模型选择器只列 Host 返回的已配置条目；切换只影响下一轮，并以 Core 的确认事件覆盖本地 pending。审批按钮严格映射 S3 的 `ApproveOnce / ApproveForRun / Deny`；关闭审批卡片不能等价于允许。

---

## 4. 协议与分层（S7 最小切片）

GUI 仍走冻结契约形状（[design.md](design.md) §3.2 GUI 协议）：帧、Command / Query / Event / Snapshot 用 V1 完整字段，S7 **只消费**对话所需子集。

| 切片 | S7 必做 | S10 再补 |
| --- | --- | --- |
| Transport | 本机 Unix socket / Named pipe | remote TLS、memory 测试矩阵 |
| Handshake | 版本协商、单客户端、本机身份 | 多客户端 capability、慢客户端隔离 |
| Query | workspace/model 列表；`SessionGet` 的追加式分页 Timeline；Snapshot 基线提供 SessionTree / ActiveRuns / PendingToolApprovals / ProviderStatus | artifact 分片、usage、workspace index |
| Command | create/resume session、submit turn、cancel、approval | fork、service、PTY resize |
| Event | message/tool/run/approval/error + `global_sequence` | 全量 Hub 订阅面 |
| Replay | 重连后从 last-ack `global_sequence` 补事件；补不齐则重新 Snapshot 并重取当前会话 Timeline | 多客户端一致性、慢客户端隔离与 protocol-probe 全矩阵 |

### 4.1 Timeline 恢复决策

V1 Snapshot 只有会话树、活动 Run、待审批与 Provider 等状态，**没有历史 Timeline 内容**；S7 不得把不存在的 `snapshot(session+timeline)` 当作可迁资产。波 A 按以下方式补齐，且不新增同 major 的枚举变体：

1. `SessionGet` 保留现有变体，在协商后的新 minor 中追加可选请求字段 `timeline_after_sequence` / `timeline_limit`。
2. `AppResponse::Data` 保留现有信封与 Session 数据形状，只追加可选 `timeline_page`：`items`、`next_sequence`、`head_sequence`、`complete`。`items` 是 gui-server 从已持久化 Agent 事件投影出的 presentation-safe 条目，不暴露 SQLite、Secret 或 Protected Blob 明文。
3. 首连走 `snapshot(N) → subscribe(after=N) → 分页取 active session Timeline`。历史页加载期间 controller 暂存 live events；到达 `head_sequence` 后按 event id / sequence 去重，再交给 projection。
4. 重连得到连续 Replay 时直接续接；得到 `SnapshotRequired` 时丢弃 stale 权威标记，以新 Snapshot 替换基线并重新分页当前会话。Desktop 重启同样从持久化 Timeline 重建，不能依赖 GUI 本地业务缓存。

上述字段属于 ADR-036 允许的 optional field 演进：波 A 必须 bump minor 并先锁 golden；旧 minor 不支持 Timeline 页时，S7 Desktop 明确报版本不兼容，不静默显示不完整历史。

### 4.2 进程与依赖边界

进程分层不变：

```text
GPUI view  →  projection（纯 Rust，可从 snapshot+events 重建）
           →  controller（只调 pawork-client）
           →  local transport  →  pawork gui serve  →  app-service
```

`apps/desktop` 的直接业务依赖只允许 `pawork-client`；GPUI 与纯 UI 辅助库不算业务依赖。它不得直接依赖 app / engine / providers / session / sqlite / tools / git；该 deny list 在波 B/C 用 `cargo metadata` 实测。`projection` 不导入 GPUI 或 OS API，`controller` 只调 client，`platform` 只允许窗口、剪贴板、选工作区目录与拉起固定 `pawork` 二进制。GPUI 依赖在创建 crate 时锁定精确 revision。

S1 起的 `--json` 仍标 **unstable**。S7 的 GUI **不**把 `--json` 当长期协议；最小 gui-protocol 激活后，Desktop 只走正式帧。S10 再把 `--json` 对齐 headless，并补 SDK / ACP / 多客户端。

---

## 5. 随阶段增量（同一窗口，不换壳）

| 阶段 | Core 新能力 | GUI 增量（只加面，不改四层） |
| --- | --- | --- |
| S7 | 最小 `gui serve` + 单客户端协议 | 本设计的 Agent 壳：会话 / Timeline / Composer / 取消 / 模型 / 审批按钮 |
| S8 | diff / checkpoint / rollback | Changes：会话 diff、回滚、审批 hunk 预览 |
| S9 | MCP / AGENTS.md / `@file` | Composer `@` 补全；Resources 只读：MCP 列表、已加载规则 |
| S10 | 正式协议 / 多客户端 / Fork / PTY / service | 重连 Replay、Fork、Terminal、多窗口本地 |
| S11 | Plan / 后台任务 / usage / 多 Agent | Workflow 与用量条；子 Agent 时间线分组 |
| S12 | 发布硬化 | 三平台窗口/输入/打包证据；不是新功能页 |
| 待决策 | WASM 插件 / Hooks / LSP / 市场 | 预留 Resources 空位与协议扩展点，**不画假市场页** |

后续阶段任务书必须带一行「GUI 增量」；没有对应投影/命令就不做按钮。

---

## 6. 视觉与交互原则

- 原生桌面密度：侧栏窄、主栏宽、Composer 固定在底。不要仪表盘卡片墙。
- 工具调用是 Timeline 里的折叠块（名字、状态、短摘要），不是单独 IDE 面板。
- 流式输出按 token/事件追加；取消只取消当轮，历史保留。
- 审批 fail-closed：无用户动作不得当默认允许。
- 主题跟随系统 light/dark；S7 不做主题市场。
- `Enter` 仅在 IME 未组合时发送，`Shift+Enter` 换行；多行粘贴保持原文。
- Timeline 只在用户位于底部时追随流式输出；用户向上阅读后不得抢滚动位置。
- 连接、Run、tool 与审批状态必须有文本/图标语义，不能只靠颜色；主路径可全键盘操作。

---

## 7. 插件预留（只留口，不实现）

GUI 与协议现在就要避开「以后为插件推倒重来」：

- Domain 已有 `PluginId`、`ToolCapability::ExternalPlugin`；时间线按普通 tool 事件渲染即可，不识别插件品牌。
- Snapshot / capability 集合预留扩展位；未知 capability 隐藏，不报错、不画灰掉的市场入口。
- 不在 S7–S12 任务里激活 `pawork-api` 的 `plugin` feature、不建 wasm-host / marketplace 页面。
- 决策记录见 [ROADMAP §4](../ROADMAP.md)。

---

## 8. 验收（设计锁定）

> 2026-08-16 · S7 波 0 已锁定。这里验收的是设计与可检查规则；真实依赖图、平台交互与视觉证据随波 B/C 执行。

- [x] 对照 §2 写明吸收/不吸收，并与本节信息架构一致。
- [x] S7 实现范围不超过 §3；后续阶段只按 §5 加面。
- [x] 四层边界与「不链 Core」已形成可执行 deny list；实测留波 B/C。
- [x] 插件/市场/Hooks/LSP 无产品入口，仅有隐藏扩展点。
