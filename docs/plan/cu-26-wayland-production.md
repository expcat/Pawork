# CU-26 · Wayland 隔离后台生产后端

> 状态：进行中。切片 1（guest 侧专用 seat 客户端与 compositor 权威身份）已落地：有离线定向回归，并在本机 headless Sway 1.10.1 做了真实 smoke。切片 2 的代码已实现、离线替身回归已通过：guest 帧协议加了 capture / input，`computer_guest --wayland` 提供服务，Host 侧新增 `WaylandSurface` 与按截图租约派发的点击 / 按键。切片 3 已接线、离线回归已通过：`computer_native` 工具暴露 `observed_click` / `observed_keys`，App 新增 `ComputerNativeMode::Wayland` 并选用 `NativeHost::wayland()`，CLI 接受 `--computer-native wayland`；默认入口未改，仍是 `host`。本机经 Host 链路的 smoke 已在独立 user + network namespace 内通过（`wayland_host_probe` → `NativeHost::wayland()` → 5907 → `computer_guest --wayland` → headless Sway，见下文「本机 Host 链路 smoke 复试」）。同日又在同类 netns 内跑通了 `pawork gui serve` 同款装配：`ComputerNativeMode::Wayland` + `GuiApprovalHost` + token 认证的本地 socket；`computer_native` 的每次调用和目标批准都经 GUI Connection Protocol 完成，持久 ObservationList / ObservationImageRead 可读（见下文「GUI Host 装配链路验收」）。这一轮的 Provider 是脚本替身，没有起 `pawork` 二进制进程，也没有 GPUI 真窗口。2026-10-14 已补上二进制 Host、GPUI 真窗口审批与真实 Provider `glm-coding / glm-5.3-flash`（见「二进制 Host + GPUI 真窗口验收」）。2026-10-15 在同类 netns 里用 `wayland_host_probe --boundaries` 补了撤销、派发前取消、断线与 scope 让出的真实 Sway 证据（见「撤销 / 取消 / 断线 / 让出」）。2026-10-15 已在 rootless podman 里跑通最小专用容器验收，同日用同一镜像复跑仍通过（见「专用容器最小验收」）。当时 CU-23 的无宿主 bind 还没有做。2026-10-15 又在同类 netns 里用 `wayland_host_probe --partial-receipt` 打出真实 seat 上的 Partial 与回执丢失（见「Partial 与回执丢失」）。同日复核打出点击中途 Partial，并用真实 Sway 复验了空闲后的 resolve 重拨（见「点击中途 Partial 与 resolve 复验」）。同日稍后在该专用容器 guest 上重跑了容器外二进制 Host、GPUI 真窗口审批与真实 Provider（见「专用容器上的 GUI 审批与真实 Provider」）。2026-10-15 稍后把拨号进程放进同一容器，guest 控制通道不再挂宿主目录（见「无宿主 bind」）。同日再后在这个无 bind 形态上重跑了 GPUI 真窗口审批与真实 Provider（见「无 bind 形态上的 GPUI 与真实 Provider」）。guest 控制通道仍无宿主目录。2026-10-15 再把产品 Host 放进这个无 bind 容器的 mount namespace（见「无 bind 容器 mount namespace 里的 GUI Host」）：与 guest 同一 `mnt` / `net`，五项能力集为 0。`DISPLAY=:9` 的 GPUI 真窗口用 X11 指针完成审批，真实 Provider 跑通整条工具链。2026-10-15 再把同一 Host 留在 guest 的 mntns 与 netns、五项能力集为 0，但不进入 guest pid namespace（见「不共享 guest pid namespace 的 GUI Host」）。guest 同 uid 扫 `/proc` 读不到 Host environ 里的 Provider key。GUI 协议客户端审批跑通 discover → request → bind → screenshot → `observed_click` → 文本 → Return。CU-26 仍是进行中：这一刀没有重开 GPUI 真窗口；tap0 例外、撤销 / Partial、观测列表面板仍未在这个形态重做；splice 仍是用户命名空间内 uid 0。更新：2026-10-15。

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

## 切片 2 记录（2026-10-13，Host 接线）

已实现（代码 + 离线替身回归；未经任何真实环境验收）：

- **guest 帧协议**（`src/guest.rs`）：在 CU-21 的 token 握手、有界 JSON 帧与连接代次之上新增 `capture` / `input` 两个 op。`CaptureQuery` / `GuestCapture` 中 JPEG 以 base64 传输，512KiB 上限的图编码后仍在 1MiB 帧内。`InputQuery` 携带 `SeatInput`（`click` 窗口本地像素 / `text` / `key`），回执为 `InputReceipt { steps_sent, steps_total, complete }`。`GuestApp` 的两个新方法默认返回 Unavailable，X11 Firefox guest 行为不变。畸形 query 按 Protocol 断开连接。`GuestLink::input_stopping` 与 press 走同一条回执丢失路径：请求写完后回执丢失或不可读，按「可能已送达」处理。Wayland guest 使用独立固定端口 `127.0.0.1:5907`（`WAYLAND_ENDPOINT`），bundle 统一为 `guest.wayland.<app_id>`。
- **guest 服务**（`src/wayland/guest.rs`，`computer_guest --wayland`，容器内监听 `0.0.0.0:5907`）：每次操作先 `require_dedicated`，再 `get_tree` 加 `WindowLedger` 解析窗口。返回给 Host 的代际把 ledger 代际、compositor 实例、连接 epoch 与进程 nonce 混合在一起，compositor 重启、重连或 guest 重启后旧值都不再匹配。Sway `rect` 是 layout 坐标，减去唯一输出原点后得到输出像素；Sway 输出尺寸与 screencopy 尺寸不一致时拒绝。目标不可见、超出输出或与其它可见窗口重叠时，截图和输入都拒绝（tiling 不会重叠，出现重叠说明有浮动 / 全屏窗口，层序未证明）。截图在帧前后各解析一次，几何或代际变化即拒绝，然后从整输出帧裁出窗口区域再做 JPEG。点击把窗口本地点加上窗口原点；文本和按键要求 compositor 焦点就在目标窗口，guest 自己从不移动焦点。动作前若 Host 已断开则不发任何事件，序列中途断开返回 partial 回执；seat I/O 失败报「action receipt lost」，并丢弃会话以便下次重连。
- **Host 侧 `WaylandSurface`**（`src/wayland_surface.rs`，全平台编译，`NativeHost::wayland()` / `shared_wayland_host()`）：发现、解析与存活检查只读元数据。截图在 guest 回包前后各复核一次元数据：窗口身份、可见、无重叠，回包的身份与几何必须等于前置值，后置值必须不变，否则按 WindowReplaced / ProbeUnavailable 拒绝。JPEG 需能解码、尺寸与回报一致、在 1280 边长与 512KiB 预算内。能力闸只放行 CaptureWindow / Pointer / TargetedKeys；读树、语义动作、元素定向输入与窗口移动一律 BackgroundUnsupported，启动由 guest 负责。
- **NativeHost 接线**（`src/native_host.rs`、`src/target/registry.rs`）：新增 `NativeRequest::ObservedClick { observation, x, y }` 与 `ObservedKeys { observation, input }`。链路与窗口移动一致：先在锁内做账本检查和能力闸，`admit_observed` 在花费任何租约 / 授权之前做载荷闸（默认拒绝，所以 macOS / X11 表面在消费前就拒绝），再在锁外读事实。`consume_observation_dispatch_with` 消费截图租约（树观测拒绝），并在花费前先完成坐标映射与越界判断，越界不烧租约。随后铸造并 armed 单次凭证，派发期间监视撤销、关闭与取消，动作后取新观测。结果为 `ObservedOutcome::{Dispatched, Partial, UnknownEffect}`，Partial 和 UnknownEffect 都保留派发事实、不自动重试。焦点缺失映射为 NotFocused；文本需先通过 `TargetedInput::validate`，再满足 keymap 只覆盖可打印 ASCII 的限制。
- **人工验收入口** `examples/wayland_host_probe.rs`：bind（未授权 / 授权后）→ 截图 → 中心点击 → 旧租约按键 → 文本 → Return → 撤销后截图，输出 JSON 行与 JPEG。不是产品路径；探针里的 grant 代替 GUI 审批。

本机经 Host 链路的 smoke 首轮（2026-10-13，**未跑通**，不构成证据；复试已通过，见下节）：

- 本沙箱的 x11vnc 服务占用了 `127.0.0.1:5900–5909`，固定端口 5907 已被占，guest 报 `Address already in use`，Host 侧解析如实失败（`no installed application resolves this bundle id`）。没有动这些进程。
- 改用 `unshare -rn` 在独立 user + network namespace 里同时运行 guest 与探针后，guest 成功监听 5907；但后两次启动的 headless Sway 1.10.1 没有在 `XDG_RUNTIME_DIR` 下生成 IPC socket（`SWAYSOCK` 为空），guest 无法连上 Sway，Host 同样如实失败。按本会话的环境探测限额停止排查，原因未定位。日志在本机 `/tmp/cu26s2/`，不检入仓库。

## 本机 Host 链路 smoke 复试（2026-10-13，通过）

代码为 HEAD `9f9ef19e`，源码与端口都没有改动。5907 是常量 `WAYLAND_GUEST_PORT` / `WAYLAND_ENDPOINT`，不可配置；独立 netns 内端口空闲，所以不需要改。环境同切片 1：x86_64 Linux 6.12，Sway 1.10.1 headless + pixman，唯一输出 HEADLESS-1 640×480，`WLR_LIBINPUT_NO_DEVICES=1`，`xwayland disable`；目标为 foot，`app_id=cu26target`，目标脚本开启 X10 鼠标上报，读 12 字节点击后再读一行文本写入 `typed.txt`。

命令：`timeout 150 unshare -rn /tmp/cu26s3/smoke.sh`。该脚本在同一个独立 user + network namespace（namespace 内 uid 映射为 0）里依次执行：拉起 lo；后台启动 headless Sway（`XDG_RUNTIME_DIR=/tmp/cu26s3/run`），轮询等到 `sway-ipc.0.<pid>.sock` 与 `wayland-1` 出现；用每轮随机生成、不写日志的 token 启动 `target/debug/computer_guest --wayland`，确认 netns 内 `0.0.0.0:5907` 由 computer_guest 监听；运行 `target/debug/examples/wayland_host_probe guest.wayland.cu26target /tmp/cu26s3/out 'Pawork CU-26 host {ok}'`；最后用 `swaymsg` 读 tree / inputs / outputs，再结束 guest 与 Sway。首轮的 Sway 是在宿主 namespace 里由单独一次 shell 调用以 `nohup … &` 启动的；本轮三者放在同一个前台脚本里，两轮 IPC socket 都在首次轮询时就已出现。首轮没有生成 socket 的确切原因仍未定位。宿主 netns 中 x11vnc（display `:7`，pid 66283）在 5907 上的监听前后不变，没有动它。

两轮（run1 / run2）结果逐阶段一致：

| 阶段 | Host 结果 | 外部事实 |
| --- | --- | --- |
| 未授权 bind | NotAuthorized | — |
| bind（探针以 ForRun grant 代替 GUI 审批） | 通过，返回窗口句柄 | Sway tree 中唯一 xdg_shell con 为 `cu26target`，pid 是 foot，处于 focused |
| 截图 | 通过，签发截图租约 | `before.jpg` 是 640×455 的窗口裁剪区（输出 640×480 减去 25px 标题栏），内容为目标就绪提示 |
| 图像中心点击 | dispatched，返回新观测 | `click.bin` = `27 91 77 32 86 50 27 91 77 35 86 50`（左键按下 / 释放，列 54 行 18，与切片 1 的中心点一致） |
| 用已花费的首个租约发文本 | StaleHandle | `typed.txt` 中没有 `stale` |
| 文本 `Pawork CU-26 host {ok}`，再按 Return | 均为 dispatched，各返回新观测 | `typed.txt` = `Pawork CU-26 host {ok}`；`after_return.jpg` 显示目标 shell 回显 `GOT:Pawork CU-26 host {ok}` |
| 撤销后截图 | NotAuthorized | — |
| 输入设备 | — | `get_inputs` 在动作前为空，动作后只有 `0:0:wlr_virtual_keyboard_v1` 与 `0:0:wlr_virtual_pointer_v1` |

负向对照：同一类 netns 里不启动 guest 时，探针两次 bind 都报 `no installed application resolves this bundle id`，exit 1，没有回退到其它表面（`/tmp/cu26s3/neg/no-guest.log`）。

证据在本机 `/tmp/cu26s3/run1/` 与 `run2/`（各含 `smoke.log`、`sway.log`、`guest.log`，以及 `out/` 下的 `probe.jsonl`、`before.jpg` / `after_click.jpg` / `after_return.jpg`、`click.bin`、`typed.txt`、`tree-before.json` / `tree-after.json`、`inputs-before.json` / `inputs-after.json`、`outputs.json`）；脚本为 `/tmp/cu26s3/{smoke.sh,sway.cfg,target.sh}`，构建日志为 `/tmp/cu26s3/build.log`。均不检入仓库。

边界：这是本机 user + netns 内经 `NativeHost::wayland()` 的 Host 链路 smoke，证明 guest 帧协议、`WaylandSurface`、截图租约、单次派发、过期租约拒绝与撤销拒绝在真实 Sway 上闭合。它**不**满足完成条件：不是 CU-23 规格的专用容器（rootfs 可写、未 CapDrop、Host 与 guest 同处一个 netns，而不是经 loopback 端口发布）；探针 grant 代替了 GUI 审批；没有经过正式 `pawork --computer-native wayland` 进程、`computer_native` 工具、真实 Provider Run 或持久 ObservationList / ObservationImageRead。取消、断线、接管与回执丢失仍只有离线替身证据。

## 切片 3 记录（2026-10-13，工具 / App / CLI 接线）

已实现（代码 + 离线回归；未经任何真实环境验收）：

