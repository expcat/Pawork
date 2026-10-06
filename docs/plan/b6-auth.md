# B6 · Auth 存储 API 与别名保留期裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决凭证 API、中转类型及旧 serde 别名的保留范围。

## 当前线索与读取范围

登记项为 `ApiKeyCredential::store/store_with_scopes/delete`、`StoredCredential::with_expires_at`、`DeviceAuthorization` 中转结构和 `keychain_*` serde alias 的版本期保留面。

[auth Spec](../spec/crates/auth.md)、[凭证与脱敏流程](../spec/flows.md)。

## 实施范围

- 核对账号导入、OAuth、Secret backend 及旧配置的真实消费和兼容要求。
- 裁决后只收敛对应接口或别名，不改变 Secret 不入数据库和日志的红线。

## 完成条件

- 每项保留期、删除条件或实际消费者有记录，旧配置处理明确。
- 凭证访问、刷新、过期和脱敏的受影响定向回归通过，Spec 同步。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

