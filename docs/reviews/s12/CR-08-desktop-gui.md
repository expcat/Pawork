# S12 CR-08：Desktop GUI 与设计一致性审查报告

| 项 | 内容 |
| --- | --- |
| 审查包 | CR-08（apps/desktop、docs/gui-design.md、design/） |
| 审查日期 | 2026-08-18 |
| 主审模型 | GLM（zai/glm-5.3） |
| 审查方式 | 只读源码 + 既有文档/协议证据；未启动 GUI、未运行测试/构建（S12 纪律） |

## 1. 实际审查路径

- apps/desktop/Cargo.toml（依赖面：业务依赖仅 pawork-client；其余为 gpui/smol/tokio/serde_json/unicode-segmentation 基建）
- apps/desktop/src/main.rs（入口、probe/probe-smoke、窗口创建、resume 辅助）
- apps/desktop/src/platform.rs（runtime 宿主、默认 socket 路径、manifest deny-list 测试）
- apps/desktop/src/controller.rs（连接/事件泵、SessionGet 分页、Command/Query 构造、last-ack 记录）
- apps/desktop/src/projection.rs（Snapshot/TimelinePage/AppEvent 投影、TaskRail 分组、Resume 三态、审批/取消状态机及全部单测）
- apps/desktop/src/ui/mod.rs（AppView、TaskRail、Timeline、审批卡、Composer、Inspector/Terminal、状态栏）
- apps/desktop/src/ui/text_input.rs（IME marked_range、UTF-16 映射、键位、单行 TextElement）
- docs/gui-design.md（S7 锁定设计 + §3.2/§3.3 v3 增补）、design/README.md（v3 视觉实施基准全文；三张 PNG 按 README 文本契约核对，未做像素级比对）
- 交叉证据（只为验证 Desktop 契约，深审归 CR-07）：foundation/protocol/src/app/command.rs、app/event.rs、app/query.rs；host/app/src/gui_host.rs 的 snapshot/timeline/RunStart/ModelList 段
- 已有报告引用：docs/reviews/s12/CR-01-manifests-layout.md（dependency deny-list 已由其 cargo tree 实测，本报告不重复立项）

### 通过项（源码可证）

- 四层边界：projection.rs:1-14 仅依赖 std / pawork-client / serde_json，无 gpui、tokio、OS API；controller.rs 只调 pawork-client（tokio/smol 为运行时基建）；platform.rs 仅 runtime + 路径发现；业务依赖唯一入口 pawork-client（Cargo.toml:13-22），并有 CR-01 的 cargo tree 实测佐证。
- 无假能力入口：S8 Changes、S9 @file/Resources、S11 Workflow/quota/Agent 列表在 UI 中确实不存在（K-04/K-06 与 ROADMAP §2 S11 行已登记延期），未画可点击假入口，符合「没有投影/命令就不做按钮」。
- 审批 wire 串一致：controller.rs 发送 approve_once/approve_for_run/deny 与 ApprovalDecision 的 snake_case serde 名（command.rs:369-377）一致；审批卡三按钮 + 无默认允许 + 断线禁用（ui/mod.rs:593-596, 1325-1365）符合 fail-closed。
- IME Enter 守卫：text_input.rs:76-79 + ui/mod.rs:422-440 组合期 Enter 不发送，符合 §6。
- 取消/终态语义：projection.rs:517-575 终态清 active run 与 pending approval，历史保留；controller.rs:361-370 断开发不 cancel（ADR-026）。
- Replay 三态：projection.rs:344-431 Fresh/Replay/SnapshotRequired/UpToDate 区分 + seen 按 sequence 去重，单测覆盖（projection.rs:1785-1955）。
- 分组/范围正交：projection.rs:960-1010 Timeline（日期→项目→Task）与 Projects 共用同一 session 投影；切换不改 active session（单测 grouping_switch_does_not_change_active_session）。

## 2. 未覆盖路径与原因

