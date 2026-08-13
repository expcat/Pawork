# Agent Client Adapters

## 职责

把 Codex App Server、Claude Code Gateway、ACP 等外部 Agent Client 的 session、agent、permission、tool 与事件协议映射到 Pawork canonical domain。Adapter 是 `app-service` 上方的并列 Client Channel，不取代 GUI Connection Protocol，也不构造第二个 Core。

## 设计要点

- Adapter 只负责 decoding/encoding、version negotiation、capability mapping、client identity extraction 与 event translation。
- 协议中立的 `ExternalAgentIdentity`（session / agent / parent-agent）落在 `client-adapter-api`；tenant 只来自宿主 `TrustedTenantContext`，身份字段不得作为跨 tenant affinity key。Claude 头名与校验仍在 `client-claude-gateway`。
- 客户端专有 JSON 类型不得泄漏进 `agent-engine`；不支持的功能返回 `ProtocolUnsupported`，禁止静默丢字段。
- Adapter 不持有 Provider credential，不做 credential failover、模型路由、权限最终决策或 Agent lifecycle ownership。
- `SessionRegistry` 保存 `client_session_id ↔ core_session_id`、`client_connection_id`、`ownership_epoch`、`last_seen_revision`、loaded/subscribed/executing 状态和 capability snapshot。
- GUI 仍只经 GUI Connection Protocol 接入；ACP、Codex、Claude、IDE、SDK 与 Mobile 是独立协议通道，共享 `app-service` 和 Core 事实源。

## 接口

```rust
#[async_trait]
pub trait ClientAdapter: Send + Sync {
    fn kind(&self) -> ClientKind;
    fn capabilities(&self) -> &ClientCapabilities;

    async fn decode(
        &self,
        frame: ClientFrame,
    ) -> Result<Vec<CanonicalClientEvent>, AdapterError>;

    async fn encode(
        &self,
        event: &CanonicalCoreEvent,
    ) -> Result<Vec<ClientFrame>, AdapterError>;
}

pub trait ClientAdapterFactory: Send + Sync {
    fn create(&self, client: ClientDescriptor) -> Result<Arc<dyn ClientAdapter>, AdapterError>;
}
```

## Adapter 范围

| Adapter | 首轮协议范围 | 关键不变量 |
| --- | --- | --- |
| Codex App Server | `thread/start/resume/fork`、`turn/start/interrupt`、item/turn notification、approval、`parentThreadId` | tool namespace/compaction capability 不静默丢失；首版以稳定 stdio/本地 socket + versioned schema 为基线 |
| Claude Gateway | Anthropic Messages stream、tool_use/result、session/agent/parent-agent headers、signed reasoning continuity capability | `X-Claude-Code-*` 身份进入 usage/audit attribution，不作为跨 tenant affinity key |
| ACP | initialize/capabilities、session create/resume/prompt/update、permission/tool event、cancel | 未支持 method 返回显式错误；继续由 `acp-host` 承载协议宿主 |

## Session Binding 生命周期

```text
Discovered → Negotiated → Loaded → Subscribed → Executing
                    └────────────→ Disconnected → Reattached
任何 ownership_epoch 冲突 → StaleOwner（拒绝写入，先重同步）
```

共享 `CODEX_HOME`、相同 transcript 路径或相同 session id 不能替代 ownership/revision 协议。

## 优先级

- P0：ClientAdapter contract、capability snapshot、SessionRegistry、Codex/Claude/ACP golden fixtures。
- P1：协议版本矩阵、热升级、session ownership recovery、兼容诊断；`pawork` Codex/Claude stdio 宿主入口。
- P2：更多客户端 adapter；DOM/CDP 注入不得成为核心兼容层。

## 验收标准

- 外部协议字段到 canonical event 有显式映射表和版本；未知能力显式拒绝或降级
- client identity、session revision 与 ownership epoch 可持久化和重放
- Adapter 无 Provider credential、账号选择和业务权限决策
- Codex、Claude、ACP 关键消息各有 golden fixture；取消/审批/断连重连有 contract test
- GUI protocol frame 与各 Agent Client channel 保持隔离

## 相关文档

- [GUI 连接与多客户端](gui-connection.md) · [sessions](sessions.md) · [multi-agent](multi-agent.md)
- [ADR-030 Core 单一事实源](../adr/ADR-030-core-sole-source-of-truth.md) · [ADR-033 控制面分离](../adr/ADR-033-control-plane-separation.md)
- [P17-7 ACP Host](../../plan/P17-7-acp-host.md) · [ROADMAP Phase 18](../../ROADMAP.md)

