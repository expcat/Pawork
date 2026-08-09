# P19-2：GUI Client Bridge 与状态投影

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-1、P13-5、P13-7～P13-10

**最终目的**：实现 Desktop 唯一的数据入口与可重建 projection，使 renderer 在首次连接、断线重连、事件重复/缺口和多客户端并发下都与 Core 权威状态收敛。

**涉及范围**：`apps/desktop/src-tauri` bridge、`apps/desktop/src/state`、生成的 `schemas/gui-protocol` TypeScript 类型、Mock GUI Server fixtures

## 细分步骤

1. **Bridge contract** —— 定义 connect/handshake/auth/query/command/subscribe/artifact/disconnect 的 typed command/event 面。目的：renderer 不处理原始 Transport frame。
2. **Projection slices** —— 建立 connection/workspace/session/run/approval/diff/terminal/provider/resource/workflow/presence normalized state。目的：按领域隔离更新与渲染。
3. **Snapshot/Event reducer** —— 原子应用 Snapshot，按 `global_sequence` 幂等应用 Event，检测 duplicate/gap/out-of-order。目的：确定性重建。
4. **补洞与重同步** —— gap 先请求 replay，超出保留窗口或版本不兼容则清空业务投影并取新 Snapshot。目的：不猜测缺失状态。
5. **Command reconciliation** —— 维护 pending `command_id`、idempotency/revision 与 response/event 对账；Core 拒绝覆盖 optimistic UI。目的：多窗口写入一致。
6. **状态机测试** —— property/fixture 覆盖乱序、重复、Snapshot/Event 竞态、陈旧响应、断线与身份变化。目的：锁定客户端正确性。

## 主要产出物

- Tauri typed bridge 与 renderer client facade
- 可丢弃 Desktop Projection Store、sequence/revision 状态机
- Mock fixtures、property/unit tests 与 projection diagnostics

## 验收标准

- [ ] 从空 store 经 Snapshot + replay 可重建与 Mock Core 相同状态
- [ ] 重复事件幂等；gap/out-of-order 不静默应用；无法补齐时重新 Snapshot
- [ ] 陈旧 command response/revision conflict 不覆盖较新 Event
- [ ] reconnect/reauth 不泄漏上一 identity/instance 的 projection
- [ ] renderer 不接触 credential、Protected Blob 明文或未校验 frame
- [ ] L1：reducer property + bridge contract tests 通过

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [GUI 连接](../docs/features/gui-connection.md) · [ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [ADR-034](../docs/adr/ADR-034-desktop-gui-client-boundary.md)

**依赖建议（2026-08）**：projection 与 command reconciliation 自实现；React 通过 `useSyncExternalStore` 等标准原语订阅，不引入第二套服务端状态框架。
