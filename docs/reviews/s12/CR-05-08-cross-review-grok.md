# CR-05 / CR-08 High Findings 交叉复核（Grok）

- 复核对象：[CR-05-persistence-ledgers.md](CR-05-persistence-ledgers.md) 的 S12-CR05-01 / 02；[CR-08-desktop-gui.md](CR-08-desktop-gui.md) 的 S12-CR08-01 / 02 / 03
- 复核人：xai/grok-4.6（grok_reviewer）
- 复核日期：2026-08-18
- 方法：不采信报告转述，逐条独立打开源码（路径+符号+行号）核对实际行为；遵守 S12 只读纪律，未运行任何构建/测试/二进制。本文件只写裁定，不新建 finding。

## 裁定表

| 编号 | 原严重度 | 裁定 | 一行理由 |
| --- | --- | --- | --- |
| S12-CR05-01 | High | uphold（维持 High） | `messages` 投影与 `resume_messages` / Timeline / compaction 删除都按 session 全局 sequence，不读 active branch 或祖先链；Fork 后消费面必然混入或误删跨分支消息。 |
| S12-CR05-02 | High | adjust-severity（降为 Medium） | 失败/取消不入账属实，但 chat 热路径没有用该 ledger 做 quota/budget 门禁；事件流仍有 `UsageUpdated`，影响是计量低估而非控制面绕过。 |
| S12-CR08-01 | High | uphold（维持 High） | `insert_entry` 按 sequence 插入后不平移 `assistant_anchor` / `tool_anchors` 的 index；分页与 live 交错时会改错槽位或复制一条永久 running 的工具条目。 |
| S12-CR08-02 | High | uphold（维持 High） | Desktop 已持有 `(provider, model)`，但 `RunStart` 只能带 model；Host 用 `models_overview().find(id)` 取首个 owner。同名模型会切错通道/凭证。报告里的 deepseek↔opencode-go 方向写反。 |
| S12-CR08-03 | High | uphold（维持 High） | 除 Composer 文本键外，审批/取消/模型/会话/新建全是无焦点、无 tooltip 的 `div.on_click`；键盘用户无法走完 fail-closed 主路径。K-03 不能替代这条实现缺口。 |

## 逐条复核记录

### S12-CR05-01 — 分支/Fork 消费面无 branch 维度投影（uphold）

- 存储形状：`storage/session/src/migration.rs` 的 `session_events` 有 `branch_id` 且 `UNIQUE(session_id, sequence)`（约 30-42 行）；`messages` 表只有 `session_id+sequence`，没有 `branch_id`（45-53 行）。后续迁移未给 messages 补维度。
- 投影写入：`storage/session/src/projection.rs` `apply_projection` 的 `MessageCommitted` 只 `INSERT INTO messages(..., session_id, ..., sequence, ...)`（91-100 行）。`projection_snapshot` / `load_snapshot` 按 `session_id` 全表读取（452-461、542-548 行）。`rebuild_projection` 重放整 session 事件，会复现同一错误投影（472-506 行）。
- 消费面：`host/app/src/lib.rs` `resume_messages` 直接返回 `projection_snapshot(session_id).messages`，不读 `active_branch`（1034-1037 行）。CLI `host/cli/src/chat.rs` 与 `sessions.rs:118`、GUI `host/app/src/gui_host.rs` `RunStart`（1025 行）和 `timeline()`（699-702 行，`replay_events` 全 session）都走这条无 branch 路径。
- 库内已有正确原语但未接到消费面：`SessionStore::events_by_branch`（`event_store.rs` 346-379 行）明确只返回目标 branch 追加事件；`fork_from_event` + `switch_branch` 已由 GUI fork 暴露（`gui_host.rs` 1225-1230 行）。S10 任务书 `plan/S10-serve-clients.md:29` 要求补「fork 操作与投影」。S10 冒烟写的两分支 resume 只证明 `--branch` 会 `switch_branch` 且后续 append 进 active branch（`persist.rs` 8-19 行），不能证明 messages 投影按祖先链过滤。
- 压缩删除：`projection.rs` `CompactionCompleted` 执行 `DELETE FROM messages WHERE session_id=?1 AND sequence<=?2`（169-177 行），不看事件所属 branch。自动压缩回调 `host/app/src/loop_ctx.rs` `compact_history` 用全量 `replay_events`，并硬编码 `DEFAULT_BRANCH_ID` 调 `CompactionEngine::compact`（182-255 行）。任一 branch 的全局水位都会删掉其他 branch 更低 sequence 的消息投影。
- 裁定：报告的实际/期望/影响链成立。事件事实表仍 append-only，所以不是不可恢复损坏，但 Fork/Resume/Timeline/compaction 主路径会稳定给出错误上下文。High 维持。

