# Pawork Desktop 初始视觉基准

> 状态：现行。2026-08-31 清理后，本目录只保留三张初始设计图；历史评审截图、差分图和过程证据不再作为仓库资产。
>
> 行为事实源：[GUI 设计](../docs/gui-design.md) · 当前计划：[ROADMAP](../ROADMAP.md)

## 1. 保留资产

| 状态 | 资产 | 用途 |
| --- | --- | --- |
| Timeline + Inspector | [desktop-shell-timeline-v3.png](desktop-shell-timeline-v3.png) | 三栏主工作台、对话、Changes |
| Timeline + 折叠 Inspector | [desktop-shell-timeline-collapsed-v3.png](desktop-shell-timeline-collapsed-v3.png) | Workspace 扩展与 Activity 入口 |
| Projects | [desktop-shell-projects-v3.png](desktop-shell-projects-v3.png) | 按项目组织 Task |

![Timeline 初始设计](desktop-shell-timeline-v3.png)

![Timeline 折叠初始设计](desktop-shell-timeline-collapsed-v3.png)

![Projects 初始设计](desktop-shell-projects-v3.png)

除以上三张 PNG 外，不再向本目录或 `docs/` 检入真窗口截图、遮罩、差分图、标注图和临时视觉证据。需要复验时重新从当前源码和真实状态采集，结论写入当轮报告，不把历史截图当成当前事实。

## 2. 布局合同

- 宽屏为三栏：TaskRail 约 288px、Workspace 弹性伸缩、Inspector 约 440px。
- `1080–1279px` 时 TaskRail 收敛到 240px，Inspector 默认折叠；主操作不得被裁切或遮挡。
- Workspace Header 常驻；Timeline 从 Header 下开始阅读，短会话不沉到窗口底部。
- Composer 常态总高 88–94px，Send/Cancel 使用单一主操作槽；RunStatusBar 高 24px。
- Inspector 提供 Changes、Terminal、Resources；折叠后 Workspace 扩展，Activity 入口位于 Workspace Header 右侧。
- 采用深色桌面工作台语言、8px 间距节奏和克制的 surface 层级。生产色值与尺寸以 `apps/desktop/src/ui/theme.rs` 为事实源，设计图不反向覆盖已验证的可访问性约束。

## 3. 交互与诚实性

- `Timeline / Projects` 是分组方式，`All projects / <project>` 是项目范围，两者正交。
- 项目范围菜单必须提供 `Add project…`，通过系统目录选择器把真实目录交给 Host；不得用 fixture 或预置项目冒充添加成功。
- 新 Task 绑定当前项目；无项目时明确要求先选择或添加项目。
- Timeline 只展示真实 Session / Run / Tool 事件。Provider、quota、Agent 或工具能力不可用时显示 unavailable、禁用或隐藏，不补假数据。
- Changes 只展示 Host 权威 Git 状态与 diff；Terminal 只展示真实 PTY 输出，纯文本视图必须过滤 ANSI/VT 控制序列。
- 菜单支持方向键、Enter 与 Escape；主路径控件需要稳定 identifier、role、name、value、enabled、focused 与 selected 状态。

## 4. 当前验收口径

1. 使用 `./scripts/pawork-desktop.sh start` 构建并启动正式 Host/Desktop；不加载 fixture、seed、probe 或测试 profile。
2. 在真实窗口完成添加项目、新建对话、发送消息、写文件、查看 Changes 和操作 Terminal。
3. 用磁盘文件、`git status` / diff 与终端 stdout 交叉核对 UI，不用截图单独证明功能正确。
4. 对照三张初始设计图检查信息架构、层级、密度和主操作可达性；动态内容不同不构成通过或失败的唯一依据。
5. 自动检查、真窗口验收、人工视觉签字和发布状态分别记录，不能互相替代。

## 5. 非目标

- 不把 Desktop 改成 WebView、IDE 或多 Agent 控制中心。
- 不为匹配设计图写入演示数据、假 quota、假 diff、假 Agent 或不可用按钮。
- 不在视觉修复中演进 GUI wire、绕过 Workspace/Policy/Sandbox，或让 Desktop 直连 Core 服务。
- 历史 R/Wave 编号与过程证据仅从 Git 历史或 [docs/history.md](../docs/history.md) 检索，不恢复为当前计划。
