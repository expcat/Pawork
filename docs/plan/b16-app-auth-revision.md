# B16 · App 子 Core 认证修订与重导出收窄

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

核实冗余装配原因并裁决 crate 内重导出。

## 当前线索与读取范围

登记项为 same-provider 子 core 的 `provider_auth_revision` 不回填导致首轮冗余重装配，以及 lib.rs 对 EventHub / IdempotencyStore 家族的 crate 内 re-export。

[app Spec](../spec/crates/app.md)、[auth Spec](../spec/crates/auth.md)。

## 实施范围

- 通过当前装配路径确认修订状态缺口，避免按历史现象补兼容分支。
- 按实际消费者收窄 crate 内导出，不改变 EventHub 和幂等账本语义。

## 完成条件

- 冗余装配的原因和结果可证实；认证更新仍能触发必要重装配。
- 相关认证、幂等及事件回归通过，必要公开面变化同步 Spec。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

