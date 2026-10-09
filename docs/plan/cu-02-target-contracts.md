# CU-02 · 目标、观测与后台能力契约

> 状态：✅ 完成（模拟验收，P1），独立复审 PASS。按 2026-10-09 用户授权核对当前契约与拒绝证据，不重复历史实现。前置：[CU-01](cu-01-background-feasibility.md)（已模拟验收并独立 review PASS）。更新：2026-10-09。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

在现有 computer-use / tools 内定义应用、窗口、元素和输入的最小有类型契约。

## 当前依据与读取范围

任务建立时只有隔离桌面的 status / screenshot 和坐标、文本、按键输入。当前已落地本机目标、窗口代际、AX 元素及新观测契约；`computer_native` 经当前 `NativeHost::execute` 与共享 `TargetRegistry` 接入正式 Host。观测继续有 60 秒期限和一次性消费，原生后台与隔离桌面显式区分。

[computer-use Spec](../spec/crates/computer-use.md)、[tools Spec](../spec/crates/tools.md)、[架构与冻结契约](../architecture.md)、[protocol Spec](../spec/crates/protocol.md)。

## 实施范围

- 将目标身份绑定应用、进程实例、窗口代际及 workspace/run；模型使用宿主发出的句柄，不直接提交任意 PID、绝对路径或网络端点。
- 定义窗口坐标、图像坐标、元素句柄、新观测和动作能力，区分原生后台与明确选择的隔离环境。
- 保留过期、一次性消费、边界和取消语义；具体契约依 CU-01 实测结果收敛，不提前设计通用插件框架。
- 原生能力与浏览器职责留在现有包；只有证明必要才增加 Rust 辅助入口。Core 正式宿主仍为 pawork，GUI 不直接访问 computer-use。
- 同批起草必要架构/设计更新；若需演进冻结 wire、持久格式或依赖布局，先完成仓库规定的裁决和 golden，再实施。

## 完成条件与验证

- 进程重启、窗口销毁/复用、树变化、旧句柄、跨 Run 和越界均有明确拒绝语义。
- 明确哪些动作允许后台执行；不支持不能自动回退到全局输入。
- 契约和依赖方向可供下游独立实现，Spec 只在对应实现落地后登记已实现。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

2026-10-06 实现完成，等待指挥方契约评审（人工验收）。写入集：`crates/computer-use/src/target.rs`（新增契约模块）、`crates/computer-use/src/lib.rs`（一行 `pub mod target;`）、[computer-use Spec](../spec/crates/computer-use.md)、[design.md](../design.md) 各一段同步、本任务文档与 ROADMAP。tools 无代码改动：Scope（workspace_id + run_id）绑定沿用既有宿主传参形态，工具接线属下游任务，故 tools Spec 不变。

契约要点（`pawork_computer_use::target`）：

- **目标身份**：`AppIdentity`（bundle_id + `AppFamily`{appkit, chromium, browser}，族划分与 CU-01 矩阵一致）、`ProcessInstance`（pid + start_token，重启/退出可检测）、`WindowIdentity`（window_id + generation，销毁/复用可检测）、`Scope`（workspace_id + run_id）。模型只持宿主 `TargetRegistry` 签发的透明字符串句柄（WindowHandle / ObservationHandle / ElementHandle，`{w,o,e}-<pid>-<宿主启动纳秒>-<进程级序号>`，Host 重启且 PID 复用时新值永不撞旧值），不能提交裸 PID、绝对路径或网络端点。
- **观测与坐标**：`TargetObservation` 携带 `Environment`（`native_background` 与既有 `isolated_desktop` 显式区分）、窗口句柄、图像像素与窗口点双尺寸；`ObservationKind` 区分窗口截图与 AX 树读（后者带 tree_revision，是元素句柄的唯一来源）；`image_to_window` 把模型图像像素换算为窗口坐标。
- **后台能力**：`NativeActionKind` 十动作词汇；`background_capability` 完整收录 CU-01 实测矩阵（按族 × 动作 × 最小化态返回 Supported / Unsupported / Unverified）；`require_background` 仅放行 Supported——Unverified 与 Unsupported 同拒，指针动作三族后台一律拒绝，不存在全局输入回退。
- **拒绝语义**（`TargetError`）：取消 Cancelled → 伪造/未知句柄 UnknownHandle（跨 Host 实例同 PID 旧句柄亦落入此类）→ 跨 Run / 跨 workspace CrossRun → 过期或已消费 StaleHandle。当前消费路径随后先查授权、占用与接管暂停，再复核进程退出/重启 ProcessRestarted、窗口销毁/复用 WindowReplaced、AX 树变化 TreeChanged（AX 观测消费与元素句柄同校，截图观测不受树变化影响）及观测尺寸；坐标越界 OutOfBounds。60 秒 TTL 与一次性消费沿用既有语义；消费元素句柄连带作废父观测与兄弟句柄，校验通过即消费、派发失败不回收。

