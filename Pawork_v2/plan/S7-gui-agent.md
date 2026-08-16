# S7：最小 Agent GUI

> 阶段 S7 · 先设计、再长出最小桌面 Agent · 状态：🔵进行中（波 0–C 完成，波 D 收口 v3 TaskRail） · 依赖：S1–S5 稳定（会话/事件/工具/审批/取消），S6 建议先行（模型切换有真实通道）· 规模：大 · 设计事实源：[../docs/gui-design.md](../docs/gui-design.md)

## 目标（本阶段结束时用户能做什么）

先对照 Codex app / OpenCode / Cursor Agent 聊天壳锁定 [GUI 设计](../docs/gui-design.md)，再交付一个**最简**独立 GPUI 窗口：连接本机 `pawork gui serve`，列出/新建/恢复会话，流式看对话与工具活动，发送消息、取消当轮、切换已配置模型，并在时间线里完成 S3 审批。不做完整工作站。Git / MCP / 多客户端 / PTY 等随后续阶段按设计 §5 长到同一壳上。

## 涉及包与 V1 资产

| V2 包 / 应用 | 本阶段动作 | V1 来源与方式 |
| --- | --- | --- |
| `docs/gui-design.md` | **波 0 已锁定（2026-08-16）**：信息架构、状态、参照取舍、Timeline 恢复、协议最小切片、后续增量图 | 对照根仓 desktop-gui、V1 连接栈与现有 Agent GUI；不搬 P19 全量 Surface |
| `pawork-protocol`（foundation/protocol） | **最小激活**：gui-protocol 帧形状（ADR-036）+ 对话所需 Command/Query/Event/Snapshot；字段用 V1 完整形状，本阶段只消费子集 | [V1→V2 映射 §4.1](../docs/v1-migration-reference.md) 的 `pawork-protocol` 行；不在本阶段做六合一收口与 `--json` breaking |
| `pawork-transport`（host/transport） | **最小激活**：local（Unix socket / Named pipe）足够单客户端连本机 Host | [V1→V2 映射 §4.1](../docs/v1-migration-reference.md) 的 `pawork-transport` 行；remote/memory 矩阵留 S10 |
| `pawork-gui-server`（host/gui-server） | **最小激活**：单客户端握手、snapshot、事件订阅、审批/取消命令；断线后至少能重新 snapshot | 从 V1 完整 gui-server 裁剪本阶段路径；多客户端/慢客户端隔离/正式 Replay 留 S10 |
| `pawork-client`（clients/gui-client） | 激活：Desktop 唯一接入 SDK（connect/snapshot/subscribe/command） | [V1→V2 映射 §4.1](../docs/v1-migration-reference.md) 的 `pawork-client` 行 |
| `pawork-app` / `pawork-cli` | 增强：`pawork gui serve` 本机单实例；Event Hub 只需扇出到这一个 GUI + 既有 CLI | 非正式化六运行模式（S10） |
| `apps/desktop` | **新建**：GPUI 最小 Agent 壳（`ui/projection/controller/platform`），只链 `pawork-client` | 对照 V1 P19-1 骨架，范围按 [gui-design.md](../docs/gui-design.md) §3 收窄 |

## 关键任务

1. **设计先行（硬前置）**：完成 [gui-design.md](../docs/gui-design.md) §8 勾选；未锁定前不写 `apps/desktop` 业务页。
2. **协议最小切片**：冻结帧形状，只接线 sessions / paged Timeline / turn / cancel / approval / model switch；`SessionGet` 只做同 major optional-field 演进并 bump minor，未知 capability 隐藏。
3. **本机闭环**：`pawork gui serve` 拉起 → Desktop 连接 → snapshot → 流式 turn → 断线重开窗口仍能 resume。
4. **最简 Agent 壳**：侧栏会话 + Timeline + Composer；工具调用是折叠块；审批按钮嵌在时间线。
5. **红线断言**：Desktop 不链接 engine/providers/sqlite；关闭窗口不取消已进入 Core 的 Run。
6. **插件只留口**：不激活 `pawork-api` `plugin` feature，不建市场/Hooks/LSP 页；`PluginId` / `ExternalPlugin` / 未知 capability 按 [gui-design.md](../docs/gui-design.md) §7 预留。

## 真实测试与评估（冒烟清单）

