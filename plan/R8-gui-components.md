# R8 — GUI 组件化与 Desktop 收口(T12)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R8 行。依据 2026-08-18 GUI 组件分析:`apps/desktop` 6,657 行、四层边界干净(ui 不碰 socket、projection 纯函数、controller 唯一写者),但 `ui/mod.rs` 1,898 行单文件承载全部渲染——15 处手写按钮、4 组复制粘贴菜单(仅数据不同)、97 处硬编码 `rgb()`、约 40 处魔法尺寸、零 hover 状态、菜单用整屏遮罩层模拟而非 gpui `anchored()/deferred()`、Timeline `uniform_list` 用于变高行(潜在裁剪 bug)。本阶段建立 theme tokens 与组件库,并收口 V2 遗留的三块 GUI 功能面与人工验收(K-03/K-04/K-06)。
>
> GUI 事实源:[docs/gui-design.md](../docs/gui-design.md) + [design/README.md](../design/README.md)(GUI v3 视觉基准,1440×1024)。有意的视觉变更(如补 hover)必须先更新基准再实现。

## 1. 目标设计

1. **`ui/theme.rs`**:`Theme` 结构(bg/surface/border/text/accent/semantic 六组,~20 token)+ 字阶/间距/圆角常量;97 处 `rgb(0x…)` 与跨文件魔法值(如 `0x3ecf8e` 在 mod.rs/pty_view 双处)全部收编;深色单主题起步,不做运行时切换(结构上留 Global 挂载点)。
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
