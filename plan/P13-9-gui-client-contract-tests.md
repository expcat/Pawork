# P13-9：测试 GUI Client 与 API Contract Tests

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P13-5、P13-7

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

- [x] 模拟 GUI 可创建 session、发送消息并接收流式事件
- [x] 覆盖关键命令/事件、Snapshot/重连/取消、多 GUI 场景，CI 通过

## 实现记录（2026-08-10）

- `gui-client` SDK（无 GUI 框架，不链接 core-runtime / app-service 运行时）：
  `GuiClient::connect`（握手 + 认证 + 版本协商 + 消费首帧 Snapshot）、
  command / query 请求-响应往返、Subscribe / next_event、Snapshot /
  Resume（Replay 补发或 SnapshotRequired 降级重建）、ArtifactRead 循环分片
  重组至 eof、Ack / Heartbeat（自动回 Pong）、close 与
  `connect_with_resume` 重连辅助；错误一律结构化 `ClientError`，不泄漏内部帧。
- `apps/protocol-test-gui`：`--self-test` 在进程内装配
  GuiServer + AppService + EventHub + pump（memory transport、tempdir 令牌），
  逐场景输出 PASS/FAIL 与退出码；`--connect local://` 提供外部连接模式。
- 契约测试（crates/gui-client/tests/contract.rs，9 项）：session 事件流、
  快照与断线重连、resume 降级、3 GUI 并发同步、命令幂等重放、100k 行
  diff 大 artifact 分片重组、版本不兼容拒绝、GUI 断线不取消 Run、ack/heartbeat。
- `--self-test` 8 场景全 PASS（session-events / snapshot-reconnect /
  resume-snapshot-fallback / three-gui-sync / command-idempotency /
  artifact-chunks / version-reject / disconnect-keeps-run）。

**相关文档**：[gui-connection](../docs/features/gui-connection.md) · [GUI Connection Protocol](../docs/architecture/api-surface.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)
