# GUI3-01 · 工具调用可扫读人工验收

> 状态：✅ 完成（模拟验收）；独立 review PASS。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收工具 headline、目标、展开内容与真实执行状态。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 核对单工具 headline 的 path / command / pattern 目标，以及多工具数量、前几项摘要和真实状态计数；MCP / 未知参数按现行回退显示。
- 用鼠标、Enter / Space 和 AX 展开，核对目标、必要参数、结果预览及全文。
- 对照真实工具回执区分进行中、缺结果、空结果、失败和取消，不把摘要误作执行成功。

## 完成条件

- 用户可从摘要判断操作目标，并能访问完整结果。
- 摘要与展开态均对应实际工具事件，真实状态可区分。

## 完成记录

2026-10-11：按用户授权以模拟方式执行人工验收。`xai/grok-4.7` / xhigh 执行对话 `01a128c0-329c-7b91-becb-55c970e5125e`，初轮 turn `01a128c0-378d-70f2-94f4-a1698e52ab10` 与补证据 turn `01a128cf-e731-7103-b4fb-e5e76b55c9c4` 均 completed、error=null。独立 gpt-6.1-sol / max review 对话 01a128d3-24d5-7061-8567-2a697bf48a94，turn 01a128d3-294d-7e82-a89d-16bd6a86a04d completed、error=null、最终 PASS；受审四文件快照 35,056 bytes，SHA256 ae1d43f7e844d50ae72d5747f0d432a9e7980c7fd9b5b231922b42d859a4e410。未发现产品缺口，生产行为未改；新增一条 GPUI 交互回归与测试辅助函数，Desktop 包级 Spec 同批同步。

1440×1024 GPUI 测试窗口将 22 项历史工具事件经 `DesktopProjection::apply_timeline_page` 投影为 9 条 Timeline 条目，参数事件合入对应工具。四工具组的摘要含数量、前三项 path / command / pattern 目标和 `1 completed / 1 running / 1 failed / 1 cancelled`；单条成功 `write_file src/new_feature.rs` 不附数量，单条 `find_files **/*.rs` 保留 running。MCP 未知参数回退名称，完整参数留在展开正文；12 行结果默认只含前 10 行，展开后可访问第 12 行。Run 已结束但工具存储状态仍 running 时，仅展示为 stopped，不回写事实，也不把另一 Run 的工具停止。

鼠标点击实际发布框中心展开 / 收起多工具组，布局高度随展开增加且 AX 行节点随之增删；给正式焦点句柄焦点后 Enter 展开单成功工具组，Space 展开长结果；AX Press 展开 MCP / stopped 组并将全文折回预览。展开态分别区分 In progress、Result not provided、Empty result、failed、cancelled 和 stopped；取消保留 `stopped by user` 原文。本次不调用 headline / summary helper 来代替这些交互。八内建工具的目标抽取、80 字符截断及畸形 / 缺键回退仍复用现有 helper 定向回归；apply_patch 完整绘制矩阵和 pending / unknown 绘制态不在本次 GPUI 夹具中。

证据目录 `/tmp/pawork-gui3-01-tool-summaries/evidence/`：`NOTES.txt` 记录输入与路径；`desktop-tool-summaries-retry.log` / `.exit` 对应 `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders -- tool_summaries_follow_projected_facts_across_pointer_keyboard_and_ax`，exit 0、`1 passed / 293 filtered out`、48.94s。`desktop-tool-helpers.log` 内两个精确过滤器 `tool_headline_extracts_builtin_targets_and_falls_back` 和 `tool_row_view_from_parts_normalizes_detail` 各 `1 passed / 293 filtered out`、EXIT:0，分别 0.58s / 1.67s。

首次单条回归已输出 `1 passed`、36.96s，但外层 zsh 因只读变量 `status` 失败，未保存 Cargo 自身退出码；原日志 `desktop-tool-summaries.log` 保留，SHA256 `2e7fb28d93c59d84ca26815752e061a7d9f39dd23ca243f118bf85b0b7296552`，不把外层失败记成 exit 0。补证据只重跑这一过滤器，由 Python subprocess 直接保存 returncode。指挥格式检查发现新增测试五处排版差异，只用 rustfmt 收敛，随后 --check 与 diff 检查通过；原检查失败及修后结果在 `format-original-failure.json`、`format*.log` / `.exit`。格式改动不改变测试语义，没有因此重跑已通过回归。

上述是历史事件 fixture、产品投影、GPUI 模拟交互与布局证据，不证明真实工具执行、Host / Provider、原生 AX bridge / VoiceOver、真窗口像素或用户真人确认；这些真实平台边界仍待验。独立 review 通过后可按本轮「模拟验收」口径关闭。

```text
Validated: 上述三项精确 Desktop Cargo filters；补证据主路径 exit 0，两项 helper EXIT:0；定向 rustfmt --check 与 git diff --check exit 0
Targeted regressions: tool_summaries_follow_projected_facts_across_pointer_keyboard_and_ax；既有目标抽取与状态 / detail 归一回归
Full workspace gate: NOT RUN（当前未设置全量门禁）
```
