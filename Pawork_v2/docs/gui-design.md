# Pawork V2 GUI 设计（最小 Agent 界面先行）

> 本文是 V2 Desktop GUI 的**设计事实源**。S7 必须先锁定本节，再写 `apps/desktop`。后续阶段只按 §5 增量图给已有壳加面，不另起一套信息架构。
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

对照现有 Agent GUI，只吸收「主对话壳」，不复制完整 IDE。

| 参照 | 吸收 | 不吸收 |
| --- | --- | --- |
| [Codex Desktop](https://developers.openai.com/codex) | 左栏会话 / 主栏 Timeline / 底栏 Composer；审批与工具活动嵌在时间线里，而不是另开 IDE | Cloud / Voice / 连接器市场 / 完整 Settings 全家桶 |
| OpenCode Desktop / Web | 流式 token、工具行可见、模型切换就在 Composer 附近 | TUI 键位、JS 插件面板、Web 先做 |
| Claude / Cursor 聊天壳 | 用户/助手气泡 + 工具折叠块；空态就是「问一句」 | 编辑器分屏、多 Agent 编排墙 |
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
| Approval | 时间线内嵌同意 / 拒绝（复用 S3 语义） | 完整 Policy 说明页、信任向导 |
| Changes / Terminal / Settings / Resources / Workflow | 占位或隐藏 | 分别随 S8–S11 增量 |

空态：无会话时主区只有一句提示和 Composer。不以假卡片冒充未实现能力。

---

## 4. 协议与分层（S7 最小切片）

GUI 仍走冻结契约形状（[design.md](design.md) §3.2 GUI 协议）：帧、Command / Query / Event / Snapshot 用 V1 完整字段，S7 **只消费**对话所需子集。

| 切片 | S7 必做 | S10 再补 |
| --- | --- | --- |
| Transport | 本机 Unix socket / Named pipe | remote TLS、memory 测试矩阵 |
| Handshake | 版本协商、单客户端、本机身份 | 多客户端 capability、慢客户端隔离 |
| Query | sessions list/show、snapshot(session+timeline) | artifact 分片、usage、workspace index |
| Command | create/resume session、submit turn、cancel、approval | fork、service、PTY resize |
| Event | message/tool/run/approval/error + `global_sequence` | 全量 Hub 订阅面 |
| Replay | 重连后从 last-ack sequence 补事件；补不齐则重新 snapshot | 正式 Resume/Replay 门禁与 protocol-probe |

进程分层不变：

```text
GPUI view  →  projection（纯 Rust，可从 snapshot+events 重建）
           →  controller（只调 pawork-client）
           →  local transport  →  pawork gui serve  →  app-service
```

`apps/desktop` 不链接 engine / providers / sqlite。`platform` 只允许：窗口、剪贴板、选工作区目录、拉起固定 `pawork` 二进制。

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
- 中文 IME、粘贴多行、滚动追随最新消息是 S7 硬路径。

---

## 7. 插件预留（只留口，不实现）

GUI 与协议现在就要避开「以后为插件推倒重来」：

- Domain 已有 `PluginId`、`ToolCapability::ExternalPlugin`；时间线按普通 tool 事件渲染即可，不识别插件品牌。
- Snapshot / capability 集合预留扩展位；未知 capability 隐藏，不报错、不画灰掉的市场入口。
- 不在 S7–S12 任务里激活 `pawork-api` 的 `plugin` feature、不建 wasm-host / marketplace 页面。
- 决策记录见 [ROADMAP §4](../ROADMAP.md)。

---

## 8. 验收（设计锁定）

- [ ] 对照 §2 写明吸收/不吸收，并与本节信息架构一致。
- [ ] S7 实现范围不超过 §3；后续阶段只按 §5 加面。
- [ ] 四层边界与「不链 Core」可在依赖图上检查。
- [ ] 插件/市场/Hooks/LSP 无产品入口，仅有隐藏扩展点。

