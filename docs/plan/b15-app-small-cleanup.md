# B15 · App 查询、设置写盘与配置错误收敛

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决并收敛三个紧相关的 App 清理线索。

## 当前线索与读取范围

旧记录包含 gui_host diff_get `complete` 表达式死乘法（原记录位置为 `query.rs`，须定位现行文件）、`set_model_enabled` / `set_provider_models_enabled` 尾部 `cleared_roles` 重复写盘，以及两处 `config_unavailable` GUI 文案。

[app Spec](../spec/crates/app.md)、[Settings Spec](../spec/settings.md)。

## 实施范围

- 先核对实际分支和文件，只有等价性成立才简化表达式或写盘。
- 统一同一失败含义的文案，保留真实错误状态和用户可执行入口。

## 完成条件

- 查询结果、默认角色清理和配置落盘行为不变；文案同义且可操作。
- 现有相关测试足够时不新增；验证真实受影响设置和查询行为。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

