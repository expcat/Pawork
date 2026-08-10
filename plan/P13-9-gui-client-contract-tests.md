# P13-9：测试 GUI Client 与 API Contract Tests

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟡未开始 · 依赖：P13-5、P13-7

**最终目的**：实现 GUI Client SDK（`gui-client`）与协议测试客户端（`apps/protocol-test-gui`），在不开发真实 GUI 的情况下验证多 GUI 全流程；并建立 GUI Connection Protocol 契约测试套件，覆盖关键 Command/Query/Event/Snapshot 与多客户端/重连/取消场景。

**涉及范围**：`gui-client`、`apps/protocol-test-gui`、`test-support`

## 细分步骤

1. **gui-client SDK** —— 连接、握手、订阅、Snapshot/重连。目的：GPUI Desktop 与 Rust 协议测试端复用。
2. **协议测试客户端** —— 创建/发送/收流式事件、多客户端并发。目的：全流程验证。
3. **契约用例集** —— 覆盖关键命令/事件、Snapshot/重连/取消、多 GUI 同步。目的：行为可验证。
4. **CI 接入** —— 目的：回归保护。

## 主要产出物

- `gui-client` SDK + `apps/protocol-test-gui` + 契约测试套件

## 验收标准

- [ ] 模拟 GUI 可创建 session、发送消息并接收流式事件
- [ ] 覆盖关键命令/事件、Snapshot/重连/取消、多 GUI 场景，CI 通过

**相关文档**：[gui-connection](../docs/features/gui-connection.md) · [GUI Connection Protocol](../docs/architecture/api-surface.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)
