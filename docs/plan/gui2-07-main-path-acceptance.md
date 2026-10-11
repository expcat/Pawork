# GUI2-07 · 受影响主路径与旧缺口收口验收

> 状态：✅ 完成（模拟验收）；独立 review PASS。前置：GUI2-01～06 已按用户授权完成模拟验收及独立 review。更新：2026-10-11。

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

2026-10-11：前置 GUI2-01～06 已按用户授权采用模拟方式验收，独立 `gpt-6.1-sol` / max review 或复审 PASS 并本地提交。本项复用其独立路径，历史结果不记成当次执行：

| 前置 | 已审覆盖 | 原始证据目录 |
| --- | --- | --- |
| [GUI2-01](accept-gui-workbench.md) | 首页、项目 / 无项目切换、分组、草稿与连接状态 | `/tmp/pawork-gui2-01-workbench/evidence/` |
| [GUI2-02](gui2-02-conversation-composer.md) | 对话阅读、原文复制、消息菜单、Composer 和 Settings 往返 | `/tmp/pawork-gui2-02-conversation-composer/evidence/` |
| [GUI2-03](gui2-03-quick-search.md) | 快捷查找、投影任务 / 页面导航、草稿隔离与断线 gate | `/tmp/pawork-gui2-03-quick-search/evidence/` |
| [GUI2-04](gui2-04-conversation-navigation.md) | 对话正文查找、回合定位、阅读解锚与切任务清空 | `/tmp/pawork-gui2-04-conversation-navigation/evidence/` |
| [GUI2-05](gui2-05-work-panels.md) | 宽窄窗口、工作面板往返、错误 / 空态和选择保持 | `/tmp/pawork-gui2-05-work-panels/evidence/` |
| [GUI2-06](gui2-06-settings-search.md) | 设置中英查找、定位清空、控件 gate 和供应商错误层级 | `/tmp/pawork-gui2-06-settings-search/evidence/` |

新增跨界主路径由 `xai/grok-4.7` / xhigh 执行对话 `01a128a2-671f-7001-8307-14fe1d7c3e41` 完成，turn `01a128a2-6af5-78a1-a678-736413b9bf15` completed、error=null。独立 `gpt-6.1-sol` / max review 对话 `01a128bc-35ac-7020-95e5-94e9e2dc1b5b`，turn `01a128bc-39af-70f3-b7c5-5117cf28e47f` completed、error=null、最终 PASS；受审五文件快照 39,488 bytes，SHA256 `1a8615dd867841439b2d96f391a3a463e276032ef404c5d2c27a4b655d144084`。真实 Host、窗口、原生 AX、像素与系统 IME 不因这些模拟结果关闭；IME / 模型目录专项继续见 [AC-02](accept-ime-model-navigation.md)。

本次发现并修复 Settings 代理输入框按 Enter 会进入隐藏 Composer 发送入口的缺口：`send_current_message` 在非 Workspace 路由直接返回。工作台内的查找仍由 `on_send_message` 分流，显式 AX Send 保留原入口。生产改动六行，新增一条 GPUI 主路径回归，Desktop 包级 Spec 同步。

1440×1024 GPUI 测试窗口使用当前 AppView、投影与产品 AX / 键盘入口：项目与无项目任务分别保存草稿；对话查找 Enter 从 `1 / 2` 步进到 `2 / 2` 且不发送；Changes 并排展开 / 收起后回到 Activity 焦点；进入 Settings Network 并给代理输入框焦点，Enter 后不锁定发送且两条草稿完整；返回原任务，焦点回到 Settings 入口。Inspector 动画在测试中 snap 到生产同一终宽，只证明终态布局与入口，不证明 180ms 动效。宽窄窗口 / 三档字号以及 Settings 查询清空另复用上表已审证据。

断线时 AX Send 无 Press 权限；恢复连接投影和一条模型后，正式 AX Send 进入 `send_current_message`，锁定本地 `composer_sending` 与 `composer_send_target=s-free`，尚无 local-echo。切到另一任务再注入原任务的 `MessageSent`，只清原任务草稿，不向当前任务上屏。该测试没有真实 client / Host；源码与本地状态只证明调用 controller，注入回执不证明 Host `RunStart` 接受或真实 Provider 执行。

当次证据 `/tmp/pawork-gui2-07-main-path/evidence/`：`command.txt`、`test-final.log` 与 `test-final.exit` 对应 `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders main_path_keeps_draft_ownership_across_surfaces -- --test-threads=1`，exit 0、`1 passed / 292 filtered out`、7.91s。前三次 exit 101 分别为单命中步进断言、未推进 Inspector 动画、Settings Enter 误发送；恢复记录在 `failure-recovery.md`，指挥从执行线程取回实际命令、输出与退出码保存在 `recovered-thread-failures.json`，不把这些失败登记为通过。初次格式检查涉及未改基线行；只格式化本次 app.rs，mod.rs 保留无关基线格式，新增门控未引入格式差异。

```text
Validated: 上述精确 Cargo filter exit 0；定向 app.rs rustfmt --check exit 0；git diff --check exit 0；GUI2-01～06 提交均在当前 HEAD 历史
Targeted regressions: main_path_keeps_draft_ownership_across_surfaces；草稿 / 查找 / 面板 / Settings / 发送门控与迟到回执归属
Full workspace gate: NOT RUN（当前未设置全量门禁）
```
