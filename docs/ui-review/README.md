# UI 视觉证据目录（R1 视觉合同与 99% 门禁）

> 本目录承载 [R1 任务书](../../plan/R1-ui-visual-contract.md) 与 [UI 视觉复审合同](../UI_Review.md) 的可复现证据：归一参考图、量图表、组件 manifest、遮罩、zones、checklist，以及后续波次的 current/overlay/diff。判定规则以 [docs/UI_Review.md §0.1](../UI_Review.md) 为准，本文件只记录管线与复现步骤。

## 1. 目录布局

每个 State 一个目录，Wave A 产出 reference/measurements/mask/checklist/crops，后续波次补 current/overlay/diff 运行产物：

| 文件 | 产生波次 | 说明 |
| --- | --- | --- |
| state-{a,b,c}/reference.png | R1-A | 归一到 1440×1024 的定稿参考图 |
| state-{a,b,c}/measurements.md | R1-A | 量图表（几何/字阶/间距/颜色/冲突表） |
| state-{a,b,c}/crops/ | R1-A | 量图证据切片 |
| state-{a,b,c}/mask.json | R1-A | 动态值 / reference 伪影遮罩（纪律见 §3） |
| state-{a,b,c}/zones.json | R1-A 收口 | 分区 SSIM 区域定义，坐标取自 measurements.md |
| state-{a,b,c}/checklist.md | R1-A 起持续更新 | 结构/状态/交互/AX 人工复核清单 |
| state-{a,b,c}/current.png | R1-D 起 | 当前实现同视口截图 |
| state-{a,b,c}/overlay-50.png、diff-heatmap.png、diff-report.json | R1-D 起 | 原坐标 overlay、遮罩后 heatmap 与机器报告 |
| state-{a,b,c}/zone-evidence/ | R1-D 起 | 各 zone 按 anchor 对齐后的 overlay / heatmap |
| component-manifest.md | R1-A | 三状态组件树 / z-order / 状态 / 能力映射 |
| normalization-report.json | R1-A | 归一化机器可读记录 |

State 定义：A = Timeline + Inspector 展开；B = Timeline + Inspector 折叠 + ActivityPopover；C = Projects。同一 State 的 reference/current/overlay/diff/mask/checklist 成套保存，不跨状态比较。

## 2. 归一化记录（reference.png 怎么来）

脚本：[scripts/ui-normalize-reference.py](../../scripts/ui-normalize-reference.py)。对每张定稿图做**全画布缩放**（LANCZOS）到 1440×1024，无裁切、无色彩 profile 转换。三张源 PNG 都是无 ICC profile 的 RGB，管线明确按 sRGB 解释；若源 mode/profile 改变，脚本 fail-closed，要求先更新合同。

| State | 源图 | 源尺寸 | scale_x | scale_y |
| --- | --- | --- | --- | --- |
| A | design/desktop-shell-timeline-v3.png | 1486×1059 | 0.969044 | 0.966950 |
| B | design/desktop-shell-timeline-collapsed-v3.png | 1487×1058 | 0.968393 | 0.967864 |
| C | design/desktop-shell-projects-v3.png | 1487×1058 | 0.968393 | 0.967864 |

- 各向异性 <0.22%，源于 ImageGen 的 1px 边差，由缩放吸收；不引入裁切对齐误差。
- **macOS titlebar 处理**：定稿图顶部 traffic lights 条带属于内容视口（目标为深色沉浸式 titlebar，见 UI_Review F-01），reference 保留该条带（三图实测条带高 32–48px 不一，属 ImageGen 边差，量图逐图记录）；current 截图也必须是 1440×1024 内容区（含同一沉浸式条带），不得把白色原生 titlebar 混入。
- 色彩：untagged RGB 按 sRGB 解释；比较时不做 profile 转换，避免双重重采样。

## 3. 判定顺序（结构一票否决 → 分区 SSIM → 人工 overlay）

1. **结构一票否决**：按 checklist.md 逐项核对区域、组件、顺序、展开/折叠与选中状态 100% 对齐；任一硬失败项（UI_Review §0.1 列表）存在即整轮不通过，SSIM 再高也不放行。
2. **分区 color SSIM**：scripts/ui-visual-diff.py 按 zones.json 对 TaskRail、Header、Timeline、Composer、Inspector/Popover、StatusBar 分区计算；每个 RGB channel 都必须 >= 0.99（zone 取三通道最低值），避免等亮度换色漏检。0.99 是代码固化的硬下限，配置只能提高；全屏均值只作辅助信息。
3. **人工 overlay 复核**：overlay-50.png 保留原坐标，检查合同几何；zone-evidence/ 中按 anchor 对齐后的 overlay 检查文字基线与组件槽位；diff-heatmap.png 同时排除 reference mask 与其对齐后的 current 位置，zone heatmap 排除对齐裁剪内的 mask，只应剩抗锯齿与真实漂移。

