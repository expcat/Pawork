# R4 Wave B 收口记录（2026-08-28）

> 范围：审批流 / error 原因 / 取消 / 流式 / 千级虚拟化 / 断线重放一致 U2 九场景 + Failed 摘要原因显示（WS-1）、九场景驱动（WS-2）、审批收口对齐（WS-3）、用户消息乐观回显与 entry-compare v2（WS-4）、合成终态闸门（WS-5）；State B shell 回归。glm_reviewer 一轮评审（P2×1 / P3×1）：P2 同批修复，P3 登记为已知限制。
> 事实源：本目录 [u2-r4b-6/](u2-r4b-6/)（最终全绿证据）、[u2/](u2/)（r4b-5，P2 修复前全绿）、[u2-failed-r4b-4/](u2-failed-r4b-4/)（WS-5 bug 定位证据）、[state-b-shell/](state-b-shell/)（r4b-shell-1）；Spec [app](../../spec/crates/app.md) / [desktop](../../spec/crates/desktop.md)。

## 1. 实现落点

- **WS-1 Failed 摘要真实原因**（[projection.rs](../../../apps/desktop/src/projection.rs) `failed_run_reason`）：种子 / 重放路径从持久化 wire 事件提取真实 provider 原因（`run failed · {reason}` 标签与 `strip_prefix` 精确互逆）；live `RunChanged{Failed}` wire 无原因字段，摘要卡诚实兜底 "The run failed."（`help_exact` 钉契约，不伪造）。
- **WS-2 九场景驱动**（[ui-r4-wave-b-states.sh](../../../scripts/ui-r4-wave-b-states.sh) + [ui-wave-d-tools.py](../../../scripts/ui-wave-d-tools.py) `states-assert`）：seed → serve → 隔离 desktop → barrier / 相位断言轮询（禁固定 sleep）→ 每场景 AX dump + 截图归档；13 相位登记，python 单测 22 例钉断言与守卫。
- **WS-3a 种子审批决议广播修复（真 bug）**（crates/app）：非 live 等待中的审批决议原先只持久化不广播，活动会话的 tool 行永不收口。修复：`append_payload` 返回落库 envelope → `resolve_waiting_tool_call` 返回序列 → Queued 臂 persist-first 成功后经既有 `GuiBroadcastSink` 逐 envelope 补广播；`broadcast_event` 过滤后仅 `ToolExecutionCompleted{is_error:true}` → `AppEvent::ToolCompleted{success:false}` 上 wire，`ToolApprovalResponded` / `MessageCommitted` 仍不进实时流——**wire 契约零变更**。测试 `tool_approve_non_live_waiting_broadcasts_tool_completed` 钉「恰好一条 failed ToolCompleted + 无 approval 类 wire 事件 + 库内三事件仍在」。
- **WS-3b 驱动对齐**：approval-resolved 相位断 tool-row value=failed（决策行 live 不推不断言）；新增 approval-replayed 相位（切走再切回强制快照重拉后决策行 `approval approve_once` 出现、卡不复活）。
- **WS-4a 用户消息乐观回显（真缺口）**：live wire 无用户消息事件（`MessageCommitted` 不进实时流），发送不回显。`MessageSent` 回执携带 text → `note_user_echo` 在 active session 直接 push UserMessage 行（event_id `local-echo-{run_id}`、timestamp UI 注入、sequence 借用最大 wire sequence——不进 seen、不占号段，后续 wire 事件有序落在其后）；非 active 不 echo，重选 / 重连后由快照重放的持久化行替换。未触碰 protocol 共享 reducer 与 wire。
- **WS-4b entry-compare v2**（ui-wave-d-tools.py）：断线重放一致性三重合同——barrier entry_count 相等 + 种子 `evt-fx-*` identifier 集合一致 + live 行 value 多重集一致（`run failed · …` 归一，app-evt / local-echo / persisted-evt 分类计数）。
- **WS-5 合成终态闸门（真 bug）**（crates/app）：spawned task 对 `outcome Err` 原先无条件补发合成 `RunChanged{Failed}`——engine 已报真实终态时再补一条幽灵失败（seq-0 插时间线顶端，cancel 被谎报 Failed）。修复：`GuiEventBus.terminal_reported` 登记（仅 `publish` 即 GuiBroadcastSink 路径登记终态 run_id；`publish_raw` 不自登记），仅 engine 未报终态即死（plan 闸门拒绝、宿主侧早退）才补发合成兜底；收尾 `clear_terminal_reported` 防无界增长。三个定向测试：fail 不重复 / cancel 不谎报 / 早死兜底仍在。
- **评审 P2 修复（合成序号）**（[bus.rs](../../../crates/app/src/gui_host/bus.rs) `SYNTHETIC_SEQUENCE_BASE`）：`publish_raw` 合成事件序号从 2^60 递增自取——真实持久化 sequence 从 1 单调递增不会到达该段，既不触发 reducer seen 去重吞掉真实事件，也让合成 "Run failed" 有序插入落在既有时间线内容（含乐观回显）之后；seq-0 旧行为把合成摘要插到时间线顶端、压在用户消息回显之上（WS-4a 落地后首次可见）。
- **常量修正**：beta-long 真实条目数 64（每轮 RunStarted+user+assistant+RunCompleted ×16，前期误判 48，r4b-2 红灯暴露）。