- **工具**（`crates/tools/src/computer_native.rs`）：新增构造器 `NativeComputerTool::wayland(host)`，`new(host)` 不变。Wayland 实例的 descriptor 使用独立说明与 schema，动作只有 discover / request / bind / screenshot / `observed_click(observation, x, y)` / `observed_keys(observation, text | key + modifiers)`。说明里写明：这是专用 guest seat，不是本机，也不是 X11 隔离桌面；一张截图最多授权一次动作；文本或按键要求目标窗口已有 compositor 焦点；partial / unknown_effect 时不要盲目重试。解析在派发前完成，且对所有表面同一套规则：text 与 key 必须恰好一个，modifiers 只能配 key，key 和 modifier 使用既有枚举，文本先过 `TargetedInput::validate`。非 seat 表面（本机、X11 隔离桌面）的 descriptor 不列 `observed_*`；模型臆造这类调用时，Host 的 `admit_observed` 在任何派发或租约花费之前拒绝（ExecutionFailed）。Wayland 实例不占用 X11 RFB 共享桌面（`Computer::claim_shared`），两者是不同 seat；同目标的并发仍由 NativeHost 的占用与单次凭证约束。descriptor 的审批、untrusted 禁用、预算与 CU-11 视觉门控都不变。
- **App**（`crates/app/src/app_core.rs`、`extensions.rs`）：`ComputerNativeMode` 增加 `Wayland`，默认仍是 `Host`。`AppCore` 记录本次选用的表面，`select_computer_native` 在 `load_with` 中先于 `attach_workspace` 调用：Wayland 时换装进程共享的 `shared_wayland_host()`，装配 `NativeComputerTool::wayland`。工作区挂载、MCP 启动和 MCP 移除三处重建内建工具表时都沿用同一表面。不写配置，GUI wire、domain 事件与存储 schema 都不变；环境名沿用 `isolated_desktop`，`AppFamily` 仍是 `Browser`。
- **CLI**（`crates/cli/src/lib.rs`）：`--computer-native` 接受 `host` / `isolated` / `wayland`（区分大小写），默认 `host`，其它值报 Usage 错误。只覆盖本次 Host 装配；共享 `--gui-host` 的客户端沿用已连接 Host 的选择。

默认入口未改：不带参数的 `pawork` 与 GUI Host 仍选本机后台，只有显式传 `--computer-native wayland` 才会连接 guest 的 5907 端口。

切片 3 Validated:

- `bash scripts/test.sh tools app cli`（即 `cargo test --offline --tests -p pawork-tools -p pawork-app -p pawork-cli --features pawork-app/ui-fixture`）：app lib 311 通过 / 2 失败，Cargo 随即停止，未跑到 cli 和 tools。两项失败是 `gateway_tests::gateway_models_purpose_filter_and_multimodal_gates` 与 `gui_host::tests::run::run_start_with_provider_does_not_silently_keep_same_model_id`，原因是本沙箱设置了 `PAWORK_API_KEY_OPENCODE_GO`，模型目录因此混入 `opencode-go/*`，与 CU-26 无关。
- `env -u PAWORK_API_KEY_OPENCODE_GO cargo test --offline --no-fail-fast --tests -p pawork-tools -p pawork-app -p pawork-cli --features pawork-app/ui-fixture`：app lib 313 通过 / 1 ignored，`timeline_projection_host` 2 通过，`ui_fixture_projection` 1 通过，cli lib 54 通过；tools lib 98 通过 / 6 失败，全部是 `run_command::tests::*`。原因是本沙箱内 bwrap 无法创建 user namespace（`Creating new namespace failed: Resource temporarily unavailable`），此前会话已有同样记录，本切片也没有改动 `run_command.rs` / `pawork-exec`。`computer_native::tests` 20 项全部通过。
- 改动文件 `rustfmt --edition 2021 --check` 干净。

切片 3 Targeted regressions: 7 项。`pawork-tools` 5 项：`observed_actions_parse_strictly_before_any_dispatch`（缺字段、类型错误、text 与 key 同时出现、text 带 modifiers、未知键 / 修饰、空文本都报 InvalidInput）；`wayland_tool_dispatches_one_seat_action_per_screenshot`（经真实 Tool / Scheduler / `NativeHost` 与 seat 替身：descriptor 动作集与审批属性、未授权 bind 拒绝、read_tree 拒绝、越界点击不烧租约、半比例坐标映射、一张截图只放行一次动作、Once 授权只覆盖截图）；`wayland_tool_keys_follow_a_fresh_screenshot_and_revoke_stops_input`（ForRun 文本后返回新观测，跨 Run 不能花费租约，Return + control 派发，撤销后零输入）；`non_seat_surfaces_reject_observed_input_without_dispatch_or_lease_spend`（非 seat 表面不列 `observed_*`，臆造调用零派发、零后端访问）；`wayland_tool_does_not_take_the_x11_desktop_claim`（X11 桌面被其它 Run 占用时 Wayland 实例照常工作）。`pawork-app` 1 项：`wayland_computer_native_is_selected_only_explicitly`（默认 Host 不含 `observed_*`；显式 Wayland 换装进程共享的 `shared_wayland_host()`，动作集精确，MCP 移除重建工具表后仍保持）。`pawork-cli` 1 项：`computer_native_defaults_to_host_and_wayland_is_explicit`（默认 host，`wayland` / `isolated` 显式解析，`Wayland` / `x11` / 空串报 Usage）。

这些回归只替代 guest 与 seat 边界，不证明真实 Sway、专用容器、真实 Provider、GUI 真窗口审批或持久观测。

## GUI Host 装配链路验收（2026-10-13，脚本 Provider 下通过；真实 Provider 阻塞）

新增 live 测试 `crates/app/src/gui_host/tests/cu26_live.rs::cu26_wayland_gui_serve_assembly`。它默认 ignored；缺少 `PAWORK_CU26_WAYLAND=1`、`PAWORK_CU26_EVIDENCE`（证据目录）或 `PAWORK_ISOLATED_GUEST_TOKEN` 时直接失败。测试按 `crates/cli/src/gui.rs::run_gui` 的顺序装配 Host：`AppLoadOptions { instance_role: GuiHost, computer_native: Wayland, approval_mode: AskForWrites, trust_workspaces: true }` → `load_for_catalog` → `set_approval_host(GuiApprovalHost)` → `GuiHostAdapter::from_locked(..).with_connections(..)` → `TokenStore` 生成 `gui.token` → `HandshakeService(gui_supported_capabilities)` + `TokenAuthenticator` → `GuiServer` 用 `LocalTransport` 绑定 Unix socket。驱动方是 Desktop 也在用的 `pawork-client::GuiClient`，经该 socket 和 token 握手；SessionCreate、RunStart、ToolApprove、ComputerTargetList、ComputerTargetApprove、ObservationList、ObservationImageRead 全部走 wire。只有 Provider 换成脚本替身（与 cu21 同一模式），并临时登记一个声明 image_input 的 `cu26-vision` 目录条目，以满足 CU-11 视觉门控。

为什么不是 `pawork --computer-native wayland gui serve` 加 Desktop：

- 真实 Provider 不可用（见下）。正式二进制无法注入进程内脚本 Provider；把某个首方通道的 `base_url` 改指本地假服务，等于冒充真实通道，因此不采用。
- Desktop 的审批卡是 GPUI 窗口，需要 display。独立 netns 里没有 display，宿主的 x11vnc display（含占用 5907 的 `:7`）不能动，Desktop 也没有可自动化的无头审批入口。
- 其它正式 CLI 入口都做不了目标批准：注册表中 `computer_target_approve` 的 `headless` 为 None，`pawork headless --json-stdio` 会回 UnsupportedCapability；`pawork run`（chat_host）只处理工具审批，不处理目标批准。

因此，没有 GUI 窗口时唯一的正式审批通道就是 GUI Connection Protocol。测试用 Desktop 同一个客户端 crate 驱动它，服务端走的是与 `gui serve` 相同的 adapter handler。驱动方代替的是用户在审批卡上的点击：先用 `ToolStarted` 核对工具名为 `computer_native`，收到 `ToolApprovalRequired` 后发 `ToolApprove(ApproveOnce)`；模型在未获批准的 bind 被拒后停下等待，驱动方读 ComputerTargetList（即 Desktop 目标卡渲染的数据），确认该目标已登记、尚无 grant，再发 `ComputerTargetApprove(ForRun)`。这不是 GPUI 真窗口，也不是人工点击。

环境与 cu26s3 相同：独立 `unshare -rn`，Sway 1.10.1 headless + pixman，唯一输出 HEADLESS-1 640×480，`xwayland disable`；目标为 foot，`app_id=cu26target`，目标脚本开启 X10 鼠标上报，读 12 字节点击后再读一行文本写入 `typed.txt`。测试进程设置空的 `XDG_CONFIG_HOME`，并移除 `PAWORK_API_KEY_OPENCODE_GO`，不使用任何真实凭据。命令为 `TEST_BIN=target/debug/deps/pawork_app-<hash> timeout 400 unshare -rn /tmp/cu26s4/smoke.sh`，测试耗时 2.17s，结果 1 passed。

| 环节 | 结果 | 外部事实 / 持久证据 |
| --- | --- | --- |
| 错误 token 握手 | `AuthenticationFailed: invalid token` | — |
| 装配 | `core.computer_native == Wayland`，`core.native` 即 `shared_wayland_host()` | — |
| discover / request | 都成功；discover 列出 `cu26target guest.wayland.cu26target browser windows=1` | — |
| 目标批准前 bind | 工具失败：NotAuthorized | — |
| ComputerTargetList（批准前） | 目标已登记，scope 为本 Run，无 grant | — |
| ComputerTargetApprove(ForRun) | 回执 `kind=for_run`，bundle 一致 | — |
| bind → screenshot | 返回窗口句柄与截图租约，JPEG 640×455 | Sway tree 中唯一 xdg_shell con 为 `cu26target`，处于 focused |
| `observed_click` 图像中心 (320, 227.5) | dispatched，返回新观测 | `click.bin` = `27 91 77 32 86 50 27 91 77 35 86 50`（左键按下 / 释放） |
| 用已花费的首个租约发文本 | 工具失败：StaleHandle | `typed.txt` 中没有 `stale` |
| `observed_keys` 文本，再按 Return | 均为 dispatched，各返回新观测 | `typed.txt` = `Pawork CU-26 formal {ok}`；最后一张持久观测显示 `GOT:Pawork CU-26 formal {ok}` |
| GUI 工具审批 | 9 次调用各收到一次 `ToolApprovalRequired`，均经 wire 回 `ToolApprove(ApproveOnce)` | 落库 9 条 ToolApprovalRequested、9 条 ToolApprovalResponded（approved_once）、9 条 ToolExecutionCompleted、1 条 RunCompleted |
| ObservationList / ObservationImageRead | 4 条（截图 + 3 次动作后观测），均为 `computer_native` / `image/jpeg` / historical；分块读回 4 张 640×455 JPEG | 4 张持久 JPEG 的字节长度与 Run 期间返回模型的图逐一相同（未逐字节比对） |
| Run 终态 | Completed；ComputerTargetList 为空（scope 已收口） | `get_inputs` 动作前为空，动作后只有 `0:0:wlr_virtual_keyboard_v1` / `0:0:wlr_virtual_pointer_v1` |

负向对照：同类 netns 内不启动 guest 时，`request` 报 `no installed application resolves this bundle id`，Run Failed，测试 FAILED（exit 101）；不设 `PAWORK_CU26_WAYLAND` 时测试直接 panic。宿主 netns 中 x11vnc（`:7`，pid 66283）在 5907 上的监听前后一致。

证据在本机 `/tmp/cu26s4/`（`smoke.sh` / `sway.cfg` / `target.sh`、`smoke.log`、`sway.log`、`guest.log`；`out/` 下有 `test.log`、`formal.json`（装配、审批、回执、目标列表、ObservationList、落库事件计数、目标事实）、`tool-trace.json`（每轮工具参数与结果 metadata）、`observation-{51,61,81,91}.jpg`、`click.bin`、`typed.txt`、tree / inputs / outputs；`neg/` 为负向对照；`preflight/` 为 Provider 预检）。均不检入仓库。

**真实 Provider：未完成（阻塞）**。`pawork auth list` 显示 `glm-coding` 与 `xai`（OAuth）都没有凭据；没有 xai 登录时目录里也没有任何 `xai/*` 模型。本机唯一可用的凭据是环境变量里的 opencode-go key：正式 CLI `pawork --provider opencode-go --model <m> run` 对 grok-4.7、glm-5.3、glm-5.3-flash、grok-4.6、kimi-k3 都返回 `Authorization (403)`；直接请求上游时，响应体为 `An active OpenCode Go subscription is required to use Go models.`。证据见 `/tmp/cu26s4/preflight/`（`run*.err`、`chat2-*.body`、`auth-list.txt`、`models-vision.txt`、`models-xai.txt`），其中不含密钥。另有一个与凭据无关的约束：目录中 `glm-5.3` / `glm-5.2` 的 `image_input=false`，CU-11 视觉门控会对它们隐藏 `computer_native`，所以即使有凭据，`glm-coding / glm-5.3` 也驱动不了本链路；授权测试模型里只有 `grok-4.7`（目录 `image_input=true`）可行，需要 xai OAuth 登录或有效的 OpenCode Go 订阅。

本轮新发现：ComputerTargetList 给 `guest.wayland.*` 目标返回的能力矩阵不对。`handlers/computer.rs::target_capabilities` 只按 family 加 CU-21 Firefox 特例做静态推导，Wayland 目标沿用 `Browser` 族，于是显示 ax_read / ax_press 支持、targeted_keys / pointer 不支持；而 `WaylandSurface` 的实际能力闸恰好相反（放行截图、指针与定向按键，拒绝读树与语义动作）。派发不受影响，但 Desktop 目标卡会给用户展示错误的能力。本轮未修，归入下面的剩余项（2026-10-13 已在后续切片修复，见下文「ComputerTargetList 能力矩阵修复」）。

## ComputerTargetList 能力矩阵修复

`crates/app/src/gui_host/handlers/computer.rs::target_capabilities` 原先只对 `guest.browser.firefox` 有特例，现把它泛化为「guest 表面已证明动作集」：bundle_id 以 `pawork_computer_use::guest::WAYLAND_BUNDLE_PREFIX`（`guest.wayland.`）开头时，背景列只有 CaptureWindow / Pointer / TargetedKeys 为 Supported，AxRead / AxSemanticWrite / AxPress / MenuShortcut / MenubarPress / WindowMove / WindowResize 为 Unsupported，与 `WaylandSurface::admit_action` 逐项一致。minimized 列与 Firefox 特例同级，十项全部 Unsupported：表面在运行时只接受可见、无重叠的窗口，没有最小化证明。Firefox 特例与其它目标的 `background_capability` 族矩阵不变。没有新增 `AppFamily` 变体，wire、存储 schema 与 Desktop 映射都不变；派发路径本来就不读这个矩阵，行为不变。

只修了 GUI 展示，没有新的真实环境证据：没有重跑 live 验收，没有用真实 Provider、专用容器或 GPUI 真窗口复核目标卡。

矩阵修复 Validated: `cargo test --offline -p pawork-app --features pawork-app/ui-fixture --lib gui_host::handlers::computer::tests`（2 passed）；同二进制 `gui_host::tests::computer`（11 passed，含十动作矩阵长度断言）；`rustfmt --edition 2021 --check crates/app/src/gui_host/handlers/computer.rs` 干净。