差分输入必须都是已经显式归一的无 ICC `RGB` PNG，并按 sRGB 字节解释；脚本对灰度、RGBA 或带 ICC profile 的 reference/current 直接报输入错误，禁止静默丢 alpha 或猜测 Display-P3→sRGB。Wave D 的真窗口截图若带 profile，须先在截图取证步骤中显式转为 sRGB 并记录转换，再进入本门禁。

**mask 纪律**：mask.json 只允许三类遮罩，逐条带 reason；`covers` 必须由脚本枚举的动态字段 token（可用 `|` 组合）或单一特殊类型构成，未知/重复 token fail-closed：

1. 动态值（covers=time/tokens/quota/filename/...）：只遮值本身，保留静态标签、容器、文字基线、字阶、行数密度与状态图标。多行正文与 Diff 必须逐行紧遮；Diff 还须保留 gutter、+/− 前缀、行距与语义底色，禁止一个大矩形吞掉整块内容。
2. reference-artifact：ImageGen 窗缘亮线等参考图伪影（真实 GPUI 截图不存在的内容），代码强制限于图像四边 2px，且不得与其它 `covers` 组合。
3. geometry-drift（2026-08-26 用户拍板的**可用类型**）：仅当 zone anchor 仍无法表达 ImageGen 边缘伪影时使用，矩形必须收敛到无文字、图标、状态点或控件的纯边缘背景。不得用纵贯 rail/Inspector 的宽遮罩隐藏右对齐时间、按钮或边界；Wave A 当前三态不需要此类 mask。

遮罩像素先在两图写入相同中性值，再排除出 SSIM 平均，避免 11×11 统计窗口把动态文字差异泄漏到 mask 外。35% 是代码固化的 fail-closed 硬上限：`zones.json` 的 `max_mask_fraction` 及单 zone 覆盖值只能更严格，不能放宽；任一对齐 zone、全图辅助区或 reference+current 映射后的 heatmap mask 并集超过有效上限即输入错误（exit 2），不能把大遮罩移到 zones 外或用重叠 zone 反复映射制造通过。当前三态统一声明 0.35，分区最高约 31%，全图分别约 20.8%/8.9%/14.7%。

**zone anchor**：zones.json 每个 zone 带 anchor、min_coverage 与可选 current 矩形（R1-D 从真窗口实测后回填），全局另带 `max_mask_fraction`。不同尺寸从 anchor 角取公共裁剪；公共面积低于任一矩形声明的 min_coverage 即输入错误（exit 2），且 `min_coverage` 不得低于代码固化的 0.50，防止用极小重叠区制造通过。Header、Composer、Inspector 与 Popover 均拆成 left/right 或 body/right zone，确保一侧裁切不会吞掉另一侧控件；TaskRail/Timeline 左锚，StatusBar 右锚。

## 4. 复现步骤

    # PY 指向带 Pillow + numpy 的 Python 3.8+；本轮验证环境使用 Codex bundled Python
    PY=/path/to/python3
    # 1) 重新生成三张归一参考图 + normalization-report.json
    $PY scripts/ui-normalize-reference.py
    # 2) 有了 current.png 之后跑分区差分（R1-D 起）
    $PY scripts/ui-visual-diff.py --reference docs/ui-review/state-a/reference.png --current docs/ui-review/state-a/current.png --zones docs/ui-review/state-a/zones.json --masks docs/ui-review/state-a/mask.json --out docs/ui-review/state-a
    # 3) 修改 diff 管线后跑定向回归
    $PY -m unittest scripts/test_ui_visual_diff.py
    $PY -m unittest scripts/test_ui_wave_d_tools.py
    # State A 真窗口闭环（需解锁屏幕；视觉 zone FAIL 在 R1 预期）
    scripts/ui-wave-d-state-a.sh run --out /tmp/pawork-wave-d-out --label baseline

## 5. 工具链事实

- 量化工具只依赖 Python 3.8+、Pillow 与 numpy；不依赖 ImageMagick / scikit-image。脚本不写死某台机器的 Python 路径。
- reference 归一化对字节未变化的产物不重写；需要更新时先写同目录临时文件再原子替换，避免并发取证读到半写入 PNG。
- SSIM 实现为 11×11 box-window（积分图），逐 RGB channel 计算并以最低通道作为 zone 分数，非 gaussian 变体；mask 在统计前中性化防边缘泄漏。自比必须为 1.0；跨 State 与等亮度换色样本必须 FAIL。换工具链时重跑定向回归。
- 每次调用先清除脚本拥有的旧 report/overlay/heatmap/zone PNG；任一输入错误即再次清理本次部分产物并返回 exit 2，禁止旧 PASS 或无 report 的半套图像残留。清理范围只含固定根文件名与 `zone-evidence/*-{overlay-50,diff-heatmap}.png`。
- 量图数值全部以 1440×1024 reference 坐标记录；原图坐标换算乘以对应 scale。
