# CU-04 · 窗口级截图与观测坐标

> 状态：✅ 已实现，自动回归与模拟验收通过；独立 review PASS（P2）。前置：[CU-03](cu-03-app-window-discovery.md)（已模拟验收并独立 review PASS）。更新：2026-10-09。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

提供目标窗口截图与可供动作使用的新鲜观测。

## 当前依据与读取范围

现有 macOS 后端提供窗口级 ScreenCaptureKit JPEG，最长边 1280、最多 512 KiB；共享 NativeHost / 正式工具截图接线已由 CU-10 完成。观测绑定目标窗口、尺寸、代际与租约，CU-03 已补同应用多窗口的明确句柄选择。本轮按用户授权核产品链模拟与拒绝边界，历史平台像素记录保留其日期与探针范围。

[computer-use Spec](../spec/crates/computer-use.md)、[tools Spec](../spec/crates/tools.md)。

## 实施范围

- 按 CU-01 验证的 ScreenCaptureKit 窗口捕获路线实现，不通过激活目标获得截图。
- 绑定目标、进程/窗口代际、时间、尺寸和缩放；处理窗口移动、缩放及实际支持的显示器组合。
- 保留图像/像素/字节/超时上限，捕获失败不返回旧图或残帧。窗口不可捕获时返回真实原因。
- 截图仍进入现有 ContentPart::Image 与持久消息通路，不提前改成模型无法解析的 Artifact 图片。

## 完成条件与验证

- 窗口被遮挡时截图仍对应授权目标，用户前台和焦点不变。
- 动作坐标严格映射到所观察窗口，尺寸变化后旧观测不能派发。
- 真实像素与窗口事实一致，截图权限拒绝在捕获前生效。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 历史后端实施记录

2026-10-06 实现；2026-10-07 按 6.1 sol 复审意见完成 5 项必须修并重取当日后端验收证据。当时等待人工验收；本轮模拟结论另列。

**实现**（写入集：`crates/computer-use/src/target.rs`、`crates/computer-use/src/macos.rs`、`crates/computer-use/Cargo.toml`、`crates/computer-use/examples/macos_target_probe.rs`）：

- `macos` 模块新增 `capture_window(authorizer, validated)`：ScreenCaptureKit 单帧窗口捕获（CU-01 实测路线，`SCContentFilter initWithDesktopIndependentWindow`，桌面无关、不激活目标），门槛顺序为台账授权 → 进程实例 → 身份绑定（实例 pid 的 bundle id 系统回读须等于获授权身份，与 `list_windows` 共用 `verify_instance_owns_bundle`）→ 窗口存活 + 代际（CGWindowList × 应用 AXWindows 双条件，复用 CU-03 `proven_window` 快照）→ Screen Recording preflight（拒绝在捕获缝之前生效，探针缝以调用计数钉住）；SCWindow 按窗口 id + 属主 pid 双因子定位，捕获与存活证明之间关窗按 WindowReplaced 拒绝，其余捕获失败保留 OS 原因按 ProbeUnavailable 拒绝；成功或报错，无旧图/残帧路径。
- 上限沿用现有口径：原始像素单边 ≤4096、总像素 ≤8M（同 RFB framebuffer 上限），捕获比例 ≤2x；输出 JPEG 最长边 ≤1280、字节 ≤512KiB（质量阶梯 75/50/30，同隔离桌面）；completion handler 统一 10 秒超时。产物是基线 JPEG，继续走现有 ContentPart::Image 与持久消息通路，无新 Artifact 形态。
- 契约层新增 `NativeProbe::window_size` 接缝（存活证明同一 fail-closed 纪律），`begin_observation` / `consume_observation` / `consume_element` 在树版本校验后比对观测记录的窗口尺寸与实时尺寸（±0.5pt 容差，与后端帧容差一致）：不一致按 StaleHandle（重新观测即可恢复，窗口绑定不受累），不可得按 WindowReplaced fail-closed；坐标映射不变（`image_to_window` 图像像素 → 窗口点）。时间戳与一次性语义沿用 CU-02 租约（60 秒、消费即废）。
- `block 0.1` 由 target-gated dev-dependencies 提为 target-gated 正式依赖（SCK completion handler 所需；非 macOS 目标仍无平台依赖）。探针新增 `capture` / `obsflow` / `capturegone` / `axunminimize` 命令。

