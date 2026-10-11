# AC-03 · 产品 Plan 窗口键盘人工验收

> 状态：✅ 完成（模拟验收）；独立 review PASS；系统边界待验。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

闭合 Plan GUI 的完整键盘矩阵；本项指产品窗口，不是 docs/plan 目录。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[GUI 设计](../gui-design.md)、[Desktop Spec](../spec/desktop.md)、[app Spec](../spec/crates/app.md)。

## 验收范围

- 按当前真实入口和 enabled gate 验收 Plan 窗口的 Tab、激活、退出和焦点返回。
- 覆盖只读、旧 Host 或无可用动作等当前合同分支，不发明新的 Plan 产品功能。

## 完成条件

- 键盘能访问并执行允许的动作；本地 enabled gate、未保存字段及旧 Host 版本拒绝分支不产生请求。只读由 Host Policy 在请求后拒绝，单独核对错误回执。
- 完整矩阵逐项有实际结果，关闭后焦点归属正确。

## 完成记录

2026-10-12，按用户授权完成 GPUI 模拟验收。执行对话 `01a12b9d-6460-7241-ad3d-5075ea560128`（Grok 4.7 xhigh）；唯一独立 review 对话 `01a12bc8-e1ea-7d90-8469-2c8d7a1d9442`（gpt-6.1-sol max），turn `01a12bc8-e74e-7112-9119-e0e6504e8a30` 为 completed / error=null / PASS，阻断与非阻断问题均无。

输入走正式 `AppView::open_plan`、TextInput 与模拟 Tab / Shift-Tab / Enter，按钮由现有 `on_activate` 分发；私有 Unix socket Host 捕获 typed 请求并给出回执，未直接调用 `PlanView::action` 冒充键盘输入。新增的窗口级 Tab 拦截先于 CodeEditor 缩进，按所属窗口过滤。本次没有复现 macOS 真窗口 Tab 故障；其 NSEvent monitor 原路径仍在 GPUI 之前处理裸 Tab。

| 分支 | 实际模拟结果 |
| --- | --- |
| 三字段与五动作 | 标题、步骤、原因可输入；Tab / Shift-Tab 到动作，Enter 完成 Save、Refresh、Submit、Approve、Reject。捕获 session、标题、步骤、expected_version 与拒绝原因，UI 随回执收敛。 |
| 已加载但无版本 | 允许 Refresh / Save，Tab 跳过 Submit / Approve / Reject，无额外请求。 |
| 未加载 / API 1.23 | Tab 跳过 Save 及后三动作；初始查询和再次 Refresh 在 controller 的 API 1.24 gate 拒绝，Host 无 Plan 请求。 |
| 未保存输入 | Submit、Approve、Reject 均显示 `plan.unsaved`，Host 方法列表不变。 |
| busy | Host 捕获一次 Refresh 后按住回执；五动作 tab stop 关闭，再按 Tab / Enter 不新增请求，释放后仍只有一次 Refresh。 |
| 只读回执 | 本地仍派发 Submit；模拟 Host 返回 `Plan is read only`，UI 保留 draft 并显示错误。这不重新证明生产 Core Policy。 |
| 两窗口 | 每个窗口的 Tab 只移动自己的焦点，一次一格。 |
| 退出与父焦点 | Escape 不关闭；以 GPUI `Window::remove_window` 模拟关闭后，父窗口仍聚焦打开前的正式 Composer 句柄。 |

最终命令 `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders plan_window_keyboard -- --nocapture`：3 passed / 0 failed / exit 0，测试用时 8.45 秒。包括既有 `plan_window_keyboard_reaches_fields_and_actions` 及新增主路径、拒绝回归。证据位于 `/tmp/pawork-ac03-plan-keyboard/evidence/plan_window_keyboard.R3.log` 与 `.exit`；两轮执行原始工具记录另存 `WORKER-tool-original.json`、`WORKER-R3-tool-original.json`。

首轮主路径编译失败（E0308 / E0599）的文件日志被成功重跑覆盖，未重造；原始工具输出仍保存在 `WORKER-tool-original.json`。拒绝回归首轮连接失败（exit 101）日志保留，修正模拟 Host 的 resume 回包后 R2 通过；补齐 busy 和 Composer 焦点后 R3 三条同轮通过。两次辅助检索的 shell 引号错误也保存在原始工具记录。最终定向 rustfmt 和 diff 检查通过；未运行全 workspace 门禁。

受审快照 `PENDING.diff` 为 58,456 字节，SHA256 `fc2f9560e8ba3fdc788183fded56725004a749dca51afda89067b2a23132525a`。审查与指挥复核均确认快照、四文件 hash 和当前差异一致，之后只同步本文件及 ROADMAP 的审查状态，源码与 Desktop Spec 未变。原始 PASS 记录保存在 `REVIEW-PASS.json`；最终提交记录见同目录 `COMMIT.json`。

原生 AX、系统按键、真窗口像素、traffic-light 关闭、系统自动焦点回返及生产 Core Plan / Policy 的真实验收继续待验。当前结论只闭合授权的模拟范围。

