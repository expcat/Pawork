# R7 Wave C — 响应式、长内容与平台偏好

> 状态：🟢 2026-08-30 收口；主动系统偏好 U3 依用户指令跳过且不记为通过，VoiceOver 未执行

## 目标与边界

- 目标：在真实 Host + Desktop 窗口中验证 1080×720、应用内字号放大、CJK/emoji/超长内容、千级 Timeline、反复 resize、断线重连与单次性能基线，只修复证据能证明的缺口。
- 非目标：不改 GUI wire、Host、Policy、正式 `fixtures/ui/seed.json` 业务数据或 1440×1024 视觉 reference；不以一次机器采样冻结性能阈值；不把默认平台偏好快照或代码级 palette 测试冒充主动系统态通过。
- 完成口径：driver、AX/几何断言、真实窗口截图与 manifest 可重放；用户明确跳过的主动 Reduce Motion / Increase Contrast 测试登记为 ⏭️，不阻塞 R7 退出，但保留为未验证边界。

## 已实现

1. [`ui-r7-wave-c-resilience.sh`](../../../scripts/ui-r7-wave-c-resilience.sh) 串行覆盖 1440×1024、1080×720、ActivityPopover 边界、三轮宽窄 resize、Composer 焦点保持、断线与重连。隔离临时数据库从正式 seed 的 64 条可投影行派生 960 条消息，得到 1024 条 Timeline；正式 fixture 不变。
2. 连接状态使用定宽槽 + `overflow_hidden` / `truncate`，并以共享 8px 间隔隔开全局「+」；窄窗断连长文案的截图级 paint 门禁为 `lit=0`，AX 仍保留完整值。
3. 全部字体 token 改为以 16px 为 100% 基准的 `Rems`；新增 `Cmd+=` / `Cmd++` 放大、`Cmd+-` 缩小、`Cmd+0` 重置，档位为 100% / 125% / 150%。状态栏与 AX 树发布当前百分比；消息正文 / 完成摘要行高以 24px 基准换算 rem，放大时同步缩放，避免多行正文负 leading。
4. 150% + 1080×720 时 TaskRail 从默认窄窗 240px 扩为 320px，Workspace 保留 760px；任务标题保持单行截断，日期固定在右侧并保留 8px 间隔。100% 的 240/288px 几何和 1440 reference 均不变。
5. macOS 读取 `NSWorkspace.accessibilityDisplayShouldIncreaseContrast`，监听 Accessibility Display Options 变更并刷新窗口；高对比 palette 只增强辅助文字、交互 surface、边界与选区，不改布局或语义色。当前生产 UI 没有动画/过渡，因此 Reduce Motion 不需要渲染分支。
6. 真滚轮、千行 `timeline_stable` barrier、重连相位、截图 paint assert 与单次性能采样均留在同一 driver；正式业务数据和协议未改。

## 验证与真实证据

- 默认字号最终基线：[`u2-reviewfix-pass-20260830/run-manifest.json`](u2-reviewfix-pass-20260830/run-manifest.json) 15 个相位全部 `structural_pass=true`；1080 Connected / ActivityPopover / Disconnected、三轮 resize、1024 行、焦点、重连与连接长文案 paint 门禁全绿。
- 字号缩放完整耐久：[`u2-text-zoom-final-20260830/run-manifest.json`](u2-text-zoom-final-20260830/run-manifest.json) 17 个相位全部 `structural_pass=true`，包含三轮宽窄 resize、150% 与重置；[`150% 截图`](u2-text-zoom-final-20260830/text-zoom-150-1080x720.png) 暴露任务标题与日期间距不足，未被当作视觉终态。
- 受影响区域最终复验：修复 8px 标题/日期间隔后，以一轮 resize 复跑 [`u2-text-zoom-visual-fix-20260830/run-manifest.json`](u2-text-zoom-visual-fix-20260830/run-manifest.json)，13 个相位全部 `structural_pass=true`。人工按原始分辨率检查 [`150% / 1080×720`](u2-text-zoom-visual-fix-20260830/text-zoom-150-1080x720.png)：320px rail、连接状态、标题/日期、760px Workspace、Header、Composer 与 StatusBar 均无可见遮挡或溢出。
- 提交前 review 复验：发现 150% 正文 27px 仍固定 24px 行高会压缩多行文本；改为 rem 行高后，以一轮 resize 复跑 [`u2-lineheight-reviewfix-20260830/run-manifest.json`](u2-lineheight-reviewfix-20260830/run-manifest.json)，13 个相位全部 `pass=true`。[`150% 截图`](u2-lineheight-reviewfix-20260830/text-zoom-150-1080x720.png) 中可见 Timeline 条目行距随之增大，OCR 可读且无行叠；此前 `u2-text-zoom-visual-fix-20260830` 保留为修复前证据。
- 最终机器样本：[`performance-baseline.json`](u2-lineheight-reviewfix-20260830/performance-baseline.json) 为 `baseline_only`，Desktop ready 7277ms、1024 行加载 1810ms、离底/回底 1026/185ms、输入 273ms、窄窗 resize 262–379ms、宽窗 283ms、截图 95–159ms；阈值仍为 `null`。
- 最终只读系统快照：[`platform-preferences.json`](u2-lineheight-reviewfix-20260830/platform-preferences.json) 记录 Reduce Motion、Increase Contrast、Reduce Transparency、Differentiate Without Color 均为 `false`。
- 自动门禁：Desktop **146/146**；Wave C Python **4/4**；shell、Python 与 Swift 定向语法/类型检查通过。

