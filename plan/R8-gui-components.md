# R8 — GUI 组件化与 Desktop 收口(T12)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R8 行。依据 2026-08-18 GUI 组件分析:`apps/desktop` 6,657 行、四层边界干净(ui 不碰 socket、projection 纯函数、controller 唯一写者),但 `ui/mod.rs` 1,898 行单文件承载全部渲染——15 处手写按钮、4 组复制粘贴菜单(仅数据不同)、97 处硬编码 `rgb()`、约 40 处魔法尺寸、零 hover 状态、菜单用整屏遮罩层模拟而非 gpui `anchored()/deferred()`、Timeline `uniform_list` 用于变高行(潜在裁剪 bug)。本阶段建立 theme tokens 与组件库,并收口 V2 遗留的三块 GUI 功能面与人工验收(K-03/K-04/K-06)。
>
> **实态回写(2026-08-23 波 A 开启,glm_explorer 三路核查)**:ui/ 仅 mod.rs(1,911 行)+ text_input.rs(690 行),无 pty_view.rs;硬编码色 78 处(23 个去重值,mod.rs 76 全为 `rgb(0x…)`、text_input.rs 2 含 1 处 `rgba(0x5b9dff55)`);`0x3ecf8e` 与本仓历史零命中,绿色无现存值;「手写按钮」实为样式 div + `on_click`(22 处,零 `on_mouse_down`);4 组菜单为触发按钮的 in-flow child,无遮罩层;`hover(`/`uniform_list`/`.list(` 均为 0 处(滚动走 `overflow_y_scroll`);魔法尺寸 45 处 `px()` 字面量(字号 26、图标头像 9、布局宽度 6、细部 4),圆角已是 rounded_sm/md 语义 helper。
>
> GUI 事实源:[docs/gui-design.md](../docs/gui-design.md) + [design/README.md](../design/README.md)(GUI v3 视觉基准,1440×1024)。有意的视觉变更(如补 hover)必须先更新基准再实现。

> **实态回写(2026-08-24 波 B 开启,glm_explorer 三路核查)**:菜单实态为 **5 组非 4 组**——grouping(触发 ui/mod.rs:1450-1466 / 面板 869-907)、scope(1468-1482 / 909-943)、model(触发 1708-1733 / 面板内联 1734-1771)、entry fork(「···」触发 1157-1175 / 面板 1176-1201)、workspace confirm(无触发器,`on_new_session` 439-449 条件打开 / 面板 1354-1394);任务书「model/mode/provider/session 四组」命名与实态不符——provider 并入 model 菜单选项 label,无独立 provider 菜单,无 mode 概念,session 是 rail 列表行而非菜单。手写按钮按 on_click 调用点计 **21 处**(审批 map 单点渲染 3 实例,实例口径 23;「22 处」快照两种口径均不符);`hover(`/`.active(` 全仓 0 处复核成立。波 A 后 `px(` 数字字面量与 `rgb(`/`rgba(`/`0x` 在 mod.rs+text_input.rs 零残留复核成立(theme.rs 集中持值)。菜单互斥不对称既有缺陷:grouping/scope toggle 互斥并关 model,model toggle 只动自身,可双开;全菜单无 Escape/外点关闭(基准 §3.6 已承诺 Escape 关闭)。FollowScroll 现内联:事件驱动查底(mod.rs:327-355)、用户上滚永久脱钩、无回底按钮、`follow_terminal` 无重置路径。gpui 0.2.2 `anchored()`/`deferred()`/`hover()`/`active()`/`tooltip()`/`occlude()`/`block_mouse_except_scroll()` 全部在位(vendored 源码核实)。`MAIN_PATH_TAB_STOP_IDS` 含 `model-picker`,迁移须保留 id + tab_stop + track_focus 三件套;`APP_VIEW_KEYBINDINGS` 与 `install_keybindings` 为双份字面量,本波不动;grouping/scope 触发器补 tab stop 与菜单内 ↑/↓ 导航维持缺口,留波 E 键盘走查。