- 真实窗口行为：IME 实测、多行粘贴渲染、1440×1024 对照 v3 定稿图、1080×720 可用性——S12 禁止启动 GUI；对应人工验收缺口已是 K-03 挂账，本报告仅登记源码可证的实现缺口。
- design/ 三张 PNG：仅按 design/README.md 的文本契约（尺寸/控件清单/层级）核对，未做像素级图像比对。
- gpui 0.2.2 内部行为（shape_line 对换行的实际渲染、div on_click 的键盘语义）：未运行验证；相关 finding 的定性不依赖内部实现（见各条「实际行为」措辞）。
- host/gui-server、clients/gui-client、transport 深审：归 CR-07；本报告只读取与 Desktop 消费直接相关的 host 段。
- probe-smoke / 真实通道：S12 禁止运行；既有 S10 记录（plan/S10-serve-clients.md:65）只作证据引用。

## 3. Findings（按严重度排序）

### S12-CR08-01 · Timeline 锚点索引在乱序插入后失效（分页/直播重叠窗口）

- 类别：Bug　严重度：High　置信度：Confirmed
- 证据：apps/desktop/src/projection.rs:892-900 insert_entry 按 sequence 二分插入新条目，但不调整 assistant_anchor / tool_anchors 中已记录的 index；锚点写入点：live ToolStarted 576-598、live assistant 666-704、历史 assistant/tool 766-832。后续 update_tool_entry（901-940）与 delta 合并（672-699）按旧 index 取条目。
- 实际行为：gui-design §4.1(3) 明确「分页期间 live 事件先到、页数据按 sequence 去重后交给 projection」。当历史页条目 sequence 小于已插入的 live 条目时（open_session 链式分页期间 live 事件持续到达，controller.rs:190-242），插入会把既有条目后移，锚点 stale：update_tool_entry 命中错误槽位（若恰是另一 ToolCall 则改错条目）或 miss（槽位非 ToolCall 时返回 false），随后 600-627 的 fallback 以 tool_call_id 为名字新推一条 ToolCall——原条目永久停留 running，出现重复/错误工具条目；assistant 锚点同理把后续 delta 开成新气泡或并入错误消息。
- 可复核复现（单元级）：select_session("s") → live ToolStarted(seq=10)（锚点 idx=0）→ apply_timeline_page 含 user_message(seq=5)（插入到 0，工具条目移到 1，锚点仍 0）→ live ToolCompleted(seq=11)：锚点 idx=0 现指向 UserMessage，update_tool_entry 返回 false，走 fallback 推入重复 ToolCall。现有测试（timeline_pages_dedup_by_sequence_and_merge_committed_text，1500-1532）只回放同 sequence 事件做去重，从未构造「低 sequence 页条目晚于高 sequence live 锚点到达」。
- 期望行为：任何插入/删除后锚点保持指向原条目（存 event_id/sequence 而非 index，或插入时平移）。
- 影响面：重连分页 + 直播重叠期间的时间线正确性（S7 主路径、gui-design §4.1 核心场景）。
- 验证建议：新增 projection 单测覆盖上述乱序交错序列；断言工具条目状态回填与 assistant 合并目标正确。
- 整改边界：仅 apps/desktop/src/projection.rs 锚点结构与相关单测；不重构 Timeline 数据结构之外的部分，不顺带改 UI 渲染。

### S12-CR08-02 · 模型选择的 provider 维度在发送链路丢失，跨 provider 同名模型无法表达

> **交叉复核裁定**（2026-08-18 主代理回写，Grok 复核，详见 [CR-05-08-cross-review-grok.md](CR-05-08-cross-review-grok.md)）：**uphold High**，方向修正——channels 顺序中 opencode-go 在 deepseek 之前，用户选 deepseek 时 find() 落到 opencode-go，与本报告原述方向相反；结论不变。

