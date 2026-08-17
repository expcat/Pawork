# S10：服务化与客户端补齐

> 阶段 S10 · 多客户端与正式协议 · 状态：🔵进行中（10a 波 A ✅）· 依赖：S7（最小 GUI 与本机 `gui serve` 已通）· 规模：大（内分 10a/10b 两波）

## 目标（本阶段结束时用户能做什么）

在 S7 已能本机单窗口对话的基础上，把 `pawork` 补齐为可被外部消费的 Agent 服务：`pawork headless --json-stdio` 供 SDK 编程驱动；`pawork gui serve` 从单客户端升级为多 GUI 并发、断线 Replay、慢客户端隔离；`pawork acp serve` 接入 ACP 编辑器；`pawork service install/start/stop` 补齐六运行模式；会话 Fork UX 上线；S1 起的 `--json` 在此对齐正式 headless 协议（唯一一次计划内 breaking）；PTY 随 GUI Terminal 消费者激活。Desktop **不再从零启动**，只按 [gui-design.md](../docs/gui-design.md) §5 加 Replay / Fork / Terminal。

## 涉及包与 V1 资产

10a（协议与 headless；S7 已有最小帧，本阶段六合一收口）：

| V2 包 | 动作 | V1 来源 |
| --- | --- | --- |
| `pawork-protocol`（foundation/protocol） | **增强/收口**：六合一（core-api 拆六模块 + 在 S7 最小帧上补齐 golden + client-adapter-api + client-auth + headless-json + schema-typegen）；「协议版本 × 包版本」映射表 | [V1→V2 映射 §4.1](../docs/v1-migration-reference.md) 的 `pawork-protocol` 行（archive/M0 正文未落仓） |
| `pawork-app`（正式化） | 增强：V1 `app-service` 语义对齐的门面整理（aggregate/router/approval/idempotency/rate_limit）+ `subscription-hub` 并入为 Event Hub；命令幂等、事件全局 sequence、不丢不重 | [archive/M4](archive/README.md) pawork-app 节 |
| `pawork-cli`（正式化） | 增强：六运行模式齐全（对齐 V1：`run` 一次性 / `chat` 交互（V1 `shell`）/ `gui serve` 常驻（V1 `serve`，`--instance` 多实例）/ `headless --json-stdio` / `acp serve` / `service install|start|stop`）；运维子命令（`status`/`watch`/`shutdown`/`doctor`）随 serve 模式激活；`session fork` 子命令；stdout 协议纪律全面执行 | [archive/M4](archive/README.md) pawork-cli 节 |
| `pawork-transport`（host/transport） | 激活：trait + local（默认）/memory/remote 三 feature；rustls 锁 remote；transport-remote-placeholder 不迁（Remote trait 上移） | [archive/M5](archive/README.md) pawork-transport 节 |
| `pawork-sdk`（clients/sdk） | 激活：agent-sdk 迁移（连接 headless --json-stdio、起 Session、流式消费）；ide-host-adapter 为 `ide` feature | [archive/M5](archive/README.md) pawork-sdk 节 |

10b（多客户端、通道与 Desktop 增量）：

| V2 包 | 动作 | V1 来源 |
| --- | --- | --- |
| `pawork-gui-server`（host/gui-server） | **增强**：在 S7 单客户端上合并 connection-manager + snapshot-service；多客户端并发、断线 Replay、慢客户端隔离 | [archive/M5](archive/README.md) pawork-gui-server 节 |
| `pawork-client`（clients/gui-client） | **增强**：补齐多客户端/Resume API；Desktop 已在 S7 消费本包 | [archive/M5](archive/README.md) pawork-client 节 |
| `pawork-channels`（host/channels） | 激活：`acp` feature 优先接线；`codex`/`claude`/`remote-control` 三 feature 随后按需（可移至本阶段后并行批），共享审计/gate 模式 | [archive/M5](archive/README.md) pawork-channels 节 |
| `pawork-exec` | 增强：`pty-service` 并入（S4 明确推迟项）——交互式命令/GUI 终端消费者出现，PTY 断线重连语义测试随迁 | [archive/M1](archive/README.md) pawork-exec 节 |
| `pawork-session` | 增强：lifecycle（lease/integrity/sequence gap）激活——多客户端并发访问会话的一致性保障；分支/Fork 消费面（`session_branches`/分支树/parent 校验 S1 起在库，此处补 fork 操作与投影） | [archive/M3](archive/README.md) 关键动作 1 |
| `protocol-probe`（apps/protocol-probe） | 激活：协议契约自检 binary（握手/版本协商/command/query/event/Resume-Replay 场景集） | [archive/M5](archive/README.md) protocol-probe 节 |