> **实态回写(2026-08-24 波 C 开启,glm_explorer 三路核查)**:Timeline 现状为 `div#timeline.overflow_y_scroll + .children(iter().map())` 全量 eager 物化(ui/mod.rs:1586-1608),无任何虚拟化——首部旧快照「uniform_list 用于变高行」与 §2 波 C「uniform_list 替换」措辞均不成立,波 C 是首次引入 gpui `list()`(vendored gpui 0.2.2 变高行 + ListAlignment::Bottom 钉底,elements/list.rs:24/216)。**DiffView 不存在**:TimelineEntryKind 无 diff 变体(protocol projection/mod.rs:252),全仓 diff 渲染零命中,其消费面(K-04 Changes 面)属波 D——按消费面先行红线,波 C 不新建 DiffView,留波 D 随 Changes 面同落。「长标题 truncate(F44 遗留)」的 F44 仓内不可溯源(v2-summary S13 序列止于 F40,ROADMAP §4 无条目),改按实态登记:截断点为 TaskRail 的 Task 标题(ui/mod.rs:1136)与项目头名称(1094),gpui `.truncate()` 为本仓首个消费点。为达 §1.4 mod.rs <900 行阶段目标(波 B 收口已指派波 C),波 C 写入集实态扩为:四组件拆分(TimelineEntryView/ApprovalCard/InputArea + Timeline 容器)之外,同模式外移 Inspector 与 Rail 渲染块;基准先行——design/README.md §8.4(虚拟化 + truncate)与 gui-design.md §6 已由主代理先行落地。

## 1. 目标设计

1. **`ui/theme.rs`**:`Theme` 结构(bg/surface/border/text/accent/semantic 六组,~20 token;落地 25 个色字段——24 个 rgb/rgba 去重值 + 审批数组裸 u32 `0x3d7a4a`)+ 字阶/间距/圆角常量;78 行 `rgb(0x…)`/`rgba(0x…)` 调用(92 处)加审批数组 3 裸 u32 全部收编(任务书快照中的跨文件值 `0x3ecf8e` 与 pty_view.rs 实态不存在);深色单主题起步,不做运行时切换(结构上留 Global 挂载点)。
2. **`ui/components/`**(gpui 无官方组件库,自建;参照 Zed `ui` crate 与 longbridge/gpui-component 的 API 形状,见 [docs/references.md](../docs/references.md)):

| 组件 | 收编现状 |
| --- | --- |
| `Button`/`IconButton` | 15 处手写 `.child(...).on_mouse_down(...)`;统一 variant(Primary/Ghost/Danger)+ hover/active/disabled |
| `Dropdown`/`ContextMenu` | model/mode/provider/session 四组复制菜单(约 300 行重复);改 `anchored()`+`deferred()`,滚轮无穿透 |
| `Tooltip`/`Badge`/`Label` | 散置 span 样式统一 |
| `Panel`/`Titlebar`/`StatusBar` | 三栏骨架抽壳;窗口拖拽区语义保留 |
| `ListRow`(TaskRow/ProjectHeader/SessionRow) | 左栏行型统一 |
| `TimelineEntryView` | 事件行渲染族(user/assistant/tool/approval/diff)拆出 mod.rs |
| `ApprovalCard`/`DiffView`/`InputArea` | 既有实现拆文件 + token 化 |
| `FollowScroll` | 跟随滚动/回底逻辑封装(现内联在 Timeline) |

3. **Timeline 虚拟化**:变高行改 gpui `list()`(measure/cache)替换 `uniform_list`;长会话(千级事件)滚动流畅性目标写入验收;长标题 truncate(F44 遗留)。
4. **`ui/mod.rs` 瘦身**:目标 <900 行(布局组合 + 路由),渲染细节全部进组件。
5. **功能收口**:K-04 Changes 面(Inspector Files/Summary 页签 + ActivityPopover,消费 `DiffListFiles/DiffGet` 与 `HunkStageService`——S12-F57 登记的零消费者服务在此接线)、K-06 `@` 引用与 Resources 面(gui-design §6.3 IA 内既有规划位)。
6. **K-03 人工验收**:IME(中文输入法候选窗)、多行粘贴、1440×1024 与 design/ 基准逐屏对照、纯键盘走查、1080×720 最小窗口。

