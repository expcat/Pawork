# Pawork 计划目录

> 更新：2026-10-13。本页只维护未完成任务与环境待验的状态、链接和简介；范围、前置、实现与验收细节写在 `docs/plan/` 的独立文档。现行能力与契约以 [Spec](spec/README.md) 和 [架构](architecture.md) 为准，计划不代表已实现。

## 状态与维护

- 本页只保留未完成的任务和需要其它系统、真实账号或人工条件验证的事项。任务的完成条件全部满足且有证据后，从本页和 `docs/plan/` 同批删除；结果写进对应 Spec，历史从 Git 追溯。
- 已实现或自动 / 模拟验收通过、但真实环境未验的部分，归入下方「环境待验」，不写成真实平台通过。模拟验收不等于真人验收。
- 2026-10-13 复审：以下任务的实现、自动回归与模拟验收已对照 HEAD 源码与定向测试确认完成，任务文档已删除——CU-01～CU-21、CU-23，B1～B6、B8、B10～B23，RV-01～RV-07、RV-11，GUI2-01～GUI2-07、GUI3-01～GUI3-08、GUI4，AC-02～AC-09，ENV-04、ENV-06。它们剩下的真实环境部分并入 [ENV-02](plan/env-02-platform-matrix.md) 与 [ENV-08](plan/env-08-macos-real-system.md)；原任务文档见提交 `03da6327` 的 `docs/plan/`。
- 2026-10-13 环境核查（Linux 盒，Debian 13 x86_64）：没有 Windows 客户机或镜像，没有 Docker / Podman / QEMU / libvirt（仅有 `/dev/kvm` 节点），没有音频设备，不是 macOS。CU-22 / CU-24 / CU-25 与下表 ENV 项均无法在该环境推进真实实现或验收；本轮只对齐状态并在各任务文档写明「阻塞」，没有条目因此完成或删除。

## 未完成任务

Computer use 的目标是：用户继续使用本机键鼠时，Agent 能在后台持续操作目标应用，不抢鼠标、键盘、焦点或剪贴板；用户接管同一目标时让出。未证明的路线明确拒绝，不回退本机全局输入，也不用容器结果替代本机验收。

| 状态 | 任务 | 简介 |
| --- | --- | --- |
| 未实现；仅 `UnsupportedProbe` fail-closed 拒绝回归在位；阻塞：缺独立 Windows 会话 / VM | [CU-22 · Windows 独立会话 / VM 后台能力](plan/cu-22-windows-isolation.md) | 在独立 Windows 会话或 VM 内实现 UIA / 截图 / 定向输入，不使用用户主桌面前台输入。 |
| 设计与契约拒绝回归完成；特权组件未实施；阻塞：用户未启用并裁决特权方案，安装与锁屏序列需真实 macOS | [CU-24 · macOS 锁屏使用独立设计与验证](plan/cu-24-macos-locked-use.md) | 锁屏下的受限后台操作，不把普通 AX 后台操作等同锁屏支持。 |
| 设计完成，本地可做部分已尽；采集与模型音频未实现；阻塞：用户未显式开启音频场景，采集需真实桌面环境 | [CU-25 · 显式授权的目标音频采集](plan/cu-25-audio.md) | 目标音频录制或模型使用链路，并明确二者的能力边界。 |
| 进行中；guest 侧 seat 客户端与身份层已落地并经本机 headless Sway 真实 smoke；Host 接线（guest 协议、`WaylandSurface`、截图租约点击 / 按键）与工具 / App / CLI 接线（`observed_click` / `observed_keys`、显式 `--computer-native wayland`，默认仍为 `host`）已实现并通过离线回归；本机独立 user + netns 内经 `wayland_host_probe` 的 Host 链路 smoke 已通过（非专用容器，探针 grant 代替 GUI 审批）；阻塞：专用容器无法建立，正式 `pawork --computer-native wayland` + GUI 审批 + 真实 Provider 的验收未做 | [CU-26 · Wayland 隔离后台生产后端](plan/cu-26-wayland-production.md) | 把 Sway / wlroots 专用 seat 的截图、指针与键盘接入正式 Host 后端。 |

## 环境待验

前置条件具备后验证真实效果；mock、模拟窗口或本机一般测试不能替代对应环境。

| 状态 | 任务 | 简介 |
| --- | --- | --- |
| 额度查询与拒绝路径已核实（三窗 forbidden）；刷新 / 双账号 / 非零读数待真实账号条件 | [ENV-01 · 真实账号与额度环境验收](plan/env-01-accounts-quota.md) | OAuth 自然临期刷新、双账号切换、非零额度读数与倒计时。 |
| macOS 部分已真机通过；Windows 四项未验，阻塞：缺 Windows 客户机 | [ENV-02 · Windows 原生平台专项验收](plan/env-02-platform-matrix.md) | 实例锁 `share_mode(0)`、Named Pipe 关闭唤醒与连接回收、Exec Job 内核限额。 |
| 两次 HTTP 429；成功往返未验，阻塞：供应商限流 | [ENV-03 · MM-2 视频真实往返验收](plan/env-03-video-roundtrip.md) | 视频理解真实往返，不以目录成功代替。 |
| 临时 example 调正式函数四次 Apple Event -1712；Pawork.app 正文读取未测，阻塞：需 macOS Automation 授权 | [ENV-05 · Chrome / Edge 只读上下文环境复验](plan/env-05-browser-context.md) | 系统 Automation 授权后读取活动标签正文，并经 Desktop 产品链复验。 |
| 三类请求均 Gateway 429；成功往返与实际费用未验，阻塞：供应商限流、无权威账单来源 | [ENV-07 · 网关媒体供应商与账单环境验收](plan/env-07-media-billing.md) | 媒体请求成功往返与供应商权威账单。 |
| 全部未验，阻塞：需真实 macOS、系统授权、真人操作与外部安装 | [ENV-08 · macOS 真实系统与人工验收汇总](plan/env-08-macos-real-system.md) | Computer use 本机后台、外部浏览器扩展、Desktop 真窗口 / IME / VoiceOver / 真实 Host 与 MoMai / YingMai 现场。 |

库激活前置、条件性工程项（RV-2026-10-A / RV-2026-10-C）和阶段外产品候选见 [backlog](spec/backlog.md)。发布与全量门禁维持另行授权（BK-RELEASE-01），不因上述收口视为完成。
