# CU-26 · Wayland 隔离后台生产后端

> 状态：进行中。切片 1（guest 侧专用 seat 客户端与 compositor 权威身份）已落地：有离线定向回归，并在本机 headless Sway 1.10.1 做了真实 smoke。切片 2 的代码已实现、离线替身回归已通过：guest 帧协议加了 capture / input，`computer_guest --wayland` 提供服务，Host 侧新增 `WaylandSurface` 与按截图租约派发的点击 / 按键。切片 3 已接线、离线回归已通过：`computer_native` 工具暴露 `observed_click` / `observed_keys`，App 新增 `ComputerNativeMode::Wayland` 并选用 `NativeHost::wayland()`，CLI 接受 `--computer-native wayland`；默认入口未改，仍是 `host`。本机经 Host 链路的 smoke 已在独立 user + network namespace 内通过（`wayland_host_probe` → `NativeHost::wayland()` → 5907 → `computer_guest --wayland` → headless Sway，见下文「本机 Host 链路 smoke 复试」）。同日又在同类 netns 内跑通了 `pawork gui serve` 同款装配：`ComputerNativeMode::Wayland` + `GuiApprovalHost` + token 认证的本地 socket；`computer_native` 的每次调用和目标批准都经 GUI Connection Protocol 完成，持久 ObservationList / ObservationImageRead 可读（见下文「GUI Host 装配链路验收」）。这一轮的 Provider 是脚本替身，没有起 `pawork` 二进制进程，也没有 GPUI 真窗口。2026-10-14 已补上二进制 Host、GPUI 真窗口审批与真实 Provider `glm-coding / glm-5.3-flash`（见「二进制 Host + GPUI 真窗口验收」）。专用容器仍无法建立，完成条件未满足。更新：2026-10-14。

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

未完成：专用容器仍不能建。验收当时 `WaylandSurface::resolve` 把 guest 元数据错误折叠成 None（修复见下文「resolve 误报「未安装」」）。同一 Host 进程、同一会话里，失败 Run `run-gui-1c1a3c-18de5a3c37013b1b-2` 的 `discover` 成功，`request` 成功（`requested guest.wayland.cu26target`），第一次 `bind` 为未授权（`target is not authorized…`），第二次 `bind` 才回报 `no installed application resolves this bundle id`；该 Run 随后因 `http timeout error from https://api.z.ai` 而 `run_failed`，不是这次 bind 之后立刻重试成功。成功 Run `run-gui-1c1a3c-18de5b4aa01fb2e3-4` 的七次调用没有这条「未安装」错误。更早的 attempt 2（另一 Host、会话 `ses-1c05b5-18de59375428ef5e-1`）是 `request` 报「未安装」、随后 `bind` 报未授权。Run 结束后 Computer 面板「Observation evidence」为 0 observations，原因未查；四张 JPEG 是从该 Run 落库的 `tool_calls.result_json` 解出的，不是面板读回。撤销、接管、断线、取消仍无真实环境证据。

本轮 Validated: 无新增 Cargo 测试。只读核对 `/tmp/cu26s6/proc-evidence.txt`、`desktop-proc.txt`、`out/formal.json`、`out/typed.txt`、`out/click.bin`、`out/tree-after.json` 与上表截图。

## resolve 误报「未安装」（2026-10-14）

根因是两层，都有会话时间戳和代码对应，不是目标真的没装，也不是代际或批准被写错。

guest 帧服务在 `serve_connection` 里用 `arm` 把读超时设成 `IO_BUDGET`（20 秒）。超时被 `read_exact` 收成 `GuestError::Disconnected("guest read failed")`，服务端关掉这条连接。Host 的 `WaylandSurface` 仍缓存着那条 `GuestLink`。下一次 `metadata` 在死套接字上失败，`with_link` 会丢掉缓存，但 `resolve` 当时写的是 `metadata().ok()?`，任何失败都变成 `None`。`NativeHost` 的 `request` / `bind` 把 `None` 写成 `Invalid("no installed application resolves this bundle id")`。批准检查在 `resolve` 成功之后，所以这条路径根本走不到 `NotAuthorized`。

