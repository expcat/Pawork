# CU-21 · 隔离 Linux X11 / AT-SPI 应用控制

> 状态：✅ 完成（隔离 Firefox 正式链与模拟用户批准）；初审两项 P1 修后独立复审 PASS；其它动作、平台与系统边界独立待验。前置：[CU-02](cu-02-target-contracts.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 触发条件

作为明确选择的隔离桌面扩展；不作为本机后台能力的替代验收。

## 目标

为现有专用 Linux 桌面增加 Rust 应用/窗口和元素控制通道。

## 当前依据与读取范围

现有 Alpine / Fluxbox / Xvnc 有 RFB 图像与输入，不具备 Rust guest AX 服务。RFB 协议不能直接返回 AT-SPI 树；LCU 的 glibc 原 runtime 要求不等于独立 Rust 实现的平台限制。

[computer-use Spec](../spec/crates/computer-use.md)、[隔离桌面部署](../../crates/computer-use/README.md)、[安全规格](../spec/security.md)。

## 实施范围

- 在现有包边界优先实现最小 Rust guest 入口，通过 X11 / D-Bus / AT-SPI 提供应用、窗口和元素；RFB 保留图像/输入职责。
- guest 状态通道受控且有身份校验，模型不能指定任意端口/路径；不挂宿主目录、设备、屏幕或剪贴板。
- 沿用观测、目标归属、审批和取消契约，不同时新建另一套 Agent 引擎或容器池。
- 容器仍显式启动；GUI/模型明确显示目标属于隔离环境，不静默把本机请求转到容器。

## 完成条件与验证

- 容器内真实应用能被定位、读树和操作，状态与图像一致。
- 用户本机键鼠、焦点和剪贴板不受影响，guest 断线和租约拒绝符合共享契约。
- 只声明此隔离环境的能力，不能记为本机既有应用后台验收通过。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

2026-10-12 已实现本项隔离 Firefox 切片。执行对话为 `01a12d84-523b-75a1-aec3-b6c6ca97a9b3`（`xai/grok-4.7 xhigh`）；正式独立 `gpt-6.1-sol max` 审查 `01a12eb6-f6a8-7273-ade2-64d2f4ffb11b` 初审发现遮挡枚举遗漏与非超时动作回执丢失两项 P1，修后同一审查者复审回合 `01a12f08-35d2-7112-ba3b-bb6caf0e7338` PASS；CLI Spec 的测试覆盖描述已按实际修正。证据目录为 `/tmp/pawork-cu21-isolated-linux/evidence`，截图不入仓库。

复审冻结快照 `PRE-REVIEW-r4-complete.diff` 为 30 文件 / 255,345 字节，SHA256 `56191207e4cf91432eb84b2e36ce677d788bd20e02cc4a6bc660ccc1993e1fdc`。PASS 后仅同步本项审查状态，源码保持与受审摘要一致；最终差异与本地提交信息保存于 `FINAL.json` / `COMMIT.json`。

`--computer-native isolated` / `ComputerNativeMode::Isolated` 只选择本次 Host 表面；GUI 目标批准、模型工具和 child 共用唯一 `Core.native` / TargetRegistry，request 不签授权，绑定前由现有 GUI handler 明确批准。原始 `computer` 与隔离 native 复用同一 scope 和桌面占用，Run 终态释放；默认 Host 与持久设置不变。目标为 `guest.browser.firefox` / Browser，结果标记 `isolated_desktop`、`background_matrix = not_applied`。只开放非最小化截图、AT-SPI 读树和元素按压，其它动作及网站目标拒绝。

独立运行镜像为 Debian bookworm / Firefox ESR，与 arm64 Linux glibc guest 对齐；既有 Alpine 部署保留。Host 只连接 `127.0.0.1:5906`，容器内 guest 监听 `0.0.0.0:5906`，compose 仅发布宿主 loopback；RFB 保持 `127.0.0.1:5905`。共享 token 仅校验客户端秘密，不证明双向认证。最终 live 运行镜像为 `sha256:167603772419c29201ac7268ef6b573f8c2dac46e6e87f16bac80507ad116fae`，与 `guest-image-build-8` 导出的 manifest list 一致；指挥实核 uid 1000、只读根、零宿主 mounts/binds 与两个 loopback 端口（`coordinator-runtime-r5`）。此前 `coordinator-runtime-r4` 所核 `9b15b7…` 是上一运行镜像，不写成同一镜像。Linux 构建容器的源码 / cache 挂载不属于运行部署。

发现阶段仅取身份元数据，不读标题或 AT-SPI 内容。窗口绑定 PID / 启动令牌、X11 窗口与代际，以及 guest 进程 / 连接代次；AT-SPI 应用用 D-Bus `GetConnectionUnixProcessID` 核进程，按具体窗口几何找到唯一 frame，不把 Gecko 的 `Application.Id` 当 PID。动作表读取 `NActions` 属性；按压复核树版本、路径及原窗口。RFB 裁剪前后复核目标身份、几何与遮挡；归属不明拒绝。已发送请求而回执丢失保留 UnknownEffect，不自动重试。

修后遮挡枚举纳入小窗与无 class 可见窗口，读取 / 枚举预算失败拒绝；非自身重叠窗口任一侧层级未知或几何不明时拒绝。QueryTree 按真实 parent 偏移解析并拒短帧；Host 排除自身父链框架与内容，遇其它获准 client 或已知 stacking 的 rejected client 停止下降。`DoAction` 保留 typed-error，NoReply、I/O、断线等派发后的回执丢失与超时同为 UnknownEffect，明确拒绝分别报告。

指挥独立核最终 guest 二进制：Linux target、`guest-image-build-8` 的上下文副本与运行容器 `/usr/local/bin/computer_guest` 三份 SHA256 均为 `b1556428ee31af6d151c53aadca94d32b91c48ba9d06fa18bd0a3ae34260bce2`（`coordinator-binary-r5` exit 0）。COPY 缓存命中同字节，不把旧运行镜像摘要代作最终产物。

### 实际验收与自动验证

正式主路径用临时 workspace / data 与 MemoryBackend，模拟 Provider 及 GUI 用户批准，真实 `AppCore::load_for_catalog` Isolated 装配、Tool / Scheduler / Engine / Storage 和容器 Firefox 均参与。按钮 `cu21-button` 按压后，独立 X11 窗口标题变为 `cu21-marked — Mozilla Firefox`；正式 ObservationList / ObservationImageRead 分块读到单张持久 JPEG，终态 scope 关闭且下一 scope 可认领桌面。指挥另核 X11 窗口 `0x800003`、`1280x758+0+19`；JPEG `cu21-for-run-observation.jpg` 为 41,714 字节，像素中页签与正文均显示 `cu21-marked`。

| 验证 | 实际结果与证据标签 |
| --- | --- |
| Linux glibc `computer_guest` 两条修后测试与构建 | 2 passed、构建 exit 0；直接 Docker/Cargo 原始回执 `coordinator-source-call-receipts-r4.json`，不补造 stdout / .exit |
| 最终镜像与运行容器重建 | exit 0；`guest-image-build-8`、`guest-container-recreate-8`、`guest-container-start-8`（最后两条为本次恢复初态的真实调用） |
| `PAWORK_CU21_ISOLATED=1 cargo test -p pawork-app --offline --lib cu21_isolated_firefox -- --ignored --test-threads=1`（本项临时 token） | 1 passed / 311 filtered，exit 0；`cu21-live-test-5` |
| 同命令，最终 Host 修后正式链 | 1 passed / 311 filtered，exit 0；`cu21-live-test-12`，实际测试 4.24 秒 |
| `cargo test -p pawork-computer-use --lib --offline unconfirmed_overlap_and_lost_action_receipt_are_not_clean_failures -- --test-threads=1` | 1 passed / 104 filtered，exit 0；`cu21-covered-rank-2`，含共享框架的 admitted / known-stack rejected client |
| `cargo test -p pawork-computer-use -p pawork-tools --lib --offline -- <六个定向过滤名>` | computer-use 3 passed、tools 3 passed，exit 0；`cu21-refusals-3.command.json` 保存完整过滤名 |
| tools 占用与释放三条定向回归 | 3 passed，exit 0；`occupancy-claims` |
| 默认本机工具与隔离 descriptor 两条回归 | 2 passed，exit 0；`descriptor-tests` |
| guest 同 XID 换进程代际拒绝 | 1 passed，exit 0；`guest-generation-rejection` |
| `cargo check -p pawork --offline` | exit 0；`pawork-check-cu21` |
| 最终运行部署与独立 X11 核对 | exit 0；`coordinator-runtime-r5`、`coordinator-x11-r5` |
| 最终 guest target / 镜像上下文 / 运行文件三方摘要 | exit 0、SHA256 相同；`coordinator-binary-r5` |
| 修后本次 15 个 Rust 文件格式 | exit 0；`cu21-rustfmt-write15-final`，skip_children 防止扩大写入集 |
| 修后源码 diff 与文档链接 | exit 0；`cu21-write-set-diff-final`；新增文档同批检查 `coordinator-doc-links-r5` 为 287 个链接 / missing 0 |

六条拒绝回归实际覆盖坏 / 缺 token 后丢连接、未授权发现只读元数据、guest 重连 / 进程代次变化后旧窗口零树读和按压、UnknownEffect 保留及明确元素拒绝不重试、树版本长读期间撤销先于派发，以及在途取消 / 活动恢复不复活撤销。后半三条为既有正式 Host / Tool / Scheduler 替身回归，不冒充真实 guest 竞态。单 Host 的早期 tools / CLI 过滤器补齐 Cli 字段后 tools 2、CLI 1 passed，证据来自执行对话原始工具回执；当时未单独保存 stdout / .exit，不补造文件。

### 失败恢复与证据边界

首轮 musl 尝试分别 exit 127 / 137，改走独立 Debian glibc 构建；首轮 glibc 编译 exit 101 的 zbus 生命周期错误修后通过。正式 live 前四次均 exit 101，依次暴露 MutexGuard 跨 await、AT-SPI 应用身份错误、树变化拒绝与动作数量读取错误，原日志保留；第五次通过。测试中的 TreeChanged 只允许有界重新观测并取得新句柄，不放宽版本校验或重放已派发动作。

初审修复阶段的 `cu21-live-test-7` 因未显式设置 `PAWORK_CU21_ISOLATED=1` exit 101；`-8` / `-9` 按压已派发，但动作后的截图被自身 Fluxbox 框架误挡而 exit 101。根因为 QueryTree 回包将 root 字段误读为 parent；纠正偏移、短帧拒绝与真实父链后 `-10` 1 passed，仍早于最后的 Host 遮挡比较加强，不算最终 Host 同版运行。`guest-glibc-build-9` 的 typed-error 测试因缺 Debug 派生 exit 101，修后 Linux 两条动作分类 / wire 解析测试和编译通过；这次直接调用的原始工具回执保存为 `coordinator-source-call-receipts-r4.json`，不补造 shell 的 .stdout / .exit。

`cu21-live-test-11` exit 101：上次按钮状态仍为 cu21-marked，等待 cu21-ready 超时，未进入本次 Run。使用同一最终镜像重启本项专用容器，`guest-ready-cu21-1` exit 0，随后 `-12` 正式链通过。`cu21-rustfmt-all15-final` 因 mod / lib 递归展开无关历史文件 exit 1；改为本次 15 文件 skip_children 检查后 `cu21-rustfmt-write15-final` exit 0，未修改无关文件。此前的拒绝、占用、descriptor 与 pawork check 结果按未变语义复用，不声称在最后修复后重新全部执行。

`cu21-refusals` 因 Cargo 多过滤名位置错误 exit 1；`cu21-refusals-2` 虽 exit 0，实际两包均 0 tests，不计通过；修正为 harness 多过滤名后 `cu21-refusals-3` 实跑 3 + 3。格式检查的失败与修复分别保留原标签。辅助脚本参数 / D-Bus 探针错误及文档标题匹配失败只认原工具回执；未启动的检查不补写 .exit。错误参数产生的仓库临时 `--label/` 两文件已按原摘要移入证据目录：跨卷 rename 的 `coordinator-relocate-aux` exit 1，改用文件移动后 `coordinator-relocate-aux-2` exit 0；原记录保留且不作为源码提交。执行者提前派出的审查子代理已关闭，接口 422 与其它模型尝试不计本项独立审查结论。

本轮是隔离 Firefox 真实产品链与模拟用户批准验收；不证明真实 Provider、GPUI 原生窗口、GTK 扩展、最小化、Windows / Wayland 或本机后台支持。无本机键鼠、屏幕、剪贴板访问路径由源码与运行部署约束核对，未另作本机用户持续输入采样。镜像内 Firefox 内容沙箱关闭，本切片依赖专用容器隔离；不得导入个人资料。新的平台能力和系统授权分别待本项以外的专项验证。

Validated: 上表实际命令、修后 15 文件格式、diff 与 287 个文档链接；独立审查初审两项 P1 修后复审 PASS。
Targeted regressions: 上述身份、授权、旧代次、取消 / 撤销、UnknownEffect、占用及工具描述。
Full workspace gate: NOT RUN（当前未设置全量门禁）