- 类别：Bug　严重度：High　置信度：Confirmed
- 证据：apps/desktop/src/ui/mod.rs:543-546 send_current_message 取 effective_model().map(|(_, id)| id)，丢弃 provider；apps/desktop/src/controller.rs:737-746 run_start_command 只写 model 参数；foundation/protocol/src/app/command.rs:313-326 AppCommand::RunStart 仅有 session_id/user_message/model/profile，无 provider 字段；host/app/src/gui_host.rs:1040-1086 收到跨 provider 模型时按 models_overview().find(id == model) 取第一个 owner 切换。
- 实际行为：Desktop 的模型选择器与投影保存 (provider_id, model_id) 二元组（projection.rs:1020-1029），但协议无法携带 provider，Host 兜底取目录首个 owner。仓库自身的低消耗矩阵就存在同名模型跨 provider：apps/desktop/src/main.rs:355-367 pick_other_model 明确在 deepseek 与 opencode-go 下找同一个 deepseek-v4-flash。用户选 opencode-go/deepseek-v4-flash 时可能实际切到 deepseek 通道（错误凭证/计费通道），随后 model.switched 诊断（gui_host.rs:1101-1113）把 UI selected_model 覆盖成实际（错误）provider。
- 期望行为：用户选择的 (provider, model) 完整传到 Host 并按 provider 解析；UI 显示与实际执行通道一致。
- 影响面：跨 provider 模型切换的正确性、凭证/额度通道选择、状态投影权威性。
- 验证建议：e2e/单测：目录同时含 deepseek/deepseek-v4-flash 与 opencode-go/deepseek-v4-flash 时选择后者，断 Host 实际 provider。
- 整改边界：foundation/protocol RunStart 追加 optional provider（minor bump + golden 先行）→ host/app 解析优先 provider → apps/desktop 发送 provider；三处一个任务链，不可只改 Desktop。跨包问题由本包首登，CR-07 报告应链接本条而不重复立项。

### S12-CR08-03 · 主路径不可全键盘操作，tooltip / accessible name 未实现

> **交叉复核裁定**（2026-08-18 主代理回写，Grok 复核，详见 [CR-05-08-cross-review-grok.md](CR-05-08-cross-review-grok.md)）：**uphold High**，行号校正——审批三按钮实际 1246-1268（原写 1325-1365）、全局新建实际 1119-1131（原写 1139-1151）；控件仍在，结论不变。