矩阵修复 Targeted regressions: `wayland_guest_matrix_mirrors_seat_surface_gate`（两个 `guest.wayland.*` id 的十动作背景 / 最小化列）；`firefox_guest_and_browser_family_matrices_do_not_regress`（Firefox 特例；`org.mozilla.firefox`、`guest.waylandfoot`、`guest.browser.wayland` 走 Browser 族通用矩阵，近似前缀不误入 Wayland 分支）。

## 二进制 Host + GPUI 真窗口验收（2026-10-14）

证据目录 `/tmp/cu26s6`（不入库）。Host、`computer_guest --wayland` 与 headless Sway 1.10.1 在同一个 `unshare -rn`（user + net）里：宿主 `127.0.0.1:5907` 仍是 x11vnc display `:7`（pid 66283），netns 内 guest 另听 `0.0.0.0:5907`。`unshare -rn` 不隔离 mount，netns 外的 Desktop 连共享路径上的 Unix socket。出网用 `slirp4netns`。`PAWORK_DATA_DIR=/tmp/cu26s6/data`，`--instance cu26bin`。

| 进程 | pid | cmdline |
| --- | --- | --- |
| Host | 1841724 | `target/debug/pawork --computer-native wayland --instance cu26bin --approval-mode ask-for-writes --trust-workspaces --provider glm-coding --model glm-5.3-flash gui serve` |
| guest | 1841707 | `target/debug/computer_guest --wayland` |
| Desktop | 1841986 | `target/debug/pawork-desktop --instance cu26bin`，宿主 netns，`DISPLAY=:9`，X11 窗口 `Pawork` 1440×1024 |

`/proc` 的 netns：Host / guest / Sway 为 `net:[4026532232]`，Desktop 与 x11vnc 为宿主 `net:[4026531840]`。验收前后宿主 5907 监听都是 pid 66283。

成功 Run `run-gui-1c1a3c-18de5b4aa01fb2e3-4`（会话 `ses-1c1a3c-18de5a39153fbe3f-1` 的第二次 Run；第一次 `run-gui-1c1a3c-18de5a3c37013b1b-2` 在 Provider HTTP 超时时 `run_failed`）。`provider_request_started` 的 provider 为 `glm-coding`、model 为 `glm-5.3-flash`。7 次 `computer_native` 都有 `tool_approval_requested` / `tool_approval_responded`（`approved_once`）。目标批准是 Computer 面板上对 `guest.wayland.cu26target` 的「Allow for this run」点击（`/tmp/cu26s6/approvals.log` 02:28:37，`(1159, 311)`；`target-before-022837.png` / `target-after-022841.png`）。工具审批卡与该按钮的点击都是 `DISPLAY=:9` 上的 X11 指针事件，不是 `GuiClient` 代批。

| 动作 | 结果 | 外部事实 |
| --- | --- | --- |
| discover / request / bind | 均成功 | Sway tree 唯一 xdg_shell con 为 `cu26target`，focused，640×455 |
| screenshot | 观测 `…-2`，JPEG 640×455 | `out/observation-2.jpg`（13645 字节） |
| `observed_click` (320, 214) | `input_dispatched=true` | `out/click.bin` = `27 91 77 32 86 49 27 91 77 35 86 49` |
| `observed_keys` 文本 | dispatched | `out/typed.txt` = `Pawork CU-26 binary {ok}` |
| `observed_keys` Return | dispatched | 末张观测 `out/observation-5.jpg` 显示 `GOT:Pawork CU-26 binary {ok}` |
| Run 终态 | `run_completed`，`stop_reason.kind=completed` | `out/formal.json`；动作后 `get_inputs` 为 `wlr_virtual_keyboard_v1` / `wlr_virtual_pointer_v1`（动作前为空） |

Desktop 为通过这次验收做了一处最小改动：`apps/desktop/src/ui/mod.rs` 只在 macOS 抓取 `raw_window_handle`。gpui 0.2.2 的 X11 `window_handle` 是 `unimplemented!()`，不改则 Linux 首帧 panic。没有重跑定向测试。

未完成：专用容器仍不能建。验收当时 `WaylandSurface::resolve` 把 guest 元数据错误折叠成 None（修复见下文「resolve 误报「未安装」」）。同一 Host 进程、同一会话里，失败 Run `run-gui-1c1a3c-18de5a3c37013b1b-2` 的 `discover` 成功，`request` 成功（`requested guest.wayland.cu26target`），第一次 `bind` 为未授权（`target is not authorized…`），第二次 `bind` 才回报 `no installed application resolves this bundle id`；该 Run 随后因 `http timeout error from https://api.z.ai` 而 `run_failed`，不是这次 bind 之后立刻重试成功。成功 Run `run-gui-1c1a3c-18de5b4aa01fb2e3-4` 的七次调用没有这条「未安装」错误。更早的 attempt 2（另一 Host、会话 `ses-1c05b5-18de59375428ef5e-1`）是 `request` 报「未安装」、随后 `bind` 报未授权。Run 结束后 Computer 面板「Observation evidence」为 0 observations；四张 JPEG 是从该 Run 落库的 `tool_calls.result_json` 解出的，不是面板读回。原因与修复见下文「面板列出 0 条观测」。撤销、接管、断线、取消仍无真实环境证据。

本轮 Validated: 无新增 Cargo 测试。只读核对 `/tmp/cu26s6/proc-evidence.txt`、`desktop-proc.txt`、`out/formal.json`、`out/typed.txt`、`out/click.bin`、`out/tree-after.json` 与上表截图。

## resolve 误报「未安装」（2026-10-14）

根因是两层，都有会话时间戳和代码对应，不是目标真的没装，也不是代际或批准被写错。

guest 帧服务在 `serve_connection` 里用 `arm` 把读超时设成 `IO_BUDGET`（20 秒）。超时被 `read_exact` 收成 `GuestError::Disconnected("guest read failed")`，服务端关掉这条连接。Host 的 `WaylandSurface` 仍缓存着那条 `GuestLink`。下一次 `metadata` 在死套接字上失败，`with_link` 会丢掉缓存，但 `resolve` 当时写的是 `metadata().ok()?`，任何失败都变成 `None`。`NativeHost` 的 `request` / `bind` 把 `None` 写成 `Invalid("no installed application resolves this bundle id")`。批准检查在 `resolve` 成功之后，所以这条路径根本走不到 `NotAuthorized`。

`/tmp/cu26s6/data/cu26bin/session.db` 里，失败 Run `run-gui-1c1a3c-18de5a3c37013b1b-2` 的 discover、request、第一次 bind 相邻执行间隔约 10 秒，都过了 `resolve`（第一次 bind 是未授权）。第二次 bind 的工具调用在第一次 bind 完成后约 5.7 秒就发出，但 `tool_approval_requested` 到 `approved_once` 停了 66726 毫秒；执行时刻比上一次 guest 接触晚 72441 毫秒，回报未安装。attempt 2（`/tmp/cu26s6/attempt2/data-cu26bin/session.db`，Run `run-gui-1c05b5-18de593a6c923c32-2`）的 request 批准等待 17228 毫秒，距 discover 完成 22707 毫秒，报未安装；再过 10460 毫秒的 bind 是未授权——失败那次已经清掉死连接，下一次重新拨号，`resolve` 又成功了。空闲来自下一次 guest 往返之前的批准等待（加上模型间隔），不是目标消失。成功 Run `…-4` 的相邻工具执行间隔是 10–18 秒，都短于 20 秒，没有这条错误。

修复：`NativeSurface::resolve` 改为 `Result<Option<AppIdentity>, TargetError>`。`Ok(None)` 才是未安装（不是 `guest.wayland.` 前缀，或元数据成功但没有该 bundle）。`GuestError::Disconnected` 重拨一次；第二次仍失败，或 guest 以 `Unavailable` 回答，按 `ProbeUnavailable` 上抛。`NotAuthorized` 仍是批准闸。离线替身把缓存套接字掐断，不睡满 20 秒，覆盖空闲后的第一次 request / bind，以及元数据失败和拨号失败的错误类型。没有改 guest 的 20 秒读预算，也没有重跑真实 Sway。同一签名下，X11 `GuestSurface::resolve` 也不再把元数据失败收成 None，但没有 Wayland 这条重拨。`discover` 在自己的第一次元数据失败时仍返回空列表，不在这一刀。

## 面板列出 0 条观测（2026-10-14）

根因在 Desktop 的读取窗口，不在落库，也不是 Run 终态把观测清掉。

`/tmp/cu26s6/data/cu26bin/session.db` 会话 `ses-1c1a3c-18de5a39153fbe3f-1`，`active_branch=main`，`head_sequence=1243`。成功 Run `run-gui-1c1a3c-18de5b4aa01fb2e3-4` 的四次带图 `tool_execution_completed` 在 sequence **586 / 718 / 836 / 926**（`computer_native`，content 第二段是 `image` / `source.kind=base64` / `image/jpeg`）。Host 的 `observation_list` 只认这种 Base64 Image part。944 及之后的 300 条事件是 `assistant_text_delta`（244）、`assistant_thinking_delta`（53）、以及各一条 `usage_updated` / `message_committed` / `run_completed`，没有 `tool_execution_completed`。

Desktop 先用 `limit=1` 探 head，再读 `[head-300, head]`，也就是 `after_sequence=943`（Host 从 944 起）。这一窗的 Image part 是 0。面板因此显示「0 observations」，并因 head>300 带上「仅列出最近的观测记录」。`metadata.observation` 在这些事件里是观测 id 字符串，不是带 `image_width` 的对象，所以条目的宽高为空，但这不会把条目滤掉。脚本 Provider 的 GUI 装配能读到观测，是因为那次事件流短，四张图还在尾窗里。

修复在 `apps/desktop/src/ui/computer.rs`：尾窗已经有观测时行为不变；尾窗是 0 条且更早还有事件时，按同样的 300 条窗口向更早回填，至多 8 窗或凑满 30 条。本窗仍新增观测就再往前一窗（四张图分在 643 与 343 两窗），本窗不再新增或已经读到 sequence 1 就停。回填完成前不改已经展示的列表。没有改 wire，也没有改 Host 扫描。

Host 的 `complete` 只表示这一页已经扫到会话 head，或这一页不足 `limit` 条。回填窗正好 300 条、上沿还没到 head 时，`complete=false`，`next_sequence` 停在窗口上沿（例如 after=643 的页游标是 943）。客户端把「游标 ≥ 本窗上沿」当成该窗结束，只在游标仍低于上沿时按字节预算同窗续读。若把这个 `complete=false` 当成截断再顺着游标读，会重读更新的窗口，重复计入已经见过的图，并且把更早的截图再推迟一轮。续读的 `limit` 仍是整窗，可能越过上沿；收敛时按 `(sequence, part_index)` 去重。没有重开 GPUI 真窗口。定向回归用上述 sequence，并按 Host 的满页 `complete=false` 回放面板状态机：`cargo test -p pawork-desktop --offline --bin pawork-desktop -- observation_empty_tail_backfills_past_streaming_deltas observation_backfill_budget_continues_inside_window_and_dedups observation_backfill_stops_for_nonempty_tail_and_session_start observation_backfill_is_bounded_when_no_evidence_exists observation_tail_fetch_keeps_identity_and_follows_latest observation_image_chunks_reassemble_and_fail_closed observation_image_validation_budgets_and_full_frame_gif evidence_decode_receipt_isolated_and_loads_selection computer_panel_rejects_stale_receipts_and_resets_on_session_switch computer_panel_mark_stale_keeps_data_and_invalidates_old_responses computer_panel_finish_action_requires_matching_generation computer_action_id_roundtrip_uses_dynamic_escape computer_stop_requires_real_release_or_terminal_state computer_panel_ax_publishes_targets_and_gates_actions computer_panel_ax_publishes_observations_and_stop_gating`，15 passed / 0 failed。

## 撤销 / 取消 / 断线 / 让出（2026-10-15，netns + headless Sway）

证据目录 `/tmp/cu26-revoke/`（不入库）。外层 `/tmp/cu26-revoke/run.sh` 先记下宿主 5907，再执行 `timeout 200 unshare -rn /tmp/cu26-revoke/smoke.sh`。netns 为 `net:[4026532235]`，宿主为 `net:[4026531840]`。Sway 1.10.1 headless + pixman，唯一输出 HEADLESS-1 640×480，`xwayland disable`；目标是 foot，`app_id=cu26target`，原始输入追加到 `out/events.bin`。`computer_guest --wayland` 在 netns 内听 `0.0.0.0:5907`。探针里的 grant 代替 GUI 审批。宿主 `127.0.0.1:5907` 前后都是 x11vnc pid 1878046（display `:7`），没有动它。探针退出码 0，`out/summary.json` 的 check 全部通过。

`out/events.txt` 里实际落地的只有一次点击的两份 X10 上报（`1b 5b 4d 20 56 32` 与 `1b 5b 4d 23 56 32`，左键按下 / 释放，列 54 行 18）以及文本 `SEAT-OK`、`AFTER-CANCEL-OK`、`LIVE-AGAIN`、`REDIAL-OK`、`YIELD-OK`。被拒绝的 `REVOKE-GHOST`、`CANCEL-GHOST`、`CANCEL2-GHOST`、`INFLIGHT-GHOST-0123456789abcdef`、`DROP-GHOST`、`STALE-GHOST`、`CLOSED-GHOST` 都没有出现。动作前 `get_inputs` 为空，动作后只有 `0:0:wlr_virtual_keyboard_v1` 与 `0:0:wlr_virtual_pointer_v1`。

| 边界 | Host 结果 | 外部事实 |
| --- | --- | --- |
| 撤销 | 撤销后截图、点击、按键都是 `NotAuthorized` | 鼠标上报停在 2；`REVOKE-GHOST` 未写入 |
| 取消 | 派发前 `Cancelled`：谓词恒真（租约未消费）；第 3 次轮询时租约已消费、guest 尚未调用；第 6 次轮询仍在 guest 请求写出之前 | 三段被拒文本都未写入；取消点击不增加鼠标上报。恒真取消没有花掉租约，同一张截图随后打出 `AFTER-CANCEL-OK` |
| 断线 | 关掉探针自己连往 5907 的套接字（`closed=1`）后，尚未消费的租约按键是 `ProcessRestarted`，旧窗口截图是 `WindowReplaced`；重新 bind 后按键 `dispatched` | `DROP-GHOST` 未写入；`REDIAL-OK` 写入；旧租约再按键是 `WindowReplaced`，`STALE-GHOST` 未写入 |
| 让出 | 没有独立接管入口。`WaylandSurface::activity` 恒为 `Idle`，当时 `target_states.dispatch_paused=false`。占用中第二个 scope 的 bind 是 `TargetOccupied`，用对方窗口截图是 `CrossRun`。`close_scope` 后原 scope 按键是 `ScopeClosed`；等待方 bind 成功并打出文本 | `CLOSED-GHOST` 未写入；`YIELD-OK` 写入 |

