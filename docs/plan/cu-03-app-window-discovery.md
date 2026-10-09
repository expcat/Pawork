# CU-03 · 应用发现、窗口选择与受控启动

> 状态：✅ 已实现，自动门禁与模拟验收通过；独立 review PASS（P2）。前置：[CU-02](cu-02-target-contracts.md)、[CU-16](cu-16-target-approvals.md)（均已模拟验收并独立 review PASS）。更新：2026-10-09。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

让 Agent 选择已授权的真实应用与窗口，并按目标身份受控启动应用。

## 当前依据与读取范围

当前 macOS 后端已提供授权后的窗口列举与受控启动，CU-10 已将应用发现、单窗口绑定和启动接入共享 NativeHost / 正式工具。本轮补同应用多窗口的正式选择：沿用 bind / launch，在原 registry 为各窗口签发句柄，模型明确回传其中一个；动作后观测精确匹配所选窗口。当前产品链的 8 条工具回归及 99 条库回归通过，独立 review PASS。

[computer-use Spec](../spec/crates/computer-use.md)、[tools Spec](../spec/crates/tools.md)、[app Spec](../spec/crates/app.md)。

## 实施范围

- 使用系统应用/窗口 API 生成最小目标描述与宿主句柄；未授权内容不扩展为标题、截图或 AX 读取。
- 按 CU-02 校验目标实例；启动只接受已发现且获授权的应用身份，不让模型传任意可执行路径。
- 启动、选择或刷新目标不激活前台、不夺取焦点；系统权限缺失诚实返回，不自动批准。

## 完成条件与验证

- 相同应用的多窗口能正确区分，关闭或重启后旧句柄失效。
- 启动和选择目标过程中用户输入连续，未授权应用在读取或启动前被拒绝。
- 优先现有定向回归；有真实应用与窗口事实支持能力结论。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 历史后端实施记录（2026-10-06）

2026-10-06 实现完成，真实验收通过，等待指挥方评审（人工验收）。写入集：`crates/computer-use/src/macos.rs`（新增 macOS 原生后端）、`crates/computer-use/src/target.rs`（`TargetError` 增第十四变体 `PermissionMissing`、第十五变体 `ProbeUnavailable`）、`crates/computer-use/src/lib.rs`（一行 cfg-gated `pub mod macos;`）、`crates/computer-use/Cargo.toml`（macOS target-gated 正式依赖与 lints）、`crates/computer-use/examples/macos_target_probe.rs`（新增人工验收探针）、[computer-use Spec](../spec/crates/computer-use.md)、[design.md](../design.md)、本任务文档与 ROADMAP。未接入 tools / Host 生产路径，tools / app 无代码与 Spec 改动。

实现要点：

- **应用发现与读取授权闸**：`MacosNative::list_applications` 列 Regular 激活策略应用（bundle id + 启发式族 + 进程实例 + 显示名 + 窗口服务器 layer-0 计数，可能含幽灵），只读元数据、不读标题、不读 AX 树；`resolve_identity` 不启动解析已安装应用身份；`list_windows(authorizer, scope, identity, instance, include_titles)` 先核验目标与 scope 授权（未授权连一次 AX 调用都不发生），并经 NSRunningApplication 回读 bundle id 校验身份与进程实例绑定（错配按 Invalid 拒绝），标题仅在授权调用显式要求、且窗口属于获授权 pid 名下时复制出窗口服务器。
- **进程实例与窗口代际**：进程实例 = pid + `kern.proc.pid` sysctl 启动时间（微秒）+ NSRunningApplication `isTerminated`；枚举、代际账本同步与代际提取在同一把互斥锁内完成（旧快照永远覆盖不了新账本），账本由 `MacosNative` 克隆共享，消失的 id 遗忘、复用的 id 拿新代际。
- **幽灵窗口与存活判定**：macOS 已关闭窗口在 CGWindowList（含 SCShareableContent）长期残留（系统未承诺回收时机），属性与活窗一致——存活 = 窗口服务器在册 + 应用自身 AXWindows 证明。对应两级：`_AXUIElementGetWindow`（HIServices 私有导出）可用时按窗口服务器 id 精确映射（幽灵 id 永不映射，同帧关窗只失效自己，幸存同帧窗继续有效）；导出缺失或失败时退化为帧组闭合（同帧服务器计数 = AX 计数才判活，任何一侧多余即歧义 fail-closed，组内 AX 窗同时匹配组外 CG 窗同样拒绝——epsilon 帧匹配不可传递；同帧幽灵会连带失效幸存窗直到被回收）。
- **受控启动**：`launch_authorized` 授权检查先于一切系统调用（未授权 / 受保护目标在触碰 LaunchServices 前拒绝，回归以启动缝调用计数钉零），只接受 bundle 身份（不存在可执行路径入参），恒 `NSWorkspaceLaunchWithoutActivation` 不激活、不夺焦点；`LaunchError::{Denied, NotFound, Failed, ExitedEarly}`。
- **系统权限与失败分类**：`permissions()` 只读 preflight（Screen Recording + Accessibility），永不触发 TCC 请求；Accessibility 缺失按 `PermissionMissing("accessibility")` 拒绝，权限在而 AX 调用失败（应用无响应 / 属性不支持 / 符号缺失，kAXErrorAPIDisabled 仍归权限类）按 `ProbeUnavailable(原因)` 拒绝，两者都拒绝访问。
- **fail-closed 缺口**：`tree_revision` / `current_origin` 无信号源恒 None——AX 树绑定与网站目标在 macOS 后端暂不可用，按契约拒绝而非静默放行。

