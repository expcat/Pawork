# GUI2-07 · 受影响主路径与旧缺口收口验收

> 状态：等待人工验收。前置：GUI2-01、GUI2-02、GUI2-03、GUI2-04、GUI2-05、GUI2-06 的用户验收记录。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

汇总 GUI2 各项验收证据，闭合跨壳层、对话和面板的剩余缺口。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 复用 [GUI2-01](accept-gui-workbench.md)、[GUI2-02](gui2-02-conversation-composer.md)、[GUI2-03](gui2-03-quick-search.md)、[GUI2-04](gui2-04-conversation-navigation.md)、[GUI2-05](gui2-05-work-panels.md)、[GUI2-06](gui2-06-settings-search.md) 的用户验收记录，避免重复执行已证明的独立路径。
- 补齐项目 / 无项目任务 → 草稿与发送 → 对话阅读 / 查找 → 工作面板 → Settings → 返回任务的跨界交互，核对焦点、草稿和真实结果。
- 逐项收口仍影响这些主路径的旧缺口；系统 IME、模型目录及跨面板专项引用 [AC-02](accept-ime-model-navigation.md)，不把历史代理验收当作用户确认。

## 完成条件

- GUI2-01～06 的用户验收已闭合，跨界交互没有未登记的功能缺口。
- 记录已实现、自动检查、代理真窗口观察和用户验收的不同证据；历史通过计数不作为当次执行结果。

## 完成记录

待填写实际环境、输入、结果、日期、用户确认与证据位置。历史日志不冒充当次验证；截图不写入仓库。满足全部完成条件后同步本文件，并将 ROADMAP 对应项标为 ✅。
