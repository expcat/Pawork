# RV-05 · 子代理回执阅读人工验收

> 状态：✅ 完成（模拟验收）；独立 review / 复审 PASS。真实窗口、真实 Provider、键鼠与像素待验。更新：2026-10-10。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收 Markdown 回执、长回执折叠与无重复全文。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[GUI 设计](../gui-design.md)、[desktop crate Spec](../spec/crates/desktop.md)。

## 验收范围

- 真实窗口核对正文、列表、代码等当前支持的 Markdown 回执。
- 检查长回执折叠、摘要和展开，不新增另一份全文显示。

## 完成条件

- 回执可读且结构一致，摘要与展开内容对应。
- Timeline 与子代理视图不存在无必要的重复全文。

## 完成记录

2026-10-10 在真实投影、`SubagentConversationState` 与 GPUI `TestAppContext` 中模拟验收，基线 `0cf7be58f3d3394bd82ece21592a80d71bb25f2e`。新增 `subagent_result_receipt_reads_markdown_and_folds` 与 `subagent_result_dedup_follows_host_prefix_boundary`；不改生产逻辑、wire 或展示契约。

- **已实现**：两条最小回归补上回执阅读验收缺口——主路径走真实投影、GPUI 布局与正式 AX Press，覆盖 Markdown 正文、列表、代码块、长回执折叠 / 展开 / 再折叠、外层滚动与无重复全文；关键边界覆盖 Host 2000 字符前缀去重。
- **模拟验收通过**：超过 600 字符且超过 10 行的回执默认折叠为 `Receipt body is readable.`；展开后 AX 值等于 `subagent_ax_text`，含列表与 `fn kept() {}`，代码复制布局宽高大于 0；结果盒自身无纵向滚动，滚轮只改变外层 transcript 偏移。再折叠并让同一份助手正文进入时间线后，结果盒显示 `The result is shown in the reply above.`，展开入口消失，`fn kept() {}` 在 AX 中只出现一次且主 Timeline 为空。去重边界覆盖 2000 字符前缀、1999 字符、全文、两侧空白、其它字符串、空对话，以及空助手消息跳过。
- **自动门禁通过**：`cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders -- --test-threads=1 subagent_result_`，2 passed / 0 failed / 282 filtered，exit 0；`rustfmt --edition 2021 --check apps/desktop/src/ui/subagent_panel.rs` 与 `git diff --check` exit 0。Spec 同步后源码哈希未变，未重跑 Cargo；表格、链接与 diff 静态检查 exit 0。
- **待验边界**：真实 Provider、真人窗口、鼠标 / 键盘实机操作与像素回读未验；Markdown 视觉可读性只由测试布局与 AX 事实支撑，本记录不登记为真实人工验收。

证据：`/tmp/pawork-rv05-subagent-receipts/evidence/`（测试与检查原始输出、结论、diff 与 SHA256）。审查元数据见下；不推送。

独立 review（2026-10-10，Sol max，thread `01a1263c-db44-75b0-a7db-0b9357a4e2a3`）：R1 turn `01a1263c-e080-7640-b18a-70840d97aeab` 确认代码、产品链路、Host Unicode 2000 字符截断与证据一致，但报 1 个 P3——desktop Spec 仍登记 3 个测试而实际已有 5 个。Grok xhigh thread `01a1261d-b595-7841-b452-78d76b357df1` 修复 turn `01a12642-d776-7b62-bc8d-70ec034eeb73`：仅同步 `subagent_panel.rs` Spec 行的 5 个测试与两条 RV-05 回归说明。R2 受审完整 diff SHA256 `40f18055b3ac25c439a0d71ccda0367ee01bd53ee1fa3aa452e3adba983a511a`，复审 turn `01a12649-7d47-75c3-99fa-a1aa051f5894` PASS，未发现剩余阻断或非阻断问题。非阻断边界为真实窗口与像素待验；本结论不代表真实人工验收。

本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。
