# CU-04 · 窗口级截图与观测坐标

> 状态：等待人工验收（P2）。前置：[CU-03](cu-03-app-window-discovery.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

提供目标窗口截图与可供动作使用的新鲜观测。

## 当前依据与读取范围

现有截图是最长边 1280、最多 512 KiB 的桌面 JPEG，已有坐标缩放、输入观测过期和 raw frame 上限。

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

## 完成记录

2026-10-06 实现；2026-10-07 按 6.1 sol 复审意见完成 5 项必须修并重取全部验收证据；等待人工验收。

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

**遗留缺口**：Screen Recording 拒绝分支未实测——本机授权沿责任进程链继承、无法为探针单独撤销；该分支由代码门序（preflight 在捕获缝之前）与门序回归的调用计数结构性钉住。生产 tools / Host 接线与 GUI 观测查询属 CU-10 / CU-17，本任务未接入生产路径。