## 系统设置与未执行项

- 用户先授权临时开启 Reduce Motion / Increase Contrast，随后明确改为“跳过需要修改系统设置的测试，恢复系统设置”。最新指令覆盖此前授权。
- 实际只曾短暂开启 Increase Contrast，macOS 同时自动开启 Reduce Transparency；Reduce Motion 从未改变。收到最新指令后立即恢复，`2026-08-30T13:50:31Z` 只读复核四项均为 `false`，最终 U2 在 `2026-08-30T14:04:58Z` 再次记录，收口前 `2026-08-30T14:33:45Z` 第三次只读复核，三次结果均为 `false`。
- 因此主动 Increase Contrast / Reduce Motion 真系统态 U3 为 **⏭️ 用户跳过**，不写入通过统计；高对比响应仅有生产实现、编译和 palette 单元测试证据。
- VoiceOver 仍未执行，屏幕朗读措辞 / 顺序未验证；一次性能样本不能证明无回退或冻结阈值。

## 审查与计划偏差

- 首轮连接状态只做 truncate 仍被外层裁剪；改为定宽槽后截图级负证转绿。千行断言也从 CLI 回显加固为真实 `timeline_stable` barrier，坏 barrier 失败路径有定向测试。
- 字号缩放完整 U2 的结构断言通过，但人工视觉检查发现标题/日期贴近；只修改 Task 行 8px 间隔，并按高返工纪律仅复跑受影响的一轮全链路。
- 提交前 review 发现消息行高仍是固定 px；只把两个调用点改为 24px 基准 rem，100% 视觉不变，并按受影响区域复跑单轮 resize U2。
- 锁屏期间 AX 递归空树样本和第一次 truncate 失败样本仍按旧记录保留；它们不构成通过。两份后续被替代的本地运行目录已从仓库移到 `/tmp/pawork-r7c-rejected.k4hNjP`，可恢复且不纳入证据集。

Implemented: 应用内 100%/125%/150% 字号缩放、150% 窄窗 320px rail、标题/日期间隔、高对比 palette 与 macOS 显示选项刷新；未新增依赖，未改 wire / Host / Policy / 正式 fixture。

Validated: `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`（146/146）；`/tmp/pawork-wave-d-venv/bin/python scripts/test_ui_r7_wave_c_resilience.py`（4/4）；`python3 -m py_compile scripts/ui-wave-d-tools.py scripts/test_ui_r7_wave-c-tools.py scripts/test_ui_r7_wave_c_resilience.py`；`bash -n scripts/ui-r7-wave-c-resilience.sh`；分别执行 `swiftc -typecheck scripts/ui-key-event.swift` 与 `swiftc -typecheck scripts/ui-platform-prefs.swift`；完整三轮字号 U2 17 相位、间隔修复 13 相位与提交前行高修复 13 相位均全绿。

Targeted regressions: 100%↔125%↔150% 快捷键与 AX 状态、150% / 1080×720 的 rail/workspace/composer 几何、Task 标题/日期间隔、消息行高 rem 缩放、三轮 resize 与焦点、1024 行虚拟化/CJK-emoji 哨兵、连接长文案 paint 门禁、重连与 Increase Contrast palette 分支。

Real-world evidence: macOS 26.6.2 `zh_CN` 的真实 Host + Desktop 窗口；最终 150% 截图已人工检查。主动系统偏好 U3 依用户指令跳过，VoiceOver 未执行。

Full workspace gate: NOT RUN（当前未设置全量门禁）
