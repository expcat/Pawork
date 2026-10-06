# B1 · Policy / Exec 公开面与安全回归裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决策略与 PTY 公开 API 的保留范围，以及五项 Exec 安全缺口的回归范围。

## 当前线索与读取范围

原 ROADMAP 登记 `PolicyEngine.mode` 字段、`classify_command` 公开面和 PTY `read_output` 系列零消费，以及 Exec 五项安全行为缺口；实施前重新核对调用者和五项缺口的具体内容。

[policy Spec](../spec/crates/policy.md)、[exec Spec](../spec/crates/exec.md)、[安全规格](../spec/security.md)。

## 实施范围

- 逐项选择保留、收窄或删除，说明公开契约和真实消费者。
- 若保留或修改安全行为，只补对应必要定向回归；不借此扩大测试体系。

## 完成条件

- 每个 API 和安全缺口有可追溯裁决，不能仅凭检索未命中删除。
- 受影响的 Policy / Sandbox / PTY 拒绝、取消和恢复语义不回退；同步对应 Spec。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