断线后的 `ProcessRestarted` 来自死连接上的存活读取失败，按键没有进 seat；下一次截图拨上新的 guest epoch，旧窗口代际变为 `WindowReplaced`。这不是 foot 进程真的重启。本轮取消都发生在 guest 请求写出之前，所以没有打出 Partial 或 `unknown_effect`；离线替身里的 partial 与回执丢失仍然只是替身。这不是专用容器，也没有 GPUI 真窗口或真实 Provider。

## Partial 与回执丢失（2026-10-15，netns + headless Sway）

证据目录 `/tmp/cu26-partial/`（不入库）。外层 `run.sh` 先记下宿主 5907，再执行 `timeout 180 unshare -rn /tmp/cu26-partial/smoke.sh`。netns 为 `net:[4026532237]`，宿主为 `net:[4026531840]`。Sway 1.10.1 headless + pixman，唯一输出 HEADLESS-1 640×480，`xwayland disable`；目标是 foot，`app_id=cu26target`，pid 2046155，前后都 focused。终端是 raw、无回显，每个字节追加到 `out/events.bin`。`computer_guest --wayland` 在 netns 内听 `0.0.0.0:5907`。探针 grant 代替 GUI 审批。宿主 `127.0.0.1:5907` 与 `[::1]:5907` 前后都是 x11vnc pid 1878046（display `:7`），`/proc/1878046/stat` 的 starttime tick 前后都是 33215211。本刀没有向该 pid 发信号。探针退出码 0。

`type_text` 在每个字符发出之后、下一次取消检查之前，若 `PAWORK_WAYLAND_STEP_DELAY_MS` 落在 1..=1000，就等待这么多毫秒。未设置、0 或越界则等待 0。等待不增减 Wayland 事件，只是让真实 compositor 能在后续字符发出前看到前缀。本刀只给这个 netns 里的 guest 设了 200。离线 `step_delay_is_off_unless_the_probe_sets_a_bounded_value` 钉住这个边界。

探针是 `wayland_host_probe --partial-receipt`。两条文本都是先等 `events.bin` 出现前缀，再动手，所以 guest 请求已经写出，而且至少有一个字符进了 seat。

| 边界 | Host 结果 | 外部事实 |
| --- | --- | --- |
| Partial | `partial 1/11: computer action cancelled`，`input_dispatched=true`。前缀 `I` 出现后，探针只关掉自己连往 5907 的写方向（`shut_write=1`），继续读回执。动作后观测失败，原因字符串是 `target process exited or restarted` | `partial.delta` 只有 `I`。全文 `INSEATGHOST` 的 `GHOST` 没有出现。600ms 内 `events.bin` 长度不变。同一张截图再发 `RETRY1GHOST` 是 `StaleHandle`，这 11 个字节也没有写入。foot pid 仍是 2046155，没有退出；失败来自写方向关掉之后，后续 metadata 无法再证明实例，Host 按进程重启 fail-closed，partial 事实还在 |
| 回执丢失 | `unknown_effect`，`input_dispatched=true`，`observation_failure=cancelled before the post-action observation`。取消标志在 `L` 进入 `events.bin` 之后才变为真，Host 放弃回执，不再读它 | `lost.delta` 只有 `L`。全文 `LOSTSEATGHOST` 的其余字节没有出现。600ms 内长度不变。同一张截图再发 `RETRY2GHOST` 是 `StaleHandle`。guest 日志有一条 `connection closed: guest write failed`，是 Host 已经关掉套接字之后，guest 写回执失败 |
| 之后 | 重新 bind、截图，再发 `AFTEROK`，结果 `dispatched` | `events.bin` 全文是 `ILAFTEROK`。动作前 `get_inputs` 为 `[]`，动作后是 `0:0:wlr_virtual_keyboard_v1` 与 `0:0:wlr_virtual_pointer_v1` |

这不是专用容器，也没有 GPUI 真窗口或真实 Provider。这一轮没有打出「点击只移动、不按键」的 Partial；外部事实是部分按键，不是 `click.bin`。同日稍后的复核补上了点击 Partial，见「点击中途 Partial 与 resolve 复验」。CU-26 完成条件仍未满足。

## 专用容器最小验收（2026-10-15，进行中）

本盒此前文档写「没有 Docker / Podman」。2026-10-15 复看时 rootless Podman 5.4.2 已经在（`podman version`；`podman info` 为 rootless、cgroupfs、crun、overlay，GraphRoot `/home/box/.local/share/containers/storage`）。没有安装 Docker Engine。先用已有的 `docker.io/library/busybox:1.36` 确认 `podman run --rm --network=none --cap-drop=ALL --read-only --security-opt=no-new-privileges` 能打印并退出。本刀没有 `sudo apt install`，也没有动宿主 x11vnc。

部署复审：CU-23 写的是只读 rootfs、非特权、`CapDrop=ALL`、network none、无宿主 bind、无端口映射。产品代码里的 Host 固定拨 `127.0.0.1:5907`，guest 在容器内听 `0.0.0.0:5907`。这两条不能同时照搬：network none 没有可发布的接口；本盒宿主 `127.0.0.1:5907` 与 `[::1]:5907` 已被 x11vnc（display `:7`）占用，不能改成「容器把 5907 发布到宿主 loopback」。本刀选择 network none，跨出容器的唯一通道是一个 Unix socket 目录挂载。探针仍拨硬编码的 `127.0.0.1:5907`，但跑在单独的 `unshare -rn` 里，该 netns 的 5907 是转到这个 socket 的转发，不是宿主上的 x11vnc。

容器定义（构建上下文在 `/tmp/cu26-container/ctx/`，不入库）：`debian:trixie-slim` 加 sway 1.10.1、wlroots 0.18.2、foot、socat、fonts-dejavu-core，以及本机已有的 `target/debug/computer_guest`。本地标签 `localhost/pawork-cu26-wayland:local`，镜像 id `8038d17eb623`。入口在容器里起 headless Sway（`WLR_BACKENDS=headless`、`WLR_RENDERER=pixman`、`WLR_LIBINPUT_NO_DEVICES=1`、`xwayland disable`、唯一输出 HEADLESS-1 640×480），等 `app_id=cu26target` 的 foot 出现后启动 `computer_guest --wayland`，再用 socat 把容器内 `127.0.0.1:5907` 转到 `/sock/guest.sock`。运行参数：

```text
podman run -d --name cu26-wayland --init --read-only --cap-drop=ALL \
  --security-opt=no-new-privileges --network=none --user 1000:1000 \
  --userns=keep-id --memory=2g --pids-limit=512 --shm-size=256m \
  --tmpfs /tmp:rw,mode=1777 --tmpfs /run:rw,mode=755 --tmpfs /home:rw,mode=755 \
  --env-file <token-env> -v /tmp/cu26-container/sock:/sock:rw \
  localhost/pawork-cu26-wayland:local
```

`--tmpfs` 不接受 `uid=`（第一次 `podman run` 因此直接失败，容器没起来）。token 只经 env 文件进入容器，不写入本页、探针 JSON 或日志；证据文件里搜过 token 原文，没有命中。验收后 `podman stop`（退出 143）。

命令：`/tmp/cu26-container/run.sh`，其中探针是 `timeout 120 unshare -rn /tmp/cu26-container/probe-netns.sh`。探针 grant 代替 GUI 审批。容器 netns 为 `net:[4026532241]`，宿主 x11vnc 为 `net:[4026531840]`。`podman port` 为空。

| 阶段 | Host 结果 | 外部事实 |
| --- | --- | --- |
| 未授权 bind | NotAuthorized | — |
| bind（探针 ForRun grant） | 通过 | 容器内 `get_outputs` 唯一输出 HEADLESS-1 640×480；动作前 `get_inputs` 为 `[]` |
| 截图 | 通过，15866 字节 | `before.jpg` 是 foot，正文为 `CU26 container ready` |
| 图像中心点击 | dispatched | `click.bin` = `1b 5b 4d 20 56 32 1b 5b 4d 23 56 32`（与切片 1 的十进制鼠标上报同一串字节） |
| 已花费租约再发文本 | StaleHandle | — |
| 文本 `Pawork CU-26 container {ok}`，再按 Return | 均为 dispatched | `typed.txt` 等于该字符串；`after_return.jpg` 显示 shell 回显 `GOT:Pawork CU-26 container {ok}` |
| 撤销后截图 | NotAuthorized | — |

安全属性（`podman inspect` 与容器内 `/proc`，容器仍在运行时采集）：

| 项 | 观测 |
| --- | --- |
| 根文件系统 | `ReadonlyRootfs=true`；`/proc/mounts` 上 `/` 为 `fuse-overlayfs`，选项含 `ro` |
| 能力 | 命令行是 `--cap-drop=ALL`。inspect 的 CapDrop 展开成 rootless 默认可授予的那一组，CapAdd 为空。容器内 CapInh / CapPrm / CapEff / CapBnd / CapAmb 全是 0 |
| 特权 | `Privileged=false`，`SecurityOpt=["no-new-privileges"]`，`UsernsMode=private`，进程 `uid=1000(cu26)` |
| 网络 | `NetworkMode=none`。`/proc/net/dev` 只有 `lo`。PortBindings 为 `{}`，没有发布到宿主 |
| 宿主挂载 | Binds 只有 `/tmp/cu26-container/sock:/sock:rw`。没有工作区、没有 X11 socket。Devices 为 `[]`。容器内没有 `/dev/input`、没有 `/dev/dri`。OCI 仍自带 `/dev/null`、`zero`、`urandom`、`shm`、`pts`、`tty` |
| 可写处 | tmpfs：`/tmp`、`/run`、`/home`，加上运行时的 `/dev` 与 `/dev/shm` |
| 5907 | 容器内 guest 听 `0.0.0.0:5907`，但唯一接口是 lo，socat 只把它接到 Unix socket。宿主 `127.0.0.1:5907` 与 `[::1]:5907` 在探针期间仍是 x11vnc pid 1878046 |

宿主 x11vnc：探针开始（04:57:45 PDT）与结束（04:57:48 PDT）的 pid、当时的 lstart、netns 与两条监听都相同。本刀脚本没有向该 pid 发信号。`ps lstart` 在不同时刻显示过 `Wed Oct 14 22:37:53 2026`、`22:52:41` 和 `23:02:08`，pid 始终是 1878046。复跑前后记下的 `/proc/1878046/stat` starttime tick 都是 33215211，包装进程 1878049 的 tick 是 33215266；按当时 btime `1791711976`、HZ=100 换算，1878046 的启动时刻是 2026-10-14 23:02:08.11 PDT，与复跑当时的 `ps lstart` 一致。更早快照里的 lstart 与现在按 starttime tick 换算出的 23:02:08 不一致。复跑前后 tick 相同，不把这条差解释成本刀重启了 x11vnc。

和 CU-23 / 完成条件的差距（所以本任务仍是进行中，ROADMAP 不删）：

- 有一个宿主目录 bind（控制 socket）。这一轮没有达到 CU-23 的「无宿主 bind」。后来的「无宿主 bind」一节去掉了这条 guest 控制 bind；这一轮本身仍然有它。
- 没有把 5907 发布到宿主 loopback。跨边界是 Unix socket，探针为了拨硬编码端口而多了一个 helper netns。
- 运行时自带最小 `/dev`，不是「完全没有设备节点」。
- 镜像是本盒的 Sway 1.10.1 / wlroots 0.18.2，不是 CU-23 那次 aarch64 上的 Sway 1.7 / wlroots 0.15.1。
- 探针 grant 代替 GUI 审批。没有在这个容器里跑 `pawork --computer-native wayland`、GPUI 真窗口或真实 Provider。
- foot 首帧有 `C locale is not UTF-8` 警告，文本仍按字节写对。

证据在 `/tmp/cu26-container/`（`out/probe.jsonl`、`before.jpg`、`after_click.jpg`、`after_return.jpg`、`out/guest/click.bin`、`out/guest/typed.txt`、`security.tsv`、`inside.txt`、`host-5907-*.txt`）。首轮产物另存 `/tmp/cu26-container/run1/`。不入库。

## 同日复跑（2026-10-15）

同一镜像 `8038d17eb623` 再用 `/tmp/cu26-container/run.sh` 跑一轮。镜像内包为 sway 1.10.1-2、libwlroots-0.18 0.18.2-3、foot 1.21.0-2、socat 1.8.0.3-1+deb13u1（`podman run --entrypoint dpkg … -l`）。容器 `716b2c544a8e`，探针退出码 0。阶段与首轮相同：未授权 bind 为 NotAuthorized，bind、截图、中心点击、文本与 Return 通过，过期租约 StaleHandle，撤销后截图 NotAuthorized。`click.bin` 仍是 `1b 5b 4d 20 56 32 1b 5b 4d 23 56 32`，`typed.txt` 仍是 `Pawork CU-26 container {ok}`。`ReadonlyRootfs=true`，容器内五项能力集为 0，`NetworkMode=none`，`/proc/net/dev` 只有 lo，PortBindings 为空，Binds 仍只有 socket 目录，没有 `/dev/input` 与 `/dev/dri`。容器 netns 为 `net:[4026532241]`（与宿主 `net:[4026531840]` 不同；已退出容器的命名空间号可以被新容器复用），进程 `uid=1000(cu26)`。token 未出现在日志。宿主 5907 的两条监听仍是 x11vnc pid 1878046 的 fd 8 与 fd 9。完成条件仍未满足，差距与上一节相同。

## 点击中途 Partial 与 resolve 复验（2026-10-15，netns + headless Sway）

证据目录 `/tmp/cu26-reverify/`（不入库）。外层 `run.sh` 先记下宿主 5907，再执行 `timeout 180 unshare -rn /tmp/cu26-reverify/smoke.sh`。netns 为 `net:[4026532237]`，宿主为 `net:[4026531840]`。Sway 1.10.1 headless + pixman，唯一输出 HEADLESS-1 640×480，`xwayland disable`；目标是 foot，`app_id=cu26target`，pid 2061045，结束后仍 focused。终端开启 DECSET 1003（经典编码，不是 SGR），原始输入追加到 `out/click.bin`。`computer_guest --wayland` 在 netns 内听 `0.0.0.0:5907`，并设 `PAWORK_WAYLAND_STEP_DELAY_MS=1000`。探针 grant 代替 GUI 审批。宿主 `127.0.0.1:5907` 与 `[::1]:5907` 前后都是 x11vnc pid 1878046（display `:7`），`/proc/1878046/stat` 的 starttime tick 前后都是 33215211。本刀没有向该 pid 发信号。两次探针退出码都是 0，`smoke_exit=0`。

