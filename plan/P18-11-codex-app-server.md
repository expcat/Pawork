# P18-11：Codex App-Server Adapter

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P18-10、P15-2、P15-5、P15-8、P12-1

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

- [ ] thread/turn/item/parentThreadId/approval/interrupt 无静默语义丢失
- [ ] tool namespace 与 compaction capability 不支持时显式返回 `ProtocolUnsupported`
- [ ] adapter 不读取 Provider credential、不绕过 app-service/policy
- [ ] app-server 版本升级可通过 golden diff 定位 breaking change

**相关文档**：[client-adapters](../docs/features/client-adapters.md) · [multi-agent](../docs/features/multi-agent.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP](../ROADMAP.md)

