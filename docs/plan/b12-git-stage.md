# B12 · Git Stage / HunkStage 产品面裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

在生产接线和归档之间裁决 Git 写操作。

## 当前线索与读取范围

Stage / HunkStage 缺少生产接线；当前 Desktop Changes 保持只读，相关候选为 BK-GIT-02。

[git Spec](../spec/crates/git.md)、[Desktop Spec](../spec/desktop.md)、[Git 候选](../spec/backlog.md)。

## 实施范围

- 先明确真实用户流程和消费面，选择生产接线或归档。
- 选择接线时先完成 Host 协议、Policy、审批/回滚语义和冻结契约裁决；GUI 不直连 Git。

## 完成条件

- 接线或归档有正式裁决；不能把库内操作当 GUI 写入已实现。
- 实施后的相关 Git、协议和审批回归通过，架构与 Spec 一致。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

