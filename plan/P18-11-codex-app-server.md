# P18-11：Codex App-Server Adapter

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢Adapter + versioned goldens 已落地 · 交付成熟度：WorkspaceMember（根 workspace member；生产 CLI 入口仍待宿主装配） · 依赖：P18-10、P15-2、P15-5、P15-8、P12-1

**最终目的**：通过官方 Codex App Server 协议把 Thread/Turn/Item、approval、subagent 与 interrupt 映射到 Pawork canonical session/agent/event，而不是模拟 UI 或依赖 DOM/CDP 注入。

**涉及范围**：新增 `client-codex-app-server`；`client-adapter-api`；`app-service` 可选 host；protocol golden fixtures

## 细分步骤

1. **官方协议基线** —— 固定目标 Codex app-server schema/version，首版使用稳定 stdio/本地 socket，记录实验性 transport 限制；目的：避免版本漂移假设。
2. **Thread/Turn/Item 映射** —— start/resume/fork、turn start/interrupt、item/turn notifications 与 usage；目的：保持 canonical lifecycle。
3. **Agent/审批映射** —— `parentThreadId`、agent metadata、approval request/result、workspace/environment identity；目的：子 Agent 与权限不丢失。
4. **能力/错误处理** —— tool namespace、compaction、bounded ingress overload 与 unsupported fields 显式协商；目的：禁止 200+JSON 即视为兼容。
5. **Golden/ownership tests** —— 覆盖 fork/resume/subagent/approval/interrupt/disconnect/revision 冲突；目的：协议升级可回归。

## 主要产出物

- `CodexAppServerAdapter` + 可选 host 启动入口
- Codex ↔ canonical 映射表与 capability matrix
- versioned golden fixtures / ownership contract tests

## 验收标准

- [x] thread/turn/item/parentThreadId/approval/interrupt 无静默语义丢失
- [x] tool namespace 与 compaction capability 不支持时显式返回 `ProtocolUnsupported`
- [x] adapter 不读取 Provider credential、不绕过 app-service/policy
- [x] app-server 版本升级可通过 golden diff 定位 breaking change

**相关文档**：[client-adapters](../docs/features/client-adapters.md) · [multi-agent](../docs/features/multi-agent.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP](../ROADMAP.md)

## 当前进度（2026-08-13）

- 独立 `client-codex-app-server` crate：`wire` / `map` / `adapter` / `host` 已实现；协议版本钉死 `PROTOCOL_VERSION = "2026-08"`。
- 线协议：JSON-RPC 风格且**省略 `jsonrpc` 字段**；stdio JSONL 会话环；握手 `initialize` → `initialized`；审批为 server→client **请求** `item/commandExecution/requestApproval`；过载 `-32001`。
- 能力矩阵：`compaction`（默认可协商）/ `experimentalApi`（opt-in）/ `tool.namespace`（不在白名单，使用点 fail-closed）。legacy `thread/compacted` 显式 `ProtocolUnsupported`，不得顶替 `contextCompaction`。
- 已加入根 workspace members；嵌套 `[workspace]` 已删除；deps 归一 `.workspace = true`。**未**依赖 `app-service`；生产 AppService 经 `CoreDispatcher` 注入，CLI 入口仍待宿主装配。
- Review 修复：`turn/steer` 不再伪装成新 `RunStart`（会丢掉「向飞行中 turn 注入」语义）；Core 无对应命令时显式 `ProtocolUnsupported`。`thread/fork` 在缺 `lastTurnId` 时仍用 sentinel `parent_event_id`，属有界限制。
- L1：`cargo test -p client-codex-app-server --test capabilities --test lifecycle --test handshake --test golden`（29 passed，含 steer fail-closed）。