验证：`bash scripts/test.sh computer-use`（41 过，含 macos 十项单测，日志 /tmp/cu03-fix2-test.log）；`bash scripts/test.sh tools`（80 过，确认无回归，日志 /tmp/cu03-fix2-tools.log）；全部 examples 构建零警告。冻结契约未触碰：无 GUI wire、持久格式、包布局与 `pawork-*` 依赖边变化；新增依赖均为 workspace 已锁定版本的 macOS target-gated 引用。

真实验收（2026-10-06 第二轮复审修复后重取，macOS 本机，证据存 /tmp/cu03/：perms3.json、apps4.json、deny5.json、launch3.json、wins3-*.json、watch8-sameframe / watch9-minimize / watch10-quit.jsonl、launch-safari.json，不入库）：未授权窗口读取先于任何 AX / 标题访问拒绝（list_without_grant=NotAuthorized）；授权后未要求 include_titles 时标题不复制，要求时仅获授权 pid 名下窗口带标题（wins3-auth / wins3-noauth）；身份-进程绑定实证——TextEdit 授权 + Edge 进程实例按 Invalid 拒绝（deny5.json 的 list_identity_mismatch）；未授权启动与撤销后绑定均 NotAuthorized、受保护目标授权与启动均 ForbiddenTarget；三窗压到同一帧（axsetframe）后关闭其一，仅被关句柄失效、同帧幸存句柄继续有效（精确 id 模式实证），wins3-afterclose 复验幸存窗在列；退出 → ProcessRestarted（watch10-quit，Safari）；启动前后 frontmost 一致（launch3.json front_unchanged=true），Safari 受控启动复验通过（launch-safari.json）；最小化本轮实测 fail-closed——macOS 26 下 TextEdit 含最小化窗口时整应用 AXWindows 元素失效，按 WindowReplaced 拒绝（watch9-minimize），与首轮「最小化持续有效」并存为平台状态差异，契约不承诺最小化存活；全程前台采样保持用户应用（Edge / Codex）。Edge 只读列举复验通过（未 quit 用户应用）；Calculator（SwiftUI）AX 窗口天然缺 AXPosition/AXSize，不能做验收 app；TextEdit / Calculator / Safari 验收后均已由探针退出。

