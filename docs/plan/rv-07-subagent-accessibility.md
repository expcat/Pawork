# RV-07 · 子代理正文键盘与朗读人工验收

> 状态：✅ 完成（模拟验收）；独立 review / 多轮复审 PASS。原生 AX bridge、VoiceOver、真窗口、像素与真实 Provider 待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收正文真正可由键盘访问和朗读。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[GUI 设计](../gui-design.md)、[验证规格](../spec/verification.md)、[desktop crate Spec](../spec/crates/desktop.md)。

## 验收范围

- 按真实焦点路径访问正文和相应操作，不能只统计 AX 节点。
- 记录实际采用的朗读机制和结果；验证规格不把全系统 VoiceOver 签字列为默认范围，不自行扩大成系统级评估。

## 完成条件

- 键盘和实际朗读结果有可复核证据，失败或范围差异明确登记。
- 不会把节点数量或纯模型映射测试写成人工可访问性通过。

## 完成记录

2026-10-11 在 GPUI `TestAppContext` 中模拟验收，基线 `5552388721a23e35bb68f42554af54a389ab2e65`。验收发现真实缺口：正文节点只有 AX 文本，未进入 Tab 焦点链，展开按钮也不回报 focused；离屏正文获得焦点后不会滚入视口，工具标题离屏时会连带裁掉仍可见的长结果展开按钮。

- **已实现**：正文 StaticText 与工具 / 长结果 / 结果盒展开按钮共用 GPUI FocusHandle 并进入 Inspector Tab 链；AX Focus dispatch 落到同一句柄，Button 声明 Focus+Press 并走与 render 同源 handler，未知 Focus 与静态文本 Press fail-closed。工具标题与长结果展开按钮按各自 bounds 裁剪，标题离屏不带走仍可见按钮。
- **模拟验收通过**：折叠态按 label · value 朗读任务、思考、助手、工具、后续用户与结果摘要；AX Focus 与键盘焦点落在同一句柄。Enter 展开工具后下一站是 Show full result，Enter 与 AX Press 均可切换全文。1440×560 真实溢出后，完全位于视口下方的正文经产品路径滚入 transcript，AX 节点发布且 focused；标题在上、按钮仍相交时标题节点消失而展开按钮保留。同帧 `follow_new_content()` 滚底覆盖 `set_offset` 时不提前登记 reveal，下一帧目标进入视口后重试，`following` 按最终偏移重估。
- **自动门禁通过**：`cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders subagent_`，12 passed / 0 failed / 276 filtered，exit 0；`rustfmt --edition 2021 --check apps/desktop/src/ui/subagent_panel.rs apps/desktop/src/ui/accessibility/app.rs apps/desktop/src/ui/components/follow_scroll.rs` 与 `git diff --check` exit 0。`mod.rs` 完整 rustfmt 会递归触发无关子模块既有格式债，仅保留本任务必要两行新增并以定向回归覆盖。
- **待验边界**：原生 macOS AX bridge、全系统 VoiceOver、真窗口、系统缩放、像素回读与真实 Provider 未验；本记录不登记为真人或系统级辅助技术验收。

证据：`/tmp/pawork-rv07-subagent-accessibility/evidence/`（R1 / R2 / R3 测试与检查原始输出、结论、diff 与 SHA256）。独立 review（Sol max，thread `01a126db-9fab-7a42-884f-6e7b91e55a07`）：R1 报 2 个 P2（离屏焦点不滚入、可见工具结果按钮被标题裁剪连带漏发）；Grok xhigh thread `01a126b2-94cf-7c31-b794-3cc33e1007db` 修复后，R2 确认独立 bounds 裁剪闭合，但发现同帧滚底覆盖 reveal 且提前登记不重试，并指出 Spec 测试计数错误；R3 修复 turn `01a126fe-4f65-7272-abc0-f9c3700ab49f` 后，最终 diff 98318 bytes、SHA256 `7a21853fe3f1c46a50f8e800dbfc8799a552f20d0e8581d5216b11df7f358692`，复审 turn `01a12719-f079-7d10-bd19-1e63a167bfce` PASS。本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。

