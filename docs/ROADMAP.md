# Pawork 计划目录

> 更新：2026-10-13。本页只维护未完成任务与环境待验的状态、链接和简介；范围、前置、实现与验收细节写在 `docs/plan/` 的独立文档。现行能力与契约以 [Spec](spec/README.md) 和 [架构](architecture.md) 为准，计划不代表已实现。

## 状态与维护

- 本页只保留未完成的任务和需要其它系统、真实账号或人工条件验证的事项。任务的完成条件全部满足且有证据后，从本页和 `docs/plan/` 同批删除；结果写进对应 Spec，历史从 Git 追溯。
- 已实现或自动 / 模拟验收通过、但真实环境未验的部分，归入下方「环境待验」，不写成真实平台通过。模拟验收不等于真人验收。
- 2026-10-13 复审：以下任务的实现、自动回归与模拟验收已对照 HEAD 源码与定向测试确认完成，任务文档已删除——CU-01～CU-21、CU-23，B1～B6、B8、B10～B23，RV-01～RV-07、RV-11，GUI2-01～GUI2-07、GUI3-01～GUI3-08、GUI4，AC-02～AC-09，ENV-04、ENV-06。它们剩下的真实环境部分并入 [ENV-02](plan/env-02-platform-matrix.md) 与 [ENV-08](plan/env-08-macos-real-system.md)；原任务文档见提交 `03da6327` 的 `docs/plan/`。

## 未完成任务

Computer use 的目标是：用户继续使用本机键鼠时，Agent 能在后台持续操作目标应用，不抢鼠标、键盘、焦点或剪贴板；用户接管同一目标时让出。未证明的路线明确拒绝，不回退本机全局输入，也不用容器结果替代本机验收。

| 状态 | 任务 | 简介 |
| --- | --- | --- |
| 未实现；当前 `UnsupportedProbe` 拒绝回归在位；缺 Windows 客户机 | [CU-22 · Windows 独立会话 / VM 后台能力](plan/cu-22-windows-isolation.md) | 在独立 Windows 会话或 VM 内实现 UIA / 截图 / 定向输入，不使用用户主桌面前台输入。 |
| 设计与契约拒绝完成；特权组件未实现，需用户启用后安装验证 | [CU-24 · macOS 锁屏使用独立设计与验证](plan/cu-24-macos-locked-use.md) | 锁屏下的受限后台操作，不把普通 AX 后台操作等同锁屏支持。 |
| 设计完成；采集与模型音频未实现 | [CU-25 · 显式授权的目标音频采集](plan/cu-25-audio.md) | 目标音频录制或模型使用链路，并明确二者的能力边界。 |
| 进行中；guest 侧 seat 客户端与身份层已落地并经本机 headless Sway 真实 smoke；Host 接线（guest 协议、`WaylandSurface`、截图租约点击 / 按键）已实现并通过离线替身回归，无真实环境证据；工具 / App 接线与专用容器验收未完成 | [CU-26 · Wayland 隔离后台生产后端](plan/cu-26-wayland-production.md) | 把 Sway / wlroots 专用 seat 的截图、指针与键盘接入正式 Host 后端。 |

## 环境待验

前置条件具备后验证真实效果；mock、模拟窗口或本机一般测试不能替代对应环境。

| 状态 | 任务 | 简介 |
| --- | --- | --- |
| 当前账号核实完成；剩余真实条件待验 | [ENV-01 · 真实账号与额度环境验收](plan/env-01-accounts-quota.md) | OAuth 自然临期刷新、双账号切换、非零额度读数与倒计时。 |
| macOS 专项通过；Windows 原生待客户机 | [ENV-02 · Windows 原生平台专项验收](plan/env-02-platform-matrix.md) | 实例锁 `share_mode(0)`、Named Pipe 关闭唤醒与连接回收、Exec Job 内核限额。 |
| 两次 HTTP 429；成功往返待供应商 | [ENV-03 · MM-2 视频真实往返验收](plan/env-03-video-roundtrip.md) | 视频理解真实往返，不以目录成功代替。 |
| 正式入口四次 Apple Event -1712；正文成功待环境 | [ENV-05 · Chrome / Edge 只读上下文环境复验](plan/env-05-browser-context.md) | 系统 Automation 授权后读取活动标签正文，并经 Desktop 产品链复验。 |
| 三类请求均 Gateway 429；成功往返与实际费用待验 | [ENV-07 · 网关媒体供应商与账单环境验收](plan/env-07-media-billing.md) | 媒体请求成功往返与供应商权威账单。 |
| 待系统授权、真人操作与外部安装 | [ENV-08 · macOS 真实系统与人工验收汇总](plan/env-08-macos-real-system.md) | Computer use 本机后台、外部浏览器扩展、Desktop 真窗口 / IME / VoiceOver / 真实 Host 与 MoMai / YingMai 现场。 |

库激活前置、条件性工程项（RV-2026-10-A / RV-2026-10-C）和阶段外产品候选见 [backlog](spec/backlog.md)。发布与全量门禁维持另行授权（BK-RELEASE-01），不因上述收口视为完成。
