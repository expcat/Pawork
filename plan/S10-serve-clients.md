# S10：服务化与客户端补齐

> 阶段 S10 · 多客户端与正式协议 · 状态：🔵进行中（10a 波 A–B ✅ · 10b ✅ · 收口实现 ✅ · 冒烟大部已齐；剩余 Zed 真实编辑器 + S0–S9 回归）· 依赖：S7（最小 GUI 与本机 `gui serve` 已通）· 规模：大（内分 10a/10b 两波 + 收口）

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

- [x] SDK 例程（Rust）：`pawork-sdk` `spawn_e2e` 对真实 `pawork headless --json-stdio` 握手 / Command / Query / compat；无凭证 `RunStart` 回 `AppResponse::Error`。完整「读-改-跑」+ 远程审批仍待凭证冒烟。
- [x] 两个 gui-client 实例并发连接真实 `gui serve`：事件不串台；kill 一个客户端重连 → Replay。2026-08-17：`protocol-probe --live-two-gui` 对 `s10gui` socket，B 的 Snapshot 看见 A 新建会话，PTY `echo s10-two-gui` 扇出到 B，kill A 后 B 再写，A `connect_with_resume` → `replay 14-61 n=48`。进程内等价仍是 `three-gui-sync` / `snapshot-reconnect`。
- [ ] `pawork acp serve` + 真实 ACP 编辑器：本机无 Zed（`/Applications` 无 Zed.app、无 `zed` CLI），fail-closed 不打勾。替代：stdio `initialize`（`protocolVersion=1`）+ `session/new`（`cwd`=仓库根、`mcpServers=[]`）回到 `sessionId`；依赖 `WorkspaceList.roots`（已补）。Zed `settings.json` 形状：`agent_servers.<id> = { type: "custom", command: "<pawork>", args: ["acp", "serve"] }`。`reject_unknown` 很严，编辑器多传未列入字段会 -32602。
- [x] protocol-probe 自检报告全绿（9 场景：`session-events` / `snapshot-reconnect` / `resume-snapshot-fallback` / `three-gui-sync` / `command-idempotency` / `artifact-chunks` / `version-reject` / `disconnect-keeps-run` / `quota-alert-roundtrip`）。
- [x] 交互式命令经 PTY：`protocol-probe --live-pty` 对真实 serve，`echo s10-pty` 回显 14 字节，重连 `up_to_date`（无新事件故 0 replayed），Snapshot 仍有 terminal id。`--live-two-gui` 在断线后继续写 PTY，Resume Replay 48 条含后续输出。Snapshot **不含** PTY buffer；Inspector Terminal 是滚动文本，不是 VT100。
- [x] `pawork sessions fork` CLI 已接（V1 **没有**该子命令；调 `fork_from_event`，默认 switch；`--no-switch` 保留原 branch）。2026-08-17 两分支真实 resume（`glm-coding`/`glm-4.7`）：`--no-switch` 后 `chat --resume S` 回 `main-branch`，`--resume S --branch <id>` 回 `fork-branch`；库内 `main` 27 条 / fork 20 条。修复：`PersistThenRender` 改为写入 session **active branch**（原先写死 `main`，fork 上 `append_event` 被拒、`--json` 空等）。首轮曾遇 GLM `timeout`（retryable），不阻塞后续两分支。
- [x] `pawork service install/start`：本机 darwin launchd `--apply` 已过（实例 `s10svc`，**不是**默认 `pawork`）。`install` 写 `~/Library/LaunchAgents/pawork.s10svc.plist`（`RunAtLoad`/`KeepAlive`，`ProgramArguments`=`pawork --instance s10svc gui serve`）；`start` 后 `status` `listening=true`，`doctor` `handshake=ok`；`stop --apply` 后未监听、进程退出并删 plist。修复：`--json --apply` 先前只打印 `applied=true` 不执行。Windows Service 本机无法验收，**不打勾**。
- [x] `--json` 新旧格式迁移说明可用：流式 stdout 为 `HeadlessResponse`（`type=event|response|error`），无 hello；见 [docs/headless-json-migration.md](../docs/headless-json-migration.md)。2026-08-17 人工对照：`run` / `chat --prompt` 顶层只有 `type=event|response`，event 带 `envelope.global_sequence`，无顶层 `schema_version`。