审查修复（2026-10-06，6.1 sol ISSUES 轮，5 项必须修 + 1 项建议）：读取授权闸——list_applications 不再对全部应用做 AX 读取（窗口计数退为窗口服务器元数据），list_windows 改为先核验授权再碰 AX / 标题，标题仅在授权调用 include_titles 时复制出窗口服务器字典；启动拒绝测试改用启动缝调用计数证明零系统调用。窗口身份——任意 AX 帧匹配不是身份证据（同帧幽灵可借活窗的帧复活），改两级对应：_AXUIElementGetWindow 精确 id 映射，缺失时帧组闭合、歧义 fail-closed；补同帧关窗 / 幽灵复活 / 计数外溢定向回归。代际账本竞态——枚举、账本同步与代际提取并入同一把互斥锁。AX 失败分类——权限预检与 AX 调用失败分开，kAXErrorAPIDisabled 归 PermissionMissing，其余保留原因归 ProbeUnavailable（TargetError 增第十五变体）。证据边界——完成记录按修复后证据重取（同帧关窗、最小化、front_unchanged 一致性补齐），AGENTS.md / Spec / design 的「无限期残留」「帧匹配即存活」表述收敛为「长期残留」「帧组闭合」。建议项——unexpected_cfgs 保持 warn，仅 check-cfg 豁免已知值。第二轮（同日 6.1 sol 复审，5 项必须修）：标题按 owner pid 门控——enumerate_windows 仅在窗口属于获授权 pid 时复制 kCGWindowName，list_applications / window_generation 不再携带标题；身份-进程绑定——running_bundle_id(pid) 经 NSRunningApplication 回读 bundle id，list_windows 在存活检查后比对授权身份，错配按 Invalid 拒绝；降级重叠拒绝——帧组闭合增加跨组重叠检查，组内 AX 窗同时匹配组外 CG 窗即拒绝（epsilon 匹配不可传递，定向回归 frame_group_overlap_fails_closed）；AX 常量修正——APIDisabled=-25211 / CannotComplete=-25204（曾写反），新增 AxPreflight 三态，axf() 符号缺失按 ProbeUnavailable 传播，ax_is_process_trusted 改经预检，定向回归钉 -25211→权限、-25204→probe；启动计数测试合并为 launch_authorization_gates_the_system_seam（拒绝零调用 + 已授权恰好一次，消除并行竞态）。另新增真实进程回归 list_windows_rejects_identity_process_mismatch（macos 单测达十项），探针新增 axdump 诊断命令（在获授权进程内读 AX 真值——无授权进程读 AXWindows 不报错而是拿到标题=应用名、属性全失败的假元素，swift 脚本 dump 不可信）。

当日遗留缺口：AX 树版本与网页 origin 无信号源，且后端尚未接入 tools / Host。后续 CU-05 已补树版本、CU-10 已补正式 Host / Tool 接线；当前原生网站 origin 仍不可得，模型网站请求在 resolve 前拒绝。帧组闭合退化模式下同帧幽灵会连带失效幸存窗直到被回收（精确 id 模式无此限制）；最小化窗口的 AX 可见性依赖平台状态（macOS 26 TextEdit 实测最小化可致整应用 AX 元素失效、属性全 AttributeUnsupported），契约不承诺最小化存活，不可验证即 fail-closed；族分类启发式有局限（无 Chromium 框架信号的壳应用归入 Appkit）。

## 本轮多窗口产品链模拟验收（2026-10-09）

基线 `main` / `3fdeb678c6495ce29c12b4f42a7a2e4c856a676a`。执行 `xai/grok-4.7 xhigh`，对话 `01a12053-4019-7271-bb48-f5987d36f3d5`，turn `01a12053-43b8-74b3-9457-7e98b69ceff2`（completed / error=null）。源码只改 `native_host.rs`、`target.rs` 与 `computer_native.rs`，无新 action、依赖、冻结 wire / 持久格式或第二注册表。

原 Host 仅接受恰好一个窗口；本轮沿用 bind / launch 的正式模型入口，在唯一活实例的多个窗口之间返回 `Host句柄 宽x高` 的普通 Text 列表，`window_id=null`，不返回标题、PID 或原生窗口 ID，不猜默认窗口。模型回传其中一个句柄继续截图或动作，单窗口结果不变。`bind_window_with` 在原授权、占用、进程及 origin 闸通过后复用同 target / scope / instance / window（含代际）的句柄；动作后的 `handle_for` 也精确回查这些字段。GUI 每目标仍一行，以句柄字典序取展示代表，不用该聚合行绑定动作后观测。

