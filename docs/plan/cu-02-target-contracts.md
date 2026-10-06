# CU-02 · 目标、观测与后台能力契约

> 状态：等待人工验收（P1）。前置：[CU-01](cu-01-background-feasibility.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

在现有 computer-use / tools 内定义应用、窗口、元素和输入的最小有类型契约。

## 当前依据与读取范围

现行 Action 只有 status / screenshot 和坐标、文本、按键输入；Observation 绑定 scope、连接与尺寸，60 秒失效且每次输入消费一次。没有本机目标、窗口代际或 AX 元素句柄。

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
- **拒绝语义**（`TargetError`，校验顺序固定）：取消 Cancelled → 伪造/未知句柄 UnknownHandle（跨 Host 实例同 PID 旧句柄亦落入此类）→ 跨 Run / 跨 workspace CrossRun → 过期或已消费 StaleHandle → 进程退出/重启 ProcessRestarted → 窗口销毁/复用 WindowReplaced → AX 树变化 TreeChanged（AX 观测消费与元素句柄同校，截图观测不受树变化影响）；坐标越界 OutOfBounds。60 秒 TTL 与一次性消费沿用既有语义；消费元素句柄连带作废父观测与兄弟句柄，校验通过即消费、派发失败不回收。

验证：`cargo test -p pawork-computer-use --offline --lib`（16 过，含 target 新增 12 项拒绝语义回归）；`bash scripts/test.sh computer-use tools`（全过，exit 0，日志 /tmp/cu02-test.log）。无新编译警告。冻结契约无需演进：未触碰 GUI wire、持久格式、包布局与依赖边，无新增依赖。

审查修复（2026-10-06，6.1 sol ISSUES 轮）：能力矩阵收敛回 CU-01 实测——AppKit AXPress 改 Unverified（TextEdit 只证明 AXSetValue / AXSelectedText）、最小化态窗口移动/缩放改 Unverified；`consume_observation` 补 AX 观测的树版本校验（TreeChanged / WindowReplaced）；句柄加入进程级启动命名空间（pid + 纳秒 + 进程级 static 序号），修复 Host 重启 PID 复用时旧句柄撞新值的隐患。

遗留缺口：`NativeProbe` 的 macOS 实现与动作派发未做（CU-03+）；契约未接入 tools / Host 生产路径；能力矩阵中 Unverified 项（AppKit AXPress、Chrome 文本语义写、Chromium AXPress、Chromium/Browser 菜单快捷键与菜单栏按压、最小化态窗口移动/缩放）需后续实测才能提升为 Supported；人工验收即指挥方对契约形状与拒绝语义的评审。