### S12-CR05-02 — 失败/取消 run 用量不进 usage ledger（adjust-severity → Medium）

- 契约：`foundation/domain/src/events.rs` `UsageUpdated` 注释写明监督器应捕获最近观测用量，「确保失败/取消时已发生用量不丢失」（111-115 行）。
- Engine：`engine/engine/src/tool_loop.rs` 只在成功轮把 `summary.usage` 累进 `run_usage`，并只把它放进 `RunCompleted`（167、234-242 行）。超轮 `RunFailed`（351-366 行）、provider `RunFailed`（362-366 行）、`emit_cancelled` 的 `RunCancelled`（574-586 行）都不携带累计 usage。流式 `UsageUpdated` 会经 `event.rs:202-205` 进事件流。
- Host：`host/app/src/lib.rs` `run_session` 只在 `Ok(summary)` 调 `record_completed_usage`；`Err(_)` 只把 task 标 Failed（1293-1320、1323-1345 行）。`session_usage_inner` 同样只累加 `state == "completed"` 的 `RunCompleted.usage`（1555-1610 行）。
- 影响面校正：同一 ledger 供 `pawork usage` / quota 查询 / supervisor budget 使用（`host/app/src/control.rs` 31-68 行），但 chat/`RunStart` 热路径没有在跑之前查 quota 或拒绝超支。`BudgetExceeded` 只出现在 `pawork agents demo`（`orchestration_host.rs`）。因此「重复失败持续免费化 / 绕过预算事实来源」高估了控制面效果；真实缺口是查询口径与 S11 账本少计已发生用量。事件并未丢，可按 `run_id` 重放补记。
- 裁定：事实 uphold，严重度降为 Medium（Requirement Gap）。若后续 chat 热路径用该 ledger 做硬门禁，应回升 High。

### S12-CR08-01 — Timeline 锚点数组索引跨分页失效（uphold）

- 锚点是 index 不是 identity：live `ToolStarted` 把 `insert_entry` 返回的位置写入 `tool_anchors`（`apps/desktop/src/projection.rs` 576-598 行）；`append_assistant_delta` 同样把 index 写入 `assistant_anchor`（666-704 行）。历史 `tool_started` / `assistant_message` 同构（766-832 行）。
- 插入不平移：`insert_entry` 用 `partition_point` + `Vec::insert`（892-900 行），已有条目后移后旧 index 不更新。`update_tool_entry` 按旧 index 取槽（901-940 行）；槽位不是 `ToolCall` 就返回 false。
- 交错窗口真实存在：`controller.rs` `open_session` 在分页循环里持续 `TimelineLoaded`，注释写明分页期间 live 事件先到（187-242 行）。`ui/mod.rs` 同线程先 `apply_timeline_page` 再 `apply_event`（231-241 行）。gui-design §4.1(3)（`docs/gui-design.md:128`）把该重叠定为正式场景。
- 复现链可静态推出：live `ToolStarted(seq=10)` 锚点 idx=0 → 历史 `user_message(seq=5)` 插到 0 → live `ToolCompleted(seq=11)` 打到 UserMessage，走 600-627 行 fallback，用 `tool_call_id` 当名字再推一条；原条目永远 running。现有单测 `timeline_pages_dedup_by_sequence_and_merge_committed_text`（1500-1532 行）只覆盖同 sequence 去重，没有「低 sequence 页条目晚于高 sequence live 锚点」。
- 裁定：Bug 成立，落在 S7 重连/分页主路径，High 维持。