`/tmp/cu26s6/data/cu26bin/session.db` 里，失败 Run `run-gui-1c1a3c-18de5a3c37013b1b-2` 的 discover、request、第一次 bind 相邻执行间隔约 10 秒，都过了 `resolve`（第一次 bind 是未授权）。第二次 bind 的工具调用在第一次 bind 完成后约 5.7 秒就发出，但 `tool_approval_requested` 到 `approved_once` 停了 66726 毫秒；执行时刻比上一次 guest 接触晚 72441 毫秒，回报未安装。attempt 2（`/tmp/cu26s6/attempt2/data-cu26bin/session.db`，Run `run-gui-1c05b5-18de593a6c923c32-2`）的 request 批准等待 17228 毫秒，距 discover 完成 22707 毫秒，报未安装；再过 10460 毫秒的 bind 是未授权——失败那次已经清掉死连接，下一次重新拨号，`resolve` 又成功了。空闲来自下一次 guest 往返之前的批准等待（加上模型间隔），不是目标消失。成功 Run `…-4` 的相邻工具执行间隔是 10–18 秒，都短于 20 秒，没有这条错误。

修复：`NativeSurface::resolve` 改为 `Result<Option<AppIdentity>, TargetError>`。`Ok(None)` 才是未安装（不是 `guest.wayland.` 前缀，或元数据成功但没有该 bundle）。`GuestError::Disconnected` 重拨一次；第二次仍失败，或 guest 以 `Unavailable` 回答，按 `ProbeUnavailable` 上抛。`NotAuthorized` 仍是批准闸。离线替身把缓存套接字掐断，不睡满 20 秒，覆盖空闲后的第一次 request / bind，以及元数据失败和拨号失败的错误类型。没有改 guest 的 20 秒读预算，也没有重跑真实 Sway。同一签名下，X11 `GuestSurface::resolve` 也不再把元数据失败收成 None，但没有 Wayland 这条重拨。`discover` 在自己的第一次元数据失败时仍返回空列表，不在这一刀。

## 阻塞与剩余（切片 3 之后；2026-10-14 更新）

- **专用容器环境缺失**：本机没有 Docker / Podman，CU-23 规格的专用 Wayland 容器（只读 rootfs、network none、CapDrop=ALL、无宿主 bind）无法建立，也无法验证「容器只把 5907 发布到宿主 loopback」这一部署形态。network none 与「发布端口」本身互相冲突，需要在部署设计中改为仅 loopback 的端口发布（同 CU-21），或改用 Unix socket 挂载，并重新评审。完成条件里的专用容器形态仍待此项。2026-10-14 的二进制 Host 验收用的是 `unshare -rn` 里的 headless Sway，不是该容器。
- ~~**正式链路缺真实 Provider、二进制进程与真窗口**~~：2026-10-14 已补上（见「二进制 Host + GPUI 真窗口验收」）。2026-10-13 的 `cu26_live` 仍是脚本 Provider、非二进制、非真窗口。~~`resolve` 误报「未安装」~~：已按离线替身修掉（见「resolve 误报「未安装」」）；没有用真实 Sway 再跑一遍。Desktop Computer 面板在 Run 结束后列出 0 条观测（原因未查）。
- ~~**ComputerTargetList 能力矩阵与 Wayland 表面不符**~~：已修（2026-10-13，见「ComputerTargetList 能力矩阵修复」）。`target_capabilities` 对 `guest.wayland.*` 改为按 bundle_id 前缀对齐 `WaylandSurface::admit_action`，未新增族，wire 与 Desktop 映射不变。
- **`AppFamily` 没有 Wayland 取值**：`WaylandSurface` 沿用 CU-21 guest 的 `Browser` 标签，只作身份字段，Host 能力闸不看族；GUI 目标列表的能力矩阵已改为按 bundle_id 前缀识别 Wayland 目标，不再依赖族（见上一条）；网站目标在这里因 `current_origin` 恒为 None 而 fail-closed。新增族会改 app / desktop 的穷举映射与 GUI 展示，留给后续。
- **seat 级权限的固有限制**：三条协议的权限来自 socket 可达性，guest 内任何能连上 socket 的客户端（包括目标应用本身）都能截图或注入输入。`require_dedicated` 只能在调用时发现非虚拟设备或多余输出 / seat，无法阻止同 guest 内其它客户端创建虚拟设备。因此部署必须保证 guest 内只有 Pawork guest 服务和获批目标，sway 配置不带任何按键绑定。
- 撤销、接管、断线、取消的真实环境证据，以及 AT-SPI、其它应用、其它 compositor、共享用户 seat、portal 许可、非 ASCII 文本（当前 keymap 只覆盖可打印 ASCII）均未验证。