验证：`cargo test -p pawork-computer-use --offline --lib`（16 过，含 target 新增 12 项拒绝语义回归）；`bash scripts/test.sh computer-use tools`（全过，exit 0，日志 /tmp/cu02-test.log）。无新编译警告。冻结契约无需演进：未触碰 GUI wire、持久格式、包布局与依赖边，无新增依赖。

审查修复（2026-10-06，6.1 sol ISSUES 轮）：能力矩阵收敛回 CU-01 实测——AppKit AXPress 改 Unverified（TextEdit 只证明 AXSetValue / AXSelectedText）、最小化态窗口移动/缩放改 Unverified；`consume_observation` 补 AX 观测的树版本校验（TreeChanged / WindowReplaced）；句柄加入进程级启动命名空间（pid + 纳秒 + 进程级 static 序号），修复 Host 重启 PID 复用时旧句柄撞新值的隐患。

2026-10-06 的后端、动作派发与 tools / Host 接线缺口已由后续任务落地，现行事实见 [CU-10](cu-10-action-observe.md)。仍保留能力矩阵中 Unverified 项（AppKit AXPress、Chrome 文本语义写、Chromium AXPress、Chromium/Browser 菜单快捷键与菜单栏按压、最小化态窗口移动/缩放）的拒绝，不因契约验收提升为 Supported。`WindowResize` / `MenuShortcut` / `MenubarPress` 只表示能力矩阵词汇，没有本机产品动作入口；原生网站因缺可信 `current_origin` 仍 fail-closed。

## 本轮契约模拟验收（2026-10-09）

基线 `main` / `6473b11f08e09895c85351261f046ca8710dbf7a`。`xai/grok-4.7 xhigh` 核对当前源码与已有回归，源码、公开 API、冻结 wire / 持久格式和依赖均无改动；本项同步计划中的历史缺口与当前证据，并按初审意见回写 computer-use Spec 的观测/元素消费顺序。

