# P0-2：领域类型基线

> Phase 0 · 架构与协议冻结 · 状态：🟡未开始 · 依赖：—

**最终目的**：冻结 Agent 的核心领域类型（消息、角色、内容块、元数据、ID）。它是所有 crate 的最底层依赖——只有先定义这些，下游的 Provider/Tool/Event 协议才有东西可建模。

**涉及范围**：`agent-domain`

## 细分步骤

1. **定义 ID 类型** —— `MessageId`/`RunId`/`SessionId`/`ToolCallId` 等 typed NewType。目的：编译期区分标识，杜绝裸字符串误用。
2. **定义 Message 与 MessageRole** —— user/assistant/system/tool 等。目的：消息是 Agent 通信的原子单位。
3. **定义 ContentPart 枚举** —— Text/Image/Thinking/ToolCall/ToolResult/ArtifactRef。目的：覆盖多模态与工具交互的全部内容形态。
4. **定义 MessageMetadata** —— 模型、provider、token usage、时间戳、artifact 引用。目的：可追溯、计费与审计。
5. **守住纯领域约束** —— 仅依赖 serde + 标准库，无 IO/HTTP/DB/Tauri。目的：满足架构红线与 ADR-001。

## 主要产出物

- `agent-domain` crate：上述类型 + 单元测试

## 验收标准

- [ ] 类型覆盖 文本/图片/Thinking/ToolCall/ToolResult/ArtifactRef
- [ ] 无任何禁依赖（Tauri/SQLite/HTTP/Git/具体 Provider）
- [ ] 全部类型可 serde 序列化

**相关文档**：[领域模型](../docs/architecture/domain-model.md) · [ADR-001 纯 Rust Core](../docs/adr/ADR-001-pure-rust-core.md) · [ROADMAP](../ROADMAP.md)