`click` 在该延迟非 0 时，先把绝对移动 round-trip 到 compositor，再等待，然后才检查取消。未设置、0 或越界则不等待，也不多一次 round-trip。探针在 `click.bin` 变长之后只关掉自己连往 5907 的写方向，继续读回执。guest 的 `serve_connection` 把读预算超时当成正常返回，所以 `guest.log` 停在监听行（51 字节），不断开日志。

| 边界 | Host 结果 | 外部事实 |
| --- | --- | --- |
| 点击 Partial | `partial 1/2: computer action cancelled`，`input_dispatched=true`。`shut_write=1`。动作后观测失败，原因字符串是 `target process exited or restarted; bind a new target` | `click.delta` = `1b 5b 4d 43 56 32`。`0x43` 是经典 1003 的无键移动（编码 32 + 移动位 32 + 按钮 3），列 54 行 18。没有按下 `0x20`，也没有释放 `0x23`。600ms 内长度不变。同一张截图再点是 `StaleHandle`，`click.bin` 不增长。foot 没有退出；失败来自写方向关掉之后，后续 metadata 无法再证明实例 |
| 之后的完整点击 | 重新 bind、截图，再点中心，`dispatched` | 这一段是 `1b 5b 4d 20 56 32 1b 5b 4d 23 56 32`（左键按下 / 释放，同一格）。指针已经在这一格上，所以没有第二份移动上报。`click.bin` 全文是移动加这一对按键，18 字节 |
| 空闲后 bind | 同一 Host 先 `bind` 得到 NotAuthorized（证明当时能解析到目标），静置 22 秒后再 `bind`，仍是 NotAuthorized，不是「no installed application」 | 静置前 socket inode `37475124`、本地端口 53056，`peer_closed=false`。22 秒后同一 inode 的 `peer_closed=true`。这次 bind 之后 inode 换成 `37480999`、本地端口 44966 |
| 空闲后 request | `requested guest.wayland.cu26target` | 走的是重拨后的连接 |
| 真正未安装 | `request guest.wayland.absent` 为 `Invalid("no installed application resolves this bundle id")`，不是 NotAuthorized | 元数据成功，列表里没有这个 bundle |
| 批准后再 bind | 通过，窗口 `w-2061128-1792070612689373258-1` | 目标仍在 tree 里，pid 2061045，focused |

动作前 `get_inputs` 为 `[]`，动作后只有 `0:0:wlr_virtual_keyboard_v1` 与 `0:0:wlr_virtual_pointer_v1`。这不是专用容器，也没有 GPUI 真窗口或真实 Provider。当时 CU-23 的无宿主 bind 还没有做。容器上的 GUI 审批和真实 Provider 见下一节；无 bind 形态见后文「无宿主 bind」。

## 专用容器上的 GUI 审批与真实 Provider（2026-10-15）

Guest 仍是上一节的镜像 `localhost/pawork-cu26-wayland:local`（`8038d17eb623`）：headless Sway 1.10.1 + pixman，唯一输出 HEADLESS-1 640×480，`xwayland disable`，foot `app_id=cu26target`。容器名 `cu26-wayland-gui`，参数与最小验收相同（只读根、`--cap-drop=ALL`、`no-new-privileges`、`network=none`、无端口发布、唯一宿主挂载 `/tmp/cu26-container-gui/sock:/sock`）。容器内 socat 把 `127.0.0.1:5907` 转到 Unix socket。产品 Host 仍固定拨 `127.0.0.1:5907`，所以 Host 不在宿主 netns 里（那里的 5907 是 x11vnc），而在容器外的另一个 `unshare -rn` 里：该 netns 的 `127.0.0.1:5907` 只是转到上述 Unix socket 的转发。Provider 出网用 `slirp4netns`（预检 `https://api.z.ai/` 返回 301）。Desktop 在宿主 netns、`DISPLAY=:9`。

成功 Run `run-gui-1fd787-18debab5f352e56e-2`（会话里这一轮只有这一次 Run）。`provider_request_started` 的 provider 为 `glm-coding`、model 为 `glm-5.3-flash`（8 次）。7 次 `tool_approval_responded` 都是 `approved_once`。工具审批卡的「Allow once」和 Computer 面板上 `guest.wayland.cu26target` 的「Allow for this run」都是 `DISPLAY=:9` 的 X11 指针点击（`/tmp/cu26-container-gui/approvals.log`：07:37:00 点击 `(1159, 311)`）。不是探针 grant，也不是 `GuiClient` 代批。

| 阶段 | Host 结果 | 外部事实 |
| --- | --- | --- |
| discover / request | 成功；discover 列出 `cu26target guest.wayland.cu26target browser windows=1` | 容器启动时 `get_tree` 唯一 `app_id=cu26target`，focused，640×455；`get_outputs` 唯一 HEADLESS-1 640×480；动作前 `get_inputs` 为 `[]` |
| bind | 窗口 `w-2086791-1792075023787440452-1` | 目标批准在这次 bind 的工具审批之前点下 |
| screenshot | 观测 `o-…-2`，JPEG 640×455，15866 字节 | `out/observation-1.jpg` |
| `observed_click` (318, 227) | `dispatched`，`input_dispatched=true` | `click.bin` = `1b 5b 4d 20 55 32 1b 5b 4d 23 55 32`（12 字节，左键按下 / 释放） |
| `observed_keys` 文本 | `dispatched` | `typed.txt` 为 `Pawork CU-26 container gui {ok}`（31 字节，与该字符串逐字节相同） |
| `observed_keys` Return | `dispatched` | `out/observation-4.jpg` 显示 shell 回显 `GOT:Pawork CU-26 container gui {ok}` |
| Run 终态 | `completed`，`stop_reason.kind=completed` | — |

进程：Host 在 helper netns `net:[4026532419]`，`target/debug/pawork --computer-native wayland --instance cu26cgui --approval-mode ask-for-writes --trust-workspaces --provider glm-coding --model glm-5.3-flash gui serve`（本刀按当前源码重建的二进制）。容器 netns `net:[4026532241]`，进程 `uid=1000(cu26)`。宿主 x11vnc 为 `net:[4026531840]`。

安全属性与最小验收相同：`ReadonlyRootfs=true`，根为只读 `fuse-overlayfs`；容器内 CapInh / CapPrm / CapEff / CapBnd / CapAmb 全是 0；`NetworkMode=none`，`/proc/net/dev` 只有 lo；PortBindings 为空；Binds 只有 socket 目录；没有 `/dev/input`、没有 `/dev/dri`。inspect 的 CapDrop 仍展开成 rootless 默认可授予的那一组，CapAdd 为空。token 与 Provider key 未出现在日志（`secret-scan.txt` 为 clean）。

宿主 5907：这一轮开始（07:36:18 PDT）与结束（07:38:07 PDT）都是 x11vnc pid 2068374（display `:7`），`/proc/2068374/stat` 的 starttime tick 都是 35920946，两条监听的 fd 仍是 8 和 9。本刀脚本没有向该 pid 发信号。更早，本会话开始时 5907 上是 pid 1878046；重建二进制期间（07:02:07 PDT）监督进程另起了 pid 2068374。那次更换不是本刀向 x11vnc 发信号造成的，成功轮的前后对照钉的是 2068374。

和 CU-23 / 完成条件的差距（条目保留）：

- 这一轮仍有一个宿主目录 bind（控制 socket），还没有达到 CU-23 的「无宿主 bind」。Host 不能在宿主 netns 里拨 5907，所以多了一个 helper netns 做端口转发。后来的「无宿主 bind」一节把拨号进程放进容器后去掉了这条 bind；这一轮本身仍然有它。
- 没有把 5907 发布到宿主 loopback。
- 运行时自带最小 `/dev`。镜像仍是本盒的 Sway 1.10.1 / wlroots 0.18.2。
- 这一轮没有在容器里重做撤销、断线、占用让出或点击中途 Partial。动作后的 `get_inputs` 没有来得及在容器退出前再读一次。
- foot 首帧仍有 `C locale is not UTF-8` 警告；文本按字节写对。

证据在 `/tmp/cu26-container-gui/`（`out/formal.json`、`out/observation-{1,2,3,4}.jpg`、`out/guest/click.bin`、`out/guest/typed.txt`、`approvals.log`、`security.tsv`、`inside.txt`、`host-5907-before.txt`、`host-5907-after.txt`）。不入库。

## 无宿主 bind（2026-10-15）

上一节的容器把 guest 的 5907 用 socat 接到宿主目录 `/sock`，Host 在另一个 netns 里拨硬编码的 `127.0.0.1:5907`。那是 guest 控制通道上的宿主 bind。这一刀把拨号进程放进 guest 所在的网络命名空间，容器不再挂任何宿主目录。

镜像在 `8038d17eb623` 上只换了入口（`localhost/pawork-cu26-wayland:nobind`，`9588f50534e2`）。入口仍起 headless Sway 与 `computer_guest --wayland`，不再执行 socat。运行参数去掉了 `-v`。内存上限从 2g 改为 4g，只为同容器里再放一份 `pawork` 二进制。`--tmpfs` 不是宿主目录 bind。token 只经 env 文件进入，不写入本页。

```text
podman run -d --name cu26-wayland-nobind --init --read-only --cap-drop=ALL \
  --security-opt=no-new-privileges --network=none --user 1000:1000 \
  --userns=keep-id --memory=4g --pids-limit=512 --shm-size=256m \
  --tmpfs /tmp:rw,mode=1777 --tmpfs /run:rw,mode=755 --tmpfs /home:rw,mode=755 \
  --env-file <token-env> localhost/pawork-cu26-wayland:nobind
```

探针是容器内的 `wayland_host_probe`（`podman cp` 进 `/tmp`，不是挂载）。它与 guest 同在 `net:[4026532241]`，宿主 x11vnc 在 `net:[4026531840]`。探针 grant 代替 GUI 审批。容器 `08c8a29c97ea`，探针退出码 0。

| 阶段 | Host 结果 | 外部事实 |
| --- | --- | --- |
| 未授权 bind | NotAuthorized | — |
| bind（探针 ForRun grant） | 通过，窗口 `w-62-1792076252506416752-1` | 动作前 `get_inputs` 为 `[]` |
| 截图 | 通过，15866 字节 | — |
| 图像中心点击 | dispatched | `click.bin` = `1b 5b 4d 20 56 32 1b 5b 4d 23 56 32` |
| 已花费租约再发文本 | StaleHandle | — |
| 文本 `Pawork CU-26 nobind {ok}`，再按 Return | 均为 dispatched | `typed.txt` 等于该字符串（24 字节） |
| 撤销后截图 | NotAuthorized | 动作后 `get_inputs` 只有 `0:0:wlr_virtual_keyboard_v1` 与 `0:0:wlr_virtual_pointer_v1`；唯一输出仍是 HEADLESS-1 640×480 |

`/proc/net/tcp` 在探针期间采了 93 行，涉及 5907 的只有三类：`00000000:1713` 监听（`0.0.0.0:5907`，状态 0A），以及 `0100007F:1713` 与 `0100007F:A05C` 互为对端的 ESTABLISHED（`127.0.0.1:5907` 与 `127.0.0.1:41052`）。已建立连接的两端都是 `127.0.0.1`，没有连到别的地址。`/proc/net/dev` 只有 lo。宿主上不存在 `/tmp/cu26-no-bind/sock`。

同容器、探针结束之后又起了产品二进制：`/tmp/pawork --computer-native wayland --instance cu26nobind --approval-mode ask-for-writes gui serve --socket /tmp/pawork-data/gui.sock`。日志有 `pawork gui serving on /tmp/pawork-data/gui.sock`。该进程的 netns 仍是 `net:[4026532241]`。socket 在容器 tmpfs 上，宿主没有 `/tmp/pawork-data`。这次 `gui serve` 没有客户端，没有 Run，所以它自己没有拨 5907。拨号闭合的证据是同一 netns 里的探针，不是这次空转的二进制。

安全属性（容器仍在运行时采集）：

| 项 | 观测 |
| --- | --- |
| 根文件系统 | `ReadonlyRootfs=true`；`/` 为只读 `fuse-overlayfs` |
| 能力 | 命令行 `--cap-drop=ALL`。inspect 的 CapDrop 仍展开成 rootless 默认可授予的那一组，CapAdd 为空。容器内五项能力集为 0 |
| 特权 | `Privileged=false`，`SecurityOpt=["no-new-privileges"]`，`UsernsMode=private`，`uid=1000(cu26)` |
| 网络 | `NetworkMode=none`。PortBindings 为 `{}`，`podman port` 为空 |
| 宿主挂载 | `HostConfig.Binds` 为 `[]`，`Mounts` 为 `[]`。没有 `/sock`。Devices 为 `[]`。没有 `/dev/input`、没有 `/dev/dri`。OCI 仍自带 `/dev/null`、`zero`、`urandom`、`shm`、`pts`、`tty` |
| 可写处 | tmpfs：`/tmp`、`/run`、`/home`，加上运行时的 `/dev` 与 `/dev/shm` |

宿主 x11vnc：探针前、探针中、`podman stop` 之后都是 pid 2068374（display `:7`），starttime tick 都是 35920946，两条监听的 fd 仍是 8 和 9，netns 仍是 `net:[4026531840]`。本刀没有向该 pid 发信号。

和 CU-23 / CU-26 完成条件的对照：

- guest 控制通道上的宿主目录 bind 这一刀去掉了。Host↔guest 走容器内 `127.0.0.1:5907`。
- 没有端口发布，network none、只读根、CapDrop 没有退回。
- 本刀没有把 GUI socket 挂到宿主。容器外 Desktop 要连这个 Host，才需要一个只含 GUI socket 的目录挂载。那是 Desktop↔Host，不是 guest 控制通道；本刀没有加，所以 Binds 是空的。
- 真实 Provider 需要出网。同一 netns 保持 network none 时，容器里的 `gui serve` 到不了 Provider。上一节的 GPUI + 真实 Provider 证据仍然依赖当时的 socket 目录，没有在这个无 bind 形态里重跑。因此这一节本身不把 CU-26 标成完成。下一节补上了 GPUI 与真实 Provider；出网用的是挂到同一 netns 的 slirp，不是把 guest 控制通道的宿主目录 bind 加回来。
- 运行时自带最小 `/dev`。镜像仍是本盒的 Sway 1.10.1 / wlroots 0.18.2。探针 grant 代替 GUI 审批。

证据在 `/tmp/cu26-no-bind/`（`out/probe/probe.jsonl`、`out/guest/click.bin`、`out/guest/typed.txt`、`tcp-5907.txt`、`security.tsv`、`mounts.json`、`inside.txt`、`binary-host.txt`、`host-5907-*.txt`）。不入库。

## 无 bind 形态上的 GPUI 与真实 Provider（2026-10-15）

上一节的探针与空转 `gui serve` 都在 `Binds=[]` 的容器里，但没有 GPUI，也没有真实 Provider。这一刀补上这两项，guest 控制通道仍然不挂宿主目录。

