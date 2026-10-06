# B18 · Client SDK MockTransport 与版本 API 裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决 SDK 对外测试辅助面和未列为稳定面的版本函数。

## 当前线索与读取范围

登记项为 MockTransport `push_responses` / `fail_next_read` / `sent_count` / `assert_sent_json` 四个零消费公开方法和 `sdk_version_string`。删除涉及对外 semver 收缩。

[client Spec](../spec/crates/client.md)、[协议契约](../spec/contracts.md)。

## 实施范围

- 核对稳定面承诺、仓库内外已知 SDK 消费和公开 rustdoc。
- 裁决后做最小保留或收窄；不把测试辅助公开面悄悄当私有实现删除。

## 完成条件

- 公开契约与版本影响有裁决，稳定消费者行为不回退。
- 相关 SDK/transport 既有回归通过，client Spec 与公开面一致。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

