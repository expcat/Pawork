# CU-23 · Wayland 隔离后台路线可行性

> 状态：✅ 隔离 Wayland 可行性完成；初审 P2 证据归属修正后独立复审 PASS；生产能力尚未实现。前置：[CU-02](cu-02-target-contracts.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 触发条件

确认具体 compositor / portal 与独立虚拟桌面环境后开展。

## 目标

验证 Wayland 是否存在满足无干扰目标的独立桌面截图与输入路线。

## 当前依据与读取范围

LCU 不支持原生 Wayland，Pawork 当前只提供专用 Xvnc。不能把 X11 验收推广为所有 Wayland 会话支持。

[computer-use Spec](../spec/crates/computer-use.md)、[安全规格](../spec/security.md)、[LCU 平台限制](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/README.md)。

## 实施范围

- 检索目标平台当前系统 API，验证 portal/compositor 的屏幕与输入授权、目标范围及独立虚拟桌面。
- 比较最小 Rust 路线，优先证明不碰用户主 seat / 焦点 / 剪贴板，不能自动回退 XTEST 主桌面输入。
- 交付可行性结论、必要系统条件与下一实现切片，不提前建设所有 compositor 的抽象层。

## 完成条件与验证

- 可行路线有真实环境证据，并清楚列出支持的 compositor / 版本和授权条件。
- 无安全后台路线时明确不支持并记录原因；不宣称平台能力已实现。
- 本项只闭合可行性结论，后续生产实现另按实际差距拆任务，不靠本文完成代表支持 Wayland。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

2026-10-12，Grok 4.7 xhigh 执行专用容器实验，指挥独立核对原始日志、三张 PNG 与当前运行状态。仅证明以下 compositor 路线可行，未新增 Pawork 生产后端、依赖或测试资产。

### 环境与授权条件

- Debian 12 / Linux aarch64，Sway 1.7、wlroots 0.15.1、libwayland-client 1.21.0、GTK 3.24.38、grim 1.4.0、wtype 0.4；headless backend / 软件渲染，唯一输出 HEADLESS-1 为 1280×720。
- 专用容器 `pawork-cu23-wayland` 的镜像为 `sha256:6f8a06803dae9601c542b4bffad1ebf9ebf78642958637ce5958021391fa1a56`。当前独立核查确认只读 rootfs、非特权、UID 1000、`CapDrop=ALL`、`no-new-privileges`、network none、private IPC，无宿主 bind、设备或端口映射，未触碰 CU-21 容器。
- Wayland socket 位于容器内 UID 1000 / mode 0700 的 runtime 目录。截图用 `zwlr_screencopy_manager_v1`，点击用 `zwlr_virtual_pointer_manager_v1`，键盘用 `zwp_virtual_keyboard_manager_v1`。这些 compositor 协议的权限来自 socket 可达性，作用范围是整个专用 seat；本实验不证明它们提供应用级授权或 portal 用户许可。
- GTK 客户端报告 `GdkWaylandDisplay`，`DISPLAY=None`；Sway 当前节点为 `app_id=cu23marker`、`shell=xdg_shell`、con 5。无 Xwayland / Xorg / Xvnc；无 `/dev/input`、`/dev/uinput` 或 X11 socket。

### 实际结果与失败留存

| 操作 | 外部事实 | 实际结果 |
| --- | --- | --- |
| 初始画面 | GTK CSS `#dc1414`，grim 截图 | 红色 921600 / 921600 像素，中心 RGB 220,20,20 |
| 虚拟指针点击 640,360 | seat capabilities 0→1，等待 1000ms 后派发；客户端 `CLICK button=1 x=640.0 y=360.0` | 绿色 921600 / 921600 像素，中心 RGB 20,180,40 |
| `wtype -s 1000 -k k` | 客户端 `KEY keyval=107 string=k` | 蓝色 921600 / 921600 像素，中心 RGB 20,40,200 |
| 无 Wayland display 的 grim | `failed to create display` | exit 1，正确拒绝 |

首轮 `docker-exec-probe` exit 1：GTK Cairo 转换缺失、全灰且没有 CLICK/KEY。第二轮 `run2-css-eventbox` exit 1：CSS 和按键可用，但 `/tmp` 的 noexec 拒绝新指针程序（内部 exit 126）；`/dev/shm` 的尝试同样被拒绝。最终沿用已明确验证可执行的容器 `/dev/cu23-vpointer` 普通文件，其 SHA256 为 `d2ca14bf1839c6713bffae61242f6a3a9bcb7b6b7dbb4880b52b9b44e7f40fac`，未重建镜像或改变隔离参数。原失败、辅助命令非零退出码与成功轮的原日志、命令和退出码分别留存，未覆盖这些原始记录；第二轮图片目录则已被成功轮覆盖，不能再作为失败图片证据。

初轮 xdg-desktop-portal 1.16.0 / portal-wlr 0.7.0 探针在媒体接口中只看到 ScreenCast，未暴露 Screenshot / RemoteDesktop；本轮没有闭合 portal 截图、输入或许可流程。可行结论依据上述直接 compositor 协议，不推广为通用 portal 或所有 Wayland 桌面支持。

### 最小 Rust 路线与生产缺口

候选是受控 guest 内的 Rust Wayland 协议客户端，仅绑定本轮已证实的截图与虚拟输入协议，再适配既有 `NativeSurface` / Host 范围、审批、占用、取消及一次动作后的新观测。当前 `computer_guest` 的 X11 / AT-SPI 定位和 `GuestSurface` 的 RFB 图像不能直接复用成 Wayland 实现。

下一实现切片须先取得 compositor 权威的目标身份、进程与窗口代际映射，并限制整个 seat 为 Pawork 专用；本轮的 con / app_id 只识别实验目标，不满足生产句柄契约。随后才能接入既有工具、审批和持久化链，复核撤销、接管、动作回执及后置观测。未实现该切片，也未验证 AT-SPI、其它应用、其它 compositor、共享用户 seat 或真实用户持续输入；不接宿主主 seat，不扩建 compositor 抽象层。

证据根目录：`/tmp/pawork-cu23-wayland-feasibility/evidence`。成功轮为 `run3-dev-click.stdout/.exit/.command.json`（exit 0），图像 / state / pointer / tree 在 `run3-dev-click-files/cu23-run2/`；首轮保存在 `run1/`。第二轮失败以 `run2-css-eventbox.stdout/.exit/.command.json` 为依据；`cu23-host-run2.sh` 的固定提取目录 `run2-files/` 已被成功轮覆盖，当前为成功轮的重复产物，第二轮失败图片未单独保留。指挥实时核查为 `coordinator-current-check.json/.stdout/.exit`（exit 0，2026-10-12T08:52:38Z）。截图与辅助客户端均不检入仓库，本机 E3 实验不是 E4 人工签字或可发布门禁。

Validated: `run3-dev-click` 与指挥 `coordinator-current-check` 均 exit 0；三张 PNG 视觉核对、目标 CLICK/KEY、Sway 身份及隔离参数一致；送审前文档链接和 diff 检查均 exit 0。

Targeted regressions: none（无 Rust 或生产行为改动，不跑 Cargo）。

Full workspace gate: NOT RUN（当前未设置全量门禁）。

独立审查：Sol max 对话 `01a12f53-276e-7373-a4aa-48fe89816455`；初审唯一 P2 指出第二轮图片目录已被成功轮覆盖，已按真实留存边界修正；复审回合 `01a12f5b-5319-7992-aa90-f2f900d0245d` 为 PASS，阻断与非阻断问题均无。原始审查记录为证据目录 `REVIEW-ISSUES-1.json` 与 `REVIEW-PASS.json`。