镜像仍是 `localhost/pawork-cu26-wayland:nobind`（`9588f50534e2`）。容器 `cu26-nobind-gui`（`0d9d958977fb`）的参数与上一节相同：只读根、`--cap-drop=ALL`、`no-new-privileges`、`network=none`、无端口发布、没有 `-v`。`--tmpfs` 不是宿主目录 bind。`HostConfig.Binds` 与 `Mounts` 在 slirp 起来之后、Run 期间和 `podman stop` 之后都是 `[]`。容器里没有 `/sock`，也没有 `/gui`。

产品 Host 不在容器 mount namespace 里，也不在宿主 netns 里。`podman unshare nsenter -t <容器 pid> -n` 把它放进容器的网络命名空间（两边都是 `net:[4026532241]`），mount namespace 仍是 podman 的用户命名空间视图（`mnt:[4026532235]`，容器是 `mnt:[4026532237]`）。它拨的是这个 netns 里的 `127.0.0.1:5907`，不是宿主上的 x11vnc。GUI socket 是这个进程在宿主可见路径上创建的文件 `/tmp/cu26-nobind-gui/data/pawork-gui-cu26nbg.sock`，属主是宿主用户。这不是 podman volume，所以 Desktop↔Host 不需要目录挂载。Desktop 在宿主 netns、`DISPLAY=:9`。

Provider 出网是 `slirp4netns --configure --disable-host-loopback --userns-path=<容器 user ns> --netns-type=path <容器 net ns> tap0`。它给这个 netns 加了一块 tap，不发布端口，也不占用宿主 5907。预检 `https://api.z.ai/` 返回 301。采样期间 `/proc/net/tcp` 里和 5907 有关的唯一三元组是：`0.0.0.0:5907` 监听，以及 `127.0.0.1:5907` 与 `127.0.0.1:52332` 互为对端的 ESTABLISHED。没有非 loopback 的 5907 连接。采集时 `/proc/net/dev` 是 `lo` 与 `tap0`：loopback 上是 guest 控制流量，tap0 上是 Provider 流量。podman 的 `NetworkMode` 仍是 `none`。

成功 Run `run-gui-2017ff-18debccb7db16a6e-2`（会话里这一轮只有这一次 Run）。`provider_request_started` 共 8 次，provider 为 `glm-coding`、model 为 `glm-5.3-flash`。7 次 `tool_approval_responded` 都是 `approved_once`。工具审批卡的「Allow once」和 Computer 面板上 `guest.wayland.cu26target` 的「Allow for this run」都是 `DISPLAY=:9` 的 X11 指针点击（`/tmp/cu26-nobind-gui/approvals.log`：08:15:14 点击 `(1169, 369)`）。不是探针 grant，也不是 `GuiClient` 代批。窗口原点这一轮是 `(20, 116)`，不是上一轮量过的 `(10, 58)`；按旧绝对坐标点下去会打开模型菜单。成功的那次提交和审批都按当时窗口原点重算了坐标。

| 阶段 | Host 结果 | 外部事实 |
| --- | --- | --- |
| discover / request | 成功 | 动作前 `get_inputs` 为 `[]`；结束后唯一 xdg_shell con 为 `cu26target`，focused，640×455；唯一输出 HEADLESS-1 640×480 |
| bind | 窗口 `w-2103295-1792077317341608443-1` | 目标批准在这次 bind 的工具审批之前点下 |
| screenshot | 观测 `o-…-2`，JPEG 640×455，15866 字节 | `out/observation-1.jpg` |
| `observed_click` (320, 240) | `dispatched`，`input_dispatched=true` | `click.bin` = `1b 5b 4d 20 56 33 1b 5b 4d 23 56 33`（12 字节，左键按下 / 释放） |
| `observed_keys` 文本 | `dispatched` | `typed.txt` 为 `Pawork CU-26 nobind gui {ok}`（28 字节，与该字符串逐字节相同） |
| `observed_keys` Return | `dispatched` | 后又有三张 640×455 JPEG（16385 / 17474 / 18471 字节） |
| Run 终态 | `completed`，`stop_reason.kind=completed` | 动作后 `get_inputs` 只有 `0:0:wlr_virtual_keyboard_v1` 与 `0:0:wlr_virtual_pointer_v1` |

进程：Host pid 2103295，命令是 `target/debug/pawork --computer-native wayland --instance cu26nbg --approval-mode ask-for-writes --trust-workspaces --provider glm-coding --model glm-5.3-flash gui serve`。容器进程 `uid=1000(cu26)`。宿主 x11vnc 为 `net:[4026531840]`。

安全属性（Run 期间采集）：

| 项 | 观测 |
| --- | --- |
| 根文件系统 | `ReadonlyRootfs=true`；`/` 为只读 `fuse-overlayfs` |
| 能力 | 命令行 `--cap-drop=ALL`。inspect 的 CapDrop 仍展开成 rootless 默认可授予的那一组，CapAdd 为空。容器内五项能力集为 0 |
| 特权 | `Privileged=false`，`SecurityOpt=["no-new-privileges"]`，`UsernsMode=private`，`uid=1000(cu26)` |
| 网络 | podman `NetworkMode=none`。PortBindings 为 `{}`，`podman port` 为空。运行时另有 slirp 的 `tap0`，只为 Provider 出网 |
| 宿主挂载 | `HostConfig.Binds` 为 `[]`，`Mounts` 为 `[]`。没有 `/sock`，没有 GUI 目录卷。Devices 为 `[]`。没有 `/dev/input`、没有 `/dev/dri`。OCI 仍自带 `/dev/null`、`zero`、`urandom`、`shm`、`pts`、`tty` |
| Desktop↔Host | 没有目录挂载。GUI socket 是 Host 进程写在宿主路径上的 Unix socket，Host 只共享容器的 netns |
| 可写处 | tmpfs：`/tmp`、`/run`、`/home`，加上运行时的 `/dev` 与 `/dev/shm` |

宿主 5907：Run 前（08:10:22 PDT）、guest 就绪时、Run 结束（08:16:10 PDT）和 `podman stop` 之后（08:16:12 PDT）都是 x11vnc pid 2068374（display `:7`），starttime tick 都是 35920946，两条监听的 fd 仍是 8 和 9，netns 仍是 `net:[4026531840]`。本刀没有向该 pid 发信号。token 与 Provider key 未出现在日志（`secret-scan.txt` 为 clean）。

和完成条件的差距（条目保留，状态仍是进行中）：

- guest 控制通道没有宿主目录 bind。5907 没有发布到宿主 loopback。
- 运行时 netns 不是「只有 lo」。slirp 的 tap0 是 Provider 出网，podman 配置仍是 network none。这不是 CU-23 那种全程无出网接口的形态。
- Host 二进制与 guest 同 netns，但不在容器的能力集为 0 的 mount namespace 里。
- 没有在这个形态里重做撤销、断线、占用让出或点击中途 Partial。
- 没有在真窗口上复核 Computer 面板的观测列表。四张 JPEG 是从该 Run 的 `tool_calls.result_json` 解出的。
- 运行时自带最小 `/dev`。镜像仍是本盒的 Sway 1.10.1 / wlroots 0.18.2。

证据在 `/tmp/cu26-nobind-gui/`（`out/formal.json`、`out/observation-{1,2,3,4}.jpg`、`out/guest/click.bin`、`out/guest/typed.txt`、`approvals.log`、`security.tsv`、`mounts.json`、`inside.txt`、`tcp-5907-summary.json`、`host-proc.txt`、`host-5907-before.txt`、`host-5907-after.txt`）。不入库。

## 无 bind 容器上的边界与观测列表（2026-10-15）

同一镜像 `localhost/pawork-cu26-wayland:nobind`（`9588f50534e2`）。容器 `cu26-nobind-boundaries`（`ad87db45b87d`）覆盖入口为 `sleep`，在容器 tmpfs 里另起 headless Sway 与持续记录的 foot（`DECSET 1003`，原始字节同时写入 `events.bin` 与 `click.bin`）。`computer_guest` 用的是本机已带 `PAWORK_WAYLAND_STEP_DELAY_MS` 的调试二进制（镜像内那份没有这个变量），经 `podman cp` 放进 tmpfs，不是宿主目录 bind。探针 grant，没有 GPUI，没有真实 Provider，没有 slirp。

安全属性：`HostConfig.Binds=[]`，`Mounts=[]`，`ReadonlyRootfs=true`，`NetworkMode=none`，PortBindings `{}`，`podman port` 为空。容器内五项能力集为 0，`uid=1000(cu26)`，`/proc/net/dev` 只有 `lo`，没有 `/sock`、`/dev/input`、`/dev/dri`。探针、guest 与容器同为 `net:[4026532241]`，宿主是 `net:[4026531840]`。采样里 5907 的 `TIME_WAIT` 对端都是 `127.0.0.1`；当场再握一条连接时，`ESTABLISHED` 是 `127.0.0.1:41996` 与 `127.0.0.1:5907` 互为对端。没有非 loopback 的 5907。宿主 5907 仍是 x11vnc pid 2068374，starttime tick 35920946。token 未出现在日志。

| 探针 | 退出码 | 外部事实 |
| --- | --- | --- |
| `--boundaries` | 0 | `events.bin` 只有一次点击和 `SEAT-OK`、`AFTER-CANCEL-OK`、`LIVE-AGAIN`、`REDIAL-OK`、`YIELD-OK`。撤销三次 `NotAuthorized`，派发前取消为 `Cancelled`，断线为 `ProcessRestarted` / `WindowReplaced`，占用为 `TargetOccupied`，`close_scope` 后为 `ScopeClosed`，等待方再写入 `YIELD-OK`。`dispatch_paused=false`，没有独立接管入口 |
| `--partial-receipt` | 0 | `partial 1/11`，新增字节只有 `I`；回执丢失为 `unknown_effect`，新增字节只有 `L`。随后 `AFTEROK` 为 `dispatched` |
| `--click-partial` | 0（重开 compositor 之后） | 绝对移动 `1b 5b 4d 43 56 32`，`partial 1/2`，没有按下 / 释放。同一张租约再点为 `StaleHandle`。重新 bind 后的完整点击才是按下 + 释放 |

第一次 `--click-partial` 和边界探针共用同一指针位置，移动没有新的鼠标上报，整段按下 / 释放一起落地，退出码 1。这不是容器网络或 bind 的问题。关掉 Sway 再只跑点击之后，上表的移动 Partial 成立。文本 Partial 与边界结果用的是重开之前的那一轮，没有作废。

观测列表：没有新开 Provider Run。把上一轮无 bind GPUI 的会话库复制出来，Host 仍用 `podman unshare nsenter -n` 进入这个容器的 netns（`Binds` 仍是 `[]`，没有为这次复核加 slirp），Desktop 在 `DISPLAY=:9`。会话 `ses-2017ff-18debcbbab30b86d-1`，head=930；四张 JPEG 在 `tool_execution_completed` 的 sequence 271 / 404 / 495 / 518，尾窗从 631 起没有图。打开 Computer use 时连接曾断过一次；点 Retry 之后面板写着「4 observations」：一条 Latest、三条 History，预览是该 Run 的终端画面（含 `Pawork CU-26 nobind gui {ok}` 与 `GOT:`）。目标区是「No run is active」，因为这条 Run 已经结束。这是有界回填在真窗口上的结果，不是从 `result_json` 手工数出来的。

### slirp tap0

无 bind、无 Provider 的这一刀全程只有 `lo`。真实 Provider 要离开 netns 访问 `api.z.ai`，而 podman `network none` 没有默认路由。已经通过的 GPUI Run（`run-gui-2017ff-18debccb7db16a6e-2`）靠挂在同一 netns 上的 `slirp4netns` tap0 出网。两件事不能在同一次 Run 里同时成立：既要真实 Provider，又要全程只有 `lo`。

正式部署因此把 tap0 定为例外，不把那次 Provider 证据拆掉：

- guest 控制通道仍是容器内 `127.0.0.1:5907`，两端都是 loopback，`HostConfig.Binds` 保持 `[]`，不把 5907 发布到宿主。
- 只有需要真实 Provider 的 Run 才允许在该 netns 上加 `slirp4netns` tap0。它不发布端口，不挂宿主目录，不承载 guest 控制连接。
- 不需要出网的探针、撤销、Partial 保持只有 `lo`。

## 无 bind 容器 mount namespace 里的 GUI Host（2026-10-15）

上一节的产品 Host 只进了容器 netns。这一刀把 `pawork --computer-native wayland gui serve` 放进 guest 所在的 mount namespace，并让该进程的五项能力集为 0。guest 控制通道仍然不挂宿主目录。

镜像仍是 `localhost/pawork-cu26-wayland:nobind`（`9588f50534e2`）。容器 `cu26-host-mntns`（`ab341c826b8e`）的参数与无 bind 形态相同：只读根、`--cap-drop=ALL`、`no-new-privileges`、`network=none`、无端口发布、没有 `-v`。`HostConfig.Binds` 与 `Mounts` 为 `[]`。

产品 Host 由 `podman exec` 启动，二进制经 `podman cp` 放在容器 tmpfs `/tmp/pawork`，不是宿主目录 bind。宿主侧 pid 2152621，容器 pid namespace 里是 60。它与容器 init 同为 `mnt:[4026532237]`、`net:[4026532241]`、`user:[4026532236]`，也在容器 pid namespace（`pid:[4026532240]`）里。`Uid` 为 1000，`CapInh` / `CapPrm` / `CapEff` / `CapBnd` / `CapAmb` 都是 0，`NoNewPrivs` 为 1。启动它的 shell 仍在宿主 netns `net:[4026531840]`，mount namespace 是 `mnt:[4026532317]`，与 Host 不同。

GUI socket 不靠目录挂载。Host 在容器 tmpfs 上监听 `/tmp/pawork-data/pawork-gui-cu26mnt.sock`。一个 splice 进程先在宿主路径 `/tmp/cu26-host-mntns/data/pawork-gui-cu26mnt.sock` 上 `bind` / `listen`，再 `setns` 进入容器 mount namespace、丢掉五项能力，然后把每条连接转到 tmpfs 上的那个 socket。`podman unshare` 使这个 splice 进程在用户命名空间里是 uid 0；能力集同样为 0，没有 `CAP_DAC_OVERRIDE`。它不进入容器 netns。Desktop 在宿主 netns、`DISPLAY=:9`，连的是宿主路径上的 socket。Desktop 与 Host 之间没有目录挂载。

Provider 出网仍是挂在同一 netns 上的 `slirp4netns` tap0。容器内预检 `api.z.ai:443` 为 `Verification: OK`。podman `NetworkMode` 仍是 `none`。采样文件里和 5907 有关的已建立连接全部是 `127.0.0.1` 对 `127.0.0.1`（2388 条），没有非 loopback。Host 拨的是这个 netns 里的 `127.0.0.1:5907`。