## 2. 评审（glm_reviewer 一轮，2026-08-28）

- **P2 早死合成终态渲染到时间线顶部**：`publish_raw` 硬编码 `stream_sequence: 0`，经 reducer 有序插入落 index 0；WS-4a echo 借用最大 wire sequence 追加末尾，两者交互使 "Run failed" 压在用户消息之上。同批修复见 §1 末条；回归：app 侧早死测试扩展（合成信封恰两条、序号 ≥2^60 且按到达序递增）+ desktop 投影级 `synthetic_terminal_after_user_echo_lands_at_bottom`（行序钉 app-4 → local-echo → 合成 failed，条目升序不变量保持）。
- **P3 早死 run 的回显行重选后消失**：plan 闸门在 `MessageCommitted` 之前拒绝，消息从未持久化——存量语义本就如此，echo 使其可观察。登记 [desktop Spec](../../spec/crates/desktop.md) §8 已知限制；是否把用户消息持久化提前到闸门之前属产品决策（见 §6 拍板项）。
- **红线核查三条全过**：① echo 不进 seen、不持久化、借用 sequence 本就在 seen 中不影响重放；② `terminal_reported` 时序安全（engine 各终态臂先 await sink 再返回 Err；run_id 进程内唯一；persist-error 早退正确落入合成兜底）；③ 补广播不重复上屏（ToolCompleted 按 run+tool_call_id 回填既有行，重放被 seen 去重；`clear_pending_for_tool` 双清后台与活动审批卡）。

## 3. 定向回归（全绿）

```text
cargo test -p pawork-app --offline --lib --tests
  -> 156 passed / 0 failed（WS-3a 广播 1 + WS-5 闸门 3 在册；早死测试扩展合成序号断言）
cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders
  -> 110 passed / 0 failed（Wave A 收口 107 → +3：WS-1 原因提取 / WS-4a echo / P2 合成排序）
cd scripts && /tmp/pawork-wave-d-venv/bin/python -m unittest test_ui_r4_wave_b_states
  -> 22 passed / 0 failed
```

Targeted regressions: 安全红线无触碰（wire / domain / protocol 零 diff，合成序号不改 reducer 逻辑）；持久化与重放契约 golden 不推迟项已由 timeline_projection_host 与 reducer 既有用例覆盖；协议与解析 golden 未受影响。
Full workspace gate: NOT RUN（当前未设置全量门禁）

## 4. U2 真窗口门禁

- **State B shell 回归 r4b-shell-1**（[state-b-shell/](state-b-shell/)）：结构全 PASS（collapsed / narrow / restored / resumed / empty 五相位；composer-height 为 F-09 已知漂移，R5 范围，blocking=false）。
- **U2 九场景 r4b-6**（[u2-r4b-6/](u2-r4b-6/)，最终二进制含 P2 修复）：14 相位断言 + entry-compare **全 PASS**——S1 审批三相位（visible / resolved / replayed）、S2 failed 种子原因、S3 cancelled、S4 tool failed、S5 虚拟化（barrier 64 条目、AX 节点 < 64 窗口切片卸载）、S6 流式（entry_count 25→29 + Ready for review + composer 清空）、S7 live-failed 诚实兜底 + failed-replayed 真实原因、S8 hang-cancel（cancel 可用 → cancelled 摘要 → composer 复可用）、S9 断线重放（entry_count 35==35、种子 5==5、live 多重集一致）；14 张截图与 AX 树归档。
- **历史轮次**：r4b-1 stalled（启动早期相位超时，证据仅 1 断言）；r4b-2 红于 virtualized（64/48 常量误判）；r4b-3 红于 live-failed（live wire 无原因 → 诚实兜底对齐后转绿）；r4b-4 红于 entry-compare 36v35（[u2-failed-r4b-4/](u2-failed-r4b-4/)：action-trace 条目递进 + AX identifier diff 定位出 WS-5 幽灵合成终态）；r4b-5 全绿（P2 修复前，[u2/](u2/)）。

## 5. 已知偏差与遗留

- live `RunChanged{Failed}` 不带失败原因：live 摘要卡诚实兜底 "The run failed."，真实原因需重放（切走再切回 / 重连）后可见；wire 演进需 ADR。
- 早死 run（plan 闸门拒绝）的回显行重选后消失（P3，存量语义，desktop Spec §8）；合成条目在屏时同会话新真实事件按序号插到合成条目之前（深边角化妆性排序，重选自愈）。
- 滚轮无法经 U2 注入（swift helper 无 wheel），BackToBottom 抢夺场景只登记不驱动（留 U1）。
- composer-height 156（合同 88–94）= F-09，R5 范围；State B zones current 映射待 F-12（R6）后补齐。

## 6. 退出拍板

1. **State A/B 区域 SSIM ≥0.99（2026-08-28 拍板 1，已确认）**：同 R3 先例移交 R10 终局门禁；R4 以结构门禁与 U2 九场景退出。记录值未达阈值的主因是 fixture 演示内容与设计稿不一致（重塑已随 R3 拍板 c 移交 R10）。
2. **wire 演进（仍开放）**：live `RunChanged` 不带失败原因、无用户消息 wire 事件——现状以乐观回显 + 重放兜底覆盖且零 wire 变更。是否接受现状或立 ADR（含把用户消息持久化提前到 plan 闸门之前）待拍板。
