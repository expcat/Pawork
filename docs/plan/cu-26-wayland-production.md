# CU-26 · Wayland 隔离后台生产后端

> 状态：进行中。切片 1（guest 侧专用 seat 客户端与 compositor 权威身份）已落地：有离线定向回归，并在本机 headless Sway 1.10.1 做了真实 smoke。切片 2 的代码已实现、离线替身回归已通过：guest 帧协议加了 capture / input，`computer_guest --wayland` 提供服务，Host 侧新增 `WaylandSurface` 与按截图租约派发的点击 / 按键。切片 3 已接线、离线回归已通过：`computer_native` 工具暴露 `observed_click` / `observed_keys`，App 新增 `ComputerNativeMode::Wayland` 并选用 `NativeHost::wayland()`，CLI 接受 `--computer-native wayland`；默认入口未改，仍是 `host`。本机经 Host 链路的 smoke 已在独立 user + network namespace 内通过（`wayland_host_probe` → `NativeHost::wayland()` → 5907 → `computer_guest --wayland` → headless Sway，见下文「本机 Host 链路 smoke 复试」）；它不是专用容器，探针 grant 代替 GUI 审批，也没有经过正式 `pawork --computer-native wayland` 进程，完成条件仍未满足。专用容器无法建立。更新：2026-10-13。

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

## 阻塞与剩余（切片 3 之后）

- **专用容器环境缺失**：本机没有 Docker / Podman，CU-23 规格的专用 Wayland 容器（只读 rootfs、network none、CapDrop=ALL、无宿主 bind）无法建立，也无法验证「容器只把 5907 发布到宿主 loopback」这一部署形态。network none 与「发布端口」本身互相冲突，需要在部署设计中改为仅 loopback 的端口发布（同 CU-21），或改用 Unix socket 挂载，并重新评审。完成条件里的真实环境验收全部待此项。
- **本机 Host 链路 smoke 只到探针层**：2026-10-13 复试已在独立 user + netns 内经 `wayland_host_probe` 通过（见「本机 Host 链路 smoke 复试」）。下一步应在同样的 netns 做法下，改为经正式 `pawork --computer-native wayland` 进程与 GUI 目标审批完成，而不只是 `wayland_host_probe`。
- **正式工具链只有离线证据**：切片 3 已把工具、App、CLI 接通，但没有 `PAWORK_CU21_ISOLATED` 那样的 Wayland live 测试，也没有真实 Provider（`glm-coding / glm-5.3` 或 `xai / grok-4.7`）驱动的 Run、持久 ObservationList / ObservationImageRead 证据或真窗口审批。探针层 smoke 已打通，这些现在只差正式进程、Provider 与 GUI 审批的组合验收。
- **`AppFamily` 没有 Wayland 取值**：`WaylandSurface` 沿用 CU-21 guest 的 `Browser` 标签，只作身份字段，能力闸不看族；网站目标在这里因 `current_origin` 恒为 None 而 fail-closed。新增族会改 app / desktop 的穷举映射与 GUI 展示，留给后续。
- **seat 级权限的固有限制**：三条协议的权限来自 socket 可达性，guest 内任何能连上 socket 的客户端（包括目标应用本身）都能截图或注入输入。`require_dedicated` 只能在调用时发现非虚拟设备或多余输出 / seat，无法阻止同 guest 内其它客户端创建虚拟设备。因此部署必须保证 guest 内只有 Pawork guest 服务和获批目标，sway 配置不带任何按键绑定。
- 撤销、接管、断线、取消的真实环境证据，以及 AT-SPI、其它应用、其它 compositor、共享用户 seat、portal 许可、非 ASCII 文本（当前 keymap 只覆盖可打印 ASCII）均未验证。

切片 2 Validated: `bash scripts/test.sh computer-use`（即 `cargo test --offline --tests -p pawork-computer-use`：lib 93 通过 / 1 失败，失败项仍是既有的 `guest::tests::bad_or_missing_token_is_rejected_and_the_connection_is_dropped`，脚本因此退出 101）；`cargo test --offline -p pawork-computer-use --lib --bins --no-fail-fast`（guest bin 2 通过）；`cargo build --offline -p pawork-computer-use --tests --examples --bins` 无新增警告。`cargo check --offline -p pawork-tools` 未能运行：离线 registry 缺少 `aho-corasick`，属环境限制；tools / app 里实现 `NativeSurface` 的只有测试替身，新方法都有默认实现，`NativeRequest` 在本包外没有穷举 match。

切片 2 Targeted regressions: 10 项。`wayland_surface::tests` 3 项经真实 `NativeHost<WaylandSurface>` 与 TCP guest 替身：未授权零截图零输入，单次租约、越界与非 ASCII 不烧租约，半比例图像坐标映射，动作后新观测，撤销 / 取消 / 重连 / 关闭 scope 零输入，重叠在帧前拒绝，帧几何漂移按 WindowReplaced，回执丢失记 unknown_effect，partial 保留，未聚焦记 NotFocused。`wayland::guest::tests` 5 项：裁剪精确与越界拒绝；隐藏 / 重叠 / 超出输出拒绝而相邻窗口放行，Host 重复同一检查；点击偏移、按键需要焦点、未知键名；代际随 epoch / compositor / ledger / nonce 变化；单输出 rect 与 app_id 校验。`guest::tests` 2 项：base64 往返与畸形拒绝；X11 guest 默认不支持 seat op，cancel 先于写出，畸形 query 断开连接。

切片 1 Validated: `cargo test --offline --tests -p pawork-computer-use --no-fail-fast`（lib 83 通过 / 1 失败、guest bin 2 通过；失败项为既有的 `guest::tests::bad_or_missing_token_is_rejected_and_the_connection_is_dropped`，已在暂存本切片改动后的干净 HEAD 上以相同断言复现，与 CU-26 无关）；`cargo build --offline -p pawork-computer-use --example wayland_seat_probe` 加上表中的真实 smoke。

切片 1 Targeted regressions: `wayland::tests` 16 项，覆盖 wire 编解码与畸形头、SCM_RIGHTS 传 fd、全局协议缺失 / 重复拒绝（替身 compositor 证明拒绝前零 bind）、compositor 错误上报、无 display 拒绝、buffer 边界与通道顺序、JPEG 预算、文本先校验、keymap 完整唯一、Sway IPC 帧、窗口身份与 Xwayland 拒绝、专用 seat 拒绝、`/proc` 启动时间解析、代际账本。

Full workspace gate: NOT RUN（当前未设置全量门禁）。
