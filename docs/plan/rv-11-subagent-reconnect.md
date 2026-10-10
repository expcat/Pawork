# RV-11 · 子代理断线与恢复人工验收

> 状态：✅ 完成（模拟验收）；独立 review / 复审 PASS。真实 Host、网络、Provider 与真窗口待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收断线保留已加载对话、重连恢复和诚实加载态。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[client Spec](../spec/crates/client.md)、[desktop crate Spec](../spec/crates/desktop.md)、[GUI 连接流程](../spec/flows.md)。

## 验收范围

- 在真实 Host / Desktop 连接中加载子代理对话后断线并重连。
- 核对已有内容、身份与新回执，避免清空内容或长期伪装加载。

## 完成条件

- 已加载内容在断线时保留，重连后正确续接。
- 状态与连接事实一致，过期回执不污染另一任务。

## 完成记录

2026-10-11 在 1440×800 GPUI `TestAppContext` 中沿正式 `select_inspector_tab`、controller 事件、`Disconnected`、`on_connected`（UpToDate 与 SnapshotRequired）和 `open_session` 路径模拟验收，基线 `0e2ac3361262339f8558b5ff010ffde43dfe5d10`。验收发现真实缺口：断线未作废子代理对话在途代际；同一会话重连会被当成切换会话而清空面板。

- **已实现**：`SubagentConversationState::mark_disconnected` 保留 agent_id 与 timeline，结束 loading 并自增 load_generation，断开前的迟到页 / 失败 fail-closed。`open_session` 在切换前判断是否重开当前会话：同会话保留子代理活动与对话并重查权威数据，切到其他会话才清空并贴底。列表成功回执后若原选中代理已不存在，清掉失效选中项与旧正文并回到未选择；查询失败继续保留旧内容。
- **模拟验收通过**：加载两个运行中子代理与长正文，滚动脱钩后断线；代理列表、child-0 选中、正文、偏移、非跟随态与 stale reason 均保留，cancelling / request_id / loading 清空。Refresh、Cancel 与改选在断线态为空操作；迟到空列表、外会话列表、旧代际页面与其它 agent 失败均不改写。重连后内容保留，补查失败不清空；权威列表只剩 child-1 时 child-0 失效选中与旧正文被收敛，迟到 child-0 页面不能进入，AX 为 No subagent selected 且无 Loading。切到 other-task 才清空并贴底。
- **自动门禁通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders disconnected_preserves_subagent_panel_content`，1 passed / 0 failed / 287 filtered，exit 0；`rustfmt --edition 2021 --check apps/desktop/src/projection/settings.rs apps/desktop/src/ui/subagent_panel.rs` 与 `git diff --check` exit 0。`ui/mod.rs` 完整 rustfmt 会递归触发无关子模块既有格式债，仅检查本次动过片段。
- **待验边界**：真实 Host 进程、真实网络断连、真实 Provider 与真窗口未验；本记录不登记为真实连接验收。

证据：`/tmp/pawork-rv11-subagent-reconnect/evidence/` 与 `.../evidence/r2/`（R1 / R2 命令、输出、结论、diff 与 SHA256）。独立 review（Sol max，thread `01a12751-9421-72e2-b36c-6bef49605517`）：R1 报 1 个 P2——同会话重连后权威列表移除原选中代理会永久 Loading。Grok xhigh thread `01a1271d-3475-7962-9217-b09103d9c4e8` 修复 turn `01a12755-78a5-72d1-9180-61d5e0d07e52` 后，R2 diff 72954 bytes、SHA256 `7e8689b451fd726289d4f5ccf0a70453126578781e9bff79f210f00b647bf2ec`，复审 turn `01a1275e-9039-7340-85ea-ddd7ba4b4b9a` PASS。本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。

