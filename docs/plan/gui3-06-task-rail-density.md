# GUI3-06 · 侧栏降噪与字号密度人工验收

> 状态：✅ 完成（模拟验收与定向回归）；独立 review PASS。真实平台边界见完成记录。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收任务分组、侧栏密度和真实 live 状态点。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 验收 Timeline 与 Projects 分组、折叠和计数；Timeline 仅一个符合条件的项目组时省略冗余项目头，Projects 仍保留项目头。
- 空闲任务不画状态点；needs-input / running / blocked 与真实 live 事件一致，不凭缺失的 wire 数据画终态绿点。
- 在三档字号，特别是 150%，观察任务标题、尾部动作和日期 / 项目头；悬停、键盘焦点和改名 / 归档入口保持可达。

## 完成条件

- 用户确认分组与真实任务 / 项目关系一致，状态点有实际事件依据。
- 三档字号下侧栏可读可操作，密度与真实像素有记录。

## 完成记录

本轮执行：2026-10-11，`xai/grok-4.7 xhigh`。基线 `5fa04e02cc15d51417762562887740bf83ae5cba`。证据目录：`/tmp/pawork-gui3-06-task-rail-density/evidence/`。

- **分组与事件**：扩充现有 `task_rail_skips_redundant_headers_and_compacts_150`，核 Timeline 单项目 / Unassigned 省头、Projects 保留头、多项目保留头，以及省头时折叠状态不能藏任务。正式 `ControllerEvent::Event` 驱动 Running、completed 清点、Blocked 和优先的 Needs input；空闲任务无点。事件产生的 Unread 标签也纳入断言，不人为清除真实投影状态。
- **150% 操作**：GPUI TestAppContext 布局与正式输入链核长标题槽大于 80px、改名 / 归档 32×32 且不压标题；模拟鼠标打开改名，调用 Enter 对应 handler 拒绝空白、接受有效标题，AX Press 触发归档。未接 Host snapshot 时，改名不先画新标题、归档不先隐藏任务；本项不将 handler 调用记为真实键盘或持久回执。
- **三档绘制复用**：核 `3d334a1191d31ad6f504fa43796fd3414351a181..HEAD` 的 `apps` / `crates` 差异为空，复用 GUI3-05 在同生产 Rust 版本的 100% / 125% / 150% 真窗截图（2026-10-11），路径在 `/tmp/pawork-gui3-05-svg-icons/evidence/`。这些图片证明普通标题下的侧栏密度；长标题与动作的结论来自本项模拟布局，图片不充作其像素证据。
- **改动与失败收口**：只扩充既有回归及 Desktop 包级 Spec 的测试资产说明，未改生产行为。首轮断言遗漏 `Unread` 失败 exit 101，原日志保留；修正后第二、第三次同一定向测试均 exit 0。初次 rustfmt 检查失败保留，最终 `rustfmt-check-3` / `diff-check-3` 均 exit 0。
- **独立审查**：`gpt-6.1-sol max`，对话 `01a12997-b1bc-79d3-a6f4-a1a7341f2304`，turn `01a12997-b786-7f42-8262-d4fa69e3cfe8` PASS，阻断 / 非阻断问题均无。核受审 4 文件与 `PENDING` 逐字一致及各轮真实退出码；`REVIEW-PASS.json` 保存结论，完成状态元数据随后同步。指挥 diff 与 121 个链接检查均 exit 0。
- **交付边界**：按用户授权完成人工路径的代理模拟；外部 live 事件由 fixture 提供，但通过正式投影，未冒充真实 Provider。系统鼠标 / 键盘、长标题像素和本项独立 Host 持久回执未由本测试覆盖；同版 GUI3-05 真实归档与撤销回执可作既有依据。

Validated: `cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders task_rail_skips_redundant_headers_and_compacts_150 -- --test-threads 1`（最终 1 passed、294 filtered、exit 0，`task-rail-150-3.log/.exit`）；`rustfmt --check --edition 2021 apps/desktop/src/ui/accessibility/app.rs` 和 `git diff --check`（最终 exit 0）。

Targeted regressions: `task_rail_skips_redundant_headers_and_compacts_150`，覆盖正式 live 投影、终态清点、分组和 150% 操作 / 拒绝路径。

Full workspace gate: NOT RUN（当前未设置全量门禁）。