| 验收输入 | 当前实现与可观察拒绝 | 有效证据 |
| --- | --- | --- |
| 进程退出/重启、Host 重启后的旧句柄、窗口销毁/复用 | 分别 ProcessRestarted / UnknownHandle / WindowReplaced，不能借旧身份派发 | `process_exit_or_restart_invalidates_everything`、`handles_from_a_previous_host_instance_are_unknown`、`window_destroyed_or_identifier_reused_invalidates` |
| 树版本、60 秒期限、消费父/兄弟句柄 | AX 版本不符 TreeChanged；过期/已消费 StaleHandle，截图不绑定树版本 | `tree_change_invalidates_element_handles`、`expired_leases_are_stale`、`observation_is_one_shot_and_scope_bound`、`consuming_any_handle_invalidates_its_siblings` |
| 跨 workspace / Run、坐标非有限值及右/下边缘 | CrossRun / OutOfBounds，错误路径不消费有效租约 | `observation_is_one_shot_and_scope_bound`、`image_coordinates_map_into_window_bounds_only`、`pointer_actions_validate_against_observed_bounds`；正式 Tool / Host 跨 scope 截图拒绝且 capture 不增 |
| 取消、活动、撤销和尺寸变化 | Cancelled / UserActive / NotAuthorized / StaleHandle 在成功消费前拒绝；活动、尺寸或授权恢复不伪造新的派发事实 | `cancellation_precedes_consumption`、`user_takeover_pauses_dispatch_without_consuming`、`window_resize_stales_observations_and_element_handles`、`revocation_blocks_dispatch_before_backend_access_and_history_stays`；CU-01 的正式 Host 在途停止及撤销后 Idle 模拟 |
| 未授权、跨目标、受保护目标 | 台账拒绝先于 probe，计数为零；网站跨源校验需要 origin 探针，不混作零 probe | `unauthorized_and_cross_target_rejections_precede_any_probe_call`、`protected_targets_are_rejected_before_any_probe_call` 及 macOS 动作缝拒绝回归 |
| Unsupported / Unverified、三族 pointer | BackgroundUnsupported，正式产品链 pointer 拒绝后 move / capture 不增加，无全局或前台输入回退 | `require_background_passes_only_supported_never_falling_back`、`capability_matrix_matches_cu01_measurements`；`native_host_action_returns_one_observation_and_rejects_closed_paths` |

上表复用 `/tmp/pawork-cu10-glm-final/17-computer-use-tests.log`（99 passed，exit 0）与 `/tmp/cu01/host-stop-20261009.log`（6 passed，exit 0），未重跑 Cargo。`target.rs`、`macos.rs`、`native_host.rs`、`lib.rs`、`approval.rs` 的当前字节均等于 CU-10 最终受审清单 `ROOT-REVIEW-R3.json`；六份相关源码磁盘字节均等于当前 HEAD，写入时间早于各自引用日志。逐文件 SHA256、日志 SHA256 和来源日期见 `/tmp/cu02/fingerprint-20261009.txt`（SHA256 `6c1367f81b35710b1fb5776566596549f5d63a46da5d2b3b7d35eb8d4692570a`）。本轮是当前契约与正式入口的模拟证据核验；真实窗口、AX/TCC、系统权限拒绝态未新验，不用旧探针或静态能力表代替产品动作证据。

```text
Validated: 字节/HEAD/完整差异核对；复用同版 cargo test --offline --tests -p pawork-computer-use（99 passed，exit 0）与 pawork-tools computer_native::tests（6 passed，exit 0），本项未新跑 Cargo
Targeted regressions: 上表 target/macOS 拒绝锚点 + 正式 Tool/Host 跨 scope、pointer 零派发、接管与取消
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

独立 `gpt-6.1-sol max` 初审（turn `01a1202a-ec81-7703-aa82-c053bdabe1c8`，completed / error=null）指出一项 P2：包级 Spec 把占用检查写在过期检查之前，与源码和本计划不符。已将 Spec 限定为观测/元素消费路径，并按当前源码同步为过期/已消费 → 授权 → 占用 → 接管暂停；仅修文档，源码和同版回归证据不变，无需重跑 Cargo。

原 reviewer R2 复审（turn `01a12036-4b64-7b33-886b-975687ee107a`，completed / error=null）裁决 PASS，P2 闭合，无剩余必须修项。完整受审 3 文件 / 18865 字节 / SHA256 `5fac27381a479b7552234225e5962d081cf34b6a7bdf7a9b358c96d7a2f80bf0` 与工作区逐字节一致；diff 和 131 个相对链接检查均 exit 0。指挥再次核对 HEAD、空索引与逐文件哈希后，仅回写完成/审查元数据并精确本地提交；包级 Spec、源码和日志保持受审字节。真实窗口、AX/TCC 和系统权限拒绝态仍按上方环境边界单列，本项闭合不代表其它任务或发布完成。
