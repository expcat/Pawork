# ENV-08 · macOS 真实系统与人工验收汇总

> 状态：待真实系统、人工与外部安装条件。来源任务的实现、自动回归与模拟验收已于 2026-10-13 复审确认完成并删除任务文档；本页只保留它们无法在模拟中证明的真实环境部分。更新：2026-10-13。

[返回 ROADMAP](../ROADMAP.md#环境待验)

## 口径

- 使用当前构建的 Pawork.app 真窗口与真实 Host。按 [AGENTS.md](../../AGENTS.md) 经验，GUI 用 `open -na <bundle> --args --instance <name>` 启动；与运行中窗口并行验收时复制平行 bundle。
- 需要真实 Provider 时按 [验证规格 §2.1](../spec/verification.md#21-功能测试用模型) 使用 `glm-coding / glm-5.3` 或 `xai / grok-4.7` 的当次参数，不写持久默认。
- Screen Recording、Accessibility、Automation、扩展安装等系统动作由用户手势完成，不代用户批准。
- 截图不入库；证据注明日期、环境、输入、实际结果与位置。真实结论须同时有真窗口状态与源码外事实（文件、Git、Host 或 PTY 输出）。
- 任一项通过后从本页删除；全部完成后删除本页及 ROADMAP 条目。

来源任务原文见提交 `03da6327` 的 `docs/plan/`（编号即文件名前缀，如 `cu-04-*`、`rv-01-*`、`accept-*`）。

## 1. Computer use 本机后台

| 范围 | 来源 | 待验内容 |
| --- | --- | --- |
| 系统权限 | CU-01～CU-08、CU-16、CU-18 | Screen Recording / Accessibility 真实授予与撤销（含 denied 态）下的发现、窗口截图、读树、语义动作、定向输入与窗口移动；Pawork 目标批准不代替系统批准。 |
| 用户并发与接管 | CU-01、CU-07、CU-08、CU-09、CU-20 | 真人在另一应用连续键鼠时，目标操作不抢焦点、不混入文本、不带动用户鼠标（含窗口移动）；真实前台切回目标或用户接管时及时让出。 |
| 真实像素与 AX | CU-03～CU-06 | ScreenCaptureKit 窗口截图（遮挡 / 最小化 / 副屏）、AppKit / Electron AX 树与语义动作效果的现场复验；启动不激活与幽灵窗口 fail-closed。 |
| GUI 闭环 | CU-09、CU-17～CU-19 | GPUI 真窗中选择真实 App / 网站目标并授权且不激活目标；查看前后证据、停止与接管；隐藏、切任务或关窗不取消已进入 Core 的 Run。 |
| 真实 Provider | CU-10～CU-12、CU-16、CU-19、CU-20 | 动作 → 新观测 → 持久化端到端成功作答；不支持图像的模型在推理请求前被拒、视觉模型仍暴露工具；多步截图请求的旧图预算。 |
| 共享 Host CLI | CU-10 | `--gui-host` 的真实 TTY 与 Unix socket（现有回归走 MemoryTransport）。 |
| 隔离 Xvnc / RFB | CU-10 | 专用桌面容器上「输入已派发、后置截图失败仍保留事实」。 |

## 2. 外部 Chrome / Edge 扩展（CU-15）

实现与 fake extension 模拟验收已完成，桥为 `pawork_browser_bridge`，扩展在 `crates/browser/extension/`，安装步骤见其 [README](../../crates/browser/extension/README.md)。

1. 用户在真实 Chrome / Edge 确认加载未打包扩展并启用后台控制，NativeMessagingHosts 拉起桥。
2. 走完 tabs → new_tab → navigate → read → type → click → screenshot → close_tab / close；用户标签保留、跨源拒绝、断线后旧句柄拒绝。
3. 用户在其它应用连续输入不中断；真实截图像素与 `sw.js` 激活 / 导航竞态；Edge 连通 smoke。

只读上下文读取另见 [ENV-05](env-05-browser-context.md)。

## 3. Desktop 真窗口与系统交互

| 范围 | 来源 | 待验内容 |
| --- | --- | --- |
| 附件 | RV-01～RV-03、B21 | 系统文件选择器选取图片 / 文本附件；鼠标点击预览 / 移除与 Escape 关闭；22px 缩略图像素；VoiceOver 朗读附件与错误；真实 Host 下的附件头格式与语言。 |
| 子代理 | RV-04～RV-07、RV-11 | 真实 Host 现场工具重放与回执；Markdown 回执可读性；系统缩放下 chip 换行像素；原生 AX bridge / VoiceOver 朗读正文与焦点；真实网络断连后重连保留内容。 |
| 工作台与对话 | GUI2-01～GUI2-05、GUI2-07、GUI3-01、GUI3-03、GUI3-04、GUI3-06、GUI3-07 | 真实 Host 连接状态与长历史分页；系统 Cmd+K / Cmd+F 与悬停像素；代码块链接在系统浏览器打开、系统剪贴板复制；失败卡下一步的真实 Host / Git 回执；侧栏改名 / 归档的 Host 持久回执；重连后焦点。 |
| 工作面板 | GUI2-05、GUI4 | Changes / Terminal / Resources / Browser 面板的真实 PTY、浏览器页面与文件系统事实；终端系统 IME、原生剪贴板与真窗口像素。 |
| 设置与模型 | GUI2-06、GUI3-02、AC-02、AC-06 | 真实权限写盘、供应商认证 / 额度显示；模型目录开关写入真实 Global 配置；系统 IME 换行后组合输入；真实 Host 建会话、RunStart 与取消回执；滚轮与原生 AX。 |
| 视觉与动效 | GUI2-01、GUI3-05、GUI3-08、RV-06 | 三档字号与系统缩放下的真窗口像素；Ghost 按钮 hover / pressed 色差；Switch / Inspector 动效在系统显示调度下的观感。 |
| Plan 窗口 | AC-03 | NSEvent monitor 先于 GPUI 的裸 Tab、traffic-light 关闭与系统自动焦点回返；生产 Host 的 Plan 状态机与只读拒绝回执（现有回归用模拟 Host）。 |
| 任务消耗 | AC-05、GUI4 | 统计窗口原生标题、AX、像素与系统剪贴板；主窗口改语言后已开子窗重绘；真实供应商用量与扣费显示。 |
| 技能录制 | AC-04 | Desktop 录制按钮与原工具实际执行；真实 Provider 下后续 Run 发现并使用录制技能。 |

## 4. 外部消费者现场（AC-07～AC-09）

Pawork 侧网关鉴权、账本隔离与三维用量查询已由仓库回归覆盖；消费者现场在 MoMai / YingMai 仓库与已安装应用中进行。

1. **MoMai**：已安装 MoMai.app 重启后经正式设置使用 Gateway 凭证（下次 OS 钥匙串授权）；四类创作界面与任务消耗页的物理键鼠、AX 与像素。
2. **YingMai**：已安装包的 Native 窗口、物理键鼠与 AX；E2 Headless 渲染不升格为 Native 像素。
3. 两者的真实供应商用量与费用依赖 [ENV-07](env-07-media-billing.md) 的供应商条件。