## 2. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| A | theme.rs + 全量 token 机械替换(视觉零变化;截图对比) | apps/desktop/src/ui/ | 串行 |
| B | components/ 基础族(Button/Dropdown/Tooltip/Label/Panel/StatusBar/ListRow/FollowScroll)+ 菜单迁移 anchored/deferred(实态五组,见首部回写)+ hover/active 补齐(**先更新 design/README.md 基准**) | apps/desktop/src/ui/、design/README.md、docs/gui-design.md | 并行 ×2(基础组件 / 菜单迁移)——同文件冲突时降串行 |
| C | TimelineEntryView/ApprovalCard/DiffView/InputArea 拆分 + Timeline `list()` 虚拟化 + truncate | apps/desktop/src/ui/ | 串行(Timeline 是核心面) |
| D | K-04 Changes 面 ∥ K-06 `@`/Resources 面(协议消费面 R3 registry 已备;HunkStageService 接线含 host 侧命令) | apps/desktop、app(diff/hunk 命令面)∥ apps/desktop(resources 面) | 并行 ×2 |
| E | K-03 人工验收 + gui-design.md 收口(组件清单/S12-CR09 已修项复核:CR09-02 错误上屏、CR09-04 focus 恢复等不回退) | 人工 + docs | 串行(用户参与) |

## 3. 验证

- 波 A/B/C:`cargo check -p pawork-desktop` + 既有 UI 测试(projection 907 行测试不受影响的证明)+ `--probe-smoke` 全绿;截图对比(基准屏 1440×1024)。
- 波 D:probe 场景扩展(diff 列表/hunk stage 命令往返);Changes 面真实仓库冒烟(本仓库 dirty 状态)。
- 波 E:人工验收清单逐项签字,结果记入 gui-design.md 附录。
- 性能:千级事件会话滚动无卡顿(手测 + 帧率观察);启动时间不回退。

## 4. 退出标准

- [ ] theme + components 落地;`rgb()` 硬编码清零;mod.rs <900 行;菜单 anchored/deferred
- [ ] Timeline 虚拟化 + hover/active 全态;design 基准与实现一致
- [ ] K-04/K-06 交付(HunkStageService 有生产消费者);K-03 人工验收签字
- [ ] probe-smoke + projection 测试全绿;v3_plan §3 更新

---

## 5. 波次收口记录

### 波 A 收口(2026-08-24,glm_explorer ×3 核查 + glm_worker 单 owner 串行 + glm_reviewer)

