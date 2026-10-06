# ENV-05 · Chrome / Edge 只读上下文环境复验

> 状态：缺浏览器授权。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#缺环境与外部条件)

## 目标

用户手动授权后复验当前页正文读取。

## 当前依据与读取范围

状态承接迁移前 ROADMAP；本次未执行对应环境验收。

[browser Spec](../spec/crates/browser.md)、[Desktop Spec](../spec/desktop.md)。

## 验收范围

- 当前记录为浏览器错误 12，Apple Events JavaScript 未启用；由用户手动开启并满足系统 Automation 权限。
- 复验 URL、标题和有界正文读取；本项是只读上下文，不是 [CU-15 外部浏览器控制](cu-15-external-browser.md)。

## 完成条件

- 真实 Chrome / Edge 正文读取成功，来源和不可信快照说明正确。
- JavaScriptDisabled / AutomationDenied 等真实拒绝仍准确显示，不自动代用户批准系统权限。

## 完成记录

待填写实际环境、输入、结果、日期与证据。前置或验收缺失时保持当前待办状态；满足全部完成条件后同步本文件，并将 ROADMAP 对应项标为 ✅。

