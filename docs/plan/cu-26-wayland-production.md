# CU-26 · Wayland 隔离后台生产后端

> 状态：进行中。切片 1（guest 侧专用 seat 客户端与 compositor 权威身份）已落地：有离线定向回归，并在本机 headless Sway 1.10.1 做了真实 smoke。切片 2（guest 协议、Host `NativeSurface` 适配、审批 / 占用 / 取消链接线与专用容器部署）未实现。更新：2026-10-13。

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

## 切片 1 记录（2026-10-13）

已实现（`crates/computer-use/src/wayland/`，仅 Linux 编译，未接入 NativeHost）：

- 纯 Rust Wayland wire 客户端：只靠 std 加 workspace 已锁定的 `libc`，用于 SCM_RIGHTS、memfd 和 SO_PEERCRED，不引入 libwayland 或 wayland-rs。有界帧（≤4096 字节）、畸形头直接报错不重同步，`wl_display.error` 原样上报。
- 全局协议协商 `select_globals`：wl_shm、wl_seat、wl_output 与三条 CU-23 协议必须各恰好一个，缺失或多于一个即拒绝，且拒绝发生在任何 bind / 设备创建之前。输出必须有当前 mode，scale 为 1、transform 为 normal，否则拒绝，保证坐标 1:1。
- 截图 `WaylandSeat::capture`：screencopy 整输出单帧，写入新 memfd shm buffer；只接受 ARGB / XRGB / ABGR / XBGR8888，尺寸沿用单边 ≤4096、总像素 ≤8M，`failed` 即错误，没有残帧路径。JPEG 预算与隔离桌面同口径。
- 指针 `click`：先 absolute motion，再平衡的按下 / 释放；动作前取消返回 Cancelled，移动后取消返回 Partial。键盘 `key` / `type_text` 使用固定 XKB keymap（可打印 ASCII 每字符一个单层 keycode，外加 14 个命名键和 Shift / Control / Alt / Super），整段文本先校验再发送，越表字符零事件拒绝。修饰键经 `modifiers` 请求设置并在按键后清零。每次输入以 `wl_display.sync` 往返作为「已送达 seat」回执；这不是目标效果证明。
- Sway IPC `SwayIpc`：compositor 实例 = SO_PEERCRED pid + `/proc/<pid>/stat` 启动时间。窗口取自 `get_tree` 的 xdg_shell toplevel（pid 来自 compositor 的客户端凭证，app_id 必填）；Xwayland 窗口按非专用拒绝。`WindowLedger` 以 (compositor 实例, con id) 记代际：消失即遗忘，compositor 重启清空。`require_dedicated` 要求恰好一个 `HEADLESS-*` 输出、一个 seat，输入设备只能是 `0:0:wlr_virtual_pointer_v1` / `0:0:wlr_virtual_keyboard_v1`。
- 人工验收入口 `examples/wayland_seat_probe.rs`（`windows` / `smoke`），不是产品路径。

本机真实 smoke（2026-10-13，x86_64 Linux 6.12，Sway 1.10.1 headless + pixman，唯一输出 HEADLESS-1 640×480，`WLR_LIBINPUT_NO_DEVICES=1`，无 Xwayland；目标为 foot，`app_id=cu26target`）：

| 操作 | 外部事实 | 结果 |
| --- | --- | --- |
| 专用 seat 校验（设备创建后） | `dedicated_inputs` 实际为两条 wlr 虚拟设备 identifier | 通过 |
| 截图 | 640×480 整帧，JPEG 目视为 foot 窗口 | 通过；headless 软件光标会出现在帧内 |
| 点击窗口中心 (320,252) | 目标终端开启 X10 鼠标上报，`click.bin` = `27 91 77 32 86 50 27 91 77 35 86 50`（左键按下 / 释放，列 54 行 18） | 通过 |
| 输入 `Pawork CU-26 {ok}` + Return | 目标 shell 写出 `typed.txt` = `Pawork CU-26 {ok}`；另一轮 `hello-cu26 A~!` 同样精确 | 通过 |
| 未设置 `WAYLAND_DISPLAY` | 探针 stage=connect | exit 1，正确拒绝 |
| `WLR_HEADLESS_OUTPUTS=2` | sway 报 HEADLESS-1 / HEADLESS-2 | exit 1，`more than one wl_output`，连接阶段拒绝 |

证据位于本机 `/tmp/cu26/`（`out/smoke.json`、`click.bin`、`typed.txt`、`before.jpg` / `after.jpg`），不检入仓库。该 smoke 是本机 headless 会话，不在 CU-23 那种只读、无网络、CapDrop=ALL 的专用容器里，也没有经过正式 Host / 工具 / 审批链，因此不满足本任务的完成条件。

## 阻塞与剩余（切片 2）

- **Host 接线未实现**：Host 运行在宿主机，seat 在 guest 容器里，需要把截图、点击、按键和窗口身份接到 `guest` 帧协议（token / 代次沿用 CU-21），再在 Host 侧实现 Wayland 版 `NativeSurface`，复用 NativeHost 的审批、占用、单次凭证、取消与动作后新观测。窗口截图裁剪需复核遮挡与几何。这部分可离线做替身回归，但「经正式 Host / 工具链完成」的验收需要下一项环境。
- **专用容器环境缺失**：本会话没有 CU-23 规格的专用 Wayland 容器（只读 rootfs、network none、CapDrop=ALL、无宿主 bind），也没有可用的 Docker；真实环境验收需要重建该容器并在其中运行 `computer_guest` 与 seat 客户端。
- **seat 级权限的固有限制**：三条协议的权限来自 socket 可达性，guest 内任何能连上 socket 的客户端（包括目标应用本身）都能截图或注入输入。`require_dedicated` 只能在调用时发现非虚拟设备或多余输出 / seat，无法阻止同 guest 内其它客户端创建虚拟设备。因此部署必须保证 guest 内只有 Pawork guest 服务和获批目标，sway 配置不带任何按键绑定。
- 撤销、接管、断线、取消的真实环境证据，以及 AT-SPI、其它应用、其它 compositor、共享用户 seat、portal 许可、非 ASCII 文本（当前 keymap 只覆盖可打印 ASCII）均未验证。

Validated: `cargo test --offline --tests -p pawork-computer-use --no-fail-fast`（lib 83 通过 / 1 失败、guest bin 2 通过；失败项为既有的 `guest::tests::bad_or_missing_token_is_rejected_and_the_connection_is_dropped`，已在暂存本切片改动后的干净 HEAD 上以相同断言复现，与 CU-26 无关）；`cargo build --offline -p pawork-computer-use --example wayland_seat_probe` 加上表中的真实 smoke。

Targeted regressions: `wayland::tests` 16 项，覆盖 wire 编解码与畸形头、SCM_RIGHTS 传 fd、全局协议缺失 / 重复拒绝（替身 compositor 证明拒绝前零 bind）、compositor 错误上报、无 display 拒绝、buffer 边界与通道顺序、JPEG 预算、文本先校验、keymap 完整唯一、Sway IPC 帧、窗口身份与 Xwayland 拒绝、专用 seat 拒绝、`/proc` 启动时间解析、代际账本。

Full workspace gate: NOT RUN（当前未设置全量门禁）。