## 关键任务

1. **协议收口**（10a 波 A ✅）：在 S7 最小帧上补齐 golden 与 typegen `.d.ts`；`--json` → headless 映射表 + 迁移说明见 [docs/headless-json-migration.md](../docs/headless-json-migration.md)（unstable 仍在，CLI 切输出留给收口）。
2. **Event Hub**：engine 事件流 → Hub → 多订阅者（CLI 渲染 / gui-server / channels）扇出，不丢不重、慢消费者隔离。
3. **GUI 红线**：GUI 只经 GUI Connection Protocol 连接 CLI，不直接访问 Provider/数据库/工具（依赖面断言）。
4. **acp 真实接入**：用一个支持 ACP 的真实编辑器完成一次 Agent 交互（真实兼容性评估，而非仅自测）。
5. **协议自检**：protocol-probe 全场景过 = 本阶段端到端验收入口。

## 真实测试与评估（冒烟清单）

- [ ] SDK 例程（Rust）：headless 驱动一个完整工具任务（S4 的「读-改-跑」），流式消费事件、发送审批决议。
- [ ] 两个 gui-client 实例并发连接 `gui serve`：事件不串台；kill 一个客户端重连 → Replay 补齐缺失事件、终态一致。
- [ ] `pawork acp serve` + 真实 ACP 编辑器：发起任务、看到流式输出与工具活动、审批往返。
- [ ] protocol-probe 自检报告全绿。
- [ ] 交互式命令（如需要 TTY 的 CLI 工具）经 PTY 执行正常；断线重连后输出续接。
- [ ] `pawork session fork`：在历史某条消息处分叉出新分支续聊，原分支不受影响；两分支各自 resume 正确。
- [ ] `pawork service install/start`（Windows Service）：开机常驻、GUI 客户端可连；`stop/uninstall` 干净。
- [ ] `--json` 新旧格式迁移说明可用（老脚本按说明改造后工作）。

## 定向自动化测试

- `cargo test -p pawork-protocol`：帧 golden、版本协商、typegen 输出与检入一致。
- `cargo test -p pawork-app`：幂等、全局 sequence、Hub 不丢不重。
- `cargo test -p pawork-gui-server`：多客户端并发、Replay、慢客户端隔离（V1 测试原样迁移）。
- `cargo test -p pawork-transport`：feature 矩阵（default/memory/remote），rustls 只在 remote（`cargo tree` 断言）。
- `cargo test -p pawork-sdk` / `pawork-channels`（`acp`）：e2e（进程内或子进程驱动）。
- `cargo test -p pawork-exec`：PTY 重连语义。

## 退出标准

- [ ] protocol-probe 自检全过；SDK e2e、acp 真实编辑器接入、GUI 并发 + Replay 冒烟通过。
- [ ] `--json` 对齐正式协议、迁移说明留档、unstable 标注移除。
- [ ] GUI/SDK 依赖面断言全绿（不含 host 实现包）；stdout 协议纪律断言。
- [ ] app/cli 正式化完成且 S0–S9 行为回归无变化（既有冒烟脚本复跑，含 S7 Desktop 最小对话）。

## GUI 增量

按 [gui-design.md](../docs/gui-design.md) §5：正式 Replay、Fork、Terminal、本机多窗口。不新起壳。

## 为后续阶段预留 / 明确不做

- 预留：channels 的 codex/claude/remote-control feature 可在本阶段后并行补齐。
- 预留：插件/Hooks/LSP 相关协议扩展点保持未知 capability 隐藏（见 ROADMAP §4），本阶段不实现扩展生态。
- 不做：远程 transport 的生产部署形态（remote feature 编译与本机回环测试为限）；不把 Desktop 推迟到 S12 后再从零开始。

## 并行拆分建议

- 10a 波 A（串行）：`pawork-protocol` 收口（关键路径）→ 10a 波 B（并行 ×3）：app 正式化、transport 补齐、sdk。
- 10b（依赖 10a，可并行 ×4）：gui-server 多客户端、Desktop Replay/Fork/Terminal、channels(acp)、exec(pty) + session(lifecycle)。
- 收口（串行）：cli 六模式 + protocol-probe + 全部冒烟。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [../docs/v1-migration-reference.md](../docs/v1-migration-reference.md) §4.1（protocol / app / cli / transport / sdk 映射；M0–M8 正文未落仓）
- [archive/README.md](archive/README.md)（缺失 M0/M4/M5 正文的回退规则）
- [../docs/headless-json-migration.md](../docs/headless-json-migration.md)（`--json` → headless 映射表；CLI 切输出在收口）