**复审修复**（2026-10-07，审查结论 ISSUES 的 5 项必须修）：

- 截图铺满与坐标诚实：`SCStreamConfiguration` 增 `scalesToFit=true`——此前只设输出尺寸未设缩放，SCK 把窗口内容按原生尺寸画在配置帧左上角、其余留黑（1280×835 图中真实内容仅约 640×418），`image_to_window` 的整图换算会把内容位置映射到错误窗口点。修复后窗口内容铺满整帧，坐标映射以整图为基准成立，并以真实内容位置复验（见下）。
- 捕获身份绑定：`capture_window` 在任何 AX 读取前复用 `list_windows` 的身份核验（提取共享 `verify_instance_owns_bundle`）：实例 pid 经 NSRunningApplication 回读的 bundle id 不等于获授权身份即按 Invalid 拒绝——TextEdit 授权搭配另一应用的实例与窗口不再能通过捕获（此前 `ValidatedWindow` 公开字段可拼合通过）；门序回归改钉此分支。
- 异步所有权：completion 状态机重写为显式所有权（`Completion` 结构 + `deliver` / `wait_completion` + 释放钩）：超时先标记 abandoned 再返回，迟到回调由 handler 侧立即释放已 retain 的对象，结果恰在超时与加锁之间落盘时由等待方释放，filter/config 在超时、错误、nil 全路径释放（此前 `?` 提前返回跳过释放，迟到帧与 shareable content 均泄漏）；第二轮复审再补「错误伴随结果」分支——回调同时返回非空结果与 error 时已 retain 对象原样泄漏，现由 deliver 统一释放，错误交付永不携带已 retain 指针。
- 整数像素上限：输出尺寸逐轴 floor 并复核整数乘积（此前独立四舍五入可突破预算：2000×2001pt → 2828×2829 = 8,000,412 > 8M）；`checked_frame_layout` 在复制实际像素前对交付帧复核单边 ≤4096、总像素 ≤8M、stride ≥ width×4。
- 验收证据与表述：原始探针输出全部落盘（/tmp/cu04/*.json）；焦点经系统级 `AXFocusedApplication` 观测——进程冷启动时该查询稳定返回 CannotComplete（-25204），须先建 WindowServer 连接（CGWindowList 枚举一次），探针已内置预热与重试，观测不到即如实报 focus_observed=false；Screen Recording 拒绝分支仍未实测（见遗留缺口）。

**定向回归**：`bash scripts/test.sh computer-use` 全绿（49 项）。契约层 `window_resize_stales_observations_and_element_handles`（注册时尺寸不符即拒且不误耗租约、消费时缩放 StaleHandle、恢复尺寸后在途租约可放行、元素句柄同规、尺寸不可得 WindowReplaced）；`macos` 层：捕获门序单一测试——未授权 → NotAuthorized、死进程 → ProcessRestarted、身份错配 → Invalid，均先于 SCK 捕获缝且调用计数钉零（窗口不存在 → WindowReplaced 分支由 capturegone 真实验收覆盖）；`capture_scale` 像素上限；`capture_pixel_size` 取整复核（2000×2001pt 边界乘积不破 8M）；`checked_frame_layout` 交付帧预算（超边 / 超积 / 退化 / stride 不足均拒）；BGRA→RGB 布局校验（截断/不一致拒绝）；JPEG 预算（噪声 3000×2000 → 1280 边 ≤512KiB 可解码往返）；completion 所有权（超时后到达的结果与错误伴随的结果各被释放恰好一次，正常交付不释放，错误交付不携带指针）。

**真实验收**（2026-10-07 重取，macOS 26.6.2 arm64 双显示器；探针为 `cargo build -p pawork-computer-use --example macos_target_probe` 构建的 target/debug/examples/macos_target_probe；Screen Recording 与 Accessibility 经责任进程链授予；目标 TextEdit pid 39584、窗口 30242「cu04-doc.txt」，经探针不激活启动并打开 /tmp/cu04/cu04-doc.txt，全程处于后台；原始探针输出与 JPEG 存 /tmp/cu04/，不入库；验收后窗口帧还原、TextEdit 退出）：

- 像素验证（捕获内容 = 授权窗口自身且铺满整帧）：`capture` 返回 1280×835 / 38.7KB JPEG（capture-doc.json / capture-doc.jpeg），四边缘带平均亮度 216–249、content_fills_frame=true，人工查看即目标文档内容（标题栏 / 工具栏 / 标尺 / 三行文本齐全、无黑边）；最小化态（capture-minimized.*）与副屏 x=-1300（capture-display2.*）捕获同规，均 content_fills_frame=true、内容一致。
- 坐标复验（真实内容位置）：`obsflow`（obsflow.json）观测 673×439 → 语义缩放为 833×529 后旧观测消费按 StaleHandle 拒绝；新尺寸重新观测后仅移动（+48,+48）派发成功；图像中心 (640, 406.5) → 窗口点 (416.5, 264.5) = 833×529 的精确中心；移动后再捕获与移动前逐像素一致率 1.0（moved_content_match_ratio）——同一文档内容留在同一图像相对位置，证明映射是窗口相对而非公式自洽；结束还原窗口帧。
- 前台采样与焦点观测：每条 capture 命令前后采样 frontmost（NSWorkspace）与焦点（系统级 `AXFocusedApplication`，探针预热 WindowServer 连接后观测）：全程 front_unchanged=true、focus_unchanged=true，frontmost 与焦点均为用户应用 Edge（pid 916），鼠标位移 0；同一命令内未授权捕获先按 NotAuthorized 拒绝。
- 关窗诚实性：`capturegone`（capturegone.json）先捕获成功，语义关窗后同一句柄捕获与校验均按 WindowReplaced 拒绝，无残帧/旧图。

**当日遗留缺口与后续接线**：Screen Recording 拒绝分支未实测——本机授权沿责任进程链继承、无法为探针单独撤销；现行代码的 preflight 位于捕获缝之前，既有门序回归钉住早期授权拒绝，不能登记为真实 Screen Recording 撤销分支已实测。生产 tools / Host 接线已由 CU-10 完成，GUI 观测查询由 CU-17 提供；当前不能沿用当日“未接入生产路径”的结论。

## 本轮产品链模拟验收（2026-10-09）

基线 `main` / `2db77c5c791e3793ce7669ea3ae4d1c10647ac6a`。执行 `xai/grok-4.7 xhigh`，对话 `01a12076-4983-77d1-9169-4cdf0b5faf12`，turn `01a12076-4d77-7f62-8271-738657a387bd`（completed / error=null）。生产实现未改；只扩展 tools 原有平台替身和两条多窗口用例，仍为八条测试。JPEG 编解码复用 lock 中已有 `image = 0.25.10`，仅加 tools 的 jpeg-only dev-dependency 和对应 lock 依赖边，不新增生产依赖或包。

| 实际输入 → 产品链 | 可观察结果 | 证据 |
| --- | --- | --- |
| 模型回传宽/窄窗 Host 句柄 screenshot → ToolScheduler → NativeComputerTool → NativeHost（替身平台窗口 22 / 11） | 所选窗口 RGB 缓冲在外部平台替身 capture 时编码；产品第二段 ContentPart::Image 的 base64 解码字节等于该次回帧记录。32×16 JPEG 可解码，中心分别为红 `[220,10,10]` / 绿 `[10,160,30]`（各通道容差 ±12），两图不同 | 原多窗口主用例，`02-tools-computer-native.log` |
| 窗口 22 move_window 到 (12,8) → 动作后新 capture / observation | 仅该窗原点改变，窗口 11 仍 (0,0)；窗口 22 图像字节不变，observation id 更新且仍绑定所选窗 | 同用例；模拟窗口相对内容与正式回帧路由 |
| read_tree → find_element；窗口 22 缩放至 800×480 → semantic set_value | 旧元素观测按 StaleHandle 拒绝，semantic 零调用、无新增帧；重新观测是恢复入口 | 同用例，经真实 registry 尺寸闸 |
| 模拟平台 Screen Recording 不可用 → screenshot | PermissionMissing，图像捕获计数和回帧数不增加 | 同用例；只证明平台错误在正式工具链的传播，不是真实 TCC 撤销 |
| 模拟捕获帧边长 1281 → adopt_capture / geometry_for | 预算错误，工具不返回图像或旧帧 | 同用例；既有库回归另覆盖编码/像素/字节上限 |
| 关闭窗口 22；继续截幸存窗口 11；列举期间撤销 Once | 关闭旧句柄 WindowReplaced 且零 capture；幸存窗可用；撤销后 NotAuthorized，不复用旧句柄 | 原关键拒绝用例 |
| 显式重新 ForRun，进程退出后截旧窗口句柄 | ProcessRestarted，capture / 回帧计数不增加 | 同拒绝用例；先满足授权前置再测进程闸 |

新增几何辅助原用手填宽高重造 TargetObservation，只证明换算公式，已按指挥意见删除。当前原生 Pointer 仍在正式入口按 BackgroundUnsupported 拒绝，本轮未声称图像坐标点击或坐标换算的新实际派发。几何与租约以当前同版 computer-use 库回归为锚，旧 `obsflow.json` 的中心点 `(640,406.5) → (416.5,264.5)` 仅保留 2026-10-07 探针事实。

失败与恢复均保留：`00-aborted-pre-cleanup.log` 是执行者停止旧命令后保留的输出，外壳原始记录 exit 143，日志实际已报 7 passed / 1 failed，没有成功退出标记；清理低信号辅助后的 `01-failed-auth-before-restart.log` 仍 7 passed / 1 failed、Cargo EXIT:101。该失败因为 Once 已撤销，授权闸先于进程检查，测试却期待 ProcessRestarted；只补显式 ForRun 前置，不改生产门序。`02-tools-computer-native.log` 才是最终修后 8 passed / 85 filtered、Cargo EXIT:0。外壳末尾 echo 可让外围命令退出 0，结论以原始 Cargo 输出与保存的 EXIT 标记为准；原始命令记录见 `ROOT-WORKER-COMMANDS.json`。

最终 tools 日志为 1432 字节、SHA256 `472d62c1238ecc27a62bddfb1057efbcffdd40402ec7a0fa4b19021f769612d8`，晚于最终 tools 源码。computer-use 全包源码未改，`native_host.rs` / `target.rs` 指纹与 CU-03 受审版本相同；复用 `/tmp/cu03-20261009/04-computer-use-lib.log` 的 99 passed / exit 0（包括 resize / 观测租约、平台门序与 JPEG 管线锚点），不登记为本轮重跑。App / Engine 未重跑。执行者三文件快照 `/tmp/cu04-20261009/writeset.diff` 为 16762 字节，SHA256 `25cf3e16b854e8a0f29cd60b5f9ed2b8e35ad96361abd458daf4d68acb2036c6`。

旧像素记录的本轮只读复核：`capture-doc.jpeg` / `capture-display2.jpeg` 均为 1280×835、39674 字节、SHA256 `138c8799f465e96c85731dbdf6f355c55e2f1a08981137b6a2c8b20331a08f1f`；最小化图为 39803 字节、SHA256 `aa7893d5e8a1a16cd5a1c379eb91018f4fc1d54bd4ec3cd00ef273649a60294d`，同尺寸。边缘亮度复算与当日 JSON 一致；doc / 副屏文件字节相同，不能据此声称新的显示器切换或内容变化验证。主指挥只读查看 doc 图，确含当日三行验收标记。

环境边界：本轮 RGB/JPEG 来自平台替身，未执行 ScreenCaptureKit、真实 AX/TCC、Provider、用户既有窗口或连续前台输入。真实 Screen Recording 拒绝分支仍待验；现有代码 preflight 顺序和早期授权门序回归不是该真实权限分支的实测成功。历史像素、前台/焦点与显示器记录保持其日期、环境和探针范围。

```text
Validated: cargo test -p pawork-tools --offline --lib computer_native::tests:: -- --test-threads=8（修后 8 passed / 85 filtered，exit 0）；定向 rustfmt / diff / 相对链接检查
Targeted regressions: 所选窗口可解码 JPEG 路由、移动后同窗新观测、缩放后零派发、平台权限错误、图像边长预算、关闭/撤销/进程退出拒绝；同版 library 99 项复用，App / Engine 未重跑
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

唯一独立 `gpt-6.1-sol max` reviewer 对话 `01a1209a-7210-7831-9dcb-1f66a9750103`，turn `01a1209a-752b-7bf2-bff3-bebf0c36c750`（completed / error=null）给出 PASS。完整六文件受审快照 `/tmp/cu04-20261009/ROOT-REVIEW-R1.diff` 为 31285 字节，SHA256 `a9be647b6c39ed549ff6dca065b50f54ee8d01d3e18a7f8c6655692739d4c05c`；结论存 `REVIEW-R1-PASS.txt/json`。提交前重核完整差异与文件指纹未变，只回写完成状态和审查元数据；真实权限等环境边界保持待验。
