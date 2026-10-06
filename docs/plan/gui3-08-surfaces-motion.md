# GUI3-08 · 层次 token 与有限动效人工验收

> 状态：等待人工验收。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收深色层次、交互反馈与 Switch / Inspector 的实际动态效果。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 在真窗口观察 canvas、panel、raised、menu 和 hover 的层次，以及正文、辅助文字和焦点的可见性。
- 动态观察 Switch 120ms 位移、Inspector 180ms 开合，以及首帧和跨窄窗时的既定行为；静态截图不能证明动效。
- 验收即时 hover / pressed / focus、Changes / Resources 静态加载骨架和流式末尾静态标记；不把循环动画或系统 Reduce Motion 分支计为已有能力。

## 完成条件

- 用户实际操作并确认有限动效，记录观察环境与结果。
- 层次和动态结果分别有证据；未观察动效时继续待验。

## 完成记录

待填写实际环境、输入、结果、日期、用户确认与证据位置。历史日志不冒充当次验证；截图不写入仓库。满足全部完成条件后同步本文件，并将 ROADMAP 对应项标为 ✅。
