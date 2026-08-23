# R8 — GUI 组件化与 Desktop 收口(T12)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R8 行。依据 2026-08-18 GUI 组件分析:`apps/desktop` 6,657 行、四层边界干净(ui 不碰 socket、projection 纯函数、controller 唯一写者),但 `ui/mod.rs` 1,898 行单文件承载全部渲染——15 处手写按钮、4 组复制粘贴菜单(仅数据不同)、97 处硬编码 `rgb()`、约 40 处魔法尺寸、零 hover 状态、菜单用整屏遮罩层模拟而非 gpui `anchored()/deferred()`、Timeline `uniform_list` 用于变高行(潜在裁剪 bug)。本阶段建立 theme tokens 与组件库,并收口 V2 遗留的三块 GUI 功能面与人工验收(K-03/K-04/K-06)。
>
> **实态回写(2026-08-23 波 A 开启,glm_explorer 三路核查)**:ui/ 仅 mod.rs(1,911 行)+ text_input.rs(690 行),无 pty_view.rs;硬编码色 78 处(23 个去重值,mod.rs 76 全为 `rgb(0x…)`、text_input.rs 2 含 1 处 `rgba(0x5b9dff55)`);`0x3ecf8e` 与本仓历史零命中,绿色无现存值;「手写按钮」实为样式 div + `on_click`(22 处,零 `on_mouse_down`);4 组菜单为触发按钮的 in-flow child,无遮罩层;`hover(`/`uniform_list`/`.list(` 均为 0 处(滚动走 `overflow_y_scroll`);魔法尺寸 45 处 `px()` 字面量(字号 26、图标头像 9、布局宽度 6、细部 4),圆角已是 rounded_sm/md 语义 helper。
>
> GUI 事实源:[docs/gui-design.md](../docs/gui-design.md) + [design/README.md](../design/README.md)(GUI v3 视觉基准,1440×1024)。有意的视觉变更(如补 hover)必须先更新基准再实现。

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
| B | components/ 基础族(Button/Dropdown/Tooltip/Label/Panel/StatusBar/ListRow/FollowScroll)+ 四组菜单迁移 anchored/deferred + hover/active 补齐(**先更新 design/README.md 基准**) | apps/desktop/src/ui/、design/README.md、docs/gui-design.md | 并行 ×2(基础组件 / 菜单迁移)——同文件冲突时降串行 |
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