GUI Host 装配验收 Validated: `cargo test --offline -p pawork-app --features pawork-app/ui-fixture --lib --no-run`（rustfmt 后重建，exit 0，新文件无警告）；在 `unshare -rn /tmp/cu26s4/smoke.sh` 内运行该 lib 测试二进制 `--ignored --exact gui_host::tests::cu26_live::cu26_wayland_gui_serve_assembly`：1 passed；负向对照两项均按预期失败（无 guest、无门控）；`rustfmt --edition 2021 --check` 干净。没有重跑 app 全量单测：本次只新增一个 ignored 测试文件和一行 `mod` 声明，非测试代码零改动。

GUI Host 装配验收 Targeted regressions: `cu26_wayland_gui_serve_assembly`（真实 Sway / guest 的 live 测试，默认 ignored），覆盖错误 token 拒绝、目标批准前 bind 拒绝、过期租约拒绝、每次工具调用都经 GUI 审批、持久观测读回与 Run 终态 scope 收口。

切片 2 Validated: `bash scripts/test.sh computer-use`（即 `cargo test --offline --tests -p pawork-computer-use`：lib 93 通过 / 1 失败，失败项仍是既有的 `guest::tests::bad_or_missing_token_is_rejected_and_the_connection_is_dropped`，脚本因此退出 101）；`cargo test --offline -p pawork-computer-use --lib --bins --no-fail-fast`（guest bin 2 通过）；`cargo build --offline -p pawork-computer-use --tests --examples --bins` 无新增警告。`cargo check --offline -p pawork-tools` 未能运行：离线 registry 缺少 `aho-corasick`，属环境限制；tools / app 里实现 `NativeSurface` 的只有测试替身，新方法都有默认实现，`NativeRequest` 在本包外没有穷举 match。

切片 2 Targeted regressions: 10 项。`wayland_surface::tests` 3 项经真实 `NativeHost<WaylandSurface>` 与 TCP guest 替身：未授权零截图零输入，单次租约、越界与非 ASCII 不烧租约，半比例图像坐标映射，动作后新观测，撤销 / 取消 / 重连 / 关闭 scope 零输入，重叠在帧前拒绝，帧几何漂移按 WindowReplaced，回执丢失记 unknown_effect，partial 保留，未聚焦记 NotFocused。`wayland::guest::tests` 5 项：裁剪精确与越界拒绝；隐藏 / 重叠 / 超出输出拒绝而相邻窗口放行，Host 重复同一检查；点击偏移、按键需要焦点、未知键名；代际随 epoch / compositor / ledger / nonce 变化；单输出 rect 与 app_id 校验。`guest::tests` 2 项：base64 往返与畸形拒绝；X11 guest 默认不支持 seat op，cancel 先于写出，畸形 query 断开连接。

切片 1 Validated: `cargo test --offline --tests -p pawork-computer-use --no-fail-fast`（lib 83 通过 / 1 失败、guest bin 2 通过；失败项为既有的 `guest::tests::bad_or_missing_token_is_rejected_and_the_connection_is_dropped`，已在暂存本切片改动后的干净 HEAD 上以相同断言复现，与 CU-26 无关）；`cargo build --offline -p pawork-computer-use --example wayland_seat_probe` 加上表中的真实 smoke。

切片 1 Targeted regressions: `wayland::tests` 16 项，覆盖 wire 编解码与畸形头、SCM_RIGHTS 传 fd、全局协议缺失 / 重复拒绝（替身 compositor 证明拒绝前零 bind）、compositor 错误上报、无 display 拒绝、buffer 边界与通道顺序、JPEG 预算、文本先校验、keymap 完整唯一、Sway IPC 帧、窗口身份与 Xwayland 拒绝、专用 seat 拒绝、`/proc` 启动时间解析、代际账本。

Full workspace gate: NOT RUN（当前未设置全量门禁）。