- **theme.rs 落地**:`Theme` 六组子结构(bg 3 / surface 2 / border 2 / text 10 / accent 2 / semantic 6,共 25 色值)+ 字阶 font(11/12/13)+ metrics 14 常量;值类型 `gpui::Rgba`;`impl Global for Theme` 仅留挂载点(注释指向未来 main.rs `Application::run` 闭包,写入集外本波不动),本波访问器为静态 `dark()`;mod.rs 加私有 `mod theme;`,text_input.rs 经 `super::theme` 引用。
- **机械替换**:mod.rs + text_input.rs 消费点 `rgb(`/`rgba(`/`0x` 字面量与数字 `px(` 字面量零残留(rg 实测);95 处消费点(92 处 rgb/rgba 调用 + 审批按钮数组 3 个裸 u32:0x2f6fed/0x3d7a4a/0x8a3b32)全部走 `dark()` token;25 值与替换前字面量逐值等价(worker 等价脚本 EQUIV_FINAL_OK + reviewer 多重集核对 95↔95)。实态修正:78 行/92 调用/24+1 值(核查清单 23 值漏 0x8a3b32);禁动常量(APP_VIEW_KEYBINDINGS / MAIN_PATH_TAB_STOP_IDS / resolve_new_task_workspace)与 HEAD 逐字节一致。
- **写入集实态调整(主代理)**:波 A 验证前置修复 Desktop 真窗口启动崩溃——`controller.connect` 握手后 `ack`/`subscribe_all` 在 gpui 前台执行器(无 tokio reactor)上 await,`receive_frame` 内 `tokio::time` 直接 panic(crates/client/src/lib.rs:813,exit 134 实证,真窗口自始无法启动);修复为握手/ack/subscribe_all 全部移入 `runtime.spawn` 任务,ack 四分支语义逐字节等价,删除死方法 `record_last_acked`。写入集因此扩为 `ui/{theme,mod,text_input}.rs` + `controller.rs`。probe-smoke 走 `platform.block_on` 自带 runtime 无法暴露此路径,真窗口启动无自动门禁,已登记 ROADMAP §4。
- **验证**:`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 28/28 绿(两轮:/tmp/r8a-worker-test.log、/tmp/r8a-fix-test.log;本机默认 desktop 构建缺 Metal toolchain 走 runtime_shaders,--bins 替代本节 §3 的 `--lib --tests` 因 pawork-desktop 为 bin-only 包);`cargo check -p pawork-desktop` 同口径绿(/tmp/r8a-check.log);probe-smoke 隔离实例 r8asmoke EXIT=0(glm-4.7 completed → deepseek-v4-flash switched,cancelled=1,persisted=12,disconnect_survive=running;approval=not_requested 为模型未发起写工具调用,写入未发生,fail-closed 保持);真窗口启动实证(修复后进程存活并连接成功)。像素截图因宿主显示器休眠未取得,视觉零变化由逐值等价 + 审查枚举兜底,截图对照并入波 E K-03 人工验收(ROADMAP §4)。
- **审查**:glm_reviewer verdict=pass,无 P0–P2;P3(theme.rs 计数口径注释)同波闭环。
- **冻结面**:协议帧 golden、schemas/、共享投影 reducer、Cargo.toml/Cargo.lock 零 diff;platform.rs deny-list 不受同 crate 新模块影响。未提交。

### 波 B 收口(2026-08-24,glm_explorer ×3 核查 + glm_worker ×2 串行 + glm_reviewer 双轮)

- **基准先行**:核查证实 design/README.md 与 gui-design.md 对 hover/active 零陈述——主代理开工前更新基准:design/README.md 新增 §8(§8.1 hover/active 取值表,新增 `surface.hover #343434` / `accent.hover #3d7bf0` / `semantic.success_hover #4a8c58` / `semantic.danger_hover #9c463c` 四 token,ghost 控件与菜单行走既有 `surface.raised`,选中行不叠加 hover,active 复用 hover 色;§8.2 浮层菜单形态——`deferred(anchored())`、单开互斥、Escape/外点关闭、occlude 滚轮无穿透;§8.3 回底控件)并同步 §7 验收追溯;gui-design.md §6 补交互态/浮层菜单/回底三行原则。任务书首部实态回写:菜单实态 **5 组**(grouping/scope/model/entry fork/workspace confirm,无 provider/mode/session 菜单)、手写按钮按 on_click 调用点计 21 处。
- **轨 1 基础族**:theme.rs 25→29 色;components/{button,label,panel,status_bar,list_row,mod}.rs 落地;21 个 on_click 中 16 处非菜单调用点迁 Button(审批 map 3 实例形状保留),菜单触发器同迁;hover/active 按 §8.1 全量补齐;worker 自修复固定尺寸按钮内边距缺陷(ButtonPadding::None)。mod.rs 1990→1852。
- **轨 2 菜单迁移 + FollowScroll**:components/dropdown.rs(Dropdown + MenuPanel + MenuRow)与 follow_scroll.rs(FollowScroll + BackToBottom)落地;五组菜单全部迁 `deferred(anchored())` 浮层(workspace-confirm 维持条件打开,锚 composer label 行下方);开合状态 5 字段收敛为单一 `Option<MenuKind>`(修互斥不对称双开);Escape 由根节点 on_key_down 冒泡承接(面板 deferred 不可聚焦,组件层 on_key_down 不可达);外点 `on_mouse_down_out`;面板 `occlude()` 滚轮无穿透;FollowScroll 封装 + timeline/terminal 双接线 + 回底控件 + `follow_terminal` 重置补齐。mod.rs 1852→1907。
- **审查双轮**:首轮 changes-needed——P2-1 FollowScroll 滚轮顺序假设方向反(gpui 0.2.2 Bubble 相注册逆序分发,内部偏移应用先于用户监听,delta 投影双计;修为直读 `is_scrolled_to_bottom()`)+ P2-2 `pending_outside_close` 残留吞后续同触发器单击(修为 `(MenuKind, Point<Pixels>)` 位置匹配同一物理点击——`ClickEvent::Mouse.down` 与外点 press 系同一事件实例,精确相等)+ P3-1 `MenuPanel.dismiss_on_escape` 不可达死路径(移除)+ P3-2 model 菜单开态在 `can_switch_model` 翻假期间残留(render 归一化)。修复由主代理落地,复核 verdict=pass;P3 建议(MODULE.md 组件/宿主措辞)同波闭环。
- **验证**:`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 28/28 绿(worker 两轨各一轮 + 主代理修复后一轮);`cargo check` 同口径绿(6 条既有警告);probe-smoke 隔离实例 r8bsmoke EXIT=0(glm-4.7 completed → deepseek-v4-flash switched,cancelled=1,persisted=12,disconnect_survive=running,approval=not_requested,签名同波 A;实例数据已清理);冻结面零 diff(协议帧 golden / schemas / projection golden / Cargo 清单);禁动符号(APP_VIEW_KEYBINDINGS / install_keybindings / MAIN_PATH_TAB_STOP_IDS / resolve_new_task_workspace)零 diff;model-picker id + tab_stop + track_focus 三件套在位。mod.rs 终态 1950 行(<900 为阶段目标,拆分在波 C)。
- **登记**:菜单内 ↑/↓ 导航与 grouping/scope 触发器 tab stop 缺口留波 E 键盘走查(基准 §3.6 既有承诺);渲染面行为(菜单开合 / FollowScroll / hover)无自动门禁为既有缺口,审查建议三例人工验收(外点关闭后再点同触发器、输入框聚焦时 Escape、滚回底部重挂时机)并入波 E K-03 清单;1440×1024 截图对照留波 E K-03(ROADMAP §4 既有登记)。未提交。

### 波 C 收口(2026-08-24,glm_explorer ×3 核查 + glm_worker 单 owner 串行 + glm_reviewer)

- **基准先行**:design/README.md 新增 §8.4(Timeline 变高虚拟化形态 + 长标题 truncate 规则),gui-design.md §6 同步一行;首部波 C 实态回写已记——DiffView 无消费面留波 D、F44 不可溯源改实态登记、uniform_list 旧快照措辞不成立(实态 eager children)。
- **拆分**:mod.rs 1950→824(<900 达标);新增 timeline.rs(容器 + list() 虚拟化 + 跟随重映射)、timeline_entry.rs(五类条目 + 「···」fork 菜单)、approval_card.rs(list 末项)、input_area.rs(composer + model 菜单族 + workspace confirm 锚点原位)、inspector.rs(终端维持 ScrollHandle 现状)、task_rail.rs(装配 + grouping/scope 菜单 + truncate)。菜单语义(Option<MenuKind> 互斥、pending_outside_close 位置匹配、Escape 冒泡、model 翻假归一化)与禁动符号三件套字节等价(审查 awk+diff 实证);tooltip_text 留 crate::ui 路径(button.rs 依赖不破)。
- **虚拟化**:gpui `list()` 变高行 + ListAlignment::Bottom 钉底;timeline 变化统一 reset(new_count)(projection 替换语义下 splice 不安全),脱钩读史恢复 reset 前偏移;审批卡作末项(count=len+pending);Entry 菜单 close-on-reset;Inspector 开合经 timeline_changed() 触发 reset;条目间距 gap_1 经逐项 pt_1 等价(list 不吃 gap);跟随 = scroll handler !is_scrolled(WeakEntity 无环),回底 = scroll_to(末项底)+Bottom 自动重挂,BackToBottom 浮层复用。FollowScroll 收窄为终端专用,死方法 reset 删除(主代理,删后重跑门禁覆盖)。
- **truncate**:TaskRail 的 Task 标题与项目头 flex_1+min_w_0+.truncate()(标题行 gap_1 防省略号贴时间,短标题布局不变),为本仓 gpui TextOverflow 首个消费点。
- **验证**:`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 28/28 绿三轮(worker /tmp/r8c-worker-test.log、主代理删死方法后 /tmp/r8c-main-test.log、审查修复后 /tmp/r8c-final-test.log);check 同口径绿;probe-smoke 隔离实例 r8csmoke EXIT=0(签名同波 A/B;实例数据已清理);真窗口启动实证两轮——截图实证 Connected 态渲染(rail/空 Timeline/Composer/Inspector/状态栏布局与基准一致,/tmp/r8c-screen2.png)。冻结面零 diff(git diff 仅 ui/ + 三文档);mod.rs 824 行。
- **审查**:glm_reviewer 首轮 changes-needed——P2-1 gui-design.md 新增行丢列表标记并重复 §8.3 句(主代理修复)+ P3-1 任务书回写段落格式(修复)+ P3-3 空 Timeline 时审批卡多 4px 顶距(修复:仅 len>0 加 pt_1);P3-4 Entry 菜单滚动卸载后状态与视觉短暂失联(滚回自现,Escape/外点仍有效)登记并入波 E K-03 渲染面人工验收清单;审查对 list() 语义、禁动符号、迁移等价逐项核实通过。
- **登记**:渲染面无自动门禁为既有缺口,虚拟化后滚动/回底/菜单锚点/truncate 四例并入波 E K-03;1440×1024 截图对照留波 E;显示器休眠/App Nap 下心跳超时断连(Reconnect 横幅)为环境性既有行为,与本波无因果。未提交。
