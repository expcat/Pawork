# RV-04 · 子代理工具展开人工验收

> 状态：✅ 完成（模拟验收）；独立 review / 复审 PASS。真实 Host 现场重放、真实窗口与鼠标像素待验。更新：2026-10-10。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收子代理工具的目标、参数、结果和拒绝状态。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[Desktop Spec](../spec/desktop.md)、[GUI 设计](../gui-design.md)、[orchestration Spec](../spec/crates/orchestration.md)。

## 验收范围

- 在真实子代理条目展开工具目标、参数和结果。
- 分别观察权限拒绝与执行失败，不用同一笼统状态隐藏差异。

## 完成条件

- 用户可检查实际调用和回执，展开与折叠不丢信息。
- 拒绝和失败可区分，并与 Host / 工具事件一致。

## 完成记录

2026-10-10 在 `TimelineProjection` 历史页水合、`SubagentConversationState` 与 GPUI `TestAppContext` 中模拟验收，基线 `854f7e7323d8f662da38564dbd2f14e6bfbcc562`。新增 `subagent_tool_row_expands_call_and_receipt` 与 `subagent_tool_denial_stays_distinct_from_execution_failure`；不改展示契约、wire 或生产逻辑。

- **已实现**：两条最小回归补上子代理工具行验收缺口——主路径覆盖投影、参数 pretty、默认折叠、正式 AX Button+Press、展开值带回执、再折叠不丢时间线事实与重复展开一致；关键失败路径覆盖权限拒绝、执行失败、畸形参数回退、长结果预览与审批状态原文。
- **模拟验收通过**：`read_file` 标题为 `read_file src/lib.rs`，参数 JSON pretty；折叠 AX 值为标题与 Completed，展开后追加 `fn main() {}`，再折叠后 status / detail / arguments 仍在，重复展开值一致。`write_file` 拒绝正文保持 `tool call denied by user`，`run_command` 执行输出保持 12 行原文；两者 wire status 均为 failed 但 detail 不互盖，审批行保留 Host 回退标题 ``approval requested · tool · tool `write_file` requires approval`` 与 `approval deny`，不新造分类或翻译数据。畸形参数 `{not json` 原样回退，超过 10 行结果先显示前 10 行、展开回全文。
- **自动门禁通过**：`cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders -- --test-threads=1 subagent_tool_`，2 passed / 0 failed / 280 filtered，exit 0；`rustfmt --edition 2021 --check apps/desktop/src/ui/subagent_panel.rs` 与 `git diff --check` exit 0。前两次编译尝试分别因测试误用私有 `AxTree::permits` 与 Press 闭包借用失败，exit 101；初审修复后同命令重跑 2 passed / 0 failed / 280 filtered，6.43s，exit 0。原始日志保留在证据中。
- **待验边界**：真实 Provider、真实窗口、鼠标像素，以及 Host 进程内 engine → project_event 的现场重放未验。live `ToolCompleted` 只有 success 且无错误正文，`TimelineProjection::apply_event` 没有审批臂；权限拒绝句与 approval deny 需 session_get 历史页水合后出现。本记录是模拟验收，不登记为真实人工验收或 live Host 验收。AX value 超过 300 字符仍按既有规则截断，全文留在时间线条目与展开预览；本轮短回执未覆盖该截断。

证据：`/tmp/pawork-rv04-subagent-tools/evidence/`（四次编译 / 测试原始输出、结论、R1 / R2 diff 与 SHA256）。审查元数据见下；不推送。

独立 review（2026-10-10，Sol max，thread `01a12604-d7b5-79c2-96e4-faa0674cb2cf`）：R1 turn `01a12604-db24-7dc3-b5d4-1d541814501c` 报 1 个 P2——审批夹具填入真实 Host 为空的 `tool_name` / `text`，且把审批请求与拒绝排在工具完成之后，不能证明历史水合。Grok xhigh thread `01a125f6-820d-7321-b26f-ed5f9e3090ee` 修复 turn `01a1260c-8915-7c91-98a9-a5241ec16cec`：请求仅保留 status=pending 与原始 detail，请求 → 拒绝 → 工具完成顺序与 Host 一致，标题回退为 approval requested · tool · tool `write_file` requires approval，拒绝为 approval deny。R2 受审完整 diff SHA256 `ba44674dd96f9ebe676e36eb437a9c0dca4acf3d96cb82ab8f468c8ff1929642`，复审 turn `01a12619-0424-7701-908b-3646e5832444` PASS，未发现新的阻断或非阻断问题。非阻断边界为长结果只验证预览函数、真实按钮交互、真实 Provider、live Host、真窗口与像素；本结论不代表真实人工验收或 live Host 验收。

本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。