## 定向自动化测试

- `cargo test -p pawork-protocol`：帧 golden、版本协商、typegen 输出与检入一致。
- `cargo test -p pawork-app`：幂等、全局 sequence、Hub 不丢不重。
- `cargo test -p pawork-gui-server`：多客户端并发、Replay、慢客户端隔离（V1 测试原样迁移）。
- `cargo test -p pawork-transport`：feature 矩阵（default/memory/remote），rustls 只在 remote（`cargo tree` 断言）。
- `cargo test -p pawork-sdk` / `pawork-channels`（`acp`）：e2e（进程内或子进程驱动）。
- `cargo test -p pawork-exec`：PTY 重连语义。

## 退出标准

- [x] protocol-probe 自检全过；SDK spawn e2e 全绿。两 GUI 对真实 serve 的 Replay 已过。acp 真实编辑器仍待（本机无 Zed；stdio 握手 + `session/new` 已过）。
- [x] `--json` 对齐正式协议、迁移说明留档、unstable 标注移除（只切 `run` / `chat --prompt`；`sessions`/`auth`/`models` 等仍是 CLI 便利输出）。2026-08-17 人工对照：`run`/`chat --prompt` stdout 顶层只有 `type=event|response`，event 带 `envelope.global_sequence`，无顶层 `schema_version` / hello。
- [x] GUI/SDK 依赖面断言全绿（不含 host 实现包）：`pawork-desktop` `desktop_direct_deps_stay_on_client_deny_list`；`cargo tree -p pawork-sdk --edges normal` 无 app/engine/provider/session/sqlite/tools/git。stdout 协议纪律见上条 `--json` 对照。
- [ ] app/cli 正式化完成且 S0–S9 行为回归无变化（既有冒烟脚本复跑，含 S7 Desktop 最小对话）。本波未复跑 S0–S9 历史冒烟。

## GUI 增量

按 [gui-design.md](../docs/gui-design.md) §5：正式 Replay、Fork、Terminal、本机多窗口。不新起壳。

## 为后续阶段预留 / 明确不做

- 预留：channels 的 codex/claude/remote-control feature 可在本阶段后并行补齐。
- 预留：插件/Hooks/LSP 相关协议扩展点保持未知 capability 隐藏（见 ROADMAP §4），本阶段不实现扩展生态。
- 不做：远程 transport 的生产部署形态（remote feature 编译与本机回环测试为限）；不把 Desktop 推迟到 S12 后再从零开始。

## 并行拆分建议

- 10a 波 A（串行，✅）：`pawork-protocol` 收口（关键路径）→ 10a 波 B（并行 ×3，✅）：app 正式化、transport 补齐、sdk。
- 10b（依赖 10a，可并行 ×4，✅）：gui-server 多客户端、Desktop Replay/Fork/Terminal、channels(acp)、exec(pty) + session(lifecycle)。
- 收口（串行，✅ 实现）：cli 六模式 + protocol-probe。冒烟补齐（2026-08-17）大部已齐；剩余 Zed 真实编辑器 + S0–S9 回归。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [../docs/v1-migration-reference.md](../docs/v1-migration-reference.md) §4.1（protocol / app / cli / transport / sdk 映射；M0–M8 正文未落仓）
- [archive/README.md](archive/README.md)（缺失 M0/M4/M5 正文的回退规则）
- [../docs/headless-json-migration.md](../docs/headless-json-migration.md)（`--json` → headless 映射表；CLI 切输出在收口）
