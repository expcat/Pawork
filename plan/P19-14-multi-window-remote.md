# P19-14：多窗口、远程连接与系统通知

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-1～P19-4、P13-5、P13-6、P17-11、P17-12、P18-2

**最终目的**：让一个 Desktop 进程安全管理多个窗口和本地/远程 `pawork` instance，在网络切换、身份变化与系统通知跳转时保持连接/窗口归属明确，且不形成窗口间权威状态。

**涉及范围**：GPUI window/connection manager、instance profiles、remote auth/reconnect UX、presence、`DesktopPlatform` notifications/deep links/single-instance、window preferences

## 细分步骤

1. **Window/instance mapping** —— 每个 GPUI Window 绑定明确 instance/client/identity；共享连接时仍保留独立 subscription/focus/revision。目的：防止跨实例串状态。
2. **窗口生命周期** —— 通过 GPUI application/window API 实现 new/close/restore、session/detail/approval pop-out、最后窗口退出策略。目的：多窗口不影响 Core task。
3. **远程连接** —— endpoint、certificate/device identity、reauth、network handoff、reconnect/backoff 与 offline banner。目的：远程 trust boundary 可见。
4. **Presence/actor** —— 显示 CLI/GUI/device actor、当前编辑/审批与 conflict，不做 peer-to-peer sync。目的：多客户端协作可理解。
5. **系统通知与 deep link** —— approval/task/automation/monitor completion 默认脱敏；通知、点击回调和 deep link 均经 `DesktopPlatform`，校验 instance/identity/scope/nonce 后再定位。目的：后台任务可达且不允许外部输入越权导航。
6. **Single-instance** —— 用平台 adapter 协调第二进程、窗口激活和参数转发；无法在所 pin GPUI revision 上统一实现时保留显式分平台实现，不伪造 framework 能力。目的：进程与窗口归属确定。
7. **本地偏好** —— window geometry/layout/last endpoint 可保存，credential/token/session transcript 不保存。目的：便利不泄密。
8. **故障测试** —— 多窗口 race、remote drop/reauth、instance restart、第二进程竞争、deep-link injection、notification redaction。目的：连接层安全。

## 主要产出物

- GPUI Window/connection manager、平台 single-instance adapter 与 instance profiles
- Remote reconnect/auth/presence UX
- 脱敏 notification/deep-link flow 与 race/failure tests

## 验收标准

- [ ] 不同 instance/identity 的 projection、pending command 与本地偏好不串用
- [ ] 关闭任意/最后窗口不隐式取消 Core Run/Task
- [ ] remote identity/certificate/version 变化触发 reauth/resync 并暂停敏感动作
- [ ] 通知不含 prompt/token/path 等敏感正文，deep link 拒绝伪造 scope/ID
- [ ] 多窗口审批竞争与 presence 最终以 Core Event 为准
- [ ] notification/deep-link/single-instance 只经 `DesktopPlatform`，三平台真实壳均有集成证据

**相关文档**：[GUI 连接](../docs/features/gui-connection.md) · [ADR-023](../docs/adr/ADR-023-one-core-many-guis.md) · [ADR-027](../docs/adr/ADR-027-local-remote-same-protocol.md) · [Desktop GUI](../docs/features/desktop-gui.md)

**依赖建议（2026-08）**：优先使用 P19-1 已验证且精确锁定的 GPUI window/notification API；GPUI 未提供稳定契约的 window-state、single-instance、deep-link 由最小 OS adapter 实现并藏在 `DesktopPlatform` 后。开工前逐项复核所 pin revision，不跟随 Zed `main`，不把 Zed 私有实现视为 GPUI 公共 API。