无项目的会话是负向对照：`computer_native` discover 两次都是 `tool is not allowed in an untrusted workspace`，当时 `click.bin` 仍是 0 字节。随后在已信任的 workspace `ws` 上发同一条任务。窗口原点是 `(20, 116)`。7 次工具审批的「Allow once」是 X11 指针点击 `(382, 843)`；Computer 面板「Allow for this run」是 `(1169, 369)`。不是协议客户端代批。

成功 Run `run-gui-3c-18dec2e13dc1621f-7`。`provider_request_started` 8 次，provider 为 `glm-coding`、model 为 `glm-5.3-flash`。7 次 `tool_approval_responded` 都是 `approved_once`。终态 `completed`，`stop_reason.kind=completed`。

| 阶段 | Host 结果 | 外部事实 |
| --- | --- | --- |
| discover / request | 成功 | 动作后 `get_inputs` 只有 `0:0:wlr_virtual_keyboard_v1` 与 `0:0:wlr_virtual_pointer_v1` |
| bind | 窗口 `w-60-1792082687818921986-6` | 目标批准在后续工具审批之前点下 |
| screenshot | 观测 `o-60-1792082687818921986-7` | 工具结果 `is_error=false` |
| `observed_click` (317, 225) | `input_dispatched=true` | `click.bin` = `1b 5b 4d 20 55 32 1b 5b 4d 23 55 32`（12 字节，左键按下 / 释放） |
| `observed_keys` 文本 | `input_dispatched=true` | `typed.txt` 为 `Pawork CU-26 mntns gpui {ok}`（28 字节，与该字符串逐字节相同） |
| `observed_keys` Return | `input_dispatched=true` | 文本行已经写在 `typed.txt`；脚本在写出该行之后进入 `sleep` |
| Run 终态 | `completed` | 同上 |

同一 Host 进程在打开真窗口之前，还有一轮 GUI 协议客户端代批的 Run `run-gui-3c-18dec174beacd127-2`（`typed.txt` 当时是 `Pawork CU-26 mntns {ok}`）。那一轮证明同 mntns 的 Host 能走完工具链；真窗口证据以上表为准。

安全属性：

| 项 | 观测 |
| --- | --- |
| 根文件系统 | `ReadonlyRootfs=true` |
| 能力 | Host 进程五项能力集为 0，`NoNewPrivs=1`。容器内 `podman exec` 的 shell 同样为 0 |
| 身份 | Host `uid=1000`。splice 进程在用户命名空间内是 uid 0，能力集为 0 |
| 网络 | podman `NetworkMode=none`，PortBindings `{}`。运行时另有 slirp 的 `tap0` |
| 宿主挂载 | `HostConfig.Binds` 为 `[]`，`Mounts` 为 `[]`。没有 `/sock`，没有 GUI 目录卷 |
| Desktop↔Host | 没有目录挂载。宿主路径上的 socket 是 splice 进程在 `setns` 之前 `bind` 的 |
| pid namespace | Host 与 guest 相同。Provider key 在 Host 的 environ 里，同 uid 的 guest 进程可以读 `/proc/<pid>/environ`。这是相对此前只 `nsenter -n` 的退步，条目因此仍不完成 |

宿主 5907：Run 前、Run 中和 `podman stop` 之后都是 x11vnc pid 2068374，`/proc/2068374/stat` 的 starttime tick 都是 35920946，监听 fd 仍是 8 和 9，netns 仍是 `net:[4026531840]`。这段时间 `/proc/stat` 的 btime 增加了 587 秒，所以 `ps lstart` 的墙上时间跟着移了 587 秒；tick 没有变。本刀没有向该 pid 发信号。token 与 Provider key 未出现在证据目录的文本日志里。

和完成条件的差距（条目保留，状态仍是进行中）：

- guest 控制通道没有宿主目录 bind。5907 没有发布到宿主 loopback。
- Host 已与 guest 同 mount namespace，五项能力集为 0，并拨容器内 `127.0.0.1:5907`。
- Host 与 guest 共享 pid namespace。Provider key 因此落在 guest 可见的 environ 里。splice 进程在用户命名空间内是 uid 0（能力集为 0）。
- 真实 Provider 仍依赖 slirp tap0。不需要出网的探针保持只有 `lo`。
- 没有在这个 Host 形态里重做撤销、断线、占用让出或点击中途 Partial。
- 没有为这条新 Run 重开真窗口复核 Computer 面板的观测列表。
- `AppFamily` 仍没有 Wayland 取值。seat 级权限的固有限制仍在。

证据在 `/tmp/cu26-host-mntns/`（`host-proc.txt`、`proxy.log`、`proxy-gpui.log`、`approvals.log`、`out/gpui/click.bin`、`out/gpui/typed.txt`、`live2/session.db`、`tcp-during.txt`、`host-5907-before.txt`、`host-5907-after.txt`）。不入库。

## 不共享 guest pid namespace 的 GUI Host（2026-10-15）

上一节的 Host 用 `podman exec` 进入容器，因此连 pid namespace 一起进去了。这一刀改成：在 `podman unshare` 拿到的用户命名空间里 `setns` 容器的 mount namespace 和 network namespace，不 `setns` pid namespace，然后丢掉五项能力，再 exec `pawork --computer-native wayland gui serve`。二进制仍经 `podman cp` 放在容器 tmpfs `/tmp/pawork`。

镜像仍是 `localhost/pawork-cu26-wayland:nobind`（`9588f50534e2`）。容器 `cu26-host-nopidns`（`8806d94d2d3e`）参数与无 bind 形态相同：只读根、`--cap-drop=ALL`、`no-new-privileges`、`--userns=keep-id`、`network=none`、无端口发布、没有 `-v`。`HostConfig.Binds`、`Mounts`、`PortBindings` 都是空的。

| 进程 | mnt | net | pid | user | 宿主视角 Uid | 能力 |
| --- | --- | --- | --- | --- | --- | --- |
| Host `2208512` | `mnt:[4026532239]` | `net:[4026532244]` | `pid:[4026532320]` | `user:[4026532234]` | 1000 | 五项都是 0，`NoNewPrivs=1`，`NStgid` 只有 `2208512` |
| guest `computer_guest` 宿主 pid `2196595`（容器 pid 47） | 同上 | 同上 | `pid:[4026532243]` | `user:[4026532238]` | 1000 | 五项都是 0，`NStgid` 为 `2196595 47` |
| 容器 init `2196482` | 同上 | 同上 | `pid:[4026532243]` | `user:[4026532238]` | 1000 | 五项都是 0，`NStgid` 为 `2196482 1` |
| splice `2210621` | 同上 | 宿主 `net:[4026531840]` | `pid:[4026532320]` | `user:[4026532234]` | 1000 | 五项都是 0 |
| 启动 shell | `mnt:[4026532317]` | `net:[4026531840]` | `pid:[4026532320]` | `user:[4026531837]` | — | — |

Host 与 guest 的 mnt、net 相同，pid 与 user 不同。Host 的 pid namespace 等于启动它的宿主 shell，`NStgid` 没有容器内 pid。uid 映射：`podman unshare` 里 uid 0 对应宿主 uid 1000；容器 `--userns=keep-id` 里 uid 1000 也对应宿主 uid 1000。两边宿主 uid 相同，用户命名空间 inode 不同。environ 可见性靠 pid namespace，不靠把 Host 降到另一个宿主 uid。

容器内以 uid 1000 扫描 `/proc/[0-9]*/environ`：可读 14 个，拒绝 0 个，变量名 `PAWORK_API_KEY_GLM_CODING` 命中 0。`/proc/2208512` 不存在。guest 自身 environ 名里没有这个变量。Host 的 environ 名里有它（只记录名字）。证据目录文本和从容器 tmpfs 拷出的数据目录里，这个 key 的字节串命中 0。

共享 mntns 之后，容器里的 `/proc` 是按容器 pid namespace 挂上的。Host 打开 `/proc/self/exe`、`/proc/self/status`、`/proc/net/dev` 都是 errno 2。产品 Host 仍然完成了下面的 Run。`--private-proc` 会先 `unshare` 出新的 mount namespace 再试图卸掉 `/proc`；`umount` 返回 EINVAL（挂载被锁），那次探针没有拿来跑 Host，所以跑通的 Host 仍留在 guest 的 mntns 里。不在共享的 mntns 上把 `/proc` 覆盖成宿主 pid namespace 的 procfs，否则 guest 会看见 Host。

GUI socket 仍不靠目录挂载。Host 在容器 tmpfs 上监听 `/tmp/pawork-data/pawork-gui-cu26npid.sock`（`srw-------`，属主 cu26）。splice 先在宿主路径 bind，再 `setns` 进容器 mntns、丢掉能力，把连接转到 tmpfs 上的 socket。它不进容器 netns，也不进容器 pid namespace，environ 里没有 Provider key。

splice 保持用户命名空间内 uid 0，因为这个 uid 才映射到宿主 uid 1000。监听 socket 和容器内 GUI socket 都是 mode 0600、属主为该 uid；能力集已经是 0，再 `setuid` 到其它 uid 会落到 subuid，连不上这两端。放宽 mode 会让已经共享 mntns、且映射到同一宿主 uid 的 guest 更容易连上，所以不降。范围只是 GUI 字节转发。

共享 mntns 的另一面：Host 的数据目录在容器 tmpfs 上，文件属主是容器 uid 1000。guest 因此能读 mode 0600 的 GUI token（`/tmp/pawork-data/gui-cu26npid.token`，65 字节）和 `/tmp/pawork-data/cu26npid/protected/master.key`（32 字节）。这与上一节 `podman exec` 的 Host 相同，不是这一刀新引入的 environ 泄漏。抽查这些文件不含 Provider key。

Provider 出网仍是挂在该 netns 上的 `slirp4netns` tap0。本刀复测 `api.z.ai:443` 为 `Verification: OK`。容器内按 0.2 秒追加的 `/proc/net/tcp` 样本（`tcp-during.txt`，23374 行）里，本地端口 5907 的 ESTABLISHED 有 1062 行，两端都是 `127.0.0.1`，非 loopback 为 0。这是重复采样，不是 1062 条不同连接。Run 结束后从容器内再拨一次 `127.0.0.1:5907`，当场看到的是 `127.0.0.1` 上的 `CLOSE_WAIT` 与 `TIME_WAIT`。

11:38 PDT 容器仍在运行时复测：guest uid 1000 再扫 `/proc`，可读 10 个 environ，`PAWORK_API_KEY_GLM_CODING` 命中 0，`/proc/2208512` 仍不存在。宿主 5907 的 starttime tick 仍是 35920946。

审批是 GUI 协议客户端：7 次 `tool_approve(approve_once)`，以及一次 `computer_target_approve`（`for_run`，`guest.wayland.cu26target`）。没有开 `DISPLAY=:9` 的 GPUI 真窗口，也没有用 X11 指针点审批卡。

同一 Host 上先有两次失败：`run-gui-21b300-18dec5b2682415cf-2` 在出网尚未可用时 `http connect error`；`run-gui-21b300-18dec5ca452e2ed5-5` 完成 discover 与 request 后 `http timeout`。成功的是 `run-gui-21b300-18dec70c185d6533-8`（会话 `ses-21b300-18dec70c14faa917-7`）。快照里的 provider 为 `glm-coding`、model 为 `glm-5.3-flash`。终态 `completed`，`stop_reason.kind=completed`。用量 `input_tokens=42230`、`output_tokens=1206`。

| 阶段 | 结果 | 外部事实 |
| --- | --- | --- |
| discover | `is_error=false` | 下一行 `no regular applications`（seat 目标不走常规应用表） |
| request `guest.wayland.cu26target` | `is_error=false` | `requested guest.wayland.cu26target` |
| bind | `is_error=false` | `bound w-2208512-1792088570669733660-1`；目标批准在后续点击之前 |
| screenshot | `is_error=false` | `observation o-2208512-1792088570669733660-2` |
| `observed_click` (332, 227) | `input_dispatched=true`，`outcome=dispatched` | `click.bin` = `1b 5b 4d 20 58 32 1b 5b 4d 23 58 32`（12 字节） |
| `observed_keys` 文本 | `input_dispatched=true`，`outcome=dispatched` | `typed.txt` 为 `Pawork CU-26 nopidns {ok}`（25 字节，与该字符串逐字节相同） |
| `observed_keys` Return | `input_dispatched=true`，`outcome=dispatched` | 文本行已经写在 `typed.txt` |

宿主 5907：本刀前后都是 x11vnc pid 2068374，`/proc/2068374/stat` 的 starttime tick 都是 35920946，监听 fd 仍是 8 和 9。没有向该 pid 发信号。

和完成条件的差距（条目保留，状态仍是进行中）：

- guest 控制通道没有宿主目录 bind。5907 没有发布到宿主。Host 与 guest 同 mntns、同 netns，五项能力集为 0，且不在 guest pid namespace。guest 读不到 Host environ 中的 Provider key。
- 这一刀的审批是 GUI 协议客户端，不是 GPUI 真窗口上的 X11 指针。
- splice 仍是用户命名空间内 uid 0（能力集为 0，不持有 Provider key，不在 guest pid / net namespace）。不能在保持 0600 socket 的同时降到别的 uid。
- Host 与 guest 的宿主 uid 都是 1000，只是用户命名空间不同。共享 mntns 使 guest 能读 Host 数据目录里属主为 cu26 的文件。
- 真实 Provider 仍依赖 slirp tap0。
- 没有在这个 Host 形态里重做撤销、断线、占用让出或点击中途 Partial。
- 没有为这条 Run 重开真窗口复核 Computer 面板的观测列表。
- `AppFamily` 仍没有 Wayland 取值。seat 级权限的固有限制仍在。

证据在 `/tmp/cu26-host-nopidns/`（`ns-table.txt`、`guest-proc.txt`、`guest-scan-now.txt`、`live-recheck.txt`、`enter-host.log`、`enter-priv.log`、`host-proc.txt`、`proxy.log`、`drive2.out`、`drive2.jsonl`、`out/click.bin`、`out/typed.txt`、`tcp-during.txt`、`db/live2.db`）。不入库。

## 阻塞与剩余（切片 3 之后；2026-10-15 Host 不共享 guest pid namespace 之后更新）