| 实际输入 → 产品链 | 可观察结果 | 证据 |
| --- | --- | --- |
| discover → Native Tool / Scheduler / Host；未授予目标授权的 bind | 发现只列元数据 / 窗口计数；bind 为 NotAuthorized，list / launch 均零调用 | 新多窗口主路径 |
| 明确 ForRun grant 后 bind 两个窗口，重复 bind | 返回不同 Host 句柄、200x100 / 640x480 两行；无默认 window_id；重复返回相同句柄 | 同主路径 |
| 回传宽窗句柄 screenshot，再回传窄窗句柄；对宽窗 move_window | 平台捕获依次收到窗口 22 / 11 / 22；一次 move 的新观测返回宽窗句柄、input_dispatched=true，未错绑窄窗 | 同主路径，经真实 Tool / Scheduler / NativeHost，平台边界使用既有替身 |
| 关闭宽窗后 screenshot；继续截窄窗 | 宽窗 WindowReplaced 且零 capture；窄窗仍捕获窗口 11 | 新关键拒绝路径 |
| 两窗恢复，Once 在授权后的 list_windows 等待期间撤销 | 原 Host 入口返回 NotAuthorized，不复用旧句柄；capture 不增加 | 同拒绝路径的真实 Host 并发调用 |
| scope / 进程 / 代际、protected、授权及受控启动门序 | 沿用当前 99 条库回归的各自锚点；不把库回归数量当作新增完整模型 launch 往返 | `04-computer-use-lib.log` |

新增仅一条多窗口主路径与一条关键关闭 / 授权拒绝路径，扩展既有平台替身来观察所选窗口和调用次数。初版八条工具测试已经通过，但未覆盖动作后观测错绑与复用授权复查，指挥定位后在原执行对话修正并扩现有两条用例；最终按修后源码重跑。首次日志 `01-tools-computer-native.log` 保留：Cargo 输出八条通过，但外壳使用 zsh 只读变量 `status` 导致命令 exit 1，不能登记为整个命令通过；改用普通返回码变量并另存 `03` 日志，未覆盖原失败。

最终原始日志 `/tmp/cu03-20261009/03-tools-computer-native.log`：8 passed / 85 filtered / EXIT:0；`04-computer-use-lib.log`：99 passed / EXIT:0；仅保留既有 block future-incompat 提示。执行者源码快照 `/tmp/cu03-20261009/source.diff` 为 28364 字节，SHA256 `6253e355da589b3c79ffb473e75de4c1389cca6bf8187d5f8cc12d7d8b7fdbc1`。

环境边界：本轮没有真实 AX/TCC、Provider、用户既有窗口、GUI 选窗按钮或连续前台输入验收；不把平台替身的 JPEG 哨兵当真实可解码截图。`macos.rs` 未改，旧 `/tmp/cu03` 探针仅保留其当日后端事实，启动不激活、幽灵和最小化限制按原后端处理，未声称本轮重跑。launch 与 bind 共用本次多窗口结果路径，库回归覆盖原启动门序，未单跑新增模型多窗口 launch 成功往返；GUI 交互属 CU-18。

```text
Validated: cargo test -p pawork-tools --offline --lib computer_native::tests:: -- --test-threads=8（8 passed / 85 filtered，exit 0）；cargo test -p pawork-computer-use --offline --lib -- --test-threads=8（99 passed，exit 0）；三文件定向 rustfmt
Targeted regressions: 多窗口明确选择及动作后精确观测绑定；关闭旧句柄零 capture；列举期间撤销不复用；既有六条 Host / Tool 边界与 99 条库锚点
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

唯一独立 `gpt-6.1-sol max` reviewer 对话 `01a1206d-8eca-7ac0-9496-bdfef0f0f00e`，turn `01a1206d-92c2-70a3-9dd8-7b8b59ae3919`（completed / error=null）给出 PASS。受审完整八文件快照 `/tmp/cu03-20261009/ROOT-REVIEW-R1.diff` 为 65264 字节，SHA256 `bdf931b13943d28bb4ad070e9aa1ba56686ee2af3ad59ddd46b0e7f8b01838e9`；原始结论存 `REVIEW-R1-PASS.txt/json`。提交前重核差异与全部文件指纹未变，只回写本项完成状态及审查元数据。
