# CU-26 · Wayland 隔离后台生产后端

> 状态：未开始；CU-23 可行性已闭合，生产实现待 Linux / Wayland 专用环境。更新：2026-10-13。

[返回 ROADMAP](../ROADMAP.md#未完成任务)

## 触发条件

确认目标 compositor / 版本与专用虚拟桌面环境，且用户选择推进 Wayland 生产支持。

## 目标

在受控 guest 内提供不碰用户主 seat 的 Wayland 后台截图与输入，接入既有 computer-use 审批、占用、取消与观测链。

## CU-23 可行性结论（2026-10-12，独立复审 PASS）

- 环境：Debian 12 / Linux aarch64，Sway 1.7、wlroots 0.15.1、headless backend；专用容器只读 rootfs、非特权、`CapDrop=ALL`、network none，无宿主 bind、设备或端口映射，无 Xwayland / X11 socket / `/dev/input`。
- 截图用 `zwlr_screencopy_manager_v1`，点击用 `zwlr_virtual_pointer_manager_v1`，键盘用 `zwp_virtual_keyboard_manager_v1`；三者在专用 seat 上均有原始事件与整帧像素证据，无 Wayland display 时正确拒绝。
- 这些协议的权限来自 socket 可达性，作用范围是整个 seat，不提供应用级授权或 portal 用户许可。xdg-desktop-portal 1.16.0 / portal-wlr 0.7.0 只暴露 ScreenCast，未暴露 Screenshot / RemoteDesktop，portal 流程未闭合。
- 结论只覆盖上述 compositor 路线，不推广为通用 portal 或所有 Wayland 桌面支持。

[computer-use Spec](../spec/crates/computer-use.md)、[安全规格](../spec/security.md)、[LCU 平台限制](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/README.md)。

## 实施范围

- 受控 guest 内的 Rust Wayland 协议客户端，只绑定已证实的截图与虚拟输入协议。现有 `computer_guest` 的 X11 / AT-SPI 定位与 `GuestSurface` 的 RFB 图像不能直接复用成 Wayland 实现。
- 先取得 compositor 权威的目标身份、进程与窗口代际映射，并把整个 seat 限制为 Pawork 专用；实验用的 con / app_id 不满足生产句柄契约。
- 再适配既有 `NativeSurface` / Host 范围、审批、占用、取消及一次动作后的新观测，复核撤销、接管、动作回执与后置观测。
- 不接宿主主 seat，不自动回退 XTEST 主桌面输入，不扩建 compositor 抽象层。

## 完成条件与验证

- 专用 Wayland 环境中的截图、点击与按键经正式 Host / 工具链完成，并有真实目标状态与持久观测证据。
- 句柄、审批、占用、取消、撤销与断线的拒绝边界有定向回归与真实环境证据。
- 未验证的 AT-SPI、其它应用、其它 compositor、共享用户 seat、portal 许可和真实用户持续输入保持明确限制，不宣称通用 Wayland 支持。
