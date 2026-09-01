# ADR-045：Terminal 生命周期 wire 演进（terminal_close + live exit/failure 事件，API 1.3）

- **状态**：Accepted（用户 2026-09-01 确认）
- **日期**：2026-09-01

## 背景

P3 片 1 审计确认 Changes / Resources 面在冻结契约内无缺口，Terminal 面 G1–G3 已在冻结 wire 内修复并真窗口验收。剩余 G4 缺口是协议词汇本身不完整：

- wire 只有 `terminal_create` / `terminal_write` / `terminal_resize` 三个命令与 `TerminalOutput` 一个事件，没有终止 / 关闭终端的命令。
- Host 底层能力已具备但不上 wire：`PtyService::kill`（含进程组终止）存在；`PtyEvent::Exit{code, signal}` 已产生，GuiHost forwarder 收到后只 `break` 不广播；forwarder 的 IO 错误分支同样静默 `break`。
- 后果：Desktop 无法提供真实 Stop；终端自然退出后，在线客户端收不到任何通知，只能靠断连重连后的 `terminal_sessions` 快照 `state` 字段得知 exited；exited 条目也没有清理路径。
- ROADMAP §5 已明令禁止用写入 `exit` 文本冒充正式能力（shell 不一定处于可接受退出命令的状态，且伪造生命周期）。

冻结契约规定 wire 演进必须 ADR、golden 先于实现。本 ADR 只拍板 wire 词汇与版本策略，不改动 schema、不新增 crate 与生产依赖。

## 拟议决策

### D1 — 新增 `terminal_close` 命令（单命令覆盖终止与清理）

- 载荷 `{terminal_session_id}`，capability 沿用 TerminalStreaming，不新增 `GuiCapability` 变体。
- 语义：running 终端先经 `PtyService::kill` 终止进程组；随后从 GuiHost 注册表注销，快照 `terminal_sessions` 节不再出现该条目。对已自然退出的条目，close 即清理 tombstone。
- 幂等边界：未知 id 报 `not_found`（与既有命令一致）；重复 close 同一已注销 id 同样报 `not_found`，不伪造成功。
- 否决 stop / close 拆成两个命令：UI 单按钮按状态换标签即可覆盖，协议词汇保持最小。

### D2 — 新增 `AppEvent::TerminalExited` 事件

- 载荷 `{terminal_session_id, exit_code: Option<i32>, signal: Option<String>, reason}`；`reason` 为 serde 枚举 `TerminalExitReason { Exited, Killed, Failed }`，覆盖自然退出、经 close 终止、forwarder IO 异常断流三种路径（Failed 时 `exit_code` / `signal` 可空）。
- 归属既有 `EventStream::Terminal(id)` 子流，不新增 stream 变体；source 为 Core。
- GuiHost forwarder 的 `PtyEvent::Exit` 与 `Err(_)` 分支由静默 `break` 改为先广播再退出；`terminal_close` 路径在 kill 完成后广播 `Killed`（与 PTY 自身 Exit 去重，同一终端只发一条终态事件）。

### D3 — 版本策略：additive 演进，minor 1.2 → 1.3

- 遵循 ADR-036（V1 归档） minor 只增策略：`API_VERSION` 升为 1.3，`SUPPORTED_API_VERSIONS` 追加，1.0 / 1.1 / 1.2 继续支持。
- 新事件推送按协商 minor 门控：协商 < 1.3 的连接不推送 `TerminalExited`（老客户端 serde 遇未知变体 decode 失败会断流）；该连接上的终端仍可从快照 `state` 获知 exited，行为不劣于现状。
- `terminal_close` 不设版本闸：老客户端词汇内不会发出该命令；registry 登记后命令路由、错误形状与既有命令一致。

### D4 — golden 先行，capability 基线不变

- client golden 15 → 16 帧（新增 `client_command_terminal_close.json`），server golden 16 → 17 帧（新增 `server_event_terminal_exited.json`）；golden 先于实现改动检入。
- 不新增 `GuiCapability` 变体：capabilities 是 handshake 内嵌 Vec，老客户端遇未知枚举值 decode 失败，比 minor bump 更具破坏性；TerminalStreaming 已足以表达该能力族。

### D5 — Desktop 接线边界（后果，不在本 ADR 细化）

- running 终端显示真实 Stop（发 `terminal_close`），exited 终端显示 Close（清理）与 New（既有重建入口）；live `TerminalExited` 即时刷新状态，不再依赖断连重连快照。
- Host 重启后进程内注册表丢失、快照无终端，照旧诚实显示 not started，不伪造恢复。

## 否决支

- **写入 `exit` 文本或控制字符冒充终止**：伪造生命周期，shell 状态不可控，ROADMAP §5 已明令禁止。
- **只加快照状态、不加 live 事件**：即现状——必须断连重连才能发现 exited，不满足 G4 的 live 要求，且违背事件驱动架构引入客户端轮询。
- **新增 `GuiCapability` 变体宣告该能力**：老客户端 handshake decode 未知枚举即失败，破坏性强于 minor bump。
- **复用 `terminal_write` 携带语义化控制参数**：污染字节流语义，write 保持纯数据通道。
- **为 exited 条目新增持久化 tombstone 存储**：终端会话本是进程内状态，Host 重启不恢复是既有诚实边界，不借本 ADR 扩持久化。

## 后果与实施切片

- Accepted 后按序推进：① protocol（新变体 + registry 登记 + API 1.3 + golden 先行，定向测试）；② app / GuiHost（close handler、forwarder 广播、按协商 minor 门控推送，含 kill 与自发 Exit 去重）；③ Desktop（Stop / Close 接线与 live 刷新，定向门禁 + 真窗口验收）。
- 预计写入包 `pawork-protocol`、`pawork-app`、`pawork-desktop`；Spec 同批回写 protocol.md / app.md / desktop.md / flows.md 终端段；ROADMAP §5 对应边界行在实施完成后更新。
- 不改动 schema、import/export、capability 基线与其余冻结契约；不新增 crate 与生产依赖。