- [x] 设计文档 §8 已勾选，实现未超出 §3 Surface。（v3 TaskRail §3.2 仍欠，见波 D）
- [x] 本机打开 Desktop：新建会话 → 真实模型流式多轮 → 侧栏可 resume。（2026-08-17 `--probe-smoke`：`glm-coding`/`glm-4.7` 流式多轮；重连后 session 仍在）
- [x] 只读或写入工具调用在 Timeline 可见；写入审批可点同意/拒绝，语义与 CLI 一致。（隔离 HOME `trust_workspaces=true` + `--approval-mode ask-for-writes`：`approval=approved`，写入 `target/s7-wave-c-smoke.txt`=`hello-s7c`。未信任工作区写工具会直接 Deny，不弹卡）
- [x] Composer 取消当轮；关窗后 CLI 侧 Run 若已开始则继续（或按 ADR-026 不因 GUI 退出而杀）。（`cancelled=1`；断线重连 `disconnect_survive=running`）
- [ ] 切换已配置 provider/model 后下一轮走新通道。（Host 已能从 ModelList overview 解析跨通道；`deepseek-v4-flash` 不在 GUI 聚合目录——overview 不探测非当前通道运行期模型——本波 `second_turn=skipped`）
- [x] 杀 Desktop 再开：同一 session 时间线连续，不丢已落盘事件。（`--probe-smoke` `persisted=12`；此前 `--probe` 亦见 `sessions=1`）
- [ ] 中文 IME 与多行粘贴可用（开发机平台）。（波 B 已接线 `apps/desktop/src/ui/text_input.rs`；本波未做人工窗口验收）

## 定向自动化测试

- `cargo test -p pawork-protocol`：本阶段消费的帧/snapshot golden（完整形状，未用字段可空）。
- `cargo test -p pawork-client`：connect / snapshot / subscribe / command 进程内或 mock server。
- `cargo test -p pawork-gui-server`：单客户端握手 + 事件顺序 + 关连接不取消 run。
- Desktop：依赖图断言（无 Core 业务 crate）；有则补 projection 单测（snapshot+events 重建时间线）。

## 退出标准

- [x] [gui-design.md](../docs/gui-design.md) 已于 2026-08-16 锁定；后续 S8–S11 只按该文 §5 加面。
- [x] 最小 Agent GUI 真实对话冒烟通过；协议走正式帧而非 `--json`。（2026-08-17 `--probe-smoke`：`first=glm-4.7` / `approval=approved` / `cancelled=1` / `persisted=12` / `disconnect_survive=running`。跨通道切换与 v3 TaskRail / 人工 IME 未齐，阶段保持 🔵）
- [x] Desktop 依赖面干净；插件/市场无产品入口。（`cargo metadata` 直接业务依赖仅 `pawork-client`）
- [x] `--json` 仍标注 unstable（对齐工作在 S10）。

## 为后续阶段预留 / 明确不做

- 预留：Event Hub 多订阅者、多客户端 Replay、PTY、session fork、artifact 分片、远程 transport——契约位在，实现归 [S10](S10-serve-clients.md)。
- 预留：Changes / `@file` / Workflow 面板接口按设计 §5，分别归 S8 / S9 / S11。
- 预留：插件相关 id / capability / 隐藏扩展点（见 ROADMAP §4），**不**实现 wasm-host / marketplace / hooks / lsp。
- 不做：Web UI、TUI、签名安装器全家桶、多窗口远程桌面。

## GUI 增量

本阶段 = 增量图的第 0 行（最小壳本身）。

## 并行拆分建议

- [x] 波 0（串行，2026-08-16）：锁定 `docs/gui-design.md`（对照参照 GUI + 根仓 desktop-gui）。
- [x] 波 A（串行，2026-08-16）：`pawork-protocol` 最小帧 + local transport + `gui serve` 单客户端；六包定向测试全绿（protocol 66 / transport 8 / gui-server 9 / app 48 / cli 8），真实对话冒烟留波 C。
- [x] 波 B（2026-08-16）：`pawork-client` 激活（V1 gui-client 整包平移 + 7 契约测试；artifact 读取 `experimental` 门控）+ `apps/desktop` 壳（gpui =0.2.2，四层结构，Sessions/Timeline/Composer + IME，`--probe` 验证模式）；修复 `gui serve` 丢弃 SessionHandle 的波 A 装配缺陷；真实 socket probe 正向冒烟通过。审批/取消/模型切换与真实对话冒烟留波 C。
- [x] 波 C（串行，2026-08-17）：审批/取消/模型切换接线收口 + `--probe-smoke` 真实冒烟（`glm-4.7` 流式、写入审批、取消、重连时间线、断线不杀 Run）。ModelList 改走 `models_overview`；未信任工作区不弹写审批。跨通道 `deepseek-v4-flash` 与 v3 TaskRail 留波 D。
- 波 D（串行）：按 [gui-design.md](../docs/gui-design.md) §3.2 / [design/README.md](../design/README.md) 落地 v3 TaskRail（日期→项目→Task、GroupingMenuButton、项目定向新建）；1440×1024 对照定稿图；补跨通道模型切换冒烟（目录含 `deepseek-v4-flash` 或 Host 探测非当前通道）。

## 参考

- [../docs/gui-design.md](../docs/gui-design.md) · [../docs/design.md](../docs/design.md) §4 · [../docs/references.md](../docs/references.md)
- [../../docs/features/desktop-gui.md](../../docs/features/desktop-gui.md) · [../../docs/features/gui-connection.md](../../docs/features/gui-connection.md)
- [../docs/v1-migration-reference.md](../docs/v1-migration-reference.md) §4.1 · [archive/README.md](archive/README.md)（缺失 M0–M8 正文的回退规则）
