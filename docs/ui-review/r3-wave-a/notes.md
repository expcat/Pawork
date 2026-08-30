# R3 Wave A 收口记录（2026-08-27）

> 范围：TaskRail 顶部 F-03（标题行 / scope 行 / 连接行三行节奏）与 F-04（日期桶 → 项目 → 任务列表、底部账户区 honest-hidden），按 [plan/R2-R3-ui-shell-navigation.md](../../../plan/R2-R3-ui-shell-navigation.md#r3--taskrail-与任务导航) 范围 1–4 的静态结构部分；导航状态与键盘（范围 5/6）留 Wave B。
> 驱动：新增 [scripts/ui-r3-wave-a-projects.sh](../../../scripts/ui-r3-wave-a-projects.sh)（State C driver：seed → serve → desktop → AXPress 会话 → AXPress grouping → AXPress Projects 菜单项 → 截图 → normalize → 分区 diff）；[scripts/ui-wave-d-tools.py](../../../scripts/ui-wave-d-tools.py) 增 projects 相位断言。

## 1. 实现落点

- **F-03 顶部三行**（[task_rail.rs](../../../apps/desktop/src/ui/task_rail.rs)）：标题行「Pawork」22px semibold + ghost grouping 角标 28×28（hit area ≥24，radius 4）；全宽 raised scope 行 h36 / 1px 描边 / r4 / 18px 垂直居中左对齐；连接行 Ø10 状态点 + `connection_status_label()` 17px secondary + 28×28 全局「+」。AX identifier 全部冻结不变。
- **F-04 列表与底部**：日期桶头 18px medium secondary；项目头 chevron + 名称 + 独立右对齐计数 + 28×28 定向「+」；44px 任务行（选中 raised + r4，时间 17px 右对齐）；底部账户区只保留「Local」本机身份行（TR-12 honest-hidden，不画头像 / quota / 组织）。
- **状态点诚实语义**（[projection.rs](../../../apps/desktop/src/projection.rs)）：新增 `SessionLiveStatus`（Running / NeedsInput）与 `session_live_status()`——Running = `active_runs` 成员，NeedsInput = 该 session 有待审批（优先于 Running）；其余空心灰圆不声明语义，wire 无每会话终态字段故不画终态绿点。
- **theme 几何冻结**（[theme.rs](../../../apps/desktop/src/ui/theme.rs)）：字阶增 TITLE=22 / BODY=18 / BODY_SM=17；`metrics::RAIL_*` 15 个 TaskRail 几何常量以量图行位锚点反推，落在 reference 遮罩坐标 ±2 内；render 与 AX 共享同一组常量。退役仅 rail 消费的 ICON_SMALL / ICON_MEDIUM / ICON_LARGE。
- **组件层**：[button.rs](../../../apps/desktop/src/ui/components/button.rs) 删除无消费点的 Icon variant，新增 `bordered()` / `radius()` / `center()` / `vcenter()`；[list_row.rs](../../../apps/desktop/src/ui/components/list_row.rs) 任务/项目头行高 44 + 垂直居中。
- **AX 同源**（[accessibility/app.rs](../../../apps/desktop/src/ui/accessibility/app.rs)）：rail 几何改用 `metrics::RAIL_*`；连接文案同源 `connection_status_label`；会话行状态词同源 `session_status_description`。

## 2. 审查修复（glm_reviewer 一轮，无 P0）

- **P1 Running 假阴性/假阳性**：`session_live_status()` 原实现只读当前 session 的 run snapshot；修复为 apply_event 在 active-session 闸门前跨会话维护 `active_runs` 成员（非终态登记、终态按 run_id 移除并清 pendings），返回 `timeline_changed || membership_changed`。
- **P3 Reconnect AX rect**：点击热区未含 `mt_2` 容器偏移，补 +PAD(8)。
- **P3 grouping 角标 AX y**：36px 行内 28px 钮垂直居中，AX rect y +4。
- **P2 Spec 回写**：本批同批完成（docs/spec/crates/desktop.md §2/§3.2/§5/§7）。
- **断言对齐（主代理）**：F-03 落地后连接行可见文案按既有 Spec 合同带「Local · 」前缀（`Local · Connected[ · resume]`），wave-c/wave-d 工具 reconnected 相位断言从过期基线对齐为 `Local · Connected` 前缀匹配（区分 Connecting… 瞬态的防线语义不变）；三处测试 fixture 同步。

收口审查（第二轮）：无 P0。补了 composer `MessageSent` 乐观 Running（`note_session_run` 始终 upsert `active_runs`，不限 active session）、后台会话 `ToolApprovalRequired`/`ToolCompleted` 过闸门前入账以免 Needs-input 假阴性/假阳性、add-task AX 在 36px 连接行垂直居中、grouping/scope 菜单锚到真实 F-03 行（标题 36 + traffic-light 36 + PAD，不再用旧 `CONTROL_HEIGHT=28`）、任务行恢复 `.px_2()`。Reconnect AX 已含 `mt_2`/`PAD` 8px；Ghost 角标已 `.radius(4.0)`。

## 3. 定向回归（全绿）

cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders：**84 passed / 0 failed**（基线 78 + 新增 6）：

- projection::tests::session_live_status_running_needs_input_priority_and_plain
- projection::tests::session_live_status_tracks_live_run_changed_membership
- projection::tests::note_session_run_marks_running_before_live_run_changed
- projection::tests::background_tool_approval_marks_needs_input_without_active_session
- theme::tests::task_rail_geometry_and_font_constants_match_frozen_tiers
- accessibility::app::tests::session_ax_description_carries_live_status_word

脚本侧：python -m unittest test_ui_r3_wave_a_tools / test_ui_wave_b_tools / test_ui_wave_c_tools / test_ui_wave_d_tools **55/55 绿**（r3-wave-a 13 新增：projects 相位断言 / grouping 菜单项 identifier 大小写兼容 / driver 守卫）。

## 4. 真窗口证据

| 门禁 | 结果 | 证据 |
| --- | --- | --- |
| State A U2/U3（ui-wave-d-state-a.sh，label r3a-2） | 结构断言全 PASS；taskrail 分区 SSIM 0.378（R2 收口基线）→ 0.6258（iter1，已移出仓库）→ **0.6941**（几何精调后） | [state-a/](state-a/) |
| State C U2/U3（ui-r3-wave-a-projects.sh） | 结构断言全 PASS（含 projects 相位：grouping 值切换 / 菜单收起 / date-group 退场 / 双项目块在场）；taskrail 分区 SSIM **0.3543**（State C 史上首个 current 基线） | [state-c/](state-c/) |
| Wave B 回归（ui-wave-b-states.sh） | 全相位 PASS | [wave-b-regression/](wave-b-regression/) |
| Wave C 重连回归（ui-wave-c-connect.sh） | 5 相位全 PASS（disconnected / reconnected / host-stopped / connect-failed / host-restart），Reconnect 渲染改动（mt_2 容器 / h36）无回退 | [wave-c-regression/](wave-c-regression/) |

## 5. 已知限制与开放决策

- **taskrail 分区 SSIM 天花板**：fixture 内容形状（7 会话演示数据的标题/时间分布）与定稿图演示数据不同，分区像素分存在结构性天花板；State C reference 色调整体比冻结 token 暗，压低全区分。2026-08-28 用户拍板 **c**：R3 以结构门禁退出，0.99 像素门禁连同 fixture 演示数据重塑移交 R10；State C reference 底色是否按冻结 token 归一属设计基准变更，R10 重采集前须另行批准。遮罩策略调整合同不可行（UI_Review §0.1）。量化分解见 [history R3](../../history.md#r3--taskrail-与任务导航2026-08-2728)。
- State C 低分同时包含非本波区域（timeline / inspector 内容缺口），按波次分工移交后续 R 阶段。

## 6. 状态

Wave A 已实现、自动门禁通过、证据归档。2026-08-28 拍板 c 后 R3 阶段已退出；overlay 人工复核见 [state-a](state-a/) / [state-c](state-c/) diff/zone-evidence/，终局视觉签字仍属 R10。Wave B 已于同日收口，见 [../r3-wave-b/notes.md](../r3-wave-b/notes.md)。