- 类别：Requirement Gap　严重度：High　置信度：Confirmed
- 证据：apps/desktop/src/ui/mod.rs:48-68 install_keybindings 只绑定 TextInput 编辑键（Enter/Shift+Enter/Backspace 等/Paste）；除文本输入外，全部交互控件是 div().on_click(...) 且无 track_focus、无 tooltip：分组菜单 1083-1096、范围筛选 1098-1116、全局新建 1139-1151、项目头/定向新建 759-876、会话行 839-876、Fork 864-925、Inspector 开合 941-960 与 1399-1420、模型选择器 1273-1300、Cancel 1350-1362、审批三按钮 1325-1365、Send 1364-1377。TaskRailGrouping::accessible_name（projection.rs:174-180）唯一的消费点是把它当 element id 字符串（ui/mod.rs:1085），并未暴露为可访问名称或 tooltip。
- 实际行为：键盘用户只能输入与发送；无法用键盘完成取消当轮、审批（allow once / for run / deny）、切换模型、切换会话、新建 Task、菜单与 Inspector 操作。设计契约要求：gui-design §6「主路径可全键盘操作」；design/README §3.2（tooltip 与 accessible name）、§3.3（两类新建按钮保留键盘焦点与快捷键入口）、§3.6（完整焦点顺序与菜单键位）。
- 期望行为：审批/取消/发送/模型/会话选择等主路径可键盘操作；角标按钮带 tooltip + accessible name + 禁用原因文本。
- 影响面：可访问性合规与审批 fail-closed 路径的可达性（等待审批是设计主路径状态之一）。
- 验证建议：真实窗口键盘走查（K-03 人工验收范围内补充）；先补控件级焦点/tooltip 静态断言或 UI 测试。
- 整改边界：apps/desktop/src/ui/*（含新增 keybinding/actions 与焦点链）；不改 projection/controller；与 K-03 的关系：K-03 登记的是缺失人工验收证据，本条登记实现缺口，验收不能替代实现。

### S12-CR08-04 · Timeline 无条件抢滚，Terminal 滚动被聊天事件驱动

- 类别：Bug　严重度：Medium　置信度：Confirmed
- 证据：apps/desktop/src/ui/mod.rs:227-241：TimelineLoaded 到达即 scroll_handle.scroll_to_bottom()（234）；任何 apply_event 返回 true 即同时滚动 Timeline 与 terminal_scroll（239-240）。代码中无「用户是否位于底部」检测。
- 实际行为：用户向上阅读历史时，新事件/分页到达会强制拉回底部；聊天事件变化还会无端滚动 Terminal 视图。
- 期望行为：gui-design §6「Timeline 只在用户位于底部时追随流式输出；用户向上阅读后不得抢滚动位置」；Terminal 滚动只应由 Terminal 输出驱动。
- 影响面：长会话阅读体验、直播期间回看、Terminal 与 Timeline 互相干扰。
- 验证建议：UI 测试或人工：流式期间上滚 → 断言视口不被拉回；Terminal 有历史时收到聊天事件 → 断言 Terminal 视口不变。
- 整改边界：仅 apps/desktop/src/ui/mod.rs 事件处理与滚动状态；不改 projection。

### S12-CR08-05 · Composer 固定单行高度，多行输入不增长

- 类别：Requirement Gap　严重度：Medium　置信度：Confirmed
- 证据：apps/desktop/src/ui/text_input.rs:381-391 request_layout 将高度固定为 window.line_height()（与内容无关）；:453 shape_line 只排版单行（无 wrap/多行布局）。composer 容器也未设置 88–94px 常态高度（ui/mod.rs:1326-1395）。
- 实际行为：Shift+Enter 与多行粘贴在数据层保留换行（:292-298 paste 归一化 CRLF），但布局高度恒为单行、排版按单行 shape——无法呈现多行，也不随内容向上增长。
- 期望行为：design/README §2「Composer 默认高 88–94 px；多行输入按需向上增长」；gui-design §6「Shift+Enter 换行；多行粘贴保持原文」。
- 影响面：多轮长 prompt 的编写与审阅（S7 主路径输入面）。换行在 shape_line 中的具体呈现（缺字形/吞行）需真实窗口确认，但「高度不增长」由布局代码直接可证。
- 验证建议：人工窗口粘贴三行文本（K-03 项内补充：断言显示三行且高度增长）。
- 整改边界：仅 apps/desktop/src/ui/text_input.rs 布局/排版（必要时引入多行 shape）；不改协议与 controller。

### S12-CR08-06 · All projects 下新建 Task 静默绑定第一个 workspace，无工作目录确认

- 类别：Requirement Gap　严重度：Medium　置信度：Confirmed
- 证据：apps/desktop/src/ui/mod.rs:297-312 on_new_session 取 scope_workspace_id.or(projection.workspace_id) 直接 create_task；projection.rs:336-341 merge_snapshot 把 workspace_id 设为 workspaces 首项。Composer 中不存在工作目录选择器（ui/mod.rs:1326-1395 仅模型选择器、ContextMeter、输入框、Cancel/Send）。
- 实际行为：All projects 范围点全局 + 时，Task 无提示绑定快照第一个 workspace；且 design/README §3.3 要求的「All projects 下创建后必须在 Composer 中确认工作目录」没有承载 UI（该文档 §4.1 还把 ContextMeter 定位在「工作目录选择器与 Send 之间」，选择器整体缺失）。
- 期望行为：单项目范围默认继承该 Workspace；All projects 下创建后必须确认工作目录；项目身份只来自 canonical workspace_id（不得隐式取首项冒充用户选择）。
- 影响面：Task 归组正确性——错误绑定会直接落入错误项目分组，且用户无感知。
- 验证建议：单测/人工：多 workspace 快照 + All projects 新建 → 断言出现工作目录确认而非静默绑定。
- 整改边界：apps/desktop/src/ui/mod.rs（新建流程 + Composer 工作目录显示/确认）+ projection.rs 如需当前 session workspace 投影；不新增协议命令（SessionCreate 已带 workspace_id）。

### S12-CR08-07 · S10 GUI 增量「本机多窗口」未实现但阶段已标完成

- 类别：False Completion　严重度：Medium　置信度：Confirmed
- 证据：ROADMAP.md:64（S10 行 🟢，GUI 增量含「本机多窗口」）、ROADMAP.md:68、plan/S10-serve-clients.md:69「正式 Replay、Fork、Terminal、本机多窗口」；apps/desktop/src/main.rs:583-602 只有唯一一次 open_window，全仓 rg open_window 无其他调用，无任何菜单/快捷键/action 可再开窗口。该缺口未出现在 ROADMAP §4 延期登记，也不在 K-01～K-10 基线内。
- 实际行为：应用内只有单窗口。多个 pawork-desktop 进程可各自作为客户端连接（host 多客户端已就绪），但「同一应用本机多窗口」增量不存在，阶段状态与证据声明未区分这一点。
- 期望行为：要么实现应用内多窗口（每窗独立/共享会话的策略需产品定义），要么把该增量显式登记延期并修正 S10 完成口径。
- 影响面：S10 需求追踪真实性；用户对多窗口能力的预期。
- 验证建议：若实现：真实开两窗验证互不串会话、Replay 各自正确；若延期：文档修正即可。
- 整改边界：文档口径修正（ROADMAP/plan）或 apps/desktop 多窗口实现，二选一任务；不与 K-03 合并（根因不同）。

### S12-CR08-08 · Timeline 事件保真度缺口：live ToolOutput 不渲染、历史 approval_requested 丢弃、approval_responded 无条目

- 类别：Bug　严重度：Low　置信度：Confirmed
- 证据：apps/desktop/src/projection.rs:510-660 apply_event 的 match 无 AppEvent::ToolOutput 臂（live 工具输出不回填，_ => {} 吞掉）；merge_history_item（710-870）无 approval_requested 臂——而该 kind 存在于协议与 host 投影（foundation/protocol/src/app/query.rs:94-109、host/app/src/gui_host.rs:1370-1376）；approval_responded（866-869）只清状态、不入 seen 也不产生条目。
- 实际行为：同一工具输出在直播期不可见、重载历史后出现（798-805 历史臂有回填），前后呈现不一致；审批请求/响应在历史回放中完全不可见（当前 pending 靠 snapshot 段恢复，完成的审批无痕）。
- 期望行为：gui-design §3 Timeline 覆盖「tool 调用起止」与内嵌审批；live 与历史对同一事件流应有一致投影。
- 影响面：回放/审计可读性；不破坏状态机正确性。
- 验证建议：projection 单测：ToolStarted→ToolOutput(live) 与等价历史页应产出相同条目；含审批的事件流重载后至少保留一条审批痕迹。
- 整改边界：仅 apps/desktop/src/projection.rs 事件映射 + 单测；不改协议。

### S12-CR08-09 · RunStatusBar 运行时长不实时更新

- 类别：Requirement Gap　严重度：Low　置信度：Confirmed
- 证据：apps/desktop/src/projection.rs:1041-1056 run_status_label(now_ms) 依赖调用方注入时间；apps/desktop/src/ui/mod.rs:1064 仅在 render 时取 now_unix_ms()；apps/desktop 中不存在任何定时器/重渲染调度（rg 无 timer/interval/UI 层 spawn+sleep）。
- 实际行为：Run 进行中若无新事件触发 cx.notify()（如等待 provider 的静默期），时长停留在上次渲染值；design/README §4.2 要求「运行中实时更新」。
- 期望行为：运行中周期性刷新时长（GPUI 定时 notify），终态停表。
- 影响面：状态栏观感；tokens/quota/tok/s 显示「—」的部分已由 ROADMAP §2 S11 行登记为延期（quota 条未接线），不在此重复立项。
- 验证建议：人工/自动化：发起慢 run，静默超过 10 秒观察时长是否走动。
- 整改边界：仅 apps/desktop/src/ui/mod.rs（运行中定时 notify）；不引入新的数据来源。

### S12-CR08-10 · render 每帧全量克隆 Timeline

- 类别：Performance　严重度：Low　置信度：Confirmed
- 证据：apps/desktop/src/ui/mod.rs:1195 self.projection.timeline.clone() 在每次 render 执行；apps/desktop/src/controller.rs:23-24 PAGE_LIMIT=500 × MAX_PAGES=200 允许单会话拉取至多 10 万条条目；条目含多个 String 字段（projection.rs:235-241）。
- 实际行为：长会话下每帧 O(n) 深拷贝（含字符串分配），叠加逐条 timeline_entry_element 构建；无虚拟化。
- 期望行为：render 路径避免全量克隆（借阅/迭代渲染），长列表虚拟化可作为后续项。
- 影响面：长历史会话的滚动/直播帧率。
- 验证建议：构造 5 万条目的投影做渲染路径基准（后续任务内执行）。
- 整改边界：apps/desktop/src/ui/mod.rs 渲染取数方式；不改 projection 数据结构语义。

### S12-CR08-11 · v3 视觉基准存在未登记的代码级漂移

- 类别：Requirement Gap　严重度：Low　置信度：Confirmed
- 证据：① Inspector 宽度 ui/mod.rs:941 为 320px，design/README §2 规定 InspectorPanel 约 440 px（320px 是 §5.1 ActivityPopover 的宽度，疑被误用）；② 折叠态 ui/mod.rs:1399-1420 是 28px 常驻竖条，而 §2/§5.1 规定折叠时「宽度归零、Workspace 扩展、右上约 320px ActivityPopover」（当前无 Changes/Agent 数据可藏，但恢复入口形态与基准不一致）；③ 每条 Timeline 条目常驻 Fork 按钮（864-925），不在 v3 定稿控件清单内。design/README §7：「后续视觉差异若属于有意改动，先更新本目录与 GUI 设计，再实现代码」——未发现对应基准更新。
- 实际行为：实现偏离冻结视觉基准且无文档先行记录；Fork 逐行按钮还与「工具调用折叠块、原生密度」的视觉原则有张力。
- 期望行为：要么按基准修正（440px、折叠形态、Fork 收进条目操作/上下文菜单），要么先更新 design/README 与 gui-design 再保留现状。
- 影响面：视觉一致性与后续 1440×1024 验收（K-03）的判定基准。与 K-03 的关系：K-03 是未做的人工对照，本条是源码与基准的可证差异，避免验收时把漂移当「实现细节」放过。
- 验证建议：整改后对照 v3 三图做 K-03 验收；本条本身以代码/文档 diff 即可复核。
- 整改边界：apps/desktop/src/ui/mod.rs 布局 +（若保留现状）design/README.md 与 docs/gui-design.md 基准更新，二选一；不动 controller/协议。

## 4. 统计

| 严重度 | 条数 | 编号 |
| --- | --- | --- |
| Critical | 0 | — |
| High | 3 | S12-CR08-01 / 02 / 03 |
| Medium | 4 | S12-CR08-04 / 05 / 06 / 07 |
| Low | 4 | S12-CR08-08 / 09 / 10 / 11 |

| 置信度 | 条数 |
| --- | --- |
| Confirmed | 11 |
| Needs Verification | 0 |

### 已知基线关联（不重复立项）

- K-03（S7 人工验收）：CR08-03/05/11 与其相关但登记的是实现缺口；验收证据仍归 K-03。
- K-04 / K-06（Changes、@file/Resources 面）：源码确认未实现且无假入口，维持原登记。
- K-08（ArtifactStreaming 能力一致性）：controller.rs:596-604 Desktop 也宣告该能力，归 K-08 统一收口。
- ROADMAP §2 S11 行（Desktop Workflow/quota 条/Agent 列表延期）：CR08-09 的 quota「—」显示归该登记。