- **专用容器：进行中，不是「环境缺失」**。rootless Podman 可用。最小容器、带 socket 目录的 GPUI + 真实 Provider、无宿主 bind 的 GPUI + 真实 Provider、无 bind 容器内的撤销 / 取消 / 断线 / 让出 / 文本 Partial / 点击 Partial、无 bind 容器 mount namespace 内能力集为 0 的 GUI Host + GPUI 真窗口 + 真实 Provider，以及同一 mntns / netns 上不共享 guest pid namespace 的 GUI Host，都已有外部事实（见对应各节）。guest 控制通道的 `HostConfig.Binds` 为 `[]`，5907 的已建立连接两端都是 `127.0.0.1`。真窗口 Computer 面板在早先那条无 bind Run 结束后列出 4 条观测。Provider 出网的 tap0 是正式例外（见「slirp tap0」）。pid namespace 这一条已经分开：guest 同 uid 读不到 Host environ 里的 Provider key。仍缺：这一刀没有 GPUI 真窗口审批；观测列表面板没有对这条 Run 复核；撤销 / Partial 没有在这个 Host 形态重做；splice 仍是用户命名空间内 uid 0；共享 mntns 使 guest 能读 Host 数据目录里属主为 cu26 的文件。不要把本刀写成 CU-26 完成。
- ~~**正式链路缺真实 Provider、二进制进程与真窗口**~~：2026-10-14 已补上（见「二进制 Host + GPUI 真窗口验收」）。2026-10-13 的 `cu26_live` 仍是脚本 Provider、非二进制、非真窗口。~~`resolve` 误报「未安装」~~：离线替身已修（见「resolve 误报「未安装」」），2026-10-15 已在真实 Sway 上复验空闲 22 秒后的重拨（见「点击中途 Partial 与 resolve 复验」）。~~Desktop Computer 面板在 Run 结束后列出 0 条观测~~：根因是尾窗只覆盖最近 300 条事件，离线已改为有界回填；满页未到 head 时 Host `complete=false`，客户端停在窗口上沿而不顺游标扫回（见「面板列出 0 条观测」）。2026-10-15 在无 bind 会话上重开 `DISPLAY=:9` 真窗口，Computer 面板列出 4 条观测（见「无 bind 容器上的边界与观测列表」）。
- ~~**ComputerTargetList 能力矩阵与 Wayland 表面不符**~~：已修（2026-10-13，见「ComputerTargetList 能力矩阵修复」）。`target_capabilities` 对 `guest.wayland.*` 改为按 bundle_id 前缀对齐 `WaylandSurface::admit_action`，未新增族，wire 与 Desktop 映射不变。
- **`AppFamily` 没有 Wayland 取值**：`WaylandSurface` 沿用 CU-21 guest 的 `Browser` 标签，只作身份字段，Host 能力闸不看族；GUI 目标列表的能力矩阵已改为按 bundle_id 前缀识别 Wayland 目标，不再依赖族（见上一条）；网站目标在这里因 `current_origin` 恒为 None 而 fail-closed。新增族会改 app / desktop 的穷举映射与 GUI 展示，留给后续。
- **seat 级权限的固有限制**：三条协议的权限来自 socket 可达性，guest 内任何能连上 socket 的客户端（包括目标应用本身）都能截图或注入输入。`require_dedicated` 只能在调用时发现非虚拟设备或多余输出 / seat，无法阻止同 guest 内其它客户端创建虚拟设备。因此部署必须保证 guest 内只有 Pawork guest 服务和获批目标，sway 配置不带任何按键绑定。
- ~~撤销、断线、派发前取消，以及占用 / `close_scope` 让出~~：2026-10-15 已在本机 netns + headless Sway 记录（见「撤销 / 取消 / 断线 / 让出」）。没有独立的用户接管产品入口；`WaylandSurface::activity` 恒为 Idle，`dispatch_paused` 在这轮为 false。AT-SPI、其它应用、其它 compositor、共享用户 seat、portal 许可、非 ASCII 文本（当前 keymap 只覆盖可打印 ASCII）仍未验证。
- ~~动作进行到一半的 Partial，以及回执丢失~~：2026-10-15 已在同类 netns + headless Sway 记录（见「Partial 与回执丢失」）。`--boundaries` 那一轮的取消仍然是 guest 请求写出之前的 Cancelled，没有改写成 Partial。点击序列中途的 Partial 同日稍后已打出（见「点击中途 Partial 与 resolve 复验」）：绝对移动进入 `click.bin`，按下和释放没有发出。

GUI Host 装配验收 Validated: `cargo test --offline -p pawork-app --features pawork-app/ui-fixture --lib --no-run`（rustfmt 后重建，exit 0，新文件无警告）；在 `unshare -rn /tmp/cu26s4/smoke.sh` 内运行该 lib 测试二进制 `--ignored --exact gui_host::tests::cu26_live::cu26_wayland_gui_serve_assembly`：1 passed；负向对照两项均按预期失败（无 guest、无门控）；`rustfmt --edition 2021 --check` 干净。没有重跑 app 全量单测：本次只新增一个 ignored 测试文件和一行 `mod` 声明，非测试代码零改动。

GUI Host 装配验收 Targeted regressions: `cu26_wayland_gui_serve_assembly`（真实 Sway / guest 的 live 测试，默认 ignored），覆盖错误 token 拒绝、目标批准前 bind 拒绝、过期租约拒绝、每次工具调用都经 GUI 审批、持久观测读回与 Run 终态 scope 收口。

切片 2 Validated: `bash scripts/test.sh computer-use`（即 `cargo test --offline --tests -p pawork-computer-use`：lib 93 通过 / 1 失败，失败项仍是既有的 `guest::tests::bad_or_missing_token_is_rejected_and_the_connection_is_dropped`，脚本因此退出 101）；`cargo test --offline -p pawork-computer-use --lib --bins --no-fail-fast`（guest bin 2 通过）；`cargo build --offline -p pawork-computer-use --tests --examples --bins` 无新增警告。`cargo check --offline -p pawork-tools` 未能运行：离线 registry 缺少 `aho-corasick`，属环境限制；tools / app 里实现 `NativeSurface` 的只有测试替身，新方法都有默认实现，`NativeRequest` 在本包外没有穷举 match。

切片 2 Targeted regressions: 10 项。`wayland_surface::tests` 3 项经真实 `NativeHost<WaylandSurface>` 与 TCP guest 替身：未授权零截图零输入，单次租约、越界与非 ASCII 不烧租约，半比例图像坐标映射，动作后新观测，撤销 / 取消 / 重连 / 关闭 scope 零输入，重叠在帧前拒绝，帧几何漂移按 WindowReplaced，回执丢失记 unknown_effect，partial 保留，未聚焦记 NotFocused。`wayland::guest::tests` 5 项：裁剪精确与越界拒绝；隐藏 / 重叠 / 超出输出拒绝而相邻窗口放行，Host 重复同一检查；点击偏移、按键需要焦点、未知键名；代际随 epoch / compositor / ledger / nonce 变化；单输出 rect 与 app_id 校验。`guest::tests` 2 项：base64 往返与畸形拒绝；X11 guest 默认不支持 seat op，cancel 先于写出，畸形 query 断开连接。

切片 1 Validated: `cargo test --offline --tests -p pawork-computer-use --no-fail-fast`（lib 83 通过 / 1 失败、guest bin 2 通过；失败项为既有的 `guest::tests::bad_or_missing_token_is_rejected_and_the_connection_is_dropped`，已在暂存本切片改动后的干净 HEAD 上以相同断言复现，与 CU-26 无关）；`cargo build --offline -p pawork-computer-use --example wayland_seat_probe` 加上表中的真实 smoke。

切片 1 Targeted regressions: `wayland::tests` 16 项，覆盖 wire 编解码与畸形头、SCM_RIGHTS 传 fd、全局协议缺失 / 重复拒绝（替身 compositor 证明拒绝前零 bind）、compositor 错误上报、无 display 拒绝、buffer 边界与通道顺序、JPEG 预算、文本先校验、keymap 完整唯一、Sway IPC 帧、窗口身份与 Xwayland 拒绝、专用 seat 拒绝、`/proc` 启动时间解析、代际账本。

撤销 / 取消 / 断线 / 让出 Validated: `cargo build --offline -p pawork-computer-use --example wayland_host_probe --bin computer_guest`；`timeout 200 unshare -rn /tmp/cu26-revoke/run.sh`（内层探针退出码 0，`out/summary.json` 的 check 全部为真）。没有改 `WaylandSurface` / guest 行为，没有新跑离线测试。

撤销 / 取消 / 断线 / 让出 Targeted regressions: none（本刀只有探针示例和文档；真实 Sway 证据见上节）。

Partial 与回执丢失 Validated: `cargo test --offline -p pawork-computer-use --lib step_delay_is_off_unless_the_probe_sets_a_bounded_value --bins --examples`（该过滤 1 passed；bin / example 过滤为 0）；`cargo build --offline -p pawork-computer-use --example wayland_host_probe --bin computer_guest`；`/tmp/cu26-partial/run.sh`（探针退出码 0，`out/summary.json` 的 check 全部为真，`events.bin` 为 `ILAFTEROK`）。

Partial 与回执丢失 Targeted regressions: `wayland::tests::step_delay_is_off_unless_the_probe_sets_a_bounded_value`。真实 Sway 上的 Partial / `unknown_effect` 见上节，不是离线替身。

专用容器最小验收 Validated: `podman version` / `podman info`；`podman run --rm --network=none --cap-drop=ALL --read-only --security-opt=no-new-privileges docker.io/library/busybox:1.36 echo podman-ok`；`podman build -t localhost/pawork-cu26-wayland:local`（镜像 `8038d17eb623`）；`/tmp/cu26-container/run.sh`（探针退出码 0，`probe.jsonl` 各阶段如上表）。同日复跑同一脚本，容器 `716b2c544a8e`，探针退出码 0；`podman run --rm --network=none --cap-drop=ALL --read-only --entrypoint dpkg localhost/pawork-cu26-wayland:local -l` 核对 sway / wlroots / foot / socat 包版本。没有改 guest / Host 代码，没有新跑离线测试。

专用容器最小验收 Targeted regressions: none（本刀只有容器验收和文档）。

点击中途 Partial 与 resolve 复验 Validated: `cargo test --offline -p pawork-computer-use --lib step_delay_is_off_unless_the_probe_sets_a_bounded_value -- --exact wayland::tests::step_delay_is_off_unless_the_probe_sets_a_bounded_value`（1 passed）；`cargo build --offline -p pawork-computer-use --example wayland_host_probe --bin computer_guest`；`/tmp/cu26-reverify/run.sh`（`smoke_exit=0`，`--click-partial` 与 `--resolve-idle` 退出码都是 0，check 全部为真）。

点击中途 Partial 与 resolve 复验 Targeted regressions: `wayland::tests::step_delay_is_off_unless_the_probe_sets_a_bounded_value`。真实 Sway 上的点击 Partial 与空闲重拨见上节，不是离线替身。

专用容器上的 GUI 审批与真实 Provider Validated: `cargo build --offline -p pawork -p pawork-desktop --bins`（exit 0）；同一镜像 `8038d17eb623` 上容器 `cu26-wayland-gui` 的 guest，加上容器外 helper netns 里的 `pawork --computer-native wayland gui serve` 与 `DISPLAY=:9` 的 `pawork-desktop`。成功 Run `run-gui-1fd787-18debab5f352e56e-2` 为 `completed`。没有改产品代码，没有新跑离线测试。

专用容器上的 GUI 审批与真实 Provider Targeted regressions: none（本刀只有容器验收和文档）。

无宿主 bind Validated: `podman build -t localhost/pawork-cu26-wayland:nobind`（镜像 `9588f50534e2`，基于 `8038d17eb623` 只换入口）；`/tmp/cu26-no-bind/run.sh`（探针退出码 0，`probe.jsonl` 各阶段如上表，`click.bin` / `typed.txt` 如上）。容器运行中采集了 `podman inspect` 与 `/proc`。没有改产品代码，没有新跑离线测试。

无宿主 bind Targeted regressions: none（本刀只有容器验收和文档）。

无 bind 形态上的 GPUI 与真实 Provider Validated: 同一镜像 `9588f50534e2` 上容器 `cu26-nobind-gui`（`Binds=[]`，`NetworkMode=none`），Host 经 `podman unshare nsenter -n` 进入该 netns 后 `pawork --computer-native wayland gui serve`，Desktop 在 `DISPLAY=:9`。成功 Run `run-gui-2017ff-18debccb7db16a6e-2` 为 `completed`。没有改产品代码，没有新跑离线测试。

无 bind 形态上的 GPUI 与真实 Provider Targeted regressions: none（本刀只有容器验收和文档）。

无 bind 容器上的边界与观测列表 Validated: 镜像 `9588f50534e2` 上容器 `cu26-nobind-boundaries`（`Binds=[]`，`NetworkMode=none`，`/proc/net/dev` 只有 lo）。容器内 `wayland_host_probe --boundaries`、`--partial-receipt`、`--click-partial` 的退出码分别为 0、0、0（点击那次是重开 compositor 之后）。`DISPLAY=:9` 打开上一轮会话的 Computer 面板，列出 4 条观测。没有改产品代码，没有新跑离线测试。

无 bind 容器上的边界与观测列表 Targeted regressions: none（本刀只有容器验收、真窗口复核和文档）。

无 bind 容器 mount namespace 里的 GUI Host Validated: 同一镜像 `9588f50534e2` 上容器 `cu26-host-mntns`（`Binds=[]`，`NetworkMode=none`）。`podman exec` 的 `pawork --computer-native wayland gui serve` 与 guest 同为 `mnt:[4026532237]` / `net:[4026532241]`，五项能力集为 0。`DISPLAY=:9` 的 X11 指针完成 7 次 `approved_once` 和 Computer 面板目标批准。Run `run-gui-3c-18dec2e13dc1621f-7` 为 `completed`。没有改产品代码，没有新跑离线测试。

无 bind 容器 mount namespace 里的 GUI Host Targeted regressions: none（本刀只有容器验收、真窗口和文档）。

不共享 guest pid namespace 的 GUI Host Validated: 同一镜像 `9588f50534e2` 上容器 `cu26-host-nopidns`（`8806d94d2d3e`，`Binds=[]`，`Mounts=[]`，`NetworkMode=none`）。`podman unshare` 内 `setns` 容器 mnt 与 net、不 `setns` pid，丢掉五项能力后 exec 的 `pawork --computer-native wayland gui serve` 与 guest 同为 `mnt:[4026532239]` / `net:[4026532244]`，pid namespace 分别为 `pid:[4026532320]` 与 `pid:[4026532243]`。容器内 uid 1000 扫 environ，`PAWORK_API_KEY_GLM_CODING` 命中 0。GUI 协议客户端审批的 Run `run-gui-21b300-18dec70c185d6533-8` 为 `completed`。没有改产品代码，没有新跑离线测试。本刀没有开 `DISPLAY=:9` 的 GPUI 真窗口。

不共享 guest pid namespace 的 GUI Host Targeted regressions: none（本刀只有容器验收和文档）。

Full workspace gate: NOT RUN（当前未设置全量门禁）。
