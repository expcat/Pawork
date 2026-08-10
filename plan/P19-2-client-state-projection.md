# P19-2：GUI Client 与状态投影

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-1、P13-5、P13-7～P13-10

**最终目的**：实现 Desktop 唯一的数据入口与可重建 projection，使 GUI 渲染层在首次连接、断线重连、事件重复/缺口和多客户端并发下都与 Core 权威状态收敛；所有界面统一走 controller+projection，不引入第二套状态源。

**涉及范围**：`apps/desktop` 的 Rust 连接层（消费 `gui-client`）、projection/controller modules、`gui-protocol` 的 Rust 类型（schema source，GUI 直接消费）、Mock GUI Server fixtures

## 细分步骤

1. **连接契约** —— 定义 connect/handshake/auth/query/command/subscribe/artifact/disconnect 的 typed command/event 面。目的：视图层不处理原始 Transport frame。
2. **Projection slices 与 Controller** —— 建立 connection/workspace/session/run/approval/diff/terminal/provider/resource/workflow/presence 的 Rust normalized state；每个 surface 一个 Controller（GPUI Entity）持有 slice、派生选择器并发送 command。目的：按领域隔离更新与渲染。
3. **Snapshot/Event reducer** —— 原子应用 Snapshot，按 `global_sequence` 幂等应用 Event，检测 duplicate/gap/out-of-order。目的：确定性重建。
4. **补洞与重同步** —— gap 先请求 replay，超出保留窗口或版本不兼容则清空业务投影并取新 Snapshot。目的：不猜测缺失状态。
5. **Command reconciliation** —— Controller 维护 pending `command_id`、idempotency/revision 与 response/event 对账；Core 拒绝覆盖 optimistic UI。目的：多窗口写入一致。
6. **状态机测试** —— property/fixture 覆盖乱序、重复、Snapshot/Event 竞态、陈旧响应、断线与身份变化。目的：锁定客户端正确性。

## 主要产出物

- Rust typed 连接层与视图 facade（仅依赖 `gui-client`，不链接 Core 业务 crate）
- 可丢弃 projection store 与 controller 层（GPUI Entity 持有，可整体重建）
- sequence/revision 状态机
- Mock fixtures、property/unit tests 与 projection diagnostics

## 验收标准

- [ ] 从空 store 经 Snapshot + replay 可重建与 Mock Core 相同状态
- [ ] 重复事件幂等；gap/out-of-order 不静默应用；无法补齐时重新 Snapshot
- [ ] 陈旧 command response/revision conflict 不覆盖较新 Event
- [ ] reconnect/reauth 不泄漏上一 identity/instance 的 projection
- [ ] GUI 进程不接触 credential、Protected Blob 明文或未校验 frame
- [ ] L1：projection property + controller/client contract tests 通过

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [GUI 连接](../docs/features/gui-connection.md) · [ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [ADR-035](../docs/adr/ADR-035-gpui-desktop.md)

**依赖建议（2026-08）**：projection 与 command reconciliation 自实现（Rust 纯逻辑，可脱离 UI 直接测试）；Controller 以 GPUI Entity 持有投影并通过其订阅机制接收变化，视图 Element 只读投影渲染，不引入第二套状态框架。