### S12-CR08-02 — RunStart 丢失 provider 维度（uphold）

- Desktop 选择器是二元组：`projection.rs` `set_pending_model` / `effective_model`（1020-1029 行）；UI 按 `provider_id + id` 高亮（`ui/mod.rs` 1273-1300 行）。发送时 `send_current_message` 只取 `(_, id)`（543-546 行）；`run_start_command` 只写 `model`（737-746 行）。
- 协议/Host：`foundation/protocol/src/app/command.rs` `AppCommand::RunStart` 仅有 `session_id/user_message/model/profile`（313-326 行）。`gui_host.rs` 在 `UnknownModel` 时用 `models_overview().into_iter().find(|entry| entry.id == model)` 取第一个 owner 再 `switch_provider`（1040-1086、1071 行），然后 `model.switched` 覆盖 UI（1101-1113 行）。
- 同名模型是仓库自己的矩阵：`ROADMAP.md` §1.1 同时登记 `deepseek/deepseek-v4-flash` 与 `opencode-go/deepseek-v4-flash`；`apps/desktop/src/main.rs` `pick_other_model`（355-367 行）也按这两个 provider 找同一 id。`models_overview` 按 `FIRST_PARTY_CHANNELS` 顺序合并，同 id 先到先得（`lib.rs` 1461-1484、1536-1548 行；`channels.rs` 106-136 行里 `opencode-go` 在 `deepseek` 之前）。因此用户选 **deepseek**/deepseek-v4-flash 时，find() 更可能落到 **opencode-go**，不是报告写的相反方向。方向写反不改变「会切错通道/凭证/计费」的结论。CR-07 未重复立项，本条作为跨包首登有效。
- 裁定：协议无法表达用户选择的 provider，High 维持。整改必须协议 minor + host 解析 + Desktop 发送一起做。

### S12-CR08-03 — 主路径不可键盘操作、无 accessible name（uphold）

- 键位面：`apps/desktop/src/ui/mod.rs` `install_keybindings` 只绑定 `TextInput` 的 Enter/Shift+Enter/编辑/粘贴（48-68 行）。全仓 Desktop UI 只有 `TextInput` 实现 `Focusable` 并 `track_focus`（`text_input.rs` 533、555-556 行）。
- 点击面：分组菜单 1084-1096 行、范围筛选 1105-1116 行、全局 `+` 1119-1131 行、会话行/Fork 855-925 行、模型选择 1273-1300 行、审批三按钮 1246-1268 行、Cancel 1350-1362 行、Send 1364-1377 行，全部是 `div().on_click(...)`，无 tooltip、无焦点链、无菜单方向键。`TaskRailGrouping::accessible_name`（`projection.rs` 174-180 行）只被当成 grouping 按钮的 element id（`ui/mod.rs` 1085 行），不是可访问名称。
- 契约：`docs/gui-design.md:177`「主路径可全键盘操作」；`design/README.md` §3.2/§3.3/§3.6/§7 要求角标按钮有 tooltip、accessible name、键盘焦点与快捷键，审批是设计主路径状态。K-03 只登记缺失人工窗口证据，不能把「未实现键盘/名称」算作已验收。
- 行号校正：报告把审批三按钮写成 1325-1365、全局新建写成 1139-1151，实际分别是 1246-1268 与 1119-1131。控件与行为仍在。
- 裁定：实现缺口成立，且挡住键盘用户的审批/取消，High 维持。

## 复核补充（不构成新 finding）

- CR-05-01 的压缩路径比报告多一处：`compact_history` 不但投影删除无 branch，连 `CompactionEngine` 调用都钉死 `DEFAULT_BRANCH_ID`。整改时不要只改 `resume_messages`。
- CR-05-02 与 CR-06 无重复立项；orchestration `flush_to_ledger` 是另一条账本，不能当作 chat 失败路径已补记。
- CR-08-02 的 `models_overview` 去重使目录本身也可能只露出一个 `deepseek-v4-flash` owner；即便 UI 列表碰巧完整，发送链路仍丢 provider。

