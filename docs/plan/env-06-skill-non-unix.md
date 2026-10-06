# ENV-06 · 非 Unix 技能录制写入前置与验收

> 状态：待平台实现。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#缺环境与外部条件)

## 目标

补齐非 Unix 安全写入原语，或明确维持 Unsupported 边界。

## 当前依据与读取范围

状态承接迁移前 ROADMAP；本次未执行对应环境验收。

[workspace Spec](../spec/crates/workspace.md)、[app Spec](../spec/crates/app.md)、[安全规格](../spec/security.md)。

## 验收范围

- 原记录指出非 Unix 未实现安全目录写入原语，入口明确返回 Unsupported。
- 取得目标平台与安全等价性依据后实现最小写入；不能用普通覆盖写绕过路径和 Secret 边界。

## 完成条件

- 若实现，真实平台路径拒绝、目录安全和产物完整性均有对应证据。
- 若维持 Unsupported，须有明确范围裁决并同步文档，不能把未实现写成支持。

## 完成记录

待填写实际环境、输入、结果、日期与证据。前置或验收缺失时保持当前待办状态；满足全部完成条件后同步本文件，并将 ROADMAP 对应项标为 ✅。

